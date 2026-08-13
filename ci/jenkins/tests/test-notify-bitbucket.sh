#!/usr/bin/env bash
# Behavioural tests for ci/jenkins/notify-bitbucket.sh.
#
# Runs the REAL script with a stubbed `curl` on PATH so no network is used, and
# asserts:
#   1. Jenkins-ish states map to the Bitbucket vocabulary
#      (SUCCESS/PASSED -> SUCCESSFUL, FAILURE/FAILED -> FAILED, other -> INPROGRESS)
#   2. Cloud vs Server/DC endpoint selection is correct
#   3. Auth scheme: BITBUCKET_USER set -> Basic (user=...), else Bearer
#   4. The JSON payload is valid JSON even when fields contain quotes/backslashes
#   5. Missing base URL / token / commit is a non-fatal skip (exit 0)
#   6. Cloud without workspace/repo is a non-fatal skip (exit 0)
#   7. (optional) shellcheck passes when the tool is installed
#
# Run:  bash ci/jenkins/tests/test-notify-bitbucket.sh
# Exit code 0 = all assertions passed.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT="${HERE}/../notify-bitbucket.sh"

PASS=0
FAIL=0
SKIP=0
ok()   { printf '[PASS] %s\n' "$1"; PASS=$((PASS + 1)); }
bad()  { printf '[FAIL] %s\n    %s\n' "$1" "${2:-}"; FAIL=$((FAIL + 1)); }
skip() { printf '[SKIP] %s\n' "$1"; SKIP=$((SKIP + 1)); }

# ── curl stub ────────────────────────────────────────────────────────────────
# Captures args to $CAP_ARGS and stdin (the -K config with the auth line) to
# $CAP_STDIN, then prints an HTTP code so the script's success path runs.
STUB_DIR="$(mktemp -d)"
cat > "${STUB_DIR}/curl" <<'STUB'
#!/usr/bin/env bash
{ printf '%s\0' "$@"; } > "${CAP_ARGS}"
cat > "${CAP_STDIN}"
printf '%s' "${STUB_HTTP:-204}"
STUB
chmod +x "${STUB_DIR}/curl"
export PATH="${STUB_DIR}:${PATH}"

run() {
    # run <state> ; env vars are passed by the caller via `env NAME=..`
    CAP_ARGS="$(mktemp)"; CAP_STDIN="$(mktemp)"
    export CAP_ARGS CAP_STDIN
    OUT="$(bash "${SCRIPT}" "$1" 2>&1)"; RC=$?
    ARGS="$(tr '\0' '\n' < "${CAP_ARGS}")"
    STDIN_CAP="$(cat "${CAP_STDIN}")"
}

BASE_SERVER="https://bitbucket.corp.example"
BASE_CLOUD="https://api.bitbucket.org"

# ── 1. state mapping ─────────────────────────────────────────────────────────
for pair in "SUCCESS:SUCCESSFUL" "PASSED:SUCCESSFUL" "SUCCESSFUL:SUCCESSFUL" \
            "FAILURE:FAILED" "FAILED:FAILED" "FAIL:FAILED" \
            "INPROGRESS:INPROGRESS" "weird:INPROGRESS"; do
    inp="${pair%%:*}"; want="${pair##*:}"
    BITBUCKET_BASE_URL="${BASE_SERVER}" BITBUCKET_TOKEN="t" GIT_COMMIT="deadbeefcafe" run "${inp}"
    if printf '%s' "${STDIN_CAP}" >/dev/null && printf '%s\n' "${ARGS}" | grep -q -- "--data"; then
        payload="$(printf '%s\n' "${ARGS}" | grep -A1 -- '--data' | tail -1)"
        got="$(printf '%s' "${payload}" | python3 -c 'import json,sys; print(json.load(sys.stdin)["state"])' 2>/dev/null)"
        if [ "${got}" = "${want}" ]; then ok "state ${inp} -> ${want}"; else bad "state ${inp} -> ${want}" "got ${got}"; fi
    else
        bad "state ${inp} -> ${want}" "no --data captured"
    fi
done

# ── 2. Server/DC endpoint selection ──────────────────────────────────────────
BITBUCKET_BASE_URL="${BASE_SERVER}" BITBUCKET_TOKEN="t" GIT_COMMIT="abcdef123456" run SUCCESS
if printf '%s\n' "${ARGS}" | grep -q "${BASE_SERVER}/rest/build-status/1.0/commits/abcdef123456"; then
    ok "Server/DC endpoint = /rest/build-status/1.0/commits/<sha>"
else
    bad "Server/DC endpoint" "$(printf '%s\n' "${ARGS}" | tail -1)"
fi

# ── 3. Cloud endpoint selection + Basic auth when BITBUCKET_USER set ─────────
BITBUCKET_BASE_URL="${BASE_CLOUD}" BITBUCKET_TOKEN="app-pw" BITBUCKET_USER="me@x.com" \
    BITBUCKET_WORKSPACE="acme" BITBUCKET_REPO="widgets" GIT_COMMIT="abcdef123456" run SUCCESS
if printf '%s\n' "${ARGS}" | grep -q "${BASE_CLOUD}/2.0/repositories/acme/widgets/commit/abcdef123456/statuses/build"; then
    ok "Cloud endpoint = /2.0/repositories/<ws>/<repo>/commit/<sha>/statuses/build"
else
    bad "Cloud endpoint" "$(printf '%s\n' "${ARGS}" | tail -1)"
fi
if printf '%s' "${STDIN_CAP}" | grep -q 'user = "me@x.com:app-pw"'; then
    ok "Cloud with BITBUCKET_USER -> Basic auth (user=...)"
else
    bad "Cloud Basic auth" "stdin was: ${STDIN_CAP}"
fi

# ── 4. Bearer auth when BITBUCKET_USER is blank ──────────────────────────────
BITBUCKET_BASE_URL="${BASE_SERVER}" BITBUCKET_TOKEN="pat123" GIT_COMMIT="abcdef123456" run SUCCESS
if printf '%s' "${STDIN_CAP}" | grep -q 'header = "Authorization: Bearer pat123"'; then
    ok "No BITBUCKET_USER -> Bearer auth"
else
    bad "Bearer auth" "stdin was: ${STDIN_CAP}"
fi

# ── 5. JSON validity with hostile characters in BUILD_NAME ───────────────────
BITBUCKET_BASE_URL="${BASE_SERVER}" BITBUCKET_TOKEN="t" GIT_COMMIT="abcdef123456" \
    BUILD_NAME='has "quote" and \backslash and
newline' run SUCCESS
payload="$(printf '%s\n' "${ARGS}" | grep -A1 -- '--data' | tail -1)"
if printf '%s' "${payload}" | python3 -c 'import json,sys; json.load(sys.stdin)' 2>/dev/null; then
    ok "payload stays valid JSON with quotes/backslash/newline in a field"
else
    bad "payload JSON validity" "payload: ${payload}"
fi

# ── 6. non-fatal skips (exit 0) ──────────────────────────────────────────────
OUT="$(BITBUCKET_TOKEN="t" GIT_COMMIT="x" bash "${SCRIPT}" SUCCESS 2>&1)"; RC=$?
if [ "${RC}" -eq 0 ] && printf '%s' "${OUT}" | grep -q "not configured"; then
    ok "missing base URL -> 'not configured' skip, exit 0"
else
    bad "missing base URL skip" "rc=${RC} out=${OUT}"
fi

OUT="$(BITBUCKET_BASE_URL="${BASE_CLOUD}" BITBUCKET_TOKEN="t" GIT_COMMIT="x" bash "${SCRIPT}" SUCCESS 2>&1)"; RC=$?
if [ "${RC}" -eq 0 ] && printf '%s' "${OUT}" | grep -q "Cloud needs"; then
    ok "Cloud without workspace/repo -> skip, exit 0"
else
    bad "Cloud missing ws/repo skip" "rc=${RC} out=${OUT}"
fi

# ── 7. shellcheck (only when installed) ──────────────────────────────────────
if command -v shellcheck >/dev/null 2>&1; then
    if shellcheck -x "${SCRIPT}"; then ok "shellcheck clean"; else bad "shellcheck" "see output above"; fi
else
    skip "shellcheck not installed"
fi

printf -- '----------------------------------------------------------------------\n'
printf 'notify-bitbucket.sh: %d passed, %d failed, %d skipped\n' "${PASS}" "${FAIL}" "${SKIP}"
[ "${FAIL}" -eq 0 ]
