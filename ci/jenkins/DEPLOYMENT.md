# oxide-sloc Jenkins — deployment & plugin resilience

This is the handoff for whoever manages the Jenkins **controller + agent** (the
"infra side"). It covers what the pipeline needs to run, what it does *without*
optional plugins or admin rights, and the exact agent-image steps to light up the
Test Result and Coverage views.

## Design principle: degrade, never fail

The pipeline is built to run on a **plain Jenkins with no admin rights and none of
the enrichment plugins**. Every plugin-specific step is guarded (`try/catch
(Throwable)`), so a missing plugin only removes *its* view — it never fails the
build. When a plugin is absent, the underlying data is still produced and
**archived under Build Artifacts**, so nothing is lost but the sidebar link.

## Plugin tiers

### Required (pipeline can't run without these — all standard Jenkins plugins)

| Plugin | Used for |
|---|---|
| `workflow-aggregator` (Pipeline) | the declarative pipeline itself |
| `git` | cloning TOOL_REPO_URL / SCAN_REPO_URL |
| `credentials-binding` | `withCredentials` (API token, SCM creds) |
| `pipeline-utility-steps` | `readJSON` in the build summary |
| `ws-cleanup` | `cleanWs()` in post-cleanup |
| `timestamper` | `timestamps()` build option |

These come with Jenkins' default "suggested plugins" and need no special action.
(`ansiColor`/AnsiColor was **removed** from the pipeline so it is no longer
required — you can install it and re-add `ansiColor('xterm')` if you want coloured
logs.)

### Optional — enrichment (guarded; absence only drops the view)

| Plugin | View you get | Without it |
|---|---|---|
| `htmlpublisher` | "OxideSLOC_CI_Report" + "OxideSLOC_HTML_Report" sidebar links | reports still archived under Build Artifacts (open/download from there) |
| `plot` | "SLOC Trends" charts on the job page | trend CSVs still archived |
| `warnings-ng` | "Warnings" trend (clippy) | clippy still gates the build; `clippy.json` archived |
| `junit` | "Test Result" trend | `junit.xml` still archived |
| `coverage` | "Coverage" trend + "Coverage Source" | lcov/cobertura still archived |
| `badge` | run-row badge + summary panel | build description still carries the metrics |
| `bitbucket-build-status-notifier` | commit status pushed to Bitbucket | the plugin-independent `notify-bitbucket.sh` path still works |

Full list + offline bundle: `ci/jenkins/plugins.txt` and `ci/resources/…` — see
`ci/jenkins/README.md`. The pipeline auto-detects which of these are installable
(`detect-capabilities.py`) and shows a banner in the native dashboard when it had
to degrade.

## Running with only the required plugins — exactly what you get

With **only the required plugins** (i.e. no enrichment plugins installed):

- ✅ Full SLOC/test/coverage/complexity/COCOMO analysis (it's the oxide-sloc
  binary, not a plugin).
- ✅ The **native self-contained dashboard** (`dashboard_<slug>.html`) with the
  interactive charts — archived under Build Artifacts; download and open it.
- ✅ HTML / PDF / CSV / XLSX / JSON reports — archived under Build Artifacts.
- ✅ Build description with the headline metrics.
- ⚠️ No sidebar report links, no trend charts, no Test/Coverage/Warnings views
  (those need the enrichment plugins, which need admin to install). A calm banner
  in the dashboard explains this — it is **not** an error.

So the report is always reachable; the plugins only add controller-side views.

## Infra agent — next steps to enable Test Result (B) and Coverage (C)

These two views need **tools on the agent**, not just plugins. Add them to the
agent image (`/root/ci-cd-example/jenkins-instance/Dockerfile` in the deployment
repo), rebuild, and restart the agent.

```dockerfile
# --- oxide-sloc test + coverage tooling (Rust already present on the agent) ---
# cargo-nextest → JUnit XML → Jenkins "Test Result" view. TEST_RUNNER defaults to
# cargo-test (green on a bare agent); bake nextest in so the "Test Result" view is
# available once an operator selects TEST_RUNNER=cargo-nextest. --locked is
# mandatory (nextest refuses to install without it).
RUN cargo install cargo-nextest --locked

# cargo-llvm-cov + llvm-tools → LCOV → Jenkins "Coverage" view (+ LCOV to SonarQube)
RUN rustup component add llvm-tools-preview \
 && cargo install cargo-llvm-cov --locked
```

The repo ships `ci/jenkins/install-rust-cache.sh`, which installs **both** tools
(from prebuilt binaries, with a `cargo install` fallback) into the agent's
persistent `~/.rust-cache/cargo/bin`. **The agent's provisioning MUST run that
script on (re)creation** — either bake it into the image build, or run it as a
one-shot on first boot. If it doesn't, a container rebuild silently loses the
tools: the pipeline then falls back to `cargo test` (no Test Result view) and
skips Coverage until the next provision. The pipeline's offline auto-install is a
safety net, **not** a guarantee.

Offline/air-gapped agents: both tools are also vendored in `ci/tools/Cargo.toml`.
The pipeline's fallback installs nextest offline by **path** — it copies
`vendor/cargo-nextest` out and runs `cargo install --offline --path …` so the dep
tree resolves against the workspace `vendored-sources`. A plain
`cargo install --offline cargo-nextest` does **not** work: `cargo install` ignores
the workspace `.cargo/config.toml` source replacement and fails with
"could not find 'cargo-nextest' in registry 'crates-io'".

After the rebuild:

- **Test Result** appears once a build sets `TEST_RUNNER=cargo-nextest`
  (`PUBLISH_TEST_RESULTS` defaults to true; note the test stage only runs when
  `RUN_QUALITY_GATES` is checked, which forces a source build). The default
  `TEST_RUNNER` is `cargo-test` so a bare agent stays green; with nextest baked into
  the image as above, selecting `cargo-nextest` yields the Test Result view with no fallback.
- **Coverage** appears when a build runs with `RUN_COVERAGE=true` (left
  opt-in: it implies `BUILD_MODE=source`, recompiles instrumented, ~4-5 min/build, and runs
  oxide-sloc's *own* llvm-cov — meaningful for self-CI, not external-repo scans). The view is served
  at **`/job/<job>/<n>/coverage/`**. SonarQube imports coverage from **LCOV**, not
  the Cobertura XML, so the Cobertura duplicate-element quirk doesn't affect it.

## Credentials the infra side owns

Create these under **Manage Jenkins → Credentials** (IDs must match exactly):

| Credential ID | Type | Enables |
|---|---|---|
| `jenkins-api-token` | Secret text (a valid **admin** API token) | plugin-capability detection → dashboard shows full mode instead of the degraded banner; also the CSP relaxation for the interactive dashboard charts |
| `<your-scm-cred>` | Username+token / SSH key | cloning **private** target repos (pass its ID as the `SCAN_CREDENTIALS_ID` build param, or map several hosts via `SCAN_GIT_CREDENTIALS`) — see `ci/jenkins/INTEGRATION.md` |
| `bitbucket-build-token` | Secret text | pushing build status to Bitbucket (optional) |
| `confluence-api-token` | Secret text | Confluence page upsert (optional) |

> The token in `jenkins-api-token` must be **non-empty and hold Administer** for
> the dashboard to report full capabilities. An empty/underprivileged token is
> the usual cause of the "native mode — enhanced plugins not enabled" banner even
> when all plugins are installed.

## Verify

```bash
# On the controller (adjust host):
J=http://localhost:8080; JOB=oxide-sloc
for p in plot/ warnings-ng/ 5/testReport/ 5/coverage/ ; do
  printf '%-16s %s\n' "$p" "$(curl -s -o /dev/null -w '%{http_code}' "$J/job/$JOB/$p")"
done
# 200 = view present; 404 = that plugin's view not produced yet (see tiers above).
```

For the capability detector specifically:

```bash
JENKINS_BASE_URL=$J JENKINS_USER=admin JENKINS_AUTH_TOKEN=<admin-token> \
  python3 ci/jenkins/detect-capabilities.py /tmp && cat /tmp/capabilities.json
# Expect: "mode":"full", "system_admin":true, "degraded":false
```
