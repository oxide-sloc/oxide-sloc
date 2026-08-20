/*
 * oxide-sloc canonical Jenkins pipeline.
 *
 * This is the entry point used by ci/jenkins/job-config.xml and seed-job.groovy.
 * It is intentionally kept thin: helper logic lives in ci/jenkins/pipeline-helpers.groovy
 * and is loaded after checkout so that each function compiles as a separate class,
 * keeping every generated method well under the JVM 64 KB bytecode limit.
 *
 * Do NOT add def functions or large sh-heredocs here — put them in pipeline-helpers.groovy
 * or in a dedicated ci/jenkins/*.sh script and call it with sh 'bash ci/jenkins/script.sh'.
 *
 * Pipeline-of-Pipelines usage:
 *   From an orchestrator pipeline, trigger this job with:
 *     build job: 'oxide-sloc', parameters: [
 *       string(name: 'SCAN_REPO_URL',        value: 'https://...'),
 *       string(name: 'CHAIN_DOWNSTREAM_JOB',  value: 'next-pipeline')
 *     ], wait: true
 *   This pipeline will trigger CHAIN_DOWNSTREAM_JOB on success, passing back
 *   CHAIN_UPSTREAM_JOB, CHAIN_UPSTREAM_BUILD, and ARTIFACT_PATH.
 *
 * Release/tag triggering (webhook): configure the job to build on a GitHub/Bitbucket
 * release or tag push and map the pushed tag into SCAN_REF. See ci/jenkins/INTEGRATION.md
 * and ci/jenkins/render-job-config.sh (SLOC_ENABLE_WEBHOOK_TRIGGER).
 */

def h   // loaded after Checkout; all runXxx() calls below delegate here

// ── Pipeline ───────────────────────────────────────────────────────────────
pipeline {
    // Honour the AGENT_LABEL build parameter so an operator can pin the job to a
    // specific node — most importantly a LINUX node, since the POSIX .sh scripts
    // that drive every stage need a shell. On Windows agents the pipeline runs
    // them through Git Bash via the shx() helper (see pipeline-helpers.groovy),
    // but pinning to Linux via AGENT_LABEL avoids the Git-Bash requirement
    // entirely. An empty label ('' — the default) means "any available agent",
    // preserving today's behaviour. params.AGENT_LABEL is safe to reference here:
    // Jenkins resolves the parameters block before selecting the agent.
    agent { label "${params.AGENT_LABEL ?: ''}" }

    options {
        skipDefaultCheckout(true)
        disableConcurrentBuilds()
        // Bound server storage on BOTH count and age. Build records (logs +
        // published HTML reports) are capped at 20 builds / 30 days; the heavier
        // archived artifacts (JSON/HTML/PDF/XLSX/sub-reports) are pruned harder —
        // last 5 builds / 14 days — since only the latest matter for review and
        // trend history is preserved separately in the persistent CSV.
        buildDiscarder(logRotator(
            numToKeepStr:         '20',
            daysToKeepStr:        '30',
            artifactNumToKeepStr: '5',
            artifactDaysToKeepStr: '14'
        ))
        timeout(time: 60, unit: 'MINUTES')
        // NOTE: ansiColor('xterm') was intentionally removed. It is a parse-time
        // hard dependency on the non-default AnsiColor plugin (a missing options
        // directive can't be try/catch'd), and it only colorized console output.
        // Dropping it lets the pipeline run on a controller where AnsiColor isn't
        // installed. To restore coloured logs, install AnsiColor and re-add it.
        // NOTE: timestamps() was intentionally removed for the same reason. It is a
        // parse-time hard dependency on the non-default Timestamper plugin — on a
        // minimal controller it fails the WHOLE pipeline at parse (it cannot be
        // try/catch'd), exactly like ansiColor above. Re-add it only after
        // confirming the Timestamper plugin is installed.
    }

    // ── Build parameters ──────────────────────────────────────────────────────
    // All fields appear as form controls in Jenkins → "Build with Parameters".
    // The first build of a Pipeline-from-SCM job is unparameterized — Jenkins
    // only discovers the parameters{} block after running Jenkinsfile once.
    // Run the first build with no arguments to seed the form; from build #2
    // onward, "Build with Parameters" in the sidebar shows the full form.
    //
    // Ordered from most-frequently changed (top) to rarely touched (bottom).
    // Artifact repo config and pipeline-chaining params are last so the form
    // stays compact for normal scan runs.
    parameters {

        // ═══════════════════════════════════════════════════════════════════════
        //  WHAT TO SCAN — point this at any existing repo and hit Build.
        //  GitHub / GitLab / Bitbucket / local all work (it's a plain git clone).
        //  Private repo? also set SCAN_CREDENTIALS_ID (single instance) or
        //  SCAN_GIT_CREDENTIALS (several instances in one run). Leave SCAN_REPO_URL
        //  blank to scan this tooling repo itself (self-CI / demo).
        //  See docs: ci/jenkins/INTEGRATION.md
        //
        //  FAST BY DEFAULT: out of the box this pipeline runs the prebuilt scanner
        //  from dist/ and skips the Rust toolchain, unit tests, lint, and coverage,
        //  so a standard scan finishes in a couple of minutes. Turn on RUN_QUALITY_GATES
        //  (or BUILD_MODE=source) only when you want to build and test from source.
        // ═══════════════════════════════════════════════════════════════════════
        string(
            name:         'SCAN_REPO_URL',
            defaultValue: '',
            description:  'The repo to analyze — any Git URL (GitHub / GitLab / Bitbucket / ' +
                          'file:///local, https:// or ssh://). Blank = scan this tooling repo itself. ' +
                          'Cloned into ./_target; SCAN_PATH is resolved inside it.'
        )
        string(
            name:         'SCAN_REF',
            defaultValue: '',
            description:  'Branch, tag, or commit SHA to scan. BLANK defaults to "main" — set this ' +
                          'explicitly for a repo whose default branch is not main (e.g. master). ' +
                          'A release/tag webhook (see ci/jenkins/INTEGRATION.md) sets this automatically. ' +
                          'e.g. main, master, develop, v2.1.0, a3f9d2c'
        )
        string(
            name:         'SCAN_PATH',
            defaultValue: '',
            description:  'Subdirectory of the target repo to scan. BLANK = the whole repo (recommended). ' +
                          'Set e.g. "src" or "packages/api" for a subtree; absolute paths also work.'
        )

        // ── Scan credentials — single instance, or many instances in one run ────
        string(
            name:         'SCAN_CREDENTIALS_ID',
            defaultValue: '',
            description:  'Jenkins credentials ID for cloning ONE private target repo (a username + ' +
                          'personal-access-token, or an SSH key if SCAN_REPO_URL is an ssh:// URL). ' +
                          'Blank = public repo, or the agent already has git access. ' +
                          'For multiple instances in a single run, use SCAN_GIT_CREDENTIALS instead.'
        )
        text(
            name:         'SCAN_GIT_CREDENTIALS',
            defaultValue: '',
            description:  'Multi-instance credentials — one "host=jenkins-credentials-id" per line, ' +
                          'mapping each git host to a Jenkins Username/Password (or Secret Text ' +
                          '"user:token") credential. Each is exported to the app per host so a single ' +
                          'run can authenticate to several GitHub/Bitbucket/GitLab instances.\n' +
                          '  bitbucket.instance2.com=bb-scanner-pat\n' +
                          '  github.enterprise.acme=ghe-scanner-pat\n' +
                          'See docs/multi-instance.md. Blank = use SCAN_CREDENTIALS_ID / ambient git auth.'
        )

        // ── Air-gapped / offline import (Case B) ───────────────────────────────
        booleanParam(
            name:         'SCAN_ALLOW_LOCAL',
            defaultValue: false,
            description:  'Permit scanning an offline copy (git bundle, file:// mirror, or local path) ' +
                          'when the source cannot be reached. Fail-closed: also set SCAN_LOCAL_ROOT. ' +
                          'Pass the bundle/path as SCAN_REPO_URL. See docs/multi-instance.md (Case B).'
        )
        string(
            name:         'SCAN_LOCAL_ROOT',
            defaultValue: '',
            description:  'Directory an offline SCAN_REPO_URL source must resolve under (required when ' +
                          'SCAN_ALLOW_LOCAL is checked). Sources outside it, plus UNC / file://host, are refused.'
        )

        // ═══════════════════════════════════════════════════════════════════════
        //  HOW TO BUILD THE SCANNER — prebuilt (fast, default) vs source.
        // ═══════════════════════════════════════════════════════════════════════
        choice(
            name:    'BUILD_MODE',
            choices: ['prebuilt', 'source'],
            description: 'How to obtain the oxide-sloc scanner binary:\n' +
                         '  prebuilt — extract the committed dist/ binary (no Rust toolchain, no vendor ' +
                         'compile, no network). Fast default; a couple of minutes end-to-end.\n' +
                         '  source   — build from source with the air-gapped Rust toolchain + vendored ' +
                         'crates. Slower; use for release verification. Automatically selected when ' +
                         'RUN_QUALITY_GATES or RUN_COVERAGE is checked (those need the toolchain).'
        )

        // ═══════════════════════════════════════════════════════════════════════
        //  OPTIONAL — everything below is pre-set to sensible defaults. For a
        //  standard fast scan, ignore it all: set SCAN_REPO_URL above and hit Build.
        // ═══════════════════════════════════════════════════════════════════════

        // ── Optional — tooling repo the scanner is built/sourced from ──────────
        string(
            name:         'TOOL_REPO_URL',
            defaultValue: '',
            description:  '(optional) Tooling repo oxide-sloc itself comes from — NOT the project to scan ' +
                          '(use SCAN_REPO_URL for that). BLANK = reuse the SCM this job was configured ' +
                          'from (a local mirror on an air-gapped controller) — no internet URL is ever ' +
                          'assumed. Resolution order: this parameter → the TOOL_REPO_URL environment ' +
                          'variable (e.g. a Jenkins global property sourced from ci/jenkins/.env) → the ' +
                          'job\'s own SCM. Set this only to override with a fork/mirror URL.'
        )
        string(
            name:         'TOOL_REPO_BRANCH',
            defaultValue: '',
            description:  '(optional) Branch/ref spec used only when TOOL_REPO_URL is set (e.g. */main, ' +
                          '*/develop, refs/tags/v1.2.0). BLANK = the TOOL_REPO_BRANCH environment variable, ' +
                          'else */main. Ignored when TOOL_REPO_URL is blank — the job\'s own SCM branch is used.'
        )
        string(
            name:         'REPORT_TITLE',
            defaultValue: 'oxide-sloc CI Report',
            description:  '(optional) Title shown in the generated HTML and PDF reports. ' +
                          'When LEFT AT THIS DEFAULT (or blank), the pipeline auto-derives a ' +
                          'project-scoped title "<RepoName> CI SLOC Report" from the scanned ' +
                          'repo name (SCAN_REPO_URL, else the tooling repo) so the report ' +
                          'identifies the project. Set any other value to use it verbatim.'
        )
        string(
            name:         'OUTPUT_SUBDIR',
            defaultValue: 'ci-out',
            description:  '(optional) Workspace sub-directory for generated artifacts (letters, digits, - _ / only).'
        )

        // ── Pipeline switches (optional) ───────────────────────────────────────
        booleanParam(
            name:         'RUN_QUALITY_GATES',
            defaultValue: false,
            description:  'Build from source and run the Format / Lint / Unit tests stage. ' +
                          'OFF by default for a fast prebuilt scan; check it to enforce code quality. ' +
                          'Implies BUILD_MODE=source (the toolchain is needed to compile and lint).'
        )
        booleanParam(
            name:         'RUN_WEB_HEALTHCHECK',
            defaultValue: false,
            description:  'Run the web UI health-check stage (binds the local server briefly). ' +
                          'OFF by default; needs loopback access and a free port 4317 on the agent.'
        )
        booleanParam(
            name:         'RUN_ANALYZE_SELFTEST',
            defaultValue: false,
            description:  'Re-scan the repo in the extra analyzer modes (per-file breakdown + the ' +
                          'four mixed-line policies) to smoke-test every code path. These passes ' +
                          'produce no artifacts — they only verify the binary. OFF by default: they ' +
                          'add ~5 redundant full-repo walks on top of the artifact run, which is slow ' +
                          'on large repos and AV-scanned agents. Turn on for release verification of ' +
                          'the scanner itself.'
        )
        booleanParam(
            name:         'RUN_ATTRIBUTION',
            defaultValue: true,
            description:  'Compute per-author code-ownership metrics via git blame. Requires full git history.'
        )
        // ── CI config preset ───────────────────────────────────────────────────
        choice(
            name:    'CI_PRESET',
            choices: ['default', 'none', 'strict', 'full-scope'],
            description: 'CI configuration preset loaded from the ci/ directory:\n' +
                         '  none        — no preset; individual flags below control everything\n' +
                         '  default     — balanced defaults, mirrors web UI defaults  (ci/sloc-ci-default.toml)\n' +
                         '  strict      — fail the pipeline if binary files are found  (ci/sloc-ci-strict.toml)\n' +
                         '  full-scope  — count everything including vendor and lockfiles  (ci/sloc-ci-full-scope.toml)'
        )

        // ── Output formats ─────────────────────────────────────────────────────
        booleanParam(
            name:         'REPORT_HTML',
            defaultValue: true,
            description:  'Write an HTML report artifact and publish it via the HTML Publisher plugin. ' +
                          'Appears as "SLOC Report" in the left-hand build menu. ' +
                          'Requires the "HTML Publisher" plugin — see ci/jenkins/plugins.txt.'
        )
        booleanParam(
            name:         'REPORT_PDF',
            defaultValue: true,
            description:  'Write a PDF report artifact alongside the HTML report. ' +
                          'Pure-Rust generation — no browser or external tool required on the agent. ' +
                          'When enabled, the "View PDF" button in the HTML report opens the archived PDF directly.'
        )

        // ── Analysis rules ─────────────────────────────────────────────────────
        choice(
            name:    'MIXED_LINE_POLICY',
            choices: ['code-only', 'code-and-comment', 'comment-only', 'separate-mixed-category'],
            description: 'How lines containing both code and an inline comment are classified. ' +
                         'Overridden by preset when CI_PRESET is not "none".\n' +
                         '  code-only               — count toward code total (default)\n' +
                         '  code-and-comment        — count toward both code and comment totals\n' +
                         '  comment-only            — count toward comment total only\n' +
                         '  separate-mixed-category — tracked in a dedicated "mixed" bucket'
        )
        booleanParam(
            name:         'DOCSTRINGS_AS_CODE',
            defaultValue: false,
            description:  'Count Python triple-quoted docstrings as code lines instead of comment lines.'
        )
        booleanParam(
            name:         'SUBMODULE_BREAKDOWN',
            defaultValue: true,
            description:  'Detect .gitmodules and emit per-submodule stats in the report.'
        )
        string(
            name:         'STYLE_COL_THRESHOLD',
            defaultValue: '80',
            description:  'Column-width threshold for Code Style N-col compliance reporting. ' +
                          'Supported values: 80, 100, 120. Controls the "N-Col Compliant" chip ' +
                          'in HTML/PDF reports and the style_col_compliant_pct metric in CI output.'
        )
        string(
            name:         'ACTIVITY_WINDOW',
            defaultValue: '90',
            description:  'Git Hotspots activity window in days (on by default). oxide-sloc runs one ' +
                          'git log pass over the last N days and ranks files by code lines x recent ' +
                          'commits in a Git Hotspots section (HTML/PDF) plus Commits/Last-changed ' +
                          'columns in CSV. Set 0 to disable. Requires SCAN_PATH to be a git checkout.'
        )
        booleanParam(
            name:         'FOLLOW_SYMLINKS',
            defaultValue: false,
            description:  'Follow symbolic links during file discovery.'
        )
        booleanParam(
            name:         'NO_IGNORE_FILES',
            defaultValue: false,
            description:  'Ignore .gitignore / .slocignore rules — scan everything under SCAN_PATH.'
        )
        string(
            name:         'ENABLED_LANGUAGES',
            defaultValue: '',
            description:  'Comma-separated language filter — restricts analysis to the listed languages. ' +
                          'Example: rust,python,javascript   (empty = all 60 supported languages)'
        )
        string(
            name:         'INCLUDE_GLOBS',
            defaultValue: '',
            description:  'Comma-separated include glob patterns. ' +
                          'Only files matching at least one pattern are analyzed. ' +
                          'Example: src/**/*.py,scripts/*.sh   (empty = all files)'
        )
        string(
            name:         'EXCLUDE_GLOBS',
            defaultValue: '',
            description:  'Comma-separated exclude glob patterns. ' +
                          'Files matching any pattern are skipped entirely. ' +
                          'Example: vendor/**,**/*.min.js   (empty = nothing excluded)'
        )

        // ── Git-ref comparison ─────────────────────────────────────────────────
        string(
            name:         'COMPARE_REF',
            defaultValue: '',
            description:  'Scan the repository at this specific git ref (branch, tag, or commit SHA). ' +
                          'Leave empty to scan HEAD (the checked-out commit). ' +
                          'When set, oxide-sloc creates a temporary worktree, scans it, then removes it. ' +
                          'Example: v1.4.0  or  refs/tags/v1.4.0  or  a3f9d2c'
        )
        string(
            name:         'COMPARE_BASELINE_REF',
            defaultValue: '',
            description:  'Compare the COMPARE_REF scan against this baseline ref. ' +
                          'Produces a JSON/HTML diff report alongside the normal scan output. ' +
                          'Leave empty to skip comparison. Example: v1.3.7'
        )
        booleanParam(
            name:         'COMPARE_PREV_TAG',
            defaultValue: false,
            description:  'Automatically detect the previous release tag (the one before COMPARE_REF or HEAD) ' +
                          'and use it as the baseline for comparison. Overrides COMPARE_BASELINE_REF when set.'
        )

        // ── Test runner & results ──────────────────────────────────────────────
        choice(
            name:    'TEST_RUNNER',
            choices: ['cargo-test', 'cargo-nextest'],
            description: 'Test runner for the Unit tests stage. Default cargo-test so a fresh ' +
                         'agent with no extra tooling passes green. Switch to cargo-nextest to ' +
                         'get the "Test Result" view + trend (requires cargo-nextest on the ' +
                         'agent — see docs/jenkins-manual-setup.md Step 13). If cargo-nextest ' +
                         'is selected but is not on the agent and the offline install fails, ' +
                         'the stage falls back to cargo test and marks the build UNSTABLE ' +
                         '(yellow) so the missing JUnit report is visible.\n' +
                         '  cargo-test    — standard stable cargo test; console output only (no JUnit XML)\n' +
                         '  cargo-nextest — faster parallel runner with JUnit XML output (needs ' +
                         'cargo-nextest on the agent; "cargo install --locked cargo-nextest"). ' +
                         'Publishes the "Test Result" trend when PUBLISH_TEST_RESULTS is checked.'
        )
        booleanParam(
            name:         'PUBLISH_TEST_RESULTS',
            defaultValue: true,
            description:  'Publish JUnit XML test results to Jenkins (requires TEST_RUNNER = cargo-nextest). ' +
                          'Results appear as a "Test Result" sidebar link, per-build trend chart, and build badge. ' +
                          'Has no effect when TEST_RUNNER is "cargo-test" (no XML is generated).'
        )
        booleanParam(
            name:         'TEST_FAIL_FAST',
            defaultValue: false,
            description:  'Stop on the first test failure instead of running all tests to completion. ' +
                          'Useful for fast feedback on a known-broken area; leave unchecked to see all failures at once.'
        )

        // ── Standalone code coverage ───────────────────────────────────────────
        booleanParam(
            name:         'RUN_COVERAGE',
            defaultValue: false,
            description:  'Run a dedicated Coverage stage (builds from source; implies BUILD_MODE=source). ' +
                          'Generates LCOV, Cobertura XML, and a browsable HTML coverage report. ' +
                          'The HTML report is published as a "Coverage Source" sidebar link. ' +
                          'Requires cargo-llvm-cov (preferred) or cargo-tarpaulin on the agent:\n' +
                          '  cargo install cargo-llvm-cov && rustup component add llvm-tools\n' +
                          'cargo-llvm-cov is vendored in ci/tools/Cargo.toml for air-gapped installs.'
        )
        string(
            name:         'COVERAGE_THRESHOLD',
            defaultValue: '0',
            description:  'Minimum line-coverage percentage required to pass the build (0 = disabled). ' +
                          'Only enforced when RUN_COVERAGE is enabled. ' +
                          'Coverage percentage is derived from the LCOV lcov.info summary lines (LH / LF). ' +
                          'Example: 60  fails the build if fewer than 60 % of lines are covered.'
        )

        // ── Delivery / notifications ───────────────────────────────────────────
        string(
            name:         'NOTIFY_WEBHOOK_URL',
            defaultValue: '',
            description:  'POST the JSON result to this URL after a successful scan (empty = skip). ' +
                          'Add SLOC_WEBHOOK_TOKEN as a Jenkins Secret Text credential for Bearer auth.'
        )
        string(
            name:         'NOTIFY_EMAIL',
            defaultValue: '',
            description:  'Comma-separated email addresses to receive the scan report (empty = skip). ' +
                          'Requires Jenkins Secret Text credentials: SLOC_SMTP_HOST, SLOC_SMTP_USER, SLOC_SMTP_PASS.'
        )
        string(
            name:         'POOL_INGEST_URL',
            defaultValue: '',
            description:  'Base URL of a central oxide-sloc "serve" instance to POST this report to, ' +
                          'so it pools with reports from other environments/agents on one dashboard ' +
                          '(e.g. https://sloc.corp.internal:4317). The report is sent to <URL>/api/ingest ' +
                          'via the `send` subcommand. Blank = disabled. This push is best-effort: a ' +
                          'failed pool push logs a warning and never fails the build. Private/RFC-1918 ' +
                          'targets are permitted (--allow-private-net).'
        )
        string(
            name:         'POOL_INGEST_TOKEN_CREDENTIAL',
            defaultValue: '',
            description:  '(optional) Jenkins Secret Text credential ID holding the API key for the ' +
                          'POOL_INGEST_URL server. When set, it is bound to SLOC_WEBHOOK_TOKEN in the ' +
                          'environment for the pool push (never passed on the command line, so it stays ' +
                          'out of argv and logs). Blank = push unauthenticated (only for an open pool server).'
        )

        // ── Artifact repository ────────────────────────────────────────────────
        // Push scan artifacts (JSON, HTML, PDF) to an external artifact repository
        // after each successful build.  Supported backends: JFrog Artifactory,
        // Sonatype Nexus 3 & 2, AWS S3, MinIO, Azure Blob Storage, and any server
        // that accepts HTTP PUT.  Set ARTIFACT_REPO_TYPE to "none" (the default) to
        // skip this stage entirely.
        //
        // Credentials — add these as Jenkins Secret Text credentials before use:
        //   SLOC_ARTIFACT_REPO_USER  username or access-key ID
        //   SLOC_ARTIFACT_REPO_PASS  password, API token, account key, or secret key
        //
        // The actual push is performed by ci/artifact-push.sh.  See that script and
        // docs/ci-integrations.md § Artifact Repository Integration for full details.
        choice(
            name:    'ARTIFACT_REPO_TYPE',
            choices: ['none', 'artifactory', 'nexus', 'nexus2', 's3', 'minio', 'azure-blob', 'generic-http'],
            description: 'Artifact repository backend to push scan results to after a successful build.\n' +
                         '  none         — skip artifact repository push (default)\n' +
                         '  artifactory  — JFrog Artifactory (REST PUT, user/pass or API key)\n' +
                         '  nexus        — Sonatype Nexus Repository Manager 3 (raw-format REST upload)\n' +
                         '  nexus2       — Sonatype Nexus Repository Manager 2 (PUT to content REST API)\n' +
                         '  s3           — Amazon S3 (requires aws CLI on the agent)\n' +
                         '  minio        — MinIO via aws CLI with custom --endpoint-url\n' +
                         '  azure-blob   — Azure Blob Storage (requires az CLI on the agent)\n' +
                         '  generic-http — any HTTP/HTTPS server accepting PUT requests'
        )
        string(
            name:         'ARTIFACT_REPO_URL',
            defaultValue: '',
            description:  'Base URL of the artifact repository (leave empty to skip push).\n' +
                          '  Artifactory : https://repo.example.com/artifactory/sloc-reports\n' +
                          '  Nexus 3     : https://nexus.example.com\n' +
                          '  Nexus 2     : https://nexus.example.com/nexus\n' +
                          '  S3 / MinIO  : s3://my-bucket\n' +
                          '  Azure Blob  : https://myaccount.blob.core.windows.net\n' +
                          '  Generic HTTP: https://artifacts.example.com/sloc'
        )
        string(
            name:         'ARTIFACT_REPO_PATH',
            defaultValue: 'oxide-sloc/${JOB_NAME}/${BUILD_NUMBER}',
            description:  'Path prefix / key prefix under which artifacts are stored in the repository. ' +
                          'The tokens ${JOB_NAME} and ${BUILD_NUMBER} are substituted at runtime. ' +
                          'For Nexus 3/2 this is the directory within the raw repository. ' +
                          'For Azure Blob this is the blob name prefix within the container. ' +
                          'Leading slashes are stripped automatically.'
        )
        string(
            name:         'ARTIFACT_REPO_EXTRA',
            defaultValue: '',
            description:  'Provider-specific extra configuration (leave empty when unused):\n' +
                          '  nexus / nexus2 — Nexus repository name (e.g. sloc-raw-hosted)\n' +
                          '  azure-blob     — storage container name (e.g. sloc-reports)\n' +
                          '  minio          — MinIO server endpoint URL (e.g. https://minio.internal:9000)\n' +
                          '  s3             — extra flags for aws s3 cp (e.g. --sse aws:kms)\n' +
                          '  others         — unused'
        )
        booleanParam(
            name:         'ARTIFACT_PUSH_JSON',
            defaultValue: true,
            description:  'Include result.json in the artifact repository push.'
        )
        booleanParam(
            name:         'ARTIFACT_PUSH_HTML',
            defaultValue: true,
            description:  'Include report.html in the artifact repository push ' +
                          '(only when REPORT_HTML is checked).'
        )
        booleanParam(
            name:         'ARTIFACT_PUSH_PDF',
            defaultValue: false,
            description:  'Include report.pdf in the artifact repository push ' +
                          '(only when REPORT_PDF is checked).'
        )
        booleanParam(
            name:         'ARTIFACT_PUSH_CSV',
            defaultValue: true,
            description:  'Include report.csv in the artifact repository push. ' +
                          'CSV is always generated — no separate output flag required.'
        )
        booleanParam(
            name:         'ARTIFACT_PUSH_XLSX',
            defaultValue: false,
            description:  'Include report.xlsx (Excel workbook) in the artifact repository push. ' +
                          'XLSX is always generated — no separate output flag required.'
        )
        booleanParam(
            name:         'ARTIFACT_PUSH_BINARY',
            defaultValue: false,
            description:  'Include the compiled oxide-sloc binary in the artifact repository push. ' +
                          'The binary is copied from target/release/ into the artifact directory before upload.'
        )
        booleanParam(
            name:         'ARTIFACT_PUSH_JUNIT',
            defaultValue: false,
            description:  'Include junit.xml test results in the artifact repository push. ' +
                          'Only applies when PUBLISH_TEST_RESULTS is checked and TEST_RUNNER is cargo-nextest.'
        )
        booleanParam(
            name:         'ARTIFACT_PUSH_COVERAGE',
            defaultValue: false,
            description:  'Include lcov.info and sonar-coverage.xml coverage reports in the push. ' +
                          'Only applies when RUN_COVERAGE is checked.'
        )
        booleanParam(
            name:         'ARTIFACT_PUSH_DIFF',
            defaultValue: false,
            description:  'Include diff.json and diff.csv diff-comparison artifacts in the push. ' +
                          'Only applies when COMPARE_REF is set and a comparison ref is configured.'
        )
        booleanParam(
            name:         'ARTIFACT_GENERATE_MANIFEST',
            defaultValue: false,
            description:  'Generate and upload a SHA-256 checksum manifest (checksums.sha256) ' +
                          'listing the hash of every pushed artifact alongside the files themselves.'
        )

        // ── Atlassian Suite: Confluence + Bitbucket (optional) ─────────────────
        // Publish the finished report to Confluence and post a commit build-status
        // to Bitbucket after every build (see post{always} → runBitbucketNotify).
        // Both are fully opt-in and plugin-independent: they no-op (exit 0) when
        // their base URL or credential is absent, so leaving these blank changes
        // nothing.  The API tokens are NOT parameters — add them as Jenkins Secret
        // Text credentials so they are masked in the log:
        //   confluence-api-token   Confluence PAT (Server/DC) or Cloud API token
        //   bitbucket-build-token  Bitbucket HTTP access token / app password
        // The full HTML + PDF report is uploaded as a page attachment when present.
        // See docs/ci-integrations.md § Atlassian and
        // testing/atlassian-integration-test-plan.md.
        string(
            name:         'CONFLUENCE_BASE_URL',
            defaultValue: '',
            description:  '(optional) Confluence base URL. Server/DC: https://confluence.example.com ; ' +
                          'Cloud: https://your-domain.atlassian.net/wiki . Blank = skip Confluence publish.'
        )
        string(
            name:         'CONFLUENCE_USER',
            defaultValue: '',
            description:  '(optional) Confluence Cloud account email for basic auth (email:api-token). ' +
                          'Leave BLANK for Server/DC when confluence-api-token is a Personal Access Token (Bearer).'
        )
        string(
            name:         'CONFLUENCE_SPACE_KEY',
            defaultValue: '',
            description:  '(optional) Confluence space key to publish the summary page into (e.g. DEV). ' +
                          'Required for the Confluence publish to run.'
        )
        string(
            name:         'CONFLUENCE_PARENT_ID',
            defaultValue: '',
            description:  '(optional) Parent page ID to nest the report page under (blank = space root).'
        )
        string(
            name:         'CONFLUENCE_PAGE_TITLE',
            defaultValue: '',
            description:  '(optional) Page title to create/update. Blank = "oxide-sloc — <JOB_NAME>". ' +
                          'Re-using the same title updates the existing page (new version) instead of duplicating.'
        )
        string(
            name:         'BITBUCKET_BASE_URL',
            defaultValue: '',
            description:  '(optional) Bitbucket base URL for commit build-status. Server/DC: ' +
                          'https://bitbucket.example.com ; Cloud: https://api.bitbucket.org . Blank = skip.'
        )
        string(
            name:         'BITBUCKET_WORKSPACE',
            defaultValue: '',
            description:  '(optional) Bitbucket CLOUD workspace ID (Cloud only; ignored on Server/DC).'
        )
        string(
            name:         'BITBUCKET_REPO',
            defaultValue: '',
            description:  '(optional) Bitbucket CLOUD repository slug (Cloud only; ignored on Server/DC).'
        )
        string(
            name:         'BITBUCKET_USER',
            defaultValue: '',
            description:  '(optional) Bitbucket CLOUD username for app-password auth. When set, ' +
                          'notify-bitbucket.sh uses Basic auth (user:app_password) — required for Cloud ' +
                          'APP PASSWORDS. Leave BLANK for a Cloud/Server ACCESS TOKEN or Server PAT ' +
                          '(Bearer auth). See docs/ci-integrations.md § Atlassian auth schemes.'
        )

        // ── Pipeline-of-Pipelines chaining ─────────────────────────────────────
        // Chain this scan into a larger pipeline. Set CHAIN_DOWNSTREAM_JOB to fan out
        // to the next job on success; this build passes CHAIN_UPSTREAM_JOB /
        // CHAIN_UPSTREAM_BUILD / ARTIFACT_PATH forward so the downstream job can locate
        // this run's artifacts. An orchestrator triggers this job with:
        //   build job: 'oxide-sloc', parameters: [
        //     string(name: 'SCAN_REPO_URL',        value: '...'),
        //     string(name: 'CHAIN_DOWNSTREAM_JOB', value: 'next-pipeline')], wait: true
        string(name: 'CHAIN_UPSTREAM_JOB',   defaultValue: '', description: '(chaining) Name of the upstream pipeline that triggered this build')
        string(name: 'CHAIN_UPSTREAM_BUILD', defaultValue: '', description: '(chaining) Build number of the upstream job')
        string(name: 'CHAIN_DOWNSTREAM_JOB', defaultValue: '', description: '(chaining) Pipeline job to trigger on success (blank = disable). It receives CHAIN_UPSTREAM_JOB, CHAIN_UPSTREAM_BUILD, and ARTIFACT_PATH.')

        // ── Agent selection ────────────────────────────────────────────────────
        string(
            name:         'AGENT_LABEL',
            defaultValue: '',
            description:  '(optional) Node label to pin this build to. BLANK = any available agent. ' +
                          'The pipeline drives every stage through POSIX .sh scripts; on a LINUX ' +
                          'agent they run natively, on a WINDOWS agent they run through Git Bash ' +
                          '(install Git for Windows, or set the SLOC_BASH env var to a bash.exe path). ' +
                          'Set this to a Linux label (e.g. "linux" or "built-in") to avoid the ' +
                          'Git-Bash requirement on mixed Windows/Linux controllers.'
        )
    }

    // NOTE: the OS-dependent environment variables (CARGO_HOME, RUSTUP_HOME, PATH,
    // BINARY, ARTIFACT_PATH, RUST_LOG) are NOT set in a declarative environment{}
    // block, because that block cannot branch on isUnix() — and the old POSIX-only
    // PATH ("…/cargo/bin:/usr/local/bin:/usr/bin:/bin") destroyed the inherited
    // Windows PATH (System32/Git/Python/cargo) on a Windows agent. They are instead
    // set by h.initEnv() in the "Load helpers" stage below, which:
    //   * Unix   — sets values BYTE-IDENTICAL to what this block used to set;
    //   * Windows — PREPENDS the cache cargo/bin dir to the inherited PATH (never
    //     replaces it), normalises HOME←USERPROFILE, and points BINARY /
    //     ARTIFACT_PATH at the .exe (POSIX-style path Git Bash accepts).
    // The persistent Rust toolchain cache lives in the agent user's home so it
    // survives cleanWs() across builds. One-time setup per agent:
    //   Docker : rebuild ci/jenkins/Dockerfile.agent (toolchain baked at /opt/rust-toolchain)
    //   Native : sudo bash ci/jenkins/install-system-deps.sh
    //            bash ci/jenkins/install-rust-cache.sh

    stages {

        // ── 0. Checkout ────────────────────────────────────────────────────────
        // Always check out the tooling repo (TOOL_REPO_URL) at the workspace root — the
        // scanner is built from it. When SCAN_REPO_URL is set, also check the
        // project-under-analysis out into ./_target and point SCAN_ROOT at it, so a
        // single job can scan any project. SCAN_ROOT is consumed by the analyze,
        // git-ref, and compare stages; it defaults to the workspace root (self-scan).
        stage('Checkout') {
            steps {
                script {
                    // Resolve the tooling repo (the scanner is built from it) WITHOUT
                    // ever hardcoding an internet URL — this is what makes the pipeline
                    // work unchanged on an air-gapped controller. Priority order:
                    //   1. TOOL_REPO_URL build parameter    (explicit per-build override)
                    //   2. TOOL_REPO_URL environment variable (Jenkins global property /
                    //      node env, typically sourced from ci/jenkins/.env) — the
                    //      air-gap knob: point it at your local mirror once.
                    //   3. the job's OWN configured SCM (checkout scm) — on an
                    //      air-gapped controller the job already points at a local
                    //      mirror, so a no-parameter build never touches github.com.
                    //
                    // Capture the checkout's commit explicitly: with
                    // skipDefaultCheckout(true) + a scripted checkout, the git plugin's
                    // implicit env.GIT_COMMIT is fragile (reflects whichever checkout
                    // ran last). runBitbucketNotify() needs a real SHA or it treats
                    // itself as "not configured" and skips.
                    // TOOL_REPO_URL is the current name; env.REPO_URL / env.REPO_BRANCH
                    // are accepted as a back-compat fallback so controllers that already
                    // set the legacy Jenkins global property keep working after the rename.
                    def repoUrl = params.TOOL_REPO_URL?.trim() ?: env.TOOL_REPO_URL?.trim() ?: env.REPO_URL?.trim()
                    def scmVars
                    if (repoUrl) {
                        def branch = params.TOOL_REPO_BRANCH?.trim() ?: env.TOOL_REPO_BRANCH?.trim() ?:
                                     env.REPO_BRANCH?.trim() ?: '*/main'
                        echo "Checkout: tooling repo from TOOL_REPO_URL=${repoUrl} (branch: ${branch})"
                        // Full (non-shallow) history: git blame (--attribution) and the
                        // 90-day Git Hotspots pass both walk history, which a shallow clone
                        // truncates. depth:0 + noTags:false keep the whole graph + tags.
                        scmVars = checkout([$class: 'GitSCM',
                                            branches: [[name: branch]],
                                            extensions: [[$class: 'CloneOption',
                                                          shallow: false, honorRefspec: true,
                                                          noTags: false, depth: 0]],
                                            userRemoteConfigs: [[url: repoUrl]]])
                    } else {
                        // No explicit URL: reuse whatever SCM this job was configured
                        // from. Guarded so a Pipeline-script (non-SCM) job fails with a
                        // clear, actionable message instead of a raw MissingProperty.
                        echo 'Checkout: TOOL_REPO_URL unset — reusing the job\'s own SCM (checkout scm).'
                        try {
                            scmVars = checkout scm
                        } catch (Throwable t) {
                            error("No TOOL_REPO_URL was provided and this job has no SCM to fall " +
                                  "back to (checkout scm failed: ${t.message}). Set the TOOL_REPO_URL " +
                                  "build parameter, or a TOOL_REPO_URL environment variable " +
                                  "(see ci/jenkins/.env.example), or run this pipeline as a " +
                                  "Pipeline-from-SCM job.")
                        }
                    }
                    env.GIT_COMMIT = scmVars.GIT_COMMIT ?: env.GIT_COMMIT
                    // Expose the effective URL so the analyze stage can derive a stable
                    // project slug (repo-name_shortsha) for a self-scan even when
                    // TOOL_REPO_URL was blank and we fell back to checkout scm.
                    env.SLOC_REPO_URL_EFFECTIVE = scmVars.GIT_URL ?: repoUrl ?: ''
                }
                script {
                    if (params.SCAN_REF?.trim() &&
                            !(params.SCAN_REF.trim() ==~ /^[A-Za-z0-9_\-\.\/]+$/)) {
                        error("SCAN_REF contains invalid characters: ${params.SCAN_REF}")
                    }
                    if (params.SCAN_CREDENTIALS_ID?.trim() &&
                            !(params.SCAN_CREDENTIALS_ID.trim() ==~ /^[A-Za-z0-9_\-\.]+$/)) {
                        error("SCAN_CREDENTIALS_ID contains invalid characters: ${params.SCAN_CREDENTIALS_ID}")
                    }
                    // Air-gap offline import (Case B): expose the app-side local gate so
                    // any app-driven git step honours it. The Jenkins git plugin clones a
                    // bundle / file:// path directly regardless; these are for completeness
                    // and parity with the CLI / server. Off unless explicitly enabled.
                    if (params.SCAN_ALLOW_LOCAL) {
                        env.SLOC_GIT_ALLOW_LOCAL = '1'
                        if (params.SCAN_LOCAL_ROOT?.trim()) {
                            env.SLOC_GIT_LOCAL_ROOT = params.SCAN_LOCAL_ROOT.trim()
                        }
                    }
                    if (params.SCAN_REPO_URL?.trim()) {
                        def ref = params.SCAN_REF?.trim() ?: 'main'
                        // A bare name (e.g. "v1.1") may be a branch OR a tag. Fetch both
                        // heads and tags via an explicit refspec, and offer both
                        // resolutions — the git plugin builds the first that resolves to a
                        // revision. A SHA or an explicit ref path ("refs/…", "origin/…")
                        // is used verbatim. Without the tags refspec a tag ref fails the
                        // checkout with "Couldn't find any revision to build".
                        def isSha = ref ==~ /^[0-9a-fA-F]{7,40}$/
                        def branchList = (isSha || ref.contains('/'))
                            ? [[name: ref]]
                            : [[name: "*/${ref}"], [name: "refs/tags/${ref}"]]
                        // Multi-instance credential selection. Pick the Jenkins credential
                        // for the target's HOST from the SCAN_GIT_CREDENTIALS map
                        // ("host=jenkins-credentials-id" per line); fall back to the single
                        // SCAN_CREDENTIALS_ID. This lets ONE job scan many GitHub/Bitbucket/
                        // GitLab instances, each authenticated with its own Jenkins
                        // credential, without editing the job — the git plugin injects the
                        // secret natively during checkout (never in env / argv / URL).
                        def scanUrl = params.SCAN_REPO_URL.trim()
                        def hostOf = { String u ->
                            def m = (u =~ /^[a-zA-Z][a-zA-Z0-9+.\-]*:\/\/(?:[^@\/]+@)?([^\/:]+)/)
                            if (m) { return m[0][1].toLowerCase() }
                            def s = (u =~ /^(?:[^@\/]+@)?([^:\/]+):/)   // scp-like git@host:path
                            return s ? s[0][1].toLowerCase() : ''
                        }
                        def credForHost = null
                        def targetHost = hostOf(scanUrl)
                        if (params.SCAN_GIT_CREDENTIALS?.trim() && targetHost) {
                            for (def line in params.SCAN_GIT_CREDENTIALS.trim().readLines()) {
                                def t = line.trim()
                                if (!t || t.startsWith('#') || !t.contains('=')) { continue }
                                def host = t.substring(0, t.indexOf('=')).trim().toLowerCase()
                                def cid  = t.substring(t.indexOf('=') + 1).trim()
                                if (host == targetHost && cid) { credForHost = cid; break }
                            }
                        }
                        def effCredId = credForHost ?: params.SCAN_CREDENTIALS_ID?.trim()
                        if (effCredId && !(effCredId ==~ /^[A-Za-z0-9_\-\.]+$/)) {
                            error("Resolved scan credentials ID contains invalid characters: ${effCredId}")
                        }
                        // Attach the credential only when resolved, so public repos
                        // (and agents with ambient git auth) keep working unchanged.
                        def remoteCfg = [
                            url:     scanUrl,
                            refspec: '+refs/heads/*:refs/remotes/origin/* +refs/tags/*:refs/tags/*',
                        ]
                        if (effCredId) {
                            remoteCfg['credentialsId'] = effCredId
                        }
                        dir('_target') {
                            // Full (non-shallow) history so git blame (--attribution) and
                            // the 90-day Git Hotspots pass have the complete commit graph;
                            // a shallow clone would truncate blame and hotspot ranking.
                            def tgtVars = checkout([$class: 'GitSCM',
                                                    branches: branchList,
                                                    extensions: [[$class: 'CloneOption',
                                                                  shallow: false, honorRefspec: true,
                                                                  noTags: false, depth: 0]],
                                                    userRemoteConfigs: [remoteCfg]])
                            // When scanning an external project, the Bitbucket
                            // build-status must attach to the SCANNED commit (the one
                            // being analyzed), not the tooling repo's HEAD — mirror the
                            // SLOC_PROJECT short-SHA logic, which also uses the target.
                            if (tgtVars?.GIT_COMMIT?.trim()) {
                                env.GIT_COMMIT = tgtVars.GIT_COMMIT
                            }
                        }
                        env.SCAN_ROOT = "${env.WORKSPACE}/_target"
                        def credNote = effCredId
                            ? " (creds: ${effCredId}${credForHost ? " — matched host ${targetHost}" : ''})"
                            : ''
                        echo "Scanning external project at ${env.SCAN_ROOT} (ref: ${ref})${credNote}"
                    } else {
                        env.SCAN_ROOT = env.WORKSPACE
                    }
                }
            }
        }

        // ── 0b. Load helpers ───────────────────────────────────────────────────
        // Loads ci/jenkins/pipeline-helpers.groovy as a separate compiled class so
        // its methods get their own 64 KB bytecode budget, keeping this script lean.
        stage('Load helpers') {
            steps {
                script {
                    h = load 'ci/jenkins/pipeline-helpers.groovy'
                    // Set the OS-aware environment (CARGO_HOME / RUSTUP_HOME / PATH /
                    // BINARY / ARTIFACT_PATH / RUST_LOG). Must run before Setup and any
                    // shx() step so cargo/rustc resolve and BINARY carries .exe on
                    // Windows. See the note above the (removed) environment{} block.
                    h.initEnv()
                    // Decide once whether this build needs the Rust toolchain + a
                    // from-source compile. The fast default (BUILD_MODE=prebuilt) extracts
                    // the committed dist/ binary and skips toolchain, vendor, fmt/clippy,
                    // and tests entirely. Quality gates and coverage compile the workspace,
                    // so either of those forces a source build regardless of BUILD_MODE.
                    def needsSource = (params.BUILD_MODE == 'source') ||
                                      params.RUN_QUALITY_GATES || params.RUN_COVERAGE
                    if (needsSource && params.BUILD_MODE != 'source') {
                        echo 'BUILD_MODE=prebuilt overridden to source: RUN_QUALITY_GATES / ' +
                             'RUN_COVERAGE require compiling the workspace.'
                    }
                    // Auto-fallback (ON BY DEFAULT): the fast prebuilt path only works when a
                    // dist/ binary for THIS platform is committed in the checkout. On any other
                    // environment (a fresh clone, a fork, or a platform with no committed dist/
                    // archive) that binary is absent, and the old behaviour hard-failed the Build
                    // stage with "no prebuilt Windows binary in dist/". Instead, detect the
                    // missing archive HERE — before the toolchain/vendor Setup stage, which is
                    // gated on SLOC_NEEDS_SOURCE — and transparently compile from the vendored
                    // crates. Prebuilt stays a best-effort accelerator; it never blocks a build.
                    if (!needsSource) {
                        def distPresent
                        if (isUnix()) {
                            def arch = h.shxStdout('uname -m 2>/dev/null || echo x86_64')
                                .readLines().findAll { it?.trim() }.last()?.trim() ?: 'x86_64'
                            arch = (arch == 'aarch64' || arch == 'arm64') ? 'arm64' : 'x86_64'
                            distPresent = fileExists("dist/oxide-sloc-linux-${arch}.tar.gz")
                        } else {
                            distPresent = fileExists('dist/oxide-sloc-windows-x64.tar.gz') ||
                                          fileExists('dist/oxide-sloc-windows-x64.zip')
                        }
                        if (!distPresent) {
                            needsSource = true
                            // Diagnostics: reveal WHY the prebuilt binary wasn't found so the
                            // next run distinguishes mirror-lag (no dist/ dir at all) from a
                            // partial/sparse clone (dir present, large *.tar.gz blobs missing).
                            // pwd()/fileExists() are pure cross-platform steps; the listing runs
                            // through shx (Git Bash on Windows, sh on Linux) only when dist/ exists.
                            echo "dist/ probe: cwd=${pwd()}"
                            if (fileExists('dist')) {
                                echo 'dist/ listing:\n' + h.shxStdout(
                                    'ls -lo dist 2>/dev/null || ls -l dist').trim()
                            } else {
                                echo 'dist/ probe: (no dist/ directory in the checkout)'
                            }
                            echo 'BUILD_MODE=prebuilt requested, but no dist/ binary for this ' +
                                 'platform is committed in the checkout — automatically falling ' +
                                 'back to a SOURCE build (vendored crates + air-gapped toolchain). ' +
                                 'Commit the matching dist/ archive to restore the fast prebuilt path.'
                        }
                    }
                    env.SLOC_NEEDS_SOURCE = needsSource ? 'true' : 'false'
                    def modeMsg = needsSource
                        ? 'Build mode: SOURCE (Rust toolchain + vendored crates + compile).'
                        : 'Build mode: PREBUILT (dist/ binary; no toolchain, no tests — fast path).'
                    echo modeMsg
                }
            }
        }

        // ── 1. Setup ───────────────────────────────────────────────────────────
        // Installs the Rust toolchain (cached persistently across builds) and
        // decompresses the vendor archive so all cargo commands run fully offline.
        //
        // Toolchain resolution order (stops at first match):
        //   1. Toolchain already in RUSTUP_HOME persistent cache      → no network
        //   2. rust-toolchain-bundle.tar.xz in workspace              → air-gapped
        //   3. /opt/rust-toolchain baked into the agent image         → air-gapped
        //   4. rustup-init binary at ${RUSTUP_HOME}/../rustup-init    → semi-offline
        //   5. curl sh.rustup.rs                                       → requires internet
        //
        // Skipped entirely on the fast prebuilt path (BUILD_MODE=prebuilt with no
        // quality gates / coverage) — no toolchain or vendor archive is needed to run
        // the dist/ binary.
        stage('Setup') {
            when { expression { env.SLOC_NEEDS_SOURCE == 'true' } }
            steps {
                retry(2) { script { h.runSetup() } }
            }
        }

        // ── 2. Quality Gates ───────────────────────────────────────────────────
        // Format and Lint run in parallel; Unit tests follow.
        // Runs only when RUN_QUALITY_GATES is checked (off by default for fast scans).
        stage('Quality Gates') {
            when { expression { params.RUN_QUALITY_GATES } }
            stages {
                stage('fmt + clippy (parallel)') {
                    parallel {
                        stage('Format') {
                            // Wrapped in script{} so the OS-aware shx() helper can
                            // dispatch to native sh (Linux) or Git Bash (Windows).
                            steps { script { h.shx 'cargo fmt --all -- --check' } }
                        }
                        stage('Lint') {
                            steps {
                                script {
                                    // Guard against status-masking shell pipes in
                                    // Jenkins Groovy / ci shell (dash makes `cmd | tail`
                                    // report 0 even on failure). Fast; runs before clippy.
                                    h.shx 'bash ci/lint-pipeline-shell.sh'
                                    // Single clippy run emitting JSON: it feeds the
                                    // warnings-ng "Warnings" trend view AND still enforces
                                    // -D warnings (we re-raise the failure below, after
                                    // recording, so any issues are visible in the view even
                                    // on a failing build). Matches the repo's lint gate
                                    // (.gitlab-ci.yml, Makefile, ci/lint.sh): plain
                                    // -D warnings, no pedantic/nursery.
                                    h.shx '''
                                    set +e
                                    cargo clippy -q --workspace --all-targets --all-features \
                                        --message-format=json \
                                        -- -D warnings \
                                           -A clippy::multiple_crate_versions \
                                        > clippy.json 2> clippy-stderr.txt
                                    rc=$?
                                    echo $rc > clippy-rc.txt
                                    # Diagnostics live in clippy.json / the Warnings view; stderr
                                    # is just cargo progress (-q silences it) plus the failure
                                    # summary. Only surface it when clippy actually failed.
                                    [ "$rc" = 0 ] || cat clippy-stderr.txt
                                '''
                                    // Publish to warnings-ng. Guarded with Throwable so a
                                    // controller WITHOUT the plugin (missing recordIssues /
                                    // cargo step) still passes instead of erroring out.
                                    try {
                                        recordIssues(
                                            enabledForFailure: true,
                                            tools: [cargo(pattern: 'clippy.json',
                                                          id: 'clippy',
                                                          name: 'Clippy')]
                                        )
                                    } catch (Throwable t) {
                                        echo "recordIssues skipped (warnings-ng not installed): ${t.message}"
                                    }
                                    // Preserve the -D warnings gate: fail if clippy did.
                                    def rc = readFile('clippy-rc.txt').trim()
                                    if (rc != '0') {
                                        error("clippy failed with -D warnings (exit ${rc}). " +
                                              "See the Warnings view or the console output above.")
                                    }
                                }
                            }
                        }
                    }
                }
                stage('Unit tests') {
                    environment {
                        RUST_BACKTRACE = '1'
                    }
                    steps {
                        script { h.runUnitTests() }
                    }
                }
            }
        }

        // ── 3. Build ───────────────────────────────────────────────────────────
        // PREBUILT (default): extract the committed dist/ binary into target/release/
        // — no toolchain, no vendor compile, no network. SOURCE: compile from the
        // vendored crates with the air-gapped toolchain. h.runBuild() branches on
        // SLOC_NEEDS_SOURCE (set in Load helpers).
        stage('Build') {
            steps {
                retry(2) { script { h.runBuild() } }
            }
        }

        // ── 4. Coverage ────────────────────────────────────────────────────────
        // Produces LCOV, Cobertura XML, and browsable HTML via cargo-llvm-cov.
        // Enabled by RUN_COVERAGE; threshold enforced by COVERAGE_THRESHOLD.
        stage('Coverage') {
            when {
                expression { params.RUN_COVERAGE }
            }
            steps {
                // Never let a coverage/test hiccup cascade and wipe the rest of the
                // published dashboard: mark the build UNSTABLE and the stage FAILURE,
                // but keep going so Analyze / Archive & Publish (reports, plots,
                // warnings, HTML) still run.
                catchError(buildResult: 'UNSTABLE', stageResult: 'FAILURE') {
                    script { h.runCoverage() }
                }
            }
        }

        // ── 5. Analyze ─────────────────────────────────────────────────────────
        // Mirrors the web UI configuration flow end-to-end.
        stage('Analyze') {
            steps {
                script { h.runAnalyze() }
            }
        }

        // ── 6. Web UI health check ─────────────────────────────────────────────
        stage('Web UI health check') {
            when { expression { params.RUN_WEB_HEALTHCHECK } }
            steps {
                script { h.shx 'bash ci/jenkins/run-web-check.sh' }
            }
        }

        // ── 7. Deliver results ─────────────────────────────────────────────────
        // Optional webhook and/or email delivery via the `send` subcommand.
        stage('Deliver results') {
            when {
                expression {
                    params.NOTIFY_WEBHOOK_URL?.trim() || params.NOTIFY_EMAIL?.trim()
                }
            }
            stages {
                stage('Send webhook') {
                    when { expression { params.NOTIFY_WEBHOOK_URL?.trim() as Boolean } }
                    steps {
                        script {
                            try {
                                def outDir = "${env.WORKSPACE}/${params.OUTPUT_SUBDIR}"
                                h.shx """
                                    '${env.BINARY}' send '${outDir}/result_${env.SLOC_PROJECT ?: 'project'}.json' \\
                                        --webhook-url '${params.NOTIFY_WEBHOOK_URL}'
                                """
                            } catch (err) {
                                unstable("Send webhook failed (delivery error, scan artifacts preserved): ${err.message}")
                            }
                        }
                    }
                }
                stage('Send email') {
                    when { expression { params.NOTIFY_EMAIL?.trim() as Boolean } }
                    environment {
                        SLOC_SMTP_HOST = credentials('SLOC_SMTP_HOST')
                        SLOC_SMTP_USER = credentials('SLOC_SMTP_USER')
                        SLOC_SMTP_PASS = credentials('SLOC_SMTP_PASS')
                    }
                    steps {
                        script {
                            try {
                                def outDir  = "${env.WORKSPACE}/${params.OUTPUT_SUBDIR}"
                                def recArgs = params.NOTIFY_EMAIL.tokenize(',')
                                    .collect { "--smtp-to '${it.trim()}'" }.join(' ')
                                h.shx """
                                    '${env.BINARY}' send '${outDir}/result_${env.SLOC_PROJECT ?: 'project'}.json' \\
                                        --smtp-from "\${SLOC_SMTP_USER}" \\
                                        ${recArgs}
                                """
                            } catch (err) {
                                unstable("Send email failed (delivery error, scan artifacts preserved): ${err.message}")
                            }
                        }
                    }
                }
            }
        }

        // ── 8. Archive & Publish ───────────────────────────────────────────────
        // Writes Plot-plugin trend CSVs, archives artifacts, publishes HTML reports.
        stage('Archive & Publish') {
            steps {
                script { h.runArchivePublish() }
            }
        }

        // ── 9. Push to Artifact Repository ────────────────────────────────────
        // Pushes scan artifacts to an external repository via ci/artifact-push.sh.
        stage('Push to Artifact Repository') {
            when {
                allOf {
                    expression { params.ARTIFACT_REPO_TYPE != 'none' }
                    expression { params.ARTIFACT_REPO_URL?.trim() as Boolean }
                }
            }
            steps {
                script { h.runPushArtifacts() }
            }
        }

        // ── 10. Git-Ref Scan ──────────────────────────────────────────────────
        stage('Git-Ref Scan') {
            when {
                expression { return params.COMPARE_REF?.trim() != '' }
            }
            steps {
                script {
                    withEnv([
                        "GIT_REF=${params.COMPARE_REF}",
                        "OUTPUT_SUBDIR=${params.OUTPUT_SUBDIR}",
                        "SCAN_ROOT=${env.SCAN_ROOT ?: env.WORKSPACE}",
                    ]) {
                        h.shx 'bash ci/jenkins/run-git-ref-scan.sh'
                    }
                }
            }
        }

        // ── 11. Git-Ref Compare ────────────────────────────────────────────────
        // Compare two refs using oxide-sloc diff.
        stage('Git-Ref Compare') {
            when {
                expression {
                    return params.COMPARE_BASELINE_REF?.trim() != '' || params.COMPARE_PREV_TAG
                }
            }
            steps {
                script { h.runGitRefCompare() }
            }
        }

    } // end stages

    post {
        success {
            script {
                if (h != null) { h.runPostSuccess() }
            }
        }
        unstable {
            // A fresh agent without cargo-nextest produces an UNSTABLE build (the
            // Unit tests stage falls back to cargo test with no JUnit report). Run
            // the same summary so the build row gets its description/displayName
            // instead of showing blank; runPostSuccess skips the downstream trigger
            // on a non-SUCCESS result.
            script {
                if (h != null) { h.runPostSuccess() }
            }
        }
        failure {
            echo 'Build failed — review the stage output above for details.'
        }
        always {
            script {
                if (h != null) { h.runBitbucketNotify() }
            }
        }
        cleanup {
            // cleanup runs LAST — after success/failure/always — guaranteeing that
            // post { success } can still read result.json before the workspace is wiped.
            script {
                try {
                    cleanWs()
                } catch (Throwable ex) {
                    // Throwable, not Exception: a missing ws-cleanup plugin throws
                    // NoSuchMethodError (a java.lang.Error), which catch(Exception)
                    // would let escape and fail the whole build.
                    echo "cleanWs skipped: ${ex.message}"
                }
            }
        }
    }
}
