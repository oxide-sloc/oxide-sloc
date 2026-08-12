# CI/CD Integrations

This document covers how to wire oxide-sloc into your CI/CD pipelines and how to push scan results to external systems such as Confluence.

> **Scanning code on a different git host than the pipeline?** (e.g. tooling on
> `bitbucket.instance1.com`, code to scan on `bitbucket.instance2.com`.) See
> **[Scanning multiple git instances](multi-instance.md)** for per-host credentials, corporate
> proxy/VLAN guidance, and air-gapped offline import — it applies to local, server, and Jenkins
> modes alike.

---

## Table of contents

1. [General approach](#general-approach)
2. [Jenkins](#jenkins)
   - [Obtaining credentials](#obtaining-credentials)
   - [Local credential storage](#local-credential-storage)
   - [Pre-flight check](#pre-flight-check)
   - [Installing plugins](#installing-plugins)
   - [Basic pipeline](#basic-pipeline)
   - [Publishing to Confluence](#publishing-to-confluence)
   - [Trend charts](#trend-charts-plot-plugin)
   - [Setting the artifact-viewer CSP](#setting-the-artifact-viewer-csp)
3. [Atlassian Suite (Confluence + Bitbucket) — built-in CI integration](#atlassian-suite-confluence--bitbucket--built-in-ci-integration)
   - [The two surfaces](#the-two-surfaces)
   - [Three-tier permission model](#three-tier-permission-model)
   - [Job parameters + credential IDs](#job-parameters--credential-ids)
   - [Cloud vs Server/DC differences](#cloud-vs-serverdc-differences)
   - [Auth schemes](#atlassian-auth-schemes)
   - [Plugin-optional design (Tier-3 fallback)](#plugin-optional-design-tier-3-fallback)
   - [Troubleshooting (keyed to log lines)](#atlassian-troubleshooting-keyed-to-log-lines)
4. [GitHub Actions](#github-actions)
   - [VirusTotal binary scanning](#virustotal-binary-scanning)
5. [GitLab CI](#gitlab-ci)
6. [Artifact Repository Integration](#artifact-repository-integration)
   - [JFrog Artifactory](#jfrog-artifactory)
   - [Sonatype Nexus Repository Manager 3](#sonatype-nexus-repository-manager-3)
   - [Sonatype Nexus Repository Manager 2](#sonatype-nexus-repository-manager-2)
   - [Amazon S3](#amazon-s3)
   - [MinIO](#minio)
   - [Azure Blob Storage](#azure-blob-storage)
   - [Generic HTTP PUT](#generic-http-put)
   - [Registering artifact repo credentials](#registering-artifact-repo-credentials)
7. [Environment variables reference](#environment-variables-reference)
8. [CLI flag quick reference](#cli-flag-quick-reference)

---

## General approach

oxide-sloc is a single self-contained binary — there are no daemon processes, build-system plugins, or language-runtime dependencies beyond the binary itself.

Every CI integration follows the same three-step pattern:

```
1. acquire the binary  →  reassemble + decompress vendor.tar.gz.*, install Rust, build oxide-sloc
2. run the scan        →  oxide-sloc analyze ./src \
                              --json-out result.json \
                              --csv-out  report.csv \
                              --xlsx-out report.xlsx \
                              --html-out report.html
3. consume outputs     →  archive, publish, or push to external tools
```

> **Note:** `--csv-out` and `--xlsx-out` produce a multi-section CSV and a four-sheet Excel workbook (Summary, By Language, Per File, Skipped Files) from the same analysis result. They work on any `analyze`, `report`, or `diff` command and are available as standalone downloads via the web API at `GET /runs/csv/<run_id>` and `GET /runs/xlsx/<run_id>`.

### Vendor sources note

The split gzip parts `vendor.tar.gz.aa` / `.ab` / `.ac` contain all Rust crate dependencies (~115 MB total, gzip-compressed, split into ≤45 MB parts to stay under GitHub's 100 MB per-file limit) alongside `vendor.checksums.sha256`. Gzip is used (not xz) so stock Git Bash on Windows, which has no `xz`, can extract them. They are **committed to the repository** — a plain `git clone` is sufficient for a fully offline Cargo build; no separate download step is required. They are also attached to each GitHub release as standalone assets for non-git workflows.

The included Jenkinsfile and GitLab CI pipeline files decompress and cache `vendor/` automatically from the committed archive. The `.cargo/config.toml` is written at build time to redirect cargo to the vendored sources:

```toml
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
```

To regenerate the archive after any dependency change, run `bash scripts/internal/update-vendor.sh` and commit both output files.

### Rust toolchain — offline options

The Rust compiler (~400 MB) is not bundled in the git repository. Two offline paths are supported:

**Path A — Rebuild `ci/jenkins/Dockerfile.agent` (recommended for Docker-based Jenkins)**

The agent Dockerfile bakes the pinned toolchain from `rust-toolchain.toml` into
`/opt/rust-toolchain` at image build time. After rebuilding:

```bash
docker build -t jenkins-oxide-sloc:latest -f ci/jenkins/Dockerfile.agent .
docker compose down && docker compose up -d
```

the Jenkinsfile Setup stage copies the toolchain into the persistent cache volume on
first use; all subsequent builds are fully offline with zero internet access.

**Path B — Commit `rust-toolchain-bundle.tar.xz`** (for bare-metal or custom images)

Run the bundling script once on any Linux machine with internet access:

```bash
bash ci/jenkins/bundle-rust-toolchain.sh
# outputs: rust-toolchain-bundle.tar.xz + rust-toolchain-bundle.tar.xz.sha256
```

Because the bundle is typically 200-350 MB, split it into ≤45 MB parts before committing:

```bash
split -b 45m rust-toolchain-bundle.tar.xz rust-toolchain-bundle.tar.xz.
rm rust-toolchain-bundle.tar.xz
git add rust-toolchain-bundle.tar.xz.* rust-toolchain-bundle.tar.xz.sha256
git commit -m "ci: add Rust toolchain bundle for offline builds"
```

Once committed, the Jenkinsfile Setup stage extracts the bundle automatically — no
internet access needed during the pipeline run.

The JSON output (`result.json`) is machine-readable and stable across versions — use it to feed dashboards, Confluence, Slack webhooks, or custom tooling. The HTML report is a self-contained single-file document suitable for artifact storage and browser viewing.

---

## Jenkins

### Obtaining credentials

#### Initial admin password

If Jenkins was just installed and has never been unlocked:

**Native install (systemd / RPM / DEB package):**
```bash
sudo cat /var/lib/jenkins/secrets/initialAdminPassword
# some installs use /var/jenkins_home instead:
sudo cat /var/jenkins_home/secrets/initialAdminPassword
```

**Docker install:**
```bash
docker exec <container-name-or-id> cat /var/jenkins_home/secrets/initialAdminPassword
```

Paste this password into the Jenkins setup wizard at `http://<host>:8080/`.

#### If initialAdminPassword is gone

After first-run setup completes, Jenkins deletes that file. If you've also lost your token (e.g. you re-cloned and `ci/jenkins/.env` is gone), use one of:

- **Browser:** sign in at `http://<host>:8080/` with your admin password → click your username (top-right) → **Configure** → scroll to **API Token** → **Add new Token**. (The direct URL `/user/<id>/configure` is gone in some recent LTS builds; use the dropdown link instead.)
- **REST (Path B below):** the same admin password works — no `initialAdminPassword` needed.
- **Reset admin password (last resort):** `docker exec -u root <container> bash -c 'echo "admin:NEW" | chpasswd'` does NOT work for Jenkins — the password lives in `/var/jenkins_home/users/admin_*/config.xml` as a hashed `passwordHash`. To reset it, edit that file with a known hash (or run Jenkins with `-Djenkins.install.runSetupWizard=true` and reuse `initialAdminPassword`). Avoid this unless the password is genuinely lost.

#### Minting a long-lived API token

After the initial setup wizard is complete:

1. Open **Manage Jenkins → Users → admin → Configure**
2. Scroll to **API Token → Add new Token**
3. Give it a name (e.g. `bootstrap-token`) and click **Generate**
4. **Copy the token now** — it is shown only once and cannot be retrieved later

#### Path B — mint via REST

If you prefer not to use the GUI, the token can be minted via the Jenkins REST API. The cookie jar is required — the CSRF crumb is only honored within the same session that issued it.

```bash
# Pre-req: the admin password (initialAdminPassword or the configured one).
JENKINS_URL=http://10.0.0.8:8080
JENKINS_USER=admin
read -rsp "Jenkins admin password: " JENKINS_PASS
echo
# Read at the prompt — keeps the password out of shell history.

cookies=$(mktemp)
crumb=$(curl -sS -c "$cookies" -u "$JENKINS_USER:$JENKINS_PASS" \
  "$JENKINS_URL/crumbIssuer/api/xml?xpath=concat(//crumbRequestField,\":\",//crumb)")

curl -sS -b "$cookies" -u "$JENKINS_USER:$JENKINS_PASS" -H "$crumb" \
  -X POST --data 'newTokenName=bootstrap-token' \
  "$JENKINS_URL/user/$JENKINS_USER/descriptorByName/jenkins.security.ApiTokenProperty/generateNewToken"
# → JSON containing tokenValue. Copy that value into JENKINS_TOKEN in ci/jenkins/.env.
rm -f "$cookies"
```

### Local credential storage

```bash
cp ci/jenkins/.env.example ci/jenkins/.env
# Open ci/jenkins/.env and fill in JENKINS_TOKEN with the value from above.
```

`ci/jenkins/.env` is listed in `.gitignore` — it will never be committed.

**URL note:** LAN/remote addresses (e.g., `http://10.0.0.8:8080`) are valid substitutions for `http://localhost:8080`. Strip any trailing slash — `${JENKINS_URL}/createItem` would otherwise produce `//createItem`, which some reverse proxies reject.

**Job name:** Use `oxide-sloc` for the SCM-driven job created from `ci/jenkins/job-config.xml`. Use `oxide-sloc-manual` only if you also intend to maintain a hand-edited copy of the pipeline in the same Jenkins instance and need to disambiguate. If both names already exist on your Jenkins and you need to remove the unwanted one, delete it with:

```bash
curl -sS -X POST -u "${JENKINS_USER}:${JENKINS_TOKEN}" \
    -H "$(curl -sS -u "${JENKINS_USER}:${JENKINS_TOKEN}" "${JENKINS_URL}/crumbIssuer/api/xml?xpath=concat(//crumbRequestField,\":\",//crumb)")" \
    "${JENKINS_URL}/job/<name-to-delete>/doDelete"
```

**Persistent storage outside the working tree.** Re-cloning the repo will delete `ci/jenkins/.env` along with the rest of the working tree. To keep the token across re-clones, copy the file to a stable location (e.g., `~/.config/oxide-sloc/jenkins.env` or `~/.oxide-sloc.env`) and set `export OXIDE_SLOC_ENV_FILE=~/.config/oxide-sloc/jenkins.env` in your shell profile. The bootstrap scripts will source that file if present.

### Pre-flight check

Run this before `createItem`. It verifies reachability, authentication, plugin presence, and that no conflicting job exists:

```bash
set -a; source ci/jenkins/.env; set +a && bash ci/jenkins/preflight.sh
```

Fix any `[fail]` before continuing. `[ok]` is green, `[warn]` is advisory, and a `[skip]`
line is benign (a probe that can't self-run, e.g. the agent library check when the script
console is sandbox-gated — verify those manually per the inline hint).

### Installing plugins

All 7 required plugins are listed in `ci/jenkins/plugins.txt`.

**Path 1 — Docker (online):**
```bash
docker exec -u root <container> jenkins-plugin-cli \
  --plugins $(grep -Ev '^#|^$' ci/jenkins/plugins.txt | awk '{print $1}' | tr '\n' ' ')
```

**Path 2 — Docker (offline):** download `.hpi` files from a machine with internet access and copy them into `/var/jenkins_home/plugins/`. See `ci/jenkins/plugins.txt` for the download loop.

**Path 3 — Native / systemd install (Jenkins CLI jar):**
```bash
set -a; source ci/jenkins/.env; set +a
curl -sS -o jenkins-cli.jar "${JENKINS_URL}/jnlpJars/jenkins-cli.jar"
java -jar jenkins-cli.jar -s "${JENKINS_URL}" -auth "${JENKINS_USER}:${JENKINS_TOKEN}" \
    install-plugin $(grep -Ev '^#|^$' ci/jenkins/plugins.txt | awk '{print $1}')
java -jar jenkins-cli.jar -s "${JENKINS_URL}" -auth "${JENKINS_USER}:${JENKINS_TOKEN}" safe-restart
```

After any install, re-run `preflight.sh` — check (c) asserts all plugins are active before you proceed.

#### Relaxing the artifact-viewer CSP (Docker)

If Jenkins is running in Docker and you need to apply the CSP relaxation to an already-running container without rebuilding the image:

```bash
# For a running container:
docker cp ci/jenkins/init.groovy.d/relax-csp.groovy <container>:/var/jenkins_home/init.groovy.d/relax-csp.groovy
docker exec -u root <container> chown jenkins:jenkins /var/jenkins_home/init.groovy.d/relax-csp.groovy
docker restart <container>
```

#### Rebuilding the agent image

The Jenkins agent image at `ci/jenkins/Dockerfile.agent` includes the system libraries `oxide-sloc`'s build needs **and** the pinned Rust toolchain baked in at `/opt/rust-toolchain`:

| Package | Required by |
|---------|-------------|
| `libssl-dev` | TLS for Rust HTTP clients |
| `libwayland-dev` | `rfd` crate (activated by `cargo --all-features`) |
| `libgtk-3-dev` | `rfd` crate (activated by `cargo --all-features`) |
| `libxdo-dev` | `rfd` crate (activated by `cargo --all-features`) |
| `pkg-config`, `build-essential` | native build steps |
| `python3` | the pipeline's plot-data extraction stage |
| Rust toolchain (rustc, cargo, rustfmt, clippy) | baked in at `/opt/rust-toolchain`; seeded into the persistent cache on first pipeline run |

Whenever the package list changes, the Rust version bumps (`rust-toolchain.toml`), or you refresh the base image, rebuild the agent image and redeploy — **merging the change alone does not update what's running:**

```bash
# In the repo root, with the patched Dockerfile.agent on disk:
docker build -t jenkins-oxide-sloc:latest -f ci/jenkins/Dockerfile.agent .

# Then in the directory containing your Jenkins docker-compose.yml:
docker compose down && docker compose up -d
```

For non-container agents, see the native setup section below.

`preflight.sh` probes the running agent for these libraries via the script console; a stale image will surface as a `[fail]` line on the next preflight run, not as a 20-second clippy compile error 5 minutes later.

#### Native / systemd agent setup

For Jenkins running directly on the host (no Docker), a three-step one-time setup replaces the Dockerfile.agent approach. The Jenkinsfile's `CARGO_HOME`, `RUSTUP_HOME`, and `PATH` all use `${HOME}/.rust-cache`, so they work correctly for any Jenkins user home directory.

**Step 1 — System packages (run once as root):**

```bash
sudo bash ci/jenkins/install-system-deps.sh
```

Installs python3, build-essential, pkg-config, libssl-dev, libwayland-dev, libgtk-3-dev, libxdo-dev, curl, and xz-utils. Supports Debian/Ubuntu (apt-get) and RHEL/CentOS/Fedora (dnf/yum).

**Step 2 — Rust toolchain cache (run once as the jenkins user):**

```bash
sudo -u jenkins bash ci/jenkins/install-rust-cache.sh
```

Installs the toolchain pinned in `rust-toolchain.toml` into `~jenkins/.rust-cache`. The Jenkinsfile reads `CARGO_HOME`/`RUSTUP_HOME` from `${HOME}/.rust-cache`, so Docker and native agents resolve to the same layout — just rooted at different home directories.

For a native agent without internet access, pre-build the cache archive and extract it on the agent:

```bash
# Build the archive (requires internet access):
bash ci/jenkins/install-rust-cache.sh
tar -czf rust-cache.tar.gz -C "${HOME}" .rust-cache

# Extract into the jenkins user's home (Debian/Ubuntu default):
sudo -u jenkins tar -xzf rust-cache.tar.gz -C /var/lib/jenkins
# For other distros, replace /var/lib/jenkins with: $(getent passwd jenkins | cut -d: -f6)
```

**Step 3 — Verify:**

```bash
bash ci/jenkins/preflight.sh
```

Run this after Step 1 and again after Step 2 to confirm both layers are in place before triggering a build. Every line should be `[ok]` (a `[skip]` is benign; fix any `[fail]`).

### Basic pipeline

The `Jenkinsfile` shipped at the repo root is a ready-to-use, fully-parameterized pipeline covering setup, quality gates, analysis, web UI health check, optional delivery (webhook/email), and artifact publishing with build-over-build trend charts.

#### Scan any project — the 30-second version

There are two ways to point oxide-sloc at an arbitrary project. Pick whichever fits:

**A. One central job, any repo (no per-project files).** Use the canonical `Jenkinsfile` and set two build parameters at run time:

| Set this parameter | To |
|---|---|
| `SCAN_REPO_URL` | the Git URL of the project you want to analyze |
| `SCAN_PATH` | a path inside that project — `.` for the whole repo, or e.g. `src` |
| `SCAN_REF` _(optional)_ | a branch, tag, or SHA (defaults to the project's `main`) |

The job checks your project out into `./_target`, builds (or, by default, extracts the committed prebuilt) scanner from the oxide-sloc tooling repo (`TOOL_REPO_URL`, left at its default), scans it, and publishes the same HTML/JSON/CSV/PDF reports and trend charts. `TOOL_REPO_URL` stays pointed at oxide-sloc (or your fork) — it only supplies the scanner. Nothing is added to the analyzed project.

> **Fast by default.** Out of the box the job runs in `prebuilt` mode (`BUILD_MODE=prebuilt`): it extracts the committed `dist/` binary — no Rust toolchain, no vendor compile, no unit tests, no lint — so a standard scan finishes in **~2-3 min** instead of ~40. No parameter changes are needed for the fast path. Set `BUILD_MODE=source` (or check `RUN_QUALITY_GATES` / `RUN_COVERAGE`, which force a source build) to compile from the vendored crates instead.

```bash
# Trigger a scan of any repo from the CLI (build #2 onward):
set -a; source ci/jenkins/.env; set +a
curl -sS -X POST -u "${JENKINS_USER}:${JENKINS_TOKEN}" \
  "${JENKINS_URL}/job/${JOB_NAME}/buildWithParameters" \
  --data-urlencode 'SCAN_REPO_URL=https://github.com/acme/widget.git' \
  --data-urlencode 'SCAN_PATH=.' \
  --data-urlencode 'REPORT_TITLE=widget SLOC'
```

**B. Drop-in per project (self-contained `Jenkinsfile`).** Copy `testing/examples/jenkins/Jenkinsfile` into your project's repository and let Jenkins auto-discover it (**Pipeline script from SCM**, script path `Jenkinsfile`). It scans the checked-out workspace with no edits, installing the scanner from crates.io when it is not already on `PATH`. For a Jenkins **shared library**, use `ci/sloc-jenkins.groovy` (`slocAnalyze(path: '.')`) instead — see the header of that file.

Use **A** for a governance/portfolio scan run centrally; use **B** when each team owns its own pipeline.

**Scanning many git instances from one job.** For a single central job that scans repos across
several hosts — each with its own credential — set the multi-line `SCAN_GIT_CREDENTIALS` parameter
(one `host=jenkins-credentials-id` per line). The pipeline auto-selects the Jenkins credential whose
host matches `SCAN_REPO_URL` (falling back to the single `SCAN_CREDENTIALS_ID` when no line matches),
injected natively by the git plugin — never in an env var, argv, or the URL. See
[Scanning multiple git instances](multi-instance.md) for the app-side per-host registry it mirrors.
For a fully disconnected agent, check `SCAN_ALLOW_LOCAL` and set `SCAN_LOCAL_ROOT`, then pass the
imported bundle/path as `SCAN_REPO_URL` (air-gap Case B in [multi-instance.md](multi-instance.md)).

#### Option A — GUI setup

1. Create a **Pipeline** job in Jenkins (**New Item → Pipeline**).
2. Set **Definition** → `Pipeline script from SCM`.
3. Point it at your repository (branch `main`, script path `Jenkinsfile`). Jenkins auto-discovers the file.

#### Option B — CLI bootstrap (one command)

Use the importable job definition at `ci/jenkins/job-config.xml`. Source your credentials file first (see [Local credential storage](#local-credential-storage)):

```bash
set -a; source ci/jenkins/.env; set +a

# 0. Render the job XML with your REPO_URL substituted
bash ci/jenkins/render-job-config.sh   # writes /tmp/job-config.xml

# 1. Obtain a CSRF crumb
CRUMB=$(curl -sS -u "${JENKINS_USER}:${JENKINS_TOKEN}" \
    "${JENKINS_URL}/crumbIssuer/api/xml?xpath=concat(//crumbRequestField,\":\",//crumb)")

# 2. Create the job
curl -sS -u "${JENKINS_USER}:${JENKINS_TOKEN}" \
    -H "${CRUMB}" -H "Content-Type: application/xml" \
    --data-binary @/tmp/job-config.xml \
    "${JENKINS_URL}/createItem?name=${JOB_NAME}"
```

A 200 response with an empty body means success. A 400 with `job already exists` means the job name is taken.

> **Updating an existing job** via `POST ${JENKINS_URL}/job/${JOB_NAME}/config.xml` (rather
> than `createItem`) must send `Content-Type: application/xml; charset=utf-8`. Without the
> charset, Jenkins decodes the body as ISO-8859-1 and the em-dash in the job description
> becomes an invalid XML character → **HTTP 500 `Invalid XML character 0x80`**. `createItem`
> is unaffected; only the config.xml update endpoint needs the explicit charset.

For **Job DSL** plugin users, `ci/jenkins/seed-job.groovy` achieves the same result as a seed job or via Manage Jenkins → Script Console.

#### First-build trigger

Trigger the first build immediately after `createItem`:

```bash
curl -sS -X POST -u "${JENKINS_USER}:${JENKINS_TOKEN}" \
    "${JENKINS_URL}/job/${JOB_NAME}/build"
```

The first build runs with no parameters — Jenkins uses it to discover the `parameters {}` block in the Jenkinsfile. From build #2 onward, **Build with Parameters** in the left-hand sidebar shows the full configurable form.

> **Note:** The SCM URL Jenkins uses to fetch the Jenkinsfile itself comes from
> `ci/jenkins/job-config.xml` — not from the `TOOL_REPO_URL` build parameter inside the Jenkinsfile.
> The parameter only takes effect from build #2 onward. If the first build fails with
> `'__placeholder__' does not appear to be a git repository`, re-render `job-config.xml`
> with the `REPO_URL` environment variable exported and re-create the job. (The tooling-repo build
> parameter is now `TOOL_REPO_URL`; the `REPO_URL` environment variable / `.env` value that bakes the
> job's SCM URL is unchanged and still works as a fallback.)

#### Build parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| `TOOL_REPO_URL` | `https://github.com/oxide-sloc/oxide-sloc.git` | **Tooling repo** — the checkout the scanner binary is built (or extracted) from. Leave at the default (or your fork). To scan a different project, use `SCAN_REPO_URL` instead; do not point `TOOL_REPO_URL` at the project you want to scan. Use `file:///path/to/repo` for air-gapped repos. (This is the per-build form field; the `REPO_URL` environment variable / `.env` value is unchanged and still works as a fallback.) |
| `BUILD_MODE` | `prebuilt` | `prebuilt` (default) extracts the committed `dist/` binary — no toolchain, no vendor compile, no unit tests, no lint — for a ~2-3 min scan. `source` compiles from the vendored crates. `RUN_QUALITY_GATES` / `RUN_COVERAGE` force `source`. |
| `SCAN_REPO_URL` | _(empty → scan self)_ | Git URL of the **project you want to analyze**. When set, it is checked out into `./_target` and scanned there — this is how one Jenkins job scans any project. Empty = scan the tooling repo itself. Use `file:///path/to/repo` for air-gapped repos, or an imported `*.bundle` with `SCAN_ALLOW_LOCAL`. |
| `SCAN_REF` | _(default branch)_ | Branch, tag, or commit SHA to check out for `SCAN_REPO_URL`. Ignored when `SCAN_REPO_URL` is empty. Example: `develop`, `v2.1.0`, `a3f9d2c`. |
| `SCAN_CREDENTIALS_ID` | _(empty)_ | Jenkins credential ID for a **private** `SCAN_REPO_URL` (username+token, or SSH key). For several instances in one run, use `SCAN_GIT_CREDENTIALS` instead. |
| `SCAN_GIT_CREDENTIALS` | _(empty)_ | Multi-line `host=jenkins-credentials-id` map. The pipeline auto-selects the credential whose host matches `SCAN_REPO_URL` (falling back to `SCAN_CREDENTIALS_ID`), so one job scans many hosts. See [multi-instance.md](multi-instance.md). |
| `SCAN_ALLOW_LOCAL` | false | Permit offline import (git bundle / `file://` / local path) when the source cannot be reached. Maps to the app's `SLOC_GIT_ALLOW_LOCAL`. Fail-closed: also set `SCAN_LOCAL_ROOT`. Pass the bundle/path as `SCAN_REPO_URL`. |
| `SCAN_LOCAL_ROOT` | _(empty)_ | Directory an offline `SCAN_REPO_URL` source must resolve under (required when `SCAN_ALLOW_LOCAL` is checked). Maps to `SLOC_GIT_LOCAL_ROOT`. Sources outside it, plus UNC / `file://host`, are refused. |
| `SCAN_PATH` | `testing/fixtures/basic` | Directory or space-separated paths to scan, relative to the scanned repo root (or absolute). When `SCAN_REPO_URL` is set this is relative to `./_target` — set it to a path inside your project (`.` for the whole repo, `src` for a subtree). The default only exists in the oxide-sloc repo. |
| `REPORT_TITLE` | `oxide-sloc CI Report` | Title embedded in generated HTML and PDF reports. |
| `OUTPUT_SUBDIR` | `ci-out` | Sub-directory for all generated artifacts (relative to workspace). Created automatically. Contains `report.html`, `result.json`, `report.pdf`, and trend CSVs. |
| `CI_PRESET` | `default` | CI configuration preset loaded from `ci/`: `default` (balanced, mirrors web UI) / `none` (no preset) / `strict` (fail on binary files) / `full-scope` (count everything including vendor). |
| `MIXED_LINE_POLICY` | `code-only` | How lines with inline comments are classified. |
| `ACTIVITY_WINDOW` | `90` | Git Hotspots window in days (on by default). Ranks files by code lines × recent commits in the HTML/PDF report and adds Commits/Last-changed CSV columns. Set `0` to disable. Needs `SCAN_PATH` to be a git checkout. |
| `DOCSTRINGS_AS_CODE` | false | Count Python triple-quoted docstrings as code instead of comments. |
| `SUBMODULE_BREAKDOWN` | true | Detect `.gitmodules` and emit per-submodule stats in the report. |
| `FOLLOW_SYMLINKS` | false | Follow symbolic links during file discovery. |
| `NO_IGNORE_FILES` | false | Ignore `.gitignore` / `.slocignore` rules. |
| `ENABLED_LANGUAGES` | _(all)_ | Comma-separated language filter, e.g. `rust,python`. |
| `INCLUDE_GLOBS` | _(all)_ | Comma-separated include glob patterns, e.g. `src/**/*.py`. |
| `EXCLUDE_GLOBS` | _(none)_ | Comma-separated exclude glob patterns, e.g. `vendor/**`. |
| `REPORT_HTML` | true | Write HTML report and publish as the `OxideSLOC_CI_Report_<proj-slug>` sidebar link (`<proj-slug>` = `oxide-sloc_<short-sha>`; project-level, so the URL changes each commit). Requires HTML Publisher plugin. |
| `REPORT_PDF` | true | Write PDF report alongside the HTML report. Pure-Rust generation — no browser or external tool required on the agent. When enabled, the "View PDF" button in the HTML report opens the archived PDF directly. |
| `RUN_QUALITY_GATES` | false | **Off by default** — a standard scan skips the fmt / clippy / unit-test stage entirely. Check it to run the quality gates; this also implies a source build (`BUILD_MODE=source`, since the toolchain is needed to compile and lint). |
| `RUN_WEB_HEALTHCHECK` | false | **Off by default** — the web UI health-check stage is skipped. Check it to run the check. Enable only on agents with loopback access where port 4317 is available. |
| `NOTIFY_WEBHOOK_URL` | _(skip)_ | POST JSON result here after scan. Add `SLOC_WEBHOOK_TOKEN` Secret Text credential for Bearer auth. |
| `NOTIFY_EMAIL` | _(skip)_ | Comma-separated recipients. Requires `SLOC_SMTP_HOST`, `SLOC_SMTP_USER`, `SLOC_SMTP_PASS` credentials. |
| `ARTIFACT_REPO_TYPE` | `none` | Artifact repository backend: `none` / `artifactory` / `nexus` / `nexus2` / `s3` / `minio` / `azure-blob` / `generic-http`. |
| `ARTIFACT_REPO_URL` | _(empty)_ | Base URL of the artifact repository (see [Artifact Repository Integration](#artifact-repository-integration)). |
| `ARTIFACT_REPO_PATH` | `oxide-sloc/${JOB_NAME}/${BUILD_NUMBER}` | Path prefix for uploaded artifacts. Tokens `${JOB_NAME}` and `${BUILD_NUMBER}` are substituted at runtime. |
| `ARTIFACT_REPO_EXTRA` | _(empty)_ | Provider-specific config: Nexus repo name, Azure container name, MinIO endpoint URL, or extra S3 flags. |
| `ARTIFACT_PUSH_JSON` | true | Include `result.json` in the artifact repository push. |
| `ARTIFACT_PUSH_HTML` | true | Include `report.html` in the push (only when `REPORT_HTML` is checked). |
| `ARTIFACT_PUSH_PDF` | false | Include `report.pdf` in the push (only when `REPORT_PDF` is checked). |

> **JSON is always generated** regardless of parameters — it is required for build-over-build trend plots, the build description summary, and the `send` delivery subcommand.

#### Optional — registering Secret Text credentials

The pipeline's webhook and email delivery features read credentials from the Jenkins store by specific IDs. Register the following Secret Text credentials before triggering a build if you plan to use those features:

| Credential ID | Used for |
|---------------|----------|
| `SLOC_WEBHOOK_TOKEN` | Bearer token for `NOTIFY_WEBHOOK_URL` delivery |
| `SLOC_SMTP_HOST` | SMTP host for `NOTIFY_EMAIL` delivery |
| `SLOC_SMTP_USER` | SMTP username |
| `SLOC_SMTP_PASS` | SMTP password |
| `SLOC_ARTIFACT_REPO_USER` | Username or access-key ID for artifact repository push |
| `SLOC_ARTIFACT_REPO_PASS` | Password, API token, or secret key for artifact repository push |

To create a credential via the REST API (repeat for each ID, substituting the correct `id` and `secret`):

```bash
set -a; source ci/jenkins/.env; set +a
crumb=$(curl -sS -u "$JENKINS_USER:$JENKINS_TOKEN" \
  "$JENKINS_URL/crumbIssuer/api/xml?xpath=concat(//crumbRequestField,\":\",//crumb)")

curl -sS -u "$JENKINS_USER:$JENKINS_TOKEN" -H "$crumb" \
  -X POST "$JENKINS_URL/credentials/store/system/domain/_/createCredentials" \
  --data-urlencode 'json={
    "": "0",
    "credentials": {
      "scope": "GLOBAL",
      "id": "SLOC_WEBHOOK_TOKEN",
      "secret": "REPLACE_ME",
      "description": "oxide-sloc webhook bearer token",
      "$class": "org.jenkinsci.plugins.plaincredentials.impl.StringCredentialsImpl"
    }
  }'
```

Alternatively, add them via **Manage Jenkins → Credentials → System → Global credentials → Add Credentials** (Kind: Secret text).

#### Required plugins

See `ci/jenkins/plugins.txt` for the full list. Minimum required:

| Plugin | Purpose |
|--------|---------|
| `workflow-aggregator` | Declarative pipeline syntax |
| `pipeline-utility-steps` | `readJSON` in `post { success }` |
| `git` | SCM checkout |
| `ws-cleanup` | `cleanWs()` in `post { cleanup }` |
| `credentials-binding` | SMTP / webhook credential bindings |
| `htmlpublisher` | `OxideSLOC_CI_Report_<proj-slug>` sidebar link |
| `plot` | Build-over-build trend charts |

#### Trend charts (Plot plugin)

After at least two successful builds, the job page shows two charts under **SLOC Trends**:

- **SLOC totals over time** — code, comment, blank lines, and file count across builds (`summary.csv`)
- **Per-language code lines** — bar chart of code lines by language for recent builds (`per_language.csv`)

The build description on each run is also set automatically, e.g.: `code=4821  files=38  comments=312  blank=890`

#### Release / tag webhook trigger (auto-scan on release)

`ci/jenkins/render-job-config.sh` can render an optional **Generic Webhook Trigger** into the job so
a GitHub/Bitbucket **release** or tag push kicks off a scan automatically. Enable it by exporting
`SLOC_ENABLE_WEBHOOK_TRIGGER=1` before rendering the job XML; it requires the
`generic-webhook-trigger` plugin (now listed in `ci/jenkins/plugins.txt`).

Once rendered, a release/tag event that POSTs to
`<JENKINS_URL>/generic-webhook-trigger/invoke?token=<SLOC_WEBHOOK_TRIGGER_TOKEN>` maps the pushed tag
into `SCAN_REF` and the repo clone URL into `SCAN_REPO_URL`, so the pushed tag is scanned with no
code added to the target repo. `SLOC_WEBHOOK_TRIGGER_TOKEN` defaults to `oxide-sloc`.

The tag and URL are extracted from the payload via JSONPath, overridable at render time with
`SLOC_WEBHOOK_TAG_JSONPATH` / `SLOC_WEBHOOK_URL_JSONPATH`:

| Event source | Tag JSONPath | Clone-URL JSONPath | Webhook to add on the repo |
|---|---|---|---|
| **GitHub release** (default) | `$.release.tag_name` | `$.repository.clone_url` | payload URL above, "Let me select individual events" → **Releases** |
| **GitHub tag push** | `$.ref` | `$.repository.clone_url` | same URL, event **Pushes** (tag refs) |
| **Bitbucket Server/DC + Cloud tag push** | `$.push.changes[0].new.name` | `$.repository.links.clone[0].href` | Repository settings → Webhooks → the payload URL above |

```bash
# Render the job with the release-webhook trigger enabled:
SLOC_ENABLE_WEBHOOK_TRIGGER=1 \
SLOC_WEBHOOK_TRIGGER_TOKEN=my-secret-token \
bash ci/jenkins/render-job-config.sh   # writes /tmp/job-config.xml, then createItem as usual
```

> **Seed the job first.** The Generic Webhook Trigger only populates build parameters
> that already exist on the job, and a Pipeline-from-SCM job discovers its `parameters {}`
> block on its **first** build. If a webhook fires before that seed build, `SCAN_REF` /
> `SCAN_REPO_URL` are not yet registered and the run silently self-scans this repo instead
> of the pushed tag. Run one build (the [First-build trigger](#first-build-trigger) above)
> before enabling or firing the webhook.

For push-side triggering from another CI system (rather than a repo webhook), and for
**upstream/downstream chaining** (`CHAIN_UPSTREAM_JOB` / `CHAIN_UPSTREAM_BUILD` /
`CHAIN_DOWNSTREAM_JOB`, including an oxide-sloc → oxide-sloc chain), see
[`ci/jenkins/INTEGRATION.md`](../ci/jenkins/INTEGRATION.md).

#### Setting the artifact-viewer CSP

**The pipeline runs fine WITHOUT any CSP relaxation and WITHOUT the `jenkins-api-token`
credential.** The published HTML report renders under Jenkins' default CSP because
`ci/jenkins/extract-report-assets.py` externalises the report's inline CSS/JS into
sidecar files, and from Jenkins 2.387.x LTS onward (including the 2.555.x series) the
artifact viewer serves a `Content-Security-Policy-Report-Only` header — non-blocking.
CSP relaxation only affects the interactive artifact-viewer styling on very old
(pre-2.387.x) controllers, where the default CSP was enforcing and blocked inline scripts.

When the in-pipeline relaxation cannot apply (Groovy sandbox active — the default for
SCM-defined pipelines — and no `jenkins-api-token`), the Setup stage logs a single calm
INFO line and continues. Nothing fails.

**Recommended default for corporate / locked-down / sandboxed controllers:** drop the
credential-free, sandbox-proof init script `ci/jenkins/init.groovy.d/relax-csp.groovy`
into `$JENKINS_HOME/init.groovy.d/` before starting Jenkins:

```bash
cp ci/jenkins/init.groovy.d/relax-csp.groovy $JENKINS_HOME/init.groovy.d/
# Then restart Jenkins.
```

This sets the CSP property at startup without requiring in-process script approval, a
credential, or a disabled sandbox. On the Jenkins host you can also run
`bash ci/jenkins/preflight.sh --install-csp` to deploy it and restart automatically.
For external origins (GitHub Pages, S3), control the `Content-Security-Policy` response
header directly on that service instead.

> **Windows agents:** every stage runs POSIX `.sh` scripts. On a Linux agent they run
> natively; on a **Windows agent** the pipeline runs them through **Git Bash** (which
> also supplies the MinGW `gcc`/`ld` the `x86_64-pc-windows-gnu` build links against).
> A **system-wide install of Git for Windows is often blocked on locked-down agents —
> you do not need it.** Choose whichever option below fits (the `Admin?` column shows
> which require elevation):
>
> | Option | Admin? | How |
> |---|---|---|
> | **Portable Git Bash (recommended)** | No | Download `PortableGit-*.7z.exe` once on any machine, copy it to the air-gapped agent, then run `powershell -ExecutionPolicy Bypass -File ci\jenkins\stage-portable-git.ps1 <PortableGit-*.7z.exe>`. It extracts into `<workspace>\.tools\PortableGit`, which the pipeline auto-detects — no env var needed. Stage it in a shared dir instead and set `SLOC_PORTABLE_GIT` to that folder. |
> | **Per-user Git install** | No | Git for Windows' installer supports a per-user install into `%LOCALAPPDATA%\Programs\Git`. The pipeline auto-detects that location. |
> | **Point at an existing bash** | No | Set `SLOC_BASH` to any `bash.exe` (or `SLOC_PORTABLE_GIT` to a PortableGit folder root). |
> | **Pin to Linux** | n/a | Set the **`AGENT_LABEL`** build parameter to a Linux node label to skip Windows entirely. |
>
> `resolveBash()` probes, in order: `SLOC_BASH` → `SLOC_PORTABLE_GIT` → `where bash`
> (skipping the WSL `System32\bash.exe` launcher, which is not used) → system installs
> → `%LOCALAPPDATA%\Programs\Git` → `<workspace>\.tools\PortableGit` → `%USERPROFILE%\PortableGit`
> → `C:\Tools\PortableGit` → `C:\PortableGit`. It derives the MinGW linker dir from
> whichever bash it finds, so a portable folder of **any** name works.
>
> **Fully bash-free backup:** if you cannot get *any* bash onto the host, build with
> the native-PowerShell installer instead — it reproduces the whole offline flow
> (checksum-verify → reassemble split parts → extract with the built-in `tar.exe` →
> bootstrap the bundled toolchain → `cargo build --release --offline`) using only
> tools that ship with Windows 10/11, no Git Bash:
>
> ```powershell
> powershell -ExecutionPolicy Bypass -File scripts\internal\install.ps1
> ```
>
> The **one** thing PowerShell cannot supply is the C linker: the bundled toolchain
> targets `x86_64-pc-windows-gnu`, which links with MinGW `gcc`/`ld`. The installer
> auto-locates it from a staged PortableGit's `mingw64\bin` (or `-MingwBin <dir>` /
> `$env:SLOC_MINGW_BIN`, or a `gcc` already on PATH). So a single staged PortableGit
> folder covers *both* the pipeline's bash path and this native path's linker — you
> never need a system/admin install of Git for Windows.

#### Adapting to your own project

Copy and edit the relevant stages. Minimum viable snippet:

```groovy
pipeline {
    agent any

    environment {
        RUST_LOG    = 'warn'
        SLOC_BROWSER = ''   // set if you need PDF export
    }

    stages {
        stage('Setup') {
            steps {
                sh '''
                    if ! command -v cargo &>/dev/null; then
                        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
                            | sh -s -- -y --default-toolchain stable
                        export PATH="$HOME/.cargo/bin:$PATH"
                    fi
                    if [ -d vendor ]; then
                        :   # already present
                    elif [ -f vendor.tar.gz.aa ]; then
                        sha256sum -c vendor.checksums.sha256
                        cat vendor.tar.gz.* | tar -xzf -
                    fi
                    # No vendor.tar.gz.* → cargo fetches from crates.io (online mode).
                '''
            }
        }

        stage('Build') {
            steps { sh 'cargo build --release -p oxide-sloc' }
        }

        stage('Scan') {
            steps {
                sh '''
                    ./target/release/oxide-sloc analyze ./src \
                        --json-out out/result.json \
                        --html-out out/report.html \
                        --report-title "${JOB_NAME} #${BUILD_NUMBER}"
                '''
            }
        }

        stage('Archive') {
            steps {
                archiveArtifacts artifacts: 'out/**', fingerprint: true
            }
        }
    }
}
```

**Environment variables:**

| Variable | Purpose |
|----------|---------|
| `RUST_LOG` | Tracing verbosity (`warn`, `info`, `debug`) |
| `SLOC_API_KEY` | API key for the web UI when deployed on a shared host |

---

### Publishing to Confluence

oxide-sloc does not have a native Confluence connector, but the JSON and HTML outputs make integration straightforward using the Confluence REST API.

#### Prerequisites

- A Confluence Cloud or Data Center instance
- A Confluence API token (Cloud) or Personal Access Token (Data Center)
- `curl` or `python3` available on the Jenkins agent

#### Step 1 — Run the scan and generate JSON + HTML

```groovy
stage('Scan') {
    steps {
        sh '''
            ./target/release/oxide-sloc analyze ./src \
                --json-out  out/result.json \
                --csv-out   out/report.csv \
                --xlsx-out  out/report.xlsx \
                --html-out  out/report.html \
                --report-title "SLOC report — ${BUILD_TAG}"
        '''
    }
}
```

#### Step 2 — Push the HTML report as a Confluence page

Use the Confluence REST API to create or update a page. The HTML report is a fully self-contained document — embed it inside a Confluence storage-format body.

> **Note (Cloud):** the `html` macro is admin-gated on Confluence Cloud. If you cannot enable it, use the "attach the HTML as a Confluence page attachment" alternative below.

```groovy
stage('Publish to Confluence') {
    environment {
        CONFLUENCE_URL   = 'https://your-org.atlassian.net/wiki'
        CONFLUENCE_SPACE = 'ENG'
        CONFLUENCE_USER  = credentials('confluence-user-email')
        CONFLUENCE_TOKEN = credentials('confluence-api-token')
        PAGE_TITLE       = "OxideSLOC — Jenkins CI Report"  // renamed from "SLOC Report — ${env.JOB_NAME}" in f96f53d; kept as example
    }
    steps {
        sh '''
            # Read code-line count from JSON for the page summary
            CODE_LINES=$(python3 -c "import json,sys; d=json.load(open('out/result.json')); print(d['summary_totals']['code_lines'])")

            # Wrap the standalone HTML inside a Confluence page body
            # (ac:structured-macro embeds the raw HTML via the HTML macro)
            PAGE_BODY=$(cat <<EOF
<ac:structured-macro ac:name="html">
  <ac:plain-text-body><![CDATA[
$(cat out/report.html)
  ]]></ac:plain-text-body>
</ac:structured-macro>
EOF
)

            # Check if the page already exists
            EXISTING=$(curl -s -u "${CONFLUENCE_USER}:${CONFLUENCE_TOKEN}" \
                "${CONFLUENCE_URL}/rest/api/content?title=$(python3 -c "import urllib.parse,sys; print(urllib.parse.quote('${PAGE_TITLE}'))")&spaceKey=${CONFLUENCE_SPACE}" \
                | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['results'][0]['id'] if d['results'] else '')")

            if [ -z "$EXISTING" ]; then
                # Create new page
                curl -s -u "${CONFLUENCE_USER}:${CONFLUENCE_TOKEN}" \
                    -X POST \
                    -H "Content-Type: application/json" \
                    -d "{
                        \"type\": \"page\",
                        \"title\": \"${PAGE_TITLE}\",
                        \"space\": {\"key\": \"${CONFLUENCE_SPACE}\"},
                        \"body\": {
                            \"storage\": {
                                \"value\": $(python3 -c "import json,sys; print(json.dumps(sys.stdin.read()))" <<< \"$PAGE_BODY\"),
                                \"representation\": \"storage\"
                            }
                        }
                    }" \
                    "${CONFLUENCE_URL}/rest/api/content"
            else
                # Update existing page (increment version)
                VERSION=$(curl -s -u "${CONFLUENCE_USER}:${CONFLUENCE_TOKEN}" \
                    "${CONFLUENCE_URL}/rest/api/content/${EXISTING}" \
                    | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['version']['number'])")
                NEXT_VERSION=$((VERSION + 1))

                curl -s -u "${CONFLUENCE_USER}:${CONFLUENCE_TOKEN}" \
                    -X PUT \
                    -H "Content-Type: application/json" \
                    -d "{
                        \"type\": \"page\",
                        \"title\": \"${PAGE_TITLE}\",
                        \"version\": {\"number\": ${NEXT_VERSION}},
                        \"body\": {
                            \"storage\": {
                                \"value\": $(python3 -c "import json,sys; print(json.dumps(sys.stdin.read()))" <<< \"$PAGE_BODY\"),
                                \"representation\": \"storage\"
                            }
                        }
                    }" \
                    "${CONFLUENCE_URL}/rest/api/content/${EXISTING}"
            fi
        '''
    }
}
```

#### Alternative — attach the HTML as a Confluence page attachment

If you prefer to keep the HTML as an artifact and link to it from a Confluence page:

```bash
# Upload the HTML file as an attachment to a known page ID
curl -u "${CONFLUENCE_USER}:${CONFLUENCE_TOKEN}" \
     -X POST \
     -H "X-Atlassian-Token: no-check" \
     -F "file=@out/report.html;type=text/html" \
     "${CONFLUENCE_URL}/rest/api/content/${PAGE_ID}/child/attachment"
```

#### Using the JSON for a custom Confluence table

Parse key metrics from JSON and build a lightweight table in the Confluence page body:

```python
#!/usr/bin/env python3
"""Generate a Confluence storage-format summary table from oxide-sloc JSON."""
import json, sys

with open("out/result.json") as f:
    run = json.load(f)

totals = run["summary_totals"]
languages = run["totals_by_language"]

rows = "\n".join(
    f"<tr><td>{lang['language']['display_name']}</td>"
    f"<td>{lang['files']}</td>"
    f"<td>{lang['code_lines']}</td>"
    f"<td>{lang['comment_lines']}</td>"
    f"<td>{lang['blank_lines']}</td></tr>"
    for lang in languages
)

table = f"""
<table>
<tbody>
<tr>
  <th>Language</th><th>Files</th><th>Code</th><th>Comments</th><th>Blank</th>
</tr>
{rows}
<tr>
  <td><strong>Total</strong></td>
  <td><strong>{totals['files_analyzed']}</strong></td>
  <td><strong>{totals['code_lines']}</strong></td>
  <td><strong>{totals['comment_lines']}</strong></td>
  <td><strong>{totals['blank_lines']}</strong></td>
</tr>
</tbody>
</table>
"""
print(table)
```

---

## Atlassian Suite (Confluence + Bitbucket) — built-in CI integration

The roll-your-own recipes above still work, but the Jenkins pipeline **ships with
a first-party Atlassian integration** that needs no Marketplace app and no
mandatory Jenkins plugin. After every build, `post { always }` calls
`runBitbucketNotify()` (in `ci/jenkins/pipeline-helpers.groovy`), which:

1. optionally fires the `bitbucketStatusNotify(...)` plugin step **if the plugin
   is installed** (wrapped in `catch (Throwable)` so an absent plugin can never
   fail the build — see [Plugin-optional design](#plugin-optional-design-tier-3-fallback)),
2. posts a commit build-status via `ci/jenkins/notify-bitbucket.sh` (plugin-independent REST), and
3. upserts a Confluence summary page and attaches the full HTML + PDF report via
   `ci/jenkins/notify-confluence.py`.

Every step is **opt-in and non-fatal**: with no base URL or credential it prints
a one-line "skipping" note and exits 0. A missing, unreachable, or unauthorized
Atlassian target **never** turns the build red.

> Full production test plan (3 privilege tiers, Cloud + Server/DC):
> [`testing/atlassian-integration-test-plan.md`](../testing/atlassian-integration-test-plan.md).
> Standalone REST examples:
> [`testing/examples/confluence/update-sloc-page.sh`](../testing/examples/confluence/update-sloc-page.sh),
> [`testing/examples/bitbucket/bitbucket-pipelines.yml`](../testing/examples/bitbucket/bitbucket-pipelines.yml).

### The two surfaces

oxide-sloc talks to Atlassian over **two independent surfaces**, both plain REST
with a bearer/basic token — neither needs a Connect/Forge app:

| Surface | Confluence | Bitbucket | Code |
|---|---|---|---|
| **Application** (`sloc-web`) | Native REST client: create/update page, versioning, HTML+PDF attachments, **SSRF guard** | report hosted by the app | `crates/sloc-web/src/confluence.rs`, `integrations.rs` |
| **Jenkins CI** | `notify-confluence.py` — upsert page + attach HTML/PDF | `notify-bitbucket.sh` — POST commit build-status | `ci/jenkins/`, wired at `Jenkinsfile` `post{always}` → `runBitbucketNotify()` |

**SSRF note:** the web app enforces an SSRF allow/deny policy on user-supplied
URLs (blocks loopback/link-local/cloud-metadata; **allows** RFC-1918 so
self-hosted Server on a LAN works). The **CI scripts do not** — they run in a
trusted CI context where the base URL is an operator-set job parameter, not
attacker input. This is by design and documented in each script's header.

### Three-tier permission model

The *functionality* (page create/update, attachment upload, build-status POST)
needs only **content-level** permissions + a token. Admin rights matter only to
install the optional plugins and to provision spaces/repos/tokens.

| Tier | Who | What still works |
|---|---|---|
| **1 — system/site admin** | can install anything | everything, incl. the optional plugin path |
| **2 — project/space admin** | manages one space/repo, cannot install apps | everything via REST; plugin path not required |
| **3 — no admin** | plain user, content/repo perms only, **no Jenkins plugin** | everything via REST fallback — this is the path the P0 `catch (Throwable)` fix keeps green |

**Minimum permission per REST call:**

| Call | Endpoint | Minimum permission |
|---|---|---|
| Confluence create/update page | `POST`/`PUT /rest/api/content` | space **Add Page** |
| Confluence attachment upload | `POST …/child/attachment[/{id}/data]` | same page/space **Add** (attachments inherit page perms) |
| Bitbucket Server/DC build-status | `POST /rest/build-status/1.0/commits/{sha}` | token owner **REPO_READ** on a repo containing the commit |
| Bitbucket Cloud build-status | `POST /2.0/repositories/{ws}/{repo}/commit/{sha}/statuses/build` | app password / token with **`repository:write`** |

### Job parameters + credential IDs

The `CONFLUENCE_*` / `BITBUCKET_*` fields are first-class Jenkins **job
parameters** (declared in `Jenkinsfile parameters{}`, auto-exposed as env vars):

| Parameter | Applies to | Example |
|---|---|---|
| `CONFLUENCE_BASE_URL` | both | `https://confluence.corp.com` or `https://acme.atlassian.net/wiki` |
| `CONFLUENCE_USER` | Cloud basic auth (blank ⇒ Server PAT/Bearer) | `ci-bot@acme.com` |
| `CONFLUENCE_SPACE_KEY` | required to publish | `DEV` |
| `CONFLUENCE_PARENT_ID` | optional nesting | `65601` |
| `CONFLUENCE_PAGE_TITLE` | optional (blank ⇒ `oxide-sloc — <JOB_NAME>`) | `oxide-sloc — payments-api` |
| `BITBUCKET_BASE_URL` | both | `https://bitbucket.corp.com` or `https://api.bitbucket.org` |
| `BITBUCKET_WORKSPACE` | Cloud only | `acme` |
| `BITBUCKET_REPO` | Cloud only | `payments-api` |
| `BITBUCKET_USER` | Cloud **app-password** basic auth (blank ⇒ Bearer token) | `ci-bot@acme.com` |

**Tokens are NOT parameters** — add them as Jenkins **Secret Text** credentials
so they are masked in the log:

| Credential ID | Value |
|---|---|
| `confluence-api-token` | Confluence PAT (Server/DC) **or** Cloud API token |
| `bitbucket-build-token` | Bitbucket HTTP access token / app password |

`GIT_COMMIT` (the commit the Bitbucket status attaches to) is resolved
automatically by the `Checkout` stage: for a self-scan it is the tooling repo's
HEAD; when `SCAN_REPO_URL` is set it is the **scanned** repo's commit, so the
status lands on the commit that was actually analyzed.

### Cloud vs Server/DC differences

| | Confluence Cloud | Confluence Server/DC | Bitbucket Cloud | Bitbucket Server/DC |
|---|---|---|---|---|
| Base URL | `https://acme.atlassian.net/wiki` | `https://confluence.corp.com` | `https://api.bitbucket.org` | `https://bitbucket.corp.com` |
| Auth | Basic (`email:api-token`) → set `CONFLUENCE_USER` | Bearer PAT → leave `CONFLUENCE_USER` blank | Basic (`user:app_password`) → set `BITBUCKET_USER`; **or** Bearer access token | Bearer PAT/HTTP token → leave `BITBUCKET_USER` blank |
| Build-status success code | — | — | **201 Created** | **204 No Content** |
| Content REST | v1 today (`/rest/api/content`); **v2 is the future** (`/wiki/api/v2`) — see the TODO in `notify-confluence.py` | v1 (`/rest/api/content`) | — | — |

`notify-bitbucket.sh` treats **any 2xx** as success and prints the actual code,
so both 201 and 204 log as `→ Bitbucket (2xx)`.

### Atlassian auth schemes

Set the username parameter only when the credential is an **app password / Cloud
basic** credential; leave it blank for a **bearer token / PAT**:

| Target | Credential | Scheme | Set the `*_USER` param? |
|---|---|---|---|
| Confluence Cloud | API token | Basic | `CONFLUENCE_USER` = account email |
| Confluence Server/DC | Personal Access Token | Bearer | no |
| Bitbucket Cloud | **app password** | Basic | `BITBUCKET_USER` = username/email |
| Bitbucket Cloud | repo/workspace access token | Bearer | no |
| Bitbucket Server/DC | HTTP access token / PAT | Bearer | no |

> **Common Cloud gotcha:** a Bitbucket **app password** must use Basic auth. If
> you mint an app password (as the test plan suggests) but leave `BITBUCKET_USER`
> blank, the script falls back to Bearer and Cloud rejects it. Set
> `BITBUCKET_USER` and it uses `user:app_password` Basic auth.

### Plugin-optional design (Tier-3 fallback)

The two Bitbucket Jenkins plugins (`bitbucket`,
`bitbucket-build-status-notifier`) are **optional**. When
`bitbucketStatusNotify` is undefined (plugin absent), Jenkins throws
`java.lang.NoSuchMethodError` — a `java.lang.Error`, **not** an `Exception`. The
call is therefore wrapped in `catch (Throwable e)` (not `catch (Exception)`), so
the error is swallowed, `post { always }` continues, and the plugin-independent
`notify-bitbucket.sh` REST path posts the status. This is what makes the
**no-admin / no-plugin Tier 3** path work and stay green. A static guard test
(`ci/jenkins/tests/test-pipeline-helpers-guards.py`) fails if this ever regresses
to `catch (Exception)` or if the unsupported `buildUrl:` argument is re-added.

The offline plugin bundle story is documented in
[`ci/jenkins/INTEGRATION.md`](../ci/jenkins/INTEGRATION.md#offline-jenkins-plugins)
and the test plan §1.

### Atlassian troubleshooting (keyed to log lines)

| Log line | Meaning | Fix |
|---|---|---|
| `notify-confluence: not configured …` | base URL / space / token missing | set `CONFLUENCE_BASE_URL`, `CONFLUENCE_SPACE_KEY`, and the `confluence-api-token` credential |
| `notify-confluence: Confluence unreachable (air-gapped?) …` | network/DNS/connection failure | expected on air-gapped controllers; non-fatal. Check base URL + reachability |
| `notify-confluence: create/update failed (401/403) …` | auth/permission problem | Cloud: set `CONFLUENCE_USER` (Basic). Server: use a PAT (Bearer). Ensure **Add Page** permission |
| `notify-confluence: warning — no scan result JSON found …` | result file name drifted from the slug | non-fatal; the page publishes with an empty table. Check `OUTPUT_SUBDIR`/naming (P1 glob fallback covers most cases) |
| `notify-confluence: attachment '…' upload returned 400 …` | strict Server/DC rejected a same-name re-POST | handled automatically — the script retries via the per-attachment `/data` endpoint |
| `notify-bitbucket: not configured …` | base URL / token / commit missing | set `BITBUCKET_BASE_URL`, `bitbucket-build-token`, and ensure `GIT_COMMIT` resolves |
| `notify-bitbucket: Cloud needs BITBUCKET_WORKSPACE + BITBUCKET_REPO …` | Cloud target without ws/repo | set both Cloud parameters |
| `notify-bitbucket: Bitbucket returned HTTP 401 …` | auth problem | Cloud app password → set `BITBUCKET_USER` (Basic). Token/PAT → leave it blank (Bearer) |
| `Bitbucket direct notify skipped (no 'bitbucket-build-token' credential) …` | credential not defined | add the `bitbucket-build-token` Secret Text credential |
| `Bitbucket status notify via plugin skipped (plugin not installed) …` | expected on Tier 3 | none — the REST path posts the status |

---

## GitHub Actions

Two workflows ship in `.github/workflows/`:

| Workflow       | Trigger                        | Purpose                                                                              |
|----------------|--------------------------------|--------------------------------------------------------------------------------------|
| `ci.yml`       | push to `main`, all PRs        | fmt → lint → build → smoke tests → web UI health check                               |
| `release.yml`  | push a `v*` tag                | cross-compile for 5 platforms → GPG + cosign sign → VirusTotal scan → publish release |
| `vt-scan.yml`  | manual (`workflow_dispatch`)   | on-demand VirusTotal scan against a release tag or HEAD build                        |

### Adding a scan step to an existing workflow

```yaml
- name: Verify + decompress vendor sources
  run: sha256sum -c vendor.checksums.sha256 && cat vendor.tar.gz.* | tar -xzf -

- name: Install oxide-sloc
  run: cargo install --path crates/sloc-cli

- name: Run SLOC scan
  run: |
    oxide-sloc analyze ./src \
      --json-out  out/result.json \
      --csv-out   out/report.csv \
      --xlsx-out  out/report.xlsx \
      --html-out  out/report.html \
      --report-title "SLOC — ${{ github.ref_name }}"

- name: Upload SLOC report
  uses: actions/upload-artifact@v4
  with:
    name: sloc-report
    path: out/          # uploads result.json, report.csv, report.xlsx, report.html
    retention-days: 30
```

### Publishing to GitHub Pages or a wiki

```yaml
- name: Publish report to GitHub Pages
  uses: peaceiris/actions-gh-pages@v4
  with:
    github_token: ${{ secrets.GITHUB_TOKEN }}
    publish_dir: out/
    destination_dir: sloc-reports/${{ github.run_number }}
```

### Sending metrics to a webhook (Slack, Teams, custom)

```yaml
- name: Post metrics to webhook
  env:
    SLOC_WEBHOOK_URL: ${{ secrets.SLOC_WEBHOOK_URL }}
  run: |
    oxide-sloc send out/result.json \
      --webhook-url "$SLOC_WEBHOOK_URL"
```

### VirusTotal binary scanning

The `virustotal` job in `release.yml` submits every release binary to VirusTotal for malware scanning and embeds the results table in the GitHub Release body. It runs automatically on every `v*` tag push when `VT_API_KEY` is set.

**Setup:** Go to **Settings → Secrets and variables → Actions** and add a repository secret named `VT_API_KEY` containing your VirusTotal v3 API key. Free accounts support 4 requests/minute and 500/day — sufficient for a 5-binary release.

The job:
1. Downloads all cross-compiled binaries from the `build` job
2. Uploads each to `POST https://www.virustotal.com/api/v3/files`
3. Polls `GET /api/v3/analyses/{id}` until status is `completed` (up to 5 min per binary)
4. Writes a summary table to `vt-report/vt-scan-summary.md` and uploads it as a workflow artifact
5. The `publish` job includes the table in the GitHub Release body

The job uses `continue-on-error: true` so a VT outage or quota exhaustion never blocks a release.

**False positives:** Freshly compiled Rust binaries are sometimes flagged by heuristic engines. Zero malicious detections is the expected result.

---

## GitLab CI

`.gitlab-ci.yml` ships at the repo root and is auto-detected by GitLab. It is a
full-parity port of the Jenkins pipeline, built on GitLab-native surfaces rather
than plugins: the merge-request widget shows the Tests tab, coverage badge +
diff annotations, and the Code Quality report; the HTML report is exposed
directly in the MR and published to GitLab Pages.

**Stages:** `quality` → `build` → `test` → `analyze` → `deliver` → `publish` → `pages`

```
quality:  fmt        clippy(-D warnings)   codequality(→ Code Quality widget)
build:    build (release binary, seeds the Cargo.lock-keyed cache)
test:     test(→ JUnit/Tests tab)   coverage(→ badge + Cobertura MR annotations)
          smoke:plain  smoke:per-file  smoke:policies  smoke:web-ui
analyze:  analyze(json/csv/xlsx/html/pdf, → "SLOC report" in MR)   analyze-external
          ref-diff (MR: diff vs target branch / prev tag)
deliver:  mr-comment   deliver:webhook   deliver:email   notify:confluence   notify:bitbucket
publish:  artifact-push (7 backends)   trigger-downstream
pages:    pages (HTML report + coverage HTML site, default branch)
```

### Native GitLab features

| Feature | Mechanism | Where it shows |
|---|---|---|
| Test results | `cargo nextest … --profile ci` → `artifacts:reports:junit` | MR widget + **Tests** tab |
| Coverage % | `ci/sonar/generate-coverage.sh` + `coverage:` regex | pipeline/MR coverage badge |
| Coverage per-line | `artifacts:reports:coverage_report:cobertura` | MR diff annotations |
| Clippy findings | `ci/gitlab/clippy-to-codeclimate.py` → `artifacts:reports:codequality` | **Code Quality** widget in MR diff |
| HTML report | `artifacts:expose_as: "SLOC report"` + Pages | MR widget link + Pages site |
| MR comment | `oxide-sloc pr-comment --provider gitlab` | comment on the MR |

The `coverage` job installs `libgtk-3-dev libxdo-dev libwayland-dev` (required by
`cargo llvm-cov --all-features`) and runs only when `COVERAGE_STANDALONE=true`,
on the default branch, or manually.

### Run-pipeline parameters

Every scan knob is a form variable (CI/CD → **Run pipeline**), mirroring the
Jenkins `parameters {}` block: `CI_PRESET`, `SCAN_PATH`, `MIXED_LINE_POLICY`,
`DOCSTRINGS_AS_CODE`, `SUBMODULE_BREAKDOWN`, `STYLE_COL_THRESHOLD`,
`ACTIVITY_WINDOW`, `ENABLED_LANGUAGES`, `INCLUDE_GLOBS`, `EXCLUDE_GLOBS`,
`GENERATE_HTML`, `GENERATE_PDF`, `TEST_RUNNER`, `COVERAGE_STANDALONE`,
`COVERAGE_THRESHOLD`, `COMPARE_TO_REF`, `COMPARE_TO_PREV_TAG`, `TARGET_REPO_URL`,
`TARGET_REF`, `WEBHOOK_URL`, `EMAIL_RECIPIENTS`, `ARTIFACT_REPO_*`,
`DOWNSTREAM_PROJECT`.

Secrets go in **Settings → CI/CD → Variables** (masked), *not* the form:

| Variable | Used by |
|---|---|
| `SLOC_VCS_TOKEN` (scope: `api`) | `mr-comment` |
| `SLOC_WEBHOOK_TOKEN` | `deliver:webhook` |
| `SLOC_SMTP_HOST` / `SLOC_SMTP_USER` / `SLOC_SMTP_PASS` | `deliver:email` |
| `SLOC_CONFLUENCE_URL` / `_USER` / `_TOKEN` / `_SPACE` | `notify:confluence` |
| `BITBUCKET_BASE_URL` / `_TOKEN` / `_USER` / `_WORKSPACE` / `_REPO` | `notify:bitbucket` |
| `ARTIFACT_REPO_USER` / `ARTIFACT_REPO_PASS` | `artifact-push` |

### Adding a scan to your project's pipeline (reusable include)

Use the typed `spec:inputs` include (GitLab 15.11+, GA 16.0):

```yaml
include:
  - project: your-group/oxide-sloc
    file: ci/sloc-gitlab.yml
    inputs:
      scan-path: "."
      preset: "default"
      generate-html: true
      ref-diff: true                 # diff vs target branch + comment on MRs
      server-url: "http://sloc.internal:4317"   # optional central-server ingest
      artifact-repo-type: "nexus"    # optional external push
```

For GitLab older than 15.11, copy the `variables:`-only fallback job printed at
the bottom of `ci/sloc-gitlab.yml`.

### Merge-request workflow

On MR pipelines the `ref-diff` job checks out the target branch into a git
worktree, analyzes both sides, and runs `oxide-sloc diff`; `mr-comment` then
posts the delta with `oxide-sloc pr-comment --provider gitlab` (using
`$CI_PROJECT_ID` + `$CI_MERGE_REQUEST_IID` directly). Set `SLOC_VCS_TOKEN`
(scope `api`) to enable the comment.

### Confluence and Bitbucket from GitLab

Confluence publishing uses the native `oxide-sloc send --confluence-*` path
(reads `SLOC_CONFLUENCE_*`). For a richer page layout, call the shared
`ci/jenkins/notify-confluence.py` instead — it is plugin-independent and reads
pure env vars. Bitbucket commit statuses reuse `ci/jenkins/notify-bitbucket.sh`
verbatim; the `notify:bitbucket` job maps GitLab variables onto the script's
inputs:

| Script env | GitLab source |
|---|---|
| `GIT_COMMIT` | `$CI_COMMIT_SHA` |
| `BUILD_KEY` | `$CI_PROJECT_PATH_SLUG` |
| `BUILD_URL` | `$CI_PIPELINE_URL` |
| `REPORT_URL` | `$CI_PAGES_URL` |

### Jenkins → GitLab capability parity

| Jenkins capability | GitLab mechanism | Notes |
|---|---|---|
| fmt / clippy / test gates | `fmt`, `clippy`, `test` jobs | equivalent |
| Clippy warnings trend (warnings-ng) | `codequality` → Code Quality widget | native, inline in MR |
| nextest "Test Result" | `reports:junit` → Tests tab | native |
| Coverage (llvm-cov) | `reports:coverage_report` + `coverage:` regex | native badge + annotations |
| `COVERAGE_THRESHOLD` gate | awk gate on the summary | equivalent |
| Analyze json/html/pdf/csv/xlsx | `analyze` job | equivalent |
| CI presets (TOMLs) | `CI_PRESET` → `--config` | equivalent |
| Analysis-rule params | form variables → CLI flags | equivalent |
| HTML report publishing | `expose_as` + Pages | native, nicer |
| Trends / dashboards | Pages + native pipeline/coverage/test analytics | equivalent |
| Git-ref scan | `analyze-external` (`git-scan`) | equivalent |
| Git-ref compare / prev-tag | `ref-diff` (worktree + `diff`) | MR baseline is native |
| Webhook / email delivery | `deliver:webhook` / `deliver:email` | equivalent |
| Confluence upsert | `notify:confluence` (`send --confluence-*`) | equivalent |
| Bitbucket commit status | `notify:bitbucket` (shared script) | equivalent |
| PR/MR comment | `mr-comment` (`pr-comment --provider gitlab`) | native |
| Artifact push (7 backends) | `artifact-push` → `ci/artifact-push.sh` | equivalent |
| Web UI health check | `smoke:web-ui` | equivalent |
| Downstream chaining | `trigger-downstream` (`trigger:`) | native |
| Air-gap vendor / offline | default `before_script` | equivalent |
| CSP relax / plugin install / disk preflight / Groovy badges | — | not needed on GitLab |

---

## Artifact Repository Integration

oxide-sloc can push build artifacts to an external artifact repository at the end of every CI build. The integration is implemented in `ci/artifact-push.sh` and wired into every supported pipeline as a dedicated publish stage.

**Supported artifact types:**

| Artifact | Filename pattern | Always generated |
|---|---|---|
| SLOC report (JSON) | `result_<proj-slug>.json` | Yes |
| SLOC report (CSV) | `report_<proj-slug>.csv` | Yes |
| SLOC report (XLSX) | `report_<proj-slug>.xlsx` | Yes |
| SLOC report (HTML) | `report_<proj-slug>.html` | When `GENERATE_HTML=true` |
| SLOC report (PDF) | `report_<proj-slug>.pdf` | When `GENERATE_PDF=true` |
| Compiled binary | `oxide-sloc` / `oxide-sloc.exe` | Yes (from build stage) |
| Test results | `junit.xml` | When cargo-nextest is used |
| Coverage (LCOV) | `lcov.info` | When coverage stage runs |
| Coverage (Cobertura) | `sonar-coverage.xml` | When coverage stage runs |
| Diff comparison | `diff.json`, `diff.csv` | When GIT_REF comparison runs |
| SHA-256 manifest | `checksums.sha256` | When `ARTIFACT_GENERATE_MANIFEST=true` |

`<proj-slug>` is the project slug `<repo-name>_<short-sha>` (e.g. `oxide-sloc_2eb74e9`) — the same
value published as the `OxideSLOC_CI_Report_<proj-slug>` sidebar link, derived in
`ci/jenkins/pipeline-helpers.groovy`.

All configuration is passed via `ARTIFACT_REPO_*` environment variables. The script can be called from Jenkins, GitLab CI, Bitbucket Pipelines, or manually.

### Dry-run mode

Set `ARTIFACT_DRY_RUN=true` to print exactly what would be uploaded without making any network calls — useful for verifying path and credential configuration before a real push.

### Checksum manifest

Set `ARTIFACT_GENERATE_MANIFEST=true` (or check the Jenkins parameter) to automatically generate `checksums.sha256` in the artifact directory and upload it alongside the other files. The manifest uses the standard `sha256sum` format: one `hash  filename` line per file.

### JFrog Artifactory

Uses the Artifactory REST API (`PUT /<repo-path>/<filename>`). Supports user/password Basic auth and API-key auth.

**Build parameters:**

| Parameter | Example value |
|-----------|---------------|
| `ARTIFACT_REPO_TYPE` | `artifactory` |
| `ARTIFACT_REPO_URL` | `https://artifactory.example.com/artifactory/sloc-reports` |
| `ARTIFACT_REPO_PATH` | `oxide-sloc/${JOB_NAME}/${BUILD_NUMBER}` |
| `ARTIFACT_REPO_EXTRA` | _(unused)_ |

**Credentials:**

| Jenkins credential ID | Content |
|-----------------------|---------|
| `SLOC_ARTIFACT_REPO_USER` | Artifactory username (omit for API-key-only auth) |
| `SLOC_ARTIFACT_REPO_PASS` | Artifactory password **or** API key (sent as `X-JFrog-Art-Api` when no username) |

**Standalone example:**

```bash
ARTIFACT_REPO_TYPE=artifactory \
ARTIFACT_REPO_URL=https://artifactory.example.com/artifactory/sloc-reports \
ARTIFACT_REPO_PATH=oxide-sloc/my-project/42 \
ARTIFACT_REPO_USER=ci-user \
ARTIFACT_REPO_PASS=my-api-token \
ARTIFACT_DIR=ci-out \
ARTIFACT_FILES="result.json report.html" \
bash ci/artifact-push.sh
```

---

### Sonatype Nexus Repository Manager 3

Uses the Nexus 3 REST API for **raw-format** repositories (`POST /service/rest/v1/components`). One multipart `POST` is sent per file. Each file is uploaded with its correct MIME type (`application/json`, `text/html`, `application/pdf`, etc.) so Nexus stores and serves assets with accurate content types.

**Prerequisites:** create a raw-format hosted repository in Nexus 3 (e.g., `sloc-raw-hosted`) under **Repositories → Create repository → raw (hosted)**.

**Build parameters:**

| Parameter | Example value |
|-----------|---------------|
| `ARTIFACT_REPO_TYPE` | `nexus` |
| `ARTIFACT_REPO_URL` | `https://nexus.example.com` |
| `ARTIFACT_REPO_PATH` | `oxide-sloc/${JOB_NAME}/${BUILD_NUMBER}` |
| `ARTIFACT_REPO_EXTRA` | `sloc-raw-hosted` ← Nexus repository name |

**Credentials:**

| Jenkins credential ID | Content |
|-----------------------|---------|
| `SLOC_ARTIFACT_REPO_USER` | Nexus username |
| `SLOC_ARTIFACT_REPO_PASS` | Nexus password or user token |

**Standalone example — push all artifact types:**

```bash
# Stage everything into one directory first
mkdir -p /tmp/nexus-stage
cp ci-out/result_myproject.json   /tmp/nexus-stage/
cp ci-out/report_myproject.csv    /tmp/nexus-stage/
cp ci-out/report_myproject.xlsx   /tmp/nexus-stage/
cp ci-out/report_myproject.html   /tmp/nexus-stage/
cp target/release/oxide-sloc      /tmp/nexus-stage/
cp test-results/junit.xml         /tmp/nexus-stage/
cp coverage/lcov.info             /tmp/nexus-stage/

ARTIFACT_REPO_TYPE=nexus \
ARTIFACT_REPO_URL=https://nexus.example.com \
ARTIFACT_REPO_PATH=oxide-sloc/my-project/42 \
ARTIFACT_REPO_EXTRA=sloc-raw-hosted \
ARTIFACT_REPO_USER=ci-user \
ARTIFACT_REPO_PASS=secret \
ARTIFACT_DIR=/tmp/nexus-stage \
ARTIFACT_FILES="result_myproject.json report_myproject.csv report_myproject.xlsx report_myproject.html oxide-sloc junit.xml lcov.info" \
ARTIFACT_GENERATE_MANIFEST=true \
bash ci/artifact-push.sh
```

**Resulting Nexus layout:**

```
sloc-raw-hosted/
  oxide-sloc/my-project/42/
    result_myproject.json
    report_myproject.csv
    report_myproject.xlsx
    report_myproject.html
    oxide-sloc
    junit.xml
    lcov.info
    checksums.sha256
```

---

### Sonatype Nexus Repository Manager 2

Uses the Nexus 2 content REST API (`PUT /content/repositories/<repo>/<path>/<file>`). Include the Nexus 2 context path (`/nexus`) in `ARTIFACT_REPO_URL` if your instance uses it.

**Build parameters:**

| Parameter | Example value |
|-----------|---------------|
| `ARTIFACT_REPO_TYPE` | `nexus2` |
| `ARTIFACT_REPO_URL` | `https://nexus.example.com/nexus` |
| `ARTIFACT_REPO_PATH` | `oxide-sloc/${JOB_NAME}/${BUILD_NUMBER}` |
| `ARTIFACT_REPO_EXTRA` | `sloc-raw-hosted` ← Nexus 2 repository ID |

---

### Amazon S3

Uses the AWS CLI (`aws s3 cp`). Credentials are read from `SLOC_ARTIFACT_REPO_USER` / `SLOC_ARTIFACT_REPO_PASS` (mapped to `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`). Omit them to fall back to the standard AWS credential chain (instance profile, `~/.aws/credentials`, etc.).

**Prerequisites:** `aws` CLI must be installed on the Jenkins agent.

**Build parameters:**

| Parameter | Example value |
|-----------|---------------|
| `ARTIFACT_REPO_TYPE` | `s3` |
| `ARTIFACT_REPO_URL` | `s3://my-sloc-bucket` |
| `ARTIFACT_REPO_PATH` | `oxide-sloc/${JOB_NAME}/${BUILD_NUMBER}` |
| `ARTIFACT_REPO_EXTRA` | `--sse aws:kms` ← optional extra `aws s3 cp` flags |

**Credentials:**

| Jenkins credential ID | Content |
|-----------------------|---------|
| `SLOC_ARTIFACT_REPO_USER` | AWS access key ID |
| `SLOC_ARTIFACT_REPO_PASS` | AWS secret access key |

---

### MinIO

Same as S3 but with a custom `--endpoint-url`. Provide the MinIO server URL in `ARTIFACT_REPO_EXTRA`.

**Prerequisites:** `aws` CLI must be installed on the Jenkins agent.

**Build parameters:**

| Parameter | Example value |
|-----------|---------------|
| `ARTIFACT_REPO_TYPE` | `minio` |
| `ARTIFACT_REPO_URL` | `s3://my-sloc-bucket` |
| `ARTIFACT_REPO_PATH` | `oxide-sloc/${JOB_NAME}/${BUILD_NUMBER}` |
| `ARTIFACT_REPO_EXTRA` | `https://minio.internal:9000` ← MinIO server URL **(required)** |

**Credentials:**

| Jenkins credential ID | Content |
|-----------------------|---------|
| `SLOC_ARTIFACT_REPO_USER` | MinIO access key |
| `SLOC_ARTIFACT_REPO_PASS` | MinIO secret key |

---

### Azure Blob Storage

Uses the Azure CLI (`az storage blob upload`). Derives the storage account name from the host portion of `ARTIFACT_REPO_URL`. Authentication accepts an account key, a SAS token (detected by the `sv=` prefix), or the ambient Azure CLI credential chain (`az login`, Managed Identity).

**Prerequisites:** `az` CLI must be installed on the Jenkins agent.

**Build parameters:**

| Parameter | Example value |
|-----------|---------------|
| `ARTIFACT_REPO_TYPE` | `azure-blob` |
| `ARTIFACT_REPO_URL` | `https://myaccount.blob.core.windows.net` |
| `ARTIFACT_REPO_PATH` | `oxide-sloc/${JOB_NAME}/${BUILD_NUMBER}` |
| `ARTIFACT_REPO_EXTRA` | `sloc-reports` ← container name (default: `sloc-reports`) |

**Credentials:**

| Jenkins credential ID | Content |
|-----------------------|---------|
| `SLOC_ARTIFACT_REPO_USER` | _(unused — auth is key or SAS-token based)_ |
| `SLOC_ARTIFACT_REPO_PASS` | Storage account key **or** SAS token (starting with `?sv=` or `sv=`) |

---

### Generic HTTP PUT

Sends each artifact via `curl -X PUT` to `<ARTIFACT_REPO_URL>/<ARTIFACT_REPO_PATH>/<filename>`. Works with any HTTP server that accepts PUT requests (Gitea package registry, custom artifact stores, Sonatype endpoints, etc.).

**Build parameters:**

| Parameter | Example value |
|-----------|---------------|
| `ARTIFACT_REPO_TYPE` | `generic-http` |
| `ARTIFACT_REPO_URL` | `https://artifacts.example.com/sloc` |
| `ARTIFACT_REPO_PATH` | `oxide-sloc/${JOB_NAME}/${BUILD_NUMBER}` |
| `ARTIFACT_REPO_EXTRA` | _(unused)_ |

**Auth:** `SLOC_ARTIFACT_REPO_USER` + `SLOC_ARTIFACT_REPO_PASS` → HTTP Basic. `SLOC_ARTIFACT_REPO_PASS` alone → `Authorization: Bearer` header.

---

### Registering artifact repo credentials

Register the following Secret Text credentials in Jenkins before triggering a push build.
The pipeline wraps each credential binding in a `try/catch` — **not** `optional: true`,
which is unsupported in credentials-binding 719.x and generates "Unknown parameter" log noise.
Builds with absent credentials log a warning and fall back to an unauthenticated push rather
than aborting.

```bash
set -a; source ci/jenkins/.env; set +a

for ID_SECRET in \
    "SLOC_ARTIFACT_REPO_USER:my-repo-username" \
    "SLOC_ARTIFACT_REPO_PASS:my-api-token-or-password"
do
    ID="${ID_SECRET%%:*}"
    SECRET="${ID_SECRET#*:}"
    crumb=$(curl -sS -u "$JENKINS_USER:$JENKINS_TOKEN" \
        "$JENKINS_URL/crumbIssuer/api/xml?xpath=concat(//crumbRequestField,\":\",//crumb)")
    curl -sS -u "$JENKINS_USER:$JENKINS_TOKEN" -H "$crumb" \
        -X POST "$JENKINS_URL/credentials/store/system/domain/_/createCredentials" \
        --data-urlencode "json={
            \"\": \"0\",
            \"credentials\": {
                \"scope\": \"GLOBAL\",
                \"id\": \"${ID}\",
                \"secret\": \"${SECRET}\",
                \"description\": \"oxide-sloc artifact repository\",
                \"\$class\": \"org.jenkinsci.plugins.plaincredentials.impl.StringCredentialsImpl\"
            }
        }"
    echo "Registered: ${ID}"
done
```

Or via **Manage Jenkins → Credentials → System → Global credentials → Add Credentials** (Kind: Secret text).

---

### Publishing from GitLab CI

The `.gitlab-ci.yml` in this repository has an `artifact-push` job in the
`publish` stage that drives `ci/artifact-push.sh` across **all seven backends**
(`artifactory`, `nexus`, `nexus2`, `s3`, `minio`, `azure-blob`, `generic-http`).
It runs automatically when `ARTIFACT_REPO_TYPE` (and `ARTIFACT_REPO_URL`) are
set, and is otherwise available as a manual action.

**GitLab CI/CD variables to set** (Settings → CI/CD → Variables):

| Variable | Required | Description |
|---|---|---|
| `ARTIFACT_REPO_TYPE` | Yes | One of `artifactory` / `nexus` / `nexus2` / `s3` / `minio` / `azure-blob` / `generic-http` |
| `ARTIFACT_REPO_URL` | Yes | Base URL of the repository service |
| `ARTIFACT_REPO_USER` | Backend-dependent — mask it | Username / access-key ID |
| `ARTIFACT_REPO_PASS` | Backend-dependent — mask it | Password / API token / secret key |
| `ARTIFACT_REPO_PATH` | No | Upload path prefix (default: `oxide-sloc/<project>/<pipeline-iid>`) |
| `ARTIFACT_REPO_EXTRA` | No | Provider-specific (Nexus repo name, Azure container, MinIO endpoint, S3 flags) |
| `ARTIFACT_GENERATE_MANIFEST` | No | `true` to upload a `checksums.sha256` manifest alongside |

The job stages every artifact produced by the pipeline (binary, `result.json`,
`report.{html,pdf,csv,xlsx}`, `junit.xml`, `lcov.info`, `sonar-coverage.xml`,
`diff.{json,csv}` — each skipped if absent).

**Nexus back-compatibility:** the older `NEXUS_REPO_URL` / `NEXUS_USER` /
`NEXUS_PASS` / `NEXUS_REPO_NAME` variables are still honored — if set (and the
generic `ARTIFACT_REPO_*` are not), the job pushes to Nexus 3 exactly as the
former `nexus:push` job did.

**Running the job manually** when no repository is configured:

1. Open the pipeline in GitLab UI.
2. Click the ▶ (play) button on the `artifact-push` job.
3. The job has `allow_failure: true` so it will not block the pipeline even if the push fails.

---

### Publishing from Bitbucket Pipelines

The `testing/examples/bitbucket/bitbucket-pipelines.yml` file includes a **Publish to Nexus** step in both the `default` and `branches.main` pipelines. The step is a no-op when `NEXUS_REPO_URL` is not set, so adding this step does not break existing builds.

**Repository variables to set** (Repository settings → Repository variables):

| Variable | Secured | Description |
|---|---|---|
| `NEXUS_REPO_URL` | No | Base URL, e.g. `https://nexus.example.com` |
| `NEXUS_USER` | No | Nexus username |
| `NEXUS_PASS` | **Yes** | Nexus password or user token — mark as secured |
| `NEXUS_REPO_NAME` | No | Raw repository name (default: `sloc-raw-hosted`) |
| `NEXUS_REPO_PATH` | No | Upload path prefix (default: `oxide-sloc/<repo-slug>/<build-number>`) |
| `NEXUS_VERSION` | No | `nexus2` for NRM 2 (default: `nexus` for NRM 3) |
| `NEXUS_GENERATE_MANIFEST` | No | `true` to upload `checksums.sha256` alongside |

The step stages the compiled binary and scan reports (`result.json`, `report.html`) from earlier steps, then calls `ci/artifact-push.sh`. Artifacts from earlier Bitbucket steps are automatically available to later steps within the same pipeline.

---

## Environment variables reference

| Variable              | Used by     | Purpose                                                                |
|-----------------------|-------------|------------------------------------------------------------------------|
| `RUST_LOG`            | All modes   | Tracing output level: `error`, `warn`, `info`, `debug`, `trace`        |
| `SLOC_API_KEY`        | Web UI      | When set, all requests must supply `Authorization: Bearer <key>` or `X-API-Key: <key>` |
| `SLOC_TLS_CERT`       | Web UI      | Path to PEM certificate for native TLS termination                     |
| `SLOC_TLS_KEY`        | Web UI      | Path to PEM private key for native TLS termination                     |
| `SLOC_GIT_CRED_<HOSTKEY>`  | All modes | Per-host HTTPS credential `user:token` (PAT). `HOSTKEY` = hostname upper-cased, non-alphanumerics → `_`. See [Scanning multiple git instances](multi-instance.md) |
| `SLOC_GIT_SSHKEY_<HOSTKEY>` | All modes | Per-host SSH private-key path                                         |
| `SLOC_GIT_CRED_FILE`  | All modes   | Bulk `host = "user:token"` secrets file (chmod 600; never commit)      |
| `SLOC_GIT_ALLOW_LOCAL`| All modes   | `1`/`true` to permit offline import (git bundle / `file://` / local path). Off by default |
| `SLOC_GIT_LOCAL_ROOT` | All modes   | Directory offline sources must resolve under (required when `SLOC_GIT_ALLOW_LOCAL` is on) |
| `SLOC_GIT_HOST_ALLOWLIST` | All modes | Comma-separated hosts permitted for clone (SSRF control)             |
| `SLOC_GIT_REQUIRE_ALLOWLIST` | All modes | `1`/`true` to fail-closed unless the host is allowlisted          |
| `SLOC_GIT_SSL_NO_VERIFY` | All modes | Last-resort: disable TLS verification (self-signed CA not in any trust store) |
| `SLOC_GIT_TIMEOUT`    | All modes   | Per-git-subprocess wall-clock ceiling in seconds (default 300)         |
| `RUN_WEB_HEALTHCHECK` | Jenkins     | Web UI health-check stage build parameter; **off by default** — check it to run the check |
| `SLOC_SMTP_HOST`      | `send`      | SMTP host (alternative to `--smtp-host`)                               |
| `SLOC_SMTP_USER`      | `send`      | SMTP username (alternative to `--smtp-user`)                           |
| `SLOC_SMTP_PASS`      | `send`      | SMTP password — prefer this over `--smtp-pass` to keep creds out of process listings |
| `SLOC_WEBHOOK_TOKEN`  | `send`      | Bearer token for webhook delivery (alternative to `--webhook-token`)   |
| `VT_API_KEY`          | `release.yml` | VirusTotal API v3 key; enables binary scanning on every tagged release |
| `ARTIFACT_REPO_TYPE`         | Artifact push | Backend: `artifactory` / `nexus` / `nexus2` / `s3` / `minio` / `azure-blob` / `generic-http` |
| `ARTIFACT_REPO_URL`          | Artifact push | Base URL of the artifact repository (see [Artifact Repository Integration](#artifact-repository-integration)) |
| `ARTIFACT_REPO_PATH`         | Artifact push | Path prefix / key prefix for uploaded artifacts |
| `ARTIFACT_REPO_EXTRA`        | Artifact push | Provider-specific config (Nexus repo name, Azure container, MinIO endpoint, S3 flags) |
| `ARTIFACT_REPO_USER`         | Artifact push | Username or access-key ID (set via `SLOC_ARTIFACT_REPO_USER` Jenkins credential) |
| `ARTIFACT_REPO_PASS`         | Artifact push | Password, API token, or secret key (set via `SLOC_ARTIFACT_REPO_PASS` Jenkins credential) |
| `ARTIFACT_GENERATE_MANIFEST` | Artifact push | `true` → generate and upload `checksums.sha256` alongside pushed artifacts |
| `ARTIFACT_DRY_RUN`           | Artifact push | `true` → print what would be uploaded without making any network calls |

---

## CLI flag quick reference

These are the flags most commonly used in CI pipelines:

```bash
oxide-sloc analyze ./src \
  --json-out out/result.json \       # machine-readable output for tooling
  --html-out out/report.html \       # self-contained HTML report
  --pdf-out  out/report.pdf \        # PDF (pure Rust, no browser required)
  --report-title "Sprint 42 Scan" \  # label shown in reports
  --config ci/sloc-ci-default.toml \ # use a pre-configured CI preset
  --include-glob "src/**" \          # narrow scan scope
  --exclude-glob "vendor/**" \       # exclude directories
  --submodule-breakdown \            # separate stats per git submodule
  --plain                            # machine-friendly terminal output

# Re-render a stored JSON without re-scanning
oxide-sloc report out/result.json \
  --html-out out/report-v2.html \
  --pdf-out  out/report-v2.pdf

# Send results via webhook
oxide-sloc send out/result.json \
  --webhook-url "https://hooks.slack.com/services/..."
```

### CI config presets

| File                       | Use case                                          |
|----------------------------|---------------------------------------------------|
| `ci/sloc-ci-default.toml`  | Balanced defaults — mirrors web UI out of the box |
| `ci/sloc-ci-strict.toml`   | Fail-fast — pipeline errors if binary files found |
| `ci/sloc-ci-full-scope.toml` | Audit mode — counts everything including vendor |
