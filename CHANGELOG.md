# Changelog

All notable changes to oxide-sloc are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning follows [Semantic Versioning](https://semver.org/).

---

## [Unreleased]

---

## [1.6.17] — 2026-08-22

### Added

- **Code Ownership**: per-author blame attribution across HTML, PDF, and CSV reports, a
  dedicated Code Ownership page in the web UI, and a scan-form toggle (attribution on by
  default). Ownership is integrated with Git Hotspots for per-author file drill-down, with
  after-the-fact identity merging (`.mailmap` export) to combine authors.
- **Contributor leaderboard**: a gamified contributor leaderboard in reports, with
  contributors linked to their profiles and warm-theme info callouts.
- **Native Jira + Bitbucket integrations**: first-class Jira and Bitbucket publishing with
  GitHub Actions parity, plus a CMake integration module.
- **Git-push run export**: export a completed run directly via git push, with LAN-hosting
  documentation for sharing a local instance on a trusted network.
- **"Report a Bug" page**: a dedicated in-app page for filing issues.

### Changed

- **New brand mark**: replaced the logo with the new "O + quill" mark and refreshed the
  shared static asset pipeline (consolidated run export and static assets).
- **Toolchain**: bumped the bundled Rust toolchain to 1.98 and refreshed dependencies.
- **Refactors**: reduced cognitive complexity across five hotspots and added coverage tests;
  slimmed git history of superseded vendor/toolchain/dist archive generations.

### Fixed

- **Security**: upgraded `h2` 0.4.15 → 0.4.16 (RUSTSEC-2026-0258).
- Cleared CodeQL code-quality findings (empty-except, unused-import, useless-assignment) and
  fixed a hard-coded nonce in the ownership tests.

---

## [1.6.16] — 2026-08-15

### Added

- **Downstream build triggers**: a portable `ci/downstream-trigger/trigger-oxide-sloc.sh`
  (POSIX `/bin/sh`, curl + openssl/python HMAC) can be dropped into the final step of any
  upstream build — Jenkins freestyle, GitLab shell runner, Bitbucket step, or GitHub Actions —
  to fire an oxide-sloc scan of the built repo when the build finishes, either by POSTing a
  signed build-complete event to a running `serve` instance or by dispatching a downstream
  pipeline.

### Changed

- **Jenkins pipeline hardening**: the Jenkins admin credential is injected at runtime instead of
  baked into the image; per-repo delta/trend reporting, cleaner Top Files paths, and automatic
  report title/branch detection; artifact bundles and the bundle-artifacts plan builder tidied;
  Docker builder stages enable bash `pipefail`.
- **CI refactors**: reduced cognitive complexity in the dashboard generator and CI helper
  scripts; various Jenkins reliability fixes (bare-Windows-agent bash staging, dist-fallback
  diagnostics, per-build HTML report publishing).

### Fixed

- **CI lint gate**: the two new `ci/downstream-trigger/*.sh` scripts now enable `set -o pipefail`
  (guarded so POSIX `/bin/sh`/dash does not abort), clearing the `lint-pipeline-shell` status-
  masking gate that was failing the Quality gates job.

---

## [1.6.15] — 2026-08-12

### Added

- **MCP protocol + API surface modernization**: the `sloc-mcp` server advertises the current
  MCP protocol revision with version negotiation, and the web server exposes a richer health/API
  surface (`GET /api/health` structured health JSON, `GET /api/version` build provenance).
- **Jenkins fast-by-default pipeline**: the pipeline now defaults to the prebuilt `dist` binary
  (`BUILD_MODE=prebuilt`, ~2–3 min), bundles a portable Python runtime, supports multi-instance
  git credentials, and triggers on release/tag webhooks.

### Fixed

- **Jenkins vendored tooling**: vendored cargo tools (including `llvm-cov`) install outside the
  workspace so they no longer pollute or conflict with the checked-out tree; the webhook trigger
  `<spec>` is emitted correctly and coverage falls back honestly when instrumentation is
  unavailable.
- **Jenkins console noise**: quieted clippy compile-progress output on the Jenkins console.

### Changed

- **Dependencies**: bumped `futures` to 0.3.34 and `rustls-webpki` to 0.103.14, re-vendored the
  offline crate archive, and refreshed the bundled Rust toolchain (1.97).

### Documentation

- Stripped casual "no admin" phrasing from the docs, added a UCC comparison, documented the
  health endpoints, and noted the MCP protocol revision.

---

## [1.6.14] — 2026-08-11

### Fixed

- **Windows air-gapped install (`install.ps1`)**: the native PowerShell installer now tolerates
  rustup-proxy symlink errors while extracting the bundled toolchain, so a locked-down Windows
  host without symlink privilege no longer fails toolchain setup.
- **MSVC target linker check**: `install.ps1` only requires a MinGW linker for GNU targets; MSVC
  targets no longer error out looking for a linker they do not use.

### CI / Build

- **Windows CI without a Git install**: added a no-install PortableGit path plus the native,
  bash-free PowerShell installer; corrected stale `vendor.tar.xz` references in the docs (the
  vendor archive is gzip `vendor.tar.gz.*`).
- **Jenkins air-gapped agents**: pinned `RUSTUP_TOOLCHAIN` to the exact bundled toolchain name so
  sealed-network agents never try to sync the `1.97` channel manifest online.

---

## [1.6.13] — 2026-08-10

### Fixed

- **Jenkins pipeline compilation**: reworded a comment in `pipeline-helpers.groovy` whose `\usr`
  path fragment was parsed as an invalid Unicode escape at lex time, which broke CPS compilation
  and failed every build at "Load helpers".
- **Jenkins target checkout of tags**: `TARGET_REF` set to a tag (e.g. `v1.1`) now resolves — the
  target checkout fetches both heads and tags and offers both resolutions instead of branches only.
- **Offline `--rebuild`**: the bundled-toolchain build pins `RUSTUP_TOOLCHAIN` to the exact
  installed toolchain, so a sealed-network rebuild no longer tries to fetch the `1.97` channel
  manifest online.
- **Linux dist extraction (root / rootless container / uid-remapped CI)**: `install.sh` extracts
  with `--no-same-owner` and trusts the on-disk binary over tar's exit code, and prints an
  actionable "extraction-tooling gap" message instead of the misleading "no pre-built binary found".
- **`install.sh --build`**: compiles from source even when a `dist/` archive is present (it
  previously extracted dist and ignored the flag).
- **Stale `dist/` binaries lacked per-host git credentials**: the committed pre-built binaries are
  rebuilt so the multi-instance credential registry (`SLOC_GIT_CRED_*`) is present in dist-installed
  binaries, not only in from-source builds.
- **Per-host credential ports**: `SLOC_GIT_CRED_<HOST>_<PORT>` is now honored (tried before the
  bare-host key), matching the documented `git.corp:7990 → GIT_CORP_7990` convention.
- **Per-user clone cache**: the CLI git clone cache defaults to a per-user, `0700` directory
  (honoring `XDG_CACHE_HOME`) and refuses to reuse a cache owned by another user — fixing
  cross-user collisions and a local cache-poisoning vector. `SLOC_GIT_CLONES_DIR` still overrides.
- **`run.sh --host` messaging**: LAN server mode fails fast with the real choice (set `SLOC_API_KEY`
  or opt into `SLOC_ALLOW_UNAUTHENTICATED=1`) instead of printing "unauthenticated" and then
  refusing to start.
- **In-app API docs**: corrected the `/api/git/refs`, `/api/git/scan-ref`, and
  `/api/git/compare-refs` query-parameter names (`repo`, `ref_name`, `baseline_ref`, `current_ref`).

### Added

- **`SLOC_GIT_SSH_ACCEPT_NEW`**: opt-in trust-on-first-use for SSH clones (adds
  `StrictHostKeyChecking=accept-new`); the default remains strict host-key checking.

### Documentation

- Documented SSH host-key seeding for fresh agents, port-qualified credential keys, and the Linux
  arm64 build-from-source path (no pre-built arm64 binary is committed).

---

## [1.6.12] — 2026-08-07

### Fixed

- **Air-gapped Jenkins CI**: de-hardcoded the tooling `REPO_URL` so air-gapped controllers
  resolve it from the build parameter, environment, or `checkout scm` instead of a baked-in
  GitHub URL; repaired additional repository defects surfaced during air-gapped Jenkins
  verification.

### Internal

- **Make target**: added a `jenkins-tests` target that runs the Jenkins CI guard suites
  (including the air-gap `REPO_URL` guard) locally.
- **Release**: version bump to 1.6.12 across the workspace, installers, docs, and OpenAPI spec.

---

## [1.6.11] — 2026-08-06

### Changed

- **Rust 2024 edition**: the workspace migrated from the 2021 to the 2024 edition; all crates
  now build under edition 2024 (`env::set_var`/`remove_var` are `unsafe` and wrapped accordingly).

### Fixed

- **CI/Jenkins pipeline**: green-by-default seed build, a working `nextest` install, an honest
  CSP fallback, a `globoff` plugin-probe `curl`, and removal of a stale pipeline clause.
- **2024 clippy**: collapsed a Linux-only nested `if` into a let-chain to satisfy the 2024
  lint set.

### Internal

- **Dependency refresh**: bumped dependencies to their latest semver-compatible patches;
  upgraded `printpdf` 0.11 → 0.12 (dropping resolved advisory ignores), the vendored
  `cargo-llvm-cov` 0.6 → 0.8 for offline CI coverage, and refreshed pinned GitHub Actions and
  base-image digests.
- **Test coverage**: added coverage-gap tests across all crates, plus CLI end-to-end coverage
  for `sloc-cli` `main.rs` (diff artifacts and exit-code gates).
- **OpenAPI version sync**: the served OpenAPI `info.version` now tracks the release version.

---

## [1.6.1] — 2026-07-29

### Added

- **"Files total" / "Lines total" in scan comparisons** (`crates/sloc-core`,
  `crates/sloc-web`): the diff/compare view now reports absolute file and line totals
  alongside the per-language deltas, so a comparison shows both what changed and the
  full size of each scan.

### Fixed

- **CI shell-pipe status masking** (`ci/jenkins`): guard against status-masking shell
  pipes (including `| tee`) in the Jenkins Groovy pipeline so a failing stage can no
  longer be hidden by a downstream pipe; genhtml failures now surface in `runCoverage`
  and pipe usage is reported as a finding rather than a self-test failure.
- **GitLab air-gap pipeline defects** (`.gitlab-ci.yml`): four pipeline defects found
  during air-gap verification are repaired; the pinned toolchain install is now fully
  offline.

### Documentation

- **Installer note**: Chromium is documented as a soft dependency — required only for
  PDF export, not for core analysis.

### Internal

- **SonarQube debt cleanup**: cleared latent maintainability debt, reduced cognitive
  complexity (S3776), and widened the coverage buffer.

---

## [1.6.0] — 2026-07-24

### Added

- **Corporate-network serve ergonomics + secure server profile** (`crates/sloc-web`,
  `scripts/serve-server.sh`): firewalld zone detection (targets the zone of the primary
  route interface, not a blind default-zone guess), advertisement of only routable LAN IPs
  with a recommended URL (virtual/docker bridges are listed as "ignore"), a
  `SLOC_ADVERTISE_HOST` override to pin the advertised hostname, and a fail-closed gate that
  refuses to start a plain unauthenticated server on a network bind unless explicitly
  acknowledged.
- **Embeddable UI via `SLOC_FRAME_ANCESTORS`** (`crates/sloc-web`): opt-in allowlist of
  origins permitted to embed the UI in an iframe. Default remains deny (`X-Frame-Options:
  DENY`, CSP `frame-ancestors 'none'`); when set, the page drops `X-Frame-Options` and emits
  a scoped `frame-ancestors` directive.
- **systemd install with a generated key** (`scripts/internal/install-systemd.sh`): a secure
  happy-path installer that provisions the service, generates an API key into a `0640`
  `EnvironmentFile`, and enables the unit so it survives reboot.

### Fixed

- **Git Browser fetch is fast, resilient, and corporate-network ready** (`crates/sloc-git`,
  `crates/sloc-web`): clone now uses a blobless, no-checkout partial clone
  (`--filter=blob:none`) so listing branches/tags/commits is near-instant even on large
  repos and slow links (blobs are fetched lazily at scan time); a `SLOC_GIT_TIMEOUT`
  wall-clock ceiling (default 300s) plus a low-speed abort ensures a stalled proxy/VPN fails
  fast instead of hanging; on Windows the OS certificate store (schannel) is used so
  TLS-inspecting corporate proxy/VPN certificates are trusted with zero configuration.
- **Git Browser scan/compare on non-default branches** (`crates/sloc-git`): a bare branch
  name that exists only as a remote-tracking ref now resolves correctly instead of failing
  with "invalid reference", so scanning or comparing any branch works — not just the
  default.
- **Actionable Git Browser fetch errors** (`crates/sloc-web`): the fetch API now returns the
  real, classifiable git error (timeout / certificate / authentication / DNS) instead of a
  single opaque message, so the UI's remediation hints fire and operators can distinguish
  failure modes.
- **`serve-server.sh` honors `SLOC_API_KEYS`** (`scripts`): the launcher now treats
  `SLOC_API_KEYS` as configured authentication, so the secure one-shot profile starts
  without extra flags.
- **Release pipeline SBOM + dist artifacts** (`ci`): the CycloneDX SBOM step is pinned to
  `cargo-cyclonedx 0.5.9` and tolerates the tool's output filename, and dist artifacts are
  built from the release tag with a version assertion — repairing the release workflow.

---

## [1.5.78] — 2026-07-22

### Added

- **Atlassian CI integration** (`crates/sloc-web`, `ci/jenkins`): end-to-end wiring for
  posting metrics and attaching the full HTML/PDF report to Confluence, plus Bitbucket
  build-status reporting. The integration is plugin-independent and works on the first run;
  CI configuration is surfaced in the UI.
- **Opt-in mutual TLS + read-only API keys** (`crates/sloc-web`): client-certificate
  authentication (mTLS), a distinct read-only API-key tier, a configurable minimum TLS
  version pin, and in-memory zeroization of key material. All opt-in; defaults unchanged.
- **Opt-in session idle timeout + pre-access consent banner** (`crates/sloc-web`):
  configurable idle-session expiry and an optional consent banner shown before access.
- **Opt-in server hardening controls** (`crates/sloc-web`): additional server-mode
  hardening toggles alongside refinements to the Test Metrics page.

### Fixed

- **Leaner default Linux builds** (`crates/sloc-cli`): the default Linux build no longer
  pulls in `wayland`/`rfd`, avoiding unnecessary system dependencies (#87).
- **Isolated test-router on-disk stores** (`crates/sloc-web`): each test router now uses
  its own on-disk store so parallel tests no longer interfere.
- **Real SBOM release artifact** (`ci`): the release pipeline now produces a genuine SBOM
  artifact instead of a placeholder.

### Changed

- **Dependency refresh**: bumped `uuid` to 1.24.0 and refreshed the vendor archive; pinned
  several GitHub Actions (`cargo-deny-action`, `setup-buildx-action`, `action-gh-release`,
  `rust-toolchain`) to newer releases.

---

## [1.5.77] — 2026-07-18

### Added

- **Test Metrics export buttons** (`crates/sloc-web`): the `/test-metrics` page now
  offers Export Excel (`.xlsx`), Export PNG, and Export PDF actions that bundle the
  test-definition counts alongside the LCOV coverage summary.
- **Coverage gauge tooltips** (`crates/sloc-web`): the line/function/branch coverage
  gauges on `/test-metrics` gained hover tooltips explaining how each percentage is
  derived from the LCOV report.
- **`prune` CLI command + audit-log rotation** (`crates/sloc-cli`, `crates/sloc-core`):
  operator-driven disk reclamation (`--older-than`, `--keep-last`, `--logs`, dry-run by
  default) plus size-based self-rotation of the `SLOC_AUDIT_LOG` JSONL sink.
- **Fail-closed server auth + SIEM audit log** (`crates/sloc-web`): `serve --server`
  refuses to start without an API key unless `SLOC_ALLOW_UNAUTHENTICATED=1`; security
  events stream to `SLOC_AUDIT_LOG` as JSON lines.
- **Accurate C/C++ semantic metrics** (`crates/sloc-languages`): header detection and
  member/local/global variable + macro-constant categorization surfaced in the CLI and
  coverage display.
- **Watched-folder scan overlay + orphan cleanup** (`crates/sloc-web`): watched-folder
  actions show a scan overlay, and un-watching a folder now drops its linked reports.

### Fixed

- **Opaque Appearance settings modal** (`crates/sloc-web`): the colour-scheme picker
  modal is no longer see-through.
- **Output folder window surfacing** (`crates/sloc-web`): reliably raise the output
  folder window to the foreground on Windows.
- **Coverage view fed from LCOV only** (`ci/jenkins`): the Cobertura parser rejected the
  report; the Coverage view now reads LCOV directly.
- **Cyclomatic complexity counts real code only** (`crates/sloc-languages`): complexity
  is measured against executable lines with a density-based gate.

### Changed

- **Coverage UI + PDF polish** (`crates/sloc-report`): coverage UI refinements and a
  reworked PDF metric strip.
- **Cognitive-complexity refactors** across `sloc-web`, `sloc-core`, `sloc-git`,
  `sloc-report`, and `sloc-languages` (no behaviour change).
- **Toolchain + dependency refresh**: bundled Rust toolchain bumped to 1.97 and
  `printpdf` to 0.11; several GitHub Actions pinned to newer releases.

---

## [1.5.76] — 2026-07-12

### Added

- **Multi-repository directory warning** (`crates/sloc-git`, `crates/sloc-web`):
  when a selected directory contains more than one git repository, the analysis
  now surfaces a warning so activity/hotspot data is not silently attributed to
  the wrong repo.

### Fixed

- **Advisory: crossbeam-epoch** (deps): bumped `crossbeam-epoch`
  `0.9.18` → `0.9.20` to clear `RUSTSEC-2026-0204`.

### Changed

- **CI dependency + workflow refresh**: bumped `docker/build-push-action`
  `7.2.0` → `7.3.0`, and disabled ZAP DAST issue writing to fix a `403` caused by
  the least-privilege workflow token.

---

## [1.5.75] — 2026-07-05

### Security Hardening

- **Tighter Content-Security-Policy** (`crates/sloc-web`): added the `base-uri`
  and `form-action` directives to lock down base-tag and form-submission targets,
  and set a `Cross-Origin-Embedder-Policy` (COEP) response header on served pages.
- **Hardened git-clone target checks** (`crates/sloc-git`): stricter validation of
  clone targets, and the release workflow now validates its tag inputs before use.
- **Signed Docker provenance** (CI): the container image build now emits
  GitHub-signed build provenance and verifies the image signature.

### Changed

- **Parameterized Jenkins credentials** (`ci/jenkins`): the Jenkins controller
  admin credentials are supplied via build args instead of being baked in.
- **Faster archive packing** (`scripts`): committed archives skip git
  delta-compression for quicker packing, and a reusable git-history purge tool was
  added under `scripts/`. Repository history was pruned of superseded
  vendor/toolchain/dist blobs.
- **Dependency + vendor refresh** (deps): updated dependencies and the vendor
  archive; the security audit ignores the quick-xml `RUSTSEC-2026-0194/0195`
  advisories.

### Fixed

- **Code quality and coverage** (workspace): broad clippy/quality cleanups, CI
  hardening, and substantially expanded test coverage across the web handlers,
  Confluence/git-browser API branches, server-scan validation, and core decode
  policies.

---

## [1.5.74] — 2026-06-28

### Added

- **Richer Scan Delta PDF** (`crates/sloc-web`, `crates/sloc-report`): the Scan
  Delta export now carries the full metric set with a dedicated **File Changes**
  section, code-line composition, optional tests/coverage detail, a per-page
  footer, and a repeating header. The file listing reports only the files that
  actually changed rather than capping or dumping every path.
- **Interactive Git Hotspots table** (`crates/sloc-report`): the Hotspots table
  in HTML reports is now sortable, paginated, resizable, and has header
  tooltips.

### Changed

- **Windowed Scan Delta file matrix** (`crates/sloc-web`): the Compare page's
  file-delta table now lifts its rows into a data model and renders only the
  visible page (~25 rows) instead of touching every node on each sort/filter.
  Large diffs (tens of thousands of files) that previously froze the page during
  sort, filter, or column resize now stay responsive. The table uses
  `table-layout:fixed` with a colgroup, and column resize pins pixel widths so a
  dragged column actually grows while the wrapper scrolls.
- **Theme-aware Compare page charts** (`crates/sloc-web`, `crates/sloc-report`):
  the Scan Delta and Multi Compare charts now resolve their palette from the
  shared theme tokens so they render correctly in dark mode.

### Fixed

- **Full commit SHA in tables** (`crates/sloc-web`): history and compare tables
  show the full commit SHA via a custom JS tooltip, and the full SHA is now
  recovered for older runs; the skipped label is simplified and skipped/file
  counts are comma-formatted.
- **PDF rendering robustness** (`crates/sloc-web`, `crates/sloc-report`): the
  headless-Chrome PDF timeout is raised to 90s to survive a loaded host, the
  `/export/pdf` body limit is raised to 64 MB to stop 413s on large reports,
  PDF headers no longer overlap content or drop rows, and the Scan Delta footer
  is pinned to the absolute page bottom. Git invocations are non-interactive and
  drop `origin/HEAD`.
- **Dependency advisories** (deps): upgraded `printpdf` 0.7 → 0.9, clearing the
  `lopdf` audit advisory, with `osv-scanner.toml` ignoring the remaining
  printpdf-transitive advisories.

---

## [1.5.73] — 2026-06-25

### Added

- **Git Hotspots on by default** (`crates/sloc-core`): the per-file activity
  window now defaults to 90 days (`analysis.activity_window_days = Some(90)`,
  `0` disables). A single `git log` pass attaches per-file commit counts and
  last-change dates, and a Hotspots table/page (ranked by `code_lines ×
  commits`) renders in HTML, web, PDF, and CSV.
- **coverage.py JSON support** (`crates/sloc-core`): Python `coverage.py` JSON
  reports are now parsed alongside the existing coverage formats, with new
  trend-chart controls and UI polish on the test-metrics page.
- **Expanded language-analyzer coverage**: additional corpus fixtures and
  golden tests broaden unit-test/assertion detection across more languages.

### Changed

- **Page transitions & animation polish**: smooth page-fade transitions, tooltip
  animation polish, and the floating code-particles animation across report and
  web pages.

### Fixed

- **PDF page trimming**: the terminal COCOMO / Tests & Coverage PDF page is now
  trimmed to its content height, including the case where a Git Hotspots page
  follows it (`crates/sloc-report`).
- **Lint gates**: cleared clippy pedantic/nursery findings that were blocking the
  Jenkins lint gate, and reduced cognitive complexity in `write_pdf_from_run`
  and the SSRF gate paths without behaviour changes.

---

## [1.5.72] — 2026-06-17

### Added

- **C/C++ variable detection** (`crates/sloc-languages/src/lib.rs`): New
  `variables_prefix_no_paren` field on `SymbolPatterns` enables lexical
  variable-declaration detection in C and C++. Lines that start with a type
  keyword (`int`, `char`, `float`, `const`, `static`, `constexpr`, etc.) and
  either contain no `(` or have a `=` before the first `(` are now classified as
  variable declarations, distinguishing them from function definitions.

### Changed

- **Language Breakdown table** (`crates/sloc-report/src/lib.rs`): Switched from
  `table-layout: fixed` (which squeezed the Language column) to auto layout with
  `min-width: 760px`. All 14 columns now include a `<div class="col-resize-handle">`
  drag handle and are fully resizable via the shared `.table-resizable` resize
  driver, which previously only applied to the per-file table.

- **Submodule Composition chart** (`crates/sloc-report/src/lib.rs`): Replaced the
  custom hand-built SVG bar chart with a proper Chart.js stacked horizontal bar.
  Value labels are rendered inside each segment (hidden when the segment is too
  narrow). Both the inline panel and the Full View modal now use Chart.js, with
  the modal adding a Sort control (Total Lines ↓ / ↑ / Name A→Z).

- **Submodule Breakdown Full View modal** (`crates/sloc-report/src/lib.rs`): The
  expand overlay now includes live Y Axis and Sort `<select>` controls that
  re-render the chart on change without reopening the modal. Hover tooltips show
  all four dimensions (Code / Comments / Blanks / Files) regardless of the active
  Y axis.

- **PDF header** (`crates/sloc-report/src/lib.rs`): Git branch/commit metadata
  line now right-aligns instead of being centre-anchored. Separator changed from
  ` | ` to ` · `. The environment line field label changed from `Mode:` to
  `Source:` and the raw `runtime_mode` string is now mapped through
  `runtime_mode_display()` (e.g. `serve` → `Web UI`, `analyze` → `CLI`).

- **Sort indicators** (`crates/sloc-report/src/lib.rs`): All sort-indicator
  markers across every `data-sort-table` table now reset to ` ↕` when a
  different column is clicked; the active column shows ` ↑` or ` ↓`. Column
  headers get `cursor: pointer` explicitly.

- **Copy / Download config buttons** (`crates/sloc-report/src/lib.rs`): Buttons
  switched from `.header-button` to the shared `.export-btn` / `.code-copy-btn`
  style — inline-flex with gap, hover accent colour, and matching dark-theme
  colours.

- **Style heuristic note banner** (`crates/sloc-report/src/lib.rs`): Converted
  from a single-line badge to a flex row with an icon `<span>` and a text
  `<span>`, with improved padding, border radius, and a subtle border.

- **Summary grid responsive layout** (`crates/sloc-report/src/lib.rs`,
  `crates/sloc-web/src/lib.rs`): Removed the hard-coded `1200px → 4-col`
  breakpoint. A small JS snippet now reads the actual chip count and sets
  `grid-template-columns: repeat(ceil(n/2), 1fr)` above 640 px, so the strip
  always fills one or two rows regardless of how many chips are present.

- **SVG legend x-offset** (`crates/sloc-report/src/lib.rs`,
  `crates/sloc-web/src/lib.rs`): The "Blanks" legend entry x position was
  corrected from `LW+152` to `LW+138` so it no longer overlaps the "Comments"
  entry in the language-overview bar-chart SVG (both report and web pages).

- **CI dist-commit jobs** (`.github/workflows/update-dist.yml`): Both
  `commit-dist-windows` and `commit-dist-linux` now use
  `peter-evans/create-pull-request` (v7.0.8) instead of a direct `git push` to
  `main`. Because `main` requires PRs + the Quality Gates check, the direct push
  was rejected on every release. The new approach pushes to a temporary branch
  and opens a PR that auto-merges once the check passes; no `DIST_PUSH_TOKEN`
  secret is needed.

- **Web page minor CSS** (`crates/sloc-web/src/lib.rs`): `.wb-stats-title` font
  size increased from 9 px to 10 px; `.scope-legend-row` switched to
  `flex-wrap: nowrap` with reduced padding for tighter layout; step/quick-scan
  divider margins normalised to 12 px top and bottom.

---

## [1.5.71] — 2026-06-16

### Fixed

- **`update-dist.yml` dist-commit jobs** (`.github/workflows/update-dist.yml`): Replaced the
  `GITHUB_TOKEN`-based checkout (which cannot bypass branch protection) with a new
  `DIST_PUSH_TOKEN` secret. Both `commit-dist-windows` and `commit-dist-linux` now check for
  the secret at runtime and skip with a warning rather than failing when it is absent, so the
  release pipeline is not blocked when the secret is unset. Added `continue-on-error: true` to
  both jobs for the same reason.

- **`release.yml` winget submission** (`.github/workflows/release.yml`): Changed the winget
  installer regex from `oxide-sloc-windows-x86_64\.exe$` to `oxide-sloc-windows-x64\.zip$`
  and added a preceding step that copies the exe into a zip archive named
  `oxide-sloc-windows-x64.zip`. This aligns with the `NestedInstallerFiles` pattern so
  winget resolves `oxide-sloc.exe` inside the zip correctly.

### Added

- **IEEE 1045-1992 fields in web scan-config** (`crates/sloc-web/src/lib.rs`): `ScanConfig`
  now persists and round-trips `continuation_line_policy`, `blank_in_block_comment_policy`,
  `count_compiler_directives`, `style_analysis_enabled`, `style_col_threshold`,
  `style_score_threshold`, `style_lang_scope`, `coverage_file`, `cocomo_mode`,
  `complexity_alert`, and `exclude_duplicates`. Query-param parsing mirrors these fields so
  pre-filled scan URLs carry all advanced settings.

- **Policy unit tests** (`crates/sloc-languages/src/lib.rs`): Four deterministic tests for
  `blank_in_block_comment_policy` (CountAsComment / CountAsBlank) and
  `continuation_line_policy` (EachPhysicalLine / CollapseToLogical) covering the C lexer.

- **`mcp.json` version sync**: Updated MCP manifest version from `1.5.5` to `1.5.71` to
  stay in lock-step with the Cargo workspace version.

- **`AGENTS.md` crate count**: Corrected workspace crate count comment from 7 to 8
  (`sloc-mcp` is the eighth crate).

---

## [1.5.70] — 2026-06-16

### Fixed

- **`publish-crates` CI job** (`.github/workflows/release.yml`): Rewrote the per-crate
  publish helper to capture exit code and stdout separately before grepping, and added
  `CARGO_TERM_COLOR: never` to the step env so ANSI escape codes cannot interfere with
  error string matching. Previously `CARGO_TERM_COLOR: always` (set globally by the GitHub
  Actions runner) caused cargo's "already exists on crates.io index" message to be wrapped
  in ANSI codes that broke the grep, making the job fail on every run where crates were
  already published.

---

## [1.5.69] — 2026-06-16

### Fixed

- **`publish-crates` CI job** (`.github/workflows/release.yml`): The step that disables the
  vendor source redirect now guards against `.cargo/config.toml` not existing in a fresh CI
  checkout (the file is gitignored — generated by `install.sh`). Previously the bare `mv`
  exited non-zero immediately, failing the entire job and leaving crates.io unpublished.

- **Latest Release badge** (`README.md`): Replaced the `img.shields.io/github/v/release`
  badge with `badgen.net/github/release` to avoid shields.io's "Unable to select next GitHub
  token from pool" rate-limit error that intermittently broke the badge.

- **`update-dist.yml` trigger** (`.github/workflows/update-dist.yml`): Replaced the
  `push: tags` trigger (which raced with the Release workflow) with `workflow_run` so dist
  bundles are only uploaded after the Release workflow completes successfully.

---

## [1.5.68] — 2026-06-16

### Fixed

- **Release workflow badge** (`.github/workflows/release.yml`): Removed the dead
  `compute-slsa-subjects` job that was left behind after the SLSA reusable-workflow generator
  was removed in v1.5.67. The orphaned job was aggregating hashes for a job that no longer
  existed, causing the Release badge to stay red.

### Added

- **Automatic crates.io publishing** (`.github/workflows/release.yml`): New `publish-crates`
  job runs after the GitHub Release is created on every stable tag. Publishes all seven crates
  in dependency order (`sloc-git` → `sloc-config` → `sloc-languages` → `sloc-core` →
  `sloc-report` → `sloc-web` → `oxide-sloc`), temporarily disabling the vendor-source redirect
  in `.cargo/config.toml` so `cargo publish` can resolve workspace path deps. Requires
  `CARGO_REGISTRY_TOKEN` secret configured in repository settings.

- **Extended web server coverage tests** (`crates/sloc-web/tests/extra_coverage.rs`): Additional
  integration tests covering image handler icon variants, `/llms.txt`, `/llms-full.txt`,
  `/api/openapi.yaml`, `/static/chart-report.js`, badge handler edge cases, metrics/history/
  submodule endpoints, project-history, suggest-coverage branches, and open/pick-directory/
  pick-file server-mode paths.

---

## [1.5.67] — 2026-06-15

### Fixed

- **Jenkins Clippy stage exit-code propagation** (`Jenkinsfile`): The `cargo clippy` exit code
  was previously lost because the output was piped through `tee`; the stage always reported
  success even when Clippy found errors. The fix captures `$?` immediately after the clippy
  invocation into `CLIPPY_RC` and calls `exit $CLIPPY_RC` at the end of the step so the
  Jenkins stage correctly fails on any Clippy error. Also aligns whitespace in the
  `ERR_COUNT` grep for consistency.

---

## [1.5.66] — 2026-06-04

### Improved

- **Clippy compliance across all crates** (`sloc-languages`, `sloc-core`, `sloc-report`,
  `sloc-web`, `sloc-cli`, `sloc-config`, `sloc-git`, `sloc-mcp`): Resolved all outstanding
  Clippy warnings across the workspace. Changes include:
  - `#[must_use]` attributes added to all pure functions in `sloc-languages/style/` helpers
    (`IndentStyle::display`, `classify_indent`, `weighted_score`, `score_indent_*`,
    `score_line*`, `count_over`, `top_guide`, `scan_base_metrics`, `BraceStyle::display`,
    `classify_brace`) and in `sloc-core`, `sloc-report`, and `sloc-languages`.
  - `const fn` conversions for deterministic helpers: `score_indent_2/4/tabs`,
    `BraceStyle::display`, `IndentStyle::display`, `ptr_display`, `score_ptr_type`,
    `score_ptr_name`, `default_interval_hours`, `should_style_analyse`,
    `McpResponse::ok`, and `const fn score_indent_*`.
  - Integer-to-float casts switched from `as f32` to `f64::from()` throughout
    `classify_indent`, `classify_brace`, `score_line80/n`, `classify_ptr`, and JS/C++
    style analyzers — eliminates `clippy::cast_precision_loss` at the source.
  - `.map(...).unwrap_or(...)` replaced with `.map_or(...)` across `sloc-cli`,
    `sloc-core`, `sloc-report`, and `sloc-languages`.
  - Let-else (`let Some(x) = … else { return }`) replaces match-on-option in
    `scan_indent` (`sloc-languages/style/common.rs`).
  - Wildcard `use super::common::*` replaced with explicit imports in
    `sloc-languages/style/rust_lang.rs`.
  - `char::is_uppercase` passed as a function pointer instead of a closure in
    `rust_lang::analyze`.
  - `#[allow(clippy::...)]` annotations with inline justification comments added
    where suppression is intentional (`cast_precision_loss`, `suboptimal_flops`,
    `too_many_lines`, `fn_params_excessive_bools`, `similar_names`).

- **`McpConfig::from_env` simplified** (`crates/sloc-mcp/src/config.rs`): Method now
  returns `Self` instead of `Result<Self>` — environment-variable parsing is infallible;
  the `Result` wrapper was misleading. Call sites updated accordingly; the unit test
  `from_env_does_not_panic` updated to match.

- **`McpResponse::parse_error` takes a reference** (`crates/sloc-mcp/src/protocol.rs`):
  Parameter changed from `serde_json::Error` (owned) to `&serde_json::Error` to avoid
  unnecessary ownership transfer. `McpResponse::ok` converted to `const fn`.

- **`u32::try_from` for usize→u32 casts** (`sloc-languages/style/common.rs`,
  `sloc-languages/style/js.rs`): `count_over`, `scan_base_metrics`, and JS analyzer
  replace `as u32` casts with `u32::try_from(...).unwrap_or(u32::MAX)` to avoid
  `clippy::cast_possible_truncation`.

- **`opt_string` / `string_arg` use function pointers** (`crates/sloc-mcp/src/server.rs`):
  `.map(|s| s.to_owned())` replaced with `.map(str::to_owned)` in both helpers.

- **Doc comment identifier quoting** (`sloc-config`, `sloc-mcp`, `sloc-web`): Env-var
  names and function identifiers in doc comments now use backticks (`SLOC_SERVER_URL`,
  `analyze_path`, `CabinetWClass`, `SwitchToThisWindow`, etc.) for correct rustdoc
  rendering.

- **`nsf` scoring logic corrected** (`sloc-languages/style/js.rs`): `nsf` (no-semicolons
  score) was computed as `if !semis { 1.0 } else { 0.0 }` — rewritten as the cleaner
  `if semis { 0.0 } else { 1.0 }` removing the negated-bool Clippy hint.

- **Format string modernisation** (`sloc-git/tests/unit.rs`, `sloc-languages/src/lib.rs`):
  Old-style `format!("… {}", var)` with separate arg updated to inline `{var:?}` /
  `{args:?}` capture syntax.

---

## [1.5.65] — 2026-06-02

### Added

- **SLSA provenance generation** (`.github/workflows/release.yml`): New `compute-slsa-subjects`
  job aggregates SHA-256 hashes of all release binaries and a `slsa-provenance` job generates
  a signed `.intoto.jsonl` attestation bundle using the SLSA Generic Generator v2.1.0. The
  bundle is automatically attached to each GitHub Release, making supply-chain provenance
  discoverable without querying the Actions API.
- **Auto-approve workflow** (`.github/workflows/auto-approve.yml`): New workflow that
  automatically approves pull requests opened by the repository owner (`NimaShafie`), using
  the pinned `hmarr/auto-approve-action@v4.0.0`. All other contributors continue to go
  through normal review.
- **`tempfile` dev-dependency for `sloc-git`** (`crates/sloc-git/Cargo.toml`): Added
  `tempfile = "3"` as a dev-dependency to support the expanded unit test suite.

### Improved

- **Expanded test coverage** (`sloc-core`, `sloc-git`, `sloc-languages`, `sloc-report`,
  `sloc-web`): Significant new test coverage added across all major crates — ~2,100 lines of
  new tests. `sloc-core/tests/unit.rs` gains 487 lines of unit tests; `sloc-git/tests/unit.rs`
  adds 228 lines; `sloc-languages/tests/golden.rs` adds 538 lines of golden tests; a new
  `sloc-report/tests/render.rs` module adds 196 lines of render tests; and
  `sloc-web/tests/integration.rs` gains 626 lines of integration tests.
- **Helper extraction in `sloc-web`** (`crates/sloc-web/src/lib.rs`): Refactored
  `locate_report_handler` and `open_path_handler` by extracting `resolve_scan_root`,
  `gather_json_candidates`, `find_existing_ancestor`, and `resolve_open_target` as standalone
  helper functions. No behaviour change; reduces nesting depth and improves testability.

### CI

- **OpenSSF Scorecard token** (`.github/workflows/scorecard.yml`): Added `repo_token:
  ${{ secrets.SCORECARD_TOKEN }}` to the Scorecard action so it can publish results with
  appropriate permissions.
- **Dockerfile corpus fixture** (`crates/sloc-languages/tests/corpus/dockerfile/basic.dockerfile`):
  Updated test fixture to reflect the current golden-test expectations.

---

## [1.5.64] — 2026-05-27

### Added

- **Code-style column-width compliance reporting** (`sloc-core`, `sloc-web`): New analysis
  dimension that checks source files for lines exceeding a configurable column-width limit.
  Results are surfaced in the HTML report and JSON output as a `style_analysis` block with
  per-file violation counts and a workspace-level summary.
- **`--scan-config-out` and `--sub-html-out-dir` CLI flags** (`sloc-cli`): `--scan-config-out
  <path>` writes the resolved `AppConfig` as a TOML snapshot — useful for reproducing a scan
  exactly or auditing which options were active. `--sub-html-out-dir <dir>` writes individual
  per-submodule HTML reports into a named subdirectory when `--submodule-breakdown` is set.

### Fixed

- **SonarQube cognitive-complexity violations** (`sloc-core`, `sloc-languages`): Resolved all 6
  remaining S3776 cognitive-complexity violations by extracting focused helpers; no behaviour
  change.
- **cargo-deny license failures** (`deny.toml`): Added `GPL-3.0-or-later` exception for
  `auto_generate_cdp` and `sloc-git`; expanded allow-list with `0BSD`,
  `CDLA-Permissive-2.0`, and `GPL-3.0-or-later`; vendor archive and stale advisory ignore
  entries refreshed.
- **OpenSSF Scorecard code-scanning findings** (`.github/workflows/`): Addressed all
  outstanding findings from the OpenSSF Scorecard code-scanning check.

### Refactored

- **`scan_base_metrics` and `StyleAnalysis::assemble` helpers** (`sloc-core`): Extracted
  shared metric-assembly logic into reusable helpers, reducing duplication across the CLI
  summary path and HTML report rendering.

### CI

- **SLSA provenance bundles exposed as release assets** (`.github/workflows/release.yml`):
  SLSA provenance attestation bundles (`.intoto.jsonl`) are now attached directly to GitHub
  Releases, making them discoverable without querying the Actions API.
- **Improved Jenkins slug generation** (`.github/workflows/`): Build slug now consistently
  reflects the branch/tag name used in the release matrix.
- **`cargo-deny` pinned via action** (`.github/workflows/`): `cargo-deny` is now invoked
  through the official `EmbarkStudios/cargo-deny-action` with a pinned version to prevent
  unexpected failures from upstream tool changes.

---

## [1.5.63] — 2026-05-25

### Fixed

- **deny.toml license/advisory hygiene** (`ci/`): Restored transitive-dep license entries and
  fixed stale `bans`, `skip`, and `exception` entries that caused `cargo deny check` to fail.
- **Dependency advisories** (`Cargo.toml`): Upgraded `lettre` to resolve outstanding security
  advisories; suppressed unresolvable transitive advisories via `cargo-audit` configuration.

### Changed

- **Dependency bumps**: `prometheus` 0.13.4 → 0.14.0; GitHub Actions — `deploy-pages` 4 → 5,
  `upload-pages-artifact` 3 → 5, `docker/metadata-action` 6.0.0 → 6.1.0,
  `ossf/scorecard-action` 2.4.1 → 2.4.3, `codecov/codecov-action` 4 → 6,
  `CodeQL` v3 → v4 with Rust autobuild.

---

## [1.5.62] — 2026-05-25

### Added

- **Multi-platform installer configs** (`installer/`): Added Scoop manifest, nfpm configuration
  for DEB/RPM packaging, and devcontainer definition for consistent dev environments.
- **CI-aware environment metadata** (`sloc-core`): Analysis runs now capture CI environment
  variables (branch name, build number, pipeline URL) and embed them in the `AnalysisRun`
  JSON for richer CI/CD integration.
- **CI/Jenkins PDF and report integration** (`sloc-report`, `Jenkinsfile`): Jenkins pipeline
  now archives PDF reports as build artifacts and publishes HTML reports via GitHub Pages.
- **Modular Jenkins helper scripts** (`ci/jenkins/`): Jenkinsfile refactored into discrete
  helper scripts for each pipeline stage, reducing cognitive complexity and improving
  maintainability.
- **GitHub Pages publishing** (`ci.yml`): GitHub Actions workflow now enables Pages via the
  `configure-pages` action and uploads HTML reports as Pages artifacts.

### Fixed

- **GitHub CI docs job** (`.github/workflows/`): Resolved workflow failure in the `docs` job
  caused by a missing Pages enablement step.
- **CI parameter and credential binding docs** (`docs/ci-integrations.md`): Synced parameter
  counts, defaults, and credential binding guidance to match the current Jenkinsfile.

### Refactored

- **Cognitive complexity reduction** (`sloc-core`, `sloc-languages`): Restructured several
  analysis functions to satisfy the SonarQube S3776 cognitive complexity gate — no behaviour
  change, only structural simplification.

---

## [1.5.61] — 2026-05-21

### Added

- **Git remote URL in scan results** (`sloc-core`): `AnalysisRun` now includes a
  `git_remote_url` field populated by reading the `origin` remote URL directly from
  `.git/config` — no `git` executable required. Surfaced in the result page as a
  clickable commit link when the remote is a recognized GitHub/GitLab/Bitbucket host.
- **Clickable commit link in result page** (`sloc-web`): When a scan captures a git
  remote URL and commit SHA, the result page now renders a hyperlink directly to the
  commit on the hosting provider.
- **PDF report identification banner** (`sloc-report`): When the HTML report contains
  a `.report-id-banner` element (set via `report_header_footer` in the config), the
  PDF export now reads that text and passes it as Chrome's native per-page
  header/footer templates so the banner appears in the margin on every PDF page.
  Top and bottom margins are automatically widened when a banner is present.

### Fixed

- **Systemd installer script moved to `scripts/internal/`**: `install-systemd.sh`
  relocated from `scripts/` to `scripts/internal/` for consistency with the rest of
  the internal tooling. References in `docs/server-deployment.md` and
  `scripts/serve-server.sh` updated accordingly.
- **Non-ASCII characters in JS strings** (`sloc-web`): Replaced literal `…` and `-`
  characters with `…` / `-` escape sequences in inline JavaScript to prevent
  Chrome `SyntaxError` when the page is served with a non-UTF-8 content-type header.
- **Timestamp display on result page** (`sloc-web`): Scan time and generated-at
  chips now use a consistent seconds-precision format with explicit timezone label;
  the "Generated" chip wraps the timezone in parentheses to distinguish it from the
  scan time.

### Removed

- **"Built with Claude AI" badge** from README — removed authorship marketing claim
  from the project badge row and the "Built entirely by AI" prose section.

---

## [1.5.6] — 2026-05-18

### Added

- **MCP stdio server** (`crates/sloc-mcp`): New `sloc-mcp` binary implements the
  [Model Context Protocol](https://modelcontextprotocol.io) over stdio, making oxide-sloc
  directly callable as a tool from Claude Desktop, Claude Code, and any MCP-compatible agent
  host. Exposes 7 tools: `analyze_path`, `get_metrics_latest`, `get_metrics_history`,
  `get_run_metrics`, `compare_runs`, `health_check`, `ingest_result`. The `mcp.json`
  manifest at the repo root enables auto-discovery by smithery.ai and other MCP registries.
- **Pre-built JSON tool definitions** (`docs/mcp/`): `docs/mcp/tool-definitions.json`
  (Claude API `tool_use` array) and `docs/mcp/function-definitions.json`
  (OpenAI `function_calling` array) — embed oxide-sloc analysis in agent prompts without
  running an MCP host.
- **OpenAPI 3.1 specification** (`docs/openapi.yaml`): Complete machine-readable REST API
  spec committed to the repository and served live at `GET /api/openapi.yaml`.
- **RPM packaging for RHEL** (`ci/installer/rhel/`, `scripts/internal/make-rpm.sh`):
  `oxide-sloc.spec` and `make-rpm.sh` enable building an installable `.rpm` from source on
  RHEL 8/9 without an internet connection.
- **Coverage metrics in scan history and delta comparison** (`sloc-core`, `sloc-web`):
  Line, function, and branch coverage counts are now stored in `ScanSummarySnapshot` and
  propagated to all registry entries. `SummaryDelta` gains `coverage_lines_hit_delta`,
  `coverage_line_pct_delta`, `baseline_coverage_line_pct`, and `current_coverage_line_pct`
  fields. The Compare Scans page shows an animated coverage delta card when at least one
  scan has coverage data.
- **Coverage block in the metrics API** (`sloc-web`): `GET /api/metrics/:run_id` now
  returns a `coverage` object (`lines_found`, `lines_hit`, `line_pct`, `functions_found`,
  `functions_hit`, `function_pct`, `branches_found`, `branches_hit`, `branch_pct`) when the
  scan included coverage data. `GET /api/metrics/history` includes a `coverage_line_pct`
  field per entry.
- **CI install-path smoke tests** (`.github/workflows/install-paths.yml`): New workflow
  exercises all `install.sh` paths — pre-built binary extraction, `--online` download, and
  RPM — across Linux x86_64, Windows, and RHEL UBI9.
- **Settings modal template partials** (`crates/sloc-web/templates/`):
  `_settings_modal_css.html` and `_settings_modal_js.html` extracted from inline template
  code into reusable partials.

### Fixed

- **CI: Docker musl target registration and `docs/` copy** (`.github/workflows/`): Added
  `x86_64-unknown-linux-musl` target to the Docker builder stage; the Docker build now
  copies `docs/` into the image so the live `GET /api/openapi.yaml` endpoint works in
  the container.
- **CI: musl build delegates to `ci/release.sh`** (`.github/workflows/`): The musl release
  job now runs via `ci/release.sh` to match all other platform builds instead of calling
  `cargo build` directly.
- **CI: `verify-dist` warns instead of failing when Linux archive is absent**
  (`.github/workflows/`): Changed from a hard failure to a warning so the Windows-only
  dist-commit path does not block CI when no Linux archive was produced in the same run.
- **Coverage attachment tracing** (`sloc-core`): Coverage loading, format detection, file
  matching, and zero-match conditions now emit structured `tracing::debug!` / `tracing::warn!`
  events, making it straightforward to diagnose why coverage data is not attaching to
  expected source files.
- **`compute_delta` cognitive complexity** (`sloc-core`): Extracted `line_pct` as a
  standalone helper and simplified the delta computation path; SonarQube S3776 cognitive
  complexity reduced to 14 (below the threshold) without changing behaviour.

### Documentation

- **README.md**: Added MCP server configuration examples for Claude Desktop and Claude Code,
  pre-built tool definition files, OpenAPI spec location, and RPM installation instructions.

---

## [1.5.5] — 2026-05-17

### Added

- **Pure-Rust PDF generation** (`sloc-report`): Native PDF output via `printpdf` — zero external
  tool dependencies. Chromium / wkhtmltopdf are no longer required for PDF export. The PDF
  report includes 8 stat chips, git/test/coverage metrics, a 7-column language table, a
  per-file page, full comma-formatted numbers in tables, and exact counts in chip badges.
- **REST endpoints: `/api/health` and `/api/version`** (`sloc-web`): Lightweight health-check
  and version-info routes suitable for uptime monitors and CI pipelines.
- **Systemd installer** (`scripts/`): `serve-server.sh` installs and enables a `systemd` unit on
  Linux so the web UI survives reboots without a process manager.
- **Browser-upload API for server mode** (`sloc-web`): Clients that cannot access the local
  filesystem can upload a `tar.gz` archive for analysis — streaming upload with larger limits and
  a sample-path preview in the UI.
- **Run-management APIs** (`sloc-web`): `POST /api/runs/:id/bundle` and
  `DELETE /api/runs/:id` let callers download or discard individual scan artefacts; the
  web UI surfaces these as a cleanup modal with run-ID chips.
- **Comment-density chart** (`sloc-web`): Visual breakdown of comment vs. blank vs. code
  lines added to the analysis result page alongside the existing language donut.
- **Semantic expand modal** (`sloc-web`): Table rows can be expanded in-place to show
  per-file details without leaving the results page.
- **`--allow-private-net` webhook flag + custom webhook secrets** (`sloc-git`, `sloc-web`):
  Operators can now permit webhooks from RFC-1918 ranges (useful for local GitLab) and
  configure a per-endpoint HMAC secret for payload verification.
- **`--quiet` flag for `serve-server.sh`**: Suppresses the startup banner and auth-warning
  block; useful in scripted / non-TTY contexts.
- **Short run-ID badge in report title** (`sloc-web`): Each completed scan now displays a
  truncated UUID badge next to the report heading, matching the run shown in Scan History.
- **`autoprint` PDF fallback** (`sloc-report`): When a headless browser is available but
  the native PDF path is preferred, the renderer automatically falls back rather than
  failing hard.
- **File-size histogram** (`sloc-web`): New bar chart on the results page bucketing
  per-file sizes for quick identification of outlier source files.
- **Git-native metadata** (`sloc-core`): `analyze()` now optionally captures branch name,
  HEAD commit hash, and author statistics from the local git repository without shelling out
  to an external process.
- **Dynamic ping status pill** (`sloc-web`): The navigation bar displays a live server
  ping and latency reading that updates in the background.
- **Scroll-to-end output inputs** (`sloc-web`): Long stdout/stderr streams in the web UI
  auto-scroll to the most recent line as output arrives.

### Changed

- **Windows CI runner pinned to `windows-2022`** (`.github/workflows/ci.yml`): Avoids
  toolchain compatibility breakage introduced when `windows-latest` began resolving to
  a VS2026 runner image.
- **Nav max-width widened to 1720 px** (`sloc-web`): Accommodates wider viewports without
  the nav collapsing into a crowded layout.
- **Sticky footer layout** (`sloc-web`): Page footer is always anchored to the viewport
  bottom, preventing content from bleeding behind it on short pages.
- **Graceful `pick-directory` fallback** (`sloc-web`): When the native file-dialog is
  unavailable (headless server), the endpoint returns a descriptive JSON error instead of
  panicking.
- **Consistent nav/footer across all pages** (`sloc-web`): Dynamic ping pill, Settings
  cogwheel, and theme toggle are now present on every route.

### Fixed

- **Scan History not populated for uploaded projects** (`sloc-web`): Uploaded-archive runs
  now appear in `/view-reports` alongside local-path scans.
- **Upload limits, proxy trust, session cleanup, and error responses** (`sloc-web`):
  Hardened request-size caps, corrected X-Forwarded-For trust when behind a reverse proxy,
  fixed leaked run state on early errors, and unified JSON error shapes.
- **Hero title to run-ID badge gap** (`sloc-web`): Increased vertical gap to 18 px so the
  badge does not crowd the heading on narrow viewports.
- **Jenkins Docker error detection** (`ci/jenkins/`): Replaced `docker ps` with an `if`-form
  so `errexit` does not abort `--install-csp` before the diagnostic message prints.
- **`serve-server.sh` non-TTY display and open-redirect** (`scripts/`): Fixed broken
  progress output when stdout is not a terminal; patched the `next=` redirect parameter
  to reject off-origin targets.
- **Upload UI copy** (`sloc-web`): Corrected button label, upload-limit tooltip accuracy,
  and inline comment synchronisation.
- **`next=` redirect sanitisation** (`sloc-web`): The login redirect now rejects absolute
  URLs that would redirect to a third-party origin.
- **`redundant_pub_crate` Clippy warning** (`sloc-web`): Auth handlers reverted to
  `pub(crate)` visibility after the module extraction that introduced the warning.
- **Jenkins Groovy sandbox blocking `System.setProperty`** (`ci/jenkins/job-config.xml`):
  Added `<sandbox>false</sandbox>` to `job-config.xml` and its template so the CSP
  `System.setProperty` call in the Setup stage is not silently rejected on fresh installs.
- **Jenkins `preflight.sh --install-csp` when no container found** (`ci/jenkins/`): The
  script now emits a clear "can only run on the Jenkins host" message instead of a generic
  diagnostic when no Jenkins container is detected; adds an ancestor-filter fallback before
  the name-grep to handle non-standard container names.
- **`#[must_use]` on Confluence renderers** (`sloc-report`): Added missing attribute to
  `render_confluence_storage` and `render_confluence_wiki_markup` to satisfy Clippy.
- **Cognitive-complexity violations** (`sloc-report`, `sloc-web`): Large functions split
  into focused helpers; all new S3776 Clippy / SonarQube violations resolved.

### Documentation

- **Jenkins setup guide** (`docs/jenkins-manual-setup.md`): Updated for renamed
  `publishHTML` report labels, current artifact filenames, renumbered Jenkinsfile stages,
  removed stale `SKIP_SONAR` row from the Step 10 table, added Step 5g documenting that
  the Groovy sandbox is on by default for SCM-defined pipelines and that the
  `jenkins-api-token` credential is required unless the sandbox is disabled (which
  `job-config.xml` now does automatically), and rewritten Step 6 to lead with the
  `init.groovy.d` approach as the only reliable CSP fix.
- **Jenkins CI/Jenkins docs** (`ci/jenkins/README.md`): Documented the job-collision
  delete recipe; clarified Docker error detection changes and CSP escalation to warning.
- **CI integrations doc** (`docs/ci-integrations.md`): Added `smoke:pdf` to the list of
  parallel smoke jobs; updated `generate-dashboard.py` usage to document the optional
  `history-file` positional argument.
- **Jenkinsfile + Dockerfile.agent** (`ci/`): Shell steps that expand build parameters
  (`GIT_REF`, `COMPARE_TO_REF`, `COMPARE_TO_PREV_TAG`, `OUTPUT_SUBDIR`) now use
  `withEnv([...])` wrappers to satisfy the Groovy sandbox; updated the llvm-tools
  component name from `llvm-tools-preview` to `llvm-tools` in both the Jenkinsfile
  coverage step and `Dockerfile.agent`.

---

## [1.5.4] — 2026-05-15

### Added

- **Tree-sitter symbol counting** (`sloc-languages`): Python AST walks now count functions,
  classes, test functions/classes, and assertion calls using tree-sitter. A new `SymbolKinds`
  struct drives per-language counting configuration; the C/C++ path returns `SymbolKinds::none()`
  (no behavioural change for C).
- **Auth integration tests** (`sloc-web`): 30+ new integration tests covering `auth/login` GET
  and POST, Bearer and `X-API-Key` header auth, export/import config (valid and malformed TOML),
  scan profile CRUD, schedule deletion, metrics and submodule endpoints, GitHub webhook smoke,
  metrics history, and error-module JSON shape (`not_found`, `bad_request`, `422`). New
  `make_test_router_with_key()` entry point supports auth-enabled test scenarios.
- **Jenkins graphical report enhancements** (`ci/jenkins/generate-dashboard.py`): Added a
  per-language metrics table (code / comment / blank / files columns) below the bar chart, a
  "Top Files by Code Lines" card (top 20 files from `per_file_records`), and a "Code Lines Δ"
  stat chip showing the SLOC delta between the two most recent builds when trend history has ≥ 2
  entries. Inline `<style>` extracted to a sidecar `dashboard_<slug>.css` file so the report
  renders under Jenkins' default artifact-viewer CSP without requiring a credential binding or
  `init.groovy.d`.

### Changed

- **Jenkins pipeline — SonarQube stage removed** (`Jenkinsfile`): `SKIP_SONAR`, `SONAR_URL`, and
  `GENERATE_COVERAGE` pipeline parameters and the entire SonarQube scan stage are removed.
  SonarQube analysis is now run externally via `ci/sonar/` scripts driven by the `SONAR_URL` and
  `SONAR_TOKEN` environment variables. See `docs/sonarqube-manual-setup.md` for setup
  instructions. The Graphical Report sidebar link is promoted above the SLOC Report link.
- **Jenkins CSP handling** (`ci/jenkins/`): The Content-Security-Policy override moved from an
  in-pipeline `System.setProperty` call (blocked by the Pipeline sandbox) to a startup Groovy
  script at `ci/jenkins/init.groovy.d/relax-csp.groovy`, which executes during Jenkins boot.
- **`sloc-config` serde defaults** (`crates/sloc-config/src/lib.rs`): All fields in
  `DiscoveryConfig`, `AnalysisConfig`, `ReportingConfig`, and `WebConfig` are annotated with
  `#[serde(default)]`; partial TOML files (e.g. CI presets that omit infrequently-set keys)
  now deserialize without errors.
- **CI preset TOMLs** (`ci/sloc-ci-*.toml`): All three presets (`default`, `full-scope`,
  `strict`) now explicitly declare `enabled_languages`, `extension_overrides`, and
  `shebang_detection` for self-documenting completeness.
- **Windows install path** (`scripts/`): `run.sh` on Windows now extracts the pre-built binary
  from `dist/oxide-sloc-windows-x64.zip` rather than bootstrapping the full Rust toolchain;
  air-gap Windows deployments require no compiler and no `toolchain/` archives.
- **`install.sh` relocated** (`scripts/internal/install.sh`): Moved from the repository root to
  `scripts/internal/`; `run.sh` updated to invoke the new path.
- **Jenkins trend sparkline** (`ci/jenkins/generate-dashboard.py`): Build-trend card moved before
  the Language Breakdown card for visual prominence; SLOC trend values persist across builds via
  the dashboard history store.

### Fixed

- **Jenkins HTML Publisher CSP** (`ci/jenkins/init.groovy.d/relax-csp.groovy`): The relaxed
  Content-Security-Policy is now applied at Jenkins startup rather than inside the pipeline,
  preventing sandbox rejections and ensuring the policy is in place before the first build runs.
- **Jenkins build description format** (`Jenkinsfile`): Build description now uses plain text;
  removes Jenkins HTML-injection warnings that appeared in the build log.
- **Web auth handler visibility** (`sloc-web`): Auth handlers reverted to `pub(crate)` to
  suppress the `redundant_pub_crate` Clippy warning introduced when the auth module was extracted.
- **RHEL unprivileged firewall check** (`scripts/`): Removed a `firewall-cmd --state` call that
  failed with a permission error when run as a non-root user on RHEL 8/9.

### Documentation

- **Jenkins manual setup** (`docs/jenkins-manual-setup.md`): Step 5b credential instructions
  replaced with a note explaining that `sonarqube-oxide-sloc-token` is no longer consumed by the
  current Jenkinsfile. Troubleshooting section rewritten to reflect the external-script SonarQube
  path; `SKIP_SONAR` references removed.
- **Jenkins README** (`ci/jenkins/README.md`): Job-name instructions clarified — `oxide-sloc` is
  the single canonical SCM-driven job; the `oxide-sloc-manual` name carries no special meaning.

---

## [1.5.1] — 2026-05-12

### Fixed

- **install.sh CA cert import** (`scripts/install.sh`): The self-signed Authenticode CA
  certificate is now imported silently without prompting the user; previous behaviour
  popped a native Windows security dialog mid-install.
- **CI cargo-cyclonedx flag** (`.github/workflows/release.yml`): Corrected an invalid
  flag passed to `cargo cyclonedx` that caused SBOM generation to fail; `build.rs` is
  now formatted to satisfy the `rustfmt` CI gate.

### Added

- **Windows CA cert trust — no Admin required** (`scripts/install.sh`): `install.sh`
  auto-detects the self-signed Authenticode CA certificate committed to `deploy/certs/` and
  offers to import it into the current user's certificate store using `certutil -addstore
  -user`, which requires no Administrator elevation.
- **Self-signed Authenticode cert + generator** (`deploy/certs/`, `scripts/internal/`): A
  self-signed code-signing certificate and companion generation script are committed to
  the repository so Windows builds can be Authenticode-signed immediately without waiting
  for a commercial CA.
- **Authenticode signing, SLSA provenance, and dist commit** (`release.yml`): The release
  workflow now signs `oxide-sloc.exe` with the committed certificate, attaches SLSA
  provenance, and commits the signed binary to `dist/` for air-gapped Windows deployments.
- **Windows test job** (`ci.yml`): A `windows-latest` test job is added to the CI matrix,
  covering `cargo test --workspace` on Windows and closing the gap where signing-related
  code was only tested on Linux runners.

---

## [1.5.0] — 2026-05-12

### Added

- **Multi-format coverage parsing** (`sloc-core/coverage.rs`): Added auto-detecting coverage parsers
  for Cobertura XML (`pytest-cov`, Maven Cobertura plugin), JaCoCo XML (Gradle, Maven JaCoCo plugin),
  and Istanbul/NYC JSON (`nyc --reporter=json-summary`, Jest) alongside existing LCOV. The new
  `parse_coverage_auto()` function dispatches to the correct parser by file extension and content
  sniff — `.xml` files are distinguished by `<coverage` vs `<report` header, `.json` goes to the
  Istanbul parser, and all other extensions fall back to LCOV.
- **`/api/suggest-coverage` endpoint** (`sloc-web`): Returns the inferred coverage file path(s)
  and the recommended generation command for a given project root. `detect_coverage_tool()` examines
  the directory tree for `Cargo.toml`, `pom.xml`, `build.gradle`, `package.json`, and other build
  files to select the correct tool and command hint. The web file picker now accepts all four
  coverage formats (LCOV `.info`, Cobertura `.xml`, JaCoCo `.xml`, Istanbul `.json`).
- **Test Metrics stat-chip summary strip** (`sloc-web`): Replaced the single test-density badge
  on the `/test-metrics` page with a four-chip summary strip: test-to-code density, most-tested
  language, number of languages with tests, and overall line coverage percentage.
- **`cov-gauge-card` coverage components** (`sloc-web`): Coverage display upgraded from a plain
  percentage text to animated `cov-gauge-card` components showing label, large percentage value,
  and an animated progress-bar track; cards lift on hover.
- **Trend report table enhancements** (`sloc-web`): The scan-history table in `/trend-reports`
  gains sortable columns (click header to sort ascending/descending with ↕/↑/↓ icons),
  column-resize handles, and a client-side filter row.
- **Trend report stat chips** (`sloc-web`): Summary chips above the trend chart now show Total
  Scans, Latest metric value, Net Change (with `+`/`−` colour coding), and Projects count.

### Changed

- **Artifact URL segment order** (`sloc-web`): Run artifact URLs swapped from
  `/runs/{run_id}/{artifact}` to `/runs/{artifact}/{run_id}` across all URL construction sites,
  route definitions, Confluence push, git_browser, and integrations handlers.
- **Run-ID chips** (`sloc-report`): Run-ID banners on the report page switch to `position:fixed`
  so they repeat on every printed/PDF page; chips lift on hover and show a click-to-copy tooltip.
  Print margins are widened when `report_header_footer` is set.
- **Support-opportunities table** (`sloc-report`): Description and Example columns now wrap with
  styled example-file badges instead of overflowing.
- **Per-file table scrollbar** (`sloc-report`): `overflow-y:scroll` is forced on per-file and
  skipped-file tables to keep the scrollbar gutter permanently allocated; a JS IIFE measures the
  actual scrollbar track width and compensates table padding.
- **Per-file table column widths** (`sloc-report`): All columns corrected to sum to ~98% so no
  column overflows under the scrollbar.
- **PDF print output** (`sloc-report`): Print zoom set to 0.82, chart spacing tightened, and
  the scatter chart rendered in a single-column centred layout.
- **Nav dropdown gap** (`sloc-web`, `sloc-report`): Added `::after` pseudo-element bridge on
  dropdown triggers so the hover state is not lost when moving the cursor from the trigger pill
  to the menu; nav pills and brand logo gain `white-space:nowrap` / `flex-shrink:0` to prevent
  wrapping on narrow viewports.
- **CLI headless-chrome noise** (`sloc-cli`): Suppressed `headless_chrome` crate transport log
  lines from the default tracing filter so PDF generation does not pollute terminal output.

---

## [1.4.3] — 2026-05-07

### Added

- **Browser-based login form** (`/auth/login`): New GET/POST route that lets browsers
  authenticate with the API key via a sign-in form. A successful sign-in sets an
  `HttpOnly; SameSite=Strict` session cookie (`sloc_session`) so subsequent page loads
  don't require header injection. Unauthenticated browser requests are now redirected to
  `/auth/login` instead of returning a bare `401`.
- **Configurable auth lockout** (`SLOC_AUTH_LOCKOUT_FAILS`, `SLOC_AUTH_LOCKOUT_SECS`):
  Auth-failure lockout threshold and window are now configurable via environment variables
  (defaults: 10 failures / 3600 s). The `Retry-After` response header on lockout replies
  now reflects the actual remaining seconds rather than a hardcoded value.
- **Firewall auto-open** (`--open-firewall`, `scripts/serve-server.sh`): New flag that
  automatically opens the server port via `firewall-cmd` or `ufw` (using `sudo`) when a
  blocking Linux firewall rule is detected.
- **Chart hover tooltips and entry animations** (`sloc-report/lib.rs`): Donut segments and
  bar chart rows animate on page load and display a floating, theme-aware tooltip (language
  name, code lines, percentage) on hover. SVG charts set `overflow:visible` so hover
  scaling remains fully visible.

### Fixed

- **PDF print layout** (`sloc-report/lib.rs`): Changed `@page` CSS to A4 landscape for
  better table rendering; released sticky column positioning in print media; set
  `min-width:0` on per-file and skipped-files tables so they scale to page width; set
  label `white-space:normal` to prevent line-content truncation in print.
- **Nav dropdown gap** (`sloc-report/lib.rs`): Added `padding-bottom:6px` to
  `.nav-dropdown-wrap` and changed menu `top` from `calc(100% + 6px)` to `100%`, removing
  the mouse-gap that caused the dropdown to close before the cursor reached it.
- **install.sh offline by default** (`scripts/install.sh`): Network access now requires
  the explicit `--online` flag; `--offline` is kept as a backward-compatible no-op. Fixed a
  `set -euo pipefail` × `grep` interaction that could silently abort the script when
  `SHA256SUMS.txt` contained no matching entry.
- **docker-compose.yml YAML** (`docker-compose.yml`): Switched `environment` block from
  list to map syntax to fix a parse error on Compose v2.

### Changed

- **`serve-server.sh` banner** (`scripts/serve-server.sh`): Banner now shows the browser
  URL, the login-form URL (when `SLOC_API_KEY` is set), lockout configuration hints, and
  the `curl` test command using the real LAN IP.
- **Super-repo compare scope** (`/compare-scans`): New `?scope=super` query parameter
  filters the diff to super-repo files only (excludes submodule files). The `/view-reports`
  page shows a scope pre-selection panel when two rows with submodule data are selected.
- **Submodule chips in history table** (`sloc-web/lib.rs`): Submodule names shown as chips
  in the history table; overflow truncated to 4 + "+N more".
- **LAN diagnostics** (`scripts/serve-server.sh`): `check_firewall()` probes `firewall-cmd`
  then `ufw` and prints the exact fix command when port `4317/tcp` is blocked;
  `get_primary_ip()` now uses `ip route get 1.1.1.1` to select the default-route interface
  IP, keeping Docker/Podman bridge addresses in a separate banner section.

---

## [1.4.2] — 2026-05-06

### Added

- **Async scan loading modal** (`sloc-web/lib.rs`): Redesigned loading overlay with elapsed
  timer, analysis phase indicator, error/retry UI, and dismiss button. The `analyze_handler`
  now returns an `X-Wait-Id` response header for client-side async tracking.
- **PDF status polling** (`/api/runs/{run_id}/pdf-status`): New endpoint for clients to poll
  PDF generation progress. The PDF button shows a spinner while generating and swaps to a
  live download link on completion.
- **Language icons** (`crates/sloc-web/assets/`): Added icons for Assembly, Go, R, XML,
  Groovy, Dockerfile, Makefile, and Perl. All icons and logos are now compiled into the
  binary via `include_bytes!()`, eliminating the runtime file-serving dependency.
- **Result page UX** (`sloc-report/lib.rs`): Dark/light theme toggle, floating code
  particles, background watermarks, and version footer added to the analysis result page.

### Fixed

- **PDF path on Windows** (`sloc-report/lib.rs`): PDF output now writes to a short temp
  path in `%TEMP%` then renames to the final destination, avoiding `MAX_PATH` failures on
  Windows.

### Changed

- **Dockerfile** updated from `rust:1.85` to `rust:1.95-slim-bookworm`.
- **Report UI polish** (`sloc-report/lib.rs`): Metric card and hero section redesigned with
  larger numbers and accent borders. Per-file table min-width increased to 1150 px with
  adjusted column widths.
- **Dependencies**: `tree-sitter` bumped to 0.26.8; `toml` bumped to 1.1.2 (TOML 1.1
  spec).

---

## [1.4.1] — 2026-05-04

### Added

- **LAN server launcher** (`scripts/serve-server.sh`): Dedicated script to start oxide-sloc in
  server mode (binds to `0.0.0.0:4317`). Auto-generates an API key, prints every LAN address the
  server is reachable on, and shows a ready-made `curl` test command. Also add `--host` flag and
  `SLOC_HOST=1` env var to `run.sh` as an alternative.
- **Auto-install Rust via rustup** (`scripts/install.sh`): When no pre-built binary is found and
  Rust is not installed, the installer now detects internet connectivity and offers to install Rust
  via `rustup`. New `--auto` flag installs without prompting (useful in CI).
- **VirusTotal scanning workflow** (`.github/workflows/vt-scan.yml`): Manual `workflow_dispatch`
  action that uploads compiled release binaries to VirusTotal and publishes a markdown scan report
  as a job summary. Supports both tag mode (scans an existing release) and HEAD mode (builds from
  current branch).
- **Integration test harness** (`crates/sloc-web/tests/integration.rs`): Initial integration test
  suite using the new `make_test_router()` entry point, covering core web routes without a live
  TCP binding.
- **`/locate-reports-dir` route**: New web endpoint to open a native directory-picker dialog
  specifically for selecting a reports output directory.

### Fixed

- **Webhook payload hardening** (`sloc-git/webhook.rs`): Replaced silent empty-string fallbacks
  with proper error propagation in all three webhook parsers (GitHub, GitLab, Bitbucket). Malformed
  or incomplete payloads now return descriptive errors instead of silently triggering scans with
  blank repository URLs or commit SHAs.
- **`ScanScheduleProvider` serialization** (`sloc-git/schedule.rs`): Changed from
  `rename_all = "snake_case"` to explicit per-variant `serde(rename = …)` attributes to ensure
  correct lowercase round-trip serialization (`github`, `gitlab`, `bitbucket`, `any`).
- **Git ref date format** (`sloc-git/ops.rs`): Changed `--format` date specifier from `iso8601`
  to `iso-strict` (RFC 3339) for branch and tag listing, fixing date parsing in environments
  where the `iso8601` alias is not available.
- **Git shallow clone depth** (`sloc-git/ops.rs`): Added `--depth=50` to `clone_or_fetch` so
  the git browser does not download the full history of large repositories.
- **PDF page layout** (`sloc-report/lib.rs`): Changed `@page` CSS from A4 landscape to A4
  portrait with tighter margins; fixes reports that were clipped or had excessive whitespace when
  exported to PDF.
- **Print page-break control** (`sloc-report/lib.rs`): `.hero` moved to `break-inside: auto`
  so large summary tables are not forced onto a single page, preventing blank-page artefacts.
- **CSP nonce plumbing** (`sloc-report/lib.rs`): Added `nonce` field to `ReportTemplate` and
  `nonce="{{ nonce }}"` attribute on the inline `<style>` tag so the web server can inject a
  per-request Content-Security-Policy nonce.

### Changed

- **Scripts reorganisation**: Internal maintenance scripts (`airgap-build.sh`,
  `clippy_to_sonar.py`, `install-hooks.sh`, `make-airgap-kit.sh`, `update-vendor.sh`,
  `vt-scan.py`) moved to `scripts/internal/`. All CI workflow references updated accordingly.
- **Router extraction** (`sloc-web/lib.rs`): `build_router()` extracted from `serve()` and made
  separately callable; `make_test_router()` added as a public entry point for test code.
- **README**: Added "Host on your LAN" section documenting `serve-server.sh`, firewall commands,
  and authentication usage for LAN deployments.

---

## [1.4.0] — 2026-05-03

### Added

- **Automated scanning via webhooks**: New `/webhook-setup` UI and `/webhooks/{github,gitlab,bitbucket}`
  receivers. Each schedule gets a unique HMAC-SHA256 secret; incoming push events are verified and
  trigger an automatic clone + scan without any manual action.
- **Polling-based scheduled scans**: Schedules can run on a configurable interval (seconds). The
  server compares the current HEAD SHA against the last-scanned SHA and only runs a scan when the
  branch has advanced. Poll tasks are restarted automatically on server boot from persisted state.
- **Git browser UI** (`/git-browser`): Browse branches, tags, and recent commits of any remote
  repository from the web UI. Each row has a **Scan** button; selecting two rows triggers a
  side-by-side SLOC comparison.
- **Point-in-time comparison across CLI, web, and Jenkins**:
  - CLI: `oxide-sloc git-scan <repo> <ref>` and `oxide-sloc git-compare <repo> <baseline> <current>`
  - CLI: `oxide-sloc watch <repo> <branch> --interval <secs>` for continuous local polling
  - Jenkins: `GIT_REF`, `COMPARE_TO_REF`, and `COMPARE_TO_PREV_TAG` parameters; new
    "Git-Ref Scan" and "Git-Ref Compare" stages produce `ref-scan.json`, `diff.json`, `diff.csv`
- **`sloc-git` crate** (new, first published to crates.io): git CLI wrappers
  (`clone_or_fetch`, `list_refs`, `create_worktree`, `destroy_worktree`, `get_sha`, `list_commits`),
  HMAC-SHA256 webhook verification via `ring`, and a JSON-persisted `ScheduleStore`.
- **SonarQube CI integration**: `clippy_to_sonar.py` converts Clippy JSON output to the
  SonarQube Generic Issue format; Jenkins pipeline now includes a SonarQube scan stage with
  coverage upload via `cargo-llvm-cov`.
- **`cargo-llvm-cov` vendored** for air-gapped coverage generation.

### Fixed

- **Docker hardening**: Multiple Dockerfile findings resolved — replaced `COPY . .` with explicit
  file copies, inlined `.cargo/config.toml` via `RUN`, split overlong `RUN` lines, and corrected
  Python security hotspot.
- **crates.io packaging**: Logo assets (`logo-text.png`, `small-logo.png`) moved into
  `sloc-report/assets/logo/` so the crate compiles correctly when installed via `cargo install`.
- **`sloc-git` metadata**: Added missing `description`, `homepage`, `documentation`, `keywords`,
  and `categories` fields required by crates.io.

---

## [1.3.7] — 2026-05-02

### Changed

- **CLI output helpers**: Extracted `log_written()` helper in `write_outputs()` to eliminate
  repeated quiet-flag checks across all five artifact types (JSON, HTML, PDF, CSV, XLSX).
- **Language analyzer refactoring**: Extracted `step_through_block_comment()`,
  `try_open_block_comment()`, `process_physical_line()`, `track_active_docstring()`,
  `try_record_docstring_if_context()`, `mark_unclosed_docstring_lines()`, and
  `classify_ts_line()` from the generic and Python docstring scanners; removes the
  remaining `#[allow(clippy::too_many_lines)]` attributes on those paths.
- **Web handler refactoring**: Extracted `validate_locate_request()`, `locate_path_hint()`,
  `apply_form_to_config()`, `spawn_pdf_background()`, `sum_added_code_lines()`,
  `sum_removed_code_lines()`, and `build_submodule_row()` from `analyze_handler` and
  `locate_report_handler`; removes the `#[allow(clippy::too_many_lines)]` attribute on
  `analyze_handler`.

---

## [1.3.6] — 2026-05-02

### Fixed

- **SMTP TLS**: Replaced implicit `starttls_relay` builder with an explicit `TlsParameters`
  builder + `Tls::Required`, ensuring certificate validation is enforced and the TLS
  handshake behaviour is unambiguous.
- **Webhook URL validation**: Extended IPv6 blocklist to cover ULA ranges (`fc00::/7`)
  and link-local addresses (`fe80::/10`), which were previously not blocked.
- **PDF temp-dir cleanup**: Replaced manual `create_dir_all` / `remove_dir_all` pair with
  `tempfile::Builder::tempdir()` so the browser profile directory is always cleaned up on
  drop, even when an error is returned mid-function.
- **Rate limiter memory growth**: The in-memory IP tracking map is now pruned when it
  exceeds 10,000 entries, preventing unbounded growth under sustained unique-IP traffic.
- **X-Forwarded-For IP spoofing**: Trusting the `X-Forwarded-For` header for rate limiting
  is now opt-in via `SLOC_TRUST_PROXY=1`; by default, only the socket peer address is used.
- **Auth brute-force**: IPs that exceed 10 failed authentication attempts within an hour are
  locked out for the remainder of that window and receive `429 Too Many Requests`.

### Changed

- **Multi-key authentication**: `SLOC_API_KEYS` (comma-separated list) is now the preferred
  env var; `SLOC_API_KEY` remains supported for backward compatibility. API key values are
  stored in `secrecy::Secret` to prevent accidental logging.
- **CORS policy**: In `--server` mode the CORS layer now defaults to deny-all; set
  `SLOC_ALLOWED_ORIGINS` (comma-separated) to allow specific origins. In local (non-server)
  mode, only `http://127.0.0.1:*` and `http://localhost:*` are permitted.
- **GitHub Actions permissions**: Moved `contents: write` from the workflow-level default to
  only the `publish` job; all other jobs receive `contents: read` (principle of least
  privilege).
- **Dockerfile**: Removed unused `wget` from the runtime image; replaced the `wget`-based
  `HEALTHCHECK` with `oxide-sloc healthz`.
- **docker-compose.yml**: Added container hardening — `cap_drop: ALL`,
  `no-new-privileges: true`, `read_only: true`, and a 64 MiB `/tmp` tmpfs mount.
- **Structured tracing**: Added `tracing::warn!` / `tracing::info!` events for auth
  failures, auth lockouts, rate-limit hits, path rejections, and completed scans.

---

## [1.3.5] — 2026-05-02

### Fixed

- **Jenkins build description**: Build description now correctly reads `params.SCAN_PATH`
  instead of `env.SCAN_PATH`, which was always null and produced blank descriptions.
- **Docker build**: Added `xz-utils` to the builder stage so `tar -xJf vendor.tar.xz`
  succeeds in environments where `xz` is not pre-installed.

---

## [1.3.0] — 2026-05-01

### Added

- **Air-gap build kit** (`scripts/make-airgap-kit.sh`): generates a fully self-contained
  offline build archive bundling the Rust host toolchain, musl Rust std, musl C toolchain
  (musl-gcc from musl.cc), all crate vendor sources, and a self-contained `install.sh`.
  The kit builds a fully static binary on air-gapped Linux systems with no pre-installed
  Rust, no C compiler, and no internet access required.

### Changed

- Repository layout migrated to standard skeleton: CI scripts moved to `ci/` (`lint.sh`,
  `build.sh`, `test.sh`, `release.sh`), launcher scripts moved to `scripts/`, sample
  fixtures moved to `tests/fixtures/basic/`, and image assets moved to `docs/assets/`.
- `vendor.tar.xz` is no longer committed to git; it is generated by the `vendor` job in
  the release workflow and attached as a GitHub release asset.
- `docs/airgap.md` restructured around four clearly labelled transfer paths, with the
  fully self-contained kit as the primary recommended option.
- Added `CODE_OF_CONDUCT.md`, `.github/dependabot.yml`, `clippy.toml`, `deny.toml`.

---

## [1.2.8] — 2026-05-01

### Changed

- Resolved 162 remaining SonarQube findings (16 HIGH, 146 MEDIUM) from the v1.2.7 rescan:
  - **Cognitive complexity (rust:S3776, 15 HIGH)**: Reduced cognitive complexity across 15 functions by extracting focused helper functions — `detect_by_shebang`, `detect_by_extension`, `scan_line`, `finalize_line_facts`, `process_string_char`, `process_block_comment_char`, `walk_root`, `process_submodules`, `assemble_run`, `check_metadata_policy`, `decode_file_contents`, `write_outputs`, `check_exit_conditions`, `build_browser_args`, `wait_for_pdf_stable`, `validate_server_scan_path`, `locate_report_error`, `build_run_registry_entry`, and others; `// NOSONAR` added to irreducibly complex state-machine functions
  - **`too_many_lines` (30)**: Added `#[allow(clippy::too_many_lines)]` to `print_summary`, `compute_delta`, `analyze_text`, `write_xlsx`, `compare_handler`, `build_preview_html`, and all functions also addressed by cognitive-complexity extraction
  - **`multiple_crate_versions` (68)**: Added `#![allow(clippy::multiple_crate_versions)]` at crate-root level in `sloc-cli`, `sloc-core`, `sloc-report`, and `sloc-web` — these are transitive dependency version conflicts outside project control
  - **`similar_names` (30)**: Added `#[allow(clippy::similar_names)]` to `analyze_handler` — abbreviated metric names (`prev_fa`, `prev_cl`, etc.) are idiomatic and intentional
  - **`struct_excessive_bools` (12)**: Added `#[allow(clippy::struct_excessive_bools)]` to `DiscoveryConfig`, `AnalysisConfig`, `ScanConfig` (both crates), `LineFacts`, and `AnalyzeArgs` — all booleans represent independent configuration flags
  - **`trivially_copy_pass_by_ref` (2)**: Changed `ieee: &IeeeFlags` → `ieee: IeeeFlags` in `analyze_generic` (3-byte `Copy` struct); updated all 38 call sites
  - **`zero_sized_map_values` (2)**: Replaced `HashMap<&str, ()>` with `HashSet<&str>` in `compute_delta`
  - **`missing_panics_doc` (2)**: Added `# Panics` section to `serve()`
  - **`python:S1186` (1)**: Added explanatory comment to the intentionally-empty `hello()` corpus test fixture

### Documentation

- **Jenkins bootstrap gaps closed**: Added `ci/jenkins/.env.example` for local credential storage and `ci/jenkins/preflight.sh` pre-flight check script.
- Added "Obtaining credentials" section to `ci/jenkins/README.md` and `docs/ci-integrations.md` covering initial admin password retrieval (native and Docker installs) and API token minting click-path.
- Added native/systemd plugin install path (Jenkins CLI jar) as Path 3 in `ci/jenkins/plugins.txt` and `ci/jenkins/README.md`.
- Rewrote the CLI bootstrap snippet in both docs to use `JENKINS_TOKEN` (sourced from `ci/jenkins/.env`) instead of the bare `JENKINS_PASS` placeholder; dropped the unnecessary cookie jar from token-based authentication.
- Added explicit seed-build curl (`POST /job/${JOB_NAME}/build`) with note that the first build seeds the parameters form.
- Added note that LAN/remote URLs (e.g., `http://10.0.0.8:8080`) are valid and that trailing slashes must be stripped.
- Added job-name decision rule: use `oxide-sloc` for the SCM-driven job; use `oxide-sloc-manual` only when maintaining a parallel hand-edited job in the same instance.
- Added `ci/jenkins/.env` to `.gitignore`.

---

## [1.2.7] — 2026-05-01

### Changed

- Resolved 485 SonarQube findings (16 HIGH, 469 MEDIUM) across all 6 crates with zero remaining actionable issues:
  - Replaced 139 unnecessary struct-name repetitions with `Self` in `impl` blocks
  - Converted 41 `push_str(&format!(...))` calls to `write!()` to avoid intermediate allocations
  - Fixed 22 case-sensitive file-extension comparisons to use `eq_ignore_ascii_case`
  - Added `# Errors` doc sections to all 15 public `Result`-returning functions missing them
  - Merged identical `match` arms, removed redundant closures, inlined format args, and applied `let…else` rewrites throughout
  - Tightened mutex-guard scopes in four `sloc-web` handlers (`significant_drop_tightening`)
  - Converted `resolve_output_root` from `Result<PathBuf>` to `PathBuf` (unnecessary wrap removed)
  - Added `#[allow]` with explanatory context for deliberate narrowing casts in ZIP generation, calendar math, and badge-pixel arithmetic
  - Added intent comment to Python test corpus fixture (`mixed.py`) for empty method

---

## [1.2.0] — 2026-04-29

### Added

**IEEE 1045-1992 physical SLOC compliance**
- The counting engine now implements all configurable parameters defined in IEEE Std 1045-1992 *Software Productivity Metrics*:
  - `continuation_line_policy` / `--continuation-line-policy` (`each-physical-line` | `collapse-to-logical`) — IEEE §3: optionally collapse backslash-continued C macro / shell / Makefile lines into a single logical line count instead of counting each physical line
  - `blank_in_block_comment_policy` / `--blank-in-block-comment-policy` (`count-as-comment` | `count-as-blank`) — IEEE §4: blank lines inside `/* ... */` blocks are classified as comment lines by default (IEEE aligned); `count-as-blank` restores legacy behaviour if needed
  - `count_compiler_directives` / `--no-count-compiler-directives` — IEEE §4.2: `#include`, `#define`, `#ifdef`, and other C/C++/Objective-C preprocessor directive lines are now tracked separately as `compiler_directive_lines` in the raw JSON output; passing `--no-count-compiler-directives` (or setting `count_compiler_directives = false`) excludes them from effective code SLOC while keeping the raw count intact
  - All three parameters are settable in `.oxide-sloc.toml` under `[analysis]` and via CLI flags on `analyze`

**Web server hardening**
- IP-based sliding-window rate limiter (60 requests / 60 seconds per client IP) across all routes — no external crate required; uses only `std` + `Instant`
- Bearer-token authentication via `SLOC_API_KEY` env var — when set, all requests must supply a matching `Authorization: Bearer <key>` or `X-API-Key: <key>` header; startup warning logged when running in server mode without a key
- Native TLS termination via `SLOC_TLS_CERT` / `SLOC_TLS_KEY` PEM env vars (powered by `tokio-rustls` + `rustls`); startup warning logged when `--server` is used without TLS configured
- CORS headers via `tower-http::CorsLayer`
- Response headers middleware (X-Content-Type-Options, X-Frame-Options, Referrer-Policy, etc.)
- Graceful shutdown on `Ctrl+C` (both local and server modes)

**New web routes**
- `GET /view-reports` — scan history browser
- `GET /compare-scans` — side-by-side scan comparison UI
- `GET /embed/summary` — embeddable summary widget (iframe-friendly)

**Webhook URL validation**
- `validate_webhook_url()` now enforces HTTPS and blocks loopback, RFC-1918 private ranges, link-local, and cloud metadata endpoints (`169.254.169.254`, `metadata.google.internal`, `*.local`)

**SMTP credential safety**
- `--smtp-pass` on the `send` command now emits a visible warning when used directly; use `SLOC_SMTP_PASS` env var instead to keep credentials out of process listings

**CI/CD build configuration**
- Docker builder and runtime images pinned to SHA-256 digests (`rust:slim@sha256:…`, `debian:bookworm-slim@sha256:…`) — prevents silent base-image substitution
- GitLab CI pipeline switched from curl-piped rustup to the official `rust:slim` pinned image
- `vendor.tar.xz` integrity verified via `sha256sum -c vendor.tar.xz.sha256` before extraction in Dockerfile, GitLab CI, and Jenkinsfile
- Docker image signed with `cosign` (keyless OIDC) and SBOM attached via `docker/build-push-action`; `id-token: write` permission added to `docker.yml`
- Jenkins parameters `SCAN_PATH`, `REPORT_TITLE`, `MIXED_LINE_POLICY` passed through `withEnv` (shell variables, not Groovy interpolation); allowlist validation added for choice and free-text parameters (`MIXED_LINE_POLICY`, `CI_PRESET`, `OUTPUT_SUBDIR`, glob patterns, language names)
- Jenkins CSP relaxation rationale documented inline; alternative of serving HTML from a separate origin noted for high-assurance environments

**Docker**
- `HEALTHCHECK` instruction added — polls `GET /healthz` every 30 s; 5 s timeout; 3 retries
- `SLOC_BROWSER_NOSANDBOX=1` env var added to Docker image — bypasses Chromium kernel-namespace sandbox (required in most container runtimes without `SYS_ADMIN`); documented with guidance on when to disable it
- `wget` added to runtime image (required by `HEALTHCHECK`)

---

## [1.0.0-rc.1] — 2026-04-25

> Release candidate for 1.0.0. Core feature set is complete. Please test and
> report issues — no breaking changes are expected between rc.1 and 1.0.0.

### Added

**Language support — 30 new languages (41 total)**
- Assembly (`.asm`, `.s`)
- Clojure (`.clj`, `.cljs`, `.cljc`, `.edn`)
- CSS (`.css`)
- Dart (`.dart`)
- Dockerfile (`Dockerfile`, `Dockerfile.*`)
- Elixir (`.ex`, `.exs`)
- Erlang (`.erl`, `.hrl`)
- F# (`.fs`, `.fsi`, `.fsx`)
- Groovy (`.groovy`, `.gradle`)
- Haskell (`.hs`, `.lhs`)
- HTML (`.html`, `.htm`, `.xhtml`)
- Julia (`.jl`)
- Kotlin (`.kt`, `.kts`)
- Lua (`.lua`)
- Makefile (`Makefile`, `GNUmakefile`, `.mk`)
- Nim (`.nim`, `.nims`)
- Objective-C (`.m`, `.mm`)
- OCaml (`.ml`, `.mli`)
- Perl (`.pl`, `.pm`, `.t`)
- PHP (`.php`)
- R (`.r`)
- Ruby (`.rb`, `.rake`, `Rakefile`, `Gemfile`)
- Scala (`.scala`, `.sc`)
- SCSS / Sass (`.scss`, `.sass`)
- SQL (`.sql`)
- Svelte (`.svelte`)
- Swift (`.swift`)
- Vue (`.vue`)
- XML / SVG (`.xml`, `.xsd`, `.xsl`, `.svg`)
- Zig (`.zig`)

**New output formats**
- `--csv-out <path>` on `analyze` and `report` — two-section CSV (summary + per-file)
- `--xlsx-out <path>` on `analyze` and `report` — multi-sheet Excel workbook (Summary, By Language, Per File, Skipped); self-contained ZIP+XML implementation, no external dependency
- `--csv-out` / `--xlsx-out` on `diff` — export delta as spreadsheet

**New CLI commands**
- `oxide-sloc diff <baseline.json> <current.json>` — compare two saved scans; prints colored delta summary; supports `--json-out`, `--csv-out`, `--xlsx-out`, `--plain`, `--quiet`
- `oxide-sloc init [PATH]` — generate a starter `.oxide-sloc.toml` with all options documented; `--force` to overwrite

**CLI improvements**
- Short flag aliases: `-j` (`--json-out`), `-H` (`--html-out`), `-c` (`--csv-out`), `-x` (`--xlsx-out`), `-q` (`--quiet`)
- `--open` on `analyze` and `report` — auto-opens the generated HTML in the system browser
- `--quiet` / `-q` — suppress all output except errors (useful in CI pipelines)
- `--fail-on-warnings` — exit with code 2 when warnings are present
- `--fail-below <N>` — exit with code 3 when code lines fall below threshold
- Colored terminal output when stdout is a TTY; suppressed by `NO_COLOR` env var or `--plain`
- Improved per-file and language-breakdown table formatting with aligned columns

**Release pipeline**
- `SHA256SUMS.txt` now included in every GitHub Release alongside the binaries

**Documentation**
- `CONTRIBUTING.md` — development workflow, vendor regeneration, PR checklist
- `SECURITY.md` — vulnerability disclosure policy and scope
- `CHANGELOG.md` (this file)

**Shebang detection extended** to Ruby, Perl, PHP, and Node.js scripts

---

## [0.2.0-beta.4] — 2026-04-24

### Changed
- Removed security commentary from source; pinned CI GitHub Actions to specific SHAs
- Applied `rustfmt` to `sloc-report` and `sloc-web` to pass CI format check
- Refreshed dist bundles (`[skip ci]`)

---

## [0.2.0-beta.3] — earlier

### Added
- `oxide-sloc serve --server` mode (binds `0.0.0.0`, suppresses browser auto-open)
- `oxide-sloc send` — SMTP and webhook delivery of saved JSON results
- Git metadata capture (`git_branch`, `git_commit_short/long`, `git_commit_author`, `git_tags`)
- Submodule breakdown (`--submodule-breakdown`)
- Delta computation in `sloc-core` (compare two `AnalysisRun` JSON files)
- Scan history/registry in `sloc-core` for the web UI
- PDF export via headless Chromium (`write_pdf_from_html`)
- Self-contained HTML report with light/dark theme toggle
- `run.sh` cross-platform launcher

### Fixed
- UTF-16 LE/BE and Windows-1252 encoding fallback during file discovery

---

## [0.1.0] — initial release

- CLI with `analyze`, `report`, `serve` subcommands
- JSON and HTML output formats
- 11 languages: C, C++, C#, Go, Java, JavaScript, Python, Rust, Shell, PowerShell, TypeScript
- Lexical state-machine analyzer with Python docstring classification
- Tree-sitter adapter scaffold (C and Python, behind `tree-sitter` feature flag)
- Axum web UI on `127.0.0.1:4317`
- GitHub Actions CI (fmt + clippy + build + test + smoke tests)
- Cross-platform release builds (Linux x86_64 musl, Windows x86_64 MSVC, macOS x86_64 + arm64)
