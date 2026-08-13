# Jenkins Manual Setup Guide — oxide-sloc

This guide walks through creating the oxide-sloc pipeline job in Jenkins entirely through
the web UI, with no CLI or scripting required.  Follow every numbered step in order.
Each section maps to one Jenkins screen.

> **Tip:** If you prefer a scripted bootstrap, see `ci/jenkins/README.md` for the CLI
> approach using `job-config.xml` and `seed-job.groovy`.

---

## Table of contents

1. [Prerequisites](#1-prerequisites)
2. [Install Jenkins](#2-install-jenkins)
3. [Complete the setup wizard](#3-complete-the-setup-wizard)
4. [Install required plugins](#4-install-required-plugins)
5. [Add credentials](#5-add-credentials)
6. [Configure the CSP header (HTML report viewer)](#6-configure-the-csp-header-html-report-viewer)
7. [Create the pipeline job](#7-create-the-pipeline-job)
8. [Configure the pipeline job](#8-configure-the-pipeline-job)
9. [Seed the parameters form (first build)](#9-seed-the-parameters-form-first-build)
10. [Run a real build](#10-run-a-real-build)
11. [Verify all features](#11-verify-all-features)
12. [Agent setup — Rust toolchain](#12-agent-setup--rust-toolchain)
13. [Optional agent setup — cargo-nextest (JUnit test results)](#13-optional-agent-setup--cargo-nextest-junit-test-results)
14. [Optional agent setup — cargo-llvm-cov (coverage)](#14-optional-agent-setup--cargo-llvm-cov-coverage)
15. [Troubleshooting](#15-troubleshooting)
16. [Windows agents — known constraints](#16-windows-agents--known-constraints)

---

## 1. Prerequisites

| Requirement | Details |
|-------------|---------|
| Jenkins LTS | 2.387.x or later recommended |
| Agent OS | Linux (x86_64) — the Rust toolchain and shell scripts require bash |
| Disk space | ~2 GB free on the agent: 400 MB toolchain + vendor archive + build output |
| Git | Must be on the agent's `PATH` |
| Internet access | Required to install Jenkins (Options B/C/D) and download plugins (Step 4 Option B). Not needed after `git clone` if `jenkins-plugins.tar.xz` is committed to the repo (Options A and E). |

---

## 2. Install Jenkins

**Which option for your setup?**

| Scenario | Option |
|----------|--------|
| Fully online — no restrictions | B, C, or D |
| Online but no Docker | C or D |
| Air-gapped with Docker | A |
| Air-gapped without Docker | E |
| Air-gapped, no networked machine, no Docker | E (requires `jenkins-plugins.tar.xz` committed to repo) |

> **Options A & E pre-requisite:** `jenkins-plugins.tar.xz` must be committed to the
> repository root.  Run `bash ci/jenkins/bundle-jenkins-plugins.sh` once on a networked
> + Docker machine, then commit both output files:
> ```bash
> git add jenkins-plugins.tar.xz jenkins-plugins.tar.xz.sha256
> git commit -m "ci: bundle Jenkins plugins for air-gapped install"
> ```
> After that one-time step, a plain `git clone` is all any air-gapped host needs —
> no internet, no Docker, no separate download step.

### Option A — Bundled controller image (air-gapped, Docker)

This image is built from `ci/jenkins/Dockerfile.controller` and has every required
plugin installed from `jenkins-plugins.tar.xz` bundled into the archive.
Requires Docker.  Once `jenkins-plugins.tar.xz` is generated and committed (see below),
no internet access is needed after `git clone`.

**No internet access needed once `jenkins-plugins.tar.xz` is committed to the repo.**

```bash
# From the repository root — ensure jenkins-plugins.tar.xz is present:
ls jenkins-plugins.tar.xz jenkins-plugins.tar.xz.sha256

# Build the controller image:
docker build \
    -t jenkins-oxide-sloc-controller:latest \
    -f ci/jenkins/Dockerfile.controller \
    .

# Run:
docker run -d \
    --name jenkins-controller \
    -p 8080:8080 \
    -v jenkins_home:/var/jenkins_home \
    jenkins-oxide-sloc-controller:latest
```

If `jenkins-plugins.tar.xz` is not yet present in the repo, generate it once on a
networked machine and commit it:

```bash
bash ci/jenkins/bundle-jenkins-plugins.sh   # requires Docker + internet
git add jenkins-plugins.tar.xz jenkins-plugins.tar.xz.sha256
git commit -m "ci: bundle Jenkins plugins for air-gapped install"
```

Open `http://localhost:8080` in a browser and proceed to Step 3.
The setup wizard is **disabled** in this image — go straight to
[Step 5 (Add credentials)](#5-add-credentials).

### Option B — Plain Docker (online, manual plugin install)

```bash
docker run -d \
  --name jenkins \
  -p 8080:8080 \
  -v jenkins_home:/var/jenkins_home \
  jenkins/jenkins:lts-jdk21
```

Open `http://localhost:8080`, complete the setup wizard, then follow Step 4 below
to install plugins manually.

### Option C — Native package (Debian/Ubuntu)

```bash
curl -fsSL https://pkg.jenkins.io/debian-stable/jenkins.io-2023.key \
  | sudo tee /usr/share/keyrings/jenkins-keyring.asc > /dev/null
echo "deb [signed-by=/usr/share/keyrings/jenkins-keyring.asc] \
  https://pkg.jenkins.io/debian-stable binary/" \
  | sudo tee /etc/apt/sources.list.d/jenkins.list > /dev/null
sudo apt-get update && sudo apt-get install -y jenkins
```

After starting Jenkins, install bundled plugins without internet access:

```bash
bash ci/jenkins/install-jenkins-plugins.sh --restart
```

### Option D — Native package (RHEL/CentOS/Rocky)

```bash
sudo wget -O /etc/yum.repos.d/jenkins.repo https://pkg.jenkins.io/redhat-stable/jenkins.repo
sudo rpm --import https://pkg.jenkins.io/redhat-stable/jenkins.io-2023.key
sudo yum install -y jenkins && sudo systemctl start jenkins
bash ci/jenkins/install-jenkins-plugins.sh --restart
```

### Option E — Native package, air-gapped (no Docker)

For hosts with no internet access and no Docker.  Requires `jenkins-plugins.tar.xz`
committed to the repository root (see the pre-requisite note at the top of this section).

Install Jenkins via your local package mirror (substitute the mirror URL for the
public `pkg.jenkins.io` URL in Option C or D), then install plugins from the committed
archive:

```bash
# From the repository root on the Jenkins host:
bash ci/jenkins/install-jenkins-plugins.sh --restart
```

`install-jenkins-plugins.sh` finds `jenkins-plugins.tar.xz` in the repository root,
verifies its SHA-256 checksum, extracts every plugin to `$JENKINS_HOME/plugins/`,
pins each plugin to prevent update-center replacement, and restarts Jenkins.

No internet access is needed after `git clone` provided the archive is committed.

Open `http://localhost:8080`, complete the setup wizard, then skip ahead to
[Step 5 (Add credentials)](#5-add-credentials) — plugins are already installed.

---

## 3. Complete the setup wizard

> **Skip this step** if you used Option A (bundled controller image) — the wizard
> is disabled in that image.  Proceed directly to [Step 5](#5-add-credentials).

1. **Get the unlock password:**
   - Docker: `docker exec jenkins cat /var/jenkins_home/secrets/initialAdminPassword`
   - Native: `sudo cat /var/lib/jenkins/secrets/initialAdminPassword`

2. Paste the password at `http://<host>:8080/`.

3. On the "Customize Jenkins" screen, choose **"Install suggested plugins"**.
   Wait for the installation to finish (takes ~2 minutes).

4. Create the first admin user:
   - Fill in **Username**, **Password**, **Full name**, **Email address**.
   - Click **Save and Continue**.

5. Confirm the Jenkins URL shown (e.g., `http://10.0.0.8:8080/`) and click
   **Save and Finish → Start using Jenkins**.

---

## 4. Install required plugins

> **Skip this step** if you used Option A (bundled Docker image) or Option E
> (native air-gapped), or if you already ran `install-jenkins-plugins.sh` during
> Step 2 — all plugins are already installed and pinned.

The suggested-plugin set from Step 3 installs the basics.  Install the remaining
oxide-sloc-specific plugins via the Jenkins UI:

### Option A — From the bundled archive (air-gapped)

```bash
bash ci/jenkins/install-jenkins-plugins.sh --restart
```

This extracts every plugin from `jenkins-plugins.tar.xz`, pins them so the update
center cannot replace them, and restarts Jenkins.

### Option B — Manual install via the Jenkins UI (internet required)

1. Go to **Manage Jenkins → Plugins → Available plugins**.

2. Search for and check each of the following:

   | Plugin (search term) | Purpose |
   |---------------------|---------|
   | `Pipeline: Stage View` | Stage visualization in the job UI |
   | `HTML Publisher` | `OxideSLOC_CI_Report_<proj-slug>` / `OxideSLOC_HTML_Report_<proj-slug>` / "Coverage Source" sidebar links (`<proj-slug>` = `oxide-sloc_<short-sha>`) |
   | `Plot` | Build-over-build SLOC trend charts |
   | `JUnit` | "Test Result" sidebar link (cargo-nextest builds) |
   | `Coverage` | Native LCOV/Cobertura integration — line/branch % on build pages |
   | `Timestamper` | Timestamps on every console output line |
   | `AnsiColor` | ANSI colours in Rust compiler / clippy output |
   | `Job DSL` | Runs `seed-job.groovy` to create the pipeline job |
   | `Copy Artifact` | Copies artifacts between chained pipeline jobs |
   | `Credentials Binding` | Binds secrets as environment variables |
   | `Workspace Cleanup` | `cleanWs()` in `post { cleanup }` |
   | `Pipeline Utility Steps` | `readJSON`, `readFile`, `fileExists` in post steps |
   | `Badge` *(optional)* | `addBadge()` / `createSummary()` — SLOC/test/coverage headline on the run row and boxed build summary. Pipeline degrades to the plain-text description when absent |
   | `Warnings Next Generation` *(optional)* | `recordIssues()` — trend graphs + per-issue drill-down for style findings. Pipeline degrades to the native dashboard when absent |

   > **Optional — Bitbucket:** `Bitbucket Branch Source` and
   > `Bitbucket Build Status Notifier`.
   >
   > The `Badge` and `Warnings Next Generation` rows above are optional too — they
   > match `ci/jenkins/plugins.txt` and `preflight.sh` (both list them as optional
   > run-page enrichment). A system-admin build can also install them automatically
   > at runtime; without them the build still succeeds with a reduced dashboard.

3. Click **Install**, then check **"Restart Jenkins when installation is complete"**.

4. Wait for Jenkins to restart and log back in.

---

## 5. Add credentials

The pipeline uses Jenkins credentials to keep secrets out of build logs and the
Jenkinsfile itself.  Add only the credentials that match the features you intend to use.

### 5a. Navigate to the credentials store

**Manage Jenkins → Credentials → System → Global credentials (unrestricted)**
→ **Add Credentials** (button in the left sidebar or top-right).

### 5b. SonarQube analysis token

> **Note:** The SonarQube pipeline stage was removed in the current Jenkinsfile.
> SonarQube analysis is now run externally via `ci/sonar/` scripts and the
> `SONAR_URL` / `SONAR_TOKEN` environment variables, not through a Jenkins
> credential binding.  The `sonarqube-oxide-sloc-token` credential ID described
> in older docs is **not consumed by the current Jenkinsfile** — you do not need
> to create it.  Skip this step unless you have re-added a SonarQube stage to a
> custom Jenkinsfile.

### 5c. Artifact repository (required for artifact push stage)

Add **two** credentials, one for the username and one for the password/token.

> **Credential fallback behavior:** the pipeline wraps this binding in a `try/catch` — not
> `optional: true`, which is unsupported in credentials-binding 719.x and would generate
> "Unknown parameter" log noise.  If either credential is absent the build logs a warning
> and falls back to an unauthenticated push rather than failing.

**Credential 1:**
| Field | Value |
|-------|-------|
| Kind | **Secret text** |
| Secret | Your artifact repo username or access-key ID |
| ID | `SLOC_ARTIFACT_REPO_USER` |

**Credential 2:**
| Field | Value |
|-------|-------|
| Kind | **Secret text** |
| Secret | Your artifact repo password, API token, or secret key |
| ID | `SLOC_ARTIFACT_REPO_PASS` |

### 5d. Webhook Bearer token (optional)

| Field | Value |
|-------|-------|
| Kind | **Secret text** |
| Secret | Your webhook Bearer token |
| ID | `SLOC_WEBHOOK_TOKEN` |

### 5e. Email / SMTP (optional)

Add **three** credentials:

| ID | Secret value |
|----|-------------|
| `SLOC_SMTP_HOST` | SMTP server hostname (e.g., `smtp.example.com`) |
| `SLOC_SMTP_USER` | SMTP username / from address |
| `SLOC_SMTP_PASS` | SMTP password or app password |

### 5f. Bitbucket credentials (optional)

If using Bitbucket as the SCM:

| Field | Value |
|-------|-------|
| Kind | **Username with password** |
| Username | Your Bitbucket username |
| Password | A Bitbucket app password with repo-read scope |
| ID | `bitbucket-credentials` |

### 5g. Jenkins API token (fully optional — in-pipeline CSP relaxation)

> **TL;DR — you almost certainly do not need this credential.** The pipeline runs
> fine WITHOUT `jenkins-api-token` and WITHOUT any CSP relaxation. HTML reports
> still render: `ci/jenkins/extract-report-assets.py` externalises the report's
> inline CSS/JS so it renders under Jenkins' default CSP, and modern Jenkins
> (2.387.x+) serves the CSP report-only (non-blocking). For corporate / locked-down
> controllers, the recommended fix for the interactive artifact viewer is the
> credential-free `init.groovy.d/relax-csp.groovy` approach in
> [Step 6](#6-configure-the-csp-header-html-report-viewer) — see also the
> [Corporate / locked-down Jenkins](#corporate--locked-down-jenkins) section.

The Setup stage *optionally* relaxes the artifact-viewer Content Security Policy via
`System.setProperty` (works only when "Use Groovy Sandbox" is **unchecked**) and, as a
fallback, via the Jenkins Script Console REST API. The REST API path uses an admin API
token bound as a Jenkins credential — but it is entirely optional:

**Sandbox is ON by default for SCM-defined pipelines.** When a Pipeline job pulls its
`Jenkinsfile` from SCM, Jenkins enforces the Groovy sandbox by default, so the
in-pipeline `System.setProperty` call cannot apply the CSP. When both the direct set
and the API path are unavailable, the Setup stage now logs a **single calm INFO line**
("CSP auto-relax skipped … This is OPTIONAL and non-fatal — HTML reports still render …")
rather than an alarming caught-exception dump. Nothing fails; only the interactive
artifact-viewer styling on very old controllers is affected. The permanent, sandbox-proof
fix is `init.groovy.d/relax-csp.groovy` (no credential needed).

On `workflow-cps` versions that still honor it, `<sandbox>false</sandbox>` in
`ci/jenkins/job-config.xml` and `ci/jenkins/job-config.xml.tmpl` disables the sandbox.
**On current Jenkins LTS (`workflow-cps` 3900+, including 2.555.x) this element is
ignored** — `CpsScmFlowDefinition` no longer exposes a sandbox field, so SCM-defined
pipelines are always sandboxed and the `<sandbox>` line is a harmless no-op (Jenkins
silently strips it on import). There is no web-UI toggle to disable it either.
Instead, rely on the `jenkins-api-token` credential (the REST fallback below) for
in-pipeline CSP relaxation, or use the `init.groovy.d/relax-csp.groovy` approach in
[Step 6](#6-configure-the-csp-header-html-report-viewer) — recommended, since it needs
no credential.

| Field | Value |
|-------|-------|
| Kind | **Secret text** |
| Secret | Your Jenkins admin API token — see `ci/jenkins/README.md § Minting a long-lived API token` |
| ID | `jenkins-api-token` |

Without this credential and with the Groovy sandbox enabled, the pipeline emits one
calm INFO line and carries on — the in-pipeline CSP relaxation is simply skipped.
On current Jenkins LTS (2.387.x+) this has no visible effect because the default header
is already `Content-Security-Policy-Report-Only` (non-blocking) and reports render
correctly regardless. On older Jenkins (pre-2.387.x), missing CSP relaxation only
affects the interactive artifact viewer — deploy the init script in
[Step 6](#6-configure-the-csp-header-html-report-viewer) as the permanent fix.

> **Credential fallback behavior:** the `jenkins-api-token` binding is wrapped in a
> `try/catch` — not `optional: true`, which is unsupported in credentials-binding 719.x
> and generates "Unknown parameter" log noise.  A missing credential is detected as
> the expected "credentials entry … not found" path and handled quietly (no raw
> exception is printed); the build never fails on account of it.

---

## 6. Configure the CSP header (HTML report viewer)

From **Jenkins 2.387.x LTS onward** (including the current 2.555.x LTS series), the
default Content Security Policy is served as `Content-Security-Policy-Report-Only` —
a **non-blocking** header that logs violations to the browser console but does not
prevent inline styles or scripts from executing.  On a current LTS install the
oxide-sloc HTML report renders correctly without any changes to this header.

**Deploying the init script below is still recommended** as a permanent preventive
measure.  It upgrades the header to an explicit allow-list so the report continues to
render correctly if Jenkins is downgraded, the default CSP changes in a future LTS,
or the container is rebuilt from a base image with a stricter policy.

On **pre-2.387.x Jenkins** this step is required — the default CSP was an enforcing
(blocking) header that prevented inline scripts and broke interactive features.

> **Why not the in-pipeline `System.setProperty` approach?** The Jenkinsfile Setup
> stage contains a `System.setProperty` call as a convenience, but it is blocked by
> the Groovy sandbox on fresh installs (see Step 5g), and even when the sandbox is
> off it only takes effect for the duration of that build's JVM session.  Treat the
> in-pipeline path as a **diagnostic fallback only**.  Use the init script below for
> all production setups — it survives restarts and requires no credentials.

### Step-by-step: deploy relax-csp.groovy (init.groovy.d approach)

1. Find your `$JENKINS_HOME`:
   - Docker: `/var/jenkins_home`
   - Native: `/var/lib/jenkins` (or wherever Jenkins is installed)

2. Create the `init.groovy.d` directory if it doesn't exist:
   ```bash
   # Docker:
   docker exec -u root <container-name> mkdir -p /var/jenkins_home/init.groovy.d
   # Native:
   sudo mkdir -p /var/lib/jenkins/init.groovy.d
   ```

3. Copy the provided Groovy script into Jenkins:
   ```bash
   # Docker (discover container name first: docker ps --format '{{.Names}}' | grep -i jenkins):
   docker cp ci/jenkins/init.groovy.d/relax-csp.groovy <container-name>:/var/jenkins_home/init.groovy.d/
   # Native:
   sudo cp ci/jenkins/init.groovy.d/relax-csp.groovy /var/lib/jenkins/init.groovy.d/
   ```
   Or paste this content manually into a new file at that path:
   ```groovy
   // $JENKINS_HOME/init.groovy.d/relax-csp.groovy
   System.setProperty(
     'hudson.model.DirectoryBrowserSupport.CSP',
     "default-src 'self'; style-src 'self' 'unsafe-inline'; " +
     "img-src 'self' data: blob:; script-src 'self' 'unsafe-inline'; " +
     "font-src 'self' data:;"
   )
   ```

4. **Restart Jenkins** for the init script to take effect:
   ```bash
   # Docker:
   docker restart <container-name>
   # Native:
   sudo systemctl restart jenkins
   ```

5. Verify the CSP is set by running `bash ci/jenkins/preflight.sh` — the `[warn] CSP at default`
   message should be gone.

> **On Jenkins 2.387.x+ LTS** this step is optional — the default `Content-Security-Policy-Report-Only`
> header is non-blocking and reports render correctly without it.
> **On pre-2.387.x Jenkins** omitting this step breaks interactive features in the published
> report (CSS loads, but JS is blocked).
> The `preflight.sh` script also supports `bash ci/jenkins/preflight.sh --install-csp`
> (requires Docker on the Jenkins host) to deploy the init script and restart automatically.

### Corporate / locked-down Jenkins

On a hardened controller (Groovy sandbox enforced, no admin API token handed out,
restricted plugins), the pipeline is designed to run cleanly with **no special
configuration**. Key points:

- **The Groovy sandbox is always on for SCM-defined pipelines** on modern
  `workflow-cps` (3900+, incl. Jenkins 2.555.x). There is no `<sandbox>false</sandbox>`
  escape and no web-UI toggle — the direct `System.setProperty` CSP path simply cannot
  apply. This is expected and handled quietly.
- **`jenkins-api-token` is OPTIONAL.** Omit it. The Setup stage detects the missing
  credential as the normal path and prints one calm INFO line instead of an error. The
  build never fails for lack of it.
- **CSP relaxation is OPTIONAL.** HTML reports render under the default CSP because
  `ci/jenkins/extract-report-assets.py` externalises the report's inline CSS/JS, and
  modern Jenkins serves CSP report-only (non-blocking). You lose nothing but interactive
  artifact-viewer styling on very old (pre-2.387.x) controllers.
- **Permanent, credential-free, sandbox-proof CSP fix (recommended default):** drop
  `ci/jenkins/init.groovy.d/relax-csp.groovy` into `$JENKINS_HOME/init.groovy.d/` and
  restart (see the step-by-step above), or run
  `bash ci/jenkins/preflight.sh --install-csp` on the Jenkins host. This is the
  recommended approach for corporate/sandboxed controllers — it needs no credential and
  survives restarts.
- **Windows agents need a POSIX shell.** Every stage runs POSIX `.sh` scripts. On a
  Linux agent they run natively; on a **Windows agent** the pipeline runs them through
  **Git Bash** (`bash.exe` from Git for Windows, which also provides `curl`, `tar`,
  `grep`, `awk`, `sha256sum`). Install Git for Windows on the agent, or set the
  `SLOC_BASH` environment variable to a `bash.exe` path. The WSL `System32\bash.exe`
  launcher is intentionally **not** used. To avoid the Git-Bash requirement entirely on
  a mixed Windows/Linux controller, set the **`AGENT_LABEL`** build parameter to a Linux
  node label so the job is pinned to Linux.

---

## 7. Create the pipeline job

1. From the Jenkins home page, click **"+ New Item"** in the left sidebar.

2. In the **"Enter an item name"** field, type: `oxide-sloc`

3. Select **"Pipeline"** from the list of item types (the blue icon with connected circles).

4. Click **OK**.

---

## 8. Configure the pipeline job

You are now in the job configuration screen.  Work through each section top-to-bottom.

### 8a. General

| Setting | Value |
|---------|-------|
| Description | `oxide-sloc SLOC analysis pipeline — scans source repositories and produces HTML, JSON, and PDF reports with build-over-build trend data.` |
| ✓ Discard old builds | Check this box |
| → Max # of builds to keep | `25` |
| → Max # of builds with artifacts to keep | `10` |

### 8b. Build Triggers (optional)

If you want the pipeline to trigger automatically:

- **Poll SCM**: Enter a cron expression (e.g., `H/5 * * * *` = every 5 minutes).
- **GitHub hook trigger**: Check this box and configure the GitHub webhook at
  `https://<your-jenkins-url>/github-webhook/`.
- Leave all triggers unchecked if you prefer to trigger builds manually.

### 8c. Pipeline definition

| Setting | Value |
|---------|-------|
| Definition | **Pipeline script from SCM** |
| SCM | **Git** |
| Repository URL | `https://github.com/oxide-sloc/oxide-sloc.git` (or your fork/mirror URL) |
| Credentials | Leave as `- none -` for public repos; select SSH or username/password credentials for private repos |
| Branch Specifier | `main` (a concrete ref — **not** `*/main`, see note) |
| Script Path | `Jenkinsfile` |
| ✓ Lightweight checkout | Check this box |

> **Use a concrete branch ref (`main`), not the wildcard `*/main`.** With
> lightweight checkout enabled, Jenkins loads the `Jenkinsfile` via the jgit-based
> `GitSCMFileSystem`, which resolves the branch with `Repository.findRef()`. That
> returns `null` for a wildcard spec, throwing a `NullPointerException` at
> Jenkinsfile load before the pipeline even starts. A concrete ref (`main` or
> `refs/heads/main`) works for both lightweight and heavyweight checkout.

> **Air-gapped / local repo:** Use `file:///absolute/path/to/oxide-sloc` as the
> Repository URL for a locally cloned copy.

### 8d. Save

Click **Save** at the bottom of the page.

---

## 9. Seed the parameters form (first build)

Jenkins discovers the `parameters {}` block in the Jenkinsfile only after running it
once.  The first build must run **without parameters** to register them.

1. On the job page, click **"Build Now"** in the left sidebar.
   The first build runs with defaults — the fast path: `BUILD_MODE=prebuilt` (extract the
   committed `dist/` binary, no toolchain/compile/tests/lint), `RUN_QUALITY_GATES` and
   `RUN_WEB_HEALTHCHECK` both **off**.  The build executes these stages in order:
   **Checkout → Load helpers → Setup → Quality Gates (Format, Lint, Unit tests — skipped unless
   `RUN_QUALITY_GATES`) → Build → Coverage → Analyze → Web UI health check → Deliver results
   (Send webhook, Send email) → Archive & Publish → Push to Artifact Repository → Git-Ref Scan →
   Git-Ref Compare**, then succeeds.
   With default parameters the Analyze stage scans the **whole tooling repo itself**
   (`SCAN_REPO_URL` blank, `SCAN_PATH` blank → `analyze .`), producing the full
   report set. Optional stages (Quality Gates, Coverage, Web UI health check, webhook, email,
   artifact push, ref scan) are skipped unless the corresponding parameters are configured. A
   standard prebuilt scan finishes in ~2-3 min instead of ~40.

   > **If the build fails** with `MethodTooLargeException` at compile time (before any
   > stage runs), see the [Troubleshooting → MethodTooLargeException](#methodtoolarge
   > exception-at-pipeline-compile-time) section below.

2. Wait for the build to complete. It should be **green** on the first run with default settings.

3. Refresh the job page.  The left sidebar now shows **"Build with Parameters"**.

From this point on, all configuration parameters are visible in the build form.

---

## 10. Run a real build

1. Click **"Build with Parameters"** in the left sidebar.

2. The build form opens with all parameters grouped by function.
   Adjust at minimum:

   | Parameter | Default | What to set |
   |-----------|---------|------------|
   | `SCAN_REPO_URL` | `` (blank) | Git URL of the repository to scan. **Leave blank to scan oxide-sloc itself.** Cloned into `./_target`; `SCAN_PATH` is resolved inside it |
   | `SCAN_REF` | `` (blank = `main`) | Branch/tag/SHA to scan. **Blank defaults to `main`** — set explicitly (e.g. `master`) for repos whose default branch is not `main` |
   | `SCAN_PATH` | `` (blank = whole repo) | Optional subdirectory of the target repo to scan (e.g., `src` or `packages/api`); blank scans the whole repo |
   | `BUILD_MODE` | `prebuilt` | `prebuilt` (default) extracts the committed `dist/` binary for a fast ~2-3 min scan; `source` compiles from vendored crates. `RUN_QUALITY_GATES` / `RUN_COVERAGE` force `source` |
   | `REPORT_TITLE` | `oxide-sloc CI Report` | A descriptive title for the HTML report |
   | `REPORT_HTML` | ✓ checked | Check to produce an HTML report (recommended) |
   | `REPORT_PDF` | ✓ checked | PDF is produced by default; uncheck to skip. **Pure-Rust — no browser required on the agent** |
   | `RUN_WEB_HEALTHCHECK` | ☐ unchecked | **Off by default.** Check only if port 4317 is available on the agent to run the web UI health check |
   | `RUN_ANALYZE_SELFTEST` | ☐ unchecked | **Off by default.** Extra analyzer self-test passes (per-file + the four mixed-line policies) that re-scan the repo with no artifacts. Check only to smoke-test the scanner itself |

   > **Note:** `TOOL_REPO_URL` is the *tooling* repo the scanner is built from (the Jenkinsfile's
   > own checkout) — **not** the scan target (that's `SCAN_REPO_URL` above). Leave it
   > **blank** to reuse the SCM this job was configured from — no internet URL is hardcoded,
   > so an air-gapped controller pointed at a local mirror just works. It resolves in order:
   > `TOOL_REPO_URL` build parameter → `TOOL_REPO_URL` environment variable → the `REPO_URL`
   > environment variable (e.g. sourced from `ci/jenkins/.env`, unchanged and still valid) →
   > the job's own SCM. `TOOL_REPO_BRANCH` sets the ref when you supply an explicit
   > `TOOL_REPO_URL` — use a concrete ref, not the wildcard `*/main`, which NPEs under
   > lightweight (jgit) checkout.

   **To enable unit test results** (requires cargo-nextest on the agent — see Step 13):

   | Parameter | Value |
   |-----------|-------|
   | `TEST_RUNNER` | `cargo-nextest` |
   | `PUBLISH_TEST_RESULTS` | ✓ checked (already the default) |
   | `TEST_FAIL_FAST` | unchecked (see all failures) |

   **To enable code coverage** (requires cargo-llvm-cov on the agent — see Step 14):

   | Parameter | Value |
   |-----------|-------|
   | `RUN_COVERAGE` | ✓ checked (implies `BUILD_MODE=source`) |
   | `COVERAGE_THRESHOLD` | `0` (disabled) or a percentage, e.g. `60` |

   **To push artifacts to an external repository**:

   | Parameter | Value |
   |-----------|-------|
   | `ARTIFACT_REPO_TYPE` | `artifactory`, `nexus`, `s3`, `minio`, `azure-blob`, or `generic-http` |
   | `ARTIFACT_REPO_URL` | Base URL of the repository |
   | `ARTIFACT_PUSH_JSON` / `ARTIFACT_PUSH_HTML` / etc. | Check the artifact types to push |

   **To trigger a downstream pipeline on success (pipeline-of-pipelines)**:

   | Parameter | Value |
   |-----------|-------|
   | `CHAIN_DOWNSTREAM_JOB` | Name of the Jenkins job to trigger automatically after this build succeeds (empty = disabled) |
   | `CHAIN_UPSTREAM_JOB` | Set automatically by an upstream pipeline that invoked this build; identifies the caller job |
   | `CHAIN_UPSTREAM_BUILD` | Set automatically by the upstream pipeline; the caller's build number, passed to the downstream job |

3. Click **Build**.

4. Watch the build progress in the **Stage View** on the job page.
   Click any stage block to see its console output.

---

## 11. Verify all features

After a successful build, confirm each feature is wired correctly:

### CI Report (HTML Publisher)
- The left sidebar shows an **`OxideSLOC_CI_Report_<proj-slug>`** link, where
  `<proj-slug>` is `oxide-sloc_<short-sha>` (the repo slug plus the short commit SHA,
  e.g. `oxide-sloc_00b64bd`). The report name is built from the project slug in
  [`ci/jenkins/pipeline-helpers.groovy`](../ci/jenkins/pipeline-helpers.groovy) (`reportName: "OxideSLOC_CI_Report_${proj}"`).
- **The name carries the commit SHA, so the link text and URL change on every commit** —
  do not hard-code or bookmark it; always reach the report from the current build's sidebar.
- The report is published at **project level** (no build number `<N>` in the path). The URL is
  `/job/<JOB>/OxideSLOC_5fCI_5fReport_5f<proj-slug>/` — Jenkins encodes each underscore as `_5f`,
  e.g. `/job/oxide-sloc/OxideSLOC_5fCI_5fReport_5foxide-sloc_5f00b64bd/`.
- Clicking it opens the report in the browser.
- On current Jenkins LTS (2.387.x+) the CSP is `Content-Security-Policy-Report-Only`
  (non-blocking) — interactive features work without any CSP changes.
  If features are broken on an older Jenkins instance (pre-2.387.x), the enforcing
  default CSP is blocking inline scripts — see [Step 6](#6-configure-the-csp-header-html-report-viewer).

### Standalone HTML Report (HTML Publisher)
- A second sidebar link, **`OxideSLOC_HTML_Report_<proj-slug>`**, is published from
  `ci/jenkins/generate-dashboard.py` (`reportName: "OxideSLOC_HTML_Report_${proj}"`). This is a
  self-contained HTML file that works even on Jenkins instances where the Plot/JUnit/Coverage
  plugins are absent.
- Also published at **project level**, so the URL is
  `/job/<JOB>/OxideSLOC_5fHTML_5fReport_5f<proj-slug>/`,
  e.g. `/job/oxide-sloc/OxideSLOC_5fHTML_5fReport_5foxide-sloc_5f00b64bd/`.
- As with the CI Report, the name includes the commit SHA, so a saved URL 404s after the next
  commit — always follow the sidebar link rather than a bookmark.

### Trend charts (Plot plugin)
- The job page shows **"SLOC Trends"** charts below the build history.
- Charts produced: **"SLOC Totals Over Time"** (line chart), **"Per-Language Code Lines"** (bar chart).
- After 2+ builds, trend lines appear.  The first build registers the data point but no chart is
  drawn until there are at least two points.
- The **"Line Coverage % Over Time"** chart appears after a build with `RUN_COVERAGE` enabled.
- If charts are missing after 2+ builds, verify the Plot plugin is installed:
  **Manage Jenkins → Plugins → Installed plugins** → search for `plot`.
  The pipeline degrades gracefully when the plugin is absent (a warning is logged but the build
  still succeeds).

### Test results (JUnit plugin — nextest only)
- To enable: set `TEST_RUNNER = cargo-nextest` and `PUBLISH_TEST_RESULTS = true`.
- Requires cargo-nextest on the agent (see [Step 13](#13-optional-agent-setup--cargo-nextest-junit-test-results)).
- After a nextest build, the left sidebar shows a **"Test Result"** link with pass/fail/skip counts.
- The job page shows a test trend chart (tests over time).
- Full stack traces on panics are always enabled via `RUST_BACKTRACE=1`.
- Transient failures are retried once before marking as failed (`.config/nextest.toml`).

### Coverage Report (Coverage plugin + HTML Publisher)
- To enable: check `RUN_COVERAGE` (opt-in — it recompiles instrumented, adding ~4-5 min/build, so it is off by default).
- Requires cargo-llvm-cov + `llvm-tools-preview` on the agent (see [Step 14](#14-optional-agent-setup--cargo-llvm-cov-coverage)).
- After a successful coverage build:
  - The build page shows **line %, branch %, function %** from the Coverage plugin.
  - **The Coverage view is served at `/job/<job>/<n>/coverage/`** (the `recordCoverage` id is `coverage`) — not `/coverage-report/` or similar.
  - The left sidebar shows a **"Coverage Source"** link with the annotated HTML source view (source painting targets `crates/`).
  - The job page shows the **"Line Coverage % Over Time"** trend chart, plus a coverage delta vs. the previous build when the `git-forensics` plugin is installed (`discoverReferenceBuild()`; harmless when absent).
- Set `COVERAGE_THRESHOLD` (e.g., `60`) to fail the build when line coverage drops below that %.
- **SonarQube note:** SonarQube imports coverage from **LCOV** (`sonar.rust.lcov.reportPaths=coverage/lcov.info`), not the Cobertura XML — so cargo-llvm-cov's Cobertura duplicate-element quirk (which the Jenkins Coverage plugin rejects) does not affect the SonarQube import.

### PDF report artifact (pure-Rust)
- To enable: check `REPORT_PDF`.
- **No browser, Chromium, or external tool required on the agent** — PDF generation is
  implemented entirely in Rust.  This parameter can be enabled without any additional agent setup.
- The PDF is archived as `ci-out/report_<proj-slug>.pdf` and can be included in artifact repository
  pushes via `ARTIFACT_PUSH_PDF`.

### Build description
- The build list shows a description line like:
  ```
  12,543 code · 1,204 cmts · 987 blank · 87 files | src
  ```
  With test results and coverage appended when enabled:
  ```
  12,543 code · 1,204 cmts · 987 blank · 87 files | src · 42/100 tests OK · 74.2% cov
  ```
  This confirms all metadata was correctly read from the build output.

### Archived artifacts
- Click the build number → **"Build Artifacts"**. Key files produced with default parameters —
  `<proj-slug>` is the project slug `<repo-name>_<short-sha>` (e.g. `oxide-sloc_2eb74e9`), the
  same slug used for the report sidebar links above, derived in
  [`ci/jenkins/pipeline-helpers.groovy`](../ci/jenkins/pipeline-helpers.groovy):
  - `ci-out/result_<proj-slug>.json` (scan output)
  - `ci-out/report_<proj-slug>.html`, `report_<proj-slug>.css`, `report_<proj-slug>.js`,
    `report_<proj-slug>.xlsx`, `report_<proj-slug>.csv` (HTML report with assets and exports)
  - `ci-out/report_<proj-slug>.pdf` (produced by default; suppressed when `REPORT_PDF = false`)
  - `ci-out/scan-config_<proj-slug>.json` (the effective scan configuration used for the run)
  - `ci-out/dashboard_<proj-slug>.{html,css,js}` (native CI dashboard — the source of the
    `OxideSLOC_HTML_Report_<proj-slug>` sidebar link)
  - `ci-out/capabilities.json` (detected plugin/permission state that drives dashboard degradation)
  - `ci-out/summary.csv`, `ci-out/per_language.csv`, `ci-out/style_analysis.csv` (trend CSVs consumed by Plot plugin)
  - `ci-out/sub_<name>.html` (per-submodule breakdown, one per detected submodule; on by default via
    `SUBMODULE_BREAKDOWN = true`)
  - `ci-out/test-results/junit.xml` (when `TEST_RUNNER = cargo-nextest` and `PUBLISH_TEST_RESULTS = true`)
  - `ci-out/coverage/{lcov.info,sonar-coverage.xml,html/}` (when `RUN_COVERAGE = true`)

---

## 12. Agent setup — Rust toolchain

The Rust compiler is **not** bundled in the repository.  The pipeline downloads or
copies it to a persistent cache directory (`~/.rust-cache/`) on first use.

### Option A — Internet-connected agent (simplest)

No setup needed.  The Jenkinsfile's Setup stage downloads the Rust toolchain
automatically via `rustup` on the first build.  Subsequent builds use the cached
toolchain.

### Option B — Pre-seed the cache (faster first build)

Run the cache-seeding script once on the agent before the first build:

```bash
bash ci/jenkins/install-rust-cache.sh
```

This installs the toolchain pinned in `rust-toolchain.toml` into `~/.rust-cache/`
so the first pipeline build skips the download entirely.

### Option C — Docker agent (recommended for isolation)

Build the provided agent image once and run all builds inside it:

```bash
# Build the image (includes the pinned Rust toolchain at /opt/rust-toolchain)
docker build -t jenkins-oxide-sloc:latest -f ci/jenkins/Dockerfile.agent .
```

In Jenkins, configure an agent (or the built-in node) to run containers from this image,
or update `agent { docker { image 'jenkins-oxide-sloc:latest' } }` in the Jenkinsfile.

### Option D — Air-gapped bundle (no internet on agent)

Bundle the toolchain on a networked machine and commit the archive:

```bash
bash ci/jenkins/bundle-rust-toolchain.sh
# Produces: rust-toolchain-bundle.tar.xz + rust-toolchain-bundle.tar.xz.sha256
# Split before committing (bundle is typically 200-350 MB):
split -b 45m rust-toolchain-bundle.tar.xz rust-toolchain-bundle.tar.xz.
rm rust-toolchain-bundle.tar.xz
git add rust-toolchain-bundle.tar.xz.* rust-toolchain-bundle.tar.xz.sha256
git commit -m "chore: add rust toolchain bundle for air-gapped agents"
```

The Jenkinsfile Setup stage detects and verifies this file automatically.

---

## 13. Optional agent setup — cargo-nextest (JUnit test results)

Enable `TEST_RUNNER = cargo-nextest` only after installing nextest on the agent.

```bash
# Install cargo-nextest (requires Rust toolchain already installed)
# --locked is mandatory: current nextest ships a tripwire crate that
# compile_error!s ("Nextest does not support being installed without --locked")
# unless the exact bundled lockfile is used.
cargo install --locked cargo-nextest

# Verify
cargo nextest --version
```

For air-gapped agents, nextest can be installed from the vendor archive if it was
included when the archive was generated.  See `ci/tools/Cargo.toml` for adding it.

**What you get after enabling:**
- JUnit XML at `<OUTPUT_SUBDIR>/test-results/junit.xml` archived with every build.
- **"Test Result"** sidebar link on each build with pass/fail/skip counts.
- Test trend chart on the job page (tests over time).
- Full stack traces on failures via `RUST_BACKTRACE=1` (always enabled in the Unit tests stage).
- Retry of flaky tests once before marking as failed (configured in `.config/nextest.toml`).

---

## 14. Optional agent setup — cargo-llvm-cov (coverage)

Enable `RUN_COVERAGE` only after installing cargo-llvm-cov on the agent.

```bash
# Install cargo-llvm-cov
cargo install cargo-llvm-cov

# Install the required LLVM tools component
rustup component add llvm-tools-preview

# Verify
cargo llvm-cov --version
```

For air-gapped agents, cargo-llvm-cov is vendored in `ci/tools/Cargo.toml`.
Install it offline with:

```bash
cargo install --offline cargo-llvm-cov
rustup component add llvm-tools-preview
```

**What you get after enabling `RUN_COVERAGE`:**
- LCOV file (`lcov.info`) and Cobertura XML (`sonar-coverage.xml`) archived per build.
- Browsable HTML coverage report under `coverage/html/` archived per build.
- **"Coverage Source"** sidebar link on each build (annotated source view).
- Line %, branch %, and function % shown on the build page via the Coverage plugin.
- **"Line Coverage % Over Time"** trend chart on the job page (Plot plugin).
- Optional threshold gate: set `COVERAGE_THRESHOLD` to a number (e.g., `60`) to fail
  the build if line coverage drops below that percentage.

---

## 15. Troubleshooting

### MethodTooLargeException at pipeline compile time

If the build console shows:

```
org.codehaus.groovy.control.MultipleCompilationErrorsException: startup failed:
General error during class generation: Method too large: WorkflowScript.___cps___N
groovyjarjarasm.asm.MethodTooLargeException: Method too large: WorkflowScript.___cps___N
```

this error occurs **at compile time** (before any stage runs) and means the JVM has
rejected the CPS-transformed pipeline because one generated method exceeds the 64 KB
bytecode limit.  No Jenkins configuration can work around it — the fix is in the
Jenkinsfile itself.

**This should not occur with the current Jenkinsfile** — all helper functions have been
extracted to `ci/jenkins/pipeline-helpers.groovy` and are loaded at runtime via
`load 'ci/jenkins/pipeline-helpers.groovy'`.  Each function in the loaded file compiles
as a separate class method with its own 64 KB budget, keeping the main `WorkflowScript`
class well under the limit.

If you see this error after editing the Jenkinsfile, you have re-inflated the compiled
size.  Remedies:

- Do **not** add `def` helper functions directly to the Jenkinsfile — put them in
  `ci/jenkins/pipeline-helpers.groovy` instead.
- Extract any new large `sh '''…'''` heredocs to a `ci/jenkins/*.sh` script and call
  it with `sh 'bash ci/jenkins/my-step.sh'`.
- The declarative linter (`Manage Jenkins → Validate Declarative Pipeline`) catches
  syntax errors but does **not** detect `MethodTooLargeException` — only a real build
  triggers the JVM class-generation check.

### "Build with Parameters" not showing after first build

The first build must **complete successfully through the Checkout and Setup stages**
for Jenkins to parse the Jenkinsfile.  If it failed at Checkout (e.g., wrong repo URL),
fix the SCM configuration under **Configure → Pipeline → Repository URL** and run
**Build Now** again.

### HTML report renders unstyled

The Jenkins CSP header is blocking inline styles.  Follow [Step 6](#6-configure-the-csp-header-html-report-viewer)
to deploy `relax-csp.groovy` and restart Jenkins.

### "vendor.tar.gz.* not found" error

The repository was cloned from a branch or tag that predates the vendor archive commit.
Ensure the `main` branch is checked out.  If using a local clone, run:

```bash
git pull origin main
ls -lh vendor.tar.gz.* vendor.checksums.sha256
```

### "cargo: command not found"

The Rust toolchain is not on the agent's `PATH`.  Check:
1. That the Setup stage completed (look for `rustup show` output in the console).
2. That `CARGO_HOME` and `RUSTUP_HOME` are pointed at a writable directory on the agent.
3. For Docker builds: that the container has write access to `$HOME/.rust-cache/`.

### "cargo-nextest not found"

Install nextest on the agent (see [Step 13](#13-optional-agent-setup--cargo-nextest-junit-test-results))
or switch `TEST_RUNNER` back to `cargo-test`.

### "cargo llvm-cov not found"

Install cargo-llvm-cov on the agent (see [Step 14](#14-optional-agent-setup--cargo-llvm-cov-coverage))
or uncheck `RUN_COVERAGE`.

### SonarQube analysis not running

The SonarQube pipeline stage was removed from the Jenkinsfile.  SonarQube scans
are now run externally via the scripts in `ci/sonar/`, driven by the `SONAR_URL`
and `SONAR_TOKEN` environment variables.  See `docs/sonarqube-manual-setup.md`
for the current setup instructions.  There is no `SKIP_SONAR` parameter in the
current Jenkinsfile.

### Plot charts not appearing

The Plot plugin must be installed and at least **two** builds must complete before
trend data is visible (one data point does not make a chart).  Check that the plugin
is installed: **Manage Jenkins → Plugins → Installed plugins** → filter for "plot".

The pipeline logs `Plot trend charts skipped (install the 'plot' plugin to enable): ...`
when the plugin is absent — the build still succeeds but no charts are registered.

Expected chart names under the **"SLOC Trends"** group on the job page:
- **"SLOC Totals Over Time"** (line chart, always produced)
- **"Per-Language Code Lines"** (bar chart, always produced)
- **"Line Coverage % Over Time"** (line chart, only when `RUN_COVERAGE` is checked)

### "Test Result" sidebar link missing after nextest build

Check that `PUBLISH_TEST_RESULTS` is checked **and** `TEST_RUNNER` is set to `cargo-nextest`.
The JUnit step is a no-op when `TEST_RUNNER = cargo-test` (no XML is generated).

Confirm the JUnit XML was produced: in **Build Artifacts**, look for
`ci-out/test-results/junit.xml`.  If the file is absent, the nextest run may have
exited before writing the XML (check the Unit tests stage console output for errors).

### No "Coverage Source" link or Coverage plugin metrics

Verify `RUN_COVERAGE` is checked and cargo-llvm-cov (or cargo-tarpaulin) is
installed on the agent.  Look in the console output of the Coverage stage for
`==> Generating coverage with cargo-llvm-cov` or `WARNING: Neither cargo-llvm-cov nor
cargo-tarpaulin is installed.`  Follow [Step 14](#14-optional-agent-setup--cargo-llvm-cov-coverage).

### Artifact repository push stage skipped

The `Push to Artifact Repository` stage is skipped when `ARTIFACT_REPO_TYPE = none` (the
default) or `ARTIFACT_REPO_URL` is empty.  Set both parameters, then add the credentials
`SLOC_ARTIFACT_REPO_USER` and `SLOC_ARTIFACT_REPO_PASS` via
**Manage Jenkins → Credentials** (see [Step 5c](#5c-artifact-repository-required-for-artifact-push-stage)).

### "COVERAGE GATE FAILED" — build fails on coverage threshold

Your line coverage is below the `COVERAGE_THRESHOLD` value.  Either:
- Lower the threshold, or
- Improve test coverage, or
- Set `COVERAGE_THRESHOLD = 0` to disable the gate.

The console output shows the exact percentage and threshold for diagnosis.

---

## 16. Windows agents — known constraints

The pipeline runs on locked-down / air-gapped **Windows** agents (its `.sh` scripts
execute through Git Bash — install **Git for Windows** on the agent), but a corporate
Windows agent has a few hard constraints an operator must plan for. Each item below is
a real limitation of the platform, not of oxide-sloc.

### MAX_PATH (260 characters)

Windows caps most file paths at **260 characters** unless long-path support is enabled
via the HKLM registry key `LongPathsEnabled` — an **admin-only** setting the operator
may not have. Rust's `target/` tree nests deeply (`target\release\build\<crate>-<hash>\out\...`),
so a long agent remote-root plus a long job name can push build paths past the limit and
the compile fails with cryptic "path too long" / "No such file" errors.

**Recommendation:** give the agent a **short remote root** (e.g. `C:\J`) and keep the
**job name short** (e.g. `sloc`). This keeps `<root>\<job>\workspace\target\...` well
under 260 characters without needing the HKLM long-path setting.

### Seed job under Job DSL script-security

`ci/jenkins/seed-job.groovy` reads `JOB_NAME` / `REPO_URL` / `REPO_BRANCH` via
`System.getenv(...)` as a fallback. With **Job DSL script-security ON** (the default),
`System.getenv` is a sandboxed call and is **rejected**. Two supported ways to run the seed:

- **Script Console (admin, unsandboxed):** paste `seed-job.groovy` into
  **Manage Jenkins → Script Console** and run it once. The Script Console is not
  sandboxed, so `System.getenv` works.
- **Job DSL seed job with String parameters:** add `JOB_NAME`, `REPO_URL`, and
  `REPO_BRANCH` as **String parameters** on the seed job. When those bindings are
  present the script uses them directly (`binding.hasVariable(...)`) and never reaches
  the sandboxed `System.getenv` fallback — so it runs cleanly under script-security.

`REPO_URL` is required either way (no hardcoded internet default). Air-gapped? Use a
local mirror, e.g. `file:///srv/git/oxide-sloc.git`.

### Test runner / coverage on Windows air-gap

The prebuilt-binary installers for **cargo-nextest** (`get.nexte.st`) and
**cargo-llvm-cov** (GitHub Releases) that `install-rust-cache.sh` uses are **Linux-only**.
On a Windows air-gapped agent those tools cannot self-install, so the supported defaults are:

- `TEST_RUNNER = cargo-test` (plain `cargo test`), and
- **coverage off** (`RUN_COVERAGE` unchecked).

If you set `TEST_RUNNER = cargo-nextest` or enable coverage on Windows **without**
pre-placing the Windows tool binaries in `%CARGO_HOME%\bin` (i.e. `<cacheroot>\.rust-cache\cargo\bin`),
those stages go **UNSTABLE (yellow)**, not green — the build does not silently pass with a
missing report. To use them, drop the Windows builds of `cargo-nextest.exe` /
`cargo-llvm-cov.exe` into that bin directory ahead of time.

### New operator knobs (environment variables)

| Variable | Effect |
|----------|--------|
| `SLOC_CACHE_DIR` | Overrides the Rust cache root. On a Windows service account whose `HOME` / `USERPROFILE` profile is read-only or ACL-locked, set this to a writable directory. Resolution order is `SLOC_CACHE_DIR` → `HOME` → `USERPROFILE` → `WORKSPACE` (the last is always agent-writable). The cache lands at `<root>\.rust-cache\`. |
| `SLOC_ALLOW_ONLINE_RUSTUP` | Set to `1` to permit `setup-toolchain.sh` to download rustup from the internet as a last resort. **Unset/`0` (default) fails fast** on an air-gapped agent instead of hanging on an unreachable `https://sh.rustup.rs`, with a message pointing at the committed `toolchain/` bundle, the persistent cache, or pinning to a seeded Linux agent via `AGENT_LABEL`. |
| `SLOC_BASH` | Absolute path to a `bash.exe` if Git Bash is not auto-discovered. |
| `SLOC_PY` | Overrides the Python interpreter. Resolution order is `SLOC_PY` → the **bundled portable Python** (below) → system `python3`/`python`/`py`. |

### Bundled portable Python (no system Python required)

The pipeline runs a few `.py` helpers (dashboard, trend CSV, build summary, Confluence
notify). Corporate / air-gapped agents frequently have **no** system Python, or one too
old, so a portable **CPython 3.14** ships in the repo under `python/`
(`cpython-3.14-windows-x64.tar.gz` and `cpython-3.14-linux-x86_64.tar.gz`, checksum-verified
against `python/checksums.sha256`). On first use `ci/jenkins/setup-python.sh` extracts it into
`.tools/python/` (offline) and `pyBin()` prefers it — so **no system Python is needed at all**.
Resolution order: `SLOC_PY` override → bundled portable Python → system `python3`/`python`/`py`.

Every `.py` call in the pipeline is best-effort (guarded / `|| true`), so even if none
resolve the build still succeeds — only the optional dashboard/trend extras are skipped.
Regenerate or bump the bundle with `bash ci/jenkins/bundle-python.sh` (see `python/README.md`).

On Windows, `setup-toolchain.sh` consumes the committed **Windows toolchain bundle**
(`toolchain/rust-toolchain-windows-x64.tar.gz.*`, checksum-verified against
`toolchain/checksums.sha256`) for a fully offline first build, so no network toolchain
download is needed. Ensure the `toolchain/` directory is present in the checkout (it ships
in the repo).
