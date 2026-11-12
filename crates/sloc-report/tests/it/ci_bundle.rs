// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>

//! Integration tests for the standalone CI bundle (summary payload, CSP externalization, and the
//! full `write_ci_bundle` artifact set). Runs on every platform, not just the Windows Pester gate.

use sloc_config::AppConfig;
use sloc_core::{
    AnalysisRun, CoverageDiagnostics, CoverageOutcome, EnvironmentMetadata, LanguageSummary,
    SummaryTotals, ToolMetadata,
};
use sloc_languages::Language;
use sloc_report::{
    CI_SUMMARY_SCHEMA, build_ci_summary, externalize_report_assets, render_ci_dashboard_html,
    write_ci_bundle,
};

fn make_run() -> AnalysisRun {
    let mut cfg = AppConfig::default();
    cfg.reporting.report_title = "Fixture Report".into();
    AnalysisRun {
        tool: ToolMetadata {
            name: "sloc".into(),
            version: "9.9.9".into(),
            run_id: "20260101-0000-abc".into(),
            timestamp_utc: chrono::Utc::now(),
        },
        environment: EnvironmentMetadata {
            operating_system: "linux".into(),
            architecture: "x86_64".into(),
            runtime_mode: "cli".into(),
            initiator_username: "u".into(),
            initiator_hostname: "h".into(),
            ci_name: None,
        },
        effective_configuration: cfg,
        input_roots: vec!["/tmp/test".into()],
        summary_totals: SummaryTotals {
            files_considered: 2,
            files_analyzed: 2,
            total_physical_lines: 120,
            code_lines: 90,
            comment_lines: 20,
            blank_lines: 10,
            test_count: 4,
            test_assertion_count: 8,
            test_suite_count: 2,
            coverage_lines_found: 50,
            coverage_lines_hit: 40,
            ..SummaryTotals::default()
        },
        totals_by_language: vec![LanguageSummary {
            language: Language::Cpp,
            files: 2,
            total_physical_lines: 120,
            code_lines: 90,
            comment_lines: 20,
            blank_lines: 10,
            ..language_zero()
        }],
        per_file_records: vec![],
        skipped_file_records: vec![],
        warnings: vec![],
        submodule_summaries: vec![],
        git_commit_short: None,
        git_commit_long: None,
        git_branch: None,
        git_commit_author: None,
        git_tags: None,
        git_nearest_tag: None,
        git_commit_date: None,
        git_remote_url: None,
        style_summary: None,
        cocomo: None,
        uloc: 0,
        dryness_pct: None,
        duplicate_groups: vec![],
        duplicates_excluded: 0,
        authors: Vec::new(),
        coverage_diagnostics: Some(CoverageDiagnostics {
            resolved_path: "/tmp/coverage.info".into(),
            outcome: CoverageOutcome::Matched,
            entries_parsed: 2,
            files_matched: 2,
            files_unmatched: 0,
            detail: String::new(),
        }),
    }
}

// LanguageSummary has more numeric fields than we set; fill the rest with zero via a default-ish
// helper so the fixture stays readable.
fn language_zero() -> LanguageSummary {
    LanguageSummary {
        language: Language::Cpp,
        files: 0,
        total_physical_lines: 0,
        code_lines: 0,
        comment_lines: 0,
        blank_lines: 0,
        mixed_lines_separate: 0,
        functions: 0,
        classes: 0,
        variables: 0,
        variables_member: 0,
        variables_local: 0,
        variables_global: 0,
        macro_definitions: 0,
        imports: 0,
        test_count: 0,
        test_assertion_count: 0,
        test_suite_count: 0,
        coverage_lines_found: 0,
        coverage_lines_hit: 0,
        coverage_functions_found: 0,
        coverage_functions_hit: 0,
        coverage_branches_found: 0,
        coverage_branches_hit: 0,
        cyclomatic_complexity: 0,
        lsloc: None,
    }
}

#[test]
fn ci_summary_carries_stable_schema_and_metrics() {
    let run = make_run();
    let s = build_ci_summary(&run);
    assert_eq!(s.schema, CI_SUMMARY_SCHEMA);
    assert_eq!(s.report_title, "Fixture Report");
    assert_eq!(s.scanner_version, "9.9.9");
    assert_eq!(s.totals.code_lines, 90);
    assert_eq!(s.lexical_tests.test_functions, 4);
    assert_eq!(s.lexical_tests.assertions, 8);
    assert!(s.lexical_tests.note.contains("not executed test results"));
    let cov = s.coverage.expect("imported coverage present");
    assert_eq!(cov.source, "imported");
    assert_eq!(cov.lines_found, 50);
    assert_eq!(cov.lines_hit, 40);
    assert!(s.coverage_diagnostics.is_some());
    assert_eq!(s.languages.len(), 1);
}

#[test]
fn ci_dashboard_links_every_artifact() {
    let html = render_ci_dashboard_html(&make_run());
    for needle in [
        "report.html",
        "report.pdf",
        "report.csv",
        "report.xlsx",
        "result.json",
        "summary.json",
        "scan-config.json",
        "csp/report.html",
        "import/",
    ] {
        assert!(html.contains(needle), "dashboard missing link: {needle}");
    }
    // Clearly separates lexical detections from executed results.
    assert!(html.contains("lexical detections, NOT executed"));
}

#[test]
fn externalize_removes_inline_style_and_script() {
    let html = "<html><head><style>b{color:red}</style></head>\
                <body><script>var y=2;</script></body></html>";
    let bundle = externalize_report_assets(html);
    assert!(!bundle.html.contains("<style"));
    assert!(bundle.html.contains("assets/report.css"));
    assert!(bundle.html.contains("assets/report-001.js"));
    assert!(bundle.assets.iter().any(|(n, _)| n == "assets/report.css"));
}

#[test]
fn write_ci_bundle_materializes_full_artifact_set() {
    let dir = std::env::temp_dir().join(format!("oxide_ci_bundle_it_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let paths = write_ci_bundle(&make_run(), &dir).expect("write bundle");
    for rel in [
        "result.json",
        "report.html",
        "report.pdf",
        "report.csv",
        "report.xlsx",
        "scan-config.json",
        "summary.json",
        "index.html",
        "assets/dashboard.css",
        "csp/report.html",
        "import/run.json",
        "import/summary.json",
        "import/README.txt",
    ] {
        assert!(dir.join(rel).is_file(), "missing bundle artifact: {rel}");
    }
    assert!(paths.dashboard_html.ends_with("index.html"));
    // The HTML report links its sibling PDF so the offline View-PDF button works.
    let report_html = std::fs::read_to_string(dir.join("report.html")).unwrap();
    assert!(report_html.contains("data-standalone-pdf=\"report.pdf\""));
    let _ = std::fs::remove_dir_all(&dir);
}
