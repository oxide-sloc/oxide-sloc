#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>
#
# trigger-oxide-sloc.sh — portable downstream trigger for oxide-sloc.
#
# Drop this into the FINAL step of any upstream build so that, when the build
# finishes, oxide-sloc runs a scan of the built repo. It is deliberately written
# for the lowest common denominator: POSIX /bin/sh, curl, and either openssl or
# python for the HMAC. No plugins, no bash-isms, no modern CI features required —
# it runs unchanged inside a 10-year-old Jenkins freestyle job, a GitLab shell
# runner, a Bitbucket step, or a GitHub Actions run.
#
# Two modes:
#   server   (default) POST a signed build-complete event to a running
#            `oxide-sloc serve` instance at $OXIDE_SLOC_URL/webhooks/ci.
#   dispatch trigger a downstream CI pipeline (github|gitlab|jenkins|bitbucket)
#            that itself runs `oxide-sloc analyze`.
#
# The guards live HERE, in the caller, so they hold no matter how old the
# upstream pipeline is:
#   * success gate  — only fires when the upstream result is a success
#   * HMAC signing  — server mode signs the exact body with a shared secret
#   * idempotency   — sends a stable <system>:<job>:<build_id> key; the server
#                     de-duplicates repeat deliveries of the same build
#   * retry/backoff — survives a transient network blip or a downstream restart
#   * fail-closed   — refuses to run with missing/blank required configuration
#
# Exit codes: 0 = triggered OR intentionally skipped (guard declined);
#             2 = misconfiguration; 3 = trigger failed after all retries.

set -eu

PROG=$(basename "$0")

# ── defaults (env first, flags override) ──────────────────────────────────────
MODE="${OXIDE_SLOC_MODE:-server}"
URL="${OXIDE_SLOC_URL:-}"
SECRET="${OXIDE_SLOC_SECRET:-}"
REPO="${OXIDE_SLOC_REPO:-}"
BRANCH="${OXIDE_SLOC_BRANCH:-}"
COMMIT="${OXIDE_SLOC_COMMIT:-}"
STATUS="${OXIDE_SLOC_STATUS:-success}"
SYSTEM="${OXIDE_SLOC_SYSTEM:-}"
JOB="${OXIDE_SLOC_JOB:-}"
BUILD_ID="${OXIDE_SLOC_BUILD_ID:-}"
BUILD_URL="${OXIDE_SLOC_BUILD_URL:-}"
DISPATCH="${OXIDE_SLOC_DISPATCH:-}"
TOKEN="${OXIDE_SLOC_TOKEN:-}"
RETRIES="${OXIDE_SLOC_RETRIES:-4}"
TIMEOUT="${OXIDE_SLOC_TIMEOUT:-30}"
EVENT_TYPE="${OXIDE_SLOC_EVENT_TYPE:-oxide-sloc-scan}"
INSECURE=0
ALWAYS=0
DRY_RUN=0

usage() {
    cat <<EOF
$PROG — trigger a downstream oxide-sloc scan when an upstream build finishes.

Usage: $PROG [options]

Common:
  --repo URL           repository that was built (required)
  --branch NAME        branch that was built (required)
  --commit SHA         exact commit to scan (optional; defaults to branch tip)
  --status VALUE       upstream result; non-success values are skipped
                       (default: success)
  --system NAME        upstream CI system (jenkins|gitlab|github|bitbucket|...)
  --job NAME           upstream job/pipeline name
  --build-id ID        upstream build number/id (enables de-duplication)
  --build-url URL      link back to the upstream build (for logs)
  --retries N          attempts before giving up (default: 4)
  --timeout SECONDS    per-attempt curl timeout (default: 30)
  --always             ignore the success gate and trigger regardless
  --dry-run            print what would be sent; do not call the network
  -h, --help           this help

server mode (default):
  --mode server
  --url URL            base URL of the oxide-sloc server (required)
  --secret VALUE       shared HMAC secret matching a scan schedule (required)
  --insecure           allow untrusted TLS (curl -k) — last resort only

dispatch mode:
  --mode dispatch
  --dispatch KIND      github | gitlab | jenkins | bitbucket
  --url URL            downstream trigger endpoint (see README per KIND)
  --token VALUE        downstream auth token / PAT (required)
  --event-type NAME    github repository_dispatch event type
                       (default: oxide-sloc-scan)

Every option has an OXIDE_SLOC_* environment equivalent (e.g. OXIDE_SLOC_URL).
EOF
}

die() { echo "$PROG: $*" >&2; exit 2; }
log() { echo "$PROG: $*" >&2; }

# ── arg parsing ───────────────────────────────────────────────────────────────
while [ $# -gt 0 ]; do
    case "$1" in
        --mode) MODE="$2"; shift 2 ;;
        --url) URL="$2"; shift 2 ;;
        --secret) SECRET="$2"; shift 2 ;;
        --repo) REPO="$2"; shift 2 ;;
        --branch) BRANCH="$2"; shift 2 ;;
        --commit) COMMIT="$2"; shift 2 ;;
        --status) STATUS="$2"; shift 2 ;;
        --system) SYSTEM="$2"; shift 2 ;;
        --job) JOB="$2"; shift 2 ;;
        --build-id) BUILD_ID="$2"; shift 2 ;;
        --build-url) BUILD_URL="$2"; shift 2 ;;
        --dispatch) DISPATCH="$2"; shift 2 ;;
        --token) TOKEN="$2"; shift 2 ;;
        --event-type) EVENT_TYPE="$2"; shift 2 ;;
        --retries) RETRIES="$2"; shift 2 ;;
        --timeout) TIMEOUT="$2"; shift 2 ;;
        --insecure) INSECURE=1; shift ;;
        --always) ALWAYS=1; shift ;;
        --dry-run) DRY_RUN=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown option: $1 (see --help)" ;;
    esac
done

command -v curl >/dev/null 2>&1 || die "curl is required but not found on PATH"

# ── success gate (guard 1) ────────────────────────────────────────────────────
# Normalise to lowercase without relying on bash ${x,,}.
status_lc=$(printf '%s' "$STATUS" | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]')
is_success=0
case "$status_lc" in
    success|succeeded|passed|pass|ok|green|completed|0|true) is_success=1 ;;
esac
if [ "$is_success" -ne 1 ] && [ "$ALWAYS" -ne 1 ]; then
    log "upstream status '$STATUS' is not a success — skipping downstream scan"
    exit 0
fi

# ── required-field validation (fail closed) ───────────────────────────────────
[ -n "$REPO" ] || die "--repo is required"
[ -n "$BRANCH" ] || die "--branch is required"
[ -n "$BUILD_ID" ] || log "warning: no --build-id given; de-duplication is disabled"

# ── JSON helpers ──────────────────────────────────────────────────────────────
# Escape a value for embedding in a JSON string (backslash, quote, control chars).
json_escape() {
    printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' \
        -e 's/	/\\t/g' | tr -d '\r\n'
}

hmac_sha256() { # $1=secret $2=message -> lowercase hex, no prefix
    _secret="$1"; _msg="$2"
    if command -v openssl >/dev/null 2>&1; then
        printf '%s' "$_msg" | openssl dgst -sha256 -hmac "$_secret" 2>/dev/null \
            | sed 's/^.*= *//'
    elif command -v python3 >/dev/null 2>&1; then
        python3 - "$_secret" "$_msg" <<'PY'
import hmac, hashlib, sys
print(hmac.new(sys.argv[1].encode(), sys.argv[2].encode(), hashlib.sha256).hexdigest())
PY
    elif command -v python >/dev/null 2>&1; then
        python - "$_secret" "$_msg" <<'PY'
import hmac, hashlib, sys
print(hmac.new(sys.argv[1].encode(), sys.argv[2].encode(), hashlib.sha256).hexdigest())
PY
    else
        die "server mode needs openssl or python to compute the HMAC signature"
    fi
}

# curl with retry + exponential backoff. $1=url; remaining args passed through.
# Echoes "HTTP <code>" on the last line; returns non-zero only after exhausting
# all retries on a hard failure (connection error or 5xx).
curl_retry() {
    _url="$1"; shift
    _attempt=1; _delay=2
    _tls=""
    [ "$INSECURE" -eq 1 ] && _tls="-k"
    while :; do
        # -sS quiet but show errors; -o body; write status code to stdout.
        _code=$(curl $_tls -sS -m "$TIMEOUT" -o /tmp/oxsloc_resp.$$ -w '%{http_code}' \
            "$@" "$_url" 2>/tmp/oxsloc_err.$$ || echo "000")
        if [ "$_code" -ge 200 ] 2>/dev/null && [ "$_code" -lt 300 ]; then
            cat /tmp/oxsloc_resp.$$ 2>/dev/null || true
            rm -f /tmp/oxsloc_resp.$$ /tmp/oxsloc_err.$$
            echo "HTTP $_code"
            return 0
        fi
        # 4xx (except 429) is a caller error — do not waste retries on it.
        if [ "$_code" -ge 400 ] 2>/dev/null && [ "$_code" -lt 500 ] && [ "$_code" -ne 429 ]; then
            log "downstream rejected the trigger (HTTP $_code):"
            cat /tmp/oxsloc_resp.$$ >&2 2>/dev/null || true
            rm -f /tmp/oxsloc_resp.$$ /tmp/oxsloc_err.$$
            return 3
        fi
        if [ "$_attempt" -ge "$RETRIES" ]; then
            log "trigger failed after $RETRIES attempt(s) (last HTTP $_code)"
            cat /tmp/oxsloc_err.$$ >&2 2>/dev/null || true
            rm -f /tmp/oxsloc_resp.$$ /tmp/oxsloc_err.$$
            return 3
        fi
        log "attempt $_attempt failed (HTTP $_code); retrying in ${_delay}s"
        sleep "$_delay"
        _attempt=$((_attempt + 1))
        _delay=$((_delay * 2))
    done
}

# ── server mode ───────────────────────────────────────────────────────────────
run_server() {
    [ -n "$URL" ] || die "server mode requires --url"
    [ -n "$SECRET" ] || die "server mode requires --secret"

    _sys=$(json_escape "${SYSTEM:-ci}")
    _job=$(json_escape "$JOB")
    _bid=$(json_escape "$BUILD_ID")
    _burl=$(json_escape "$BUILD_URL")
    _repo=$(json_escape "$REPO")
    _branch=$(json_escape "$BRANCH")
    _commit=$(json_escape "$COMMIT")

    # Build the exact body once; sign those exact bytes.
    BODY=$(printf '{"repo_url":"%s","branch":"%s","commit_sha":"%s","status":"success","upstream":{"system":"%s","job":"%s","build_id":"%s","url":"%s"}}' \
        "$_repo" "$_branch" "$_commit" "$_sys" "$_job" "$_bid" "$_burl")

    SIG="sha256=$(hmac_sha256 "$SECRET" "$BODY")"
    ENDPOINT="${URL%/}/webhooks/ci"

    if [ "$DRY_RUN" -eq 1 ]; then
        log "[dry-run] POST $ENDPOINT"
        log "[dry-run] X-Sloc-Signature: $SIG"
        log "[dry-run] body: $BODY"
        return 0
    fi

    log "triggering scan of $REPO@$BRANCH via $ENDPOINT"
    _out=$(curl_retry "$ENDPOINT" \
        -X POST \
        -H "Content-Type: application/json" \
        -H "X-Sloc-Signature: $SIG" \
        --data "$BODY") || return 3
    log "downstream response: $_out"
    return 0
}

# ── dispatch mode ─────────────────────────────────────────────────────────────
run_dispatch() {
    [ -n "$DISPATCH" ] || die "dispatch mode requires --dispatch KIND"
    [ -n "$URL" ] || die "dispatch mode requires --url"
    _repo=$(json_escape "$REPO"); _branch=$(json_escape "$BRANCH")
    _commit=$(json_escape "$COMMIT"); _bid=$(json_escape "$BUILD_ID")
    case "$DISPATCH" in
        github)
            [ -n "$TOKEN" ] || die "github dispatch requires --token (PAT with repo scope)"
            BODY=$(printf '{"event_type":"%s","client_payload":{"repo_url":"%s","branch":"%s","commit_sha":"%s","build_id":"%s"}}' \
                "$EVENT_TYPE" "$_repo" "$_branch" "$_commit" "$_bid")
            [ "$DRY_RUN" -eq 1 ] && { log "[dry-run] POST $URL body: $BODY"; return 0; }
            _out=$(curl_retry "$URL" -X POST \
                -H "Accept: application/vnd.github+json" \
                -H "Authorization: Bearer $TOKEN" \
                -H "Content-Type: application/json" \
                --data "$BODY") || return 3
            ;;
        gitlab)
            [ -n "$TOKEN" ] || die "gitlab dispatch requires --token (pipeline trigger token)"
            [ "$DRY_RUN" -eq 1 ] && { log "[dry-run] POST $URL (ref=$BRANCH)"; return 0; }
            _out=$(curl_retry "$URL" -X POST \
                -F "token=$TOKEN" \
                -F "ref=$BRANCH" \
                -F "variables[OXIDE_SLOC_REPO]=$REPO" \
                -F "variables[OXIDE_SLOC_COMMIT]=$COMMIT" \
                -F "variables[OXIDE_SLOC_BUILD_ID]=$BUILD_ID") || return 3
            ;;
        jenkins)
            # $URL should be the buildWithParameters endpoint; --token is the
            # job's remote-trigger auth token.
            [ -n "$TOKEN" ] || die "jenkins dispatch requires --token (job remote trigger token)"
            [ "$DRY_RUN" -eq 1 ] && { log "[dry-run] POST $URL (token, repo, branch params)"; return 0; }
            _out=$(curl_retry "$URL" -X POST \
                --data-urlencode "token=$TOKEN" \
                --data-urlencode "OXIDE_SLOC_REPO=$REPO" \
                --data-urlencode "OXIDE_SLOC_BRANCH=$BRANCH" \
                --data-urlencode "OXIDE_SLOC_COMMIT=$COMMIT" \
                --data-urlencode "OXIDE_SLOC_BUILD_ID=$BUILD_ID") || return 3
            ;;
        bitbucket)
            [ -n "$TOKEN" ] || die "bitbucket dispatch requires --token (app password / access token)"
            BODY=$(printf '{"target":{"type":"pipeline_ref_target","ref_type":"branch","ref_name":"%s","selector":{"type":"custom","pattern":"oxide-sloc-scan"}}}' \
                "$_branch")
            [ "$DRY_RUN" -eq 1 ] && { log "[dry-run] POST $URL body: $BODY"; return 0; }
            _out=$(curl_retry "$URL" -X POST \
                -H "Authorization: Bearer $TOKEN" \
                -H "Content-Type: application/json" \
                --data "$BODY") || return 3
            ;;
        *)
            die "unknown --dispatch kind '$DISPATCH' (github|gitlab|jenkins|bitbucket)"
            ;;
    esac
    log "downstream response: $_out"
    return 0
}

case "$MODE" in
    server) run_server ;;
    dispatch) run_dispatch ;;
    *) die "unknown --mode '$MODE' (server|dispatch)" ;;
esac
