// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>

//! Standalone CI bundle: one self-contained directory an adopting pipeline (Jenkins, GitLab, …)
//! can archive and publish without copying any oxide-sloc pipeline helper scripts.
//!
//! [`write_ci_bundle`] materializes, under one output directory:
//!
//! * `result.json` — the full [`AnalysisRun`] model (also the local-import / compare artifact);
//! * `report.html` / `report.pdf` / `report.csv` / `report.xlsx` — the standard report surfaces
//!   (this `report.html` is the standalone *`OxideSLOC_HTML_Report_*`* entry page);
//! * `scan-config.json` — the effective scan configuration;
//! * `summary.json` — a stable, machine-readable [`CiSummary`] payload for CI build cards;
//! * `index.html` — a self-contained CI dashboard (the standalone *`OxideSLOC_CI_Report_*`*
//!   entry page) with test counts, imported coverage, and links to every artifact;
//! * `csp/report.html` + `csp/assets/*` — a Jenkins-CSP-compatible copy of the HTML report with
//!   its inline `<style>` / `<script>` blocks externalized into separate asset files;
//! * `import/` — a local-import / compare bundle (`run.json`, `summary.json`, `README.txt`).
//!
//! The payload contains no credentials and performs no network access.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sloc_core::{AnalysisRun, CoverageDiagnostics, write_json};

use crate::{render_html, write_csv, write_pdf_from_run, write_xlsx};

/// Stable schema identifier stamped into every [`CiSummary`]. Bump the suffix on a
/// breaking field change so consumers can branch on it.
pub const CI_SUMMARY_SCHEMA: &str = "oxide-sloc.ci-summary/v1";

/// Code/comment/blank/file totals for a CI card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiTotals {
    pub files_analyzed: u64,
    pub code_lines: u64,
    pub comment_lines: u64,
    pub blank_lines: u64,
    pub total_physical_lines: u64,
}

/// Compact style-metrics projection for a CI card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiStyle {
    pub files_analyzed: u32,
    pub common_indent_style: String,
    pub line_col_compliant_pct: u8,
    pub col_threshold: u16,
}

/// One language-breakdown row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiLanguage {
    pub language: String,
    pub files: u64,
    pub code_lines: u64,
    pub comment_lines: u64,
    pub blank_lines: u64,
}

/// Lexically detected test signals. These are *source-text detections*, NOT executed test
/// results — a CI card should present them separately from JUnit/CTest pass-fail counts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiLexicalTests {
    /// Explicit reminder that these numbers come from lexical scanning, not test execution.
    pub note: String,
    pub test_functions: u64,
    pub assertions: u64,
    pub suites: u64,
}

/// Imported coverage totals. `source` is always `"imported"` — oxide-sloc never executes tests
/// or instruments code; coverage is read from a supplied LCOV/Cobertura/JaCoCo file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiCoverage {
    pub source: String,
    pub lines_found: u64,
    pub lines_hit: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_pct: Option<f64>,
    pub functions_found: u64,
    pub functions_hit: u64,
    pub branches_found: u64,
    pub branches_hit: u64,
}

/// Stable, machine-readable summary payload for CI build cards. Lets adopters render a Jenkins /
/// GitLab build summary straight from JSON without parsing the HTML report or depending on any
/// oxide-sloc pipeline helper script.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiSummary {
    /// Schema identifier, e.g. `oxide-sloc.ci-summary/v1`.
    pub schema: String,
    pub report_title: String,
    pub scanner: String,
    pub scanner_version: String,
    pub generated_utc: String,
    pub totals: CiTotals,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<CiStyle>,
    pub languages: Vec<CiLanguage>,
    pub lexical_tests: CiLexicalTests,
    /// Imported coverage, present only when a coverage file matched scanned sources.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage: Option<CiCoverage>,
    /// Coverage-ingestion diagnostics, present only when a coverage file was supplied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage_diagnostics: Option<CoverageDiagnostics>,
}

#[allow(clippy::cast_precision_loss)]
fn pct(hit: u64, found: u64) -> Option<f64> {
    if found == 0 {
        None
    } else {
        Some((hit as f64 / found as f64) * 100.0)
    }
}

/// Build the stable CI summary payload from an analysis run.
#[must_use]
pub fn build_ci_summary(run: &AnalysisRun) -> CiSummary {
    let totals = &run.summary_totals;
    let style = run.style_summary.as_ref().map(|ss| CiStyle {
        files_analyzed: ss.files_analyzed,
        common_indent_style: ss.common_indent_style.clone(),
        line_col_compliant_pct: ss.line_col_compliant_pct,
        col_threshold: ss.col_threshold,
    });
    let languages = run
        .totals_by_language
        .iter()
        .map(|l| CiLanguage {
            language: l.language.display_name().to_string(),
            files: l.files,
            code_lines: l.code_lines,
            comment_lines: l.comment_lines,
            blank_lines: l.blank_lines,
        })
        .collect();
    let coverage = if totals.coverage_lines_found > 0 {
        Some(CiCoverage {
            source: "imported".to_string(),
            lines_found: totals.coverage_lines_found,
            lines_hit: totals.coverage_lines_hit,
            line_pct: pct(totals.coverage_lines_hit, totals.coverage_lines_found),
            functions_found: totals.coverage_functions_found,
            functions_hit: totals.coverage_functions_hit,
            branches_found: totals.coverage_branches_found,
            branches_hit: totals.coverage_branches_hit,
        })
    } else {
        None
    };
    CiSummary {
        schema: CI_SUMMARY_SCHEMA.to_string(),
        report_title: run.effective_configuration.reporting.report_title.clone(),
        scanner: run.tool.name.clone(),
        scanner_version: run.tool.version.clone(),
        generated_utc: run.tool.timestamp_utc.to_rfc3339(),
        totals: CiTotals {
            files_analyzed: totals.files_analyzed,
            code_lines: totals.code_lines,
            comment_lines: totals.comment_lines,
            blank_lines: totals.blank_lines,
            total_physical_lines: totals.total_physical_lines,
        },
        style,
        languages,
        lexical_tests: CiLexicalTests {
            note: "Lexical source detections, not executed test results. Compare against your \
                   JUnit/CTest run for pass-fail counts."
                .to_string(),
            test_functions: totals.test_count,
            assertions: totals.test_assertion_count,
            suites: totals.test_suite_count,
        },
        coverage,
        coverage_diagnostics: run.coverage_diagnostics.clone(),
    }
}

/// Serialize the CI summary payload to `path` as pretty JSON.
///
/// # Errors
/// Returns an error if serialization or file I/O fails.
pub fn write_ci_summary(run: &AnalysisRun, path: &Path) -> Result<()> {
    let summary = build_ci_summary(run);
    let json =
        serde_json::to_string_pretty(&summary).context("failed to serialize CI summary payload")?;
    fs::write(path, json)
        .with_context(|| format!("failed to write CI summary to {}", path.display()))
}

/// A Jenkins-CSP-compatible copy of an HTML report: the main document with inline `<style>` and
/// inline `<script>` blocks replaced by references to externalized asset files.
#[derive(Debug, Clone)]
pub struct CspBundle {
    /// The rewritten HTML document.
    pub html: String,
    /// `(relative asset path, contents)` pairs, e.g. `("assets/report.css", "…")`.
    pub assets: Vec<(String, String)>,
}

fn find_ci(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    haystack
        .get(from..)
        .and_then(|s| s.to_ascii_lowercase().find(needle))
        .map(|p| from + p)
}

/// Externalize the inline `<style>` and inline `<script>` blocks of a rendered report so the page
/// satisfies a Content-Security-Policy that forbids inline scripts/styles (e.g. Jenkins HTML
/// Publisher). Inline `<style>` blocks are concatenated into `assets/report.css` and linked from
/// `<head>`; each inline `<script>` is written to `assets/report-NNN.js` and referenced in place,
/// preserving execution order. `<script src=…>` tags are passed through with their `nonce`
/// stripped (external scripts need no nonce under `script-src 'self'`).
#[must_use]
pub fn externalize_report_assets(html: &str) -> CspBundle {
    let mut assets: Vec<(String, String)> = Vec::new();
    let mut css = String::new();
    let mut out = String::with_capacity(html.len());
    let mut js_index = 0usize;
    let mut i = 0usize;

    while i < html.len() {
        let next_style = find_ci(html, "<style", i);
        let next_script = find_ci(html, "<script", i);
        let (tag_start, is_style) = match (next_style, next_script) {
            (Some(s), Some(c)) => {
                if s < c {
                    (s, true)
                } else {
                    (c, false)
                }
            }
            (Some(s), None) => (s, true),
            (None, Some(c)) => (c, false),
            (None, None) => {
                out.push_str(&html[i..]);
                break;
            }
        };
        out.push_str(&html[i..tag_start]);

        // End of the opening tag.
        let Some(gt) = html[tag_start..].find('>').map(|p| tag_start + p + 1) else {
            out.push_str(&html[tag_start..]);
            break;
        };
        let open_tag = &html[tag_start..gt];

        if is_style {
            let close = "</style>";
            let Some(close_pos) = find_ci(html, close, gt) else {
                out.push_str(&html[tag_start..]);
                break;
            };
            css.push_str(&html[gt..close_pos]);
            css.push('\n');
            i = close_pos + close.len();
        } else if open_tag.to_ascii_lowercase().contains("src=") {
            // Already-external script: keep it, just drop any nonce.
            out.push_str(&strip_nonce(open_tag));
            i = gt;
        } else {
            let close = "</script>";
            let Some(close_pos) = find_ci(html, close, gt) else {
                out.push_str(&html[tag_start..]);
                break;
            };
            js_index += 1;
            let name = format!("assets/report-{js_index:03}.js");
            assets.push((name.clone(), html[gt..close_pos].to_string()));
            out.push_str(&format!("<script src=\"{name}\"></script>"));
            i = close_pos + close.len();
        }
    }

    if !css.is_empty() {
        assets.insert(0, ("assets/report.css".to_string(), css));
        let link = "<link rel=\"stylesheet\" href=\"assets/report.css\">\n";
        if let Some(head_close) = find_ci(&out, "</head>", 0) {
            out.insert_str(head_close, link);
        } else {
            out.insert_str(0, link);
        }
    }

    CspBundle { html: out, assets }
}

/// Remove a `nonce="…"` attribute from a single opening tag.
fn strip_nonce(tag: &str) -> String {
    let lower = tag.to_ascii_lowercase();
    let Some(start) = lower.find("nonce=") else {
        return tag.to_string();
    };
    // Attribute value is quoted; find the matching closing quote.
    let after = start + "nonce=".len();
    let Some(quote) = tag[after..].chars().next() else {
        return tag.to_string();
    };
    if quote != '"' && quote != '\'' {
        return tag.to_string();
    }
    let val_start = after + 1;
    let Some(end_rel) = tag[val_start..].find(quote) else {
        return tag.to_string();
    };
    let end = val_start + end_rel + 1;
    // Also swallow one leading space so we don't leave a double space.
    let cut_start = if start > 0 && tag.as_bytes()[start - 1] == b' ' {
        start - 1
    } else {
        start
    };
    format!("{}{}", &tag[..cut_start], &tag[end..])
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Render the self-contained CI dashboard (`index.html` / `OxideSLOC_CI_Report_*`). Links are
/// relative to the bundle directory. The dashboard separates lexical test detections, imported
/// coverage, and (absent) executed-test results so a build card never conflates them.
#[must_use]
pub fn render_ci_dashboard_html(run: &AnalysisRun) -> String {
    let summary = build_ci_summary(run);
    let version = env!("CARGO_PKG_VERSION");
    let t = &summary.totals;

    let cov_block = if let Some(c) = &summary.coverage {
        let pct = c
            .line_pct
            .map_or_else(|| "—".to_string(), |p| format!("{p:.1}%"));
        format!(
            "<div class=\"card\"><h2>Imported coverage</h2>\
             <p class=\"big\">{pct}</p>\
             <p class=\"muted\">Lines {hit}/{found} · imported from a supplied coverage file \
             (not executed by oxide-sloc)</p></div>",
            hit = c.lines_hit,
            found = c.lines_found,
        )
    } else if let Some(d) = &summary.coverage_diagnostics {
        format!(
            "<div class=\"card warn\"><h2>Imported coverage</h2>\
             <p class=\"big\">—</p>\
             <p class=\"muted\">A coverage file was supplied but produced no displayed coverage. \
             Ingestion status: <strong>{status:?}</strong>. Resolved path: <code>{path}</code>. \
             This is an oxide-sloc ingestion result, independent of your build/test config.</p></div>",
            status = d.outcome,
            path = esc(&d.resolved_path),
        )
    } else {
        "<div class=\"card\"><h2>Imported coverage</h2><p class=\"big\">—</p>\
         <p class=\"muted\">No coverage file supplied. Pass <code>--coverage-file &lt;path&gt;</code> \
         (LCOV / Cobertura / JaCoCo) to import coverage.</p></div>"
            .to_string()
    };

    let lang_rows = summary
        .languages
        .iter()
        .map(|l| {
            format!(
                "<tr><td>{lang}</td><td>{files}</td><td>{code}</td><td>{comment}</td><td>{blank}</td></tr>",
                lang = esc(&l.language),
                files = l.files,
                code = l.code_lines,
                comment = l.comment_lines,
                blank = l.blank_lines,
            )
        })
        .collect::<String>();

    format!(
        "<!DOCTYPE html>\n<html lang=\"en\"><head>\n<meta charset=\"utf-8\">\n\
    <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
    <title>oxide-sloc CI Report — {title}</title>\n\
    <link rel=\"stylesheet\" href=\"assets/dashboard.css\">\n</head>\n<body>\n\
    <header><h1>{title}</h1>\
    <p class=\"muted\">oxide-sloc v{version} · scanner {scanner} v{sver} · generated {gen}</p></header>\n\
    <section class=\"grid\">\n\
    <div class=\"card\"><h2>Code lines</h2><p class=\"big\">{code}</p>\
    <p class=\"muted\">{files} files · {phys} physical lines</p></div>\n\
    <div class=\"card\"><h2>Comments</h2><p class=\"big\">{comment}</p>\
    <p class=\"muted\">{blank} blank lines</p></div>\n\
    <div class=\"card\"><h2>Lexical tests</h2><p class=\"big\">{tfn}</p>\
    <p class=\"muted\">{asrt} assertions · {suite} suites · lexical detections, NOT executed \
    test results</p></div>\n\
    {cov_block}\n\
    </section>\n\
    <section class=\"note\">\n\
    <p><strong>How to read this card.</strong> oxide-sloc reports <em>lexical</em> test detections \
    (source-text matches) and <em>imported</em> coverage (read from a file you supply). It does not \
    run tests. Your JUnit/CTest results and their pass-fail trend are published separately by your \
    pipeline.</p>\n\
    </section>\n\
    <section><h2>Languages</h2>\n\
    <table><thead><tr><th>Language</th><th>Files</th><th>Code</th><th>Comment</th><th>Blank</th></tr>\
    </thead><tbody>{lang_rows}</tbody></table>\n</section>\n\
    <section><h2>Artifacts</h2>\n<ul class=\"links\">\n\
    <li><a href=\"report.html\">HTML report</a> — full interactive report (OxideSLOC_HTML_Report)</li>\n\
    <li><a href=\"report.pdf\">PDF report</a></li>\n\
    <li><a href=\"report.csv\">CSV</a> · <a href=\"report.xlsx\">XLSX</a></li>\n\
    <li><a href=\"result.json\">result.json</a> — full model / local-import &amp; compare artifact</li>\n\
    <li><a href=\"summary.json\">summary.json</a> — machine-readable CI summary ({schema})</li>\n\
    <li><a href=\"scan-config.json\">scan-config.json</a></li>\n\
    <li><a href=\"csp/report.html\">CSP-compatible HTML report</a> — externalized CSS/JS</li>\n\
    <li><a href=\"import/\">import/</a> — local-import &amp; compare bundle</li>\n\
    </ul>\n</section>\n\
    <footer class=\"muted\">oxide-sloc v{version} — local code metrics workbench · \
    <a href=\"https://github.com/oxide-sloc/oxide-sloc\">GitHub</a> · AGPL-3.0-or-later</footer>\n\
    </body></html>\n",
        title = esc(&summary.report_title),
        version = version,
        scanner = esc(&summary.scanner),
        sver = esc(&summary.scanner_version),
        gen = esc(&summary.generated_utc),
        code = t.code_lines,
        files = t.files_analyzed,
        phys = t.total_physical_lines,
        comment = t.comment_lines,
        blank = t.blank_lines,
        tfn = summary.lexical_tests.test_functions,
        asrt = summary.lexical_tests.assertions,
        suite = summary.lexical_tests.suites,
        cov_block = cov_block,
        lang_rows = lang_rows,
        schema = esc(&summary.schema),
    )
}

const DASHBOARD_CSS: &str = r#":root{--oxide:#c45c10;--line:#e6d0bf;--muted:#7b675b;--text:#43342d;--surface:#fff;--bg:#faf6f2}
*{box-sizing:border-box}
body{font-family:Inter,ui-sans-serif,system-ui,-apple-system,"Segoe UI",Roboto,sans-serif;margin:0;background:var(--bg);color:var(--text)}
header{padding:28px 32px;border-bottom:1px solid var(--line)}
h1{margin:0 0 6px;font-size:24px;color:var(--oxide)}
h2{font-size:13px;text-transform:uppercase;letter-spacing:.06em;color:var(--muted);margin:0 0 8px}
.muted{color:var(--muted);font-size:13px}
.grid{display:grid;grid-template-columns:repeat(4,1fr);gap:16px;padding:24px 32px}
@media(max-width:860px){.grid{grid-template-columns:repeat(2,1fr)}}
.card{background:var(--surface);border:1px solid var(--line);border-radius:12px;padding:16px}
.card.warn{border-color:#d4a017;background:#fff9ec}
.big{font-size:28px;font-weight:900;color:var(--oxide);margin:4px 0}
section{padding:0 32px 24px}
.note p{background:var(--surface);border:1px solid var(--line);border-radius:12px;padding:14px 16px;font-size:13px}
table{width:100%;border-collapse:collapse;background:var(--surface);border:1px solid var(--line);border-radius:12px;overflow:hidden}
th,td{text-align:left;padding:8px 12px;border-bottom:1px solid var(--line);font-size:13px}
th{background:#f3e7da;text-transform:uppercase;font-size:11px;letter-spacing:.05em;color:var(--muted)}
td:not(:first-child),th:not(:first-child){text-align:right;font-variant-numeric:tabular-nums}
.links{list-style:none;padding:0;margin:0}
.links li{padding:6px 0;border-bottom:1px solid var(--line);font-size:14px}
.links a{color:var(--oxide);font-weight:700;text-decoration:none}
.links a:hover{text-decoration:underline}
footer{padding:20px 32px;border-top:1px solid var(--line);font-size:12px}
footer a{color:var(--oxide)}
"#;

/// Paths of the artifacts written by [`write_ci_bundle`], relative to the bundle directory.
#[derive(Debug, Clone)]
pub struct CiBundlePaths {
    pub dir: PathBuf,
    pub dashboard_html: PathBuf,
    pub report_html: PathBuf,
    pub summary_json: PathBuf,
}

/// Write the complete standalone CI bundle into `dir` (created if missing). See the module docs
/// for the full artifact list. Returns the key entry-page paths.
///
/// # Errors
/// Returns an error if any artifact fails to render or write.
pub fn write_ci_bundle(run: &AnalysisRun, dir: &Path) -> Result<CiBundlePaths> {
    fs::create_dir_all(dir).with_context(|| format!("creating CI bundle dir {}", dir.display()))?;

    // Core report surfaces.
    let result_json = dir.join("result.json");
    write_json(run, &result_json)?;
    let report_html = dir.join("report.html");
    let report_pdf = dir.join("report.pdf");
    crate::write_html_with_pdf_link(run, &report_html, Some(&report_pdf))?;
    write_pdf_from_run(run, &report_pdf)?;
    write_csv(run, &dir.join("report.csv"))?;
    write_xlsx(run, &dir.join("report.xlsx"))?;

    // Effective scan configuration.
    let scan_config = serde_json::to_string_pretty(&run.effective_configuration)
        .context("failed to serialize effective configuration")?;
    fs::write(dir.join("scan-config.json"), scan_config)
        .context("failed to write scan-config.json")?;

    // Machine-readable CI summary payload.
    let summary_json = dir.join("summary.json");
    write_ci_summary(run, &summary_json)?;

    // CSP-compatible HTML bundle (externalized CSS/JS).
    let csp_dir = dir.join("csp");
    let csp_assets = csp_dir.join("assets");
    fs::create_dir_all(&csp_assets)
        .with_context(|| format!("creating CSP assets dir {}", csp_assets.display()))?;
    let plain_html = render_html(run)?;
    let csp = externalize_report_assets(&plain_html);
    fs::write(csp_dir.join("report.html"), &csp.html).context("failed to write csp/report.html")?;
    for (rel, contents) in &csp.assets {
        let path = csp_dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&path, contents)
            .with_context(|| format!("failed to write {}", path.display()))?;
    }

    // CI dashboard (index.html + its stylesheet).
    fs::create_dir_all(dir.join("assets")).context("creating dashboard assets dir")?;
    fs::write(dir.join("assets").join("dashboard.css"), DASHBOARD_CSS)
        .context("failed to write dashboard stylesheet")?;
    let dashboard_html = dir.join("index.html");
    fs::write(&dashboard_html, render_ci_dashboard_html(run))
        .context("failed to write CI dashboard index.html")?;

    // Local-import / compare bundle.
    let import_dir = dir.join("import");
    fs::create_dir_all(&import_dir)
        .with_context(|| format!("creating import dir {}", import_dir.display()))?;
    write_json(run, &import_dir.join("run.json"))?;
    write_ci_summary(run, &import_dir.join("summary.json"))?;
    fs::write(import_dir.join("README.txt"), IMPORT_README)
        .context("failed to write import README")?;

    Ok(CiBundlePaths {
        dir: dir.to_path_buf(),
        dashboard_html,
        report_html,
        summary_json,
    })
}

const IMPORT_README: &str = "oxide-sloc local-import / compare bundle\n\
=======================================\n\n\
This directory is a self-contained, offline import/compare artifact. It needs no oxide-sloc\n\
source tree, Rust toolchain, or network access to re-use.\n\n\
Files:\n\
  run.json      Full AnalysisRun model for this scan (the importable artifact).\n\
  summary.json  Stable machine-readable CI summary payload.\n\n\
Re-render the report from the imported run:\n\
  oxide-sloc report run.json --html-out report.html --pdf-out report.pdf\n\n\
Compare this run against a previous run's run.json:\n\
  oxide-sloc diff previous-run.json run.json\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn externalize_pulls_out_style_and_script() {
        let html = "<html><head><style>body{color:red}</style></head>\
                    <body><script nonce=\"abc\">var x=1;</script>\
                    <script src=\"ext.js\" nonce=\"z\"></script></body></html>";
        let bundle = externalize_report_assets(html);
        // No inline <style> or inline-content <script> left in the document.
        assert!(!bundle.html.contains("<style"));
        assert!(bundle.html.contains("assets/report.css"));
        assert!(bundle.html.contains("src=\"assets/report-001.js\""));
        assert!(bundle.html.contains("src=\"ext.js\""));
        // Nonce stripped from the passthrough external script.
        assert!(!bundle.html.contains("nonce="));
        // CSS + one inline JS asset were captured.
        assert!(
            bundle
                .assets
                .iter()
                .any(|(n, c)| n == "assets/report.css" && c.contains("color:red"))
        );
        assert!(
            bundle
                .assets
                .iter()
                .any(|(n, c)| n == "assets/report-001.js" && c.contains("var x=1;"))
        );
    }

    #[test]
    fn strip_nonce_removes_attr_and_space() {
        assert_eq!(
            strip_nonce("<script nonce=\"abc\" src=\"x\">"),
            "<script src=\"x\">"
        );
        assert_eq!(strip_nonce("<script src=\"x\">"), "<script src=\"x\">");
    }
}
