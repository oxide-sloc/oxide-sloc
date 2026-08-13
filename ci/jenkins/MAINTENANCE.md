# oxide-sloc Jenkins — storage, security & maintenance

The CLI-driven pipeline writes reports and build records to the Jenkins
controller, so left unbounded it would fill the disk. This documents the
mechanisms that keep a long-running instance healthy, and how to tune them.

## Storage controls (all active by default)

| Mechanism | Where | Effect |
|---|---|---|
| Build retention | `Jenkinsfile` `buildDiscarder` | Keeps **20 builds / 30 days** of build records + logs; older are deleted. |
| Artifact retention | same `logRotator` | Heavier archived artifacts pruned harder: **last 5 builds / 14 days**. |
| Published report retention | `pipeline-helpers.groovy` `publishHTML keepAll:true` | Keeps a copy of the CI / HTML / coverage report **on each build's page** (so per-build views have their report), with the job page always linking the latest. Old per-build copies are pruned with the build record by `buildDiscarder` (20 builds / 30 days). |
| Archive excludes | `runArchivePublish` `archiveArtifacts excludes:` | The curated `ci-report/` + `html-report/` bundles are published but **not** re-archived (no duplicate copies). |
| Trend history cap | `generate-trend-csv.py` | The persistent per-job CSV is capped at **50 rows** (oldest dropped). |
| Workspace wipe | `seed-job.groovy` `wipeWorkspace()` + `post { cleanup } cleanWs()` | Each build starts clean and wipes its workspace at the end. |
| Disk preflight | `check-disk-space.sh` (first step of `runSetup`) | **Fails fast** below `SLOC_DISK_MIN_GB` (3 GiB), warns below `SLOC_DISK_WARN_GB` (8 GiB). |
| Rust-cache cap | same script | Prunes the persistent `~/.rust-cache` download caches when it exceeds `SLOC_CACHE_CAP_GB` (20 GiB). Installed tools + vendored sources are kept. |

### Tuning

- Retention: edit the `buildDiscarder(...)` values in the `Jenkinsfile`.
- Disk thresholds / cache cap: set `SLOC_DISK_MIN_GB`, `SLOC_DISK_WARN_GB`,
  `SLOC_CACHE_CAP_GB` as node/global env or build env vars.
- The persistent Rust cache (`CARGO_HOME`/`RUSTUP_HOME` under `~/.rust-cache`)
  survives `cleanWs()` on purpose — it's what makes rebuilds fast. The cache-cap
  prune is the guard that stops it growing forever.

## Security

- **Least-privilege parameters** — `SCAN_PATH`, `OUTPUT_SUBDIR`, globs, languages,
  and `SCAN_REF` are regex-validated in `runAnalyze` before use.
- **Credentials, never inline** — artifact-repo, Bitbucket, Confluence, SMTP, and
  webhook secrets are pulled from the Jenkins credential store; every integration
  no-ops cleanly when its credential is absent.
- **Plugin-install gate** — `detect-capabilities.py` checks the caller's Jenkins
  authorization before attempting any controller change; unprivileged builds
  degrade to the native dashboard (see the capability banner).
- **Artifact-viewer CSP** is relaxed only as far as needed to render reports.
- **HTTPS** — serve Jenkins over TLS (`ci/jenkins/https/`) to remove the
  browser's "insecure download blocked" warning and protect credentials in
  transit.

## Build parameters — quick scan vs. advanced

The `Jenkinsfile` `parameters {}` block is ordered **required-first**: a quick
scan needs only `SCAN_REPO_URL` (blank scans oxide-sloc itself) and optionally
`SCAN_PATH`. Everything else is optional and pre-set to oxide-sloc's application
defaults, so leaving it all untouched produces a standard default scan on the fast
prebuilt path (`BUILD_MODE=prebuilt`).

### True collapse/hide (optional, needs a plugin)

Jenkins' built-in "Build with Parameters" form cannot *hide* the advanced fields
without the **Active Choices** (`uno-choice`) plugin, and wiring it in requires
moving parameter definitions out of the declarative `parameters {}` block — which
would break the job on any controller that lacks the plugin (against this repo's
offline-first rule). It is therefore **not enabled by default**. If your controller
has Active Choices installed and you want the advanced fields collapsed behind a
toggle, that can be added as an opt-in job variant — ask a maintainer to enable it.
