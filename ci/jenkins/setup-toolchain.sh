#!/usr/bin/env bash
# Install or verify the Rust toolchain for a Jenkins build.
# Reads the required channel from rust-toolchain.toml.
# CARGO_HOME, RUSTUP_HOME, and PATH are set by initEnv() (pipeline-helpers.groovy).
#
# Tiers, in order of preference (first that succeeds wins):
#   1. Persistent-cache HIT       — rustup already has the channel (fast path).
#   2. Committed air-gap bundle    — the toolchain/ split archives that ship in the
#                                    repo for a plain `git clone` offline build. This
#                                    mirrors scripts/internal/install.sh so a fresh
#                                    air-gapped Windows agent NEVER falls through to
#                                    an internet rustup (which would hang offline).
#   3. Agent-image seed            — /opt/rust-toolchain (Dockerfile.agent layout).
#   4. Bundled rustup-init          — semi-offline pre-seeded installer.
#   5. Internet rustup             — FAIL FAST on an air-gapped agent unless the
#                                    operator opts in with SLOC_ALLOW_ONLINE_RUSTUP=1.
set -euo pipefail

TOOLCHAIN=$(grep '^channel' rust-toolchain.toml | cut -d'"' -f2)

# ── Platform detection (Git Bash reports MINGW*/MSYS* via uname -s) ────────────
detect_platform() {
    local u=""
    u="$(uname -s 2>/dev/null || echo "")"
    case "${u}" in
        MINGW*|MSYS*|CYGWIN*) echo windows; return ;;
    esac
    case "${OS:-}" in
        Windows_NT) echo windows; return ;;
    esac
    case "${OSTYPE:-}" in
        msys*|cygwin*|win32) echo windows; return ;;
    esac
    echo linux
}

# ── Air-gap bundle extraction (mirrors scripts/internal/install.sh ~590-717) ──
# Selects the committed toolchain archive for this platform, collects split parts,
# verifies each against toolchain/checksums.sha256, extracts (tolerating Windows
# symlink-ENOENT noise), patches the cargo/bin proxies from rustup.exe, then points
# RUSTUP_HOME/CARGO_HOME/PATH at the extracted tree so the tier-1 cache-hit check
# succeeds on the next build. Returns 0 on success, 1 if no bundle is present.
extract_airgap_toolchain() {
    local platform arch archive parts=() have=false
    platform="$(detect_platform)"

    if [ "${platform}" = "windows" ]; then
        archive="toolchain/rust-toolchain-windows-x64.tar.gz"
    else
        arch="$(uname -m 2>/dev/null || echo x86_64)"
        case "${arch}" in
            aarch64|arm64) arch="arm64" ;;
            *)             arch="x86_64" ;;
        esac
        archive="toolchain/rust-toolchain-linux-${arch}.tar.gz"
    fi

    # Collect split parts (*.tar.gz.aa, .ab, …); fall back to a single archive.
    local _tp
    for _tp in "${archive}".*; do
        [ -f "${_tp}" ] && parts+=("${_tp}")
    done
    [ "${#parts[@]}" -gt 0 ] && have=true
    [ -f "${archive}" ]      && have=true
    if [ "${have}" != true ]; then
        return 1
    fi

    echo "Extracting committed Rust toolchain bundle (air-gapped): ${archive}"

    # Verify checksums of each present part (or the single archive) by basename.
    local checksums="toolchain/checksums.sha256"
    if [ -f "${checksums}" ] && command -v sha256sum >/dev/null 2>&1; then
        local verify=()
        if [ "${#parts[@]}" -gt 0 ]; then
            verify=("${parts[@]}")
        else
            verify=("${archive}")
        fi
        local _vf _name _expected _actual
        for _vf in "${verify[@]}"; do
            _name="$(basename "${_vf}")"
            _expected="$(grep "${_name}" "${checksums}" 2>/dev/null | awk '{print $1}')"
            if [ -n "${_expected}" ]; then
                _actual="$(sha256sum "${_vf}" | awk '{print $1}')"
                if [ "${_expected}" != "${_actual}" ]; then
                    echo "ERROR: toolchain checksum mismatch for ${_name} — bundle may be corrupt." >&2
                    echo "       Expected: ${_expected}" >&2
                    echo "       Actual:   ${_actual}" >&2
                    exit 1
                fi
            fi
        done
        echo "Toolchain bundle checksum verified."
    fi

    # Extract into the parent of CARGO_HOME (…/.rust-cache), so the layout becomes
    #   <cacheroot>/rustup/toolchains/<ver>-<target>/bin
    #   <cacheroot>/cargo/bin
    # matching what tier 1 (rustup toolchain list) and install-rust-cache.sh expect.
    local cache_root
    cache_root="$(dirname "${CARGO_HOME}")"
    mkdir -p "${cache_root}"

    if [ "${platform}" = "windows" ]; then
        # Git Bash tar records rustup's proxy hardlinks (cargo.exe, rustc.exe, …) as
        # POSIX symlinks to rustup.exe. Symlink creation fails with ENOENT when the
        # target hasn't been extracted yet (cargo.exe sorts before rustup.exe).
        # Extract while tolerating those specific errors, then copy rustup.exe over
        # any proxy tar couldn't create — same handling as install.sh.
        local _tar_stderr
        _tar_stderr="$(mktemp)"
        if [ "${#parts[@]}" -gt 0 ]; then
            cat "${parts[@]}" | tar -xzf - -C "${cache_root}" 2>"${_tar_stderr}" || true
        else
            tar -xzf "${archive}" -C "${cache_root}" 2>"${_tar_stderr}" || true
        fi
        # Surface any real (non-symlink) errors; silently discard symlink noise.
        grep -v "Cannot create symlink" "${_tar_stderr}" >&2 || true
        rm -f "${_tar_stderr}"
        local _rustup_proxy="${CARGO_HOME}/bin/rustup.exe"
        if [ -f "${_rustup_proxy}" ] && [ ! -r "${_rustup_proxy}" ]; then
            # The proxy was extracted but we cannot read it back. On corporate
            # Windows nodes this almost always means AV/EDR quarantined or
            # exclusive-locked the freshly-written rustup.exe. Skip the copy so we
            # do not spew a raw "cp: permission denied"; the cargo-not-found block
            # below turns this into the real remediation guidance.
            echo "WARNING: rustup.exe was extracted but is present-yet-unreadable:" >&2
            echo "           ${_rustup_proxy}" >&2
            echo "         A corporate AV/EDR agent has most likely quarantined or" >&2
            echo "         exclusive-locked the freshly-extracted file. Remediation:" >&2
            echo "           - Use the fast prebuilt path (BUILD_MODE=prebuilt) so the" >&2
            echo "             toolchain is never extracted or run, OR" >&2
            echo "           - Ask the platform team to add an AV/EDR real-time-scan" >&2
            echo "             exclusion for the Rust cache directory:" >&2
            echo "               $(dirname "${CARGO_HOME}")" >&2
            echo "             (relocatable via the SLOC_CACHE_DIR env var)." >&2
        elif [ -f "${_rustup_proxy}" ]; then
            local _proxy
            for _proxy in cargo.exe rustc.exe rustdoc.exe; do
                [ -f "${CARGO_HOME}/bin/${_proxy}" ] || cp "${_rustup_proxy}" "${CARGO_HOME}/bin/${_proxy}"
            done
            echo "Windows toolchain proxy binaries verified."
        fi
    elif [ "${#parts[@]}" -gt 0 ]; then
        cat "${parts[@]}" | tar -xzf - -C "${cache_root}"
    else
        tar -xzf "${archive}" -C "${cache_root}"
    fi

    # Point the environment at the extracted tree so the next build's tier-1
    # cache-hit check (rustup toolchain list) succeeds, and cargo is on PATH now.
    local tc_bin
    tc_bin="$(find "${RUSTUP_HOME}/toolchains" -maxdepth 2 -name bin -type d 2>/dev/null | head -1)"
    export PATH="${CARGO_HOME}/bin:${tc_bin:-${RUSTUP_HOME}/bin}:${PATH}"

    if ! command -v cargo >/dev/null 2>&1; then
        echo "ERROR: cargo not found after extracting the committed toolchain bundle." >&2
        if [ -f "${CARGO_HOME}/bin/rustup.exe" ] && [ ! -r "${CARGO_HOME}/bin/rustup.exe" ]; then
            # rustup.exe is on disk but unreadable: the proxy copy above was skipped,
            # so cargo.exe was never created. This is an AV/EDR block, not a bad bundle.
            echo "       rustup.exe is present but unreadable, so cargo.exe could not be" >&2
            echo "       created from it. A corporate AV/EDR agent has most likely" >&2
            echo "       quarantined or exclusive-locked the extracted toolchain. Fix by:" >&2
            echo "         - Using the fast prebuilt path (BUILD_MODE=prebuilt), which" >&2
            echo "           never extracts or runs the toolchain, OR" >&2
            echo "         - Having the platform team add an AV/EDR real-time-scan" >&2
            echo "           exclusion for the Rust cache directory:" >&2
            echo "             $(dirname "${CARGO_HOME}")" >&2
            echo "           (relocatable via the SLOC_CACHE_DIR env var)." >&2
            echo "       (If the file is genuinely readable, the bundle under toolchain/" >&2
            echo "       may instead be incomplete; regenerate it with" >&2
            echo "       bash scripts/internal/bundle-rust-toolchain.sh)" >&2
        else
            echo "       The bundle under toolchain/ may be incomplete; regenerate it with" >&2
            echo "       bash scripts/internal/bundle-rust-toolchain.sh" >&2
        fi
        exit 1
    fi
    echo "Rust toolchain bootstrapped from the committed air-gap bundle."
    return 0
}

# ── Pin RUSTUP_TOOLCHAIN to the exact installed toolchain ─────────────────────
# rust-toolchain.toml pins the "1.97" channel, but the committed air-gap bundle
# (and every cache seeded from it) installs the CONCRETE name 1.97.0-<triple>.
# Offline, rustup cannot map "1.97" → "1.97.0" without fetching the channel
# manifest from static.rust-lang.org, so `rustup show` and every later cargo call
# abort with "could not download file …/channel-rust-1.97.toml (connection reset
# by peer)". Pinning RUSTUP_TOOLCHAIN to the exact installed name makes rustup use
# that toolchain directly and never touch the network. Also record the name so the
# Jenkins pipeline can export it for the downstream stages (each shx() is a fresh
# shell, so this export does not carry on its own). Mirrors the .gitlab-ci.yml pin.
pin_rustup_toolchain() {
    local name _rest
    while read -r name _rest; do
        case "${name}" in
            "${TOOLCHAIN}".*|"${TOOLCHAIN}"-*|"${TOOLCHAIN}")
                export RUSTUP_TOOLCHAIN="${name}"
                echo "Pinned RUSTUP_TOOLCHAIN=${name} (air-gap: skips channel sync)."
                printf '%s\n' "${name}" > .rust-toolchain-name 2>/dev/null || true
                return 0
                ;;
        esac
    done < <(rustup toolchain list 2>/dev/null)
    return 1
}

if rustup toolchain list 2>/dev/null | grep -q "${TOOLCHAIN}"; then
    echo "Rust ${TOOLCHAIN} already in persistent cache — skipping install."
elif extract_airgap_toolchain; then
    : # handled inside extract_airgap_toolchain (env exported, cargo on PATH)
elif [ -d /opt/rust-toolchain/rustup/toolchains ]; then
    echo "Seeding toolchain from agent image (/opt/rust-toolchain)..."
    cp -a /opt/rust-toolchain/cargo/. "${CARGO_HOME}/"
    cp -a /opt/rust-toolchain/rustup/. "${RUSTUP_HOME}/"
elif [ -x "${RUSTUP_HOME}/../rustup-init" ]; then
    echo "Using bundled rustup-init (semi-offline)..."
    "${RUSTUP_HOME}/../rustup-init" -y \
        --default-toolchain "${TOOLCHAIN}" \
        --no-modify-path
elif [ "${SLOC_ALLOW_ONLINE_RUSTUP:-0}" = "1" ]; then
    echo "============================================================"
    echo "WARNING: This build is NOT air-gapped."
    echo "  No committed toolchain bundle (toolchain/), no persistent"
    echo "  cache hit, no agent-image seed, and no bundled rustup-init"
    echo "  were found — but SLOC_ALLOW_ONLINE_RUSTUP=1 was set, so"
    echo "  falling back to internet rustup. To make this air-gap-safe,"
    echo "  commit the toolchain bundle (bundle-rust-toolchain.sh) or"
    echo "  rebuild the agent image: see docs/ci-integrations.md."
    echo "============================================================"
    echo "Downloading rustup installer (requires internet access)..."
    # Download rustup-init to a file before executing — avoids the curl|sh pattern
    # flagged by OpenSSF Scorecard Pinned-Dependencies.
    _RUSTUP_INIT="$(mktemp)"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o "${_RUSTUP_INIT}"
    sh "${_RUSTUP_INIT}" -y --default-toolchain "${TOOLCHAIN}" --no-modify-path
    rm -f "${_RUSTUP_INIT}"
else
    # FAIL FAST on an air-gapped agent rather than hanging on an unreachable
    # https://sh.rustup.rs. This is the tier that used to silently curl the net.
    echo "============================================================" >&2
    echo "ERROR: air-gapped — no Rust toolchain available." >&2
    echo "  * No committed Windows/Linux toolchain bundle was found under toolchain/." >&2
    echo "  * No cached toolchain (rustup channel ${TOOLCHAIN}) is present." >&2
    echo "  * No agent-image seed (/opt/rust-toolchain) and no bundled rustup-init." >&2
    echo "" >&2
    echo "  Fix ONE of the following:" >&2
    echo "    - Install Git for Windows and ensure the toolchain/ directory is present" >&2
    echo "      in the checkout (it ships in the repo for offline builds)." >&2
    echo "    - Pin this job to a pre-seeded Linux agent via the AGENT_LABEL parameter." >&2
    echo "    - Seed the persistent Rust cache once with install-rust-cache.sh." >&2
    echo "    - As a last resort on a networked agent, set SLOC_ALLOW_ONLINE_RUSTUP=1" >&2
    echo "      to permit an internet rustup download." >&2
    echo "============================================================" >&2
    exit 1
fi

# Pin to the exact installed toolchain BEFORE `rustup show`, or that very command
# would try to resolve the "1.97" channel over the network on an air-gapped agent.
pin_rustup_toolchain || echo "No installed toolchain matched channel ${TOOLCHAIN} — leaving rustup to resolve it."

rustup show
cargo --version
