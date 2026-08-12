#!/usr/bin/env bash
# Renders job-config.xml.tmpl with REPO_URL / REPO_BRANCH substituted and writes to
# a temp file. Prints the temp file path on stdout so callers can capture it:
#   JOB_XML=$(bash ci/jenkins/render-job-config.sh)
set -euo pipefail

# Allow operators to keep credentials outside the working tree (e.g. ~/.config/oxide-sloc/jenkins.env).
# Set OXIDE_SLOC_ENV_FILE in your shell profile or invoke as:
#     OXIDE_SLOC_ENV_FILE=~/.config/oxide-sloc/jenkins.env bash ci/jenkins/render-job-config.sh
if [ -n "${OXIDE_SLOC_ENV_FILE:-}" ] && [ -f "${OXIDE_SLOC_ENV_FILE}" ]; then
    set -a
    # shellcheck source=/dev/null
    . "${OXIDE_SLOC_ENV_FILE}"   # operator-supplied path, resolved at runtime
    set +a
fi

# REPO_URL is REQUIRED — never silently default to an internet URL, so this is safe
# to run on an air-gapped host. Source ci/jenkins/.env (copied from .env.example) or
# export REPO_URL first.
if [ -z "${REPO_URL:-}" ]; then
    {
        echo "error: REPO_URL is not set."
        echo "  Set it to the oxide-sloc tooling repo (or your fork/mirror), e.g.:"
        echo "     export REPO_URL=https://git.internal.example/oxide-sloc/oxide-sloc.git"
        echo "     export REPO_URL=file:///srv/git/oxide-sloc.git   # air-gapped local mirror"
        echo "  Or: cp ci/jenkins/.env.example ci/jenkins/.env && \$EDITOR ci/jenkins/.env"
        echo "      set -a; . ci/jenkins/.env; set +a   (or set OXIDE_SLOC_ENV_FILE)"
    } >&2
    exit 1
fi

# Branch/ref spec for the pipeline SCM checkout. Defaults to a CONCRETE ref (main),
# not the wildcard */main: the jgit-based lightweight checkout resolves the branch via
# Repository.findRef(), which returns null for a wildcard and NPEs at Jenkinsfile load.
# A concrete ref works for both lightweight (jgit) and heavyweight (git-plugin) checkout.
# Override for forks tracking a different branch or a pinned tag.
REPO_BRANCH="${REPO_BRANCH:-main}"

# ── Optional: release/tag webhook trigger (Generic Webhook Trigger plugin) ─────
# When SLOC_ENABLE_WEBHOOK_TRIGGER=1, render a <triggers> block so a GitHub or
# Bitbucket "release" / tag-push webhook starts this pipeline and maps the pushed
# tag into SCAN_REF (and the repo clone URL into SCAN_REPO_URL). The scanned
# project's repo posts to:
#   http://<jenkins>/generic-webhook-trigger/invoke?token=<SLOC_WEBHOOK_TRIGGER_TOKEN>
# A core SCM trigger is also added so a push to the tooling repo rebuilds too.
#
# Requires the "Generic Webhook Trigger" plugin on the controller (it is listed in
# ci/jenkins/plugins.txt). The JSONPaths default to the GitHub RELEASE payload;
# override per provider — see docs/ci-integrations.md and ci/jenkins/INTEGRATION.md:
#   GitHub release   : tag $.release.tag_name        url $.repository.clone_url
#   GitHub tag push  : tag $.ref                     url $.repository.clone_url
#   Bitbucket tag    : tag $.push.changes[0].new.name
TRIGGERS=""
if [ "${SLOC_ENABLE_WEBHOOK_TRIGGER:-0}" = "1" ] || [ "${SLOC_ENABLE_WEBHOOK_TRIGGER:-}" = "true" ]; then
    WEBHOOK_TOKEN="${SLOC_WEBHOOK_TRIGGER_TOKEN:-oxide-sloc}"
    TAG_JSONPATH="${SLOC_WEBHOOK_TAG_JSONPATH:-\$.release.tag_name}"
    URL_JSONPATH="${SLOC_WEBHOOK_URL_JSONPATH:-\$.repository.clone_url}"
    SCM_POLL_SPEC="${SLOC_SCM_POLL_SPEC:-}"   # blank = webhook/push only, no timed poll
    TRIGGERS=$(cat <<TRIGGER_XML
<org.jenkinsci.plugins.workflow.job.properties.PipelineTriggersJobProperty>
      <triggers>
        <org.jenkinsci.plugins.gwt.GenericTrigger plugin="generic-webhook-trigger">
          <genericVariables>
            <org.jenkinsci.plugins.gwt.GenericVariable>
              <key>SCAN_REF</key>
              <value>${TAG_JSONPATH}</value>
              <expressionType>JSONPath</expressionType>
              <regexpFilter>refs/tags/</regexpFilter>
              <defaultValue></defaultValue>
            </org.jenkinsci.plugins.gwt.GenericVariable>
            <org.jenkinsci.plugins.gwt.GenericVariable>
              <key>SCAN_REPO_URL</key>
              <value>${URL_JSONPATH}</value>
              <expressionType>JSONPath</expressionType>
              <regexpFilter></regexpFilter>
              <defaultValue></defaultValue>
            </org.jenkinsci.plugins.gwt.GenericVariable>
          </genericVariables>
          <token>${WEBHOOK_TOKEN}</token>
          <printPostContent>false</printPostContent>
          <printContributedVariables>false</printContributedVariables>
          <causeString>Triggered by release/tag webhook (SCAN_REF=\$SCAN_REF)</causeString>
          <regexpFilterText>\$SCAN_REF</regexpFilterText>
          <regexpFilterExpression>.+</regexpFilterExpression>
        </org.jenkinsci.plugins.gwt.GenericTrigger>
        <hudson.triggers.SCMTrigger>
          <spec>${SCM_POLL_SPEC}</spec>
          <ignorePostCommitHooks>false</ignorePostCommitHooks>
        </hudson.triggers.SCMTrigger>
      </triggers>
    </org.jenkinsci.plugins.workflow.job.properties.PipelineTriggersJobProperty>
TRIGGER_XML
)
    echo "Webhook trigger ENABLED (token=${WEBHOOK_TOKEN}, tag=${TAG_JSONPATH})." >&2
fi

OUT="$(mktemp -t oxide-sloc-job.XXXXXX.xml)"
# Substitute the trigger block via a temp file so the multi-line XML survives sed.
TRIG_FILE="$(mktemp -t oxide-sloc-trig.XXXXXX.xml)"
printf '%s' "${TRIGGERS}" > "${TRIG_FILE}"
sed -e "s|__REPO_URL__|${REPO_URL}|g" \
    -e "s|__REPO_BRANCH__|${REPO_BRANCH}|g" \
    -e "/__TRIGGERS__/{
           r ${TRIG_FILE}
           d
        }" \
    "$(dirname "$0")/job-config.xml.tmpl" > "$OUT"
rm -f "${TRIG_FILE}"
echo "Written: $OUT (REPO_URL=${REPO_URL}, REPO_BRANCH=${REPO_BRANCH})" >&2
echo "$OUT"
