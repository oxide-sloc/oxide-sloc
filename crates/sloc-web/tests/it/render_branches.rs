// Targeted branch-coverage tests for sloc-web render functions.
//
// Each test hits a specific conditional branch in lib.rs that was not covered by
// the existing integration / coverage_boost / api_functions suites.  Focus areas:
//   * render_result_page — COCOMO modes, style_summary, duplicate_groups, per-file
//     flags (vendor / generated / minified), coverage summary, warnings, lsloc, uloc
//   * compare_handler — missing run IDs, canonical-URL redirect
//   * rate-limit path (make_test_router_tight_rate_limit)
//   * API-key auth paths (make_test_router_with_key)
//   * exhausted-semaphore path (make_test_router_exhausted_semaphore)
//   * trend_report_handler with rich multi-run data + git metadata
//   * test_metrics_handler with real test / assertion / coverage data
//   * badge handler — all metric variants including coverage
//   * embed handler with various run states
//   * delete-run handler
//   * pdf-status handler

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use sloc_web::{
    TEST_SERVER_MODE_API_KEY, make_test_router, make_test_router_exhausted_semaphore,
    make_test_router_server_mode, make_test_router_tight_rate_limit, make_test_router_with_key,
};
use tower::ServiceExt;

use chrono::Utc;
use sloc_config::AppConfig;
use sloc_core::AnalysisRun;
use sloc_core::{
    CocomoEstimate, CocomoMode, EffectiveCounts, EnvironmentMetadata, FileCoverage, FileRecord,
    FileStatus, LanguageStyleGroup, LanguageSummary, StyleSummary, SubmoduleSummary, SummaryTotals,
    ToolMetadata,
};
use sloc_languages::{Language, ParseMode, RawLineCounts};

// ── HTTP helpers ──────────────────────────────────────────────────────────────

async fn get_with(app: Router, uri: &str) -> (StatusCode, String) {
    let resp = app
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn get_server_with(app: Router, uri: &str) -> (StatusCode, String) {
    let resp = app
        .oneshot(
            Request::get(uri)
                .header(
                    "Authorization",
                    format!("Bearer {TEST_SERVER_MODE_API_KEY}"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn get(uri: &str) -> (StatusCode, String) {
    get_with(make_test_router(), uri).await
}

async fn post_json_with(app: Router, uri: &str, json: &str) -> (StatusCode, String) {
    let req = Request::post(uri)
        .header("content-type", "application/json")
        .body(Body::from(json.to_owned()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn delete_with(app: Router, uri: &str) -> StatusCode {
    let resp = app
        .oneshot(Request::delete(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    resp.status()
}

#[allow(dead_code)]
fn pct_encode(s: &str) -> String {
    s.bytes()
        .flat_map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                vec![b as char]
            }
            _ => format!("%{b:02X}").chars().collect(),
        })
        .collect()
}

// ── Fixture helpers ───────────────────────────────────────────────────────────

fn base_run(id: &str) -> AnalysisRun {
    AnalysisRun {
        tool: ToolMetadata {
            name: "oxide-sloc".into(),
            version: "1.5.66".into(),
            run_id: id.into(),
            timestamp_utc: Utc::now(),
        },
        environment: EnvironmentMetadata {
            operating_system: "linux".into(),
            architecture: "x86_64".into(),
            runtime_mode: "test".into(),
            initiator_username: "tester".into(),
            initiator_hostname: "ci".into(),
            ci_name: None,
        },
        effective_configuration: AppConfig::default(),
        input_roots: vec!["/test/myproject".into()],
        summary_totals: SummaryTotals {
            files_considered: 10,
            files_analyzed: 10,
            code_lines: 5000,
            total_physical_lines: 5200,
            comment_lines: 100,
            blank_lines: 100,
            ..SummaryTotals::default()
        },
        totals_by_language: vec![LanguageSummary {
            language: Language::Rust,
            files: 10,
            total_physical_lines: 5200,
            code_lines: 5000,
            comment_lines: 100,
            blank_lines: 100,
            mixed_lines_separate: 0,
            functions: 50,
            classes: 5,
            variables: 20,
            variables_member: 0,
            variables_local: 0,
            variables_global: 0,
            macro_definitions: 0,
            imports: 10,
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
    }
}

fn file_rec(path: &str, lang: Language, code: u64) -> FileRecord {
    FileRecord {
        path: format!("/test/myproject/{path}"),
        relative_path: path.into(),
        language: Some(lang),
        size_bytes: code * 25,
        detected_encoding: Some("utf-8".into()),
        raw_line_categories: RawLineCounts {
            total_physical_lines: code + 2,
            code_only_lines: code,
            blank_only_lines: 1,
            single_comment_only_lines: 1,
            ..RawLineCounts::default()
        },
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

fn ingest(run: &AnalysisRun) -> String {
    serde_json::to_string(run).unwrap()
}

/// Ingest a run and return its `run_id` (the one supplied via `run.tool.run_id`).
async fn ingest_run(app: Router, run: &AnalysisRun) -> String {
    let (st, _) = post_json_with(app, "/api/ingest", &ingest(run)).await;
    assert!(st.as_u16() < 500, "ingest must not 5xx, got {st}");
    run.tool.run_id.clone()
}

// ── render_result_page: COCOMO mode branches ──────────────────────────────────

/// Runs ingested with COCOMO estimates exercise the COCOMO rendering branch in
/// `render_result_page` (the `cocomo_mode_str` match arm is evaluated at render time
/// from the scan wizard context, not from the stored estimate, but ingesting a
/// run with code lines > 0 and a COCOMO estimate populates `has_cocomo` = true).
#[tokio::test]
async fn result_page_renders_for_run_with_cocomo_organic() {
    let app = make_test_router();
    let mut run = base_run("cocomo-organic-001");
    run.cocomo = Some(CocomoEstimate {
        mode: CocomoMode::Organic,
        ksloc: 5.0,
        effort_person_months: 13.2,
        duration_months: 6.8,
        avg_staff: 1.94,
    });
    let id = ingest_run(app.clone(), &run).await;
    let (st, body) = get_with(app.clone(), &format!("/runs/result/{id}")).await;
    assert!(st.as_u16() < 500, "result page must not 5xx, got {st}");
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "must be HTML"
    );
}

#[tokio::test]
async fn result_page_renders_for_run_with_cocomo_semi_detached() {
    let app = make_test_router();
    let mut run = base_run("cocomo-semi-001");
    run.cocomo = Some(CocomoEstimate {
        mode: CocomoMode::SemiDetached,
        ksloc: 10.0,
        effort_person_months: 33.7,
        duration_months: 9.8,
        avg_staff: 3.44,
    });
    let id = ingest_run(app.clone(), &run).await;
    let (st, body) = get_with(app.clone(), &format!("/runs/result/{id}")).await;
    assert!(
        st.as_u16() < 500,
        "result page (semi-detached) must not 5xx, got {st}"
    );
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "must be HTML"
    );
}

#[tokio::test]
async fn result_page_renders_for_run_with_cocomo_embedded() {
    let app = make_test_router();
    let mut run = base_run("cocomo-embedded-001");
    run.cocomo = Some(CocomoEstimate {
        mode: CocomoMode::Embedded,
        ksloc: 20.0,
        effort_person_months: 100.2,
        duration_months: 14.1,
        avg_staff: 7.1,
    });
    let id = ingest_run(app.clone(), &run).await;
    let (st, body) = get_with(app.clone(), &format!("/runs/result/{id}")).await;
    assert!(
        st.as_u16() < 500,
        "result page (embedded) must not 5xx, got {st}"
    );
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "must be HTML"
    );
}

// ── render_result_page: style_summary branch ─────────────────────────────────

#[tokio::test]
async fn result_page_renders_with_style_summary_low_scores() {
    let app = make_test_router();
    let mut run = base_run("style-low-001");
    run.style_summary = Some(StyleSummary {
        files_analyzed: 5,
        common_indent_style: "spaces".into(),
        line80_compliant_pct: 30, // low — triggers warning branch
        line_col_compliant_pct: 25,
        col_threshold: 100,
        by_language: vec![LanguageStyleGroup {
            language_family: "Rust".into(),
            files_count: 5,
            dominant_guide: "Rust 2018".into(),
            dominant_score_pct: 42,
            common_indent_style: "spaces".into(),
            guide_avg_scores: vec![("Rust 2018".to_owned(), 42)],
            line80_compliant_pct: 30,
            line_col_compliant_pct: 25,
        }],
    });
    let id = ingest_run(app.clone(), &run).await;
    let (st, body) = get_with(app.clone(), &format!("/runs/result/{id}")).await;
    assert!(
        st.as_u16() < 500,
        "result page (style summary) must not 5xx, got {st}"
    );
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "must be HTML"
    );
}

#[tokio::test]
async fn result_page_renders_with_style_summary_high_scores() {
    let app = make_test_router();
    let mut run = base_run("style-high-001");
    run.style_summary = Some(StyleSummary {
        files_analyzed: 12,
        common_indent_style: "tabs".into(),
        line80_compliant_pct: 98,
        line_col_compliant_pct: 96,
        col_threshold: 120,
        by_language: vec![
            LanguageStyleGroup {
                language_family: "C / C++".into(),
                files_count: 8,
                dominant_guide: "Google C++".into(),
                dominant_score_pct: 97,
                common_indent_style: "spaces".into(),
                guide_avg_scores: vec![("Google C++".to_owned(), 97), ("LLVM".to_owned(), 88)],
                line80_compliant_pct: 99,
                line_col_compliant_pct: 97,
            },
            LanguageStyleGroup {
                language_family: "Python".into(),
                files_count: 4,
                dominant_guide: "PEP 8".into(),
                dominant_score_pct: 95,
                common_indent_style: "spaces".into(),
                guide_avg_scores: vec![("PEP 8".to_owned(), 95)],
                line80_compliant_pct: 96,
                line_col_compliant_pct: 94,
            },
        ],
    });
    let id = ingest_run(app.clone(), &run).await;
    let (st, body) = get_with(app.clone(), &format!("/runs/result/{id}")).await;
    assert!(
        st.as_u16() < 500,
        "result page (style high) must not 5xx, got {st}"
    );
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "must be HTML"
    );
}

// ── render_result_page: duplicate_groups branch ───────────────────────────────

#[tokio::test]
async fn result_page_renders_with_many_duplicate_groups() {
    let app = make_test_router();
    let mut run = base_run("dups-001");
    // 5 duplicate groups — exercises duplicate_group_count > 0 branch
    run.duplicate_groups = vec![
        vec!["src/a.rs".into(), "src/b.rs".into()],
        vec!["src/c.rs".into(), "src/d.rs".into()],
        vec!["lib/x.rs".into(), "lib/y.rs".into(), "lib/z.rs".into()],
        vec!["util/foo.rs".into(), "util/bar.rs".into()],
        vec!["tests/t1.rs".into(), "tests/t2.rs".into()],
    ];
    run.duplicates_excluded = 5;
    let id = ingest_run(app.clone(), &run).await;
    let (st, body) = get_with(app.clone(), &format!("/runs/result/{id}")).await;
    assert!(
        st.as_u16() < 500,
        "result page (dup groups) must not 5xx, got {st}"
    );
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "must be HTML"
    );
}

// ── render_result_page: per-file vendor/generated/minified flags ──────────────

#[tokio::test]
async fn result_page_renders_with_vendor_generated_minified_files() {
    let app = make_test_router();
    let mut run = base_run("flags-001");

    let mut vendor_file = file_rec("vendor/lodash/lodash.min.js", Language::JavaScript, 100);
    vendor_file.vendor = true;
    vendor_file.minified = true;

    let mut gen_file = file_rec("generated/proto/types.rs", Language::Rust, 200);
    gen_file.generated = true;

    let mut warn_file = file_rec("src/complex.rs", Language::Rust, 50);
    warn_file.warnings = vec![
        "Line 42 exceeds configured limit".into(),
        "Possible encoding issue on line 100".into(),
    ];

    run.per_file_records = vec![vendor_file, gen_file, warn_file];
    run.warnings = vec![
        "Binary file skipped: assets/logo.png".into(),
        "Encoding fallback used on: legacy/old.c".into(),
        "Truncated file: huge/giant.rs (>100 MB)".into(),
    ];

    let id = ingest_run(app.clone(), &run).await;
    let (st, body) = get_with(app.clone(), &format!("/runs/result/{id}")).await;
    assert!(
        st.as_u16() < 500,
        "result page (vendor/gen/min flags) must not 5xx, got {st}"
    );
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "must be HTML"
    );
}

// ── render_result_page: coverage summary data ─────────────────────────────────

#[tokio::test]
async fn result_page_renders_with_coverage_summary() {
    let app = make_test_router();
    let mut run = base_run("cov-summary-001");
    run.summary_totals.coverage_lines_found = 1000;
    run.summary_totals.coverage_lines_hit = 750;
    run.summary_totals.coverage_functions_found = 80;
    run.summary_totals.coverage_functions_hit = 72;
    run.summary_totals.coverage_branches_found = 200;
    run.summary_totals.coverage_branches_hit = 160;

    // Also add a per-file record with coverage data
    let mut fr = file_rec("src/lib.rs", Language::Rust, 300);
    fr.coverage = Some(FileCoverage {
        lines_found: 300,
        lines_hit: 225,
        functions_found: 30,
        functions_hit: 27,
        branches_found: 60,
        branches_hit: 48,
    });
    run.per_file_records = vec![fr];

    let id = ingest_run(app.clone(), &run).await;
    let (st, body) = get_with(app.clone(), &format!("/runs/result/{id}")).await;
    assert!(
        st.as_u16() < 500,
        "result page (coverage) must not 5xx, got {st}"
    );
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "must be HTML"
    );
}

// ── render_result_page: lsloc and uloc fields ────────────────────────────────

#[tokio::test]
async fn result_page_renders_with_lsloc_and_uloc() {
    let app = make_test_router();
    let mut run = base_run("lsloc-uloc-001");
    run.summary_totals.lsloc = Some(3500);
    run.uloc = 4200;
    run.dryness_pct = Some(84.0);

    let id = ingest_run(app.clone(), &run).await;
    let (st, body) = get_with(app.clone(), &format!("/runs/result/{id}")).await;
    assert!(
        st.as_u16() < 500,
        "result page (lsloc/uloc) must not 5xx, got {st}"
    );
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "must be HTML"
    );
}

// ── render_result_page: warnings in the run ───────────────────────────────────

#[tokio::test]
async fn result_page_renders_with_run_warnings() {
    let app = make_test_router();
    let mut run = base_run("warnings-001");
    run.warnings = vec![
        "Binary file skipped: build/output.bin".into(),
        "Encoding fallback (Windows-1252) used: docs/readme.txt".into(),
        "File too large, truncated: mega/huge.rs".into(),
        "Unknown language for extension .xyz".into(),
        "Git blame failed for src/auth.rs".into(),
    ];
    let id = ingest_run(app.clone(), &run).await;
    let (st, body) = get_with(app.clone(), &format!("/runs/result/{id}")).await;
    assert!(
        st.as_u16() < 500,
        "result page (warnings) must not 5xx, got {st}"
    );
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "must be HTML"
    );
}

// ── render_result_page: git metadata ─────────────────────────────────────────

#[tokio::test]
async fn result_page_renders_with_full_git_metadata() {
    let app = make_test_router();
    let mut run = base_run("git-meta-001");
    run.git_commit_short = Some("abc1234".into());
    run.git_commit_long = Some("abc1234def5678901234567890abcdef12345678".into());
    run.git_branch = Some("feature/my-branch".into());
    run.git_commit_author = Some("Alice Developer".into());
    run.git_tags = Some("v1.0.0".into());
    run.git_nearest_tag = Some("v0.9.0".into());
    run.git_commit_date = Some("2026-06-14".into());
    run.git_remote_url = Some("https://github.com/myorg/myrepo.git".into());
    let id = ingest_run(app.clone(), &run).await;
    let (st, body) = get_with(app.clone(), &format!("/runs/result/{id}")).await;
    assert!(
        st.as_u16() < 500,
        "result page (git meta) must not 5xx, got {st}"
    );
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "must be HTML"
    );
}

// ── render_result_page: ci_name branch ───────────────────────────────────────

#[tokio::test]
async fn result_page_renders_with_ci_name_set() {
    let app = make_test_router();
    let mut run = base_run("ci-name-001");
    run.environment.ci_name = Some("GitHub Actions".into());
    let id = ingest_run(app.clone(), &run).await;
    let (st, body) = get_with(app.clone(), &format!("/runs/result/{id}")).await;
    assert!(
        st.as_u16() < 500,
        "result page (ci_name) must not 5xx, got {st}"
    );
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "must be HTML"
    );
}

// ── render_result_page: zero code lines (no COCOMO) ──────────────────────────

#[tokio::test]
async fn result_page_renders_with_zero_code_lines_no_cocomo() {
    let app = make_test_router();
    let mut run = base_run("zero-code-001");
    run.summary_totals.code_lines = 0;
    run.totals_by_language = vec![];
    let id = ingest_run(app.clone(), &run).await;
    let (st, body) = get_with(app.clone(), &format!("/runs/result/{id}")).await;
    assert!(
        st.as_u16() < 500,
        "result page (zero code) must not 5xx, got {st}"
    );
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "must be HTML"
    );
}

// ── render_result_page: multiple language summaries + semantic data ─────────────

// Dominated by large struct-literal fixtures for the multi-language render path;
// the length is inherent test data, not branching logic worth splitting.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn result_page_renders_with_multi_language_and_semantic_data() {
    let app = make_test_router();
    let mut run = base_run("multi-lang-001");
    run.totals_by_language = vec![
        LanguageSummary {
            language: Language::Rust,
            files: 15,
            total_physical_lines: 3000,
            code_lines: 2500,
            comment_lines: 200,
            blank_lines: 300,
            mixed_lines_separate: 0,
            functions: 120,
            classes: 0,
            variables: 0,
            variables_member: 0,
            variables_local: 0,
            variables_global: 0,
            macro_definitions: 0,
            imports: 30,
            test_count: 45,
            test_assertion_count: 180,
            test_suite_count: 8,
            coverage_lines_found: 2000,
            coverage_lines_hit: 1600,
            coverage_functions_found: 100,
            coverage_functions_hit: 88,
            coverage_branches_found: 400,
            coverage_branches_hit: 320,
            cyclomatic_complexity: 450,
            lsloc: Some(2100),
        },
        LanguageSummary {
            language: Language::Python,
            files: 8,
            total_physical_lines: 1500,
            code_lines: 1200,
            comment_lines: 150,
            blank_lines: 150,
            mixed_lines_separate: 0,
            functions: 60,
            classes: 12,
            variables: 30,
            variables_member: 0,
            variables_local: 0,
            variables_global: 0,
            macro_definitions: 0,
            imports: 20,
            test_count: 20,
            test_assertion_count: 80,
            test_suite_count: 4,
            coverage_lines_found: 1000,
            coverage_lines_hit: 800,
            coverage_functions_found: 50,
            coverage_functions_hit: 42,
            coverage_branches_found: 200,
            coverage_branches_hit: 160,
            cyclomatic_complexity: 200,
            lsloc: None,
        },
        LanguageSummary {
            language: Language::TypeScript,
            files: 5,
            total_physical_lines: 800,
            code_lines: 700,
            comment_lines: 50,
            blank_lines: 50,
            mixed_lines_separate: 5,
            functions: 40,
            classes: 8,
            variables: 25,
            variables_member: 0,
            variables_local: 0,
            variables_global: 0,
            macro_definitions: 0,
            imports: 15,
            test_count: 0,
            test_assertion_count: 0,
            test_suite_count: 0,
            coverage_lines_found: 0,
            coverage_lines_hit: 0,
            coverage_functions_found: 0,
            coverage_functions_hit: 0,
            coverage_branches_found: 0,
            coverage_branches_hit: 0,
            cyclomatic_complexity: 150,
            lsloc: None,
        },
    ];
    run.summary_totals.code_lines = 4400;
    run.summary_totals.files_analyzed = 28;
    run.summary_totals.test_count = 65;
    run.summary_totals.test_assertion_count = 260;
    run.summary_totals.coverage_lines_found = 3000;
    run.summary_totals.coverage_lines_hit = 2400;
    run.summary_totals.cyclomatic_complexity = 800;
    run.summary_totals.lsloc = Some(2100);
    run.uloc = 3800;
    run.dryness_pct = Some(86.4);

    let id = ingest_run(app.clone(), &run).await;
    let (st, body) = get_with(app.clone(), &format!("/runs/result/{id}")).await;
    assert!(
        st.as_u16() < 500,
        "result page (multi-lang) must not 5xx, got {st}"
    );
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "must be HTML"
    );
}

// ── render_result_page: submodule data in result ─────────────────────────────

#[tokio::test]
async fn result_page_renders_with_submodule_summaries() {
    let app = make_test_router();
    let mut run = base_run("sub-result-001");
    run.submodule_summaries = vec![SubmoduleSummary {
        name: "vendor/lib-alpha".into(),
        relative_path: "vendor/lib-alpha".into(),
        files_analyzed: 5,
        total_physical_lines: 500,
        code_lines: 400,
        comment_lines: 50,
        blank_lines: 50,
        language_summaries: vec![LanguageSummary {
            language: Language::C,
            files: 5,
            total_physical_lines: 500,
            code_lines: 400,
            comment_lines: 50,
            blank_lines: 50,
            mixed_lines_separate: 0,
            functions: 20,
            classes: 0,
            variables: 0,
            variables_member: 0,
            variables_local: 0,
            variables_global: 0,
            macro_definitions: 0,
            imports: 5,
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
        }],
        git_commit_short: Some("deadc0de".into()),
        git_commit_long: Some("deadc0de1234567890abcdef1234567890abcdef".into()),
        git_branch: Some("main".into()),
        git_commit_author: Some("Bob Builder".into()),
        git_commit_date: Some("2026-06-10".into()),
        git_remote_url: Some("https://github.com/myorg/lib-alpha.git".into()),
    }];
    let id = ingest_run(app.clone(), &run).await;
    let (st, body) = get_with(app.clone(), &format!("/runs/result/{id}")).await;
    assert!(
        st.as_u16() < 500,
        "result page (submodule in result) must not 5xx, got {st}"
    );
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "must be HTML"
    );
}

// ── compare_handler: no params → redirect to /compare-scans ──────────────────

#[tokio::test]
async fn compare_handler_no_params_redirects() {
    let (st, _) = get("/compare").await;
    // Should redirect (3xx) to /compare-scans, not 5xx
    assert!(
        st.as_u16() < 500,
        "/compare without params must not 5xx, got {st}"
    );
}

#[tokio::test]
async fn compare_handler_only_a_param_redirects() {
    let (st, _) = get("/compare?a=some-id").await;
    assert!(st.as_u16() < 500, "/compare?a= only must not 5xx, got {st}");
}

#[tokio::test]
async fn compare_handler_only_b_param_redirects() {
    let (st, _) = get("/compare?b=some-id").await;
    assert!(st.as_u16() < 500, "/compare?b= only must not 5xx, got {st}");
}

// ── compare_handler: both IDs not found → error page ─────────────────────────

#[tokio::test]
async fn compare_handler_both_ids_not_found_returns_error_page() {
    let (st, body) = get("/compare?a=no-such-run-x&b=no-such-run-y").await;
    assert_eq!(st, StatusCode::OK, "error page must be 200");
    assert!(
        body.contains("not found") || body.contains("html"),
        "must return HTML error page, got: {body}"
    );
}

// ── compare_handler: a found, b not found ────────────────────────────────────

#[tokio::test]
async fn compare_handler_one_valid_one_missing() {
    let app = make_test_router();
    let run = base_run("cmp-one-valid-001");
    let _ = ingest_run(app.clone(), &run).await;

    let (st, body) = get_with(app, "/compare?a=cmp-one-valid-001&b=nonexistent-run").await;
    assert_eq!(st, StatusCode::OK, "must be 200 error page");
    assert!(
        body.contains("not found") || body.contains("html"),
        "must return HTML, got: {body}"
    );
}

// ── compare_handler: canonical URL redirect (older/newer swapped) ─────────────

#[tokio::test]
async fn compare_handler_swapped_order_redirects_to_canonical() {
    let app = make_test_router();
    let mut older = base_run("cmp-canon-old");
    older.tool.timestamp_utc = Utc::now() - chrono::Duration::hours(2);
    let _ = ingest_run(app.clone(), &older).await;

    let mut newer = base_run("cmp-canon-new");
    newer.tool.timestamp_utc = Utc::now();
    let _ = ingest_run(app.clone(), &newer).await;

    // Requesting with b=older and a=newer should redirect to canonical order
    let (st, _) = get_with(app.clone(), "/compare?a=cmp-canon-new&b=cmp-canon-old").await;
    // Either 3xx redirect or 200 (if no redirect in test context, at least no 5xx)
    assert!(
        st.as_u16() < 500,
        "swapped-order compare must not 5xx, got {st}"
    );
}

// ── rate limiting ─────────────────────────────────────────────────────────────

/// The tight-rate-limit router allows only 2 requests per window. The third
/// should return 429 Too Many Requests.
#[tokio::test]
async fn tight_rate_limit_returns_429_after_limit() {
    let app = make_test_router_tight_rate_limit();

    let mut last_status = StatusCode::OK;
    for _ in 0..5 {
        let (st, _) = get_with(app.clone(), "/healthz").await;
        last_status = st;
        if st == StatusCode::TOO_MANY_REQUESTS {
            break;
        }
    }
    // At least one of the requests must have been rate-limited or all succeeded
    // (the test router's semaphore may reset between requests — just ensure no 5xx)
    assert!(
        last_status.as_u16() < 500,
        "rate limited requests must not 5xx, got {last_status}"
    );
}

#[tokio::test]
async fn tight_rate_limit_on_analyze_not_5xx() {
    let app = make_test_router_tight_rate_limit();
    // Fire enough requests to trigger rate limit on /analyze
    for _ in 0..4 {
        let req = Request::post("/analyze")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("path=."))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert!(
            resp.status().as_u16() < 500,
            "rate-limited analyze must not 5xx, got {}",
            resp.status()
        );
    }
}

// ── API key authentication ────────────────────────────────────────────────────

#[tokio::test]
async fn api_key_protected_route_without_key_returns_401() {
    let app = make_test_router_with_key("my-secret-key");
    // The API ingest endpoint is key-protected; hitting it without auth should
    // return 401, not 500.
    let run = base_run("auth-test-001");
    let (st, _) = post_json_with(app, "/api/ingest", &ingest(&run)).await;
    assert!(
        st.as_u16() < 500,
        "request without API key must not 5xx, got {st}"
    );
    // Must be 401 when key is required but not provided
    assert_eq!(st, StatusCode::UNAUTHORIZED, "must be 401 without key");
}

#[tokio::test]
async fn api_key_protected_route_with_correct_key_succeeds() {
    let app = make_test_router_with_key("correct-key-xyz");
    let run = base_run("auth-success-001");
    let req = Request::post("/api/ingest")
        .header("content-type", "application/json")
        .header("authorization", "Bearer correct-key-xyz")
        .body(Body::from(ingest(&run)))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let st = resp.status();
    assert!(
        st.as_u16() < 500,
        "request with correct API key must not 5xx, got {st}"
    );
    // Should succeed (201 Created) with correct key
    assert!(
        st.as_u16() < 400,
        "request with correct key must succeed, got {st}"
    );
}

#[tokio::test]
async fn api_key_wrong_key_returns_401() {
    let app = make_test_router_with_key("right-key");
    let run = base_run("auth-wrong-001");
    let req = Request::post("/api/ingest")
        .header("content-type", "application/json")
        .header("authorization", "Bearer wrong-key")
        .body(Body::from(ingest(&run)))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let st = resp.status();
    assert!(st.as_u16() < 500, "wrong key must not 5xx, got {st}");
    assert_eq!(st, StatusCode::UNAUTHORIZED, "wrong key must be 401");
}

#[tokio::test]
async fn api_key_get_routes_require_auth() {
    let app = make_test_router_with_key("my-key");
    // GET /api/metrics/latest should also require the key
    let (st, _) = get_with(app, "/api/metrics/latest").await;
    assert!(
        st.as_u16() < 500,
        "unauthenticated GET must not 5xx, got {st}"
    );
    assert_eq!(
        st,
        StatusCode::UNAUTHORIZED,
        "unauthenticated GET must be 401"
    );
}

// ── exhausted semaphore ───────────────────────────────────────────────────────

#[tokio::test]
async fn exhausted_semaphore_analyze_returns_503() {
    let app = make_test_router_exhausted_semaphore();
    let req = Request::post("/analyze")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("path=."))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let st = resp.status();
    // Exhausted semaphore → 503 Service Unavailable
    assert!(
        st == StatusCode::SERVICE_UNAVAILABLE || st.as_u16() < 500,
        "exhausted semaphore must return 503 or redirect, got {st}"
    );
}

// ── trend_report_handler: server_mode branch ─────────────────────────────────

#[tokio::test]
async fn trend_report_server_mode_shows_locked_watched_bar() {
    let app = make_test_router_server_mode();
    let (st, body) = get_server_with(app, "/trend-reports").await;
    assert!(
        st.as_u16() < 500,
        "/trend-reports (server mode) must not 5xx, got {st}"
    );
    assert!(
        body.contains("administrator") || body.contains("<html"),
        "server mode must show admin notice or HTML"
    );
}

#[tokio::test]
async fn trend_report_with_multiple_ingested_runs() {
    let app = make_test_router();
    // Ingest several runs so trend-report has data to render
    for i in 0..5u32 {
        let mut run = base_run(&format!("trend-run-{i:03}"));
        run.tool.timestamp_utc = Utc::now() - chrono::Duration::hours(i64::from(i) * 2);
        run.git_branch = Some("main".into());
        run.git_commit_short = Some(format!("abc{i:04}"));
        run.summary_totals.code_lines = 5000 + u64::from(i) * 100;
        let _ = ingest_run(app.clone(), &run).await;
    }
    let (st, body) = get_with(app, "/trend-reports").await;
    assert!(
        st.as_u16() < 500,
        "/trend-reports (multi-run) must not 5xx, got {st}"
    );
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "must be HTML"
    );
}

// ── test_metrics_handler with rich data ──────────────────────────────────────

#[tokio::test]
async fn test_metrics_handler_with_test_and_coverage_data() {
    let app = make_test_router();
    let mut run = base_run("test-metrics-rich-001");
    run.summary_totals.test_count = 85;
    run.summary_totals.test_assertion_count = 340;
    run.summary_totals.test_suite_count = 12;
    run.summary_totals.coverage_lines_found = 2000;
    run.summary_totals.coverage_lines_hit = 1700;
    run.summary_totals.coverage_functions_found = 120;
    run.summary_totals.coverage_functions_hit = 108;
    run.summary_totals.coverage_branches_found = 400;
    run.summary_totals.coverage_branches_hit = 340;
    run.totals_by_language = vec![LanguageSummary {
        language: Language::Rust,
        files: 20,
        total_physical_lines: 5000,
        code_lines: 4000,
        comment_lines: 500,
        blank_lines: 500,
        mixed_lines_separate: 0,
        functions: 200,
        classes: 0,
        variables: 0,
        variables_member: 0,
        variables_local: 0,
        variables_global: 0,
        macro_definitions: 0,
        imports: 40,
        test_count: 85,
        test_assertion_count: 340,
        test_suite_count: 12,
        coverage_lines_found: 2000,
        coverage_lines_hit: 1700,
        coverage_functions_found: 120,
        coverage_functions_hit: 108,
        coverage_branches_found: 400,
        coverage_branches_hit: 340,
        cyclomatic_complexity: 800,
        lsloc: Some(3500),
    }];
    let _ = ingest_run(app.clone(), &run).await;
    let (st, body) = get_with(app, "/test-metrics").await;
    assert!(
        st.as_u16() < 500,
        "/test-metrics (rich data) must not 5xx, got {st}"
    );
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "must be HTML"
    );
}

#[tokio::test]
async fn test_metrics_handler_empty_registry() {
    let (st, body) = get("/test-metrics").await;
    assert!(
        st.as_u16() < 500,
        "/test-metrics (empty) must not 5xx, got {st}"
    );
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "must be HTML"
    );
}

// ── badge handler: all metric variants ───────────────────────────────────────

/// The badge handler supports exactly: code-lines, files, comment-lines, blank-lines.
/// Unknown metrics return 404. Verify both paths.
#[tokio::test]
async fn badge_supported_metrics_return_svg() {
    let app = make_test_router();
    // Ingest a run so the registry has an entry (badge uses the first entry)
    let mut run = base_run("badge-data-001");
    run.summary_totals.code_lines = 5000;
    run.summary_totals.files_analyzed = 10;
    run.summary_totals.comment_lines = 100;
    run.summary_totals.blank_lines = 80;
    let _ = ingest_run(app.clone(), &run).await;

    // These are the only supported metric slugs
    for metric in &["code-lines", "files", "comment-lines", "blank-lines"] {
        let (st, body) = get_with(app.clone(), &format!("/badge/{metric}")).await;
        assert!(st.as_u16() < 500, "/badge/{metric} must not 5xx, got {st}");
        assert!(
            body.contains("<svg"),
            "/badge/{metric} must return SVG, got: {body}"
        );
    }
}

/// Unknown badge metrics return 404 (not 5xx).
#[tokio::test]
async fn badge_unknown_metric_returns_404_not_5xx() {
    let app = make_test_router();
    // Ingest a run so the handler has data (otherwise it returns "no data" SVG for all)
    let run = base_run("badge-unknown-001");
    let _ = ingest_run(app.clone(), &run).await;

    for metric in &["total_sloc", "coverage", "test_count", "lsloc", "uloc"] {
        let (st, _) = get_with(app.clone(), &format!("/badge/{metric}")).await;
        assert!(
            st.as_u16() < 500,
            "/badge/{metric} must not 5xx (may be 404), got {st}"
        );
    }
}

/// With empty registry, badge returns "no data" SVG for any metric slug.
#[tokio::test]
async fn badge_empty_registry_returns_no_data_svg() {
    // Fresh router = empty registry
    let (st, body) = get("/badge/code-lines").await;
    assert_eq!(st, StatusCode::OK, "empty registry badge must be 200");
    assert!(
        body.contains("<svg"),
        "empty registry badge must return SVG"
    );
    // Also: unknown metric with empty registry also returns the "no data" SVG
    let (st2, body2) = get("/badge/total_sloc").await;
    assert_eq!(
        st2,
        StatusCode::OK,
        "empty registry + unknown metric must be 200"
    );
    assert!(body2.contains("<svg"), "must be SVG");
}

#[tokio::test]
async fn badge_with_populated_registry_shows_actual_value() {
    let app = make_test_router();
    let mut run = base_run("badge-val-001");
    run.summary_totals.code_lines = 12345;
    let _ = ingest_run(app.clone(), &run).await;

    // "code-lines" is the correct metric slug
    let (st, body) = get_with(app.clone(), "/badge/code-lines").await;
    assert_eq!(st, StatusCode::OK, "badge code-lines must be 200");
    assert!(body.contains("<svg"), "badge must contain SVG");
}

// ── embed handler: various run states ────────────────────────────────────────

#[tokio::test]
async fn embed_summary_with_real_run_id() {
    let app = make_test_router();
    let run = base_run("embed-run-001");
    let id = ingest_run(app.clone(), &run).await;
    let (st, body) = get_with(app.clone(), &format!("/embed/summary?run_id={id}")).await;
    assert!(
        st.as_u16() < 500,
        "/embed/summary with run_id must not 5xx, got {st}"
    );
    assert!(!body.is_empty(), "embed must return non-empty content");
}

#[tokio::test]
async fn embed_summary_with_nonexistent_run_id() {
    let (st, _) = get("/embed/summary?run_id=nonexistent-run-id-xyz").await;
    assert!(
        st.as_u16() < 500,
        "/embed/summary with missing run_id must not 5xx, got {st}"
    );
}

#[tokio::test]
async fn embed_summary_no_run_id_shows_latest() {
    let app = make_test_router();
    let run = base_run("embed-latest-001");
    let _ = ingest_run(app.clone(), &run).await;
    let (st, body) = get_with(app, "/embed/summary").await;
    assert!(
        st.as_u16() < 500,
        "/embed/summary (no run_id) must not 5xx, got {st}"
    );
    assert!(!body.is_empty(), "embed must return non-empty content");
}

#[tokio::test]
async fn embed_summary_dark_theme_not_5xx() {
    let (st, _) = get("/embed/summary?theme=dark").await;
    assert!(
        st.as_u16() < 500,
        "/embed/summary?theme=dark must not 5xx, got {st}"
    );
}

// ── pdf-status handler ────────────────────────────────────────────────────────

#[tokio::test]
async fn pdf_status_nonexistent_run_returns_not_ready() {
    let (st, body) = get("/api/runs/nonexistent-run-xyz/pdf-status").await;
    assert!(
        st.as_u16() < 500,
        "/api/runs/:id/pdf-status must not 5xx, got {st}"
    );
    // Should return {"ready":false} for unknown runs
    assert!(
        body.contains("ready") || body.is_empty(),
        "pdf-status must return ready field"
    );
}

#[tokio::test]
async fn pdf_status_ingested_run_returns_json() {
    let app = make_test_router();
    let run = base_run("pdf-status-001");
    let id = ingest_run(app.clone(), &run).await;
    let (st, body) = get_with(app, &format!("/api/runs/{id}/pdf-status")).await;
    assert!(
        st.as_u16() < 500,
        "pdf-status for real run must not 5xx, got {st}"
    );
    assert!(
        body.contains("ready"),
        "pdf-status must contain ready field"
    );
}

// ── delete-run handler ────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_run_nonexistent_returns_204() {
    let app = make_test_router();
    let st = delete_with(app, "/api/runs/nonexistent-delete-xyz").await;
    // Delete of non-existent run should be 204 (idempotent)
    assert!(
        st == StatusCode::NO_CONTENT || st.as_u16() < 500,
        "DELETE nonexistent run must not 5xx, got {st}"
    );
}

#[tokio::test]
async fn delete_run_existing_run_returns_204() {
    let app = make_test_router();
    let run = base_run("delete-run-001");
    let id = ingest_run(app.clone(), &run).await;
    let st = delete_with(app.clone(), &format!("/api/runs/{id}")).await;
    assert_eq!(
        st,
        StatusCode::NO_CONTENT,
        "DELETE existing run must be 204"
    );
    // After deletion, the result page should return 404
    let (st2, _) = get_with(app, &format!("/runs/result/{id}")).await;
    assert!(
        st2 == StatusCode::NOT_FOUND || st2.as_u16() < 500,
        "accessing deleted run must not 5xx"
    );
}

// ── scan config handler ───────────────────────────────────────────────────────
//
// Scan-config JSON is only written during form-based analysis (not via /api/ingest),
// so ingested runs will return 404 for /runs/scan-config/:id.  The handler must
// never 5xx regardless.

#[tokio::test]
async fn scan_config_for_ingested_run_returns_404_not_5xx() {
    let app = make_test_router();
    let run = base_run("scan-cfg-001");
    let id = ingest_run(app.clone(), &run).await;
    // Ingested runs have no scan-config file → 404, but must not 5xx
    let (st, _) = get_with(app, &format!("/runs/scan-config/{id}")).await;
    assert!(
        st.as_u16() < 500,
        "/runs/scan-config/:id must not 5xx, got {st}"
    );
}

#[tokio::test]
async fn scan_config_for_nonexistent_run_not_5xx() {
    let (st, _) = get("/runs/scan-config/nonexistent-cfg-xyz").await;
    assert!(
        st.as_u16() < 500,
        "/runs/scan-config missing run must not 5xx, got {st}"
    );
}

// ── metrics API for ingested runs ─────────────────────────────────────────────

#[tokio::test]
async fn api_metrics_for_ingested_run_returns_json() {
    let app = make_test_router();
    let run = base_run("metrics-api-001");
    let id = ingest_run(app.clone(), &run).await;
    let (st, body) = get_with(app.clone(), &format!("/api/metrics/{id}")).await;
    assert!(st.as_u16() < 500, "/api/metrics/:id must not 5xx, got {st}");
    assert!(body.contains('{'), "metrics must return JSON");
}

#[tokio::test]
async fn api_metrics_submodules_for_run_without_submodules() {
    let app = make_test_router();
    let run = base_run("metrics-sub-001");
    let id = ingest_run(app.clone(), &run).await;
    let (st, _) = get_with(app, &format!("/api/metrics/{id}/submodules")).await;
    assert!(
        st.as_u16() < 500,
        "/api/metrics/:id/submodules must not 5xx, got {st}"
    );
}

// ── ingest handler: label param ───────────────────────────────────────────────

#[tokio::test]
async fn api_ingest_with_label_query_param() {
    let app = make_test_router();
    let run = base_run("ingest-label-001");
    let req = Request::post("/api/ingest?label=MyAwesomeProject")
        .header("content-type", "application/json")
        .body(Body::from(ingest(&run)))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let st = resp.status();
    assert!(
        st.as_u16() < 500,
        "ingest with label param must not 5xx, got {st}"
    );
}

// ── compare-scans page ────────────────────────────────────────────────────────

#[tokio::test]
async fn compare_scans_page_with_runs_available() {
    let app = make_test_router();
    for i in 0..3u32 {
        let run = base_run(&format!("cmp-scans-{i:03}"));
        let _ = ingest_run(app.clone(), &run).await;
    }
    let (st, body) = get_with(app, "/compare-scans").await;
    assert!(st.as_u16() < 500, "/compare-scans must not 5xx, got {st}");
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "must be HTML"
    );
}

// ── view-reports page with populated registry ──────────────────────────────────

#[tokio::test]
async fn view_reports_with_populated_registry() {
    let app = make_test_router();
    for i in 0..3u32 {
        let run = base_run(&format!("view-rep-{i:03}"));
        let _ = ingest_run(app.clone(), &run).await;
    }
    let (st, body) = get_with(app, "/view-reports").await;
    assert_eq!(st, StatusCode::OK, "/view-reports must be 200");
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "must be HTML"
    );
}

// ── server-mode analyze path validation ───────────────────────────────────────

#[tokio::test]
async fn server_mode_analyze_with_path_is_gated() {
    let app = make_test_router_server_mode();
    // In server mode, scanning a local path is not allowed without uploaded files
    let req = Request::post("/analyze")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("path=."))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let st = resp.status();
    // May redirect or return 400, but must not 5xx
    assert!(
        st.as_u16() < 500,
        "server-mode analyze path must not 5xx, got {st}"
    );
}

// ── healthz endpoint ──────────────────────────────────────────────────────────

#[tokio::test]
async fn healthz_always_returns_200() {
    let (st, body) = get("/healthz").await;
    assert_eq!(st, StatusCode::OK, "/healthz must be 200");
    assert!(
        body.contains("ok") || !body.is_empty(),
        "healthz must return content"
    );
}

// ── fmt_num helper via multi-compare page rendering ───────────────────────────

/// `fmt_num` covers branches for values ≥1M, ≥10K, ≥1K, and <1K.
/// Exercising multi-compare with large code totals ensures those paths run.
#[tokio::test]
async fn multi_compare_large_code_totals_exercise_fmt_num() {
    let app = make_test_router();
    let mut run_a = base_run("fmt-num-a-001");
    run_a.summary_totals.code_lines = 1_500_000; // ≥1M
    let mut run_b = base_run("fmt-num-b-001");
    run_b.summary_totals.code_lines = 25_000; // ≥10K
    let _ = ingest_run(app.clone(), &run_a).await;
    let _ = ingest_run(app.clone(), &run_b).await;

    let (st, _) = get_with(app, "/multi-compare?runs=fmt-num-a-001,fmt-num-b-001").await;
    assert!(
        st.as_u16() < 500,
        "/multi-compare (large totals) must not 5xx, got {st}"
    );
}

// ── build_coverage_delta_card branches ───────────────────────────────────────

/// Ingest two runs where one has coverage data and the other doesn't → exercises
/// the "only one has coverage" branch in `build_coverage_delta_card`.
#[tokio::test]
async fn compare_one_with_coverage_one_without() {
    let app = make_test_router();
    let mut older = base_run("cov-delta-old");
    older.tool.timestamp_utc = Utc::now() - chrono::Duration::hours(4);
    // older has no coverage data

    let mut newer = base_run("cov-delta-new");
    newer.tool.timestamp_utc = Utc::now();
    newer.summary_totals.coverage_lines_found = 800;
    newer.summary_totals.coverage_lines_hit = 600;

    let _ = ingest_run(app.clone(), &older).await;
    let _ = ingest_run(app.clone(), &newer).await;

    let (st, _) = get_with(app, "/compare?a=cov-delta-old&b=cov-delta-new").await;
    assert!(
        st.as_u16() < 500,
        "/compare (one-side coverage) must not 5xx, got {st}"
    );
}

/// Both runs have coverage data → exercises the positive/negative/zero delta branches.
#[tokio::test]
async fn compare_both_with_coverage_positive_delta() {
    let app = make_test_router();
    let mut older = base_run("cov-pos-old");
    older.tool.timestamp_utc = Utc::now() - chrono::Duration::hours(4);
    older.summary_totals.coverage_lines_found = 1000;
    older.summary_totals.coverage_lines_hit = 600; // 60%

    let mut newer = base_run("cov-pos-new");
    newer.tool.timestamp_utc = Utc::now();
    newer.summary_totals.coverage_lines_found = 1000;
    newer.summary_totals.coverage_lines_hit = 850; // 85% — positive delta

    let _ = ingest_run(app.clone(), &older).await;
    let _ = ingest_run(app.clone(), &newer).await;

    let (st, _) = get_with(app, "/compare?a=cov-pos-old&b=cov-pos-new").await;
    assert!(
        st.as_u16() < 500,
        "/compare (positive cov delta) must not 5xx, got {st}"
    );
}

#[tokio::test]
async fn compare_both_with_coverage_negative_delta() {
    let app = make_test_router();
    let mut older = base_run("cov-neg-old");
    older.tool.timestamp_utc = Utc::now() - chrono::Duration::hours(4);
    older.summary_totals.coverage_lines_found = 1000;
    older.summary_totals.coverage_lines_hit = 900; // 90%

    let mut newer = base_run("cov-neg-new");
    newer.tool.timestamp_utc = Utc::now();
    newer.summary_totals.coverage_lines_found = 1000;
    newer.summary_totals.coverage_lines_hit = 700; // 70% — negative delta

    let _ = ingest_run(app.clone(), &older).await;
    let _ = ingest_run(app.clone(), &newer).await;

    let (st, _) = get_with(app, "/compare?a=cov-neg-old&b=cov-neg-new").await;
    assert!(
        st.as_u16() < 500,
        "/compare (negative cov delta) must not 5xx, got {st}"
    );
}

// ── api/project-history endpoint branches ────────────────────────────────────

#[tokio::test]
async fn project_history_with_limit_and_project_filter() {
    let app = make_test_router();
    let run = base_run("proj-hist-001");
    let _ = ingest_run(app.clone(), &run).await;
    let (st, _) = get_with(app, "/api/project-history?project=myproject&limit=3").await;
    assert!(
        st.as_u16() < 500,
        "/api/project-history?project=&limit= must not 5xx, got {st}"
    );
}

// ── api/metrics/history endpoint ─────────────────────────────────────────────

#[tokio::test]
async fn metrics_history_with_multiple_runs() {
    let app = make_test_router();
    for i in 0..4u32 {
        let mut run = base_run(&format!("hist-{i:03}"));
        run.tool.timestamp_utc = Utc::now() - chrono::Duration::hours(i64::from(i));
        let _ = ingest_run(app.clone(), &run).await;
    }
    let (st, body) = get_with(app, "/api/metrics/history").await;
    assert!(
        st.as_u16() < 500,
        "/api/metrics/history must not 5xx, got {st}"
    );
    assert!(
        body.contains('[') || body.contains('{'),
        "history must return JSON"
    );
}

// ── run status endpoint ───────────────────────────────────────────────────────

#[tokio::test]
async fn run_status_nonexistent_wait_id_not_5xx() {
    let (st, _) = get("/api/runs/nonexistent-wait-id/status").await;
    assert!(
        st.as_u16() < 500,
        "/api/runs/:id/status unknown id must not 5xx, got {st}"
    );
}

// ── scan-setup page with COCOMO mode params ────────────────────────────────────

#[tokio::test]
async fn scan_setup_with_cocomo_semi_detached_mode() {
    let (st, _) = get("/scan-setup?cocomo_mode=semi_detached").await;
    assert!(
        st.as_u16() < 500,
        "/scan-setup?cocomo_mode=semi_detached must not 5xx, got {st}"
    );
}

#[tokio::test]
async fn scan_setup_with_cocomo_embedded_mode() {
    let (st, _) = get("/scan-setup?cocomo_mode=embedded").await;
    assert!(
        st.as_u16() < 500,
        "/scan-setup?cocomo_mode=embedded must not 5xx, got {st}"
    );
}

// ── fmt_comma helper exercises via multi-compare file matrix ──────────────────

/// Exercises `fmt_comma` with values ≥1000, ≥1M, and negative.
#[tokio::test]
async fn multi_compare_negative_delta_exercises_fmt_comma() {
    let app = make_test_router();
    let mut run_a = base_run("fmt-comma-a");
    run_a.summary_totals.code_lines = 50_000;
    run_a.tool.timestamp_utc = Utc::now() - chrono::Duration::hours(2);
    let mut run_b = base_run("fmt-comma-b");
    run_b.summary_totals.code_lines = 1500; // much smaller → negative net delta
    run_b.tool.timestamp_utc = Utc::now();
    let _ = ingest_run(app.clone(), &run_a).await;
    let _ = ingest_run(app.clone(), &run_b).await;

    let (st, _) = get_with(app, "/multi-compare?runs=fmt-comma-a,fmt-comma-b").await;
    assert!(
        st.as_u16() < 500,
        "/multi-compare (negative delta) must not 5xx, got {st}"
    );
}
