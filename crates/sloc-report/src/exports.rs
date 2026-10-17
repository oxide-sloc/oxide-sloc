// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>

//! Machine-readable interchange exports that let oxide-sloc slot into the code-scanning and
//! line-counting ecosystems other tools already speak:
//!
//! * **SARIF 2.1.0** — ingested natively by GitHub code scanning and by GitLab SAST
//!   (`sast` report artifact). Findings surface as PR / MR inline annotations and in the
//!   Security tab.
//! * **Code Climate JSON** — GitLab's Code Quality widget format (MR quality diff).
//! * **Bitbucket Code Insights** — a `{ report, annotations }` document ready to `PUT`/`POST`
//!   to the Bitbucket Code Insights REST API, rendering an inline report on pull requests.
//! * **cloc-compatible JSON** — the de-facto `cloc --json` shape (`header` + `SUM` + per-language
//!   objects) so existing cloc-consuming dashboards and scripts accept oxide-sloc output with no
//!   changes.
//!
//! The first three share one finding model: per-file high cyclomatic complexity (above a
//! configurable threshold) and any per-file analyzer warnings. cloc JSON is a pure totals
//! projection and carries no findings.

use std::hash::{Hash, Hasher};
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};
use sloc_core::AnalysisRun;

/// Default per-file cyclomatic-complexity ceiling used to raise a finding when the caller does
/// not pass an explicit threshold (mirrors the CLI `--max-complexity` gate when it is unset).
pub const DEFAULT_COMPLEXITY_THRESHOLD: u32 = 25;

const TOOL_NAME: &str = "oxide-sloc";
const TOOL_URI: &str = "https://github.com/oxide-sloc/oxide-sloc";
const RULE_COMPLEXITY: &str = "oxide-sloc/high-complexity";
const RULE_WARNING: &str = "oxide-sloc/file-warning";

/// Tuning for the finding-producing exporters (SARIF / Code Climate / Bitbucket).
#[derive(Debug, Clone, Copy)]
pub struct CodeScanOptions {
    /// Files whose approximate cyclomatic complexity exceeds this value raise a finding.
    pub complexity_threshold: u32,
}

impl Default for CodeScanOptions {
    fn default() -> Self {
        Self {
            complexity_threshold: DEFAULT_COMPLEXITY_THRESHOLD,
        }
    }
}

/// Severity shared across the three code-scanning formats; each exporter maps it to its own
/// vocabulary (SARIF level, Code Climate severity, Bitbucket annotation severity).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Severity {
    Note,
    Warning,
    Error,
}

/// One normalized finding, format-agnostic. `line` is 1-based; we report line 1 for whole-file
/// findings since the lexical analyzer does not retain per-finding line numbers.
struct Finding {
    rule: &'static str,
    severity: Severity,
    message: String,
    path: String,
    line: u32,
}

/// Build the shared finding list from a run. Deterministic order (per-file record order) so the
/// emitted reports and their fingerprints are stable across identical scans.
fn collect_findings(run: &AnalysisRun, opts: CodeScanOptions) -> Vec<Finding> {
    let mut findings = Vec::new();
    for rec in &run.per_file_records {
        let path = rec.relative_path.replace('\\', "/");
        if let Some(cc) = rec.cyclomatic_complexity
            && opts.complexity_threshold > 0
            && cc > opts.complexity_threshold
        {
            findings.push(Finding {
                rule: RULE_COMPLEXITY,
                // Grade a run-away file (2x the ceiling) up to error so it fails stricter gates.
                severity: if cc > opts.complexity_threshold.saturating_mul(2) {
                    Severity::Error
                } else {
                    Severity::Warning
                },
                message: format!(
                    "Approximate cyclomatic complexity {cc} exceeds threshold {} — \
                     consider splitting this file.",
                    opts.complexity_threshold
                ),
                path: path.clone(),
                line: 1,
            });
        }
        for w in &rec.warnings {
            findings.push(Finding {
                rule: RULE_WARNING,
                severity: Severity::Note,
                message: w.clone(),
                path: path.clone(),
                line: 1,
            });
        }
    }
    findings
}

/// Stable 16-hex-char fingerprint for a finding (Code Climate / Bitbucket external id). Uses the
/// fixed-key `DefaultHasher`, which is deterministic across runs and platforms for equal input.
fn fingerprint(f: &Finding) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    f.rule.hash(&mut h);
    f.path.hash(&mut h);
    f.line.hash(&mut h);
    f.message.hash(&mut h);
    format!("{:016x}", h.finish())
}

// ── SARIF 2.1.0 (GitHub code scanning + GitLab SAST) ────────────────────────────

fn sarif_level(sev: Severity) -> &'static str {
    match sev {
        Severity::Note => "note",
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}

/// Serialize a run to a SARIF 2.1.0 document (as pretty JSON string).
pub fn render_sarif(run: &AnalysisRun, opts: CodeScanOptions) -> String {
    let findings = collect_findings(run, opts);

    let rules = json!([
        {
            "id": RULE_COMPLEXITY,
            "name": "HighCyclomaticComplexity",
            "shortDescription": { "text": "File exceeds the configured cyclomatic-complexity threshold." },
            "helpUri": TOOL_URI,
            "defaultConfiguration": { "level": "warning" }
        },
        {
            "id": RULE_WARNING,
            "name": "FileAnalysisWarning",
            "shortDescription": { "text": "oxide-sloc emitted a per-file analysis warning." },
            "helpUri": TOOL_URI,
            "defaultConfiguration": { "level": "note" }
        }
    ]);

    let results: Vec<Value> = findings
        .iter()
        .map(|f| {
            json!({
                "ruleId": f.rule,
                "level": sarif_level(f.severity),
                "message": { "text": f.message },
                "locations": [{
                    "physicalLocation": {
                        "artifactLocation": { "uri": f.path },
                        "region": { "startLine": f.line }
                    }
                }],
                "partialFingerprints": { "oxideSlocFingerprint/v1": fingerprint(f) }
            })
        })
        .collect();

    let doc = json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": TOOL_NAME,
                    "informationUri": TOOL_URI,
                    "version": env!("CARGO_PKG_VERSION"),
                    "rules": rules
                }
            },
            "results": results
        }]
    });

    serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".to_string())
}

/// Write the SARIF document to `path`.
pub fn write_sarif(run: &AnalysisRun, path: &Path, opts: CodeScanOptions) -> Result<()> {
    std::fs::write(path, render_sarif(run, opts))
        .with_context(|| format!("writing SARIF to {}", path.display()))
}

// ── Code Climate JSON (GitLab Code Quality) ─────────────────────────────────────

fn codeclimate_severity(sev: Severity) -> &'static str {
    match sev {
        Severity::Note => "minor",
        Severity::Warning => "major",
        Severity::Error => "critical",
    }
}

/// Serialize a run to the GitLab Code Quality (Code Climate) JSON array as a pretty string.
pub fn render_codeclimate(run: &AnalysisRun, opts: CodeScanOptions) -> String {
    let findings = collect_findings(run, opts);
    let issues: Vec<Value> = findings
        .iter()
        .map(|f| {
            json!({
                "type": "issue",
                "check_name": f.rule,
                "description": f.message,
                "categories": ["Complexity"],
                "severity": codeclimate_severity(f.severity),
                "fingerprint": fingerprint(f),
                "location": {
                    "path": f.path,
                    "lines": { "begin": f.line }
                }
            })
        })
        .collect();
    serde_json::to_string_pretty(&Value::Array(issues)).unwrap_or_else(|_| "[]".to_string())
}

/// Write the Code Climate / GitLab Code Quality report to `path`.
pub fn write_codeclimate(run: &AnalysisRun, path: &Path, opts: CodeScanOptions) -> Result<()> {
    std::fs::write(path, render_codeclimate(run, opts))
        .with_context(|| format!("writing Code Climate report to {}", path.display()))
}

// ── Bitbucket Code Insights ─────────────────────────────────────────────────────

fn bitbucket_severity(sev: Severity) -> &'static str {
    match sev {
        Severity::Note => "LOW",
        Severity::Warning => "MEDIUM",
        Severity::Error => "HIGH",
    }
}

/// Serialize a run to a Bitbucket Code Insights `{ report, annotations }` document (pretty JSON).
///
/// The `report` object is the body for `PUT .../reports/{key}` and `annotations` is the body for
/// `POST .../reports/{key}/annotations` (Bitbucket Cloud) / the equivalent Data Center endpoint.
pub fn render_bitbucket(run: &AnalysisRun, opts: CodeScanOptions) -> String {
    let findings = collect_findings(run, opts);
    let t = &run.summary_totals;
    let failed = findings
        .iter()
        .any(|f| matches!(f.severity, Severity::Warning | Severity::Error));

    let annotations: Vec<Value> = findings
        .iter()
        .map(|f| {
            json!({
                "external_id": fingerprint(f),
                "path": f.path,
                "line": f.line,
                "summary": f.message,
                "severity": bitbucket_severity(f.severity),
                "annotation_type": "CODE_SMELL"
            })
        })
        .collect();

    let doc = json!({
        "report": {
            "title": "oxide-sloc code metrics",
            "details": format!(
                "{} files, {} code lines across {} languages.",
                t.files_analyzed,
                t.code_lines,
                run.totals_by_language.len()
            ),
            "report_type": "BUG",
            "reporter": TOOL_NAME,
            "link": TOOL_URI,
            "result": if failed { "FAILED" } else { "PASSED" },
            "data": [
                { "title": "Code lines",  "type": "NUMBER", "value": t.code_lines },
                { "title": "Comment lines", "type": "NUMBER", "value": t.comment_lines },
                { "title": "Files analyzed", "type": "NUMBER", "value": t.files_analyzed },
                { "title": "Test functions", "type": "NUMBER", "value": t.test_count },
                { "title": "Cyclomatic complexity", "type": "NUMBER", "value": t.cyclomatic_complexity }
            ]
        },
        "annotations": annotations
    });

    serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".to_string())
}

/// Write the Bitbucket Code Insights document to `path`.
pub fn write_bitbucket(run: &AnalysisRun, path: &Path, opts: CodeScanOptions) -> Result<()> {
    std::fs::write(path, render_bitbucket(run, opts)).with_context(|| {
        format!(
            "writing Bitbucket Code Insights report to {}",
            path.display()
        )
    })
}

// ── cloc-compatible JSON ─────────────────────────────────────────────────────────

/// Serialize a run to the de-facto `cloc --json` shape: a `header`, a `SUM` roll-up, and one
/// object per language keyed by its display name. Lets cloc-consuming tooling accept oxide-sloc
/// output unchanged. Pretty JSON string.
pub fn render_cloc_json(run: &AnalysisRun) -> String {
    let t = &run.summary_totals;
    let mut root = Map::new();

    root.insert(
        "header".to_string(),
        json!({
            "cloc_url": TOOL_URI,
            "cloc_version": format!("{} {}", TOOL_NAME, env!("CARGO_PKG_VERSION")),
            "elapsed_seconds": 0,
            "n_files": t.files_analyzed,
            "n_lines": t.total_physical_lines,
            "files_per_second": 0,
            "lines_per_second": 0
        }),
    );

    for lang in &run.totals_by_language {
        root.insert(
            lang.language.display_name().to_string(),
            json!({
                "nFiles": lang.files,
                "blank": lang.blank_lines,
                "comment": lang.comment_lines,
                "code": lang.code_lines
            }),
        );
    }

    root.insert(
        "SUM".to_string(),
        json!({
            "blank": t.blank_lines,
            "comment": t.comment_lines,
            "code": t.code_lines,
            "nFiles": t.files_analyzed
        }),
    );

    serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_else(|_| "{}".to_string())
}

/// Write cloc-compatible JSON to `path`.
pub fn write_cloc_json(run: &AnalysisRun, path: &Path) -> Result<()> {
    std::fs::write(path, render_cloc_json(run))
        .with_context(|| format!("writing cloc-compatible JSON to {}", path.display()))
}
