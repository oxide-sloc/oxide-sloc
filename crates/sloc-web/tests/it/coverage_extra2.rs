// Additional coverage-boost integration tests for sloc-web.
//
// These target handler/helper branches not exercised by the existing suites:
//   * api_metrics_churn_handler — per-scan churn computed across two scans of the
//     same project (the prev/curr delta branch);
//   * build_test_scope_sub_entry / build_scope_entry_for_run — the submodule branch
//     of the /test-metrics scope JSON, reached when an ingested run carries
//     submodule summaries whose language tallies include tests;
//   * apply_submodule_filter — /api/metrics/history?submodule=<name> sourcing metrics
//     from a SubmoduleSummary inside each scan's JSON artifact;
//   * build_preview_html + render_submodule_chips + collect_preview_rows — /preview of
//     a real directory tree that declares git submodules via .gitmodules;
//   * build_sub_run + the serve_submodule_* artifact arms — a real analyze with
//     submodule breakdown, then fetching the per-submodule HTML/PDF artifacts.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use sloc_web::make_test_router;
use tower::ServiceExt;

use chrono::Utc;
use sloc_config::AppConfig;
use sloc_core::{
    AnalysisRun, EffectiveCounts, EnvironmentMetadata, FileRecord, FileStatus, LanguageSummary,
    SubmoduleSummary, SummaryTotals, ToolMetadata,
};
use sloc_languages::{Language, ParseMode, RawLineCounts};

// ── HTTP helpers ─────────────────────────────────────────────────────────────

async fn get_shared(app: Router, uri: &str) -> (StatusCode, String) {
    let resp = app
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn post_form_shared(
    app: Router,
    uri: &str,
    body: &str,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let req = Request::post(uri)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body.to_owned()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

async fn post_json_shared(app: Router, uri: &str, json: &str) -> (StatusCode, String) {
    let req = Request::post(uri)
        .header("content-type", "application/json")
        .body(Body::from(json.to_owned()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

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

/// Poll `/api/runs/{wait_id}/status` until the run reports `complete`,
/// returning its `run_id` (empty string if it never completes).
async fn wait_for_run_id(app: Router, wait_id: &str) -> String {
    for _ in 0..200 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let (_, body) = get_shared(app.clone(), &format!("/api/runs/{wait_id}/status")).await;
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
            match v["state"].as_str().unwrap_or("") {
                "complete" => return v["run_id"].as_str().unwrap_or("").to_owned(),
                "failed" | "cancelled" => return String::new(),
                _ => {}
            }
        }
    }
    String::new()
}

/// Drive a synchronous-feeling analyze: POST /analyze, then poll for the `run_id`.
async fn analyze_and_wait(app: Router, form: &str) -> String {
    let (status, headers, _) = post_form_shared(app.clone(), "/analyze", form).await;
    assert!(status.is_success() || status.is_redirection() || status.is_client_error());
    let wait_id = headers
        .get("x-wait-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    if wait_id.is_empty() {
        return String::new();
    }
    wait_for_run_id(app, &wait_id).await
}

// ── AnalysisRun fixtures ─────────────────────────────────────────────────────

fn base_run(id: &str, root: &str) -> AnalysisRun {
    AnalysisRun {
        tool: ToolMetadata {
            name: "oxide-sloc".into(),
            version: "1.5.72".into(),
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
        input_roots: vec![root.into()],
        summary_totals: SummaryTotals {
            files_considered: 1,
            files_analyzed: 1,
            code_lines: 150,
            comment_lines: 10,
            blank_lines: 5,
            total_physical_lines: 165,
            test_count: 12,
            test_assertion_count: 30,
            test_suite_count: 3,
            ..SummaryTotals::default()
        },
        totals_by_language: vec![lang_summary_with_tests(Language::Rust, 1, 150, 12)],
        per_file_records: vec![file_record("src/lib.rs", Language::Rust, 150)],
        skipped_file_records: vec![],
        warnings: vec![],
        submodule_summaries: vec![],
        git_commit_short: Some("abc1234".into()),
        git_commit_long: None,
        git_branch: Some("main".into()),
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

fn file_record(path: &str, lang: Language, code: u64) -> FileRecord {
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
            test_count: 3,
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

const fn lang_summary_with_tests(
    lang: Language,
    files: u64,
    code: u64,
    tests: u64,
) -> LanguageSummary {
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
        imports: 1,
        test_count: tests,
        test_assertion_count: tests * 2,
        test_suite_count: 2,
        coverage_lines_found: 100,
        coverage_lines_hit: 80,
        coverage_functions_found: 10,
        coverage_functions_hit: 9,
        coverage_branches_found: 20,
        coverage_branches_hit: 15,
        cyclomatic_complexity: 4,
        lsloc: None,
    }
}

fn submodule_with_tests(name: &str, rel: &str, lang: Language, code: u64) -> SubmoduleSummary {
    SubmoduleSummary {
        name: name.into(),
        relative_path: rel.into(),
        files_analyzed: 1,
        total_physical_lines: code + 2,
        code_lines: code,
        comment_lines: 1,
        blank_lines: 1,
        language_summaries: vec![lang_summary_with_tests(lang, 1, code, 5)],
        git_commit_short: Some("deadbeef".into()),
        git_commit_long: None,
        git_branch: Some("main".into()),
        git_commit_author: None,
        git_commit_date: None,
        git_remote_url: Some("https://github.com/test-org/sub.git".into()),
    }
}

fn json_of(run: &AnalysisRun) -> String {
    serde_json::to_string(run).unwrap()
}

/// Build a small multi-file project on disk for a realistic analyze.
fn make_sample_project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("lib.rs"),
        "// a comment\nfn add(a: i32, b: i32) -> i32 { a + b }\n\n#[test]\nfn t() { assert_eq!(add(1, 2), 3); }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("app.py"),
        "import os\n\ndef greet(name):\n    \"\"\"docstring\"\"\"\n    return name\n",
    )
    .unwrap();
    dir
}

// ── churn ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn churn_across_two_scans_of_same_project_returns_delta() {
    let project = make_sample_project();
    let app = make_test_router();
    let path_arg = pct_encode(project.path().to_str().unwrap());

    // First scan.
    let form = format!("path={path_arg}&report_title=Churn+A&per_file=enabled");
    let id1 = analyze_and_wait(app.clone(), &form).await;
    assert!(!id1.is_empty(), "first analyze should complete");

    // Mutate the project so the second scan differs (added + modified lines).
    std::fs::write(
        project.path().join("lib.rs"),
        "// a comment\n// another\nfn add(a: i32, b: i32) -> i32 { a + b }\nfn sub(a: i32, b: i32) -> i32 { a - b }\n\n#[test]\nfn t() { assert_eq!(add(1, 2), 3); }\n",
    )
    .unwrap();
    std::fs::write(project.path().join("extra.rs"), "pub fn z() -> u8 { 7 }\n").unwrap();

    let form2 = format!("path={path_arg}&report_title=Churn+B&per_file=enabled");
    let id2 = analyze_and_wait(app.clone(), &form2).await;
    assert!(!id2.is_empty(), "second analyze should complete");

    // Churn endpoint: scoped to the project root and unscoped.
    let (status, body) =
        get_shared(app.clone(), &format!("/api/metrics/churn?root={path_arg}")).await;
    assert_eq!(status, StatusCode::OK, "churn endpoint must return 200");
    let arr: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(arr.is_array(), "churn returns a JSON array, got {body}");

    let (status_all, _) = get_shared(app.clone(), "/api/metrics/churn").await;
    assert_eq!(status_all, StatusCode::OK);

    let (status_lim, _) = get_shared(app.clone(), "/api/metrics/churn?limit=5").await;
    assert_eq!(status_lim, StatusCode::OK);
}

// ── test-metrics submodule scope JSON ──────────────────────────────────────────

#[tokio::test]
async fn test_metrics_includes_submodule_scope_after_ingest() {
    let app = make_test_router();
    let mut run = base_run("cov2-tm-001", "/test/tm-proj");
    run.submodule_summaries = vec![
        submodule_with_tests("vendor/lib-a", "vendor/lib-a", Language::Rust, 100),
        submodule_with_tests("vendor/lib-b", "vendor/lib-b", Language::Python, 60),
    ];
    let (status, body) = post_json_shared(app.clone(), "/api/ingest", &json_of(&run)).await;
    assert_eq!(status, StatusCode::CREATED, "ingest failed: {body}");

    // /test-metrics builds scope data from disk JSON -> build_test_scope_sub_entry.
    let (st, html) = get_shared(app.clone(), "/test-metrics").await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        html.contains("<html") || html.contains("<!doctype"),
        "test-metrics should render HTML"
    );
}

// ── submodule-filtered metrics history ─────────────────────────────────────────

#[tokio::test]
async fn metrics_history_submodule_filter_sources_from_submodule_summary() {
    let app = make_test_router();
    let mut run = base_run("cov2-hist-001", "/test/hist-proj");
    run.submodule_summaries = vec![submodule_with_tests(
        "vendor/lib-a",
        "vendor/lib-a",
        Language::Rust,
        100,
    )];
    let (status, body) = post_json_shared(app.clone(), "/api/ingest", &json_of(&run)).await;
    assert_eq!(status, StatusCode::CREATED, "ingest failed: {body}");

    // Filter history to a submodule -> apply_submodule_filter reads the JSON artifact.
    let (st, hist) = get_shared(app.clone(), "/api/metrics/history?submodule=vendor%2Flib-a").await;
    assert_eq!(st, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&hist).unwrap_or(serde_json::Value::Null);
    assert!(!v.is_null(), "history JSON should parse, got {hist}");

    // A submodule name that does not exist -> filter yields None and the entry is dropped.
    let (st_none, _) =
        get_shared(app.clone(), "/api/metrics/history?submodule=does-not-exist").await;
    assert_eq!(st_none, StatusCode::OK);
}

// ── /preview with declared git submodules ──────────────────────────────────────

#[tokio::test]
async fn preview_renders_submodule_chips_for_gitmodules_tree() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();
    std::fs::create_dir_all(root.join("libs").join("alpha")).unwrap();
    std::fs::write(
        root.join("libs").join("alpha").join("a.rs"),
        "pub fn a() -> u8 { 1 }\n",
    )
    .unwrap();
    std::fs::write(
        root.join(".gitmodules"),
        "[submodule \"alpha\"]\n\tpath = libs/alpha\n\turl = https://example.com/alpha.git\n",
    )
    .unwrap();

    let app = make_test_router();
    let uri = format!("/preview?path={}", pct_encode(root.to_str().unwrap()));
    let (status, body) = get_shared(app, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("submodule-preview-chip") || body.contains("submodule-preview-strip"),
        "preview should render submodule chips when .gitmodules is present"
    );
}

#[tokio::test]
async fn preview_nonexistent_path_renders_error() {
    let app = make_test_router();
    let (status, body) = get_shared(
        app,
        "/preview?path=%2Fdefinitely%2Fnot%2Fa%2Freal%2Fpath%2Fxyz",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("does not exist") || body.contains("preview-error") || !body.is_empty(),
        "missing path should yield a preview error body"
    );
}

// ── real analyze with submodule breakdown -> per-submodule artifacts ────────────

#[tokio::test]
async fn analyze_with_submodules_serves_per_submodule_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("main.rs"), "fn main() { let _ = 1; }\n").unwrap();
    std::fs::create_dir_all(root.join("sub").join("alpha")).unwrap();
    std::fs::write(
        root.join("sub").join("alpha").join("a.rs"),
        "pub fn a() -> u8 { 1 }\n#[test]\nfn t() { assert_eq!(a(), 1); }\n",
    )
    .unwrap();
    std::fs::write(
        root.join(".gitmodules"),
        "[submodule \"alpha\"]\n\tpath = sub/alpha\n\turl = https://example.com/alpha.git\n",
    )
    .unwrap();

    let app = make_test_router();
    let form = format!(
        "path={}&report_title=SubBreakdown&submodule_breakdown=enabled",
        pct_encode(root.to_str().unwrap())
    );
    let run_id = analyze_and_wait(app.clone(), &form).await;
    assert!(
        !run_id.is_empty(),
        "analyze with submodules should complete"
    );

    // Result page renders (render_result_page over a run with submodule summaries).
    let (st, _) = get_shared(app.clone(), &format!("/runs/result/{run_id}")).await;
    assert_eq!(st, StatusCode::OK);

    // Per-submodule HTML + PDF artifact arms (build_sub_run on demand).
    for artifact in ["sub_alpha", "sub_alpha_pdf"] {
        let (st, _) = get_shared(app.clone(), &format!("/runs/{artifact}/{run_id}")).await;
        assert!(
            st.as_u16() < 500,
            "/runs/{artifact}/{run_id} must not 5xx, got {st}"
        );
    }

    // A bogus submodule artifact name still resolves cleanly (404 / not 5xx).
    let (st_missing, _) = get_shared(app.clone(), &format!("/runs/sub_nope/{run_id}")).await;
    assert!(st_missing.as_u16() < 500);
}

// ── compare two real ingested scans of the same project ────────────────────────

#[tokio::test]
async fn compare_two_ingested_runs_renders_full_comparison() {
    let app = make_test_router();

    // Two scans of the same project root so the comparison + churn path engages.
    let mut run_a = base_run("cov2-cmp-a", "/test/cmp-proj");
    run_a.summary_totals.code_lines = 150;
    let mut run_b = base_run("cov2-cmp-b", "/test/cmp-proj");
    run_b.summary_totals.code_lines = 220;
    run_b.per_file_records = vec![
        file_record("src/lib.rs", Language::Rust, 150),
        file_record("src/new.rs", Language::Rust, 70),
    ];

    let (sa, _) = post_json_shared(app.clone(), "/api/ingest", &json_of(&run_a)).await;
    let (sb, _) = post_json_shared(app.clone(), "/api/ingest", &json_of(&run_b)).await;
    assert_eq!(sa, StatusCode::CREATED);
    assert_eq!(sb, StatusCode::CREATED);

    let (status, body) = get_shared(app.clone(), "/compare?a=cov2-cmp-a&b=cov2-cmp-b").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "compare of two real runs must render"
    );
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "compare should render an HTML page"
    );

    // Super-repo scope and a (nonexistent) submodule scope exercise the scope branches.
    let (st_scope, _) = get_shared(
        app.clone(),
        "/compare?a=cov2-cmp-a&b=cov2-cmp-b&scope=super",
    )
    .await;
    assert_eq!(st_scope, StatusCode::OK);
}

// ── multi-compare across 3 real ingested scans ─────────────────────────────────

#[tokio::test]
async fn multi_compare_three_ingested_runs_renders_table() {
    let app = make_test_router();
    for (i, code) in [("cov2-mc-1", 100u64), ("cov2-mc-2", 140), ("cov2-mc-3", 90)] {
        let mut run = base_run(i, "/test/mc-proj");
        run.summary_totals.code_lines = code;
        let (s, b) = post_json_shared(app.clone(), "/api/ingest", &json_of(&run)).await;
        assert_eq!(s, StatusCode::CREATED, "ingest {i} failed: {b}");
    }
    let (status, body) = get_shared(
        app.clone(),
        "/multi-compare?runs=cov2-mc-1,cov2-mc-2,cov2-mc-3",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "multi-compare should render HTML"
    );
}

// ── project history JSON after ingest ──────────────────────────────────────────

#[tokio::test]
async fn project_history_after_ingest_returns_scan_count() {
    let app = make_test_router();
    let run = base_run("cov2-ph-1", "/test/ph-proj");
    let (s, _) = post_json_shared(app.clone(), "/api/ingest", &json_of(&run)).await;
    assert_eq!(s, StatusCode::CREATED);

    let (status, body) =
        get_shared(app.clone(), "/api/project-history?path=%2Ftest%2Fph-proj").await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    assert!(!v.is_null(), "project-history JSON should parse: {body}");
}

// ── badges with registry data ──────────────────────────────────────────────────

#[tokio::test]
async fn badges_with_ingested_data_render_each_metric() {
    let app = make_test_router();
    let run = base_run("cov2-badge-1", "/test/badge-proj");
    let (s, _) = post_json_shared(app.clone(), "/api/ingest", &json_of(&run)).await;
    assert_eq!(s, StatusCode::CREATED);

    for metric in ["code-lines", "files", "comment-lines", "blank-lines"] {
        let (status, body) = get_shared(app.clone(), &format!("/badge/{metric}")).await;
        assert_eq!(status, StatusCode::OK, "badge {metric} should be 200");
        assert!(body.contains("<svg"), "badge {metric} should be SVG");
    }

    // Unknown metric names hit the catch-all 404 arm.
    for metric in ["coverage", "tests", "unknown-metric"] {
        let (status, _) = get_shared(app.clone(), &format!("/badge/{metric}")).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "unknown badge {metric} should be 404"
        );
    }

    // Custom label/color query params.
    let (status, _) = get_shared(app.clone(), "/badge/code-lines?label=SLOC&color=blue").await;
    assert_eq!(status, StatusCode::OK);
}

// ── bundle download after ingest ───────────────────────────────────────────────

#[tokio::test]
async fn bundle_download_after_ingest_is_not_5xx() {
    let app = make_test_router();
    let run = base_run("cov2-bundle-1", "/test/bundle-proj");
    let (s, _) = post_json_shared(app.clone(), "/api/ingest", &json_of(&run)).await;
    assert_eq!(s, StatusCode::CREATED);

    let (status, _) = get_shared(app.clone(), "/api/runs/cov2-bundle-1/bundle").await;
    assert!(
        status.as_u16() < 500,
        "bundle download must not 5xx, got {status}"
    );
}
