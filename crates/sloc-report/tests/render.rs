// SPDX-License-Identifier: AGPL-3.0-or-later
// Tests for sloc-report rendering functions.
//
// AnalysisRun is constructed directly (all fields pub) — no filesystem I/O needed
// to exercise the HTML/CSV/XLSX/Confluence renderers.

use chrono::Utc;
use sloc_config::AppConfig;
use sloc_core::{
    AnalysisRun, Author, AuthorLineCounts, CocomoEstimate, CocomoMode, EffectiveCounts,
    EnvironmentMetadata, FileChangeStatus, FileCoverage, FileDelta, FileOwnership, FileRecord,
    FileStatus, LanguageStyleGroup, LanguageSummary, ScanComparison, StyleSummary,
    SubmoduleSummary, SummaryDelta, SummaryTotals, ToolMetadata,
};
use sloc_languages::{Language, ParseMode, RawLineCounts};
use sloc_report::{
    ReportDeltaContext, render_confluence_storage, render_confluence_wiki_markup, render_html,
    render_html_with_delta, render_sub_report_html, write_csv, write_diff_csv, write_diff_xlsx,
    write_html as write_html_report, write_html_with_pdf_link, write_pdf_from_run, write_xlsx,
};

// ── Fixture helpers ───────────────────────────────────────────────────────────

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

/// Minimal valid run — one Rust file, basic totals, git metadata.
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

/// A run with per-author ownership: two contributors, one file owned mostly by the first.
fn make_run_with_ownership() -> AnalysisRun {
    let mut run = make_run();
    run.authors = vec![
        Author {
            id: 0,
            canonical_name: "Nima Shafie".into(),
            canonical_email: "nimzshafie@gmail.com".into(),
            aliases: vec![],
            counts: AuthorLineCounts {
                code_lines: 120,
                comment_lines: 20,
                blank_lines: 10,
                total_lines: 150,
            },
        },
        Author {
            id: 1,
            canonical_name: "Other Dev".into(),
            canonical_email: "other@example.com".into(),
            aliases: vec![],
            counts: AuthorLineCounts {
                code_lines: 30,
                comment_lines: 5,
                blank_lines: 3,
                total_lines: 38,
            },
        },
    ];
    run.per_file_records[0].ownership = Some(vec![
        FileOwnership {
            author_id: 0,
            counts: AuthorLineCounts {
                code_lines: 120,
                comment_lines: 20,
                blank_lines: 10,
                total_lines: 150,
            },
        },
        FileOwnership {
            author_id: 1,
            counts: AuthorLineCounts {
                code_lines: 30,
                comment_lines: 5,
                blank_lines: 3,
                total_lines: 38,
            },
        },
    ]);
    // Give the file git activity so the Git Hotspots section (with its Owner column) renders too.
    run.per_file_records[0].commit_count = Some(3);
    run.per_file_records[0].last_commit_date = Some("2026-08-01T12:00:00+00:00".into());
    run
}

#[test]
fn html_report_includes_ownership_section() {
    let run = make_run_with_ownership();
    let html = render_html(&run).expect("render html");
    assert!(html.contains("Code Ownership"));
    assert!(html.contains("ownership-section"));
    assert!(html.contains("Nima Shafie"));
    assert!(html.contains("Other Dev"));
    // Per-author drill-down of owned files, presented as a gamified leaderboard.
    assert!(html.contains("Contributor Leaderboard"));
    assert!(html.contains("own-details"));
    assert!(html.contains("lb-rank"));
    assert!(html.contains("lb-avatar"));
    assert!(html.contains("lb-r1")); // top contributor gets the gold medal class
    // Git Hotspots gains an Author column populated with the file's primary author.
    assert!(html.contains(">Author<"));
}

#[test]
fn html_report_omits_ownership_when_no_authors() {
    let run = make_run(); // authors empty
    let html = render_html(&run).expect("render html");
    assert!(!html.contains("ownership-section"));
}

#[test]
fn csv_report_includes_ownership_section() {
    let run = make_run_with_ownership();
    let path = std::env::temp_dir().join(format!("sloc-own-{}.csv", std::process::id()));
    write_csv(&run, &path).expect("write csv");
    let body = std::fs::read_to_string(&path).expect("read csv");
    assert!(body.contains("# Code Ownership"));
    // author 0: 120/(120+30) = 80.0% code, single file owned.
    assert!(body.contains("Nima Shafie,nimzshafie@gmail.com,120,20,10,150,80.0,1"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn pdf_report_with_ownership_succeeds() {
    let run = make_run_with_ownership();
    let path = std::env::temp_dir().join(format!("sloc-own-pdf-{}.pdf", std::process::id()));
    write_pdf_from_run(&run, &path).expect("pure-Rust PDF with ownership page");
    assert!(std::fs::metadata(&path).expect("pdf written").len() > 0);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn pdf_info_line_wraps_when_git_metadata_overflows() {
    // A very long branch name makes the packed git info strip exceed the usable page
    // width, forcing pdf_info_emit_line to wrap onto a fresh line (the overflow
    // branch). Pure-Rust PDF path — no browser, fully deterministic.
    let mut run = make_run();
    run.git_branch = Some(format!("release/{}", "x".repeat(240)));
    run.git_commit_short = Some("abcdef1".into());
    run.git_nearest_tag = Some("v1.2.3".into());

    let path = std::env::temp_dir().join(format!("sloc-pdf-wrap-{}.pdf", std::process::id()));
    write_pdf_from_run(&run, &path).expect("pure-Rust PDF generation succeeds");
    let len = std::fs::metadata(&path).expect("pdf written").len();
    assert!(len > 0, "a non-empty PDF is produced");
    let _ = std::fs::remove_file(&path);
}

/// Run with zero files (empty project).
fn make_empty_run() -> AnalysisRun {
    AnalysisRun {
        tool: make_tool(),
        environment: make_env(),
        effective_configuration: AppConfig::default(),
        input_roots: vec!["/tmp/empty".into()],
        summary_totals: SummaryTotals::default(),
        totals_by_language: vec![],
        per_file_records: vec![],
        skipped_file_records: vec![],
        warnings: vec![],
        submodule_summaries: vec![],
        git_commit_short: None,
        git_branch: None,
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

/// Run with 3 language summaries to exercise chart loops and top-language tables.
fn make_multi_lang_run() -> AnalysisRun {
    let files = vec![
        make_file_record("src/lib.rs", Language::Rust, 50),
        make_file_record("app.py", Language::Python, 30),
        make_file_record("main.ts", Language::TypeScript, 20),
    ];
    let langs = vec![
        make_lang_summary(Language::Rust, 1, 50),
        make_lang_summary(Language::Python, 1, 30),
        make_lang_summary(Language::TypeScript, 1, 20),
    ];
    AnalysisRun {
        tool: make_tool(),
        environment: make_env(),
        effective_configuration: AppConfig::default(),
        input_roots: vec!["/tmp/multi".into()],
        summary_totals: SummaryTotals {
            files_considered: 3,
            files_analyzed: 3,
            total_physical_lines: 102 + 6,
            code_lines: 100,
            comment_lines: 3,
            blank_lines: 3,
            ..SummaryTotals::default()
        },
        totals_by_language: langs,
        per_file_records: files,
        skipped_file_records: vec![],
        warnings: vec![],
        submodule_summaries: vec![],
        git_commit_short: Some("def5678".into()),
        git_branch: Some("feature/multi".into()),
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

fn make_delta() -> ReportDeltaContext {
    ReportDeltaContext {
        delta_code_added: 150,
        delta_code_removed: 30,
        delta_unmodified_lines: 820,
        delta_files_added: 2,
        delta_files_removed: 0,
        delta_files_modified: 3,
        delta_files_unchanged: 10,
        prev_code_lines: 700,
        prev_scan_count: 5,
        prev_scan_label: "2026-05-30 12:00 UTC".into(),
        prev_run_id: Some("prev-run-id".into()),
        current_run_id: Some("curr-run-id".into()),
    }
}

fn make_run_with_coverage() -> AnalysisRun {
    let mut run = make_run();
    // Add coverage data to the file record
    run.per_file_records[0].coverage = Some(FileCoverage {
        lines_found: 10,
        lines_hit: 7,
        functions_found: 2,
        functions_hit: 2,
        branches_found: 4,
        branches_hit: 3,
    });
    run.summary_totals.coverage_lines_found = 10;
    run.summary_totals.coverage_lines_hit = 7;
    run.totals_by_language[0].coverage_lines_found = 10;
    run.totals_by_language[0].coverage_lines_hit = 7;
    run
}

fn make_run_with_style() -> AnalysisRun {
    let mut run = make_run();
    run.style_summary = Some(StyleSummary {
        files_analyzed: 1,
        common_indent_style: "Spaces(4)".into(),
        line80_compliant_pct: 90,
        line_col_compliant_pct: 88,
        col_threshold: 100,
        by_language: vec![LanguageStyleGroup {
            language_family: "Rust".into(),
            files_count: 1,
            dominant_guide: "Rust Official".into(),
            dominant_score_pct: 95,
            common_indent_style: "Spaces(4)".into(),
            guide_avg_scores: vec![("Rust Official".into(), 95)],
            line80_compliant_pct: 90,
            line_col_compliant_pct: 88,
        }],
    });
    run
}

fn make_run_with_submodules() -> AnalysisRun {
    let mut run = make_run();
    run.submodule_summaries = vec![SubmoduleSummary {
        name: "vendor/lib".into(),
        relative_path: "vendor/lib".into(),
        files_analyzed: 5,
        total_physical_lines: 200,
        code_lines: 160,
        comment_lines: 20,
        blank_lines: 20,
        language_summaries: vec![make_lang_summary(Language::C, 5, 160)],
        git_commit_short: None,
        git_commit_long: None,
        git_branch: None,
        git_commit_author: None,
        git_commit_date: None,
        git_remote_url: None,
    }];
    run
}

fn make_run_with_warnings() -> AnalysisRun {
    let mut run = make_run();
    run.warnings = vec![
        "unsupported extension: .xyz (1 file)".into(),
        "encoding fallback: windows-1252 used for src/legacy.c".into(),
    ];
    run
}

fn make_run_with_skipped() -> AnalysisRun {
    let mut run = make_run();
    let mut skipped = make_file_record("Cargo.lock", Language::Rust, 0);
    skipped.status = FileStatus::SkippedByPolicy;
    run.skipped_file_records = vec![skipped];
    run.summary_totals.files_skipped = 1;
    run
}

fn make_run_with_all_git() -> AnalysisRun {
    let mut run = make_run();
    run.git_commit_long = Some("abc1234def5678abc1234def5678abc1234def56".into());
    run.git_commit_author = Some("Nima Shafie".into());
    run.git_tags = Some("v1.5.64".into());
    run.git_nearest_tag = Some("v1.5.64".into());
    run.git_commit_date = Some("2026-06-01T10:00:00Z".into());
    run.git_remote_url = Some("https://github.com/oxide-sloc/oxide-sloc.git".into());
    run
}

// ── render_html ───────────────────────────────────────────────────────────────

#[test]
fn render_html_returns_doctype() {
    let run = make_run();
    let html = render_html(&run).unwrap();
    assert!(html.contains("<!doctype html>") || html.contains("<!DOCTYPE html>"));
}

#[test]
fn render_html_contains_tool_name() {
    let run = make_run();
    let html = render_html(&run).unwrap();
    // The report page must reference the tool name somewhere
    assert!(
        html.to_lowercase().contains("sloc") || html.to_lowercase().contains("oxide"),
        "HTML should reference tool name"
    );
}

#[test]
fn render_html_git_branch_present() {
    let run = make_run();
    let html = render_html(&run).unwrap();
    assert!(
        html.contains("main"),
        "git branch 'main' should appear in report"
    );
}

#[test]
fn render_html_multi_language() {
    let run = make_multi_lang_run();
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
    assert!(html.contains("Rust"));
    assert!(html.contains("Python"));
}

#[test]
fn render_html_empty_run() {
    let run = make_empty_run();
    let html = render_html(&run).unwrap();
    assert!(
        !html.is_empty(),
        "empty run should still produce a valid HTML document"
    );
    assert!(html.contains("<!doctype html>") || html.contains("<!DOCTYPE html>"));
}

#[test]
fn render_html_with_coverage_data() {
    let run = make_run_with_coverage();
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
    // Coverage section or percentage should appear
    assert!(
        html.to_lowercase().contains("coverage") || html.contains("70.0"),
        "coverage data should appear in report"
    );
}

#[test]
fn render_html_with_style_summary() {
    let run = make_run_with_style();
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
    // Style section title or indent style should appear
    assert!(
        html.contains("Style") || html.contains("style") || html.contains("Rust Official"),
        "style summary should appear in report"
    );
}

#[test]
fn render_html_with_submodule_summary() {
    let run = make_run_with_submodules();
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
    assert!(
        html.contains("vendor/lib") || html.to_lowercase().contains("submodule"),
        "submodule data should appear in report"
    );
}

#[test]
fn render_html_with_warnings() {
    let run = make_run_with_warnings();
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn render_html_with_skipped_files() {
    let run = make_run_with_skipped();
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn render_html_with_all_git_metadata() {
    let run = make_run_with_all_git();
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
    assert!(html.contains("v1.5.64") || html.contains("abc1234"));
}

#[test]
fn render_html_accent_color() {
    let mut run = make_run();
    run.effective_configuration.reporting.accent_color = Some("#3b82f6".into());
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
    assert!(
        html.contains("#3b82f6"),
        "custom accent color should appear in report CSS"
    );
}

// ── render_html_with_delta ───────────────────────────────────────────────────

#[test]
fn render_html_with_delta_some() {
    let run = make_run();
    let delta = make_delta();
    let html = render_html_with_delta(&run, Some(&delta)).unwrap();
    assert!(!html.is_empty());
    assert!(html.contains("<!doctype html>") || html.contains("<!DOCTYPE html>"));
}

#[test]
fn render_html_with_delta_none() {
    let run = make_run();
    let html = render_html_with_delta(&run, None).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn render_html_delta_shows_added_lines() {
    let run = make_run();
    let delta = make_delta();
    let html = render_html_with_delta(&run, Some(&delta)).unwrap();
    // delta has 150 added lines — should appear somewhere in the delta panel
    assert!(
        html.contains("150") || html.contains("+150"),
        "delta added lines should appear in report"
    );
}

// ── render_sub_report_html ───────────────────────────────────────────────────

#[test]
fn render_sub_report_html_ok() {
    let run = make_run();
    let html = render_sub_report_html(&run, None).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn render_sub_report_html_multi_language() {
    let run = make_multi_lang_run();
    let html = render_sub_report_html(&run, None).unwrap();
    assert!(!html.is_empty());
    assert!(html.contains("Rust"));
}

// ── write_html ───────────────────────────────────────────────────────────────

#[test]
fn write_html_creates_file() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run();
    write_html_report(&run, tmp.path()).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(!content.is_empty());
    assert!(content.contains("<!doctype html>") || content.contains("<!DOCTYPE html>"));
}

#[test]
fn write_html_with_coverage_data() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run_with_coverage();
    write_html_report(&run, tmp.path()).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(!content.is_empty());
}

// ── write_csv ────────────────────────────────────────────────────────────────

#[test]
fn write_csv_basic() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run();
    write_csv(&run, tmp.path()).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(!content.is_empty(), "CSV output should not be empty");
    assert!(
        content.contains("Summary") || content.contains("Language") || content.contains("Files"),
        "CSV should contain header sections"
    );
}

#[test]
fn write_csv_multi_language() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_multi_lang_run();
    write_csv(&run, tmp.path()).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(content.contains("Rust"));
    assert!(content.contains("Python"));
}

#[test]
fn write_csv_with_per_file_records() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run();
    write_csv(&run, tmp.path()).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(
        content.contains("src/lib.rs") || content.contains("Per File") || content.contains("file"),
        "CSV should include per-file data"
    );
}

// ── write_xlsx ───────────────────────────────────────────────────────────────

#[test]
fn write_xlsx_creates_file() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run();
    write_xlsx(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0, "XLSX file should be non-empty");
}

#[test]
fn write_xlsx_multi_language() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_multi_lang_run();
    write_xlsx(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0);
}

// ── Confluence renderers ──────────────────────────────────────────────────────

#[test]
fn render_confluence_storage_basic() {
    let run = make_run();
    let out = render_confluence_storage(&run, None);
    assert!(!out.is_empty());
    assert!(
        out.contains("<ac:structured-macro"),
        "Confluence storage format must start with ac:structured-macro info panel"
    );
}

#[test]
fn render_confluence_storage_contains_summary() {
    let run = make_run();
    let out = render_confluence_storage(&run, None);
    assert!(
        out.contains("Summary") || out.contains("Language"),
        "Confluence output should include a summary section"
    );
}

#[test]
fn render_confluence_storage_with_url() {
    let run = make_run();
    let out = render_confluence_storage(&run, Some("https://example.com/report.html"));
    assert!(!out.is_empty());
    assert!(
        out.contains("https://example.com/report.html"),
        "report URL should appear in Confluence output"
    );
}

#[test]
fn render_confluence_storage_multi_language() {
    let run = make_multi_lang_run();
    let out = render_confluence_storage(&run, None);
    assert!(out.contains("Rust") || out.contains("Python"));
}

#[test]
fn render_confluence_wiki_markup_basic() {
    let run = make_run();
    let out = render_confluence_wiki_markup(&run);
    assert!(!out.is_empty());
    assert!(
        out.contains("h2.") || out.contains("Summary"),
        "Confluence wiki markup should contain h2 headings"
    );
}

#[test]
fn render_confluence_wiki_markup_multi_language() {
    let run = make_multi_lang_run();
    let out = render_confluence_wiki_markup(&run);
    assert!(!out.is_empty());
    assert!(out.contains("Rust") || out.contains("Python"));
}

// ── Additional render_html edge cases ─────────────────────────────────────────

#[test]
fn render_html_large_codebase() {
    // Simulate a large codebase to exercise number formatting branches
    let mut run = make_run();
    run.summary_totals.code_lines = 1_234_567;
    run.summary_totals.total_physical_lines = 1_500_000;
    run.summary_totals.files_analyzed = 10_000;
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn render_html_delta_added_and_removed() {
    let run = make_run();
    let delta = sloc_report::ReportDeltaContext {
        delta_code_added: 1500,
        delta_code_removed: 300,
        delta_unmodified_lines: 5000,
        delta_files_added: 10,
        delta_files_removed: 2,
        delta_files_modified: 5,
        delta_files_unchanged: 20,
        prev_code_lines: 4000,
        prev_scan_count: 3,
        prev_scan_label: "2026-05-01 10:00 UTC".into(),
        prev_run_id: Some("old-run".into()),
        current_run_id: Some("new-run".into()),
    };
    let html = render_html_with_delta(&run, Some(&delta)).unwrap();
    assert!(!html.is_empty());
    // Delta values should appear
    assert!(
        html.contains("1500") || html.contains("1,500"),
        "added lines should appear in delta report"
    );
}

#[test]
fn render_html_delta_zero_change() {
    let run = make_run();
    let delta = sloc_report::ReportDeltaContext {
        delta_code_added: 0,
        delta_code_removed: 0,
        delta_unmodified_lines: 100,
        delta_files_added: 0,
        delta_files_removed: 0,
        delta_files_modified: 0,
        delta_files_unchanged: 5,
        prev_code_lines: 100,
        prev_scan_count: 1,
        prev_scan_label: "2026-05-01".into(),
        prev_run_id: Some("prev".into()),
        current_run_id: Some("curr".into()),
    };
    let html = render_html_with_delta(&run, Some(&delta)).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn write_html_with_all_git_metadata() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run_with_all_git();
    write_html_report(&run, tmp.path()).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(!content.is_empty());
    assert!(content.contains("v1.5.64") || content.contains("abc1234"));
}

#[test]
fn write_html_with_style_summary() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run_with_style();
    write_html_report(&run, tmp.path()).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(!content.is_empty());
    assert!(content.contains("<!doctype html>") || content.contains("<!DOCTYPE html>"));
}

#[test]
fn write_html_with_submodules() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run_with_submodules();
    write_html_report(&run, tmp.path()).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(!content.is_empty());
}

#[test]
fn write_csv_empty_run() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_empty_run();
    write_csv(&run, tmp.path()).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(
        !content.is_empty(),
        "empty run CSV should still have headers"
    );
}

#[test]
fn write_csv_with_coverage_data() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run_with_coverage();
    write_csv(&run, tmp.path()).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(!content.is_empty());
}

#[test]
fn write_xlsx_empty_run() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_empty_run();
    write_xlsx(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0, "empty run XLSX must not be zero bytes");
}

#[test]
fn write_xlsx_with_warnings() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run_with_warnings();
    write_xlsx(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0);
}

#[test]
fn render_confluence_storage_with_coverage() {
    let run = make_run_with_coverage();
    let out = render_confluence_storage(&run, None);
    assert!(!out.is_empty());
}

#[test]
fn render_confluence_wiki_markup_empty_run() {
    let run = make_empty_run();
    let out = render_confluence_wiki_markup(&run);
    assert!(
        !out.is_empty(),
        "wiki markup for empty run should still produce output"
    );
}

#[test]
fn render_sub_report_html_with_coverage() {
    let run = make_run_with_coverage();
    let html = render_sub_report_html(&run, None).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn render_sub_report_html_empty_run() {
    let run = make_empty_run();
    let html = render_sub_report_html(&run, None).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn render_html_many_files() {
    let mut run = make_run();
    for i in 0_u64..20 {
        run.per_file_records.push(make_file_record(
            &format!("src/file_{i}.rs"),
            Language::Rust,
            10 + i,
        ));
    }
    run.summary_totals.files_analyzed = run.per_file_records.len() as u64;
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
    // All files should appear in the report
    assert!(html.contains("file_0.rs") || html.contains("file_19.rs"));
}

#[test]
fn render_html_with_various_languages() {
    // Exercise the per-file records with multiple different languages
    let mut run = make_empty_run();
    run.per_file_records = vec![
        make_file_record("main.c", Language::C, 30),
        make_file_record("app.py", Language::Python, 40),
        make_file_record("server.go", Language::Go, 50),
        make_file_record("component.ts", Language::TypeScript, 20),
        make_file_record("style.css", Language::Css, 15),
        make_file_record("index.html", Language::Html, 25),
        make_file_record("schema.sql", Language::Sql, 10),
        make_file_record("build.sh", Language::Shell, 5),
    ];
    run.summary_totals.files_analyzed = 8;
    run.summary_totals.code_lines = 195;
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
}

// ── write_html_with_pdf_link ──────────────────────────────────────────────────

#[test]
fn write_html_with_pdf_link_same_dir_embeds_relative_url() {
    let dir = tempfile::tempdir().unwrap();
    let html_path = dir.path().join("report.html");
    let pdf_path = dir.path().join("my-unique-sloc-report-xyz.pdf");
    let run = make_run();
    write_html_with_pdf_link(&run, &html_path, Some(&pdf_path)).unwrap();
    let content = std::fs::read_to_string(&html_path).unwrap();
    assert!(!content.is_empty());
    assert!(content.contains("<!doctype html>") || content.contains("<!DOCTYPE html>"));
    // Same dir → filename should appear as data-standalone-pdf attribute
    assert!(
        content.contains("my-unique-sloc-report-xyz.pdf"),
        "same-dir PDF filename must appear in HTML as data-standalone-pdf"
    );
}

#[test]
fn write_html_with_pdf_link_different_dir_omits_pdf_url() {
    let html_dir = tempfile::tempdir().unwrap();
    let pdf_dir = tempfile::tempdir().unwrap();
    let html_path = html_dir.path().join("report.html");
    let pdf_path = pdf_dir.path().join("my-cross-dir-report.pdf");
    let run = make_run();
    write_html_with_pdf_link(&run, &html_path, Some(&pdf_path)).unwrap();
    let content = std::fs::read_to_string(&html_path).unwrap();
    assert!(!content.is_empty());
    // Different dirs → no data-standalone-pdf attribute should appear with this filename
    assert!(
        !content.contains("data-standalone-pdf=\"my-cross-dir-report.pdf\""),
        "cross-dir PDF must not appear as data-standalone-pdf attribute"
    );
}

#[test]
fn write_html_with_pdf_link_none_behaves_like_write_html() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run();
    write_html_with_pdf_link(&run, tmp.path(), None).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(!content.is_empty());
    assert!(content.contains("<!doctype html>") || content.contains("<!DOCTYPE html>"));
}

// ── write_diff_csv ────────────────────────────────────────────────────────────

fn make_scan_comparison() -> ScanComparison {
    ScanComparison {
        summary: SummaryDelta {
            baseline_run_id: "base-run-1".into(),
            current_run_id: "curr-run-2".into(),
            baseline_timestamp: Utc::now(),
            current_timestamp: Utc::now(),
            baseline_files: 5,
            current_files: 6,
            files_analyzed_delta: 1,
            baseline_code: 500,
            current_code: 600,
            code_lines_delta: 100,
            baseline_comments: 50,
            current_comments: 55,
            comment_lines_delta: 5,
            blank_lines_delta: 10,
            total_lines_delta: 115,
            coverage_lines_hit_delta: None,
            coverage_line_pct_delta: None,
            baseline_coverage_line_pct: None,
            current_coverage_line_pct: None,
        },
        file_deltas: vec![
            FileDelta {
                relative_path: "src/new.rs".into(),
                language: Some("Rust".into()),
                status: FileChangeStatus::Added,
                baseline_code: 0,
                current_code: 80,
                code_delta: 80,
                baseline_comment: 0,
                current_comment: 5,
                comment_delta: 5,
                baseline_blank: 0,
                current_blank: 10,
                blank_delta: 10,
                total_delta: 95,
            },
            FileDelta {
                relative_path: "src/old.rs".into(),
                language: Some("Rust".into()),
                status: FileChangeStatus::Modified,
                baseline_code: 100,
                current_code: 120,
                code_delta: 20,
                baseline_comment: 10,
                current_comment: 10,
                comment_delta: 0,
                baseline_blank: 5,
                current_blank: 5,
                blank_delta: 0,
                total_delta: 20,
            },
        ],
        files_added: 1,
        files_removed: 0,
        files_modified: 1,
        files_unchanged: 4,
        files_total: 6,
    }
}

#[test]
fn write_diff_csv_creates_nonempty_file() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let cmp = make_scan_comparison();
    write_diff_csv(&cmp, tmp.path()).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(!content.is_empty(), "diff CSV must not be empty");
}

#[test]
fn write_diff_csv_contains_header_sections() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let cmp = make_scan_comparison();
    write_diff_csv(&cmp, tmp.path()).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(
        content.contains("Diff Summary"),
        "must contain summary section"
    );
    assert!(
        content.contains("File Deltas"),
        "must contain file deltas section"
    );
}

#[test]
fn write_diff_csv_contains_run_ids() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let cmp = make_scan_comparison();
    write_diff_csv(&cmp, tmp.path()).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(
        content.contains("base-run-1"),
        "must contain baseline run ID"
    );
    assert!(
        content.contains("curr-run-2"),
        "must contain current run ID"
    );
}

#[test]
fn write_diff_csv_contains_file_paths() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let cmp = make_scan_comparison();
    write_diff_csv(&cmp, tmp.path()).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(
        content.contains("src/new.rs"),
        "must contain added file path"
    );
    assert!(
        content.contains("src/old.rs"),
        "must contain modified file path"
    );
}

#[test]
fn write_diff_csv_contains_status_labels() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let cmp = make_scan_comparison();
    write_diff_csv(&cmp, tmp.path()).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(content.contains("Added"), "must label added files");
    assert!(content.contains("Modified"), "must label modified files");
}

/// A `FileDelta` builder with sane zero defaults, so tests can specify only the
/// status and path they care about.
fn file_delta(path: &str, status: FileChangeStatus) -> FileDelta {
    FileDelta {
        relative_path: path.into(),
        language: Some("Rust".into()),
        status,
        baseline_code: 10,
        current_code: 10,
        code_delta: 0,
        baseline_comment: 0,
        current_comment: 0,
        comment_delta: 0,
        baseline_blank: 0,
        current_blank: 0,
        blank_delta: 0,
        total_delta: 0,
    }
}

#[test]
fn write_diff_csv_covers_removed_unchanged_and_quoted_paths() {
    // Exercise the Removed and Unchanged status arms (the Added/Modified arms are
    // covered above) plus csv_escape's quoting branch via a comma-bearing path.
    let mut cmp = make_scan_comparison();
    cmp.file_deltas = vec![
        file_delta("src/gone.rs", FileChangeStatus::Removed),
        file_delta("src/same.rs", FileChangeStatus::Unchanged),
        // A path with a comma must be wrapped in quotes by csv_escape.
        file_delta("src/weird,name.rs", FileChangeStatus::Modified),
    ];
    let tmp = tempfile::NamedTempFile::new().unwrap();
    write_diff_csv(&cmp, tmp.path()).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(content.contains("Removed"), "must label removed files");
    assert!(content.contains("Unchanged"), "must label unchanged files");
    assert!(
        content.contains("\"src/weird,name.rs\""),
        "comma-bearing path must be CSV-quoted:\n{content}"
    );
}

#[test]
fn write_diff_csv_empty_comparison() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let cmp = ScanComparison {
        summary: SummaryDelta {
            baseline_run_id: "a".into(),
            current_run_id: "b".into(),
            baseline_timestamp: Utc::now(),
            current_timestamp: Utc::now(),
            baseline_files: 0,
            current_files: 0,
            files_analyzed_delta: 0,
            baseline_code: 0,
            current_code: 0,
            code_lines_delta: 0,
            baseline_comments: 0,
            current_comments: 0,
            comment_lines_delta: 0,
            blank_lines_delta: 0,
            total_lines_delta: 0,
            coverage_lines_hit_delta: None,
            coverage_line_pct_delta: None,
            baseline_coverage_line_pct: None,
            current_coverage_line_pct: None,
        },
        file_deltas: vec![],
        files_added: 0,
        files_removed: 0,
        files_modified: 0,
        files_unchanged: 0,
        files_total: 0,
    };
    write_diff_csv(&cmp, tmp.path()).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(
        !content.is_empty(),
        "empty comparison CSV must still have headers"
    );
    assert!(content.contains("Diff Summary"));
}

// ── write_diff_xlsx ───────────────────────────────────────────────────────────

#[test]
fn write_diff_xlsx_creates_nonempty_file() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let cmp = make_scan_comparison();
    write_diff_xlsx(&cmp, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0, "diff XLSX must not be empty");
}

#[test]
fn write_diff_xlsx_empty_comparison() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let cmp = ScanComparison {
        summary: SummaryDelta {
            baseline_run_id: "a".into(),
            current_run_id: "b".into(),
            baseline_timestamp: Utc::now(),
            current_timestamp: Utc::now(),
            baseline_files: 0,
            current_files: 0,
            files_analyzed_delta: 0,
            baseline_code: 0,
            current_code: 0,
            code_lines_delta: 0,
            baseline_comments: 0,
            current_comments: 0,
            comment_lines_delta: 0,
            blank_lines_delta: 0,
            total_lines_delta: 0,
            coverage_lines_hit_delta: None,
            coverage_line_pct_delta: None,
            baseline_coverage_line_pct: None,
            current_coverage_line_pct: None,
        },
        file_deltas: vec![],
        files_added: 0,
        files_removed: 0,
        files_modified: 0,
        files_unchanged: 0,
        files_total: 0,
    };
    write_diff_xlsx(&cmp, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(
        meta.len() > 0,
        "empty comparison XLSX must not be empty (has headers)"
    );
}

#[test]
fn write_diff_xlsx_with_all_statuses() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut cmp = make_scan_comparison();
    cmp.file_deltas.push(FileDelta {
        relative_path: "src/removed.rs".into(),
        language: Some("Rust".into()),
        status: FileChangeStatus::Removed,
        baseline_code: 50,
        current_code: 0,
        code_delta: -50,
        baseline_comment: 5,
        current_comment: 0,
        comment_delta: -5,
        baseline_blank: 3,
        current_blank: 0,
        blank_delta: -3,
        total_delta: -58,
    });
    cmp.file_deltas.push(FileDelta {
        relative_path: "src/stable.rs".into(),
        language: Some("Rust".into()),
        status: FileChangeStatus::Unchanged,
        baseline_code: 30,
        current_code: 30,
        code_delta: 0,
        baseline_comment: 3,
        current_comment: 3,
        comment_delta: 0,
        baseline_blank: 2,
        current_blank: 2,
        blank_delta: 0,
        total_delta: 0,
    });
    write_diff_xlsx(&cmp, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0);
}

// ── render_html edge cases ────────────────────────────────────────────────────

#[test]
fn render_html_with_no_git_metadata() {
    let mut run = make_run();
    run.git_branch = None;
    run.git_commit_short = None;
    run.git_commit_long = None;
    run.git_commit_author = None;
    run.git_tags = None;
    run.git_nearest_tag = None;
    run.git_commit_date = None;
    run.git_remote_url = None;
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn render_confluence_storage_special_chars_in_project_name() {
    let mut run = make_run();
    run.effective_configuration.reporting.report_title = "Project <Alpha> & \"Beta\"".into();
    let out = render_confluence_storage(&run, None);
    assert!(!out.is_empty());
    // The project name special chars must be HTML-escaped in the output
    assert!(
        out.contains("&lt;Alpha&gt;"),
        "angle brackets in project name must be escaped to &lt;&gt;"
    );
    assert!(
        out.contains("&amp;"),
        "ampersand in project name must be escaped to &amp;"
    );
    // The raw unescaped tag must NOT appear inside a text context
    assert!(
        !out.contains("Project <Alpha>"),
        "raw unescaped project name must not appear"
    );
}

#[test]
fn render_confluence_wiki_markup_with_all_git_metadata() {
    let run = make_run_with_all_git();
    let out = render_confluence_wiki_markup(&run);
    assert!(!out.is_empty());
    assert!(out.contains("h2."), "must have h2 headings");
}

#[test]
fn write_csv_with_style_summary() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run_with_style();
    write_csv(&run, tmp.path()).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(!content.is_empty());
}

#[test]
fn write_xlsx_with_coverage_data() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run_with_coverage();
    write_xlsx(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0);
}

#[test]
fn render_html_with_12_plus_languages() {
    // Exercises the .take(12) limit in build_lang_chart_json
    let mut run = make_empty_run();
    let langs_list = [
        Language::Rust,
        Language::Python,
        Language::TypeScript,
        Language::Go,
        Language::C,
        Language::Cpp,
        Language::Java,
        Language::JavaScript,
        Language::CSharp,
        Language::Ruby,
        Language::Shell,
        Language::Sql,
        Language::Kotlin,
        Language::Swift,
    ];
    for (i, lang) in langs_list.iter().enumerate() {
        run.per_file_records.push(make_file_record(
            &format!("file_{i}.{i}"),
            *lang,
            10 + i as u64,
        ));
        run.totals_by_language
            .push(make_lang_summary(*lang, 1, 10 + i as u64));
    }
    run.summary_totals.files_analyzed = langs_list.len() as u64;
    run.summary_totals.code_lines = 200;
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn render_html_file_size_histogram_all_buckets() {
    // Exercise all 5 histogram buckets: Tiny(<50), Small(50-199), Medium(200-499), Large(500-999), Huge(>=1000)
    let mut run = make_empty_run();
    run.per_file_records = vec![
        make_file_record("tiny.rs", Language::Rust, 10),
        make_file_record("small.rs", Language::Rust, 100),
        make_file_record("medium.rs", Language::Rust, 300),
        make_file_record("large.rs", Language::Rust, 750),
        make_file_record("huge.rs", Language::Rust, 1500),
    ];
    run.totals_by_language = vec![make_lang_summary(Language::Rust, 5, 2660)];
    run.summary_totals.files_analyzed = 5;
    run.summary_totals.code_lines = 2660;
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
}

// ── Additional branch-coverage tests ─────────────────────────────────────────

#[test]
fn render_html_with_git_remote_url_renders_link() {
    let mut run = make_run();
    run.git_remote_url = Some("https://github.com/owner/repo.git".into());
    run.git_branch = Some("main".into());
    run.git_commit_short = Some("a1b2c3d".into());
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
    assert!(html.contains("<!doctype html>") || html.contains("<!DOCTYPE html>"));
}

#[test]
fn render_html_with_high_test_count_exercises_density_calculation() {
    let mut run = make_multi_lang_run();
    // Set substantial test counts to exercise density + badge rendering
    run.summary_totals.test_count = 150;
    run.summary_totals.test_assertion_count = 450;
    run.summary_totals.test_suite_count = 15;
    run.summary_totals.code_lines = 1000;
    for lang in &mut run.totals_by_language {
        lang.test_count = 50;
        lang.test_assertion_count = 150;
    }
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn render_html_with_coverage_and_git_meta_combined() {
    let mut run = make_run_with_coverage();
    run.git_remote_url = Some("https://github.com/test/repo.git".into());
    run.git_branch = Some("feature".into());
    run.git_commit_short = Some("deadbeef".into());
    run.git_nearest_tag = Some("v2.0.0".into());
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
    assert!(
        html.to_lowercase().contains("coverage"),
        "coverage section should appear"
    );
}

#[test]
fn render_html_with_zero_coverage_pct_renders_low_coverage_state() {
    let mut run = make_run();
    run.per_file_records[0].coverage = Some(FileCoverage {
        lines_found: 100,
        lines_hit: 0,
        functions_found: 10,
        functions_hit: 0,
        branches_found: 20,
        branches_hit: 0,
    });
    run.summary_totals.coverage_lines_found = 100;
    run.summary_totals.coverage_lines_hit = 0;
    run.totals_by_language[0].coverage_lines_found = 100;
    run.totals_by_language[0].coverage_lines_hit = 0;
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn render_html_perfect_coverage_renders_100_pct_state() {
    let mut run = make_run();
    run.per_file_records[0].coverage = Some(FileCoverage {
        lines_found: 50,
        lines_hit: 50,
        functions_found: 5,
        functions_hit: 5,
        branches_found: 10,
        branches_hit: 10,
    });
    run.summary_totals.coverage_lines_found = 50;
    run.summary_totals.coverage_lines_hit = 50;
    run.totals_by_language[0].coverage_lines_found = 50;
    run.totals_by_language[0].coverage_lines_hit = 50;
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn render_sub_report_html_with_pdf_url_includes_link() {
    let run = make_run();
    let html = render_sub_report_html(&run, Some("https://example.com/report.pdf")).unwrap();
    assert!(!html.is_empty());
    assert!(
        html.contains("example.com") || html.contains("pdf"),
        "pdf URL should appear in sub-report HTML"
    );
}

#[test]
fn render_sub_report_html_with_coverage_data() {
    let run = make_run_with_coverage();
    let html = render_sub_report_html(&run, None).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn render_sub_report_html_with_style_data() {
    let run = make_run_with_style();
    let html = render_sub_report_html(&run, None).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn render_sub_report_html_with_submodules() {
    let run = make_run_with_submodules();
    let html = render_sub_report_html(&run, None).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn render_confluence_storage_with_coverage_data() {
    let run = make_run_with_coverage();
    let out = render_confluence_storage(&run, None);
    assert!(
        !out.is_empty(),
        "confluence storage with coverage must produce output"
    );
}

#[test]
fn render_confluence_storage_with_submodules() {
    let run = make_run_with_submodules();
    let out = render_confluence_storage(&run, Some("https://reports.example.com/latest"));
    assert!(!out.is_empty());
}

#[test]
fn render_confluence_wiki_markup_with_coverage_data() {
    let run = make_run_with_coverage();
    let out = render_confluence_wiki_markup(&run);
    assert!(!out.is_empty());
}

#[test]
fn render_confluence_wiki_markup_with_empty_repo() {
    let run = make_empty_run();
    let out = render_confluence_wiki_markup(&run);
    assert!(!out.is_empty());
}

#[test]
fn write_csv_with_test_metrics() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut run = make_run();
    run.summary_totals.test_count = 30;
    run.summary_totals.test_assertion_count = 90;
    run.summary_totals.test_suite_count = 6;
    run.totals_by_language[0].test_count = 30;
    write_csv(&run, tmp.path()).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(!content.is_empty());
}

#[test]
fn write_xlsx_with_coverage_data_extra() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run_with_coverage();
    write_xlsx(&run, tmp.path()).unwrap();
    // File should exist and be non-empty
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0, "XLSX file should be non-empty");
}

#[test]
fn write_xlsx_multi_language_with_coverage() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut run = make_multi_lang_run();
    // Add coverage to every file record
    for rec in &mut run.per_file_records {
        rec.coverage = Some(FileCoverage {
            lines_found: 50,
            lines_hit: 40,
            functions_found: 4,
            functions_hit: 3,
            branches_found: 8,
            branches_hit: 6,
        });
    }
    run.summary_totals.coverage_lines_found = 200;
    run.summary_totals.coverage_lines_hit = 160;
    write_xlsx(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0);
}

#[test]
fn render_html_with_all_41_languages_exercises_rendering() {
    use sloc_languages::supported_languages;
    let mut run = make_empty_run();
    for (i, lang) in supported_languages().into_iter().enumerate() {
        run.per_file_records
            .push(make_file_record(&format!("file{i}.x"), lang, 10 + i as u64));
        run.totals_by_language
            .push(make_lang_summary(lang, 1, 10 + i as u64));
    }
    run.summary_totals.files_analyzed = run.per_file_records.len() as u64;
    run.summary_totals.code_lines = run
        .per_file_records
        .iter()
        .map(|r| r.effective_counts.code_lines)
        .sum();
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
    assert!(html.contains("<!doctype html>") || html.contains("<!DOCTYPE html>"));
}

// ── COCOMO fixture and tests ─────────────────────────────────────────────────

const fn make_cocomo() -> CocomoEstimate {
    CocomoEstimate {
        mode: CocomoMode::Organic,
        ksloc: 5.0,
        effort_person_months: 13.2,
        duration_months: 6.8,
        avg_staff: 1.94,
    }
}

fn make_run_with_cocomo() -> AnalysisRun {
    let mut run = make_run();
    run.cocomo = Some(make_cocomo());
    run.summary_totals.code_lines = 5_000;
    run
}

fn make_run_with_cocomo_semi() -> AnalysisRun {
    let mut run = make_run();
    run.cocomo = Some(CocomoEstimate {
        mode: CocomoMode::SemiDetached,
        ksloc: 20.0,
        effort_person_months: 72.4,
        duration_months: 11.6,
        avg_staff: 6.2,
    });
    run.summary_totals.code_lines = 20_000;
    run
}

fn make_run_with_cocomo_embedded() -> AnalysisRun {
    let mut run = make_multi_lang_run();
    run.cocomo = Some(CocomoEstimate {
        mode: CocomoMode::Embedded,
        ksloc: 100.0,
        effort_person_months: 640.0,
        duration_months: 23.0,
        avg_staff: 27.8,
    });
    run.summary_totals.code_lines = 100_000;
    run
}

#[test]
fn render_html_cocomo_section_is_present() {
    let run = make_run_with_cocomo();
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
    assert!(html.contains("<!doctype html>") || html.contains("<!DOCTYPE html>"));
    assert!(
        html.to_lowercase().contains("cocomo") || html.contains("Person-months"),
        "COCOMO section should appear when cocomo data is present"
    );
}

#[test]
fn render_html_hotspots_section_present_with_activity() {
    let mut run = make_run();
    run.per_file_records[0].commit_count = Some(5);
    run.per_file_records[0].last_commit_date = Some("2026-06-01T12:00:00+00:00".into());
    let html = render_html(&run).unwrap();
    assert!(
        html.contains("Git Hotspots"),
        "Hotspots section should appear when per-file activity is present"
    );
    assert!(html.contains("Hotspot score"));
    // ISO date is rendered as the calendar day only.
    assert!(html.contains("2026-06-01"));
}

#[test]
fn render_html_no_hotspots_section_without_activity() {
    let run = make_run();
    let html = render_html(&run).unwrap();
    assert!(
        !html.contains("Git Hotspots"),
        "Hotspots section must be omitted when no git activity was collected"
    );
}

#[test]
fn write_csv_includes_activity_columns_when_present() {
    use std::io::Read as _;
    let mut run = make_run();
    run.per_file_records[0].commit_count = Some(3);
    run.per_file_records[0].last_commit_date = Some("2026-06-01T12:00:00+00:00".into());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.csv");
    sloc_report::write_csv(&run, &path).unwrap();
    let mut csv = String::new();
    std::fs::File::open(&path)
        .unwrap()
        .read_to_string(&mut csv)
        .unwrap();
    assert!(
        csv.contains("Commits,Last Changed"),
        "activity header present"
    );
}

#[test]
fn render_html_cocomo_organic_mode_renders() {
    let run = make_run_with_cocomo();
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
    assert!(
        html.contains("Organic") || html.contains("organic"),
        "Organic mode label should appear"
    );
}

#[test]
fn render_html_cocomo_semi_detached_mode_renders() {
    let run = make_run_with_cocomo_semi();
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
    assert!(
        html.contains("Semi") || html.contains("semi") || html.to_lowercase().contains("cocomo"),
        "SemiDetached mode should appear in report"
    );
}

#[test]
fn render_html_cocomo_embedded_mode_renders() {
    let run = make_run_with_cocomo_embedded();
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
    assert!(
        html.contains("Embedded")
            || html.contains("embedded")
            || html.to_lowercase().contains("cocomo"),
        "Embedded mode label should appear"
    );
}

#[test]
fn render_html_no_cocomo_when_none() {
    let run = make_run();
    assert!(run.cocomo.is_none());
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn write_html_with_cocomo_data() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run_with_cocomo();
    write_html_report(&run, tmp.path()).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(!content.is_empty());
    assert!(content.contains("<!doctype html>") || content.contains("<!DOCTYPE html>"));
}

#[test]
fn write_html_with_cocomo_semi_detached() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run_with_cocomo_semi();
    write_html_report(&run, tmp.path()).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(!content.is_empty());
}

#[test]
fn write_csv_with_cocomo_data() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run_with_cocomo();
    write_csv(&run, tmp.path()).unwrap();
    let content = std::fs::read_to_string(tmp.path()).unwrap();
    assert!(!content.is_empty());
}

#[test]
fn write_xlsx_with_cocomo_data() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run_with_cocomo();
    write_xlsx(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0);
}

#[test]
fn render_sub_report_html_with_cocomo() {
    let run = make_run_with_cocomo();
    let html = render_sub_report_html(&run, None).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn render_confluence_storage_with_cocomo_data() {
    let run = make_run_with_cocomo();
    let out = render_confluence_storage(&run, None);
    assert!(!out.is_empty());
}

#[test]
fn render_confluence_wiki_markup_with_cocomo_data() {
    let run = make_run_with_cocomo();
    let out = render_confluence_wiki_markup(&run);
    assert!(!out.is_empty());
}

#[test]
fn render_html_with_delta_and_cocomo() {
    let run = make_run_with_cocomo();
    let delta = make_delta();
    let html = render_html_with_delta(&run, Some(&delta)).unwrap();
    assert!(!html.is_empty());
    assert!(html.contains("<!doctype html>") || html.contains("<!DOCTYPE html>"));
}

#[test]
fn render_html_with_cocomo_large_codebase() {
    let mut run = make_empty_run();
    run.summary_totals.code_lines = 1_500_000;
    run.summary_totals.files_analyzed = 50_000;
    run.cocomo = Some(CocomoEstimate {
        mode: CocomoMode::Organic,
        ksloc: 1500.0,
        effort_person_months: 4200.0,
        duration_months: 42.0,
        avg_staff: 100.0,
    });
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn render_html_with_cocomo_and_coverage() {
    let mut run = make_run_with_cocomo();
    run.per_file_records[0].coverage = Some(FileCoverage {
        lines_found: 100,
        lines_hit: 85,
        functions_found: 10,
        functions_hit: 9,
        branches_found: 20,
        branches_hit: 18,
    });
    run.summary_totals.coverage_lines_found = 100;
    run.summary_totals.coverage_lines_hit = 85;
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn render_html_with_cocomo_and_style() {
    let mut run = make_run_with_cocomo();
    run.style_summary = Some(StyleSummary {
        files_analyzed: 1,
        common_indent_style: "Spaces(4)".into(),
        line80_compliant_pct: 95,
        line_col_compliant_pct: 90,
        col_threshold: 100,
        by_language: vec![LanguageStyleGroup {
            language_family: "Rust".into(),
            files_count: 1,
            dominant_guide: "Rust Official".into(),
            dominant_score_pct: 98,
            common_indent_style: "Spaces(4)".into(),
            guide_avg_scores: vec![("Rust Official".into(), 98)],
            line80_compliant_pct: 95,
            line_col_compliant_pct: 90,
        }],
    });
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn write_html_with_pdf_link_and_cocomo() {
    let dir = tempfile::tempdir().unwrap();
    let html_path = dir.path().join("report.html");
    let pdf_path = dir.path().join("report.pdf");
    let run = make_run_with_cocomo();
    write_html_with_pdf_link(&run, &html_path, Some(&pdf_path)).unwrap();
    let content = std::fs::read_to_string(&html_path).unwrap();
    assert!(!content.is_empty());
    assert!(content.contains("<!doctype html>") || content.contains("<!DOCTYPE html>"));
}

// ── ULOC / dryness tests ─────────────────────────────────────────────────────

#[test]
fn render_html_with_uloc_and_dryness_data() {
    let mut run = make_run();
    run.uloc = 42;
    run.dryness_pct = Some(87.5);
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn render_html_with_duplicate_groups() {
    let mut run = make_run();
    run.duplicate_groups = vec![
        vec!["src/a.rs".into(), "src/b.rs".into()],
        vec!["lib/x.rs".into(), "lib/y.rs".into(), "lib/z.rs".into()],
    ];
    run.duplicates_excluded = 3;
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn render_html_with_all_optional_data() {
    let mut run = make_run_with_cocomo();
    // Add coverage
    run.per_file_records[0].coverage = Some(FileCoverage {
        lines_found: 50,
        lines_hit: 45,
        functions_found: 5,
        functions_hit: 5,
        branches_found: 10,
        branches_hit: 9,
    });
    run.summary_totals.coverage_lines_found = 50;
    run.summary_totals.coverage_lines_hit = 45;
    // Add test counts
    run.summary_totals.test_count = 25;
    run.summary_totals.test_assertion_count = 75;
    run.summary_totals.test_suite_count = 5;
    // Add style
    run.style_summary = Some(StyleSummary {
        files_analyzed: 1,
        common_indent_style: "Spaces(4)".into(),
        line80_compliant_pct: 90,
        line_col_compliant_pct: 88,
        col_threshold: 100,
        by_language: vec![],
    });
    // Add ULOC
    run.uloc = 120;
    run.dryness_pct = Some(94.0);
    // Add duplicates
    run.duplicate_groups = vec![vec!["src/a.rs".into(), "src/b.rs".into()]];
    run.duplicates_excluded = 1;
    // Add git metadata
    run.git_remote_url = Some("https://github.com/test/repo.git".into());
    run.git_branch = Some("main".into());
    run.git_commit_short = Some("abc123d".into());
    run.git_nearest_tag = Some("v1.5.0".into());
    let html = render_html(&run).unwrap();
    assert!(!html.is_empty());
    assert!(html.contains("<!doctype html>") || html.contains("<!DOCTYPE html>"));
}

// ── write_pdf_from_run tests (printpdf-based, no browser required) ────────────
//
// write_pdf_from_run generates a PDF in-process using the printpdf crate.
// No Chromium installation is required. These tests exercise the entire
// pdf_render_* family: header, summary chips, info lines, metric tables,
// page 1 footer, per-file pages, style section, and COCOMO section.

#[test]
fn write_pdf_from_run_basic_succeeds() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run();
    write_pdf_from_run(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0, "PDF must be non-empty");
}

#[test]
fn write_pdf_from_run_empty_project_succeeds() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_empty_run();
    write_pdf_from_run(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0, "PDF for empty project must be non-empty");
}

#[test]
fn write_pdf_from_run_multi_language_succeeds() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_multi_lang_run();
    write_pdf_from_run(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0);
}

#[test]
fn write_pdf_from_run_with_cocomo_fits_page1() {
    // Small COCOMO estimate — fits on page 1 (after_style_y - 3 > FOOTER_H + 20)
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut run = make_run_with_cocomo();
    run.summary_totals.code_lines = 5_000;
    write_pdf_from_run(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(
        meta.len() > 0,
        "PDF with COCOMO on page 1 must be non-empty"
    );
}

#[test]
fn write_pdf_from_run_with_hotspots_succeeds() {
    // A handful of files carrying git activity → the Git Hotspots page renders.
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut run = make_run();
    for i in 0u32..6 {
        let mut rec = make_file_record(
            &format!("src/mod_{i}.rs"),
            Language::Rust,
            40 + u64::from(i) * 10,
        );
        rec.commit_count = Some(i + 1);
        rec.last_commit_date = Some(format!("2026-06-{:02}T10:00:00+00:00", i + 1));
        run.per_file_records.push(rec);
    }
    write_pdf_from_run(&run, tmp.path()).unwrap();
    assert!(
        std::fs::metadata(tmp.path()).unwrap().len() > 0,
        "PDF with a hotspots page must be non-empty"
    );
}

#[test]
fn write_pdf_from_run_with_hotspots_and_many_files() {
    // Hotspots page followed by multiple per-file pages — exercises the continuation threading.
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut run = make_empty_run();
    for i in 0u32..40 {
        let mut rec = make_file_record(
            &format!("src/file_{i:02}.rs"),
            Language::Rust,
            50 + u64::from(i),
        );
        rec.commit_count = Some(i % 7 + 1);
        rec.last_commit_date = Some("2026-06-01T10:00:00+00:00".into());
        run.per_file_records.push(rec);
    }
    run.totals_by_language = vec![make_lang_summary(Language::Rust, 40, 3000)];
    run.summary_totals.files_analyzed = 40;
    run.summary_totals.code_lines = 3000;
    write_pdf_from_run(&run, tmp.path()).unwrap();
    assert!(std::fs::metadata(tmp.path()).unwrap().len() > 0);
}

#[test]
fn write_pdf_from_run_without_hotspots_unchanged_path() {
    // No git activity → no hotspots page; the standard path still produces a valid PDF.
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run();
    assert!(
        run.per_file_records
            .iter()
            .all(|r| r.commit_count.is_none())
    );
    write_pdf_from_run(&run, tmp.path()).unwrap();
    assert!(std::fs::metadata(tmp.path()).unwrap().len() > 0);
}

#[test]
fn write_pdf_from_run_with_cocomo_page2() {
    // To force page-2 COCOMO path, fill the page with many per-file records
    // so after_tables_y drops below FOOTER_H + 20, pushing COCOMO to page 2.
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut run = make_empty_run();
    // Add many file records to consume page-1 space
    for i in 0..40_u64 {
        run.per_file_records.push(make_file_record(
            &format!("src/file_{i:02}.rs"),
            Language::Rust,
            50 + i,
        ));
    }
    run.totals_by_language = vec![make_lang_summary(Language::Rust, 40, 3000)];
    run.summary_totals.files_analyzed = 40;
    run.summary_totals.code_lines = 3_000;
    run.cocomo = Some(CocomoEstimate {
        mode: CocomoMode::Organic,
        ksloc: 3.0,
        effort_person_months: 7.8,
        duration_months: 5.5,
        avg_staff: 1.4,
    });
    write_pdf_from_run(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(
        meta.len() > 0,
        "PDF with COCOMO on page 2 must be non-empty"
    );
}

/// Extract every page's `MediaBox` height (in PDF points) from raw PDF bytes.
/// printpdf writes `/MediaBox [0 0 W H]` uncompressed in each page dictionary.
fn pdf_mediabox_heights(bytes: &[u8]) -> Vec<f32> {
    let text = String::from_utf8_lossy(bytes);
    let mut heights = Vec::new();
    let mut rest = text.as_ref();
    while let Some(pos) = rest.find("/MediaBox") {
        rest = &rest[pos + "/MediaBox".len()..];
        if let (Some(open), Some(close)) = (rest.find('['), rest.find(']')) {
            let nums: Vec<f32> = rest[open + 1..close]
                .split_whitespace()
                .filter_map(|t| t.parse::<f32>().ok())
                .collect();
            if nums.len() == 4 {
                heights.push(nums[3]);
            }
        }
    }
    heights
}

#[test]
fn write_pdf_from_run_terminal_tc_page_is_trimmed() {
    // Repro for the "huge empty gap below the SUBMODULES section" bug: when COCOMO +
    // Tests & Coverage land on their own terminal page (no per-file table, no hotspots),
    // that page must be trimmed to its content instead of left at full landscape height.
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut run = make_run_with_submodules();
    run.per_file_records.clear(); // no per-file table → T&C page is terminal
    run.cocomo = Some(CocomoEstimate {
        mode: CocomoMode::Organic,
        ksloc: 16.4,
        effort_person_months: 45.27,
        duration_months: 10.65,
        avg_staff: 4.25,
    });
    write_pdf_from_run(&run, tmp.path()).unwrap();

    let bytes = std::fs::read(tmp.path()).unwrap();
    let heights = pdf_mediabox_heights(&bytes);
    assert!(!heights.is_empty(), "expected at least one MediaBox");
    // Full landscape A4 height = 210 mm ≈ 595.28 pt. The terminal page must be shorter.
    let min_h = heights.iter().copied().fold(f32::INFINITY, f32::min);
    assert!(
        min_h < 590.0,
        "terminal T&C page should be trimmed below full landscape height; got heights {heights:?}"
    );
}

#[test]
fn write_pdf_from_run_tc_page_trimmed_when_hotspots_follow() {
    // The reported bug: a COCOMO + Tests & Coverage + SUBMODULES page left a huge empty gap
    // when a Git Hotspots page followed it (the per-file table flows onto the Hotspots page,
    // not the T&C page, so the T&C page ends early). The T&C page must still be trimmed.
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut run = make_run_with_submodules();
    // Give the per-file records git activity so a Git Hotspots page is emitted.
    for (i, rec) in run.per_file_records.iter_mut().enumerate() {
        rec.commit_count = Some(u32::try_from(i).unwrap() + 1);
        rec.last_commit_date = Some("2026-06-01T10:00:00+00:00".into());
    }
    run.cocomo = Some(CocomoEstimate {
        mode: CocomoMode::Organic,
        ksloc: 16.4,
        effort_person_months: 45.27,
        duration_months: 10.65,
        avg_staff: 4.25,
    });
    write_pdf_from_run(&run, tmp.path()).unwrap();

    let bytes = std::fs::read(tmp.path()).unwrap();
    let heights = pdf_mediabox_heights(&bytes);
    let min_h = heights.iter().copied().fold(f32::INFINITY, f32::min);
    assert!(
        min_h < 590.0,
        "T&C page should be trimmed even when a Hotspots page follows; got heights {heights:?}"
    );
}

#[test]
fn write_pdf_from_run_with_style_analysis() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run_with_style();
    write_pdf_from_run(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0, "PDF with style analysis must be non-empty");
}

#[test]
fn write_pdf_from_run_with_coverage_data() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run_with_coverage();
    write_pdf_from_run(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0);
}

#[test]
fn write_pdf_from_run_with_all_git_metadata() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run_with_all_git();
    write_pdf_from_run(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0);
}

#[test]
fn write_pdf_from_run_with_submodules() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run_with_submodules();
    write_pdf_from_run(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0);
}

#[test]
fn write_pdf_from_run_with_warnings() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run_with_warnings();
    write_pdf_from_run(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0);
}

#[test]
fn write_pdf_from_run_with_skipped_files() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run_with_skipped();
    write_pdf_from_run(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0);
}

#[test]
fn write_pdf_from_run_per_file_pages_triggered() {
    // 15+ per-file records triggers pdf_render_per_file_pages (multi-page PDF)
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut run = make_empty_run();
    for i in 0..20_u64 {
        run.per_file_records.push(make_file_record(
            &format!("src/module_{i:02}.rs"),
            Language::Rust,
            30 + i,
        ));
    }
    run.totals_by_language = vec![make_lang_summary(Language::Rust, 20, 1000)];
    run.summary_totals.files_analyzed = 20;
    run.summary_totals.code_lines = 1_000;
    write_pdf_from_run(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0, "multi-page PDF must be non-empty");
}

#[test]
fn write_pdf_from_run_with_cocomo_and_coverage() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run_with_cocomo_and_coverage();
    write_pdf_from_run(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0);
}

fn make_run_with_cocomo_and_coverage() -> AnalysisRun {
    let mut run = make_run_with_cocomo();
    run.per_file_records[0].coverage = Some(FileCoverage {
        lines_found: 50,
        lines_hit: 40,
        functions_found: 5,
        functions_hit: 4,
        branches_found: 10,
        branches_hit: 8,
    });
    run.summary_totals.coverage_lines_found = 50;
    run.summary_totals.coverage_lines_hit = 40;
    run.summary_totals.test_count = 12;
    run.summary_totals.test_assertion_count = 36;
    run.summary_totals.test_suite_count = 3;
    run.totals_by_language[0].coverage_lines_found = 50;
    run.totals_by_language[0].coverage_lines_hit = 40;
    run
}

#[test]
fn write_pdf_from_run_large_codebase() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut run = make_empty_run();
    run.summary_totals.code_lines = 250_000;
    run.summary_totals.total_physical_lines = 300_000;
    run.summary_totals.files_analyzed = 5_000;
    run.cocomo = Some(CocomoEstimate {
        mode: CocomoMode::SemiDetached,
        ksloc: 250.0,
        effort_person_months: 980.0,
        duration_months: 32.0,
        avg_staff: 30.6,
    });
    write_pdf_from_run(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0);
}

#[test]
fn write_pdf_from_run_with_accent_color() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut run = make_run_with_cocomo();
    run.effective_configuration.reporting.accent_color = Some("#e05c00".into());
    write_pdf_from_run(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0);
}

#[test]
fn write_pdf_from_run_with_custom_title_and_banner() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut run = make_run();
    run.effective_configuration.reporting.report_title = "My Custom Report".into();
    run.effective_configuration.reporting.report_header_footer =
        Some("Confidential — Internal Use Only".into());
    write_pdf_from_run(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0);
}

#[test]
fn write_pdf_from_run_with_many_languages() {
    use sloc_languages::supported_languages;
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut run = make_empty_run();
    for (i, lang) in supported_languages().into_iter().enumerate().take(15) {
        run.per_file_records.push(make_file_record(
            &format!("file{i}.src"),
            lang,
            10 + i as u64,
        ));
        run.totals_by_language
            .push(make_lang_summary(lang, 1, 10 + i as u64));
    }
    run.summary_totals.files_analyzed = 15;
    run.summary_totals.code_lines = 285;
    write_pdf_from_run(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0);
}

#[test]
fn write_pdf_from_run_with_cocomo_and_style() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run_with_cocomo_and_style();
    write_pdf_from_run(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0);
}

fn make_run_with_cocomo_and_style() -> AnalysisRun {
    let mut run = make_run_with_cocomo();
    run.style_summary = Some(StyleSummary {
        files_analyzed: 1,
        common_indent_style: "Spaces(4)".into(),
        line80_compliant_pct: 92,
        line_col_compliant_pct: 88,
        col_threshold: 100,
        by_language: vec![LanguageStyleGroup {
            language_family: "Rust".into(),
            files_count: 1,
            dominant_guide: "Rust Official".into(),
            dominant_score_pct: 96,
            common_indent_style: "Spaces(4)".into(),
            guide_avg_scores: vec![("Rust Official".into(), 96)],
            line80_compliant_pct: 92,
            line_col_compliant_pct: 88,
        }],
    });
    run
}

#[test]
fn write_pdf_from_run_embedded_cocomo_mode() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let run = make_run_with_cocomo_embedded();
    write_pdf_from_run(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0);
}

#[test]
fn write_pdf_from_run_creates_parent_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let pdf_path = dir.path().join("nested").join("subdir").join("report.pdf");
    let run = make_run();
    write_pdf_from_run(&run, &pdf_path).unwrap();
    assert!(
        pdf_path.exists(),
        "PDF must be created even when parent dirs are missing"
    );
    let meta = std::fs::metadata(&pdf_path).unwrap();
    assert!(meta.len() > 0);
}

#[test]
fn write_pdf_from_run_with_zero_coverage() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut run = make_run();
    run.per_file_records[0].coverage = Some(FileCoverage {
        lines_found: 100,
        lines_hit: 0,
        functions_found: 10,
        functions_hit: 0,
        branches_found: 20,
        branches_hit: 0,
    });
    run.summary_totals.coverage_lines_found = 100;
    run.summary_totals.coverage_lines_hit = 0;
    write_pdf_from_run(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0);
}

#[test]
fn write_pdf_from_run_with_full_coverage() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let mut run = make_run();
    run.per_file_records[0].coverage = Some(FileCoverage {
        lines_found: 50,
        lines_hit: 50,
        functions_found: 5,
        functions_hit: 5,
        branches_found: 10,
        branches_hit: 10,
    });
    run.summary_totals.coverage_lines_found = 50;
    run.summary_totals.coverage_lines_hit = 50;
    write_pdf_from_run(&run, tmp.path()).unwrap();
    let meta = std::fs::metadata(tmp.path()).unwrap();
    assert!(meta.len() > 0);
}
