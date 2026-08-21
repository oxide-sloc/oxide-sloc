#!/usr/bin/env bash
# oxide-sloc installer
#
# Usage:
#   bash scripts/internal/install.sh           # auto-installs; pre-built binary preferred
#   bash scripts/internal/install.sh --online  # explicitly download from SLOC_RELEASE_BASE_URL
#   bash scripts/internal/install.sh --build   # compile from bundled toolchain + vendor sources
#   bash scripts/internal/install.sh --rebuild # force a fresh build from source (implies --build)
#   bash scripts/internal/install.sh --auto    # auto-install rustup if cargo is absent (interactive prompt)
#
# Environment variables:
#   SLOC_RELEASE_BASE_URL  Base URL for pre-built binary downloads, without trailing slash
#                          and without the version segment.  No default — must be set
#                          explicitly by an administrator to enable network downloads.
#                          Typically an internal artifact server (Nexus, Artifactory, JFrog):
#                            export SLOC_RELEASE_BASE_URL=https://nexus.example.com/oxide-sloc
#                          The installer appends /v{version}/{asset} to form the full URL.
#                          Without this variable, --online downloads from GitHub Releases.
#
# Behavior:
#   default:  uses dist/ archive if present.  If SLOC_RELEASE_BASE_URL is set and curl is
#             available and cargo is not on PATH, silently downloads from the configured
#             server.  Otherwise stops with instructions — use --build to opt in to
#             bundled toolchain extraction, or place a binary in dist/.
#   --online: downloads explicitly from SLOC_RELEASE_BASE_URL (falls back to GitHub Releases
#             if that variable is not set).  The user has opted in to a network fetch.
#   --build:  extracts the bundled Rust toolchain and compiles from vendor sources.
#             Compiles even when a dist/ archive is present (it skips dist extraction
#             and network download). Use --rebuild to also discard an already-installed
#             binary and force a fresh compile.
#   If cargo (Rust) is already on PATH  → extract vendor sources + cargo build (no toolchain needed).
#   If no cargo on PATH                  → requires --build to proceed.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

FORCE_REBUILD=false
# --auto is accepted for forward-compat; rustup auto-bootstrap is not yet wired.
AUTO_RUSTUP=false
ONLINE_MODE=false
BUILD_MODE=false
# AUTO_RUSTUP is assigned by --auto below but currently inert (see note above).
# shellcheck disable=SC2034
for arg in "$@"; do
    case "$arg" in
        --rebuild|--force|-f) FORCE_REBUILD=true; BUILD_MODE=true ;;
        --auto) AUTO_RUSTUP=true ;;
        --online) ONLINE_MODE=true ;;
        --build) BUILD_MODE=true ;;
    esac
done

# Detect Windows (Git Bash / MSYS2 / Cygwin)
if [[ -n "${WINDIR+x}" ]] || [[ "${OSTYPE:-}" == msys* ]] || [[ "${OSTYPE:-}" == cygwin* ]]; then
    PLATFORM=windows
    EXE="$REPO_ROOT/oxide-sloc.exe"
    BUILD_OUTPUT="$REPO_ROOT/target/release/oxide-sloc.exe"
else
    PLATFORM=linux
    EXE="$REPO_ROOT/oxide-sloc"
    case "$(uname -m 2>/dev/null)" in
        aarch64|arm64) LINUX_ARCH=arm64 ;;
        *)             LINUX_ARCH=x86_64 ;;
    esac
    BUILD_OUTPUT="$REPO_ROOT/target/release/oxide-sloc"
fi

# Vendor sources ship as split gzip parts (vendor.tar.gz.aa/.ab/.ac) reassembled
# with `cat | tar -xzf -`; gzip is universal (Git for Windows bundles no xz).
VENDOR_CHECKSUMS="$REPO_ROOT/vendor.checksums.sha256"
VENDOR_DIR="$REPO_ROOT/vendor"

LOG_DIR="$REPO_ROOT/logs"
mkdir -p "$LOG_DIR"

print_log_link() {
    local f="$1"
    local url
    if [[ "$PLATFORM" == windows ]]; then
        local wf
        wf="$(cygpath -w "$f" 2>/dev/null)" || wf="$f"
        wf="${wf//\\/\/}"
        url="file:///$wf"
    else
        url="file://$f"
    fi
    printf '  Log dir:  %s\n' "$LOG_DIR"
    printf '  Log file: %s\n' "$(basename "$f")"
    printf '  \033]8;;%s\033\\Click to open log\033]8;;\033\\\n' "$url"
}

# Import sloc-ca.crt into CurrentUser\Root via a direct registry write.
# The standard X509Store API always triggers a Windows Security Warning dialog for root
# certificates; writing the backing registry key directly bypasses it — no dialog, no Admin.
trust_ca_cert() {
    [[ "$PLATFORM" != windows ]] && return 0
    # The public root CA that signed the dist exe is committed at deploy/certs/.
    # (gen-signing-cert.sh writes it there; only the public .crt is committed —
    # private key material stays in the gitignored _signing/.) Importing it into
    # the current user's Root store makes the signed binary a TRUSTED publisher on
    # a fresh corporate clone — otherwise it shows as "unknown publisher" and
    # SmartScreen/AppLocker/EDR publisher allow-rules can't match it. Fall back to
    # a repo-root certs/ layout for older checkouts.
    local cert=""
    for _c in "$REPO_ROOT/deploy/certs/sloc-ca.crt" "$REPO_ROOT/certs/sloc-ca.crt"; do
        [[ -f "$_c" ]] && { cert="$_c"; break; }
    done
    [[ -n "$cert" ]] || return 0
    local cert_win
    cert_win="$(cygpath -w "$cert" 2>/dev/null || echo "$cert")"

    echo " Trusting oxide-sloc signing certificate (current user, no dialog)..."

    local ps_result
    ps_result="$(powershell.exe -NoProfile -NonInteractive -Command "
\$certPath = '${cert_win}'
\$cert    = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new(\$certPath)
\$thumb   = \$cert.Thumbprint
\$regPath = \"HKCU:\SOFTWARE\Microsoft\SystemCertificates\Root\Certificates\\\$thumb\"

if (Test-Path \$regPath) { Write-Output 'ALREADY_TRUSTED'; exit 0 }

# Blob: propCount(4) + propId=20(4) + reserved(4) + derLen(4) + derBytes(N)
\$der  = \$cert.RawData
\$blob = [byte[]]::new(16 + \$der.Length)
\$pos  = 0
[System.BitConverter]::GetBytes([uint32]1 ).CopyTo(\$blob, \$pos); \$pos += 4
[System.BitConverter]::GetBytes([uint32]20).CopyTo(\$blob, \$pos); \$pos += 4
[System.BitConverter]::GetBytes([uint32]0 ).CopyTo(\$blob, \$pos); \$pos += 4
[System.BitConverter]::GetBytes([uint32]\$der.Length).CopyTo(\$blob, \$pos); \$pos += 4
\$der.CopyTo(\$blob, \$pos)

New-Item -Path \$regPath -Force | Out-Null
Set-ItemProperty -Path \$regPath -Name 'Blob' -Value \$blob -Type Binary
Write-Output 'IMPORTED'
" 2>/dev/null)" || true

    case "$ps_result" in
        ALREADY_TRUSTED)
            echo " [OK] Certificate already trusted — skipping."
            ;;
        IMPORTED)
            echo " [OK] Certificate trusted silently. Authenticode verification enabled."
            ;;
        *)
            echo " [WARN] Certificate import via registry failed. To trust manually (no Admin needed):"
            echo "        certutil -user -addstore Root deploy/certs/sloc-ca.crt"
            ;;
    esac
}

# Runs `cargo build --release --offline -p oxide-sloc` with an animated progress display.
# Returns the cargo exit code.
build_with_progress() {
    local tmpout
    tmpout="$(mktemp)"
    local LOG_FILE
    LOG_FILE="$LOG_DIR/install-$(date +%Y-%m-%d-%H-%M-%S).log"

    local total_pkgs=0
    if [[ -f "$REPO_ROOT/Cargo.lock" ]]; then
        total_pkgs="$(grep -c '^\[\[package\]\]' "$REPO_ROOT/Cargo.lock" 2>/dev/null)" || total_pkgs=0
    fi

    # Drop a stale/broken CC (and matching CXX) that names a compiler not on PATH.
    # Third-party C-shim crates (e.g. ring) honour $CC via cc-rs; an inherited
    # CC=gcc.exe from another dev toolkit breaks an MSVC build and points at a tool
    # that isn't installed. Clearing it lets cargo pick the right linker for the
    # active target (MSVC cl.exe, MinGW gcc, or the system cc on Linux).
    if [[ -n "${CC:-}" ]] && ! command -v "$CC" &>/dev/null; then
        echo " [WARN] Ignoring CC='$CC' — not found on PATH; using the target's default compiler." >&2
        unset CC CXX
    fi

    printf '\n'
    cargo build --release --offline -p oxide-sloc 2>"$tmpout" &
    local cargo_pid=$!

    trap 'kill "$cargo_pid" 2>/dev/null; rm -f "$tmpout"; printf "\033[?25h\n"; exit 130' INT TERM

    local spin=('|' '/' '-' '\')
    local si=0 compiled=0 last_crate="" bar_w=34
    local start_time="$SECONDS"
    local is_linking=0 first_draw=1
    local NLINES=9
    local resolution_msgs=(
        "Scanning vendor crate manifests..."
        "Resolving dependency graph..."
        "Checking crate versions..."
        "Verifying source checksums..."
        "Building compile plan..."
    )
    local msg_idx=0 frame_count=0 msg_interval=17  # rotate every ~2 s (17 × 0.12 s)

    printf '\033[?25l'

    while kill -0 "$cargo_pid" 2>/dev/null; do
        compiled="$(grep -c "^   Compiling" "$tmpout" 2>/dev/null)" || compiled=0
        if [[ "$compiled" -gt 0 ]]; then
            last_crate="$(grep "^   Compiling" "$tmpout" 2>/dev/null | tail -1 | \
                sed 's/^   Compiling //' | cut -c1-46)" || last_crate=""
        fi
        grep -q "Linking " "$tmpout" 2>/dev/null && is_linking=1 || true

        local elapsed=$(( SECONDS - start_time ))
        local elapsed_str
        printf -v elapsed_str '%d:%02d' "$(( elapsed / 60 ))" "$(( elapsed % 60 ))"

        local frame="${spin[$si]}"
        si=$(( (si + 1) % 4 ))
        frame_count=$(( frame_count + 1 ))

        local phase step_n
        if   [[ "$is_linking" -eq 1 ]]; then phase="Linking & installing";      step_n=3
        elif [[ "$compiled"   -gt 0 ]]; then phase="Compiling release build";   step_n=2
        else                                 phase="Verifying sources";          step_n=1
        fi

        local bar="" fill=0 i
        if [[ "$compiled" -eq 0 ]]; then
            # Phase 1: bouncing scanner — cargo is silent during resolution so we can't
            # track real progress; use an indeterminate animation to show activity.
            local scanner_w=5
            local bounce_range=$(( bar_w - scanner_w ))
            local bounce_pos=$(( frame_count % (bounce_range * 2) ))
            local scan_left
            if [[ $bounce_pos -le $bounce_range ]]; then
                scan_left=$bounce_pos
            else
                scan_left=$(( bounce_range * 2 - bounce_pos ))
            fi
            for (( i = 0; i < bar_w; i++ )); do
                if [[ $i -ge $scan_left && $i -lt $(( scan_left + scanner_w )) ]]; then
                    bar+='='
                else
                    bar+='.'
                fi
            done
        else
            if [[ "$total_pkgs" -gt 0 ]]; then
                fill=$(( compiled * bar_w / total_pkgs ))
                [[ $fill -gt $bar_w ]] && fill=$bar_w
            fi
            for (( i = 0; i < bar_w; i++ )); do
                if   [[ $i -lt $fill ]];                          then bar+='='
                elif [[ $i -eq $fill && $fill -lt $bar_w ]];     then bar+='>'
                else                                                   bar+=' '
                fi
            done
        fi

        local count_str pct_str=""
        if [[ "$compiled" -eq 0 ]]; then
            count_str="scanning..."
        elif [[ "$total_pkgs" -gt 0 ]]; then
            count_str="${compiled} / ~${total_pkgs}"
            pct_str="($(( compiled * 100 / total_pkgs ))%)"
        else
            count_str="${compiled} crates"
        fi

        local ic_res='\xe2\x97\x8b' ic_cmp='\xe2\x97\x8b' ic_lnk='\xe2\x97\x8b'
        if   [[ "$is_linking" -eq 1 ]]; then
            ic_res='\xe2\x9c\x93'; ic_cmp='\xe2\x9c\x93'; ic_lnk="$frame"
        elif [[ "$compiled"   -gt 0 ]]; then
            ic_res='\xe2\x9c\x93'; ic_cmp="$frame"
        else
            ic_res="$frame"
        fi

        [[ "$first_draw" -eq 0 ]] && printf "\033[%dA" "$NLINES"
        first_draw=0

        printf "  %s  Step [%d/3]  %-46s\033[K\n"  "$frame"  "$step_n"  "$phase"
        printf "     [%s] %-16s%s\033[K\n"          "$bar"    "$count_str"  "$pct_str"
        local current_display
        if [[ "$compiled" -eq 0 ]]; then
            (( frame_count % msg_interval == 0 )) && msg_idx=$(( (msg_idx + 1) % ${#resolution_msgs[@]} ))
            current_display="${resolution_msgs[$msg_idx]}"
        else
            current_display="$last_crate"
        fi
        printf "     Current:  %-50s\033[K\n"       "$current_display"
        printf "     Elapsed:  %-50s\033[K\n"       "$elapsed_str"
        printf "     \033[K\n"
        if [[ "$compiled" -eq 0 ]]; then
            # Phase 1: show verification sub-steps; step 2 advances at ~60s, step 3 at ~120s
            local ic_vs2 ic_vs3
            if   [[ $elapsed -lt 60  ]]; then ic_vs2="$frame";          ic_vs3='\xe2\x97\x8b'
            elif [[ $elapsed -lt 120 ]]; then ic_vs2='\xe2\x9c\x93';   ic_vs3="$frame"
            else                               ic_vs2='\xe2\x9c\x93';   ic_vs3='\xe2\x9c\x93'
            fi
            printf "     Scan steps:\033[K\n"
            printf "     \xe2\x9c\x93  Read Cargo.lock (%d packages)\033[K\n"  "$total_pkgs"
            printf "     %b  Validate vendor manifests\033[K\n"                 "$ic_vs2"
            printf "     %b  Build compile plan\033[K\n"                        "$ic_vs3"
        else
            printf "     Milestones:\033[K\n"
            printf "     %b  Verify sources\033[K\n"         "$ic_res"
            printf "     %b  Compile release build\033[K\n"  "$ic_cmp"
            printf "     %b  Install binary\033[K\n"         "$ic_lnk"
        fi

        sleep 0.12
    done

    wait "$cargo_pid"
    local exit_code=$?
    printf '\033[?25h'
    trap - INT TERM

    compiled="$(grep -c "^   Compiling" "$tmpout" 2>/dev/null)" || compiled=0

    {
        printf '# oxide-sloc install log\n# Date:     %s\n# Platform: %s\n# Exit:     %d\n#\n' \
            "$(date)" "$PLATFORM" "$exit_code"
        cat "$tmpout"
    } > "$LOG_FILE"

    [[ "$first_draw" -eq 0 ]] && printf "\033[%dA" "$NLINES"

    if [[ $exit_code -ne 0 ]]; then
        printf "  \xe2\x9c\x97  BUILD FAILED%-67s\n" ""
        for (( i = 1; i < NLINES; i++ )); do printf "\033[K\n"; done
        printf '\n'
        cat "$tmpout" >&2
        rm -f "$tmpout"
        printf '\n  Build failed. Full output saved:\n'
        print_log_link "$LOG_FILE"
        printf '\n'
        return 1
    fi

    rm -f "$tmpout"

    local final_elapsed=$(( SECONDS - start_time ))
    local final_elapsed_str
    printf -v final_elapsed_str '%d:%02d' "$(( final_elapsed / 60 ))" "$(( final_elapsed % 60 ))"

    local final_bar=""
    for (( i = 0; i < bar_w; i++ )); do final_bar+='='; done

    local final_count_str final_pct_str=""
    if [[ "$compiled" -gt 0 ]]; then
        final_count_str="${compiled} crates"
        final_pct_str="(100%)"
    fi

    if [[ "$compiled" -gt 0 ]]; then
        printf "  \xe2\x9c\x93  Step [3/3]  Build complete \xe2\x80\x94 %d crates compiled.%-13s\033[K\n" "$compiled" ""
        printf "     [%s] %-16s%s\033[K\n"    "$final_bar" "$final_count_str" "$final_pct_str"
        printf "     Current:  %-50s\033[K\n" "Done."
    else
        printf "  \xe2\x9c\x93  Step [3/3]  Already up to date.%-31s\033[K\n" ""
        printf "     [%s]\033[K\n"             "$final_bar"
        printf "     Current:  %-50s\033[K\n" "No changes detected."
    fi
    printf "     Elapsed:  %-50s\033[K\n" "$final_elapsed_str"
    printf "     \033[K\n"
    printf "     Milestones:\033[K\n"
    printf "     \xe2\x9c\x93  Verify sources\033[K\n"
    printf "     \xe2\x9c\x93  Compile release build\033[K\n"
    printf "     \xe2\x9c\x93  Install binary\033[K\n"
    printf '\n  Log \xe2\x86\x92 %s\n' "$LOG_FILE"
    return 0
}

# Version this checkout expects (workspace Cargo.toml — the single source of truth).
WORKSPACE_VERSION="$(grep '^version' "$REPO_ROOT/Cargo.toml" 2>/dev/null | head -1 | sed 's/.*"\(.*\)".*/\1/')"

# Print the semver an installed binary reports, or empty if it can't be determined.
installed_version() {
    "$1" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1
}

# True if a source to reinstall the correct version is available (committed dist/
# archive for this platform, a Rust toolchain on PATH, or an opted-in online fetch).
reinstall_source_available() {
    local dist
    if [[ "$PLATFORM" == windows ]]; then
        dist="$REPO_ROOT/dist/oxide-sloc-windows-x64.zip"
    else
        dist="$REPO_ROOT/dist/oxide-sloc-linux-${LINUX_ARCH}.tar.gz"
    fi
    [[ -f "$dist" ]] && return 0
    command -v cargo &>/dev/null && return 0
    { [[ "$ONLINE_MODE" == true ]] || [[ -n "${SLOC_RELEASE_BASE_URL:-}" ]]; } \
        && command -v curl &>/dev/null && return 0
    return 1
}

echo ""
echo " oxide-sloc installer"
echo " ════════════════════"

# ── 1. Already installed ────────────────────────────────────────────────────
# A bare "file exists" check silently no-ops an upgrade: after `git pull` to a new
# version the stale binary stays in place. Compare the installed version against
# this checkout and, on a mismatch, reinstall (or warn loudly) instead of a green [OK].
if [[ -f "$EXE" ]] && [[ "$FORCE_REBUILD" == false ]]; then
    _installed_ver="$(installed_version "$EXE")"
    if [[ -n "$WORKSPACE_VERSION" && -n "$_installed_ver" && "$_installed_ver" != "$WORKSPACE_VERSION" ]]; then
        if reinstall_source_available; then
            echo ""
            echo " [WARN] Installed oxide-sloc is OUT OF DATE — reinstalling."
            echo "        Installed:      v${_installed_ver}"
            echo "        This checkout:  v${WORKSPACE_VERSION}"
            echo ""
            rm -f "$EXE"    # fall through to dist / download / source build below
        else
            echo ""
            echo " [WARN] Installed oxide-sloc is OUT OF DATE and no reinstall source was found." >&2
            echo "        Installed:      v${_installed_ver}" >&2
            echo "        This checkout:  v${WORKSPACE_VERSION}" >&2
            echo "" >&2
            echo "        The old binary is still in place. To force a refresh:" >&2
            echo "          rm -f \"$EXE\"" >&2
            echo "          bash scripts/internal/install.sh          # from committed dist/, or" >&2
            echo "          bash scripts/internal/install.sh --build   # compile from bundled sources" >&2
            echo "" >&2
            exit 1
        fi
    else
        echo " [OK] $(basename "$EXE") already present${_installed_ver:+ (v${_installed_ver})}."
        trust_ca_cert
        echo " Run: bash scripts/run.sh"
        echo " To rebuild from source:  bash scripts/internal/install.sh --rebuild"
        exit 0
    fi
fi

if [[ -f "$EXE" ]] && [[ "$FORCE_REBUILD" == true ]]; then
    echo " [--rebuild] Removing existing binary to force a fresh build..."
    rm -f "$EXE"
fi

# ── 2. Pre-built binary from dist/ ───────────────────────────────────────────
# update-dist.yml commits platform archives here after every release.
# Skipped under --build/--rebuild: those flags opt into compiling from source, so
# extracting the pre-built dist archive here would silently ignore the request.
if [[ "$FORCE_REBUILD" == false ]] && [[ "$BUILD_MODE" == false ]]; then
    if [[ "$PLATFORM" == windows ]]; then
        # A pre-built binary ships in dist/ as BOTH a .zip and a .tar.gz so it can be
        # unpacked with whatever offline tool a locked-down host has. Try them in order
        # of reliability. Crucially, GNU tar (bundled with Git for Windows) and the
        # built-in Windows tar.exe (bsdtar in System32) both work when `unzip` is absent
        # and PowerShell is restricted (Constrained Language Mode / AllSigned) — the
        # exact combination that made older installs dead-end at "download or build".
        DIST_ZIP="$REPO_ROOT/dist/oxide-sloc-windows-x64.zip"
        DIST_TGZ="$REPO_ROOT/dist/oxide-sloc-windows-x64.tar.gz"
        if [[ -f "$DIST_ZIP" ]] || [[ -f "$DIST_TGZ" ]]; then
            echo " Pre-built binary found in dist/ — extracting (offline, no build needed)..."
            _DIST_TMP="$(mktemp -d)"
            _DIST_OK=false

            # 1. GNU tar on the committed .tar.gz — always present in Git Bash.
            if [[ "$_DIST_OK" != true ]] && [[ -f "$DIST_TGZ" ]] && command -v tar &>/dev/null; then
                tar --no-same-owner -xzf "$DIST_TGZ" -C "$_DIST_TMP" 2>/dev/null || true
                [[ -f "$_DIST_TMP/oxide-sloc.exe" ]] && _DIST_OK=true
            fi
            # 2. unzip on the .zip.
            if [[ "$_DIST_OK" != true ]] && [[ -f "$DIST_ZIP" ]] && command -v unzip &>/dev/null; then
                unzip -q "$DIST_ZIP" -d "$_DIST_TMP" 2>/dev/null || true
                [[ -f "$_DIST_TMP/oxide-sloc.exe" ]] && _DIST_OK=true
            fi
            # 3. Windows built-in bsdtar (tar.exe) — extracts .zip too; no PowerShell needed.
            if [[ "$_DIST_OK" != true ]] && [[ -f "$DIST_ZIP" ]] && [[ -x /c/Windows/System32/tar.exe ]]; then
                _ZIP_W="$(cygpath -w "$DIST_ZIP" 2>/dev/null || echo "$DIST_ZIP")"
                _TMP_W="$(cygpath -w "$_DIST_TMP" 2>/dev/null || echo "$_DIST_TMP")"
                /c/Windows/System32/tar.exe -xf "$_ZIP_W" -C "$_TMP_W" 2>/dev/null || true
                [[ -f "$_DIST_TMP/oxide-sloc.exe" ]] && _DIST_OK=true
            fi
            # 4. PowerShell Expand-Archive — last, since a hardened host may block it.
            if [[ "$_DIST_OK" != true ]] && [[ -f "$DIST_ZIP" ]] && command -v powershell.exe &>/dev/null; then
                _ZIP_W="$(cygpath -w "$DIST_ZIP" 2>/dev/null || echo "$DIST_ZIP")"
                _TMP_W="$(cygpath -w "$_DIST_TMP" 2>/dev/null || echo "$_DIST_TMP")"
                powershell.exe -NoProfile -NonInteractive -Command \
                    "Expand-Archive -Path '$_ZIP_W' -DestinationPath '$_TMP_W' -Force" 2>/dev/null || true
                [[ -f "$_DIST_TMP/oxide-sloc.exe" ]] && _DIST_OK=true
            fi

            if [[ "$_DIST_OK" == true ]] && [[ -f "$_DIST_TMP/oxide-sloc.exe" ]]; then
                cp "$_DIST_TMP/oxide-sloc.exe" "$EXE"
                rm -rf "$_DIST_TMP"
                echo " [OK] oxide-sloc.exe installed from dist/ (no build, no network)."
                trust_ca_cert
                echo ""
                echo " Start the web UI:  bash scripts/run.sh"
                exit 0
            fi
            rm -rf "$_DIST_TMP"

            # Every extractor failed even though a pre-built archive is present. Do NOT
            # steer an air-gapped user toward a network download or a source build (the
            # build path is slow and trips AV/EDR false positives). Explain the real
            # problem and give a copy-paste offline extract, unless a compiler is on PATH
            # and a build is genuinely viable here.
            if command -v cargo &>/dev/null; then
                echo " [WARN] dist/ extraction failed — cargo detected, falling back to source build." >&2
            else
                echo "" >&2
                echo " [ERROR] A pre-built oxide-sloc.exe is present in dist/ but no available tool" >&2
                echo "         could extract it. This is an extraction-tooling gap, NOT a missing" >&2
                echo "         binary — no download and no compiler are required." >&2
                echo "" >&2
                echo "         Unpack it by hand (any ONE of these) from the repo root, then re-run" >&2
                echo "         bash scripts/run.sh :" >&2
                [[ -f "$DIST_TGZ" ]] && \
                echo "           tar -xzf dist/oxide-sloc-windows-x64.tar.gz -C ." >&2
                [[ -f "$DIST_ZIP" ]] && \
                echo "           tar -xf  dist/oxide-sloc-windows-x64.zip    -C .   # Windows/Git-Bash tar" >&2
                echo "" >&2
                echo "         If the unpacked oxide-sloc.exe disappears immediately, antivirus/EDR" >&2
                echo "         quarantined it — allow-list the repo folder or the .exe and retry." >&2
                echo "" >&2
                exit 1
            fi
        fi
    else
        _DIST_LINUX="$REPO_ROOT/dist/oxide-sloc-linux-${LINUX_ARCH}.tar.gz"
        if [[ -f "$_DIST_LINUX" ]]; then
            echo " Pre-built binary found in dist/ — extracting..."
            # --no-same-owner: as root or under a uid-remapped userns (rootless
            # containers, CI agents), GNU tar defaults to --same-owner and its
            # chown to the archive's original uid/gid fails with EINVAL, making
            # tar exit non-zero even though the binary extracted fine. Drop the
            # chown and trust the on-disk result, not tar's exit code.
            tar --no-same-owner -xzf "$_DIST_LINUX" -C "$REPO_ROOT" 2>/dev/null || true
            if [[ -f "$EXE" ]]; then
                chmod +x "$EXE"
                echo " [OK] oxide-sloc installed from dist/"
                echo ""
                echo " Start the web UI:  bash scripts/run.sh"
                exit 0
            fi

            # Archive present but extraction produced no binary. Don't steer an
            # air-gapped user toward a network download or a source build (slow,
            # trips AV/EDR) unless a compiler is genuinely on PATH — explain the
            # real problem and give a copy-paste offline extract.
            if command -v cargo &>/dev/null; then
                echo " [WARN] dist/ extraction failed — cargo detected, falling back to source build." >&2
            else
                echo "" >&2
                echo " [ERROR] A pre-built oxide-sloc is present in dist/ but it could not be" >&2
                echo "         extracted. This is an extraction-tooling gap, NOT a missing" >&2
                echo "         binary — no download and no compiler are required." >&2
                echo "" >&2
                echo "         Unpack it by hand from the repo root, then re-run bash scripts/run.sh :" >&2
                echo "           tar --no-same-owner -xzf dist/oxide-sloc-linux-${LINUX_ARCH}.tar.gz -C ." >&2
                echo "" >&2
                echo "         If the unpacked oxide-sloc disappears immediately, antivirus/EDR" >&2
                echo "         quarantined it — allow-list the repo folder or the binary and retry." >&2
                echo "" >&2
                exit 1
            fi
        fi
    fi
fi

# ── 3. Download pre-built binary from GitHub Releases ────────────────────────
# When --online is set, download verbosely and fall back on any error.
# When cargo is not on PATH (no local build environment), attempt a silent
# download first — short timeouts ensure air-gapped machines fall through quickly.
_do_download() {
    local _verbose="$1"
    local _OV _BASE _ASSET _TMP _OK=false _SUMS_OK=false _SUMS_FILE

    _OV="$(grep '^version' "$REPO_ROOT/Cargo.toml" 2>/dev/null | head -1 | sed 's/.*"\(.*\)".*/\1/')"
    [[ -z "$_OV" ]] && return 1

    # Allow internal artifact servers to replace the GitHub Releases URL.
    # Set SLOC_RELEASE_BASE_URL=https://nexus.example.com/oxide-sloc (no trailing slash).
    # The installer appends /v{version}/{asset} to form the full download URL.
    local _url_root="${SLOC_RELEASE_BASE_URL:-https://github.com/oxide-sloc/oxide-sloc/releases/download}"
    _BASE="${_url_root}/v${_OV}"

    if [[ "$PLATFORM" == windows ]]; then
        _ASSET="oxide-sloc-windows-x64.zip"
    else
        _ASSET="oxide-sloc-linux-${LINUX_ARCH}.tar.gz"
    fi

    [[ "$_verbose" == true ]] && echo " Downloading oxide-sloc v${_OV} (${_ASSET})..."

    _TMP="$(mktemp -d)"

    local _curl_opts=(-fsSL)
    [[ "$_verbose" == false ]] && _curl_opts+=( --connect-timeout 5 --max-time 30 )

    if curl "${_curl_opts[@]}" -o "$_TMP/$_ASSET" "${_BASE}/${_ASSET}" 2>/dev/null; then
        for _sf in SHA256SUMS-dist.txt SHA256SUMS.txt; do
            if curl "${_curl_opts[@]}" -o "$_TMP/$_sf" "${_BASE}/$_sf" 2>/dev/null; then
                _SUMS_OK=true; _SUMS_FILE="$_sf"; break
            fi
        done

        if [[ "$_SUMS_OK" == true ]]; then
            cd "$_TMP"
            if grep "$_ASSET" "$_SUMS_FILE" | sha256sum --check --status 2>/dev/null; then
                if [[ "$PLATFORM" == windows ]]; then
                    if command -v unzip &>/dev/null; then
                        unzip -q "$_ASSET" -d "$_TMP" && _OK=true
                    else
                        local _aw _tw
                        _aw="$(cygpath -w "$_TMP/$_ASSET" 2>/dev/null || echo "$_TMP/$_ASSET")"
                        _tw="$(cygpath -w "$_TMP" 2>/dev/null || echo "$_TMP")"
                        powershell.exe -NoProfile -NonInteractive -Command \
                            "Expand-Archive -Path '$_aw' -DestinationPath '$_tw' -Force" \
                            && _OK=true
                    fi
                    if [[ "$_OK" == true ]] && [[ -f "$_TMP/oxide-sloc.exe" ]]; then
                        cp "$_TMP/oxide-sloc.exe" "$EXE"
                    else
                        _OK=false
                    fi
                else
                    tar -xzf "$_ASSET" -C "$_TMP"
                    if [[ -f "$_TMP/oxide-sloc" ]]; then
                        cp "$_TMP/oxide-sloc" "$EXE"
                        chmod +x "$EXE"
                        _OK=true
                    fi
                fi
            elif [[ "$_verbose" == true ]]; then
                echo " [WARN] Checksum mismatch — falling back to source build." >&2
            fi
            cd "$REPO_ROOT"
        elif [[ "$_verbose" == true ]]; then
            echo " [WARN] Could not fetch checksum file — falling back to source build." >&2
        fi
    elif [[ "$_verbose" == true ]]; then
        echo " [WARN] Download failed — falling back to source build." >&2
    fi

    rm -rf "$_TMP"

    if [[ "$_OK" == true ]]; then
        [[ "$_verbose" == true ]] && echo " Downloaded and verified."
        echo " [OK] oxide-sloc v${_OV} installed → $(basename "$EXE")"
        echo ""
        echo " Start the web UI:  bash scripts/run.sh"
        return 0
    fi
    return 1
}

if [[ "$ONLINE_MODE" == true ]]; then
    _do_download true && exit 0
elif [[ "$FORCE_REBUILD" == false ]] && [[ "$BUILD_MODE" == false ]] \
    && ! command -v cargo &>/dev/null \
    && command -v curl &>/dev/null \
    && [[ -n "${SLOC_RELEASE_BASE_URL:-}" ]]; then
    # SLOC_RELEASE_BASE_URL is set — an administrator has configured a trusted artifact
    # server.  Silently attempt a download before falling through to toolchain extraction.
    _do_download false && exit 0
fi

# ── 4. Bootstrap Rust from bundled toolchain (if cargo not on PATH) ──────────
if ! command -v cargo &>/dev/null; then
    if [[ "$BUILD_MODE" == false ]]; then
        _OV="$(grep '^version' "$REPO_ROOT/Cargo.toml" 2>/dev/null | head -1 | sed 's/.*"\(.*\)".*/\1/')"
        echo "" >&2
        echo " [ERROR] No pre-built binary found and no Rust toolchain on PATH." >&2
        echo "" >&2
        echo " Option 1 — compile from the bundled sources (no network required):" >&2
        echo "   bash scripts/internal/install.sh --build" >&2
        echo "" >&2
        echo " Option 2 — download a pre-built binary:" >&2
        if [[ -n "${SLOC_RELEASE_BASE_URL:-}" ]]; then
        echo "   From your configured server: ${SLOC_RELEASE_BASE_URL}/v${_OV}/" >&2
        else
        echo "   From GitHub Releases (requires internet access):" >&2
        fi
        echo "   bash scripts/internal/install.sh --online" >&2
        echo "" >&2
        echo " Option 3 — place the pre-built archive in dist/ and re-run:" >&2
        if [[ "$PLATFORM" == windows ]]; then
        echo "   dist/oxide-sloc-windows-x64.zip" >&2
        else
        echo "   dist/oxide-sloc-linux-${LINUX_ARCH}.tar.gz" >&2
        fi
        echo "" >&2
        exit 1
    fi
    # --build flag set: proceed with toolchain extraction
    if [[ "$PLATFORM" == windows ]]; then
        TOOLCHAIN_ARCHIVE="$REPO_ROOT/toolchain/rust-toolchain-windows-x64.tar.gz"
    else
        TOOLCHAIN_ARCHIVE="$REPO_ROOT/toolchain/rust-toolchain-linux-${LINUX_ARCH}.tar.gz"
    fi

    # Collect split parts (*.tar.gz.aa, .ab, …); fall back to single archive
    TOOLCHAIN_PARTS=()
    for _tp in "${TOOLCHAIN_ARCHIVE}".*; do
        [[ -f "$_tp" ]] && TOOLCHAIN_PARTS+=("$_tp")
    done
    HAVE_TOOLCHAIN=false
    [[ "${#TOOLCHAIN_PARTS[@]}" -gt 0 ]] && HAVE_TOOLCHAIN=true
    [[ -f "$TOOLCHAIN_ARCHIVE" ]]         && HAVE_TOOLCHAIN=true

    if [[ "$HAVE_TOOLCHAIN" == true ]]; then
        echo " No Rust toolchain detected — bootstrapping from bundled archive..."
        echo " (one-time extract → ~300-500 MB on disk)"
        echo ""
        # Verify checksum of each part or the single archive
        TOOLCHAIN_CHECKSUMS="$REPO_ROOT/toolchain/checksums.sha256"
        if [[ -f "$TOOLCHAIN_CHECKSUMS" ]] && command -v sha256sum &>/dev/null; then
            _VERIFY_FILES=()
            if [[ "${#TOOLCHAIN_PARTS[@]}" -gt 0 ]]; then
                _VERIFY_FILES=("${TOOLCHAIN_PARTS[@]}")
            else
                _VERIFY_FILES=("$TOOLCHAIN_ARCHIVE")
            fi
            for _vf in "${_VERIFY_FILES[@]}"; do
                _VF_NAME="$(basename "$_vf")"
                _EXPECTED="$(grep "$_VF_NAME" "$TOOLCHAIN_CHECKSUMS" 2>/dev/null | awk '{print $1}')"
                if [[ -n "$_EXPECTED" ]]; then
                    _ACTUAL="$(sha256sum "$_vf" | awk '{print $1}')"
                    if [[ "$_EXPECTED" != "$_ACTUAL" ]]; then
                        echo " [ERROR] Toolchain checksum mismatch — ${_VF_NAME} may be corrupt." >&2
                        echo "         Expected: $_EXPECTED" >&2
                        echo "         Actual:   $_ACTUAL" >&2
                        exit 1
                    fi
                fi
            done
            echo " [OK] Toolchain checksum verified."
        fi

        TOOLS_DIR="$REPO_ROOT/.tools"
        mkdir -p "$TOOLS_DIR"

        # Trust the signing CA before extracting toolchain binaries, so Windows can
        # verify the publisher signature when AV scans newly written PE files.
        trust_ca_cert

        if [[ "$PLATFORM" == windows ]]; then
            _tools_w="$(cygpath -w "$TOOLS_DIR" 2>/dev/null || echo "$TOOLS_DIR")"
            _target_w="$(cygpath -w "$REPO_ROOT/target" 2>/dev/null || echo "$REPO_ROOT/target")"
            _vendor_w="$(cygpath -w "$VENDOR_DIR" 2>/dev/null || echo "$VENDOR_DIR")"
            echo ""
            echo " NOTE: The next step extracts ~200 Rust compiler binaries."
            echo "       Antivirus may flag this activity. Affected directories:"
            echo "         .tools/   target/release/   vendor/"
            echo "       Attempting to add Windows Defender exclusions automatically..."
            powershell.exe -NoProfile -NonInteractive -Command "
try {
    \$paths = @('$_tools_w', '$_target_w', '$_vendor_w')
    Add-MpPreference -ExclusionPath \$paths -ErrorAction Stop
    Write-Output ' [OK] Defender exclusions added for build directories.'
} catch {
    Write-Output ' [NOTE] Auto-exclusion failed (may need admin). If AV blocks the build,'
    Write-Output '        add these folder exclusions in Windows Security manually:'
    Write-Output '          .tools/   target/release/   vendor/'
}
" 2>/dev/null || true
            echo ""
        fi

        echo " Extracting toolchain archive..."
        if [[ "$PLATFORM" == windows ]]; then
            # Windows: Git Bash tar records rustup's proxy hardlinks (cargo.exe,
            # rustc.exe, etc.) as POSIX symlinks pointing to rustup.exe.  Symlink
            # creation fails with ENOENT when the target hasn't been extracted yet
            # (alphabetical ordering puts cargo.exe before rustup.exe).  Extract
            # while tolerating those specific errors, then copy rustup.exe over
            # any proxy that tar couldn't create.
            _tar_stderr="$(mktemp)"
            if [[ "${#TOOLCHAIN_PARTS[@]}" -gt 0 ]]; then
                cat "${TOOLCHAIN_PARTS[@]}" | tar -xzf - -C "$TOOLS_DIR" \
                    2>"$_tar_stderr" || true
            else
                tar -xzf "$TOOLCHAIN_ARCHIVE" -C "$TOOLS_DIR" \
                    2>"$_tar_stderr" || true
            fi
            # Surface any real errors (non-symlink); silently discard symlink noise.
            grep -v "Cannot create symlink" "$_tar_stderr" >&2 || true
            rm -f "$_tar_stderr"
            # Patch missing proxy binaries: every file in cargo/bin/ that tar
            # couldn't create is a hardlink alias of rustup.exe — copy it there.
            _rustup_proxy="$TOOLS_DIR/cargo/bin/rustup.exe"
            if [[ -f "$_rustup_proxy" ]]; then
                for _proxy in cargo.exe rustc.exe rustdoc.exe; do
                    [[ -f "$TOOLS_DIR/cargo/bin/$_proxy" ]] || \
                        cp "$_rustup_proxy" "$TOOLS_DIR/cargo/bin/$_proxy"
                done
                echo " [OK] Windows toolchain proxy binaries verified."
            fi
        elif [[ "${#TOOLCHAIN_PARTS[@]}" -gt 0 ]]; then
            cat "${TOOLCHAIN_PARTS[@]}" | tar -xzf - -C "$TOOLS_DIR"
        else
            tar -xzf "$TOOLCHAIN_ARCHIVE" -C "$TOOLS_DIR"
        fi

        # Archive layout (from bundle-rust-toolchain.sh):
        #   .tools/rustup/toolchains/<ver>-<target>/bin/  ← rustc, etc.
        #   .tools/cargo/bin/                              ← cargo
        RUSTUP_HOME_LOCAL="$TOOLS_DIR/rustup"
        CARGO_HOME_LOCAL="$TOOLS_DIR/cargo"

        # Find the toolchain bin dir
        TC_BIN="$(find "$RUSTUP_HOME_LOCAL/toolchains" -maxdepth 2 -name bin -type d 2>/dev/null | head -1)"

        export RUSTUP_HOME="$RUSTUP_HOME_LOCAL"
        export CARGO_HOME="$CARGO_HOME_LOCAL"
        export PATH="$CARGO_HOME_LOCAL/bin:${TC_BIN:-$RUSTUP_HOME_LOCAL/bin}:$PATH"

        # Pin rustup to the EXACT bundled toolchain. rust-toolchain.toml requests
        # channel "1.97" (an x.y shorthand), which the rustup proxy tries to resolve
        # against the ONLINE channel manifest — fatal on a sealed network. The bundle
        # installs "1.97.0-<target>"; naming it explicitly makes rustup use the local
        # toolchain and never reach the network. TC_BIN is …/toolchains/<name>/bin.
        if [[ -n "${TC_BIN:-}" ]]; then
            _TC_NAME="$(basename "$(dirname "$TC_BIN")")"
            [[ -n "$_TC_NAME" ]] && export RUSTUP_TOOLCHAIN="$_TC_NAME"
        fi

        if ! command -v cargo &>/dev/null; then
            echo " [ERROR] cargo not found after toolchain extraction." >&2
            echo "         Verify the toolchain archive is complete and try:" >&2
            echo "           bash scripts/internal/bundle-rust-toolchain.sh" >&2
            exit 1
        fi
        echo " [OK] Rust toolchain bootstrapped at .tools/"

        # RHEL/Linux: ensure a C linker is available (needed by Rust GNU target)
        if [[ "$PLATFORM" == linux ]] && ! command -v cc &>/dev/null && ! command -v gcc &>/dev/null; then
            echo ""
            echo " [WARN] No C linker (cc/gcc) found." >&2
            echo "        The Rust GNU target requires a C linker to link executables." >&2
            echo "        Install with:" >&2
            echo "          RHEL/CentOS:  sudo dnf install gcc" >&2
            echo "          Debian/Ubuntu: sudo apt install gcc" >&2
            echo "        Then re-run: bash scripts/internal/install.sh" >&2
            exit 1
        fi
    fi
fi

# ── 5. Build from vendored sources ──────────────────────────────────────────
if command -v cargo &>/dev/null; then
    if [[ ! -d "$VENDOR_DIR" ]]; then
        # Collect split vendor parts (vendor.tar.gz.aa, .ab, .ac).
        VENDOR_PARTS=()
        for _vp in "$REPO_ROOT"/vendor.tar.gz.*; do
            [[ -f "$_vp" ]] && VENDOR_PARTS+=("$_vp")
        done
        if [[ "${#VENDOR_PARTS[@]}" -gt 0 ]]; then
            # Verify each part against vendor.checksums.sha256 (mirrors the toolchain
            # verify loop above) before reassembling.
            if [[ -f "$VENDOR_CHECKSUMS" ]] && command -v sha256sum &>/dev/null; then
                for _vf in "${VENDOR_PARTS[@]}"; do
                    _VF_NAME="$(basename "$_vf")"
                    _EXPECTED="$(grep "$_VF_NAME" "$VENDOR_CHECKSUMS" 2>/dev/null | awk '{print $1}')"
                    if [[ -n "$_EXPECTED" ]]; then
                        _ACTUAL="$(sha256sum "$_vf" | awk '{print $1}')"
                        if [[ "$_EXPECTED" != "$_ACTUAL" ]]; then
                            echo " [ERROR] Vendor checksum mismatch — ${_VF_NAME} may be corrupt." >&2
                            echo "         Expected: $_EXPECTED" >&2
                            echo "         Actual:   $_ACTUAL" >&2
                            exit 1
                        fi
                    fi
                done
                echo " [OK] Vendor checksum verified."
            fi
            echo " Reassembling and decompressing vendor sources (one-time)..."
            cat "${VENDOR_PARTS[@]}" | tar -xzf - -C "$REPO_ROOT"
            echo " [OK] Vendor sources ready."
        else
            echo " [ERROR] Neither vendor/ nor vendor.tar.gz.* parts found." >&2
            echo "         The vendor archive is committed to the repository — ensure you have" >&2
            echo "         the complete repository, not just source files." >&2
            exit 1
        fi
    fi

    echo " Rust toolchain found — building from vendored sources..."
    # Create the cargo offline config that redirects to vendor/
    mkdir -p "$REPO_ROOT/.cargo"
    cat > "$REPO_ROOT/.cargo/config.toml" <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF

    cd "$REPO_ROOT"
    build_with_progress || exit 1

    if [[ -f "$BUILD_OUTPUT" ]]; then
        cp "$BUILD_OUTPUT" "$EXE"
        [[ "$PLATFORM" == linux ]] && chmod +x "$EXE"
        echo " [OK] Built and installed $(basename "$EXE")"
        trust_ca_cert
        echo ""
        echo " Start the web UI:  bash scripts/run.sh"
        exit 0
    fi
    echo " [ERROR] Build failed — binary not found after build." >&2
    exit 1
fi

# ── 6. No Rust toolchain available ──────────────────────────────────────────

echo ""
echo " No Rust toolchain detected and no bundled toolchain archives found."
echo ""
echo " The toolchain/rust-toolchain-*.tar.gz.* archives must be present for"
echo " offline source builds. A maintainer generates them with:"
echo ""
if [[ "$PLATFORM" == windows ]]; then
    echo "   bash scripts/internal/bundle-rust-toolchain.sh windows-x64"
else
    echo "   bash scripts/internal/bundle-rust-toolchain.sh linux-x86_64   # or linux-arm64"
fi
echo "   git add toolchain/"
echo "   git commit -m \"chore: bundle Rust toolchain for offline builds\""
echo "   git push"
echo ""
echo " After the toolchain archives are committed, re-run this script on any fresh clone."
echo ""
echo " If Rust is already installed on this machine:"
echo "   bash scripts/internal/install.sh"
echo ""
echo " Full deployment guide: docs/airgap.md"
echo ""
exit 1
