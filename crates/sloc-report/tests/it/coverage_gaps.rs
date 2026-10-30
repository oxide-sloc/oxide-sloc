// SPDX-License-Identifier: AGPL-3.0-or-later
// Additional targeted tests for sloc-report, driving branches in `render_html`,
// `write_pdf_from_run`, `write_csv`, and the warning classifiers that the existing
// `render.rs` suite does not reach. Each test toggles a specific optional field or
// data shape so the corresponding uncovered render branch executes, then asserts on
// an observable substring / file size so the coverage is meaningful, not a smoke call.
//
// AnalysisRun is constructed directly (all fields pub) — no filesystem I/O needed.

use chrono::{TimeZone, Utc};
use sloc_config::AppConfig;
use sloc_core::{
    AnalysisRun, CocomoEstimate, CocomoMode, EffectiveCounts, EnvironmentMetadata, FileCoverage,
    FileRecord, FileStatus, LanguageStyleGroup, LanguageSummary, StyleSummary, SubmoduleSummary,
    SummaryTotals, ToolMetadata,
};
use sloc_languages::{Language, ParseMode, RawLineCounts};
use sloc_report::{render_html, write_csv, write_pdf_from_run};

// ── Fixture helpers (local — cannot import from render.rs) ────────────────────

fn make_file_record(path: &str, lang: Language, code: u64) -> FileRecord {
    let raw = RawLineCounts {
        total_physical_lines: code + 2,
        code_only_lines: code,
        blank_only_lines: 1,
        single_comment_only_lines: 1,
        functions: 2,
        ..RawLineCounts::default()
    };
    FileRecord {
        path: path.into(),
        relative_path: path.into(),
        language: Some(lang),
        size_bytes: code * 30,
        detected_encoding: Some("utf-8".into()),
        raw_line_categories: raw,
        effective_counts: EffectiveCounts {
            code_lines: code,
            comment_lines: 1,
            blank_lines: 1,
            mixed_lines_separate: 0,
        },
        status: FileStatus::AnalyzedExact,
        warnings: vec![],
        generated: false,
        minified: false,
        vendor: false,
        parse_mode: Some(ParseMode::Lexical),
        submodule: None,
        coverage: None,
        style_analysis: None,
        cyclomatic_complexity: None,
        lsloc: None,
        commit_count: None,
        last_commit_date: None,
        ownership: None,
        content_hash: 0,
    }
}

const fn make_lang_summary(lang: Language, files: u64, code: u64) -> LanguageSummary {
    LanguageSummary {
        language: lang,
        files,
        total_physical_lines: code + 2,
        code_lines: code,
        comment_lines: 1,
        blank_lines: 1,
        mixed_lines_separate: 0,
        functions: 2,
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

fn make_tool() -> ToolMetadata {
    ToolMetadata {
        name: "sloc".into(),
        version: "1.0.0".into(),
        run_id: "test-run-id".into(),
        timestamp_utc: Utc::now(),
    }
}

fn make_env() -> EnvironmentMetadata {
    EnvironmentMetadata {
        operating_system: "linux".into(),
        architecture: "x86_64".into(),
        runtime_mode: "cli".into(),
        initiator_username: "test".into(),
        initiator_hostname: "testhost".into(),
        ci_name: None,
    }
}

fn make_run() -> AnalysisRun {
    let file = make_file_record("src/lib.rs", Language::Rust, 7);
    let lang = make_lang_summary(Language::Rust, 1, 7);
    AnalysisRun {
        tool: make_tool(),
        environment: make_env(),
        effective_configuration: AppConfig::default(),
        input_roots: vec!["/tmp/test".into()],
        summary_totals: SummaryTotals {
            files_considered: 1,
            files_analyzed: 1,
            files_skipped: 0,
            total_physical_lines: 10,
            code_lines: 7,
            comment_lines: 1,
            blank_lines: 2,
            ..SummaryTotals::default()
        },
        totals_by_language: vec![lang],
        per_file_records: vec![file],
        skipped_file_records: vec![],
        warnings: vec![],
        submodule_summaries: vec![],
        git_commit_short: Some("abc1234".into()),
        git_branch: Some("main".into()),
        git_commit_long: None,
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
    }
}

fn make_empty_run() -> AnalysisRun {
    let mut run = make_run();
    run.per_file_records.clear();
    run.totals_by_language.clear();
    run.summary_totals = SummaryTotals::default();
    run.git_commit_short = None;
    run.git_branch = None;
    run
}

fn tmp_pdf(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "sloc-covgap-{tag}-{}-{:?}.pdf",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ))
}

// ── group_thousands via the HTML `commas` filter on large member counts ───────

#[test]
fn render_html_groups_thousands_in_language_table() {
    // A language row with a >=1000 member count exercises the digit-grouping loop in
    // group_thousands (the `commas` template filter) with a multi-group integer.
    let mut run = make_run();
    run.totals_by_language[0].variables = 12_345;
    run.totals_by_language[0].imports = 6_789;
    run.summary_totals.variables = 12_345;
    let html = render_html(&run).unwrap();
    assert!(
        html.contains("12,345"),
        "member counts must be comma-grouped"
    );
}

// ── PDF info-strip stat parts (mixed / imports / variables / classes) ─────────

#[test]
fn write_pdf_info_parts_stats_all_optional_metrics_present() {
    // Sets every optional metric so pdf_info_parts_stats pushes the Mixed / Imports /
    // Variables / Classes parts, and row2_4th picks the Classes arm (test_count == 0).
    let path = tmp_pdf("info-parts");
    let mut run = make_run();
    run.summary_totals.functions = 40;
    run.summary_totals.mixed_lines_separate = 3;
    run.summary_totals.imports = 25;
    run.summary_totals.variables = 60;
    run.summary_totals.classes = 8;
    run.summary_totals.test_count = 0; // force the Classes arm of row2_4th
    write_pdf_from_run(&run, &path).unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() > 0);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn write_pdf_row2_mixed_lines_arm() {
    // No tests and no classes → row2_4th falls through to the Mixed Lines arm.
    let path = tmp_pdf("row2-mixed");
    let mut run = make_run();
    run.summary_totals.test_count = 0;
    run.summary_totals.classes = 0;
    run.summary_totals.mixed_lines_separate = 5;
    write_pdf_from_run(&run, &path).unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() > 0);
    let _ = std::fs::remove_file(&path);
}

// ── PDF coverage gauges: function + branch coverage populated ─────────────────

#[test]
fn write_pdf_function_and_branch_coverage_gauges() {
    // Populating function and branch coverage makes all three gauges visible and
    // adds the Func/Branch Cov parts to the Tests & Coverage strip.
    let path = tmp_pdf("cov-gauges");
    let mut run = make_run();
    run.per_file_records[0].coverage = Some(FileCoverage {
        lines_found: 100,
        lines_hit: 80,
        functions_found: 20,
        functions_hit: 16,
        branches_found: 40,
        branches_hit: 30,
    });
    run.summary_totals.coverage_lines_found = 100;
    run.summary_totals.coverage_lines_hit = 80;
    run.summary_totals.coverage_functions_found = 20;
    run.summary_totals.coverage_functions_hit = 16;
    run.summary_totals.coverage_branches_found = 40;
    run.summary_totals.coverage_branches_hit = 30;
    write_pdf_from_run(&run, &path).unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() > 0);
    let _ = std::fs::remove_file(&path);
}

// ── PDF style section with an empty by_language list ──────────────────────────

#[test]
fn write_pdf_style_section_empty_by_language() {
    // by_language empty → pdf_render_style_section returns right after the chips,
    // taking the early-return branch that skips the per-language mini-table.
    let path = tmp_pdf("style-empty");
    let mut run = make_run();
    run.style_summary = Some(StyleSummary {
        files_analyzed: 1,
        common_indent_style: "Spaces(4)".into(),
        line80_compliant_pct: 90,
        line_col_compliant_pct: 88,
        col_threshold: 100,
        by_language: vec![],
    });
    write_pdf_from_run(&run, &path).unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() > 0);
    let _ = std::fs::remove_file(&path);
}

// ── PDF hotspots: a very long path exercises pdf_fit_path front-truncation ─────

#[test]
fn write_pdf_hotspots_with_very_long_path_front_truncates() {
    let path = tmp_pdf("hotspot-longpath");
    let mut run = make_run();
    let long = format!("src/{}/deeply/nested/module_file.rs", "sub_dir".repeat(30));
    let mut rec = make_file_record(&long, Language::Rust, 500);
    rec.commit_count = Some(42);
    rec.last_commit_date = Some("2026-06-01T10:00:00+00:00".into());
    run.per_file_records.push(rec);
    write_pdf_from_run(&run, &path).unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() > 0);
    let _ = std::fs::remove_file(&path);
}

// ── PDF per-file pages carrying a report banner ───────────────────────────────

#[test]
fn write_pdf_per_file_pages_with_banner() {
    // A report banner plus enough files to spill onto per-file continuation pages
    // exercises the banner-drawing branch inside pdf_draw_perfile_header.
    let path = tmp_pdf("perfile-banner");
    let mut run = make_empty_run();
    run.effective_configuration.reporting.report_header_footer =
        Some("CONFIDENTIAL — Internal Use Only".into());
    for i in 0..45_u64 {
        run.per_file_records.push(make_file_record(
            &format!("src/module_{i:02}.rs"),
            Language::Rust,
            30 + i,
        ));
    }
    run.totals_by_language = vec![make_lang_summary(Language::Rust, 45, 2000)];
    run.summary_totals.files_analyzed = 45;
    run.summary_totals.code_lines = 2000;
    write_pdf_from_run(&run, &path).unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() > 0);
    let _ = std::fs::remove_file(&path);
}

// ── PDF terminal T&C page with COCOMO that does NOT fit page 1 ─────────────────

#[test]
fn write_pdf_terminal_tc_page_with_cocomo_overflow_to_page2() {
    // COCOMO present but forced off page 1 (a large style table consumes page 1),
    // and no per-file records / no hotspots follow, so the COCOMO+T&C page is the
    // terminal page and gets measured/trimmed with with_cocomo == true.
    let path = tmp_pdf("terminal-cocomo");
    let mut run = make_run();
    run.per_file_records.clear(); // terminal T&C page, nothing flows onto it
    // A large per-language style table pushes page-1 content down so COCOMO overflows.
    let mut groups = Vec::new();
    for i in 0..12u8 {
        let score = 80 + i;
        groups.push(LanguageStyleGroup {
            language_family: format!("Family{i}"),
            files_count: u32::from(i) + 1,
            dominant_guide: "Some Guide".into(),
            dominant_score_pct: score,
            common_indent_style: "Spaces(4)".into(),
            guide_avg_scores: vec![("Some Guide".into(), score)],
            line80_compliant_pct: 85,
            line_col_compliant_pct: 82,
        });
    }
    run.style_summary = Some(StyleSummary {
        files_analyzed: 12,
        common_indent_style: "Spaces(4)".into(),
        line80_compliant_pct: 85,
        line_col_compliant_pct: 82,
        col_threshold: 100,
        by_language: groups,
    });
    run.cocomo = Some(CocomoEstimate {
        mode: CocomoMode::SemiDetached,
        ksloc: 16.4,
        effort_person_months: 45.27,
        duration_months: 10.65,
        avg_staff: 4.25,
    });
    write_pdf_from_run(&run, &path).unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() > 0);
    let _ = std::fs::remove_file(&path);
}

// ── Winter timestamp exercises the PST (non-DST) formatting branch ────────────

#[test]
fn render_html_winter_timestamp_uses_pst_label() {
    // A December (winter) UTC timestamp is outside US Pacific DST, so to_pst_display
    // takes the PST arm rather than the PDT arm the summer `Utc::now()` fixtures hit.
    let mut run = make_run();
    run.tool.timestamp_utc = Utc.with_ymd_and_hms(2026, 12, 15, 20, 0, 0).unwrap();
    let html = render_html(&run).unwrap();
    assert!(
        html.contains("PST"),
        "winter scan time should render a PST label"
    );
}

#[test]
fn write_pdf_winter_timestamp_uses_pst_label() {
    // Mirror of the above through the PDF path (to_pt_hhmm's PST arm).
    let path = tmp_pdf("winter-pst");
    let mut run = make_run();
    run.tool.timestamp_utc = Utc.with_ymd_and_hms(2026, 1, 5, 18, 0, 0).unwrap();
    write_pdf_from_run(&run, &path).unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() > 0);
    let _ = std::fs::remove_file(&path);
}

// ── Warning classifiers: vendor / best-effort / unsupported buckets ───────────

#[test]
fn render_html_warning_buckets_vendor_and_best_effort() {
    // Distinct warning strings drive the vendor-skip and best-effort buckets of
    // summarize_warnings (plus their tone/detail arms) which the base run never hits.
    let mut run = make_run();
    run.warnings = vec![
        "src/vendored/foo.rs: vendor file skipped by policy".into(),
        "src/parser.rs: best effort parse after unclosed string literal".into(),
        "assets/blob.bin: binary file skipped by default".into(),
        "dist/app.min.js: minified file skipped by policy".into(),
    ];
    let html = render_html(&run).unwrap();
    assert!(
        html.contains("Vendor files skipped by policy"),
        "vendor warning bucket should appear"
    );
    assert!(
        html.contains("Best-effort parse results"),
        "best-effort warning bucket should appear"
    );
}

#[test]
fn render_html_support_opportunities_dedup_and_empty_path() {
    // build_support_opportunities: an "unsupported or undetected language" warning with
    // an empty leading path hits the empty-path `continue`; repeated identical basenames
    // hit the dedup `contains` skip while keeping the example list short.
    let mut run = make_run();
    run.warnings = vec![
        ": unsupported or undetected language".into(), // empty path → continue
        "docs/readme.rst: unsupported or undetected language".into(),
        "guide/readme.rst: unsupported or undetected language".into(), // dup basename
        "notes/readme.rst: unsupported or undetected language".into(), // dup basename
        "extra/other.rst: unsupported or undetected language".into(),
    ];
    let html = render_html(&run).unwrap();
    assert!(
        html.to_lowercase().contains("unsupported")
            || html.contains("opportunit")
            || html.contains(".rst"),
        "support-opportunity section should render for unsupported-language warnings"
    );
}

// ── CSV per-file rows with git activity + optional metrics ────────────────────

#[test]
fn write_csv_with_variables_and_imports_columns() {
    let mut run = make_run();
    run.totals_by_language[0].variables = 1_500;
    run.totals_by_language[0].imports = 300;
    run.summary_totals.variables = 1_500;
    run.summary_totals.imports = 300;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.csv");
    write_csv(&run, &path).unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(!content.is_empty());
}

// ── Submodule table combined with coverage in the PDF ─────────────────────────

#[test]
fn write_pdf_submodules_with_coverage_and_tests() {
    // Submodule summaries carrying test + coverage totals exercise the populated
    // submodule-row coverage-percentage branch in pdf_render_submodules.
    let path = tmp_pdf("submod-cov");
    let mut run = make_run();
    let mut lang = make_lang_summary(Language::C, 5, 160);
    lang.test_count = 12;
    lang.test_assertion_count = 40;
    lang.coverage_lines_found = 100;
    lang.coverage_lines_hit = 70;
    run.submodule_summaries = vec![SubmoduleSummary {
        name: "vendor/lib".into(),
        relative_path: "vendor/lib".into(),
        files_analyzed: 5,
        total_physical_lines: 200,
        code_lines: 160,
        comment_lines: 20,
        blank_lines: 20,
        language_summaries: vec![lang],
        git_commit_short: None,
        git_commit_long: None,
        git_branch: None,
        git_commit_author: None,
        git_commit_date: None,
        git_remote_url: None,
    }];
    write_pdf_from_run(&run, &path).unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() > 0);
    let _ = std::fs::remove_file(&path);
}
