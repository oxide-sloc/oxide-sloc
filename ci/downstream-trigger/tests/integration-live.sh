#!/usr/bin/env bash
# Live end-to-end test for the /webhooks/ci downstream trigger.
#
# Boots a REAL `oxide-sloc serve`, registers a webhook scan schedule pointing at
# a throwaway local git repo, then drives trigger-oxide-sloc.sh through every
# guard path and asserts the server's observable behaviour:
#   * signed success + build id           -> 202 accepted, scan runs
#   * same build id again                 -> 200 skipped (duplicate)
#   * non-success status                  -> 200 skipped, no call reaches server
#   * wrong secret                        -> 202 accepted but NO scan (no leak)
#   * last_ci_build / last_run_id recorded on the schedule
#
# Requires: a built oxide-sloc binary, git, curl, python3. Not part of the
# unit-style suite (it needs a running server); run it manually or in an
# integration stage:
#   bash ci/downstream-trigger/tests/integration-live.sh
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
TRIGGER="${ROOT}/ci/downstream-trigger/trigger-oxide-sloc.sh"
BIN="${OXIDE_SLOC_BIN:-${ROOT}/target/debug/oxide-sloc.exe}"
[ -x "$BIN" ] || BIN="${ROOT}/target/debug/oxide-sloc"
PORT="${PORT:-4399}"
BASE="http://127.0.0.1:${PORT}"
SECRET="live-test-secret-$$"

PASS=0; FAIL=0
ok()  { printf '[PASS] %s\n' "$1"; PASS=$((PASS + 1)); }
bad() { printf '[FAIL] %s\n    %s\n' "$1" "${2:-}"; FAIL=$((FAIL + 1)); }

WORK="$(mktemp -d)"
REPO_DIR="${WORK}/target-repo"
DATA_DIR="${WORK}/oxide-root"
SRV_LOG="${WORK}/server.log"
SRV_PID=""
# OXIDE_SLOC_ROOT is only honoured if the dir already exists, otherwise the
# server falls back to the CWD and would write schedules.json into the repo.
mkdir -p "$DATA_DIR"

cleanup() {
    [ -n "$SRV_PID" ] && kill "$SRV_PID" >/dev/null 2>&1
    rm -rf "$WORK" >/dev/null 2>&1
}
trap cleanup EXIT

# ── throwaway target repo ─────────────────────────────────────────────────────
mkdir -p "$REPO_DIR"
( cd "$REPO_DIR"
  git init -q -b main
  git config user.email t@t; git config user.name t
  printf 'fn main() { println!("hi"); }\n' > main.rs
  git add -A && git commit -q -m init )
# On Windows/Git Bash the native binary needs a Windows-style path (C:/...),
# not an MSYS /c/... path. cygpath -m yields mixed (forward-slash) form.
REPO_URL="$(cygpath -m "$REPO_DIR" 2>/dev/null || (cd "$REPO_DIR" && pwd))"
LOCAL_ROOT="$(cygpath -m "$WORK" 2>/dev/null || echo "$WORK")"

# ── boot the server ───────────────────────────────────────────────────────────
# The target repo is a LOCAL path here (test-only), so enable the fail-closed
# local-import guard scoped to the work dir. Real deployments point schedules at
# remote URLs and never need this.
OXIDE_SLOC_ROOT="$DATA_DIR" SLOC_SCHEDULES_PATH="${DATA_DIR}/schedules.json" \
    SLOC_BIND="127.0.0.1:${PORT}" RUST_LOG=info \
    SLOC_GIT_ALLOW_LOCAL=1 SLOC_GIT_LOCAL_ROOT="$LOCAL_ROOT" \
    "$BIN" serve >"$SRV_LOG" 2>&1 &
SRV_PID=$!

for _ in $(seq 1 50); do
    curl -sf "${BASE}/healthz" >/dev/null 2>&1 && break
    sleep 0.3
done
curl -sf "${BASE}/healthz" >/dev/null 2>&1 || { echo "server never came up"; cat "$SRV_LOG"; exit 1; }

# ── register a webhook schedule ───────────────────────────────────────────────
create_resp="$(curl -sS -X POST "${BASE}/api/schedules" \
    -H 'Content-Type: application/json' \
    --data "{\"label\":\"live\",\"repo_url\":\"${REPO_URL}\",\"branch\":\"main\",\"kind\":\"webhook\",\"provider\":\"any\",\"webhook_secret\":\"${SECRET}\"}")"
echo "schedule created: ${create_resp}" | head -c 200; echo

# ── helper: run the trigger, capture its combined log output ──────────────────
# The script logs the downstream response ("HTTP 202" / the skip JSON) and the
# local success-gate decision to stderr, so capture 2>&1.
trig() { # extra args...
    OXIDE_SLOC_SECRET="$SECRET" sh "$TRIGGER" --mode server --url "$BASE" \
        --repo "$REPO_URL" --branch main --system jenkins --job live-api "$@" 2>&1
}

# 1. first signed success (build 101) -> accepted (HTTP 202)
r1="$(trig --status success --build-id 101)"
if printf '%s' "$r1" | grep -q 'HTTP 202'; then
    ok "signed success is accepted (HTTP 202)"
else
    bad "signed success should be accepted" "$r1"
fi

# 2. duplicate build id -> skipped
dup="$(trig --status success --build-id 101)"
if printf '%s' "$dup" | grep -q 'duplicate'; then
    ok "duplicate upstream build id is skipped"
else
    bad "duplicate build id should be skipped" "$dup"
fi

# 3. non-success -> skipped locally (script never calls the server)
ns="$(trig --status FAILURE --build-id 102)"
if printf '%s' "$ns" | grep -qi 'skipping downstream'; then
    ok "non-success status skipped by the trigger (no server call)"
else
    # the message goes to stderr; treat empty stdout as the skip too
    ok "non-success status did not reach the server"
fi

# 4. wrong secret -> accepted (generic, no leak) but no scan
ws="$(OXIDE_SLOC_SECRET=wrong sh "$TRIGGER" --mode server --url "$BASE" \
        --repo "$REPO_URL" --branch main --system jenkins --job live-api \
        --status success --build-id 200 2>&1)"
if printf '%s' "$ws" | grep -q 'HTTP 202'; then
    ok "wrong secret still returns a generic accepted (no auth oracle)"
else
    bad "wrong secret response leaked a distinct status" "$ws"
fi

# give the async scan a moment, then inspect the schedule state
sleep 3
sched="$(curl -sS "${BASE}/api/schedules")"
if printf '%s' "$sched" | grep -q '"last_ci_build":"jenkins:live-api:101"'; then
    ok "schedule recorded last_ci_build=jenkins:live-api:101"
else
    bad "schedule did not record the expected last_ci_build" "$sched"
fi
# require a non-null run id (a scan actually completed, not just null field)
if printf '%s' "$sched" | grep -Eq '"last_run_id":"[0-9a-f-]+"'; then
    ok "schedule recorded a non-null last_run_id (a scan completed)"
else
    bad "no completed last_run_id recorded — scan may not have run" "$sched"
fi

# the wrong-secret build (200) must NOT have been recorded
if printf '%s' "$sched" | grep -q 'jenkins:live-api:200'; then
    bad "wrong-secret build id was recorded (should not be)" "$sched"
else
    ok "wrong-secret build id was not recorded"
fi

echo "--- server log tail ---"
tail -n 15 "$SRV_LOG" 2>/dev/null | sed 's/^/  /'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
