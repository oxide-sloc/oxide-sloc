#!/usr/bin/env bash
# Behavioural tests for ci/downstream-trigger/trigger-oxide-sloc.sh.
#
# Runs the REAL script with a stubbed `curl` on PATH so no network is used, and
# asserts the guards that must hold even when the upstream pipeline is ancient:
#   1. success gate: a non-success --status skips (exit 0) and never calls curl
#   2. success gate: recognised success tokens DO trigger a call
#   3. --always overrides the success gate
#   4. fail-closed: missing --url / --secret / --repo / --branch -> exit 2
#   5. the X-Sloc-Signature header is the correct HMAC-SHA256 of the exact body
#   6. transient 5xx is retried up to --retries then fails (exit 3)
#   7. a 4xx is a hard failure with NO retry (single call, exit 3)
#   8. dispatch github builds a repository_dispatch body + Bearer auth
#
# Run:  bash ci/downstream-trigger/tests/test-trigger-guards.sh
# Exit code 0 = all assertions passed.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT="${HERE}/../trigger-oxide-sloc.sh"

PASS=0
FAIL=0
SKIP=0
ok()   { printf '[PASS] %s\n' "$1"; PASS=$((PASS + 1)); }
bad()  { printf '[FAIL] %s\n    %s\n' "$1" "${2:-}"; FAIL=$((FAIL + 1)); }
skip() { printf '[SKIP] %s\n' "$1"; SKIP=$((SKIP + 1)); }

# ── curl stub ─────────────────────────────────────────────────────────────────
# Records every invocation's args to $CAP_ARGS (newline-delimited) and bumps a
# call counter in $CAP_COUNT, then prints the HTTP code from $STUB_HTTP so the
# script's status handling runs. Honours -o by not writing (script tolerates a
# missing body file).
STUB_DIR="$(mktemp -d)"
cat > "${STUB_DIR}/curl" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$@" >> "${CAP_ARGS}"
printf 'x' >> "${CAP_COUNT}"
printf '%s' "${STUB_HTTP:-200}"
STUB
chmod +x "${STUB_DIR}/curl"
export PATH="${STUB_DIR}:${PATH}"

reset_caps() {
    CAP_ARGS="$(mktemp)"; CAP_COUNT="$(mktemp)"; : > "${CAP_COUNT}"
    export CAP_ARGS CAP_COUNT
}
call_count() { wc -c < "${CAP_COUNT}" | tr -d ' '; }

# Independent HMAC reference (python), so the assertion is not circular with the
# script's own openssl path.
ref_hmac() { # $1=secret $2=body
    python3 - "$1" "$2" <<'PY' 2>/dev/null || python - "$1" "$2" <<'PY2'
import hmac,hashlib,sys
print(hmac.new(sys.argv[1].encode(),sys.argv[2].encode(),hashlib.sha256).hexdigest())
PY
import hmac,hashlib,sys
print(hmac.new(sys.argv[1].encode(),sys.argv[2].encode(),hashlib.sha256).hexdigest())
PY2
}

# ── 1. success gate blocks non-success ────────────────────────────────────────
reset_caps
STUB_HTTP=200 sh "${SCRIPT}" --mode server --url http://h --secret s \
    --repo r --branch main --status FAILURE --build-id 9 >/dev/null 2>&1
rc=$?
if [ "$rc" -eq 0 ] && [ "$(call_count)" -eq 0 ]; then
    ok "non-success status skips without any network call"
else
    bad "non-success status should skip" "rc=$rc calls=$(call_count)"
fi

# ── 2. success tokens trigger a call ──────────────────────────────────────────
for st in success SUCCESS passed ok green 0 true; do
    reset_caps
    STUB_HTTP=200 sh "${SCRIPT}" --mode server --url http://h --secret s \
        --repo r --branch main --status "$st" --build-id 9 >/dev/null 2>&1
    rc=$?
    if [ "$rc" -eq 0 ] && [ "$(call_count)" -eq 1 ]; then
        ok "success token '$st' triggers exactly one call"
    else
        bad "success token '$st' should trigger" "rc=$rc calls=$(call_count)"
    fi
done

# ── 3. --always overrides the gate ────────────────────────────────────────────
reset_caps
STUB_HTTP=200 sh "${SCRIPT}" --mode server --url http://h --secret s \
    --repo r --branch main --status FAILURE --build-id 9 --always >/dev/null 2>&1
if [ "$(call_count)" -eq 1 ]; then
    ok "--always triggers even on a failed upstream status"
else
    bad "--always should override the gate" "calls=$(call_count)"
fi

# ── 4. fail-closed on missing required config ─────────────────────────────────
check_die() { # desc, args...
    desc="$1"; shift
    reset_caps
    STUB_HTTP=200 sh "${SCRIPT}" "$@" >/dev/null 2>&1
    rc=$?
    if [ "$rc" -eq 2 ] && [ "$(call_count)" -eq 0 ]; then
        ok "$desc"
    else
        bad "$desc" "rc=$rc calls=$(call_count)"
    fi
}
check_die "missing --url dies (exit 2)"    --mode server --secret s --repo r --branch main --build-id 9
check_die "missing --secret dies (exit 2)" --mode server --url http://h --repo r --branch main --build-id 9
check_die "missing --repo dies (exit 2)"   --mode server --url http://h --secret s --branch main --build-id 9
check_die "missing --branch dies (exit 2)" --mode server --url http://h --secret s --repo r --build-id 9

# ── 5. signature is the correct HMAC of the exact body ────────────────────────
reset_caps
STUB_HTTP=200 sh "${SCRIPT}" --mode server --url http://h --secret topsecret \
    --repo https://x/y.git --branch main --commit abc \
    --system jenkins --job api --build-id 1234 --build-url http://ci/1234 >/dev/null 2>&1
# Extract the signed body (value passed after --data) and the header value.
body="$(grep -A1 -- '--data' "${CAP_ARGS}" | tail -1)"
sig_line="$(grep -F 'X-Sloc-Signature:' "${CAP_ARGS}")"
sent_hex="${sig_line#X-Sloc-Signature: sha256=}"
if command -v python3 >/dev/null 2>&1 || command -v python >/dev/null 2>&1; then
    want_hex="$(ref_hmac topsecret "$body")"
    if [ -n "$sent_hex" ] && [ "$sent_hex" = "$want_hex" ]; then
        ok "X-Sloc-Signature matches HMAC-SHA256 of the exact body"
    else
        bad "signature mismatch" "sent=$sent_hex want=$want_hex body=$body"
    fi
else
    skip "signature parity (python not available for reference HMAC)"
fi

# ── 6. transient 5xx is retried then fails ────────────────────────────────────
reset_caps
STUB_HTTP=503 sh "${SCRIPT}" --mode server --url http://h --secret s \
    --repo r --branch main --build-id 9 --retries 2 --timeout 1 >/dev/null 2>&1
rc=$?
if [ "$rc" -eq 3 ] && [ "$(call_count)" -eq 2 ]; then
    ok "5xx retried up to --retries then exits 3"
else
    bad "5xx retry behaviour wrong" "rc=$rc calls=$(call_count) (want rc=3 calls=2)"
fi

# ── 7. 4xx is a hard failure with no retry ────────────────────────────────────
reset_caps
STUB_HTTP=400 sh "${SCRIPT}" --mode server --url http://h --secret s \
    --repo r --branch main --build-id 9 --retries 5 --timeout 1 >/dev/null 2>&1
rc=$?
if [ "$rc" -eq 3 ] && [ "$(call_count)" -eq 1 ]; then
    ok "4xx fails fast with no retry"
else
    bad "4xx should not retry" "rc=$rc calls=$(call_count) (want rc=3 calls=1)"
fi

# ── 8. dispatch github shape ──────────────────────────────────────────────────
reset_caps
STUB_HTTP=204 sh "${SCRIPT}" --mode dispatch --dispatch github \
    --url https://api.github.com/repos/o/r/dispatches --token ghp_xxx \
    --repo https://x/y.git --branch main --build-id 9 >/dev/null 2>&1
args="$(cat "${CAP_ARGS}")"
if printf '%s' "$args" | grep -q 'Authorization: Bearer ghp_xxx' \
   && printf '%s' "$args" | grep -q '"event_type":"oxide-sloc-scan"'; then
    ok "dispatch github sends repository_dispatch body with Bearer auth"
else
    bad "dispatch github payload/auth wrong" "$args"
fi

# ── summary ───────────────────────────────────────────────────────────────────
printf '\n%d passed, %d failed, %d skipped\n' "$PASS" "$FAIL" "$SKIP"
[ "$FAIL" -eq 0 ]
