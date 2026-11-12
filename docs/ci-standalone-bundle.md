# Standalone CI bundle (`analyze --ci-bundle`)

`oxide-sloc analyze --ci-bundle --out-dir <dir>` emits one self-contained directory that a CI
pipeline (Jenkins, GitLab, GitHub Actions, …) can archive and publish **without copying any
oxide-sloc pipeline helper scripts**. One command produces the complete artifact set, two
standalone entry pages, a machine-readable summary, a Jenkins-CSP-compatible report, and a
local-import/compare bundle.

```bash
oxide-sloc analyze ./my-project \
  --coverage-file coverage/lcov.info \
  --report-title "My Project — main" \
  --ci-bundle --out-dir out/oxide-sloc
```

## What it writes

| Path | Purpose |
|---|---|
| `result.json` | Full `AnalysisRun` model; also the local-import / compare artifact. |
| `report.html` | Full interactive HTML report (the standalone **`OxideSLOC_HTML_Report_*`** entry page). Its **View PDF** button opens the sibling `report.pdf` directly, even offline / from Jenkins HTML Publisher. |
| `report.pdf` | PDF report (pure-Rust writer — **no browser required**). |
| `report.csv`, `report.xlsx` | Tabular exports. |
| `scan-config.json` | The effective scan configuration. |
| `summary.json` | Stable, machine-readable CI summary payload (see below). |
| `index.html` | Self-contained CI dashboard (the standalone **`OxideSLOC_CI_Report_*`** entry page) with totals, lexical test counts, imported coverage, and links to every artifact. |
| `csp/report.html` + `csp/assets/*` | A Jenkins-CSP-compatible copy of the HTML report with inline `<style>` / `<script>` externalized into `assets/report.css` and `assets/report-NNN.js`. |
| `import/` | Local-import / compare bundle: `run.json`, `summary.json`, and a `README.txt` with the re-render / compare commands. |

Everything is relative-linked, so the whole directory can be moved, zipped, or served as static
files. No credentials are written and nothing performs network access.

## Jenkins HTML Publisher

Publish the two standalone entry pages directly — no helper scripts:

```groovy
publishHTML(target: [
  reportDir: 'out/oxide-sloc', reportFiles: 'index.html',
  reportName: 'OxideSLOC CI Report', keepAll: true,
  allowMissing: false, alwaysLinkToLastBuild: true])

publishHTML(target: [
  reportDir: 'out/oxide-sloc', reportFiles: 'report.html',
  reportName: 'OxideSLOC HTML Report', keepAll: true])
```

If your Jenkins controller runs a strict resource Content-Security-Policy that blocks inline
scripts/styles, publish `csp/report.html` instead of `report.html` — its CSS/JS are externalized
into `csp/assets/` and load under `script-src 'self'` / `style-src 'self'`.

## Machine-readable summary (`summary.json`)

`summary.json` is a stable payload (`"schema": "oxide-sloc.ci-summary/v1"`) for rendering a build
card without parsing the HTML report or depending on any oxide-sloc helper. Shape:

```jsonc
{
  "schema": "oxide-sloc.ci-summary/v1",
  "report_title": "My Project — main",
  "scanner": "sloc",
  "scanner_version": "1.6.28",
  "generated_utc": "2026-11-12T04:35:37+00:00",
  "totals": { "files_analyzed": 42, "code_lines": 9123, "comment_lines": 1200,
              "blank_lines": 800, "total_physical_lines": 11123 },
  "style": { "files_analyzed": 42, "common_indent_style": "4-Space",
             "line_col_compliant_pct": 88, "col_threshold": 80 },
  "languages": [ { "language": "C++", "files": 30, "code_lines": 7000,
                   "comment_lines": 900, "blank_lines": 600 } ],
  "lexical_tests": {
    "note": "Lexical source detections, not executed test results. Compare against your JUnit/CTest run for pass-fail counts.",
    "test_functions": 66, "assertions": 233, "suites": 12
  },
  "coverage": { "source": "imported", "lines_found": 5000, "lines_hit": 4100,
                "line_pct": 82.0, "functions_found": 300, "functions_hit": 260,
                "branches_found": 0, "branches_hit": 0 },
  "coverage_diagnostics": { "resolved_path": "coverage/lcov.info", "outcome": "matched",
                            "entries_parsed": 30, "files_matched": 29, "files_unmatched": 1 }
}
```

### Lexical tests vs. executed tests vs. imported coverage

The payload (and both HTML pages) keep three distinct signals separate so a build card never
conflates them:

- **`lexical_tests`** — source-text detections (e.g. GoogleTest `TEST(...)`, `EXPECT_*`). These
  are *not* executed test results. Your JUnit/CTest run and its pass-fail trend are published
  separately by your pipeline.
- **`coverage`** — coverage **imported** from a file you supply (`--coverage-file`, accepting
  LCOV, Cobertura XML, or JaCoCo XML). oxide-sloc never runs tests or instruments code.
- **`coverage_diagnostics`** — present whenever a coverage file was supplied, so you can tell an
  oxide-sloc *ingestion* problem from a *build/test* configuration problem. `outcome` is one of
  `read_error`, `parsed_no_entries`, `no_match`, or `matched`, alongside the resolved path and the
  matched/unmatched file counts.

## Local-import / compare bundle (`import/`)

`import/run.json` is a self-contained, offline artifact. On any machine (no oxide-sloc source
tree, Rust toolchain, or network needed):

```bash
# Re-render the report from the imported run
oxide-sloc report import/run.json --html-out report.html --pdf-out report.pdf

# Compare this run against a previous run's run.json
oxide-sloc diff previous-run.json import/run.json
```

## Related flags

- `--all-formats --out-dir <dir>` writes just the six core artifacts (`result.json`,
  `report.html`, `report.pdf`, `report.csv`, `report.xlsx`, `scan-config.json`). `--ci-bundle` is
  a superset that adds `summary.json`, `index.html`, the `csp/` bundle, and `import/`.
- `--report-title "<text>"` overrides the report title from both the base TOML and any
  `--profile`. CLI arguments always win.
