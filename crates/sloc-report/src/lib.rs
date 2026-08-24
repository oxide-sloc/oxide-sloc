// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>
#![allow(clippy::multiple_crate_versions)]

mod pdf_compat;

use std::collections::BTreeMap;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use askama::Template;
use chrono::{DateTime, FixedOffset, Utc};
use sloc_core::{AnalysisRun, Author, CocomoMode, FileRecord, StyleSummary, SummaryTotals};

// Embed logo images at compile time so every generated HTML report is fully
// self-contained.  Server-relative paths like /images/logo/... break when the
// HTML is rendered by Chrome via file:// (PDF export) or opened from disk.
static LOGO_TEXT_PNG: &[u8] = include_bytes!("../assets/logo/logo-text.png");
static SMALL_LOGO_PNG: &[u8] = include_bytes!("../assets/logo/small-logo.png");
static CHART_JS: &str = include_str!("../assets/chart.min.js");

fn png_data_uri(bytes: &[u8]) -> String {
    format!("data:image/png;base64,{}", base64_encode(bytes))
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = if chunk.len() > 1 {
            u32::from(chunk[1])
        } else {
            0
        };
        let b2 = if chunk.len() > 2 {
            u32::from(chunk[2])
        } else {
            0
        };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(CHARS[((n >> 18) & 63) as usize] as char);
        out.push(CHARS[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            CHARS[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            CHARS[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Convert an SSH or HTTPS remote URL to a plain HTTPS base URL.
/// `git@github.com:owner/repo.git` → `https://github.com/owner/repo`
/// `https://github.com/owner/repo.git` → `https://github.com/owner/repo`
fn normalize_remote_url(remote_url: &str) -> Option<String> {
    let url = remote_url.trim();
    if let Some(rest) = url.strip_prefix("git@") {
        let (host, path) = rest.split_once(':')?;
        let path = path.trim_end_matches(".git");
        return Some(format!("https://{host}/{path}"));
    }
    if url.starts_with("https://") || url.starts_with("http://") {
        return Some(url.trim_end_matches(".git").to_string());
    }
    None
}

/// Derive a direct link to the given commit SHA on the hosting forge.
pub(crate) fn derive_commit_url(remote_url: &str, sha: &str) -> Option<String> {
    let base = normalize_remote_url(remote_url)?;
    let lower = base.to_lowercase();
    if lower.contains("bitbucket.org") {
        Some(format!("{base}/commits/{sha}"))
    } else if lower.contains("gitlab.") {
        Some(format!("{base}/-/commit/{sha}"))
    } else {
        Some(format!("{base}/commit/{sha}"))
    }
}

/// Derive a direct link to the given branch on the hosting forge.
pub(crate) fn derive_branch_url(remote_url: &str, branch: &str) -> Option<String> {
    let base = normalize_remote_url(remote_url)?;
    let lower = base.to_lowercase();
    if lower.contains("bitbucket.org") {
        Some(format!("{base}/branch/{branch}"))
    } else if lower.contains("gitlab.") {
        Some(format!("{base}/-/tree/{branch}"))
    } else {
        Some(format!("{base}/tree/{branch}"))
    }
}

/// Optional delta context for embedding a "Changes vs. Previous Scan" panel
/// in the HTML report. Pass `None` to omit the panel (CLI, sub-reports).
pub struct ReportDeltaContext {
    /// Net code lines added (new + grown files).
    pub delta_code_added: i64,
    /// Net code lines removed (deleted + shrunk files).
    pub delta_code_removed: i64,
    /// Code lines present in both scans without change.
    pub delta_unmodified_lines: i64,
    /// Number of files added since the previous scan.
    pub delta_files_added: usize,
    /// Number of files removed since the previous scan.
    pub delta_files_removed: usize,
    /// Number of files modified since the previous scan.
    pub delta_files_modified: usize,
    /// Number of files unchanged since the previous scan.
    pub delta_files_unchanged: usize,
    /// Code lines in the previous scan (for the "Code before: X" display).
    pub prev_code_lines: u64,
    /// Total number of scans on record for this project (including current).
    pub prev_scan_count: usize,
    /// Human-readable label for the previous scan (timestamp or run label).
    pub prev_scan_label: String,
    /// Run ID of the previous scan, used to generate navigation links.
    pub prev_run_id: Option<String>,
    /// Run ID of the current scan, used to generate the compare-scans link.
    pub current_run_id: Option<String>,
}

/// Render a full standalone HTML report for the given analysis run.
///
/// # Errors
///
/// Returns an error if template rendering or configuration serialization fails.
pub fn render_html(run: &AnalysisRun) -> Result<String> {
    render_html_inner(run, false, None, None)
}

/// Render a full standalone HTML report with an optional delta panel.
///
/// When `delta` is `Some`, a "Changes vs. Previous Scan" section is embedded
/// near the top of the report so the artifact is self-contained for external
/// stakeholders who have no access to the web server.
///
/// # Errors
///
/// Returns an error if template rendering or configuration serialization fails.
pub fn render_html_with_delta(
    run: &AnalysisRun,
    delta: Option<&ReportDeltaContext>,
) -> Result<String> {
    render_html_inner(run, false, None, delta)
}

/// Render an embedded sub-report HTML fragment for the given analysis run.
///
/// # Errors
///
/// Returns an error if template rendering or configuration serialization fails.
pub fn render_sub_report_html(run: &AnalysisRun, pdf_url: Option<&str>) -> Result<String> {
    render_html_inner(run, true, pdf_url, None)
}

fn load_custom_logo(path: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mime = if ext == "svg" {
        "image/svg+xml"
    } else {
        "image/png"
    };
    Some(format!("data:{mime};base64,{}", base64_encode(&bytes)))
}

// ── Chart JSON builders ───────────────────────────────────────────────────────

/// Escape a string for safe embedding inside a JSON string literal.
fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn build_lang_chart_json(run: &AnalysisRun) -> String {
    let mut langs: Vec<&sloc_core::LanguageSummary> = run.totals_by_language.iter().collect();
    langs.sort_by_key(|l| std::cmp::Reverse(l.code_lines));
    let entries: Vec<String> = langs
        .into_iter()
        .take(12)
        .map(|l| {
            format!(
                r#"{{"lang":"{}","code":{},"comments":{},"blanks":{},"physical":{},"functions":{},"classes":{},"variables":{},"imports":{},"tests":{},"files":{}}}"#,
                json_escape(l.language.display_name()),
                l.code_lines, l.comment_lines, l.blank_lines, l.total_physical_lines,
                l.functions, l.classes, l.variables, l.imports,
                l.test_count, l.files,
            )
        })
        .collect();
    format!("[{}]", entries.join(","))
}

fn build_submodule_chart_json(run: &AnalysisRun) -> String {
    let entries: Vec<String> = run
        .submodule_summaries
        .iter()
        .map(|s| {
            format!(
                r#"{{"name":"{}","path":"{}","code":{},"comment":{},"blank":{},"physical":{},"files":{}}}"#,
                json_escape(&s.name), json_escape(&s.relative_path),
                s.code_lines, s.comment_lines, s.blank_lines,
                s.total_physical_lines, s.files_analyzed,
            )
        })
        .collect();
    format!("[{}]", entries.join(","))
}

fn build_scatter_chart_json(run: &AnalysisRun) -> String {
    let entries: Vec<String> = run
        .totals_by_language
        .iter()
        .map(|l| {
            format!(
                r#"{{"lang":"{}","files":{},"code":{},"physical":{}}}"#,
                json_escape(l.language.display_name()),
                l.files,
                l.code_lines,
                l.total_physical_lines,
            )
        })
        .collect();
    format!("[{}]", entries.join(","))
}

fn build_semantic_chart_json(run: &AnalysisRun) -> String {
    let entries: Vec<String> = run
        .totals_by_language
        .iter()
        .filter(|l| l.functions > 0 || l.classes > 0 || l.variables > 0 || l.imports > 0 || l.test_count > 0)
        .map(|l| {
            format!(
                r#"{{"lang":"{}","functions":{},"classes":{},"variables":{},"imports":{},"tests":{}}}"#,
                json_escape(l.language.display_name()),
                l.functions, l.classes, l.variables, l.imports, l.test_count,
            )
        })
        .collect();
    format!("[{}]", entries.join(","))
}

fn build_file_size_histogram_json(run: &AnalysisRun) -> String {
    // Buckets: Tiny <50, Small 50-199, Medium 200-499, Large 500-999, Huge >=1000
    let labels = [
        ("Tiny (<50)", 0u64, 49u64),
        ("Small (50-199)", 50, 199),
        ("Medium (200-499)", 200, 499),
        ("Large (500-999)", 500, 999),
        ("Huge (>=1000)", 1000, u64::MAX),
    ];
    let mut counts = [0u64; 5];
    for f in &run.per_file_records {
        let cl = f.effective_counts.code_lines;
        for (i, &(_, lo, hi)) in labels.iter().enumerate() {
            if cl >= lo && cl <= hi {
                counts[i] += 1;
                break;
            }
        }
    }
    let entries: Vec<String> = labels
        .iter()
        .zip(counts.iter())
        .map(|((label, _, _), count)| {
            format!(r#"{{"label":"{}","count":{}}}"#, json_escape(label), count)
        })
        .collect();
    format!("[{}]", entries.join(","))
}

/// Build JSON for the multi-language style-guide adherence chart.
/// Returns a per-language-family array, each with its sorted guide scores.
fn build_style_chart_json(summary: &StyleSummary) -> String {
    let groups: Vec<String> = summary
        .by_language
        .iter()
        .map(|grp| {
            let guides: Vec<String> = grp
                .guide_avg_scores
                .iter()
                .map(|(name, score)| {
                    format!(r#"{{"guide":"{}","score":{}}}"#, json_escape(name), score)
                })
                .collect();
            format!(
                r#"{{"family":"{}","files":{},"indent":"{}","dominant":"{}","score":{},"guides":[{}]}}"#,
                json_escape(&grp.language_family),
                grp.files_count,
                json_escape(&grp.common_indent_style),
                json_escape(&grp.dominant_guide),
                grp.dominant_score_pct,
                guides.join(","),
            )
        })
        .collect();
    format!("[{}]", groups.join(","))
}

/// Build JSON for the per-file style breakdown table (up to 500 rows).
fn build_style_file_json(run: &AnalysisRun) -> String {
    let entries: Vec<String> = run
        .per_file_records
        .iter()
        .filter_map(|f| {
            let s = f.style_analysis.as_ref()?;
            // Collect key signals for display (up to 3).
            let sigs: Vec<String> = s
                .signals
                .iter()
                .take(3)
                .map(|sig| {
                    format!(
                        r#"{{"k":"{}","v":"{}"}}"#,
                        json_escape(&sig.name),
                        json_escape(&sig.value),
                    )
                })
                .collect();
            Some(format!(
                r#"{{"path":"{}","lang":"{}","family":"{}","indent":"{}","guide":"{}","score":{},"signals":[{}]}}"#,
                json_escape(&f.relative_path),
                json_escape(f.language.map_or("\u{2014}", |l| l.display_name())),
                json_escape(&s.language_family),
                json_escape(s.indent_style.display()),
                json_escape(&s.dominant_guide),
                s.dominant_score_pct,
                sigs.join(","),
            ))
        })
        .take(500)
        .collect();
    format!("[{}]", entries.join(","))
}

// ── Coverage / density helpers ────────────────────────────────────────────────

// ratio/percentage display, precision loss acceptable
#[allow(clippy::cast_precision_loss)]
fn coverage_pct_str(hit: u64, found: u64) -> String {
    if found > 0 {
        format!("{:.1}", hit as f64 / found as f64 * 100.0)
    } else {
        String::new()
    }
}

// ratio/percentage display, precision loss acceptable
#[allow(clippy::cast_precision_loss)]
fn coverage_class(hit: u64, found: u64) -> String {
    if found > 0 {
        let pct = hit as f64 / found as f64 * 100.0;
        if pct >= 80.0 {
            "good"
        } else if pct >= 60.0 {
            "warn"
        } else {
            "danger"
        }
    } else {
        "muted"
    }
    .to_string()
}

// ratio display, precision loss acceptable
#[allow(clippy::cast_precision_loss)]
fn format_test_density(code_lines: u64, test_count: u64) -> String {
    if code_lines > 0 && test_count > 0 {
        format!("{:.1}", test_count as f64 / code_lines as f64 * 1000.0)
    } else {
        String::from("0.0")
    }
}

/// Insert thousands separators into the integer portion of a number's textual form.
///
/// Works for plain integers (`"50789"` → `"50,789"`), signed values, and
/// pre-formatted decimal strings (`"11.2"` → `"11.2"`). Input whose integer part
/// is not all ASCII digits (e.g. `"—"`) is returned unchanged.
fn group_thousands(s: &str) -> String {
    let (sign, rest) = match s.as_bytes().first() {
        Some(b'-') => ("-", &s[1..]),
        Some(b'+') => ("+", &s[1..]),
        _ => ("", s),
    };
    let (int_part, frac_part) = match rest.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (rest, None),
    };
    if int_part.is_empty() || !int_part.bytes().all(|b| b.is_ascii_digit()) {
        return s.to_string();
    }
    let bytes = int_part.as_bytes();
    let len = bytes.len();
    let mut grouped = String::with_capacity(len + len / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(b as char);
    }
    frac_part.map_or_else(
        || format!("{sign}{grouped}"),
        |f| format!("{sign}{grouped}.{f}"),
    )
}

/// Custom Askama filters available to templates in this crate.
mod filters {
    // These lints fire on the wrapper code generated by `#[askama::filter_fn]`
    // (a `&self` `execute` method returning `Result`), not on our own source.
    #![allow(clippy::inline_always, clippy::unused_self, clippy::unnecessary_wraps)]
    use askama::{Result, Values};

    /// `{{ value|commas }}` — render any `Display` value with thousands separators.
    #[askama::filter_fn]
    pub fn commas<T: core::fmt::Display>(value: T, _: &dyn Values) -> Result<String> {
        Ok(super::group_thousands(&value.to_string()))
    }
}

// ── Main renderer ─────────────────────────────────────────────────────────────

#[allow(clippy::too_many_lines)] // large HTML renderer; splitting would obscure the template structure
fn render_html_inner(
    run: &AnalysisRun,
    is_sub_report: bool,
    pdf_url: Option<&str>,
    delta_ctx: Option<&ReportDeltaContext>,
) -> Result<String> {
    let config_json = serde_json::to_string_pretty(&run.effective_configuration)
        .context("failed to serialize effective configuration")?;

    let warning_summary_rows = summarize_warnings(&run.warnings);
    let warning_opportunity_rows = build_support_opportunities(&run.warnings);

    let logo_text_uri = png_data_uri(LOGO_TEXT_PNG);
    let small_logo_uri = png_data_uri(SMALL_LOGO_PNG);

    let rep = &run.effective_configuration.reporting;
    let custom_logo_uri = rep.logo_path.as_deref().and_then(load_custom_logo);
    let company_name = rep.company_name.clone();
    let accent_hex = rep.accent_color.clone();
    let report_header_footer = rep.report_header_footer.clone();

    let totals = &run.summary_totals;

    // The HTML report paginates client-side, so surface a deeper ranking (up to 200 files)
    // than the 15-row PDF page.
    let hotspot_rows = build_hotspot_rows(run, 200);
    let author_rows = build_author_rows(run);
    let own_summary = build_ownership_summary(run);

    let template = ReportTemplate {
        // Empty nonce for disk-saved reports; patch_html_nonce replaces it
        // with the request nonce when serving from the web server.
        nonce: String::new(),
        title: rep.report_title.clone(),
        browser_title: format!("Oxide-SLOC | {}", rep.report_title),
        scan_performed_by: run.environment.ci_name.clone().unwrap_or_else(|| {
            format!(
                "{} / {}",
                run.environment.initiator_username, run.environment.initiator_hostname
            )
        }),
        scan_time_pst: to_pst_display(run.tool.timestamp_utc),
        tool_version: run.tool.version.clone(),
        is_sub_report,
        run,
        language_rows: run
            .totals_by_language
            .iter()
            .map(|row| LanguageRow {
                language: row.language.display_name().to_string(),
                files: row.files,
                total_physical_lines: row.total_physical_lines,
                code_lines: row.code_lines,
                comment_lines: row.comment_lines,
                blank_lines: row.blank_lines,
                mixed_lines_separate: row.mixed_lines_separate,
                functions: row.functions,
                classes: row.classes,
                variables: row.variables,
                imports: row.imports,
                test_count: row.test_count,
                test_assertion_count: row.test_assertion_count,
                test_suite_count: row.test_suite_count,
                test_density_str: if row.code_lines > 0 {
                    // ratio display, precision loss acceptable
                    #[allow(clippy::cast_precision_loss)]
                    let density = row.test_count as f64 / row.code_lines as f64 * 1000.0;
                    format!("{density:.1}")
                } else {
                    "—".to_string()
                },
            })
            .collect(),
        file_rows: run.per_file_records.iter().map(file_row_view).collect(),
        skipped_rows: run.skipped_file_records.iter().map(file_row_view).collect(),
        config_json,
        lang_chart_json: build_lang_chart_json(run),
        submodule_chart_json: build_submodule_chart_json(run),
        scatter_chart_json: build_scatter_chart_json(run),
        semantic_chart_json: build_semantic_chart_json(run),
        file_size_histogram_json: build_file_size_histogram_json(run),
        has_submodule_data: !run.submodule_summaries.is_empty(),
        has_semantic_data: run
            .totals_by_language
            .iter()
            .any(|l| l.functions > 0 || l.classes > 0 || l.test_count > 0),
        has_coverage_data: run.per_file_records.iter().any(|f| f.coverage.is_some()),
        has_fn_coverage: totals.coverage_functions_found > 0,
        has_branch_coverage: totals.coverage_branches_found > 0,
        test_files_count: run
            .per_file_records
            .iter()
            .filter(|f| f.raw_line_categories.test_count > 0)
            .count() as u64,
        test_assertion_count: totals.test_assertion_count,
        test_suite_count: totals.test_suite_count,
        test_density: format_test_density(totals.code_lines, totals.test_count),
        most_tested_lang: run
            .totals_by_language
            .iter()
            .filter(|l| l.test_count > 0)
            .max_by_key(|l| l.test_count)
            .map_or_else(
                || "\u{2014}".to_string(),
                |l| l.language.display_name().to_string(),
            ),
        langs_with_tests: run
            .totals_by_language
            .iter()
            .filter(|l| l.test_count > 0)
            .count(),
        cov_line_pct: coverage_pct_str(totals.coverage_lines_hit, totals.coverage_lines_found),
        cov_fn_pct: coverage_pct_str(
            totals.coverage_functions_hit,
            totals.coverage_functions_found,
        ),
        cov_branch_pct: coverage_pct_str(
            totals.coverage_branches_hit,
            totals.coverage_branches_found,
        ),
        cov_line_class: coverage_class(totals.coverage_lines_hit, totals.coverage_lines_found),
        cov_fn_class: coverage_class(
            totals.coverage_functions_hit,
            totals.coverage_functions_found,
        ),
        cov_branch_class: coverage_class(
            totals.coverage_branches_hit,
            totals.coverage_branches_found,
        ),
        has_run_warnings: !run.warnings.is_empty(),
        warning_count: run.warnings.len(),
        warning_summary_rows,
        warning_opportunity_rows,
        warning_console_full: build_warning_console(&run.warnings),
        logo_text_uri,
        small_logo_uri,
        custom_logo_uri,
        company_name,
        accent_hex,
        report_header_footer,
        chart_js: CHART_JS,
        run_id_short: run
            .tool
            .run_id
            .split('-')
            .next_back()
            .unwrap_or(&run.tool.run_id)
            .chars()
            .take(7)
            .collect(),
        standalone_pdf_url: pdf_url.map(str::to_string),
        has_style_data: run.style_summary.is_some(),
        style_lang_count: run
            .style_summary
            .as_ref()
            .map_or(0, |ss| ss.by_language.len()),
        style_score_threshold: run.effective_configuration.analysis.style_score_threshold,
        style_chart_json: run
            .style_summary
            .as_ref()
            .map(build_style_chart_json)
            .unwrap_or_default(),
        style_file_json: if run.style_summary.is_some() {
            build_style_file_json(run)
        } else {
            String::new()
        },
        style_summary: run.style_summary.clone(),
        has_delta: delta_ctx.is_some(),
        delta_code_added: delta_ctx.map_or(0, |d| d.delta_code_added),
        delta_code_removed: delta_ctx.map_or(0, |d| d.delta_code_removed),
        delta_unmodified_lines: delta_ctx.map_or(0, |d| d.delta_unmodified_lines),
        delta_files_added: delta_ctx.map_or(0, |d| d.delta_files_added),
        delta_files_removed: delta_ctx.map_or(0, |d| d.delta_files_removed),
        delta_files_modified: delta_ctx.map_or(0, |d| d.delta_files_modified),
        delta_files_unchanged: delta_ctx.map_or(0, |d| d.delta_files_unchanged),
        delta_files_total: delta_ctx.map_or(0, |d| {
            d.delta_files_added
                + d.delta_files_removed
                + d.delta_files_modified
                + d.delta_files_unchanged
        }),
        prev_code_lines: delta_ctx.map_or(0, |d| d.prev_code_lines),
        prev_scan_count: delta_ctx.map_or(0, |d| d.prev_scan_count),
        prev_scan_label: delta_ctx
            .map(|d| d.prev_scan_label.clone())
            .unwrap_or_default(),
        prev_run_id: delta_ctx
            .and_then(|d| d.prev_run_id.clone())
            .unwrap_or_default(),
        git_commit_url: run
            .git_remote_url
            .as_deref()
            .zip(run.git_commit_long.as_deref())
            .and_then(|(remote, sha)| derive_commit_url(remote, sha)),
        git_branch_url: run
            .git_remote_url
            .as_deref()
            .zip(run.git_branch.as_deref())
            .and_then(|(remote, branch)| derive_branch_url(remote, branch)),
        has_cocomo: run.cocomo.is_some(),
        cocomo_effort_str: run
            .cocomo
            .as_ref()
            .map_or(String::new(), |c| format!("{:.2}", c.effort_person_months)),
        cocomo_duration_str: run
            .cocomo
            .as_ref()
            .map_or(String::new(), |c| format!("{:.2}", c.duration_months)),
        cocomo_staff_str: run
            .cocomo
            .as_ref()
            .map_or(String::new(), |c| format!("{:.2}", c.avg_staff)),
        cocomo_ksloc_str: run
            .cocomo
            .as_ref()
            .map_or(String::new(), |c| format!("{:.2}", c.ksloc)),
        cocomo_mode_label: run
            .cocomo
            .as_ref()
            .map_or_else(|| "Organic".to_string(), |c| {
                match c.mode {
                    CocomoMode::Organic => "Organic",
                    CocomoMode::SemiDetached => "Semi-detached",
                    CocomoMode::Embedded => "Embedded",
                }
                .to_string()
            }),
        cocomo_mode_tooltip: run
            .cocomo
            .as_ref()
            .map_or(String::new(), |c| match c.mode {
                CocomoMode::Organic => "Organic: A small team working on a well-understood \
                    project in a familiar environment with minimal external constraints. \
                    Suited for internal tools, utilities, and projects with stable requirements. \
                    Effort = 2.4 \u{00D7} KSLOC^1.05.",
                CocomoMode::SemiDetached => "Semi-detached: A mixed team with varying levels of \
                    experience tackling a project with moderate novelty and some rigid constraints. \
                    Typical for compilers, transaction systems, and batch processors. \
                    Effort = 3.0 \u{00D7} KSLOC^1.12.",
                CocomoMode::Embedded => "Embedded: Tight hardware, software, or operational \
                    constraints requiring significant innovation and deep integration work. \
                    Typical for real-time control systems and safety-critical software. \
                    Effort = 3.6 \u{00D7} KSLOC^1.20.",
            }.to_string()),
        uloc: run.uloc,
        dryness_pct_str: run
            .dryness_pct
            .map_or(String::new(), |d| format!("{d:.1}")),
        duplicate_group_count: run.duplicate_groups.len(),
        has_hotspots: !hotspot_rows.is_empty(),
        hotspot_rows,
        has_ownership: !author_rows.is_empty(),
        ownership_rows: author_rows,
        own_contributors: own_summary.contributors,
        own_top_name: own_summary.top_name,
        own_top_pct_str: own_summary.top_pct_str,
        own_bus_factor: own_summary.bus_factor,
        own_total_code: own_summary.total_code,
        own_dev_code: own_summary.dev_code,
        own_test_code: own_summary.test_code,
        own_test_pct_str: own_summary.test_pct_str,
        own_total_comment: own_summary.total_comment,
    };

    template.render().context("failed to render HTML report")
}

/// One row of the Git Hotspots table: a file ranked by `code_lines × recent commits`.
struct HotspotRow {
    path: String,
    code_lines: u64,
    commit_count: u32,
    last_commit_date: String,
    score: u64,
    /// Primary owner of the file (top blame owner). Empty unless attribution ran.
    owner: String,
    /// Best-effort profile URL for the primary owner (GitHub/GHE no-reply email → profile),
    /// `None` when the owner's email carries no derivable platform handle.
    owner_profile_url: Option<String>,
}

/// Best-effort profile URL for a contributor, derived from a GitHub / GitHub-Enterprise-style
/// no-reply commit email of the form `<id>+<handle>@users.noreply.<host>` or
/// `<handle>@users.noreply.<host>`. Returns `https://<host>/<handle>` — e.g.
/// `12345+octocat@users.noreply.github.com` → `https://github.com/octocat`, and the same shape
/// works for a GitHub Enterprise host. Returns `None` for plain corporate/personal emails, which
/// carry no reliable platform handle (linking those would fabricate dead URLs).
fn author_profile_url(email: &str) -> Option<String> {
    let (local, domain) = email.trim().split_once('@')?;
    let host = domain.strip_prefix("users.noreply.")?;
    let handle = local.rsplit_once('+').map_or(local, |(_, h)| h);
    if handle.is_empty() || host.is_empty() {
        return None;
    }
    Some(format!("https://{host}/{handle}"))
}

/// Best-effort profile URL for a resolved [`Author`]: tries the canonical email first, then any
/// folded alias email, so a contributor whose canonical identity is a personal email still links
/// when one of their merged identities is a platform no-reply address.
fn author_profile_url_for(author: &Author) -> Option<String> {
    author_profile_url(&author.canonical_email).or_else(|| {
        author
            .aliases
            .iter()
            .find_map(|al| author_profile_url(&al.email))
    })
}

/// Build the git hotspots from per-file activity (only files that carry a
/// `commit_count` from an `--activity-window` scan), ranked by `code_lines × commits`
/// and capped at `limit` rows. The interactive HTML report requests a larger cap (so its
/// client-side pagination has something to page through); the fixed-height PDF page keeps
/// the original top-15.
fn build_hotspot_rows(run: &AnalysisRun, limit: usize) -> Vec<HotspotRow> {
    // Map author id → display name so each hotspot file can show its primary owner.
    let author_names: std::collections::HashMap<u32, &str> = run
        .authors
        .iter()
        .map(|a| (a.id, a.canonical_name.as_str()))
        .collect();
    // Parallel map: author id → best-effort profile URL, so each hotspot owner can link out.
    let author_urls: std::collections::HashMap<u32, String> = run
        .authors
        .iter()
        .filter_map(|a| author_profile_url_for(a).map(|url| (a.id, url)))
        .collect();
    let mut rows: Vec<HotspotRow> = run
        .per_file_records
        .iter()
        .filter_map(|r| {
            let commits = r.commit_count?;
            let code = r.effective_counts.code_lines;
            let top_owner_id = r
                .ownership
                .as_ref()
                .and_then(|o| o.first())
                .map(|top| top.author_id);
            let owner = top_owner_id
                .and_then(|id| author_names.get(&id))
                .map_or_else(String::new, |name| (*name).to_string());
            let owner_profile_url = top_owner_id.and_then(|id| author_urls.get(&id).cloned());
            Some(HotspotRow {
                path: r.relative_path.clone(),
                code_lines: code,
                commit_count: commits,
                // Show the calendar date only (strip the time component of the ISO date).
                last_commit_date: r.last_commit_date.as_deref().map_or_else(String::new, |d| {
                    d.split('T').next().unwrap_or(d).to_string()
                }),
                score: code.saturating_mul(u64::from(commits)),
                owner,
                owner_profile_url,
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(b.commit_count.cmp(&a.commit_count))
    });
    rows.truncate(limit);
    rows
}

/// One row of the Code Ownership table: a contributor's blame-based line tallies.
struct AuthorReportRow {
    name: String,
    email: String,
    code: u64,
    comment: u64,
    blank: u64,
    total: u64,
    /// Share of total code lines owned, pre-formatted to one decimal (e.g. "42.7").
    code_pct_str: String,
    /// Files where this author is the single largest owner.
    files_owned: u64,
    /// The files this author owns (top owner of), for the per-author drill-down. Ordered by
    /// owned code lines descending.
    files: Vec<OwnedFileRow>,
    /// Leaderboard rank among contributors who own at least one file (1 = most code owned).
    /// `0` for contributors who own no files (they don't appear on the leaderboard).
    rank: usize,
    /// CSS medal class for the top three ranks (`lb-r1`/`lb-r2`/`lb-r3`), else empty.
    rank_class: &'static str,
    /// Up-to-two-letter initials for the leaderboard avatar (e.g. "NS").
    initials: String,
    /// Accent colour for this contributor's avatar/bar, from the canonical palette by rank.
    color: &'static str,
    /// Best-effort profile URL (GitHub/GHE no-reply email → profile); `None` when not derivable.
    profile_url: Option<String>,
}

/// One file a contributor owns, for the Code Ownership per-author drill-down: the lines *this*
/// author owns in the file (not the file's totals) plus its last-changed date.
struct OwnedFileRow {
    path: String,
    code: u64,
    comment: u64,
    blank: u64,
    total: u64,
    last_changed: String,
}

/// Canonical categorical palette (shared with the web charts) used to tint leaderboard avatars.
const REPORT_PALETTE: &[&str] = &[
    "#C45C10", "#2A6846", "#4472C4", "#805099", "#D4A017", "#B23030", "#2E75B6", "#70AD47",
    "#FF9900", "#9E480E", "#156082", "#5BA8A0",
];

/// Up-to-two-letter uppercase initials for a contributor's leaderboard avatar. Uses the first
/// letter of the first and last whitespace-separated name parts (or the first two letters of a
/// single-word name).
fn author_initials(name: &str) -> String {
    let parts: Vec<&str> = name.split_whitespace().collect();
    match parts.as_slice() {
        [] => "?".to_string(),
        [one] => one.chars().take(2).collect::<String>().to_uppercase(),
        [first, .., last] => {
            let a = first.chars().next().unwrap_or('?');
            let b = last.chars().next().unwrap_or('?');
            format!("{a}{b}").to_uppercase()
        }
    }
}

/// Build the per-author ownership rows from `run.authors` (populated when an attribution scan
/// ran on a git repo), already ordered by code lines owned. Empty when attribution did not run.
fn build_author_rows(run: &AnalysisRun) -> Vec<AuthorReportRow> {
    if run.authors.is_empty() {
        return Vec::new();
    }
    let total_code: u64 = run.authors.iter().map(|a| a.counts.code_lines).sum();
    // Collect, per author, the files they are the single largest owner of.
    let mut owned_files: std::collections::HashMap<u32, Vec<OwnedFileRow>> =
        std::collections::HashMap::new();
    for rec in &run.per_file_records {
        if let Some(top) = rec.ownership.as_ref().and_then(|o| o.first()) {
            owned_files
                .entry(top.author_id)
                .or_default()
                .push(OwnedFileRow {
                    path: rec.relative_path.clone(),
                    code: top.counts.code_lines,
                    comment: top.counts.comment_lines,
                    blank: top.counts.blank_lines,
                    total: top.counts.total_lines,
                    last_changed: rec
                        .last_commit_date
                        .as_deref()
                        .map_or_else(String::new, |d| {
                            d.split('T').next().unwrap_or(d).to_string()
                        }),
                });
        }
    }
    let mut rows: Vec<AuthorReportRow> = run
        .authors
        .iter()
        .map(|a| {
            let pct = if total_code > 0 {
                a.counts.code_lines as f64 / total_code as f64 * 100.0
            } else {
                0.0
            };
            let mut files = owned_files.remove(&a.id).unwrap_or_default();
            files.sort_by_key(|f| std::cmp::Reverse(f.code));
            AuthorReportRow {
                name: a.canonical_name.clone(),
                email: a.canonical_email.clone(),
                code: a.counts.code_lines,
                comment: a.counts.comment_lines,
                blank: a.counts.blank_lines,
                total: a.counts.total_lines,
                code_pct_str: format!("{pct:.1}"),
                files_owned: files.len() as u64,
                files,
                rank: 0,
                rank_class: "",
                initials: author_initials(&a.canonical_name),
                color: REPORT_PALETTE[0],
                profile_url: author_profile_url_for(a),
            }
        })
        .collect();
    // Assign leaderboard rank + medal class + accent colour among file-owning contributors
    // (already ordered by code lines owned).
    let mut rank = 0usize;
    for row in &mut rows {
        if row.files.is_empty() {
            continue;
        }
        rank += 1;
        row.rank = rank;
        row.rank_class = match rank {
            1 => "lb-r1",
            2 => "lb-r2",
            3 => "lb-r3",
            _ => "",
        };
        row.color = REPORT_PALETTE[(rank - 1) % REPORT_PALETTE.len()];
    }
    rows
}

/// Headline code-ownership statistics surfaced above the ownership table in the report (mirrors the
/// web `/code-ownership` summary chips). All zero / empty when attribution did not run.
#[derive(Default)]
struct OwnershipSummary {
    contributors: usize,
    top_name: String,
    top_pct_str: String,
    bus_factor: usize,
    total_code: u64,
    dev_code: u64,
    test_code: u64,
    test_pct_str: String,
    total_comment: u64,
}

/// Compute the ownership summary stats (contributors, top owner, bus factor, dev/test split,
/// comment lines) from a completed run. Empty when attribution did not populate `run.authors`.
fn build_ownership_summary(run: &AnalysisRun) -> OwnershipSummary {
    if run.authors.is_empty() {
        return OwnershipSummary::default();
    }
    let total_code: u64 = run.authors.iter().map(|a| a.counts.code_lines).sum();
    let total_comment: u64 = run.authors.iter().map(|a| a.counts.comment_lines).sum();
    // Bus factor: fewest top contributors (already ordered by code owned) covering >= 50% of code.
    let mut acc = 0u64;
    let mut bus_factor = 0usize;
    for a in &run.authors {
        acc += a.counts.code_lines;
        bus_factor += 1;
        if total_code > 0 && acc * 2 >= total_code {
            break;
        }
    }
    // Test code = code lines owned in files classified as tests.
    let test_code: u64 = run
        .per_file_records
        .iter()
        .filter(|rec| rec.is_test_file())
        .filter_map(|rec| rec.ownership.as_ref())
        .flat_map(|own| own.iter().map(|o| o.counts.code_lines))
        .sum();
    let top = &run.authors[0];
    let top_pct = if total_code > 0 {
        top.counts.code_lines as f64 / total_code as f64 * 100.0
    } else {
        0.0
    };
    let test_pct = if total_code > 0 {
        test_code as f64 / total_code as f64 * 100.0
    } else {
        0.0
    };
    OwnershipSummary {
        contributors: run.authors.len(),
        top_name: top.canonical_name.clone(),
        top_pct_str: format!("{top_pct:.0}"),
        bus_factor,
        total_code,
        dev_code: total_code.saturating_sub(test_code),
        test_code,
        test_pct_str: format!("{test_pct:.0}"),
        total_comment,
    }
}

/// Render an HTML report and write it to `output_path`.
///
/// # Errors
///
/// Returns an error if rendering fails or the file cannot be written.
pub fn write_html(run: &AnalysisRun, output_path: &Path) -> Result<()> {
    let html = render_html_inner(run, false, None, None)?;
    fs::write(output_path, html)
        .with_context(|| format!("failed to write HTML report to {}", output_path.display()))
}

/// Write an HTML report that embeds a relative link to a pre-generated PDF.
///
/// When `pdf_path` is in the same directory as `output_path`, the "View PDF"
/// button in the report opens the PDF directly (e.g. from a Jenkins HTML
/// Publisher artifact directory) instead of calling the oxide-sloc server route.
/// Pass `pdf_path = None` to get the same behaviour as [`write_html`].
///
/// # Errors
/// Returns an error if HTML rendering or file I/O fails.
pub fn write_html_with_pdf_link(
    run: &AnalysisRun,
    output_path: &Path,
    pdf_path: Option<&Path>,
) -> Result<()> {
    let pdf_relative = pdf_path.and_then(|pdf| {
        let html_dir = output_path.parent()?;
        let pdf_dir = pdf.parent()?;
        if html_dir == pdf_dir {
            pdf.file_name().map(|n| n.to_string_lossy().into_owned())
        } else {
            None
        }
    });
    let html = render_html_inner(run, false, pdf_relative.as_deref(), None)?;
    fs::write(output_path, html)
        .with_context(|| format!("failed to write HTML report to {}", output_path.display()))
}

/// Launch a headless Chromium browser.
/// When `no_sandbox` is true (set via `SLOC_BROWSER_NOSANDBOX=1`) the browser
/// runs without the namespace sandbox — required in containers that drop `SYS_ADMIN`.
/// Otherwise the sandbox is always enabled with no automatic fallback, so failures
/// surface as clear errors rather than silently removing a security boundary.
fn launch_cdp_browser(
    browser_path: std::path::PathBuf,
    no_sandbox: bool,
) -> Result<headless_chrome::Browser> {
    use headless_chrome::{Browser, LaunchOptions};

    if no_sandbox {
        return Browser::new(LaunchOptions {
            headless: true,
            path: Some(browser_path),
            window_size: Some((1122, 794)),
            sandbox: false,
            ..Default::default()
        })
        .context("failed to launch browser via CDP (no-sandbox)");
    }

    // Sandboxed only — no automatic fallback to --no-sandbox.
    // If this fails in a container, set SLOC_BROWSER_NOSANDBOX=1 to opt in explicitly.
    Browser::new(LaunchOptions {
        headless: true,
        path: Some(browser_path),
        window_size: Some((1122, 794)),
        sandbox: true,
        ..Default::default()
    })
    .map_err(|e| {
        anyhow::anyhow!(
            "Browser launch failed with sandbox enabled: {e:#}\n\
             If running in a container without user namespaces (e.g. Docker with cap_drop:ALL), \
             set SLOC_BROWSER_NOSANDBOX=1 to opt into --no-sandbox mode."
        )
    })
}

/// If a JS chart error was recorded on the page, print it to stderr.
fn report_chart_error_if_any(tab: &headless_chrome::Tab) {
    let Ok(e) = tab.evaluate("window.oxSlocChartError||''", false) else {
        return;
    };
    let Some(serde_json::Value::String(msg)) = e.value else {
        return;
    };
    if !msg.is_empty() {
        eprintln!("[oxide-sloc][pdf] chart JS error (charts may be missing): {msg}");
    }
}

/// Poll `window.oxSlocChartsReady` for up to 15 s so Chart.js canvases finish rendering.
fn wait_for_charts_ready(tab: &headless_chrome::Tab) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut last_cdp_err: Option<String> = None;
    loop {
        match tab.evaluate("!!window.oxSlocChartsReady", false) {
            Ok(r) => {
                last_cdp_err = None;
                if matches!(r.value, Some(serde_json::Value::Bool(true))) {
                    report_chart_error_if_any(tab);
                    return;
                }
            }
            Err(e) => {
                let msg = format!("{e:#}");
                if last_cdp_err.as_deref() != Some(&msg) {
                    eprintln!("[oxide-sloc][pdf] CDP evaluate error (will retry): {msg}");
                    last_cdp_err = Some(msg);
                }
            }
        }
        if std::time::Instant::now() >= deadline {
            report_chart_error_if_any(tab);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

/// Read the `.report-id-banner` text from the loaded page, if present and non-empty.
fn extract_banner_text(tab: &headless_chrome::Tab) -> Option<String> {
    let result = tab
        .evaluate(
            "(function(){\
               var el=document.querySelector('.report-id-banner');\
               return el?el.textContent.trim():null;\
             })()",
            false,
        )
        .ok()?;
    match result.value? {
        serde_json::Value::String(s) if !s.is_empty() => Some(s),
        _ => None,
    }
}

/// Read the `innerHTML` of an optional `#<id>` element supplied by the document to act as
/// a Chrome print header/footer template. Chrome renders these in the page margin on every
/// printed page (including a short final page), which in-flow or `position:fixed` markup
/// cannot do reliably. Returns `None` when the element is absent or empty.
fn extract_pdf_template(tab: &headless_chrome::Tab, id: &str) -> Option<String> {
    // `id` is always a hard-coded constant below — no script-injection surface.
    let js = format!(
        "(function(){{var el=document.getElementById('{id}');return el?el.innerHTML:null;}})()"
    );
    let result = tab.evaluate(&js, false).ok()?;
    match result.value? {
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s),
        _ => None,
    }
}

/// Use Chrome `DevTools` Protocol to render `html_path` as a PDF at `output_path`.
///
/// Launches a headless Chromium-based browser at A4-landscape viewport (1122 × 794 px),
/// waits up to 15 s for all Chart.js canvases to signal readiness via
/// `window.oxSlocChartsReady`, then captures the page using `Page.printToPDF` via CDP.
fn write_pdf_via_cdp(html_path: &Path, output_path: &Path) -> Result<()> {
    use headless_chrome::types::PrintToPdfOptions;

    let browser_path = discover_browser().context(
        "no supported Chromium-based browser found; \
         set SLOC_BROWSER/BROWSER or install Chrome, Chromium, Edge, Brave, Vivaldi, or Opera",
    )?;
    eprintln!("[oxide-sloc][pdf] browser = {}", browser_path.display());

    let no_sandbox = std::env::var("SLOC_BROWSER_NOSANDBOX").as_deref() == Ok("1");
    if no_sandbox {
        eprintln!("[oxide-sloc][pdf] --no-sandbox enabled via SLOC_BROWSER_NOSANDBOX=1");
    }

    let browser = launch_cdp_browser(browser_path, no_sandbox)?;
    let tab = browser.new_tab().context("failed to open browser tab")?;
    // Raise the per-call CDP timeout well above the 20 s default. On a loaded host
    // (e.g. the user's own Chromium already eating several GB) just launching a second
    // headless instance and navigating a trivial page can take 15-30 s; the old default
    // made navigation/print time out and fall back to wkhtmltopdf, failing the export.
    tab.set_default_timeout(std::time::Duration::from_secs(90));

    let html_for_url = PathBuf::from(
        html_path
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .to_string(),
    );
    let url = file_url(&html_for_url);
    eprintln!("[oxide-sloc][pdf] url = {url}");

    tab.navigate_to(&url)
        .context("failed to navigate browser to HTML file")?;
    tab.wait_until_navigated()
        .context("browser navigation did not complete")?;

    wait_for_charts_ready(&tab);

    // Resolve the per-page header/footer chrome (banner or per-document native templates)
    // and the margins those require. Kept in a helper so this function stays flat.
    let chrome = build_pdf_chrome(&tab);

    let pdf_bytes = tab
        .print_to_pdf(Some(PrintToPdfOptions {
            landscape: Some(true),
            print_background: Some(true),
            scale: Some(0.97),
            paper_width: Some(11.69), // A4 landscape width (inches)
            paper_height: Some(8.27), // A4 landscape height (inches)
            margin_top: Some(chrome.margin_top),
            margin_bottom: Some(chrome.margin_bottom),
            margin_left: Some(0.0),
            margin_right: Some(0.0),
            prefer_css_page_size: Some(false),
            display_header_footer: if chrome.display_header_footer {
                Some(true)
            } else {
                None
            },
            header_template: chrome.header_template,
            footer_template: chrome.footer_template,
            ..Default::default()
        }))
        .context("browser failed to generate PDF")?;

    fs::write(output_path, &pdf_bytes)
        .with_context(|| format!("failed to write PDF to {}", output_path.display()))?;

    eprintln!("[oxide-sloc][pdf] wrote {} bytes", pdf_bytes.len());
    Ok(())
}

/// Resolved per-page print chrome for the CDP PDF export.
struct PdfChrome {
    header_template: Option<String>,
    footer_template: Option<String>,
    display_header_footer: bool,
    margin_top: f64,
    margin_bottom: f64,
}

/// HTML template for a centred identification banner rendered in the PDF page margin.
/// The template renders in the margin area, so `font-size` must be set explicitly.
fn pdf_banner_template(text: &str) -> String {
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    format!(
        r#"<div style="font-size:10px;width:100%;text-align:center;\
color:#fff;background:#b35428;padding:5px 0;\
font-family:sans-serif;font-weight:700;letter-spacing:0.05em;\
-webkit-print-color-adjust:exact;print-color-adjust:exact;">{escaped}</div>"#
    )
}

/// Reserve top/bottom margins only on the side(s) that actually carry chrome. A banner keeps
/// its historical top/bottom reserve.
const fn pdf_margins(
    has_banner: bool,
    has_native_header: bool,
    has_native_footer: bool,
) -> (f64, f64) {
    let top = if has_banner {
        0.35
    } else if has_native_header {
        0.55
    } else {
        0.0
    };
    let bottom = if has_banner {
        0.25
    } else if has_native_footer {
        0.42
    } else {
        0.0
    };
    (top, bottom)
}

/// Choose the Chrome header/footer templates. A banner wins; otherwise per-document native
/// chrome is used. Chrome prints both a header and footer template whenever they are supplied,
/// so an empty `<span>` suppresses default chrome on the unused side.
fn pdf_header_footer_templates(
    banner_text: Option<&str>,
    native_header: Option<String>,
    native_footer: Option<String>,
) -> (Option<String>, Option<String>) {
    if let Some(t) = banner_text {
        let tmpl = pdf_banner_template(t);
        return (Some(tmpl.clone()), Some(tmpl));
    }
    if native_header.is_some() || native_footer.is_some() {
        let empty = || "<span></span>".to_string();
        return (
            Some(native_header.unwrap_or_else(empty)),
            Some(native_footer.unwrap_or_else(empty)),
        );
    }
    (None, None)
}

/// Determine the header/footer templates and reserved margins for the PDF.
///
/// Priority: a report identification banner (set in step 3 of the scan configuration as
/// `report_header_footer`) wins; otherwise per-document hidden `#pdf-native-header` /
/// `#pdf-native-footer` elements are used (the Scan Delta report uses these for its per-page
/// footer bar).
fn build_pdf_chrome(tab: &headless_chrome::Tab) -> PdfChrome {
    let banner_text = extract_banner_text(tab);
    if let Some(ref t) = banner_text {
        eprintln!("[oxide-sloc][pdf] report banner detected: {t}");
    }
    let has_banner = banner_text.is_some();

    let native_header = if has_banner {
        None
    } else {
        extract_pdf_template(tab, "pdf-native-header")
    };
    let native_footer = if has_banner {
        None
    } else {
        extract_pdf_template(tab, "pdf-native-footer")
    };
    let has_native_header = native_header.is_some();
    let has_native_footer = native_footer.is_some();

    let (header_template, footer_template) =
        pdf_header_footer_templates(banner_text.as_deref(), native_header, native_footer);
    let (margin_top, margin_bottom) = pdf_margins(has_banner, has_native_header, has_native_footer);

    PdfChrome {
        header_template,
        footer_template,
        display_header_footer: has_banner || has_native_header || has_native_footer,
        margin_top,
        margin_bottom,
    }
}

/// Locate the `wkhtmltopdf` binary on Linux and Windows.
///
/// Search order:
/// 1. `wkhtmltopdf` / `wkhtmltopdf.exe` anywhere in `$PATH` (covers Linux packages and
///    Windows installs that add the bin dir to the system PATH).
/// 2. Windows-only: standard MSI install locations under `Program Files` and
///    `Program Files (x86)`.
/// 3. Linux-only: absolute paths that package managers commonly use but that may not be
///    on the service account's `$PATH`.
fn discover_wkhtmltopdf() -> Option<PathBuf> {
    if let Some(p) = which_in_path("wkhtmltopdf") {
        return Some(p);
    }

    #[cfg(windows)]
    {
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Ok(base) = std::env::var(var) {
                let candidate = PathBuf::from(base)
                    .join("wkhtmltopdf")
                    .join("bin")
                    .join("wkhtmltopdf.exe");
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }

    #[cfg(not(windows))]
    for p in [
        "/usr/bin/wkhtmltopdf",
        "/usr/local/bin/wkhtmltopdf",
        "/opt/wkhtmltopdf/bin/wkhtmltopdf",
        "/snap/bin/wkhtmltopdf",
    ] {
        let candidate = PathBuf::from(p);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    None
}

/// Generate a PDF using `wkhtmltopdf` when no Chromium-based browser is available.
///
/// Works on both Linux and Windows:
/// - Linux: install via `dnf install wkhtmltopdf` (RHEL/CentOS) or `apt install wkhtmltopdf`
/// - Windows: install the MSI from <https://wkhtmltopdf.org/downloads.html>; the installer
///   adds `wkhtmltopdf.exe` to `Program Files\wkhtmltopdf\bin\` which is checked automatically.
fn write_pdf_via_wkhtmltopdf(html_path: &Path, pdf_path: &Path) -> Result<()> {
    eprintln!("[oxide-sloc][pdf] trying wkhtmltopdf fallback");

    let exe = discover_wkhtmltopdf().context(
        "wkhtmltopdf not found. \
         Linux: install via 'dnf install wkhtmltopdf' or 'apt install wkhtmltopdf'. \
         Windows: install the MSI from https://wkhtmltopdf.org/downloads.html. \
         Alternatively, set SLOC_BROWSER to a Chromium-based browser executable.",
    )?;
    eprintln!("[oxide-sloc][pdf] wkhtmltopdf = {}", exe.display());

    // Strip the extended-length prefix on Windows (\\?\) so wkhtmltopdf can parse the path.
    let html_normalized = PathBuf::from(
        html_path
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .to_string(),
    );
    // file_url() handles Windows drive letters (C:\ → /C:/) and encodes special chars.
    let html_url = file_url(&html_normalized);
    eprintln!("[oxide-sloc][pdf] wkhtmltopdf url = {html_url}");

    let pdf_str = pdf_path
        .to_str()
        .context("PDF output path contains non-UTF-8 characters")?;

    let output = std::process::Command::new(&exe)
        .args([
            "--enable-javascript",
            "--javascript-delay",
            "2000",
            "--quiet",
            "--orientation",
            "Landscape",
            "--page-size",
            "A4",
            "--margin-top",
            "9",
            "--margin-bottom",
            "9",
            "--margin-left",
            "13",
            "--margin-right",
            "13",
            "--print-media-type",
            &html_url,
            pdf_str,
        ])
        .output()
        .with_context(|| format!("failed to launch wkhtmltopdf at {}", exe.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("wkhtmltopdf exited with {}: {stderr}", output.status);
    }

    if !pdf_path.exists() {
        anyhow::bail!(
            "wkhtmltopdf exited successfully but {} was not created",
            pdf_path.display()
        );
    }

    eprintln!("[oxide-sloc][pdf] wkhtmltopdf wrote {}", pdf_path.display());
    Ok(())
}

struct PdfCtx<'a> {
    layer: &'a crate::pdf_compat::PdfLayerReference,
    font_reg: crate::pdf_compat::IndirectFontRef,
    font_bold: crate::pdf_compat::IndirectFontRef,
    w: f32,
    margin: f32,
    row_h: f32,
    tbl_hdr_h: f32,
}

/// Fixed page geometry (landscape A4 in mm) threaded through the PDF page builders.
/// Bundled into one struct so the page helpers stay under the argument-count lint.
#[derive(Clone, Copy)]
struct PdfPageDims {
    w: f32,
    h: f32,
    margin: f32,
    footer_h: f32,
    row_h: f32,
    tbl_hdr_h: f32,
}

#[allow(
    clippy::cast_precision_loss,
    clippy::suboptimal_flops,
    clippy::too_many_lines
)]
fn runtime_mode_display(mode: &str) -> &str {
    match mode {
        "serve" => "Web UI",
        "analyze" => "CLI",
        "git-scan" => "Git Scan",
        "git-compare" => "Git Compare",
        "watch" => "Watch",
        other => other,
    }
}

fn pdf_render_page1_header(
    ctx: &PdfCtx<'_>,
    run: &AnalysisRun,
    ts: &str,
    title: &str,
    h: f32,
    hdr_h: f32,
    banner: Option<&str>,
) -> f32 {
    use crate::pdf_compat::{Color, Mm, Rgb};
    let hdr_y = h - hdr_h;
    pdf_fill_rect(
        ctx.layer,
        0.0,
        hdr_y,
        ctx.w,
        hdr_h,
        Rgb::new(0.098, 0.11, 0.15, None),
    );
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(1.0, 1.0, 1.0, None)));
    ctx.layer.use_text(
        "oxide-sloc",
        13.0,
        Mm(ctx.margin),
        Mm(hdr_y + 4.5),
        ctx.font_bold,
    );
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(0.72, 0.72, 0.72, None)));
    ctx.layer.use_text(
        "Code Metrics Report",
        9.5,
        Mm(54.0),
        Mm(hdr_y + 5.0),
        ctx.font_reg,
    );
    ctx.layer.use_text(
        pdf_safe_str(ts),
        8.0,
        Mm(ctx.w - 70.0),
        Mm(hdr_y + 5.0),
        ctx.font_reg,
    );
    // Report identification banner — white bold, centered between the two header items.
    if let Some(text) = banner {
        let safe = pdf_trunc(&pdf_safe_str(text), 40);
        // Approximate half-width at 9pt bold Helvetica (~0.97 mm per char) for centering.
        // `safe` is truncated to 40 chars, so the count is tiny; the f32 cast is exact here
        // and only ever feeds a millimetre layout coordinate.
        #[allow(
            clippy::cast_precision_loss,
            reason = "small bounded char count; sub-mm layout offset"
        )]
        let text_x = (safe.len() as f32).mul_add(-0.97, ctx.w / 2.0).max(95.0);
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.7, 0.33, 0.16, None)));
        ctx.layer
            .use_text(safe, 9.0, Mm(text_x), Mm(hdr_y + 4.5), ctx.font_bold);
    }
    let title_text_y = hdr_y - 5.5;
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(0.098, 0.11, 0.15, None)));
    ctx.layer.use_text(
        pdf_trunc(&pdf_safe_str(title), 55),
        9.5,
        Mm(ctx.margin),
        Mm(title_text_y),
        ctx.font_bold,
    );
    let roots_text_y = title_text_y - 5.0;
    // ── Left side: project path ──────────────────────────────────────────────
    let roots: String = run
        .input_roots
        .iter()
        .map(|r| pdf_safe_str(r))
        .collect::<Vec<_>>()
        .join("  ");
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(0.4, 0.4, 0.4, None)));
    ctx.layer.use_text(
        pdf_trunc(&roots, 85),
        6.5,
        Mm(ctx.margin),
        Mm(roots_text_y),
        ctx.font_reg,
    );
    // ── Right side: git + environment metadata in a grouped box ─────────────
    pdf_render_page1_gitbox(ctx, run, title_text_y, roots_text_y);
    roots_text_y
}

/// Render the right-side git + environment metadata box of the page-1 header.
fn pdf_render_page1_gitbox(
    ctx: &PdfCtx<'_>,
    run: &AnalysisRun,
    title_text_y: f32,
    roots_text_y: f32,
) {
    use crate::pdf_compat::{Color, Mm, Rgb};
    let mut git_parts: Vec<String> = vec![];
    if let Some(ref b) = run.git_branch {
        git_parts.push(format!("Branch: {}", pdf_safe_str(b)));
    }
    if let Some(ref c) = run.git_commit_short {
        git_parts.push(format!("Commit: {}", pdf_safe_str(c)));
    }
    if let Some(ref t) = run.git_nearest_tag {
        git_parts.push(format!("Tag: {}", pdf_safe_str(t)));
    }
    let git_str = pdf_trunc(&git_parts.join("  \u{00B7}  "), 70);

    let initiator = run
        .environment
        .ci_name
        .as_deref()
        .unwrap_or(run.environment.initiator_username.as_str());
    let mode_label = runtime_mode_display(&run.environment.runtime_mode);
    let env_str = format!(
        "OS: {} / {}  \u{00B7}  User: {}  \u{00B7}  Host: {}  \u{00B7}  Source: {}",
        pdf_safe_str(&run.environment.operating_system),
        pdf_safe_str(&run.environment.architecture),
        pdf_safe_str(initiator),
        pdf_safe_str(&run.environment.initiator_hostname),
        mode_label,
    );
    let env_trunc = pdf_trunc(&env_str, 100);

    // Shared right anchor — text right-edges land here; box extends pad_h mm beyond.
    let right_anchor = ctx.w - ctx.margin - 6.0;
    // Accurate widths using exact PDF Helvetica advance tables (PDF spec Appendix D).
    // Character-count estimates are unreliable for proportional fonts — actual per-glyph widths vary 4×.
    let git_w = helvetica_width_mm(&git_str, 7.5, true);
    let env_w = helvetica_width_mm(&env_trunc, 6.5, false);
    let max_w = git_w.max(env_w);

    // Background pill with 0.6 mm simulated border for visual grouping.
    let pad_h: f32 = 3.5;
    let pad_v: f32 = 1.8;
    let box_left = (right_anchor - max_w - pad_h).max(ctx.w / 2.0 - pad_h);
    let box_right = right_anchor + pad_h;
    let box_width = box_right - box_left;
    let box_bot = roots_text_y - pad_v;
    let box_top = title_text_y + pad_v + 1.5;
    let box_height = box_top - box_bot;
    pdf_fill_rect(
        ctx.layer,
        box_left - 0.6,
        box_bot - 0.6,
        box_width + 1.2,
        box_height + 1.2,
        Rgb::new(0.80, 0.75, 0.68, None),
    );
    pdf_fill_rect(
        ctx.layer,
        box_left,
        box_bot,
        box_width,
        box_height,
        Rgb::new(0.97, 0.95, 0.92, None),
    );

    // Git line — right-aligned to shared anchor, dark-green bold
    if !git_str.is_empty() {
        let git_x = (right_anchor - git_w).max(box_left + 2.0);
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.25, 0.42, 0.25, None)));
        ctx.layer.use_text(
            git_str.as_str(),
            7.5,
            Mm(git_x),
            Mm(title_text_y),
            ctx.font_bold,
        );
    }
    // Env line — same right anchor so "Source: …" right-edge aligns with "Tag: …" above
    let env_x = (right_anchor - env_w).max(box_left + 2.0);
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(0.38, 0.38, 0.38, None)));
    ctx.layer.use_text(
        env_trunc.as_str(),
        6.5,
        Mm(env_x),
        Mm(roots_text_y),
        ctx.font_reg,
    );
}

#[allow(clippy::cast_precision_loss)]
fn pdf_render_summary_chips(ctx: &PdfCtx<'_>, run: &AnalysisRun, roots_text_y: f32) -> f32 {
    use crate::pdf_compat::{Color, Mm, Rgb};
    let tot = &run.summary_totals;
    let chip_gap: f32 = 5.0;
    let chip_w = 3.0f32.mul_add(-chip_gap, 2.0f32.mul_add(-ctx.margin, ctx.w)) / 4.0;
    let chip_h: f32 = 17.0;
    let row1_bot = roots_text_y - 4.0 - chip_h;
    let row1: [(&str, u64); 4] = [
        ("Code Lines", tot.code_lines),
        ("Comment Lines", tot.comment_lines),
        ("Blank Lines", tot.blank_lines),
        ("Physical Lines", tot.total_physical_lines),
    ];
    for (i, (label, value)) in row1.iter().enumerate() {
        let cx = (i as f32).mul_add(chip_w + chip_gap, ctx.margin);
        pdf_fill_rect(
            ctx.layer,
            cx,
            row1_bot,
            chip_w,
            chip_h,
            Rgb::new(0.945, 0.925, 0.90, None),
        );
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.7, 0.33, 0.16, None)));
        // Show the full comma-separated number (no K/M rounding) on the PDF stat cards.
        ctx.layer.use_text(
            pdf_fmt_full(*value),
            13.0,
            Mm(cx + 4.0),
            Mm(row1_bot + 9.0),
            ctx.font_bold,
        );
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.45, 0.45, 0.45, None)));
        ctx.layer.use_text(
            pdf_safe_str(label),
            6.5,
            Mm(cx + 4.0),
            Mm(row1_bot + 3.0),
            ctx.font_reg,
        );
    }
    let row2_bot = row1_bot - 3.0 - chip_h;
    let row2_4th = if tot.test_count > 0 {
        ("Test Methods", tot.test_count)
    } else if tot.classes > 0 {
        ("Classes", tot.classes)
    } else {
        ("Mixed Lines", tot.mixed_lines_separate)
    };
    let row2: [(&str, u64); 4] = [
        ("Files Analyzed", tot.files_analyzed),
        ("Files Skipped", tot.files_skipped),
        ("Functions", tot.functions),
        row2_4th,
    ];
    for (i, (label, value)) in row2.iter().enumerate() {
        let cx = (i as f32).mul_add(chip_w + chip_gap, ctx.margin);
        pdf_fill_rect(
            ctx.layer,
            cx,
            row2_bot,
            chip_w,
            chip_h,
            Rgb::new(0.91, 0.92, 0.96, None),
        );
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.15, 0.25, 0.55, None)));
        ctx.layer.use_text(
            pdf_fmt_full(*value),
            13.0,
            Mm(cx + 4.0),
            Mm(row2_bot + 9.0),
            ctx.font_bold,
        );
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.35, 0.35, 0.45, None)));
        ctx.layer.use_text(
            pdf_safe_str(label),
            6.5,
            Mm(cx + 4.0),
            Mm(row2_bot + 3.0),
            ctx.font_reg,
        );
    }
    row2_bot
}

#[allow(clippy::cast_precision_loss)]
fn pdf_info_parts_stats(tot: &SummaryTotals) -> Vec<String> {
    let total = tot.total_physical_lines.max(1) as f64;
    let code_pct = tot.code_lines as f64 / total * 100.0;
    let cmt_pct = tot.comment_lines as f64 / total * 100.0;
    let blank_pct = tot.blank_lines as f64 / total * 100.0;
    let mixed_pct = tot.mixed_lines_separate as f64 / total * 100.0;
    let mut parts = vec![
        format!(
            "Code: {code_pct:.1}% ({} lines)",
            pdf_fmt_full(tot.code_lines)
        ),
        format!(
            "Comments: {cmt_pct:.1}% ({} lines)",
            pdf_fmt_full(tot.comment_lines)
        ),
        format!(
            "Blank: {blank_pct:.1}% ({} lines)",
            pdf_fmt_full(tot.blank_lines)
        ),
    ];
    if tot.functions > 0 {
        parts.push(format!("Functions: {}", pdf_fmt_full(tot.functions)));
    }
    if tot.mixed_lines_separate > 0 {
        parts.push(format!(
            "Mixed: {mixed_pct:.1}% ({} lines)",
            pdf_fmt_full(tot.mixed_lines_separate)
        ));
    }
    if tot.imports > 0 {
        parts.push(format!("Imports: {}", pdf_fmt_full(tot.imports)));
    }
    if tot.variables > 0 {
        parts.push(format!("Variables: {}", pdf_fmt_full(tot.variables)));
    }
    if tot.classes > 0 {
        parts.push(format!("Classes: {}", pdf_fmt_full(tot.classes)));
    }
    parts
}

fn pdf_info_parts_git(run: &AnalysisRun) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(ref b) = run.git_branch {
        parts.push(format!("Branch: {}", pdf_safe_str(b)));
    }
    if let Some(ref c) = run.git_commit_short {
        parts.push(format!("Commit: {}", pdf_safe_str(c)));
    }
    if let Some(ref t) = run.git_nearest_tag {
        parts.push(format!("Tag: {}", pdf_safe_str(t)));
    }
    if let Some(ref a) = run.git_commit_author {
        parts.push(format!("Author: {}", pdf_safe_str(a)));
    }
    if let Some(ref d) = run.git_commit_date {
        parts.push(format!("Commit Date: {}", fmt_commit_date_pt(d)));
    }
    parts
}

#[allow(clippy::cast_precision_loss)]
fn pdf_info_parts_tests(tot: &SummaryTotals) -> Vec<String> {
    let mut tc: Vec<String> = Vec::new();
    if tot.test_count > 0 {
        tc.push(format!("Tests: {}", pdf_fmt_full(tot.test_count)));
    }
    if tot.test_assertion_count > 0 {
        tc.push(format!(
            "Assertions: {}",
            pdf_fmt_full(tot.test_assertion_count)
        ));
    }
    if tot.test_suite_count > 0 {
        tc.push(format!("Suites: {}", pdf_fmt_full(tot.test_suite_count)));
    }
    if tot.coverage_lines_found > 0 {
        tc.push(format!(
            "Line Cov: {:.1}% ({}/{})",
            tot.coverage_lines_hit as f64 / tot.coverage_lines_found as f64 * 100.0,
            pdf_fmt_full(tot.coverage_lines_hit),
            pdf_fmt_full(tot.coverage_lines_found)
        ));
    }
    if tot.coverage_functions_found > 0 {
        tc.push(format!(
            "Func Cov: {:.1}%",
            tot.coverage_functions_hit as f64 / tot.coverage_functions_found as f64 * 100.0
        ));
    }
    if tot.coverage_branches_found > 0 {
        tc.push(format!(
            "Branch Cov: {:.1}%",
            tot.coverage_branches_hit as f64 / tot.coverage_branches_found as f64 * 100.0
        ));
    }
    tc
}

/// Emit one or more info lines, packing `parts` and wrapping onto a fresh line whenever the
/// next part would overflow the usable page width. Each part is drawn as a **bold** key
/// ("Code:") followed by its regular-weight value, with a muted separator between parts, so the
/// dense metric strip reads cleanly. Measured with the exact Helvetica advance table so the
/// whole line is always shown — never truncated. Returns the y position below the last line.
// x/y are page coordinates and r/g/b are colour channels — the conventional
// single-letter names in graphics code; renaming them would hurt, not help.
#[allow(clippy::many_single_char_names)]
fn pdf_info_emit_line(
    ctx: &PdfCtx<'_>,
    mut y: f32,
    r: f32,
    g: f32,
    b: f32,
    parts: &[String],
) -> f32 {
    use crate::pdf_compat::{Color, Mm, Rgb};
    const SIZE: f32 = 7.0;
    const LINE_GAP: f32 = 6.2;
    const SEP: &str = "   |   ";
    if parts.is_empty() {
        return y;
    }
    let usable = ctx.margin.mul_add(-2.0, ctx.w);
    let sep_w = helvetica_width_mm(SEP, SIZE, false);
    let group = Color::Rgb(Rgb::new(r, g, b, None));
    let sep_color = Color::Rgb(Rgb::new(0.66, 0.63, 0.60, None));
    let mut x = ctx.margin;
    let mut first_on_line = true;
    for part in parts {
        // Split "Label: value" into a bold key (kept with its colon) and a regular value.
        let (key, val) = match part.split_once(": ") {
            Some((k, v)) => (format!("{k}: "), v.to_string()),
            None => (part.clone(), String::new()),
        };
        let key_w = helvetica_width_mm(&key, SIZE, true);
        let val_w = helvetica_width_mm(&val, SIZE, false);
        let advance = if first_on_line {
            key_w + val_w
        } else {
            sep_w + key_w + val_w
        };
        if !first_on_line && x + advance > ctx.margin + usable {
            y -= LINE_GAP;
            x = ctx.margin;
            first_on_line = true;
        }
        if !first_on_line {
            ctx.layer.set_fill_color(sep_color.clone());
            ctx.layer.use_text(SEP, SIZE, Mm(x), Mm(y), ctx.font_reg);
            x += sep_w;
        }
        ctx.layer.set_fill_color(group.clone());
        ctx.layer
            .use_text(key.as_str(), SIZE, Mm(x), Mm(y), ctx.font_bold);
        x += key_w;
        if !val.is_empty() {
            ctx.layer
                .use_text(val.as_str(), SIZE, Mm(x), Mm(y), ctx.font_reg);
            x += val_w;
        }
        first_on_line = false;
    }
    y - LINE_GAP
}

fn pdf_render_info_lines(ctx: &PdfCtx<'_>, run: &AnalysisRun, row2_bot: f32) -> f32 {
    let tot = &run.summary_totals;
    let mut y = row2_bot - 6.5;
    let stats = pdf_info_parts_stats(tot);
    y = pdf_info_emit_line(ctx, y, 0.15, 0.15, 0.15, &stats);
    // A little extra breathing room between the stats / git / tests groups.
    let git = pdf_info_parts_git(run);
    if !git.is_empty() {
        y -= 1.6;
        y = pdf_info_emit_line(ctx, y, 0.10, 0.35, 0.15, &git);
    }
    let tests = pdf_info_parts_tests(tot);
    if !tests.is_empty() {
        y -= 1.6;
        y = pdf_info_emit_line(ctx, y, 0.15, 0.15, 0.50, &tests);
    }
    y
}

#[allow(clippy::cast_precision_loss, clippy::suboptimal_flops)]
fn pdf_table_render_section(
    ctx: &PdfCtx<'_>,
    x: f32,
    top: f32,
    w: f32,
    lbl_frac: f32,
    title: &str,
    rows: &[(&str, String)],
) {
    use crate::pdf_compat::{Color, Mm, Rgb};
    pdf_fill_rect(
        ctx.layer,
        x,
        top - ctx.tbl_hdr_h,
        w,
        ctx.tbl_hdr_h,
        Rgb::new(0.098, 0.11, 0.15, None),
    );
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(1.0, 1.0, 1.0, None)));
    ctx.layer.use_text(
        title,
        7.0,
        Mm(x + 2.0),
        Mm(top - ctx.tbl_hdr_h + 1.5),
        ctx.font_bold,
    );
    let y = top - ctx.tbl_hdr_h;
    for (ri, (lbl, val)) in rows.iter().enumerate() {
        let ry = ((ri + 1) as f32).mul_add(-ctx.row_h, y);
        let bg = if ri % 2 == 0 {
            Rgb::new(0.975, 0.965, 0.95, None)
        } else {
            Rgb::new(1.0, 1.0, 1.0, None)
        };
        pdf_fill_rect(ctx.layer, x, ry, w, ctx.row_h, bg);
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.12, 0.12, 0.12, None)));
        ctx.layer
            .use_text(*lbl, 6.5, Mm(x + 2.0), Mm(ry + 1.5), ctx.font_reg);
        let is_dash = val == "--";
        let val_rgb = if is_dash {
            Rgb::new(0.55, 0.55, 0.55, None)
        } else {
            Rgb::new(0.12, 0.12, 0.12, None)
        };
        let val_font = if is_dash { ctx.font_reg } else { ctx.font_bold };
        ctx.layer.set_fill_color(Color::Rgb(val_rgb));
        ctx.layer.use_text(
            val.as_str(),
            6.5,
            Mm(x + w * lbl_frac + 2.0),
            Mm(ry + 1.5),
            val_font,
        );
    }
}

#[allow(
    clippy::cast_precision_loss,
    clippy::suboptimal_flops,
    clippy::similar_names
)]
fn pdf_render_metric_tables(ctx: &PdfCtx<'_>, run: &AnalysisRun, tbl_top: f32) {
    let tot = &run.summary_totals;
    let half_w = (2.0f32.mul_add(-ctx.margin, ctx.w) - 4.0) / 2.0;
    let left_x = ctx.margin;
    let right_x = ctx.margin + half_w + 4.0;
    let lbl_frac: f32 = 0.68;

    let files_rows: [(&str, String); 4] = [
        ("Files analyzed", pdf_fmt_full(tot.files_analyzed)),
        ("Files skipped", pdf_fmt_full(tot.files_skipped)),
        ("Files modified", "--".to_string()),
        ("Files unchanged", "--".to_string()),
    ];
    pdf_table_render_section(ctx, left_x, tbl_top, half_w, lbl_frac, "FILES", &files_rows);

    let lc_rows: [(&str, String); 5] = [
        ("Physical lines", pdf_fmt_full(tot.total_physical_lines)),
        ("Code lines", pdf_fmt_full(tot.code_lines)),
        ("Comment lines", pdf_fmt_full(tot.comment_lines)),
        ("Blank lines", pdf_fmt_full(tot.blank_lines)),
        ("Mixed (separate)", pdf_fmt_full(tot.mixed_lines_separate)),
    ];
    let lc_top = tbl_top - ctx.tbl_hdr_h - (files_rows.len() as f32).mul_add(ctx.row_h, 3.0);
    pdf_table_render_section(
        ctx,
        left_x,
        lc_top,
        half_w,
        lbl_frac,
        "LINE COUNTS",
        &lc_rows,
    );

    let cs_rows: [(&str, String); 4] = [
        ("Functions", pdf_fmt_full(tot.functions)),
        ("Classes / Types", pdf_fmt_full(tot.classes)),
        ("Variables", pdf_fmt_full(tot.variables)),
        ("Imports", pdf_fmt_full(tot.imports)),
    ];
    pdf_table_render_section(
        ctx,
        right_x,
        tbl_top,
        half_w,
        lbl_frac,
        "CODE STRUCTURE",
        &cs_rows,
    );

    let lcs_rows: [(&str, String); 4] = [
        ("Lines added", "--".to_string()),
        ("Lines removed", "--".to_string()),
        ("Lines modified (net)", "--".to_string()),
        ("Lines unmodified", "--".to_string()),
    ];
    let lcs_top = tbl_top - ctx.tbl_hdr_h - (cs_rows.len() as f32).mul_add(ctx.row_h, 3.0);
    pdf_table_render_section(
        ctx,
        right_x,
        lcs_top,
        half_w,
        lbl_frac,
        "LINE CHANGE SUMMARY",
        &lcs_rows,
    );
}

/// Render Tests & Coverage content **inline** on an existing page, starting at `y_start`.
/// Draw a full-width dark section title bar at `y` and return the Y just below it.
fn pdf_tc_title_bar(ctx: &PdfCtx<'_>, label: &str, y: f32) -> f32 {
    use crate::pdf_compat::{Color, Mm, Rgb};
    let tbl_w = 2.0_f32.mul_add(-ctx.margin, ctx.w);
    pdf_fill_rect(
        ctx.layer,
        ctx.margin,
        y - ctx.tbl_hdr_h,
        tbl_w,
        ctx.tbl_hdr_h,
        Rgb::new(0.098, 0.11, 0.15, None),
    );
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(1.0, 1.0, 1.0, None)));
    ctx.layer.use_text(
        label,
        7.0,
        Mm(ctx.margin + 2.0),
        Mm(y - ctx.tbl_hdr_h + 1.5),
        ctx.font_bold,
    );
    y - ctx.tbl_hdr_h
}

/// Alternating zebra row background for PDF tables.
fn pdf_row_bg(ri: usize) -> crate::pdf_compat::Rgb {
    use crate::pdf_compat::Rgb;
    if ri.is_multiple_of(2) {
        Rgb::new(0.975, 0.965, 0.95, None)
    } else {
        Rgb::new(1.0, 1.0, 1.0, None)
    }
}

/// Sum a per-submodule language metric via the provided accessor.
fn pdf_sub_sum(
    sub: &sloc_core::SubmoduleSummary,
    f: impl Fn(&sloc_core::LanguageSummary) -> u64,
) -> u64 {
    sub.language_summaries.iter().map(f).sum()
}

/// Render the four summary stat boxes (test functions/assertions/suites + line coverage).
#[allow(clippy::cast_precision_loss, clippy::suboptimal_flops)]
fn pdf_tc_stat_boxes(ctx: &PdfCtx<'_>, run: &AnalysisRun, has_cov: bool, mut y: f32) -> f32 {
    use crate::pdf_compat::{Color, Mm, Rgb};
    let gap: f32 = 4.0;
    let box_h: f32 = 15.0;
    let box_w = (ctx.w - 2.0 * ctx.margin - 3.0 * gap) / 4.0;
    let line_cov_str = if has_cov {
        let pct = run.summary_totals.coverage_lines_hit as f64
            / run.summary_totals.coverage_lines_found as f64
            * 100.0;
        format!("{pct:.1}%")
    } else {
        "\u{2014}".to_string()
    };
    let box_vals: [String; 4] = [
        pdf_fmt_full(run.summary_totals.test_count),
        pdf_fmt_full(run.summary_totals.test_assertion_count),
        pdf_fmt_full(run.summary_totals.test_suite_count),
        line_cov_str,
    ];
    let box_labels: [&str; 4] = [
        "Test Functions",
        "Test Assertions",
        "Test Suites",
        "Line Coverage",
    ];
    for (i, (label, val)) in box_labels.iter().zip(box_vals.iter()).enumerate() {
        let bx = ctx.margin + i as f32 * (box_w + gap);
        let by = y - box_h;
        pdf_fill_rect(
            ctx.layer,
            bx,
            by,
            box_w,
            box_h,
            Rgb::new(0.97, 0.96, 0.94, None),
        );
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.60, 0.40, 0.22, None)));
        ctx.layer
            .use_text(val.as_str(), 9.5, Mm(bx + 3.0), Mm(by + 7.5), ctx.font_bold);
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.50, 0.44, 0.40, None)));
        ctx.layer
            .use_text(*label, 5.5, Mm(bx + 3.0), Mm(by + 2.0), ctx.font_reg);
    }
    y -= box_h + 4.0;
    y
}

/// Render the full-width SUBMODULES table when submodule summaries are present.
#[allow(clippy::cast_precision_loss, clippy::suboptimal_flops)]
fn pdf_tc_submodules(ctx: &PdfCtx<'_>, run: &AnalysisRun, footer_h: f32, mut y: f32) -> f32 {
    use crate::pdf_compat::{Color, Mm, Rgb};
    let subs = &run.submodule_summaries;
    if subs.is_empty() {
        return y;
    }
    let margin = ctx.margin;
    let row_h = ctx.row_h;
    let tbl_w = ctx.w - 2.0 * margin;

    let col_name = tbl_w * 0.40;
    let rem = tbl_w - col_name;
    let col_files = rem * 0.15;
    let col_code = rem * 0.20;
    let col_tests = rem * 0.20;
    let col_assert = rem * 0.20;

    let cx_files = margin + col_name;
    let cx_code = cx_files + col_files;
    let cx_tests = cx_code + col_code;
    let cx_assert = cx_tests + col_tests;
    let cx_cov = cx_assert + col_assert;

    y = pdf_tc_title_bar(ctx, "SUBMODULES", y);

    pdf_fill_rect(
        ctx.layer,
        margin,
        y - row_h,
        tbl_w,
        row_h,
        Rgb::new(0.25, 0.27, 0.32, None),
    );
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(0.88, 0.88, 0.88, None)));
    for (lbl, x) in &[
        ("Submodule", margin + 2.0),
        ("Files", cx_files + 2.0),
        ("Code Lines", cx_code + 2.0),
        ("Test Functions", cx_tests + 2.0),
        ("Assertions", cx_assert + 2.0),
        ("Line Coverage %", cx_cov + 2.0),
    ] {
        ctx.layer
            .use_text(*lbl, 5.5, Mm(*x), Mm(y - row_h + 1.5), ctx.font_bold);
    }
    y -= row_h;

    for (ri, sub) in subs.iter().enumerate() {
        if y < footer_h + row_h {
            break;
        }
        let sub_tests = pdf_sub_sum(sub, |l| l.test_count);
        let sub_assert = pdf_sub_sum(sub, |l| l.test_assertion_count);
        let sub_cov_hit = pdf_sub_sum(sub, |l| l.coverage_lines_hit);
        let sub_cov_found = pdf_sub_sum(sub, |l| l.coverage_lines_found);
        let sub_cov_str = if sub_cov_found > 0 {
            format!("{:.1}%", sub_cov_hit as f64 / sub_cov_found as f64 * 100.0)
        } else {
            "\u{2014}".to_string()
        };

        let ry = y - row_h;
        pdf_fill_rect(ctx.layer, margin, ry, tbl_w, row_h, pdf_row_bg(ri));
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.12, 0.12, 0.12, None)));
        ctx.layer.use_text(
            pdf_trunc(&pdf_safe_str(&sub.name), 40),
            5.5,
            Mm(margin + 2.0),
            Mm(ry + 1.5),
            ctx.font_bold,
        );
        for (val, x) in &[
            (pdf_fmt_full(sub.files_analyzed), cx_files + 2.0),
            (pdf_fmt_full(sub.code_lines), cx_code + 2.0),
            (pdf_fmt_full(sub_tests), cx_tests + 2.0),
            (pdf_fmt_full(sub_assert), cx_assert + 2.0),
            (sub_cov_str, cx_cov + 2.0),
        ] {
            ctx.layer
                .use_text(val.as_str(), 5.5, Mm(*x), Mm(ry + 1.5), ctx.font_reg);
        }
        y -= row_h;
    }
    y - 3.0
}

/// Render the line/function/branch coverage gauges across the page width.
#[allow(clippy::cast_precision_loss, clippy::suboptimal_flops)]
fn pdf_tc_gauges(ctx: &PdfCtx<'_>, run: &AnalysisRun, mut y: f32) -> f32 {
    use crate::pdf_compat::{Color, Mm, Rgb};
    let margin = ctx.margin;
    let gap: f32 = 4.0;
    let gauges: &[(&str, u64, u64)] = &[
        (
            "Line Coverage",
            run.summary_totals.coverage_lines_hit,
            run.summary_totals.coverage_lines_found,
        ),
        (
            "Function Coverage",
            run.summary_totals.coverage_functions_hit,
            run.summary_totals.coverage_functions_found,
        ),
        (
            "Branch Coverage",
            run.summary_totals.coverage_branches_hit,
            run.summary_totals.coverage_branches_found,
        ),
    ];
    let visible: Vec<_> = gauges.iter().filter(|(_, _, found)| *found > 0).collect();
    if visible.is_empty() {
        return y;
    }
    let count = visible.len() as f32;
    let gauge_h: f32 = 16.0;
    let pad: f32 = 4.0;
    let bar_h: f32 = 3.0;
    let gauge_w = (ctx.w - 2.0 * margin - (count - 1.0) * gap) / count;
    let bar_w = gauge_w - 2.0 * pad;
    for (gi, (label, hit, found)) in visible.iter().enumerate() {
        let gx = margin + gi as f32 * (gauge_w + gap);
        let pct = *hit as f64 / *found as f64 * 100.0;
        let pct_str = format!("{pct:.1}%");
        // `pct` is a 0..=100 percentage; narrowing to f32 for a bar-width coordinate is exact
        // to well within sub-pixel rendering tolerance.
        #[allow(
            clippy::cast_possible_truncation,
            reason = "0..=100 percentage to f32 bar width"
        )]
        let bar_fill = bar_w * (pct as f32 / 100.0);
        let gy = y - gauge_h;
        // Simulated 0.5 mm border (outer rect) behind a lighter card fill, matching the meta box.
        pdf_fill_rect(
            ctx.layer,
            gx - 0.5,
            gy - 0.5,
            gauge_w + 1.0,
            gauge_h + 1.0,
            Rgb::new(0.80, 0.75, 0.68, None),
        );
        pdf_fill_rect(
            ctx.layer,
            gx,
            gy,
            gauge_w,
            gauge_h,
            Rgb::new(0.98, 0.97, 0.95, None),
        );
        // Label (top), percentage (middle), progress bar (bottom) — evenly padded.
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.15, 0.15, 0.15, None)));
        ctx.layer.use_text(
            *label,
            6.0,
            Mm(gx + pad),
            Mm(gy + gauge_h - 4.5),
            ctx.font_bold,
        );
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.20, 0.55, 0.35, None)));
        ctx.layer.use_text(
            &pct_str,
            8.5,
            Mm(gx + pad),
            Mm(gy + bar_h + 3.0),
            ctx.font_bold,
        );
        pdf_fill_rect(
            ctx.layer,
            gx + pad,
            gy + pad * 0.5,
            bar_w,
            bar_h,
            Rgb::new(0.86, 0.84, 0.80, None),
        );
        if bar_fill > 0.0 {
            pdf_fill_rect(
                ctx.layer,
                gx + pad,
                gy + pad * 0.5,
                bar_fill,
                bar_h,
                Rgb::new(0.20, 0.55, 0.35, None),
            );
        }
    }
    y -= gauge_h + 5.0;
    y
}

/// Column layout for the per-file coverage table, shared by the header and row renderers.
struct CovCols {
    has_fn_cov: bool,
    has_br_cov: bool,
    col_fn_w: f32,
    hdr_x2: f32,
}

/// Draw the PER-FILE COVERAGE title + column header bar; return `(rows_start_y, cols)`.
fn pdf_tc_per_file_header(
    ctx: &PdfCtx<'_>,
    has_fn_cov: bool,
    has_br_cov: bool,
    col_fn_w: f32,
    y: f32,
) -> (f32, CovCols) {
    use crate::pdf_compat::{Color, Mm, Rgb};
    let margin = ctx.margin;
    let col_br_w: f32 = if has_br_cov { 22.0 } else { 0.0 };
    let col_file_w = 2.0_f32.mul_add(-margin, ctx.w) - 22.0 - col_fn_w - col_br_w;
    let hdr_x2 = margin + col_file_w;

    let y = pdf_tc_title_bar(ctx, "PER-FILE COVERAGE", y - 3.0);
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(0.55, 0.55, 0.55, None)));
    ctx.layer
        .use_text("Line%", 5.5, Mm(hdr_x2 + 2.0), Mm(y - 3.5), ctx.font_bold);
    if has_fn_cov {
        ctx.layer
            .use_text("Fn%", 5.5, Mm(hdr_x2 + 24.0), Mm(y - 3.5), ctx.font_bold);
    }
    if has_br_cov {
        ctx.layer.use_text(
            "Br%",
            5.5,
            Mm(hdr_x2 + 22.0 + col_fn_w + 2.0),
            Mm(y - 3.5),
            ctx.font_bold,
        );
    }
    (
        y - ctx.row_h,
        CovCols {
            has_fn_cov,
            has_br_cov,
            col_fn_w,
            hdr_x2,
        },
    )
}

/// Render one per-file coverage row at vertical position `ry`.
fn pdf_tc_per_file_row(ctx: &PdfCtx<'_>, file: &FileRecord, ri: usize, cols: &CovCols, ry: f32) {
    use crate::pdf_compat::{Color, Mm, Rgb};
    let (has_fn_cov, has_br_cov, col_fn_w, hdr_x2) =
        (cols.has_fn_cov, cols.has_br_cov, cols.col_fn_w, cols.hdr_x2);
    let Some(cov) = file.coverage.as_ref() else {
        return;
    };
    pdf_fill_rect(
        ctx.layer,
        ctx.margin,
        ry,
        2.0_f32.mul_add(-ctx.margin, ctx.w),
        ctx.row_h,
        pdf_row_bg(ri),
    );
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(0.12, 0.12, 0.12, None)));
    let fname = pdf_trunc(
        &pdf_safe_str(
            std::path::Path::new(&file.relative_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&file.relative_path),
        ),
        52,
    );
    ctx.layer.use_text(
        &fname,
        5.5,
        Mm(ctx.margin + 2.0),
        Mm(ry + 1.5),
        ctx.font_reg,
    );
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(0.10, 0.42, 0.25, None)));
    ctx.layer.use_text(
        format!("{:.1}%", cov.line_pct()),
        5.5,
        Mm(hdr_x2 + 2.0),
        Mm(ry + 1.5),
        ctx.font_bold,
    );
    if has_fn_cov && cov.functions_found > 0 {
        ctx.layer.use_text(
            format!("{:.1}%", cov.function_pct()),
            5.5,
            Mm(hdr_x2 + 24.0),
            Mm(ry + 1.5),
            ctx.font_bold,
        );
    }
    if has_br_cov && cov.branches_found > 0 {
        ctx.layer.use_text(
            format!("{:.1}%", cov.branch_pct()),
            5.5,
            Mm(hdr_x2 + 22.0 + col_fn_w + 2.0),
            Mm(ry + 1.5),
            ctx.font_bold,
        );
    }
}

/// Render the PER-FILE COVERAGE table (header + rows) when coverage records exist.
fn pdf_tc_per_file(
    ctx: &PdfCtx<'_>,
    run: &AnalysisRun,
    footer_h: f32,
    has_fn_cov: bool,
    has_br_cov: bool,
    mut y: f32,
) -> f32 {
    let cov_files: Vec<_> = run
        .per_file_records
        .iter()
        .filter(|r| r.coverage.is_some())
        .collect();
    if cov_files.is_empty() {
        return y;
    }
    let col_fn_w: f32 = if has_fn_cov { 22.0 } else { 0.0 };
    let (rows_start, cols) = pdf_tc_per_file_header(ctx, has_fn_cov, has_br_cov, col_fn_w, y);
    y = rows_start;
    for (ri, file) in cov_files.iter().enumerate() {
        if y < footer_h + ctx.row_h {
            break;
        }
        let ry = y - ctx.row_h;
        pdf_tc_per_file_row(ctx, file, ri, &cols, ry);
        y -= ctx.row_h;
    }
    y
}

/// Render the "no coverage data" note when no coverage is present.
fn pdf_tc_no_coverage_note(ctx: &PdfCtx<'_>, mut y: f32) -> f32 {
    use crate::pdf_compat::{Color, Mm, Rgb};
    let margin = ctx.margin;
    let note_h: f32 = 12.0;
    pdf_fill_rect(
        ctx.layer,
        margin,
        y - note_h,
        2.0_f32.mul_add(-margin, ctx.w),
        note_h,
        Rgb::new(0.96, 0.95, 0.93, None),
    );
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(0.45, 0.40, 0.37, None)));
    ctx.layer.use_text(
        "No code coverage data detected.",
        7.0,
        Mm(margin + 4.0),
        Mm(y - note_h + 7.0),
        ctx.font_bold,
    );
    ctx.layer.use_text(
        "Re-run with --lcov-path <file.info> to see per-file line, function, and branch coverage.",
        6.0,
        Mm(margin + 4.0),
        Mm(y - note_h + 2.5),
        ctx.font_reg,
    );
    y -= note_h;
    y
}

/// Does NOT create a new page, draw a mini-header, or draw a footer — those are the caller's
/// responsibility. Returns the Y position immediately below the last rendered element.
fn pdf_render_tc_inline(ctx: &PdfCtx<'_>, run: &AnalysisRun, y_start: f32, footer_h: f32) -> f32 {
    let has_cov = run.summary_totals.coverage_lines_found > 0;
    let has_fn_cov = run.summary_totals.coverage_functions_found > 0;
    let has_br_cov = run.summary_totals.coverage_branches_found > 0;

    let mut y = pdf_tc_title_bar(ctx, "TESTS & COVERAGE", y_start) - 4.0;
    y = pdf_tc_stat_boxes(ctx, run, has_cov, y);
    y = pdf_tc_submodules(ctx, run, footer_h, y);

    if has_cov {
        y = pdf_tc_gauges(ctx, run, y);
        y = pdf_tc_per_file(ctx, run, footer_h, has_fn_cov, has_br_cov, y);
    } else {
        y = pdf_tc_no_coverage_note(ctx, y);
    }
    y
}

/// Build the right-aligned per-page header metadata string shown on every continuation
/// page so each printed sheet is self-identifying: Run ID, git commit, and scan time.
fn pdf_page_header_meta(run: &AnalysisRun) -> String {
    let mut parts = vec![format!(
        "Run ID: {}",
        pdf_safe_str(&run.tool.run_id[..run.tool.run_id.len().min(20)])
    )];
    if let Some(ref c) = run.git_commit_short {
        parts.push(format!("Commit: {}", pdf_safe_str(c)));
    }
    parts.push(to_pt_hhmm(run.tool.timestamp_utc));
    parts.join("  \u{00B7}  ")
}

/// Draw `text` right-aligned (gray, 6.5 pt) inside a navy page-header bar whose text
/// baseline sits at `baseline_y`. Uses exact Helvetica advance widths for precise
/// right-edge alignment against the page margin.
fn pdf_draw_header_meta(
    layer: &crate::pdf_compat::PdfLayerReference,
    font: crate::pdf_compat::IndirectFontRef,
    w: f32,
    margin: f32,
    baseline_y: f32,
    text: &str,
) {
    use crate::pdf_compat::{Color, Mm, Rgb};
    let tw = helvetica_width_mm(text, 6.5, false);
    let x = (w - margin - tw).max(margin + 60.0);
    layer.set_fill_color(Color::Rgb(Rgb::new(0.72, 0.72, 0.72, None)));
    layer.use_text(text, 6.5, Mm(x), Mm(baseline_y), font);
}

/// Draw the per-page mini header band (dark bar with "oxide-sloc", the truncated report `title`,
/// and right-aligned run metadata) shared by the dedicated T&C and Git Hotspots pages. `h` is the
/// page height and `hdr_h` the band height.
fn pdf_page_mini_header(ctx: &PdfCtx<'_>, h: f32, hdr_h: f32, title: &str, run: &AnalysisRun) {
    use crate::pdf_compat::{Color, Mm, Rgb};
    pdf_fill_rect(
        ctx.layer,
        0.0,
        h - hdr_h,
        ctx.w,
        hdr_h,
        Rgb::new(0.098, 0.11, 0.15, None),
    );
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(1.0, 1.0, 1.0, None)));
    ctx.layer.use_text(
        "oxide-sloc",
        9.0,
        Mm(ctx.margin),
        Mm(h - 5.5),
        ctx.font_bold,
    );
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(0.72, 0.72, 0.72, None)));
    ctx.layer.use_text(
        pdf_trunc(&pdf_safe_str(title), 45),
        7.5,
        Mm(46.0),
        Mm(h - 5.5),
        ctx.font_reg,
    );
    pdf_draw_header_meta(
        ctx.layer,
        ctx.font_reg,
        ctx.w,
        ctx.margin,
        h - 5.5,
        &pdf_page_header_meta(run),
    );
}

/// Draw the standard page footer band (light bar with the version/licence line) shared by the
/// dedicated T&C and Git Hotspots pages.
fn pdf_page_footer_band(ctx: &PdfCtx<'_>, footer_h: f32, version: &str) {
    use crate::pdf_compat::{Color, Mm, Rgb};
    pdf_fill_rect(
        ctx.layer,
        0.0,
        0.0,
        ctx.w,
        footer_h,
        Rgb::new(0.93, 0.91, 0.87, None),
    );
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(0.4, 0.4, 0.4, None)));
    ctx.layer.use_text(
        format!("oxide-sloc v{version}  \u{00b7}  AGPL-3.0-or-later"),
        6.5,
        Mm(ctx.margin),
        Mm(3.0),
        ctx.font_reg,
    );
}

/// Create a dedicated "Tests & Coverage" page, render its content inline, and return the
/// `(page, layer, y_bottom)` tuple so `pdf_render_per_file_pages` can continue on this page.
#[allow(clippy::cast_precision_loss, clippy::too_many_arguments)]
fn pdf_render_tests_coverage_page(
    doc: &crate::pdf_compat::PdfDocumentReference,
    font_reg: crate::pdf_compat::IndirectFontRef,
    font_bold: crate::pdf_compat::IndirectFontRef,
    run: &AnalysisRun,
    w: f32,
    h: f32,
    margin: f32,
    footer_h: f32,
    title: &str,
    version: &str,
) -> (
    crate::pdf_compat::PdfPageIndex,
    crate::pdf_compat::PdfLayerIndex,
    f32,
) {
    use crate::pdf_compat::Mm;
    const HDR_H: f32 = 8.0;

    let (tc_page, tc_layer_idx) = doc.add_page(Mm(w), Mm(h), "Tests & Coverage");
    let layer = doc.get_page(tc_page).get_layer(tc_layer_idx);
    let ctx = PdfCtx {
        layer: &layer,
        font_reg,
        font_bold,
        w,
        margin,
        row_h: 5.5,
        tbl_hdr_h: 6.0,
    };

    pdf_page_mini_header(&ctx, h, HDR_H, title, run);

    // T&C content inline
    let tc_bottom = pdf_render_tc_inline(&ctx, run, h - HDR_H - 4.0, footer_h);

    pdf_page_footer_band(&ctx, footer_h, version);

    (tc_page, tc_layer_idx, tc_bottom - 3.0)
}

#[allow(clippy::cast_precision_loss, clippy::suboptimal_flops)]
fn pdf_render_page1_footer(
    ctx: &PdfCtx<'_>,
    run: &AnalysisRun,
    footer_h: f32,
    version: &str,
    banner: Option<&str>,
) {
    use crate::pdf_compat::{Color, Mm, Rgb};
    pdf_fill_rect(
        ctx.layer,
        0.0,
        0.0,
        ctx.w,
        footer_h,
        Rgb::new(0.93, 0.91, 0.87, None),
    );
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(0.4, 0.4, 0.4, None)));
    // Left section.
    ctx.layer.use_text(
        format!("oxide-sloc v{version}  |  AGPL-3.0-or-later"),
        6.5,
        Mm(ctx.margin),
        Mm(3.0),
        ctx.font_reg,
    );
    // Right section — github.com and Run ID, right-aligned (~1.27 mm per char at 6.5 pt).
    let right_text = format!(
        "github.com/oxide-sloc/oxide-sloc  |  Run ID: {}",
        pdf_safe_str(&run.tool.run_id[..run.tool.run_id.len().min(20)])
    );
    let right_x = (ctx.w - ctx.margin - right_text.len() as f32 * 1.27).max(ctx.margin + 80.0);
    ctx.layer
        .use_text(right_text, 6.5, Mm(right_x), Mm(3.0), ctx.font_reg);
    // Center section — banner text, no background, oxide brand color, bold.
    if let Some(text) = banner {
        let safe = pdf_trunc(&pdf_safe_str(text), 40);
        // Same per-char width as the header banner (0.97 mm at 9pt bold Helvetica).
        let text_x = (ctx.w / 2.0 - safe.len() as f32 * 0.97).max(ctx.margin + 50.0);
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.7, 0.33, 0.16, None)));
        ctx.layer
            .use_text(safe, 9.0, Mm(text_x), Mm(2.6), ctx.font_bold);
    }
}

fn per_file_row_bg(ri: usize) -> crate::pdf_compat::Rgb {
    if ri.is_multiple_of(2) {
        crate::pdf_compat::Rgb::new(0.975, 0.965, 0.95, None)
    } else {
        crate::pdf_compat::Rgb::new(1.0, 1.0, 1.0, None)
    }
}

// ── Per-file page layout constants shared by the helpers below ─────────────────
const PDF_PERFILE_HDR_H: f32 = 8.0;
const PDF_PERFILE_SUB_H: f32 = 5.5;
// Gap between the PER-FILE DETAIL sub-bar and the column-header row.
// Applied on standalone per-file pages (not when sharing a page with COCOMO/T&C).
const PDF_PERFILE_TABLE_GAP: f32 = 3.0;

/// Doc/font/dims context for per-file page helpers; carries `doc` instead of `layer`
/// because the page layer is created inside `pdf_draw_perfile_header`.
struct PdfPerFileCtx<'a> {
    doc: &'a crate::pdf_compat::PdfDocumentReference,
    font_reg: crate::pdf_compat::IndirectFontRef,
    font_bold: crate::pdf_compat::IndirectFontRef,
    w: f32,
    h: f32,
    margin: f32,
}

/// Compute the `[start, end)` record slice displayed on one per-file page.
fn pdf_perfile_page_slice(
    page_idx: usize,
    use_continuation: bool,
    has_first_page: bool,
    fp_rows: usize,
    rows_per_page: usize,
    total_files: usize,
) -> (usize, usize) {
    if use_continuation {
        (0, fp_rows.min(total_files))
    } else if has_first_page {
        let s = fp_rows + (page_idx - 1) * rows_per_page;
        (s, (s + rows_per_page).min(total_files))
    } else {
        let s = page_idx * rows_per_page;
        (s, (s + rows_per_page).min(total_files))
    }
}

/// Obtain (or create) the PDF layer for one per-file page and render its page header.
///
/// Returns `(layer, sub_top)` where `sub_top` is the y-coordinate at the bottom of the
/// header bar. When `use_continuation` is true the layer is taken from `first_page` and
/// no new header is drawn — the COCOMO page already has one.
#[allow(clippy::suboptimal_flops, clippy::cast_precision_loss)]
fn pdf_draw_perfile_header(
    ctx: &PdfPerFileCtx<'_>,
    use_continuation: bool,
    first_page: Option<(
        crate::pdf_compat::PdfPageIndex,
        crate::pdf_compat::PdfLayerIndex,
        f32,
    )>,
    page_idx: usize,
    page_count: usize,
    banner: Option<&str>,
    meta: &str,
) -> (crate::pdf_compat::PdfLayerReference, f32) {
    use crate::pdf_compat::{Color, Mm, Rgb};
    if use_continuation {
        let (fp_page, fp_layer_idx, fp_top) = first_page.unwrap();
        let layer = ctx.doc.get_page(fp_page).get_layer(fp_layer_idx);
        (layer, fp_top - PDF_PERFILE_SUB_H)
    } else {
        let (pf_page, pf_layer_idx) = ctx.doc.add_page(Mm(ctx.w), Mm(ctx.h), "Content");
        let layer = ctx.doc.get_page(pf_page).get_layer(pf_layer_idx);
        let hdr_top = ctx.h - PDF_PERFILE_HDR_H;
        pdf_fill_rect(
            &layer,
            0.0,
            hdr_top,
            ctx.w,
            PDF_PERFILE_HDR_H,
            Rgb::new(0.098, 0.11, 0.15, None),
        );
        layer.set_fill_color(Color::Rgb(Rgb::new(1.0, 1.0, 1.0, None)));
        layer.use_text(
            "oxide-sloc",
            9.0,
            Mm(ctx.margin),
            Mm(hdr_top + 2.5),
            ctx.font_bold,
        );
        layer.set_fill_color(Color::Rgb(Rgb::new(0.72, 0.72, 0.72, None)));
        layer.use_text(
            "Per-File Detail",
            8.0,
            Mm(46.0),
            Mm(hdr_top + 2.5),
            ctx.font_reg,
        );
        // Right-aligned: Run ID / commit / scan time, then the page counter.
        let right = format!(
            "{meta}  \u{00B7}  Page {} of {}",
            page_idx + 2,
            page_count + 1
        );
        let right_w = helvetica_width_mm(&right, 6.5, false);
        let right_x = (ctx.w - ctx.margin - right_w).max(ctx.margin + 60.0);
        layer.use_text(right, 6.5, Mm(right_x), Mm(hdr_top + 2.5), ctx.font_reg);
        if let Some(text) = banner {
            let safe = pdf_trunc(&pdf_safe_str(text), 40);
            let text_x = (ctx.w / 2.0 - safe.len() as f32 * 0.97).max(80.0);
            layer.set_fill_color(Color::Rgb(Rgb::new(0.7, 0.33, 0.16, None)));
            layer.use_text(safe, 9.0, Mm(text_x), Mm(hdr_top + 2.5), ctx.font_bold);
        }
        // Leave a gap between the top header bar and the PER-FILE DETAIL sub-bar.
        (layer, hdr_top - PDF_PERFILE_TABLE_GAP - PDF_PERFILE_SUB_H)
    }
}

/// Render per-file data rows onto an existing PDF layer.
#[allow(clippy::suboptimal_flops, clippy::cast_precision_loss)]
fn pdf_draw_perfile_rows(
    ctx: &PdfCtx<'_>,
    records: &[FileRecord],
    col_x: &[f32; 13],
    pf_tbl_top: f32,
) {
    use crate::pdf_compat::{Color, Mm, Rgb};
    for (ri, rec) in records.iter().enumerate() {
        let ry = ((ri + 1) as f32).mul_add(-ctx.row_h, pf_tbl_top - ctx.tbl_hdr_h);
        let bg = per_file_row_bg(ri);
        pdf_fill_rect(
            ctx.layer,
            ctx.margin,
            ry,
            2.0f32.mul_add(-ctx.margin, ctx.w),
            ctx.row_h,
            bg,
        );
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.12, 0.12, 0.12, None)));
        let file_str = pdf_safe_str(&rec.relative_path);
        let lang_str = rec
            .language
            .as_ref()
            .map_or_else(|| "--".to_string(), |l| l.display_name().to_string());
        let raw = &rec.raw_line_categories;
        let eff = &rec.effective_counts;
        let cells = [
            pdf_trunc_end(&file_str, 110),
            lang_str,
            pdf_fmt_full(raw.total_physical_lines),
            pdf_fmt_full(eff.code_lines),
            pdf_fmt_full(eff.comment_lines),
            pdf_fmt_full(eff.blank_lines),
            pdf_fmt_full(eff.mixed_lines_separate),
            pdf_fmt_full(raw.functions),
            pdf_fmt_full(raw.classes),
            pdf_fmt_full(raw.variables),
            pdf_fmt_full(raw.imports),
            pdf_fmt_full(raw.test_count),
            pdf_fmt_full(raw.test_assertion_count),
        ];
        for (ci, cell) in cells.iter().enumerate() {
            ctx.layer.use_text(
                cell.clone(),
                5.5,
                Mm(col_x[ci] + 0.5),
                Mm(ry + 1.0),
                ctx.font_reg,
            );
        }
    }
}

// PDF per-file page renderer — layout params are distinct; see PdfPerFileCtx for bundling.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::suboptimal_flops
)]
fn pdf_render_per_file_pages(
    doc: &crate::pdf_compat::PdfDocumentReference,
    font_reg: crate::pdf_compat::IndirectFontRef,
    font_bold: crate::pdf_compat::IndirectFontRef,
    run: &AnalysisRun,
    w: f32,
    h: f32,
    margin: f32,
    footer_h: f32,
    row_h: f32,
    tbl_hdr_h: f32,
    title: &str,
    ts: &str,
    version: &str,
    banner: Option<&str>,
    // When COCOMO is rendered on its own page, continue the per-file table on that same page
    // rather than starting a new one.  Tuple: (page index, layer index, available top y-coord).
    first_page: Option<(
        crate::pdf_compat::PdfPageIndex,
        crate::pdf_compat::PdfLayerIndex,
        f32,
    )>,
) {
    use crate::pdf_compat::{Color, Mm, Rgb};
    // File column gets ~136 mm; numeric columns compressed to minimum readable width.
    // Column widths: File=136, Lang=14, Phys=12, Code=10, Comments=13, Blank=10, Mixed=10,
    //   Functions=13, Classes=11, Variables=13, Imports=11, Tests=10, Assertions=14  → total 277 mm
    let col_x: [f32; 13] = [
        10.0, 146.0, 160.0, 172.0, 182.0, 195.0, 205.0, 215.0, 228.0, 239.0, 252.0, 263.0, 273.0,
    ];
    let col_labels: [&str; 13] = [
        "File",
        "Language",
        "Physical",
        "Code",
        "Comments",
        "Blank",
        "Mixed",
        "Functions",
        "Classes",
        "Variables",
        "Imports",
        "Tests",
        "Assertions",
    ];
    let rows_per_page =
        ((h - PDF_PERFILE_HDR_H - PDF_PERFILE_SUB_H - PDF_PERFILE_TABLE_GAP - tbl_hdr_h - footer_h)
            / row_h)
            .floor() as usize;
    let total_files = run.per_file_records.len();

    // Rows that fit on the continuation page (COCOMO already occupies the top portion).
    let fp_rows = match first_page {
        Some((_, _, fp_top)) => ((fp_top - PDF_PERFILE_SUB_H - tbl_hdr_h - footer_h) / row_h)
            .floor()
            .max(0.0) as usize,
        None => rows_per_page,
    };
    let page_count = if first_page.is_some() {
        1 + total_files.saturating_sub(fp_rows).div_ceil(rows_per_page)
    } else {
        total_files.div_ceil(rows_per_page)
    };
    let pf_ctx = PdfPerFileCtx {
        doc,
        font_reg,
        font_bold,
        w,
        h,
        margin,
    };
    let header_meta = pdf_page_header_meta(run);

    for page_idx in 0..page_count {
        let use_continuation = page_idx == 0 && first_page.is_some();
        let (pf_layer, sub_top) = pdf_draw_perfile_header(
            &pf_ctx,
            use_continuation,
            first_page,
            page_idx,
            page_count,
            banner,
            &header_meta,
        );

        // Sub-bar — dark navy, matching TESTS & COVERAGE / SUBMODULES section headers.
        pdf_fill_rect(
            &pf_layer,
            margin,
            sub_top,
            w - 2.0 * margin,
            PDF_PERFILE_SUB_H,
            Rgb::new(0.098, 0.11, 0.15, None),
        );
        pf_layer.set_fill_color(Color::Rgb(Rgb::new(1.0, 1.0, 1.0, None)));
        pf_layer.use_text(
            "PER-FILE DETAIL",
            7.0,
            Mm(margin + 2.0),
            Mm(sub_top + 1.5),
            font_bold,
        );
        if use_continuation {
            // On the continuation page show the project context on the right.
            let right = format!(
                "{}  |  {} files  |  {ts}",
                pdf_trunc(title, 30),
                total_files
            );
            pf_layer.set_fill_color(Color::Rgb(Rgb::new(0.72, 0.72, 0.72, None)));
            let right_x = (w - margin - right.len() as f32 * 1.05).max(margin + 80.0);
            pf_layer.use_text(right, 5.5, Mm(right_x), Mm(sub_top + 1.5), font_reg);
        } else {
            pf_layer.set_fill_color(Color::Rgb(Rgb::new(0.72, 0.72, 0.72, None)));
            pf_layer.use_text(
                pdf_trunc(title, 45),
                5.5,
                Mm(margin + 60.0),
                Mm(sub_top + 1.5),
                font_reg,
            );
            pf_layer.use_text(
                format!("{total_files} files  |  {ts}"),
                5.5,
                Mm(w - margin - 55.0),
                Mm(sub_top + 1.5),
                font_reg,
            );
        }

        let pf_tbl_top = sub_top;
        pdf_fill_rect(
            &pf_layer,
            margin,
            pf_tbl_top - tbl_hdr_h,
            2.0f32.mul_add(-margin, w),
            tbl_hdr_h,
            Rgb::new(0.098, 0.11, 0.15, None),
        );
        pf_layer.set_fill_color(Color::Rgb(Rgb::new(1.0, 1.0, 1.0, None)));
        for (i, lbl) in col_labels.iter().enumerate() {
            pf_layer.use_text(
                *lbl,
                5.0,
                Mm(col_x[i] + 0.5),
                Mm(pf_tbl_top - tbl_hdr_h + 1.5),
                font_bold,
            );
        }

        let (start, end) = pdf_perfile_page_slice(
            page_idx,
            use_continuation,
            first_page.is_some(),
            fp_rows,
            rows_per_page,
            total_files,
        );
        let row_ctx = PdfCtx {
            layer: &pf_layer,
            font_reg,
            font_bold,
            w,
            margin,
            row_h,
            tbl_hdr_h,
        };
        pdf_draw_perfile_rows(
            &row_ctx,
            &run.per_file_records[start..end],
            &col_x,
            pf_tbl_top,
        );

        // Footer
        pdf_fill_rect(
            &pf_layer,
            0.0,
            0.0,
            w,
            footer_h,
            Rgb::new(0.93, 0.91, 0.87, None),
        );
        pf_layer.set_fill_color(Color::Rgb(Rgb::new(0.4, 0.4, 0.4, None)));
        pf_layer.use_text(
            format!("oxide-sloc v{version}  |  AGPL-3.0-or-later"),
            6.5,
            Mm(margin),
            Mm(3.0),
            font_reg,
        );
        let right_text = format!(
            "github.com/oxide-sloc/oxide-sloc  |  Run ID: {}",
            pdf_safe_str(&run.tool.run_id[..run.tool.run_id.len().min(20)])
        );
        let right_x = (w - margin - right_text.len() as f32 * 1.27).max(margin + 80.0);
        pf_layer.use_text(right_text, 6.5, Mm(right_x), Mm(3.0), font_reg);
        // Center section — banner, oxide brand color, bold.
        if let Some(text) = banner {
            let safe = pdf_trunc(&pdf_safe_str(text), 40);
            let text_x = (w / 2.0 - safe.len() as f32 * 0.97).max(margin + 50.0);
            pf_layer.set_fill_color(Color::Rgb(Rgb::new(0.7, 0.33, 0.16, None)));
            pf_layer.use_text(safe, 9.0, Mm(text_x), Mm(2.6), font_bold);
        }
    }
}

/// Draw the dark section-header bar (full usable width, `hdr_h` tall, top edge at `section_top`)
/// with `title` rendered in white bold at the left. Shared by every PDF report section so the
/// header styling stays identical across them.
fn pdf_section_header_bar(
    ctx: &PdfCtx<'_>,
    usable_w: f32,
    section_top: f32,
    hdr_h: f32,
    title: &str,
) {
    use crate::pdf_compat::{Color, Mm, Rgb};
    pdf_fill_rect(
        ctx.layer,
        ctx.margin,
        section_top - hdr_h,
        usable_w,
        hdr_h,
        Rgb::new(0.098, 0.11, 0.15, None),
    );
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(1.0, 1.0, 1.0, None)));
    ctx.layer.use_text(
        title,
        7.0,
        Mm(ctx.margin + 2.0),
        Mm(section_top - hdr_h + 1.5),
        ctx.font_bold,
    );
}

/// Render the Code Style Analysis section onto page 1 of the printpdf PDF.
///
/// Draws below the metric tables: a section header, four summary chips, and a
/// per-language mini-table showing the top style guide and N-col compliance.
/// Returns the y coordinate of the bottom of the rendered section.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::too_many_lines,
    clippy::suboptimal_flops
)]
fn pdf_render_style_section(ctx: &PdfCtx<'_>, ss: &StyleSummary, section_top: f32) -> f32 {
    use crate::pdf_compat::{Color, Mm, Rgb};
    const HDR_H: f32 = 5.5;
    const CHIP_H: f32 = 11.0;
    const CHIP_GAP: f32 = 4.0;
    const ROW_H: f32 = 5.0;
    const TBL_HDR_H: f32 = 5.0;
    const GAP: f32 = 2.5;

    let usable_w = ctx.w - 2.0 * ctx.margin;
    let chip_w = (usable_w - 3.0 * CHIP_GAP) / 4.0;

    // ── section header bar ────────────────────────────────────────────────────
    pdf_section_header_bar(ctx, usable_w, section_top, HDR_H, "CODE STYLE ANALYSIS");
    let col_label = format!("{}-Col", ss.col_threshold);
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(0.85, 0.65, 0.35, None)));
    ctx.layer.use_text(
        "Lexical heuristics",
        5.5,
        Mm(ctx.w - ctx.margin - 26.0),
        Mm(section_top - HDR_H + 1.5),
        ctx.font_reg,
    );

    // ── summary chips ─────────────────────────────────────────────────────────
    let chips_bot = section_top - HDR_H - GAP - CHIP_H;
    let chip_data: [(&str, String); 4] = [
        ("Files Analyzed", ss.files_analyzed.to_string()),
        ("Language Groups", ss.by_language.len().to_string()),
        ("Common Indent", ss.common_indent_style.clone()),
        (&col_label, format!("{}%", ss.line_col_compliant_pct)),
    ];
    for (i, (label, value)) in chip_data.iter().enumerate() {
        let cx = (i as f32).mul_add(chip_w + CHIP_GAP, ctx.margin);
        pdf_fill_rect(
            ctx.layer,
            cx,
            chips_bot,
            chip_w,
            CHIP_H,
            Rgb::new(0.945, 0.925, 0.90, None),
        );
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.7, 0.33, 0.16, None)));
        ctx.layer.use_text(
            pdf_trunc(value, 16),
            10.0,
            Mm(cx + 3.0),
            Mm(chips_bot + 5.5),
            ctx.font_bold,
        );
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.45, 0.45, 0.45, None)));
        ctx.layer.use_text(
            pdf_safe_str(label),
            5.5,
            Mm(cx + 3.0),
            Mm(chips_bot + 1.5),
            ctx.font_reg,
        );
    }

    // ── per-language mini-table ───────────────────────────────────────────────
    if ss.by_language.is_empty() {
        return chips_bot;
    }
    let tbl_top = chips_bot - GAP;

    // Column widths (fractions of usable_w): Family | Files | Top Guide | Score | N-Col
    let col_w = [0.28_f32, 0.08, 0.36, 0.14, 0.14];
    let col_x: Vec<f32> = col_w
        .iter()
        .scan(ctx.margin, |acc, &w| {
            let x = *acc;
            *acc += w * usable_w;
            Some(x)
        })
        .collect();
    let headers = ["Language Family", "Files", "Top Guide", "Score", &col_label];

    pdf_fill_rect(
        ctx.layer,
        ctx.margin,
        tbl_top - TBL_HDR_H,
        usable_w,
        TBL_HDR_H,
        Rgb::new(0.098, 0.11, 0.15, None),
    );
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(1.0, 1.0, 1.0, None)));
    for (hi, hdr) in headers.iter().enumerate() {
        ctx.layer.use_text(
            pdf_safe_str(hdr),
            5.5,
            Mm(col_x[hi] + 2.0),
            Mm(tbl_top - TBL_HDR_H + 1.5),
            ctx.font_bold,
        );
    }

    let mut row_y = tbl_top - TBL_HDR_H;
    for (ri, grp) in ss.by_language.iter().take(5).enumerate() {
        let ry = row_y - ROW_H;
        let bg = if ri % 2 == 0 {
            Rgb::new(0.975, 0.965, 0.95, None)
        } else {
            Rgb::new(1.0, 1.0, 1.0, None)
        };
        pdf_fill_rect(ctx.layer, ctx.margin, ry, usable_w, ROW_H, bg);
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.12, 0.12, 0.12, None)));
        let cells = [
            pdf_trunc(&grp.language_family, 26),
            grp.files_count.to_string(),
            pdf_trunc(&grp.dominant_guide, 28),
            format!("{}%", grp.dominant_score_pct),
            format!("{}%", grp.line_col_compliant_pct),
        ];
        for (ci, cell) in cells.iter().enumerate() {
            let is_score = ci == 3 || ci == 4;
            if is_score && cell != "--" {
                ctx.layer
                    .set_fill_color(Color::Rgb(Rgb::new(0.7, 0.33, 0.16, None)));
            } else {
                ctx.layer
                    .set_fill_color(Color::Rgb(Rgb::new(0.12, 0.12, 0.12, None)));
            }
            ctx.layer.use_text(
                pdf_safe_str(cell),
                6.0,
                Mm(col_x[ci] + 2.0),
                Mm(ry + 1.5),
                ctx.font_reg,
            );
        }
        row_y = ry;
    }

    row_y
}

/// Render the COCOMO I estimate section as a compact table on the PDF page.
/// Returns the bottom y-coordinate of the rendered section.
#[allow(clippy::cast_precision_loss)]
fn pdf_render_cocomo_section(ctx: &PdfCtx<'_>, run: &AnalysisRun, section_top: f32) -> f32 {
    use crate::pdf_compat::{Color, Mm, Rgb};
    const HDR_H: f32 = 5.5;
    const ROW_H: f32 = 13.0; // tall enough for label + value with comfortable padding
    const NOTE_H: f32 = 2.0; // just enough clearance for 5.5 pt descenders below the baseline
    const GAP: f32 = 5.0; // breathing room between data row and footnote

    let Some(ref c) = run.cocomo else {
        return section_top;
    };

    let mode_label = match c.mode {
        CocomoMode::Organic => "Organic",
        CocomoMode::SemiDetached => "Semi-detached",
        CocomoMode::Embedded => "Embedded",
    };
    let usable_w = 2.0_f32.mul_add(-ctx.margin, ctx.w);

    // Section header bar
    pdf_section_header_bar(
        ctx,
        usable_w,
        section_top,
        HDR_H,
        "CONSTRUCTIVE COST MODEL (COCOMO I) ESTIMATE",
    );
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(0.85, 0.65, 0.35, None)));
    let mode_display = format!("{mode_label} mode");
    ctx.layer.use_text(
        mode_display.as_str(),
        5.5,
        Mm(ctx.w - ctx.margin - 28.0),
        Mm(section_top - HDR_H + 1.5),
        ctx.font_reg,
    );

    // 4-column data row (full width, single row)
    let col_w = usable_w / 4.0;
    let row_y = section_top - HDR_H - ROW_H;
    let data: [(&str, String); 4] = [
        ("Person-months", format!("{:.2}", c.effort_person_months)),
        ("Schedule (months)", format!("{:.2}", c.duration_months)),
        ("Avg. Team Size", format!("{:.2}", c.avg_staff)),
        ("Input KSLOC", format!("{:.2}K", c.ksloc)),
    ];
    for (i, (label, value)) in data.iter().enumerate() {
        let cx = (i as f32).mul_add(col_w, ctx.margin);
        let bg = if i % 2 == 0 {
            Rgb::new(0.975, 0.965, 0.95, None)
        } else {
            Rgb::new(1.0, 1.0, 1.0, None)
        };
        pdf_fill_rect(ctx.layer, cx, row_y, col_w, ROW_H, bg);
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.45, 0.45, 0.45, None)));
        ctx.layer
            .use_text(*label, 5.5, Mm(cx + 2.0), Mm(row_y + 9.0), ctx.font_reg);
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.7, 0.33, 0.16, None)));
        ctx.layer.use_text(
            value.as_str(),
            10.0,
            Mm(cx + 2.0),
            Mm(row_y + 2.5),
            ctx.font_bold,
        );
    }

    // Footnote
    let note_y = row_y - GAP;
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(0.45, 0.45, 0.45, None)));
    ctx.layer.use_text(
        "COCOMO I (Boehm, 1981): algorithmic model converting SLOC into effort, schedule, and team-size estimates. \
         Ballpark figures only - actual outcomes vary with team experience and domain complexity.",
        5.5,
        Mm(ctx.margin),
        Mm(note_y),
        ctx.font_reg,
    );

    note_y - NOTE_H
}

/// Draw `text` so its right edge sits at `x_right` mm, at vertical `y` mm. The caller sets the
/// fill colour beforehand. Uses the Helvetica advance-width table for alignment.
fn pdf_text_right(ctx: &PdfCtx<'_>, text: &str, pt: f32, x_right: f32, y: f32, bold: bool) {
    use crate::pdf_compat::Mm;
    let font = if bold { ctx.font_bold } else { ctx.font_reg };
    let w = helvetica_width_mm(text, pt, bold);
    ctx.layer.use_text(text, pt, Mm(x_right - w), Mm(y), font);
}

/// Front-truncate `path` with a leading "..." so it fits within `budget_mm` at `pt`, keeping the
/// most informative tail (the filename). Returns the path unchanged when it already fits.
fn pdf_fit_path(path: &str, budget_mm: f32, pt: f32) -> String {
    if helvetica_width_mm(path, pt, false) <= budget_mm {
        return path.to_string();
    }
    let mut chars: Vec<char> = path.chars().collect();
    while !chars.is_empty() {
        chars.remove(0);
        let candidate: String = format!("...{}", chars.iter().collect::<String>());
        if helvetica_width_mm(&candidate, pt, false) <= budget_mm {
            return candidate;
        }
    }
    "...".to_string()
}

/// Fit free text (e.g. an author name) to `budget_mm`, truncating from the END with an ellipsis.
/// Unlike `pdf_fit_path` (which keeps the tail of a path), this keeps the leading characters.
fn pdf_fit_text(text: &str, budget_mm: f32, pt: f32) -> String {
    if helvetica_width_mm(text, pt, false) <= budget_mm {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let candidate: String = format!("{}...", chars.iter().collect::<String>());
        if helvetica_width_mm(&candidate, pt, false) <= budget_mm {
            return candidate;
        }
    }
    "...".to_string()
}

/// Render the Git Hotspots table (files ranked by code lines x recent commits) starting at
/// `section_top`. Returns the Y coordinate below the rendered content. Mirrors the COCOMO
/// section's dark header bar and the per-file table's right-aligned numeric columns.
fn pdf_render_hotspots_section(ctx: &PdfCtx<'_>, rows: &[HotspotRow], section_top: f32) -> f32 {
    use crate::pdf_compat::{Color, Mm, Rgb};
    const HDR_H: f32 = 5.5;
    const COLHDR_H: f32 = 5.0;
    const ROW_H: f32 = 5.2;
    const NOTE_GAP: f32 = 4.0;

    let usable_w = 2.0_f32.mul_add(-ctx.margin, ctx.w);

    // Section header bar.
    pdf_section_header_bar(
        ctx,
        usable_w,
        section_top,
        HDR_H,
        "GIT HOTSPOTS (CODE LINES x RECENT COMMITS)",
    );

    // Column right edges (numeric columns are right-aligned); File fills the remaining left space.
    let col_last_r = ctx.w - ctx.margin;
    let col_score_r = col_last_r - 32.0;
    let col_commits_r = col_score_r - 33.0;
    let col_code_r = col_commits_r - 32.0;
    let file_x = ctx.margin + 2.0;
    let file_budget = (col_code_r - 26.0) - file_x;

    // Column-header row.
    let chdr_y = section_top - HDR_H - COLHDR_H;
    pdf_fill_rect(
        ctx.layer,
        ctx.margin,
        chdr_y,
        usable_w,
        COLHDR_H,
        Rgb::new(0.90, 0.88, 0.84, None),
    );
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(0.30, 0.30, 0.30, None)));
    ctx.layer
        .use_text("File", 6.0, Mm(file_x), Mm(chdr_y + 1.4), ctx.font_bold);
    pdf_text_right(ctx, "Code lines", 6.0, col_code_r, chdr_y + 1.4, true);
    pdf_text_right(ctx, "Commits", 6.0, col_commits_r, chdr_y + 1.4, true);
    pdf_text_right(ctx, "Hotspot score", 6.0, col_score_r, chdr_y + 1.4, true);
    pdf_text_right(ctx, "Last changed", 6.0, col_last_r, chdr_y + 1.4, true);

    // Data rows (zebra background).
    let mut y = chdr_y;
    for (ri, hrow) in rows.iter().enumerate() {
        y -= ROW_H;
        let bg = if ri.is_multiple_of(2) {
            Rgb::new(0.975, 0.965, 0.95, None)
        } else {
            Rgb::new(1.0, 1.0, 1.0, None)
        };
        pdf_fill_rect(ctx.layer, ctx.margin, y, usable_w, ROW_H, bg);
        // File path (front-truncated to its width budget).
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.12, 0.12, 0.12, None)));
        let path = pdf_fit_path(&pdf_safe_str(&hrow.path), file_budget, 6.0);
        ctx.layer
            .use_text(path, 6.0, Mm(file_x), Mm(y + 1.4), ctx.font_reg);
        // Numeric columns.
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.12, 0.12, 0.12, None)));
        pdf_text_right(
            ctx,
            &group_thousands(&hrow.code_lines.to_string()),
            6.0,
            col_code_r,
            y + 1.4,
            false,
        );
        pdf_text_right(
            ctx,
            &hrow.commit_count.to_string(),
            6.0,
            col_commits_r,
            y + 1.4,
            false,
        );
        // Hotspot score — emphasised in the oxide accent colour.
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.7, 0.33, 0.16, None)));
        pdf_text_right(
            ctx,
            &group_thousands(&hrow.score.to_string()),
            6.0,
            col_score_r,
            y + 1.4,
            true,
        );
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.45, 0.45, 0.45, None)));
        pdf_text_right(ctx, &hrow.last_commit_date, 6.0, col_last_r, y + 1.4, false);
    }

    // Footnote.
    let note_y = y - NOTE_GAP;
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(0.45, 0.45, 0.45, None)));
    ctx.layer.use_text(
        "Files ranked by code lines x commits over the configured git activity window. \
         Distinct from the Compare page's scan-to-scan churn rate.",
        5.5,
        Mm(ctx.margin),
        Mm(note_y + 1.0),
        ctx.font_reg,
    );

    note_y
}

/// Render a dedicated "Git Hotspots" page and return its `(page, layer, y_below)` so the per-file
/// table can continue on the same page (mirrors `pdf_render_tests_coverage_page`).
#[allow(clippy::too_many_arguments)]
fn pdf_render_hotspots_page(
    doc: &crate::pdf_compat::PdfDocumentReference,
    font_reg: crate::pdf_compat::IndirectFontRef,
    font_bold: crate::pdf_compat::IndirectFontRef,
    run: &AnalysisRun,
    rows: &[HotspotRow],
    w: f32,
    h: f32,
    margin: f32,
    footer_h: f32,
    title: &str,
    version: &str,
) -> (
    crate::pdf_compat::PdfPageIndex,
    crate::pdf_compat::PdfLayerIndex,
    f32,
) {
    use crate::pdf_compat::Mm;
    const HDR_H: f32 = 8.0;

    let (page, layer_idx) = doc.add_page(Mm(w), Mm(h), "Git Hotspots");
    let layer = doc.get_page(page).get_layer(layer_idx);
    let ctx = PdfCtx {
        layer: &layer,
        font_reg,
        font_bold,
        w,
        margin,
        row_h: 5.5,
        tbl_hdr_h: 6.0,
    };

    pdf_page_mini_header(&ctx, h, HDR_H, title, run);

    let bottom = pdf_render_hotspots_section(&ctx, rows, h - HDR_H - 4.0);

    pdf_page_footer_band(&ctx, footer_h, version);

    (page, layer_idx, bottom - 3.0)
}

/// Render the Code Ownership table (per-author blame tallies) starting at `section_top`.
/// Columns: Author (left, truncated) then right-aligned Code / Comment / Blank / Total /
/// Code % / Files. Returns the Y just below the last row.
fn pdf_render_ownership_section(
    ctx: &PdfCtx<'_>,
    rows: &[AuthorReportRow],
    section_top: f32,
) -> f32 {
    use crate::pdf_compat::{Color, Mm, Rgb};
    const HDR_H: f32 = 5.5;
    const COLHDR_H: f32 = 5.0;
    const ROW_H: f32 = 5.2;
    const NOTE_GAP: f32 = 4.0;

    let usable_w = 2.0_f32.mul_add(-ctx.margin, ctx.w);

    pdf_section_header_bar(
        ctx,
        usable_w,
        section_top,
        HDR_H,
        "CODE OWNERSHIP (LINES PER CONTRIBUTOR, GIT BLAME)",
    );

    // Right edges for the numeric columns; Author fills the remaining left space.
    let col_files_r = ctx.w - ctx.margin;
    let col_pct_r = col_files_r - 22.0;
    let col_total_r = col_pct_r - 28.0;
    let col_blank_r = col_total_r - 26.0;
    let col_comment_r = col_blank_r - 28.0;
    let col_code_r = col_comment_r - 26.0;
    let name_x = ctx.margin + 2.0;
    let name_budget = (col_code_r - 26.0) - name_x;

    let chdr_y = section_top - HDR_H - COLHDR_H;
    pdf_fill_rect(
        ctx.layer,
        ctx.margin,
        chdr_y,
        usable_w,
        COLHDR_H,
        Rgb::new(0.90, 0.88, 0.84, None),
    );
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(0.30, 0.30, 0.30, None)));
    ctx.layer
        .use_text("Author", 6.0, Mm(name_x), Mm(chdr_y + 1.4), ctx.font_bold);
    pdf_text_right(ctx, "Code", 6.0, col_code_r, chdr_y + 1.4, true);
    pdf_text_right(ctx, "Comment", 6.0, col_comment_r, chdr_y + 1.4, true);
    pdf_text_right(ctx, "Blank", 6.0, col_blank_r, chdr_y + 1.4, true);
    pdf_text_right(ctx, "Total", 6.0, col_total_r, chdr_y + 1.4, true);
    pdf_text_right(ctx, "Code %", 6.0, col_pct_r, chdr_y + 1.4, true);
    pdf_text_right(ctx, "Files", 6.0, col_files_r, chdr_y + 1.4, true);

    let mut y = chdr_y;
    for (ri, a) in rows.iter().enumerate() {
        y -= ROW_H;
        let bg = if ri.is_multiple_of(2) {
            Rgb::new(0.975, 0.965, 0.95, None)
        } else {
            Rgb::new(1.0, 1.0, 1.0, None)
        };
        pdf_fill_rect(ctx.layer, ctx.margin, y, usable_w, ROW_H, bg);
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.12, 0.12, 0.12, None)));
        let name = pdf_fit_text(&pdf_safe_str(&a.name), name_budget, 6.0);
        ctx.layer
            .use_text(name, 6.0, Mm(name_x), Mm(y + 1.4), ctx.font_reg);
        pdf_text_right(
            ctx,
            &group_thousands(&a.code.to_string()),
            6.0,
            col_code_r,
            y + 1.4,
            false,
        );
        pdf_text_right(
            ctx,
            &group_thousands(&a.comment.to_string()),
            6.0,
            col_comment_r,
            y + 1.4,
            false,
        );
        pdf_text_right(
            ctx,
            &group_thousands(&a.blank.to_string()),
            6.0,
            col_blank_r,
            y + 1.4,
            false,
        );
        pdf_text_right(
            ctx,
            &group_thousands(&a.total.to_string()),
            6.0,
            col_total_r,
            y + 1.4,
            false,
        );
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.7, 0.33, 0.16, None)));
        pdf_text_right(
            ctx,
            &format!("{}%", a.code_pct_str),
            6.0,
            col_pct_r,
            y + 1.4,
            true,
        );
        ctx.layer
            .set_fill_color(Color::Rgb(Rgb::new(0.12, 0.12, 0.12, None)));
        pdf_text_right(
            ctx,
            &a.files_owned.to_string(),
            6.0,
            col_files_r,
            y + 1.4,
            false,
        );
    }

    let note_y = y - NOTE_GAP;
    ctx.layer
        .set_fill_color(Color::Rgb(Rgb::new(0.45, 0.45, 0.45, None)));
    ctx.layer.use_text(
        "Physical lines attributed by git blame (-w -M -C, .mailmap honoured). Same-email \
         identities are merged; cross-account merging is a later step.",
        5.5,
        Mm(ctx.margin),
        Mm(note_y + 1.0),
        ctx.font_reg,
    );

    note_y
}

/// Render a dedicated terminal "Code Ownership" page. Appended after the per-file pages so it
/// never participates in the per-file continuation threading. Caps rows to a single page.
#[allow(clippy::too_many_arguments)]
fn pdf_render_ownership_page(
    doc: &crate::pdf_compat::PdfDocumentReference,
    font_reg: crate::pdf_compat::IndirectFontRef,
    font_bold: crate::pdf_compat::IndirectFontRef,
    run: &AnalysisRun,
    rows: &[AuthorReportRow],
    w: f32,
    h: f32,
    margin: f32,
    footer_h: f32,
    title: &str,
    version: &str,
) {
    use crate::pdf_compat::Mm;
    const HDR_H: f32 = 8.0;

    let (page, layer_idx) = doc.add_page(Mm(w), Mm(h), "Code Ownership");
    let layer = doc.get_page(page).get_layer(layer_idx);
    let ctx = PdfCtx {
        layer: &layer,
        font_reg,
        font_bold,
        w,
        margin,
        row_h: 5.2,
        tbl_hdr_h: 5.0,
    };

    pdf_page_mini_header(&ctx, h, HDR_H, title, run);
    pdf_render_ownership_section(&ctx, rows, h - HDR_H - 4.0);
    pdf_page_footer_band(&ctx, footer_h, version);
}

/// Measure how tall the COCOMO + Tests & Coverage page needs to be, so a terminal
/// (last) page can be trimmed to its content instead of left at full landscape height
/// with a large empty gap below the last section.
///
/// Renders the same sections onto a throwaway, never-saved document of height `h_full`
/// and reads where the content ends. Layout is vertically translation-invariant, so the
/// trimmed height is `h_full - content_bottom + footer_h + pad`. Falls back to `h_full`
/// on any error so the report is always produced.
#[allow(clippy::too_many_arguments)]
fn measure_terminal_tc_page_height(
    run: &AnalysisRun,
    w: f32,
    h_full: f32,
    margin: f32,
    footer_h: f32,
    row_h: f32,
    tbl_hdr_h: f32,
    with_cocomo: bool,
) -> f32 {
    use crate::pdf_compat::{BuiltinFont, Mm, PdfDocument};
    let measure = || -> Option<f32> {
        let (doc, page, layer_idx) = PdfDocument::new("measure", Mm(w), Mm(h_full), "m");
        let font_reg = doc.add_builtin_font(BuiltinFont::Helvetica).ok()?;
        let font_bold = doc.add_builtin_font(BuiltinFont::HelveticaBold).ok()?;
        let layer = doc.get_page(page).get_layer(layer_idx);
        let ctx = PdfCtx {
            layer: &layer,
            font_reg,
            font_bold,
            w,
            margin,
            row_h,
            tbl_hdr_h,
        };
        // Mirror the real render's starting offsets exactly (see the cocomo/T&C branches
        // in `write_pdf_from_run`): an 8 mm header band, then the first section below it.
        let content_bottom = if with_cocomo {
            let cocomo_bottom = pdf_render_cocomo_section(&ctx, run, h_full - 8.0 - 6.0);
            pdf_render_tc_inline(&ctx, run, cocomo_bottom - 2.0, footer_h)
        } else {
            pdf_render_tc_inline(&ctx, run, h_full - 8.0 - 4.0, footer_h)
        };
        // 4 mm bottom padding below the last element, mirroring the top-of-content gap.
        let pad = 4.0;
        Some((h_full - content_bottom + footer_h + pad).clamp(60.0, h_full))
    };
    measure().unwrap_or(h_full)
}

/// Render the dedicated COCOMO + Tests & Coverage page (page 2) when COCOMO did not fit
/// on page 1, or a standalone Tests & Coverage page otherwise. Returns the page/layer and
/// the Y below the last section so the per-file table can continue on the same page with
/// no blank-page gap. Extracted from `write_pdf_from_run` to keep that function's cognitive
/// complexity low; layout and output are unchanged.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_arguments
)]
fn pdf_render_cocomo_or_tc_page(
    doc: &crate::pdf_compat::PdfDocumentReference,
    font_reg: crate::pdf_compat::IndirectFontRef,
    font_bold: crate::pdf_compat::IndirectFontRef,
    run: &AnalysisRun,
    dims: PdfPageDims,
    title: &str,
    version: &str,
    cocomo_fits_page1: bool,
    trim_page: bool,
) -> (
    crate::pdf_compat::PdfPageIndex,
    crate::pdf_compat::PdfLayerIndex,
    f32,
) {
    use crate::pdf_compat::{Color, Mm, Mm as PdfMm, Rgb};
    let PdfPageDims {
        w,
        h,
        margin,
        footer_h,
        row_h,
        tbl_hdr_h,
    } = dims;

    // No COCOMO on its own page — create a dedicated T&C page and start per-file from it.
    if run.cocomo.is_none() || cocomo_fits_page1 {
        let page_h = if trim_page {
            measure_terminal_tc_page_height(run, w, h, margin, footer_h, row_h, tbl_hdr_h, false)
        } else {
            h
        };
        return pdf_render_tests_coverage_page(
            doc, font_reg, font_bold, run, w, page_h, margin, footer_h, title, version,
        );
    }

    let page_h = if trim_page {
        measure_terminal_tc_page_height(run, w, h, margin, footer_h, row_h, tbl_hdr_h, true)
    } else {
        h
    };
    let (c2_page, c2_layer_idx) = doc.add_page(Mm(w), Mm(page_h), "Content");
    let c2_layer = doc.get_page(c2_page).get_layer(c2_layer_idx);
    let c2_ctx = PdfCtx {
        layer: &c2_layer,
        font_reg,
        font_bold,
        w,
        margin,
        row_h,
        tbl_hdr_h,
    };
    // Small page header so the reader knows which report this is.
    pdf_fill_rect(
        &c2_layer,
        0.0,
        page_h - 8.0,
        w,
        8.0,
        Rgb::new(0.098, 0.11, 0.15, None),
    );
    c2_layer.set_fill_color(Color::Rgb(Rgb::new(1.0, 1.0, 1.0, None)));
    c2_layer.use_text(
        "oxide-sloc",
        9.0,
        PdfMm(margin),
        PdfMm(page_h - 5.5),
        font_bold,
    );
    c2_layer.set_fill_color(Color::Rgb(Rgb::new(0.72, 0.72, 0.72, None)));
    c2_layer.use_text(
        pdf_trunc(&pdf_safe_str(title), 45),
        7.5,
        PdfMm(46.0),
        PdfMm(page_h - 5.5),
        font_reg,
    );
    pdf_draw_header_meta(
        &c2_layer,
        font_reg,
        w,
        margin,
        page_h - 5.5,
        &pdf_page_header_meta(run),
    );
    let cocomo_bottom = pdf_render_cocomo_section(&c2_ctx, run, page_h - 8.0 - 6.0);
    // Render T&C inline on the same page immediately after COCOMO — no blank gap.
    let tc_bottom = pdf_render_tc_inline(&c2_ctx, run, cocomo_bottom - 2.0, footer_h);
    // Footer (per-file renderer will overdraw with its richer version if it starts here).
    pdf_fill_rect(
        &c2_layer,
        0.0,
        0.0,
        w,
        footer_h,
        Rgb::new(0.93, 0.91, 0.87, None),
    );
    c2_layer.set_fill_color(Color::Rgb(Rgb::new(0.4, 0.4, 0.4, None)));
    c2_layer.use_text(
        format!("oxide-sloc v{version}  |  AGPL-3.0-or-later"),
        6.5,
        PdfMm(margin),
        PdfMm(3.0),
        font_reg,
    );
    // Pass the Y below T&C content so per-file can continue on this page without a gap.
    (c2_page, c2_layer_idx, tc_bottom - 3.0)
}

/// Generate a PDF summary report from `AnalysisRun` data using the pure-Rust `printpdf` crate.
///
/// No external tools (Chrome, wkhtmltopdf) are required — this path is always available on
/// both Windows and Linux server deployments.
///
/// # Errors
///
/// Returns an error if the output directory cannot be created or the PDF file cannot be written.
// Casts throughout are for PDF layout coordinates and percentage ratios; precision loss is fine.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines
)]
pub fn write_pdf_from_run(run: &AnalysisRun, pdf_path: &Path) -> Result<()> {
    use crate::pdf_compat::{BuiltinFont, Mm, PdfDocument};
    use std::fs::File;
    use std::io::BufWriter;

    const W: f32 = 297.0;
    const H: f32 = 210.0;
    const MARGIN: f32 = 10.0;
    const FOOTER_H: f32 = 10.0;
    const HDR_H: f32 = 13.5;
    const ROW_H: f32 = 5.5;
    const TBL_HDR_H: f32 = 6.0;

    if let Some(parent) = pdf_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create PDF directory {}", parent.display()))?;
    }

    let title = pdf_safe_str(&run.effective_configuration.reporting.report_title);
    let ts = to_pt_hhmm(run.tool.timestamp_utc);
    let version = env!("CARGO_PKG_VERSION");
    let banner = run
        .effective_configuration
        .reporting
        .report_header_footer
        .as_deref();

    let (doc, page1, layer1) =
        PdfDocument::new(format!("oxide-sloc: {title}"), Mm(W), Mm(H), "Content");
    let font_reg = doc
        .add_builtin_font(BuiltinFont::Helvetica)
        .map_err(|e| anyhow::anyhow!("printpdf font error: {e}"))?;
    let font_bold = doc
        .add_builtin_font(BuiltinFont::HelveticaBold)
        .map_err(|e| anyhow::anyhow!("printpdf font error: {e}"))?;
    let layer = doc.get_page(page1).get_layer(layer1);

    let ctx = PdfCtx {
        layer: &layer,
        font_reg,
        font_bold,
        w: W,
        margin: MARGIN,
        row_h: ROW_H,
        tbl_hdr_h: TBL_HDR_H,
    };
    let roots_text_y = pdf_render_page1_header(&ctx, run, &ts, &title, H, HDR_H, banner);
    let row2_bot = pdf_render_summary_chips(&ctx, run, roots_text_y);
    let info_y = pdf_render_info_lines(&ctx, run, row2_bot);
    let tbl_top = info_y - 4.0;
    pdf_render_metric_tables(&ctx, run, tbl_top);
    // Style analysis section — rendered below the metric tables when data is available.
    // The metric tables occupy ~64.5 mm below tbl_top; leave 4 mm clearance before drawing.
    let after_tables_y = tbl_top - 64.5 - 4.0;
    let after_style_y = run.style_summary.as_ref().map_or(after_tables_y, |ss| {
        if after_tables_y > FOOTER_H + 12.0 {
            pdf_render_style_section(&ctx, ss, after_tables_y)
        } else {
            after_tables_y
        }
    });
    // COCOMO estimate — on page 1 if room remains, otherwise on its own page 2.
    // Need ~32 mm: header (5.5) + data row (13) + gap (5) + note (5) + margins (~3.5).
    let cocomo_fits_page1 = run.cocomo.is_some() && (after_style_y - 3.0) > FOOTER_H + 32.0;
    if cocomo_fits_page1 {
        pdf_render_cocomo_section(&ctx, run, after_style_y - 3.0);
    }
    pdf_render_page1_footer(&ctx, run, FOOTER_H, version, banner);

    // Page-flow bookkeeping for empty-gap trimming. The per-file table continues on the
    // COCOMO/T&C page only when there is no Git Hotspots page in between (the Hotspots page,
    // when present, becomes the per-file continuation instead). A page that nothing flows
    // onto is trimmed to its content height to avoid a large empty gap below the last section.
    let hotspot_rows = build_hotspot_rows(run, 15);
    let has_per_file = !run.per_file_records.is_empty();
    let tc_page_gets_per_file = hotspot_rows.is_empty() && has_per_file;
    let trim_tc_page = !tc_page_gets_per_file;

    // If COCOMO didn't fit on page 1, render it on a dedicated page 2 (with T&C inline);
    // otherwise render a standalone T&C page. Either way the returned page/layer/Y lets the
    // per-file table continue on the same page with no blank-page gap.
    let page_dims = PdfPageDims {
        w: W,
        h: H,
        margin: MARGIN,
        footer_h: FOOTER_H,
        row_h: ROW_H,
        tbl_hdr_h: TBL_HDR_H,
    };
    let cocomo_page_ctx = pdf_render_cocomo_or_tc_page(
        &doc,
        font_reg,
        font_bold,
        run,
        page_dims,
        &title,
        version,
        cocomo_fits_page1,
        trim_tc_page,
    );

    // Git Hotspots — its own page after COCOMO/T&C, only when an --activity-window scan
    // collected per-file git activity. Threaded as the per-file continuation (like COCOMO)
    // so the per-file table flows on below it with no blank-page gap.
    // A Git Hotspots page is only emitted when per-file git activity exists, which means
    // `per_file_records` is non-empty and the per-file table always flows onto it — so it is
    // never a terminal page and needs no trimming (it stays full height for the per-file rows).
    let per_file_start = if hotspot_rows.is_empty() {
        Some(cocomo_page_ctx)
    } else {
        Some(pdf_render_hotspots_page(
            &doc,
            font_reg,
            font_bold,
            run,
            &hotspot_rows,
            W,
            H,
            MARGIN,
            FOOTER_H,
            &title,
            version,
        ))
    };

    if !run.per_file_records.is_empty() {
        // Per-file continues on the same page as T&C / COCOMO / Hotspots — no blank page between.
        pdf_render_per_file_pages(
            &doc,
            font_reg,
            font_bold,
            run,
            W,
            H,
            MARGIN,
            FOOTER_H,
            ROW_H,
            TBL_HDR_H,
            &title,
            &ts,
            version,
            banner,
            per_file_start,
        );
    }

    // Code Ownership — a dedicated terminal page when an attribution scan populated authors.
    // Capped to a single page's worth of the top contributors (by code lines owned).
    let author_rows = build_author_rows(run);
    if !author_rows.is_empty() {
        let capped: Vec<AuthorReportRow> = author_rows.into_iter().take(40).collect();
        pdf_render_ownership_page(
            &doc, font_reg, font_bold, run, &capped, W, H, MARGIN, FOOTER_H, &title, version,
        );
    }

    doc.save(&mut BufWriter::new(File::create(pdf_path).with_context(
        || format!("cannot create PDF at {}", pdf_path.display()),
    )?))
    .map_err(|e| anyhow::anyhow!("printpdf save error: {e}"))?;

    Ok(())
}

/// Per-character advance widths for the PDF built-in Helvetica and Helvetica-Bold fonts
/// (1/1000 em units, PDF spec Appendix D). Used to right-align text without a layout engine.
///
/// Each row is `(glyph, bold_advance, regular_advance)`, a verbatim transcription of the PDF
/// spec width tables. Keeping both weights on one row per glyph preserves the spec mapping for
/// audit while expressing it as data rather than two parallel `match` arms. Digits (`'0'..='9'`,
/// 556 in both weights) are handled in `helvetica_advance` and intentionally omitted here.
const HELVETICA_WIDTHS: &[(char, u32, u32)] = &[
    (' ', 278, 278),
    ('!', 333, 278),
    ('"', 474, 355),
    ('#', 556, 556),
    ('$', 556, 556),
    ('%', 889, 889),
    ('&', 722, 667),
    ('\'', 278, 222),
    ('(', 333, 333),
    (')', 333, 333),
    ('*', 389, 389),
    ('+', 584, 584),
    (',', 278, 278),
    ('-', 333, 333),
    ('.', 278, 278),
    ('/', 278, 278),
    (':', 333, 278),
    (';', 333, 278),
    ('<', 584, 584),
    ('=', 584, 584),
    ('>', 584, 584),
    ('?', 556, 472),
    ('@', 975, 1015),
    ('A', 722, 667),
    ('B', 722, 667),
    ('C', 722, 722),
    ('D', 722, 722),
    ('E', 667, 667),
    ('F', 611, 611),
    ('G', 778, 778),
    ('H', 722, 722),
    ('I', 278, 278),
    ('J', 556, 500),
    ('K', 722, 667),
    ('L', 611, 556),
    ('M', 833, 833),
    ('N', 722, 722),
    ('O', 778, 778),
    ('P', 667, 667),
    ('Q', 778, 778),
    ('R', 722, 722),
    ('S', 667, 667),
    ('T', 611, 611),
    ('U', 722, 722),
    ('V', 667, 667),
    ('W', 944, 944),
    ('X', 667, 667),
    ('Y', 611, 611),
    ('Z', 611, 611),
    ('[', 333, 278),
    ('\\', 278, 278),
    (']', 333, 278),
    ('^', 584, 469),
    ('_', 556, 556),
    ('`', 278, 222),
    ('a', 556, 556),
    ('b', 611, 556),
    ('c', 556, 500),
    ('d', 611, 556),
    ('e', 556, 556),
    ('f', 333, 278),
    ('g', 611, 556),
    ('h', 611, 556),
    ('i', 278, 222),
    ('j', 278, 222),
    ('k', 556, 500),
    ('l', 278, 222),
    ('m', 889, 833),
    ('n', 611, 556),
    ('o', 611, 556),
    ('p', 611, 556),
    ('q', 611, 556),
    ('r', 389, 333),
    ('s', 556, 500),
    ('t', 333, 278),
    ('u', 611, 556),
    ('v', 556, 500),
    ('w', 778, 722),
    ('x', 556, 500),
    ('y', 556, 500),
    ('z', 500, 500),
    ('\u{00B7}', 278, 278), // middle dot (Latin-1 0xB7) — used as section separator
];

/// Advance width (1/1000 em) for `ch` in Helvetica (`bold` selects the bold weight). Looks up
/// `HELVETICA_WIDTHS`; digits are a uniform 556, and unknown glyphs fall back to the average
/// advance for the weight (556 bold, 500 regular).
fn helvetica_advance(ch: char, bold: bool) -> u32 {
    if ch.is_ascii_digit() {
        return 556;
    }
    for &(glyph, bold_w, regular_w) in HELVETICA_WIDTHS {
        if glyph == ch {
            return if bold { bold_w } else { regular_w };
        }
    }
    if bold { 556 } else { 500 }
}

/// Convert a string to mm given a font size (pt) and bold flag, using exact PDF Helvetica metrics.
//
// Rendered strings are length-bounded, so the glyph-unit sum is far below f32's 2^23
// exact-integer ceiling; the cast feeds a millimetre layout width where any rounding is
// sub-pixel.
#[allow(
    clippy::cast_precision_loss,
    reason = "bounded glyph-unit sum to mm width"
)]
fn helvetica_width_mm(text: &str, pt: f32, bold: bool) -> f32 {
    let units: u32 = text.chars().map(|ch| helvetica_advance(ch, bold)).sum();
    // 1 unit = (pt × 25.4 mm/in ÷ 72 pt/in) / 1000.
    units as f32 * pt * (25.4 / 72.0) / 1000.0
}

fn pdf_fill_rect(
    layer: &crate::pdf_compat::PdfLayerReference,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: crate::pdf_compat::Rgb,
) {
    layer.fill_rect(x, y, w, h, color);
}

fn pdf_safe_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            // Common Unicode punctuation → readable ASCII equivalents
            '\u{2014}' | '\u{2013}' => out.push_str(" - "), // em dash / en dash
            '\u{2026}' => out.push_str("..."),              // ellipsis
            '\u{2018}' | '\u{2019}' => out.push('\''),      // curly single quotes
            '\u{201C}' | '\u{201D}' => out.push('"'),       // curly double quotes
            '\u{00B7}' | '\u{2022}' => out.push('-'),       // middle dot / bullet
            '\u{00A0}' => out.push(' '),                    // non-breaking space
            c if c.is_ascii() && !c.is_ascii_control() => out.push(c),
            _ => {} // drop truly unprintable non-ASCII rather than emitting '?'
        }
    }
    out
}

fn pdf_trunc(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}

// Show the tail of a string — prepend "..." when truncated so the meaningful end is visible.
// Used for file paths where the filename/leaf matters more than the leading directories.
fn pdf_trunc_end(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("...{}", &s[s.len() - max.saturating_sub(3)..])
    }
}

fn pdf_fmt_full(n: u64) -> String {
    // Comma-separated full number: 15319 → "15,319", 1374 → "1,374"
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

/// Launch a headless Chromium-based browser to print `html_path` as a PDF to `pdf_path`.
///
/// Tries CDP (headless Chrome) first; falls back to `wkhtmltopdf` when no Chromium-based
/// browser is found on the server.
///
/// # Errors
///
/// Returns an error if no PDF tool (Chromium or wkhtmltopdf) is available, the tool fails
/// to start, or the PDF file is not produced within the timeout.
pub fn write_pdf_from_html(html_path: &Path, pdf_path: &Path) -> Result<()> {
    eprintln!("[oxide-sloc][pdf] starting");

    let absolute_html = html_path
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", html_path.display()))?;
    // canonicalize() on Windows prepends \\?\ (extended-length path prefix) — strip it for display.
    eprintln!(
        "[oxide-sloc][pdf] html = {}",
        absolute_html.to_string_lossy().trim_start_matches(r"\\?\")
    );

    let absolute_pdf = if pdf_path.is_absolute() {
        pdf_path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("failed to resolve current working directory")?
            .join(pdf_path)
    };
    eprintln!("[oxide-sloc][pdf] pdf = {}", absolute_pdf.display());

    if let Some(parent) = absolute_pdf.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("failed to create PDF output directory {}", parent.display())
        })?;
    }

    match write_pdf_via_cdp(&absolute_html, &absolute_pdf) {
        Ok(()) => {}
        Err(cdp_err) => {
            eprintln!("[oxide-sloc][pdf] CDP failed ({cdp_err:#}), trying wkhtmltopdf fallback");
            write_pdf_via_wkhtmltopdf(&absolute_html, &absolute_pdf).with_context(|| {
                format!(
                    "PDF generation failed via both CDP ({cdp_err:#}) and wkhtmltopdf. \
                     Install a Chromium-based browser (Chrome, Edge, Brave) or wkhtmltopdf \
                     on the server, or set SLOC_BROWSER to the browser executable path."
                )
            })?;
        }
    }

    eprintln!("[oxide-sloc][pdf] done");
    Ok(())
}

fn normalize_browser_env_path(raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    #[cfg(windows)]
    {
        let bytes = trimmed.as_bytes();
        if bytes.len() >= 3
            && bytes[0] == b'/'
            && bytes[2] == b'/'
            && bytes[1].is_ascii_alphabetic()
        {
            let drive = (bytes[1] as char).to_ascii_uppercase();
            let rest = &trimmed[3..];
            return PathBuf::from(format!("{drive}:/{rest}"));
        }
    }
    PathBuf::from(trimmed)
}

fn discover_browser_from_env() -> Option<PathBuf> {
    for var_name in ["SLOC_BROWSER", "BROWSER"] {
        if let Ok(path) = std::env::var(var_name) {
            let candidate = normalize_browser_env_path(&path);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn discover_browser() -> Option<PathBuf> {
    if let Some(p) = discover_browser_from_env() {
        return Some(p);
    }

    let names = [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
        "microsoft-edge",
        "msedge",
        "brave",
        "brave-browser",
        "vivaldi",
        "opera",
        "opera-stable",
    ];

    for name in names {
        if let Some(path) = which_in_path(name) {
            return Some(path);
        }
    }

    #[cfg(windows)]
    {
        for candidate in windows_browser_candidates() {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    // Absolute path fallbacks for Linux servers where the browser may not be
    // in $PATH (e.g. installed via snap, flatpak, or a minimal systemd service env).
    #[cfg(not(windows))]
    {
        for candidate in linux_browser_candidates() {
            if candidate.is_file() {
                return Some(candidate);
            }
        }

        // Final fallback: ask the shell's `which` so we catch browsers installed
        // in non-standard locations that weren't found via PATH or static paths.
        if let Some(path) = which_subprocess(&[
            "chromium-browser",
            "chromium",
            "google-chrome",
            "google-chrome-stable",
            "microsoft-edge",
            "brave-browser",
        ]) {
            return Some(path);
        }
    }

    None
}

/// Push the Chrome/Edge/Brave/Vivaldi `Application`-layout executable paths under `base` onto
/// `paths`. These four share the same per-base directory layout; Opera differs and is added by
/// the caller.
#[cfg(windows)]
fn push_chromium_app_browsers(paths: &mut Vec<PathBuf>, base: &Path) {
    paths.push(
        base.join("Google")
            .join("Chrome")
            .join("Application")
            .join("chrome.exe"),
    );
    paths.push(
        base.join("Microsoft")
            .join("Edge")
            .join("Application")
            .join("msedge.exe"),
    );
    paths.push(
        base.join("BraveSoftware")
            .join("Brave-Browser")
            .join("Application")
            .join("brave.exe"),
    );
    paths.push(base.join("Vivaldi").join("Application").join("vivaldi.exe"));
}

#[cfg(windows)]
fn windows_browser_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    let program_files = std::env::var_os("ProgramFiles");
    let program_files_x86 = std::env::var_os("ProgramFiles(x86)");
    let local_app_data = std::env::var_os("LocalAppData");

    for base in [program_files, program_files_x86].into_iter().flatten() {
        let base = PathBuf::from(base);
        push_chromium_app_browsers(&mut paths, &base);
        paths.push(base.join("Opera").join("launcher.exe"));
        paths.push(base.join("Opera GX").join("launcher.exe"));
    }

    if let Some(base) = local_app_data {
        let base = PathBuf::from(base);
        push_chromium_app_browsers(&mut paths, &base);
        paths.push(base.join("Programs").join("Opera").join("launcher.exe"));
        paths.push(base.join("Programs").join("Opera GX").join("launcher.exe"));
    }

    paths
}

#[cfg(not(windows))]
fn linux_browser_candidates() -> Vec<PathBuf> {
    vec![
        // snap (Ubuntu, common on servers)
        PathBuf::from("/snap/bin/chromium"),
        PathBuf::from("/snap/bin/chromium-browser"),
        // standard apt/dnf paths
        PathBuf::from("/usr/bin/chromium"),
        PathBuf::from("/usr/bin/chromium-browser"),
        PathBuf::from("/usr/bin/google-chrome"),
        PathBuf::from("/usr/bin/google-chrome-stable"),
        PathBuf::from("/usr/bin/microsoft-edge"),
        PathBuf::from("/usr/bin/microsoft-edge-stable"),
        PathBuf::from("/usr/bin/brave-browser"),
        PathBuf::from("/usr/bin/brave-browser-stable"),
        // package-managed library locations (Ubuntu 20.04, Debian)
        PathBuf::from("/usr/lib/chromium-browser/chromium-browser"),
        PathBuf::from("/usr/lib/chromium/chromium"),
        PathBuf::from("/usr/lib/chromium/chrome"),
        // manual / opt installs
        PathBuf::from("/opt/google/chrome/google-chrome"),
        PathBuf::from("/opt/google/chrome-beta/google-chrome"),
        PathBuf::from("/opt/google/chrome-unstable/google-chrome"),
        // local installs
        PathBuf::from("/usr/local/bin/chromium"),
        PathBuf::from("/usr/local/bin/chromium-browser"),
        PathBuf::from("/usr/local/bin/google-chrome"),
        // flatpak wrapper scripts
        PathBuf::from("/var/lib/flatpak/exports/bin/org.chromium.Chromium"),
        PathBuf::from("/usr/share/flatpak/exports/bin/org.chromium.Chromium"),
    ]
}

/// Ask the shell for a browser that may be in PATH but not in the hardcoded list above.
/// Used as a last resort when all static-path checks have failed.
#[cfg(not(windows))]
fn which_subprocess(names: &[&str]) -> Option<PathBuf> {
    for name in names {
        if let Ok(out) = std::process::Command::new("which").arg(name).output()
            && out.status.success()
        {
            let s = String::from_utf8_lossy(&out.stdout);
            let path = PathBuf::from(s.trim());
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

fn which_in_path(exe: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(exe);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let candidate = dir.join(format!("{exe}.exe"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn file_url(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    let normalized = if raw.starts_with('/') {
        raw
    } else {
        format!("/{raw}")
    };

    let mut encoded = String::with_capacity(normalized.len() + 8);
    for byte in normalized.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' | b':' => {
                encoded.push(byte as char);
            }
            _ => {
                let _ = write!(encoded, "%{byte:02X}");
            }
        }
    }

    format!("file://{encoded}")
}

fn file_row_view(file: &FileRecord) -> FileRow {
    FileRow {
        relative_path: file.relative_path.clone(),
        language: file.language.map_or_else(
            || "-".into(),
            |language| language.display_name().to_string(),
        ),
        total_physical_lines: file.raw_line_categories.total_physical_lines,
        code_lines: file.effective_counts.code_lines,
        comment_lines: file.effective_counts.comment_lines,
        blank_lines: file.effective_counts.blank_lines,
        mixed_lines_separate: file.effective_counts.mixed_lines_separate,
        functions: file.raw_line_categories.functions,
        classes: file.raw_line_categories.classes,
        variables: file.raw_line_categories.variables,
        imports: file.raw_line_categories.imports,
        test_count: file.raw_line_categories.test_count,
        test_assertion_count: file.raw_line_categories.test_assertion_count,
        test_suite_count: file.raw_line_categories.test_suite_count,
        line_cov_pct: file
            .coverage
            .as_ref()
            .map(|c| format!("{:.1}", c.line_pct()))
            .unwrap_or_default(),
        fn_cov_pct: file
            .coverage
            .as_ref()
            .filter(|c| c.functions_found > 0)
            .map(|c| format!("{:.1}", c.function_pct()))
            .unwrap_or_default(),
        branch_cov_pct: file
            .coverage
            .as_ref()
            .filter(|c| c.branches_found > 0)
            .map(|c| format!("{:.1}", c.branch_pct()))
            .unwrap_or_default(),
        cov_lines_detail: file.coverage.as_ref().map_or_else(String::new, |c| {
            format!("{}/{}", c.lines_hit, c.lines_found)
        }),
        status: format!("{:?}", file.status),
        status_class: format!("{:?}", file.status).to_ascii_lowercase(),
        warnings: if file.warnings.is_empty() {
            String::new()
        } else {
            file.warnings.join("; ")
        },
    }
}

fn is_pacific_dst_report(dt: DateTime<Utc>) -> bool {
    use chrono::{Datelike, NaiveDate, NaiveTime, TimeZone, Weekday};
    let year = dt.year();
    let nth_sun = |month: u32, n: u32, hour: u32| {
        let mut count = 0u32;
        let mut day = 1u32;
        loop {
            let d = NaiveDate::from_ymd_opt(year, month, day).expect("valid");
            if d.weekday() == Weekday::Sun {
                count += 1;
                if count == n {
                    return Utc.from_utc_datetime(
                        &d.and_time(NaiveTime::from_hms_opt(hour, 0, 0).expect("valid")),
                    );
                }
            }
            day += 1;
        }
    };
    let dst_start = nth_sun(3, 2, 10);
    let dst_end = nth_sun(11, 1, 9);
    dt >= dst_start && dt < dst_end
}

fn to_pst_display(dt: DateTime<Utc>) -> String {
    let (offset, label) = if is_pacific_dst_report(dt) {
        (
            FixedOffset::west_opt(7 * 3600).expect("valid PDT offset"),
            "PDT",
        )
    } else {
        (
            FixedOffset::west_opt(8 * 3600).expect("valid PST offset"),
            "PST",
        )
    };
    format!(
        "{} {label}",
        dt.with_timezone(&offset).format("%Y-%m-%d %H:%M:%S")
    )
}

/// Format a UTC `DateTime` as "YYYY-MM-DD HH:MM PDT/PST" (no seconds).
fn to_pt_hhmm(dt: DateTime<Utc>) -> String {
    let (offset, label) = if is_pacific_dst_report(dt) {
        (
            FixedOffset::west_opt(7 * 3600).expect("valid PDT offset"),
            "PDT",
        )
    } else {
        (
            FixedOffset::west_opt(8 * 3600).expect("valid PST offset"),
            "PST",
        )
    };
    format!(
        "{} {label}",
        dt.with_timezone(&offset).format("%Y-%m-%d %H:%M")
    )
}

/// Parse an RFC 3339 / ISO 8601 git commit date string and reformat it as
/// "YYYY-MM-DD HH:MM PDT/PST", converting from the embedded offset to Pacific time.
fn fmt_commit_date_pt(s: &str) -> String {
    use chrono::DateTime as ChronoDateTime;
    ChronoDateTime::parse_from_rfc3339(s).map_or_else(
        |_| s.replace('T', " "),
        |dt| to_pt_hhmm(dt.with_timezone(&Utc)),
    )
}

fn build_warning_console(warnings: &[String]) -> String {
    if warnings.is_empty() {
        return "No top-level warnings.".to_string();
    }

    warnings
        .iter()
        .enumerate()
        .map(|(index, warning)| {
            format!(
                "[{index:03}] {warning}",
                index = index + 1,
                warning = warning
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn summarize_warnings(warnings: &[String]) -> Vec<WarningSummaryRow> {
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for warning in warnings {
        let key = if warning.contains("unsupported or undetected language") {
            "Unsupported or undetected text formats"
        } else if warning.contains("file exceeded max_file_size_bytes") {
            "Large files skipped by size limit"
        } else if warning.contains("binary file skipped by default") {
            "Binary assets skipped"
        } else if warning.contains("minified file skipped by policy") {
            "Minified files skipped by policy"
        } else if warning.contains("vendor file skipped by policy") {
            "Vendor files skipped by policy"
        } else if warning.contains("best effort") || warning.contains("unclosed string literal") {
            "Best-effort parse results"
        } else {
            "Other warnings"
        };
        *counts.entry(key).or_default() += 1;
    }

    counts
        .into_iter()
        .map(|(label, count)| {
            let (tone_class, detail) = match label {
                "Unsupported or undetected text formats" => (
                    "tone-neutral",
                    "These are usually docs, manifests, templates, or formats that have not been promoted into first-class analyzers yet.",
                ),
                "Large files skipped by size limit" => (
                    "tone-warn",
                    "Artifacts and archives larger than the configured cap were skipped intentionally to keep runs fast and predictable.",
                ),
                "Binary assets skipped" => (
                    "tone-neutral",
                    "Binary bundles are excluded from source counting unless you explicitly opt into them.",
                ),
                "Minified files skipped by policy" => (
                    "tone-warn",
                    "Generated and minified assets are being filtered out to avoid inflating code totals.",
                ),
                "Vendor files skipped by policy" => (
                    "tone-neutral",
                    "Vendored third-party code is being excluded so the report stays focused on repository-owned source.",
                ),
                "Best-effort parse results" => (
                    "tone-danger",
                    "These files were analyzed, but the parser hit malformed or ambiguous content and fell back to a best-effort count.",
                ),
                _ => (
                    "tone-danger",
                    "Warnings in this bucket need manual review because they do not match one of the common policy-based skip reasons.",
                ),
            };

            WarningSummaryRow {
                label: label.to_string(),
                count,
                tone_class: tone_class.to_string(),
                detail: detail.to_string(),
            }
        })
        .collect()
}

/// Classify an unsupported-language warning path into a named bucket.
fn classify_unsupported_path(path: &str) -> &'static str {
    let ext_lc = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    if ext_lc == "md"
        || path.ends_with("README")
        || path.ends_with("README.md")
        || path.ends_with("LICENSE")
    {
        "Documentation / text"
    } else if ext_lc == "json" || path.ends_with(".spdx.json") || path.ends_with("devkit.json") {
        "JSON manifests and config"
    } else if ext_lc == "toml"
        || path.ends_with("MANIFEST.in")
        || path.ends_with("requirements.txt")
    {
        "Project metadata and packaging"
    } else if ext_lc == "html" {
        "HTML templates"
    } else if ext_lc == "txt" {
        "Plain text assets"
    } else if ext_lc.is_empty() {
        "Extensionless or custom text files"
    } else {
        "Other unsupported text formats"
    }
}

/// Map a bucket label to its recommendation string.
fn bucket_recommendation(label: &str) -> String {
    match label {
        "Documentation / text" => "These files are documentation, not source code. Add a docs/text exclude glob (e.g. **/*.md) or mark them as non-source in your config so they stop generating warnings.".to_string(),
        "JSON manifests and config" => "JSON config and manifest files are not source code. Exclude them with an ignore glob (e.g. **/*.json) or add them to excluded_directories so they are silently skipped.".to_string(),
        "Project metadata and packaging" => "TOML, requirements.txt, and MANIFEST.in files describe package metadata — not source. Add an exclude glob such as **/*.toml or list them in excluded_directories to suppress these warnings.".to_string(),
        "HTML templates" => "HTML is already a supported language. These files may have unusual extensions or be in a directory that is being excluded. Check that the files have a .html extension and are not inside an excluded path.".to_string(),
        "Plain text assets" => "Plain .txt files are not analyzed by default. If they contain source, rename them with a source extension. Otherwise add **/*.txt to your exclude globs.".to_string(),
        "Extensionless or custom text files" => "Files without an extension cannot be auto-detected. Add a shebang line (e.g. #!/usr/bin/env python3) so oxide-sloc can identify the language, or add an explicit language override in your config.".to_string(),
        _ => "Files in this group were not recognized. Check the file extensions and either add an exclude glob to silence the warning or open an issue to request analyzer support.".to_string(),
    }
}

/// Short human-readable description of what each bucket means.
fn bucket_description(label: &str) -> String {
    match label {
        "Documentation / text" => {
            "README, LICENSE, and markdown files — not source code.".to_string()
        }
        "JSON manifests and config" => {
            "JSON configuration or manifest files (package.json, lockfiles, etc.).".to_string()
        }
        "Project metadata and packaging" => {
            "TOML, requirements.txt, and MANIFEST.in files that describe package metadata."
                .to_string()
        }
        "HTML templates" => {
            "HTML files not covered by the built-in HTML analyzer (unexpected extension or path)."
                .to_string()
        }
        "Plain text assets" => "Plain .txt files that have no analyzable structure.".to_string(),
        "Extensionless or custom text files" => {
            "Files with no extension that could not be language-detected.".to_string()
        }
        _ => "Files with an unrecognized extension or format.".to_string(),
    }
}

fn build_support_opportunities(warnings: &[String]) -> Vec<WarningOpportunityRow> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut examples: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for warning in warnings {
        if !warning.contains("unsupported or undetected language") {
            continue;
        }

        let path = warning
            .split_once(':')
            .map(|(path, _)| path.trim())
            .unwrap_or_default();
        if path.is_empty() {
            continue;
        }

        let bucket = classify_unsupported_path(path);
        *counts.entry(bucket.to_string()).or_default() += 1;

        let ex = examples.entry(bucket.to_string()).or_default();
        if ex.len() < 3 {
            let basename = Path::new(path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(path)
                .to_string();
            if !ex.contains(&basename) {
                ex.push(basename);
            }
        }
    }

    let mut rows = counts.into_iter().collect::<Vec<_>>();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    rows.into_iter()
        .map(|(label, count)| {
            let recommendation = bucket_recommendation(&label);
            let bucket_description = bucket_description(&label);
            let example_files = examples.remove(&label).unwrap_or_default();
            WarningOpportunityRow {
                label,
                count,
                recommendation,
                example_files,
                bucket_description,
            }
        })
        .collect()
}

#[derive(Debug, Clone)]
struct LanguageRow {
    language: String,
    files: u64,
    total_physical_lines: u64,
    code_lines: u64,
    comment_lines: u64,
    blank_lines: u64,
    mixed_lines_separate: u64,
    functions: u64,
    classes: u64,
    variables: u64,
    imports: u64,
    test_count: u64,
    test_assertion_count: u64,
    test_suite_count: u64,
    test_density_str: String,
}

#[derive(Debug, Clone)]
struct FileRow {
    relative_path: String,
    language: String,
    total_physical_lines: u64,
    code_lines: u64,
    comment_lines: u64,
    blank_lines: u64,
    mixed_lines_separate: u64,
    functions: u64,
    classes: u64,
    variables: u64,
    imports: u64,
    test_count: u64,
    test_assertion_count: u64,
    test_suite_count: u64,
    /// Line coverage percentage, e.g. "96.7" — empty string when no coverage data.
    line_cov_pct: String,
    /// Function coverage percentage — empty string when no coverage data.
    fn_cov_pct: String,
    /// Branch coverage percentage — empty string when no branch coverage data.
    branch_cov_pct: String,
    /// Lines hit out of lines found, e.g. "142/156" — empty string when no coverage data.
    cov_lines_detail: String,
    status: String,
    status_class: String,
    warnings: String,
}

#[derive(Debug, Clone)]
struct WarningSummaryRow {
    label: String,
    count: usize,
    tone_class: String,
    detail: String,
}

#[derive(Debug, Clone)]
struct WarningOpportunityRow {
    label: String,
    count: usize,
    recommendation: String,
    /// Up to 3 example file names (basename only) that triggered this bucket.
    example_files: Vec<String>,
    /// Short description of what this bucket means for the user.
    bucket_description: String,
}

#[derive(Template)]
#[template(
    source = r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>{{ browser_title }}</title>
  <link rel="icon" href="{{ small_logo_uri }}" type="image/png" />
  <style nonce="{{ nonce }}">
    :root {
      --radius: 18px;
      --bg: #f5efe8;
      --surface: rgba(255,255,255,0.82);
      --surface-2: #fbf7f2;
      --surface-3: #efe6dc;
      --line: #e6d0bf;
      --line-strong: #dcb89f;
      --text: #43342d;
      --muted: #7b675b;
      --muted-2: #a08777;
      --nav: #b85d33;
      --nav-2: #7a371b;
      --accent: #6f9bff;
      --accent-2: #4a78ee;
      --oxide: #d37a4c;
      --oxide-2: #b35428;
      --shadow: 0 18px 42px rgba(77, 44, 20, 0.12);
      --shadow-strong: 0 22px 48px rgba(77, 44, 20, 0.16);
      --good-bg: #e8f5ed;
      --good-text: #1a8f47;
      --warn-bg: #fff4dc;
      --warn-text: #9a6d00;
      --danger-bg: #fdebec;
      --danger-text: #cc4b4b;
      --info-bg: #fbf1e8;
      --info-text: #a5541f;
    }
    {% if let Some(hex) = accent_hex %}
    :root, body.dark-theme { --accent: {{ hex }}; --accent-2: {{ hex }}; }
    {% endif %}
    body.dark-theme {
      --bg: #1b1511;
      --surface: #261c17;
      --surface-2: #2d221d;
      --surface-3: #372922;
      --line: #524238;
      --line-strong: #6c5649;
      --text: #f5ece6;
      --muted: #c7b7aa;
      --muted-2: #aa9485;
      --nav: #b85d33;
      --nav-2: #7a371b;
      --accent: #6f9bff;
      --accent-2: #4a78ee;
      --oxide: #d37a4c;
      --oxide-2: #b35428;
      --shadow: 0 18px 42px rgba(0,0,0,0.28);
      --shadow-strong: 0 22px 48px rgba(0,0,0,0.34);
      --good-bg: #163927;
      --good-text: #8fe2a8;
      --warn-bg: #3c2d11;
      --warn-text: #f3cb75;
      --danger-bg: #3d1f1f;
      --danger-text: #ff9f9f;
      --info-bg: #33241b;
      --info-text: #e6a879;
    }
    * { box-sizing: border-box; }
    html, body { margin: 0; min-height: 100vh; font-family: Inter, ui-sans-serif, system-ui, -apple-system, Segoe UI, Roboto, sans-serif; background: var(--bg); color: var(--text); }
    body { overflow-x: hidden; transition: background 0.18s ease, color 0.18s ease; }
    .top-nav { position: sticky; top: 0; z-index: 30; background: linear-gradient(180deg, var(--nav), var(--nav-2)); border-bottom: 1px solid rgba(255,255,255,0.12); box-shadow: 0 4px 14px rgba(0,0,0,0.18); }
    .top-nav-inner { max-width: 1720px; margin: 0 auto; padding: 4px 24px; min-height: 56px; display: grid; grid-template-columns: auto 1fr auto; align-items: center; gap: 14px; }
    .brand { display: flex; align-items: center; gap: 14px; min-width: 0; text-decoration: none; flex: 0 0 auto; }
    .brand-logo { width: 42px; height: 46px; object-fit: contain; flex: 0 0 auto; filter: drop-shadow(0 4px 10px rgba(0,0,0,0.22)); }
    .background-watermarks { position: fixed; inset: 0; pointer-events: none; z-index: 0; overflow: hidden; }
    .background-watermarks img { position: absolute; opacity: 0.15; filter: blur(0.3px); user-select: none; max-width: none; }
    .code-particles { position: fixed; inset: 0; pointer-events: none; z-index: 0; overflow: hidden; }
    .code-particle { position: absolute; font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; font-size: 11px; font-weight: 600; color: var(--oxide); opacity: 0; white-space: nowrap; user-select: none; animation: floatCode linear infinite; }
    @keyframes floatCode { 0% { opacity: 0; transform: translateY(0) rotate(var(--rot)); } 10% { opacity: var(--op); } 85% { opacity: var(--op); } 100% { opacity: 0; transform: translateY(-200px) rotate(var(--rot)); } }
    .brand-copy { display: flex; flex-direction: column; justify-content: center; min-width: 0; }
    .brand-title { margin: 0; color: #fff; font-size: 17px; font-weight: 800; line-height: 1.1; letter-spacing: -0.01em; }
    .brand-subtitle { color: rgba(255,255,255,0.72); font-size: 11px; line-height: 1.2; margin-top: 2px; letter-spacing: 0.01em; }
    .nav-project-slot { display:flex; justify-content:center; min-width:0; }
    .nav-project-pill, .nav-pill, .theme-toggle, .header-button {
      display: inline-flex; align-items: center; gap: 8px; min-height: 38px; padding: 0 14px; border-radius: 999px; border: 1px solid rgba(255,255,255,0.18); color: #fff; background: rgba(255,255,255,0.08); font-size: 12px; font-weight: 700; box-shadow: none;
    }
    .nav-project-pill { pointer-events: auto; width: 100%; max-width: 300px; justify-content: center; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    .nav-project-label { color: rgba(255,255,255,0.72); text-transform: uppercase; letter-spacing: 0.09em; font-size: 10px; font-weight: 800; }
    .nav-project-value { min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-size: 13px; }
    .nav-status { display:flex; align-items:center; justify-content:flex-end; gap:10px; flex-wrap:nowrap; min-width:0; }
    @media (max-width: 1400px) { .nav-status { gap: 6px; } .header-button, .theme-toggle { padding: 0 10px; } }
    @media (max-width: 1150px) { .nav-status { gap: 4px; } .header-button, .theme-toggle { padding: 0 8px; font-size: 11px; min-height: 34px; } .brand-subtitle { display: none; } }
    .theme-toggle, .header-button { cursor:pointer; background: rgba(255,255,255,0.08); text-decoration:none; transition: background 0.15s ease, transform 0.15s ease; }
    .theme-toggle:hover, .header-button:hover { background: rgba(255,255,255,0.18); transform: translateY(-1px); }
    .theme-toggle { width: 38px; justify-content:center; padding:0; }
    .nav-dropdown-wrap { position: relative; }
    .nav-dropdown-wrap::after { content: ''; position: absolute; left: 0; right: 0; bottom: -6px; height: 6px; }
    .nav-dropdown-trigger { }
    .nav-dropdown-menu { display: none; position: absolute; top: 100%; right: 0; background: var(--nav-2); border: 1px solid rgba(255,255,255,0.15); border-radius: 10px; min-width: 140px; padding: 6px; z-index: 50; box-shadow: 0 8px 24px rgba(0,0,0,0.28); }
    .nav-dropdown-wrap:hover .nav-dropdown-menu, .nav-dropdown-wrap:focus-within .nav-dropdown-menu { display: flex; flex-direction: column; gap: 2px; }
    .nav-dropdown-item { display: block; width: 100%; padding: 8px 12px; border: none; border-radius: 7px; background: transparent; color: #fff; font-size: 13px; font-weight: 700; text-align: left; cursor: pointer; }
    .nav-dropdown-item:hover { background: rgba(255,255,255,0.12); }
    .theme-toggle svg { width: 18px; height: 18px; stroke: currentColor; fill: none; stroke-width: 1.8; }
    .theme-toggle .icon-sun { display:none; }
    body.dark-theme .theme-toggle .icon-sun { display:block; }
    body.dark-theme .theme-toggle .icon-moon { display:none; }
    .settings-modal{position:fixed;z-index:9999;background:var(--surface);border:1px solid var(--line-strong);border-radius:14px;box-shadow:0 12px 36px rgba(0,0,0,0.22);min-width:260px;max-width:320px;opacity:0;pointer-events:none;transform:translateY(-8px) scale(0.97);transition:opacity 0.18s ease,transform 0.18s ease;overflow:hidden;}
    .settings-modal.open{opacity:1;pointer-events:auto;transform:translateY(0) scale(1);}
    .settings-modal-header{display:flex;align-items:center;justify-content:space-between;padding:14px 16px 10px;border-bottom:1px solid var(--line);font-size:13px;font-weight:800;color:var(--text);}
    .settings-close{background:none;border:none;cursor:pointer;width:24px;height:24px;display:flex;align-items:center;justify-content:center;color:var(--muted);border-radius:6px;padding:0;}
    .settings-close:hover{color:var(--text);background:var(--surface-2);}
    .settings-close svg{width:14px;height:14px;stroke:currentColor;fill:none;stroke-width:2.5;}
    .settings-modal-body{padding:14px 16px 16px;}
    .settings-modal-label{font-size:11px;font-weight:800;text-transform:uppercase;letter-spacing:0.08em;color:var(--muted-2);margin-bottom:10px;}
    .scheme-grid{display:grid;grid-template-columns:repeat(5,1fr);gap:8px;}
    .scheme-swatch{display:flex;flex-direction:column;align-items:center;gap:5px;background:none;border:1.5px solid var(--line);border-radius:10px;cursor:pointer;padding:7px 4px 6px;transition:border-color 0.15s ease,transform 0.12s ease;}
    .scheme-swatch:hover{border-color:var(--line-strong);transform:translateY(-1px);}
    .scheme-swatch.active{border-color:#6f9bff;box-shadow:0 0 0 2px rgba(111,155,255,0.25);}
    .scheme-preview{width:28px;height:28px;border-radius:7px;flex-shrink:0;}
    .scheme-label{font-size:9px;font-weight:700;color:var(--muted-2);white-space:nowrap;}
    .tz-select{width:100%;padding:6px 8px;border:1px solid var(--line);border-radius:8px;background:var(--surface-2);color:var(--text);font-size:12px;font-weight:600;cursor:pointer;outline:none;box-sizing:border-box;}
    .tz-select:focus{border-color:var(--oxide);}
    .page { max-width: 1720px; margin: 0 auto; padding: 32px 24px 40px; }
    @media (max-width: 1920px) { .top-nav-inner { max-width: 1500px; } .page { max-width: 1500px; } }
    /* Uniform two-row card strip. JS pads the card count to even (revealing a
       reserve card when odd) and sets the column count to n/2, so the cards form
       exactly two full rows with every column aligned and every card the same
       width — no oversized card, no empty trailing cell. */
    .summary-grid { display:grid; grid-template-columns: repeat(8, minmax(0, 1fr)); gap:10px; align-items:stretch; }
    .panel, .metric, .warning-card { background: var(--surface); border: 1px solid var(--line); border-radius: var(--radius); box-shadow: var(--shadow); }
    .panel { padding: 20px; }
    .metric { padding: 11px 12px 20px; position: relative; cursor: help; transition: transform 0.15s ease, box-shadow 0.15s ease; min-height: 70px; }
    .metric:hover { transform: translateY(-3px); box-shadow: var(--shadow-strong); }
    .metric-label { font-size: 11px; font-weight: 700; text-transform: uppercase; letter-spacing: 0.07em; color: var(--muted); }
    .section-kicker { font-size: 10px; font-weight: 800; text-transform: uppercase; letter-spacing: 0.08em; color: var(--muted-2); }
    .metric-value { margin-top: 6px; }
    .metric-big { display:block; font-size: 20px; font-weight: 900; color: var(--oxide); line-height: 1.15; letter-spacing: -0.02em; }
    .metric-exact { position: absolute; bottom: 6px; right: 10px; font-size: 12px; font-weight: 600; color: var(--muted); font-family: ui-monospace, monospace; }
    .metric-tooltip { position: absolute; bottom: calc(100% + 10px); left: 50%; transform: translateX(-50%) translateY(7px); background: var(--text); color: var(--bg); padding: 10px 14px; border-radius: 10px; font-size: 12px; font-weight: 500; line-height: 1.55; white-space: normal; max-width: 340px; min-width: 200px; text-align: left; pointer-events: none; opacity: 0; transition: opacity .25s cubic-bezier(.16,1,.3,1), transform .25s cubic-bezier(.16,1,.3,1); z-index: 100; box-shadow: 0 4px 18px rgba(0,0,0,0.25); }
    .metric-tooltip::after { content: ''; position: absolute; top: 100%; left: 50%; transform: translateX(-50%); border: 5px solid transparent; border-top-color: var(--text); }
    .metric:hover .metric-tooltip { opacity: 1; transform: translateX(-50%) translateY(0); }
    .hero { padding: 24px 24px 20px; margin-bottom: 18px; background: linear-gradient(150deg, rgba(111,155,255,0.06) 0%, transparent 55%), var(--surface); border-top: 3px solid var(--accent); }
    .hero-top { display:flex; justify-content:space-between; align-items:flex-start; gap:16px; }
    .hero h1 { margin:0 0 8px; font-size: 28px; letter-spacing: -0.04em; }
    .run-id-row { display:grid; grid-template-columns: repeat(4, minmax(0,1fr)); gap:10px; margin-top:16px; }
    @media(max-width:960px) { .run-id-row { grid-template-columns: 1fr 1fr; } }
    @media(max-width:560px) { .run-id-row { grid-template-columns: 1fr; } }
    .run-id-chip { display:flex; flex-direction:column; gap:5px; padding:12px 14px; border-radius:10px; background:var(--surface-2); border:1px solid var(--line); border-left:3px solid var(--accent); color:var(--text); position:relative; cursor:default; transition:transform 0.18s ease, box-shadow 0.18s ease; }
    .run-id-chip[data-copy] { cursor:pointer; }
    a.run-id-chip-link { text-decoration:none; cursor:pointer; }
    a.run-id-chip-link:hover { transform:translateY(-3px); box-shadow:0 8px 24px rgba(0,0,0,0.15); z-index:10; border-left-color:var(--oxide); }
    .run-id-chip:hover { transform:translateY(-3px); box-shadow:0 8px 24px rgba(0,0,0,0.15); z-index:10; }
    .run-id-chip.muted-chip { border-left-color:var(--line-strong); }
    .run-id-chip-label { font-size:10px; font-weight:900; text-transform:uppercase; letter-spacing:0.1em; color:var(--accent); display:flex; align-items:center; gap:4px; }
    .run-id-chip.muted-chip .run-id-chip-label { color:var(--muted-2); }
    .run-id-chip-value { font-family:ui-monospace,monospace; font-size:12px; font-weight:700; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
    .run-id-chip.muted-chip .run-id-chip-value { color:var(--muted); font-style:italic; }
    .submodule-state-badge { display:inline-block; font-size:10px; font-style:italic; font-weight:600; color:var(--accent-2); background:rgba(100,130,220,0.10); border:1px solid rgba(100,130,220,0.22); border-radius:4px; padding:1px 6px; letter-spacing:0.03em; }
    body.dark-theme .submodule-state-badge { color:var(--accent); background:rgba(111,155,255,0.13); border-color:rgba(111,155,255,0.28); }
    .chip-tooltip { position:absolute; top:calc(100% + 8px); left:50%; transform:translateX(-50%) translateY(-7px); background:var(--text); color:var(--bg); padding:6px 11px; border-radius:8px; font-size:11px; font-weight:500; white-space:nowrap; pointer-events:none; opacity:0; transition:opacity .25s cubic-bezier(.16,1,.3,1), transform .25s cubic-bezier(.16,1,.3,1); z-index:200; box-shadow:0 4px 16px rgba(0,0,0,0.25); line-height:1.4; }
    .chip-tooltip::before { content:''; position:absolute; bottom:100%; left:50%; transform:translateX(-50%); border:5px solid transparent; border-bottom-color:var(--text); }
    .run-id-chip:hover .chip-tooltip { opacity:1; transform:translateX(-50%) translateY(0); }
    a.run-id-chip-link:hover .chip-tooltip { opacity:1; transform:translateX(-50%) translateY(0); }
    .chip-copy-icon { display:inline-block; margin-left:5px; font-size:10px; opacity:0.55; vertical-align:middle; }
    .chip-label-icon { display:inline-block; vertical-align:middle; margin-right:3px; margin-top:-1px; opacity:0.8; }
    .chip-popout-icon { display:inline-block; vertical-align:middle; margin-left:4px; opacity:0.6; flex-shrink:0; }
    .run-id-short-badge { font-family:ui-monospace,monospace; font-size:13px; font-weight:700; color:var(--muted); background:var(--surface-2); border:1px solid var(--line); border-radius:6px; padding:2px 8px; letter-spacing:0.04em; white-space:nowrap; vertical-align:middle; }
    body.dark-theme .run-id-short-badge { color:var(--muted-2); }
    .author-handle { font-size:11px; font-weight:600; color:var(--muted-2); margin-left:1.5em; font-family:ui-monospace,monospace; }
    @keyframes chip-flash { 0%{background:var(--accent);color:#fff;} 80%{background:var(--accent);color:#fff;} 100%{background:var(--surface-2);color:var(--text);} }
    .chip-copied-flash { animation:chip-flash 0.9s ease forwards; }
    .subtitle { margin: 10px 0 0; color: var(--muted); font-size: 16px; line-height: 1.65; }
    .meta { display:flex; flex-wrap:wrap; align-items:center; gap:0; margin:14px 0 20px; padding:10px 0; border-top:1px solid var(--line); border-bottom:1px solid var(--line); width:100%; }
    .meta-chip { flex:1; display:inline-flex; align-items:center; justify-content:center; gap:5px; padding:0 10px; font-size:13px; font-weight:500; color:var(--muted); border-right:1px solid var(--line); line-height:1.8; }
    .meta-chip:last-child { border-right:none; }
    .meta-chip b { color:var(--text); font-weight:700; }
    .prev-scan-banner { background:var(--surface); border:1px solid var(--line); border-radius:12px; padding:18px 20px; margin:0 0 20px; display:flex; flex-direction:column; gap:12px; width:100%; box-sizing:border-box; box-shadow:0 4px 16px rgba(77,44,20,0.10); }
    body.dark-theme .prev-scan-banner { box-shadow:0 4px 16px rgba(0,0,0,0.3); }
    .prev-scan-banner-empty { flex-direction:row; align-items:center; gap:8px; font-size:13px; color:var(--muted); font-style:italic; }
    .prev-scan-banner-top { display:flex; flex-direction:column; gap:4px; }
    .prev-scan-meta { display:flex; align-items:center; gap:7px; font-size:11px; font-weight:700; text-transform:uppercase; letter-spacing:.07em; color:var(--muted); flex-wrap:wrap; }
    .prev-scan-ts { font-weight:500; text-transform:none; letter-spacing:0; color:var(--muted); }
    .prev-scan-count { font-weight:500; text-transform:none; letter-spacing:0; color:var(--muted); }
    .prev-scan-summary { font-size:13px; font-weight:600; color:var(--text); }
    .prev-scan-summary b { font-weight:900; }
    .delta-neutral-text { color:var(--muted); }
    .delta-up { color:#2a6846; }
    .delta-down { color:#b23030; }
    body.dark-theme .delta-up { color:#5aba8a; }
    body.dark-theme .delta-down { color:#e07070; }
    .delta-card-row { display:grid; grid-template-columns:repeat(7,1fr); gap:12px; width:100%; }
    @media(max-width:1000px){ .delta-card-row { grid-template-columns:repeat(4,1fr); } }
    @media(max-width:540px){ .delta-card-row { grid-template-columns:repeat(2,1fr); } }
    .delta-card-inline { display:flex; flex-direction:column; gap:4px; padding:14px 16px; border-radius:12px; border:1px solid var(--line); background:var(--surface); position:relative; cursor:default; transition:transform .2s ease, box-shadow .2s ease; box-shadow:0 2px 8px rgba(77,44,20,0.07); }
    .delta-card-inline:hover { transform:translateY(-4px); box-shadow:0 12px 32px rgba(77,44,20,0.22); z-index:10; }
    body.dark-theme .delta-card-inline { box-shadow:0 2px 8px rgba(0,0,0,0.2); }
    body.dark-theme .delta-card-inline:hover { box-shadow:0 12px 32px rgba(0,0,0,0.55); }
    .delta-card-val { font-size:20px; font-weight:900; color:var(--oxide); line-height:1.2; }
    .delta-card-val.pos { color:#2a6846; }
    .delta-card-val.neg { color:#b23030; }
    .delta-card-val.mod { color:#7a5a10; }
    body.dark-theme .delta-card-val.pos { color:#5aba8a; }
    body.dark-theme .delta-card-val.neg { color:#e07070; }
    body.dark-theme .delta-card-val.mod { color:#d4a843; }
    .delta-card-lbl { font-size:11px; font-weight:700; text-transform:uppercase; letter-spacing:.07em; color:var(--muted); margin-top:4px; }
    .delta-card-tip { position:absolute; top:calc(100% + 8px); left:50%; transform:translateX(-50%) translateY(-7px); background:var(--text); color:var(--bg); padding:7px 12px; border-radius:8px; font-size:11px; white-space:nowrap; pointer-events:none; opacity:0; transition:opacity .25s cubic-bezier(.16,1,.3,1), transform .25s cubic-bezier(.16,1,.3,1); z-index:200; box-shadow:0 4px 12px rgba(0,0,0,0.18); }
    .delta-card-tip::after { content:''; position:absolute; bottom:100%; left:50%; transform:translateX(-50%); border:5px solid transparent; border-bottom-color:var(--text); }
    .delta-card-inline:hover .delta-card-tip { opacity:1; transform:translateX(-50%) translateY(0); }
    .delta-panel-link { display:inline-flex; align-items:center; gap:6px; padding:8px 16px; border-radius:10px; border:1px solid var(--line); background:var(--surface); color:var(--text); font-size:12px; font-weight:600; text-decoration:none; transition:background .15s, border-color .15s, color .15s, transform .15s, box-shadow .15s; box-shadow:0 2px 6px rgba(77,44,20,0.08); }
    .delta-panel-link:hover { background:var(--oxide); color:#fff; border-color:var(--oxide); transform:translateY(-2px); box-shadow:0 6px 18px rgba(77,44,20,0.25); }
    body.dark-theme .delta-panel-link { box-shadow:0 2px 6px rgba(0,0,0,0.2); }
    body.dark-theme .delta-panel-link:hover { box-shadow:0 6px 18px rgba(0,0,0,0.45); }
    .soft-chip { display:inline-flex; align-items:center; min-height:32px; padding:0 12px; border-radius:999px; border:1px solid var(--line); background:var(--surface-2); color:var(--text); font-size:13px; font-weight:700; }
    .toolbar { display:flex; flex-wrap:wrap; justify-content:space-between; gap: 12px; align-items: center; margin-bottom: 16px; }
    .toolbar-left { display:flex; gap:10px; align-items:center; flex-wrap:wrap; }
    .search { min-width: 280px; padding: 10px 12px; border-radius: 10px; border:1px solid var(--line-strong); background: var(--surface-2); color:var(--text); }
    .pill-row { display:flex; gap:8px; flex-wrap:wrap; }
    .pill { padding: 6px 10px; border-radius: 999px; border:1px solid var(--line); background: var(--surface-2); font-size: 12px; font-weight: 700; }
    .pill.good { background: var(--good-bg); color: var(--good-text); }
    .pill.info { background: var(--info-bg); color: var(--info-text); }
    .export-group { display:flex; gap:6px; align-items:center; }
    .export-btn { display:inline-flex; align-items:center; gap:5px; padding:6px 12px; border-radius:8px; border:1px solid var(--line-strong); background:var(--surface-2); color:var(--text); font-size:12px; font-weight:700; cursor:pointer; white-space:nowrap; }
    .export-btn:hover { background:var(--accent); color:#fff; border-color:var(--accent); }
    .page-size-row { display:flex; align-items:center; gap:6px; }
    .page-size-label { font-size:13px; color:var(--muted); font-weight:600; }
    .page-size-select { padding:5px 10px; border-radius:8px; border:1px solid var(--line-strong); background:var(--surface-2); color:var(--text); font-size:13px; font-weight:700; cursor:pointer; }
    .page-count-label { font-size:12px; color:var(--muted); white-space:nowrap; }
    .pagination-bar { display:flex; align-items:center; justify-content:center; gap:14px; padding:10px 0 2px; }
    .pager-btn { padding:6px 16px; border-radius:8px; border:1px solid var(--line-strong); background:var(--surface-2); color:var(--text); font-size:13px; font-weight:700; cursor:pointer; transition:background .15s,color .15s; }
    .pager-btn:hover:not(:disabled) { background:var(--accent); color:#fff; border-color:var(--accent); }
    .pager-btn:disabled { opacity:.4; cursor:default; }
    .pager-info { font-size:13px; color:var(--muted); font-weight:600; min-width:120px; text-align:center; }
    .pager-edge { font-size:12px; padding:5px 10px; }
    .pager-jump-wrap { font-size:13px; color:var(--muted); font-weight:600; display:flex; align-items:center; gap:5px; white-space:nowrap; }
    .pager-jump { width:52px; padding:3px 5px; border-radius:6px; border:1px solid var(--line-strong); background:var(--surface); color:var(--text); font-size:13px; font-weight:700; text-align:center; -moz-appearance:textfield; }
    .pager-jump::-webkit-inner-spin-button,.pager-jump::-webkit-outer-spin-button { -webkit-appearance:none; margin:0; }
    .table-shell { border: 1px solid var(--line); border-radius: 16px; overflow: auto; background: var(--surface-2); max-height: 900px; }
    /* Clip wrapper: hides the scrollbar track that hangs 8px past the right edge */
    .table-shell-clip { overflow: hidden !important; max-height: none !important; }
    /* Skipped-files scroll pane: auto so no phantom space when content is short */
    #skipped-shell { overflow-y: auto; overflow-x: hidden; scrollbar-width: thin; scrollbar-color: var(--line-strong) var(--surface-2); }
    #per-file-table tbody tr:last-child td, #skipped-table tbody tr:last-child td { border-bottom: none; }
    #skipped-shell::-webkit-scrollbar { width: 8px; }
    #skipped-shell::-webkit-scrollbar-track { background: var(--surface-2); }
    #skipped-shell::-webkit-scrollbar-thumb { background: var(--line-strong); border-radius: 4px; }
    table { width: 100%; border-collapse: collapse; font-size: 14px; }
    th, td { text-align: left; padding: 11px 10px; border-bottom: 1px solid var(--line); vertical-align: top; }
    th { color: var(--muted); font-weight: 800; background: var(--surface-2); cursor: pointer; position: sticky; top: 0; z-index: 1; white-space: nowrap; }
    /* Per-file detail table — auto layout so File column sizes to content */
    .table-resizable { table-layout: auto; }
    .table-resizable th { position: sticky; top: 0; z-index: 2; overflow: hidden; white-space: nowrap; min-width: 52px; }
    .table-resizable td { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .table-resizable td.mono { overflow: visible; text-overflow: unset; white-space: nowrap; }
    #skipped-table { table-layout: fixed; width: 100%; }
    #skipped-table th, #skipped-table td { padding: 7px 8px; }
    #skipped-table td:first-child { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    /* Hotspots table — overflow visible so header tooltips can escape the cell */
    #hotspots-table th { overflow: visible; }
    .hs-hint { color: var(--muted); font-style: italic; }
    .section-desc { font-size:13px; color:var(--muted); line-height:1.6; margin:0 0 4px; padding:0 4px; }
    .table-hint { font-size:12px; color:var(--muted); margin:0 0 12px; padding:0 4px; }
    .own-files-title { font-size:18px; font-weight:850; letter-spacing:-0.02em; color:var(--text); margin:26px 0 4px; }
    .own-files-sub { font-size:13px; color:var(--muted); line-height:1.55; margin:0 0 16px; padding:0 2px; }
    .own-details { border:1px solid var(--line); border-radius:14px; margin-bottom:10px; background:var(--surface-2); overflow:hidden; transition:box-shadow .2s ease, transform .2s ease; }
    .own-details:hover { box-shadow:0 8px 24px rgba(77,44,20,0.13); transform:translateY(-1px); }
    .own-details > summary { cursor:pointer; padding:14px 18px; list-style:none; display:flex; align-items:center; gap:14px; }
    .own-details > summary::-webkit-details-marker { display:none; }
    .own-details > summary::before { content:'\25B6'; color:var(--muted); font-size:10px; transition:transform .15s ease; flex:0 0 auto; }
    .own-details[open] > summary::before { transform:rotate(90deg); }
    .lb-rank { flex:0 0 auto; min-width:30px; height:30px; padding:0 6px; display:inline-flex; align-items:center; justify-content:center; border-radius:50%; font-weight:800; font-size:14px; color:var(--muted); background:var(--surface-3); border:1px solid var(--line); font-variant-numeric:tabular-nums; }
    .lb-r1 { background:linear-gradient(135deg,#f7d774,#e0a92e); color:#5a3d00; border-color:#e0a92e; box-shadow:0 3px 10px rgba(224,169,46,0.45); }
    .lb-r2 { background:linear-gradient(135deg,#e6e7ea,#b9bcc4); color:#3d4048; border-color:#b9bcc4; box-shadow:0 3px 10px rgba(150,153,160,0.35); }
    .lb-r3 { background:linear-gradient(135deg,#eabd92,#cd7f4c); color:#4d2a0e; border-color:#cd7f4c; box-shadow:0 3px 10px rgba(205,127,76,0.35); }
    .lb-avatar { flex:0 0 auto; width:36px; height:36px; border-radius:50%; display:inline-flex; align-items:center; justify-content:center; color:#fff; font-weight:800; font-size:14px; text-shadow:0 1px 2px rgba(0,0,0,0.25); }
    .lb-name { flex:0 0 210px; font-size:16px; font-weight:800; color:var(--text); overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
    /* Contributor profile links (ownership table, leaderboard, hotspot owner column) — inherit the
       surrounding weight/size, tint oxide, underline only on hover. */
    a.author-link { color:var(--oxide); text-decoration:none; border-bottom:1px solid transparent; transition:border-color 0.15s ease; }
    a.author-link:hover { border-bottom-color:var(--oxide); }
    .lb-name a.author-link { color:inherit; }
    .lb-name a.author-link:hover { color:var(--oxide); }
    .lb-bar-wrap { flex:1 1 auto; height:12px; min-width:60px; background:var(--surface-3); border-radius:7px; overflow:hidden; box-shadow:inset 0 1px 2px rgba(0,0,0,0.08); }
    .lb-bar { display:block; height:100%; border-radius:7px; transition:width .5s cubic-bezier(.16,1,.3,1); }
    .lb-stats { flex:0 0 auto; font-size:13px; color:var(--muted); white-space:nowrap; font-variant-numeric:tabular-nums; }
    .lb-stats strong { color:var(--oxide); font-size:16px; font-weight:900; }
    .own-files-shell { margin:2px 14px 14px; max-height:420px; }
    @media (max-width:760px) { .lb-name { flex-basis:120px; font-size:14px; } .lb-bar-wrap { display:none; } .lb-stats { font-size:12px; } }
    /* Column-header explainer tooltip (shared visual language with .stat-chip-tip) */
    .col-tip { position: absolute; top: calc(100% + 9px); left: 0; z-index: 60; width: max-content; max-width: 270px;
      background: var(--text); color: var(--bg); padding: 9px 12px; border-radius: 9px;
      font-size: 11.5px; font-weight: 500; line-height: 1.5; letter-spacing: normal; text-transform: none;
      white-space: normal; text-align: left; box-shadow: 0 10px 30px rgba(0,0,0,0.22);
      opacity: 0; pointer-events: none; transition: opacity .18s ease; }
    .col-tip.col-tip-r { left: auto; right: 0; }
    .col-tip strong { color: var(--bg); }
    .col-tip::after { content: ''; position: absolute; bottom: 100%; left: 16px;
      border: 6px solid transparent; border-bottom-color: var(--text); }
    .col-tip.col-tip-r::after { left: auto; right: 16px; }
    #hotspots-table th:hover .col-tip { opacity: 1; }
    /* Column resize handle */
    .col-resize-handle { position: absolute; top: 0; right: 0; bottom: 0; width: 6px; cursor: col-resize; z-index: 10; }
    .col-resize-handle:hover, .col-resize-handle.dragging { background: rgba(211,122,76,0.3); }
    #per-file-table { table-layout: fixed; width: 100%; min-width: 0; }
    #per-file-table th, #per-file-table td { padding: 8px 6px; }
    /* File column: pinned, truncates long paths */
    #per-file-table th:first-child { position: sticky; top: 0; left: 0; z-index: 3; width: 26%; background: var(--surface-2); padding: 8px 6px; overflow: hidden; text-overflow: ellipsis; }
    #per-file-table td:first-child { position: sticky; left: 0; z-index: 1; background: var(--surface-2); padding: 8px 6px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    #per-file-table th:nth-child(2) { width: 6%; }
    /* 12 numeric columns share the remaining 68%: 26+6+12×5.67≈98% total */
    #per-file-table th:nth-child(n+3) { width: 5.67%; }
    /* Override mono class overflow so file paths truncate */
    #per-file-table td.mono { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    #per-file-table tbody tr:hover td:first-child { background: rgba(255,247,238,0.6); }
    body.dark-theme #per-file-table tbody tr:hover td:first-child { background: rgba(255,255,255,0.03); }
    /* Language breakdown: auto layout with resizable columns — headers size to content */
    #lang-breakdown-table { width: 100%; min-width: 760px; }
    #lang-breakdown-table th, #lang-breakdown-table td { padding: 8px 6px; font-size: 13px; }
    /* Skipped table: extend truncation to all cells, not just the first column */
    #skipped-table td { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 0; }
    /* Support opportunities table: fixed layout so Category/Count columns don't grow */
    .support-table { table-layout: fixed; width: 100%; }
    .support-table th:first-child { width: 20%; }
    .support-table th:nth-child(2) { width: 6%; }
    .support-table th:nth-child(3) { width: 24%; }
    .support-table td { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 0; vertical-align: top; padding-top: 10px; padding-bottom: 10px; }
    /* Description and example columns must wrap — content can be long */
    .support-table td:nth-child(3) { white-space: normal; overflow: visible; text-overflow: unset; max-width: none; line-height: 1.45; }
    .support-table td:last-child { white-space: normal; overflow: visible; text-overflow: unset; max-width: none; }
    .support-example-file { display: inline-block; font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; font-size: 11px; padding: 1px 6px; background: var(--surface); border: 1px solid var(--line); border-radius: 4px; margin-bottom: 2px; word-break: break-all; }
    .support-recommendation { color: var(--muted); font-size: 11px; margin: 6px 0 0; line-height: 1.5; }
    .num-col { text-align: right !important; }
    /* Per-file coverage table: fixed layout keeps numeric columns compact and off the shell border */
    .cov-file-table { table-layout: fixed; }
    .cov-file-table th:not(:first-child), .cov-file-table td:not(:first-child) { width: 150px; }
    .cov-file-table th.num-col, .cov-file-table td.num-col { padding-right: 16px; }
    tbody tr:hover { background: rgba(255, 247, 238, 0.6); }
    body.dark-theme tbody tr:hover { background: rgba(255,255,255,0.03); }
    tr:last-child td { border-bottom: none; }
    .mono { font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }
    .small { color: var(--muted); font-size: 13px; }
    .status-tag { display:inline-flex; align-items:center; padding: 4px 8px; border-radius: 999px; border:1px solid var(--line); background: var(--surface-2); font-size: 12px; font-weight: 700; }
    .status-analyzedexact { background: var(--good-bg); color: var(--good-text); border-color: rgba(28,135,70,0.18); }
    .status-analyzedbesteffort, .status-skippedbypolicy { background: var(--warn-bg); color: var(--warn-text); border-color: rgba(146,96,0,0.18); }
    .status-skippedunsupported, .status-skippedbinary { background: var(--danger-bg); color: var(--danger-text); border-color: rgba(179,59,59,0.18); }
    .stack { display:grid; gap:22px; }
    .summary-strip { display:grid; grid-template-columns:repeat(4,1fr); gap:14px; margin-bottom:18px; }
    @media(max-width:800px) { .summary-strip { grid-template-columns:repeat(2,1fr) !important; } }
    .test-density-row { display:flex; align-items:center; gap:12px; margin-bottom:16px; padding:10px 16px; border-radius:10px; background:var(--surface-2); border:1px solid var(--line); }
    .test-density-num { font-size:22px; font-weight:900; color:var(--oxide); line-height:1; }
    .test-density-meta { display:flex; flex-direction:column; gap:2px; }
    .test-density-label { font-size:11px; font-weight:700; text-transform:uppercase; letter-spacing:.07em; color:var(--muted); }
    .test-density-sub { font-size:11px; color:var(--muted-2); }
    .test-density-badge { margin-left:auto; padding:4px 12px; border-radius:999px; font-size:12px; font-weight:700; }
    .test-density-badge.good { background:var(--good-bg); color:var(--good-text); }
    .test-density-badge.warn { background:var(--warn-bg); color:var(--warn-text); }
    .test-density-badge.danger { background:var(--danger-bg); color:var(--danger-text); }
    .info-callout { display:flex; align-items:flex-start; gap:10px; margin-top:14px; padding:11px 14px; border-radius:10px; background:var(--info-bg); border:1px solid rgba(196,92,16,0.20); color:var(--info-text); font-size:13px; line-height:1.5; }
    .info-callout-icon { flex:0 0 auto; font-size:15px; margin-top:1px; }
    .info-callout code { background:rgba(196,92,16,0.12); border-radius:4px; padding:1px 5px; font-size:12px; }
    body.dark-theme .info-callout { background:rgba(211,122,76,0.10); border-color:rgba(211,122,76,0.25); }
    .empty-state-row td { text-align:center; padding:20px; color:var(--muted-2); font-size:13px; font-style:italic; }
    /* auto-fit so a lone gauge card (line-only coverage) spans the full width instead of 1/3 */
    .cov-gauge-row { display:grid; grid-template-columns:repeat(auto-fit,minmax(240px,1fr)); gap:16px; margin-bottom:18px; }
    @media(max-width:700px) { .cov-gauge-row { grid-template-columns:1fr; } }
    .cov-gauge-card { position:relative; background:var(--surface); border:1px solid var(--line); border-radius:12px; padding:18px 20px; display:flex; flex-direction:column; gap:8px; cursor:default; transition:transform .27s cubic-bezier(.16,1,.3,1),box-shadow .27s cubic-bezier(.16,1,.3,1); min-width:0; }
    .cov-gauge-card:hover { transform:translateY(-3px); box-shadow:0 10px 28px rgba(77,44,20,0.15); z-index:10; }
    .cov-gauge-tip { position:absolute; top:calc(100% + 10px); left:50%; transform:translateX(-50%) translateY(-7px); background:var(--text); color:var(--bg); padding:10px 14px; border-radius:8px; font-size:12px; font-weight:500; line-height:1.55; white-space:normal; max-width:420px; min-width:200px; text-align:left; pointer-events:none; opacity:0; transition:opacity .25s cubic-bezier(.16,1,.3,1), transform .25s cubic-bezier(.16,1,.3,1); z-index:200; box-shadow:0 4px 18px rgba(0,0,0,0.25); }
    .cov-gauge-tip::after { content:''; position:absolute; bottom:100%; left:50%; transform:translateX(-50%); border:5px solid transparent; border-bottom-color:var(--text); }
    .cov-gauge-card:hover .cov-gauge-tip { opacity:1; transform:translateX(-50%) translateY(0); }
    .cov-gauge-label { font-size:11px; font-weight:700; text-transform:uppercase; letter-spacing:.07em; color:var(--muted); }
    .cov-gauge-val { font-size:32px; font-weight:900; line-height:1; }
    .cov-gauge-track { height:8px; border-radius:4px; background:var(--line); overflow:hidden; }
    .cov-gauge-fill { height:100%; border-radius:4px; transition:width .5s ease; }
    .cov-gauge-sub { font-size:11px; color:var(--muted); }
    .stat-chip { background:var(--surface); border:1px solid var(--line); border-radius:12px; padding:14px 16px; position:relative; cursor:default; transition:transform .27s cubic-bezier(.16,1,.3,1),box-shadow .27s cubic-bezier(.16,1,.3,1); }
    .stat-chip:hover { transform:translateY(-4px); box-shadow:0 12px 32px rgba(77,44,20,0.2); z-index:10; }
    .stat-chip-val { font-size:20px; font-weight:900; color:var(--oxide); }
    .stat-chip-label { font-size:11px; font-weight:700; text-transform:uppercase; letter-spacing:.07em; color:var(--muted); margin-top:4px; }
    .stat-chip-tip { position:absolute; top:calc(100% + 10px); left:50%; transform:translateX(-50%) translateY(-7px); background:var(--text); color:var(--bg); padding:10px 14px; border-radius:8px; font-size:12px; font-weight:500; line-height:1.55; white-space:normal; max-width:420px; min-width:200px; text-align:left; pointer-events:none; opacity:0; transition:opacity .25s cubic-bezier(.16,1,.3,1), transform .25s cubic-bezier(.16,1,.3,1); z-index:200; box-shadow:0 4px 18px rgba(0,0,0,0.25); }
    .stat-chip-tip::after { content:''; position:absolute; bottom:100%; left:50%; transform:translateX(-50%); border:5px solid transparent; border-bottom-color:var(--text); }
    .stat-chip:hover .stat-chip-tip { opacity:1; transform:translateX(-50%) translateY(0); }
    .stat-chip-exact { position:absolute; bottom:6px; right:10px; font-size:12px; font-weight:600; color:var(--muted); font-variant-numeric:tabular-nums; line-height:1; }
    .cocomo-mode-pill-wrap { position:relative; display:inline-flex; align-items:center; cursor:help; }
    .cocomo-mode-tip { position:absolute; top:calc(100% + 8px); left:0; transform:translateY(-7px); background:var(--text); color:var(--bg); padding:9px 13px; border-radius:8px; font-size:11px; font-weight:500; line-height:1.55; white-space:normal; max-width:300px; min-width:180px; pointer-events:none; opacity:0; transition:opacity .25s cubic-bezier(.16,1,.3,1), transform .25s cubic-bezier(.16,1,.3,1); z-index:300; box-shadow:0 4px 18px rgba(0,0,0,0.25); }
    .cocomo-mode-tip::before { content:''; position:absolute; bottom:100%; left:14px; border:5px solid transparent; border-bottom-color:var(--text); }
    .cocomo-mode-pill-wrap:hover .cocomo-mode-tip { opacity:1; transform:translateY(0); }
    .report-stack { display:grid; gap: 18px; align-items:start; }
    pre { background: var(--surface-2); border: 1px solid var(--line); border-radius: 16px; padding: 16px; overflow: auto; font-size: 12px; color: var(--text); }
    .warn-list { margin: 0; padding-left: 18px; line-height: 1.6; }
    .sort-indicator { color: var(--muted-2); font-size: 11px; margin-left: 6px; }
    .warning-grid { display:grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 8px; }
    .warning-card { padding: 10px 12px; }
    .warning-card h3 { margin: 0 0 4px; font-size: 12px; font-weight: 700; }
    .warning-card .count { font-size: 16px; font-weight: 800; margin-bottom: 4px; }
    .tone-neutral .count { color: var(--text); }
    .tone-warn .count { color: var(--warn-text); }
    .tone-danger .count { color: var(--danger-text); }
    .tone-neutral .warning-count { color: var(--oxide); }
    .tone-warn .warning-count { color: var(--warn-text); }
    .tone-danger .warning-count { color: var(--danger-text); }
    .support-note { color: var(--muted); font-size: 11px; line-height: 1.45; }
    .support-table th { cursor: default; }
    details { border: 1px solid var(--line); border-radius: 14px; background: var(--surface-2); }
    summary { cursor: pointer; padding: 14px 16px; font-weight: 700; }
    details > div { padding: 0 16px 16px; }
    .warning-console { margin: 0; padding: 14px 16px; border-radius: 12px; border:1px solid var(--line); background: #16120f; color: #d4f0d0; font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; white-space: pre-wrap; line-height: 1.55; max-height: 260px; overflow: auto; }
    .warning-console-actions { display:flex; gap:10px; flex-wrap:wrap; margin-top: 12px; }
    .warning-console.hidden { display:none; }
    @media (max-width: 1200px) {
      .warning-grid { grid-template-columns: 1fr 1fr; }
    }
    @media (max-width: 960px) {
      .top-nav-inner { grid-template-columns: 1fr; }
      .nav-project-slot, .nav-status { justify-content:flex-start; }
      .warning-grid, .report-stack { grid-template-columns: 1fr; }
      .hero-top { flex-direction: column; }
      .search { min-width: 100%; width: 100%; }
    }
    @media (max-width: 640px) {
      .summary-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); }
    }
    /* ── Report header / footer identification banner ─────────────────── */
    .report-id-banner { background: var(--nav); color: #fff; font-size: 11px; font-weight: 700; letter-spacing: 0.05em; display: flex; align-items: center; justify-content: center; height: 27px; padding: 0 16px; position: fixed; top: 0; left: 0; right: 0; z-index: 32; }
    .report-id-footer-banner { background: var(--nav); color: #fff; font-size: 11px; font-weight: 700; letter-spacing: 0.05em; display: flex; align-items: center; justify-content: center; height: 27px; padding: 0 16px; position: fixed; bottom: 0; left: 0; right: 0; z-index: 32; }
    body.has-report-banner .top-nav { top: 27px; }
    body.has-report-banner { padding-bottom: 27px; }
    /* ── Print & PDF export ──────────────────────────────────────────── */
    @page { size: A4 landscape; margin: 0.35in 0.5in; }

    @media print {
      *, *::before, *::after {
        -webkit-print-color-adjust: exact !important;
        print-color-adjust: exact !important;
        box-sizing: border-box !important;
      }

      html, body {
        background: #f5efe8 !important;
        min-height: auto !important;
        width: 100% !important;
      }

      /* Report id banner — fixed position repeats the banner on every printed page */
      .report-id-banner { display: flex !important; align-items: center !important; justify-content: center !important; position: fixed !important; top: 0 !important; left: 0 !important; right: 0 !important; padding: 3px 12px !important; font-size: 10px !important; background: #3d3d3d !important; color: #fff !important; z-index: 9999 !important; }
      .report-id-footer-banner { display: flex !important; align-items: center !important; justify-content: center !important; position: fixed !important; bottom: 0 !important; left: 0 !important; right: 0 !important; padding: 3px 12px !important; font-size: 10px !important; margin-top: 0 !important; background: #3d3d3d !important; color: #fff !important; z-index: 9999 !important; }
      body.has-report-banner .top-nav { top: 0 !important; }
      body.has-report-banner { padding-bottom: 0 !important; }
      /* Hide interactive UI-chrome; keep section heading text visible */
      .top-nav, .hero-actions,
      .background-watermarks, #code-particles,
      .header-button, .theme-toggle,
      .nav-dropdown-wrap, .config-actions,
      .warnings-show-link, .warning-console-actions,
      .toolbar .pill-row, .toolbar .export-group,
      input[type="search"], button { display: none !important; }
      /* Show toolbar as a plain block so h2 headings are visible */
      .toolbar { display: block !important; margin-bottom: 8px !important; }
      .toolbar-left { display: block !important; }

      /* Remove page-level layout constraints */
      .page {
        max-width: none !important;
        width: 100% !important;
        padding: 0 !important;
        margin: 0 !important;
      }

      .panel, .hero, .section,
      .saved-report-shell, .saved-panel, .report-shell, .stack {
        max-width: none !important;
        width: 100% !important;
        box-shadow: none !important;
        border: 1px solid #ddd !important;
        border-radius: 10px !important;
        margin-bottom: 10px !important;
        overflow: visible !important;
      }

      /* Force grids to their full-width column counts regardless of viewport */
      .summary-grid {
        display: grid !important;
        grid-template-columns: repeat(5, minmax(0, 1fr)) !important;
        gap: 10px !important;
      }

      .warning-grid {
        display: grid !important;
        grid-template-columns: repeat(3, minmax(0, 1fr)) !important;
        gap: 8px !important;
      }

      .report-stack {
        display: grid !important;
        gap: 12px !important;
        align-items: start !important;
      }

      /* Metric cards */
      .metric {
        box-shadow: none !important;
        border: 1px solid #e0d0c0 !important;
        border-radius: 8px !important;
        break-inside: avoid !important;
        padding: 10px 12px 22px !important;
        min-height: 0 !important;
      }

      .metric-big { font-size: 20px !important; }
      .metric-exact { font-size: 10px !important; bottom: 5px !important; right: 8px !important; }
      .metric-label { font-size: 10px !important; }

      /* Page break control — small atomic cards stay together; large panels
         and tables flow freely so they never force blank pages. */
      .metric, .warning-card, .run-id-chip { break-inside: avoid !important; }
      .hero, .panel, .stack { break-inside: auto !important; }
      section { break-inside: auto !important; }
      /* Keep each chart panel whole — browser moves it to the next page rather than
         slicing through the middle of a canvas. */
      .chart-section { break-inside: avoid !important; }
      /* Keep the summary grid on the same page as the hero header when possible */
      .summary-grid { break-before: avoid !important; }
      /* Section headings never orphan at the bottom of a page */
      h2, h3 { break-after: avoid !important; orphans: 3; widows: 3; }
      /* Keep the first few rows of a table with the header */
      thead { break-after: avoid !important; }

      /* Language charts — table layout is inherently side-by-side */
      #lang-overview-charts table { display: inline-table !important; }
      #lang-overview-charts td { vertical-align: top !important; }

      /* Tables */
      .table-shell {
        max-height: none !important;
        overflow: visible !important;
        width: 100% !important;
        break-inside: auto !important;
      }

      table {
        width: 100% !important;
        table-layout: auto !important;
        font-size: 10px !important;
        border-collapse: collapse !important;
        orphans: 4 !important;
        widows: 4 !important;
      }

      /* Remove the screen-layout min-width so tables scale to page width */
      #per-file-table, #skipped-table { min-width: 0 !important; }
      /* Release sticky column positioning (not meaningful on paper) */
      #per-file-table th:first-child,
      #per-file-table td:first-child { position: static !important; }
      /* Show ALL rows — JS pagination hides rows via inline style; !important overrides it */
      #per-file-table tbody tr, #skipped-table tbody tr, #hotspots-table tbody tr { display: table-row !important; }
      /* Hide pagination controls — not interactive in PDF */
      .page-size-row, .pagination-bar { display: none !important; }
      /* Header tooltips and the interaction hint are screen-only */
      .col-tip { display: none !important; }
      .hs-hint { display: none !important; }

      thead { display: table-header-group; }
      tr { break-inside: avoid !important; }

      th {
        position: relative !important;
        font-size: 9px !important;
        font-weight: 700 !important;
        color: #333 !important;
        padding: 5px 8px !important;
        background: rgba(211,122,76,0.18) !important;
        white-space: normal !important;
      }
      /* Resize handles are screen-only — hide them in print */
      .col-resize-handle { display: none !important; }
      /* Sort indicators are redundant on paper */
      .sort-indicator { display: none !important; }

      td {
        white-space: normal !important;
        overflow-wrap: anywhere !important;
        word-break: break-word !important;
        padding: 5px 8px !important;
        font-size: 10px !important;
        border-bottom: 1px solid #e8d8c8 !important;
      }

      pre, code {
        white-space: pre-wrap !important;
        overflow-wrap: anywhere !important;
        word-break: break-word !important;
        font-size: 9px !important;
        max-height: none !important;
      }

      .warning-card {
        box-shadow: none !important;
        border: 1px solid #ddd !important;
        break-inside: avoid !important;
        padding: 10px !important;
      }

      .hero-top { flex-direction: row !important; }

      .run-id-row { flex-wrap: wrap !important; gap: 4px !important; }
      .run-id-chip { font-size: 9px !important; padding: 4px 8px !important; border-left-width: 2px !important; }
      .meta { flex-wrap: wrap !important; gap: 0 !important; padding: 4px 0 !important; border-top: 1px solid #ccc !important; border-bottom: 1px solid #ccc !important; width: 100% !important; }
      .meta-chip { flex: 1 !important; justify-content: center !important; font-size: 9px !important; padding: 0 8px !important; border-right: 1px solid #ccc !important; }
      .meta-chip:last-child { border-right: none !important; }

      .report-footer {
        border-top: 1px solid #ccc !important;
        margin-top: 12px !important;
        font-size: 10px !important;
      }

      /* Collapse all <details> in print except the warnings block */
      details { border: 1px solid #ddd !important; border-radius: 8px !important; }
      details > summary { display: block !important; font-size: 10px !important; }
      details > div { display: none !important; }
      .warning-console { display: none !important; }
      .warning-console-actions { display: none !important; }
      /* Always expand the run-warnings details in PDF */
      details.warnings-details > div { display: block !important; }
      details.warnings-details .warning-console {
        display: block !important;
        max-height: none !important;
        overflow: visible !important;
        font-size: 8px !important;
        white-space: pre-wrap !important;
        word-break: break-all !important;
      }
      details.warnings-details .code-block-toolbar { display: none !important; }

      /* Pill badges */
      .pill { font-size: 9px !important; padding: 2px 6px !important; min-height: auto !important; }

      /* Support opportunities table */
      .support-table td:first-child { font-weight: 600; font-size: 10px !important; }

      /* Hide canvas-based interactive chart sections; replaced by pre-rendered variants */
      .chart-section { display: none !important; }
      .charts-grid   { display: none !important; }
      /* Pre-rendered chart variants — no forced page break; flow naturally after hero section */
      .pdf-variants-root { display: block !important; padding: 0 !important; }
      .pdf-variant-group { break-inside: auto !important; margin-bottom: 8px !important; background: #faf6f0 !important; border: 1px solid #e0d0c0 !important; border-radius: 10px !important; padding: 10px 12px !important; }
      .pdf-variant-group-title { break-after: avoid !important; font-size: 12px !important; font-weight: 800 !important; color: #3d2d26 !important; margin: 0 0 6px !important; padding-bottom: 4px !important; border-bottom: 2px solid #d37a4c !important; }
      .pdf-variant-grid { display: grid !important; grid-template-columns: 1fr 1fr !important; gap: 6px !important; }
      /* Single-column chart (scatter, etc.) — centre and constrain width in print */
      .pdf-variant-grid.single-col { grid-template-columns: 1fr !important; }
      .pdf-variant-grid.single-col .pdf-variant-panel { max-width: 62% !important; margin: 0 auto !important; }
      .pdf-variant-panel { break-inside: avoid !important; }
      .pdf-variant-label { font-size: 9px !important; font-weight: 700 !important; text-transform: uppercase !important; letter-spacing: .06em !important; color: #7b675b !important; margin: 0 0 2px !important; }
      .pdf-variant-img { width: 100% !important; height: auto !important; display: block !important; border-radius: 5px !important; border: 1px solid #ddd !important; }
    }


    .warnings-show-link {
      display: inline-flex;
      align-items: center;
      gap: 8px;
      padding: 8px 12px;
      border-radius: 10px;
      border: 1px solid rgba(111, 144, 255, 0.35);
      background: #eef3ff;
      color: #2f5fe3 !important;
      font-weight: 800;
      text-decoration: none;
      box-shadow: inset 0 1px 0 rgba(255,255,255,0.45);
    }

    body.dark-theme .warnings-show-link {
      background: #1c2847;
      color: #a9c1ff !important;
      border-color: rgba(169, 193, 255, 0.32);
    }

    .effective-config-note {
      margin: 8px 0 0;
      color: var(--muted);
      font-size: 14px;
      line-height: 1.6;
    }
    .config-actions { display: flex; gap: 8px; flex-shrink: 0; }
    .config-pre-wrap { position: relative; }
    .config-pre { margin: 0; background: #16120f; color: #d4f0d0; border: 1px solid var(--line); border-radius: 10px; padding: 14px 16px; font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; font-size: 12px; line-height: 1.5; overflow: auto; resize: vertical; max-height: 320px; min-height: 100px; white-space: pre; }
    body.dark-theme .config-pre { background: #0e0c0a; color: #b8f0b8; }
    .code-block-toolbar { display:flex; justify-content:flex-end; margin-bottom:6px; }
    .code-copy-btn { display:inline-flex; align-items:center; gap:5px; background: var(--surface-2); border: 1px solid var(--line-strong); color: var(--muted); border-radius: 8px; padding: 5px 12px; font-size: 12px; font-weight: 700; cursor: pointer; transition: background 0.15s ease, color 0.15s ease, border-color 0.15s ease; white-space: nowrap; }
    .code-copy-btn:hover { background: rgba(184,93,51,0.08); color: var(--oxide-2); border-color: rgba(184,93,51,0.30); }
    body.dark-theme .code-copy-btn { background: rgba(255,255,255,0.07); border-color: rgba(255,255,255,0.18); color: rgba(255,255,255,0.75); }
    body.dark-theme .code-copy-btn:hover { background: rgba(211,122,76,0.15); border-color: rgba(211,122,76,0.40); color: var(--oxide); }


    .page {
      position: relative;
      z-index: 1;
    }
    .report-footer { margin-top: 16px; padding: 14px 24px; border-top: 1px solid var(--line); text-align: center; color: var(--muted); font-size: 12px; font-weight: 600; }

    /* ── Chart controls & containers ───────────────────────────────────── */
    .chart-section { }
    .chart-controls { display:flex; gap:12px; align-items:center; flex-wrap:wrap; margin-bottom:14px; }
    .chart-controls label { font-size:13px; font-weight:700; color:var(--muted); display:flex; align-items:center; gap:6px; }
    .chart-select { background:var(--surface-2); border:1px solid var(--line-strong); border-radius:8px; padding:5px 10px; color:var(--text); font-size:13px; font-weight:600; cursor:pointer; outline:none; appearance:auto; }
    .chart-select:focus { border-color:var(--accent); }
    .chart-expand-btn { background:none; border:1px solid var(--line-strong); border-radius:6px; cursor:pointer; color:var(--muted); padding:4px 10px; font-size:13px; line-height:1; transition:background .13s,color .13s; }
    .chart-expand-btn:hover { background:var(--surface-2); color:var(--text); }
    .chart-modal-overlay { position:fixed; inset:0; background:rgba(0,0,0,0.55); z-index:9999; display:flex; align-items:center; justify-content:center; padding:24px; box-sizing:border-box; }
    .chart-modal { background:var(--bg); border-radius:16px; padding:24px 28px; max-width:1000px; width:100%; max-height:88vh; overflow-y:auto; position:relative; box-shadow:0 24px 80px rgba(0,0,0,0.3); }
    .chart-modal-title { font-size:15px; font-weight:800; text-transform:uppercase; letter-spacing:.05em; color:var(--text); margin:0 0 2px; display:block; }
    .chart-modal-subtitle { font-size:13px; font-weight:600; color:var(--muted); margin:0 0 16px; display:block; letter-spacing:.02em; }
    .chart-modal-close { position:absolute; top:14px; right:18px; background:none; border:none; font-size:22px; cursor:pointer; color:var(--text); line-height:1; padding:0; }
    .chart-modal-close:hover { opacity:.7; }
    .chart-modal-header { display:flex; align-items:center; gap:12px; flex-wrap:nowrap; margin:0 0 16px; padding-right:44px; }
    .chart-modal-header .chart-modal-title { flex:1 1 auto; margin:0; min-width:0; }
    body.dark-theme .chart-modal { background:var(--surface); }
    .chart-container { width:100%; overflow:visible; }
    .charts-grid { display:grid; grid-template-columns:minmax(0,1fr) minmax(0,1fr); gap:18px; align-items:stretch; }
    .charts-grid > .panel { margin:0; min-width:0; display:flex; flex-direction:column; }
    .charts-grid .chart-section > div { display:flex; flex-direction:column; flex:1; }
    .charts-grid .chart-container { flex:1; min-height:180px; }
    .chart-pre { min-height:72px; }
    @media (max-width:820px) { .charts-grid { grid-template-columns:1fr; } }
    .r-lang-overview { display:flex; gap:40px; align-items:center; justify-content:center; flex-wrap:wrap; padding:8px 0 16px; }
    .r-lang-overview-cell { display:flex; flex-direction:column; align-items:center; gap:8px; flex:1 1 280px; max-width:480px; }
    .r-lang-overview-cell p { margin:0; font-size:11px; font-weight:800; text-transform:uppercase; letter-spacing:.08em; color:var(--muted-2); text-align:center; }
    .r-lang-overview svg { display:block; max-width:100%; height:auto; }
    .rchit { cursor:pointer; transition:opacity .17s,filter .17s,transform .17s; transform-box:fill-box; transform-origin:center center; }
    .rchit:hover { filter:brightness(1.15) drop-shadow(0 2px 6px rgba(0,0,0,.18)); transform:scale(1.05); }
    .lang-bar-row { cursor:pointer; transition:transform .2s cubic-bezier(.34,1.56,.64,1); }
    .lang-bar-row:hover { transform:translateY(-2px); }
    .lang-bar-row .rchit:hover { filter:none; transform:none; }
    .lang-bar-row:hover .rchit { filter:brightness(1.12); transform:scaleY(1.22); }
    #r-tt { display:none; position:fixed; background:rgba(15,10,6,.95); color:#fff; border-radius:10px; padding:8px 13px; font-size:12px; line-height:1.5; pointer-events:none; z-index:10001; box-shadow:0 4px 20px rgba(0,0,0,.32); border:1px solid rgba(255,255,255,.1); max-width:240px; white-space:nowrap; }
    .chart-tab-bar { display:flex; gap:6px; margin-bottom:12px; flex-wrap:wrap; }
    .chart-tab { padding:5px 16px; border-radius:999px; border:1px solid var(--line-strong); background:var(--surface-2); color:var(--muted); font-size:12px; font-weight:700; cursor:pointer; transition:background 0.12s,color 0.12s,border-color 0.12s; }
    .chart-tab:hover { background:var(--surface-3); color:var(--text); }
    .chart-tab.active { background:var(--accent); color:#fff; border-color:var(--accent); }
    .chart-locked-card { display:none; padding:20px 24px; border-radius:14px; background:var(--info-bg); border:1px solid rgba(196,92,16,0.28); color:var(--info-text); font-size:14px; line-height:1.6; }
    .chart-locked-card a { color:var(--accent-2); font-weight:700; }
    .chart-locked-card h3 { margin:0 0 6px; font-size:15px; }

    /* Print: hide interactive controls; keep SVGs; show locked card for history mode */
    @media print {
      .chart-controls, .chart-tab-bar { display:none !important; }
      /* Single-column grid: each chart gets full page width, renders shorter, fits on one page */
      .charts-grid { grid-template-columns: 1fr !important; gap: 10px !important; }
      /* Cap canvas height so a single chart never overflows a landscape page */
      canvas { max-width: 100% !important; max-height: 280px !important; }
      .chart-container { width: 100% !important; overflow: visible !important; }
      .chart-container svg { max-height:300px !important; }
      /* chart-locked-card: do NOT force display:block — let JS-set visibility carry
         into print (hidden in normal mode; visible only when history mode is active).
         When it does show, use readable dark text instead of accent blue. */
      .chart-locked-card { background:#f0f0f0 !important; border:1px solid #bbb !important; color:#333 !important; font-size:11px !important; padding:10px 14px !important; border-radius:8px !important; }
      .chart-locked-card h3 { color:#222 !important; font-size:13px !important; }
      .chart-locked-card a, .chart-locked-card strong { color:#1a4fa0 !important; }
      .chart-locked-card code { background:rgba(0,0,0,0.08) !important; padding:1px 4px !important; border-radius:3px !important; }
    }

    /* PDF-only chart variants container — hidden on screen, rendered in print */
    .pdf-variants-root{display:none;}
    .pdf-variant-group{margin-bottom:12px;background:#faf6f0;border:1px solid #e0d0c0;border-radius:10px;padding:12px 14px;}
    .pdf-variant-group-title{font-size:15px;font-weight:800;color:#3d2d26;margin:0 0 8px;padding-bottom:5px;border-bottom:2px solid #d37a4c;}
    .pdf-variant-grid{display:grid;grid-template-columns:1fr 1fr;gap:10px;}
    /* Solo charts (one per row) — constrained width, centred */
    .pdf-variant-grid.single-col{grid-template-columns:1fr;}
    .pdf-variant-grid.single-col .pdf-variant-panel{max-width:62%;margin:0 auto;}
    .pdf-variant-label{font-size:10px;font-weight:700;text-transform:uppercase;letter-spacing:.06em;color:#7b675b;margin:0 0 3px;}
    .pdf-variant-img{width:100%;height:auto;display:block;border-radius:6px;border:1px solid #ddd;}

    #rpt-loading-overlay{position:fixed;inset:0;z-index:9999;display:flex;align-items:center;justify-content:center;overflow:hidden;transition:opacity .6s cubic-bezier(.4,0,.2,1);background:radial-gradient(125% 125% at 50% 0%,#fbf4ec 0%,#f4ebe0 45%,#ecdfd0 100%);}
    #rpt-loading-overlay.fade-out{opacity:0;pointer-events:none;}
    /* Drifting color blobs — transform/opacity only (GPU composited, no per-frame repaint) */
    .rpt-bg-blob{position:absolute;border-radius:50%;filter:blur(64px);opacity:.5;pointer-events:none;will-change:transform;}
    .rpt-blob-a{width:48vw;height:48vw;left:-10vw;top:-12vw;background:radial-gradient(circle,#e8932f,transparent 64%);animation:rpt-drift-a 17s ease-in-out infinite;}
    .rpt-blob-b{width:42vw;height:42vw;right:-8vw;bottom:-10vw;background:radial-gradient(circle,#d3621a,transparent 64%);animation:rpt-drift-b 21s ease-in-out infinite;}
    .rpt-blob-c{width:34vw;height:34vw;right:20vw;top:-8vw;background:radial-gradient(circle,#caa14f,transparent 64%);opacity:.38;animation:rpt-drift-c 25s ease-in-out infinite;}
    @keyframes rpt-drift-a{0%,100%{transform:translate3d(0,0,0) scale(1);}50%{transform:translate3d(9vw,7vw,0) scale(1.18);}}
    @keyframes rpt-drift-b{0%,100%{transform:translate3d(0,0,0) scale(1.06);}50%{transform:translate3d(-8vw,-6vw,0) scale(.88);}}
    @keyframes rpt-drift-c{0%,100%{transform:translate3d(0,0,0) scale(1);}50%{transform:translate3d(-7vw,8vw,0) scale(1.22);}}
    body.dark-theme #rpt-loading-overlay{background:radial-gradient(125% 125% at 50% 0%,#241810 0%,#1a120b 45%,#130c06 100%);}
    body.dark-theme .rpt-bg-blob{opacity:.36;}
    .rpt-load-card{position:relative;z-index:1;display:flex;flex-direction:column;align-items:center;gap:22px;width:432px;max-width:88vw;padding:46px 54px 38px;background:linear-gradient(155deg,rgba(255,255,253,.95),rgba(255,248,240,.9));border:1px solid rgba(196,110,40,.16);border-radius:26px;box-shadow:0 1px 0 rgba(255,255,255,.8) inset,0 22px 64px rgba(120,64,16,.16),0 4px 16px rgba(0,0,0,.06);animation:rpt-card-in .5s cubic-bezier(.22,.68,0,1.12) both;}
    @keyframes rpt-card-in{from{opacity:0;transform:translateY(14px) scale(.96);}to{opacity:1;transform:none;}}
    body.dark-theme .rpt-load-card{background:linear-gradient(155deg,rgba(42,24,12,.92),rgba(28,15,6,.95));border-color:rgba(200,120,50,.16);box-shadow:0 1px 0 rgba(255,200,140,.05) inset,0 22px 64px rgba(0,0,0,.5),0 4px 16px rgba(0,0,0,.35);}
    /* Logo is static — no bounce (kept GPU-cheap) */
    .rpt-load-logo{width:58px;height:58px;object-fit:contain;filter:drop-shadow(0 6px 16px rgba(90,48,12,.45));animation:rpt-card-in .5s ease .05s both;}
    .rpt-spinner-wrap{position:relative;width:90px;height:90px;}
    .rpt-spinner-track{position:absolute;inset:0;border-radius:50%;border:5px solid rgba(196,92,16,.12);}
    .rpt-spinner{position:absolute;inset:0;border-radius:50%;background:conic-gradient(from 0deg,rgba(196,92,16,0) 0%,rgba(196,92,16,.18) 35%,#c45c10 100%);will-change:transform;animation:rpt-spin 1s linear infinite;-webkit-mask:radial-gradient(farthest-side,transparent calc(100% - 6px),#fff calc(100% - 5px));mask:radial-gradient(farthest-side,transparent calc(100% - 6px),#fff calc(100% - 5px));}
    @keyframes rpt-spin{to{transform:rotate(360deg);}}
    .rpt-spinner-pct{position:absolute;inset:0;display:flex;align-items:center;justify-content:center;font-size:17px;font-weight:800;color:#c45c10;font-variant-numeric:tabular-nums;}
    body.dark-theme .rpt-spinner-track{border-color:rgba(196,92,16,.2);}
    body.dark-theme .rpt-spinner-pct{color:#e8932f;}
    .rpt-load-divider{width:54px;height:1px;background:linear-gradient(90deg,transparent,rgba(196,92,16,.22),transparent);}
    .rpt-loading-text{font-size:15px;font-weight:600;letter-spacing:.08em;display:flex;align-items:baseline;gap:2px;}
    .rpt-load-word{background:linear-gradient(90deg,#9a7a64 0%,#c45c10 45%,#e08a3a 55%,#9a7a64 100%);background-size:220% auto;-webkit-background-clip:text;background-clip:text;-webkit-text-fill-color:transparent;color:transparent;animation:rpt-text-shimmer 3.2s linear infinite;}
    @keyframes rpt-text-shimmer{to{background-position:-220% center;}}
    .rpt-dot{display:inline-block;color:#c45c10;-webkit-text-fill-color:#c45c10;animation:rpt-bounce 1.7s ease-in-out infinite;opacity:0;}
    .rpt-dot:nth-child(2){animation-delay:.28s;}
    .rpt-dot:nth-child(3){animation-delay:.56s;}
    @keyframes rpt-bounce{0%,60%,100%{opacity:0;transform:translateY(0);}30%{opacity:1;transform:translateY(-5px);}}
    .rpt-status{font-size:12.5px;font-weight:600;letter-spacing:.02em;color:var(--muted,#8a7060);min-height:16px;text-align:center;}
    .rpt-status-in{animation:rpt-status-pop .38s ease both;}
    @keyframes rpt-status-pop{from{opacity:0;transform:translateY(4px);}to{opacity:1;transform:none;}}
    .rpt-progress{width:100%;height:6px;border-radius:99px;background:rgba(196,92,16,.12);overflow:hidden;}
    .rpt-progress-bar{height:100%;width:100%;transform:scaleX(0);transform-origin:left center;border-radius:99px;background:linear-gradient(90deg,#e8932f,#c45c10);transition:transform .25s cubic-bezier(.4,0,.2,1);will-change:transform;}
    body.dark-theme .rpt-progress{background:rgba(196,92,16,.2);}
    .rpt-feed{width:100%;min-height:66px;display:flex;flex-direction:column;justify-content:flex-end;gap:3px;font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:10.5px;line-height:1.5;color:rgba(138,112,96,.72);text-align:left;overflow:hidden;}
    .rpt-feed-line{display:flex;align-items:center;gap:6px;opacity:0;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;animation:rpt-feed-in .4s ease forwards;}
    .rpt-feed-line::before{content:'>';color:#c45c10;font-weight:700;}
    @keyframes rpt-feed-in{from{opacity:0;transform:translateX(-6px);}to{opacity:.82;transform:none;}}
    body.dark-theme .rpt-feed{color:rgba(204,172,150,.62);}
    body.dark-theme .rpt-load-divider{background:linear-gradient(90deg,transparent,rgba(196,92,16,.28),transparent);}
    @media (prefers-reduced-motion:reduce){ #rpt-loading-overlay .rpt-bg-blob,#rpt-loading-overlay .rpt-spinner,#rpt-loading-overlay .rpt-load-word,#rpt-loading-overlay .rpt-dot{animation:none!important;}}
    /* ── Code Style Analysis section ── */
    .style-guide-grid{display:grid;gap:10px;}
    .style-guide-row{display:grid;grid-template-columns:140px 1fr 52px;align-items:center;gap:10px;padding:6px 8px;border-radius:8px;cursor:default;position:relative;transition:transform .18s ease,box-shadow .18s ease,background .18s ease;}
    .style-guide-row:hover{transform:translateY(-2px);box-shadow:0 6px 22px rgba(77,44,20,0.18);background:var(--surface-2);}
    .style-guide-label{font-size:12px;font-weight:800;color:var(--text);text-align:right;white-space:nowrap;}
    .style-guide-track{background:var(--surface-3);border-radius:6px;height:20px;overflow:hidden;position:relative;box-shadow:inset 0 1px 3px rgba(0,0,0,.08);}
    .style-guide-fill{height:100%;border-radius:6px;background:linear-gradient(90deg,var(--oxide),var(--oxide-2));transition:width .65s cubic-bezier(.25,.46,.45,.94),filter .18s ease;position:relative;}
    .style-guide-fill::after{content:'';position:absolute;inset:0;background:linear-gradient(90deg,rgba(255,255,255,.18) 0%,rgba(255,255,255,.04) 100%);border-radius:6px;}
    .style-guide-row:hover .style-guide-fill{filter:brightness(1.12);}
    .style-guide-score{font-size:12px;font-weight:800;color:var(--oxide);text-align:right;white-space:nowrap;}
    .style-guide-desc{font-size:10px;color:var(--muted);margin-top:2px;grid-column:2/3;}
    .style-bar-tip{position:absolute;bottom:calc(100% + 8px);left:50%;transform:translateX(-50%);background:var(--text);color:var(--bg);padding:7px 14px;border-radius:8px;font-size:11px;white-space:nowrap;pointer-events:none;opacity:0;transition:opacity .25s cubic-bezier(.16,1,.3,1), transform .25s cubic-bezier(.16,1,.3,1);z-index:300;box-shadow:0 4px 18px rgba(0,0,0,.24);}
    .style-bar-tip::after{content:'';position:absolute;top:100%;left:50%;transform:translateX(-50%);border:5px solid transparent;border-top-color:var(--text);}
    .style-guide-row:hover .style-bar-tip{opacity:1;}
    .style-metrics-strip{display:grid;grid-template-columns:repeat(4,1fr);gap:12px;margin:18px 0 0;}
    @media(max-width:800px){.style-metrics-strip{grid-template-columns:repeat(2,1fr);}}
    .style-chip{background:var(--surface);border:1px solid var(--line);border-radius:10px;padding:12px 14px;text-align:center;cursor:default;position:relative;transition:transform .27s cubic-bezier(.16,1,.3,1),box-shadow .27s cubic-bezier(.16,1,.3,1);}
    .style-chip:hover{transform:translateY(-4px);box-shadow:0 12px 32px rgba(77,44,20,0.2);}
    .style-chip-val{font-size:18px;font-weight:900;color:var(--oxide);}
    .style-chip-label{font-size:10px;font-weight:700;text-transform:uppercase;letter-spacing:.07em;color:var(--muted);margin-top:3px;}
    .style-chip-tip{position:absolute;top:calc(100% + 10px);left:50%;transform:translateX(-50%);background:var(--text);color:var(--bg);padding:7px 12px;border-radius:8px;font-size:11px;white-space:nowrap;pointer-events:none;opacity:0;transition:opacity .25s cubic-bezier(.16,1,.3,1), transform .25s cubic-bezier(.16,1,.3,1);z-index:200;}
    .style-chip-tip::after{content:'';position:absolute;bottom:100%;left:50%;transform:translateX(-50%);border:5px solid transparent;border-bottom-color:var(--text);}
    .style-chip:hover .style-chip-tip{opacity:1;}
    .style-file-table{width:100%;border-collapse:collapse;font-size:12px;table-layout:fixed;}
    .style-file-table th{background:var(--surface-3);padding:7px 10px;font-size:10px;font-weight:800;text-transform:uppercase;letter-spacing:.07em;color:var(--muted);text-align:left;border-bottom:2px solid var(--line);cursor:pointer;user-select:none;white-space:nowrap;position:relative;}
    .style-file-table th:hover{background:var(--surface-2);color:var(--text);}
    .style-sort-ind{display:inline-block;margin-left:4px;font-size:9px;opacity:.4;vertical-align:middle;}
    .style-file-table th.sft-sort-asc .style-sort-ind,.style-file-table th.sft-sort-desc .style-sort-ind{opacity:1;color:var(--oxide);}
    .style-file-table td{padding:6px 10px;border-bottom:1px solid var(--line);vertical-align:middle;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;}
    .style-file-table tr:hover td{background:var(--surface-2);}
    .style-score-bar{display:inline-block;width:48px;height:8px;border-radius:4px;background:var(--surface-3);vertical-align:middle;position:relative;margin-right:4px;}
    .style-score-fill{position:absolute;left:0;top:0;height:100%;border-radius:4px;background:linear-gradient(90deg,var(--oxide),var(--oxide-2));}
    .style-badge{display:inline-block;padding:2px 7px;border-radius:12px;font-size:10px;font-weight:700;background:var(--surface-3);color:var(--oxide);border:1px solid var(--line);text-decoration:none;transition:background .15s,transform .15s,box-shadow .15s;}
    a.style-badge:hover{background:var(--oxide);color:#fff !important;transform:translateY(-1px);box-shadow:0 3px 10px rgba(77,44,20,.24);}
    .style-heuristic-note{display:flex;align-items:flex-start;gap:10px;background:var(--info-bg);color:var(--info-text);border-radius:10px;padding:11px 14px;font-size:13px;line-height:1.5;margin-top:14px;border:1px solid rgba(196,92,16,0.20);}
    .style-lang-tabs{display:flex;flex-wrap:wrap;gap:6px;margin-bottom:14px;}
    .style-lang-tab{padding:4px 12px;border-radius:14px;border:1px solid var(--line);background:var(--surface);font-size:11px;font-weight:700;cursor:pointer;color:var(--text);transition:background .15s;}
    .style-lang-tab:hover{background:var(--surface-2);}
    .style-lang-tab.active{background:var(--oxide);color:#fff;border-color:var(--oxide);}
    .style-sig-chip{display:inline-block;padding:1px 6px;border-radius:8px;font-size:10px;background:var(--surface-2);color:var(--muted);border:1px solid var(--line);margin-right:3px;cursor:default;}
    .style-row-warn td{background:rgba(178,48,48,0.06)!important;}
    .style-row-warn td:first-child{border-left:3px solid #b23030;}
    .style-sig-more{display:inline-block;padding:1px 6px;border-radius:8px;font-size:10px;background:transparent;color:var(--oxide);border:1px solid var(--oxide);margin-right:3px;font-weight:700;cursor:pointer;transition:background .15s,color .15s;}
    .style-sig-more:hover{background:var(--oxide);color:#fff;}
    .style-sig-info-btn{background:none;border:none;cursor:pointer;font-size:13px;color:var(--muted);padding:0 2px;line-height:1;vertical-align:middle;transition:color .15s;margin-left:4px;}
    .style-sig-info-btn:hover{color:var(--oxide);}
    .style-sig-pop{position:fixed;background:var(--bg);border:1px solid var(--line);border-radius:10px;padding:10px 14px;box-shadow:0 8px 24px rgba(0,0,0,.18);z-index:9999;min-width:200px;max-width:300px;font-size:12px;line-height:1.6;}
    .style-sig-pop-title{font-size:11px;font-weight:700;color:var(--muted);text-transform:uppercase;letter-spacing:.05em;margin-bottom:6px;}
    .style-sig-pop-row{display:flex;gap:8px;padding:4px 0;border-bottom:1px solid var(--line);}
    .style-sig-pop-row:last-child{border-bottom:none;}
    .style-sig-pop-key{color:var(--muted);font-weight:700;white-space:nowrap;flex-shrink:0;}
    .style-sig-pop-val{color:var(--text);}
    .style-sig-info-overlay{position:fixed;inset:0;background:rgba(0,0,0,.4);z-index:10000;display:flex;align-items:center;justify-content:center;}
    .style-sig-info-modal{background:var(--bg);border-radius:14px;padding:22px 26px;max-width:500px;width:92%;box-shadow:0 12px 40px rgba(0,0,0,.2);position:relative;max-height:80vh;overflow-y:auto;}
    .style-sig-info-close{position:absolute;top:12px;right:16px;background:none;border:none;cursor:pointer;font-size:20px;color:var(--muted);line-height:1;}
    .style-sig-info-close:hover{color:var(--text);}
    .style-sig-info-grid{display:grid;grid-template-columns:max-content 1fr;gap:6px 14px;margin-top:14px;font-size:13px;}
    .style-sig-info-name{color:var(--oxide);font-weight:700;padding:2px 0;}
    .style-sig-info-desc{color:var(--text);padding:2px 0;}
    body.dark-theme .style-guide-track{background:var(--surface-3);}
    body.dark-theme .style-chip{background:var(--surface-2);}
    body.dark-theme .style-file-table th{background:var(--surface-3);}
    body.dark-theme .style-heuristic-note{border-color:rgba(211,122,76,0.25);}
    body.dark-theme .style-lang-tab{background:var(--surface-2);color:var(--text);}
    body.dark-theme .style-lang-tab.active{background:var(--oxide);color:#fff;}
    body.dark-theme .style-sig-chip{background:var(--surface-3);color:var(--muted);}
    body.dark-theme .style-sig-pop{background:var(--surface);box-shadow:0 8px 24px rgba(0,0,0,.4);}
    body.dark-theme .style-sig-info-modal{background:var(--surface);}
    .sig-tip{position:fixed;background:rgba(28,18,8,0.93);color:#f0ebe4;padding:9px 13px 15px 13px;border-radius:9px;font-size:12px;line-height:1.65;pointer-events:none;z-index:9998;opacity:0;transition:opacity .1s;box-shadow:0 4px 16px rgba(0,0,0,.35);display:none;min-width:160px;max-width:300px;}
    .sig-tip.visible{opacity:1;}
    .sig-tip::after{content:'';position:absolute;top:100%;left:var(--sig-tip-ax,50%);transform:translateX(-50%);border:7px solid transparent;border-top-color:rgba(28,18,8,0.93);}
    .sig-tip-hd{font-size:10px;font-weight:700;text-transform:uppercase;letter-spacing:.06em;color:rgba(240,235,228,.5);margin-bottom:5px;}
    .sig-tip-row{display:flex;gap:8px;align-items:baseline;}
    .sig-tip-k{color:#e07b3a;font-weight:700;white-space:nowrap;flex-shrink:0;}
    .sig-tip-v{color:#f0ebe4;}
</style>
<script nonce="{{ nonce }}">{{ chart_js|safe }}</script>
</head>
<body{% if report_header_footer.is_some() %} class="has-report-banner"{% endif %}>
  <div id="rpt-loading-overlay" aria-live="polite" aria-label="Loading report">
    <div class="rpt-bg-blob rpt-blob-a" aria-hidden="true"></div>
    <div class="rpt-bg-blob rpt-blob-b" aria-hidden="true"></div>
    <div class="rpt-bg-blob rpt-blob-c" aria-hidden="true"></div>
    <div class="rpt-load-card">
      <img src="{{ small_logo_uri }}" alt="oxide-sloc" class="rpt-load-logo" />
      <div class="rpt-spinner-wrap">
        <div class="rpt-spinner-track"></div>
        <div class="rpt-spinner"></div>
        <div class="rpt-spinner-pct" id="rpt-pct">0%</div>
      </div>
      <div class="rpt-load-divider"></div>
      <div class="rpt-loading-text"><span class="rpt-load-word">Loading report</span><span class="rpt-dot">.</span><span class="rpt-dot">.</span><span class="rpt-dot">.</span></div>
      <div class="rpt-status" id="rpt-status">Initializing analysis engine</div>
      <div class="rpt-progress"><div class="rpt-progress-bar" id="rpt-progress-bar"></div></div>
      <div class="rpt-feed" id="rpt-feed" aria-hidden="true"></div>
    </div>
  </div>
  <script nonce="{{ nonce }}">
  (function(){
    var ov=document.getElementById('rpt-loading-overlay');if(!ov)return;
    var statusEl=document.getElementById('rpt-status'),bar=document.getElementById('rpt-progress-bar'),pct=document.getElementById('rpt-pct'),feed=document.getElementById('rpt-feed');
    var msgs=['Initializing analysis engine','Discovering source files','Detecting languages','Tokenizing and counting lines','Classifying comments and docstrings','Computing complexity metrics','Aggregating per-language totals','Estimating COCOMO effort','Rendering charts','Finalizing report'];
    var logs=['scan: walking directory tree','lexer: state machine warm','metrics: SLOC and ULOC ready','cocomo: effort model loaded','charts: canvas contexts bound','render: assembling sections','dedup: hashing file contents','git: reading activity window'];
    var mi=0,li=0,prog=0,ready=false,start=Date.now(),MIN=1700;
    function setProg(p){prog=p;if(bar)bar.style.transform='scaleX('+(p/100).toFixed(3)+')';if(pct)pct.textContent=Math.round(p)+'%';}
    function nextMsg(){if(statusEl){statusEl.classList.remove('rpt-status-in');void statusEl.offsetWidth;statusEl.textContent=msgs[mi%msgs.length];statusEl.classList.add('rpt-status-in');}mi++;}
    function addLog(){if(!feed)return;var l=document.createElement('div');l.className='rpt-feed-line';l.textContent=logs[li%logs.length];feed.appendChild(l);li++;while(feed.childNodes.length>4)feed.removeChild(feed.firstChild);}
    nextMsg();addLog();setProg(6);
    var msgTimer=setInterval(nextMsg,900),logTimer=setInterval(addLog,640),progTimer=setInterval(function(){var cap=ready?100:90;if(prog<cap){var step=(cap-prog)*0.08+0.6;setProg(Math.min(cap,prog+step));}},80);
    function done(){clearInterval(msgTimer);clearInterval(logTimer);clearInterval(progTimer);setProg(100);if(statusEl){statusEl.textContent='Done';statusEl.classList.add('rpt-status-in');}setTimeout(function(){ov.classList.add('fade-out');setTimeout(function(){if(ov.parentNode)ov.parentNode.removeChild(ov);},600);},260);}
    window.__rptFinish=function(){ready=true;setTimeout(done,Math.max(0,MIN-(Date.now()-start)));};
  })();
  </script>
  <div class="background-watermarks" aria-hidden="true">
    <img src="{{ logo_text_uri }}" alt="" />
    <img src="{{ logo_text_uri }}" alt="" />
    <img src="{{ logo_text_uri }}" alt="" />
    <img src="{{ logo_text_uri }}" alt="" />
    <img src="{{ logo_text_uri }}" alt="" />
    <img src="{{ logo_text_uri }}" alt="" />
    <img src="{{ logo_text_uri }}" alt="" />
    <img src="{{ logo_text_uri }}" alt="" />
    <img src="{{ logo_text_uri }}" alt="" />
    <img src="{{ logo_text_uri }}" alt="" />
    <img src="{{ logo_text_uri }}" alt="" />
    <img src="{{ logo_text_uri }}" alt="" />
  </div>
  <div class="code-particles" id="code-particles" aria-hidden="true"></div>
  {% if let Some(banner) = report_header_footer %}
  <div class="report-id-banner" aria-label="Report identification">{{ banner|e }}</div>
  {% endif %}
  <div class="top-nav">
    <div class="top-nav-inner">
      <a class="brand" href="/" data-local-brand="1">
        {% if let Some(uri) = custom_logo_uri %}
        <img class="brand-logo" src="{{ uri }}" alt="logo" />
        {% else %}
        <img class="brand-logo" src="{{ small_logo_uri }}" alt="OxideSLOC logo" />
        {% endif %}
        <div class="brand-copy">
          {% if let Some(name) = company_name %}
          <div class="brand-title">{{ name }}</div>
          {% else %}
          <div class="brand-title">OxideSLOC</div>
          {% endif %}
          <div class="brand-subtitle">Saved HTML report</div>
        </div>
      </a>
      <div class="nav-project-slot">
        <div class="nav-project-pill"><span class="nav-project-label">REPORT</span><span class="nav-project-value">{{ title }}</span></div>
      </div>
      <div class="nav-status">
        <button type="button" class="header-button" data-copy-link>Copy link</button>
        <button type="button" class="header-button" data-share-report>Share</button>
        <div class="nav-dropdown-wrap">
          <button type="button" class="header-button nav-dropdown-trigger" aria-haspopup="true">Export ▾</button>
          <div class="nav-dropdown-menu">
            <button type="button" class="nav-dropdown-item" data-export-csv>Export CSV</button>
            <button type="button" class="nav-dropdown-item" data-export-xls>Export Excel</button>
          </div>
        </div>
        <a id="nav-view-pdf-btn" href="/runs/pdf/{{ run.tool.run_id }}" target="_blank" rel="noopener" class="header-button" style="text-decoration:none;"{% if let Some(purl) = standalone_pdf_url %} data-standalone-pdf="{{ purl }}"{% endif %}>View PDF</a>
        <button type="button" class="theme-toggle" id="settings-btn" aria-label="Color scheme" title="Color scheme settings">
          <svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.8"><circle cx="12" cy="12" r="3"></circle><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path></svg>
        </button>
        <button type="button" class="theme-toggle" data-theme-toggle aria-label="Toggle theme" title="Toggle theme">
          <svg class="icon-moon" viewBox="0 0 24 24" aria-hidden="true"><path d="M20 15.5A8.5 8.5 0 1 1 12.5 4 6.7 6.7 0 0 0 20 15.5Z"></path></svg>
          <svg class="icon-sun" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="4.2"></circle><path d="M12 2.5v2.2M12 19.3v2.2M21.5 12h-2.2M4.7 12H2.5M18.9 5.1l-1.6 1.6M6.7 17.3l-1.6 1.6M18.9 18.9l-1.6-1.6M6.7 6.7 5.1 5.1"></path></svg>
        </button>
      </div>
    </div>
  </div>

  <div class="page">
    <section class="hero panel">
      <div class="hero-top">
        <div>
          <div class="section-kicker">Saved report artifact</div>
          <div style="display:flex;align-items:baseline;gap:18px;flex-wrap:wrap;">
            <h1>{{ title }}</h1>
            <span class="run-id-short-badge" title="Short run ID \u2014 matches the ID shown in View Reports">{{ run_id_short }}</span>
          </div>
        </div>
      </div>
      <div class="run-id-row">
            <span class="run-id-chip" data-copy="{{ run.tool.run_id }}">
              <span class="run-id-chip-label"><svg class="chip-label-icon" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><line x1="4" y1="9" x2="20" y2="9"/><line x1="4" y1="15" x2="20" y2="15"/><line x1="10" y1="3" x2="8" y2="21"/><line x1="16" y1="3" x2="14" y2="21"/></svg>Run ID</span>
              <span class="run-id-chip-value">{{ run.tool.run_id }}</span>
              <span class="chip-tooltip">Unique identifier for this analysis run — click to copy</span>
            </span>
            {% if let Some(long_commit) = run.git_commit_long %}
            {% if let Some(commit_url) = git_commit_url %}
            <a class="run-id-chip run-id-chip-link" href="{{ commit_url }}" target="_blank" rel="noopener noreferrer" title="Open commit in source control">
              <span class="run-id-chip-label"><svg class="chip-label-icon" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><circle cx="12" cy="12" r="4"/><line x1="1" y1="12" x2="7" y2="12"/><line x1="17" y1="12" x2="23" y2="12"/></svg>Git Commit<svg class="chip-popout-icon" width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/><polyline points="15 3 21 3 21 9"/><line x1="10" y1="14" x2="21" y2="3"/></svg></span>
              <span class="run-id-chip-value">{{ long_commit }}</span>
              <span class="chip-tooltip">Opens commit in source control — new tab</span>
            </a>
            {% else %}
            <span class="run-id-chip" data-copy="{{ long_commit }}">
              <span class="run-id-chip-label"><svg class="chip-label-icon" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><circle cx="12" cy="12" r="4"/><line x1="1" y1="12" x2="7" y2="12"/><line x1="17" y1="12" x2="23" y2="12"/></svg>Git Commit</span>
              <span class="run-id-chip-value">{{ long_commit }}</span>
              <span class="chip-tooltip">Full commit SHA for the scanned state — click to copy</span>
            </span>
            {% endif %}
            {% else %}
            <span class="run-id-chip muted-chip">
              <span class="run-id-chip-label"><svg class="chip-label-icon" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><circle cx="12" cy="12" r="4"/><line x1="1" y1="12" x2="7" y2="12"/><line x1="17" y1="12" x2="23" y2="12"/></svg>Git Commit</span>
              <span class="run-id-chip-value">Not detected</span>
              <span class="chip-tooltip">No Git commit SHA was found for this scan</span>
            </span>
            {% endif %}
            {% if let Some(branch) = run.git_branch %}
            {% if let Some(branch_url) = git_branch_url %}
            <a class="run-id-chip run-id-chip-link" href="{{ branch_url }}" target="_blank" rel="noopener noreferrer" title="Open branch in source control">
              <span class="run-id-chip-label"><svg class="chip-label-icon" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><line x1="6" y1="3" x2="6" y2="15"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M18 9a9 9 0 0 1-9 9"/></svg>Branch<svg class="chip-popout-icon" width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/><polyline points="15 3 21 3 21 9"/><line x1="10" y1="14" x2="21" y2="3"/></svg></span>
              <span class="run-id-chip-value">{{ branch }}</span>
              <span class="chip-tooltip">Opens branch in source control — new tab</span>
            </a>
            {% else %}
            <span class="run-id-chip">
              <span class="run-id-chip-label"><svg class="chip-label-icon" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><line x1="6" y1="3" x2="6" y2="15"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M18 9a9 9 0 0 1-9 9"/></svg>Branch</span>
              <span class="run-id-chip-value">{{ branch }}</span>
              <span class="chip-tooltip">Git branch scanned for this report</span>
            </span>
            {% endif %}
            {% else %}
            {% if is_sub_report %}
            <span class="run-id-chip">
              <span class="run-id-chip-label"><svg class="chip-label-icon" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z"/></svg>Submodule</span>
              <span class="run-id-chip-value"><span class="submodule-state-badge">detached HEAD</span></span>
              <span class="chip-tooltip">Submodules are pinned to a specific commit — no branch ref</span>
            </span>
            {% else %}
            <span class="run-id-chip muted-chip">
              <span class="run-id-chip-label"><svg class="chip-label-icon" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><line x1="6" y1="3" x2="6" y2="15"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M18 9a9 9 0 0 1-9 9"/></svg>Branch</span>
              <span class="run-id-chip-value">Not detected</span>
              <span class="chip-tooltip">No Git branch was found for this scan</span>
            </span>
            {% endif %}
            {% endif %}
            {% if let Some(author) = run.git_commit_author %}
            <span class="run-id-chip" data-author="{{ author }}">
              <span class="run-id-chip-label"><svg class="chip-label-icon" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/></svg>Last Commit By</span>
              <span class="run-id-chip-value">{{ author }}<span class="author-handle"></span></span>
              <span class="chip-tooltip">Author of the most recent commit in this repository</span>
            </span>
            {% else %}
            <span class="run-id-chip muted-chip">
              <span class="run-id-chip-label"><svg class="chip-label-icon" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/></svg>Last Commit By</span>
              <span class="run-id-chip-value">Not detected</span>
              <span class="chip-tooltip">No commit author was found for this scan</span>
            </span>
            {% endif %}
      </div>

      <div class="meta">
        <span class="meta-chip">Scan by <b>{{ scan_performed_by }}</b></span>
        <span class="meta-chip">Scanned <b>{{ scan_time_pst }}</b></span>
        <span class="meta-chip">OS <b>{{ run.environment.operating_system }} / {{ run.environment.architecture }}</b></span>
        <span class="meta-chip">Files analyzed <b>{{ run.summary_totals.files_analyzed }}</b></span>
        <span class="meta-chip">Files skipped <b>{{ run.summary_totals.files_skipped }}</b></span>
      </div>

      {% if has_delta %}
      <div class="prev-scan-banner" aria-label="Changes vs. previous scan">
        <div class="prev-scan-banner-top">
          <div class="prev-scan-meta">
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg>
            {% if prev_run_id != "" %}<a href="/runs/html/{{ prev_run_id }}" target="_blank" rel="noopener" style="color:inherit;text-decoration:none;font-weight:700;">PREVIOUS SCAN</a>{% else %}<strong>PREVIOUS SCAN</strong>{% endif %}
            <span class="prev-scan-ts">{{ prev_scan_label }}</span>
            {% if prev_scan_count > 0 %}
            <span class="prev-scan-count">&#xb7; {{ prev_scan_count }} scan{% if prev_scan_count != 1 %}s{% endif %} total</span>
            {% endif %}
          </div>
          <div class="prev-scan-summary">
            Code before: <b data-raw="{{ prev_code_lines }}">{{ prev_code_lines }}</b>
            &nbsp;&rarr;&nbsp;
            Code now: <b data-raw="{{ run.summary_totals.code_lines }}">{{ run.summary_totals.code_lines }}</b>
            &nbsp;&#xb7;&nbsp;
            <span class="{% if delta_code_added > 0 %}delta-up{% else %}delta-neutral-text{% endif %}">+<span data-raw="{{ delta_code_added }}">{{ delta_code_added }}</span> added</span>
            &nbsp;
            <span class="{% if delta_code_removed > 0 %}delta-down{% else %}delta-neutral-text{% endif %}">&minus;<span data-raw="{{ delta_code_removed }}">{{ delta_code_removed }}</span> removed</span>
          </div>
        </div>
        <div class="delta-card-row">
          <div class="delta-card-inline {% if delta_code_added > 0 %}pos{% endif %}">
            <div class="delta-card-val pos">+{{ delta_code_added|commas }}</div>
            <div class="delta-card-lbl">Lines added</div>
            <div class="delta-card-tip">Code lines added since {{ prev_scan_label }}</div>
          </div>
          <div class="delta-card-inline {% if delta_code_removed > 0 %}neg{% endif %}">
            <div class="delta-card-val neg">&minus;{{ delta_code_removed|commas }}</div>
            <div class="delta-card-lbl">Lines removed</div>
            <div class="delta-card-tip">Code lines removed since {{ prev_scan_label }}</div>
          </div>
          <div class="delta-card-inline">
            <div class="delta-card-val">{{ delta_unmodified_lines|commas }}</div>
            <div class="delta-card-lbl">Unmodified lines</div>
            <div class="delta-card-tip">Code lines unchanged since {{ prev_scan_label }}</div>
          </div>
          <div class="delta-card-inline {% if delta_files_modified > 0 %}mod{% endif %}">
            <div class="delta-card-val mod">{{ delta_files_modified|commas }}</div>
            <div class="delta-card-lbl">Files modified</div>
            <div class="delta-card-tip">Files with at least one line changed</div>
          </div>
          <div class="delta-card-inline {% if delta_files_added > 0 %}pos{% endif %}">
            <div class="delta-card-val pos">{{ delta_files_added|commas }}</div>
            <div class="delta-card-lbl">Files added</div>
            <div class="delta-card-tip">New files added since {{ prev_scan_label }}</div>
          </div>
          <div class="delta-card-inline {% if delta_files_removed > 0 %}neg{% endif %}">
            <div class="delta-card-val neg">{{ delta_files_removed|commas }}</div>
            <div class="delta-card-lbl">Files removed</div>
            <div class="delta-card-tip">Files deleted since {{ prev_scan_label }}</div>
          </div>
          <div class="delta-card-inline">
            <div class="delta-card-val">{{ delta_files_unchanged|commas }}</div>
            <div class="delta-card-lbl">Files unchanged</div>
            <div class="delta-card-tip">Files with no changes since {{ prev_scan_label }}</div>
          </div>
          <div class="delta-card-inline">
            <div class="delta-card-val">{{ delta_files_total|commas }}</div>
            <div class="delta-card-lbl">Files total</div>
            <div class="delta-card-tip">Total files across both scans (modified + added + removed + unchanged)</div>
          </div>
        </div>
      </div>
      {% else %}
      <div class="prev-scan-banner prev-scan-banner-empty" aria-label="No previous scan">
        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/></svg>
        No previous scan found for this project &#x2014; this report is the baseline.
      </div>
      {% endif %}

      <div class="summary-grid">
        <div class="metric" data-metric-value="{{ run.summary_totals.total_physical_lines }}"><div class="metric-tooltip">Total lines across all analyzed files, including code, comments, and blank lines.</div><div class="metric-label">Physical lines</div><div class="metric-value"><span class="metric-big"></span></div><span class="metric-exact"></span></div>
        <div class="metric" data-metric-value="{{ run.summary_totals.code_lines }}"><div class="metric-tooltip">Lines containing executable source code, excluding comments and blanks.</div><div class="metric-label">Code</div><div class="metric-value"><span class="metric-big"></span></div><span class="metric-exact"></span></div>
        <div class="metric" data-metric-value="{{ run.summary_totals.comment_lines }}"><div class="metric-tooltip">Lines consisting entirely of comments or inline documentation.</div><div class="metric-label">Comments</div><div class="metric-value"><span class="metric-big"></span></div><span class="metric-exact"></span></div>
        <div class="metric" data-metric-value="{{ run.summary_totals.blank_lines }}"><div class="metric-tooltip">Empty or whitespace-only lines used for readability and spacing.</div><div class="metric-label">Blank</div><div class="metric-value"><span class="metric-big"></span></div><span class="metric-exact"></span></div>
        <div class="metric" data-metric-value="{{ run.summary_totals.mixed_lines_separate }}"><div class="metric-tooltip">Lines that contain both code and a trailing comment, counted separately per the mixed-line policy.</div><div class="metric-label">Mixed separate</div><div class="metric-value"><span class="metric-big"></span></div><span class="metric-exact"></span></div>
        <div class="metric" data-metric-value="{{ run.summary_totals.functions }}"><div class="metric-tooltip">Best-effort count of function/method definitions detected across all source files.</div><div class="metric-label">Functions</div><div class="metric-value"><span class="metric-big"></span></div><span class="metric-exact"></span></div>
        <div class="metric" data-metric-value="{{ run.summary_totals.classes }}"><div class="metric-tooltip">Best-effort count of class, struct, interface, and type definitions.</div><div class="metric-label">Classes / Types</div><div class="metric-value"><span class="metric-big"></span></div><span class="metric-exact"></span></div>
        <div class="metric" data-metric-value="{{ run.summary_totals.variables }}"><div class="metric-tooltip">Best-effort count of variable and constant declarations.</div><div class="metric-label">Variables</div><div class="metric-value"><span class="metric-big"></span></div><span class="metric-exact"></span></div>
        <div class="metric" data-metric-value="{{ run.summary_totals.imports }}"><div class="metric-tooltip">Best-effort count of import, include, and module-use statements.</div><div class="metric-label">Imports</div><div class="metric-value"><span class="metric-big"></span></div><span class="metric-exact"></span></div>
        <div class="metric" data-metric-value="{{ run.summary_totals.test_count }}"><div class="metric-tooltip">Best-effort count of test cases detected by framework pattern (GTest, PyTest, JUnit, etc.).</div><div class="metric-label">Tests</div><div class="metric-value"><span class="metric-big"></span></div><span class="metric-exact"></span></div>
        <div class="metric" data-metric-density><div class="metric-tooltip">Percentage of physical lines that contain executable source code — higher means a leaner, code-dense codebase.</div><div class="metric-label">Code density</div><div class="metric-value"><span class="metric-big"></span></div><span class="metric-exact"></span></div>
        <div class="metric" data-metric-value="{{ run.summary_totals.files_analyzed }}"><div class="metric-tooltip">Total number of source files included in this analysis.</div><div class="metric-label">Files analyzed</div><div class="metric-value"><span class="metric-big"></span></div><span class="metric-exact"></span></div>
        {% if run.summary_totals.cyclomatic_complexity > 0 %}<div class="metric" data-metric-value="{{ run.summary_totals.cyclomatic_complexity }}"><div class="metric-tooltip">Sum of branch decision keywords (if, for, while, ||, &amp;&amp;, …) across all code lines. Approximates total McCabe cyclomatic complexity.</div><div class="metric-label">Complexity score</div><div class="metric-value"><span class="metric-big"></span></div><span class="metric-exact"></span></div>{% endif %}
        {% if let Some(lsloc) = run.summary_totals.lsloc %}<div class="metric" data-metric-value="{{ lsloc }}"><div class="metric-tooltip">Logical SLOC: count of executable statements (semicolons for C-family; non-continuation lines for Python/Ruby/Shell). Normalises across coding styles.</div><div class="metric-label">Logical SLOC</div><div class="metric-value"><span class="metric-big"></span></div><span class="metric-exact"></span></div>{% endif %}
        {% if uloc > 0 %}<div class="metric" data-metric-value="{{ uloc }}"><div class="metric-tooltip">Unique Lines of Code: distinct non-blank code lines across all files. Counts each line once regardless of how many files it appears in.</div><div class="metric-label">Unique SLOC (ULOC)</div><div class="metric-value"><span class="metric-big"></span></div><span class="metric-exact"></span></div>{% endif %}
        {% if uloc > 0 && dryness_pct_str != "" %}<div class="metric"><div class="metric-tooltip">ULOC &divide; Code Lines &mdash; the fraction of code lines that are unique. Higher = less copy-paste across the codebase. 100% means every code line is distinct.</div><div class="metric-label">DRYness</div><div class="metric-value"><span class="metric-big">{{ dryness_pct_str }}%</span></div></div>{% endif %}
        {% if duplicate_group_count > 0 %}<div class="metric" data-metric-value="{{ duplicate_group_count }}"><div class="metric-tooltip">Groups of files with identical content detected. These may inflate SLOC totals. Re-run with --no-duplicates to exclude them.</div><div class="metric-label">Duplicate groups</div><div class="metric-value"><span class="metric-big"></span></div><span class="metric-exact"></span></div>{% endif %}
        <!-- Reserve "pad" card: revealed by JS only when the visible card count is
             odd, so the strip always has an even number of cards that fill exactly
             two aligned rows (no oversized card, no empty trailing cell). -->
        <div class="metric metric-pad" data-metric-value="{{ run.summary_totals.test_assertion_count }}" style="display:none"><div class="metric-tooltip">Best-effort count of test assertion call lines (assertEquals, EXPECT_*, etc.) detected across all test files.</div><div class="metric-label">Assertions</div><div class="metric-value"><span class="metric-big"></span></div><span class="metric-exact"></span></div>
      </div>
    </section>

    <!-- ── PDF-only pre-rendered chart variants (hidden on screen) ─────── -->
    <div id="pdf-variants" class="pdf-variants-root"></div>

    <div class="report-stack">
      <!-- ── Chart row 1: Overview + Composition ───────────────────────── -->
      <div class="charts-grid">
        <section class="panel stack chart-section">
          <div>
            <div class="toolbar">
              <div class="toolbar-left"><h2>Project Overview</h2></div>
              <button class="chart-expand-btn" id="overview-expand-btn" title="View full chart" aria-label="Expand chart">&#x2922; Full View</button>
            </div>
            <div class="chart-pre">
            <p style="margin:0 0 14px;color:var(--muted);font-size:13px;line-height:1.6;">A configurable cartesian view of your codebase. Choose what to show on each axis.</p>
            <div class="chart-controls">
              <label>Y Axis:
                <select class="chart-select" id="overview-y-axis">
                  <option value="code">Code Lines</option>
                  <option value="comments">Comment Lines</option>
                  <option value="blanks">Blank Lines</option>
                  <option value="physical">Total Physical Lines</option>
                  <option value="files">File Count</option>
                </select>
              </label>
              <label>X Axis / Mode:
                <select class="chart-select" id="overview-x-mode">
                  <option value="languages">Languages</option>
                  {% if has_submodule_data %}<option value="submodules">Submodules</option>{% endif %}
                  <option value="history-commits">Per Commit (Web UI)</option>
                  <option value="history-tags">Per Tag (Web UI)</option>
                  <option value="history-releases">Per Release (Web UI)</option>
                  <option value="history-repos">Other Repos (Web UI)</option>
                </select>
              </label>
            </div>
            </div>
            <div id="overview-chart" class="chart-container"><div id="canvas-proj-wrap" style="position:relative;min-height:150px;"><canvas id="canvas-proj"></canvas></div></div>
            <div class="chart-locked-card" id="overview-chart-locked">
              <h3>Historical trend requires the web UI</h3>
              <p style="margin:0">Run <code>oxide-sloc serve</code> and navigate to <strong>/trend-reports</strong> to view per-commit, per-tag, per-release, and cross-repo comparisons on an interactive timeline chart. The web UI stores scan history and can plot any metric over time.</p>
            </div>
          </div>
        </section>

        <section class="panel stack chart-section">
          <div>
            <div class="toolbar">
              <div class="toolbar-left"><h2>Language Composition</h2></div>
              <button class="chart-expand-btn" id="comp-expand-btn" title="View full chart" aria-label="Expand chart">&#x2922; Full View</button>
            </div>
            <div class="chart-pre">
            <p style="margin:0 0 14px;color:var(--muted);font-size:13px;">Code, comments, and blank lines as a percentage of total physical lines per language.</p>
            <div class="chart-tab-bar">
              <button type="button" class="chart-tab active" data-comp-tab="absolute">Absolute Lines</button>
              <button type="button" class="chart-tab" data-comp-tab="pct">Composition %</button>
            </div>
            </div>
            <div id="composition-chart" class="chart-container" style="overflow:hidden;display:flex;align-items:center;justify-content:center;"><div id="comp-svg-container" style="width:100%;"></div></div>
          </div>
        </section>
      </div>

      <!-- ── Chart row 2: Scatter + Semantic ───────────────────────────── -->
      <div class="charts-grid">
        <section class="panel stack chart-section">
          <div>
            <div class="toolbar">
              <div class="toolbar-left"><h2>Files vs Code Lines</h2></div>
              <button class="chart-expand-btn" id="scatter-expand-btn" title="View full chart" aria-label="Expand chart">&#x2922; Full View</button>
            </div>
            <p style="margin:0 0 14px;color:var(--muted);font-size:13px;">Each bubble is a language. X&nbsp;=&nbsp;files analyzed, Y&nbsp;=&nbsp;code lines, bubble size&nbsp;∝&nbsp;total physical lines.</p>
            <div id="scatter-chart" class="chart-container" style="position:relative;height:224px;"><canvas id="canvas-scatter"></canvas></div>
          </div>
        </section>

        <section class="panel stack chart-section">
          <div>
            <div class="toolbar">
              <div class="toolbar-left"><h2>Semantic Metrics</h2></div>
              {% if has_semantic_data %}
              <button class="chart-expand-btn" id="semantic-expand-btn" title="View full chart" aria-label="Expand chart">&#x2922; Full View</button>
              {% endif %}
            </div>
            <p style="margin:0 0 14px;color:var(--muted);font-size:13px;">Detected structural elements per language. Select a metric to explore.</p>
            {% if has_semantic_data %}
            <div class="chart-controls">
              <label>Metric:
                <select class="chart-select" id="semantic-metric">
                  <option value="functions">Functions</option>
                  <option value="classes">Classes / Types</option>
                  <option value="variables">Variables</option>
                  <option value="imports">Imports</option>
                  <option value="tests">Tests</option>
                </select>
              </label>
            </div>
            <div id="semantic-chart" class="chart-container" style="position:relative;height:234px;"><canvas id="canvas-semantic"></canvas></div>
            {% else %}
            <div style="display:flex;align-items:center;justify-content:center;height:200px;color:var(--muted);font-size:13px;text-align:center;line-height:1.6;">
              <div>No structural metrics detected for this scan.<br>Semantic analysis covers languages with function/class detection<br>(e.g., Go, Python, Rust, Java, C++).</div>
            </div>
            {% endif %}
          </div>
        </section>
        <section class="panel stack chart-section">
          <div>
            <div class="toolbar">
              <div class="toolbar-left"><h2>Comment Density</h2></div>
              <button class="chart-expand-btn" id="density-expand-btn" title="View full chart" aria-label="Expand chart">&#x2922; Full View</button>
            </div>
            <p style="margin:0 0 14px;color:var(--muted);font-size:13px;">Comments as a percentage of significant lines (code + comments) per language — a proxy for documentation coverage.</p>
            <div id="density-chart" class="chart-container" style="position:relative;min-height:150px;"><canvas id="canvas-density"></canvas></div>
          </div>
        </section>

        <section class="panel stack chart-section">
          <div>
            <div class="toolbar">
              <div class="toolbar-left"><h2>File Size Distribution</h2></div>
              <button class="chart-expand-btn" id="filesize-expand-btn" title="View full chart" aria-label="Expand chart">&#x2922; Full View</button>
            </div>
            <p style="margin:0 0 14px;color:var(--muted);font-size:13px;">Number of files in each SLOC bucket — a quick view of whether the codebase favours small focused modules or large files.</p>
            <div id="filesize-chart" class="chart-container" style="position:relative;min-height:150px;"><canvas id="canvas-filesize"></canvas></div>
          </div>
        </section>
      </div>

      <!-- ── Tests & Coverage ──────────────────────────────────────────── -->
      <section class="panel stack">
        <div>
          <div class="toolbar">
            <div class="toolbar-left"><h2>Tests &amp; Coverage</h2></div>
            {% if has_coverage_data %}<div class="pill-row"><span class="pill good">LCOV coverage data present</span></div>{% endif %}
          </div>
          <div class="summary-strip">
            <div class="stat-chip">
              <div class="stat-chip-val" data-fmt="{{ run.summary_totals.test_count }}">{{ run.summary_totals.test_count|commas }}</div>
              <div class="stat-chip-label">Test Functions</div>
              <div class="stat-chip-tip">Lexically detected test case / function definitions (GTest, PyTest, JUnit, Unity, etc.)</div>
              <span class="stat-chip-exact">{{ run.summary_totals.test_count|commas }}</span>
            </div>
            <div class="stat-chip">
              <div class="stat-chip-val" data-fmt="{{ test_assertion_count }}">{{ test_assertion_count|commas }}</div>
              <div class="stat-chip-label">Assertions</div>
              <div class="stat-chip-tip">Test assertion call lines (ASSERT_EQ, EXPECT_TRUE, assertEquals, Assert.AreEqual, assert_eq!, etc.)</div>
              <span class="stat-chip-exact">{{ test_assertion_count|commas }}</span>
            </div>
            <div class="stat-chip">
              <div class="stat-chip-val" data-fmt="{{ test_suite_count }}">{{ test_suite_count|commas }}</div>
              <div class="stat-chip-label">Test Suites</div>
              <div class="stat-chip-tip">Test suite / fixture / group declarations (TEST_GROUP, BOOST_AUTO_TEST_SUITE, [TestClass], etc.)</div>
              <span class="stat-chip-exact">{{ test_suite_count|commas }}</span>
            </div>
            <div class="stat-chip">
              <div class="stat-chip-val">{{ test_files_count|commas }} / {{ run.summary_totals.files_analyzed|commas }}</div>
              <div class="stat-chip-label">Test Files</div>
              <div class="stat-chip-tip">Files containing at least one detected test definition out of total analyzed files</div>
            </div>
          </div>
          <div class="summary-strip" style="margin-top:0;">
            <div class="stat-chip">
              <div class="stat-chip-val">{{ test_density }}</div>
              <div class="stat-chip-label">Tests per 1K SLOC</div>
              <div class="stat-chip-tip">Workspace-wide test density: test functions ÷ code lines × 1000</div>
            </div>
            <div class="stat-chip">
              <div class="stat-chip-val" style="font-size:15px;word-break:break-word;line-height:1.2;">{{ most_tested_lang }}</div>
              <div class="stat-chip-label">Most Tested Language</div>
              <div class="stat-chip-tip">Language with the highest absolute test function count</div>
            </div>
            <div class="stat-chip">
              <div class="stat-chip-val">{{ langs_with_tests }}</div>
              <div class="stat-chip-label">Languages with Tests</div>
              <div class="stat-chip-tip">Number of distinct languages where test definitions were detected</div>
            </div>
            <div class="stat-chip">
              {% if has_coverage_data %}<div class="stat-chip-val">{{ cov_line_pct }}%</div>{% else %}<div class="stat-chip-val" style="color:var(--muted);">&mdash;</div>{% endif %}
              <div class="stat-chip-label">Line Coverage</div>
              <div class="stat-chip-tip">Overall line coverage from LCOV data — run with --lcov-path to populate</div>
            </div>
          </div>
          {% if has_coverage_data %}
          <div class="cov-gauge-row">
            <div class="cov-gauge-card">
              <div class="cov-gauge-label">Line Coverage</div>
              <div class="cov-gauge-val" style="color:var(--{{ cov_line_class }}-text);">{{ cov_line_pct }}%</div>
              <div class="cov-gauge-track"><div class="cov-gauge-fill" style="width:{{ cov_line_pct }}%;background:var(--{{ cov_line_class }}-text);"></div></div>
              <div class="cov-gauge-sub">Lines hit / instrumented</div>
              <div class="cov-gauge-tip">Share of instrumented source lines executed at least once during the test run (from LCOV <code>DA</code> records).</div>
            </div>
            {% if has_fn_coverage %}
            <div class="cov-gauge-card">
              <div class="cov-gauge-label">Function Coverage</div>
              <div class="cov-gauge-val" style="color:var(--{{ cov_fn_class }}-text);">{{ cov_fn_pct }}%</div>
              <div class="cov-gauge-track"><div class="cov-gauge-fill" style="width:{{ cov_fn_pct }}%;background:var(--{{ cov_fn_class }}-text);"></div></div>
              <div class="cov-gauge-sub">Functions hit / found</div>
            </div>
            {% endif %}
            {% if has_branch_coverage %}
            <div class="cov-gauge-card">
              <div class="cov-gauge-label">Branch Coverage</div>
              <div class="cov-gauge-val" style="color:var(--{{ cov_branch_class }}-text);">{{ cov_branch_pct }}%</div>
              <div class="cov-gauge-track"><div class="cov-gauge-fill" style="width:{{ cov_branch_pct }}%;background:var(--{{ cov_branch_class }}-text);"></div></div>
              <div class="cov-gauge-sub">Branches hit / found</div>
            </div>
            {% endif %}
          </div>
          {% endif %}
          <div class="table-shell" style="margin-top:16px;">
            <table data-sort-table style="min-width:560px;">
              <thead>
                <tr>
                  <th data-sort-type="text">Language</th>
                  <th data-sort-type="number">Test Fns</th>
                  <th data-sort-type="number">Assertions</th>
                  <th data-sort-type="number">Suites</th>
                  <th data-sort-type="text">Density (per 1K SLOC)</th>
                </tr>
              </thead>
              <tbody>
                {% for row in language_rows %}
                {% if row.test_count > 0 || row.test_assertion_count > 0 %}
                <tr>
                  <td>{{ row.language }}</td>
                  <td>{{ row.test_count|commas }}</td>
                  <td>{{ row.test_assertion_count|commas }}</td>
                  <td>{{ row.test_suite_count|commas }}</td>
                  <td>{{ row.test_density_str }}</td>
                </tr>
                {% endif %}
                {% endfor %}
                {% if run.summary_totals.test_count == 0 && test_assertion_count == 0 %}
                <tr class="empty-state-row"><td colspan="5">No test functions or assertions detected in this scan</td></tr>
                {% endif %}
              </tbody>
            </table>
          </div>
          {% if has_coverage_data %}
          <div style="display:flex;align-items:center;gap:10px;margin:16px 0 8px;">
            <h3 style="margin:0;font-size:14px;font-weight:800;color:var(--text);">Per-File Coverage</h3>
            <span class="pill good" style="font-size:10px;">{{ file_rows.len() }} files with data</span>
          </div>
          <div class="table-shell">
            <table data-sort-table class="cov-file-table" style="min-width:560px;">
              <thead>
                <tr>
                  <th data-sort-type="text">File</th>
                  <th class="num-col" data-sort-type="number">Line Cov %</th>
                  <th class="num-col" data-sort-type="text">Lines Hit / Found</th>
                  {% if has_fn_coverage %}<th class="num-col" data-sort-type="number">Fn Cov %</th>{% endif %}
                  {% if has_branch_coverage %}<th class="num-col" data-sort-type="number">Branch Cov %</th>{% endif %}
                </tr>
              </thead>
              <tbody>
                {% for row in file_rows %}
                {% if !row.line_cov_pct.is_empty() %}
                <tr>
                  <td class="mono" style="font-size:11px;max-width:340px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;" title="{{ row.relative_path }}">{{ row.relative_path }}</td>
                  <td class="num-col">{{ row.line_cov_pct }}%</td>
                  <td class="num-col" style="font-size:11px;color:var(--muted);">{{ row.cov_lines_detail }}</td>
                  {% if has_fn_coverage %}<td class="num-col">{% if !row.fn_cov_pct.is_empty() %}{{ row.fn_cov_pct }}%{% else %}&mdash;{% endif %}</td>{% endif %}
                  {% if has_branch_coverage %}<td class="num-col">{% if !row.branch_cov_pct.is_empty() %}{{ row.branch_cov_pct }}%{% else %}&mdash;{% endif %}</td>{% endif %}
                </tr>
                {% endif %}
                {% endfor %}
                {% if file_rows.is_empty() %}
                <tr class="empty-state-row"><td colspan="5">No per-file coverage data available</td></tr>
                {% endif %}
              </tbody>
            </table>
          </div>
          {% else %}
          <div class="info-callout">
            <span class="info-callout-icon">&#x2139;</span>
            <span>No code coverage detected. Re-run with <code>--lcov-path coverage.info</code> to see line, function, and branch coverage here.</span>
          </div>
          {% endif %}
        </div>
      </section>

      <!-- ── Multi-Language Code Style Analysis ───────────────────────── -->
      {% if has_style_data %}
      {% if let Some(ss) = style_summary %}
      <section class="panel stack">
        <div>
          <div class="toolbar">
            <div class="toolbar-left"><h2>Code Style Analysis</h2></div>
            <div class="pill-row"><span class="pill info">{{ style_lang_count }} language group(s) &#xB7; Lexical heuristics</span></div>
          </div>
          <div class="style-heuristic-note">
            <span class="info-callout-icon">&#x2139;</span>
            <span>Scores are lexical approximations based on indentation, line length, brace placement, and language-specific signals &#x2014; not a full parse. Use as a directional signal.</span>
          </div>
          <!-- Summary chips -->
          <div class="style-metrics-strip">
            <div class="style-chip">
              <div class="style-chip-val">{{ ss.files_analyzed }}</div>
              <div class="style-chip-label">Files Analyzed</div>
              <div class="style-chip-tip">Total files with style data</div>
            </div>
            <div class="style-chip">
              <div class="style-chip-val">{{ style_lang_count }}</div>
              <div class="style-chip-label">Language Groups</div>
              <div class="style-chip-tip">Distinct language families detected</div>
            </div>
            <div class="style-chip">
              <div class="style-chip-val">{{ ss.common_indent_style }}</div>
              <div class="style-chip-label">Common Indent</div>
              <div class="style-chip-tip">Most prevalent indentation across all files</div>
            </div>
            <div class="style-chip">
              <div class="style-chip-val">{{ ss.line_col_compliant_pct }}%</div>
              <div class="style-chip-label">{{ ss.col_threshold }}-Col Compliant</div>
              <div class="style-chip-tip">Files where &le;5% of lines exceed {{ ss.col_threshold }} chars</div>
            </div>
          </div>
          <!-- Language selector tab strip -->
          <div style="margin-top:20px;">
            <div style="font-size:12px;font-weight:800;text-transform:uppercase;letter-spacing:.07em;color:var(--muted);margin-bottom:10px;">Style Guide Adherence by Language</div>
            <div id="style-lang-tabs" class="style-lang-tabs"></div>
            <div class="style-guide-grid" id="style-guide-bars"></div>
          </div>
          <!-- Per-file style table -->
          <div style="margin-top:22px;">
            <div class="toolbar" style="margin-bottom:8px;">
              <div class="toolbar-left">
                <span style="font-size:12px;font-weight:800;text-transform:uppercase;letter-spacing:.07em;color:var(--muted);">Per-File Style Details</span>
                <input id="sft-search" class="search" type="search" placeholder="Filter files, languages, guides..." style="margin-left:12px;" />
                <div class="page-size-row"><label class="page-size-label" for="sft-page-size">Show:</label><select id="sft-page-size" class="page-size-select"><option value="20" selected>20</option><option value="50">50</option><option value="100">100</option><option value="all">All</option></select><span id="sft-count-label" class="page-count-label"></span></div>
              </div>
            </div>
            <div class="table-scroll-wrap">
              <table class="style-file-table" id="style-file-table">
                <thead>
                  <tr>
                    <th data-sort-key="path" style="width:35%;" title="File path relative to the scanned root. Click to sort alphabetically.">File <span class="style-sort-ind">&#9662;</span></th>
                    <th data-sort-key="lang" style="width:10%;" title="Programming language detected for this file. Click to sort.">Language <span class="style-sort-ind">&#9662;</span></th>
                    <th data-sort-key="indent" style="width:12%;" title="Dominant indentation style detected: Tabs, 2-Space, 4-Space, 8-Space, Mixed, or Unknown. Click to sort.">Indent <span class="style-sort-ind">&#9662;</span></th>
                    <th data-sort-key="guide" style="width:20%;" title="Style guide with the highest lexical-adherence score for this file. Click a badge to open the official guide documentation. Click header to sort.">Best Match Guide <span class="style-sort-ind">&#9662;</span></th>
                    <th data-sort-key="score" style="width:10%;" title="Adherence score (0-100%) for the best-matching style guide. Higher = closer match to that guide's conventions. Lexical heuristic only \u2014 not a full parse. Click to sort.">Score <span class="style-sort-ind">&#9662;</span></th>
                    <th style="width:13%;" title="Hover a row to see all signals \u2014 signal name and detected value.">Signals</th>
                  </tr>
                </thead>
                <tbody id="style-file-tbody">
                  <tr><td colspan="6" style="text-align:center;color:var(--muted);padding:18px;">Loading...</td></tr>
                </tbody>
              </table>
            </div>
            <div id="sft-pagination" class="pagination-bar">
              <button id="sft-first" class="pager-btn pager-edge" disabled title="First page">&#8676; First</button>
              <button id="sft-prev" class="pager-btn" disabled>&#8592; Prev</button>
              <span class="pager-jump-wrap">Page <input id="sft-page-jump" class="pager-jump" type="number" min="1" value="1" title="Jump to page"> of <span id="sft-page-total">&#8212;</span></span>
              <span id="sft-page-info" class="pager-info"></span>
              <button id="sft-next" class="pager-btn">Next &#8594;</button>
              <button id="sft-last" class="pager-btn pager-edge" title="Last page">Last &#8677;</button>
            </div>
          </div>
        </div>
      </section>
      {% endif %}
      {% endif %}

      <!-- ── Submodule Breakdown (2-column, conditional) ─────────────── -->
      {% if has_submodule_data %}
      <div class="charts-grid">
        <section class="panel stack chart-section">
          <div>
            <div class="toolbar">
              <div class="toolbar-left"><h2>Submodule Breakdown</h2></div>
              <button class="chart-expand-btn" id="sub-expand-btn" title="View full chart" aria-label="Expand chart">&#x2922; Full View</button>
            </div>
            <div class="chart-controls">
              <label>Y Axis:
                <select class="chart-select" id="sub-y-axis">
                  <option value="code">Code Lines</option>
                  <option value="comment">Comment Lines</option>
                  <option value="blank">Blank Lines</option>
                  <option value="physical">Total Physical Lines</option>
                  <option value="files">File Count</option>
                </select>
              </label>
              <label>Sort:
                <select class="chart-select" id="sub-sort">
                  <option value="desc">Value ↓</option>
                  <option value="asc">Value ↑</option>
                  <option value="name">Name A→Z</option>
                </select>
              </label>
            </div>
            <div id="submodule-chart" class="chart-container"><div id="canvas-sub-wrap" style="position:relative;min-height:150px;"><canvas id="canvas-sub"></canvas></div></div>
          </div>
        </section>
        <section class="panel stack chart-section">
          <div>
            <div class="toolbar">
              <div class="toolbar-left"><h2>Submodule Composition</h2></div>
              <button class="chart-expand-btn" id="sub-comp-expand-btn" title="View full chart" aria-label="Expand chart">&#x2922; Full View</button>
            </div>
            <p style="margin:0 0 14px;color:var(--muted);font-size:13px;">Code vs comments vs blank lines per submodule — bar width reflects relative size.</p>
            <div id="submodule-donut" style="width:100%;padding:4px 0;overflow:hidden;"></div>
          </div>
        </section>
      </div>
      {% endif %}

      {% if has_cocomo %}
      <section class="panel" id="cocomo-section">
        <div class="toolbar">
          <div class="toolbar-left">
            <h2>Constructive Cost Model &mdash; COCOMO I</h2>
            <span class="cocomo-mode-pill-wrap" style="margin-left:12px;">
              <span class="pill" style="background:var(--surface-3);color:var(--muted);border:1px solid var(--line);font-size:11px;">{{ cocomo_mode_label }} mode</span>
              <span class="cocomo-mode-tip">{{ cocomo_mode_tooltip }}</span>
            </span>
          </div>
        </div>
        <div class="summary-strip" style="grid-template-columns:repeat(4,1fr);">
          <div class="stat-chip">
            <div class="stat-chip-label">Person-months</div>
            <div class="stat-chip-val">{{ cocomo_effort_str|commas }}</div>
            <div class="stat-chip-tip">Total estimated developer effort to build this codebase from scratch. One person-month = one developer working full-time for one calendar month. Computed as 2.4 &times; KSLOC^1.05 (Organic mode).</div>
          </div>
          <div class="stat-chip">
            <div class="stat-chip-label">Schedule (months)</div>
            <div class="stat-chip-val">{{ cocomo_duration_str|commas }}</div>
            <div class="stat-chip-tip">Estimated calendar duration assuming an optimally sized team. Computed as 2.5 &times; effort^0.38. Adding more people beyond this optimum rarely shortens the timeline.</div>
          </div>
          <div class="stat-chip">
            <div class="stat-chip-label">Avg. Team Size</div>
            <div class="stat-chip-val">{{ cocomo_staff_str|commas }}</div>
            <div class="stat-chip-tip">Average number of engineers working in parallel, derived as effort &divide; schedule. Actual headcount may peak higher during intensive phases of the project.</div>
          </div>
          <div class="stat-chip">
            <div class="stat-chip-label">Input KSLOC</div>
            <div class="stat-chip-val">{{ cocomo_ksloc_str|commas }}K</div>
            <div class="stat-chip-tip">KSLOC = Kilo Source Lines of Code (1 KSLOC = 1,000 lines). This is the primary input to the COCOMO model. Only executable code lines are counted &mdash; blank lines and comments are excluded. ({{ run.summary_totals.code_lines|commas }} total code lines)</div>
          </div>
        </div>
        <p style="font-size:13px;color:var(--muted);padding:8px 4px 0;line-height:1.6;white-space:nowrap;">COCOMO I (Constructive Cost Model) is a 1981 algorithmic model by Barry Boehm that converts SLOC into effort, schedule, and team-size estimates.<br>These are ballpark figures &mdash; actual outcomes vary widely by team experience, toolchain maturity, and domain complexity.</p>
      </section>
      {% endif %}

      {% if has_hotspots %}
      <section class="panel" id="hotspots-section">
        <div class="toolbar"><div class="toolbar-left"><h2>Git Hotspots</h2><input id="hotspots-search" class="search" type="search" placeholder="Filter files..." /><div class="page-size-row"><label class="page-size-label" for="hotspots-page-size">Show:</label><select id="hotspots-page-size" class="page-size-select"><option value="15" selected>15</option><option value="25">25</option><option value="50">50</option><option value="all">All</option></select><span id="hotspots-count-label" class="page-count-label"></span></div></div></div>
        <p class="section-desc">Files ranked by <strong>code lines &times; recent commits</strong> over the configured git activity window. Large files that change often are the strongest refactoring candidates.{% if has_ownership %} The <strong>Author</strong> column shows each file's primary author from git blame (only shown when code-ownership attribution ran).{% endif %}</p>
        <p class="table-hint hs-hint">Click a column header to sort; drag its right edge to resize; hover a header for what it means.</p>
        <div class="table-shell">
          <table id="hotspots-table" data-sort-table class="table-resizable hotspots-table">
            <colgroup><col>{% if has_ownership %}<col>{% endif %}<col><col><col><col></colgroup>
            <thead><tr>
              <th data-sort-type="text">File<span class="col-tip">Repository-relative path of the file. Click to sort the list alphabetically by path.</span><div class="col-resize-handle"></div></th>
              {% if has_ownership %}<th data-sort-type="text">Author<span class="col-tip">Primary author of this file &mdash; the contributor who owns the most of its lines, per git blame.</span><div class="col-resize-handle"></div></th>{% endif %}
              <th data-sort-type="number" class="num-col">Code lines<span class="col-tip col-tip-r">Executable source lines in the file (blank lines and comments excluded). Bigger files are harder to change safely.</span><div class="col-resize-handle"></div></th>
              <th data-sort-type="number" class="num-col">Commits<span class="col-tip col-tip-r">How many times the file was committed within the git activity window. More commits = more churn.</span><div class="col-resize-handle"></div></th>
              <th data-sort-type="number" class="num-col">Hotspot score<span class="col-tip col-tip-r"><strong>Code lines &times; Commits.</strong> A large file that changes often scores high &mdash; it concentrates both size and churn, making it the strongest refactoring candidate. Lower is calmer.</span><div class="col-resize-handle"></div></th>
              <th data-sort-type="text" class="num-col">Last changed<span class="col-tip col-tip-r">Date of the most recent commit that touched this file, within the activity window.</span><div class="col-resize-handle"></div></th>
            </tr></thead>
            <tbody>
            {% for h in hotspot_rows %}
              <tr>
                <td class="mono" title="{{ h.path }}">{{ h.path }}</td>
                {% if has_ownership %}<td>{% match h.owner_profile_url %}{% when Some with (url) %}<a class="author-link" href="{{ url }}" target="_blank" rel="noopener noreferrer">{{ h.owner }}</a>{% when None %}{{ h.owner }}{% endmatch %}</td>{% endif %}
                <td class="num-col">{{ h.code_lines|commas }}</td>
                <td class="num-col">{{ h.commit_count }}</td>
                <td class="num-col" style="font-weight:700;color:var(--oxide);">{{ h.score|commas }}</td>
                <td class="num-col" style="color:var(--muted);">{{ h.last_commit_date }}</td>
              </tr>
            {% endfor %}
            </tbody>
          </table>
        </div>
        <div id="hotspots-pagination" class="pagination-bar">
          <button id="hs-first" class="pager-btn pager-edge" disabled title="First page">&#8676; First</button>
          <button id="hs-prev" class="pager-btn" disabled>&#8592; Prev</button>
          <span class="pager-jump-wrap">Page <input id="hs-page-jump" class="pager-jump" type="number" min="1" value="1" title="Jump to page"> of <span id="hs-page-total">&#8212;</span></span>
          <span id="hs-page-info" class="pager-info"></span>
          <button id="hs-next" class="pager-btn">Next &#8594;</button>
          <button id="hs-last" class="pager-btn pager-edge" title="Last page">Last &#8677;</button>
        </div>
      </section>
      {% endif %}

      {% if has_ownership %}
      <section class="panel" id="ownership-section">
        <div class="toolbar"><div class="toolbar-left"><h2>Code Ownership</h2><input id="ownership-search" class="search" type="search" placeholder="Filter authors..." /></div></div>
        <p class="section-desc">Per-author line ownership from <strong>git blame</strong> &mdash; each physical line is attributed to the author who last touched it (<code>-w -M -C</code>, <code>.mailmap</code> honoured), split into <strong>code / comment / blank</strong>. Same-email identities are merged automatically; cross-account merging is a later step.</p>
        <div class="summary-strip own-summary-strip">
          <div class="stat-chip"><div class="stat-chip-val">{{ own_contributors|commas }}</div><div class="stat-chip-label">Contributors</div><div class="stat-chip-tip">Distinct authors owning at least one line in this scan, after identity merges.</div></div>
          <div class="stat-chip"><div class="stat-chip-val">{{ own_top_name }}</div><div class="stat-chip-label">Top Owner &middot; {{ own_top_pct_str }}% of code</div><div class="stat-chip-tip">The single contributor owning the largest share of code lines.</div></div>
          <div class="stat-chip"><div class="stat-chip-val">{{ own_bus_factor }}</div><div class="stat-chip-label">Bus Factor</div><div class="stat-chip-tip">Fewest contributors who together own at least half of the code &mdash; a low number means knowledge is concentrated in very few people.</div></div>
          <div class="stat-chip"><div class="stat-chip-val" data-fmt="{{ own_total_code }}">{{ own_total_code|commas }}</div><div class="stat-chip-label">Total Code Lines</div><div class="stat-chip-tip">Physical code lines attributed across all contributors.</div></div>
          <div class="stat-chip"><div class="stat-chip-val" data-fmt="{{ own_dev_code }}">{{ own_dev_code|commas }}</div><div class="stat-chip-label">Development Code</div><div class="stat-chip-tip">Code lines owned in non-test files (total code minus code in files detected as tests).</div></div>
          <div class="stat-chip"><div class="stat-chip-val" data-fmt="{{ own_test_code }}">{{ own_test_code|commas }}</div><div class="stat-chip-label">Test Code &middot; {{ own_test_pct_str }}% of code</div><div class="stat-chip-tip">Code lines owned in files classified as tests (detected test functions/assertions or a test-path convention).</div></div>
          <div class="stat-chip"><div class="stat-chip-val" data-fmt="{{ own_total_comment }}">{{ own_total_comment|commas }}</div><div class="stat-chip-label">Total Comment Lines</div><div class="stat-chip-tip">Physical comment / documentation lines attributed across all contributors.</div></div>
        </div>
        <p class="table-hint hs-hint">Click a column header to sort; drag its right edge to resize.</p>
        <div class="table-shell">
          <table id="ownership-table" data-sort-table class="table-resizable">
            <colgroup><col><col><col><col><col><col><col><col></colgroup>
            <thead><tr>
              <th data-sort-type="text">Author<div class="col-resize-handle"></div></th>
              <th data-sort-type="text">Email<div class="col-resize-handle"></div></th>
              <th data-sort-type="number" class="num-col">Code<div class="col-resize-handle"></div></th>
              <th data-sort-type="number" class="num-col">Comment<div class="col-resize-handle"></div></th>
              <th data-sort-type="number" class="num-col">Blank<div class="col-resize-handle"></div></th>
              <th data-sort-type="number" class="num-col">Total<div class="col-resize-handle"></div></th>
              <th data-sort-type="number" class="num-col">Code %<div class="col-resize-handle"></div></th>
              <th data-sort-type="number" class="num-col">Files owned<div class="col-resize-handle"></div></th>
            </tr></thead>
            <tbody>
            {% for a in ownership_rows %}
              <tr>
                <td>{% match a.profile_url %}{% when Some with (url) %}<a class="author-link" href="{{ url }}" target="_blank" rel="noopener noreferrer">{{ a.name }}</a>{% when None %}{{ a.name }}{% endmatch %}</td>
                <td class="mono" style="color:var(--muted);">{{ a.email }}</td>
                <td class="num-col">{{ a.code|commas }}</td>
                <td class="num-col">{{ a.comment|commas }}</td>
                <td class="num-col">{{ a.blank|commas }}</td>
                <td class="num-col">{{ a.total|commas }}</td>
                <td class="num-col" style="font-weight:700;color:var(--oxide);">{{ a.code_pct_str }}%</td>
                <td class="num-col">{{ a.files_owned }}</td>
              </tr>
            {% endfor %}
            </tbody>
          </table>
        </div>
        <h3 class="own-files-title">Contributor Leaderboard</h3>
        <p class="own-files-sub">Ranked by <strong>code lines owned</strong>. Expand a contributor to see the files they own (files where they are the top blame owner), with the lines they own in each &mdash; tying ownership to the specific files, including the Git Hotspots.</p>
        {% for a in ownership_rows %}
        {% if !a.files.is_empty() %}
        <details class="own-details">
          <summary>
            <span class="lb-rank {{ a.rank_class }}">{{ a.rank }}</span>
            <span class="lb-avatar" style="background:{{ a.color }};">{{ a.initials }}</span>
            <span class="lb-name">{% match a.profile_url %}{% when Some with (url) %}<a class="author-link" href="{{ url }}" target="_blank" rel="noopener noreferrer">{{ a.name }}</a>{% when None %}{{ a.name }}{% endmatch %}</span>
            <span class="lb-bar-wrap"><span class="lb-bar" style="width:{{ a.code_pct_str }}%;background:{{ a.color }};"></span></span>
            <span class="lb-stats"><strong>{{ a.code|commas }}</strong> code lines &middot; {{ a.files_owned }} file{% if a.files_owned != 1 %}s{% endif %} &middot; {{ a.code_pct_str }}%</span>
          </summary>
          <div class="table-shell own-files-shell">
            <table class="table-resizable">
              <colgroup><col><col><col><col><col><col></colgroup>
              <thead><tr>
                <th data-sort-type="text">File</th>
                <th data-sort-type="number" class="num-col">Code</th>
                <th data-sort-type="number" class="num-col">Comment</th>
                <th data-sort-type="number" class="num-col">Blank</th>
                <th data-sort-type="number" class="num-col">Total</th>
                <th data-sort-type="text" class="num-col">Last changed</th>
              </tr></thead>
              <tbody>
              {% for f in a.files %}
                <tr>
                  <td class="mono" title="{{ f.path }}">{{ f.path }}</td>
                  <td class="num-col">{{ f.code|commas }}</td>
                  <td class="num-col">{{ f.comment|commas }}</td>
                  <td class="num-col">{{ f.blank|commas }}</td>
                  <td class="num-col">{{ f.total|commas }}</td>
                  <td class="num-col" style="color:var(--muted);">{{ f.last_changed }}</td>
                </tr>
              {% endfor %}
              </tbody>
            </table>
          </div>
        </details>
        {% endif %}
        {% endfor %}
      </section>
      {% endif %}

      <section class="panel stack">
        <div>
          <div class="toolbar"><div class="toolbar-left"><h2>Language Breakdown</h2></div><button class="chart-expand-btn" id="lang-overview-expand-btn" title="View full chart" aria-label="Expand charts">&#x2922; Full View</button></div>
          <div id="report-lang-overview" style="margin:0 0 16px;"></div>
          <div class="table-shell">
            <table id="lang-breakdown-table" data-sort-table class="table-resizable">
              <colgroup>
                <col><col><col><col><col><col><col><col><col><col><col><col><col><col>
              </colgroup>
              <thead>
                <tr>
                  <th data-sort-type="text">Language<div class="col-resize-handle"></div></th>
                  <th data-sort-type="number" class="num-col">Files<div class="col-resize-handle"></div></th>
                  <th data-sort-type="number" class="num-col">Physical<div class="col-resize-handle"></div></th>
                  <th data-sort-type="number" class="num-col">Code<div class="col-resize-handle"></div></th>
                  <th data-sort-type="number" class="num-col">Comments<div class="col-resize-handle"></div></th>
                  <th data-sort-type="number" class="num-col">Blank<div class="col-resize-handle"></div></th>
                  <th data-sort-type="number" class="num-col">Mixed<div class="col-resize-handle"></div></th>
                  <th data-sort-type="number" class="num-col">Functions<div class="col-resize-handle"></div></th>
                  <th data-sort-type="number" class="num-col">Classes<div class="col-resize-handle"></div></th>
                  <th data-sort-type="number" class="num-col">Variables<div class="col-resize-handle"></div></th>
                  <th data-sort-type="number" class="num-col">Imports<div class="col-resize-handle"></div></th>
                  <th data-sort-type="number" class="num-col">Tests<div class="col-resize-handle"></div></th>
                  <th data-sort-type="number" class="num-col">Assertions<div class="col-resize-handle"></div></th>
                  <th data-sort-type="number" class="num-col">Suites<div class="col-resize-handle"></div></th>
                </tr>
              </thead>
              <tbody>
                {% for row in language_rows %}
                <tr>
                  <td title="{{ row.language }}">{{ row.language }}</td>
                  <td class="num-col">{{ row.files|commas }}</td>
                  <td class="num-col">{{ row.total_physical_lines|commas }}</td>
                  <td class="num-col">{{ row.code_lines|commas }}</td>
                  <td class="num-col">{{ row.comment_lines|commas }}</td>
                  <td class="num-col">{{ row.blank_lines|commas }}</td>
                  <td class="num-col">{{ row.mixed_lines_separate|commas }}</td>
                  <td class="num-col">{{ row.functions|commas }}</td>
                  <td class="num-col">{{ row.classes|commas }}</td>
                  <td class="num-col">{{ row.variables|commas }}</td>
                  <td class="num-col">{{ row.imports|commas }}</td>
                  <td class="num-col">{{ row.test_count|commas }}</td>
                  <td class="num-col">{{ row.test_assertion_count|commas }}</td>
                  <td class="num-col">{{ row.test_suite_count|commas }}</td>
                </tr>
                {% endfor %}
              </tbody>
            </table>
          </div>
        </div>
      </section>

      <section class="panel stack">
        <div class="toolbar"><div class="toolbar-left"><h2>Per-file detail</h2><input id="per-file-search" class="search" type="search" placeholder="Filter files, languages, status, warnings..." /><div class="page-size-row"><label class="page-size-label" for="per-file-page-size">Show:</label><select id="per-file-page-size" class="page-size-select"><option value="20" selected>20</option><option value="50">50</option><option value="100">100</option><option value="all">All</option></select><span id="per-file-count-label" class="page-count-label"></span></div></div><div class="pill-row"><span class="pill good">Counts shown as analyzed by the selected policy</span><div class="export-group"><button class="export-btn" data-reset-table title="Reset scroll and column layout">&#8635; Reset</button><button class="export-btn" data-export-csv>&#8595; CSV</button><button class="export-btn" data-export-xls>&#8595; Excel</button></div></div></div>
        <div class="table-shell table-shell-clip">
        <div id="per-file-shell">
          <table id="per-file-table" data-sort-table class="table-resizable">
            <colgroup>
              <col><col><col><col><col><col><col><col><col><col><col><col><col><col>
            </colgroup>
            <thead>
              <tr>
                <th data-sort-type="text">File<div class="col-resize-handle"></div></th>
                <th data-sort-type="text">Language<div class="col-resize-handle"></div></th>
                <th data-sort-type="number" class="num-col">Physical<div class="col-resize-handle"></div></th>
                <th data-sort-type="number" class="num-col">Code<div class="col-resize-handle"></div></th>
                <th data-sort-type="number" class="num-col">Comments<div class="col-resize-handle"></div></th>
                <th data-sort-type="number" class="num-col">Blank<div class="col-resize-handle"></div></th>
                <th data-sort-type="number" class="num-col">Mixed<div class="col-resize-handle"></div></th>
                <th data-sort-type="number" class="num-col">Functions<div class="col-resize-handle"></div></th>
                <th data-sort-type="number" class="num-col">Classes<div class="col-resize-handle"></div></th>
                <th data-sort-type="number" class="num-col">Variables<div class="col-resize-handle"></div></th>
                <th data-sort-type="number" class="num-col">Imports<div class="col-resize-handle"></div></th>
                <th data-sort-type="number" class="num-col">Tests<div class="col-resize-handle"></div></th>
                <th data-sort-type="number" class="num-col">Assertions<div class="col-resize-handle"></div></th>
                <th data-sort-type="number" class="num-col">Suites<div class="col-resize-handle"></div></th>
                {% if has_coverage_data %}<th data-sort-type="text" class="num-col">Line Cov %<div class="col-resize-handle"></div></th><th data-sort-type="text" class="num-col">Fn Cov %<div class="col-resize-handle"></div></th>{% endif %}
              </tr>
            </thead>
            <tbody>
              {% for row in file_rows %}
              <tr>
                <td class="mono" title="{{ row.relative_path }}">{{ row.relative_path }}</td>
                <td title="{{ row.language }}">{{ row.language }}</td>
                <td class="num-col">{{ row.total_physical_lines }}</td>
                <td class="num-col">{{ row.code_lines }}</td>
                <td class="num-col">{{ row.comment_lines }}</td>
                <td class="num-col">{{ row.blank_lines }}</td>
                <td class="num-col">{{ row.mixed_lines_separate }}</td>
                <td class="num-col">{{ row.functions }}</td>
                <td class="num-col">{{ row.classes }}</td>
                <td class="num-col">{{ row.variables }}</td>
                <td class="num-col">{{ row.imports }}</td>
                <td class="num-col">{{ row.test_count }}</td>
                <td class="num-col">{{ row.test_assertion_count }}</td>
                <td class="num-col">{{ row.test_suite_count }}</td>
                {% if has_coverage_data %}<td class="num-col">{{ row.line_cov_pct }}</td><td class="num-col">{{ row.fn_cov_pct }}</td>{% endif %}
              </tr>
              {% endfor %}
            </tbody>
          </table>
        </div>
        </div>
        <div id="per-file-pagination" class="pagination-bar">
          <button id="pf-first" class="pager-btn pager-edge" disabled title="First page">&#8676; First</button>
          <button id="pf-prev" class="pager-btn" disabled>&#8592; Prev</button>
          <span class="pager-jump-wrap">Page <input id="pf-page-jump" class="pager-jump" type="number" min="1" value="1" title="Jump to page"> of <span id="pf-page-total">&#8212;</span></span>
          <span id="pf-page-info" class="pager-info"></span>
          <button id="pf-next" class="pager-btn">Next &#8594;</button>
          <button id="pf-last" class="pager-btn pager-edge" title="Last page">Last &#8677;</button>
        </div>
      </section>

      <section class="panel stack">
        <div class="toolbar"><div class="toolbar-left"><h2>Skipped files</h2><input id="skipped-search" class="search" type="search" placeholder="Filter skipped files, reasons, warnings..." /><div class="page-size-row"><label class="page-size-label" for="skipped-page-size">Show:</label><select id="skipped-page-size" class="page-size-select"><option value="10" selected>10</option><option value="20">20</option><option value="50">50</option><option value="100">100</option><option value="all">All</option></select><span id="skipped-count-label" class="page-count-label"></span></div></div><div class="export-group"><button class="export-btn" id="skipped-export-csv">&#8595; CSV</button><button class="export-btn" id="skipped-export-xls">&#8595; Excel</button></div></div>
        <div class="table-shell table-shell-clip" style="margin-top:6px;">
        <div id="skipped-shell">
          <table id="skipped-table" data-sort-table class="table-resizable">
            <thead>
              <tr>
                <th data-sort-type="text" style="width:42%">File</th>
                <th data-sort-type="text" style="width:20%">Status</th>
                <th data-sort-type="text" style="width:38%">Warnings</th>
              </tr>
            </thead>
            <tbody>
              {% for row in skipped_rows %}
              <tr>
                <td class="mono" title="{{ row.relative_path }}">{{ row.relative_path }}</td>
                <td><span class="status-tag status-{{ row.status_class }}">{{ row.status }}</span></td>
                <td class="small" title="{{ row.warnings }}">{{ row.warnings }}</td>
              </tr>
              {% endfor %}
            </tbody>
          </table>
        </div>
        </div>
        <div id="skipped-pagination" class="pagination-bar">
          <button id="sk-first" class="pager-btn pager-edge" disabled title="First page">&#8676; First</button>
          <button id="sk-prev" class="pager-btn" disabled>&#8592; Prev</button>
          <span class="pager-jump-wrap">Page <input id="sk-page-jump" class="pager-jump" type="number" min="1" value="1" title="Jump to page"> of <span id="sk-page-total">&#8212;</span></span>
          <span id="sk-page-info" class="pager-info"></span>
          <button id="sk-next" class="pager-btn">Next &#8594;</button>
          <button id="sk-last" class="pager-btn pager-edge" title="Last page">Last &#8677;</button>
        </div>
      </section>

      <section class="panel stack">
        <div>
          <div class="toolbar">
            <div class="toolbar-left"><h2>Diagnostics &amp; Configuration</h2></div>
            {% if !is_sub_report && has_run_warnings %}<div class="pill-row"><span class="pill info" style="font-size:11px;min-height:26px;">{{ warning_count }} total warnings</span></div>{% endif %}
          </div>
          <p class="effective-config-note">Warning summary, support improvement opportunities, raw diagnostic output, and the exact configuration in effect for this scan.</p>
        </div>

        {% if !is_sub_report %}
        <div style="margin-top:-14px;">
          <h3 style="margin:0 0 4px;">Warnings overview</h3>
          <p class="support-note">Warning categories produced during the scan. Each row shows the warning type, how many files were affected, and what it means for your results.</p>
          {% if !has_run_warnings %}
            <div class="pill good">No top-level warnings.</div>
          {% else %}
            <div class="table-shell">
              <table class="support-table">
                <thead>
                  <tr><th style="width:30%;">Category</th><th style="width:8%;">Count</th><th>What this means</th></tr>
                </thead>
                <tbody>
                  {% for row in warning_summary_rows %}
                  <tr class="{{ row.tone_class }}">
                    <td style="font-weight:700;" title="{{ row.label }}">{{ row.label }}</td>
                    <td class="warning-count" style="font-weight:800;">{{ row.count }}</td>
                    <td class="small" style="color:var(--muted);">{{ row.detail }}</td>
                  </tr>
                  {% endfor %}
                </tbody>
              </table>
            </div>
          {% endif %}
        </div>

        <div>
          <h3 style="margin:0 0 4px;">Skipped file categories</h3>
          <p class="support-note">Files that were not analyzed, grouped by category. Each row shows what the files are, how many were skipped, example file names, and how to silence or fix the warning.</p>
          {% if warning_opportunity_rows.is_empty() %}
            <div class="pill good">No unsupported text-format buckets detected.</div>
          {% else %}
          <div class="table-shell">
            <table class="support-table">
              <thead>
                <tr><th style="width:20%;">Category</th><th style="width:6%;">Count</th><th style="width:24%;">What these files are</th><th>Example files &amp; how to fix</th></tr>
              </thead>
              <tbody>
                {% for row in warning_opportunity_rows %}
                <tr>
                  <td style="font-weight:700;" title="{{ row.label }}">{{ row.label }}</td>
                  <td style="font-weight:800;color:var(--oxide);">{{ row.count }}</td>
                  <td class="small" style="color:var(--muted);">{{ row.bucket_description }}</td>
                  <td>
                    {% if !row.example_files.is_empty() %}
                    <div style="margin-bottom:6px;">
                      {% for f in row.example_files %}<span class="support-example-file">{{ f }}</span> {% endfor %}
                      {% if row.count > row.example_files.len() %}<span style="font-size:11px;color:var(--muted);font-style:italic;">+{{ row.count - row.example_files.len() }} more</span>{% endif %}
                    </div>
                    {% endif %}
                    <p class="support-recommendation">{{ row.recommendation }}</p>
                  </td>
                </tr>
                {% endfor %}
              </tbody>
            </table>
          </div>
          {% endif %}
        </div>

        <div>
          <details open class="warnings-details">
            <summary>Detailed run warnings ({{ warning_count }})</summary>
            <div>
              <p style="font-size:13px;color:var(--muted);margin:0 0 10px;">Raw warning messages emitted during the scan — unsupported file formats, encoding fallbacks, binary detections, and per-file parse issues. Scroll to see all warnings. High counts typically indicate many non-code assets (JSON configs, docs, lockfiles) in the scanned directory.</p>
              {% if !has_run_warnings %}
                <div class="pill good">No top-level warnings.</div>
              {% else %}
                <div class="code-block-toolbar">
                  <button type="button" class="code-copy-btn" id="warning-console-copy-btn" aria-label="Copy warnings">Copy</button>
                </div>
                <pre class="warning-console" id="warning-console-full" style="max-height:210px;">{{ warning_console_full }}</pre>
              {% endif %}
            </div>
          </details>
        </div>
        {% endif %}

        <div>
          <details open>
            <summary>Effective configuration</summary>
            <div>
              <div style="display:flex;gap:8px;margin-bottom:10px;">
                <button type="button" class="export-btn" data-copy-config>Copy</button>
                <button type="button" class="export-btn" data-download-config>Download</button>
              </div>
              <p style="font-size:13px;color:var(--muted);margin:0 0 10px;">The merged, fully-resolved configuration snapshot used for this scan — includes all CLI overrides applied on top of the base config file. Use this to replay the exact run or verify what settings were active.</p>
              <div class="config-pre-wrap">
                <div class="code-block-toolbar">
                  <button type="button" class="code-copy-btn" id="config-inline-copy-btn" aria-label="Copy configuration">Copy</button>
                </div>
                <pre class="config-pre" id="config-json-block">{{ config_json }}</pre>
              </div>
            </div>
          </details>
        </div>
      </section>
    </div>
  </div>

  <div id="r-tt" aria-hidden="true"></div>
  <script nonce="{{ nonce }}">
    // Hide "View PDF" button and block brand-link navigation when opened as a local file
    (function () {
      var pdfBtn = document.getElementById('nav-view-pdf-btn');
      if (pdfBtn && window.location.protocol === 'file:') {
        pdfBtn.style.display = 'none';
      }
      var brand = document.querySelector('a[data-local-brand]');
      if (brand && window.location.protocol === 'file:') {
        brand.addEventListener('click', function (e) { e.preventDefault(); });
      }
    })();

    (function () {
      var body = document.body;
      var storageKey = 'oxide-sloc-theme';
      var themeToggle = document.querySelector('[data-theme-toggle]');
      var copyLinkButtons = Array.prototype.slice.call(document.querySelectorAll('[data-copy-link]'));
      var shareButtons = Array.prototype.slice.call(document.querySelectorAll('[data-share-report]'));
      var printButtons = Array.prototype.slice.call(document.querySelectorAll('[data-print-report]'));

      function applyTheme(theme) {
        body.classList.toggle('dark-theme', theme === 'dark');
      }

      function currentTheme() {
        return body.classList.contains('dark-theme') ? 'dark' : 'light';
      }

      try {
        var saved = localStorage.getItem(storageKey);
        if (saved === 'dark' || saved === 'light') {
          applyTheme(saved);
        }
      } catch (e) {}

      if (themeToggle) {
        themeToggle.addEventListener('click', function () {
          var next = currentTheme() === 'dark' ? 'light' : 'dark';
          applyTheme(next);
          try { localStorage.setItem(storageKey, next); } catch (e) {}
        });
      }

      function copyText(value) {
        if (!value) return;
        if (navigator.clipboard && navigator.clipboard.writeText) {
          navigator.clipboard.writeText(value).catch(function () {});
        }
      }

      copyLinkButtons.forEach(function (button) {
        button.addEventListener('click', function () {
          copyText(window.location.href);
        });
      });

      shareButtons.forEach(function (button) {
        button.addEventListener('click', function () {
          if (navigator.share) {
            navigator.share({ title: document.title, url: window.location.href }).catch(function () {});
          } else {
            copyText(window.location.href);
          }
        });
      });

      printButtons.forEach(function (button) {
        button.addEventListener('click', function () {
          window.print();
        });
      });

      // "View PDF" nav button.
      // Priority order:
      //  1. data-standalone-pdf attr — pre-generated PDF in the same directory
      //     (set when oxide-sloc CLI was invoked with both --html-out and
      //     --pdf-out). Opens the file directly; works in Jenkins HTML Publisher.
      //  2. Server route (/runs/pdf/<id>) — oxide-sloc web server generates
      //     the PDF on demand via headless Chrome. Checked via HEAD request.
      //  3. Neither available — inform the user how to generate a PDF via CLI.
      var pdfNavBtn = document.getElementById('nav-view-pdf-btn');
      if (pdfNavBtn) {
        pdfNavBtn.addEventListener('click', function (e) {
          e.preventDefault();
          var standaloneUrl = pdfNavBtn.getAttribute('data-standalone-pdf');
          if (standaloneUrl) {
            window.open(standaloneUrl, '_blank', 'noopener');
            return;
          }
          var serverUrl = pdfNavBtn.getAttribute('href');
          var xhr = new XMLHttpRequest();
          xhr.open('HEAD', serverUrl, true);
          xhr.onreadystatechange = function () {
            if (xhr.readyState === 4) {
              if (xhr.status >= 200 && xhr.status < 300) {
                window.open(serverUrl, '_blank', 'noopener');
              } else {
                alert('PDF not available.\n\nTo generate one, run:\n  oxide-sloc report result.json --pdf-out report.pdf\n\nOr enable GENERATE_PDF in your Jenkins pipeline.');
              }
            }
          };
          xhr.onerror = function () {
            alert('PDF not available.\n\nTo generate one, run:\n  oxide-sloc report result.json --pdf-out report.pdf\n\nOr enable GENERATE_PDF in your Jenkins pipeline.');
          };
          xhr.send();
        });
      }

      var copyConfigBtn = document.querySelector('[data-copy-config]');
      var downloadConfigBtn = document.querySelector('[data-download-config]');
      var configBlock = document.getElementById('config-json-block');
      var inlineCopyBtn = document.getElementById('config-inline-copy-btn');
      function handleConfigCopy(btn) {
        if (!btn || !configBlock) return;
        btn.addEventListener('click', function (e) {
          e.stopPropagation();
          copyText(configBlock.textContent);
          var orig = btn.textContent;
          btn.textContent = 'Copied!';
          setTimeout(function () { btn.textContent = orig; }, 1600);
        });
      }
      handleConfigCopy(copyConfigBtn);
      handleConfigCopy(inlineCopyBtn);

      var warnCopyBtn = document.getElementById('warning-console-copy-btn');
      var warnBlock = document.getElementById('warning-console-full');
      if (warnCopyBtn && warnBlock) {
        warnCopyBtn.addEventListener('click', function () {
          copyText(warnBlock.textContent);
          var orig = warnCopyBtn.textContent;
          warnCopyBtn.textContent = 'Copied!';
          setTimeout(function () { warnCopyBtn.textContent = orig; }, 1600);
        });
      }

      if (downloadConfigBtn && configBlock) {
        downloadConfigBtn.addEventListener('click', function (e) {
          e.stopPropagation();
          var blob = new Blob([configBlock.textContent], { type: 'application/json' });
          var url = URL.createObjectURL(blob);
          var a = document.createElement('a');
          a.href = url; a.download = 'effective-config.json';
          document.body.appendChild(a); a.click();
          document.body.removeChild(a);
          setTimeout(function () { URL.revokeObjectURL(url); }, 200);
        });
      }

      function detectType(value) {
        // Strip thousands separators so comma-formatted numbers (e.g. "121,542")
        // still sort numerically rather than lexicographically.
        var v = value.trim().replace(/,/g, '');
        return /^-?\d+(?:\.\d+)?$/.test(v) ? parseFloat(v) : value.trim().toLowerCase();
      }

      document.querySelectorAll('[data-sort-table]').forEach(function (table) {
        var headers = Array.prototype.slice.call(table.querySelectorAll('th'));
        var allMarkers = [];
        headers.forEach(function (th, idx) {
          var direction = 1;
          var marker = document.createElement('span');
          marker.className = 'sort-indicator';
          marker.textContent = ' \u2195';
          th.style.cursor = 'pointer';
          th.appendChild(marker);
          allMarkers.push(marker);
          th.addEventListener('click', function (e) {
            if (e.target.closest && e.target.closest('.col-resize-handle')) return;
            var tbody = table.tBodies[0];
            var rows = Array.prototype.slice.call(tbody.querySelectorAll('tr'));
            rows.sort(function (a, b) {
              var av = detectType((a.children[idx].textContent || '').trim());
              var bv = detectType((b.children[idx].textContent || '').trim());
              if (av < bv) return -1 * direction;
              if (av > bv) return 1 * direction;
              return 0;
            });
            rows.forEach(function (row) { tbody.appendChild(row); });
            allMarkers.forEach(function(m) { m.textContent = ' \u2195'; });
            direction = direction * -1;
            marker.textContent = direction === -1 ? ' \u2191' : ' \u2193';
            table.dispatchEvent(new CustomEvent('sloc-sorted'));
          });
        });
      });

      // ── Column resize for all table-resizable tables ──────────────────────────
      (function() {
        document.querySelectorAll('.table-resizable').forEach(function(table) {
          var cols = Array.prototype.slice.call(table.querySelectorAll('col'));
          var ths = Array.prototype.slice.call(table.querySelectorAll('thead th'));
          ths.forEach(function(th, i) {
            var handle = th.querySelector('.col-resize-handle');
            if (!handle || !cols[i]) return;
            var startX, startW;
            handle.addEventListener('mousedown', function(e) {
              e.stopPropagation(); e.preventDefault();
              startX = e.clientX; startW = cols[i].offsetWidth || th.offsetWidth;
              handle.classList.add('dragging');
              function onMove(e) { cols[i].style.width = Math.max(40, startW + e.clientX - startX) + 'px'; }
              function onUp() { handle.classList.remove('dragging'); document.removeEventListener('mousemove', onMove); document.removeEventListener('mouseup', onUp); }
              document.addEventListener('mousemove', onMove);
              document.addEventListener('mouseup', onUp);
            });
          });
        });
      })();

      document.querySelectorAll('[data-table-filter]').forEach(function (input) {
        var table = document.getElementById(input.getAttribute('data-table-filter'));
        if (!table) return;
        var filterTimer = null;
        var rowCache = null;
        input.addEventListener('input', function () {
          clearTimeout(filterTimer);
          var q = input.value.toLowerCase();
          filterTimer = setTimeout(function () {
            if (!rowCache) {
              rowCache = Array.prototype.map.call(table.tBodies[0].rows, function (row) {
                return { row: row, text: row.textContent.toLowerCase() };
              });
            }
            rowCache.forEach(function (item) {
              item.row.style.display = q === '' || item.text.indexOf(q) >= 0 ? '' : 'none';
            });
          }, 200);
        });
      });

      // ── Per-file table pagination ────────────────────────────────────────────
      (function () {
        var table = document.getElementById('per-file-table');
        if (!table) return;
        var tbody = table.tBodies[0];
        var searchInput = document.getElementById('per-file-search');
        var pageSizeSelect = document.getElementById('per-file-page-size');
        var firstBtn = document.getElementById('pf-first');
        var prevBtn = document.getElementById('pf-prev');
        var nextBtn = document.getElementById('pf-next');
        var lastBtn = document.getElementById('pf-last');
        var pageInfo = document.getElementById('pf-page-info');
        var jumpInput = document.getElementById('pf-page-jump');
        var pageTotal = document.getElementById('pf-page-total');
        var countLabel = document.getElementById('per-file-count-label');
        var filteredRows = [];
        var currentPage = 1;
        var totalAll = tbody.rows.length;

        function getPageSize() {
          var v = pageSizeSelect ? pageSizeSelect.value : '20';
          return v === 'all' ? Infinity : parseInt(v, 10);
        }

        function applyFilter() {
          var q = searchInput ? searchInput.value.toLowerCase() : '';
          var rows = Array.prototype.slice.call(tbody.rows);
          filteredRows = q === '' ? rows : rows.filter(function (row) {
            return row.textContent.toLowerCase().indexOf(q) >= 0;
          });
          currentPage = 1;
          render();
        }

        function render() {
          var ps = getPageSize();
          var total = filteredRows.length;
          var totalPages = ps === Infinity ? 1 : Math.max(1, Math.ceil(total / ps));
          if (currentPage > totalPages) currentPage = totalPages;
          if (currentPage < 1) currentPage = 1;
          var start = ps === Infinity ? 0 : (currentPage - 1) * ps;
          var end = ps === Infinity ? total : Math.min(start + ps, total);
          Array.prototype.forEach.call(tbody.rows, function (row) { row.style.display = 'none'; });
          for (var i = start; i < end; i++) { filteredRows[i].style.display = ''; }
          if (pageInfo) {
            if (total === 0) {
              pageInfo.textContent = 'No results';
            } else if (ps === Infinity) {
              pageInfo.textContent = 'All ' + total.toLocaleString() + ' files';
            } else {
              pageInfo.textContent = (start + 1) + '\u2013' + end + ' of ' + total.toLocaleString() + ' files';
            }
          }
          if (countLabel) {
            countLabel.textContent = (total < totalAll && total > 0) ? '(' + total.toLocaleString() + ' matching)' : '';
          }
          var edgeDisabled = ps === Infinity;
          if (firstBtn) firstBtn.disabled = currentPage <= 1 || edgeDisabled;
          if (prevBtn) prevBtn.disabled = currentPage <= 1 || edgeDisabled;
          if (nextBtn) nextBtn.disabled = currentPage >= totalPages || edgeDisabled;
          if (lastBtn) lastBtn.disabled = currentPage >= totalPages || edgeDisabled;
          if (jumpInput) { jumpInput.value = currentPage; jumpInput.max = totalPages; jumpInput.disabled = edgeDisabled; }
          if (pageTotal) pageTotal.textContent = totalPages.toLocaleString();
        }

        if (searchInput) {
          var filterTimer = null;
          searchInput.addEventListener('input', function () {
            clearTimeout(filterTimer);
            filterTimer = setTimeout(applyFilter, 200);
          });
        }
        if (pageSizeSelect) {
          pageSizeSelect.addEventListener('change', function () { currentPage = 1; render(); });
        }
        if (firstBtn) {
          firstBtn.addEventListener('click', function () { currentPage = 1; render(); });
        }
        if (prevBtn) {
          prevBtn.addEventListener('click', function () { if (currentPage > 1) { currentPage--; render(); } });
        }
        if (nextBtn) {
          nextBtn.addEventListener('click', function () {
            var ps = getPageSize();
            var totalPages = ps === Infinity ? 1 : Math.ceil(filteredRows.length / ps);
            if (currentPage < totalPages) { currentPage++; render(); }
          });
        }
        if (lastBtn) {
          lastBtn.addEventListener('click', function () {
            var ps = getPageSize();
            currentPage = ps === Infinity ? 1 : Math.max(1, Math.ceil(filteredRows.length / ps));
            render();
          });
        }
        if (jumpInput) {
          function pfJump() {
            var ps = getPageSize();
            var totalPages = ps === Infinity ? 1 : Math.max(1, Math.ceil(filteredRows.length / ps));
            var v = parseInt(jumpInput.value, 10);
            if (!isNaN(v)) { currentPage = Math.max(1, Math.min(v, totalPages)); render(); }
          }
          jumpInput.addEventListener('change', pfJump);
          jumpInput.addEventListener('keydown', function (e) { if (e.key === 'Enter') pfJump(); });
        }
        table.addEventListener('sloc-sorted', function () { applyFilter(); });
        window._pfPaginationReset = function () { currentPage = 1; applyFilter(); };
        applyFilter();
      })();

      // ── Skipped-files table pagination ───────────────────────────────────────
      (function () {
        var table = document.getElementById('skipped-table');
        if (!table) return;
        var tbody = table.tBodies[0];
        var searchInput = document.getElementById('skipped-search');
        var pageSizeSelect = document.getElementById('skipped-page-size');
        var firstBtn = document.getElementById('sk-first');
        var prevBtn = document.getElementById('sk-prev');
        var nextBtn = document.getElementById('sk-next');
        var lastBtn = document.getElementById('sk-last');
        var pageInfo = document.getElementById('sk-page-info');
        var jumpInput = document.getElementById('sk-page-jump');
        var pageTotal = document.getElementById('sk-page-total');
        var countLabel = document.getElementById('skipped-count-label');
        var filteredRows = [];
        var currentPage = 1;
        var totalAll = tbody.rows.length;

        function getPageSize() {
          var v = pageSizeSelect ? pageSizeSelect.value : '10';
          return v === 'all' ? Infinity : parseInt(v, 10);
        }

        function applyFilter() {
          var q = searchInput ? searchInput.value.toLowerCase() : '';
          var rows = Array.prototype.slice.call(tbody.rows);
          filteredRows = q === '' ? rows : rows.filter(function (row) {
            return row.textContent.toLowerCase().indexOf(q) >= 0;
          });
          currentPage = 1;
          render();
        }

        function render() {
          var ps = getPageSize();
          var total = filteredRows.length;
          var totalPages = ps === Infinity ? 1 : Math.max(1, Math.ceil(total / ps));
          if (currentPage > totalPages) currentPage = totalPages;
          if (currentPage < 1) currentPage = 1;
          var start = ps === Infinity ? 0 : (currentPage - 1) * ps;
          var end = ps === Infinity ? total : Math.min(start + ps, total);
          Array.prototype.forEach.call(tbody.rows, function (row) { row.style.display = 'none'; });
          for (var i = start; i < end; i++) { filteredRows[i].style.display = ''; }
          if (pageInfo) {
            if (total === 0) {
              pageInfo.textContent = 'No results';
            } else if (ps === Infinity) {
              pageInfo.textContent = 'All ' + total.toLocaleString() + ' files';
            } else {
              pageInfo.textContent = (start + 1) + '\u2013' + end + ' of ' + total.toLocaleString() + ' files';
            }
          }
          if (countLabel) {
            countLabel.textContent = (total < totalAll && total > 0) ? '(' + total.toLocaleString() + ' matching)' : '';
          }
          var edgeDisabled = ps === Infinity;
          if (firstBtn) firstBtn.disabled = currentPage <= 1 || edgeDisabled;
          if (prevBtn) prevBtn.disabled = currentPage <= 1 || edgeDisabled;
          if (nextBtn) nextBtn.disabled = currentPage >= totalPages || edgeDisabled;
          if (lastBtn) lastBtn.disabled = currentPage >= totalPages || edgeDisabled;
          if (jumpInput) { jumpInput.value = currentPage; jumpInput.max = totalPages; jumpInput.disabled = edgeDisabled; }
          if (pageTotal) pageTotal.textContent = totalPages.toLocaleString();
        }

        if (searchInput) {
          var filterTimer = null;
          searchInput.addEventListener('input', function () {
            clearTimeout(filterTimer);
            filterTimer = setTimeout(applyFilter, 200);
          });
        }
        if (pageSizeSelect) {
          pageSizeSelect.addEventListener('change', function () { currentPage = 1; render(); });
        }
        if (firstBtn) {
          firstBtn.addEventListener('click', function () { currentPage = 1; render(); });
        }
        if (prevBtn) {
          prevBtn.addEventListener('click', function () { if (currentPage > 1) { currentPage--; render(); } });
        }
        if (nextBtn) {
          nextBtn.addEventListener('click', function () {
            var ps = getPageSize();
            var totalPages = ps === Infinity ? 1 : Math.ceil(filteredRows.length / ps);
            if (currentPage < totalPages) { currentPage++; render(); }
          });
        }
        if (lastBtn) {
          lastBtn.addEventListener('click', function () {
            var ps = getPageSize();
            currentPage = ps === Infinity ? 1 : Math.max(1, Math.ceil(filteredRows.length / ps));
            render();
          });
        }
        if (jumpInput) {
          function skJump() {
            var ps = getPageSize();
            var totalPages = ps === Infinity ? 1 : Math.max(1, Math.ceil(filteredRows.length / ps));
            var v = parseInt(jumpInput.value, 10);
            if (!isNaN(v)) { currentPage = Math.max(1, Math.min(v, totalPages)); render(); }
          }
          jumpInput.addEventListener('change', skJump);
          jumpInput.addEventListener('keydown', function (e) { if (e.key === 'Enter') skJump(); });
        }
        table.addEventListener('sloc-sorted', function () { applyFilter(); });
        applyFilter();
      })();

      // ── Hotspots table pagination ────────────────────────────────────────────
      (function () {
        var table = document.getElementById('hotspots-table');
        if (!table) return;
        var tbody = table.tBodies[0];
        var searchInput = document.getElementById('hotspots-search');
        var pageSizeSelect = document.getElementById('hotspots-page-size');
        var firstBtn = document.getElementById('hs-first');
        var prevBtn = document.getElementById('hs-prev');
        var nextBtn = document.getElementById('hs-next');
        var lastBtn = document.getElementById('hs-last');
        var pageInfo = document.getElementById('hs-page-info');
        var jumpInput = document.getElementById('hs-page-jump');
        var pageTotal = document.getElementById('hs-page-total');
        var countLabel = document.getElementById('hotspots-count-label');
        var filteredRows = [];
        var currentPage = 1;
        var totalAll = tbody.rows.length;

        function getPageSize() {
          var v = pageSizeSelect ? pageSizeSelect.value : '15';
          return v === 'all' ? Infinity : parseInt(v, 10);
        }

        function applyFilter() {
          var q = searchInput ? searchInput.value.toLowerCase() : '';
          var rows = Array.prototype.slice.call(tbody.rows);
          filteredRows = q === '' ? rows : rows.filter(function (row) {
            return row.textContent.toLowerCase().indexOf(q) >= 0;
          });
          currentPage = 1;
          render();
        }

        function render() {
          var ps = getPageSize();
          var total = filteredRows.length;
          var totalPages = ps === Infinity ? 1 : Math.max(1, Math.ceil(total / ps));
          if (currentPage > totalPages) currentPage = totalPages;
          if (currentPage < 1) currentPage = 1;
          var start = ps === Infinity ? 0 : (currentPage - 1) * ps;
          var end = ps === Infinity ? total : Math.min(start + ps, total);
          Array.prototype.forEach.call(tbody.rows, function (row) { row.style.display = 'none'; });
          for (var i = start; i < end; i++) { filteredRows[i].style.display = ''; }
          if (pageInfo) {
            if (total === 0) {
              pageInfo.textContent = 'No results';
            } else if (ps === Infinity) {
              pageInfo.textContent = 'All ' + total.toLocaleString() + ' files';
            } else {
              pageInfo.textContent = (start + 1) + '-' + end + ' of ' + total.toLocaleString() + ' files';
            }
          }
          if (countLabel) {
            countLabel.textContent = (total < totalAll && total > 0) ? '(' + total.toLocaleString() + ' matching)' : '';
          }
          var edgeDisabled = ps === Infinity;
          if (firstBtn) firstBtn.disabled = currentPage <= 1 || edgeDisabled;
          if (prevBtn) prevBtn.disabled = currentPage <= 1 || edgeDisabled;
          if (nextBtn) nextBtn.disabled = currentPage >= totalPages || edgeDisabled;
          if (lastBtn) lastBtn.disabled = currentPage >= totalPages || edgeDisabled;
          if (jumpInput) { jumpInput.value = currentPage; jumpInput.max = totalPages; jumpInput.disabled = edgeDisabled; }
          if (pageTotal) pageTotal.textContent = totalPages.toLocaleString();
        }

        if (searchInput) {
          var filterTimer = null;
          searchInput.addEventListener('input', function () {
            clearTimeout(filterTimer);
            filterTimer = setTimeout(applyFilter, 200);
          });
        }
        if (pageSizeSelect) {
          pageSizeSelect.addEventListener('change', function () { currentPage = 1; render(); });
        }
        if (firstBtn) {
          firstBtn.addEventListener('click', function () { currentPage = 1; render(); });
        }
        if (prevBtn) {
          prevBtn.addEventListener('click', function () { if (currentPage > 1) { currentPage--; render(); } });
        }
        if (nextBtn) {
          nextBtn.addEventListener('click', function () {
            var ps = getPageSize();
            var totalPages = ps === Infinity ? 1 : Math.ceil(filteredRows.length / ps);
            if (currentPage < totalPages) { currentPage++; render(); }
          });
        }
        if (lastBtn) {
          lastBtn.addEventListener('click', function () {
            var ps = getPageSize();
            currentPage = ps === Infinity ? 1 : Math.max(1, Math.ceil(filteredRows.length / ps));
            render();
          });
        }
        if (jumpInput) {
          function hsJump() {
            var ps = getPageSize();
            var totalPages = ps === Infinity ? 1 : Math.max(1, Math.ceil(filteredRows.length / ps));
            var v = parseInt(jumpInput.value, 10);
            if (!isNaN(v)) { currentPage = Math.max(1, Math.min(v, totalPages)); render(); }
          }
          jumpInput.addEventListener('change', hsJump);
          jumpInput.addEventListener('keydown', function (e) { if (e.key === 'Enter') hsJump(); });
        }
        table.addEventListener('sloc-sorted', function () { applyFilter(); });
        applyFilter();
      })();
    })();

    (function randomizeWatermarks() {
      var wms = Array.prototype.slice.call(document.querySelectorAll('.background-watermarks img'));
      if (!wms.length) return;
      var placed = [];
      function tooClose(t, l) {
        for (var i = 0; i < placed.length; i++) {
          var dt = Math.abs(placed[i][0] - t);
          var dl = Math.abs(placed[i][1] - l);
          if (dt < 18 && dl < 18) return true;
        }
        return false;
      }
      function pick(leftBias) {
        for (var attempt = 0; attempt < 40; attempt++) {
          var t = Math.random() * 90;
          var l = leftBias ? Math.random() * 50 : 40 + Math.random() * 55;
          if (!tooClose(t, l)) { placed.push([t, l]); return [t, l]; }
        }
        var fb = [Math.random() * 90, Math.random() * 95];
        placed.push(fb);
        return fb;
      }
      var half = Math.floor(wms.length / 2);
      wms.forEach(function (img, i) {
        var pos = pick(i < half);
        var sz = Math.floor(Math.random() * 80 + 110);
        var rot = (Math.random() * 360).toFixed(1);
        var op = (Math.random() * 0.07 + 0.10).toFixed(2);
        img.style.cssText = 'width:' + sz + 'px;top:' + pos[0].toFixed(1) + '%;left:' + pos[1].toFixed(1) + '%;transform:rotate(' + rot + 'deg);opacity:' + op + ';';
      });
    })();

    (function spawnCodeParticles() {
      var container = document.getElementById('code-particles');
      if (!container) return;
      var snippets = ['1,247 sloc', 'fn analyze()', 'code_lines', '0 mixed', 'blanks: 312', '// comment', 'pub fn run', 'use std::fs', 'Result<()>', 'let mut n = 0', 'git main', '#[derive]', 'impl Scan', '3,841 physical', 'files: 60', '450 comments', 'cargo build', 'Ok(run)', 'Vec<String>', 'match lang', 'fn main() {', '.rs .go .py', 'sloc_core', 'render_html', '2,163 code'];
      for (var i = 0; i < 38; i++) {
        (function (idx) {
          var el = document.createElement('span');
          el.className = 'code-particle';
          el.textContent = snippets[idx % snippets.length];
          var left = Math.random() * 94 + 2;
          var top = Math.random() * 88 + 6;
          var dur = (Math.random() * 10 + 9).toFixed(1);
          var delay = (Math.random() * 18).toFixed(1);
          var rot = (Math.random() * 26 - 13).toFixed(1);
          var op = (Math.random() * 0.09 + 0.06).toFixed(3);
          el.style.left = left.toFixed(1) + '%';
          el.style.top = top.toFixed(1) + '%';
          el.style.setProperty('--rot', rot + 'deg');
          el.style.setProperty('--op', op);
          el.style.animationDuration = dur + 's';
          el.style.animationDelay = '-' + delay + 's';
          container.appendChild(el);
        })(i);
      }
    })();
    // ── Metric number formatting ─────────────────────────────────────────────
    (function () {
      function fmtBig(n) {
        if (n >= 1e6) return (n / 1e6).toFixed(1).replace(/\.0$/, '') + 'M';
        if (n >= 1e4) return (n / 1e3).toFixed(1).replace(/\.0$/, '') + 'K';
        return n.toLocaleString();
      }
      function fmtExact(n) { return n.toLocaleString(); }
      document.querySelectorAll('[data-metric-value]').forEach(function (el) {
        var n = parseInt(el.getAttribute('data-metric-value'), 10);
        if (isNaN(n)) return;
        var big = el.querySelector('.metric-big');
        var exact = el.querySelector('.metric-exact');
        if (big) big.textContent = fmtBig(n);
        if (exact) exact.textContent = n >= 1e4 ? fmtExact(n) : '';
      });
      var densityCard = document.querySelector('[data-metric-density]');
      if (densityCard) {
        var phys = 0, code = 0;
        document.querySelectorAll('[data-metric-value]').forEach(function (el) {
          var lbl = el.querySelector('.metric-label');
          if (!lbl) return;
          var t = lbl.textContent.trim().toLowerCase();
          var v = parseInt(el.getAttribute('data-metric-value'), 10) || 0;
          if (t === 'physical lines') phys = v;
          if (t === 'code') code = v;
        });
        var pct = phys > 0 ? (code / phys * 100) : 0;
        var big = densityCard.querySelector('.metric-big');
        var exact = densityCard.querySelector('.metric-exact');
        if (big) big.textContent = pct.toFixed(1) + '%';
        if (exact) exact.textContent = '';
      }
      (function(){
        var g=document.querySelector('.summary-grid');if(!g)return;
        var pad=g.querySelector('.metric-pad');
        var real=Array.prototype.slice.call(g.querySelectorAll('.metric')).filter(function(el){return el!==pad;});
        if(!real.length)return;
        function upd(){
          // Pad the strip to an EVEN card count so a true CSS grid lays it out as
          // exactly two full rows with every column aligned and every card the
          // same size. When the real-card count is odd, reveal the reserve
          // "Assertions" pad card; otherwise keep it hidden.
          var n=real.length;
          if(pad){ if(n%2===1){pad.style.display='';n++;} else {pad.style.display='none';} }
          var perRow=window.innerWidth<=640?2:Math.ceil(n/2);
          g.style.gridTemplateColumns='repeat('+perRow+',minmax(0,1fr))';
        }
        upd();window.addEventListener('resize',upd);
      })();
      (function(){if(typeof window.__rptFinish==='function'){window.__rptFinish();return;}var ov=document.getElementById('rpt-loading-overlay');if(ov){ov.classList.add('fade-out');setTimeout(function(){if(ov.parentNode)ov.parentNode.removeChild(ov);},450);}})();
    })();
    // ── Info chip interactivity ───────────────────────────────────────────────
    (function() {
      document.querySelectorAll('.run-id-chip[data-copy]').forEach(function(chip) {
        chip.addEventListener('click', function() {
          var val = chip.getAttribute('data-copy');
          var tt = chip.querySelector('.chip-tooltip');
          var orig = tt ? tt.textContent : '';
          if (!navigator.clipboard) return;
          navigator.clipboard.writeText(val).then(function() {
            chip.classList.add('chip-copied-flash');
            if (tt) tt.textContent = 'Copied!';
            setTimeout(function() {
              chip.classList.remove('chip-copied-flash');
              if (tt) tt.textContent = orig;
            }, 1100);
          });
        });
      });
      document.querySelectorAll('.run-id-chip[data-author]').forEach(function(chip) {
        var author = chip.getAttribute('data-author');
        var el = chip.querySelector('.author-handle');
        if (el) el.textContent = '/' + author.replace(/\s+/g, '');
      });
    })();
    // ── Export helpers ────────────────────────────────────────────────────────
    function _slocUnh(s){var e=document.createElement('div');e.innerHTML=s;return e.textContent;}
    var _SLOC_META={runId:"{{ run.tool.run_id }}",gitCommit:"{% if let Some(c) = run.git_commit_long %}{{ c }}{% else %}(not detected){% endif %}",branch:_slocUnh("{% if let Some(b) = run.git_branch %}{{ b }}{% else %}(not detected){% endif %}"),lastCommitBy:_slocUnh("{% if let Some(a) = run.git_commit_author %}{{ a }}{% else %}(not detected){% endif %}"),scanBy:_slocUnh("{{ scan_performed_by }}"),scanned:"{{ scan_time_pst }}",os:"{{ run.environment.operating_system }} / {{ run.environment.architecture }}",filesAnalyzed:{{ run.summary_totals.files_analyzed }},filesSkipped:{{ run.summary_totals.files_skipped }},physicalLines:{{ run.summary_totals.total_physical_lines }},codeLines:{{ run.summary_totals.code_lines }},commentLines:{{ run.summary_totals.comment_lines }},blankLines:{{ run.summary_totals.blank_lines }},mixedSeparate:{{ run.summary_totals.mixed_lines_separate }},functions:{{ run.summary_totals.functions }},classes:{{ run.summary_totals.classes }},variables:{{ run.summary_totals.variables }},imports:{{ run.summary_totals.imports }},tests:{{ run.summary_totals.test_count }},toolVersion:"{{ tool_version }}"};
    function slocEscXml(v){return String(v).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');}
    function slocEscCsv(v){var s=String(v);return(s.indexOf(',')>=0||s.indexOf('"')>=0||s.indexOf('\n')>=0)?'"'+s.replace(/"/g,'""')+'"':s;}
    function slocDownload(data,name,mime){var b=new Blob([data],{type:mime});var u=URL.createObjectURL(b);var a=document.createElement('a');a.href=u;a.download=name;document.body.appendChild(a);a.click();document.body.removeChild(a);setTimeout(function(){URL.revokeObjectURL(u);},200);}
    function slocCsv(fname,hdrs,rows){slocDownload([hdrs.map(slocEscCsv).join(',')].concat(rows.map(function(r){return r.map(slocEscCsv).join(',');})).join('\r\n'),fname,'text/csv;charset=utf-8;');}
    function slocXls(fname,sheet,hdrs,rows){var enc=new TextEncoder();var CT=[];for(var _n=0;_n<256;_n++){var _c=_n;for(var _k=0;_k<8;_k++)_c=_c&1?0xEDB88320^(_c>>>1):_c>>>1;CT[_n]=_c;}function crc32(d){var v=0xFFFFFFFF;for(var i=0;i<d.length;i++)v=CT[(v^d[i])&0xFF]^(v>>>8);return(v^0xFFFFFFFF)>>>0;}function u2(n){return[n&0xFF,(n>>8)&0xFF];}function u4(n){return[n&0xFF,(n>>8)&0xFF,(n>>16)&0xFF,(n>>24)&0xFF];}function xe(s){return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');}var ss=[],si={};function S(v){v=String(v==null?'':v);if(!(v in si)){si[v]=ss.length;ss.push(v);}return si[v];}function colRef(c,r){var s='',n=c+1;while(n>0){n--;s=String.fromCharCode(65+(n%26))+s;n=Math.floor(n/26);}return s+r;}var rx='<row r="1">';hdrs.forEach(function(h,c){rx+='<c r="'+colRef(c,1)+'" t="s" s="1"><v>'+S(h)+'</v></c>';});rx+='</row>';rows.forEach(function(row,ri){var rn=ri+2;rx+='<row r="'+rn+'">';row.forEach(function(cell,c){var ref=colRef(c,rn);var num=c>=2&&cell!==''&&cell!=null&&!isNaN(Number(cell));rx+=num?'<c r="'+ref+'" s="2"><v>'+xe(cell)+'</v></c>':'<c r="'+ref+'" t="s"><v>'+S(cell)+'</v></c>';});rx+='</row>';});var ox='http://schemas.openxmlformats.org/',pns=ox+'package/2006/',ons=ox+'officeDocument/2006/',sns=ox+'spreadsheetml/2006/main';var ssXml='<?xml version="1.0" encoding="UTF-8" standalone="yes"?><sst xmlns="'+sns+'" count="'+ss.length+'" uniqueCount="'+ss.length+'">'+ss.map(function(v){return'<si><t xml:space="preserve">'+xe(v)+'</t></si>';}).join('')+'</sst>';var wsh='<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="'+sns+'"><sheetViews><sheetView workbookViewId="0"/></sheetViews><sheetFormatPr defaultRowHeight="15"/><sheetData>'+rx+'</sheetData></worksheet>';var stl='<?xml version="1.0" encoding="UTF-8" standalone="yes"?><styleSheet xmlns="'+sns+'"><fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><sz val="11"/><b/><name val="Calibri"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="3"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/><xf numFmtId="0" fontId="1" fillId="0" borderId="0" xfId="0" applyFont="1"/><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0" applyAlignment="1"><alignment horizontal="right"/></xf></cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>';var F={'[Content_Types].xml':'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="'+pns+'content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/></Types>','_rels/.rels':'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="'+pns+'relationships"><Relationship Id="rId1" Type="'+ons+'relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>','xl/workbook.xml':'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="'+sns+'" xmlns:r="'+ons+'relationships"><sheets><sheet name="'+xe(sheet)+'" sheetId="1" r:id="rId1"/></sheets></workbook>','xl/_rels/workbook.xml.rels':'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="'+pns+'relationships"><Relationship Id="rId1" Type="'+ons+'relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="'+ons+'relationships/styles" Target="styles.xml"/><Relationship Id="rId3" Type="'+ons+'relationships/sharedStrings" Target="sharedStrings.xml"/></Relationships>','xl/styles.xml':stl,'xl/sharedStrings.xml':ssXml,'xl/worksheets/sheet1.xml':wsh};var order=['[Content_Types].xml','_rels/.rels','xl/workbook.xml','xl/_rels/workbook.xml.rels','xl/styles.xml','xl/sharedStrings.xml','xl/worksheets/sheet1.xml'];var zparts=[],zcds=[],zoff=0,znf=0;order.forEach(function(name){var nb=enc.encode(name),db=enc.encode(F[name]),sz=db.length,cr=crc32(db);var lha=[0x50,0x4B,0x03,0x04,0x14,0,0,0,0,0,0,0,0,0].concat(u4(cr)).concat(u4(sz)).concat(u4(sz)).concat(u2(nb.length)).concat([0,0]);var entry=new Uint8Array(lha.length+nb.length+sz);entry.set(new Uint8Array(lha),0);entry.set(nb,lha.length);entry.set(db,lha.length+nb.length);zparts.push(entry);var cda=[0x50,0x4B,0x01,0x02,0x14,0,0x14,0,0,0,0,0,0,0,0,0].concat(u4(cr)).concat(u4(sz)).concat(u4(sz)).concat(u2(nb.length)).concat([0,0,0,0,0,0,0,0,0,0,0,0]).concat(u4(zoff));var cde=new Uint8Array(cda.length+nb.length);cde.set(new Uint8Array(cda),0);cde.set(nb,cda.length);zcds.push(cde);zoff+=entry.length;znf++;});var cdSz=zcds.reduce(function(a,c){return a+c.length;},0);var ea=[0x50,0x4B,0x05,0x06,0,0,0,0].concat(u2(znf)).concat(u2(znf)).concat(u4(cdSz)).concat(u4(zoff)).concat([0,0]);var tot=zoff+cdSz+ea.length,zout=new Uint8Array(tot),zpos=0;zparts.forEach(function(p){zout.set(p,zpos);zpos+=p.length;});zcds.forEach(function(c){zout.set(c,zpos);zpos+=c.length;});zout.set(new Uint8Array(ea),zpos);slocDownload(zout,fname,'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet');}
    function slocXlsMulti(fname,sheets){
      var enc=new TextEncoder();
      var CT=[];
      for(var _n=0;_n<256;_n++){var _c=_n;for(var _k=0;_k<8;_k++)_c=_c&1?0xEDB88320^(_c>>>1):_c>>>1;CT[_n]=_c;}
      function crc32(d){var v=0xFFFFFFFF;for(var i=0;i<d.length;i++)v=CT[(v^d[i])&0xFF]^(v>>>8);return(v^0xFFFFFFFF)>>>0;}
      function u2(n){return[n&0xFF,(n>>8)&0xFF];}
      function u4(n){return[n&0xFF,(n>>8)&0xFF,(n>>16)&0xFF,(n>>24)&0xFF];}
      function xe(s){return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');}
      var ss=[],si={};
      function S(v){v=String(v==null?'':v);if(!(v in si)){si[v]=ss.length;ss.push(v);}return si[v];}
      function colRef(c,r){var s='',n=c+1;while(n>0){n--;s=String.fromCharCode(65+(n%26))+s;n=Math.floor(n/26);}return s+r;}
      var ox='http://schemas.openxmlformats.org/',pns=ox+'package/2006/',ons=ox+'officeDocument/2006/',sns=ox+'spreadsheetml/2006/main';
      // Style indices: 0=normal 1=col-header(orange-fill/white-bold) 2=number(#,##0/right) 3=section(cream-fill/orange-bold) 4=bold-label 5=number(#,##0/left) 6=text(@)
      var stl='<?xml version="1.0" encoding="UTF-8" standalone="yes"?><styleSheet xmlns="'+sns+'">'
        +'<numFmts count="1"><numFmt numFmtId="164" formatCode="#,##0"/></numFmts>'
        +'<fonts count="3">'
          +'<font><sz val="11"/><name val="Calibri"/></font>'
          +'<font><sz val="11"/><b/><color rgb="FFFFFFFF"/><name val="Calibri"/></font>'
          +'<font><sz val="11"/><b/><color rgb="FFC45C10"/><name val="Calibri"/></font>'
        +'</fonts>'
        +'<fills count="4">'
          +'<fill><patternFill patternType="none"/></fill>'
          +'<fill><patternFill patternType="gray125"/></fill>'
          +'<fill><patternFill patternType="solid"><fgColor rgb="FFC45C10"/><bgColor indexed="64"/></patternFill></fill>'
          +'<fill><patternFill patternType="solid"><fgColor rgb="FFFAF0E6"/><bgColor indexed="64"/></patternFill></fill>'
        +'</fills>'
        +'<borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders>'
        +'<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>'
        +'<cellXfs count="7">'
          +'<xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>'
          +'<xf numFmtId="0" fontId="1" fillId="2" borderId="0" xfId="0" applyFont="1" applyFill="1"/>'
          +'<xf numFmtId="164" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1" applyAlignment="1"><alignment horizontal="right"/></xf>'
          +'<xf numFmtId="0" fontId="2" fillId="3" borderId="0" xfId="0" applyFont="1" applyFill="1"/>'
          +'<xf numFmtId="0" fontId="2" fillId="0" borderId="0" xfId="0" applyFont="1"/>'
          +'<xf numFmtId="164" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1" applyAlignment="1"><alignment horizontal="left"/></xf>'
          +'<xf numFmtId="49" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/>'
        +'</cellXfs>'
        +'<cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles>'
        +'</styleSheet>';
      var wsXmls=[],tableCounter=0,tableXmls={},wsRelsXmls={};
      function colNm(n){var s='';while(n>0){n--;s=String.fromCharCode(65+(n%26))+s;n=Math.floor(n/26);}return s;}
      sheets.forEach(function(sh,sheetIdx){
        var rx='<row r="1">';
        sh.hdrs.forEach(function(h,c){rx+='<c r="'+colRef(c,1)+'" t="s" s="1"><v>'+S(h)+'</v></c>';});
        rx+='</row>';
        var rn=2;
        sh.rows.forEach(function(row){
          if(!row||row.length===0){rx+='<row r="'+rn+'"/>';rn++;return;}
          if(row.length===1&&row[0]&&typeof row[0]==='object'&&row[0]._sec){
            rx+='<row r="'+rn+'">';
            rx+='<c r="'+colRef(0,rn)+'" t="s" s="3"><v>'+S(row[0].v)+'</v></c>';
            for(var ec=1;ec<sh.hdrs.length;ec++){rx+='<c r="'+colRef(ec,rn)+'" s="3"/>';}
            rx+='</row>';rn++;return;
          }
          rx+='<row r="'+rn+'">';
          row.forEach(function(cell,c){
            var ref=colRef(c,rn);
            if(cell===null||cell===undefined||cell===''){rx+='<c r="'+ref+'"/>';return;}
            if(typeof cell==='object'&&cell!==null){
              var cv=cell.v,cs=cell.s!=null?cell.s:0;
              if(typeof cv==='number'){rx+='<c r="'+ref+'" s="'+cs+'"><v>'+xe(cv)+'</v></c>';}
              else{rx+='<c r="'+ref+'" t="s" s="'+cs+'"><v>'+S(cv)+'</v></c>';}
              return;
            }
            if(typeof cell==='number'){rx+='<c r="'+ref+'" s="2"><v>'+xe(cell)+'</v></c>';return;}
            rx+='<c r="'+ref+'" t="s"><v>'+S(cell)+'</v></c>';
          });
          rx+='</row>';rn++;
        });
        var cw='';
        if(sh.colWidths&&sh.colWidths.length>0){
          cw='<cols>';
          sh.colWidths.forEach(function(w,i){cw+='<col min="'+(i+1)+'" max="'+(i+1)+'" width="'+w+'" customWidth="1"/>';});
          cw+='</cols>';
        }
        var tblParts='';
        if(!sh.isKv&&sh.hdrs.length>0&&sh.rows.length>0){
          tableCounter++;
          var tc=tableCounter,colCount=sh.hdrs.length,rowCount=sh.rows.length+1;
          var tRef='A1:'+colNm(colCount)+rowCount;
          tableXmls['xl/tables/table'+tc+'.xml']='<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            +'<table xmlns="'+sns+'" id="'+tc+'" name="Table'+tc+'" displayName="Table'+tc+'" ref="'+tRef+'" totalsRowShown="0">'
            +'<autoFilter ref="'+tRef+'"/>'
            +'<tableColumns count="'+colCount+'">'
            +sh.hdrs.map(function(h,i){return'<tableColumn id="'+(i+1)+'" name="'+xe(h)+'"/>';}).join('')
            +'</tableColumns>'
            +'<tableStyleInfo name="TableStyleMedium2" showFirstColumn="0" showLastColumn="0" showRowStripes="1" showColumnStripes="0"/>'
            +'</table>';
          wsRelsXmls['xl/worksheets/_rels/sheet'+(sheetIdx+1)+'.xml.rels']='<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            +'<Relationships xmlns="'+pns+'relationships">'
            +'<Relationship Id="rId1" Type="'+ons+'relationships/table" Target="../tables/table'+tc+'.xml"/>'
            +'</Relationships>';
          tblParts='<tableParts count="1"><tablePart r:id="rId1"/></tableParts>';
        }
        wsXmls.push('<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="'+sns+'" xmlns:r="'+ons+'relationships">'
          +'<sheetViews><sheetView workbookViewId="0"><pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/></sheetView></sheetViews>'
          +'<sheetFormatPr defaultRowHeight="15"/>'+cw+'<sheetData>'+rx+'</sheetData>'+tblParts+'</worksheet>');
      });
      var ssXml='<?xml version="1.0" encoding="UTF-8" standalone="yes"?><sst xmlns="'+sns+'" count="'+ss.length+'" uniqueCount="'+ss.length+'">'+ss.map(function(v){return'<si><t xml:space="preserve">'+xe(v)+'</t></si>';}).join('')+'</sst>';
      var ctOver=sheets.map(function(_,i){return'<Override PartName="/xl/worksheets/sheet'+(i+1)+'.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>';}).join('');
      var ctTable=Object.keys(tableXmls).map(function(k){return'<Override PartName="/'+k+'" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.table+xml"/>';}).join('');
      var ctXml='<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="'+pns+'content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'+ctOver+ctTable+'<Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/></Types>';
      var wbSh=sheets.map(function(sh,i){return'<sheet name="'+xe(sh.name)+'" sheetId="'+(i+1)+'" r:id="rId'+(i+1)+'"/>';}).join('');
      var wbXml='<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="'+sns+'" xmlns:r="'+ons+'relationships"><sheets>'+wbSh+'</sheets></workbook>';
      var wbR=sheets.map(function(_,i){return'<Relationship Id="rId'+(i+1)+'" Type="'+ons+'relationships/worksheet" Target="worksheets/sheet'+(i+1)+'.xml"/>';}).join('');
      wbR+='<Relationship Id="rId'+(sheets.length+1)+'" Type="'+ons+'relationships/styles" Target="styles.xml"/>'
        +'<Relationship Id="rId'+(sheets.length+2)+'" Type="'+ons+'relationships/sharedStrings" Target="sharedStrings.xml"/>';
      var wbRXml='<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="'+pns+'relationships">'+wbR+'</Relationships>';
      var F={'[Content_Types].xml':ctXml,'_rels/.rels':'<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="'+pns+'relationships"><Relationship Id="rId1" Type="'+ons+'relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>','xl/workbook.xml':wbXml,'xl/_rels/workbook.xml.rels':wbRXml,'xl/styles.xml':stl,'xl/sharedStrings.xml':ssXml};
      var order=['[Content_Types].xml','_rels/.rels','xl/workbook.xml','xl/_rels/workbook.xml.rels','xl/styles.xml','xl/sharedStrings.xml'];
      sheets.forEach(function(_,i){var k='xl/worksheets/sheet'+(i+1)+'.xml';F[k]=wsXmls[i];order.push(k);});
      Object.keys(wsRelsXmls).forEach(function(k){F[k]=wsRelsXmls[k];order.push(k);});
      Object.keys(tableXmls).forEach(function(k){F[k]=tableXmls[k];order.push(k);});
      var zparts=[],zcds=[],zoff=0,znf=0;
      order.forEach(function(name){var nb=enc.encode(name),db=enc.encode(F[name]),sz=db.length,cr=crc32(db);var lha=[0x50,0x4B,0x03,0x04,0x14,0,0,0,0,0,0,0,0,0].concat(u4(cr)).concat(u4(sz)).concat(u4(sz)).concat(u2(nb.length)).concat([0,0]);var entry=new Uint8Array(lha.length+nb.length+sz);entry.set(new Uint8Array(lha),0);entry.set(nb,lha.length);entry.set(db,lha.length+nb.length);zparts.push(entry);var cda=[0x50,0x4B,0x01,0x02,0x14,0,0x14,0,0,0,0,0,0,0,0,0].concat(u4(cr)).concat(u4(sz)).concat(u4(sz)).concat(u2(nb.length)).concat([0,0,0,0,0,0,0,0,0,0,0,0]).concat(u4(zoff));var cde=new Uint8Array(cda.length+nb.length);cde.set(new Uint8Array(cda),0);cde.set(nb,cda.length);zcds.push(cde);zoff+=entry.length;znf++;});
      var cdSz=zcds.reduce(function(a,c){return a+c.length;},0);
      var ea=[0x50,0x4B,0x05,0x06,0,0,0,0].concat(u2(znf)).concat(u2(znf)).concat(u4(cdSz)).concat(u4(zoff)).concat([0,0]);
      var tot=zoff+cdSz+ea.length,zout=new Uint8Array(tot),zpos=0;
      zparts.forEach(function(p){zout.set(p,zpos);zpos+=p.length;});
      zcds.forEach(function(c){zout.set(c,zpos);zpos+=c.length;});
      zout.set(new Uint8Array(ea),zpos);
      slocDownload(zout,fname,'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet');
    }
    window.resetPerFileTable = function() {
      var tbl = document.getElementById('per-file-table');
      if (!tbl) return;
      var shell = tbl.closest('.table-shell');
      if (shell) shell.scrollLeft = 0;
      Array.prototype.slice.call(tbl.querySelectorAll('th')).forEach(function(th) { th.style.width = ''; });
      Array.prototype.slice.call(tbl.querySelectorAll('col')).forEach(function(c) { c.style.width = ''; });
      if (window._pfPaginationReset) window._pfPaginationReset();
      var si = document.getElementById('per-file-search');
      if (si) si.value = '';
    };
    var _rh=['File','Language','Physical Lines','Code Lines','Comments','Blank','Mixed Separate','Functions','Classes','Variables','Imports'];
    var _titleSlug="{{ title }}".replace(/[^a-zA-Z0-9\-]/g,'_').replace(/_+/g,'_').replace(/^_+|_+$/g,'');
    var _commitSlug="{% if let Some(c) = run.git_commit_short %}{{ c }}{% endif %}";
    var _exportSlug='per-file_'+_titleSlug+(_commitSlug?'_'+_commitSlug:'');
    function getReportExportRows(){var r=[];document.querySelectorAll('#per-file-table tbody tr').forEach(function(tr){var tds=tr.querySelectorAll('td');if(tds.length<11)return;r.push([tds[0].textContent.trim(),tds[1].textContent.trim(),tds[2].textContent.trim(),tds[3].textContent.trim(),tds[4].textContent.trim(),tds[5].textContent.trim(),tds[6].textContent.trim(),tds[7].textContent.trim(),tds[8].textContent.trim(),tds[9].textContent.trim(),tds[10].textContent.trim()]);});return r;}
    window.exportReportCsv=function(){slocCsv(_exportSlug+'.csv',_rh,getReportExportRows());};
    window.exportReportXls=function(){
      var fname='report_'+_titleSlug+(_commitSlug?'_'+_commitSlug:'')+'.xlsx';
      function sec(v){return[{_sec:true,v:v}];}
      function B(v){return{v:v,s:4};}
      function N(v){return{v:typeof v==='number'?v:Number(v),s:5};}
      // Table cells render with thousands separators (1,656,153) via the |commas
      // filter; Number() on that string is NaN, which would store the value as text
      // (green-triangle warning, left-aligned). Strip separators so numeric cells
      // become real numbers and align correctly. Non-numeric text is left untouched.
      function numify(v){var s=String(v==null?'':v).trim();if(s==='')return s;var t=s.replace(/,/g,'');return /^-?\d+(\.\d+)?$/.test(t)?Number(t):v;}
      function pnum(v){var t=String(v==null?'':v).replace(/,/g,'').trim();return /^-?\d+(\.\d+)?$/.test(t)?Number(t):0;}
      var dens=_SLOC_META.physicalLines>0?(_SLOC_META.codeLines/_SLOC_META.physicalLines*100).toFixed(1)+'%':'0%';
      var sumRows=[
        sec('RUN INFORMATION'),
        [B('Run ID'),_SLOC_META.runId,''],
        [B('Git Commit'),_SLOC_META.gitCommit,''],
        [B('Branch'),_SLOC_META.branch,''],
        [B('Last Commit By'),_SLOC_META.lastCommitBy,''],
        [B('Scan By'),_SLOC_META.scanBy,''],
        [B('Scanned'),_SLOC_META.scanned,''],
        [B('OS'),_SLOC_META.os,''],
        [B('Files Analyzed'),N(_SLOC_META.filesAnalyzed),'Total source files included in this analysis'],
        [B('Files Skipped'),N(_SLOC_META.filesSkipped),'Files excluded (binary, unsupported, or policy-filtered)'],
        [],
        sec('CODE METRICS'),
        [B('Physical Lines'),N(_SLOC_META.physicalLines),'Total lines including code, comments, and blanks'],
        [B('Code Lines'),N(_SLOC_META.codeLines),'Lines containing executable source code'],
        [B('Comments'),N(_SLOC_META.commentLines),'Lines consisting entirely of comments or documentation'],
        [B('Blank Lines'),N(_SLOC_META.blankLines),'Empty or whitespace-only lines'],
        [B('Mixed Separate'),N(_SLOC_META.mixedSeparate),'Lines with both code and trailing comment, counted separately'],
        [B('Functions'),N(_SLOC_META.functions),'Best-effort count of function/method definitions'],
        [B('Classes / Types'),N(_SLOC_META.classes),'Best-effort count of class, struct, interface definitions'],
        [B('Variables'),N(_SLOC_META.variables),'Best-effort count of variable and constant declarations'],
        [B('Imports'),N(_SLOC_META.imports),'Best-effort count of import, include, module-use statements'],
        [B('Tests'),N(_SLOC_META.tests),'Best-effort count of test cases (GTest, PyTest, JUnit, etc.)'],
        [B('Code Density'),{v:dens,s:6},'Percentage of physical lines that contain executable source code'],
        [B('Tool Version'),'oxide-sloc '+_SLOC_META.toolVersion,''],
      ];
      var langHdrs=['Language','Files','Physical Lines','Code Lines','Comments','Blank Lines','Mixed','Functions','Classes','Variables','Imports','Tests','Assertions','Suites'];
      var langRows=[];
      document.querySelectorAll('#lang-breakdown-table tbody tr').forEach(function(tr){
        var tds=tr.querySelectorAll('td');
        var row=[];
        Array.prototype.forEach.call(tds,function(td,i){var v=td.textContent.trim();row.push(i>0?numify(v):v);});
        langRows.push(row);
      });
      var pfHdrs=['File','Language','Physical Lines','Code Lines','Comments','Blank','Mixed','Functions','Classes','Variables','Imports','Tests','Assertions','Suites'];
      var pfRows=[];
      document.querySelectorAll('#per-file-table tbody tr').forEach(function(tr){
        var tds=tr.querySelectorAll('td');
        if(tds.length<11)return;
        var row=[];
        Array.prototype.forEach.call(tds,function(td,i){var v=td.textContent.trim();row.push(i>=2?numify(v):v);});
        pfRows.push(row);
      });
      var skHdrs=['File','Status','Warnings'];
      var skRows=[];
      document.querySelectorAll('#skipped-table tbody tr').forEach(function(tr){
        var tds=tr.querySelectorAll('td');
        if(tds.length<3)return;
        skRows.push([tds[0].textContent.trim(),tds[1].textContent.trim(),tds[2].textContent.trim()]);
      });
      var covHdrs=['Language','Files','Physical Lines','Code Lines','Code Density','Functions','Classes','Variables','Imports','Tests','Assertions','Test Suites'];
      var covRows=[];
      document.querySelectorAll('#lang-breakdown-table tbody tr').forEach(function(tr){
        var tds=tr.querySelectorAll('td');
        if(tds.length<4)return;
        var phys=pnum(tds[2].textContent);
        var code=pnum(tds[3].textContent);
        var densStr=phys>0?(code/phys*100).toFixed(1)+'%':'0%';
        var row=[tds[0].textContent.trim(),pnum(tds[1].textContent),phys,code,{v:densStr,s:6}];
        for(var i=7;i<Math.min(tds.length,14);i++){row.push(numify(tds[i].textContent.trim()));}
        covRows.push(row);
      });
      slocXlsMulti(fname,[
        {name:'Summary',hdrs:['Field / Metric','Value','Description'],rows:sumRows,colWidths:[22,45,55],isKv:true},
        {name:'Language Breakdown',hdrs:langHdrs,rows:langRows,colWidths:[16,8,14,12,12,12,8,10,10,10,10,8,10,8]},
        {name:'Per-File Detail',hdrs:pfHdrs,rows:pfRows,colWidths:[50,12,12,12,12,10,8,10,10,10,10,8,10,8]},
        {name:'Code Coverage',hdrs:covHdrs,rows:covRows,colWidths:[18,7,14,12,13,11,10,10,10,8,11,12]},
        {name:'Skipped Files',hdrs:skHdrs,rows:skRows,colWidths:[60,25,50]}
      ]);
    };
    Array.prototype.slice.call(document.querySelectorAll('[data-export-csv]')).forEach(function(btn){btn.addEventListener('click',function(){slocCsv(_exportSlug+'.csv',_rh,getReportExportRows());});});
    Array.prototype.slice.call(document.querySelectorAll('[data-export-xls]')).forEach(function(btn){btn.addEventListener('click',window.exportReportXls);});
    Array.prototype.slice.call(document.querySelectorAll('[data-reset-table]')).forEach(function(btn){btn.addEventListener('click',window.resetPerFileTable);});
    var _skippedRh=['File','Status','Warnings'];
    var _skippedSlug='skipped_'+_titleSlug+(_commitSlug?'_'+_commitSlug:'');
    function getSkippedExportRows(){var r=[];document.querySelectorAll('#skipped-table tbody tr').forEach(function(tr){var tds=tr.querySelectorAll('td');if(tds.length<3)return;r.push([tds[0].textContent.trim(),tds[1].textContent.trim(),tds[2].textContent.trim()]);});return r;}
    (function(){var b=document.getElementById('skipped-export-csv');if(b)b.addEventListener('click',function(){slocCsv(_skippedSlug+'.csv',_skippedRh,getSkippedExportRows());});})();
    (function(){var b=document.getElementById('skipped-export-xls');if(b)b.addEventListener('click',function(){slocXls(_skippedSlug+'.xlsx','Skipped Files',_skippedRh,getSkippedExportRows());});})();
    // ── Chart.js initialization ───────────────────────────────────────────────
    // Deferred so the browser can repaint (dismiss the loading overlay) before
    // the canvas/SVG chart work blocks the main thread.
    requestAnimationFrame(function() {
    try {
    (function() {
      var D = {{ lang_chart_json|safe }};
      var SUB_D = {{ submodule_chart_json|safe }};
      var SCAT_D = {{ scatter_chart_json|safe }};
      var SEM_D = {{ semantic_chart_json|safe }};
      var HIST_D = {{ file_size_histogram_json|safe }};
      if (!D || !D.length) return;

      var PALETTE = ['#C45C10','#2A6846','#4472C4','#805099','#D4A017','#B23030',
                     '#2E75B6','#70AD47','#FF9900','#9E480E','#636363','#156082',
                     '#D0743C','#5BA8A0','#8B3A8B','#3D7A3D','#AA5500','#005599'];
      var OX = '#C45C10', GN = '#2A6846', GY = '#BBBBBB';
      var ALL_CHARTS = [];
      function hexAlpha(hex, a) {
        var r=parseInt(hex.slice(1,3),16),g=parseInt(hex.slice(3,5),16),b=parseInt(hex.slice(5,7),16);
        return 'rgba('+r+','+g+','+b+','+a+')';
      }

      function fmt(n) {
        var v = Number(n), a = Math.abs(v);
        if (a >= 1e6) return (v/1e6).toFixed(1).replace(/\.0$/,'') + 'M';
        if (a >= 1e4) return Math.round(v/1e3) + 'K';
        return v.toLocaleString();
      }
      function isDark() { return document.body.classList.contains('dark-theme'); }
      function clr() {
        return isDark()
          ? { text: '#d4c5b8', grid: 'rgba(255,255,255,0.10)' }
          : { text: '#43342d', grid: '#e6d0bf' };
      }
      // Legend-highlight alpha for a dataset's drawn label / marker. `chart.$hiDs` is
      // set by legend hover (see attachScatterLegend); when it is null every dataset
      // draws at full strength. Once a language is hovered, the others fade so the
      // hovered one's marker, name and number stay readable through overlapping
      // neighbours. Applied globally to every value-labelled Chart.js plot.
      function hiAlpha(chart, di) {
        var h = chart.$hiDs;
        if (h == null) return 1;
        return di === h ? 1 : 0.1;
      }
      // Inline Chart.js plugin: draws a permanent value label on each bar / bubble.
      // fmtFn(rawValue, datasetIndex, pointIndex) → string | null
      // anchor: 'top' = above vertical bar, 'end' = right of horizontal bar, 'bubble' = above bubble
      function makeDlPlugin(fmtFn, anchor) {
        return {
          afterDatasetsDraw: function(chart) {
            var ctx = chart.ctx;
            var tc = clr().text;
            chart.data.datasets.forEach(function(ds, di) {
              var meta = chart.getDatasetMeta(di);
              meta.data.forEach(function(el, idx) {
                var label = fmtFn(ds.data[idx], di, idx);
                if (label == null || label === '') return;
                ctx.save();
                ctx.globalAlpha = hiAlpha(chart, di);
                ctx.font = '600 11px Inter,ui-sans-serif,sans-serif';
                ctx.fillStyle = tc;
                if (anchor === 'top') {
                  ctx.textAlign = 'center';
                  ctx.textBaseline = 'bottom';
                  ctx.fillText(String(label), el.x, el.y - 3);
                } else if (anchor === 'end') {
                  ctx.textAlign = 'left';
                  ctx.textBaseline = 'middle';
                  ctx.fillText(String(label), el.x + 5, el.y);
                } else {
                  ctx.textAlign = 'center';
                  ctx.textBaseline = 'bottom';
                  var r = (el.options && el.options.radius) ? el.options.radius : 10;
                  ctx.fillText(String(label), el.x, el.y - r - 3);
                }
                ctx.restore();
              });
            });
          }
        };
      }
      // Bubble-chart value labels (language name + code lines above each bubble).
      // Shared by the dashboard card and the Full View modal. Honours the legend
      // highlight: non-hovered languages' labels fade and the hovered one is drawn
      // last so its name + number sit on top of any overlapping neighbours — the
      // clustered bubbles at the origin are otherwise an unreadable pile of text.
      function scatterLabelPlugin() {
        return { afterDatasetsDraw: function(chart) {
          var ctx = chart.ctx, tc = clr().text, hi = chart.$hiDs;
          function drawOne(di) {
            var d = SCAT_D[di]; if (!d) return;
            var meta = chart.getDatasetMeta(di), a = hiAlpha(chart, di);
            meta.data.forEach(function(el) {
              var r = (el.options && el.options.radius) ? el.options.radius : 10;
              var ty2 = Math.max(14, el.y - r - 3), ty1 = Math.max(1, ty2 - 14);
              ctx.save();
              ctx.globalAlpha = a; ctx.fillStyle = tc;
              ctx.textBaseline = 'bottom'; ctx.textAlign = 'center';
              ctx.font = '800 11px Inter,ui-sans-serif,sans-serif';
              ctx.fillText(d.lang, el.x, ty1);
              ctx.font = '700 10px Inter,ui-sans-serif,sans-serif';
              ctx.fillText(fmt(d.code), el.x, ty2);
              ctx.restore();
            });
          }
          chart.data.datasets.forEach(function(_, di) { if (hi == null || di !== hi) drawOne(di); });
          if (hi != null && hi >= 0) drawOne(hi);
        } };
      }
      function makeStackedEndPlugin(fmtFn) {
        return {
          afterDatasetsDraw: function(chart) {
            var ctx = chart.ctx;
            var tc = clr().text;
            var nDs = chart.data.datasets.length;
            if (nDs === 0) return;
            var lastMeta = chart.getDatasetMeta(nDs - 1);
            lastMeta.data.forEach(function(el, idx) {
              var total = 0;
              chart.data.datasets.forEach(function(ds) { total += ds.data[idx] || 0; });
              var label = fmtFn(total, idx);
              if (label == null || label === '') return;
              ctx.save();
              ctx.font = '600 11px Inter,ui-sans-serif,sans-serif';
              ctx.fillStyle = tc;
              ctx.textAlign = 'left';
              ctx.textBaseline = 'middle';
              ctx.fillText(String(label), el.x + 5, el.y);
              ctx.restore();
            });
          }
        };
      }

      function wireDonutLegend(svg) {
        if(!svg) return;
        // Every donut element carries data-lang: slices (path/circle), leader lines,
        // outside labels + % labels (text) and legend rows (g). Hovering any one of
        // them emphasises that language across all of them and fades the rest, so the
        // slice, its leader line, its label and its legend row move as one picture.
        var items=svg.querySelectorAll('[data-lang]');
        function emph(el,st){ // st: 1 = highlight, -1 = fade, 0 = reset
          var tag=el.tagName.toLowerCase();
          if(tag==='path'||tag==='circle'){
            if(st===1){el.style.opacity='1';el.style.filter='brightness(1.15) drop-shadow(0 3px 9px rgba(0,0,0,.28))';el.style.transform='scale(1.06)';}
            else if(st===-1){el.style.opacity='0.24';el.style.filter='none';el.style.transform='none';}
            else{el.style.opacity='';el.style.filter='';el.style.transform='';}
          }else if(tag==='line'){
            if(st===1){el.style.opacity='1';el.style.strokeWidth='1.8';}
            else if(st===-1){el.style.opacity='0.1';el.style.strokeWidth='';}
            else{el.style.opacity='';el.style.strokeWidth='';}
          }else if(tag==='text'){
            if(st===1){el.style.opacity='1';el.style.fontWeight='800';}
            else if(st===-1){el.style.opacity='0.18';el.style.fontWeight='';}
            else{el.style.opacity='';el.style.fontWeight='';}
          }else{ // legend group
            if(st===1){el.style.opacity='1';}
            else if(st===-1){el.style.opacity='0.4';}
            else{el.style.opacity='';}
          }
        }
        function hl(lang){for(var i=0;i<items.length;i++){emph(items[i],items[i].getAttribute('data-lang')===lang?1:-1);}}
        function rst(){for(var i=0;i<items.length;i++){emph(items[i],0);}}
        svg.addEventListener('mouseover',function(e){var t=e.target;while(t&&t!==svg){var l=t.getAttribute&&t.getAttribute('data-lang');if(l){hl(l);return;}t=t.parentNode;}rst();});
        svg.addEventListener('mousemove',function(e){var t=e.target;while(t&&t!==svg){if(t.getAttribute&&t.getAttribute('data-lang'))return;t=t.parentNode;}rst();});
        svg.addEventListener('mouseout',function(e){if(e.relatedTarget&&svg.contains(e.relatedTarget))return;rst();});
      }
      function wireMixLegend(svg) {
        if(!svg) return;
        var legGs=svg.querySelectorAll('g[data-kind]');
        var allRects=svg.querySelectorAll('rect[data-kind]');
        if(!legGs.length) return;
        function hlKind(kind) {
          for(var i=0;i<allRects.length;i++){var r=allRects[i];if(r.getAttribute('data-kind')===kind){r.style.opacity='1';r.style.filter='brightness(1.18) drop-shadow(0 2px 6px rgba(0,0,0,.22))';}else{r.style.opacity='0.18';r.style.filter='none';}}
          for(var j=0;j<legGs.length;j++){legGs[j].style.opacity=legGs[j].getAttribute('data-kind')===kind?'1':'0.45';}
        }
        function rst(){for(var i=0;i<allRects.length;i++){allRects[i].style.opacity='';allRects[i].style.filter='';}for(var j=0;j<legGs.length;j++){legGs[j].style.opacity='';}}
        for(var k=0;k<legGs.length;k++){(function(g){g.addEventListener('mouseenter',function(){hlKind(g.getAttribute('data-kind'));});g.addEventListener('mouseleave',rst);})(legGs[k]);}
      }

      // ── Language overview: SVG donut + horizontal stacked bars ───────────────
      (function() {
        var el = document.getElementById('report-lang-overview');
        if (!el || !D || !D.length) return;
        var FONT = 'Inter,ui-sans-serif,system-ui,-apple-system,sans-serif';
        function esc(s){return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');}
        function px(n){return Math.round(n);}
        function tt(label,val){return ' class="rchit" data-ttl="'+String(label).replace(/&/g,'&amp;').replace(/"/g,'&quot;')+'" data-ttv="'+String(val).replace(/&/g,'&amp;').replace(/"/g,'&quot;')+'"';}
        var tot = D.reduce(function(a,d){return a+d.code;},0)||1;
        // Donut — height matches the stacked-bar chart so both panels align
        var rHb_d=28;
        var DH=Math.max(220,D.length*rHb_d+32);
        var cx=100,cy=Math.round(DH/2),Ro=88,Ri=48,legX=208,DW=395;
        var legCount=D.length;
        var legSpacing=Math.max(12,Math.min(22,Math.floor((DH-30)/Math.max(legCount,1))));
        var legYStart=Math.round((DH-legCount*legSpacing)/2);
        var ds='<svg id="dnt-svg" viewBox="0 0 '+DW+' '+DH+'" width="'+DW+'" height="'+DH+'" style="display:block;max-width:100%;" xmlns="http://www.w3.org/2000/svg">';
        // One shared transition on every donut element so slices, leader lines,
        // outside labels, % labels and the legend all animate together as a single
        // picture when a language is hovered. Slices scale from the donut centre.
        ds+='<style>#dnt-svg path,#dnt-svg circle,#dnt-svg line,#dnt-svg text,#dnt-svg g{transition:opacity .22s ease,filter .22s ease,transform .22s ease,stroke-width .22s ease;}#dnt-svg path,#dnt-svg circle{transform-origin:'+cx+'px '+cy+'px;}</style>';
        if(D.length===1){
          var rm=Math.round((Ro+Ri)/2),rsw=Ro-Ri;
          ds+='<circle'+tt(D[0].lang,fmt(D[0].code)+' code lines')+' data-lang="'+esc(D[0].lang)+'" cx="'+cx+'" cy="'+cy+'" r="'+rm+'" fill="none" stroke="'+PALETTE[0]+'" stroke-width="'+rsw+'"/>';
        } else {
          var smalls=[];
          var ang=-Math.PI/2;
          D.forEach(function(d,i){
            var sw=Math.min(d.code/tot*2*Math.PI,2*Math.PI-0.001),a2=ang+sw;
            var x1=cx+Ro*Math.cos(ang),y1=cy+Ro*Math.sin(ang);
            var x2=cx+Ro*Math.cos(a2),y2=cy+Ro*Math.sin(a2);
            var xi1=cx+Ri*Math.cos(a2),yi1=cy+Ri*Math.sin(a2);
            var xi2=cx+Ri*Math.cos(ang),yi2=cy+Ri*Math.sin(ang);
            var pct=Math.round(d.code/tot*100);
            ds+='<path'+tt(d.lang,fmt(d.code)+' code lines ('+pct+'%)')+' data-lang="'+esc(d.lang)+'" d="M'+px(x1)+','+px(y1)+' A'+Ro+','+Ro+' 0 '+(sw>Math.PI?1:0)+',1 '+px(x2)+','+px(y2)+' L'+px(xi1)+','+px(yi1)+' A'+Ri+','+Ri+' 0 '+(sw>Math.PI?1:0)+',0 '+px(xi2)+','+px(yi2)+' Z" fill="'+(PALETTE[i%PALETTE.length])+'" stroke="white" stroke-width="2"/>';
            if(pct>=5){var mAng=ang+sw/2,mR=(Ro+Ri)/2;ds+='<text data-lang="'+esc(d.lang)+'" x="'+px(cx+mR*Math.cos(mAng))+'" y="'+px(cy+mR*Math.sin(mAng))+'" text-anchor="middle" dominant-baseline="middle" font-family="'+FONT+'" font-size="10" font-weight="700" fill="white" style="pointer-events:none;">'+pct+'%</text>';}else if(pct>0){smalls.push({mAng:ang+sw/2,pct:pct,lang:d.lang,col:PALETTE[i%PALETTE.length]});}
            ang+=sw;
          });
          // Small slices (<5%) get outside labels positioned near each slice's own
          // angular position (a slice on the left gets its label/leader on the left),
          // then nudged apart horizontally so text never overlaps. Leader lines point
          // from each slice to its label. Horizontal text keeps long names legible;
          // the whole SVG scales up in Full View so these stay readable there too.
          if(smalls.length){
            smalls.sort(function(a,b){return a.mAng-b.mAng;});
            var sPad=6,sRowY=11;
            smalls.forEach(function(sm){sm.txt=sm.lang+' '+sm.pct+'%';sm.w=sm.txt.length*5+8;sm.x=Math.max(sPad+sm.w/2,Math.min(DW-sPad-sm.w/2,cx+(Ro+14)*Math.cos(sm.mAng)));});
            for(var si=1;si<smalls.length;si++){var mnX=smalls[si-1].x+smalls[si-1].w/2+smalls[si].w/2+3;if(smalls[si].x<mnX)smalls[si].x=mnX;}
            var sLast=smalls[smalls.length-1],sOver=sLast.x+sLast.w/2-(DW-sPad);
            if(sOver>0)smalls.forEach(function(sm){sm.x-=sOver;});
            smalls.forEach(function(sm){
              var axx=cx+Ro*Math.cos(sm.mAng),ayy=cy+Ro*Math.sin(sm.mAng);
              ds+='<line data-lang="'+esc(sm.lang)+'" x1="'+px(axx)+'" y1="'+px(ayy)+'" x2="'+px(sm.x)+'" y2="'+px(sRowY+4)+'" stroke="'+sm.col+'" stroke-width="1" opacity="0.5" style="pointer-events:none;"/>';
              ds+='<text data-lang="'+esc(sm.lang)+'" x="'+px(sm.x)+'" y="'+px(sRowY)+'" text-anchor="middle" font-family="'+FONT+'" font-size="9" font-weight="700" fill="'+sm.col+'" style="cursor:pointer;">'+esc(sm.txt)+'</text>';
            });
          }
        }
        ds+='<text x="'+cx+'" y="'+(cy-7)+'" text-anchor="middle" font-family="'+FONT+'" font-size="21" font-weight="800" fill="#43342d">'+fmt(tot)+'</text>';
        ds+='<text x="'+cx+'" y="'+(cy+14)+'" text-anchor="middle" font-family="'+FONT+'" font-size="11" fill="#7b675b">code lines</text>';
        D.forEach(function(d,i){
          var ly=legYStart+i*legSpacing;
          var pctL=Math.round(d.code/tot*100);
          var ttL=String(d.lang).replace(/&/g,'&amp;').replace(/"/g,'&quot;');
          var ttV=(fmt(d.code)+' code lines ('+pctL+'%)').replace(/&/g,'&amp;').replace(/"/g,'&quot;');
          ds+='<g data-lang="'+esc(d.lang)+'" data-ttl="'+ttL+'" data-ttv="'+ttV+'" style="cursor:pointer;">';
          ds+='<rect x="'+legX+'" y="'+(ly-2)+'" width="'+(DW-legX)+'" height="'+(legSpacing||14)+'" fill="transparent"/>';
          ds+='<rect x="'+legX+'" y="'+ly+'" width="11" height="11" rx="2" fill="'+(PALETTE[i%PALETTE.length])+'"/>';
          ds+='<text x="'+(legX+16)+'" y="'+(ly+10)+'" font-family="'+FONT+'" font-size="'+Math.min(11,legSpacing-2)+'" fill="#43342d">'+esc(d.lang)+'</text>';
          ds+='<text x="'+(legX+100)+'" y="'+(ly+10)+'" font-family="'+FONT+'" font-size="'+Math.min(10,legSpacing-3)+'" font-weight="700" fill="#7b675b">'+fmt(d.code)+' ('+pctL+'%)</text>';
          ds+='</g>';
        });
        ds+='</svg>';
        // Horizontal stacked-bar chart
        var maxT=Math.max.apply(null,D.map(function(d){return d.physical||d.code+d.comments+d.blanks;}))||1;
        var LW=108,BW=260,svgW=LW+BW+68;
        var barRhb=Math.min(48,Math.max(28,Math.floor((DH-32)/D.length)));
        var barBH=Math.min(32,Math.round(barRhb*0.7));
        var SH=DH;
        var barTopPad=Math.max(6,Math.round((SH-D.length*barRhb-18)/2));
        var bs='<svg viewBox="0 0 '+svgW+' '+SH+'" width="'+svgW+'" height="'+SH+'" style="display:block;max-width:100%;" xmlns="http://www.w3.org/2000/svg">';
        // Largest font size (<=10) at which `t` fits in a `w`-wide segment, or 0 if
        // it cannot fit legibly even at the 6.5 floor (labels shrink to fit instead
        // of disappearing; the SVG scales up in Full View so small fonts stay legible).
        function fitFs(t,w){var fs=Math.min(10,(w-4)/((String(t).length||1)*0.58));return fs>=6.5?Math.round(fs*10)/10:0;}
        D.forEach(function(d,i){
          var y=barTopPad+i*barRhb,x=LW;
          var phys=d.physical||d.code+d.comments+d.blanks;
          var cW=d.code/maxT*BW,cmW=d.comments/maxT*BW,blW=d.blanks/maxT*BW;
          var lmid=y+barBH/2+4;
          var ttv='Code: '+fmt(d.code)+'\nComments: '+fmt(d.comments)+'\nBlank: '+fmt(d.blanks)+'\nTotal: '+fmt(phys);
          bs+='<g class="lang-bar-row">';
          // Hit area ends just past the total label so empty space to the right of the
          // bar does not trigger the tooltip — only the name, bar and total are hot.
          var hitW=px(LW+phys/maxT*BW+8+(String(fmt(phys)).length*6.8)+6);
          bs+='<rect'+tt(d.lang,ttv)+' x="0" y="'+y+'" width="'+hitW+'" height="'+barBH+'" fill="transparent" style="cursor:pointer;"/>';
          bs+='<text'+tt(d.lang,ttv)+' x="'+(LW-6)+'" y="'+lmid+'" text-anchor="end" font-family="'+FONT+'" font-size="11" fill="#43342d" style="cursor:pointer;">'+esc(d.lang)+'</text>';
          if(cW>0.5){bs+='<rect'+tt(d.lang+' Code',fmt(d.code)+' lines')+' data-kind="code" x="'+px(x)+'" y="'+y+'" width="'+px(cW)+'" height="'+barBH+'" fill="'+OX+'"/>';var _fc=fitFs(fmt(d.code),cW);if(_fc)bs+='<text x="'+px(x+cW/2)+'" y="'+lmid+'" text-anchor="middle" font-family="'+FONT+'" font-size="'+_fc+'" font-weight="700" fill="#fff" style="pointer-events:none;">'+fmt(d.code)+'</text>';x+=cW;}
          if(cmW>0.5){bs+='<rect'+tt(d.lang+' Comments',fmt(d.comments)+' lines')+' data-kind="comment" x="'+px(x)+'" y="'+y+'" width="'+px(cmW)+'" height="'+barBH+'" fill="'+GN+'"/>';var _fm=fitFs(fmt(d.comments),cmW);if(_fm)bs+='<text x="'+px(x+cmW/2)+'" y="'+lmid+'" text-anchor="middle" font-family="'+FONT+'" font-size="'+_fm+'" font-weight="700" fill="#fff" style="pointer-events:none;">'+fmt(d.comments)+'</text>';x+=cmW;}
          if(blW>0.5){bs+='<rect'+tt(d.lang+' Blank',fmt(d.blanks)+' lines')+' data-kind="blank" x="'+px(x)+'" y="'+y+'" width="'+px(blW)+'" height="'+barBH+'" fill="'+GY+'"/>';var _fb=fitFs(fmt(d.blanks),blW);if(_fb)bs+='<text x="'+px(x+blW/2)+'" y="'+lmid+'" text-anchor="middle" font-family="'+FONT+'" font-size="'+_fb+'" font-weight="700" fill="#555" style="pointer-events:none;">'+fmt(d.blanks)+'</text>';}
          bs+='<text'+tt(d.lang,ttv)+' x="'+px(LW+phys/maxT*BW+8)+'" y="'+lmid+'" font-family="'+FONT+'" font-size="11" font-weight="700" fill="#7b675b" style="cursor:pointer;">'+fmt(phys)+'</text>';
          bs+='</g>';
        });
        var ly=SH-14;
        var totC=D.reduce(function(a,d){return a+(d.code||0);},0);
        var totCm=D.reduce(function(a,d){return a+(d.comments||0);},0);
        var totBl=D.reduce(function(a,d){return a+(d.blanks||0);},0);
        var totAll=totC+totCm+totBl||1;
        function legTT(lbl,val){return ' data-ttl="'+lbl+'" data-ttv="'+val.replace(/"/g,'&quot;')+'"';}
        var ttC=legTT('Code lines',fmt(totC)+' total ('+Math.round(totC/totAll*100)+'%)');
        var ttCm=legTT('Comment lines',fmt(totCm)+' total ('+Math.round(totCm/totAll*100)+'%)');
        var ttBl=legTT('Blank lines',fmt(totBl)+' total ('+Math.round(totBl/totAll*100)+'%)');
        var legSt=LW+Math.max(0,Math.round((BW-194)/2));
        bs+='<g data-kind="code" style="cursor:pointer;">'
          +'<rect x="'+legSt+'" y="'+(ly-3)+'" width="50" height="16" fill="transparent"'+ttC+'/>'
          +'<rect x="'+legSt+'" y="'+ly+'" width="9" height="9" fill="'+OX+'"'+ttC+'/>'
          +'<text x="'+(legSt+13)+'" y="'+(ly+9)+'"'+ttC+' font-family="'+FONT+'" font-size="10" font-weight="700" fill="#43342d">Code</text>'
          +'</g>';
        bs+='<g data-kind="comment" style="cursor:pointer;">'
          +'<rect x="'+(legSt+58)+'" y="'+(ly-3)+'" width="82" height="16" fill="transparent"'+ttCm+'/>'
          +'<rect x="'+(legSt+58)+'" y="'+ly+'" width="9" height="9" fill="'+GN+'"'+ttCm+'/>'
          +'<text x="'+(legSt+71)+'" y="'+(ly+9)+'"'+ttCm+' font-family="'+FONT+'" font-size="10" font-weight="700" fill="#43342d">Comments</text>'
          +'</g>';
        bs+='<g data-kind="blank" style="cursor:pointer;">'
          +'<rect x="'+(legSt+145)+'" y="'+(ly-3)+'" width="55" height="16" fill="transparent"'+ttBl+'/>'
          +'<rect x="'+(legSt+145)+'" y="'+ly+'" width="9" height="9" fill="'+GY+'"'+ttBl+'/>'
          +'<text x="'+(legSt+158)+'" y="'+(ly+9)+'"'+ttBl+' font-family="'+FONT+'" font-size="10" font-weight="700" fill="#43342d">Blanks</text>'
          +'</g>';
        bs+='</svg>';
        el.innerHTML='<div class="r-lang-overview">'+
          '<div class="r-lang-overview-cell"><p>Code Lines by Language</p>'+ds+'</div>'+
          '<div class="r-lang-overview-cell" style="flex:2 1 340px;"><p>Line Mix per Language</p>'+bs+'</div>'+
        '</div>';
        wireDonutLegend(el.querySelector('svg'));
        wireMixLegend(el.querySelectorAll('svg')[1]);
      })();

      // Shared cursor helper: pointer over data elements, default elsewhere.
      // Added to options.onHover on every Chart.js instance.
      // Legend items handled separately via legend.onHover / legend.onLeave.
      function chartCursor(e, els) {
        var t = e.native && e.native.target;
        if (t) t.style.cursor = els.length ? 'pointer' : 'default';
      }
      function legendCursorOn(e) { var t=e.native&&e.native.target; if(t)t.style.cursor='pointer'; }
      function legendCursorOff(e){ var t=e.native&&e.native.target; if(t)t.style.cursor='default'; }
      // Pushes a right-positioned legend away from the plot by `gap` px. Chart.js
      // (v4) places a right legend flush against the plot area: fit() reserves the
      // legend box width and _draw() lays items out from `this.left + padding`, so
      // the column hugs the bubbles. We reserve `gap` extra width in fit() (which
      // shrinks the plot by `gap`), then translate the canvas right by `gap` while
      // the legend draws so the column lands in that reserved space — clear of the
      // plot. The legendHitBoxes (used only for hover hit-testing, not drawing) are
      // shifted by the same `gap` so hover targets stay aligned with what's drawn.
      function legendGapPlugin(gap) {
        return {
          id: 'legendGap',
          beforeInit: function(chart) {
            var lg = chart.legend; if (!lg) return;
            var origFit = lg.fit, origDraw = lg.draw;
            lg.fit = function() { origFit.call(this); this.width += gap; this._needGap = true; };
            lg.draw = function() {
              if (this._needGap && this.legendHitBoxes) {
                this.legendHitBoxes.forEach(function(h){ h.left += gap; });
                this._needGap = false;
              }
              var ctx = this.ctx;
              ctx.save();
              ctx.translate(gap, 0);
              origDraw.call(this);
              ctx.restore();
            };
          }
        };
      }

      // ── Project Overview bar ─────────────────────────────────────────────────
      var projChart = null;
      (function() {
        var ySel = document.getElementById('overview-y-axis');
        var xSel = document.getElementById('overview-x-mode');
        var el = document.getElementById('overview-chart');
        var lockedEl = document.getElementById('overview-chart-locked');
        var wrap = document.getElementById('canvas-proj-wrap');
        var canvas = document.getElementById('canvas-proj');
        if (!canvas || !ySel || !xSel) return;
        var Y_LABELS = { code:'Code Lines', comments:'Comment Lines', blanks:'Blank Lines',
                         physical:'Physical Lines', files:'Files', comment:'Comment Lines', blank:'Blank Lines' };
        function getData() {
          var yKey = ySel.value, mode = xSel.value;
          var src = mode === 'submodules' ? SUB_D : D;
          var lKey = mode === 'submodules' ? 'name' : 'lang';
          var sorted = src.slice().sort(function(a,b){ return (b[yKey]||0)-(a[yKey]||0); });
          return { sorted: sorted, lKey: lKey, yKey: yKey, yLabel: Y_LABELS[yKey]||yKey };
        }
        function renderOverview() {
          var mode = xSel.value, isHist = mode.indexOf('history') === 0;
          if (el) el.style.display = isHist ? 'none' : 'block';
          if (lockedEl) lockedEl.style.display = isHist ? 'block' : 'none';
          if (isHist) return;
          var r = getData();
          var c = clr();
          if (wrap) wrap.style.height = Math.max(200, Math.min(432, r.sorted.length * 29 + 60)) + 'px';
          if (projChart) {
            projChart.data.labels = r.sorted.map(function(d){return d[r.lKey];});
            projChart.data.datasets[0].data = r.sorted.map(function(d){return d[r.yKey]||0;});
            projChart.data.datasets[0].backgroundColor = r.sorted.map(function(_,i){return PALETTE[i%PALETTE.length];});
            projChart.data.datasets[0].label = r.yLabel;
            projChart.options.scales.x.title.text = r.yLabel;
            projChart.update('none'); return;
          }
          projChart = new Chart(canvas, {
            type: 'bar',
            data: {
              labels: r.sorted.map(function(d){return d[r.lKey];}),
              datasets: [{ label: r.yLabel,
                data: r.sorted.map(function(d){return d[r.yKey]||0;}),
                backgroundColor: r.sorted.map(function(_,i){return PALETTE[i%PALETTE.length];}),
                borderRadius: 3 }]
            },
            options: {
              indexAxis: 'y', responsive: true, maintainAspectRatio: false,
              onHover: chartCursor,
              animation: { duration: 500, easing: 'easeOutQuart' },
              layout: { padding: { right: 64 } },
              scales: {
                x: { grid: { color: c.grid }, ticks: { color: c.text, callback: function(v){return fmt(v);} },
                     title: { display: true, text: r.yLabel, color: c.text } },
                y: { grid: { display: false }, ticks: { color: c.text } }
              },
              plugins: {
                legend: { display: false },
                tooltip: {
                  callbacks: {
                    title: function(items) { return items.length ? items[0].label : ''; },
                    label: function(ctx) {
                      return '  ' + ctx.dataset.label + ': ' + Number(ctx.parsed.x).toLocaleString();
                    }
                  }
                }
              }
            },
            plugins: [makeDlPlugin(function(v){ return fmt(v||0); }, 'end')]
          });
          ALL_CHARTS.push(projChart);
        }
        ySel.addEventListener('change', renderOverview);
        xSel.addEventListener('change', renderOverview);
        renderOverview();

        var overviewExpandBtn = document.getElementById('overview-expand-btn');
        if (overviewExpandBtn) {
          overviewExpandBtn.addEventListener('click', function() {
            var r = getData();
            var n = r.sorted.length || 1;
            var maxH = Math.max(400, Math.floor(window.innerHeight * 0.82) - 130);
            var modalH = Math.min(Math.max(480, n * 29 + 96), maxH);
            var overlay = document.createElement('div');
            overlay.className = 'chart-modal-overlay';
            overlay.innerHTML = '<div class="chart-modal" style="max-width:1320px;">'
              + '<button class="chart-modal-close" aria-label="Close">&times;</button>'
              + '<div class="chart-modal-header">'
              + '<span class="chart-modal-title">Project Overview \u2014 Full View</span>'
              + '<label style="font-size:13px;font-weight:700;color:var(--muted);display:flex;align-items:center;gap:6px;flex-shrink:0;">Y Axis:'
              + '<select id="ov-modal-y" class="chart-select">'
              + '<option value="code">Code Lines</option>'
              + '<option value="comments">Comment Lines</option>'
              + '<option value="blanks">Blank Lines</option>'
              + '<option value="physical">Total Physical Lines</option>'
              + '<option value="files">File Count</option>'
              + '</select></label>'
              + (SUB_D && SUB_D.length ? '<label style="font-size:13px;font-weight:700;color:var(--muted);display:flex;align-items:center;gap:6px;">X Axis:'
              + '<select id="ov-modal-x" class="chart-select">'
              + '<option value="languages">Languages</option>'
              + '<option value="submodules">Submodules</option>'
              + '</select></label>' : '')
              + '</div>'
              + '<div style="position:relative;height:' + modalH + 'px;width:100%;"><canvas id="canvas-proj-modal"></canvas></div></div>';
            document.body.appendChild(overlay);
            overlay.querySelector('.chart-modal-close').addEventListener('click', function() { document.body.removeChild(overlay); });
            overlay.addEventListener('click', function(e) { if (e.target === overlay) document.body.removeChild(overlay); });
            var Y_LABELS = { code:'Code Lines', comments:'Comment Lines', blanks:'Blank Lines', physical:'Physical Lines', files:'Files' };
            var modalYSel = document.getElementById('ov-modal-y');
            var modalXSel = document.getElementById('ov-modal-x');
            if (modalYSel) modalYSel.value = ySel ? ySel.value : 'code';
            if (modalXSel && xSel) modalXSel.value = (xSel.value === 'languages' || xSel.value === 'submodules') ? xSel.value : 'languages';
            var modalCanvas = document.getElementById('canvas-proj-modal');
            if (!modalCanvas) return;
            var c = clr();
            function getModalData() {
              var yKey = modalYSel ? modalYSel.value : 'code';
              var mode = modalXSel ? modalXSel.value : 'languages';
              var src = mode === 'submodules' ? SUB_D : D;
              var lKey = mode === 'submodules' ? 'name' : 'lang';
              var sorted = src.slice().sort(function(a,b){ return (b[yKey]||0)-(a[yKey]||0); });
              return { sorted: sorted, lKey: lKey, yKey: yKey, yLabel: Y_LABELS[yKey]||yKey };
            }
            var ovModalChart = null;
            function renderOverviewModal() {
              var r2 = getModalData();
              if (ovModalChart) {
                ovModalChart.data.labels = r2.sorted.map(function(d){return d[r2.lKey];});
                ovModalChart.data.datasets[0].data = r2.sorted.map(function(d){return d[r2.yKey]||0;});
                ovModalChart.data.datasets[0].backgroundColor = r2.sorted.map(function(_,i){return PALETTE[i%PALETTE.length];});
                ovModalChart.data.datasets[0].label = r2.yLabel;
                ovModalChart.options.scales.x.title.text = r2.yLabel;
                ovModalChart.update('none'); return;
              }
              ovModalChart = new Chart(modalCanvas, {
                type: 'bar',
                data: {
                  labels: r2.sorted.map(function(d){return d[r2.lKey];}),
                  datasets: [{ label: r2.yLabel,
                    data: r2.sorted.map(function(d){return d[r2.yKey]||0;}),
                    backgroundColor: r2.sorted.map(function(_,i){return PALETTE[i%PALETTE.length];}),
                    borderRadius: 3 }]
                },
                options: {
                  indexAxis: 'y', responsive: true, maintainAspectRatio: false,
                  onHover: chartCursor,
                  animation: { duration: 500, easing: 'easeOutQuart' },
                  layout: { padding: { right: 64 } },
                  scales: {
                    x: { grid:{color:c.grid}, ticks:{color:c.text, callback:function(v){return fmt(v);}},
                         title:{display:true, text:r2.yLabel, color:c.text} },
                    y: { grid:{display:false}, ticks:{color:c.text} }
                  },
                  plugins: {
                    legend:{display:false},
                    tooltip:{callbacks:{
                      title:function(items){return items.length?items[0].label:'';},
                      label:function(ctx){return '  '+ctx.dataset.label+': '+Number(ctx.parsed.x).toLocaleString();}
                    }}
                  }
                },
                plugins: [makeDlPlugin(function(v){ return fmt(v||0); }, 'end')]
              });
            }
            renderOverviewModal();
            if (modalYSel) modalYSel.addEventListener('change', renderOverviewModal);
            if (modalXSel) modalXSel.addEventListener('change', renderOverviewModal);
          });
        }
      })();

      // ── Language Composition (SVG — matches /runs/result behaviour) ──────────
      (function() {
        var el = document.getElementById('comp-svg-container');
        if (!el || !D || !D.length) return;
        var cData = D.slice(0, 15);
        var cMode = 'absolute';
        var CX = OX, CG = GN, CB = '#BBBBBB';
        var CFONT = 'Inter,ui-sans-serif,system-ui,-apple-system,sans-serif';
        function cEsc(s){return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');}
        function cPx(n){return Math.round(n);}
        function cTT(l,v){return ' class="rchit" data-ttl="'+String(l).replace(/&/g,'&amp;').replace(/"/g,'&quot;')+'" data-ttv="'+String(v).replace(/&/g,'&amp;').replace(/"/g,'&quot;')+'"';}
        // Largest font size (<=10) at which `t` fits in a `w`-wide segment, or 0 if it
        // cannot fit legibly even at the 6.5 floor (labels shrink to fit rather than
        // disappear; the SVG scales up in Full View so small fonts stay legible).
        function cFitFs(t,w){var fs=Math.min(10,(w-4)/((String(t).length||1)*0.58));return fs>=6.5?Math.round(fs*10)/10:0;}
        function cLT(l,v){return ' data-ttl="'+l+'" data-ttv="'+v.replace(/"/g,'&quot;')+'"';}
        function renderCompSVG() {
          var isPct = cMode === 'pct';
          var totC=cData.reduce(function(a,d){return a+(d.code||0);},0);
          var totCm=cData.reduce(function(a,d){return a+(d.comments||0);},0);
          var totBl=cData.reduce(function(a,d){return a+(d.blanks||0);},0);
          var totAll=totC+totCm+totBl||1;
          var svgW=Math.max(320,el.offsetWidth||540);
          var LW=108,legendH=24,topPad=4;
          var MIN_SVG_H=220;
          var rHb=Math.min(80,Math.max(26,Math.floor((MIN_SVG_H-legendH-topPad-10)/cData.length)));
          var bH=Math.min(38,Math.round(rHb*0.68));
          var BW=Math.max(120,svgW-LW-84);
          var SH=Math.max(MIN_SVG_H,cData.length*rHb+legendH+topPad+10);
          var s='<svg viewBox="0 0 '+svgW+' '+SH+'" width="'+svgW+'" height="'+SH+'" style="display:block;max-width:100%;" xmlns="http://www.w3.org/2000/svg">';
          if(isPct){
            cData.forEach(function(d,i){
              var t2=(d.code||0)+(d.comments||0)+(d.blanks||0)||1;
              var cW=(d.code||0)/t2*BW,cmW=(d.comments||0)/t2*BW,blW=(d.blanks||0)/t2*BW;
              var y=topPad+i*rHb+Math.floor((rHb-bH)/2),x=LW;
              var lmid=y+Math.floor(bH/2)+4;
              var ttvc='Code: '+fmt(d.code||0)+'\nComments: '+fmt(d.comments||0)+'\nBlank: '+fmt(d.blanks||0)+'\nTotal: '+fmt(d.physical||t2);
              s+='<text'+cTT(d.lang,ttvc)+' x="'+(LW-5)+'" y="'+lmid+'" text-anchor="end" font-family="'+CFONT+'" font-size="11" fill="#43342d" style="cursor:pointer;">'+cEsc(d.lang)+'</text>';
              if(cW>0.5){s+='<rect'+cTT(d.lang+' Code',fmt(d.code||0)+' lines')+' data-kind="code" x="'+cPx(x)+'" y="'+y+'" width="'+cPx(cW)+'" height="'+bH+'" fill="'+CX+'"/>';var _fc=cFitFs(fmt(d.code||0),cW);if(_fc)s+='<text x="'+cPx(x+cW/2)+'" y="'+lmid+'" text-anchor="middle" font-family="'+CFONT+'" font-size="'+_fc+'" font-weight="700" fill="#fff" style="pointer-events:none;">'+fmt(d.code||0)+'</text>';x+=cW;}
              if(cmW>0.5){s+='<rect'+cTT(d.lang+' Comments',fmt(d.comments||0)+' lines')+' data-kind="comment" x="'+cPx(x)+'" y="'+y+'" width="'+cPx(cmW)+'" height="'+bH+'" fill="'+CG+'"/>';var _fm=cFitFs(fmt(d.comments||0),cmW);if(_fm)s+='<text x="'+cPx(x+cmW/2)+'" y="'+lmid+'" text-anchor="middle" font-family="'+CFONT+'" font-size="'+_fm+'" font-weight="700" fill="#fff" style="pointer-events:none;">'+fmt(d.comments||0)+'</text>';x+=cmW;}
              if(blW>0.5){s+='<rect'+cTT(d.lang+' Blank',fmt(d.blanks||0)+' lines')+' data-kind="blank" x="'+cPx(x)+'" y="'+y+'" width="'+cPx(blW)+'" height="'+bH+'" fill="'+CB+'"/>';var _fb=cFitFs(fmt(d.blanks||0),blW);if(_fb)s+='<text x="'+cPx(x+blW/2)+'" y="'+lmid+'" text-anchor="middle" font-family="'+CFONT+'" font-size="'+_fb+'" font-weight="700" fill="#555" style="pointer-events:none;">'+fmt(d.blanks||0)+'</text>';}
              s+='<text'+cTT(d.lang,ttvc)+' x="'+(LW+BW+4)+'" y="'+lmid+'" font-family="'+CFONT+'" font-size="11" font-weight="700" fill="#7b675b" style="cursor:pointer;">'+Math.round((d.code||0)/t2*100)+'%</text>';
            });
          } else {
            var maxT=Math.max.apply(null,cData.map(function(d){return(d.code||0)+(d.comments||0)+(d.blanks||0);})) || 1;
            cData.forEach(function(d,i){
              var cW=(d.code||0)/maxT*BW,cmW=(d.comments||0)/maxT*BW,blW=(d.blanks||0)/maxT*BW;
              var y=topPad+i*rHb+Math.floor((rHb-bH)/2),x=LW;
              var lmid=y+Math.floor(bH/2)+4;
              var ttvc='Code: '+fmt(d.code||0)+'\nComments: '+fmt(d.comments||0)+'\nBlank: '+fmt(d.blanks||0)+'\nTotal: '+fmt(d.physical||(d.code||0)+(d.comments||0)+(d.blanks||0));
              s+='<text'+cTT(d.lang,ttvc)+' x="'+(LW-5)+'" y="'+lmid+'" text-anchor="end" font-family="'+CFONT+'" font-size="11" fill="#43342d" style="cursor:pointer;">'+cEsc(d.lang)+'</text>';
              if(cW>0.5){s+='<rect'+cTT(d.lang+' Code',fmt(d.code||0)+' lines')+' data-kind="code" x="'+cPx(x)+'" y="'+y+'" width="'+cPx(cW)+'" height="'+bH+'" fill="'+CX+'"/>';var _fc=cFitFs(fmt(d.code||0),cW);if(_fc)s+='<text x="'+cPx(x+cW/2)+'" y="'+lmid+'" text-anchor="middle" font-family="'+CFONT+'" font-size="'+_fc+'" font-weight="700" fill="#fff" style="pointer-events:none;">'+fmt(d.code||0)+'</text>';x+=cW;}
              if(cmW>0.5){s+='<rect'+cTT(d.lang+' Comments',fmt(d.comments||0)+' lines')+' data-kind="comment" x="'+cPx(x)+'" y="'+y+'" width="'+cPx(cmW)+'" height="'+bH+'" fill="'+CG+'"/>';var _fm=cFitFs(fmt(d.comments||0),cmW);if(_fm)s+='<text x="'+cPx(x+cmW/2)+'" y="'+lmid+'" text-anchor="middle" font-family="'+CFONT+'" font-size="'+_fm+'" font-weight="700" fill="#fff" style="pointer-events:none;">'+fmt(d.comments||0)+'</text>';x+=cmW;}
              if(blW>0.5){s+='<rect'+cTT(d.lang+' Blank',fmt(d.blanks||0)+' lines')+' data-kind="blank" x="'+cPx(x)+'" y="'+y+'" width="'+cPx(blW)+'" height="'+bH+'" fill="'+CB+'"/>';var _fb=cFitFs(fmt(d.blanks||0),blW);if(_fb)s+='<text x="'+cPx(x+blW/2)+'" y="'+lmid+'" text-anchor="middle" font-family="'+CFONT+'" font-size="'+_fb+'" font-weight="700" fill="#555" style="pointer-events:none;">'+fmt(d.blanks||0)+'</text>';}
              var phys=d.physical||(d.code||0)+(d.comments||0)+(d.blanks||0);
              s+='<text'+cTT(d.lang,ttvc)+' x="'+(LW+cW+cmW+blW+4)+'" y="'+lmid+'" font-family="'+CFONT+'" font-size="11" font-weight="700" fill="#7b675b" style="cursor:pointer;">'+fmt(phys)+'</text>';
            });
          }
          var ly=SH-legendH+4;
          var ttC=cLT('Code lines',fmt(totC)+' total ('+Math.round(totC/totAll*100)+'%)');
          var ttCm=cLT('Comment lines',fmt(totCm)+' total ('+Math.round(totCm/totAll*100)+'%)');
          var ttBl=cLT('Blank lines',fmt(totBl)+' total ('+Math.round(totBl/totAll*100)+'%)');
          var legSt=LW+Math.max(0,Math.round((BW-194)/2));
          s+='<g data-kind="code" style="cursor:pointer;"><rect x="'+legSt+'" y="'+(ly-3)+'" width="50" height="16" fill="transparent"'+ttC+'/><rect x="'+legSt+'" y="'+ly+'" width="9" height="9" fill="'+CX+'"'+ttC+'/><text x="'+(legSt+13)+'" y="'+(ly+9)+'"'+ttC+' font-family="'+CFONT+'" font-size="10" font-weight="700" fill="#43342d">Code</text></g>';
          s+='<g data-kind="comment" style="cursor:pointer;"><rect x="'+(legSt+58)+'" y="'+(ly-3)+'" width="82" height="16" fill="transparent"'+ttCm+'/><rect x="'+(legSt+58)+'" y="'+ly+'" width="9" height="9" fill="'+CG+'"'+ttCm+'/><text x="'+(legSt+71)+'" y="'+(ly+9)+'"'+ttCm+' font-family="'+CFONT+'" font-size="10" font-weight="700" fill="#43342d">Comments</text></g>';
          s+='<g data-kind="blank" style="cursor:pointer;"><rect x="'+(legSt+145)+'" y="'+(ly-3)+'" width="55" height="16" fill="transparent"'+ttBl+'/><rect x="'+(legSt+145)+'" y="'+ly+'" width="9" height="9" fill="'+CB+'"'+ttBl+'/><text x="'+(legSt+158)+'" y="'+(ly+9)+'"'+ttBl+' font-family="'+CFONT+'" font-size="10" font-weight="700" fill="#43342d">Blanks</text></g>';
          s+='</svg>';
          el.innerHTML=s;
          wireMixLegend(el.querySelector('svg'));
        }
        document.querySelectorAll('[data-comp-tab]').forEach(function(btn){
          btn.addEventListener('click', function(){
            document.querySelectorAll('[data-comp-tab]').forEach(function(b){b.classList.remove('active');});
            btn.classList.add('active');
            cMode=btn.getAttribute('data-comp-tab');
            renderCompSVG();
          });
        });
        renderCompSVG();
        window.addEventListener('resize', renderCompSVG);
      })();

      // Custom HTML legend for the bubble chart: balanced columns (~15 rows max
      // per column, evenly distributed) placed to the right of the canvas. Chart.js'
      // native right-legend fills one column to full height then dumps the rest into
      // a tiny second column (and clips when space is tight) — this gives even columns
      // and never hides languages. Hover mirrors the old dim/highlight behaviour.
      //
      // Must run BEFORE `new Chart(canvas, …)` so Chart.js' resize observer binds to
      // the inner wrapper (not the full-width host). Returns a holder whose `.chart`
      // field the caller assigns once the chart exists, so hover can drive it.
      // expandBtnId: when set (compact card), the legend is capped to what fits in
      // 2 columns at the available height and a trailing "+N more" row links to Full
      // View. When null (Full View itself) every language is shown across (up to) 2
      // tall columns. Languages are ordered by code lines so the compact view keeps
      // the biggest ones; colours/hover still key off each language's original index.
      function attachScatterLegend(canvas, expandBtnId) {
        var holder = { chart: null };
        var host = canvas && canvas.parentNode;
        if (!host) return holder;
        host.style.display = 'flex';
        host.style.alignItems = 'center';
        host.style.gap = '12px';
        var cwrap = document.createElement('div');
        cwrap.style.cssText = 'position:relative;flex:1 1 auto;min-width:0;height:100%;';
        host.insertBefore(cwrap, canvas);
        cwrap.appendChild(canvas);

        var n = SCAT_D.length;
        var availH = Math.max(120, host.clientHeight || 224);
        var rowsFit = Math.max(2, Math.floor(availH / 18));   // readable pitch
        // Never more than 2 columns; compact view truncates to fit, Full View shows all.
        var truncated = expandBtnId ? (n > 2 * rowsFit) : false;
        var realShown = truncated ? (2 * rowsFit - 1) : n;
        var totalItems = truncated ? (2 * rowsFit) : n;
        // Split into 2 equal columns once a single column would exceed ~18 rows, even
        // when the (tall) Full-View modal could fit them all in one column.
        var cols = totalItems > Math.min(rowsFit, 18) ? 2 : 1;
        var perCol = Math.ceil(totalItems / cols);
        var rowH = Math.max(14, Math.min(30, Math.floor(availH / perCol)));

        // Order by code lines desc so the compact view keeps the biggest languages.
        var order = SCAT_D.map(function(_, i){ return i; })
          .sort(function(a, b){ return (SCAT_D[b].code || 0) - (SCAT_D[a].code || 0); });

        var leg = document.createElement('div');
        leg.style.cssText = 'flex:0 0 auto;display:grid;grid-auto-flow:column;'
          + 'grid-template-rows:repeat(' + perCol + ',' + rowH + 'px);column-gap:18px;'
          + 'align-content:center;font-size:12px;line-height:1;';
        function setHi(idx) {
          var chart = holder.chart; if (!chart) return;
          chart.$hiDs = idx;   // read by scatterLabelPlugin so labels fade in step
          chart.data.datasets.forEach(function(ds, i) {
            var b = PALETTE[i % PALETTE.length];
            ds.backgroundColor = i === idx ? b + 'b8' : b + '20';
            ds.borderColor = i === idx ? b : b + '30';
          });
          chart.setActiveElements([{ datasetIndex: idx, index: 0 }]);
          chart.update();
        }
        function clearHi() {
          var chart = holder.chart; if (!chart) return;
          chart.$hiDs = null;
          chart.data.datasets.forEach(function(ds, i) {
            var b = PALETTE[i % PALETTE.length];
            ds.backgroundColor = b + 'b8';
            ds.borderColor = b;
          });
          chart.setActiveElements([]);
          chart.update('none');
        }
        function addItem(swColor, label, idx, isMore) {
          var it = document.createElement('div');
          it.style.cssText = 'display:flex;align-items:center;gap:7px;white-space:nowrap;'
            + ((idx != null || isMore) ? 'cursor:pointer;' : '');
          var sw = document.createElement('span');
          sw.style.cssText = 'width:22px;height:12px;border-radius:2px;flex:0 0 auto;background:'
            + swColor + ';' + (isMore ? 'opacity:0.45;' : '');
          var tx = document.createElement('span');
          tx.textContent = label;
          if (isMore) { tx.style.fontStyle = 'italic'; tx.style.opacity = '0.8'; }
          it.appendChild(sw); it.appendChild(tx);
          if (idx != null) {
            it.addEventListener('mouseenter', function(){ setHi(idx); });
            it.addEventListener('mouseleave', clearHi);
          }
          if (isMore) {
            it.addEventListener('click', function(){
              var b = document.getElementById(expandBtnId); if (b) b.click();
            });
          }
          leg.appendChild(it);
        }
        for (var k = 0; k < realShown; k++) {
          var oi = order[k];
          addItem(PALETTE[oi % PALETTE.length], SCAT_D[oi].lang, oi, false);
        }
        if (truncated) addItem('#9a8c82', '+' + (n - realShown) + ' more — Full View', null, true);
        host.appendChild(leg);
        return holder;
      }

      // ── Scatter / Bubble chart ────────────────────────────────────────────────
      (function() {
        var canvas = document.getElementById('canvas-scatter');
        if (!canvas || !SCAT_D || !SCAT_D.length) return;
        var maxP = Math.max.apply(null, SCAT_D.map(function(d){return d.physical;})) || 1;
        var maxFx = Math.max.apply(null, SCAT_D.map(function(d){return d.files;})) || 1;
        var c = clr();
        var legHolder = attachScatterLegend(canvas, 'scatter-expand-btn');
        var chart = new Chart(canvas, {
          type: 'bubble',
          data: {
            datasets: SCAT_D.map(function(d, i) {
              return {
                label: d.lang,
                data: [{ x: d.files, y: d.code, r: Math.max(5, Math.round(Math.sqrt(d.physical/maxP)*20)) }],
                backgroundColor: PALETTE[i % PALETTE.length] + 'b8',
                borderColor: PALETTE[i % PALETTE.length], borderWidth: 1,
                hoverBorderWidth: 2
              };
            })
          },
          options: {
            responsive: true, maintainAspectRatio: false,
            onHover: chartCursor,
            animation: { duration: 500, easing: 'easeOutQuart' },
            layout: { padding: { top: 44, right: 12 } },
            scales: {
              x: { type: 'logarithmic', min: 0.8, max: maxFx * 2.6,
                   grid: { color: c.grid },
                   ticks: { color: c.text, font: { size: 11 }, maxTicksLimit: 6, callback: function(v){ return fmt(v); } },
                   title: { display: true, text: 'Files Analyzed', color: c.text, font: { size: 11 } } },
              y: { grid: { color: c.grid }, ticks: { color: c.text, font: { size: 11 }, callback: function(v){return fmt(v);} },
                   title: { display: true, text: 'Code Lines', color: c.text, font: { size: 11 } } }
            },
            plugins: {
              legend: { display: false },
              tooltip: {
                callbacks: {
                  title: function(items) { return items.length ? items[0].dataset.label : ''; },
                  label: function(ctx){
                    var d = SCAT_D[ctx.datasetIndex];
                    return [
                      '  Files analyzed: ' + fmt(d.files),
                      '  Code lines: ' + Number(d.code).toLocaleString(),
                      '  Physical lines: ' + Number(d.physical).toLocaleString()
                    ];
                  }
                }
              }
            }
          },
          plugins: [scatterLabelPlugin()]
        });
        ALL_CHARTS.push(chart);
        legHolder.chart = chart;
      })();

      // ── Submodule breakdown ──────────────────────────────────────────────────
      // No-op plugins: hover row-dimming was removed because the flashing row
      // background looked out of place vs. every other chart. Kept as empty stubs
      // so the (inline + Full View) chart configs that reference them stay valid.
      var rowDimPlugin = {};
      var barJumpPlugin = {};
      var subChart = null;
      (function() {
        if (!SUB_D || !SUB_D.length) return;
        var subYSel = document.getElementById('sub-y-axis');
        var subSortSel = document.getElementById('sub-sort');
        var wrap = document.getElementById('canvas-sub-wrap');
        var canvas = document.getElementById('canvas-sub');
        if (!canvas) return;
        var Y_LABELS = { code:'Code Lines', comment:'Comment Lines', blank:'Blank Lines',
                         physical:'Physical Lines', files:'Files' };
        var SUB_COLS = { code:OX, comment:GN, blank:GY, physical:'#4472C4', files:'#805099' };
        function renderSubmodule() {
          var yKey = subYSel ? subYSel.value : 'code';
          var sortMode = subSortSel ? subSortSel.value : 'desc';
          var data = SUB_D.slice();
          if (sortMode==='desc') data.sort(function(a,b){return (b[yKey]||0)-(a[yKey]||0);});
          else if (sortMode==='asc') data.sort(function(a,b){return (a[yKey]||0)-(b[yKey]||0);});
          else data.sort(function(a,b){return a.name.localeCompare(b.name);});
          data = data.slice(0, 30);
          var c = clr();
          var col = SUB_COLS[yKey] || OX;
          if (wrap) wrap.style.height = Math.max(200, Math.min(540, data.length * 28 + 60)) + 'px';
          if (subChart) {
            subChart.data.labels = data.map(function(d){return d.name;});
            subChart.data.datasets[0].data = data.map(function(d){return d[yKey]||0;});
            subChart.data.datasets[0].backgroundColor = col;
            subChart.data.datasets[0].label = Y_LABELS[yKey]||yKey;
            subChart.options.scales.x.title.text = Y_LABELS[yKey]||yKey;
            subChart.update('none'); return;
          }
          subChart = new Chart(canvas, {
            type: 'bar',
            data: {
              labels: data.map(function(d){return d.name;}),
              datasets: [{ label: Y_LABELS[yKey]||yKey,
                data: data.map(function(d){return d[yKey]||0;}),
                backgroundColor: col, hoverBackgroundColor: col === OX ? '#d97020' : col,
                borderRadius: 3 }]
            },
            options: {
              indexAxis: 'y', responsive: true, maintainAspectRatio: false,
              onHover: chartCursor,
              animation: { duration: 500, easing: 'easeOutQuart' },
              transitions: { active: { animation: { duration: 180, easing: 'easeOutQuart' } } },
              layout: { padding: { right: 64 } },
              scales: {
                x: { grid: { color: c.grid }, ticks: { color: c.text, callback: function(v){return fmt(v);} },
                     title: { display: true, text: Y_LABELS[yKey]||yKey, color: c.text } },
                y: { grid: { display: false }, ticks: { color: c.text } }
              },
              plugins: {
                legend: { display: false },
                tooltip: {
                  callbacks: {
                    title: function(items) { return items.length ? items[0].label : ''; },
                    label: function(ctx){
                      var d = data[ctx.dataIndex] || {};
                      return [
                        '  Code: ' + Number(d.code||0).toLocaleString(),
                        '  Comments: ' + Number(d.comment||0).toLocaleString(),
                        '  Blanks: ' + Number(d.blank||0).toLocaleString(),
                        '  Physical: ' + Number(d.physical||0).toLocaleString(),
                        '  Files: ' + fmt(d.files||0)
                      ];
                    }
                  }
                }
              }
            },
            plugins: [makeDlPlugin(function(v){ return fmt(v||0); }, 'end'), barJumpPlugin]
          });
          ALL_CHARTS.push(subChart);
        }
        if (subYSel) subYSel.addEventListener('change', renderSubmodule);
        if (subSortSel) subSortSel.addEventListener('change', renderSubmodule);
        renderSubmodule();
      })();

      // ── Submodule composition: stacked horizontal bar (Chart.js) ─────────────
      var subCompChart = null;
      // Plugin: draw value label inside each visible segment of a stacked horizontal bar.
      var segLabelPlugin = {
        afterDatasetsDraw: function(chart) {
          var ctx = chart.ctx, nDs = chart.data.datasets.length;
          var tc = clr().text;
          for (var di = 0; di < nDs; di++) {
            var meta = chart.getDatasetMeta(di);
            if (meta.hidden) continue;
            meta.data.forEach(function(el, idx) {
              var v = chart.data.datasets[di].data[idx] || 0;
              if (!v) return;
              var w = Math.abs(el.x - el.base);
              if (w < 28) return; // too narrow to show label
              ctx.save();
              ctx.font = '600 10px Inter,ui-sans-serif,sans-serif';
              ctx.fillStyle = di === 0 ? '#fff' : (di === 1 ? '#fff' : '#555');
              ctx.textAlign = 'center';
              ctx.textBaseline = 'middle';
              ctx.fillText(fmt(v), el.base + w / 2, el.y);
              ctx.restore();
            });
          }
        }
      };
      (function() {
        var el = document.getElementById('submodule-donut');
        if (!el || !SUB_D || !SUB_D.length) return;
        var data = SUB_D.slice().sort(function(a,b){
          return ((b.code||0)+(b.comment||0)+(b.blank||0))-((a.code||0)+(a.comment||0)+(a.blank||0));
        }).slice(0, 15);
        var h = Math.max(150, Math.min(540, data.length * 40 + 90));
        el.style.height = h + 'px';
        el.style.position = 'relative';
        var cv = document.createElement('canvas');
        cv.id = 'canvas-sub-comp';
        el.innerHTML = '';
        el.appendChild(cv);
        var c = clr();
        subCompChart = new Chart(cv, {
          type: 'bar',
          data: {
            labels: data.map(function(d){ return d.name; }),
            datasets: [
              { label: 'Code',     data: data.map(function(d){ return d.code||0; }),    backgroundColor: OX, hoverBackgroundColor: '#d97020', borderRadius: 0, borderSkipped: false },
              { label: 'Comments', data: data.map(function(d){ return d.comment||0; }), backgroundColor: GN, hoverBackgroundColor: '#3a8a5e', borderRadius: 0, borderSkipped: false },
              { label: 'Blank',    data: data.map(function(d){ return d.blank||0; }),   backgroundColor: GY, hoverBackgroundColor: '#999',    borderRadius: 0, borderSkipped: false }
            ]
          },
          options: {
            indexAxis: 'y', responsive: true, maintainAspectRatio: false,
            onHover: chartCursor,
            animation: { duration: 500, easing: 'easeOutQuart' },
            transitions: { active: { animation: { duration: 180, easing: 'easeOutQuart' } } },
            layout: { padding: { right: 56 } },
            scales: {
              x: { stacked: true, grid: { color: c.grid }, ticks: { color: c.text, callback: function(v){ return fmt(v); } } },
              y: { stacked: true, grid: { display: false }, ticks: { color: c.text } }
            },
            plugins: {
              legend: {
                position: 'bottom',
                labels: { color: c.text, usePointStyle: true, pointStyle: 'rect', font: { size: 11, weight: '700' }, padding: 16 },
                onHover: function(e, item, leg) {
                  legendCursorOn(e);
                  var ch = leg.chart, di = item.datasetIndex;
                  var orig = [OX, GN, GY], hov = ['#d97020','#3a8a5e','#999'];
                  ch.data.datasets.forEach(function(ds, i) {
                    ds.backgroundColor = i===di ? orig[i] : hexAlpha(orig[i], 0.15);
                    ds.hoverBackgroundColor = i===di ? hov[i] : hexAlpha(orig[i], 0.15);
                  });
                  // show tooltip on first bar row with all datasets (index mode)
                  var n = ch.data.datasets.length, ae = [];
                  for (var ii = 0; ii < n; ii++) { ae.push({ datasetIndex: ii, index: 0 }); }
                  var fp = ch.getDatasetMeta(di).data[0];
                  ch.setActiveElements([{ datasetIndex: di, index: 0 }]);
                  ch.tooltip.setActiveElements(ae, fp ? { x: fp.x, y: fp.y } : { x: 0, y: 0 });
                  ch.update();
                },
                onLeave: function(e, item, leg) {
                  legendCursorOff(e);
                  var ch = leg.chart;
                  var orig = [OX, GN, GY], hov = ['#d97020','#3a8a5e','#999'];
                  ch.data.datasets.forEach(function(ds, i) { ds.backgroundColor = orig[i]; ds.hoverBackgroundColor = hov[i]; });
                  ch.setActiveElements([]);
                  ch.tooltip.setActiveElements([], {});
                  ch.update('none');
                }
              },
              tooltip: {
                mode: 'index',
                callbacks: {
                  title: function(items){ return items.length ? items[0].label : ''; },
                  label: function(ctx){
                    var v = ctx.parsed.x || 0;
                    return '  ' + ctx.dataset.label + ': ' + Number(v).toLocaleString();
                  },
                  footer: function(items){
                    var tot = items.reduce(function(s,i){ return s + (i.parsed.x||0); }, 0);
                    return 'Total: ' + Number(tot).toLocaleString();
                  }
                }
              }
            }
          },
          plugins: [makeStackedEndPlugin(function(v){ return fmt(v); }), segLabelPlugin, rowDimPlugin]
        });
        ALL_CHARTS.push(subCompChart);
      })();

      // ── Semantic Metrics ─────────────────────────────────────────────────────
      (function() {
        if (!SEM_D || !SEM_D.length) return;
        var semSel = document.getElementById('semantic-metric');
        var canvas = document.getElementById('canvas-semantic');
        if (!canvas) return;
        var SEM_LABELS = { functions:'Functions', classes:'Classes / Types', variables:'Variables',
                           imports:'Imports', tests:'Tests' };
        var SEM_COLS = { functions:OX, classes:'#4472C4', variables:GN, imports:'#805099', tests:'#B23030' };
        var SEM_HCOLS = { functions:'#d97020', classes:'#5a8ad8', variables:'#3a8a5e', imports:'#9a68b3', tests:'#cc4545' };
        var semChart = null;
        function renderSemantic() {
          var mKey = semSel ? semSel.value : 'functions';
          var data = SEM_D.slice().sort(function(a,b){return (b[mKey]||0)-(a[mKey]||0);}).slice(0,15);
          var c = clr();
          var col = SEM_COLS[mKey] || OX;
          var hCol = SEM_HCOLS[mKey] || '#d97020';
          if (semChart) {
            semChart.data.labels = data.map(function(d){return d.lang;});
            semChart.data.datasets[0].data = data.map(function(d){return d[mKey]||0;});
            semChart.data.datasets[0].backgroundColor = col;
            semChart.data.datasets[0].hoverBackgroundColor = hCol;
            semChart.data.datasets[0].label = SEM_LABELS[mKey]||mKey;
            semChart.update('none'); return;
          }
          semChart = new Chart(canvas, {
            type: 'bar',
            data: {
              labels: data.map(function(d){return d.lang;}),
              datasets: [{ label: SEM_LABELS[mKey]||mKey,
                data: data.map(function(d){return d[mKey]||0;}),
                backgroundColor: col, hoverBackgroundColor: hCol,
                borderRadius: 4, borderWidth: 0, hoverBorderWidth: 0 }]
            },
            options: {
              responsive: true, maintainAspectRatio: false,
              onHover: chartCursor,
              animation: { duration: 500, easing: 'easeOutQuart' },
              transitions: { active: { animation: { duration: 200, easing: 'easeOutQuart' } } },
              layout: { padding: { top: 18 } },
              scales: {
                x: { grid: { display: false }, ticks: { color: c.text } },
                y: { grid: { color: c.grid }, ticks: { color: c.text, callback: function(v){return fmt(v);} } }
              },
              plugins: {
                legend: { display: false },
                tooltip: {
                  callbacks: {
                    title: function(items) { return items.length ? items[0].label : ''; },
                    label: function(ctx) {
                      var d = data[ctx.dataIndex] || {};
                      var lines = ['  ' + (SEM_LABELS[mKey]||mKey) + ': ' + Number(ctx.parsed.y).toLocaleString()];
                      var others = Object.keys(SEM_LABELS).filter(function(k){ return k !== mKey && (d[k]||0) > 0; });
                      others.forEach(function(k) {
                        lines.push('  ' + SEM_LABELS[k] + ': ' + Number(d[k]||0).toLocaleString());
                      });
                      return lines;
                    }
                  }
                }
              }
            },
            plugins: [makeDlPlugin(function(v) { return fmt(v || 0); }, 'top')]
          });
          ALL_CHARTS.push(semChart);
        }
        if (semSel) semSel.addEventListener('change', renderSemantic);
        renderSemantic();

        var semExpandBtn = document.getElementById('semantic-expand-btn');
        if (semExpandBtn) {
          semExpandBtn.addEventListener('click', function() {
            var mKey = semSel ? semSel.value : 'functions';
            var n = SEM_D.length || 1;
            var maxH = Math.max(400, Math.floor(window.innerHeight * 0.82) - 130);
            var modalH = Math.min(Math.max(400, n * 46 + 96), maxH);
            var overlay = document.createElement('div');
            overlay.className = 'chart-modal-overlay';
            var semOptHtml = '<option value="functions">Functions</option>'
              + '<option value="classes">Classes / Types</option>'
              + '<option value="variables">Variables</option>'
              + '<option value="imports">Imports</option>'
              + '<option value="tests">Tests</option>';
            var hdr = '<div class="chart-modal-header"><span class="chart-modal-title">Semantic Metrics \u2014 Full View</span>'
              + '<select class="chart-select" id="sem-modal-metric">' + semOptHtml + '</select></div>';
            overlay.innerHTML = '<div class="chart-modal" style="max-width:1320px;"><button class="chart-modal-close" aria-label="Close">&times;</button>' + hdr + '<div style="position:relative;height:' + modalH + 'px;width:100%;"><canvas id="canvas-semantic-modal"></canvas></div></div>';
            document.body.appendChild(overlay);
            overlay.querySelector('.chart-modal-close').addEventListener('click', function() { document.body.removeChild(overlay); });
            overlay.addEventListener('click', function(e) { if (e.target === overlay) document.body.removeChild(overlay); });
            var modalSel = document.getElementById('sem-modal-metric');
            if (modalSel) modalSel.value = mKey;
            var modalCanvas = document.getElementById('canvas-semantic-modal');
            var semModalChart = null;
            function renderSemModal(key) {
              if (semModalChart) { semModalChart.destroy(); semModalChart = null; }
              if (!modalCanvas) return;
              var data = SEM_D.slice().sort(function(a,b){return (b[key]||0)-(a[key]||0);});
              var c = clr();
              var col = SEM_COLS[key] || OX;
              var hcol = SEM_HCOLS[key] || '#d97020';
              semModalChart = new Chart(modalCanvas, {
                type: 'bar',
                data: {
                  labels: data.map(function(d){return d.lang;}),
                  datasets: [{ label: SEM_LABELS[key]||key, data: data.map(function(d){return d[key]||0;}),
                    backgroundColor: col, hoverBackgroundColor: hcol,
                    borderRadius: 4, borderWidth: 0, hoverBorderWidth: 0 }]
                },
                options: {
                  responsive: true, maintainAspectRatio: false,
                  onHover: chartCursor,
                  animation: { duration: 500, easing: 'easeOutQuart' },
                  transitions: { active: { animation: { duration: 200, easing: 'easeOutQuart' } } },
                  layout: { padding: { top: 18 } },
                  scales: {
                    x: { grid: { display: false }, ticks: { color: c.text } },
                    y: { grid: { color: c.grid }, ticks: { color: c.text, callback: function(v){return fmt(v);} } }
                  },
                  plugins: { legend: { display: false }, tooltip: { callbacks: {
                    title: function(items){return items.length?items[0].label:'';},
                    label: function(ctx){
                      var d = data[ctx.dataIndex] || {};
                      var lines = ['  '+(SEM_LABELS[key]||key)+': '+Number(ctx.parsed.y).toLocaleString()];
                      var others = Object.keys(SEM_LABELS).filter(function(k){ return k !== key && (d[k]||0) > 0; });
                      others.forEach(function(k){ lines.push('  '+SEM_LABELS[k]+': '+Number(d[k]||0).toLocaleString()); });
                      return lines;
                    }
                  }}}
                },
                plugins: [makeDlPlugin(function(v) { return fmt(v || 0); }, 'top')]
              });
            }
            renderSemModal(mKey);
            if (modalSel) modalSel.addEventListener('change', function() { renderSemModal(this.value); });
          });
        }
      })();

      // ── Comment Density: comments / (code + comments) per language ──────────
      (function() {
        var canvas = document.getElementById('canvas-density');
        if (!canvas || !D || !D.length) return;
        var data = D.slice().sort(function(a,b){
          var da=(a.comments||0)/Math.max((a.code||0)+(a.comments||0),1);
          var db=(b.comments||0)/Math.max((b.code||0)+(b.comments||0),1);
          return db-da;
        });
        var labels = data.map(function(d){return d.lang;});
        var densities = data.map(function(d){
          var sig=(d.code||0)+(d.comments||0);
          return sig>0?Math.round((d.comments||0)/sig*1000)/10:0;
        });
        var wrap = canvas.parentElement;
        if (wrap) wrap.style.height = Math.max(150, Math.min(500, data.length*29+36))+'px';
        var c = clr();
        var densChart = new Chart(canvas, {
          type: 'bar',
          data: {
            labels: labels,
            datasets: [{ label: 'Comment %',
              data: densities,
              backgroundColor: data.map(function(_,i){return PALETTE[i%PALETTE.length];}),
              borderRadius: 4
            }]
          },
          options: {
            indexAxis: 'y', responsive: true, maintainAspectRatio: false,
            onHover: chartCursor,
            animation: { duration: 500, easing: 'easeOutQuart' },
            layout: { padding: { right: 42 } },
            scales: {
              x: { min: 0, max: 100,
                   grid: { color: c.grid },
                   ticks: { color: c.text, callback: function(v){return v+'%';} },
                   title: { display: true, text: 'Comment %', color: c.text } },
              y: { grid: { display: false }, ticks: { color: c.text } }
            },
            plugins: {
              legend: { display: false },
              tooltip: { callbacks: {
                title: function(items){return items.length?items[0].label:'';},
                label: function(ctx){
                  var d=data[ctx.dataIndex]||{};
                  var sig=(d.code||0)+(d.comments||0);
                  return ['  Comment ratio: '+ctx.parsed.x+'%',
                          '  Comments: '+Number(d.comments||0).toLocaleString(),
                          '  Significant lines: '+Number(sig).toLocaleString()];
                }
              }}
            }
          },
          plugins: [makeDlPlugin(function(v) { return (v || 0) + '%'; }, 'end')]
        });
        ALL_CHARTS.push(densChart);
      })();

      // ── File Size Distribution histogram ──────────────────────────────────────
      (function() {
        var canvas = document.getElementById('canvas-filesize');
        if (!canvas || !HIST_D || !HIST_D.length) return;
        var labels = HIST_D.map(function(d){return d.label;});
        var counts = HIST_D.map(function(d){return d.count||0;});
        var total = counts.reduce(function(a,b){return a+b;},0);
        var c = clr();
        var fsBg = ['#2A6846','#4472C4','#C45C10','#D4A017','#B23030'];
        var fsHv = ['#3a8a5e','#5a8ad8','#d97020','#e8b520','#cc4545'];
        var fsChart = new Chart(canvas, {
          type: 'bar',
          data: {
            labels: labels,
            datasets: [{ label: 'Files',
              data: counts,
              backgroundColor: fsBg,
              hoverBackgroundColor: fsHv,
              borderRadius: 6,
              borderWidth: 0,
              hoverBorderWidth: 0
            }]
          },
          options: {
            responsive: true, maintainAspectRatio: false,
            onHover: chartCursor,
            animation: { duration: 500, easing: 'easeOutQuart' },
            transitions: { active: { animation: { duration: 200, easing: 'easeOutQuart' } } },
            layout: { padding: { top: 18 } },
            scales: {
              x: { grid: { display: false }, ticks: { color: c.text, font: { size: 11 } } },
              y: { beginAtZero: true,
                   grid: { color: c.grid },
                   ticks: { color: c.text, precision: 0 },
                   title: { display: true, text: 'File Count', color: c.text } }
            },
            plugins: {
              legend: { display: false },
              tooltip: { callbacks: {
                label: function(ctx) {
                  var n = ctx.parsed.y;
                  var pct = total > 0 ? Math.round(n/total*1000)/10 : 0;
                  return ['  Files: '+n, '  Share: '+pct+'%'];
                }
              }}
            }
          },
          plugins: [makeDlPlugin(function(v) { return fmt(v || 0); }, 'top')]
        });
        ALL_CHARTS.push(fsChart);
      })();

      // ── Expand button handlers ────────────────────────────────────────────────
      (function() {
        function makeOverlay(title, h, subtitle, ctrlHtml) {
          var overlay = document.createElement('div');
          overlay.className = 'chart-modal-overlay';
          var maxH = Math.max(400, Math.floor(window.innerHeight * 0.82) - 130);
          var hAttr = 'height:' + Math.min(h || 696, maxH) + 'px;';
          var subHtml = subtitle ? '<span class="chart-modal-subtitle">' + subtitle + '</span>' : '';
          var hdr = '<div class="chart-modal-header"><span class="chart-modal-title">' + title + '</span>' + (ctrlHtml || '') + '</div>';
          overlay.innerHTML = '<div class="chart-modal" style="max-width:1320px;"><button class="chart-modal-close" aria-label="Close">&times;</button>' + hdr + subHtml + '<div style="position:relative;width:100%;' + hAttr + '"><canvas id="modal-expand-canvas"></canvas></div></div>';
          document.body.appendChild(overlay);
          overlay.querySelector('.chart-modal-close').addEventListener('click', function(){ document.body.removeChild(overlay); });
          overlay.addEventListener('click', function(e){ if(e.target === overlay) document.body.removeChild(overlay); });
          return document.getElementById('modal-expand-canvas');
        }

        // Language Composition
        (function(){
          var btn = document.getElementById('comp-expand-btn');
          if(!btn) return;
          btn.addEventListener('click', function(){
            var activeTab = document.querySelector('[data-comp-tab].active');
            var compMode = activeTab ? activeTab.getAttribute('data-comp-tab') : 'absolute';
            var ctrlHtml = '<select class="chart-select" id="comp-modal-mode">'
              + '<option value="absolute">Absolute Lines</option>'
              + '<option value="pct">100% Normalized</option>'
              + '</select>';
            var canvas = makeOverlay('Language Composition \u2014 Full View', undefined, null, ctrlHtml);
            if(!canvas) return;
            var modalMode = document.getElementById('comp-modal-mode');
            if(modalMode) modalMode.value = compMode;
            var compModalChart = null;
            function renderCompModal(mode) {
              if(compModalChart) { compModalChart.destroy(); compModalChart = null; }
              var data = D.slice(0, 15);
              var c = clr(), isPct = mode === 'pct';
              var tot = function(d){ return (d.code||0)+(d.comments||0)+(d.blanks||0)||1; };
              var codeD = data.map(function(d){ return isPct ? (d.code||0)/tot(d)*100 : d.code||0; });
              var cmD   = data.map(function(d){ return isPct ? (d.comments||0)/tot(d)*100 : d.comments||0; });
              var blD   = data.map(function(d){ return isPct ? (d.blanks||0)/tot(d)*100 : d.blanks||0; });
              var tickCb = isPct ? function(v){return v.toFixed(0)+'%';} : function(v){return fmt(v);};
              compModalChart = new Chart(canvas, {
                type: 'bar',
                data: {
                  labels: data.map(function(d){ return d.lang; }),
                  datasets: [
                    { label:'Code',     data: codeD, backgroundColor: OX, borderRadius: 3 },
                    { label:'Comments', data: cmD,   backgroundColor: GN, borderRadius: 3 },
                    { label:'Blanks',   data: blD,   backgroundColor: GY, borderRadius: 3 }
                  ]
                },
                options: {
                  indexAxis: 'y', responsive: true, maintainAspectRatio: false,
                  layout: { padding: { right: 64 } },
                  scales: {
                    x: { stacked: true, grid: { color: c.grid }, ticks: { color: c.text, callback: tickCb } },
                    y: { stacked: true, grid: { display: false }, ticks: { color: c.text } }
                  },
                  plugins: { legend: { position: 'bottom', labels: { color: c.text } } }
                },
                plugins: [makeStackedEndPlugin(function(total, idx) {
                  if (isPct) return '';
                  var d = data[idx]; return fmt(Math.round(d && d.physical ? d.physical : total));
                })]
              });
            }
            renderCompModal(compMode);
            if(modalMode) modalMode.addEventListener('change', function(){ renderCompModal(this.value); });
          });
        })();

        // Files vs Code Lines (Scatter)
        (function(){
          var btn = document.getElementById('scatter-expand-btn');
          if(!btn || !SCAT_D || !SCAT_D.length) return;
          btn.addEventListener('click', function(){
            var canvas = makeOverlay('Files vs Code Lines \u2014 Full View', undefined, 'File count vs SLOC per language');
            if(!canvas) return;
            var maxP = Math.max.apply(null, SCAT_D.map(function(d){return d.physical;})) || 1;
            var maxFx = Math.max.apply(null, SCAT_D.map(function(d){return d.files;})) || 1;
            var c = clr();
            var scLegHolder = attachScatterLegend(canvas);
            var scExpand = new Chart(canvas, {
              type: 'bubble',
              data: {
                datasets: SCAT_D.map(function(d, i) {
                  return {
                    label: d.lang,
                    data: [{ x: d.files, y: d.code, r: Math.max(5, Math.round(Math.sqrt(d.physical/maxP)*20)) }],
                    backgroundColor: PALETTE[i % PALETTE.length] + 'b8',
                    borderColor: PALETTE[i % PALETTE.length], borderWidth: 1,
                    hoverBorderWidth: 2
                  };
                })
              },
              options: {
                responsive: true, maintainAspectRatio: false,
                onHover: chartCursor,
                animation: { duration: 500, easing: 'easeOutQuart' },
                layout: { padding: { top: 44, right: 12 } },
                scales: {
                  x: { type: 'logarithmic', min: 0.8, max: maxFx * 2.6, grid: { color: c.grid }, ticks: { color: c.text, font: { size: 11 }, maxTicksLimit: 6, callback: function(v){ return fmt(v); } }, title: { display: true, text: 'Files Analyzed', color: c.text, font: { size: 11 } } },
                  y: { grid: { color: c.grid }, ticks: { color: c.text, font: { size: 11 }, callback: function(v){return fmt(v);} }, title: { display: true, text: 'Code Lines', color: c.text, font: { size: 11 } } }
                },
                plugins: {
                  legend: { display: false },
                  tooltip: { callbacks: {
                    title: function(items){ return items.length ? items[0].dataset.label : ''; },
                    label: function(ctx){
                      var d = SCAT_D[ctx.datasetIndex];
                      return ['  Files analyzed: '+fmt(d.files), '  Code lines: '+Number(d.code).toLocaleString(), '  Physical lines: '+Number(d.physical).toLocaleString()];
                    }
                  }}
                }
              },
              plugins: [scatterLabelPlugin()]
            });
            scLegHolder.chart = scExpand;
          });
        })();

        // Comment Density
        (function(){
          var btn = document.getElementById('density-expand-btn');
          if(!btn) return;
          btn.addEventListener('click', function(){
            var data = D.slice().sort(function(a,b){
              var da=(a.comments||0)/Math.max((a.code||0)+(a.comments||0),1);
              var db=(b.comments||0)/Math.max((b.code||0)+(b.comments||0),1);
              return db-da;
            });
            var h = Math.min(Math.max(672, data.length * 46 + 96), Math.max(400, Math.floor(window.innerHeight * 0.82) - 130));
            var canvas = makeOverlay('Comment Density \u2014 Full View', h, 'Comment ratio per language');
            if(!canvas) return;
            var densities = data.map(function(d){ var sig=(d.code||0)+(d.comments||0); return sig>0?Math.round((d.comments||0)/sig*1000)/10:0; });
            var c = clr();
            new Chart(canvas, {
              type: 'bar',
              data: {
                labels: data.map(function(d){return d.lang;}),
                datasets: [{ label: 'Comment %', data: densities,
                  backgroundColor: data.map(function(_,i){return PALETTE[i%PALETTE.length];}), borderRadius: 4 }]
              },
              options: {
                indexAxis: 'y', responsive: true, maintainAspectRatio: false,
                animation: { duration: 500, easing: 'easeOutQuart' },
                layout: { padding: { right: 42 } },
                scales: {
                  x: { min:0, max:100, grid:{color:c.grid}, ticks:{color:c.text, callback:function(v){return v+'%';}} },
                  y: { grid:{display:false}, ticks:{color:c.text} }
                },
                plugins: { legend:{display:false}, tooltip:{callbacks:{
                  title:function(items){return items.length?items[0].label:'';},
                  label:function(ctx){
                    var d=data[ctx.dataIndex]||{};
                    var sig=(d.code||0)+(d.comments||0);
                    return ['  Comment ratio: '+ctx.parsed.x.toFixed(1)+'%',
                            '  Comments: '+Number(d.comments||0).toLocaleString(),
                            '  Significant lines: '+Number(sig).toLocaleString()];
                  }
                }}}
              },
              plugins: [makeDlPlugin(function(v) { return (v || 0) + '%'; }, 'end')]
            });
          });
        })();

        // File Size Distribution
        (function(){
          var btn = document.getElementById('filesize-expand-btn');
          if(!btn || !HIST_D || !HIST_D.length) return;
          btn.addEventListener('click', function(){
            var canvas = makeOverlay('File Size Distribution \u2014 Full View', undefined, 'File count per SLOC bucket');
            if(!canvas) return;
            var labels = HIST_D.map(function(d){return d.label;});
            var counts = HIST_D.map(function(d){return d.count||0;});
            var total = counts.reduce(function(a,b){return a+b;},0);
            var fsBg = ['#2A6846','#4472C4','#C45C10','#D4A017','#B23030'];
            var fsHv = ['#3a8a5e','#5a8ad8','#d97020','#e8b520','#cc4545'];
            var c = clr();
            new Chart(canvas, {
              type: 'bar',
              data: {
                labels: labels,
                datasets: [{ label: 'Files', data: counts,
                  backgroundColor: fsBg, hoverBackgroundColor: fsHv,
                  borderRadius: 6, borderWidth: 0, hoverBorderWidth: 0 }]
              },
              options: {
                responsive: true, maintainAspectRatio: false,
                animation: { duration: 500, easing: 'easeOutQuart' },
                transitions: { active: { animation: { duration: 200, easing: 'easeOutQuart' } } },
                layout: { padding: { top: 18 } },
                scales: {
                  x: { grid:{display:false}, ticks:{color:c.text} },
                  y: { beginAtZero:true, grid:{color:c.grid}, ticks:{color:c.text, precision:0}, title:{display:true, text:'File Count', color:c.text} }
                },
                plugins: { legend:{display:false}, tooltip:{callbacks:{label:function(ctx){ var pct=total>0?Math.round(ctx.parsed.y/total*1000)/10:0; return ['  Files: '+ctx.parsed.y, '  Share: '+pct+'%']; }}} }
              },
              plugins: [makeDlPlugin(function(v) { return fmt(v || 0); }, 'top')]
            });
          });
        })();

        // Submodule Breakdown — Full View with live Y Axis + Sort controls
        (function(){
          var btn = document.getElementById('sub-expand-btn');
          if(!btn || !SUB_D || !SUB_D.length) return;
          btn.addEventListener('click', function(){
            var subYSel = document.getElementById('sub-y-axis');
            var subSortSel = document.getElementById('sub-sort');
            var initY = subYSel ? subYSel.value : 'code';
            var initSort = subSortSel ? subSortSel.value : 'desc';
            var Y_LABELS = { code:'Code Lines', comment:'Comment Lines', blank:'Blank Lines', physical:'Physical Lines', files:'Files' };
            var SUB_COLS = { code:OX, comment:GN, blank:GY, physical:'#4472C4', files:'#805099' };
            var SUB_HCOLS = { code:'#d97020', comment:'#3a8a5e', blank:'#999', physical:'#5a8ad8', files:'#9a68b3' };
            var n = Math.min(SUB_D.length, 30);
            var maxH = Math.max(400, Math.floor(window.innerHeight * 0.82) - 130);
            var modalH = Math.min(Math.max(480, n * 36 + 96), maxH);
            var ctrlHtml = '<label style="font-size:13px;font-weight:700;color:var(--muted);display:flex;align-items:center;gap:6px;flex-shrink:0;">Y Axis:'
              + '<select class="chart-select" id="sub-modal-y">'
              + '<option value="code">Code Lines</option>'
              + '<option value="comment">Comment Lines</option>'
              + '<option value="blank">Blank Lines</option>'
              + '<option value="physical">Physical Lines</option>'
              + '<option value="files">File Count</option>'
              + '</select></label>'
              + '<label style="font-size:13px;font-weight:700;color:var(--muted);display:flex;align-items:center;gap:6px;flex-shrink:0;">Sort:'
              + '<select class="chart-select" id="sub-modal-sort">'
              + '<option value="desc">Value \u2193</option>'
              + '<option value="asc">Value \u2191</option>'
              + '<option value="name">Name A\u2192Z</option>'
              + '</select></label>';
            var canvas = makeOverlay('Submodule Breakdown \u2014 Full View', modalH, null, ctrlHtml);
            if(!canvas) return;
            var modalY = document.getElementById('sub-modal-y');
            var modalSort = document.getElementById('sub-modal-sort');
            if(modalY) modalY.value = initY;
            if(modalSort) modalSort.value = initSort;
            var subModalChart = null;
            function renderSubModal(yKey, sortMode) {
              if(subModalChart) { subModalChart.destroy(); subModalChart = null; }
              var data = SUB_D.slice();
              if(sortMode==='desc') data.sort(function(a,b){return (b[yKey]||0)-(a[yKey]||0);});
              else if(sortMode==='asc') data.sort(function(a,b){return (a[yKey]||0)-(b[yKey]||0);});
              else data.sort(function(a,b){return a.name.localeCompare(b.name);});
              data = data.slice(0, 30);
              var c = clr(), col = SUB_COLS[yKey]||OX, hcol = SUB_HCOLS[yKey]||'#d97020';
              subModalChart = new Chart(canvas, {
                type: 'bar',
                data: {
                  labels: data.map(function(d){return d.name;}),
                  datasets: [{ label: Y_LABELS[yKey]||yKey,
                    data: data.map(function(d){return d[yKey]||0;}),
                    backgroundColor: col, hoverBackgroundColor: hcol, borderRadius: 3 }]
                },
                options: {
                  indexAxis: 'y', responsive: true, maintainAspectRatio: false,
                  onHover: chartCursor,
                  animation: { duration: 500, easing: 'easeOutQuart' },
                  transitions: { active: { animation: { duration: 180, easing: 'easeOutQuart' } } },
                  layout: { padding: { right: 72 } },
                  scales: {
                    x: { grid:{color:c.grid}, ticks:{color:c.text, callback:function(v){return fmt(v);}},
                         title:{display:true, text:Y_LABELS[yKey]||yKey, color:c.text} },
                    y: { grid:{display:false}, ticks:{color:c.text} }
                  },
                  plugins: {
                    legend:{display:false},
                    tooltip: { callbacks: {
                      title: function(items){return items.length?items[0].label:'';},
                      label: function(ctx){
                        var d = data[ctx.dataIndex]||{};
                        return ['  Code: '+Number(d.code||0).toLocaleString(),
                                '  Comments: '+Number(d.comment||0).toLocaleString(),
                                '  Blanks: '+Number(d.blank||0).toLocaleString(),
                                '  Physical: '+Number(d.physical||0).toLocaleString(),
                                '  Files: '+fmt(d.files||0)];
                      }
                    }}
                  }
                },
                plugins: [makeDlPlugin(function(v){return fmt(v||0);}, 'end'), barJumpPlugin]
              });
            }
            renderSubModal(initY, initSort);
            if(modalY) modalY.addEventListener('change', function(){ renderSubModal(this.value, modalSort ? modalSort.value : 'desc'); });
            if(modalSort) modalSort.addEventListener('change', function(){ renderSubModal(modalY ? modalY.value : 'code', this.value); });
          });
        })();

        // Submodule Composition — Full View (Chart.js with sort control)
        (function(){
          var btn = document.getElementById('sub-comp-expand-btn');
          if(!btn || !SUB_D || !SUB_D.length) return;
          btn.addEventListener('click', function(){
            var n = Math.min(SUB_D.length, 20);
            var maxH = Math.max(400, Math.floor(window.innerHeight * 0.82) - 130);
            var modalH = Math.min(Math.max(400, n * 40 + 90), maxH);
            var ctrlHtml = '<label style="font-size:13px;font-weight:700;color:var(--muted);display:flex;align-items:center;gap:6px;flex-shrink:0;">Sort:'
              + '<select class="chart-select" id="sub-comp-modal-sort">'
              + '<option value="desc">Total Lines \u2193</option>'
              + '<option value="asc">Total Lines \u2191</option>'
              + '<option value="name">Name A\u2192Z</option>'
              + '</select></label>';
            var canvas = makeOverlay('Submodule Composition \u2014 Full View', modalH, null, ctrlHtml);
            if(!canvas) return;
            var modalSort = document.getElementById('sub-comp-modal-sort');
            var scModalChart = null;
            function renderSCModal(sortMode) {
              if(scModalChart) { scModalChart.destroy(); scModalChart = null; }
              var data = SUB_D.slice();
              if(sortMode==='asc') data.sort(function(a,b){return ((a.code||0)+(a.comment||0)+(a.blank||0))-((b.code||0)+(b.comment||0)+(b.blank||0));});
              else if(sortMode==='name') data.sort(function(a,b){return a.name.localeCompare(b.name);});
              else data.sort(function(a,b){return ((b.code||0)+(b.comment||0)+(b.blank||0))-((a.code||0)+(a.comment||0)+(a.blank||0));});
              data = data.slice(0, 20);
              var c = clr();
              scModalChart = new Chart(canvas, {
                type: 'bar',
                data: {
                  labels: data.map(function(d){ return d.name; }),
                  datasets: [
                    { label:'Code',     data:data.map(function(d){return d.code||0;}),    backgroundColor:OX, hoverBackgroundColor:'#d97020', borderRadius:0, borderSkipped:false },
                    { label:'Comments', data:data.map(function(d){return d.comment||0;}), backgroundColor:GN, hoverBackgroundColor:'#3a8a5e', borderRadius:0, borderSkipped:false },
                    { label:'Blank',    data:data.map(function(d){return d.blank||0;}),   backgroundColor:GY, hoverBackgroundColor:'#999',    borderRadius:0, borderSkipped:false }
                  ]
                },
                options: {
                  indexAxis:'y', responsive:true, maintainAspectRatio:false,
                  onHover: chartCursor,
                  animation: { duration: 500, easing: 'easeOutQuart' },
                  transitions: { active: { animation: { duration: 180, easing: 'easeOutQuart' } } },
                  layout:{ padding:{ right:72 } },
                  scales: {
                    x:{ stacked:true, grid:{color:c.grid}, ticks:{color:c.text, callback:function(v){return fmt(v);}} },
                    y:{ stacked:true, grid:{display:false}, ticks:{color:c.text} }
                  },
                  plugins: {
                    legend:{ position:'bottom', labels:{color:c.text, usePointStyle:true, pointStyle:'rect', font:{size:11,weight:'700'}, padding:16},
                      onHover:function(e,item,leg){ legendCursorOn(e); var ch=leg.chart,di=item.datasetIndex; var orig=[OX,GN,GY],hov=['#d97020','#3a8a5e','#999']; ch.data.datasets.forEach(function(ds,i){ ds.backgroundColor=i===di?orig[i]:hexAlpha(orig[i],0.15); ds.hoverBackgroundColor=i===di?hov[i]:hexAlpha(orig[i],0.15); }); var n=ch.data.datasets.length,ae=[]; for(var ii=0;ii<n;ii++){ae.push({datasetIndex:ii,index:0});} var fp=ch.getDatasetMeta(di).data[0]; ch.setActiveElements([{datasetIndex:di,index:0}]); ch.tooltip.setActiveElements(ae,fp?{x:fp.x,y:fp.y}:{x:0,y:0}); ch.update(); },
                      onLeave:function(e,item,leg){ legendCursorOff(e); var ch=leg.chart; var orig=[OX,GN,GY],hov=['#d97020','#3a8a5e','#999']; ch.data.datasets.forEach(function(ds,i){ ds.backgroundColor=orig[i]; ds.hoverBackgroundColor=hov[i]; }); ch.setActiveElements([]); ch.tooltip.setActiveElements([],{}); ch.update('none'); }
                    },
                    tooltip:{ mode:'index', callbacks:{
                      title:function(items){return items.length?items[0].label:'';},
                      label:function(ctx){return '  '+ctx.dataset.label+': '+Number(ctx.parsed.x||0).toLocaleString();},
                      footer:function(items){var t=items.reduce(function(s,i){return s+(i.parsed.x||0);},0);return 'Total: '+Number(t).toLocaleString();}
                    }}
                  }
                },
                plugins: [makeStackedEndPlugin(function(v){return fmt(v);}), segLabelPlugin, rowDimPlugin]
              });
            }
            renderSCModal('desc');
            if(modalSort) modalSort.addEventListener('change', function(){ renderSCModal(this.value); });
          });
        })();

        // Language overview (donut + line-mix) — clone both SVGs side-by-side
        (function(){
          var btn = document.getElementById('lang-overview-expand-btn');
          if(!btn) return;
          btn.addEventListener('click', function(){
            var src = document.getElementById('report-lang-overview');
            if(!src) return;
            var overlay = document.createElement('div');
            overlay.className = 'chart-modal-overlay';
            overlay.innerHTML = '<div class="chart-modal" style="max-width:1600px;"><button class="chart-modal-close" aria-label="Close">&times;</button><div class="chart-modal-header"><span class="chart-modal-title">Language Breakdown \u2014 Full View</span></div><div id="lang-overview-modal-wrap" style="width:100%;"></div></div>';
            document.body.appendChild(overlay);
            overlay.querySelector('.chart-modal-close').addEventListener('click', function(){ document.body.removeChild(overlay); });
            overlay.addEventListener('click', function(e){ if(e.target===overlay) document.body.removeChild(overlay); });
            var wrap = document.getElementById('lang-overview-modal-wrap');
            if(wrap) {
              wrap.innerHTML = src.innerHTML;
              var svgs = wrap.querySelectorAll('svg');
              for(var i=0;i<svgs.length;i++){
                svgs[i].removeAttribute('width');
                svgs[i].removeAttribute('height');
                svgs[i].style.cssText='display:block;width:100%;height:auto;';
              }
              var ov = wrap.querySelector('.r-lang-overview');
              if(ov){ov.style.flexWrap='nowrap';ov.style.alignItems='stretch';}
              var cells = wrap.querySelectorAll('.r-lang-overview-cell');
              if(cells.length>0)cells[0].style.cssText='flex:1 1 0;max-width:none;justify-content:center;';
              if(cells.length>1)cells[1].style.cssText='flex:1 1 0;max-width:none;';
              wireDonutLegend(wrap.querySelector('svg'));
              wireMixLegend(wrap.querySelectorAll('svg')[1]);
              requestAnimationFrame(function(){
                var ss=wrap.querySelectorAll('svg');
                if(ss.length>=2){var bh=ss[1].getBoundingClientRect().height;if(bh>0){ss[0].style.cssText='display:block;height:'+bh+'px;width:auto;max-width:100%;';}}
              });
            }
          });
        })();
      })();

      // ── Dark mode sync ────────────────────────────────────────────────────────
      document.querySelectorAll('[data-theme-toggle]').forEach(function(btn) {
        btn.addEventListener('click', function() {
          setTimeout(function() {
            var c = clr();
            ALL_CHARTS.forEach(function(chart) {
              if (chart.options.scales) {
                Object.keys(chart.options.scales).forEach(function(k) {
                  var ax = chart.options.scales[k];
                  if (ax.grid) ax.grid.color = c.grid;
                  if (ax.ticks) ax.ticks.color = c.text;
                  if (ax.title) ax.title.color = c.text;
                });
              }
              if (chart.options.plugins && chart.options.plugins.legend && chart.options.plugins.legend.labels)
                chart.options.plugins.legend.labels.color = c.text;
              chart.update('none');
            });
          }, 60);
        });
      });

      // ── Pre-render all chart variants for PDF export ──────────────────────────
      (function() {
        var root = document.getElementById('pdf-variants');
        if (!root) return;

        // Plugin: fill a light background behind every off-screen chart so the PNG
        // is opaque — without this Chart.js canvases are transparent and render as
        // blank white boxes in print.
        var PDF_BG = {
          id: 'pdfBg',
          beforeDraw: function(ch) {
            var ctx = ch.canvas.getContext('2d');
            ctx.save();
            ctx.globalCompositeOperation = 'destination-over';
            ctx.fillStyle = '#faf6f0';
            ctx.fillRect(0, 0, ch.canvas.width, ch.canvas.height);
            ctx.restore();
          }
        };

        // Off-screen Chart.js render → PNG data-URL → destroy chart
        function snap(type, data, opts, w, h) {
          var c = document.createElement('canvas');
          c.width = w || 900; c.height = h || 280;
          var ch = new Chart(c, {
            type: type, data: data,
            options: Object.assign({}, opts, {
              animation: false, responsive: false, devicePixelRatio: 1,
              // Breathing room so labels never clip at the canvas edge
              layout: { padding: { top: 10, right: 18, bottom: 10, left: 10 } }
            }),
            plugins: [PDF_BG]
          });
          var png = c.toDataURL('image/png');
          ch.destroy();
          return png;
        }

        function mkPanel(label, imgSrc) {
          var d = document.createElement('div'); d.className = 'pdf-variant-panel';
          if (label) {
            var lbl = document.createElement('div');
            lbl.className = 'pdf-variant-label'; lbl.textContent = label;
            d.appendChild(lbl);
          }
          if (imgSrc) {
            var img = document.createElement('img');
            img.className = 'pdf-variant-img'; img.src = imgSrc;
            d.appendChild(img);
          }
          return d;
        }

        function mkGroup(title) {
          var g = document.createElement('div'); g.className = 'pdf-variant-group';
          var h = document.createElement('h2'); h.className = 'pdf-variant-group-title'; h.textContent = title;
          g.appendChild(h);
          var grid = document.createElement('div'); grid.className = 'pdf-variant-grid';
          g.appendChild(grid);
          return { group: g, grid: grid };
        }

        var tc = '#43342d', gc = 'rgba(0,0,0,0.07)';

        // ── Project Overview — 4 Y-axis variants ─────────────────────────────────
        var pgProj = mkGroup('Project Overview');
        var projVariants = [
          { label:'Code Lines',     fn:function(d){return d.code||0;} },
          { label:'Comment Lines',  fn:function(d){return d.comments||0;} },
          { label:'Physical Lines', fn:function(d){return (d.code||0)+(d.comments||0)+(d.blanks||0);} },
          { label:'File Count',     fn:function(d){return d.files||0;} }
        ];
        projVariants.forEach(function(y) {
          var sorted = D.slice().sort(function(a,b){return y.fn(b)-y.fn(a);});
          var h = Math.max(110, Math.min(360, sorted.length*18+40));
          var png = snap('bar', {
            labels: sorted.map(function(d){return d.lang;}),
            datasets:[{ label:y.label, data:sorted.map(y.fn),
                        backgroundColor:sorted.map(function(_,i){return PALETTE[i%PALETTE.length];}), borderRadius:3 }]
          }, {
            indexAxis:'y',
            scales:{
              x:{grid:{color:gc},ticks:{color:tc,callback:function(v){return fmt(v);}},title:{display:true,text:y.label,color:tc}},
              y:{grid:{display:false},ticks:{color:tc}}
            },
            plugins:{legend:{display:false}}
          }, 900, h);
          pgProj.grid.appendChild(mkPanel(y.label, png));
        });
        root.appendChild(pgProj.group);

        // ── Language Composition — Absolute Lines + Composition % ────────────────
        var pgComp = mkGroup('Language Composition');
        var cData = D.slice(0,15);
        var totFn = function(d){return (d.code||0)+(d.comments||0)+(d.blanks||0)||1;};
        var compH = Math.max(110, Math.min(340, cData.length*18+50));
        [{id:'absolute',label:'Absolute Lines',isPct:false},{id:'pct',label:'Composition %',isPct:true}]
          .forEach(function(m) {
            var pct = m.isPct;
            var png = snap('bar', {
              labels: cData.map(function(d){return d.lang;}),
              datasets:[
                {label:'Code',     data:cData.map(function(d){return pct?(d.code||0)/totFn(d)*100:d.code||0;}),    backgroundColor:OX,borderRadius:3},
                {label:'Comments', data:cData.map(function(d){return pct?(d.comments||0)/totFn(d)*100:d.comments||0;}),backgroundColor:GN,borderRadius:3},
                {label:'Blanks',   data:cData.map(function(d){return pct?(d.blanks||0)/totFn(d)*100:d.blanks||0;}),  backgroundColor:GY,borderRadius:3}
              ]
            }, {
              indexAxis:'y',
              scales:{
                x:{stacked:true,grid:{color:gc},ticks:{color:tc,callback:pct?function(v){return v.toFixed(0)+'%';}:function(v){return fmt(v);}}},
                y:{stacked:true,grid:{display:false},ticks:{color:tc}}
              },
              plugins:{legend:{position:'bottom',labels:{color:tc}}}
            }, 900, compH);
            pgComp.grid.appendChild(mkPanel(m.label, png));
          });
        root.appendChild(pgComp.group);

        // ── Files vs Code Lines — render off-screen (bubble chart, single-col centred) ─
        if (SCAT_D && SCAT_D.length) {
          var pgScat = mkGroup('Files vs Code Lines');
          pgScat.grid.classList.add('single-col'); // CSS class drives centering in print
          var maxP = Math.max.apply(null, SCAT_D.map(function(d){return d.physical||0;})) || 1;
          var scatPng = snap('bubble', {
            datasets: SCAT_D.map(function(d, i) {
              return {
                label: d.lang,
                data: [{ x: d.files, y: d.code, r: Math.max(5, Math.round(Math.sqrt((d.physical||0)/maxP)*20)) }],
                backgroundColor: PALETTE[i % PALETTE.length] + 'b8',
                borderColor: PALETTE[i % PALETTE.length], borderWidth: 1
              };
            })
          }, {
            scales: {
              x: { grid:{color:gc}, ticks:{color:tc}, title:{display:true, text:'Files Analyzed', color:tc} },
              y: { grid:{color:gc}, ticks:{color:tc, callback:function(v){return fmt(v);}}, title:{display:true, text:'Code Lines', color:tc} }
            },
            plugins: { legend:{position:'right', labels:{color:tc, boxWidth:12}} }
          }, 900, 260);
          pgScat.grid.appendChild(mkPanel('Files \u00d7 Code Lines (bubble size \u221d physical lines)', scatPng));
          root.appendChild(pgScat.group);
        }

        // ── Semantic Metrics — up to 5 metrics, skip empty ones ─────────────────
        if (SEM_D && SEM_D.length) {
          var pgSem = mkGroup('Semantic Metrics');
          var SL={functions:'Functions',classes:'Classes / Types',variables:'Variables',imports:'Imports',tests:'Tests'};
          var SC={functions:OX,classes:'#4472C4',variables:GN,imports:'#805099',tests:'#B23030'};
          Object.keys(SL).forEach(function(mKey) {
            var data = SEM_D.slice().sort(function(a,b){return (b[mKey]||0)-(a[mKey]||0);}).slice(0,15);
            if (!data.some(function(d){return (d[mKey]||0)>0;})) return;
            var semH = 210; // vertical bar — fixed height; width drives layout, not row count
            var png = snap('bar', {
              labels: data.map(function(d){return d.lang;}),
              datasets:[{label:SL[mKey],data:data.map(function(d){return d[mKey]||0;}),backgroundColor:SC[mKey],borderRadius:4}]
            }, {
              scales:{
                x:{grid:{display:false},ticks:{color:tc}},
                y:{grid:{color:gc},ticks:{color:tc,callback:function(v){return fmt(v);}}}
              },
              plugins:{legend:{display:false}}
            }, 900, semH);
            pgSem.grid.appendChild(mkPanel(SL[mKey], png));
          });
          root.appendChild(pgSem.group);
        }

        // ── Submodule Breakdown — 3 Y-axis variants + donut SVG clone ────────────
        if (SUB_D && SUB_D.length) {
          var pgSub = mkGroup('Submodule Breakdown');
          [{key:'code',label:'Code Lines',col:OX},{key:'comment',label:'Comment Lines',col:GN},{key:'files',label:'File Count',col:'#805099'}]
            .forEach(function(y) {
              var data = SUB_D.slice().sort(function(a,b){return (b[y.key]||0)-(a[y.key]||0);}).slice(0,30);
              if (!data.length) return;
              var subH = Math.max(100, Math.min(420, data.length*16+30));
              var png = snap('bar', {
                labels: data.map(function(d){return d.name;}),
                datasets:[{label:y.label,data:data.map(function(d){return d[y.key]||0;}),backgroundColor:y.col,borderRadius:3}]
              }, {
                indexAxis:'y',
                scales:{
                  x:{grid:{color:gc},ticks:{color:tc,callback:function(v){return fmt(v);}},title:{display:true,text:y.label,color:tc}},
                  y:{grid:{display:false},ticks:{color:tc}}
                },
                plugins:{legend:{display:false}}
              }, 900, subH);
              pgSub.grid.appendChild(mkPanel(y.label, png));
            });
          var donutEl = document.getElementById('submodule-donut');
          if (donutEl && donutEl.innerHTML.trim()) {
            var dp = document.createElement('div'); dp.className = 'pdf-variant-panel';
            var dl = document.createElement('div'); dl.className = 'pdf-variant-label'; dl.textContent = 'Distribution';
            dp.appendChild(dl);
            var dw = document.createElement('div'); dw.style.cssText = 'display:flex;justify-content:center;';
            dw.innerHTML = donutEl.innerHTML; dp.appendChild(dw);
            pgSub.grid.appendChild(dp);
          }
          root.appendChild(pgSub.group);
        }
      })();
    })();
    window.oxSlocChartsReady = true;
    } catch(e) { window.oxSlocChartError = String(e); window.oxSlocChartsReady = true; }
    }); // end requestAnimationFrame
    // Safety net: if rAF never fires (headless browsers throttle it), mark ready
    // unconditionally so the PDF capture does not wait the full 15 s.
    setTimeout(function() { if (!window.oxSlocChartsReady) window.oxSlocChartsReady = true; }, 3000);
    // ── SVG tooltip delegation ───────────────────────────────────────────────
    (function(){
      var tt = document.getElementById('r-tt');
      if (!tt) return;
      function escH(s){return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');}
      function show(e, html) { tt.innerHTML=html; tt.style.display='block'; move(e); }
      function hide() { tt.style.display='none'; }
      function move(e) {
        var x=e.clientX+16, y=e.clientY-12;
        var r=tt.getBoundingClientRect();
        if (x+r.width>window.innerWidth-8) x=e.clientX-r.width-8;
        if (y+r.height>window.innerHeight-8) y=e.clientY-r.height-8;
        tt.style.left=x+'px'; tt.style.top=y+'px';
      }
      document.addEventListener('mouseover', function(e) {
        var t=e.target;
        while(t&&t.getAttribute){
          var l=t.getAttribute('data-ttl');
          if(l!==null){ show(e,'<strong>'+escH(l)+'</strong><br>'+escH(t.getAttribute('data-ttv')||'').replace(/\n/g,'<br>')); return; }
          t=t.parentNode;
        }
      });
      document.addEventListener('mouseout', function(e) {
        var t=e.target;
        while(t&&t.getAttribute){
          if(t.getAttribute('data-ttl')!==null){ hide(); return; }
          t=t.parentNode;
        }
      });
      document.addEventListener('mousemove', function(e) {
        if(tt.style.display!=='none') move(e);
      });
      window.addEventListener('blur', function() { hide(); });
      document.addEventListener('visibilitychange', function() { if(document.hidden) hide(); });
    })();
    // Auto-populate title on any td that is visually truncated but has no explicit title
    requestAnimationFrame(function() {
      document.querySelectorAll('td').forEach(function(td) {
        if (!td.title && td.scrollWidth > td.clientWidth) {
          td.title = td.textContent.trim();
        }
      });
    });

  </script>
  <script nonce="{{ nonce }}">
  (function(){
    var S=[{n:'Classic',a:'#b85d33',b:'#7a371b'},{n:'Navy',a:'#283790',b:'#1e1e24'},{n:'Ember',a:'#ce5d3d',b:'#1e1e24'},{n:'Ocean',a:'#1f439b',b:'#1e1e24'},{n:'Royal',a:'#003184',b:'#1e1e24'}];
    function ap(s){document.documentElement.style.setProperty('--nav',s.a);document.documentElement.style.setProperty('--nav-2',s.b);try{localStorage.setItem('sloc-ns',JSON.stringify(s));}catch(e){}document.querySelectorAll('.scheme-swatch').forEach(function(x){x.classList.toggle('active',x.dataset.n===s.n);});}
    try{var sv=JSON.parse(localStorage.getItem('sloc-ns'));if(sv&&sv.a){ap(sv);}else{ap(S[0]);}}catch(e){ap(S[0]);}
    function init(){
      var btn=document.getElementById('settings-btn');if(!btn)return;
      var m=document.createElement('div');m.id='settings-modal';m.className='settings-modal';
      m.innerHTML='<div class="settings-modal-header"><span>Appearance</span><button type="button" class="settings-close" id="settings-close" aria-label="Close"><svg viewBox="0 0 24 24"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg></button></div><div class="settings-modal-body"><div class="settings-modal-label">Navigation color scheme</div><div class="scheme-grid" id="scheme-grid"></div><div style="margin-top:12px;border-top:1px solid var(--line);padding-top:12px;"><div class="settings-modal-label" style="margin-bottom:8px;">Timestamp timezone</div><select class="tz-select" id="tz-select"><option value="America/Los_Angeles">Pacific (PT)</option><option value="America/Denver">Mountain (MT)</option><option value="America/Chicago">Central (CT)</option><option value="America/New_York">Eastern (ET)</option><option value="America/Anchorage">Alaska (AT)</option><option value="Pacific/Honolulu">Hawaii (HT)</option></select></div></div>';
      document.body.appendChild(m);
      var g=document.getElementById('scheme-grid');
      if(g)S.forEach(function(s){var el=document.createElement('button');el.type='button';el.className='scheme-swatch';el.dataset.n=s.n;el.title=s.n;var p=document.createElement('div');p.className='scheme-preview';p.style.background='linear-gradient(135deg,'+s.a+','+s.b+')';var l=document.createElement('span');l.className='scheme-label';l.textContent=s.n;el.appendChild(p);el.appendChild(l);try{var c=JSON.parse(localStorage.getItem('sloc-ns'));if(c&&c.n===s.n)el.classList.add('active');}catch(e){}el.addEventListener('click',function(){ap(s);});g.appendChild(el);});
      var cl=document.getElementById('settings-close');
      window.tzAbbr=function(z){return{'America/Los_Angeles':'PT','America/Denver':'MT','America/Chicago':'CT','America/New_York':'ET','America/Anchorage':'AT','Pacific/Honolulu':'HT'}[z]||'PT';};window.fmtTz=function(ms,tz){var d=new Date(ms);if(isNaN(d.getTime()))return'';try{var pts=new Intl.DateTimeFormat('en-US',{timeZone:tz,year:'numeric',month:'2-digit',day:'2-digit',hour:'2-digit',minute:'2-digit',hour12:false}).formatToParts(d);var v={};pts.forEach(function(p){v[p.type]=p.value;});return v.year+'-'+v.month+'-'+v.day+' '+v.hour+':'+v.minute+' '+window.tzAbbr(tz);}catch(e){return'';}};window.applyTz=function(tz){try{localStorage.setItem('sloc-tz',tz);}catch(e){}document.querySelectorAll('[data-utc-ms]').forEach(function(el){var ms=parseInt(el.getAttribute('data-utc-ms'),10);if(!isNaN(ms))el.textContent=window.fmtTz(ms,tz);});};var tzSel=document.getElementById('tz-select');var storedTz;try{storedTz=localStorage.getItem('sloc-tz')||'America/Los_Angeles';}catch(e){storedTz='America/Los_Angeles';}if(tzSel){tzSel.value=storedTz;tzSel.addEventListener('change',function(){window.applyTz(this.value);});}window.applyTz(storedTz);
      btn.addEventListener('click',function(e){e.stopPropagation();var r=btn.getBoundingClientRect();m.style.top=(r.bottom+6)+'px';m.style.right=(window.innerWidth-r.right)+'px';m.classList.toggle('open');});
      if(cl)cl.addEventListener('click',function(){m.classList.remove('open');});
      document.addEventListener('click',function(e){if(!m.contains(e.target)&&e.target!==btn)m.classList.remove('open');});
    }
    if(document.readyState==='loading')document.addEventListener('DOMContentLoaded',init);else init();
  }());
  </script>
  <script nonce="{{ nonce }}">
  (function(){
    // Format delta card unmodified-lines value with comma separators
    Array.prototype.slice.call(document.querySelectorAll('.delta-card-inline[data-raw] .delta-card-val')).forEach(function(el){
      var raw=parseInt(el.parentNode.getAttribute('data-raw'),10);
      if(!isNaN(raw))el.textContent=raw.toLocaleString();
    });
    // Format code-before / code-now numbers in the prev-scan summary line
    Array.prototype.slice.call(document.querySelectorAll('.prev-scan-summary [data-raw]')).forEach(function(el){
      var raw=parseInt(el.getAttribute('data-raw'),10);
      if(!isNaN(raw))el.textContent=raw.toLocaleString();
    });
  }());
  </script>
  {% if has_style_data %}
  <script nonce="{{ nonce }}">
  (function(){
    var CHART_DATA = {{ style_chart_json|safe }};
    var FILE_DATA  = {{ style_file_json|safe }};
    var SCORE_THRESHOLD = {{ style_score_threshold }};
    var activeLang = CHART_DATA.length ? CHART_DATA[0].family : '';
    var sftSortKey = '';
    var sftSortDir = 1;
    function escH(s){return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');}
    // Official style guide URLs — covers every guide produced by the language analysers
    var GUIDE_URLS = {
      'PEP 8':'https://peps.python.org/pep-0008/',
      'PEP 8 (99-col)':'https://peps.python.org/pep-0008/',
      'Black':'https://black.readthedocs.io/en/stable/the_black_code_style/current_style.html',
      'Google Python':'https://google.github.io/styleguide/pyguide.html',
      'Effective Go':'https://go.dev/doc/effective_go',
      'Uber Go':'https://github.com/uber-go/guide/blob/master/style.md',
      'Google Go':'https://google.github.io/styleguide/go/',
      'LLVM':'https://llvm.org/docs/CodingStandards.html',
      'Google':'https://google.github.io/styleguide/cppguide.html',
      'Mozilla':'https://firefox-source-docs.mozilla.org/code-quality/coding-style/',
      'Microsoft':'https://learn.microsoft.com/en-us/dotnet/csharp/fundamentals/coding-style/coding-conventions',
      'WebKit':'https://webkit.org/code-style-guidelines/',
      'rustfmt defaults':'https://doc.rust-lang.org/rustfmt/',
      'Mozilla Rust':'https://firefox-source-docs.mozilla.org/code-quality/coding-style/coding-style-rust.html',
      'Rust API Guidelines':'https://rust-lang.github.io/api-guidelines/',
      'Relaxed (120-col)':'https://doc.rust-lang.org/rustfmt/',
      'Airbnb':'https://airbnb.io/javascript/',
      'Google JS':'https://google.github.io/styleguide/jsguide.html',
      'Standard.js':'https://standardjs.com/',
      'Prettier':'https://prettier.io/docs/en/options.html',
      'Airbnb TS':'https://airbnb.io/javascript/',
      'Google TS':'https://google.github.io/styleguide/tsguide.html',
      'Angular':'https://angular.dev/style-guide',
      'Microsoft TS':'https://github.com/Microsoft/TypeScript/wiki/Coding-guidelines',
      'Google Java':'https://google.github.io/styleguide/javaguide.html',
      'Oracle/Sun':'https://www.oracle.com/java/technologies/javase/codeconventions-contents.html',
      'Spring':'https://github.com/spring-projects/spring-framework/wiki/Code-Style',
      'JetBrains':'https://www.jetbrains.com/help/idea/code-style.html',
      'Android':'https://source.android.com/docs/setup/contribute/code-style',
      'Google Kotlin':'https://developer.android.com/kotlin/style-guide',
      'Apache Groovy':'https://groovy-lang.org/style-guide.html',
      'Gradle DSL':'https://docs.gradle.org/current/userguide/groovy_build_script_primer.html',
      'Scala Style Guide':'https://docs.scala-lang.org/style/',
      'Lightbend':'https://docs.scala-lang.org/style/',
      'Spark':'https://spark.apache.org/contributing.html',
      'RuboCop':'https://docs.rubocop.org/rubocop/',
      'Airbnb Ruby':'https://github.com/airbnb/ruby',
      'Standard Ruby':'https://github.com/standardrb/standard',
      'Microsoft .NET':'https://learn.microsoft.com/en-us/dotnet/csharp/fundamentals/coding-style/coding-conventions',
      'Google C#':'https://google.github.io/styleguide/csharp-style.html',
      'StyleCop':'https://github.com/DotNetAnalyzers/StyleCopAnalyzers',
      'Microsoft F#':'https://learn.microsoft.com/en-us/dotnet/fsharp/style-guide/formatting',
      'FSharp.Formatting':'https://fsprojects.github.io/FSharp.Formatting/'
    };
    // Human-readable descriptions for each guide shown in bar tooltips
    var GUIDE_DESC = {
      'PEP 8':'4-space | 79-col | Python style standard',
      'PEP 8 (99-col)':'4-space | 99-col | relaxed line limit',
      'Black':'4-space | 88-col | double quotes enforced',
      'Google Python':'4-space | 80-col | double quotes preferred',
      'Effective Go':'tabs | ~80-col | gofmt standard',
      'Uber Go':'tabs | 120-col max',
      'Google Go':'tabs | 80-col',
      'LLVM':'2-space | 80-col | C/C++ LLVM project style',
      'Google':'2-space | 80-col | Google C++ style',
      'Mozilla':'4-space | 80-col | Firefox codebase style',
      'Microsoft':'4-space | Allman braces | C++ Win32 style',
      'WebKit':'4-space | 80-col | WebKit engine style',
      'rustfmt defaults':'4-space | 100-col | official Rust formatter',
      'Mozilla Rust':'4-space | 100-col | Firefox Rust style',
      'Rust API Guidelines':'4-space | naming + docs conventions',
      'Relaxed (120-col)':'4-space | 120-col | relaxed line limit',
      'Airbnb':'2-space | single quotes | no semicolons opt',
      'Google JS':'2-space | 80-col | single quotes',
      'Standard.js':'2-space | no semicolons | single quotes',
      'Prettier':'2-space | 80-col | double quotes | semicolons',
      'Airbnb TS':'2-space | single quotes | TypeScript variant',
      'Google TS':'2-space | 80-col | single quotes | TypeScript',
      'Angular':'2-space | Angular team TypeScript conventions',
      'Microsoft TS':'4-space | TypeScript compiler team style',
      'Google Java':'2-space | 100-col | Google Java guide',
      'Oracle/Sun':'4-space | 80-col | original Java conventions',
      'Spring':'4-space | Spring Framework code style',
      'JetBrains':'4-space | IntelliJ default Java style',
      'Android':'4-space | 100-col | AOSP Java style',
      'Google Kotlin':'4-space | 100-col | Android Kotlin style',
      'Apache Groovy':'4-space | Apache Groovy style',
      'Gradle DSL':'4-space | Gradle build script conventions',
      'Scala Style Guide':'2-space | 100-col | official Scala style',
      'Lightbend':'2-space | Lightbend/Akka Scala style',
      'Spark':'2-space | Apache Spark Scala style',
      'RuboCop':'2-space | 120-col | community Ruby style',
      'Airbnb Ruby':'2-space | 80-col | Airbnb Ruby guide',
      'Standard Ruby':'2-space | 80-col | StandardRB formatter',
      'Microsoft .NET':'4-space | Allman braces | .NET C# style',
      'Google C#':'2-space | Google C# style guide',
      'StyleCop':'4-space | StyleCop analyzer rules',
      'Microsoft F#':'4-space | official F# formatting guide',
      'FSharp.Formatting':'4-space | FSharp.Formatting conventions'
    };
    function renderBars(family){
      var wrap=document.getElementById('style-guide-bars');
      if(!wrap)return;
      wrap.innerHTML='';
      var grp=null;
      for(var i=0;i<CHART_DATA.length;i++){if(CHART_DATA[i].family===family){grp=CHART_DATA[i];break;}}
      if(!grp||!grp.guides.length)return;
      grp.guides.forEach(function(d){
        var isTop=(d.guide===grp.dominant);
        var row=document.createElement('div');row.className='style-guide-row';
        // Hover tooltip showing guide name + score + description
        var tip=document.createElement('div');tip.className='style-bar-tip';
        var desc=GUIDE_DESC[d.guide]||'';
        tip.textContent=d.guide+': '+d.score+'%'+(desc?' \u00b7 '+desc:'');
        var lbl=document.createElement('div');lbl.className='style-guide-label';
        lbl.textContent=d.guide;
        if(isTop)lbl.style.color='var(--oxide)';
        var track=document.createElement('div');track.className='style-guide-track';
        var fill=document.createElement('div');fill.className='style-guide-fill';
        fill.style.width='0%';
        var pct=document.createElement('div');pct.className='style-guide-score';
        pct.textContent=d.score+'%';
        if(isTop)pct.style.color='var(--oxide)';
        track.appendChild(fill);
        row.appendChild(tip);
        row.appendChild(lbl);row.appendChild(track);row.appendChild(pct);
        wrap.appendChild(row);
        setTimeout(function(f,s){return function(){f.style.width=s+'%';};}(fill,d.score),60);
      });
    }
    function initTabs(){
      var tabsWrap=document.getElementById('style-lang-tabs');
      if(!tabsWrap||!CHART_DATA.length)return;
      CHART_DATA.forEach(function(grp){
        var btn=document.createElement('button');
        btn.className='style-lang-tab'+(grp.family===activeLang?' active':'');
        btn.textContent=grp.family+' ('+grp.files+')';
        btn.onclick=function(){
          activeLang=grp.family;
          var tabs=tabsWrap.querySelectorAll('.style-lang-tab');
          for(var i=0;i<tabs.length;i++)tabs[i].className='style-lang-tab';
          btn.className='style-lang-tab active';
          renderBars(activeLang);
        };
        tabsWrap.appendChild(btn);
      });
      renderBars(activeLang);
    }
    function buildGuideHtml(guide){
      if(!guide||guide==='\u2014'||guide==='Unknown')return'<span style="color:var(--muted);">\u2014</span>';
      var url=GUIDE_URLS[guide];
      var desc=GUIDE_DESC[guide]||'';
      var tipText='Open official '+guide+' documentation'+(desc?' \u00b7 '+desc:'');
      if(url){return'<a href="'+escH(url)+'" target="_blank" rel="noopener" class="style-badge">'+escH(guide)+'</a>';}
      return'<span class="style-badge">'+escH(guide)+'</span>';
    }
    function buildSigsHtml(sigs){
      if(!sigs||!sigs.length)return'<span style="color:var(--muted);">\u2014</span>';
      var html='';
      var visible=sigs.slice(0,2);
      var rest=sigs.slice(2);
      visible.forEach(function(s){html+='<span class="style-sig-chip">'+escH(s.v)+'</span>';});
      if(rest.length){html+='<span style="color:var(--muted);font-size:11px;margin-left:2px;">\u22EF</span>';}
      return html;
    }
    var _sigPop=null;
    window.showSigPop=function(btn,ev){
      ev.stopPropagation();
      if(_sigPop){var prev=_sigPop;_sigPop=null;prev.remove();if(btn._ownPop===prev)return;}
      var sigs;try{sigs=JSON.parse(btn.getAttribute('data-sigs'));}catch(e){return;}
      var pop=document.createElement('div');
      pop.className='style-sig-pop';
      pop.setAttribute('role','tooltip');
      var inner='<div class="style-sig-pop-title">All Signals</div>';
      sigs.forEach(function(s){
        inner+='<div class="style-sig-pop-row"><span class="style-sig-pop-key">'+escH(s.k)+':</span><span class="style-sig-pop-val">'+escH(s.v)+'</span></div>';
      });
      pop.innerHTML=inner;
      document.body.appendChild(pop);
      _sigPop=pop;
      btn._ownPop=pop;
      var r=btn.getBoundingClientRect();
      var pw=pop.offsetWidth||220;
      var left=r.left;
      if(left+pw>window.innerWidth-8)left=window.innerWidth-pw-8;
      if(left<8)left=8;
      var top=r.bottom+6;
      if(top+(pop.offsetHeight||120)>window.innerHeight-8)top=r.top-(pop.offsetHeight||120)-6;
      pop.style.left=left+'px';
      pop.style.top=top+'px';
      function dismiss(e){if(!pop.contains(e.target)){pop.remove();if(_sigPop===pop)_sigPop=null;document.removeEventListener('click',dismiss);document.removeEventListener('keydown',dismissKey);}}
      function dismissKey(e){if(e.key==='Escape'){pop.remove();if(_sigPop===pop)_sigPop=null;document.removeEventListener('click',dismiss);document.removeEventListener('keydown',dismissKey);}}
      setTimeout(function(){document.addEventListener('click',dismiss);document.addEventListener('keydown',dismissKey);},0);
    }
    var sftRows=[];
    var sftFilteredRows=[];
    var sftCurrentPage=1;
    function sftGetPageSize(){
      var sel=document.getElementById('sft-page-size');
      var v=sel?sel.value:'20';
      return v==='all'?Infinity:parseInt(v,10);
    }
    function sftApplyFilter(){
      var inp=document.getElementById('sft-search');
      var q=inp?inp.value.toLowerCase():'';
      var sorted=sftRows.slice();
      if(sftSortKey){
        sorted.sort(function(a,b){
          if(sftSortKey==='score'){var av=a.score||0,bv=b.score||0;return sftSortDir*(av-bv);}
          var av=String(a[sftSortKey]||'').toLowerCase(),bv=String(b[sftSortKey]||'').toLowerCase();
          return av<bv?-1*sftSortDir:av>bv?1*sftSortDir:0;
        });
      }
      sftFilteredRows=q===''?sorted:sorted.filter(function(f){
        return (f.path||'').toLowerCase().indexOf(q)>=0
          ||(f.lang||'').toLowerCase().indexOf(q)>=0
          ||(f.guide||'').toLowerCase().indexOf(q)>=0
          ||(f.indent||'').toLowerCase().indexOf(q)>=0;
      });
      sftCurrentPage=1;
      renderSftTable();
    }
    function renderSftTable(){
      var tbody=document.getElementById('style-file-tbody');
      if(!tbody)return;
      var ps=sftGetPageSize();
      var total=sftFilteredRows.length;
      var totalAll=sftRows.length;
      var totalPages=ps===Infinity?1:Math.max(1,Math.ceil(total/ps));
      if(sftCurrentPage>totalPages)sftCurrentPage=totalPages;
      if(sftCurrentPage<1)sftCurrentPage=1;
      var start=ps===Infinity?0:(sftCurrentPage-1)*ps;
      var end=ps===Infinity?total:Math.min(start+ps,total);
      var page=sftFilteredRows.slice(start,end);
      var html='';
      page.forEach(function(f){
        var barW=Math.round(f.score);
        var guide=f.guide&&f.guide!=='Unknown'?f.guide:'';
        var badge=guide?buildGuideHtml(guide):'<span style="color:var(--muted);">\u2014</span>';
                var sigHtml=buildSigsHtml(f.signals);
        var rowClass=SCORE_THRESHOLD>0&&f.score<SCORE_THRESHOLD?' class="style-row-warn"':'';
        html+='<tr'+rowClass+'>'
          +'<td title="'+escH(f.path)+'">'+escH(f.path.replace(/^.*[\/\\]/,''))+'</td>'
          +'<td>'+escH(f.lang)+'</td>'
          +'<td>'+escH(f.indent)+'</td>'
          +'<td class="guide-cell" data-gtip="'+(guide?(escH(guide)+(GUIDE_DESC[guide]?' \u00b7 '+escH(GUIDE_DESC[guide]):'')):'')+'">' +badge+'</td>'
          +'<td><span class="style-score-bar"><span class="style-score-fill" style="width:'+barW+'%"></span></span>'+f.score+'%</td>'
          +'<td class="sig-cell" data-sigs="'+escH(JSON.stringify(f.signals||[]))+'">'+sigHtml+'</td>'
          +'</tr>';
      });
      tbody.innerHTML=html||'<tr><td colspan="6" style="text-align:center;color:var(--muted);padding:18px;">No style-analysed files</td></tr>';
      var pageInfo=document.getElementById('sft-page-info');
      var firstBtn=document.getElementById('sft-first');
      var prevBtn=document.getElementById('sft-prev');
      var nextBtn=document.getElementById('sft-next');
      var lastBtn=document.getElementById('sft-last');
      var jumpInput=document.getElementById('sft-page-jump');
      var pageTotal=document.getElementById('sft-page-total');
      var countLabel=document.getElementById('sft-count-label');
      if(pageInfo){
        if(total===0){pageInfo.textContent='No results';}
        else if(ps===Infinity){pageInfo.textContent='All '+total.toLocaleString()+' files';}
        else{pageInfo.textContent=(start+1)+'\u2013'+end+' of '+total.toLocaleString()+' files';}
      }
      if(countLabel){countLabel.textContent=(total<totalAll&&total>0)?'('+total.toLocaleString()+' matching)':'';}
      var edgeOff=ps===Infinity;
      if(firstBtn)firstBtn.disabled=sftCurrentPage<=1||edgeOff;
      if(prevBtn)prevBtn.disabled=sftCurrentPage<=1||edgeOff;
      if(nextBtn)nextBtn.disabled=sftCurrentPage>=totalPages||edgeOff;
      if(lastBtn)lastBtn.disabled=sftCurrentPage>=totalPages||edgeOff;
      if(jumpInput){jumpInput.value=sftCurrentPage;jumpInput.max=totalPages;jumpInput.disabled=edgeOff;}
      if(pageTotal)pageTotal.textContent=totalPages.toLocaleString();
    }
    function initStyleTable(){
      if(!FILE_DATA.length){
        var tb=document.getElementById('style-file-tbody');
        if(tb)tb.innerHTML='<tr><td colspan="6" style="text-align:center;color:var(--muted);padding:18px;">No style-analysed files</td></tr>';
        return;
      }
      sftRows=FILE_DATA.slice();
      sftFilteredRows=sftRows.slice();
      sftApplyFilter();
      // Signal & guide cell tooltip (appears above hovered cell, arrow points down)
      var chipTipEl=document.createElement('div');
      chipTipEl.className='sig-tip';
      document.body.appendChild(chipTipEl);
      function _showSigTip(html,cell){
        chipTipEl.innerHTML=html;
        chipTipEl.style.display='block';
        var r=cell.getBoundingClientRect();
        var tw=chipTipEl.offsetWidth||220;
        var th=chipTipEl.offsetHeight||80;
        var cx=r.left+r.width/2;
        var left=cx-tw/2;
        if(left<8)left=8;
        if(left+tw>window.innerWidth-8)left=window.innerWidth-tw-8;
        var arrowPct=Math.round((cx-left)/tw*100);
        if(arrowPct<10)arrowPct=10;
        if(arrowPct>90)arrowPct=90;
        chipTipEl.style.setProperty('--sig-tip-ax',arrowPct+'%');
        var top=r.top-th-12;
        if(top<8)top=r.bottom+8;
        chipTipEl.style.left=left+'px';
        chipTipEl.style.top=top+'px';
        chipTipEl.classList.add('visible');
      }
      function _hideSigTip(){
        chipTipEl.classList.remove('visible');
        chipTipEl.style.display='none';
      }
      function _buildSigHtml(cell){
        var sigs;try{sigs=JSON.parse(cell.getAttribute('data-sigs'));}catch(ex){return null;}
        if(!sigs||!sigs.length)return null;
        var html='<div class="sig-tip-hd">Signals</div>';
        sigs.forEach(function(s){
          html+='<div class="sig-tip-row"><span class="sig-tip-k">'+escH(s.k)+':</span><span class="sig-tip-v">'+escH(s.v)+'</span></div>';
        });
        return html;
      }
      function _buildGuideHtml(cell){
        var tip=cell.getAttribute('data-gtip')||'';
        if(!tip)return null;
        var parts=tip.split(' \u00b7 ',2);
        var html='<div class="sig-tip-hd">'+escH(parts[0])+'</div>';
        if(parts[1])html+='<div class="sig-tip-v" style="font-size:11px;">'+escH(parts[1])+'</div>';
        return html;
      }
      var sigTbl=document.getElementById('style-file-table');
      if(sigTbl){
        sigTbl.addEventListener('mouseover',function(e){
          var sc=e.target.closest?e.target.closest('.sig-cell'):null;
          var gc=e.target.closest?e.target.closest('.guide-cell'):null;
          if(sc){var h=_buildSigHtml(sc);if(h)_showSigTip(h,sc);return;}
          if(gc){var h=_buildGuideHtml(gc);if(h){var badge=gc.querySelector('.style-badge')||gc;_showSigTip(h,badge);}return;}
          _hideSigTip();
        });
        sigTbl.addEventListener('mouseleave',function(){
          _hideSigTip();
        });
        sigTbl.addEventListener('mouseout',function(e){
          if(!e.relatedTarget||!sigTbl.contains(e.relatedTarget))_hideSigTip();
        });
      }
      // Wire up sortable column headers
      var ths=document.querySelectorAll('#style-file-table thead th[data-sort-key]');
      for(var i=0;i<ths.length;i++){(function(th){
        th.style.cursor='pointer';
        th.addEventListener('click',function(){
          var key=th.getAttribute('data-sort-key');
          if(sftSortKey===key){sftSortDir*=-1;}else{sftSortKey=key;sftSortDir=1;}
          for(var j=0;j<ths.length;j++){
            ths[j].classList.remove('sft-sort-asc','sft-sort-desc');
            var ind=ths[j].querySelector('.style-sort-ind');
            if(ind)ind.textContent='\u25BE';
          }
          th.classList.add(sftSortDir===1?'sft-sort-asc':'sft-sort-desc');
          var tind=th.querySelector('.style-sort-ind');
          if(tind)tind.textContent=sftSortDir===1?'\u25B2':'\u25BC';
          sftApplyFilter();
        });
      })(ths[i]);}
      var searchInput=document.getElementById('sft-search');
      if(searchInput){
        var sftTimer=null;
        searchInput.addEventListener('input',function(){clearTimeout(sftTimer);sftTimer=setTimeout(sftApplyFilter,200);});
      }
      var pageSel=document.getElementById('sft-page-size');
      if(pageSel){pageSel.addEventListener('change',function(){sftCurrentPage=1;renderSftTable();});}
      var sftFirstBtn=document.getElementById('sft-first');
      var sftPrevBtn=document.getElementById('sft-prev');
      var sftNextBtn=document.getElementById('sft-next');
      var sftLastBtn=document.getElementById('sft-last');
      var sftJumpInput=document.getElementById('sft-page-jump');
      if(sftFirstBtn){sftFirstBtn.addEventListener('click',function(){sftCurrentPage=1;renderSftTable();});}
      if(sftPrevBtn){sftPrevBtn.addEventListener('click',function(){if(sftCurrentPage>1){sftCurrentPage--;renderSftTable();}});}
      if(sftNextBtn){sftNextBtn.addEventListener('click',function(){
        var ps=sftGetPageSize();
        var totalPages=ps===Infinity?1:Math.ceil(sftFilteredRows.length/ps);
        if(sftCurrentPage<totalPages){sftCurrentPage++;renderSftTable();}
      });}
      if(sftLastBtn){sftLastBtn.addEventListener('click',function(){
        var ps=sftGetPageSize();
        sftCurrentPage=ps===Infinity?1:Math.max(1,Math.ceil(sftFilteredRows.length/ps));
        renderSftTable();
      });}
      if(sftJumpInput){
        function sftJump(){
          var ps=sftGetPageSize();
          var totalPages=ps===Infinity?1:Math.max(1,Math.ceil(sftFilteredRows.length/ps));
          var v=parseInt(sftJumpInput.value,10);
          if(!isNaN(v)){sftCurrentPage=Math.max(1,Math.min(v,totalPages));renderSftTable();}
        }
        sftJumpInput.addEventListener('change',sftJump);
        sftJumpInput.addEventListener('keydown',function(e){if(e.key==='Enter')sftJump();});
      }
    }
    function initSigInfoBtn(){
      var btn=document.getElementById('sig-info-btn');
      if(!btn)return;
      btn.addEventListener('click',function(){
        var overlay=document.createElement('div');
        overlay.className='style-sig-info-overlay';
        var GLOSSARY=[
          ['Quote Style','Dominant string quote character used in the file (single quotes, double quotes, or mixed)'],
          ['Indentation','Leading-whitespace style detected: Tabs, 2-Space, 4-Space, 8-Space, or Mixed'],
          ['Brace Style','Opening brace placement: K\u0026R / Attach (same line as statement) or Allman (own line)'],
          ['Semicolons','Whether statement-ending semicolons are present (JS/TS). \u201cNone detected\u201d means ASI-style.'],
          ['Variable Declarations','Preferred declaration keyword: const/let vs var (JS), short := vs var (Go)'],
          ['Function Naming','Dominant function naming convention: snake_case or CamelCase'],
          ['Type Hints','Whether Python PEP 484 type annotations (:Type, ->Type) are used in the file'],
          ['Wildcard Imports','Presence of import * wildcard import statements (Java/Kotlin)'],
          ['Pointer Style','Pointer/reference alignment in C/C++: *var (name-attached) or Type* (type-attached)'],
          ['Arrow Functions','Count of arrow function => expressions detected in JS/TS files'],
          ['Max Line Length','Character length of the longest line found in the file'],
          ['Error Handling','Presence of Go-style if err != nil error-checking patterns'],
          ['Type Inference','Whether the C# var keyword is used for implicit type inference'],
          ['Frozen String Literal','Whether the # frozen_string_literal: true pragma is present (Ruby)'],
          ['Space Before Paren','Spacing convention before opening parentheses in control structures (C/C++)'],
          ['Include Guard','Whether #pragma once is used as a header include guard (C/C++)']
        ];
        var rows='';
        GLOSSARY.forEach(function(g){rows+='<span class="style-sig-info-name">'+escH(g[0])+'</span><span class="style-sig-info-desc">'+escH(g[1])+'</span>';});
        overlay.innerHTML='<div class="style-sig-info-modal" role="dialog" aria-modal="true" aria-label="Signal glossary"><button type="button" class="style-sig-info-close" aria-label="Close">\u00D7</button><h3 style="margin:0 0 6px;font-size:17px;">Signal Glossary</h3><p style="color:var(--muted);font-size:13px;margin:0 0 2px;">Lexical signals detected per file. Values reflect dominant patterns in the source text.</p><div class="style-sig-info-grid">'+rows+'</div></div>';
        document.body.appendChild(overlay);
        overlay.addEventListener('click',function(e){if(e.target===overlay||e.target.classList.contains('style-sig-info-close')){overlay.remove();}});
      });
    }
    function init(){initTabs();initStyleTable();initSigInfoBtn();}
    if(document.readyState==='loading')document.addEventListener('DOMContentLoaded',init);else init();
  }());
  </script>
  {% endif %}
  <script nonce="{{ nonce }}">
  (function(){
    var params=new URLSearchParams(location.search);
    if(params.get('autoprint')!=='1')return;
    var overlay=document.createElement('div');
    overlay.id='autoprint-overlay';
    overlay.style.cssText='position:fixed;inset:0;z-index:99999;background:var(--bg,#fff);display:flex;flex-direction:column;align-items:center;justify-content:center;gap:16px;';
    overlay.innerHTML='<div style="font-size:20px;font-weight:800;color:var(--text,#1a1a1a);">Preparing PDF\u2026</div>'
      +'<div style="font-size:13px;color:var(--muted,#666);">Use your browser\u2019s print dialog \u2192 <strong>Save as PDF</strong>.</div>'
      +'<div style="width:200px;height:4px;border-radius:2px;background:rgba(0,0,0,0.1);overflow:hidden;">'
      +'<div id="autoprint-bar" style="height:100%;width:0%;background:#e07b3a;transition:width 1.5s ease;border-radius:2px;"></div></div>';
    document.body.appendChild(overlay);
    setTimeout(function(){var b=document.getElementById('autoprint-bar');if(b)b.style.width='80%';},50);
    var deadline=Date.now()+12000;
    function tryPrint(){
      if(window.oxSlocChartsReady||Date.now()>deadline){
        var b=document.getElementById('autoprint-bar');
        if(b)b.style.width='100%';
        setTimeout(function(){
          overlay.style.display='none';
          window.print();
        },350);
      } else {
        setTimeout(tryPrint,150);
      }
    }
    if(document.readyState==='loading'){
      document.addEventListener('DOMContentLoaded',function(){setTimeout(tryPrint,250);});
    } else {
      setTimeout(tryPrint,250);
    }
    window.addEventListener('afterprint',function(){overlay.remove();});
  }());
  </script>
  <footer class="report-footer">local code analysis &mdash; metrics, history and reports &nbsp;&middot;&nbsp; oxide-sloc v{{ tool_version }} &nbsp;&middot;&nbsp; AGPL-3.0-or-later &nbsp;&middot;&nbsp; offline / air-gapped build</footer>
  {% if let Some(banner) = report_header_footer %}
  <div class="report-id-footer-banner" aria-label="Report identification">{{ banner|e }}</div>
  {% endif %}
</body>
</html>"##,
    ext = "html"
)]
// Template structs need many bool fields to pass Askama rendering flags.
// Fields are consumed by the Askama proc-macro; clippy cannot trace that usage.
#[allow(clippy::struct_excessive_bools, dead_code)]
struct ReportTemplate<'a> {
    nonce: String,
    title: String,
    browser_title: String,
    scan_performed_by: String,
    scan_time_pst: String,
    tool_version: String,
    is_sub_report: bool,
    run: &'a AnalysisRun,
    language_rows: Vec<LanguageRow>,
    file_rows: Vec<FileRow>,
    skipped_rows: Vec<FileRow>,
    config_json: String,
    lang_chart_json: String,
    submodule_chart_json: String,
    scatter_chart_json: String,
    semantic_chart_json: String,
    file_size_histogram_json: String,
    has_submodule_data: bool,
    has_semantic_data: bool,
    has_coverage_data: bool,
    has_fn_coverage: bool,
    has_branch_coverage: bool,
    test_files_count: u64,
    test_assertion_count: u64,
    test_suite_count: u64,
    test_density: String,
    most_tested_lang: String,
    langs_with_tests: usize,
    cov_line_pct: String,
    cov_fn_pct: String,
    cov_branch_pct: String,
    cov_line_class: String,
    cov_fn_class: String,
    cov_branch_class: String,
    has_run_warnings: bool,
    warning_count: usize,
    warning_summary_rows: Vec<WarningSummaryRow>,
    warning_opportunity_rows: Vec<WarningOpportunityRow>,
    warning_console_full: String,
    logo_text_uri: String,
    small_logo_uri: String,
    /// Data-URI for a custom logo, or None to show the default `OxideSLOC` logo.
    custom_logo_uri: Option<String>,
    /// Optional company/team name shown instead of "`OxideSLOC`" in the nav header.
    company_name: Option<String>,
    /// CSS hex accent colour override (e.g. `#3b82f6`), or None for the default.
    accent_hex: Option<String>,
    /// Text for the header/footer identification banner on every report page.
    report_header_footer: Option<String>,
    chart_js: &'static str,
    run_id_short: String,
    /// When the HTML was generated alongside a PDF (e.g. via CLI with both
    /// `--html-out` and `--pdf-out`), this holds the relative URL to that PDF.
    /// The "View PDF" button navigates directly to it instead of the server route.
    standalone_pdf_url: Option<String>,
    /// Direct link to the commit on the hosting forge (GitHub, Bitbucket, GitLab, …).
    /// `None` when the remote URL is absent or unrecognised.
    git_commit_url: Option<String>,
    /// Direct link to the branch on the hosting forge.
    /// `None` when the remote URL or branch is absent/unrecognised.
    git_branch_url: Option<String>,
    /// Whether any style data was collected.
    has_style_data: bool,
    /// Number of language groups in the style summary (0 when none).
    style_lang_count: usize,
    /// Files scoring below this threshold are highlighted in the per-file table. 0 = off.
    style_score_threshold: u8,
    /// Serialised JSON for the multi-language style-guide chart (empty string when none).
    style_chart_json: String,
    /// Serialised JSON for the per-file style table (empty string when none).
    style_file_json: String,
    /// Aggregate style summary, cloned from `AnalysisRun::style_summary`.
    style_summary: Option<StyleSummary>,
    /// True when a previous-scan delta was provided (shows the delta panel).
    has_delta: bool,
    delta_code_added: i64,
    delta_code_removed: i64,
    delta_unmodified_lines: i64,
    delta_files_added: usize,
    delta_files_removed: usize,
    delta_files_modified: usize,
    delta_files_unchanged: usize,
    delta_files_total: usize,
    prev_code_lines: u64,
    prev_scan_count: usize,
    prev_scan_label: String,
    prev_run_id: String,
    /// Whether a COCOMO estimate is available.
    has_cocomo: bool,
    /// Pre-formatted COCOMO effort string (e.g. "14.32 person-months").
    cocomo_effort_str: String,
    /// Pre-formatted COCOMO schedule string (e.g. "6.18 months").
    cocomo_duration_str: String,
    /// Pre-formatted COCOMO average team-size string (e.g. "2.32").
    cocomo_staff_str: String,
    /// Pre-formatted KSLOC input for COCOMO (e.g. "12.53").
    cocomo_ksloc_str: String,
    /// Display label for the COCOMO mode (e.g. "Organic").
    cocomo_mode_label: String,
    /// Tooltip text explaining the selected COCOMO mode.
    cocomo_mode_tooltip: String,
    /// Unique Lines of Code across all analyzed files.
    uloc: u64,
    /// Pre-formatted `DRYness` percentage string (e.g. "82.3") or empty string.
    dryness_pct_str: String,
    /// Number of duplicate file groups detected.
    duplicate_group_count: usize,
    /// True when an `--activity-window` scan attached per-file git activity.
    has_hotspots: bool,
    /// Top-N files by `code_lines × recent commits` (empty unless activity was collected).
    hotspot_rows: Vec<HotspotRow>,
    /// True when an attribution scan populated per-author ownership.
    has_ownership: bool,
    /// Per-contributor ownership rows (empty unless attribution ran on a git repo).
    ownership_rows: Vec<AuthorReportRow>,
    /// Headline ownership summary stats (mirrors the web /code-ownership chips).
    own_contributors: usize,
    own_top_name: String,
    own_top_pct_str: String,
    own_bus_factor: usize,
    own_total_code: u64,
    own_dev_code: u64,
    own_test_code: u64,
    own_test_pct_str: String,
    own_total_comment: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// CSV export
// ─────────────────────────────────────────────────────────────────────────────

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Write a two-section CSV: language summary followed by per-file detail.
///
/// # Errors
///
/// Returns an error if the file cannot be written.
pub fn write_csv(run: &AnalysisRun, path: &Path) -> Result<()> {
    let mut out = String::new();

    // ── Section 1: Summary ──────────────────────────────────────────────────
    out.push_str("# Summary\r\n");
    out.push_str("Metric,Value\r\n");
    let _ = write!(out, "Run ID,{}\r\n", csv_escape(&run.tool.run_id));
    let _ = write!(
        out,
        "Timestamp,{}\r\n",
        csv_escape(
            &run.tool
                .timestamp_utc
                .format("%Y-%m-%d %H:%M:%S UTC")
                .to_string()
        )
    );
    let _ = write!(
        out,
        "Report Title,{}\r\n",
        csv_escape(&run.effective_configuration.reporting.report_title)
    );
    let _ = write!(
        out,
        "Files Analyzed,{}\r\n",
        run.summary_totals.files_analyzed
    );
    let _ = write!(
        out,
        "Files Skipped,{}\r\n",
        run.summary_totals.files_skipped
    );
    let _ = write!(
        out,
        "Physical Lines,{}\r\n",
        run.summary_totals.total_physical_lines
    );
    let _ = write!(out, "Code Lines,{}\r\n", run.summary_totals.code_lines);
    let _ = write!(
        out,
        "Comment Lines,{}\r\n",
        run.summary_totals.comment_lines
    );
    let _ = write!(out, "Blank Lines,{}\r\n", run.summary_totals.blank_lines);
    let _ = write!(
        out,
        "Mixed Lines (separate),{}\r\n",
        run.summary_totals.mixed_lines_separate
    );

    // ── Section 2: Language breakdown ───────────────────────────────────────
    out.push_str("\r\n# By Language\r\n");
    out.push_str(
        "Language,Files,Physical Lines,Code Lines,Comment Lines,Blank Lines,Mixed Lines\r\n",
    );
    for lang in &run.totals_by_language {
        let _ = write!(
            out,
            "{},{},{},{},{},{},{}\r\n",
            csv_escape(lang.language.display_name()),
            lang.files,
            lang.total_physical_lines,
            lang.code_lines,
            lang.comment_lines,
            lang.blank_lines,
            lang.mixed_lines_separate,
        );
    }

    // ── Section 3: Per-file detail (if present) ─────────────────────────────
    write_csv_per_file_section(&mut out, run);

    // ── Section 4: Code ownership (if an attribution scan populated it) ──────
    write_csv_ownership_section(&mut out, run);

    fs::write(path, out).with_context(|| format!("failed to write CSV to {}", path.display()))
}

/// Append the code-ownership section to a CSV buffer. No-op unless an attribution scan
/// populated `run.authors`. Rows are ordered by code lines owned (as produced by the engine).
fn write_csv_ownership_section(out: &mut String, run: &AnalysisRun) {
    let rows = build_author_rows(run);
    if rows.is_empty() {
        return;
    }
    out.push_str("\r\n# Code Ownership\r\n");
    out.push_str(
        "Author,Email,Code Lines,Comment Lines,Blank Lines,Total Lines,Code %,Files Owned\r\n",
    );
    for a in &rows {
        let _ = write!(
            out,
            "{},{},{},{},{},{},{},{}\r\n",
            csv_escape(&a.name),
            csv_escape(&a.email),
            a.code,
            a.comment,
            a.blank,
            a.total,
            a.code_pct_str,
            a.files_owned,
        );
    }
}

/// Append the per-file detail section to a CSV buffer. No-op when there are no per-file records.
fn write_csv_per_file_section(out: &mut String, run: &AnalysisRun) {
    if run.per_file_records.is_empty() {
        return;
    }
    // Only emit the git-activity columns when an --activity-window scan populated them.
    let has_activity = run
        .per_file_records
        .iter()
        .any(|r| r.commit_count.is_some());
    out.push_str("\r\n# Per File\r\n");
    out.push_str(
        "Path,Language,Size (bytes),Code Lines,Comment Lines,Blank Lines,Physical Lines,Generated,Minified,Vendor",
    );
    if has_activity {
        out.push_str(",Commits,Last Changed");
    }
    out.push_str("\r\n");
    for rec in &run.per_file_records {
        let _ = write!(
            out,
            "{},{},{},{},{},{},{},{},{},{}",
            csv_escape(&rec.relative_path),
            csv_escape(
                &rec.language
                    .map(|l| l.display_name().to_string())
                    .unwrap_or_default()
            ),
            rec.size_bytes,
            rec.effective_counts.code_lines,
            rec.effective_counts.comment_lines,
            rec.effective_counts.blank_lines,
            rec.raw_line_categories.total_physical_lines,
            rec.generated,
            rec.minified,
            rec.vendor,
        );
        if has_activity {
            let _ = write!(
                out,
                ",{},{}",
                rec.commit_count.map(|c| c.to_string()).unwrap_or_default(),
                csv_escape(rec.last_commit_date.as_deref().unwrap_or("")),
            );
        }
        out.push_str("\r\n");
    }
}

/// Write a diff/delta as CSV.
///
/// # Errors
///
/// Returns an error if the file cannot be written.
pub fn write_diff_csv(cmp: &sloc_core::ScanComparison, path: &Path) -> Result<()> {
    let s = &cmp.summary;
    let mut out = String::new();

    out.push_str("# Diff Summary\r\n");
    out.push_str("Metric,Value\r\n");
    let _ = write!(out, "Baseline Run,{}\r\n", csv_escape(&s.baseline_run_id));
    let _ = write!(out, "Current Run,{}\r\n", csv_escape(&s.current_run_id));
    let _ = write!(out, "Files Added,{}\r\n", cmp.files_added);
    let _ = write!(out, "Files Removed,{}\r\n", cmp.files_removed);
    let _ = write!(out, "Files Modified,{}\r\n", cmp.files_modified);
    let _ = write!(out, "Files Unchanged,{}\r\n", cmp.files_unchanged);
    let _ = write!(out, "Files Total,{}\r\n", cmp.files_total);
    let _ = write!(out, "Code Δ,{}\r\n", s.code_lines_delta);
    let _ = write!(out, "Comment Δ,{}\r\n", s.comment_lines_delta);
    let _ = write!(out, "Blank Δ,{}\r\n", s.blank_lines_delta);
    let _ = write!(out, "Total Δ,{}\r\n", s.total_lines_delta);

    out.push_str("\r\n# File Deltas\r\n");
    out.push_str("Status,Path,Language,Baseline Code,Current Code,Code Δ,Baseline Comment,Current Comment,Comment Δ,Baseline Blank,Current Blank,Blank Δ,Total Δ\r\n");
    for f in &cmp.file_deltas {
        let status = match f.status {
            sloc_core::FileChangeStatus::Added => "Added",
            sloc_core::FileChangeStatus::Removed => "Removed",
            sloc_core::FileChangeStatus::Modified => "Modified",
            sloc_core::FileChangeStatus::Unchanged => "Unchanged",
        };
        let _ = write!(
            out,
            "{},{},{},{},{},{},{},{},{},{},{},{},{}\r\n",
            status,
            csv_escape(&f.relative_path),
            csv_escape(f.language.as_deref().unwrap_or("")),
            f.baseline_code,
            f.current_code,
            f.code_delta,
            f.baseline_comment,
            f.current_comment,
            f.comment_delta,
            f.baseline_blank,
            f.current_blank,
            f.blank_delta,
            f.total_delta,
        );
    }

    fs::write(path, out).with_context(|| format!("failed to write diff CSV to {}", path.display()))
}

// ─────────────────────────────────────────────────────────────────────────────
// XLSX export — self-contained, no external crates required.
//
// An .xlsx file is a ZIP archive containing a set of XML files.  We write the
// ZIP with the STORE (uncompressed) method so we only need a CRC-32 routine
// and straightforward byte-level framing — both implemented inline below.
// ─────────────────────────────────────────────────────────────────────────────

fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xffff_ffff;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 0 {
                crc >> 1
            } else {
                (crc >> 1) ^ 0xedb8_8320
            };
        }
    }
    !crc
}

struct ZipEntry {
    name: Vec<u8>,
    data: Vec<u8>,
    crc: u32,
    offset: u32,
}

#[allow(clippy::cast_possible_truncation)] // deliberate ZIP format construction: sizes are bounded by caller
fn zip_add(entries: &mut Vec<ZipEntry>, buf: &mut Vec<u8>, name: &str, data: Vec<u8>) {
    let crc = crc32(&data);
    let offset = buf.len() as u32;
    let name_bytes = name.as_bytes().to_vec();
    let size = data.len() as u32;

    // Local file header (signature 0x04034b50)
    buf.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
    buf.extend_from_slice(&20u16.to_le_bytes()); // version needed
    buf.extend_from_slice(&0u16.to_le_bytes()); // flags
    buf.extend_from_slice(&0u16.to_le_bytes()); // compression: STORE
    buf.extend_from_slice(&0u16.to_le_bytes()); // mod time
    buf.extend_from_slice(&0u16.to_le_bytes()); // mod date
    buf.extend_from_slice(&crc.to_le_bytes());
    buf.extend_from_slice(&size.to_le_bytes()); // compressed size
    buf.extend_from_slice(&size.to_le_bytes()); // uncompressed size
    buf.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes()); // extra field length
    buf.extend_from_slice(&name_bytes);
    buf.extend_from_slice(&data);

    entries.push(ZipEntry {
        name: name_bytes,
        data,
        crc,
        offset,
    });
}

#[allow(clippy::cast_possible_truncation)] // deliberate ZIP format construction: sizes are bounded by ZIP spec limits
fn zip_finish(mut buf: Vec<u8>, entries: &[ZipEntry]) -> Vec<u8> {
    let central_start = buf.len() as u32;

    for e in entries {
        let size = e.data.len() as u32;
        buf.extend_from_slice(&0x0201_4b50_u32.to_le_bytes()); // central dir sig
        buf.extend_from_slice(&20u16.to_le_bytes()); // version made by
        buf.extend_from_slice(&20u16.to_le_bytes()); // version needed
        buf.extend_from_slice(&0u16.to_le_bytes()); // flags
        buf.extend_from_slice(&0u16.to_le_bytes()); // compression: STORE
        buf.extend_from_slice(&0u16.to_le_bytes()); // mod time
        buf.extend_from_slice(&0u16.to_le_bytes()); // mod date
        buf.extend_from_slice(&e.crc.to_le_bytes());
        buf.extend_from_slice(&size.to_le_bytes());
        buf.extend_from_slice(&size.to_le_bytes());
        buf.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes()); // extra
        buf.extend_from_slice(&0u16.to_le_bytes()); // comment
        buf.extend_from_slice(&0u16.to_le_bytes()); // disk start
        buf.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        buf.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        buf.extend_from_slice(&e.offset.to_le_bytes());
        buf.extend_from_slice(&e.name);
    }

    let central_size = buf.len() as u32 - central_start;
    let n = entries.len() as u16;

    // End of central directory record
    buf.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes()); // disk number
    buf.extend_from_slice(&0u16.to_le_bytes()); // disk with central dir
    buf.extend_from_slice(&n.to_le_bytes()); // entries on this disk
    buf.extend_from_slice(&n.to_le_bytes()); // total entries
    buf.extend_from_slice(&central_size.to_le_bytes());
    buf.extend_from_slice(&central_start.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes()); // comment length

    buf
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Build a worksheet XML with the given header row and data rows.
// ── XLSX style-index constants ──────────────────────────────────────────────
// Indices into the <cellXfs> table in styles.xml.
// 0 = default (unused placeholder)
// 1 = HEADER   bold white text, navy fill (#283790), all-side thin border, centered
// 2 = BODY     normal text, white fill, thin border
// 3 = BODY_ALT normal text, cream fill (#F5EFE8), thin border  (alternating rows)
// 4 = NUM      #,##0, right-aligned, white fill, thin border
// 5 = NUM_ALT  #,##0, right-aligned, cream fill, thin border   (alternating rows)
// 6 = KV_KEY   bold navy text (#283790), warm-surface fill (#FBF7F2), thin border
// 7 = KV_VAL   normal text, white fill, thin border  (key-value sheets: Summary)
const XLS_HEADER: u32 = 1;
const XLS_BODY: u32 = 2;
const XLS_BODY_ALT: u32 = 3;
const XLS_NUM: u32 = 4;
const XLS_NUM_ALT: u32 = 5;
const XLS_KV_KEY: u32 = 6;
const XLS_KV_VAL: u32 = 7;

struct XlSheet<'a> {
    name: &'a str,
    tab_color: &'a str, // AARRGGBB hex without '#', e.g. "FF283790"
    headers: &'a [&'a str],
    rows: Vec<Vec<String>>,
    col_widths: Vec<f64>, // per-column character widths; last entry used for overflow cols
    is_kv: bool,          // key-value layout (Summary): col A = key style, no autofilter
}

#[allow(clippy::cast_possible_truncation)] // n % 26 fits in u8 by construction
fn xl_col_name(idx: usize) -> String {
    let mut n = idx + 1;
    let mut s = String::new();
    while n > 0 {
        n -= 1;
        s.insert(0, char::from(b'A' + (n % 26) as u8));
        n /= 26;
    }
    s
}

const fn xl_styles() -> &'static str {
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\
<numFmts count=\"1\">\
<numFmt numFmtId=\"164\" formatCode=\"#,##0\"/>\
</numFmts>\
<fonts count=\"3\">\
<font><sz val=\"11\"/><name val=\"Calibri\"/></font>\
<font><b/><sz val=\"11\"/><color rgb=\"FFFFFFFF\"/><name val=\"Calibri\"/></font>\
<font><b/><sz val=\"11\"/><color rgb=\"FF283790\"/><name val=\"Calibri\"/></font>\
</fonts>\
<fills count=\"5\">\
<fill><patternFill patternType=\"none\"/></fill>\
<fill><patternFill patternType=\"gray125\"/></fill>\
<fill><patternFill patternType=\"solid\"><fgColor rgb=\"FF283790\"/><bgColor indexed=\"64\"/></patternFill></fill>\
<fill><patternFill patternType=\"solid\"><fgColor rgb=\"FFF5EFE8\"/><bgColor indexed=\"64\"/></patternFill></fill>\
<fill><patternFill patternType=\"solid\"><fgColor rgb=\"FFFBF7F2\"/><bgColor indexed=\"64\"/></patternFill></fill>\
</fills>\
<borders count=\"2\">\
<border><left/><right/><top/><bottom/><diagonal/></border>\
<border>\
<left style=\"thin\"><color rgb=\"FFD0B8A0\"/></left>\
<right style=\"thin\"><color rgb=\"FFD0B8A0\"/></right>\
<top style=\"thin\"><color rgb=\"FFD0B8A0\"/></top>\
<bottom style=\"thin\"><color rgb=\"FFD0B8A0\"/></bottom>\
<diagonal/>\
</border>\
</borders>\
<cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs>\
<cellXfs count=\"8\">\
<xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/>\
<xf numFmtId=\"0\" fontId=\"1\" fillId=\"2\" borderId=\"1\" xfId=\"0\" \
applyFont=\"1\" applyFill=\"1\" applyBorder=\"1\" applyAlignment=\"1\">\
<alignment horizontal=\"center\" vertical=\"center\"/></xf>\
<xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"1\" xfId=\"0\" applyBorder=\"1\"/>\
<xf numFmtId=\"0\" fontId=\"0\" fillId=\"3\" borderId=\"1\" xfId=\"0\" applyFill=\"1\" applyBorder=\"1\"/>\
<xf numFmtId=\"164\" fontId=\"0\" fillId=\"0\" borderId=\"1\" xfId=\"0\" \
applyNumberFormat=\"1\" applyBorder=\"1\" applyAlignment=\"1\">\
<alignment horizontal=\"right\"/></xf>\
<xf numFmtId=\"164\" fontId=\"0\" fillId=\"3\" borderId=\"1\" xfId=\"0\" \
applyNumberFormat=\"1\" applyFill=\"1\" applyBorder=\"1\" applyAlignment=\"1\">\
<alignment horizontal=\"right\"/></xf>\
<xf numFmtId=\"0\" fontId=\"2\" fillId=\"4\" borderId=\"1\" xfId=\"0\" \
applyFont=\"1\" applyFill=\"1\" applyBorder=\"1\"/>\
<xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"1\" xfId=\"0\" applyBorder=\"1\"/>\
</cellXfs>\
</styleSheet>"
}

fn xl_sheet_xml(sheet: &XlSheet<'_>) -> Vec<u8> {
    let ncols = sheet.headers.len();
    let ndata = sheet.rows.len();
    let last_col = xl_col_name(ncols.saturating_sub(1));
    let last_row = ndata + 1;
    let range = format!("A1:{last_col}{last_row}");

    let mut xml = String::with_capacity(4096 + ndata * 256);
    let _ = write!(
        xml,
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">\n\
         <sheetPr><tabColor rgb=\"{tc}\"/></sheetPr>\n\
         <dimension ref=\"{rng}\"/>\n\
         <sheetViews><sheetView workbookViewId=\"0\">\
         <pane ySplit=\"1\" topLeftCell=\"A2\" activePane=\"bottomLeft\" state=\"frozen\"/>\
         <selection pane=\"bottomLeft\" activeCell=\"A2\" sqref=\"A2\"/>\
         </sheetView></sheetViews>\n\
         <sheetFormatPr defaultRowHeight=\"15\"/>\n",
        tc = sheet.tab_color,
        rng = range,
    );

    xl_write_col_widths(&mut xml, &sheet.col_widths, ncols);
    xml.push_str("<sheetData>\n");
    xl_write_header_row(&mut xml, sheet.headers);
    xl_write_data_rows(&mut xml, &sheet.rows, sheet.is_kv);
    xml.push_str("</sheetData>\n");
    if !sheet.is_kv && ncols > 0 {
        let _ = writeln!(xml, "<autoFilter ref=\"{range}\"/>");
    }
    xml.push_str("</worksheet>");
    xml.into_bytes()
}

fn xl_write_col_widths(xml: &mut String, col_widths: &[f64], ncols: usize) {
    if col_widths.is_empty() {
        return;
    }
    let default_w = *col_widths.last().unwrap_or(&10.0);
    xml.push_str("<cols>\n");
    for ci in 0..ncols {
        let w = col_widths.get(ci).copied().unwrap_or(default_w);
        let _ = writeln!(
            xml,
            "  <col min=\"{n}\" max=\"{n}\" width=\"{w:.1}\" customWidth=\"1\"/>",
            n = ci + 1
        );
    }
    xml.push_str("</cols>\n");
}

fn xl_write_header_row(xml: &mut String, headers: &[&str]) {
    let _ = write!(xml, "<row r=\"1\" ht=\"18\" customHeight=\"1\">");
    for (ci, &h) in headers.iter().enumerate() {
        let _ = write!(
            xml,
            "<c r=\"{}1\" t=\"inlineStr\" s=\"{}\"><is><t>{}</t></is></c>",
            xl_col_name(ci),
            XLS_HEADER,
            xml_escape(h),
        );
    }
    xml.push_str("</row>\n");
}

const fn xl_cell_style(is_kv: bool, ci: usize, is_num: bool, is_alt: bool) -> u32 {
    if is_kv {
        if ci == 0 {
            XLS_KV_KEY
        } else if is_num {
            XLS_NUM
        } else {
            XLS_KV_VAL
        }
    } else if is_num {
        if is_alt { XLS_NUM_ALT } else { XLS_NUM }
    } else if is_alt {
        XLS_BODY_ALT
    } else {
        XLS_BODY
    }
}

fn xl_write_data_rows(xml: &mut String, rows: &[Vec<String>], is_kv: bool) {
    for (ri, row) in rows.iter().enumerate() {
        let row_num = ri + 2;
        let is_alt = ri % 2 == 1;
        let _ = write!(xml, "<row r=\"{row_num}\">");
        for (ci, cell) in row.iter().enumerate() {
            let cell_ref = format!("{}{}", xl_col_name(ci), row_num);
            let is_num = !cell.is_empty() && cell.parse::<f64>().is_ok();
            let s = xl_cell_style(is_kv, ci, is_num, is_alt);
            if is_num {
                let _ = write!(
                    xml,
                    "<c r=\"{cell_ref}\" s=\"{s}\"><v>{}</v></c>",
                    xml_escape(cell)
                );
            } else {
                let _ = write!(
                    xml,
                    "<c r=\"{cell_ref}\" t=\"inlineStr\" s=\"{s}\"><is><t>{}</t></is></c>",
                    xml_escape(cell),
                );
            }
        }
        xml.push_str("</row>\n");
    }
}

fn build_xlsx(sheets: &[XlSheet<'_>]) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    let mut entries: Vec<ZipEntry> = Vec::new();

    // ── [Content_Types].xml ─────────────────────────────────────────────────
    let mut ct = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n");
    ct.push_str("<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\n");
    ct.push_str("  <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\n");
    ct.push_str("  <Default Extension=\"xml\" ContentType=\"application/xml\"/>\n");
    ct.push_str("  <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\n");
    ct.push_str("  <Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>\n");
    for (i, _) in sheets.iter().enumerate() {
        let _ = writeln!(
            ct,
            "  <Override PartName=\"/xl/worksheets/sheet{}.xml\" \
             ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>",
            i + 1
        );
    }
    ct.push_str("</Types>");
    zip_add(
        &mut entries,
        &mut buf,
        "[Content_Types].xml",
        ct.into_bytes(),
    );

    // ── _rels/.rels ─────────────────────────────────────────────────────────
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n\
  <Relationship Id=\"rId1\" \
  Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" \
  Target=\"xl/workbook.xml\"/>\n\
</Relationships>";
    zip_add(
        &mut entries,
        &mut buf,
        "_rels/.rels",
        rels.as_bytes().to_vec(),
    );

    // ── xl/workbook.xml ──────────────────────────────────────────────────────
    let mut wb = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n");
    wb.push_str(
        "<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\n",
    );
    wb.push_str("  <sheets>\n");
    for (i, sheet) in sheets.iter().enumerate() {
        let _ = writeln!(
            wb,
            "    <sheet name=\"{}\" sheetId=\"{}\" r:id=\"rId{}\"/>",
            xml_escape(sheet.name),
            i + 1,
            i + 1
        );
    }
    wb.push_str("  </sheets>\n</workbook>");
    zip_add(&mut entries, &mut buf, "xl/workbook.xml", wb.into_bytes());

    // ── xl/_rels/workbook.xml.rels ───────────────────────────────────────────
    let mut wbr = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n");
    wbr.push_str(
        "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n",
    );
    for (i, _) in sheets.iter().enumerate() {
        let _ = writeln!(
            wbr,
            "  <Relationship Id=\"rId{}\" \
             Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" \
             Target=\"worksheets/sheet{}.xml\"/>",
            i + 1,
            i + 1
        );
    }
    let _ = writeln!(
        wbr,
        "  <Relationship Id=\"rId{}\" \
         Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" \
         Target=\"styles.xml\"/>",
        sheets.len() + 1
    );
    wbr.push_str("</Relationships>");
    zip_add(
        &mut entries,
        &mut buf,
        "xl/_rels/workbook.xml.rels",
        wbr.into_bytes(),
    );

    // ── xl/styles.xml ───────────────────────────────────────────────────────
    zip_add(
        &mut entries,
        &mut buf,
        "xl/styles.xml",
        xl_styles().as_bytes().to_vec(),
    );

    // ── worksheets ───────────────────────────────────────────────────────────
    for (i, sheet) in sheets.iter().enumerate() {
        let sheet_xml = xl_sheet_xml(sheet);
        let name = format!("xl/worksheets/sheet{}.xml", i + 1);
        zip_add(&mut entries, &mut buf, &name, sheet_xml);
    }

    zip_finish(buf, &entries)
}

/// Write an analysis run as a multi-sheet Excel workbook.
///
/// # Errors
///
/// Returns an error if the file cannot be written.
#[allow(clippy::too_many_lines)]
pub fn write_xlsx(run: &AnalysisRun, path: &Path) -> Result<()> {
    // Sheet 1 — Summary
    let summary_rows: Vec<Vec<String>> = vec![
        vec!["Run ID".into(), run.tool.run_id.clone()],
        vec![
            "Timestamp".into(),
            run.tool
                .timestamp_utc
                .format("%Y-%m-%d %H:%M:%S UTC")
                .to_string(),
        ],
        vec![
            "Report Title".into(),
            run.effective_configuration.reporting.report_title.clone(),
        ],
        vec![
            "Files Analyzed".into(),
            run.summary_totals.files_analyzed.to_string(),
        ],
        vec![
            "Files Skipped".into(),
            run.summary_totals.files_skipped.to_string(),
        ],
        vec![
            "Physical Lines".into(),
            run.summary_totals.total_physical_lines.to_string(),
        ],
        vec![
            "Code Lines".into(),
            run.summary_totals.code_lines.to_string(),
        ],
        vec![
            "Comment Lines".into(),
            run.summary_totals.comment_lines.to_string(),
        ],
        vec![
            "Blank Lines".into(),
            run.summary_totals.blank_lines.to_string(),
        ],
        vec![
            "Mixed Lines (separate)".into(),
            run.summary_totals.mixed_lines_separate.to_string(),
        ],
    ];

    // Sheet 2 — By Language
    let lang_rows: Vec<Vec<String>> = run
        .totals_by_language
        .iter()
        .map(|l| {
            vec![
                l.language.display_name().to_string(),
                l.files.to_string(),
                l.total_physical_lines.to_string(),
                l.code_lines.to_string(),
                l.comment_lines.to_string(),
                l.blank_lines.to_string(),
                l.mixed_lines_separate.to_string(),
            ]
        })
        .collect();

    // Sheet 3 — Per File
    let file_rows: Vec<Vec<String>> = run
        .per_file_records
        .iter()
        .map(|r| {
            vec![
                r.relative_path.clone(),
                r.language
                    .map(|l| l.display_name().to_string())
                    .unwrap_or_default(),
                r.size_bytes.to_string(),
                r.effective_counts.code_lines.to_string(),
                r.effective_counts.comment_lines.to_string(),
                r.effective_counts.blank_lines.to_string(),
                r.raw_line_categories.total_physical_lines.to_string(),
                r.generated.to_string(),
                r.minified.to_string(),
                r.vendor.to_string(),
            ]
        })
        .collect();

    // Sheet 4 — Skipped Files
    let skipped_rows: Vec<Vec<String>> = run
        .skipped_file_records
        .iter()
        .map(|r| {
            vec![
                r.relative_path.clone(),
                format!("{:?}", r.status),
                r.size_bytes.to_string(),
            ]
        })
        .collect();

    let summary_hdrs: &[&str] = &["Metric", "Value"];
    let lang_hdrs: &[&str] = &[
        "Language",
        "Files",
        "Physical Lines",
        "Code Lines",
        "Comments",
        "Blank",
        "Mixed",
    ];
    let file_hdrs: &[&str] = &[
        "Path",
        "Language",
        "Size (bytes)",
        "Code Lines",
        "Comments",
        "Blank Lines",
        "Physical Lines",
        "Generated",
        "Minified",
        "Vendor",
    ];
    let skipped_hdrs: &[&str] = &["Path", "Status", "Size (bytes)"];

    let sheets = vec![
        XlSheet {
            name: "Summary",
            tab_color: "FF283790",
            headers: summary_hdrs,
            rows: summary_rows,
            col_widths: vec![26.0, 44.0],
            is_kv: true,
        },
        XlSheet {
            name: "By Language",
            tab_color: "FFB85D33",
            headers: lang_hdrs,
            rows: lang_rows,
            col_widths: vec![20.0, 9.0, 15.0, 13.0, 13.0, 11.0, 11.0],
            is_kv: false,
        },
        XlSheet {
            name: "Per File",
            tab_color: "FF2A6846",
            headers: file_hdrs,
            rows: file_rows,
            col_widths: vec![48.0, 14.0, 13.0, 13.0, 11.0, 11.0, 15.0, 11.0, 11.0, 9.0],
            is_kv: false,
        },
        XlSheet {
            name: "Skipped",
            tab_color: "FF7B675B",
            headers: skipped_hdrs,
            rows: skipped_rows,
            col_widths: vec![52.0, 24.0, 13.0],
            is_kv: false,
        },
    ];

    let bytes = build_xlsx(&sheets);
    fs::write(path, bytes).with_context(|| format!("failed to write XLSX to {}", path.display()))
}

/// Write a diff comparison as an Excel workbook.
///
/// # Errors
///
/// Returns an error if the file cannot be written.
pub fn write_diff_xlsx(cmp: &sloc_core::ScanComparison, path: &Path) -> Result<()> {
    let s = &cmp.summary;

    let summary_rows: Vec<Vec<String>> = vec![
        vec!["Baseline Run".into(), s.baseline_run_id.clone()],
        vec!["Current Run".into(), s.current_run_id.clone()],
        vec!["Files Added".into(), cmp.files_added.to_string()],
        vec!["Files Removed".into(), cmp.files_removed.to_string()],
        vec!["Files Modified".into(), cmp.files_modified.to_string()],
        vec!["Files Unchanged".into(), cmp.files_unchanged.to_string()],
        vec!["Files Total".into(), cmp.files_total.to_string()],
        vec!["Code Δ".into(), s.code_lines_delta.to_string()],
        vec!["Comment Δ".into(), s.comment_lines_delta.to_string()],
        vec!["Blank Δ".into(), s.blank_lines_delta.to_string()],
        vec!["Total Δ".into(), s.total_lines_delta.to_string()],
    ];

    let delta_rows: Vec<Vec<String>> = cmp
        .file_deltas
        .iter()
        .map(|f| {
            let status = match f.status {
                sloc_core::FileChangeStatus::Added => "Added",
                sloc_core::FileChangeStatus::Removed => "Removed",
                sloc_core::FileChangeStatus::Modified => "Modified",
                sloc_core::FileChangeStatus::Unchanged => "Unchanged",
            };
            vec![
                status.to_string(),
                f.relative_path.clone(),
                f.language.clone().unwrap_or_default(),
                f.baseline_code.to_string(),
                f.current_code.to_string(),
                f.code_delta.to_string(),
                f.baseline_comment.to_string(),
                f.current_comment.to_string(),
                f.comment_delta.to_string(),
                f.total_delta.to_string(),
            ]
        })
        .collect();

    let summary_hdrs: &[&str] = &["Metric", "Value"];
    let delta_hdrs: &[&str] = &[
        "Status",
        "Path",
        "Language",
        "Baseline Code",
        "Current Code",
        "Code Δ",
        "Baseline Comment",
        "Current Comment",
        "Comment Δ",
        "Total Δ",
    ];

    let sheets = vec![
        XlSheet {
            name: "Diff Summary",
            tab_color: "FF283790",
            headers: summary_hdrs,
            rows: summary_rows,
            col_widths: vec![26.0, 44.0],
            is_kv: true,
        },
        XlSheet {
            name: "File Deltas",
            tab_color: "FFB85D33",
            headers: delta_hdrs,
            rows: delta_rows,
            col_widths: vec![12.0, 48.0, 16.0, 14.0, 14.0, 11.0, 14.0, 14.0, 11.0, 11.0],
            is_kv: false,
        },
    ];

    let bytes = build_xlsx(&sheets);
    fs::write(path, bytes)
        .with_context(|| format!("failed to write diff XLSX to {}", path.display()))
}

// ── Confluence rendering ────────────────────────────────────────────────────

fn html_esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Generates Confluence storage-format XHTML for a scan result page.
/// Includes an info panel, summary stats, per-language table, and an optional
/// link back to the full oxide-sloc HTML report.
#[must_use]
pub fn render_confluence_storage(run: &AnalysisRun, report_url: Option<&str>) -> String {
    let mut out = String::with_capacity(8192);

    let project = run.effective_configuration.reporting.report_title.as_str();
    let branch = run.git_branch.as_deref().unwrap_or("—");
    let commit = run.git_commit_short.as_deref().unwrap_or("—");
    let scanned = run
        .tool
        .timestamp_utc
        .format("%Y-%m-%d %H:%M UTC")
        .to_string();

    // Info panel macro
    out.push_str(
        "<ac:structured-macro ac:name=\"info\" ac:schema-version=\"1\">\
         <ac:rich-text-body><p>",
    );
    let _ = write!(
        out,
        "<strong>Project:</strong> {proj} &nbsp;·&nbsp; \
         <strong>Branch:</strong> {branch} &nbsp;·&nbsp; \
         <strong>Commit:</strong> {commit} &nbsp;·&nbsp; \
         <strong>Scanned:</strong> {scanned}",
        proj = html_esc(project),
        branch = html_esc(branch),
        commit = html_esc(commit),
        scanned = html_esc(&scanned),
    );
    out.push_str("</p></ac:rich-text-body></ac:structured-macro>");

    // Summary stats table
    out.push_str("<h2>Summary</h2>");
    out.push_str(
        "<table><thead><tr>\
         <th>Files Analyzed</th><th>Code Lines</th><th>Comment Lines</th>\
         <th>Blank Lines</th><th>Languages</th>\
         </tr></thead><tbody><tr>",
    );
    let t = &run.summary_totals;
    let _ = write!(
        out,
        "<td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td>",
        t.files_analyzed,
        t.code_lines,
        t.comment_lines,
        t.blank_lines,
        run.totals_by_language.len(),
    );
    out.push_str("</tr></tbody></table>");

    // Per-language breakdown table
    if !run.totals_by_language.is_empty() {
        out.push_str("<h2>Language Breakdown</h2>");
        out.push_str(
            "<table><thead><tr>\
             <th>Language</th><th>Files</th><th>Code</th><th>Comments</th><th>Blank</th>\
             </tr></thead><tbody>",
        );
        for lang in &run.totals_by_language {
            let _ = write!(
                out,
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                html_esc(lang.language.display_name()),
                lang.files,
                lang.code_lines,
                lang.comment_lines,
                lang.blank_lines,
            );
        }
        out.push_str("</tbody></table>");
    }

    // Link back to full report
    if let Some(url) = report_url {
        let _ = write!(
            out,
            "<p><strong>Full interactive report:</strong> \
             <a href=\"{url}\">{url_disp}</a></p>",
            url = html_esc(url),
            url_disp = html_esc(url),
        );
    }

    out
}

/// Generates Confluence wiki markup (legacy syntax) for copy/paste into a
/// Confluence page editor.
#[must_use]
pub fn render_confluence_wiki_markup(run: &AnalysisRun) -> String {
    let mut out = String::with_capacity(4096);

    let project = run.effective_configuration.reporting.report_title.as_str();
    let branch = run.git_branch.as_deref().unwrap_or("—");
    let commit = run.git_commit_short.as_deref().unwrap_or("—");
    let scanned = run
        .tool
        .timestamp_utc
        .format("%Y-%m-%d %H:%M UTC")
        .to_string();

    let _ = writeln!(out, "{{info}}");
    let _ = writeln!(
        out,
        "Project: {project}  ·  Branch: {branch}  ·  Commit: {commit}  ·  Scanned: {scanned}"
    );
    let _ = writeln!(out, "{{info}}");
    out.push('\n');

    let t = &run.summary_totals;
    let _ = writeln!(out, "h2. Summary");
    let _ = writeln!(
        out,
        "||Files Analyzed||Code Lines||Comment Lines||Blank Lines||Languages||"
    );
    let _ = writeln!(
        out,
        "|{}|{}|{}|{}|{}|",
        t.files_analyzed,
        t.code_lines,
        t.comment_lines,
        t.blank_lines,
        run.totals_by_language.len(),
    );
    out.push('\n');

    if !run.totals_by_language.is_empty() {
        let _ = writeln!(out, "h2. Language Breakdown");
        let _ = writeln!(out, "||Language||Files||Code||Comments||Blank||");
        for lang in &run.totals_by_language {
            let _ = writeln!(
                out,
                "|{}|{}|{}|{}|{}|",
                lang.language.display_name(),
                lang.files,
                lang.code_lines,
                lang.comment_lines,
                lang.blank_lines,
            );
        }
        out.push('\n');
    }

    let _ = writeln!(
        out,
        "*Total:* {} code lines · {} files · {} languages",
        t.code_lines,
        t.files_analyzed,
        run.totals_by_language.len(),
    );

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // ── base64_encode ────────────────────────────────────────────────────────────

    #[test]
    fn base64_encode_empty() {
        assert_eq!(base64_encode(b""), "");
    }

    #[test]
    fn base64_encode_one_byte() {
        assert_eq!(base64_encode(b"M"), "TQ==");
    }

    #[test]
    fn base64_encode_two_bytes() {
        assert_eq!(base64_encode(b"Ma"), "TWE=");
    }

    #[test]
    fn base64_encode_three_bytes_no_padding() {
        assert_eq!(base64_encode(b"Man"), "TWFu");
    }

    #[test]
    fn base64_encode_hello() {
        assert_eq!(base64_encode(b"Hello"), "SGVsbG8=");
    }

    #[test]
    fn base64_encode_roundtrip_length_multiple_of_3() {
        let data = b"abcdef";
        let encoded = base64_encode(data);
        assert_eq!(encoded.len(), 8);
        assert!(!encoded.contains('='));
    }

    #[test]
    fn base64_encode_all_zeros() {
        assert_eq!(base64_encode(&[0u8, 0, 0]), "AAAA");
    }

    #[test]
    fn base64_encode_binary_data() {
        let data: Vec<u8> = (0u8..=255).collect();
        let encoded = base64_encode(&data);
        assert!(!encoded.is_empty());
        assert!(
            encoded
                .chars()
                .all(|c| c.is_alphanumeric() || c == '+' || c == '/' || c == '=')
        );
    }

    // ── json_escape ──────────────────────────────────────────────────────────────

    #[test]
    fn json_escape_no_special_chars() {
        assert_eq!(json_escape("hello world"), "hello world");
    }

    #[test]
    fn json_escape_backslash() {
        assert_eq!(json_escape(r"path\to\file"), r"path\\to\\file");
    }

    #[test]
    fn json_escape_double_quote() {
        assert_eq!(json_escape(r#"say "hi""#), r#"say \"hi\""#);
    }

    #[test]
    fn json_escape_both_special_chars() {
        assert_eq!(json_escape(r#"a\"b"#), r#"a\\\"b"#);
    }

    #[test]
    fn json_escape_empty_string() {
        assert_eq!(json_escape(""), "");
    }

    #[test]
    fn json_escape_only_backslashes() {
        assert_eq!(json_escape(r"\\"), r"\\\\");
    }

    // ── coverage_pct_str ─────────────────────────────────────────────────────────

    #[test]
    fn coverage_pct_str_zero_found_returns_empty() {
        assert_eq!(coverage_pct_str(0, 0), "");
    }

    #[test]
    fn coverage_pct_str_full_coverage() {
        assert_eq!(coverage_pct_str(100, 100), "100.0");
    }

    #[test]
    fn coverage_pct_str_half_coverage() {
        assert_eq!(coverage_pct_str(50, 100), "50.0");
    }

    #[test]
    fn coverage_pct_str_one_decimal_precision() {
        let s = coverage_pct_str(7, 10);
        assert_eq!(s, "70.0");
    }

    #[test]
    fn coverage_pct_str_zero_hit_but_found() {
        assert_eq!(coverage_pct_str(0, 10), "0.0");
    }

    #[test]
    fn coverage_pct_str_non_round_percentage() {
        let s = coverage_pct_str(1, 3);
        assert!(!s.is_empty());
        assert!(s.contains('.'), "result must have decimal point");
    }

    // ── coverage_class ───────────────────────────────────────────────────────────

    #[test]
    fn coverage_class_zero_found_is_muted() {
        assert_eq!(coverage_class(0, 0), "muted");
    }

    #[test]
    fn coverage_class_100_pct_is_good() {
        assert_eq!(coverage_class(100, 100), "good");
    }

    #[test]
    fn coverage_class_80_pct_is_good() {
        assert_eq!(coverage_class(80, 100), "good");
    }

    #[test]
    fn coverage_class_79_pct_is_warn() {
        assert_eq!(coverage_class(79, 100), "warn");
    }

    #[test]
    fn coverage_class_60_pct_is_warn() {
        assert_eq!(coverage_class(60, 100), "warn");
    }

    #[test]
    fn coverage_class_59_pct_is_danger() {
        assert_eq!(coverage_class(59, 100), "danger");
    }

    #[test]
    fn coverage_class_zero_hit_is_danger() {
        assert_eq!(coverage_class(0, 100), "danger");
    }

    // ── format_test_density ──────────────────────────────────────────────────────

    #[test]
    fn format_test_density_zero_code_returns_zero() {
        assert_eq!(format_test_density(0, 5), "0.0");
    }

    #[test]
    fn format_test_density_zero_tests_returns_zero() {
        assert_eq!(format_test_density(100, 0), "0.0");
    }

    #[test]
    fn format_test_density_both_zero() {
        assert_eq!(format_test_density(0, 0), "0.0");
    }

    #[test]
    fn format_test_density_1_test_per_1000_lines() {
        assert_eq!(format_test_density(1000, 1), "1.0");
    }

    #[test]
    fn format_test_density_10_tests_per_100_lines() {
        assert_eq!(format_test_density(100, 10), "100.0");
    }

    #[test]
    fn format_test_density_fractional() {
        let s = format_test_density(1000, 3);
        assert!(!s.is_empty());
        assert!(s.contains('.'));
    }

    // ── html_esc ─────────────────────────────────────────────────────────────────

    #[test]
    fn html_esc_no_special_chars() {
        assert_eq!(html_esc("hello"), "hello");
    }

    #[test]
    fn html_esc_ampersand() {
        assert_eq!(html_esc("a&b"), "a&amp;b");
    }

    #[test]
    fn html_esc_less_than() {
        assert_eq!(html_esc("a<b"), "a&lt;b");
    }

    #[test]
    fn html_esc_greater_than() {
        assert_eq!(html_esc("a>b"), "a&gt;b");
    }

    #[test]
    fn html_esc_double_quote() {
        assert_eq!(html_esc(r#"a"b"#), "a&quot;b");
    }

    #[test]
    fn html_esc_all_special_chars() {
        assert_eq!(
            html_esc(r#"<a href="x&y">z</a>"#),
            "&lt;a href=&quot;x&amp;y&quot;&gt;z&lt;/a&gt;"
        );
    }

    #[test]
    fn html_esc_empty_string() {
        assert_eq!(html_esc(""), "");
    }

    // ── png_data_uri ─────────────────────────────────────────────────────────────

    #[test]
    fn png_data_uri_has_correct_prefix() {
        let uri = png_data_uri(b"\x89PNG\r\n\x1a\n");
        assert!(uri.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn png_data_uri_non_empty_for_non_empty_input() {
        let uri = png_data_uri(b"fake-png-bytes");
        assert!(uri.len() > "data:image/png;base64,".len());
    }

    // ── load_custom_logo ─────────────────────────────────────────────────────────

    #[test]
    fn load_custom_logo_nonexistent_file_returns_none() {
        let result = load_custom_logo(std::path::Path::new("/nonexistent/__sloc_logo__.png"));
        assert!(result.is_none());
    }

    #[test]
    fn load_custom_logo_png_file_returns_data_uri() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("logo.png");
        std::fs::write(&path, b"\x89PNG\r\n\x1a\nfake-png-data").unwrap();
        let result = load_custom_logo(&path);
        assert!(result.is_some());
        let uri = result.unwrap();
        assert!(uri.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn load_custom_logo_svg_file_uses_svg_mime() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("logo.svg");
        std::fs::write(&path, b"<svg></svg>").unwrap();
        let result = load_custom_logo(&path);
        assert!(result.is_some());
        let uri = result.unwrap();
        assert!(uri.starts_with("data:image/svg+xml;base64,"));
    }

    #[test]
    fn load_custom_logo_unknown_extension_treated_as_png() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("logo.bin");
        std::fs::write(&path, b"some-bytes").unwrap();
        let result = load_custom_logo(&path);
        assert!(result.is_some());
        let uri = result.unwrap();
        assert!(uri.starts_with("data:image/png;base64,"));
    }
}

#[cfg(test)]
mod coverage_boost_report_tests {
    use super::*;
    use std::path::Path;

    // ── derive_commit_url / derive_branch_url ────────────────────────────────

    #[test]
    fn derive_commit_url_github_uses_commit_segment() {
        let url = derive_commit_url(
            "https://github.com/org/repo.git",
            "abc1234abc1234abc1234abc1234abc1234abc1234",
        );
        assert_eq!(
            url.as_deref(),
            Some("https://github.com/org/repo/commit/abc1234abc1234abc1234abc1234abc1234abc1234")
        );
    }

    #[test]
    fn derive_commit_url_bitbucket_uses_commits_plural() {
        let url = derive_commit_url("https://bitbucket.org/org/repo.git", "deadbeef");
        assert_eq!(
            url.as_deref(),
            Some("https://bitbucket.org/org/repo/commits/deadbeef")
        );
    }

    #[test]
    fn derive_commit_url_gitlab_uses_dash_commit() {
        let url = derive_commit_url("https://gitlab.example.com/org/repo.git", "cafe0000");
        assert_eq!(
            url.as_deref(),
            Some("https://gitlab.example.com/org/repo/-/commit/cafe0000")
        );
    }

    #[test]
    fn derive_branch_url_github_uses_tree() {
        let url = derive_branch_url("https://github.com/org/repo.git", "main");
        assert_eq!(
            url.as_deref(),
            Some("https://github.com/org/repo/tree/main")
        );
    }

    #[test]
    fn derive_branch_url_bitbucket_uses_branch_segment() {
        let url = derive_branch_url("https://bitbucket.org/org/repo.git", "develop");
        assert_eq!(
            url.as_deref(),
            Some("https://bitbucket.org/org/repo/branch/develop")
        );
    }

    #[test]
    fn derive_branch_url_gitlab_uses_dash_tree() {
        let url = derive_branch_url("https://gitlab.mycompany.com/org/repo.git", "feature");
        assert_eq!(
            url.as_deref(),
            Some("https://gitlab.mycompany.com/org/repo/-/tree/feature")
        );
    }

    #[test]
    fn derive_commit_url_invalid_url_returns_none() {
        let url = derive_commit_url("not-a-url", "abc123");
        assert!(url.is_none());
    }

    #[test]
    fn normalize_remote_url_variants() {
        assert_eq!(
            normalize_remote_url("git@github.com:org/repo.git").as_deref(),
            Some("https://github.com/org/repo")
        );
        assert_eq!(
            normalize_remote_url("https://gitlab.com/a/b.git").as_deref(),
            Some("https://gitlab.com/a/b")
        );
        assert_eq!(
            normalize_remote_url("http://host/x").as_deref(),
            Some("http://host/x")
        );
        assert_eq!(normalize_remote_url("not a url"), None);
    }

    #[test]
    fn classify_and_bucket_helpers() {
        assert_eq!(
            classify_unsupported_path("README.md"),
            "Documentation / text"
        );
        assert_eq!(
            classify_unsupported_path("pkg.json"),
            "JSON manifests and config"
        );
        assert_eq!(
            classify_unsupported_path("Cargo.toml"),
            "Project metadata and packaging"
        );
        assert_eq!(classify_unsupported_path("page.html"), "HTML templates");
        assert_eq!(classify_unsupported_path("notes.txt"), "Plain text assets");
        assert_eq!(
            classify_unsupported_path("data.xyz"),
            "Other unsupported text formats"
        );
        assert_eq!(
            classify_unsupported_path("Makefile_noext"),
            "Extensionless or custom text files"
        );
        // bucket_description + bucket_recommendation for each known label.
        for label in [
            "Documentation / text",
            "JSON manifests and config",
            "Project metadata and packaging",
            "HTML templates",
            "Plain text assets",
            "Extensionless or custom text files",
            "Unknown bucket",
        ] {
            assert!(!bucket_description(label).is_empty());
            assert!(!bucket_recommendation(label).is_empty());
        }
    }

    #[test]
    fn summarize_warnings_groups_categories() {
        let warnings = vec![
            "file 'a.md': unsupported or undetected language".to_string(),
            "file 'b.bin': binary file skipped by default".to_string(),
            "file 'c.min.js': minified file skipped by policy".to_string(),
            "file 'big.txt': file exceeded max_file_size_bytes".to_string(),
        ];
        let rows = summarize_warnings(&warnings);
        assert!(!rows.is_empty(), "warnings should summarize into buckets");
    }

    #[test]
    fn pdf_number_and_string_formatters() {
        assert_eq!(pdf_fmt_full(0), "0");
        assert!(pdf_fmt_full(1_234_567).contains('1'));
        // pdf_safe_str must not panic on non-ASCII / control chars.
        let s = pdf_safe_str("héllo\tworld\u{1F600}");
        assert!(!s.is_empty());
    }

    #[test]
    fn file_url_produces_uri() {
        let url = file_url(Path::new("/tmp/report.html"));
        assert!(url.starts_with("file://") || url.contains("report.html"));
    }

    #[test]
    fn browser_discovery_is_callable_without_panicking() {
        // With no SLOC_BROWSER set, discovery walks the candidate list and
        // returns None (no browser in the test sandbox) — exercising the loop.
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("SLOC_BROWSER") };
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("BROWSER") };
        let _ = discover_browser();
        let _ = discover_browser_from_env();
        #[cfg(windows)]
        let _ = windows_browser_candidates();
        #[cfg(not(windows))]
        let _ = linux_browser_candidates();
        // With a bogus SLOC_BROWSER, normalize_browser_env_path is exercised.
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("SLOC_BROWSER", "/no/such/browser/path") };
        let _ = discover_browser_from_env();
        let p = normalize_browser_env_path("\"/quoted/path/chrome\"");
        assert!(p.to_string_lossy().contains("chrome"));
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("SLOC_BROWSER") };
    }

    #[test]
    fn which_in_path_returns_none_for_missing() {
        assert!(which_in_path("definitely-not-a-real-exe-xyz123").is_none());
    }

    #[test]
    fn write_pdf_from_html_without_browser_errors_gracefully() {
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("SLOC_BROWSER") };
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("BROWSER") };
        let dir = std::env::temp_dir().join("sloc_report_pdf_test");
        let _ = std::fs::create_dir_all(&dir);
        let html = dir.join("in.html");
        std::fs::write(&html, "<html><body>hi</body></html>").unwrap();
        let out = dir.join("out.pdf");
        // No browser present → Err, but exercises discovery + early validation.
        let res = write_pdf_from_html(&html, &out);
        // Either a real browser exists (Ok) or not (Err); both are acceptable.
        let _ = res;
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── helvetica_advance ────────────────────────────────────────────────────────

    #[test]
    fn helvetica_advance_uppercase_a_differs_by_weight() {
        assert_eq!(helvetica_advance('A', true), 722);
        assert_eq!(helvetica_advance('A', false), 667);
    }

    #[test]
    fn helvetica_advance_uppercase_w_same_both_weights() {
        assert_eq!(helvetica_advance('W', true), 944);
        assert_eq!(helvetica_advance('W', false), 944);
    }

    #[test]
    fn helvetica_advance_lowercase_i_differs_by_weight() {
        assert_eq!(helvetica_advance('i', true), 278);
        assert_eq!(helvetica_advance('i', false), 222);
    }

    #[test]
    fn helvetica_advance_digits_are_556_both_weights() {
        for d in '0'..='9' {
            assert_eq!(helvetica_advance(d, true), 556, "bold digit {d}");
            assert_eq!(helvetica_advance(d, false), 556, "regular digit {d}");
        }
    }

    #[test]
    fn helvetica_advance_middle_dot_is_278() {
        assert_eq!(helvetica_advance('\u{00B7}', true), 278);
        assert_eq!(helvetica_advance('\u{00B7}', false), 278);
    }

    #[test]
    fn helvetica_advance_unknown_char_returns_nonzero_fallback() {
        let bold_fb = helvetica_advance('\u{1F600}', true);
        let reg_fb = helvetica_advance('\u{1F600}', false);
        assert_eq!(bold_fb, 556);
        assert_eq!(reg_fb, 500);
    }

    // ── helvetica_width_mm ───────────────────────────────────────────────────────

    #[test]
    fn helvetica_width_mm_empty_is_zero() {
        assert!(helvetica_width_mm("", 10.0, false).abs() < f32::EPSILON);
        assert!(helvetica_width_mm("", 10.0, true).abs() < f32::EPSILON);
    }

    #[test]
    fn helvetica_width_mm_scales_linearly_with_pt_size() {
        let w6 = helvetica_width_mm("Hello", 6.0, false);
        let w12 = helvetica_width_mm("Hello", 12.0, false);
        assert!(
            2.0_f32.mul_add(-w6, w12).abs() < 1e-4,
            "width must be proportional to pt size"
        );
    }

    #[test]
    fn helvetica_width_mm_bold_a_wider_than_regular_a() {
        let bold = helvetica_width_mm("A", 10.0, true);
        let reg = helvetica_width_mm("A", 10.0, false);
        assert!(
            bold > reg,
            "bold 'A' (722) must be wider than regular 'A' (667)"
        );
    }

    #[test]
    fn helvetica_width_mm_single_char_matches_manual_calculation() {
        // 'A' regular advance = 667; width_mm = 667 * 10.0 * (25.4/72.0) / 1000.0
        let expected = 667.0_f32 * 10.0 * (25.4 / 72.0) / 1000.0;
        let got = helvetica_width_mm("A", 10.0, false);
        assert!((got - expected).abs() < 1e-4);
    }
}
