#!/usr/bin/env bash
# preflight.sh — verify a host can install, build, and/or run oxide-sloc, and that
# it is compatible with a target server, BEFORE wiring oxide-sloc into a workflow.
#
# This is deliberately standalone and dependency-free (POSIX sh features + coreutils
# + curl). Copy this ONE file into any project — a CI job, a container entrypoint, a
# Makefile target — to confirm the prerequisites are present and fail fast with
# actionable guidance instead of erroring deep inside a build or a scan.
#
# Usage:
#   bash scripts/preflight.sh [--mode prebuilt|source|run]
#                             [--server-url URL] [--require-version X.Y.Z]
#                             [--json] [--quiet]
#
# Modes (what the host is expected to do):
#   prebuilt  host extracts the committed dist/ binary and runs it (no toolchain).
#   source    host compiles oxide-sloc from source (needs a Rust toolchain).
#   run       host only runs an already-installed oxide-sloc (default when a binary
#             is found on PATH; otherwise defaults to prebuilt).
#
# Options:
#   --server-url URL      Probe a running `oxide-sloc serve` at URL: GET /healthz and
#                         /api/version. Reports the server version and health.
#   --require-version V   Assert the resolved binary (and --server-url server, if
#                         given) report version V (a leading "v" is tolerated). A
#                         hard failure on mismatch — use it to pin a reproducible tool.
#   --json                Emit a machine-readable JSON summary on the last line.
#   --quiet               Suppress the per-check lines; only the summary is printed.
#
# Exit codes:
#   0  all hard checks passed (warnings may still be present)
#   1  one or more hard prerequisites are missing / a version pin mismatched
#   2  bad usage
set -u

MODE=""
SERVER_URL=""
REQUIRE_VERSION=""
JSON=0
QUIET=0

while [ "$#" -gt 0 ]; do
    case "$1" in
        --mode)            MODE="${2:-}"; shift 2 ;;
        --mode=*)          MODE="${1#*=}"; shift ;;
        --server-url)      SERVER_URL="${2:-}"; shift 2 ;;
        --server-url=*)    SERVER_URL="${1#*=}"; shift ;;
        --require-version) REQUIRE_VERSION="${2:-}"; shift 2 ;;
        --require-version=*) REQUIRE_VERSION="${1#*=}"; shift ;;
        --json)            JSON=1; shift ;;
        --quiet)           QUIET=1; shift ;;
        -h|--help)
            sed -n '2,37p' "$0"; exit 0 ;;
        *)
            echo "preflight: unknown argument '$1' (try --help)" >&2; exit 2 ;;
    esac
done

# Normalise a version string: drop a leading v/V.
norm_ver() { printf '%s' "${1#[vV]}"; }
REQUIRE_VERSION="$(norm_ver "$REQUIRE_VERSION")"

FAILS=0
WARNS=0
PASSES=0

ok()   { PASSES=$((PASSES+1)); [ "$QUIET" -eq 1 ] || printf '[ok]   %s\n' "$*"; }
warn() { WARNS=$((WARNS+1));  [ "$QUIET" -eq 1 ] || printf '[warn] %s\n' "$*" >&2; }
fail() { FAILS=$((FAILS+1));  [ "$QUIET" -eq 1 ] || printf '[fail] %s\n' "$*" >&2; }
info() {                       [ "$QUIET" -eq 1 ] || printf '[info] %s\n' "$*"; }

have() { command -v "$1" >/dev/null 2>&1; }

# ── Platform / architecture detection (matches ci/jenkins/use-prebuilt-binary.sh) ─
detect_platform() {
    case "$(uname -s 2>/dev/null || echo)" in
        MINGW*|MSYS*|CYGWIN*) echo windows; return ;;
    esac
    case "${OS:-}" in Windows_NT) echo windows; return ;; esac
    case "$(uname -s 2>/dev/null || echo)" in
        Darwin) echo macos; return ;;
    esac
    echo linux
}
detect_arch() {
    case "$(uname -m 2>/dev/null || echo x86_64)" in
        aarch64|arm64) echo arm64 ;;
        *)             echo x86_64 ;;
    esac
}
PLATFORM="$(detect_platform)"
ARCH="$(detect_arch)"
info "Host: platform=${PLATFORM} arch=${ARCH}"

# Repo root is this script's parent's parent (scripts/ → repo root). Checks that
# look for dist/ or toolchain/ only run when those directories actually exist, so
# the script is equally valid dropped into an unrelated project.
SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "${SCRIPT_DIR}/.." && pwd)"

# ── Resolve an oxide-sloc binary, if one is reachable ─────────────────────────
BINARY=""
BIN_NAME="oxide-sloc"
[ "$PLATFORM" = "windows" ] && BIN_NAME="oxide-sloc.exe"
if have "$BIN_NAME"; then
    BINARY="$(command -v "$BIN_NAME")"
elif [ -x "${REPO_ROOT}/target/release/${BIN_NAME}" ]; then
    BINARY="${REPO_ROOT}/target/release/${BIN_NAME}"
fi

# Default the mode: if a binary is already present and no build dir is implied, run.
if [ -z "$MODE" ]; then
    if [ -n "$BINARY" ]; then MODE="run"; else MODE="prebuilt"; fi
fi
case "$MODE" in prebuilt|source|run) ;; *)
    echo "preflight: --mode must be prebuilt|source|run (got '$MODE')" >&2; exit 2 ;;
esac
info "Mode: ${MODE}"

# ── git (clone + hotspots/attribution) ────────────────────────────────────────
if have git; then
    ok "git present ($(git --version 2>/dev/null | head -1))"
else
    warn "git not found — cloning remote repos and git hotspots/attribution are unavailable (analyzing a local path still works)"
fi

# ── curl (webhook delivery, send, server probe) ───────────────────────────────
if have curl; then
    ok "curl present"
else
    warn "curl not found — the send subcommand's webhook delivery and --server-url probing need it"
fi

# ── Mode-specific prerequisites ───────────────────────────────────────────────
case "$MODE" in
prebuilt)
    # Extraction tooling: GNU tar is bundled with Git Bash and present on Linux.
    EXTRACTORS=""
    have tar   && EXTRACTORS="tar"
    have unzip && EXTRACTORS="${EXTRACTORS:+$EXTRACTORS }unzip"
    if [ -n "$EXTRACTORS" ]; then
        ok "archive extraction tool present (${EXTRACTORS})"
    else
        fail "no tar or unzip on PATH — cannot extract the dist/ archive. Install one, or use --mode source."
    fi
    # dist/ archive for this platform (only meaningful inside the oxide-sloc repo).
    if [ -d "${REPO_ROOT}/dist" ]; then
        case "$PLATFORM" in
            windows) DIST_GLOB="${REPO_ROOT}/dist/oxide-sloc-windows-x64.tar.gz ${REPO_ROOT}/dist/oxide-sloc-windows-x64.zip" ;;
            macos)   DIST_GLOB="${REPO_ROOT}/dist/oxide-sloc-macos-${ARCH}.tar.gz" ;;
            *)       DIST_GLOB="${REPO_ROOT}/dist/oxide-sloc-linux-${ARCH}.tar.gz" ;;
        esac
        FOUND_DIST=""
        for d in $DIST_GLOB; do [ -f "$d" ] && FOUND_DIST="$d" && break; done
        if [ -n "$FOUND_DIST" ]; then
            ok "prebuilt dist archive present ($(basename "$FOUND_DIST"))"
        else
            warn "no dist/ archive for ${PLATFORM}/${ARCH} — a prebuilt run will have to fall back to a source build"
        fi
    fi
    ;;
source)
    # A usable Rust toolchain: cargo on PATH, a bundled offline toolchain, or rustup.
    if have cargo; then
        ok "cargo present ($(cargo --version 2>/dev/null | head -1))"
    elif [ -d "${REPO_ROOT}/toolchain" ] && ls "${REPO_ROOT}"/toolchain/rust-toolchain-*.tar.gz* >/dev/null 2>&1; then
        ok "no cargo on PATH, but a bundled offline Rust toolchain is present under toolchain/"
    elif have rustup; then
        warn "no cargo on PATH but rustup is present — the toolchain will be installed on first build"
    else
        fail "source build requires a Rust toolchain: install rustc+cargo, or run inside the oxide-sloc repo with the bundled toolchain/, or use --mode prebuilt."
    fi
    # Vendored crates for a fully offline build (otherwise cargo fetches from the network).
    if ls "${REPO_ROOT}"/vendor.tar.gz.* >/dev/null 2>&1 || [ -d "${REPO_ROOT}/vendor" ]; then
        ok "vendored crate sources present (offline build supported)"
        if ! have gzip && ! have tar; then
            warn "vendor archive present but no gzip/tar to extract it"
        fi
    else
        warn "no vendored crates — the build will need network access to crates.io"
    fi
    # Free disk (a from-scratch workspace build wants a few GB).
    if have df; then
        FREE_KB="$(df -Pk "${REPO_ROOT}" 2>/dev/null | awk 'NR==2{print $4}')"
        if [ -n "${FREE_KB:-}" ]; then
            FREE_GB=$(( FREE_KB / 1024 / 1024 ))
            if [ "$FREE_GB" -lt 3 ]; then
                warn "only ${FREE_GB} GB free on the build volume — a source build may run out of space"
            else
                ok "disk space OK (${FREE_GB} GB free)"
            fi
        fi
    fi
    ;;
run)
    : # handled by the binary-resolution block below
    ;;
esac

# ── Optional: a browser for PDF export ────────────────────────────────────────
BROWSER_FOUND=""
for b in "${SLOC_BROWSER:-}" google-chrome chromium chromium-browser msedge microsoft-edge brave-browser vivaldi opera; do
    [ -n "$b" ] && have "$b" && { BROWSER_FOUND="$b"; break; }
done
if [ -z "$BROWSER_FOUND" ] && [ "$PLATFORM" = "windows" ]; then
    for p in "/c/Program Files/Google/Chrome/Application/chrome.exe" \
             "/c/Program Files (x86)/Microsoft/Edge/Application/msedge.exe"; do
        [ -x "$p" ] && { BROWSER_FOUND="$p"; break; }
    done
fi
if [ -n "$BROWSER_FOUND" ]; then
    ok "Chromium-based browser found for PDF export (${BROWSER_FOUND})"
else
    warn "no Chromium-based browser found — HTML/CSV/JSON still work; headless-Chrome PDF export will fall back to the pure-Rust writer (set SLOC_BROWSER to override)"
fi

# ── Binary presence + version ─────────────────────────────────────────────────
RESOLVED_VERSION=""
if [ -n "$BINARY" ]; then
    RAW="$("$BINARY" --version 2>/dev/null | tr -d '\r' | awk 'NF{last=$0} END{print last}')"
    RESOLVED_VERSION="$(printf '%s\n' "$RAW" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.\-]+)?' | head -1)"
    if [ -n "$RESOLVED_VERSION" ]; then
        ok "oxide-sloc binary resolved: ${BINARY} (v${RESOLVED_VERSION})"
    else
        ok "oxide-sloc binary resolved: ${BINARY}"
    fi
elif [ "$MODE" = "run" ]; then
    fail "run mode requires an oxide-sloc binary on PATH (or at target/release/) — none found."
else
    info "no oxide-sloc binary resolvable yet (it is produced by the ${MODE} step; the version pin is checked after build)"
fi

# ── Version pin ───────────────────────────────────────────────────────────────
if [ -n "$REQUIRE_VERSION" ]; then
    if [ -n "$RESOLVED_VERSION" ]; then
        if [ "$RESOLVED_VERSION" = "$REQUIRE_VERSION" ]; then
            ok "version pin satisfied: binary is ${RESOLVED_VERSION}"
        else
            fail "version pin mismatch: required ${REQUIRE_VERSION} but binary is ${RESOLVED_VERSION}"
        fi
    else
        warn "version pin ${REQUIRE_VERSION} requested but no binary is resolvable yet to verify"
    fi
fi

# ── Optional: probe a running server for reachability + compatibility ─────────
SERVER_VERSION=""
if [ -n "$SERVER_URL" ]; then
    BASE="${SERVER_URL%/}"
    if ! have curl; then
        warn "cannot probe ${BASE} — curl is not installed"
    else
        HCODE="$(curl -fsS -o /dev/null -w '%{http_code}' --max-time 8 "${BASE}/healthz" 2>/dev/null || echo 000)"
        if [ "$HCODE" = "200" ]; then
            ok "server /healthz reachable at ${BASE} (HTTP 200)"
        else
            fail "server /healthz at ${BASE} returned HTTP ${HCODE} (expected 200) — is 'oxide-sloc serve' running there?"
        fi
        VBODY="$(curl -fsS --max-time 8 "${BASE}/api/version" 2>/dev/null || echo)"
        SERVER_VERSION="$(printf '%s' "$VBODY" | grep -oE '"version"[[:space:]]*:[[:space:]]*"[^"]+"' | head -1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.\-]+)?' | head -1)"
        if [ -n "$SERVER_VERSION" ]; then
            ok "server reports oxide-sloc v${SERVER_VERSION}"
            # Compatibility: warn on a major-version skew between a local binary and the server.
            if [ -n "$RESOLVED_VERSION" ]; then
                LMAJ="${RESOLVED_VERSION%%.*}"; SMAJ="${SERVER_VERSION%%.*}"
                if [ "$LMAJ" != "$SMAJ" ]; then
                    warn "major-version skew: local v${RESOLVED_VERSION} vs server v${SERVER_VERSION} — API payloads may differ"
                fi
            fi
            if [ -n "$REQUIRE_VERSION" ] && [ "$SERVER_VERSION" != "$REQUIRE_VERSION" ]; then
                fail "server version ${SERVER_VERSION} does not match the required pin ${REQUIRE_VERSION}"
            fi
        else
            warn "could not read a version from ${BASE}/api/version"
        fi
    fi
fi

# ── Summary ───────────────────────────────────────────────────────────────────
STATUS="pass"
[ "$FAILS" -gt 0 ] && STATUS="fail"

if [ "$JSON" -eq 1 ]; then
    printf '{"status":"%s","mode":"%s","platform":"%s","arch":"%s","binary":"%s","binary_version":"%s","server_version":"%s","passes":%d,"warnings":%d,"failures":%d}\n' \
        "$STATUS" "$MODE" "$PLATFORM" "$ARCH" "$BINARY" "$RESOLVED_VERSION" "$SERVER_VERSION" "$PASSES" "$WARNS" "$FAILS"
fi

if [ "$FAILS" -gt 0 ]; then
    [ "$QUIET" -eq 1 ] || printf '\nPreflight FAILED — %d hard check(s) failed, %d warning(s). Resolve the [fail] items above.\n' "$FAILS" "$WARNS" >&2
    exit 1
fi
[ "$QUIET" -eq 1 ] || printf '\nPreflight PASSED — %d check(s) OK, %d warning(s).\n' "$PASSES" "$WARNS"
exit 0
