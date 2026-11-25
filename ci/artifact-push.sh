#!/usr/bin/env bash
# ci/artifact-push.sh — push oxide-sloc artifacts to an external artifact repository
#
# All configuration is passed via environment variables (set by the Jenkinsfile,
# a GitLab CI job, a Bitbucket Pipelines step, or by hand for local testing).
#
# ── Required ────────────────────────────────────────────────────────────────
#   ARTIFACT_REPO_TYPE   artifactory | nexus | nexus2 | s3 | minio | azure-blob
#                        | gcs | gitlab-packages | github-packages | generic-http
#   ARTIFACT_REPO_URL    base URL of the repository service:
#                          Artifactory     https://repo.example.com/artifactory/sloc-reports
#                          Nexus 3         https://nexus.example.com
#                          Nexus 2         https://nexus.example.com/nexus
#                          S3              s3://my-bucket
#                          MinIO           s3://my-bucket  (set endpoint via ARTIFACT_REPO_EXTRA)
#                          Azure Blob      https://account.blob.core.windows.net
#                          GCS             gs://my-bucket
#                          GitLab Packages https://gitlab.example.com/api/v4
#                          GitHub Packages https://maven.pkg.github.com/OWNER/REPO
#                          Generic HTTP    https://artifacts.example.com/sloc
#   ARTIFACT_REPO_PATH   path prefix / key prefix for uploaded artifacts
#                          e.g. oxide-sloc/my-job/42
#   ARTIFACT_DIR         local directory containing the files to push
#   ARTIFACT_FILES       space-separated filenames to push
#                          e.g. "result.json report.html oxide-sloc junit.xml"
#
# ── Optional ────────────────────────────────────────────────────────────────
#   ARTIFACT_REPO_USER   username or access-key ID (omit for token-only auth)
#   ARTIFACT_REPO_PASS   password, API token, account key, or secret key
#   ARTIFACT_REPO_EXTRA  provider-specific extra setting:
#                          nexus / nexus2    Nexus repository name        (default: sloc-raw-hosted)
#                          azure-blob        storage container name       (default: sloc-reports)
#                          minio             MinIO endpoint URL           (e.g. https://minio.internal:9000)
#                          s3                extra flags for aws s3 cp    (e.g. --sse aws:kms)
#                          gcs               extra flags for gcloud cp    (e.g. --cache-control=no-cache)
#                          gitlab-packages   <project-id>/<package>/<version>
#                                            (e.g. 42/oxide-sloc/1.6.30; default package/version: oxide-sloc/latest)
#                          github-packages   extra path segments under the Maven repo (default: none)
#                          others            unused
#   ARTIFACT_GENERATE_MANIFEST  true | false (default: false)
#                          Generates checksums.sha256 in ARTIFACT_DIR listing the
#                          SHA-256 hash of every pushed file, then uploads it too.
#   ARTIFACT_DRY_RUN     true | false (default: false)
#                          Prints what would be pushed without making any network calls.
#
# ── Artifact types supported ─────────────────────────────────────────────────
#   Scan reports  : result_<slug>.json, report_<slug>.csv, report_<slug>.xlsx,
#                   report_<slug>.html, report_<slug>.pdf
#   Binary        : oxide-sloc (or oxide-sloc.exe on Windows)
#   Test results  : junit.xml
#   Coverage      : lcov.info, sonar-coverage.xml
#   Diff/compare  : diff.json, diff.csv, ref-scan.json, baseline-scan.json
#   Manifest      : checksums.sha256 (auto-generated; see ARTIFACT_GENERATE_MANIFEST)
#
# ── Exit codes ───────────────────────────────────────────────────────────────
#   0   all pushes succeeded
#   1   one or more file pushes failed
#   2   configuration error (bad type, missing required var, missing CLI tool)

set -euo pipefail

REPO_TYPE="${ARTIFACT_REPO_TYPE:-}"
REPO_URL="${ARTIFACT_REPO_URL:-}"
REPO_PATH="${ARTIFACT_REPO_PATH:-oxide-sloc}"
ARTIFACT_DIR="${ARTIFACT_DIR:-}"
ARTIFACT_FILES="${ARTIFACT_FILES:-result.json}"
REPO_USER="${ARTIFACT_REPO_USER:-}"
REPO_PASS="${ARTIFACT_REPO_PASS:-}"
REPO_EXTRA="${ARTIFACT_REPO_EXTRA:-}"
GENERATE_MANIFEST="${ARTIFACT_GENERATE_MANIFEST:-false}"
DRY_RUN="${ARTIFACT_DRY_RUN:-false}"

# ── Validation ───────────────────────────────────────────────────────────────

if [ -z "${REPO_TYPE}" ]; then
    echo "ERROR: ARTIFACT_REPO_TYPE is not set." >&2
    exit 2
fi

VALID_TYPES="artifactory nexus nexus2 s3 minio azure-blob gcs gitlab-packages github-packages generic-http"
valid=0
for t in ${VALID_TYPES}; do [ "${REPO_TYPE}" = "${t}" ] && valid=1 && break; done
if [ "${valid}" -eq 0 ]; then
    echo "ERROR: Unknown ARTIFACT_REPO_TYPE '${REPO_TYPE}'." >&2
    echo "       Supported: ${VALID_TYPES}" >&2
    exit 2
fi

if [ -z "${REPO_URL}" ]; then
    echo "ERROR: ARTIFACT_REPO_URL is not set." >&2
    exit 2
fi

if [ -z "${ARTIFACT_DIR}" ] || [ ! -d "${ARTIFACT_DIR}" ]; then
    echo "ERROR: ARTIFACT_DIR '${ARTIFACT_DIR}' is not set or does not exist." >&2
    exit 2
fi

# Normalise: strip trailing slash from URL, leading slash from path
REPO_URL="${REPO_URL%/}"
REPO_PATH="${REPO_PATH#/}"

PUSH_FAILURES=0

# ── MIME type helper ──────────────────────────────────────────────────────────
#
# Returns the appropriate Content-Type for a filename so Nexus and other
# repositories store assets with the correct media type.

mime_type() {
    case "${1##*.}" in
        json)   echo "application/json" ;;
        html)   echo "text/html" ;;
        pdf)    echo "application/pdf" ;;
        csv)    echo "text/csv" ;;
        xlsx)   echo "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" ;;
        xml)    echo "application/xml" ;;
        txt|sha256|info) echo "text/plain" ;;
        *)      echo "application/octet-stream" ;;
    esac
}

# ── SHA-256 manifest generation ───────────────────────────────────────────────
#
# When ARTIFACT_GENERATE_MANIFEST=true, writes checksums.sha256 into ARTIFACT_DIR
# containing one "hash  filename" line per file being pushed, then appends
# "checksums.sha256" to ARTIFACT_FILES so it is uploaded alongside the rest.

generate_manifest() {
    local manifest="${ARTIFACT_DIR}/checksums.sha256"
    echo "Generating SHA-256 manifest…"
    rm -f "${manifest}"
    local count=0
    for f in ${ARTIFACT_FILES}; do
        local file="${ARTIFACT_DIR}/${f}"
        [ -f "${file}" ] || continue
        (cd "${ARTIFACT_DIR}" && sha256sum "${f}") >> "${manifest}"
        count=$((count + 1))
    done
    if [ "${count}" -gt 0 ]; then
        echo "  Checksums: ${count} file(s) → checksums.sha256"
        ARTIFACT_FILES="${ARTIFACT_FILES} checksums.sha256"
    else
        echo "  WARNING: no files found to checksum — manifest skipped." >&2
        rm -f "${manifest}"
    fi
}

# ── Provider: JFrog Artifactory ──────────────────────────────────────────────
#
# Uses the Artifactory REST API: PUT /<repo-path>/<filename>
# Auth options (tested in order):
#   user + pass  → HTTP Basic
#   pass only    → X-JFrog-Art-Api header (API key)
#   neither      → anonymous (requires anonymous read/write enabled on the repo)
#
# Docs: https://jfrog.com/help/r/jfrog-rest-apis/deploy-artifact

push_artifactory() {
    echo "=== Artifactory → ${REPO_URL}/${REPO_PATH}/ ==="
    for f in ${ARTIFACT_FILES}; do
        local file="${ARTIFACT_DIR}/${f}"
        if [ ! -f "${file}" ]; then
            echo "  SKIP  ${f} (file not found)"
            continue
        fi
        local target="${REPO_URL}/${REPO_PATH}/${f}"
        echo "  PUT   ${f}  →  ${target}"

        if [ "${DRY_RUN}" = "true" ]; then echo "  [DRY-RUN] skipping upload"; continue; fi

        local curl_auth=()
        if [ -n "${REPO_USER}" ] && [ -n "${REPO_PASS}" ]; then
            curl_auth=(-u "${REPO_USER}:${REPO_PASS}")
        elif [ -n "${REPO_PASS}" ]; then
            curl_auth=(-H "X-JFrog-Art-Api: ${REPO_PASS}")
        fi

        if curl -sf "${curl_auth[@]}" -X PUT "${target}" -T "${file}" \
                --retry 3 --retry-delay 2 --retry-max-time 60; then
            echo "  OK"
        else
            echo "  FAIL  ${f}" >&2
            PUSH_FAILURES=$((PUSH_FAILURES + 1))
        fi
    done
}

# ── Provider: Sonatype Nexus Repository Manager 3 ────────────────────────────
#
# Uses the Nexus 3 REST API for raw-format repositories:
#   POST /service/rest/v1/components?repository=<repo-name>
# One multipart request per file (avoids the 3-asset limit per request).
#
# ARTIFACT_REPO_URL  : base Nexus URL, e.g. https://nexus.example.com
# ARTIFACT_REPO_EXTRA: repository name, e.g. sloc-raw-hosted  (default: sloc-raw-hosted)
#
# Docs: https://help.sonatype.com/en/uploading-components.html

push_nexus() {
    local repo_name="${REPO_EXTRA:-sloc-raw-hosted}"
    local api_url="${REPO_URL}/service/rest/v1/components?repository=${repo_name}"
    echo "=== Nexus 3 (repo: ${repo_name}) → ${REPO_URL}/${REPO_PATH}/ ==="

    local curl_auth=()
    if [ -n "${REPO_USER}" ] && [ -n "${REPO_PASS}" ]; then
        curl_auth=(-u "${REPO_USER}:${REPO_PASS}")
    elif [ -n "${REPO_PASS}" ]; then
        curl_auth=(-H "Authorization: Bearer ${REPO_PASS}")
    fi

    for f in ${ARTIFACT_FILES}; do
        local file="${ARTIFACT_DIR}/${f}"
        if [ ! -f "${file}" ]; then
            echo "  SKIP  ${f} (file not found)"
            continue
        fi
        local content_type
        content_type=$(mime_type "${f}")
        echo "  POST  ${f}  →  /${REPO_PATH}/${f}  (${content_type})"

        if [ "${DRY_RUN}" = "true" ]; then echo "  [DRY-RUN] skipping upload"; continue; fi

        if curl -sf "${curl_auth[@]}" -X POST "${api_url}" \
                -F "raw.directory=/${REPO_PATH}" \
                -F "raw.asset1=@${file};type=${content_type}" \
                -F "raw.asset1.filename=${f}" \
                --retry 3 --retry-delay 2 --retry-max-time 60; then
            echo "  OK"
        else
            echo "  FAIL  ${f}" >&2
            PUSH_FAILURES=$((PUSH_FAILURES + 1))
        fi
    done
}

# ── Provider: Sonatype Nexus Repository Manager 2 ────────────────────────────
#
# Uses the Nexus 2 content REST endpoint: PUT /content/repositories/<repo>/<path>/<file>
# The Nexus 2 context path (/nexus) must be included in ARTIFACT_REPO_URL if applicable.
#
# ARTIFACT_REPO_URL  : e.g. https://nexus.example.com/nexus
# ARTIFACT_REPO_EXTRA: repository ID (default: sloc-raw-hosted)
#
# Docs: https://support.sonatype.com/hc/en-us/articles/213465668

push_nexus2() {
    local repo_name="${REPO_EXTRA:-sloc-raw-hosted}"
    echo "=== Nexus 2 (repo: ${repo_name}) → ${REPO_URL}/${REPO_PATH}/ ==="

    local curl_auth=()
    if [ -n "${REPO_USER}" ] && [ -n "${REPO_PASS}" ]; then
        curl_auth=(-u "${REPO_USER}:${REPO_PASS}")
    fi

    for f in ${ARTIFACT_FILES}; do
        local file="${ARTIFACT_DIR}/${f}"
        if [ ! -f "${file}" ]; then
            echo "  SKIP  ${f} (file not found)"
            continue
        fi
        local target="${REPO_URL}/content/repositories/${repo_name}/${REPO_PATH}/${f}"
        echo "  PUT   ${f}  →  ${target}"

        if [ "${DRY_RUN}" = "true" ]; then echo "  [DRY-RUN] skipping upload"; continue; fi

        if curl -sf "${curl_auth[@]}" -X PUT "${target}" -T "${file}" \
                --retry 3 --retry-delay 2 --retry-max-time 60; then
            echo "  OK"
        else
            echo "  FAIL  ${f}" >&2
            PUSH_FAILURES=$((PUSH_FAILURES + 1))
        fi
    done
}

# ── Provider: AWS S3 ──────────────────────────────────────────────────────────
#
# Uses the AWS CLI: aws s3 cp <file> <s3://bucket/path/file>
# Credentials are taken from ARTIFACT_REPO_USER (access key ID) and
# ARTIFACT_REPO_PASS (secret access key), falling back to the standard
# AWS credential chain (~/.aws, instance profile, env vars).
#
# ARTIFACT_REPO_URL  : s3://bucket-name
# ARTIFACT_REPO_EXTRA: extra flags for aws s3 cp (e.g. --sse aws:kms --storage-class STANDARD_IA)
#
# Requires: aws CLI  (https://docs.aws.amazon.com/cli/latest/userguide/install-cliv2.html)

push_s3() {
    local extra_flags="${REPO_EXTRA:-}"
    echo "=== AWS S3 → ${REPO_URL}/${REPO_PATH}/ ==="

    if ! command -v aws >/dev/null 2>&1; then
        echo "ERROR: 'aws' CLI not found on PATH. Install the AWS CLI on this agent." >&2
        PUSH_FAILURES=$((PUSH_FAILURES + 1))
        return
    fi

    if [ -n "${REPO_USER}" ] && [ -n "${REPO_PASS}" ]; then
        export AWS_ACCESS_KEY_ID="${REPO_USER}"
        export AWS_SECRET_ACCESS_KEY="${REPO_PASS}"
    fi

    for f in ${ARTIFACT_FILES}; do
        local file="${ARTIFACT_DIR}/${f}"
        if [ ! -f "${file}" ]; then
            echo "  SKIP  ${f} (file not found)"
            continue
        fi
        local target="${REPO_URL}/${REPO_PATH}/${f}"
        echo "  cp    ${f}  →  ${target}"

        if [ "${DRY_RUN}" = "true" ]; then echo "  [DRY-RUN] skipping upload"; continue; fi

        # shellcheck disable=SC2086
        if aws s3 cp "${file}" "${target}" ${extra_flags}; then
            echo "  OK"
        else
            echo "  FAIL  ${f}" >&2
            PUSH_FAILURES=$((PUSH_FAILURES + 1))
        fi
    done
}

# ── Provider: MinIO (S3-compatible) ──────────────────────────────────────────
#
# Uses the AWS CLI with a custom --endpoint-url for MinIO.
#
# ARTIFACT_REPO_URL  : s3://bucket-name  (the bucket, not the server URL)
# ARTIFACT_REPO_USER : MinIO access key
# ARTIFACT_REPO_PASS : MinIO secret key
# ARTIFACT_REPO_EXTRA: MinIO server endpoint URL (e.g. https://minio.internal:9000)
#
# Requires: aws CLI  (https://docs.aws.amazon.com/cli/latest/userguide/install-cliv2.html)

push_minio() {
    local endpoint="${REPO_EXTRA:-}"
    echo "=== MinIO → ${REPO_URL}/${REPO_PATH}/ (endpoint: ${endpoint:-<from REPO_URL>}) ==="

    if ! command -v aws >/dev/null 2>&1; then
        echo "ERROR: 'aws' CLI not found on PATH. MinIO uses the aws CLI with --endpoint-url." >&2
        PUSH_FAILURES=$((PUSH_FAILURES + 1))
        return
    fi

    if [ -z "${endpoint}" ]; then
        echo "WARNING: ARTIFACT_REPO_EXTRA not set — cannot construct MinIO endpoint URL." >&2
        echo "         Set ARTIFACT_REPO_EXTRA to your MinIO server URL (e.g. https://minio.internal:9000)." >&2
        PUSH_FAILURES=$((PUSH_FAILURES + 1))
        return
    fi

    if [ -n "${REPO_USER}" ] && [ -n "${REPO_PASS}" ]; then
        export AWS_ACCESS_KEY_ID="${REPO_USER}"
        export AWS_SECRET_ACCESS_KEY="${REPO_PASS}"
    fi

    for f in ${ARTIFACT_FILES}; do
        local file="${ARTIFACT_DIR}/${f}"
        if [ ! -f "${file}" ]; then
            echo "  SKIP  ${f} (file not found)"
            continue
        fi
        local target="${REPO_URL}/${REPO_PATH}/${f}"
        echo "  cp    ${f}  →  ${target}"

        if [ "${DRY_RUN}" = "true" ]; then echo "  [DRY-RUN] skipping upload"; continue; fi

        if aws s3 cp "${file}" "${target}" --endpoint-url "${endpoint}"; then
            echo "  OK"
        else
            echo "  FAIL  ${f}" >&2
            PUSH_FAILURES=$((PUSH_FAILURES + 1))
        fi
    done
}

# ── Provider: Azure Blob Storage ─────────────────────────────────────────────
#
# Uses the Azure CLI: az storage blob upload
# Auth: account key (ARTIFACT_REPO_PASS as raw key) or SAS token (ARTIFACT_REPO_PASS
# starting with "?sv=" or "sv="). Omitting ARTIFACT_REPO_PASS falls back to the
# Azure CLI's ambient credential chain (Managed Identity, az login, etc.).
#
# ARTIFACT_REPO_URL  : https://<account>.blob.core.windows.net
# ARTIFACT_REPO_EXTRA: container name (default: sloc-reports)
#
# Requires: az CLI  (https://learn.microsoft.com/cli/azure/install-azure-cli)

push_azure_blob() {
    local container="${REPO_EXTRA:-sloc-reports}"
    echo "=== Azure Blob Storage → ${REPO_URL}/${container}/${REPO_PATH}/ ==="

    if ! command -v az >/dev/null 2>&1; then
        echo "ERROR: 'az' CLI not found on PATH. Install the Azure CLI on this agent." >&2
        PUSH_FAILURES=$((PUSH_FAILURES + 1))
        return
    fi

    # Derive storage account name from URL: https://<account>.blob.core.windows.net
    local account
    account=$(echo "${REPO_URL}" | sed -E 's|https?://([^.]+)\..*|\1|')

    local auth_flags=()
    if [ -n "${REPO_PASS}" ]; then
        if [[ "${REPO_PASS}" == "?sv="* ]] || [[ "${REPO_PASS}" == "sv="* ]]; then
            auth_flags=(--sas-token "${REPO_PASS}")
        else
            auth_flags=(--account-key "${REPO_PASS}")
        fi
    fi

    for f in ${ARTIFACT_FILES}; do
        local file="${ARTIFACT_DIR}/${f}"
        if [ ! -f "${file}" ]; then
            echo "  SKIP  ${f} (file not found)"
            continue
        fi
        local blob_name="${REPO_PATH}/${f}"
        echo "  upload ${f}  →  ${container}/${blob_name}"

        if [ "${DRY_RUN}" = "true" ]; then echo "  [DRY-RUN] skipping upload"; continue; fi

        if az storage blob upload \
                --account-name "${account}" \
                --container-name "${container}" \
                --name "${blob_name}" \
                --file "${file}" \
                --overwrite \
                "${auth_flags[@]}" \
                --output none; then
            echo "  OK"
        else
            echo "  FAIL  ${f}" >&2
            PUSH_FAILURES=$((PUSH_FAILURES + 1))
        fi
    done
}

# ── Provider: Google Cloud Storage ───────────────────────────────────────────
#
# Uses the gcloud CLI: gcloud storage cp <file> <gs://bucket/path/file>
# Authentication is taken from the ambient gcloud credential chain — a service
# account activated with `gcloud auth activate-service-account`, the
# GOOGLE_APPLICATION_CREDENTIALS key file, or a workload-identity/metadata
# credential on a GCP runner. No key material is read from ARTIFACT_REPO_*.
#
# ARTIFACT_REPO_URL  : gs://bucket-name
# ARTIFACT_REPO_EXTRA: extra flags for gcloud storage cp (e.g. --cache-control=no-cache)
#
# Requires: gcloud CLI  (https://cloud.google.com/sdk/docs/install)
#           Older SDKs without `gcloud storage` fall back to `gsutil cp`.

push_gcs() {
    local extra_flags="${REPO_EXTRA:-}"
    echo "=== Google Cloud Storage → ${REPO_URL}/${REPO_PATH}/ ==="

    local cp_cmd=""
    if command -v gcloud >/dev/null 2>&1; then
        cp_cmd="gcloud storage cp"
    elif command -v gsutil >/dev/null 2>&1; then
        cp_cmd="gsutil cp"
    else
        echo "ERROR: neither 'gcloud' nor 'gsutil' found on PATH. Install the Google Cloud SDK." >&2
        PUSH_FAILURES=$((PUSH_FAILURES + 1))
        return
    fi

    for f in ${ARTIFACT_FILES}; do
        local file="${ARTIFACT_DIR}/${f}"
        if [ ! -f "${file}" ]; then
            echo "  SKIP  ${f} (file not found)"
            continue
        fi
        local target="${REPO_URL}/${REPO_PATH}/${f}"
        echo "  cp    ${f}  →  ${target}"

        if [ "${DRY_RUN}" = "true" ]; then echo "  [DRY-RUN] skipping upload"; continue; fi

        # shellcheck disable=SC2086
        if ${cp_cmd} "${file}" "${target}" ${extra_flags}; then
            echo "  OK"
        else
            echo "  FAIL  ${f}" >&2
            PUSH_FAILURES=$((PUSH_FAILURES + 1))
        fi
    done
}

# ── Provider: GitLab Package Registry (generic packages) ─────────────────────
#
# Uses the generic Package Registry API (one PUT per file):
#   PUT <api-v4>/projects/<id>/packages/generic/<package>/<version>/<file>
# Auth: a personal/project access token (ARTIFACT_REPO_PASS) is sent as the
# PRIVATE-TOKEN header; inside a GitLab CI job the CI_JOB_TOKEN works too and is
# picked up automatically from the environment when ARTIFACT_REPO_PASS is unset.
#
# ARTIFACT_REPO_URL  : GitLab API v4 base, e.g. https://gitlab.example.com/api/v4
# ARTIFACT_REPO_EXTRA: <project-id>[/<package>[/<version>]]
#                      e.g. 42/oxide-sloc/1.6.30  (default package/version: oxide-sloc/latest)
#
# Docs: https://docs.gitlab.com/ee/user/packages/generic_packages/

push_gitlab_packages() {
    local project="" package="oxide-sloc" version="latest"
    local IFS='/'
    # shellcheck disable=SC2086
    set -- ${REPO_EXTRA:-}
    unset IFS
    project="${1:-}"
    [ -n "${2:-}" ] && package="${2}"
    [ -n "${3:-}" ] && version="${3}"

    echo "=== GitLab Packages (project: ${project:-<unset>}, ${package}/${version}) → ${REPO_URL} ==="

    if [ -z "${project}" ]; then
        echo "ERROR: ARTIFACT_REPO_EXTRA must start with the numeric project id" >&2
        echo "       (e.g. 42/oxide-sloc/1.6.30)." >&2
        PUSH_FAILURES=$((PUSH_FAILURES + 1))
        return
    fi

    local curl_auth=()
    if [ -n "${REPO_PASS}" ]; then
        curl_auth=(-H "PRIVATE-TOKEN: ${REPO_PASS}")
    elif [ -n "${CI_JOB_TOKEN:-}" ]; then
        curl_auth=(-H "JOB-TOKEN: ${CI_JOB_TOKEN}")
    fi

    for f in ${ARTIFACT_FILES}; do
        local file="${ARTIFACT_DIR}/${f}"
        if [ ! -f "${file}" ]; then
            echo "  SKIP  ${f} (file not found)"
            continue
        fi
        local target="${REPO_URL}/projects/${project}/packages/generic/${package}/${version}/${f}"
        echo "  PUT   ${f}  →  ${target}"

        if [ "${DRY_RUN}" = "true" ]; then echo "  [DRY-RUN] skipping upload"; continue; fi

        if curl -sf "${curl_auth[@]}" -X PUT "${target}" -T "${file}" \
                --retry 3 --retry-delay 2 --retry-max-time 60; then
            echo "  OK"
        else
            echo "  FAIL  ${f}" >&2
            PUSH_FAILURES=$((PUSH_FAILURES + 1))
        fi
    done
}

# ── Provider: GitHub Packages (Maven repository) ─────────────────────────────
#
# GitHub Packages has no raw/generic file store, so reports are published to the
# repo's Maven registry — the one GitHub Packages surface that accepts arbitrary
# file uploads over a plain HTTP PUT:
#   PUT https://maven.pkg.github.com/OWNER/REPO/<path>/<file>
# Auth: HTTP Basic with the GitHub username (ARTIFACT_REPO_USER) and a PAT or the
# Actions GITHUB_TOKEN (ARTIFACT_REPO_PASS) carrying `write:packages` scope.
#
# ARTIFACT_REPO_URL  : https://maven.pkg.github.com/OWNER/REPO
# ARTIFACT_REPO_EXTRA: optional extra path segments under the repo (default: none)
#
# Docs: https://docs.github.com/packages/working-with-a-github-packages-registry/working-with-the-apache-maven-registry

push_github_packages() {
    local subpath="${REPO_EXTRA:-}"
    subpath="${subpath#/}"
    subpath="${subpath%/}"
    echo "=== GitHub Packages (Maven) → ${REPO_URL}/${subpath:+${subpath}/}${REPO_PATH}/ ==="

    if [ -z "${REPO_PASS}" ]; then
        echo "ERROR: ARTIFACT_REPO_PASS (a PAT / GITHUB_TOKEN with write:packages) is required." >&2
        PUSH_FAILURES=$((PUSH_FAILURES + 1))
        return
    fi

    # GitHub requires a username alongside the token for Basic auth; default to a
    # conventional placeholder when the caller only supplies a token.
    local user="${REPO_USER:-x-access-token}"

    for f in ${ARTIFACT_FILES}; do
        local file="${ARTIFACT_DIR}/${f}"
        if [ ! -f "${file}" ]; then
            echo "  SKIP  ${f} (file not found)"
            continue
        fi
        local target="${REPO_URL}/${subpath:+${subpath}/}${REPO_PATH}/${f}"
        echo "  PUT   ${f}  →  ${target}"

        if [ "${DRY_RUN}" = "true" ]; then echo "  [DRY-RUN] skipping upload"; continue; fi

        if curl -sf -u "${user}:${REPO_PASS}" -X PUT "${target}" -T "${file}" \
                --retry 3 --retry-delay 2 --retry-max-time 60; then
            echo "  OK"
        else
            echo "  FAIL  ${f}" >&2
            PUSH_FAILURES=$((PUSH_FAILURES + 1))
        fi
    done
}

# ── Provider: Generic HTTP PUT ───────────────────────────────────────────────
#
# Sends each artifact via HTTP PUT to <ARTIFACT_REPO_URL>/<ARTIFACT_REPO_PATH>/<filename>.
# Works with any HTTP server that accepts PUT requests (Sonatype Nexus raw,
# Artifactory legacy endpoints, Gitea packages, custom artifact stores, etc.).
# Auth: user+pass → Basic; pass only → Bearer token.

push_generic_http() {
    echo "=== Generic HTTP PUT → ${REPO_URL}/${REPO_PATH}/ ==="

    local curl_auth=()
    if [ -n "${REPO_USER}" ] && [ -n "${REPO_PASS}" ]; then
        curl_auth=(-u "${REPO_USER}:${REPO_PASS}")
    elif [ -n "${REPO_PASS}" ]; then
        curl_auth=(-H "Authorization: Bearer ${REPO_PASS}")
    fi

    for f in ${ARTIFACT_FILES}; do
        local file="${ARTIFACT_DIR}/${f}"
        if [ ! -f "${file}" ]; then
            echo "  SKIP  ${f} (file not found)"
            continue
        fi
        local target="${REPO_URL}/${REPO_PATH}/${f}"
        echo "  PUT   ${f}  →  ${target}"

        if [ "${DRY_RUN}" = "true" ]; then echo "  [DRY-RUN] skipping upload"; continue; fi

        if curl -sf "${curl_auth[@]}" -X PUT "${target}" -T "${file}" \
                --retry 3 --retry-delay 2 --retry-max-time 60; then
            echo "  OK"
        else
            echo "  FAIL  ${f}" >&2
            PUSH_FAILURES=$((PUSH_FAILURES + 1))
        fi
    done
}

# ── Dispatch ─────────────────────────────────────────────────────────────────

[ "${GENERATE_MANIFEST}" = "true" ] && generate_manifest

echo ""
echo "oxide-sloc artifact push"
echo "  type  : ${REPO_TYPE}"
echo "  url   : ${REPO_URL}"
echo "  path  : ${REPO_PATH}"
echo "  dir   : ${ARTIFACT_DIR}"
echo "  files : ${ARTIFACT_FILES}"
[ "${DRY_RUN}" = "true" ] && echo "  mode  : DRY-RUN (no files will be uploaded)"
echo ""

case "${REPO_TYPE}" in
    artifactory)      push_artifactory ;;
    nexus)            push_nexus ;;
    nexus2)           push_nexus2 ;;
    s3)               push_s3 ;;
    minio)            push_minio ;;
    azure-blob)       push_azure_blob ;;
    gcs)              push_gcs ;;
    gitlab-packages)  push_gitlab_packages ;;
    github-packages)  push_github_packages ;;
    generic-http)     push_generic_http ;;
esac

echo ""
if [ "${PUSH_FAILURES}" -gt 0 ]; then
    echo "RESULT: ${PUSH_FAILURES} artifact(s) failed to push — see output above." >&2
    exit 1
fi

echo "RESULT: all artifacts pushed successfully."
