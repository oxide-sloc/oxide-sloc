#!/bin/sh
# Example: LEGACY Jenkins freestyle job — no pipeline, no plugins, no Groovy.
#
# This is the "very outdated pipeline" path. Add an "Execute shell" build step as
# the LAST step of the freestyle job and paste this in (or, better, check the
# trigger script into the repo and just call it). Freestyle runs later build
# steps only if earlier ones passed, so reaching this step already implies the
# build succeeded — but we still pass --status success (and the script's gate
# would skip a non-success value if you wire one in).
#
# Configure these as Jenkins global properties or job environment variables:
#   OXIDE_SLOC_URL     https://sloc.internal.example.com
#   OXIDE_SLOC_SECRET  shared HMAC secret of a matching scan schedule (use the
#                      "Inject passwords" / Credentials Binding plugin, or a
#                      global env var on a trusted controller)
#
# Freestyle exposes GIT_URL / GIT_BRANCH / GIT_COMMIT when the Git SCM plugin is
# used; adjust if your job uses a different SCM.
set -e
# Enable pipefail when the shell supports it (bash/ksh/zsh); dash stays without
# it rather than aborting. Keeps a failing pipe stage from being masked.
if (set -o pipefail) 2>/dev/null; then
    set -o pipefail
fi

REPO="${GIT_URL:-$(git config --get remote.origin.url)}"
BRANCH="${GIT_BRANCH#origin/}"
[ -n "$BRANCH" ] || BRANCH="$(git rev-parse --abbrev-ref HEAD)"
COMMIT="${GIT_COMMIT:-$(git rev-parse HEAD)}"

# If you have not checked the script into the repo, fetch it once to the
# workspace (offline mirrors work too — it is plain POSIX sh):
#   curl -fsSL "$OXIDE_SLOC_URL/... " -o trigger-oxide-sloc.sh   # or copy it in
TRIGGER="${TRIGGER:-ci/downstream-trigger/trigger-oxide-sloc.sh}"

sh "$TRIGGER" \
  --repo "$REPO" \
  --branch "$BRANCH" \
  --commit "$COMMIT" \
  --status success \
  --system jenkins \
  --job "${JOB_NAME:-freestyle}" \
  --build-id "${BUILD_NUMBER:-0}" \
  --build-url "${BUILD_URL:-}"
