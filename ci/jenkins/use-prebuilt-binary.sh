#!/usr/bin/env bash
# Fast-path build for the oxide-sloc Jenkins pipeline: extract the committed
# prebuilt scanner binary from dist/ into target/release/ — NO Rust toolchain, NO
# vendor archive, NO compile, NO network. This is what makes a standard scan finish
# in a couple of minutes instead of tens.
#
# Invoked by pipeline-helpers.groovy runBuild() when BUILD_MODE=prebuilt (the
# default) and neither RUN_QUALITY_GATES nor RUN_COVERAGE is set. Mirrors the dist/
# extraction in scripts/internal/install.sh (prefers the .tar.gz for locked-down
# hosts where unzip is absent and PowerShell is blocked).
#
# On success the binary lands at target/release/oxide-sloc(.exe) — exactly where
# BINARY (set by initEnv) points. Exits non-zero with actionable guidance when no
# usable dist/ archive is present (re-run with BUILD_MODE=source to compile).
set -euo pipefail

# ── Platform detection (matches setup-toolchain.sh) ───────────────────────────
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

PLATFORM="$(detect_platform)"
DEST="target/release"
mkdir -p "${DEST}"

if [ "${PLATFORM}" = "windows" ]; then
    EXE="${DEST}/oxide-sloc.exe"
    DIST_ZIP="dist/oxide-sloc-windows-x64.zip"
    DIST_TGZ="dist/oxide-sloc-windows-x64.tar.gz"

    if [ ! -f "${DIST_TGZ}" ] && [ ! -f "${DIST_ZIP}" ]; then
        echo "ERROR: no prebuilt Windows binary in dist/ (${DIST_TGZ} / ${DIST_ZIP})." >&2
        echo "       Re-run the job with BUILD_MODE=source to compile from the vendored crates." >&2
        exit 1
    fi

    echo "Extracting prebuilt oxide-sloc.exe from dist/ (offline, no build) ..."
    OK=false
    # 1. GNU tar on the committed .tar.gz — always present in Git Bash.
    if [ "${OK}" != true ] && [ -f "${DIST_TGZ}" ] && command -v tar >/dev/null 2>&1; then
        tar --no-same-owner -xzf "${DIST_TGZ}" -C "${DEST}" 2>/dev/null || true
        [ -f "${EXE}" ] && OK=true
    fi
    # 2. unzip on the .zip.
    if [ "${OK}" != true ] && [ -f "${DIST_ZIP}" ] && command -v unzip >/dev/null 2>&1; then
        unzip -q -o "${DIST_ZIP}" -d "${DEST}" 2>/dev/null || true
        [ -f "${EXE}" ] && OK=true
    fi
    # 3. Windows built-in bsdtar (tar.exe) — extracts .zip too, no PowerShell needed.
    if [ "${OK}" != true ] && [ -f "${DIST_ZIP}" ] && [ -x /c/Windows/System32/tar.exe ]; then
        _ZIP_W="$(cygpath -w "${DIST_ZIP}" 2>/dev/null || echo "${DIST_ZIP}")"
        _DST_W="$(cygpath -w "${DEST}" 2>/dev/null || echo "${DEST}")"
        /c/Windows/System32/tar.exe -xf "${_ZIP_W}" -C "${_DST_W}" 2>/dev/null || true
        [ -f "${EXE}" ] && OK=true
    fi
    # 4. PowerShell Expand-Archive — last, a hardened host may block it.
    if [ "${OK}" != true ] && [ -f "${DIST_ZIP}" ] && command -v powershell.exe >/dev/null 2>&1; then
        _ZIP_W="$(cygpath -w "${DIST_ZIP}" 2>/dev/null || echo "${DIST_ZIP}")"
        _DST_W="$(cygpath -w "${DEST}" 2>/dev/null || echo "${DEST}")"
        powershell.exe -NoProfile -NonInteractive -Command \
            "Expand-Archive -Path '${_ZIP_W}' -DestinationPath '${_DST_W}' -Force" 2>/dev/null || true
        [ -f "${EXE}" ] && OK=true
    fi

    if [ "${OK}" != true ] || [ ! -f "${EXE}" ]; then
        echo "ERROR: a prebuilt archive is present in dist/ but no available tool could" >&2
        echo "       extract it (extraction-tooling gap, not a missing binary). Unpack by" >&2
        echo "       hand from the repo root, or re-run with BUILD_MODE=source:" >&2
        [ -f "${DIST_TGZ}" ] && echo "         tar -xzf ${DIST_TGZ} -C ${DEST}" >&2
        [ -f "${DIST_ZIP}" ] && echo "         tar -xf  ${DIST_ZIP} -C ${DEST}" >&2
        exit 1
    fi
    echo "[OK] ${EXE} installed from dist/ (no build, no network)."
else
    ARCH="$(uname -m 2>/dev/null || echo x86_64)"
    case "${ARCH}" in
        aarch64|arm64) ARCH="arm64" ;;
        *)             ARCH="x86_64" ;;
    esac
    EXE="${DEST}/oxide-sloc"
    DIST_LINUX="dist/oxide-sloc-linux-${ARCH}.tar.gz"

    if [ ! -f "${DIST_LINUX}" ]; then
        echo "ERROR: no prebuilt Linux binary in dist/ (${DIST_LINUX})." >&2
        echo "       Re-run the job with BUILD_MODE=source to compile from the vendored crates." >&2
        exit 1
    fi

    echo "Extracting prebuilt oxide-sloc from ${DIST_LINUX} (offline, no build) ..."
    # --no-same-owner: as root or under a uid-remapped userns (rootless containers,
    # CI agents) GNU tar's chown to the archive uid/gid fails with EINVAL and tar
    # exits non-zero even though the binary extracted fine. Trust the on-disk result.
    tar --no-same-owner -xzf "${DIST_LINUX}" -C "${DEST}" 2>/dev/null || true
    if [ ! -f "${EXE}" ]; then
        echo "ERROR: ${DIST_LINUX} present but extraction produced no binary at ${EXE}." >&2
        echo "       Unpack by hand (tar -xzf ${DIST_LINUX} -C ${DEST}) or re-run with BUILD_MODE=source." >&2
        exit 1
    fi
    chmod +x "${EXE}" 2>/dev/null || true
    echo "[OK] ${EXE} installed from dist/ (no build, no network)."
fi
