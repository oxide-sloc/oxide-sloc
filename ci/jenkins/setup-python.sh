#!/usr/bin/env bash
# Extract or verify a portable, offline CPython interpreter for a Jenkins build.
#
# Mirrors ci/jenkins/setup-toolchain.sh: same platform detection, same
# checksum-verify-then-extract-into-.tools pattern, same tolerance for the
# Windows symlink-ENOENT noise that Git Bash tar emits.
#
# Python is OPTIONAL — the pipeline treats a missing bundle as non-fatal and
# continues without a bundled interpreter. This script therefore fails fast with
# a clear message (non-zero exit) when no bundle is present for the platform,
# rather than reaching out to the network.
#
# Tiers, in order of preference (first that succeeds wins):
#   1. Already-extracted bundle    — .tools/python/ has the platform interpreter
#                                    (fast path; no re-extract).
#   2. Committed air-gap bundle    — the python/ archive that ships in the repo
#                                    for a plain `git clone` offline build.
#
# On success the ABSOLUTE-ish POSIX path to the interpreter is printed on the
# LAST line of stdout, alone, so a caller can capture it:
#   Linux   -> .tools/python/bin/python3
#   Windows -> .tools/python/python.exe
# All human-readable log text goes to earlier lines / stderr.
set -euo pipefail

TOOLS_DIR=".tools"
PY_ROOT="${TOOLS_DIR}/python"

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

# ── Per-platform interpreter path inside the extracted python/ tree ───────────
# Windows unpacks python/python.exe; Linux unpacks python/bin/python3 (verified
# against the actual python-build-standalone install_only_stripped layout).
interpreter_path() {
    local platform="$1"
    if [ "${platform}" = "windows" ]; then
        echo "${PY_ROOT}/python.exe"
    else
        echo "${PY_ROOT}/bin/python3"
    fi
}

# ── Committed archive name for this platform ──────────────────────────────────
archive_path() {
    local platform="$1"
    if [ "${platform}" = "windows" ]; then
        echo "python/cpython-3.14-windows-x64.tar.gz"
    else
        echo "python/cpython-3.14-linux-x86_64.tar.gz"
    fi
}

# ── Air-gap bundle extraction (mirrors setup-toolchain.sh) ────────────────────
# Selects the committed Python archive for this platform, verifies it against
# python/checksums.sha256 (only the relevant line, matched by basename),
# extracts into .tools/ (so it becomes .tools/python/...), tolerating Windows
# symlink-ENOENT noise. Returns 0 on success, 1 if no bundle is present.
extract_python_bundle() {
    local platform="$1"
    local archive
    archive="$(archive_path "${platform}")"

    if [ ! -f "${archive}" ]; then
        return 1
    fi

    echo "Extracting committed portable Python bundle (air-gapped): ${archive}" >&2

    # Verify the checksum of the archive by basename against python/checksums.sha256.
    local checksums="python/checksums.sha256"
    if [ -f "${checksums}" ] && command -v sha256sum >/dev/null 2>&1; then
        local _name _expected _actual
        _name="$(basename "${archive}")"
        _expected="$(grep "${_name}" "${checksums}" 2>/dev/null | awk '{print $1}')"
        if [ -n "${_expected}" ]; then
            _actual="$(sha256sum "${archive}" | awk '{print $1}')"
            if [ "${_expected}" != "${_actual}" ]; then
                echo "ERROR: Python bundle checksum mismatch for ${_name} — bundle may be corrupt." >&2
                echo "       Expected: ${_expected}" >&2
                echo "       Actual:   ${_actual}" >&2
                exit 1
            fi
            echo "Python bundle checksum verified." >&2
        fi
    fi

    mkdir -p "${TOOLS_DIR}"

    if [ "${platform}" = "windows" ]; then
        # Git Bash tar can record some entries as POSIX symlinks; symlink creation
        # fails with ENOENT when the target hasn't been extracted yet. Extract while
        # tolerating those specific errors, and surface any real (non-symlink) ones.
        local _tar_stderr
        _tar_stderr="$(mktemp)"
        tar -xzf "${archive}" -C "${TOOLS_DIR}" 2>"${_tar_stderr}" || true
        grep -v "Cannot create symlink" "${_tar_stderr}" >&2 || true
        rm -f "${_tar_stderr}"
    else
        tar -xzf "${archive}" -C "${TOOLS_DIR}"
    fi

    local interp
    interp="$(interpreter_path "${platform}")"
    if [ ! -f "${interp}" ]; then
        echo "ERROR: interpreter not found after extracting the committed Python bundle." >&2
        echo "       Expected: ${interp}" >&2
        echo "       The bundle under python/ may be incomplete; regenerate it with" >&2
        echo "       bash ci/jenkins/bundle-python.sh" >&2
        exit 1
    fi
    echo "Portable Python bootstrapped from the committed air-gap bundle." >&2
    return 0
}

PLATFORM="$(detect_platform)"
INTERP="$(interpreter_path "${PLATFORM}")"

# Tier 1: already extracted — fast path, no re-extract.
if [ -f "${INTERP}" ]; then
    echo "Portable Python already extracted at ${INTERP} — skipping extract." >&2
    printf '%s\n' "${INTERP}"
    exit 0
fi

# Tier 2: committed air-gap bundle.
if extract_python_bundle "${PLATFORM}"; then
    printf '%s\n' "${INTERP}"
    exit 0
fi

# No bundle present for this platform — Python is optional, so fail fast with a
# clear message and let the caller continue without a bundled interpreter.
echo "============================================================" >&2
echo "No committed portable Python bundle found for platform: ${PLATFORM}" >&2
echo "  Expected archive: $(archive_path "${PLATFORM}")" >&2
echo "" >&2
echo "  Python is optional — the pipeline will continue without a" >&2
echo "  bundled interpreter. To ship one for offline agents, run:" >&2
echo "    bash ci/jenkins/bundle-python.sh" >&2
echo "  then commit the python/ directory." >&2
echo "============================================================" >&2
exit 1
