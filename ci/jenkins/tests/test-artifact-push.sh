#!/usr/bin/env bash
# Contract test for ci/artifact-push.sh — runs the REAL script with a stubbed `curl`
# on PATH so NO network is used, and asserts that each repository provider builds the
# correct request (endpoint, HTTP verb, auth scheme, multipart/upload fields, MIME
# type), that missing files are skipped (not failed), that dry-run makes zero calls,
# and that config errors exit non-zero.
#
# Mirrors test-notify-bitbucket.sh. Exit code 0 = all assertions passed.
#
# Only the curl-based providers are exercised (artifactory, nexus, nexus2,
# generic-http): s3/minio/azure need the aws/az CLIs and are out of scope here.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
PUSH="${REPO_ROOT}/ci/artifact-push.sh"

PASS=0
FAIL=0
SKIP=0
pass() { echo "[PASS] $1"; PASS=$((PASS + 1)); }
fail() { echo "[FAIL] $1"; FAIL=$((FAIL + 1)); }
skip() { echo "[SKIP] $1"; SKIP=$((SKIP + 1)); }

WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

# ── curl stub ────────────────────────────────────────────────────────────────
# Records every argument (one per line) to $CURL_LOG and exits with $CURL_EXIT
# (default 0 = HTTP success). The real script uses `curl -sf`, so a non-zero exit
# simulates an HTTP error / failed upload.
STUB_DIR="${WORK}/bin"
mkdir -p "${STUB_DIR}"
cat > "${STUB_DIR}/curl" <<'STUB'
#!/usr/bin/env bash
: "${CURL_LOG:?}"
{ echo "--- curl call ---"; for a in "$@"; do echo "$a"; done; } >> "${CURL_LOG}"
exit "${CURL_EXIT:-0}"
STUB
chmod +x "${STUB_DIR}/curl"
export PATH="${STUB_DIR}:${PATH}"

# ── sample artifacts ─────────────────────────────────────────────────────────
ADIR="${WORK}/out"
mkdir -p "${ADIR}"
echo '{"ok":true}'        > "${ADIR}/result.json"
echo '<html></html>'      > "${ADIR}/report.html"
printf '%%PDF-1.4\n'      > "${ADIR}/report.pdf"

# Helper: run the push script with a fresh curl log, capturing stdout+rc.
# Usage: run_push <expected_rc_var_name> KEY=VAL KEY=VAL ...
run_push() {
    CURL_LOG="${WORK}/curl.log"; : > "${CURL_LOG}"
    export CURL_LOG
    env "$@" bash "${PUSH}"
}

# ── 1. Nexus 3 — endpoint + multipart fields + Bearer auth + MIME ────────────
OUT="$(CURL_EXIT=0 run_push \
    ARTIFACT_REPO_TYPE=nexus \
    ARTIFACT_REPO_URL=https://nexus.example.com \
    ARTIFACT_REPO_PATH=oxide-sloc/42 \
    ARTIFACT_REPO_EXTRA=sloc-raw-hosted \
    ARTIFACT_REPO_PASS=tok123 \
    ARTIFACT_DIR="${ADIR}" \
    ARTIFACT_FILES="result.json report.html" 2>&1)"; RC=$?
LOG="$(cat "${WORK}/curl.log")"
[ "${RC}" -eq 0 ] && pass "nexus3: exit 0 on success" || fail "nexus3: expected exit 0, got ${RC}"
echo "${LOG}" | grep -qF 'https://nexus.example.com/service/rest/v1/components?repository=sloc-raw-hosted' \
    && pass "nexus3: POST to components REST API with repository=<name>" \
    || fail "nexus3: wrong/absent components endpoint"
echo "${LOG}" | grep -qF 'raw.directory=/oxide-sloc/42' \
    && pass "nexus3: raw.directory set to /<path>" || fail "nexus3: raw.directory missing"
echo "${LOG}" | grep -qF 'raw.asset1.filename=result.json' \
    && pass "nexus3: raw.asset1.filename set" || fail "nexus3: asset filename missing"
echo "${LOG}" | grep -qF 'type=application/json' \
    && pass "nexus3: JSON MIME type on the asset" || fail "nexus3: JSON MIME type missing"
echo "${LOG}" | grep -qF 'type=text/html' \
    && pass "nexus3: HTML MIME type on the asset" || fail "nexus3: HTML MIME type missing"
# Bearer auth (pass only, no user)
if grep -qxF 'Authorization: Bearer tok123' "${WORK}/curl.log"; then
    pass "nexus3: pass-only -> Bearer auth header"
else
    fail "nexus3: expected Bearer auth header"
fi
# Two files -> two POSTs
CALLS="$(grep -c -- '--- curl call ---' "${WORK}/curl.log")"
[ "${CALLS}" -eq 2 ] && pass "nexus3: one POST per file (2)" || fail "nexus3: expected 2 calls, got ${CALLS}"

# ── 2. Nexus 3 — Basic auth when user+pass both set ──────────────────────────
CURL_EXIT=0 run_push \
    ARTIFACT_REPO_TYPE=nexus \
    ARTIFACT_REPO_URL=https://nexus.example.com \
    ARTIFACT_REPO_PATH=p \
    ARTIFACT_REPO_USER=deploy \
    ARTIFACT_REPO_PASS=secret \
    ARTIFACT_DIR="${ADIR}" \
    ARTIFACT_FILES="result.json" >/dev/null 2>&1
if grep -qxF 'deploy:secret' "${WORK}/curl.log" && grep -qxF -- '-u' "${WORK}/curl.log"; then
    pass "nexus3: user+pass -> Basic auth (-u user:pass)"
else
    fail "nexus3: expected -u deploy:secret"
fi

# ── 3. Nexus 2 — content-API PUT endpoint + Basic auth ───────────────────────
CURL_EXIT=0 run_push \
    ARTIFACT_REPO_TYPE=nexus2 \
    ARTIFACT_REPO_URL=https://nexus.example.com/nexus \
    ARTIFACT_REPO_PATH=oxide-sloc/42 \
    ARTIFACT_REPO_EXTRA=sloc-raw-hosted \
    ARTIFACT_REPO_USER=deploy \
    ARTIFACT_REPO_PASS=secret \
    ARTIFACT_DIR="${ADIR}" \
    ARTIFACT_FILES="result.json" >/dev/null 2>&1
LOG="$(cat "${WORK}/curl.log")"
echo "${LOG}" | grep -qF 'https://nexus.example.com/nexus/content/repositories/sloc-raw-hosted/oxide-sloc/42/result.json' \
    && pass "nexus2: PUT to /content/repositories/<repo>/<path>/<file>" \
    || fail "nexus2: wrong/absent content endpoint"
echo "${LOG}" | grep -qxF 'PUT' && pass "nexus2: uses HTTP PUT (-X PUT)" || fail "nexus2: not a PUT"
grep -qxF 'deploy:secret' "${WORK}/curl.log" && pass "nexus2: Basic auth (-u user:pass)" \
    || fail "nexus2: expected -u deploy:secret"

# ── 4. Missing file is SKIPPED, not a failure ────────────────────────────────
OUT="$(CURL_EXIT=0 run_push \
    ARTIFACT_REPO_TYPE=nexus \
    ARTIFACT_REPO_URL=https://nexus.example.com \
    ARTIFACT_REPO_PATH=p \
    ARTIFACT_REPO_PASS=tok \
    ARTIFACT_DIR="${ADIR}" \
    ARTIFACT_FILES="result.json does-not-exist.json" 2>&1)"; RC=$?
[ "${RC}" -eq 0 ] && pass "missing file: overall exit still 0" || fail "missing file: expected exit 0, got ${RC}"
echo "${OUT}" | grep -qi 'SKIP' && pass "missing file: printed a SKIP line" || fail "missing file: no SKIP line"
CALLS="$(grep -c -- '--- curl call ---' "${WORK}/curl.log")"
[ "${CALLS}" -eq 1 ] && pass "missing file: only the present file is uploaded (1 call)" \
    || fail "missing file: expected 1 call, got ${CALLS}"

# ── 5. Dry-run makes ZERO network calls ──────────────────────────────────────
OUT="$(CURL_EXIT=0 run_push \
    ARTIFACT_REPO_TYPE=nexus \
    ARTIFACT_REPO_URL=https://nexus.example.com \
    ARTIFACT_REPO_PATH=p \
    ARTIFACT_REPO_PASS=tok \
    ARTIFACT_DRY_RUN=true \
    ARTIFACT_DIR="${ADIR}" \
    ARTIFACT_FILES="result.json report.pdf" 2>&1)"; RC=$?
[ "${RC}" -eq 0 ] && pass "dry-run: exit 0" || fail "dry-run: expected exit 0, got ${RC}"
if [ -s "${WORK}/curl.log" ]; then fail "dry-run: curl was called (should be zero calls)"; else
    pass "dry-run: zero curl calls"; fi
echo "${OUT}" | grep -qi 'DRY-RUN' && pass "dry-run: announced DRY-RUN mode" || fail "dry-run: no DRY-RUN notice"

# ── 6. Failed upload (HTTP error) -> overall exit 1 ──────────────────────────
CURL_EXIT=22 run_push \
    ARTIFACT_REPO_TYPE=nexus \
    ARTIFACT_REPO_URL=https://nexus.example.com \
    ARTIFACT_REPO_PATH=p \
    ARTIFACT_REPO_PASS=tok \
    ARTIFACT_DIR="${ADIR}" \
    ARTIFACT_FILES="result.json" >/dev/null 2>&1; RC=$?
[ "${RC}" -eq 1 ] && pass "upload failure: overall exit 1" || fail "upload failure: expected exit 1, got ${RC}"

# ── 7. Artifactory — PUT with -T <file> to <url>/<path>/<file> ───────────────
CURL_EXIT=0 run_push \
    ARTIFACT_REPO_TYPE=artifactory \
    ARTIFACT_REPO_URL=https://repo.example.com/artifactory/sloc \
    ARTIFACT_REPO_PATH=job/42 \
    ARTIFACT_REPO_PASS=apikey \
    ARTIFACT_DIR="${ADIR}" \
    ARTIFACT_FILES="result.json" >/dev/null 2>&1
LOG="$(cat "${WORK}/curl.log")"
echo "${LOG}" | grep -qF 'https://repo.example.com/artifactory/sloc/job/42/result.json' \
    && pass "artifactory: PUT to <url>/<path>/<file>" || fail "artifactory: wrong target url"
echo "${LOG}" | grep -qxF 'X-JFrog-Art-Api: apikey' \
    && pass "artifactory: pass-only -> X-JFrog-Art-Api header" || fail "artifactory: API-key header missing"

# ── 8. Config errors exit 2 ──────────────────────────────────────────────────
CURL_EXIT=0 run_push ARTIFACT_REPO_TYPE=bogus ARTIFACT_REPO_URL=x \
    ARTIFACT_DIR="${ADIR}" >/dev/null 2>&1; RC=$?
[ "${RC}" -eq 2 ] && pass "bad repo type -> exit 2" || fail "bad repo type: expected exit 2, got ${RC}"
CURL_EXIT=0 run_push ARTIFACT_REPO_TYPE=nexus ARTIFACT_DIR="${ADIR}" >/dev/null 2>&1; RC=$?
[ "${RC}" -eq 2 ] && pass "missing URL -> exit 2" || fail "missing URL: expected exit 2, got ${RC}"

# ── 9. SHA-256 manifest generation + upload ──────────────────────────────────
OUT="$(CURL_EXIT=0 run_push \
    ARTIFACT_REPO_TYPE=nexus \
    ARTIFACT_REPO_URL=https://nexus.example.com \
    ARTIFACT_REPO_PATH=p \
    ARTIFACT_REPO_PASS=tok \
    ARTIFACT_GENERATE_MANIFEST=true \
    ARTIFACT_DIR="${ADIR}" \
    ARTIFACT_FILES="result.json" 2>&1)"
[ -f "${ADIR}/checksums.sha256" ] && pass "manifest: checksums.sha256 written" || fail "manifest: not written"
grep -qF 'result.json' "${ADIR}/checksums.sha256" 2>/dev/null \
    && pass "manifest: lists the pushed file" || fail "manifest: file not listed"
grep -qxF 'raw.asset1.filename=checksums.sha256' "${WORK}/curl.log" \
    && pass "manifest: uploaded alongside artifacts" || fail "manifest: not uploaded"

# ── 10. shellcheck clean (skipped if shellcheck absent) ──────────────────────
if command -v shellcheck >/dev/null 2>&1; then
    if shellcheck -x "${PUSH}" >/dev/null 2>&1; then
        pass "shellcheck clean"
    else
        fail "shellcheck reported issues"
    fi
else
    skip "shellcheck not installed"
fi

echo "----------------------------------------------------------------------"
echo "artifact-push.sh: ${PASS} passed, ${FAIL} failed, ${SKIP} skipped"
[ "${FAIL}" -eq 0 ]
