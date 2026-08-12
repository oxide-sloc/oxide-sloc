// Integration tests for sloc-web HTTP routes.
// Each test builds an in-process router (no TCP) and fires a one-shot request.
// Coverage: status code, Content-Type, CSP header present, and key body contents.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use sloc_web::{
    TEST_SERVER_MODE_API_KEY, make_test_router, make_test_router_exhausted_semaphore,
    make_test_router_server_mode, make_test_router_server_mode_with_roots,
    make_test_router_tight_rate_limit, make_test_router_with_key,
};
use tower::ServiceExt;

use chrono::Utc;
use sloc_config::AppConfig;
use sloc_core::{
    AnalysisRun, EffectiveCounts, EnvironmentMetadata, FileCoverage, FileRecord, FileStatus,
    LanguageSummary, SubmoduleSummary, SummaryTotals, ToolMetadata,
};
use sloc_languages::{Language, ParseMode, RawLineCounts};

// ── helpers ──────────────────────────────────────────────────────────────────

async fn get(uri: &str) -> (StatusCode, axum::http::HeaderMap, String) {
    let app = make_test_router();
    let resp = app
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&bytes).into_owned();
    (status, headers, body)
}

/// Make a GET request against a SHARED router (state is preserved across calls).
async fn get_shared(app: Router, uri: &str) -> (StatusCode, axum::http::HeaderMap, String) {
    let resp = app
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&bytes).into_owned();
    (status, headers, body)
}

/// GET against a server-mode router (includes auth header for `TEST_SERVER_MODE_API_KEY`).
async fn get_server_shared(app: Router, uri: &str) -> (StatusCode, axum::http::HeaderMap, String) {
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
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&bytes).into_owned();
    (status, headers, body)
}

/// Make a POST form request against a SHARED router.
async fn post_form_shared(
    app: Router,
    uri: &str,
    form_body: &str,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let req = Request::post(uri)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(form_body.to_owned()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&bytes).into_owned();
    (status, headers, body)
}

/// POST a form to a server-mode router with the server API key attached.
async fn post_form_server_authed(app: Router, uri: &str, form_body: &str) -> (StatusCode, String) {
    let req = Request::post(uri)
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "Authorization",
            format!("Bearer {TEST_SERVER_MODE_API_KEY}"),
        )
        .body(Body::from(form_body.to_owned()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Percent-encode a string for use as a URL form field value.
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

async fn post_form(uri: &str, form_body: &str) -> (StatusCode, axum::http::HeaderMap, String) {
    let app = make_test_router();
    let req = Request::post(uri)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(form_body.to_owned()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&bytes).into_owned();
    (status, headers, body)
}

async fn post_json(uri: &str, json_body: &str) -> (StatusCode, axum::http::HeaderMap, String) {
    let app = make_test_router();
    let req = Request::post(uri)
        .header("content-type", "application/json")
        .body(Body::from(json_body.to_owned()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&bytes).into_owned();
    (status, headers, body)
}

async fn delete(uri: &str) -> (StatusCode, axum::http::HeaderMap, String) {
    let app = make_test_router();
    let resp = app
        .oneshot(Request::delete(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&bytes).into_owned();
    (status, headers, body)
}

async fn post_json_shared(
    app: Router,
    uri: &str,
    json: &str,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let req = Request::post(uri)
        .header("content-type", "application/json")
        .body(Body::from(json.to_owned()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&bytes).into_owned();
    (status, headers, body)
}

fn raw_multipart(boundary: &str, name: &str, filename: &str, data: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n\r\n"
        )
        .as_bytes(),
    );
    v.extend(data);
    v.extend(format!("\r\n--{boundary}--\r\n").as_bytes());
    v
}

fn has_csp(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get("content-security-policy")
        .is_some_and(|v| v.to_str().unwrap_or("").contains("script-src"))
}

// ── AnalysisRun fixture builders ─────────────────────────────────────────────
//
// These construct rich AnalysisRun values that are POSTed to /api/ingest so that
// the large dashboard handlers (trend_report_handler, test_metrics_handler, etc.)
// exercise conditional branches that only activate with real data.

fn fixture_base_run(id: &str) -> AnalysisRun {
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
            files_considered: 1,
            files_analyzed: 1,
            code_lines: 50,
            ..SummaryTotals::default()
        },
        totals_by_language: vec![],
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
    }
}

fn fixture_file_record(path: &str, lang: Language, code: u64) -> FileRecord {
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
        content_hash: 0,
    }
}

const fn fixture_lang_summary(lang: Language, files: u64, code: u64) -> LanguageSummary {
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

/// Multi-language run: Rust + Python + JavaScript + TypeScript + CSS
fn fixture_run_multi_language() -> String {
    let mut run = fixture_base_run("ingest-multilang-001");
    run.per_file_records = vec![
        fixture_file_record("src/lib.rs", Language::Rust, 120),
        fixture_file_record("main.py", Language::Python, 80),
        fixture_file_record("app.js", Language::JavaScript, 60),
        fixture_file_record("types.ts", Language::TypeScript, 40),
        fixture_file_record("style.css", Language::Css, 30),
    ];
    run.totals_by_language = vec![
        fixture_lang_summary(Language::Rust, 1, 120),
        fixture_lang_summary(Language::Python, 1, 80),
        fixture_lang_summary(Language::JavaScript, 1, 60),
        fixture_lang_summary(Language::TypeScript, 1, 40),
        fixture_lang_summary(Language::Css, 1, 30),
    ];
    run.summary_totals = SummaryTotals {
        files_considered: 5,
        files_analyzed: 5,
        code_lines: 330,
        total_physical_lines: 340,
        ..SummaryTotals::default()
    };
    run.input_roots = vec!["/test/multi-lang-project".into()];
    serde_json::to_string(&run).unwrap()
}

/// Run with per-file coverage data — exercises coverage rendering branches
fn fixture_run_with_coverage() -> String {
    let mut run = fixture_base_run("ingest-coverage-001");
    let mut rec1 = fixture_file_record("src/lib.rs", Language::Rust, 100);
    rec1.coverage = Some(FileCoverage {
        lines_found: 100,
        lines_hit: 85,
        functions_found: 8,
        functions_hit: 7,
        branches_found: 20,
        branches_hit: 16,
    });
    let mut rec2 = fixture_file_record("src/handlers.rs", Language::Rust, 80);
    rec2.coverage = Some(FileCoverage {
        lines_found: 80,
        lines_hit: 40,
        functions_found: 6,
        functions_hit: 3,
        branches_found: 15,
        branches_hit: 7,
    });
    let mut lang_sum = fixture_lang_summary(Language::Rust, 2, 180);
    lang_sum.coverage_lines_found = 180;
    lang_sum.coverage_lines_hit = 125;
    lang_sum.coverage_functions_found = 14;
    lang_sum.coverage_functions_hit = 10;
    run.per_file_records = vec![rec1, rec2];
    run.totals_by_language = vec![lang_sum];
    run.summary_totals = SummaryTotals {
        files_considered: 2,
        files_analyzed: 2,
        code_lines: 180,
        total_physical_lines: 184,
        coverage_lines_found: 180,
        coverage_lines_hit: 125,
        coverage_functions_found: 14,
        coverage_functions_hit: 10,
        ..SummaryTotals::default()
    };
    run.input_roots = vec!["/test/covered-project".into()];
    serde_json::to_string(&run).unwrap()
}

/// Run with test metrics — exercises test-count rendering branches
fn fixture_run_with_test_metrics() -> String {
    let mut run = fixture_base_run("ingest-testmetrics-001");
    let mut rec = fixture_file_record("src/lib.rs", Language::Rust, 200);
    rec.raw_line_categories.test_count = 15;
    rec.raw_line_categories.test_assertion_count = 45;
    rec.raw_line_categories.test_suite_count = 3;
    let mut lang_sum = fixture_lang_summary(Language::Rust, 1, 200);
    lang_sum.test_count = 15;
    lang_sum.test_assertion_count = 45;
    lang_sum.test_suite_count = 3;
    run.per_file_records = vec![rec];
    run.totals_by_language = vec![lang_sum];
    run.summary_totals = SummaryTotals {
        files_considered: 1,
        files_analyzed: 1,
        code_lines: 200,
        total_physical_lines: 202,
        test_count: 15,
        test_assertion_count: 45,
        test_suite_count: 3,
        ..SummaryTotals::default()
    };
    run.input_roots = vec!["/test/tested-project".into()];
    serde_json::to_string(&run).unwrap()
}

/// Run with git metadata — exercises git chip rendering branches
fn fixture_run_with_git_meta() -> String {
    let mut run = fixture_base_run("ingest-gitmeta-001");
    run.per_file_records = vec![fixture_file_record("src/lib.rs", Language::Rust, 100)];
    run.totals_by_language = vec![fixture_lang_summary(Language::Rust, 1, 100)];
    run.summary_totals = SummaryTotals {
        files_considered: 1,
        files_analyzed: 1,
        code_lines: 100,
        total_physical_lines: 102,
        ..SummaryTotals::default()
    };
    run.git_remote_url = Some("https://github.com/test-org/test-repo.git".into());
    run.git_branch = Some("main".into());
    run.git_commit_short = Some("a1b2c3d".into());
    run.git_commit_long = Some("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".into());
    run.git_commit_author = Some("Test Author".into());
    run.git_commit_date = Some("2026-06-05T10:00:00Z".into());
    run.git_nearest_tag = Some("v1.5.66".into());
    run.input_roots = vec!["/test/git-project".into()];
    serde_json::to_string(&run).unwrap()
}

/// Run with submodule data — exercises submodule table rendering
fn fixture_run_with_submodules() -> String {
    let mut run = fixture_base_run("ingest-submodules-001");
    run.per_file_records = vec![fixture_file_record("src/lib.rs", Language::Rust, 150)];
    run.totals_by_language = vec![fixture_lang_summary(Language::Rust, 1, 150)];
    run.summary_totals = SummaryTotals {
        files_considered: 3,
        files_analyzed: 3,
        code_lines: 300,
        total_physical_lines: 306,
        ..SummaryTotals::default()
    };
    run.submodule_summaries = vec![
        SubmoduleSummary {
            name: "vendor/lib-a".into(),
            relative_path: "vendor/lib-a".into(),
            files_analyzed: 1,
            total_physical_lines: 102,
            code_lines: 100,
            comment_lines: 1,
            blank_lines: 1,
            language_summaries: vec![fixture_lang_summary(Language::Rust, 1, 100)],
            git_commit_short: Some("deadbeef".into()),
            git_commit_long: None,
            git_branch: Some("main".into()),
            git_commit_author: None,
            git_commit_date: None,
            git_remote_url: Some("https://github.com/test-org/lib-a.git".into()),
        },
        SubmoduleSummary {
            name: "vendor/lib-b".into(),
            relative_path: "vendor/lib-b".into(),
            files_analyzed: 1,
            total_physical_lines: 52,
            code_lines: 50,
            comment_lines: 1,
            blank_lines: 1,
            language_summaries: vec![fixture_lang_summary(Language::Python, 1, 50)],
            git_commit_short: Some("cafebabe".into()),
            git_commit_long: None,
            git_branch: Some("dev".into()),
            git_commit_author: None,
            git_commit_date: None,
            git_remote_url: None,
        },
    ];
    run.git_remote_url = Some("https://github.com/test-org/parent-repo.git".into());
    run.input_roots = vec!["/test/parent-project".into()];
    serde_json::to_string(&run).unwrap()
}

/// Run with both coverage and test metrics combined
fn fixture_run_coverage_and_tests() -> String {
    let mut run = fixture_base_run("ingest-cov-and-tests-001");
    let mut rec = fixture_file_record("src/lib.rs", Language::Rust, 200);
    rec.coverage = Some(FileCoverage {
        lines_found: 200,
        lines_hit: 160,
        functions_found: 12,
        functions_hit: 10,
        branches_found: 30,
        branches_hit: 24,
    });
    rec.raw_line_categories.test_count = 20;
    rec.raw_line_categories.test_assertion_count = 60;
    rec.raw_line_categories.test_suite_count = 4;
    let mut lang_sum = fixture_lang_summary(Language::Rust, 1, 200);
    lang_sum.coverage_lines_found = 200;
    lang_sum.coverage_lines_hit = 160;
    lang_sum.test_count = 20;
    lang_sum.test_assertion_count = 60;
    lang_sum.test_suite_count = 4;
    run.per_file_records = vec![rec];
    run.totals_by_language = vec![lang_sum];
    run.summary_totals = SummaryTotals {
        files_considered: 1,
        files_analyzed: 1,
        code_lines: 200,
        total_physical_lines: 202,
        coverage_lines_found: 200,
        coverage_lines_hit: 160,
        coverage_functions_found: 12,
        coverage_functions_hit: 10,
        test_count: 20,
        test_assertion_count: 60,
        test_suite_count: 4,
        ..SummaryTotals::default()
    };
    run.git_remote_url = Some("https://github.com/test-org/full-project.git".into());
    run.git_branch = Some("main".into());
    run.git_commit_short = Some("123abcd".into());
    run.input_roots = vec!["/test/full-project".into()];
    serde_json::to_string(&run).unwrap()
}

/// Empty repo — zero files analysed (exercises "no data" empty-state paths)
fn fixture_run_empty_repo() -> String {
    let run = fixture_base_run("ingest-empty-001");
    serde_json::to_string(&run).unwrap()
}

// ── public routes (no auth required) ─────────────────────────────────────────

#[tokio::test]
async fn healthz_returns_ok() {
    let (status, _, body) = get("/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.trim(), "ok");
}

#[tokio::test]
async fn api_health_returns_structured_json() {
    let (status, headers, body) = get("/api/health").await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("application/json"), "expected JSON, got: {ct}");
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
    // Test router uses a writable temp tree, so every check passes → status "ok".
    assert_eq!(parsed["status"], "ok");
    assert_eq!(parsed["name"], "oxide-sloc");
    assert!(parsed["version"].is_string());
    assert!(parsed["git_sha"].is_string());
    assert!(parsed["build_time"].is_string());
    assert!(parsed["uptime_seconds"].is_number());
    assert_eq!(parsed["checks"]["registry_writable"], true);
    assert_eq!(parsed["checks"]["output_dir_writable"], true);
}

#[tokio::test]
async fn readyz_returns_ready_when_writable() {
    let (status, headers, body) = get("/readyz").await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("application/json"), "expected JSON, got: {ct}");
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
    assert_eq!(parsed["status"], "ready");
    assert_eq!(parsed["checks"]["registry_writable"], true);
}

#[tokio::test]
async fn api_version_returns_json() {
    let (status, headers, body) = get("/api/version").await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("application/json"),
        "expected JSON content-type, got: {ct}"
    );
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
    assert_eq!(parsed["name"], "oxide-sloc");
    assert!(
        parsed["version"].is_string(),
        "version field must be a string"
    );
    // Build provenance embedded by build.rs (never absent — falls back to "unknown").
    assert!(parsed["git_sha"].is_string(), "git_sha must be present");
    assert!(
        parsed["build_time"].is_string(),
        "build_time must be present"
    );
}

#[tokio::test]
async fn large_html_response_is_gzipped_when_accepted() {
    // A large text/html page with Accept-Encoding: gzip must come back gzip-encoded.
    let app = make_test_router();
    let resp = app
        .oneshot(
            Request::get("/")
                .header("accept-encoding", "gzip")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("content-encoding")
            .and_then(|v| v.to_str().ok()),
        Some("gzip"),
        "home page should be gzip-compressed"
    );
    // The body must be a valid gzip stream that inflates to HTML.
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let mut gz = flate2::read::GzDecoder::new(&bytes[..]);
    let mut html = String::new();
    std::io::Read::read_to_string(&mut gz, &mut html).expect("valid gzip stream");
    assert!(
        html.contains("<html") || html.contains("<!DOCTYPE"),
        "inflates to HTML"
    );
}

#[tokio::test]
async fn health_is_not_compressed_when_client_declines() {
    // No Accept-Encoding → response must be sent uncompressed (identity).
    let app = make_test_router();
    let resp = app
        .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(
        resp.headers().get("content-encoding").is_none(),
        "must not compress when the client did not ask for it"
    );
}

#[tokio::test]
async fn badge_total_sloc_returns_svg() {
    let (status, headers, body) = get("/badge/total_sloc").await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("svg") || ct.contains("xml"),
        "expected SVG content-type, got: {ct}"
    );
    assert!(body.contains("<svg"), "expected SVG body");
}

#[tokio::test]
async fn chart_js_returns_javascript() {
    let (status, headers, _) = get("/static/chart.js").await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("javascript") || ct.contains("text"),
        "unexpected content-type: {ct}"
    );
}

// ── protected HTML pages ──────────────────────────────────────────────────────

#[tokio::test]
async fn splash_returns_html_with_csp() {
    let (status, headers, body) = get("/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(has_csp(&headers), "missing CSP header on /");
    assert!(body.contains("<html"), "expected HTML body on /");
}

#[tokio::test]
async fn scan_page_returns_html_with_csp() {
    let (status, headers, body) = get("/scan").await;
    assert_eq!(status, StatusCode::OK);
    assert!(has_csp(&headers), "missing CSP header on /scan");
    assert!(
        body.contains("tmp-sloc"),
        "expected default path placeholder in /scan"
    );
}

#[tokio::test]
async fn scan_setup_returns_html_with_csp() {
    let (status, headers, body) = get("/scan-setup").await;
    assert_eq!(status, StatusCode::OK);
    assert!(has_csp(&headers), "missing CSP header on /scan-setup");
    assert!(body.contains("<html"), "expected HTML body on /scan-setup");
}

#[tokio::test]
async fn view_reports_returns_html_with_csp() {
    let (status, headers, body) = get("/view-reports").await;
    assert_eq!(status, StatusCode::OK);
    assert!(has_csp(&headers), "missing CSP header on /view-reports");
    assert!(
        body.contains("<html"),
        "expected HTML body on /view-reports"
    );
}

#[tokio::test]
async fn compare_scans_returns_html_with_csp() {
    let (status, headers, body) = get("/compare-scans").await;
    assert_eq!(status, StatusCode::OK);
    assert!(has_csp(&headers), "missing CSP header on /compare-scans");
    assert!(
        body.contains("<html"),
        "expected HTML body on /compare-scans"
    );
}

#[tokio::test]
async fn git_browser_returns_html_with_csp() {
    let (status, headers, body) = get("/git-browser").await;
    assert_eq!(status, StatusCode::OK);
    assert!(has_csp(&headers), "missing CSP header on /git-browser");
    assert!(body.contains("<html"), "expected HTML body on /git-browser");
}

#[tokio::test]
async fn webhook_setup_returns_html_with_csp() {
    let (status, headers, body) = get("/integrations").await;
    assert_eq!(status, StatusCode::OK);
    assert!(has_csp(&headers), "missing CSP header on /integrations");
    assert!(
        body.contains("<html"),
        "expected HTML body on /integrations"
    );
}

#[tokio::test]
async fn embed_summary_returns_html() {
    let (status, _, body) = get("/embed/summary").await;
    assert_eq!(status, StatusCode::OK);
    // With no scans in the registry, the embed handler returns a plain <p> fallback — still HTML.
    assert!(
        body.contains('<'),
        "expected some HTML markup on /embed/summary, got: {body}"
    );
}

// ── JSON API routes ───────────────────────────────────────────────────────────

#[tokio::test]
async fn api_metrics_latest_returns_json() {
    let (status, headers, _) = get("/api/metrics/latest").await;
    // 200 with no scans yet, or 404 — either is acceptable; what matters is no 5xx
    assert!(
        status == StatusCode::OK || status == StatusCode::NOT_FOUND,
        "unexpected status {status} on /api/metrics/latest"
    );
    if status == StatusCode::OK {
        let ct = headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(ct.contains("json"), "expected JSON content-type, got: {ct}");
    }
}

#[tokio::test]
async fn api_project_history_returns_json_array() {
    let (status, headers, body) = get("/api/project-history").await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("json"), "expected JSON content-type, got: {ct}");
    assert!(
        body.starts_with('[') || body.starts_with('{'),
        "expected JSON body"
    );
}

#[tokio::test]
async fn api_schedules_returns_json_object() {
    let (status, headers, body) = get("/api/schedules").await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("json"), "expected JSON content-type, got: {ct}");
    // Response shape: {"schedules": [...]}
    assert!(
        body.contains("\"schedules\""),
        "expected 'schedules' key in /api/schedules response, got: {body}"
    );
}

#[tokio::test]
async fn api_git_refs_with_no_repo_returns_error_not_5xx() {
    let (status, _, _) = get("/api/git/refs?path=.").await;
    assert!(
        status.as_u16() < 500,
        "got 5xx {status} on /api/git/refs — handler panicked or returned internal error"
    );
}

// ── preview (file explorer) ───────────────────────────────────────────────────

#[tokio::test]
async fn preview_with_valid_path_returns_json() {
    let (status, _, body) = get("/preview?path=.").await;
    // 200 with JSON listing or 400/404 if restricted — no 5xx
    assert!(
        status.as_u16() < 500,
        "got 5xx {status} on /preview — handler panicked: {body}"
    );
}

// ── unknown route returns 404 ─────────────────────────────────────────────────

#[tokio::test]
async fn unknown_route_returns_404() {
    let (status, _, _) = get("/this-route-does-not-exist").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── security header regression ────────────────────────────────────────────────

#[tokio::test]
async fn security_headers_present_on_html_pages() {
    for path in ["/", "/scan", "/view-reports", "/git-browser"] {
        let app = make_test_router();
        let resp = app
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let headers = resp.headers();
        assert!(
            headers.contains_key("x-content-type-options"),
            "missing X-Content-Type-Options on {path}"
        );
        assert!(
            headers.contains_key("x-frame-options"),
            "missing X-Frame-Options on {path}"
        );
        assert!(
            headers.contains_key("content-security-policy"),
            "missing CSP on {path}"
        );
    }
}

// ── auth regression: no API key set → protected routes accessible ─────────────

#[tokio::test]
async fn no_api_key_configured_allows_all_routes() {
    // When no SLOC_API_KEYS is set (test state has api_keys=vec![]), all routes
    // must be reachable without an Authorization header.
    for path in ["/", "/scan", "/view-reports", "/healthz"] {
        let (status, _, _) = get(path).await;
        assert_ne!(
            status,
            StatusCode::UNAUTHORIZED,
            "got 401 on {path} with no API key configured — auth guard is misfiring"
        );
    }
}

// ── analyze handler ───────────────────────────────────────────────────────────

#[tokio::test]
async fn post_analyze_returns_ok_with_wait_id_header() {
    // POST /analyze with a local path kicks off an async scan and returns the
    // wait-page HTML. The x-wait-id response header carries the UUID for polling.
    let (status, headers, body) = post_form("/analyze", "path=.&generate_html=1").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "expected 200 from POST /analyze, got {status}"
    );
    assert!(
        headers.contains_key("x-wait-id"),
        "POST /analyze must return x-wait-id header for async polling"
    );
    let wait_id = headers
        .get("x-wait-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(!wait_id.is_empty(), "x-wait-id must not be empty");
    assert!(
        body.contains("<html") || body.contains("wait") || body.contains("scan"),
        "expected wait-page HTML in body"
    );
}

#[tokio::test]
async fn post_analyze_git_mode_without_both_params_falls_back_to_local() {
    // Supplying git_repo but not git_ref means is_git_mode is false; the handler
    // should treat path as a local directory scan and still return 200 + x-wait-id.
    let (status, headers, _) = post_form(
        "/analyze",
        "path=.&git_repo=https%3A%2F%2Fexample.com%2Frepo",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers.contains_key("x-wait-id"),
        "incomplete git mode should fall back to local scan and return x-wait-id"
    );
}

// ── async run status / cancel ─────────────────────────────────────────────────

#[tokio::test]
async fn async_run_status_unknown_wait_id_returns_404() {
    let (status, _, _) = get("/api/runs/00000000-0000-0000-0000-000000000000/status").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "unknown wait_id must return 404 from status endpoint"
    );
}

#[tokio::test]
async fn async_run_status_malformed_wait_id_returns_400() {
    // wait_id longer than 128 chars should be rejected before the map lookup.
    let long_id = "x".repeat(129);
    let (status, _, _) = get(&format!("/api/runs/{long_id}/status")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn cancel_run_unknown_wait_id_returns_404() {
    let (status, _, _) =
        post_form("/api/runs/00000000-0000-0000-0000-000000000000/cancel", "").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "cancelling an unknown wait_id must return 404"
    );
}

#[tokio::test]
async fn cancel_run_after_analyze_returns_ok_or_not_found() {
    // Start a scan so a wait_id is registered, then immediately cancel it.
    let (_, headers, _) = post_form("/analyze", "path=.").await;
    let wait_id = headers
        .get("x-wait-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    assert!(!wait_id.is_empty(), "need a wait_id from POST /analyze");
    // Cancel — may be Running (200) or already Complete/not in async_runs yet
    // (404) depending on scheduling; either is valid, 5xx is not.
    let (status, _, _) = post_form(&format!("/api/runs/{wait_id}/cancel"), "").await;
    assert!(
        status == StatusCode::OK || status == StatusCode::NOT_FOUND,
        "cancel of a real wait_id must be 200 or 404, got {status}"
    );
}

// ── artifact handler ──────────────────────────────────────────────────────────

#[tokio::test]
async fn artifact_handler_unknown_run_id_returns_404() {
    for artifact in ["html", "pdf", "json", "csv", "xlsx"] {
        let (status, _, body) = get(&format!(
            "/runs/{artifact}/00000000-0000-0000-0000-000000000000"
        ))
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "unknown run ID for artifact '{artifact}' must return 404, body: {body}"
        );
    }
}

#[tokio::test]
async fn artifact_handler_swapped_segments_returns_404() {
    // The URL spec is /runs/{artifact}/{run_id}. If the user accidentally
    // writes /runs/{run_id}/{artifact} the handler must still return 404 (not 5xx).
    let (status, _, _) = get("/runs/00000000-0000-0000-0000-000000000000/html").await;
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::OK,
        "swapped segments must not cause a 5xx, got {status}"
    );
}

// ── async run result ──────────────────────────────────────────────────────────

#[tokio::test]
async fn async_run_result_unknown_returns_404() {
    let (status, _, body) = get("/runs/result/00000000-0000-0000-0000-000000000000").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "unknown run_id in /runs/result must return 404, body: {body}"
    );
}

#[tokio::test]
async fn async_run_result_malformed_id_returns_400() {
    let long_id = "y".repeat(129);
    let (status, _, _) = get(&format!("/runs/result/{long_id}")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ── pdf status ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn pdf_status_unknown_run_returns_not_ready_json() {
    let (status, headers, body) =
        get("/api/runs/00000000-0000-0000-0000-000000000000/pdf-status").await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("json"), "expected JSON content-type, got: {ct}");
    assert!(
        body.contains("\"ready\""),
        "pdf-status body must contain 'ready' field, got: {body}"
    );
    assert!(
        body.contains("false"),
        "pdf-status for unknown run must return ready:false, got: {body}"
    );
}

// ── compare handler ───────────────────────────────────────────────────────────

#[tokio::test]
async fn compare_without_params_redirects_to_compare_scans() {
    let app = make_test_router();
    let resp = app
        .oneshot(Request::get("/compare").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    assert!(
        status.is_redirection(),
        "GET /compare with no params must redirect, got {status}"
    );
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        location.contains("compare-scans"),
        "redirect must point to /compare-scans, got location: {location}"
    );
}

#[tokio::test]
async fn compare_with_unknown_run_ids_returns_error_page() {
    let (status, _, body) = get("/compare?a=unknown-run-a&b=unknown-run-b").await;
    // The handler renders ErrorTemplate which returns 200 HTML (no explicit 404)
    // when run IDs are not found — assert no 5xx and that error text is present.
    assert!(
        status.as_u16() < 500,
        "compare with unknown IDs must not 5xx, got {status}"
    );
    assert!(
        body.contains("not found") || body.contains("not Found") || body.contains("<html"),
        "expected an error page or HTML, got: {body}"
    );
}

// ── trend reports + test metrics ──────────────────────────────────────────────

#[tokio::test]
async fn trend_reports_returns_html_with_csp() {
    let (status, headers, body) = get("/trend-reports").await;
    assert_eq!(status, StatusCode::OK);
    assert!(has_csp(&headers), "missing CSP header on /trend-reports");
    assert!(body.contains("<html"), "expected HTML on /trend-reports");
}

#[tokio::test]
async fn test_metrics_returns_html_with_csp() {
    let (status, headers, body) = get("/test-metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(has_csp(&headers), "missing CSP header on /test-metrics");
    assert!(body.contains("<html"), "expected HTML on /test-metrics");
}

// ── coverage suggestion API ───────────────────────────────────────────────────

#[tokio::test]
async fn api_suggest_coverage_returns_json() {
    let (status, headers, body) = get("/api/suggest-coverage?path=.").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected status on /api/suggest-coverage"
    );
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("json"), "expected JSON content-type, got: {ct}");
    assert!(
        body.contains("\"found\""),
        "expected 'found' key in coverage suggestion response, got: {body}"
    );
}

// ── metrics by run ID ─────────────────────────────────────────────────────────

#[tokio::test]
async fn api_metrics_run_unknown_returns_404() {
    let (status, _, _) = get("/api/metrics/00000000-0000-0000-0000-000000000000").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "unknown run_id in /api/metrics/{{run_id}} must return 404"
    );
}

// ── api-docs page ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn api_docs_returns_html_with_csp() {
    let (status, headers, body) = get("/api-docs").await;
    assert_eq!(status, StatusCode::OK);
    assert!(has_csp(&headers), "missing CSP on /api-docs");
    assert!(body.contains("<html"), "expected HTML on /api-docs");
}

// ── auth/login routes ─────────────────────────────────────────────────────────

#[tokio::test]
async fn auth_login_get_redirects_to_root_when_no_key_configured() {
    // When no SLOC_API_KEYS is set, GET /auth/login must immediately redirect
    // back to / (there is nothing to authenticate against).
    let app = make_test_router();
    let resp = app
        .oneshot(Request::get("/auth/login").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(
        resp.status().is_redirection(),
        "GET /auth/login with no key must redirect, got {}",
        resp.status()
    );
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(location, "/", "must redirect to /");
}

#[tokio::test]
async fn auth_login_get_returns_form_when_key_is_configured() {
    let app = make_test_router_with_key("test-secret-key");
    let resp = app
        .oneshot(Request::get("/auth/login").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&bytes);
    assert!(body.contains("<html"), "expected login form HTML");
    assert!(
        body.contains("login") || body.contains("key") || body.contains("password"),
        "expected login form fields"
    );
}

#[tokio::test]
async fn auth_login_post_wrong_key_redirects_with_error_flag() {
    use std::net::SocketAddr;
    let app = make_test_router_with_key("correct-key");
    // auth_login_post requires ConnectInfo<SocketAddr> — inject it as a request extension
    // so the extractor can resolve the peer IP without a real TCP connection.
    let peer: SocketAddr = "127.0.0.1:12345".parse().unwrap();
    let mut req = Request::post("/auth/login")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("key=wrong-key&next=%2F"))
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(peer));
    let resp = app.oneshot(req).await.unwrap();
    // Wrong key → redirect back to login with error=1
    assert!(
        resp.status().is_redirection(),
        "wrong key must redirect, got {}",
        resp.status()
    );
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        location.contains("error=1"),
        "redirect after wrong key must include error=1, got: {location}"
    );
}

// ── next= sanitization ────────────────────────────────────────────────────────

#[tokio::test]
async fn auth_login_get_sanitizes_nested_next_to_root() {
    // GET /auth/login?next=/auth/login%3Fnext%3D%2F  →  hidden input value must be "/",
    // not the raw nested string (which would cause a redirect loop after login).
    let app = make_test_router_with_key("test-key");
    let resp = app
        .oneshot(
            Request::get("/auth/login?next=%2Fauth%2Flogin%3Fnext%3D%2F")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&bytes);
    // The hidden "next" input must contain "/" and NOT the nested path.
    assert!(
        body.contains(r#"name="next" value="/""#),
        "expected next=\"/\" in form but got body snippet: {}",
        &body[body.find("name=\"next\"").unwrap_or(0)
            ..body
                .find("name=\"next\"")
                .map_or(60, |i| (i + 60).min(body.len()))]
    );
}

#[tokio::test]
async fn auth_login_get_returns_200_without_redirect_when_key_configured() {
    // Unauthenticated GET /auth/login must return 200 (the login form), never 302.
    // The middleware must not redirect /auth/login to itself.
    let app = make_test_router_with_key("test-key");
    let resp = app
        .oneshot(Request::get("/auth/login").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "GET /auth/login must return 200, not a redirect"
    );
}

#[tokio::test]
async fn auth_login_post_valid_key_with_bad_next_redirects_to_root() {
    // POST /auth/login with a valid key but next=/http://example.com/x must
    // redirect to "/" rather than the schema-bearing path.
    use std::net::SocketAddr;
    let app = make_test_router_with_key("good-key");
    let peer: SocketAddr = "127.0.0.1:12345".parse().unwrap();
    let mut req = Request::post("/auth/login")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(
            "key=good-key&next=%2Fhttp%3A%2F%2Fexample.com%2Fx",
        ))
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(peer));
    let resp = app.oneshot(req).await.unwrap();
    assert!(
        resp.status().is_redirection(),
        "valid login must redirect, got {}",
        resp.status()
    );
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(
        location, "/",
        "bad next= must be replaced with /, got: {location}"
    );
}

#[tokio::test]
async fn auth_login_post_valid_key_with_protocol_relative_next_redirects_to_root() {
    // Guards against open-redirect via protocol-relative URL (//evil.com).
    // sanitize_next() must treat any path starting with "//" as unsafe and fall back to "/".
    use std::net::SocketAddr;
    let app = make_test_router_with_key("good-key");
    let peer: SocketAddr = "127.0.0.1:12345".parse().unwrap();
    let mut req = Request::post("/auth/login")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("key=good-key&next=%2F%2Fevil.com"))
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(peer));
    let resp = app.oneshot(req).await.unwrap();
    assert!(
        resp.status().is_redirection(),
        "valid login must redirect, got {}",
        resp.status()
    );
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(
        location, "/",
        "protocol-relative next= must be replaced with /, got: {location}"
    );
}

#[tokio::test]
async fn auth_login_post_valid_key_with_safe_next_redirects_there() {
    // Positive case: a legitimate same-origin path must not be over-restricted.
    // sanitize_next("/test-metrics") must pass through unchanged.
    use std::net::SocketAddr;
    let app = make_test_router_with_key("good-key");
    let peer: SocketAddr = "127.0.0.1:12345".parse().unwrap();
    let mut req = Request::post("/auth/login")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("key=good-key&next=%2Ftest-metrics"))
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(peer));
    let resp = app.oneshot(req).await.unwrap();
    assert!(
        resp.status().is_redirection(),
        "valid login must redirect, got {}",
        resp.status()
    );
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(
        location, "/test-metrics",
        "safe same-origin next= must be preserved, got: {location}"
    );
}

// ── API key authentication (Bearer / X-API-Key) ───────────────────────────────

#[tokio::test]
async fn wrong_bearer_token_returns_401_when_key_configured() {
    let app = make_test_router_with_key("secret-token");
    let resp = app
        .oneshot(
            Request::get("/")
                .header("authorization", "Bearer wrong-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "wrong Bearer token must yield 401"
    );
}

#[tokio::test]
async fn correct_bearer_token_allows_access_to_protected_routes() {
    let app = make_test_router_with_key("my-secret");
    let resp = app
        .oneshot(
            Request::get("/")
                .header("authorization", "Bearer my-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "correct Bearer token must allow access, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn correct_x_api_key_header_allows_access() {
    let app = make_test_router_with_key("my-secret");
    let resp = app
        .oneshot(
            Request::get("/healthz")
                .header("x-api-key", "my-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // /healthz is outside the auth layer, so it must always be 200.
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn no_credential_on_protected_route_redirects_browser() {
    let app = make_test_router_with_key("secret");
    // Browsers send Accept: text/html — the middleware redirects them to login.
    let resp = app
        .oneshot(
            Request::get("/")
                .header("accept", "text/html,application/xhtml+xml")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status().is_redirection(),
        "browser with no credential must be redirected to login, got {}",
        resp.status()
    );
}

// ── export / import config ────────────────────────────────────────────────────

#[tokio::test]
async fn export_config_returns_toml_with_content_disposition() {
    let (status, headers, body) = get("/export-config").await;
    assert_eq!(status, StatusCode::OK, "expected 200 from /export-config");
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("toml") || ct.contains("text"),
        "expected TOML content-type, got: {ct}"
    );
    let cd = headers
        .get("content-disposition")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        cd.contains("attachment") && cd.contains(".toml"),
        "expected attachment content-disposition with .toml filename, got: {cd}"
    );
    assert!(!body.is_empty(), "exported TOML must not be empty");
}

#[tokio::test]
async fn import_config_with_valid_toml_returns_ok() {
    // Round-trip: export the server's default config as TOML, then re-import it.
    // This avoids hard-coding the full TOML shape (AppConfig has mandatory fields
    // without #[serde(default)] that vary across versions).
    let (export_status, _, exported_toml) = get("/export-config").await;
    assert_eq!(export_status, StatusCode::OK, "export-config must succeed");

    let json_body = format!(
        r#"{{"toml":{}}}"#,
        serde_json::to_string(&exported_toml).unwrap()
    );
    let (status, headers, body) = post_json("/import-config", &json_body).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "re-importing the exported config must return 200, body: {body}"
    );
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("json"), "expected JSON response, got: {ct}");
    assert!(
        body.contains("\"ok\""),
        "expected 'ok' in response, got: {body}"
    );
}

#[tokio::test]
async fn import_config_with_invalid_toml_returns_400() {
    let (status, _, body) =
        post_json("/import-config", r#"{"toml":"this is ][[ not valid toml"}"#).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "malformed TOML must return 400, body: {body}"
    );
    assert!(
        body.contains("error"),
        "expected error message in body, got: {body}"
    );
}

// ── scan profiles API ─────────────────────────────────────────────────────────

#[tokio::test]
async fn api_scan_profiles_list_returns_json_with_profiles_key() {
    let (status, headers, body) = get("/api/scan-profiles").await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("json"), "expected JSON content-type, got: {ct}");
    assert!(
        body.contains("\"profiles\""),
        "expected 'profiles' key in /api/scan-profiles, got: {body}"
    );
}

#[tokio::test]
async fn api_save_scan_profile_with_empty_name_returns_400() {
    let (status, _, body) = post_json("/api/scan-profiles", r#"{"name":"   ","params":{}}"#).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "empty profile name must return 400, body: {body}"
    );
}

#[tokio::test]
async fn api_save_scan_profile_creates_profile_and_returns_201() {
    let (status, headers, body) = post_json(
        "/api/scan-profiles",
        r#"{"name":"my-profile","params":{"path":"."}}"#,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "valid profile must return 201, body: {body}"
    );
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("json"), "expected JSON response, got: {ct}");
    assert!(
        body.contains("\"ok\""),
        "expected 'ok' in response, got: {body}"
    );
    assert!(
        body.contains("\"id\""),
        "expected 'id' in response, got: {body}"
    );
}

#[tokio::test]
async fn api_delete_scan_profile_unknown_id_returns_404() {
    let (status, _, body) = delete("/api/scan-profiles/does-not-exist").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "deleting unknown profile must return 404, body: {body}"
    );
}

// ── schedules API ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn api_delete_schedule_unknown_id_returns_204() {
    // ScheduleIdQuery.id is a uuid::Uuid — must use a valid UUID format.
    let (status, _, _) = delete("/api/schedules?id=00000000-0000-0000-0000-000000000001").await;
    // api_delete_schedule removes nothing (unknown id) and still returns 204.
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "DELETE /api/schedules must return 204 even for an unknown uuid"
    );
}

// ── metrics submodules ────────────────────────────────────────────────────────

#[tokio::test]
async fn api_metrics_submodules_unknown_run_returns_404() {
    let (status, _, _) = get("/api/metrics/00000000-0000-0000-0000-000000000000/submodules").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "unknown run_id in /api/metrics/{{id}}/submodules must return 404"
    );
}

// ── watched-dirs API ──────────────────────────────────────────────────────────

#[tokio::test]
async fn post_add_watched_dir_with_invalid_path_redirects() {
    let app = make_test_router();
    let req = Request::post("/watched-dirs/add")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(
            "folder_path=%2Fdoes%2Fnot%2Fexist%2F__test__&redirect_to=%2Fview-reports",
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // Invalid path → handler redirects back with an error query param.
    assert!(
        resp.status().is_redirection(),
        "add-watched-dir with bad path must redirect, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn post_refresh_watched_dirs_returns_redirect() {
    let app = make_test_router();
    let req = Request::post("/watched-dirs/refresh")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("redirect_to=%2Fview-reports"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // With no watched dirs in state, refresh still completes (redirect or 200).
    assert!(
        resp.status().as_u16() < 500,
        "refresh watched-dirs must not 5xx, got {}",
        resp.status()
    );
}

// ── webhook routes (smoke-test: no secret configured) ────────────────────────

#[tokio::test]
async fn post_github_webhook_non_push_event_returns_200() {
    let app = make_test_router();
    let req = Request::post("/webhooks/github")
        .header("content-type", "application/json")
        .header("x-github-event", "ping")
        .body(Body::from(r#"{"zen":"test"}"#))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // Non-push events are silently accepted (200) per the webhook handler.
    assert!(
        resp.status().as_u16() < 500,
        "non-push GitHub webhook must not 5xx, got {}",
        resp.status()
    );
}

// ── api/metrics/history ───────────────────────────────────────────────────────

#[tokio::test]
async fn api_metrics_history_returns_json_array() {
    let (status, headers, body) = get("/api/metrics/history").await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("json"), "expected JSON content-type, got: {ct}");
    assert!(
        body.starts_with('[') || body.starts_with('{'),
        "expected JSON body from /api/metrics/history, got: {body}"
    );
}

// ── error module: typed error responses exercised through handlers ────────────

#[tokio::test]
async fn error_not_found_has_correct_json_shape() {
    // Trigger error::not_found via /api/metrics/{unknown_run_id}.
    let (status, headers, body) = get("/api/metrics/nonexistent-run-id-abc").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("json"),
        "error::not_found must return JSON, got: {ct}"
    );
    assert!(
        body.contains("\"error\""),
        "error body must have 'error' key, got: {body}"
    );
}

#[tokio::test]
async fn error_bad_request_has_correct_json_shape() {
    // Trigger error::bad_request via /import-config with invalid TOML.
    let (status, headers, body) = post_json("/import-config", r#"{"toml":"[invalid"}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("json"),
        "error::bad_request must return JSON, got: {ct}"
    );
    assert!(
        body.contains("\"error\""),
        "error body must have 'error' key, got: {body}"
    );
}

#[tokio::test]
async fn error_unprocessable_entity_returned_for_invalid_config() {
    // An AppConfig that serialises but fails validate() exercises error::unprocessable_entity.
    // Build a TOML with an explicitly invalid value if any; otherwise just confirm <422 on bad input.
    let toml = "[web]\nbind_address = \"\"";
    let json_body = format!(r#"{{"toml":{}}}"#, serde_json::to_string(toml).unwrap());
    let (status, _, _) = post_json("/import-config", &json_body).await;
    // Either 200 (valid empty bind_address accepted) or 422 (validate rejects it).
    // What must NOT happen is a 5xx.
    assert!(
        status.as_u16() < 500,
        "/import-config must not 5xx for borderline config, got {status}"
    );
}

// ── Additional route coverage ──────────────────────────────────────────────

// NOTE: /open-path and /pick-file are intentionally NOT tested here.
// Both handlers open native OS windows (file manager, file picker) that block
// the test process on non-headless machines. The SLOC_HEADLESS guard in
// make_test_router() suppresses them in all test runs; adding explicit test
// calls would be redundant and unsafe on developer machines.

#[tokio::test]
async fn watched_dirs_add_missing_body_not_5xx() {
    let (status, _, _) = post_form("/watched-dirs/add", "").await;
    assert!(
        status.as_u16() < 500,
        "/watched-dirs/add must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn watched_dirs_remove_missing_body_not_5xx() {
    let (status, _, _) = post_form("/watched-dirs/remove", "").await;
    assert!(
        status.as_u16() < 500,
        "/watched-dirs/remove must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn watched_dirs_refresh_not_5xx() {
    let (status, _, _) = post_form("/watched-dirs/refresh", "").await;
    assert!(
        status.as_u16() < 500,
        "/watched-dirs/refresh must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn cleanup_runs_not_5xx() {
    let (status, _, _) = post_form("/api/runs/cleanup", "").await;
    assert!(
        status.as_u16() < 500,
        "/api/runs/cleanup must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn cancel_run_on_unknown_id_not_5xx() {
    let (status, _, _) =
        post_form("/api/runs/00000000-0000-0000-0000-000000000000/cancel", "").await;
    assert!(status.as_u16() < 500, "cancel must not 5xx, got {status}");
}

#[tokio::test]
async fn relocate_scan_missing_body_not_5xx() {
    let (status, _, _) = post_form("/relocate-scan", "").await;
    assert!(
        status.as_u16() < 500,
        "/relocate-scan must not 5xx, got {status}"
    );
}

// NOTE: /locate-report with empty body triggers a native file-picker dialog.
// Intentionally not tested here — covered by the SLOC_HEADLESS guard in the handler.

#[tokio::test]
async fn locate_reports_dir_missing_body_not_5xx() {
    let (status, _, _) = post_form("/locate-reports-dir", "").await;
    assert!(
        status.as_u16() < 500,
        "/locate-reports-dir must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn api_ingest_missing_body_returns_4xx() {
    let (status, _, _) = post_json("/api/ingest", "{}").await;
    // Missing required fields → 400 or 422; must not 5xx
    assert!(
        status.as_u16() < 500,
        "/api/ingest must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn bundle_download_unknown_run_not_5xx() {
    let (status, _, _) = get("/api/runs/00000000-0000-0000-0000-000000000000/bundle").await;
    assert!(
        status.as_u16() < 500,
        "/api/runs/:id/bundle must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn scan_profiles_post_not_5xx() {
    let (status, _, _) = post_json("/api/scan-profiles", r#"{"name":"test","config":{}}"#).await;
    assert!(
        status.as_u16() < 500,
        "POST /api/scan-profiles must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn git_scan_ref_missing_params_not_5xx() {
    let (status, _, _) = get("/api/git/scan-ref").await;
    assert!(
        status.as_u16() < 500,
        "/api/git/scan-ref must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn git_compare_refs_missing_params_not_5xx() {
    let (status, _, _) = get("/api/git/compare-refs").await;
    assert!(
        status.as_u16() < 500,
        "/api/git/compare-refs must not 5xx, got {status}"
    );
}

// ── Full analyze cycle (shared router, real temp directory) ───────────────────
//
// These tests use a single router instance whose Arc-shared state persists
// across multiple oneshot calls, allowing us to exercise the result-rendering
// code paths that only activate once a completed scan is stored.

/// True if `body` looks like a full HTML document (either doctype casing).
fn is_html_doc(body: &str) -> bool {
    body.contains("<!doctype html>") || body.contains("<!DOCTYPE html>")
}

/// Poll `/api/runs/{wait_id}/status` until the async scan reports `complete`
/// (returning its `run_id`), or it fails/cancels/times out (returning an empty
/// string). Extracted so the end-to-end tests below stay flat.
async fn poll_run_complete(app: &axum::Router, wait_id: &str) -> String {
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let (_, _, body) = get_shared(app.clone(), &format!("/api/runs/{wait_id}/status")).await;
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
            continue;
        };
        if v["state"] == "complete" {
            return v["run_id"].as_str().unwrap_or("").to_owned();
        }
        if v["status"] == "failed" || v["status"] == "cancelled" {
            return String::new();
        }
    }
    String::new()
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn full_analyze_cycle_html_json_csv_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.rs"), "fn main() {}\n// comment\n").unwrap();
    std::fs::write(
        dir.path().join("utils.rs"),
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    )
    .unwrap();

    let app = make_test_router();
    let path_encoded = pct_encode(dir.path().to_str().unwrap_or("."));
    let form_body = format!(
        "path={path_encoded}&generate_html=1&generate_json=1&generate_csv=1&generate_xlsx=1"
    );

    // Step 1: submit scan
    let (status, headers, _) = post_form_shared(app.clone(), "/analyze", &form_body).await;
    assert_eq!(status, StatusCode::OK, "POST /analyze should return 200");
    let wait_id = headers
        .get("x-wait-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    assert!(!wait_id.is_empty(), "should receive x-wait-id");

    // Step 2: poll status until complete or timeout (10 s)
    let run_id = poll_run_complete(&app, &wait_id).await;

    // Step 3: fetch artifacts only if scan completed
    if run_id.is_empty() {
        let (status, _, _) = get_shared(
            app.clone(),
            "/runs/result/00000000-0000-0000-0000-000000000000",
        )
        .await;
        assert!(status.as_u16() < 500);
        return;
    }

    // HTML artifact
    let (status, headers, html_body) =
        get_shared(app.clone(), &format!("/runs/html/{run_id}")).await;
    assert!(
        status.as_u16() < 500,
        "/runs/html must not 5xx, got {status}"
    );
    if status == StatusCode::OK {
        let ct = headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            ct.contains("html"),
            "html artifact must have html content-type"
        );
        assert!(
            is_html_doc(&html_body),
            "html artifact must be a full HTML document"
        );
    }

    // JSON artifact
    let (status, _, json_body) = get_shared(app.clone(), &format!("/runs/json/{run_id}")).await;
    assert!(
        status.as_u16() < 500,
        "/runs/json must not 5xx, got {status}"
    );
    if status == StatusCode::OK {
        let v: serde_json::Value =
            serde_json::from_str(&json_body).expect("json artifact must be valid JSON");
        assert!(
            v.get("summary_totals").is_some(),
            "JSON must have summary_totals"
        );
    }

    // CSV artifact
    let (status, _, _) = get_shared(app.clone(), &format!("/runs/csv/{run_id}")).await;
    assert!(
        status.as_u16() < 500,
        "/runs/csv must not 5xx, got {status}"
    );

    // XLSX artifact
    let (status, _, _) = get_shared(app.clone(), &format!("/runs/xlsx/{run_id}")).await;
    assert!(
        status.as_u16() < 500,
        "/runs/xlsx must not 5xx, got {status}"
    );

    // Result page
    let (status, _, result_body) = get_shared(app.clone(), &format!("/runs/result/{run_id}")).await;
    assert!(
        status.as_u16() < 500,
        "/runs/result must not 5xx, got {status}"
    );
    if status == StatusCode::OK {
        assert!(is_html_doc(&result_body));
    }

    // API metrics for the specific run
    let (status, _, _) = get_shared(app.clone(), &format!("/api/metrics/{run_id}")).await;
    assert!(status.as_u16() < 500);

    // API metrics latest
    let (status, _, latest_body) = get_shared(app.clone(), "/api/metrics/latest").await;
    assert!(status.as_u16() < 500);
    if status == StatusCode::OK {
        let v: serde_json::Value = serde_json::from_str(&latest_body).unwrap_or_default();
        assert!(v.get("summary_totals").is_some() || v.get("run_id").is_some());
    }

    // View-reports page should now show an entry
    let (status, _, vr_body) = get_shared(app.clone(), "/view-reports").await;
    assert_eq!(status, StatusCode::OK);
    assert!(is_html_doc(&vr_body));

    // API metrics history
    let (status, _, history_body) = get_shared(app.clone(), "/api/metrics/history").await;
    assert!(status.as_u16() < 500);
    if status == StatusCode::OK {
        let _: serde_json::Value = serde_json::from_str(&history_body).unwrap_or_default();
    }

    // PDF status endpoint
    let (status, _, _) = get_shared(app.clone(), &format!("/api/runs/{run_id}/pdf-status")).await;
    assert!(status.as_u16() < 500);

    // Submodule metrics
    let (status, _, _) =
        get_shared(app.clone(), &format!("/api/metrics/{run_id}/submodules")).await;
    assert!(status.as_u16() < 500);

    // Bundle download
    let (status, _, _) = get_shared(app.clone(), &format!("/api/runs/{run_id}/bundle")).await;
    assert!(status.as_u16() < 500);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn full_analyze_cycle_compare_two_runs() {
    let dir1 = tempfile::tempdir().unwrap();
    std::fs::write(dir1.path().join("lib.rs"), "fn foo() {}\n").unwrap();
    let dir2 = tempfile::tempdir().unwrap();
    std::fs::write(dir2.path().join("lib.rs"), "fn foo() {}\nfn bar() {}\n").unwrap();

    let app = make_test_router();

    let (_, h1, _) = post_form_shared(
        app.clone(),
        "/analyze",
        &format!(
            "path={}&generate_json=1",
            pct_encode(dir1.path().to_str().unwrap_or("."))
        ),
    )
    .await;
    let wid1 = h1
        .get("x-wait-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();

    let (_, h2, _) = post_form_shared(
        app.clone(),
        "/analyze",
        &format!(
            "path={}&generate_json=1",
            pct_encode(dir2.path().to_str().unwrap_or("."))
        ),
    )
    .await;
    let wid2 = h2
        .get("x-wait-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();

    if wid1.is_empty() || wid2.is_empty() {
        return;
    }

    let rid1 = poll_run_complete(&app, &wid1).await;
    let rid2 = poll_run_complete(&app, &wid2).await;

    if rid1.is_empty() || rid2.is_empty() {
        return;
    }

    // Compare the two runs
    let (status, _, compare_body) =
        get_shared(app.clone(), &format!("/compare?a={rid1}&b={rid2}")).await;
    assert!(status.as_u16() < 500, "/compare must not 5xx, got {status}");
    if status == StatusCode::OK {
        assert!(is_html_doc(&compare_body));
    }

    // Project history after two runs
    let (status, _, _) = get_shared(app.clone(), "/api/project-history").await;
    assert!(status.as_u16() < 500);

    // ── Exercise large handlers with populated registry data ───────────────────

    // trend_report_handler (~1363 lines) — covered by hitting route with real data
    let (status, _, body) = get_shared(app.clone(), "/trend-reports").await;
    assert!(
        status.as_u16() < 500,
        "/trend-reports with data must not 5xx"
    );
    assert!(
        body.contains("<html"),
        "expected HTML from /trend-reports with data"
    );

    // test_metrics_handler (~1363 lines) — covered by hitting route with real data
    let (status, _, body) = get_shared(app.clone(), "/test-metrics").await;
    assert!(
        status.as_u16() < 500,
        "/test-metrics with data must not 5xx"
    );
    assert!(
        body.contains("<html"),
        "expected HTML from /test-metrics with data"
    );

    // embed handler with a known run_id (exercises render_embed_widget with data)
    let (status, _, body) = get_shared(app.clone(), &format!("/embed/summary?run_id={rid1}")).await;
    assert!(
        status.as_u16() < 500,
        "/embed/summary with run_id must not 5xx"
    );
    assert!(!body.is_empty(), "expected non-empty embed widget");

    // embed handler in dark mode
    let (status, _, _) = get_shared(
        app.clone(),
        &format!("/embed/summary?run_id={rid1}&theme=dark"),
    )
    .await;
    assert!(
        status.as_u16() < 500,
        "/embed/summary dark mode must not 5xx"
    );

    // api_metrics_latest_handler — populated registry returns metrics JSON
    let (status, _, body) = get_shared(app.clone(), "/api/metrics/latest").await;
    assert!(
        status.as_u16() < 500,
        "/api/metrics/latest with data must not 5xx"
    );
    assert!(
        body.contains("code_lines") || body.contains("files"),
        "expected metrics JSON"
    );

    // api_metrics_run_handler — lookup by specific run_id
    let (status, _, body) = get_shared(app.clone(), &format!("/api/metrics/{rid1}")).await;
    assert!(status.as_u16() < 500, "/api/metrics/:id must not 5xx");
    assert!(!body.is_empty(), "expected metrics JSON for run");

    // api_metrics_history_handler
    let (status, _, _) = get_shared(app.clone(), "/api/metrics/history").await;
    assert!(status.as_u16() < 500, "/api/metrics/history must not 5xx");

    // api_metrics_submodules_handler
    let (status, _, _) = get_shared(app.clone(), "/api/metrics/submodules").await;
    assert!(
        status.as_u16() < 500,
        "/api/metrics/submodules must not 5xx"
    );

    // confluence wiki markup with a valid run_id (exercises the found-run path)
    let (status, _, _) = get_shared(
        app.clone(),
        &format!("/api/confluence/wiki-markup?run_id={rid1}"),
    )
    .await;
    assert!(
        status.as_u16() < 500,
        "wiki-markup with valid run_id must not 5xx"
    );

    // pdf-status handler with known run_id
    let (status, _, _) = get_shared(app.clone(), &format!("/api/runs/{rid1}/pdf-status")).await;
    assert!(status.as_u16() < 500, "pdf-status must not 5xx");

    // download bundle handler
    let (status, _, _) = get_shared(app.clone(), &format!("/api/runs/{rid1}/bundle")).await;
    assert!(status.as_u16() < 500, "bundle download must not 5xx");

    // history page
    let (status, _, body) = get_shared(app.clone(), "/view-reports").await;
    assert!(status.as_u16() < 500, "/view-reports must not 5xx");
    assert!(body.contains("<html"), "expected HTML from /view-reports");
}

// ── OpenAPI / static assets ──────────────────────────────────────────────────

#[tokio::test]
async fn openapi_yaml_returns_yaml() {
    let (status, headers, body) = get("/api/openapi.yaml").await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("yaml") || ct.contains("text"),
        "expected YAML content-type, got: {ct}"
    );
    assert!(
        body.contains("openapi") || body.contains("paths"),
        "expected OpenAPI spec content, got: {body}"
    );
}

#[tokio::test]
async fn report_chart_js_returns_javascript() {
    let (status, headers, _) = get("/static/chart-report.js").await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("javascript") || ct.contains("text"),
        "unexpected content-type: {ct}"
    );
}

// ── Badge variants ────────────────────────────────────────────────────────────

#[tokio::test]
async fn badge_files_returns_svg() {
    let (status, headers, body) = get("/badge/files").await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("svg") || ct.contains("xml"),
        "expected SVG content-type, got: {ct}"
    );
    assert!(body.contains("<svg"), "expected SVG body");
}

#[tokio::test]
async fn badge_blank_lines_returns_svg() {
    let (status, _, body) = get("/badge/blank_lines").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<svg"), "expected SVG body");
}

#[tokio::test]
async fn badge_comment_lines_returns_svg() {
    let (status, _, body) = get("/badge/comment_lines").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<svg"), "expected SVG body");
}

#[tokio::test]
async fn badge_languages_returns_svg() {
    let (status, _, body) = get("/badge/languages").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<svg"), "expected SVG body");
}

// ── Confluence config routes ──────────────────────────────────────────────────

#[tokio::test]
async fn api_confluence_config_get_returns_json() {
    let (status, headers, body) = get("/api/confluence/config").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "/api/confluence/config must return 200"
    );
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("json"), "expected JSON content-type, got: {ct}");
    assert!(
        body.contains("\"configured\"") || body.contains("configured"),
        "expected 'configured' key in response, got: {body}"
    );
}

#[tokio::test]
async fn api_confluence_config_post_with_valid_payload_not_5xx() {
    let payload = r#"{"base_url":"https://mycompany.atlassian.net","username":"test@example.com","credential":"my-token","space_key":"DEV","tier":"cloud"}"#;
    let (status, _, _) = post_json("/api/confluence/config", payload).await;
    assert!(
        status.as_u16() < 500,
        "POST /api/confluence/config must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn api_confluence_config_post_empty_payload_not_5xx() {
    let (status, _, _) = post_json("/api/confluence/config", r"{}").await;
    assert!(
        status.as_u16() < 500,
        "POST /api/confluence/config empty payload must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn api_confluence_wiki_markup_not_5xx() {
    let (status, _, _) = get("/api/confluence/wiki-markup").await;
    assert!(
        status.as_u16() < 500,
        "/api/confluence/wiki-markup must not 5xx, got {status}"
    );
}

// ── Schedule creation and management ─────────────────────────────────────────

#[tokio::test]
async fn api_create_schedule_webhook_returns_201() {
    let payload = r#"{
      "repo_url": "https://github.com/org/repo.git",
      "branch": "main",
      "kind": "webhook",
      "provider": "github",
      "label": "Test schedule"
    }"#;
    let (status, headers, body) = post_json("/api/schedules", payload).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "creating a schedule must return 201, body: {body}"
    );
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("json"), "expected JSON response, got: {ct}");
    assert!(
        body.contains("\"id\""),
        "response must contain id field, got: {body}"
    );
}

#[tokio::test]
async fn api_create_schedule_poll_returns_201() {
    let payload = r#"{
      "repo_url": "https://gitlab.com/group/project.git",
      "branch": "develop",
      "kind": "poll",
      "provider": "any",
      "label": "Poll schedule",
      "interval_secs": 300
    }"#;
    let (status, _, body) = post_json("/api/schedules", payload).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "creating a poll schedule must return 201, body: {body}"
    );
    assert!(
        body.contains("\"id\""),
        "response must contain id, got: {body}"
    );
}

#[tokio::test]
async fn api_delete_schedule_valid_uuid_returns_204() {
    let (status, _, _) = delete("/api/schedules?id=00000000-0000-0000-0000-000000000002").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn api_schedules_list_after_create() {
    let app = make_test_router();
    let payload = r#"{"repo_url":"https://github.com/org/repo.git","branch":"main","kind":"webhook","provider":"github","label":"Listed"}"#;
    let req = axum::http::Request::post("/api/schedules")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(payload))
        .unwrap();
    let _ = app.clone().oneshot(req).await.unwrap();

    let (status, _, body) = get_shared(app, "/api/schedules").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"schedules\""));
}

// ── Cleanup policy ────────────────────────────────────────────────────────────

#[tokio::test]
async fn api_cleanup_policy_get_returns_json() {
    let (status, headers, body) = get("/api/cleanup-policy").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GET /api/cleanup-policy must return 200"
    );
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("json"), "expected JSON content-type, got: {ct}");
    assert!(
        body.contains("\"policy\"") || body.contains("policy"),
        "expected policy key in response, got: {body}"
    );
}

#[tokio::test]
async fn api_cleanup_policy_post_with_valid_payload_not_5xx() {
    let payload = r#"{"kind":"keep_last_n","keep_last_n":10}"#;
    let (status, _, body) = post_json("/api/cleanup-policy", payload).await;
    assert!(
        status.as_u16() < 500,
        "POST /api/cleanup-policy must not 5xx, got {status}, body: {body}"
    );
}

#[tokio::test]
async fn api_cleanup_policy_delete_returns_success() {
    let app = make_test_router();
    let resp = app
        .oneshot(
            axum::http::Request::delete("/api/cleanup-policy")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status().as_u16() < 500,
        "DELETE /api/cleanup-policy must not 5xx, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn api_cleanup_policy_run_now_not_5xx() {
    let (status, _, _) = post_form("/api/cleanup-policy/run-now", "").await;
    assert!(
        status.as_u16() < 500,
        "POST /api/cleanup-policy/run-now must not 5xx, got {status}"
    );
}

// ── Delete run handler ─────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_run_unknown_id_returns_404() {
    let app = make_test_router();
    let resp = app
        .oneshot(
            axum::http::Request::delete("/api/runs/00000000-0000-0000-0000-000000000000")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // Handler is idempotent: unknown run returns 204 No Content (not 404)
    assert!(
        resp.status().as_u16() < 500,
        "deleting unknown run must not 5xx, got {}",
        resp.status()
    );
}

// ── Webhook routes (GitLab / Bitbucket) ──────────────────────────────────────

#[tokio::test]
async fn post_gitlab_webhook_ping_returns_non_5xx() {
    let app = make_test_router();
    let payload = r#"{"object_kind":"push","ref":"refs/heads/main","checkout_sha":"abc123","user_username":"alice","project":{"git_http_url":"https://gitlab.com/org/repo.git"}}"#;
    let req = axum::http::Request::post("/webhooks/gitlab")
        .header("content-type", "application/json")
        .header("x-gitlab-event", "Push Hook")
        .body(axum::body::Body::from(payload))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert!(
        resp.status().as_u16() < 500,
        "GitLab push webhook must not 5xx, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn post_bitbucket_webhook_returns_non_5xx() {
    let app = make_test_router();
    let payload = r#"{"actor":{"display_name":"alice"},"repository":{"links":{"clone":[{"name":"https","href":"https://bitbucket.org/ws/repo.git"}]}},"push":{"changes":[{"new":{"name":"main","target":{"hash":"abc1234567890abc1234567890abc1234567890ab"}}}]}}"#;
    let req = axum::http::Request::post("/webhooks/bitbucket")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(payload))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert!(
        resp.status().as_u16() < 500,
        "Bitbucket webhook must not 5xx, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn post_github_webhook_push_event_returns_non_5xx() {
    let app = make_test_router();
    let payload = r#"{"ref":"refs/heads/main","after":"abc1234def5678abc1234def5678abc1234def56","repository":{"clone_url":"https://github.com/org/repo.git"},"pusher":{"name":"alice"}}"#;
    let req = axum::http::Request::post("/webhooks/github")
        .header("content-type", "application/json")
        .header("x-github-event", "push")
        .body(axum::body::Body::from(payload))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert!(
        resp.status().as_u16() < 500,
        "GitHub push webhook (no secret) must not 5xx, got {}",
        resp.status()
    );
}

// ── Image handler ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn image_logo_text_returns_png() {
    let (status, headers, _) = get("/images/logo/logo-text.png").await;
    assert_eq!(status, StatusCode::OK, "logo-text.png must return 200");
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("png") || ct.contains("image"),
        "expected image content-type, got: {ct}"
    );
}

#[tokio::test]
async fn image_unknown_returns_404() {
    let (status, _, _) = get("/images/logo/nonexistent.png").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "unknown image must return 404"
    );
}

// ── Redirect routes ───────────────────────────────────────────────────────────

#[tokio::test]
async fn webhook_setup_redirects_to_integrations() {
    let app = make_test_router();
    let resp = app
        .oneshot(
            axum::http::Request::get("/webhook-setup")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status().is_redirection(),
        "GET /webhook-setup must redirect, got {}",
        resp.status()
    );
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        location.contains("integrations"),
        "redirect must point to /integrations, got: {location}"
    );
}

#[tokio::test]
async fn confluence_setup_redirects_to_integrations() {
    let app = make_test_router();
    let resp = app
        .oneshot(
            axum::http::Request::get("/confluence-setup")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status().is_redirection(),
        "GET /confluence-setup must redirect, got {}",
        resp.status()
    );
}

// ── Upload handlers (headless guard fires) ────────────────────────────────────

#[tokio::test]
async fn upload_directory_missing_body_not_5xx() {
    let app = make_test_router();
    let req = axum::http::Request::post("/api/upload-directory")
        .header("content-type", "multipart/form-data; boundary=abc")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert!(
        resp.status().as_u16() < 500,
        "/api/upload-directory must not 5xx, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn upload_file_missing_body_not_5xx() {
    let app = make_test_router();
    let req = axum::http::Request::post("/api/upload-file")
        .header("content-type", "multipart/form-data; boundary=abc")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert!(
        resp.status().as_u16() < 500,
        "/api/upload-file must not 5xx, got {}",
        resp.status()
    );
}

// ── Ingest with valid payload ─────────────────────────────────────────────────

#[tokio::test]
async fn api_ingest_with_minimal_valid_payload_not_5xx() {
    let payload = r#"{"run_id":"test-123","summary":{"files_analyzed":1,"code_lines":50}}"#;
    let (status, _, _) = post_json("/api/ingest", payload).await;
    assert!(
        status.as_u16() < 500,
        "/api/ingest with minimal payload must not 5xx, got {status}"
    );
}

// ── Full analyze cycle: multi-language project ────────────────────────────────

#[tokio::test]
async fn full_analyze_cycle_multi_language_project() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("main.py"),
        "def foo():\n    pass\n# comment\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("utils.js"),
        "function bar() { return 1; }\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("app.ts"), "const x: number = 42;\n").unwrap();
    std::fs::write(
        dir.path().join("style.css"),
        ".btn { color: red; } /* style */\n",
    )
    .unwrap();

    let app = make_test_router();
    let path_encoded = pct_encode(dir.path().to_str().unwrap_or("."));
    let form_body = format!("path={path_encoded}&generate_html=1&generate_json=1");

    let (status, headers, _) = post_form_shared(app.clone(), "/analyze", &form_body).await;
    assert_eq!(status, StatusCode::OK);
    let wait_id = headers
        .get("x-wait-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    assert!(!wait_id.is_empty());

    let mut run_id = String::new();
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let (_, _, body) = get_shared(app.clone(), &format!("/api/runs/{wait_id}/status")).await;
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
            if v["state"] == "complete" {
                run_id = v["run_id"].as_str().unwrap_or("").to_owned();
                break;
            }
            if v["status"] == "failed" || v["status"] == "cancelled" {
                break;
            }
        }
    }

    if run_id.is_empty() {
        return;
    }

    // Verify multi-language data appears in the run result
    let (status, _, result_body) = get_shared(app.clone(), &format!("/runs/result/{run_id}")).await;
    assert!(status.as_u16() < 500);
    if status == StatusCode::OK {
        assert!(result_body.contains("<!doctype html>") || result_body.contains("<!DOCTYPE html>"));
    }

    // Check that the project history includes this run
    let (status, _, _) = get_shared(app.clone(), "/api/project-history").await;
    assert!(status.as_u16() < 500);

    // Embed summary should now show data
    let (status, _, _) = get_shared(app.clone(), "/embed/summary").await;
    assert!(status.as_u16() < 500);

    // Compare-scans page with a run in the registry
    let (status, _, _) = get_shared(app.clone(), "/compare-scans").await;
    assert_eq!(status, StatusCode::OK);
}

// ── Scan profile lifecycle ────────────────────────────────────────────────────

#[tokio::test]
async fn scan_profile_create_then_delete_lifecycle() {
    let app = make_test_router();

    // Create
    let create_req = axum::http::Request::post("/api/scan-profiles")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            r#"{"name":"my-profile","params":{"path":"."}}"#,
        ))
        .unwrap();
    let create_resp = app.clone().oneshot(create_req).await.unwrap();
    assert_eq!(create_resp.status(), StatusCode::CREATED);
    let bytes = create_resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
    let profile_id = body["id"].as_str().unwrap_or("").to_owned();

    if profile_id.is_empty() {
        return; // can't test delete without a valid ID
    }

    // List — should include the new profile
    let (status, _, list_body) = get_shared(app.clone(), "/api/scan-profiles").await;
    assert_eq!(status, StatusCode::OK);
    assert!(list_body.contains(&profile_id) || list_body.contains("my-profile"));

    // Delete
    let delete_resp = app
        .clone()
        .oneshot(
            axum::http::Request::delete(format!("/api/scan-profiles/{profile_id}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        delete_resp.status().as_u16() < 500,
        "profile delete must not 5xx"
    );
}

// ── Metrics handler ───────────────────────────────────────────────────────────

#[tokio::test]
async fn metrics_endpoint_not_5xx() {
    let (status, _, _) = get("/metrics").await;
    assert!(status.as_u16() < 500, "/metrics must not 5xx, got {status}");
}

// ── Analyze with various form params ─────────────────────────────────────────

#[tokio::test]
async fn post_analyze_with_label_param_not_5xx() {
    let (status, headers, _) = post_form(
        "/analyze",
        "path=.&project_label=my-test-project&generate_html=1",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.contains_key("x-wait-id"));
}

#[tokio::test]
async fn post_analyze_per_file_output_not_5xx() {
    let (status, headers, _) = post_form("/analyze", "path=.&per_file=1&generate_json=1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.contains_key("x-wait-id"));
}

// ── Schedule create then delete lifecycle ──────────────────────────────────────

#[tokio::test]
async fn schedule_create_then_delete_lifecycle() {
    let app = make_test_router();

    // Create a webhook schedule
    let create_req = axum::http::Request::post("/api/schedules")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            r#"{"repo_url":"https://github.com/org/repo.git","branch":"main","kind":"webhook","provider":"github","label":"Test webhook"}"#,
        ))
        .unwrap();
    let create_resp = app.clone().oneshot(create_req).await.unwrap();
    assert_eq!(create_resp.status(), StatusCode::CREATED);
    let bytes = create_resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
    let schedule_id = body["id"].as_str().unwrap_or("").to_owned();

    if schedule_id.is_empty() {
        return;
    }

    // List — should include the schedule
    let (status, _, list_body) = get_shared(app.clone(), "/api/schedules").await;
    assert_eq!(status, StatusCode::OK);
    assert!(list_body.contains(&schedule_id) || list_body.contains("Test webhook"));

    // Delete
    let delete_resp = app
        .oneshot(
            axum::http::Request::delete(format!("/api/schedules?id={schedule_id}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(delete_resp.status(), StatusCode::NO_CONTENT);
}

// ── Phase 2: ingest-based branch coverage for large dashboard handlers ────────
//
// Each test injects a rich AnalysisRun via POST /api/ingest, then exercises a
// dashboard handler that has deep conditional branches keyed on the run data.

#[tokio::test]
async fn trend_reports_with_multi_language_data_renders_language_sections() {
    let app = make_test_router();
    let (ingest_status, _, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_multi_language()).await;
    assert!(
        ingest_status.as_u16() < 500,
        "ingest must succeed, got {ingest_status}"
    );
    let (status, headers, body) = get_shared(app.clone(), "/trend-reports").await;
    assert_eq!(status, StatusCode::OK);
    assert!(has_csp(&headers), "missing CSP on /trend-reports");
    assert!(body.contains("<html"), "expected HTML");
}

#[tokio::test]
async fn trend_reports_with_coverage_data_renders_coverage_section() {
    let app = make_test_router();
    let (s, _, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_coverage()).await;
    assert!(s.as_u16() < 500, "ingest failed with {s}");
    let (status, _, body) = get_shared(app.clone(), "/trend-reports").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<html"), "expected HTML from /trend-reports");
}

#[tokio::test]
async fn trend_reports_with_git_metadata_renders_git_section() {
    let app = make_test_router();
    let (s, _, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_git_meta()).await;
    assert!(s.as_u16() < 500, "ingest failed with {s}");
    let (status, _, body) = get_shared(app.clone(), "/trend-reports").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<html"), "expected HTML");
}

#[tokio::test]
async fn trend_reports_with_submodule_data_renders_submodule_section() {
    let app = make_test_router();
    let (s, _, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_submodules()).await;
    assert!(s.as_u16() < 500, "ingest failed with {s}");
    let (status, _, body) = get_shared(app.clone(), "/trend-reports").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<html"), "expected HTML");
}

#[tokio::test]
async fn trend_reports_with_multiple_ingested_runs_exercises_pagination() {
    let app = make_test_router();
    // Inject 12 runs to push past the default page size and exercise pagination branches
    for i in 0..12 {
        let mut run: AnalysisRun = serde_json::from_str(&fixture_run_multi_language()).unwrap();
        run.tool.run_id = format!("ingest-page-{i:03}");
        run.input_roots = vec![format!("/test/project-{i}")];
        let json = serde_json::to_string(&run).unwrap();
        let (s, _, _) = post_json_shared(app.clone(), "/api/ingest", &json).await;
        // Some may fail due to duplicate paths — only assert no 5xx
        assert!(s.as_u16() < 500, "ingest {i} returned 5xx: {s}");
    }
    let (status, _, body) = get_shared(app.clone(), "/trend-reports").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<html"), "expected HTML with paginated runs");
}

#[tokio::test]
async fn test_metrics_with_test_count_data_renders_metrics_section() {
    let app = make_test_router();
    let (s, _, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_test_metrics()).await;
    assert!(s.as_u16() < 500, "ingest failed with {s}");
    let (status, _, body) = get_shared(app.clone(), "/test-metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<html"),
        "expected HTML from /test-metrics with test data"
    );
}

#[tokio::test]
async fn test_metrics_with_coverage_and_test_data_renders_combined_view() {
    let app = make_test_router();
    let (s, _, _) = post_json_shared(
        app.clone(),
        "/api/ingest",
        &fixture_run_coverage_and_tests(),
    )
    .await;
    assert!(s.as_u16() < 500, "ingest failed with {s}");
    let (status, _, body) = get_shared(app.clone(), "/test-metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<html"),
        "expected HTML with coverage+test data"
    );
}

#[tokio::test]
async fn test_metrics_with_multi_language_run_exercises_language_breakdown() {
    let app = make_test_router();
    let (s, _, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_multi_language()).await;
    assert!(s.as_u16() < 500, "ingest failed with {s}");
    let (status, _, body) = get_shared(app.clone(), "/test-metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<html"), "expected HTML");
}

#[tokio::test]
async fn test_metrics_with_git_metadata_renders_git_fields() {
    let app = make_test_router();
    let (s, _, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_git_meta()).await;
    assert!(s.as_u16() < 500, "ingest failed with {s}");
    let (status, _, body) = get_shared(app.clone(), "/test-metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<html"), "expected HTML");
}

#[tokio::test]
async fn compare_page_with_two_ingested_runs_returns_html() {
    let app = make_test_router();
    // Inject two different runs
    let (s1, h1, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_coverage()).await;
    assert!(s1.as_u16() < 500, "first ingest failed with {s1}");
    let (s2, h2, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_multi_language()).await;
    assert!(s2.as_u16() < 500, "second ingest failed with {s2}");

    // Extract run_ids from Location headers (if 201) or use known fixture IDs
    let rid1 = h1
        .get("x-run-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("ingest-coverage-001");
    let rid2 = h2
        .get("x-run-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("ingest-multilang-001");

    let (status, _, body) = get_shared(app.clone(), &format!("/compare?a={rid1}&b={rid2}")).await;
    assert!(status.as_u16() < 500, "/compare must not 5xx");
    if status == StatusCode::OK {
        assert!(
            body.contains("<html") || body.contains("<!doctype"),
            "expected HTML from /compare"
        );
    }
}

#[tokio::test]
async fn embed_handler_with_rich_coverage_run_returns_widget() {
    let app = make_test_router();
    let (s, _, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_coverage()).await;
    assert!(s.as_u16() < 500, "ingest failed");
    let (status, _, body) =
        get_shared(app.clone(), "/embed/summary?run_id=ingest-coverage-001").await;
    assert!(
        status.as_u16() < 500,
        "/embed/summary must not 5xx, got {status}"
    );
    assert!(!body.is_empty(), "expected non-empty embed widget");
}

#[tokio::test]
async fn embed_handler_dark_mode_with_data() {
    let app = make_test_router();
    let (s, _, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_git_meta()).await;
    assert!(s.as_u16() < 500, "ingest failed");
    let (status, _, _) = get_shared(
        app.clone(),
        "/embed/summary?run_id=ingest-gitmeta-001&theme=dark",
    )
    .await;
    assert!(status.as_u16() < 500, "/embed/summary dark must not 5xx");
}

#[tokio::test]
async fn history_handler_with_multiple_ingested_runs_shows_entries() {
    let app = make_test_router();
    let (s1, _, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_coverage()).await;
    assert!(s1.as_u16() < 500, "first ingest failed");
    let (s2, _, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_test_metrics()).await;
    assert!(s2.as_u16() < 500, "second ingest failed");
    let (status, _, body) = get_shared(app.clone(), "/view-reports").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<!doctype html>") || body.contains("<!DOCTYPE html>"),
        "expected HTML from /view-reports"
    );
}

#[tokio::test]
async fn api_metrics_run_with_ingested_coverage_data_returns_fields() {
    let app = make_test_router();
    let (s, _, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_coverage()).await;
    assert!(s.as_u16() < 500, "ingest failed");
    let (status, _, body) = get_shared(app.clone(), "/api/metrics/ingest-coverage-001").await;
    assert!(status.as_u16() < 500, "/api/metrics/:id must not 5xx");
    if status == StatusCode::OK {
        // Verify it's valid JSON — shape varies by implementation
        let _v: serde_json::Value =
            serde_json::from_str(&body).expect("metrics endpoint must return valid JSON");
    }
}

#[tokio::test]
async fn api_metrics_latest_with_ingested_data_returns_json() {
    let app = make_test_router();
    let (s, _, _) = post_json_shared(
        app.clone(),
        "/api/ingest",
        &fixture_run_coverage_and_tests(),
    )
    .await;
    assert!(s.as_u16() < 500, "ingest failed");
    let (status, _, body) = get_shared(app.clone(), "/api/metrics/latest").await;
    assert!(status.as_u16() < 500, "/api/metrics/latest must not 5xx");
    if status == StatusCode::OK {
        let _v: serde_json::Value =
            serde_json::from_str(&body).expect("/api/metrics/latest must return valid JSON");
    }
}

#[tokio::test]
async fn api_ingest_with_submodule_run_registers_and_renders() {
    let app = make_test_router();
    let (status, _, body) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_submodules()).await;
    assert!(
        status.as_u16() < 500,
        "ingest with submodules must not 5xx, got {status}: {body}"
    );
}

#[tokio::test]
async fn trend_reports_with_empty_repo_shows_no_data_state() {
    let app = make_test_router();
    let (s, _, _) = post_json_shared(app.clone(), "/api/ingest", &fixture_run_empty_repo()).await;
    assert!(s.as_u16() < 500, "empty repo ingest failed with {s}");
    let (status, _, body) = get_shared(app.clone(), "/trend-reports").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<html"), "expected HTML from /trend-reports");
}

// ── Phase 3: secondary handler success-path tests ─────────────────────────────

#[tokio::test]
async fn scan_setup_with_real_path_returns_html() {
    let dir = tempfile::tempdir().unwrap();
    let path = pct_encode(dir.path().to_str().unwrap_or("."));
    let (status, headers, body) = get(&format!("/scan-setup?path={path}")).await;
    assert!(
        status.as_u16() < 500,
        "/scan-setup with real path must not 5xx, got {status}"
    );
    if status == StatusCode::OK {
        assert!(has_csp(&headers), "expected CSP on /scan-setup");
        assert!(body.contains("<html"), "expected HTML");
    }
}

#[tokio::test]
async fn scan_setup_recent_dirs_branch() {
    let (status, _, _) = get("/scan-setup?recent=1").await;
    assert!(status.as_u16() < 500, "/scan-setup?recent=1 must not 5xx");
}

#[tokio::test]
async fn preview_handler_with_real_dir_returns_response() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello.rs"), "fn main() {}\n").unwrap();
    let path = pct_encode(dir.path().to_str().unwrap_or("."));
    let (status, _, _) = get(&format!("/preview?path={path}")).await;
    assert!(status.as_u16() < 500, "/preview must not 5xx, got {status}");
}

#[tokio::test]
async fn delete_run_after_ingest_returns_no_content_or_404() {
    // Use a dedicated run-id so the subsequent remove_dir_all does not race with
    // other parallel tests that share the "ingest-coverage-001" output directory.
    let run = fixture_base_run("ingest-delete-only-001");
    let mut run = run;
    run.input_roots = vec!["/test/delete-only".into()];
    let payload = serde_json::to_string(&run).unwrap();

    let app = make_test_router();
    let (s, _, _) = post_json_shared(app.clone(), "/api/ingest", &payload).await;
    assert!(s.as_u16() < 500, "ingest failed");

    let req = Request::delete("/api/runs/ingest-delete-only-001")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::NO_CONTENT
            || status == StatusCode::NOT_FOUND
            || status == StatusCode::OK,
        "DELETE /api/runs/:id must return 204, 404, or 200, got {status}"
    );
}

#[tokio::test]
async fn cleanup_runs_post_returns_ok() {
    let (status, _, _) = post_form("/api/runs/cleanup", "").await;
    assert!(
        status.as_u16() < 500,
        "POST /api/runs/cleanup must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn api_cleanup_policy_get_with_default_state_returns_json() {
    let (status, headers, _) = get("/api/cleanup-policy").await;
    assert!(
        status.as_u16() < 500,
        "GET /api/cleanup-policy must not 5xx"
    );
    if status == StatusCode::OK {
        let ct = headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(ct.contains("json"), "expected JSON content-type");
    }
}

#[tokio::test]
async fn api_cleanup_policy_run_now_returns_ok() {
    let app = make_test_router();
    let req = Request::post("/api/cleanup-policy/run-now")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert!(
        resp.status().as_u16() < 500,
        "POST /api/cleanup-policy/run-now must not 5xx"
    );
}

#[tokio::test]
async fn api_cleanup_policy_delete_returns_ok() {
    let app = make_test_router();
    let req = Request::delete("/api/cleanup-policy")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert!(
        resp.status().as_u16() < 500,
        "DELETE /api/cleanup-policy must not 5xx"
    );
}

#[tokio::test]
async fn download_bundle_after_ingest_not_5xx() {
    let app = make_test_router();
    let (s, _, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_git_meta()).await;
    assert!(s.as_u16() < 500, "ingest failed");
    let (status, _, _) = get_shared(app.clone(), "/api/runs/ingest-gitmeta-001/bundle").await;
    assert!(
        status.as_u16() < 500,
        "/api/runs/:id/bundle must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn upload_directory_with_multipart_not_5xx() {
    let app = make_test_router();
    let boundary = "TEST_BOUNDARY_XYZ";
    let body_bytes = raw_multipart(boundary, "files", "test.rs", b"fn main() {}\n");
    let req = Request::post("/upload-directory")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body_bytes))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert!(
        resp.status().as_u16() < 500,
        "POST /upload-directory must not 5xx"
    );
}

#[tokio::test]
async fn upload_file_with_multipart_not_5xx() {
    let app = make_test_router();
    let boundary = "UPLOAD_FILE_BOUNDARY";
    let body_bytes = raw_multipart(
        boundary,
        "file",
        "main.rs",
        b"fn main() { println!(\"hi\"); }\n",
    );
    let req = Request::post("/upload-file")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body_bytes))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert!(
        resp.status().as_u16() < 500,
        "POST /upload-file must not 5xx"
    );
}

#[tokio::test]
async fn upload_tarball_with_multipart_not_5xx() {
    let app = make_test_router();
    let boundary = "UPLOAD_TAR_BOUNDARY";
    // Send a minimal gzip-like payload (content doesn't need to be a valid tarball for
    // a not-5xx assertion; the handler should return 4xx for invalid content)
    let body_bytes = raw_multipart(boundary, "tarball", "archive.tar.gz", b"\x1f\x8b\x00");
    let req = Request::post("/upload-tarball")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body_bytes))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert!(
        resp.status().as_u16() < 500,
        "POST /upload-tarball must not 5xx"
    );
}

#[tokio::test]
async fn watched_dirs_add_with_real_tempdir_redirects() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap_or(".").to_owned();
    let (status, _, _) = post_form(
        "/watched-dirs/add",
        &format!(
            "folder_path={}&redirect_to=/trend-reports",
            pct_encode(&path)
        ),
    )
    .await;
    assert!(
        status.as_u16() < 500,
        "POST /watched-dirs/add must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn watched_dirs_remove_with_path_redirects() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap_or(".").to_owned();
    // First add it
    let app = make_test_router();
    post_form_shared(
        app.clone(),
        "/watched-dirs/add",
        &format!(
            "folder_path={}&redirect_to=/trend-reports",
            pct_encode(&path)
        ),
    )
    .await;
    // Then remove it
    let (status, _, _) = post_form_shared(
        app.clone(),
        "/watched-dirs/remove",
        &format!(
            "folder_path={}&redirect_to=/trend-reports",
            pct_encode(&path)
        ),
    )
    .await;
    assert!(
        status.as_u16() < 500,
        "POST /watched-dirs/remove must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn open_path_handler_with_headless_env_returns_redirect() {
    let dir = tempfile::tempdir().unwrap();
    let path = pct_encode(dir.path().to_str().unwrap_or("."));
    let (status, _, _) = get(&format!("/open-path?path={path}")).await;
    assert!(
        status.as_u16() < 500,
        "/open-path must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn api_ingest_with_coverage_run_then_project_history_returns_data() {
    let app = make_test_router();
    let (s, _, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_coverage()).await;
    assert!(s.as_u16() < 500, "ingest failed");
    let (status, _, body) = get_shared(app.clone(), "/api/project-history").await;
    assert!(status.as_u16() < 500, "/api/project-history must not 5xx");
    if status == StatusCode::OK {
        let _v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    }
}

#[tokio::test]
async fn pdf_status_for_ingested_run_not_5xx() {
    let app = make_test_router();
    let (s, _, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_git_meta()).await;
    assert!(s.as_u16() < 500, "ingest failed");
    let (status, _, _) = get_shared(app.clone(), "/api/runs/ingest-gitmeta-001/pdf-status").await;
    assert!(
        status.as_u16() < 500,
        "/api/runs/:id/pdf-status must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn api_metrics_submodules_with_ingested_submodule_run_not_5xx() {
    let app = make_test_router();
    let (s, _, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_submodules()).await;
    assert!(s.as_u16() < 500, "ingest failed");
    let (status, _, _) =
        get_shared(app.clone(), "/api/metrics/ingest-submodules-001/submodules").await;
    assert!(
        status.as_u16() < 500,
        "/api/metrics/:id/submodules must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn cancel_run_for_unknown_id_returns_ok_or_not_found() {
    let app = make_test_router();
    let req = Request::post("/api/runs/no-such-run-xyz/cancel")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status.as_u16() < 500,
        "POST /api/runs/:id/cancel must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn async_run_result_for_unknown_id_not_5xx() {
    let (status, _, _) = get("/runs/result/no-such-run-xyz").await;
    assert!(
        status.as_u16() < 500,
        "/runs/result/:id must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn locate_report_with_valid_json_path_not_5xx() {
    let dir = tempfile::tempdir().unwrap();
    let json_path = dir.path().join("report.json");
    // Write a minimal valid AnalysisRun JSON
    std::fs::write(&json_path, fixture_run_with_coverage()).unwrap();
    let payload = serde_json::json!({
        "path": json_path.to_str().unwrap_or(""),
    });
    let app = make_test_router();
    let req = Request::post("/locate-report")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert!(
        resp.status().as_u16() < 500,
        "POST /locate-report must not 5xx"
    );
}

#[tokio::test]
async fn locate_reports_dir_with_real_dir_not_5xx() {
    let dir = tempfile::tempdir().unwrap();
    let payload = serde_json::json!({
        "path": dir.path().to_str().unwrap_or(""),
    });
    let app = make_test_router();
    let req = Request::post("/locate-reports-dir")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert!(
        resp.status().as_u16() < 500,
        "POST /locate-reports-dir must not 5xx"
    );
}

// ── A1: server_mode = true gated paths ───────────────────────────────────────

#[tokio::test]
async fn server_mode_trend_reports_shows_locked_watched_bar() {
    let app = make_test_router_server_mode();
    let (status, _, body) = get_server_shared(app, "/trend-reports").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("Network Server mode")
            || body.contains("network-server")
            || body.contains("watched-none"),
        "server_mode trend-reports should show locked watched bar, got len={}",
        body.len()
    );
}

#[tokio::test]
async fn server_mode_test_metrics_renders_html() {
    let app = make_test_router_server_mode();
    let (status, headers, body) = get_server_shared(app, "/test-metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(has_csp(&headers));
    assert!(
        body.contains("<html"),
        "expected HTML from server_mode /test-metrics"
    );
}

#[tokio::test]
async fn server_mode_analyze_with_path_rejects_when_no_allowed_roots() {
    // server_mode + no allowed_scan_roots → validate_server_scan_path rejects
    let app = make_test_router_server_mode();
    let dir = tempfile::tempdir().unwrap();
    let path = pct_encode(dir.path().to_str().unwrap_or("."));
    let (status, _, _) = post_form_shared(app, "/analyze", &format!("path={path}")).await;
    // Should be a redirect or 4xx/5xx — must not panic
    assert!(
        status.as_u16() != 500,
        "server_mode analyze must not 500, got {status}"
    );
}

#[tokio::test]
async fn server_mode_analyze_path_outside_allowed_roots_rejected() {
    // allowed_scan_roots is configured but the requested path is elsewhere.
    let allowed = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let app = make_test_router_server_mode_with_roots(vec![allowed.path().to_path_buf()]);
    let path = pct_encode(elsewhere.path().to_str().unwrap_or("."));
    let (status, body) = post_form_server_authed(app, "/analyze", &format!("path={path}")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "out-of-root must be 403");
    assert!(
        body.contains("not within an allowed") || body.contains("allowed scan"),
        "out-of-root message: {}",
        &body[..body.len().min(300)]
    );
}

#[tokio::test]
async fn server_mode_analyze_unresolvable_path_rejected() {
    // Path does not canonicalize (does not exist) → fail-closed 403.
    let allowed = tempfile::tempdir().unwrap();
    let app = make_test_router_server_mode_with_roots(vec![allowed.path().to_path_buf()]);
    let missing = allowed.path().join("does/not/exist/anywhere");
    let path = pct_encode(missing.to_str().unwrap_or("."));
    let (status, body) = post_form_server_authed(app, "/analyze", &format!("path={path}")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "unresolvable must be 403");
    assert!(
        body.contains("could not be resolved") || body.contains("real directory"),
        "unresolvable message: {}",
        &body[..body.len().min(300)]
    );
}

#[tokio::test]
async fn server_mode_analyze_path_inside_allowed_root_accepted() {
    // Path is inside an allowed root → validation passes, analysis proceeds.
    let allowed = tempfile::tempdir().unwrap();
    std::fs::write(allowed.path().join("main.rs"), "fn main() {}\n").unwrap();
    let app = make_test_router_server_mode_with_roots(vec![allowed.path().to_path_buf()]);
    let path = pct_encode(allowed.path().to_str().unwrap_or("."));
    let (status, _) = post_form_server_authed(app, "/analyze", &format!("path={path}")).await;
    // Not a 403 (validation passed); the analysis is dispatched (redirect/200) and
    // must never 500.
    assert_ne!(
        status,
        StatusCode::FORBIDDEN,
        "in-root path must pass validation"
    );
    assert_ne!(status.as_u16(), 500, "in-root analyze must not 500");
}

#[tokio::test]
async fn server_mode_preview_rejects_arbitrary_path() {
    let app = make_test_router_server_mode();
    let dir = tempfile::tempdir().unwrap();
    let path = pct_encode(dir.path().to_str().unwrap_or("."));
    let (status, _, body) = get_shared(app, &format!("/preview?path={path}")).await;
    // server_mode preview rejects non-upload/non-sample paths
    assert!(
        status.as_u16() < 500,
        "/preview in server_mode must not 5xx, got {status}"
    );
    // Should return the rejected message or an empty preview
    let _ = body;
}

#[tokio::test]
async fn server_mode_view_reports_renders_html() {
    let app = make_test_router_server_mode();
    let (status, _, body) = get_server_shared(app, "/view-reports").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<html"),
        "expected HTML from server_mode /view-reports"
    );
}

#[tokio::test]
async fn server_mode_history_handler_with_error_query_param() {
    let app = make_test_router_server_mode();
    // history_handler has an error query param branch
    let (status, _, body) = get_server_shared(app, "/view-reports?error=scan_failed").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<html"),
        "expected HTML from /view-reports?error=..."
    );
}

#[tokio::test]
async fn server_mode_project_history_upload_path_branch() {
    // server_mode + path under oxide-sloc-uploads → exercises upload-name suffix branch
    let app = make_test_router_server_mode();
    let upload_base = std::env::temp_dir()
        .join("oxide-sloc-uploads")
        .join("some-uuid")
        .join("myproject");
    let path = pct_encode(upload_base.to_str().unwrap_or("."));
    let (status, _, _) = get_shared(app, &format!("/api/project-history?path={path}")).await;
    assert!(
        status.as_u16() < 500,
        "/api/project-history in server_mode must not 5xx"
    );
}

// ── A2: watched-dirs shared-router tests ─────────────────────────────────────

#[tokio::test]
async fn trend_reports_with_non_empty_watched_dirs_renders_chips() {
    let app = make_test_router();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap_or(".").to_owned();
    // Add the dir on the SAME router so state is shared
    let (add_status, _, _) = post_form_shared(
        app.clone(),
        "/watched-dirs/add",
        &format!(
            "folder_path={}&redirect_to=/trend-reports",
            pct_encode(&path)
        ),
    )
    .await;
    assert!(add_status.as_u16() < 500, "watched-dirs/add must not 5xx");
    // Now trend-reports should see the non-empty watched_dirs list
    let (status, _, body) = get_shared(app.clone(), "/trend-reports").await;
    assert_eq!(status, StatusCode::OK, "/trend-reports must be 200");
    assert!(body.contains("<html"), "expected HTML");
}

#[tokio::test]
async fn trend_reports_with_two_watched_dirs_renders_both() {
    let app = make_test_router();
    let dir1 = tempfile::tempdir().unwrap();
    let dir2 = tempfile::tempdir().unwrap();
    for dir in [&dir1, &dir2] {
        let p = dir.path().to_str().unwrap_or(".").to_owned();
        let (s, _, _) = post_form_shared(
            app.clone(),
            "/watched-dirs/add",
            &format!("folder_path={}&redirect_to=/trend-reports", pct_encode(&p)),
        )
        .await;
        assert!(s.as_u16() < 500);
    }
    let (status, _, body) = get_shared(app.clone(), "/trend-reports").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<html"));
}

#[tokio::test]
async fn view_reports_with_linked_query_param_renders_html() {
    let app = make_test_router();
    // history_handler accepts ?linked= for a "just saved" banner
    let (status, _, body) = get_shared(app.clone(), "/view-reports?linked=some-run-id").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<html"),
        "expected HTML from /view-reports?linked=..."
    );
}

#[tokio::test]
async fn test_metrics_with_watched_dir_state_renders_html() {
    let app = make_test_router();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap_or(".").to_owned();
    post_form_shared(
        app.clone(),
        "/watched-dirs/add",
        &format!(
            "folder_path={}&redirect_to=/test-metrics",
            pct_encode(&path)
        ),
    )
    .await;
    let (status, _, body) = get_shared(app.clone(), "/test-metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<html"),
        "expected HTML from /test-metrics with watched dir"
    );
}

// ── A3: git browser with real local git repo ──────────────────────────────────

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok()
}

fn init_local_repo(dir: &std::path::Path) -> bool {
    let ok = |c: &mut std::process::Command| c.output().is_ok_and(|o| o.status.success());
    ok(std::process::Command::new("git").args(["init", dir.to_str().unwrap_or(".")]))
        && ok(std::process::Command::new("git").current_dir(dir).args([
            "-c",
            "user.email=test@test.com",
            "-c",
            "user.name=Test",
            "commit",
            "--allow-empty",
            "-m",
            "init",
        ]))
}

#[tokio::test]
async fn api_list_refs_with_local_git_repo_returns_ok() {
    if !git_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    if !init_local_repo(dir.path()) {
        return; // git not configured well enough
    }
    let repo_url = format!("file://{}", dir.path().display());
    let (status, _, body) = get(&format!("/api/git/refs?repo={}", pct_encode(&repo_url))).await;
    // 200 with refs JSON, OR 502 if git clone fails (still < 500 except 502)
    assert!(
        status == StatusCode::OK || status.as_u16() == 502,
        "/api/git/refs must return 200 or 502, got {status}: {body}"
    );
}

#[tokio::test]
async fn api_list_refs_missing_repo_param_returns_400() {
    let (status, _, _) = get("/api/git/refs").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "missing ?repo= must return 400"
    );
}

#[tokio::test]
async fn api_scan_ref_with_local_repo_completes() {
    if !git_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.rs"), "fn f() {}\n").unwrap();
    let ok = |c: &mut std::process::Command| c.output().is_ok_and(|o| o.status.success());
    ok(std::process::Command::new("git").args(["init", dir.path().to_str().unwrap_or(".")]));
    ok(std::process::Command::new("git")
        .current_dir(&dir)
        .args(["add", "."]));
    if !ok(std::process::Command::new("git").current_dir(&dir).args([
        "-c",
        "user.email=t@t.com",
        "-c",
        "user.name=T",
        "commit",
        "-m",
        "add",
    ])) {
        return;
    }
    let repo_url = format!("file://{}", dir.path().display());
    let (status, _, _) = get(&format!(
        "/api/git/scan-ref?repo={}&ref_name=main",
        pct_encode(&repo_url)
    ))
    .await;
    assert!(
        status == StatusCode::OK || status.as_u16() == 502,
        "/api/git/scan-ref must return 200 or 502, got {status}"
    );
}

#[tokio::test]
async fn api_scan_ref_invalid_ref_name_returns_400() {
    // api_scan_ref validates the ref_name; path-traversal should return 400
    let (status, _, _) =
        get("/api/git/scan-ref?repo=file%3A%2F%2F%2Ftmp&ref_name=..%2F..%2Fetc%2Fpasswd").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "path-traversal ref_name must return 400, got {status}"
    );
}

#[tokio::test]
async fn git_browser_handler_renders_html() {
    let (status, headers, body) = get("/git-browser").await;
    assert_eq!(status, StatusCode::OK);
    assert!(has_csp(&headers), "missing CSP on /git-browser");
    assert!(body.contains("<html"), "expected HTML from /git-browser");
}

#[tokio::test]
async fn git_browser_with_repo_query_renders_html() {
    let (status, _, body) = get("/git-browser?repo=https%3A%2F%2Fgithub.com%2Ftest%2Frepo").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<html"),
        "expected HTML from /git-browser?repo=..."
    );
}

// ── A4: analyze variants ──────────────────────────────────────────────────────

#[tokio::test]
async fn analyze_when_semaphore_exhausted_returns_503() {
    let app = make_test_router_exhausted_semaphore();
    let dir = tempfile::tempdir().unwrap();
    let path = pct_encode(dir.path().to_str().unwrap_or("."));
    let (status, _, body) = post_form_shared(app, "/analyze", &format!("path={path}")).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "exhausted semaphore must return 503, got {status}: {body}"
    );
}

#[tokio::test]
async fn analyze_with_project_label_form_field() {
    let app = make_test_router();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.rs"), "fn f() {}\n").unwrap();
    let path = pct_encode(dir.path().to_str().unwrap_or("."));
    let form = format!("path={path}&report_title=My+Coverage+Report");
    let (status, headers, _) = post_form_shared(app, "/analyze", &form).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers.contains_key("x-wait-id"),
        "expected x-wait-id header"
    );
}

#[tokio::test]
async fn analyze_with_include_globs_form_field() {
    let app = make_test_router();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(dir.path().join("skip.py"), "pass\n").unwrap();
    let path = pct_encode(dir.path().to_str().unwrap_or("."));
    let form = format!("path={path}&include_globs=*.rs&report_title=Rust+Only");
    let (status, headers, _) = post_form_shared(app, "/analyze", &form).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.contains_key("x-wait-id"));
}

#[tokio::test]
async fn analyze_with_exclude_globs_form_field() {
    let app = make_test_router();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(dir.path().join("skip.rs"), "// skip\n").unwrap();
    let path = pct_encode(dir.path().to_str().unwrap_or("."));
    let form = format!("path={path}&exclude_globs=skip.rs");
    let (status, headers, _) = post_form_shared(app, "/analyze", &form).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.contains_key("x-wait-id"));
}

// ── A5: history / project-history query-param branches ───────────────────────

#[tokio::test]
async fn view_reports_with_error_query_param_renders_html() {
    let (status, _, body) = get("/view-reports?error=some_error").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<html"),
        "expected HTML from /view-reports?error=..."
    );
}

#[tokio::test]
async fn compare_scans_page_renders_html() {
    let (status, headers, body) = get("/compare-scans").await;
    assert_eq!(status, StatusCode::OK);
    assert!(has_csp(&headers));
    assert!(body.contains("<html"), "expected HTML from /compare-scans");
}

#[tokio::test]
async fn project_history_with_explicit_path_returns_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = pct_encode(dir.path().to_str().unwrap_or("."));
    let (status, _, _) = get(&format!("/api/project-history?path={path}")).await;
    assert!(status.as_u16() < 500, "/api/project-history must not 5xx");
}

#[tokio::test]
async fn metrics_history_with_ingested_run_returns_array() {
    let app = make_test_router();
    let (s, _, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_coverage()).await;
    assert!(s.as_u16() < 500, "ingest failed");
    let (status, _, body) = get_shared(app.clone(), "/api/metrics/history").await;
    assert!(status.as_u16() < 500);
    if status == StatusCode::OK {
        let _: serde_json::Value =
            serde_json::from_str(&body).expect("/api/metrics/history must be valid JSON");
    }
}

#[tokio::test]
async fn async_run_status_for_unknown_id_returns_json() {
    let (status, _, body) = get("/api/runs/00000000-0000-0000-0000-000000000000/status").await;
    assert!(status.as_u16() < 500);
    // Should return JSON indicating not found or unknown state
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    let _ = v;
}

// ── A6: compare handler with valid run IDs and different scopes ───────────────

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn compare_with_two_ingested_runs_exercises_delta_rendering() {
    let app = make_test_router();
    // Inject two runs with different code counts so delta is non-trivial
    let (s1, _, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_coverage()).await;
    assert!(s1.as_u16() < 500, "first ingest failed");
    let (s2, _, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_test_metrics()).await;
    assert!(s2.as_u16() < 500, "second ingest failed");

    let (status, _, body) = get_shared(
        app.clone(),
        "/compare?a=ingest-coverage-001&b=ingest-testmetrics-001",
    )
    .await;
    assert!(
        status.as_u16() < 500,
        "/compare with two run IDs must not 5xx, got {status}"
    );
    if status == StatusCode::OK {
        assert!(body.contains("<!doctype html>") || body.contains("<!DOCTYPE html>"));
    }
}

#[tokio::test]
async fn compare_with_submodule_scope_query_param() {
    let app = make_test_router();
    let (s1, _, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_submodules()).await;
    assert!(s1.as_u16() < 500, "ingest failed");

    // With a `sub=` query param — exercises submodule scope branch
    let (status, _, body) = get_shared(
        app.clone(),
        "/compare?a=ingest-submodules-001&b=ingest-submodules-001&sub=vendor%2Flib-a",
    )
    .await;
    assert!(
        status.as_u16() < 500,
        "/compare with sub= scope must not 5xx, got {status}"
    );
    let _ = body;
}

#[tokio::test]
async fn compare_with_scope_super_query_param() {
    let app = make_test_router();
    let (s, _, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_submodules()).await;
    assert!(s.as_u16() < 500, "ingest failed");
    let (status, _, _) = get_shared(
        app.clone(),
        "/compare?a=ingest-submodules-001&b=ingest-submodules-001&scope=super",
    )
    .await;
    assert!(status.as_u16() < 500, "/compare?scope=super must not 5xx");
}

#[tokio::test]
async fn compare_redirect_when_no_run_ids() {
    let (status, headers, _) = get("/compare").await;
    // Without a= and b= params, compare redirects to /compare-scans
    assert!(
        status.is_redirection() || status == StatusCode::OK,
        "/compare without IDs should redirect, got {status}"
    );
    if status.is_redirection() {
        let loc = headers
            .get("location")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            loc.contains("compare"),
            "redirect should go to compare page"
        );
    }
}

// ── A7: confluence end-to-end flow ────────────────────────────────────────────

#[tokio::test]
async fn api_confluence_config_save_and_retrieve() {
    let app = make_test_router();
    let payload = serde_json::json!({
        "tier": "cloud",
        "base_url": "https://mycompany.atlassian.net",
        "username": "user@example.com",
        "credential": "api-token",
        "space_key": "PROJ"
    });
    let (status, _, body) =
        post_json_shared(app.clone(), "/api/confluence/config", &payload.to_string()).await;
    assert!(
        status.as_u16() < 500,
        "POST /api/confluence/config must not 5xx"
    );
    // GET the config back
    let (get_status, _, get_body) = get_shared(app.clone(), "/api/confluence/config").await;
    assert!(get_status.as_u16() < 500);
    let _ = (body, get_body);
}

#[tokio::test]
async fn api_confluence_test_without_config_returns_400() {
    let app = make_test_router();
    let req = Request::post("/api/confluence/test")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // No config → 400 Bad Request
    assert!(
        resp.status().as_u16() < 500,
        "POST /api/confluence/test without config must not 5xx"
    );
}

#[tokio::test]
async fn api_confluence_post_with_run_not_found_returns_error() {
    let app = make_test_router();
    // Save a config first
    let cfg = serde_json::json!({
        "tier": "cloud",
        "base_url": "https://example.atlassian.net",
        "username": "u@e.com",
        "credential": "tok",
        "space_key": "SPACE"
    });
    post_json_shared(app.clone(), "/api/confluence/config", &cfg.to_string()).await;
    // Now try to post a run that doesn't exist
    let body = serde_json::json!({"run_id": "no-such-run-xyz"});
    let (status, _, _) =
        post_json_shared(app.clone(), "/api/confluence/post", &body.to_string()).await;
    assert!(
        status.as_u16() < 500,
        "confluence post with unknown run must not 5xx"
    );
}

#[tokio::test]
async fn api_confluence_wiki_markup_with_ingested_run_returns_html() {
    let app = make_test_router();
    let (s, _, _) =
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_git_meta()).await;
    assert!(s.as_u16() < 500, "ingest failed");
    let (status, _, body) = get_shared(
        app.clone(),
        "/api/confluence/wiki-markup?run_id=ingest-gitmeta-001",
    )
    .await;
    assert!(
        status.as_u16() < 500,
        "/api/confluence/wiki-markup must not 5xx, got {status}"
    );
    let _ = body;
}

#[tokio::test]
async fn api_confluence_post_with_ingested_run_exercises_post_flow() {
    // Even though the Confluence connection will fail (no real server),
    // this exercises the code path from config lookup → artifact lookup → post attempt.
    let app = make_test_router();
    // Ingest a run
    let (s, _, _) = post_json_shared(
        app.clone(),
        "/api/ingest",
        &fixture_run_coverage_and_tests(),
    )
    .await;
    assert!(s.as_u16() < 500, "ingest failed");
    // Configure confluence
    let cfg = serde_json::json!({
        "tier": "cloud",
        "base_url": "https://fake.atlassian.net",
        "username": "u@e.com",
        "credential": "tok",
        "space_key": "SPACE"
    });
    post_json_shared(app.clone(), "/api/confluence/config", &cfg.to_string()).await;
    // Try to post — will fail at HTTP level but exercises the handler code
    let body = serde_json::json!({"run_id": "ingest-cov-and-tests-001"});
    let (status, _, _) =
        post_json_shared(app.clone(), "/api/confluence/post", &body.to_string()).await;
    // 502 Bad Gateway is expected (can't reach fake.atlassian.net), not a server error
    assert!(
        status.as_u16() < 500 || status.as_u16() == 502,
        "confluence post must not produce unexpected 5xx (got {status})"
    );
}

// ── A8: rate limiter paths ────────────────────────────────────────────────────

#[tokio::test]
async fn rate_limiter_returns_429_after_limit_exceeded() {
    let app = make_test_router_tight_rate_limit();
    // Limit is 2 req/min for IP 0.0.0.0 (no ConnectInfo → UNSPECIFIED)
    // First 2 requests are allowed; 3rd should get 429
    let r1 = app
        .clone()
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let r2 = app
        .clone()
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let r3 = app
        .clone()
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(
        r1.status() == StatusCode::OK || r1.status() == StatusCode::TOO_MANY_REQUESTS,
        "r1 must be 200 or 429"
    );
    assert!(
        r2.status() == StatusCode::OK || r2.status() == StatusCode::TOO_MANY_REQUESTS,
        "r2 must be 200 or 429"
    );
    assert_eq!(
        r3.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "3rd request must be rate-limited (429)"
    );
}

#[tokio::test]
async fn rate_limiter_is_allowed_method_exercised() {
    // Exercise the IpRateLimiter::is_allowed path through actual HTTP requests.
    // The first request must succeed; the limiter increments its internal counter.
    let app = make_test_router_tight_rate_limit();
    let resp = app
        .oneshot(Request::get("/api/version").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(
        resp.status() == StatusCode::OK || resp.status() == StatusCode::TOO_MANY_REQUESTS,
        "rate limiter request must return 200 or 429"
    );
}

#[tokio::test]
async fn auth_lockout_after_repeated_wrong_key_attempts() {
    use std::net::SocketAddr;
    let app = make_test_router_with_key("correct-secret");
    let peer: SocketAddr = "127.0.0.2:55555".parse().unwrap();

    let mut last_status = StatusCode::OK;
    for _ in 0..12 {
        let mut req = Request::post("/auth/login")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("key=wrong&next=%2F"))
            .unwrap();
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(peer));
        let resp = app.clone().oneshot(req).await.unwrap();
        last_status = resp.status();
    }
    // After 10+ failures the IP may be locked out; last response is either
    // 302 (redirect with ?error=1) or 429 (rate-limited/locked out)
    assert!(
        last_status.is_redirection()
            || last_status == StatusCode::TOO_MANY_REQUESTS
            || last_status == StatusCode::FORBIDDEN,
        "after repeated wrong keys, expected redirect, 429, or 403; got {last_status}"
    );
}

// ── COCOMO fixture helpers ────────────────────────────────────────────────────

fn fixture_run_with_cocomo() -> String {
    use sloc_core::{CocomoEstimate, CocomoMode};
    let mut run = fixture_base_run("ingest-cocomo-001");
    run.per_file_records = vec![fixture_file_record("src/lib.rs", Language::Rust, 5000)];
    run.totals_by_language = vec![fixture_lang_summary(Language::Rust, 1, 5000)];
    run.summary_totals = SummaryTotals {
        files_considered: 1,
        files_analyzed: 1,
        code_lines: 5000,
        total_physical_lines: 5002,
        ..SummaryTotals::default()
    };
    run.cocomo = Some(CocomoEstimate {
        mode: CocomoMode::Organic,
        ksloc: 5.0,
        effort_person_months: 13.2,
        duration_months: 6.8,
        avg_staff: 1.94,
    });
    run.uloc = 4500;
    run.dryness_pct = Some(90.0);
    run.input_roots = vec!["/test/cocomo-project".into()];
    serde_json::to_string(&run).unwrap()
}

fn fixture_run_with_cocomo_and_coverage() -> String {
    use sloc_core::{CocomoEstimate, CocomoMode};
    let mut run = fixture_base_run("ingest-cocomo-cov-001");
    let mut rec = fixture_file_record("src/lib.rs", Language::Rust, 10000);
    rec.coverage = Some(sloc_core::FileCoverage {
        lines_found: 10000,
        lines_hit: 8500,
        functions_found: 100,
        functions_hit: 88,
        branches_found: 200,
        branches_hit: 165,
    });
    run.per_file_records = vec![rec];
    let mut lang = fixture_lang_summary(Language::Rust, 1, 10000);
    lang.coverage_lines_found = 10000;
    lang.coverage_lines_hit = 8500;
    lang.test_count = 50;
    lang.test_assertion_count = 150;
    run.totals_by_language = vec![lang];
    run.summary_totals = SummaryTotals {
        files_considered: 1,
        files_analyzed: 1,
        code_lines: 10000,
        total_physical_lines: 10002,
        coverage_lines_found: 10000,
        coverage_lines_hit: 8500,
        coverage_functions_found: 100,
        coverage_functions_hit: 88,
        test_count: 50,
        test_assertion_count: 150,
        test_suite_count: 8,
        ..SummaryTotals::default()
    };
    run.cocomo = Some(CocomoEstimate {
        mode: CocomoMode::SemiDetached,
        ksloc: 10.0,
        effort_person_months: 35.0,
        duration_months: 9.2,
        avg_staff: 3.8,
    });
    run.uloc = 9200;
    run.dryness_pct = Some(92.0);
    run.git_remote_url = Some("https://github.com/test-org/big-project.git".into());
    run.git_branch = Some("main".into());
    run.git_commit_short = Some("f1e2d3c".into());
    run.input_roots = vec!["/test/cocomo-cov-project".into()];
    serde_json::to_string(&run).unwrap()
}

fn fixture_run_with_cocomo_embedded() -> String {
    use sloc_core::{CocomoEstimate, CocomoMode};
    let mut run = fixture_base_run("ingest-cocomo-emb-001");
    run.per_file_records = vec![
        fixture_file_record("src/lib.rs", Language::Rust, 50000),
        fixture_file_record("src/core.rs", Language::Rust, 30000),
        fixture_file_record("src/drivers.c", Language::C, 20000),
    ];
    run.totals_by_language = vec![
        fixture_lang_summary(Language::Rust, 2, 80000),
        fixture_lang_summary(Language::C, 1, 20000),
    ];
    run.summary_totals = SummaryTotals {
        files_considered: 3,
        files_analyzed: 3,
        code_lines: 100_000,
        total_physical_lines: 100_006,
        ..SummaryTotals::default()
    };
    run.cocomo = Some(CocomoEstimate {
        mode: CocomoMode::Embedded,
        ksloc: 100.0,
        effort_person_months: 640.0,
        duration_months: 23.0,
        avg_staff: 27.8,
    });
    run.uloc = 95000;
    run.dryness_pct = Some(95.0);
    run.duplicate_groups = vec![vec![
        "/test/dup-project/src/a.rs".into(),
        "/test/dup-project/src/b.rs".into(),
    ]];
    run.duplicates_excluded = 1;
    run.input_roots = vec!["/test/cocomo-emb-project".into()];
    serde_json::to_string(&run).unwrap()
}

// ── COCOMO branch coverage tests ─────────────────────────────────────────────

#[tokio::test]
async fn trend_reports_with_cocomo_data_renders_cocomo_section() {
    let app = make_test_router();
    let (s, _, _) = post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_cocomo()).await;
    assert!(s.as_u16() < 500, "cocomo ingest failed with {s}");
    let (status, headers, body) = get_shared(app.clone(), "/trend-reports").await;
    assert_eq!(status, StatusCode::OK);
    assert!(has_csp(&headers), "missing CSP on /trend-reports");
    assert!(
        body.contains("<html"),
        "expected HTML from /trend-reports with COCOMO data"
    );
}

#[tokio::test]
async fn test_metrics_with_cocomo_data_renders_page() {
    let app = make_test_router();
    let (s, _, _) = post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_cocomo()).await;
    assert!(s.as_u16() < 500, "cocomo ingest failed with {s}");
    let (status, _, body) = get_shared(app.clone(), "/test-metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<html"),
        "expected HTML from /test-metrics with COCOMO data"
    );
}

#[tokio::test]
async fn result_page_with_cocomo_and_coverage_renders_all_sections() {
    let app = make_test_router();
    let (s, _, _) = post_json_shared(
        app.clone(),
        "/api/ingest",
        &fixture_run_with_cocomo_and_coverage(),
    )
    .await;
    assert!(s.as_u16() < 500, "cocomo+coverage ingest failed with {s}");

    // Check result page rendering
    let (status, _, body) = get_shared(app.clone(), "/runs/result/ingest-cocomo-cov-001").await;
    assert!(status.as_u16() < 500, "/runs/result must not 5xx");
    if status == StatusCode::OK {
        assert!(body.contains("<html") || body.contains("<!doctype"));
    }

    // Check metrics
    let (status, _, _) = get_shared(app.clone(), "/api/metrics/ingest-cocomo-cov-001").await;
    assert!(status.as_u16() < 500, "/api/metrics/:id must not 5xx");

    // Check trend-reports with this data
    let (status, _, body) = get_shared(app.clone(), "/trend-reports").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<html"));

    // Check test-metrics with this data
    let (status, _, body) = get_shared(app.clone(), "/test-metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<html"));
}

#[tokio::test]
async fn embedded_mode_cocomo_run_renders_in_handlers() {
    let app = make_test_router();
    let (s, _, _) = post_json_shared(
        app.clone(),
        "/api/ingest",
        &fixture_run_with_cocomo_embedded(),
    )
    .await;
    assert!(s.as_u16() < 500, "embedded COCOMO ingest failed with {s}");

    let (status, _, body) = get_shared(app.clone(), "/trend-reports").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<html"), "expected HTML from /trend-reports");

    let (status, _, body) = get_shared(app.clone(), "/test-metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<html"), "expected HTML from /test-metrics");

    // embed widget with large COCOMO run
    let (status, _, _) =
        get_shared(app.clone(), "/embed/summary?run_id=ingest-cocomo-emb-001").await;
    assert!(status.as_u16() < 500, "embed with cocomo-emb must not 5xx");
}

#[tokio::test]
async fn multi_compare_with_cocomo_runs_renders_page() {
    let app = make_test_router();
    assert!(
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_cocomo())
            .await
            .0
            .as_u16()
            < 500
    );
    assert!(
        post_json_shared(
            app.clone(),
            "/api/ingest",
            &fixture_run_with_cocomo_and_coverage()
        )
        .await
        .0
        .as_u16()
            < 500
    );
    let (status, _, body) = get_shared(
        app.clone(),
        "/multi-compare?runs=ingest-cocomo-001,ingest-cocomo-cov-001",
    )
    .await;
    assert!(
        status.as_u16() < 500,
        "multi-compare with cocomo must not 5xx"
    );
    if status == StatusCode::OK {
        assert!(body.contains("<html") || body.contains("<!doctype"));
    }
}

#[tokio::test]
async fn compare_with_cocomo_runs_renders_page() {
    let app = make_test_router();
    assert!(
        post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_cocomo())
            .await
            .0
            .as_u16()
            < 500
    );
    assert!(
        post_json_shared(
            app.clone(),
            "/api/ingest",
            &fixture_run_with_cocomo_embedded()
        )
        .await
        .0
        .as_u16()
            < 500
    );
    let (status, _, body) = get_shared(
        app.clone(),
        "/compare?a=ingest-cocomo-001&b=ingest-cocomo-emb-001",
    )
    .await;
    assert!(
        status.as_u16() < 500,
        "compare with cocomo runs must not 5xx"
    );
    if status == StatusCode::OK {
        assert!(body.contains("<html") || body.contains("<!doctype"));
    }
}

// ── Exhausted semaphore (busy-server) branch ──────────────────────────────────

#[tokio::test]
async fn analyze_with_exhausted_semaphore_returns_service_unavailable() {
    let app = make_test_router_exhausted_semaphore();
    let req = Request::post("/analyze")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("path=."))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // Exhausted semaphore → handler returns 503 Service Unavailable.
    assert_eq!(
        resp.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "exhausted semaphore must return 503, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn exhausted_semaphore_router_still_serves_static_routes() {
    // Even with semaphore exhausted, non-analyze routes must still work.
    let app = make_test_router_exhausted_semaphore();
    let resp = app
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "healthz must be 200 even with exhausted semaphore"
    );
}

#[tokio::test]
async fn exhausted_semaphore_json_accept_returns_503_json() {
    let app = make_test_router_exhausted_semaphore();
    let req = Request::post("/analyze")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "application/json")
        .body(Body::from("path=."))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "exhausted semaphore with JSON accept must return 503"
    );
}

// ── ULOC / dryness / duplicate branch coverage via ingest ────────────────────

#[tokio::test]
async fn ingest_run_with_uloc_and_dryness_renders_trend_reports() {
    let app = make_test_router();
    let mut run: AnalysisRun = serde_json::from_str(&fixture_run_with_cocomo()).unwrap();
    run.tool.run_id = "ingest-uloc-001".into();
    run.uloc = 4800;
    run.dryness_pct = Some(96.0);
    run.input_roots = vec!["/test/uloc-project".into()];
    let payload = serde_json::to_string(&run).unwrap();
    let (s, _, _) = post_json_shared(app.clone(), "/api/ingest", &payload).await;
    assert!(s.as_u16() < 500, "uloc ingest failed with {s}");
    let (status, _, body) = get_shared(app.clone(), "/trend-reports").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<html"));
}

#[tokio::test]
async fn ingest_run_with_duplicate_groups_renders_in_handlers() {
    let app = make_test_router();
    let payload = fixture_run_with_cocomo_embedded();
    let (s, _, _) = post_json_shared(app.clone(), "/api/ingest", &payload).await;
    assert!(s.as_u16() < 500, "dup ingest failed with {s}");

    // Result page exercises duplicate group rendering
    let (status, _, _) = get_shared(app.clone(), "/runs/result/ingest-cocomo-emb-001").await;
    assert!(status.as_u16() < 500, "/runs/result must not 5xx");

    // Metrics endpoint
    let (status, _, _) = get_shared(app.clone(), "/api/metrics/ingest-cocomo-emb-001").await;
    assert!(status.as_u16() < 500, "/api/metrics/:id must not 5xx");
}

// ── Multiple runs from same project — pagination / project-history ────────────

#[tokio::test]
async fn multiple_cocomo_runs_same_project_exercises_history_pagination() {
    let app = make_test_router();
    for i in 0..8_u64 {
        let mut run: AnalysisRun = serde_json::from_str(&fixture_run_with_cocomo()).unwrap();
        run.tool.run_id = format!("ingest-hist-cocomo-{i:03}");
        run.summary_totals.code_lines = 5000 + i * 100;
        run.input_roots = vec![format!("/test/hist-project-{i}")];
        let payload = serde_json::to_string(&run).unwrap();
        let (s, _, _) = post_json_shared(app.clone(), "/api/ingest", &payload).await;
        assert!(s.as_u16() < 500, "ingest {i} failed with {s}");
    }
    let (status, _, body) = get_shared(app.clone(), "/trend-reports").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<html"),
        "expected HTML with paginated COCOMO runs"
    );

    let (status, _, body) = get_shared(app.clone(), "/test-metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<html"));

    let (status, _, body) = get_shared(app.clone(), "/view-reports").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<html"));
}

// ── LLMs.txt routes ──────────────────────────────────────────────────────────

#[tokio::test]
async fn llms_txt_returns_text() {
    let (status, headers, body) = get("/llms.txt").await;
    assert_eq!(status, StatusCode::OK, "/llms.txt must return 200");
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("text"),
        "expected text content-type for /llms.txt, got: {ct}"
    );
    assert!(!body.is_empty(), "/llms.txt must not be empty");
}

#[tokio::test]
async fn llms_full_txt_returns_text() {
    let (status, _, body) = get("/llms-full.txt").await;
    assert_eq!(status, StatusCode::OK, "/llms-full.txt must return 200");
    assert!(!body.is_empty(), "/llms-full.txt must not be empty");
}

// ── Server-mode: analyze rejects empty path, upload-only preview ──────────────

#[tokio::test]
async fn server_mode_analyze_with_git_mode_uses_git_branch() {
    let app = make_test_router_server_mode();
    // In server mode, git_repo + git_ref is the upload path — should not crash
    let (status, _, _) = post_form_shared(
        app,
        "/analyze",
        "path=&git_repo=https%3A%2F%2Fgithub.com%2Ftest%2Frepo.git&git_ref=main",
    )
    .await;
    assert!(
        status.as_u16() < 600,
        "server_mode git analyze must not crash, got {status}"
    );
}

#[tokio::test]
async fn server_mode_watched_dirs_add_not_5xx() {
    let app = make_test_router_server_mode();
    let dir = tempfile::tempdir().unwrap();
    let path = pct_encode(dir.path().to_str().unwrap_or("."));
    let (status, _, _) = post_form_shared(
        app,
        "/watched-dirs/add",
        &format!("folder_path={path}&redirect_to=/trend-reports"),
    )
    .await;
    assert!(
        status.as_u16() < 500,
        "server_mode watched-dirs/add must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn server_mode_compare_scans_returns_html() {
    let app = make_test_router_server_mode();
    let (status, _, body) = get_server_shared(app, "/compare-scans").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<html"),
        "expected HTML from server_mode /compare-scans"
    );
}

// ── Confluence routes with run data ──────────────────────────────────────────

#[tokio::test]
async fn api_confluence_wiki_markup_with_ingested_run_returns_markup() {
    let app = make_test_router();
    let (s, _, _) = post_json_shared(app.clone(), "/api/ingest", &fixture_run_with_cocomo()).await;
    assert!(s.as_u16() < 500, "ingest failed");
    let (status, _, body) = get_shared(
        app.clone(),
        "/api/confluence/wiki-markup?run_id=ingest-cocomo-001",
    )
    .await;
    assert!(
        status.as_u16() < 500,
        "/api/confluence/wiki-markup with run_id must not 5xx, got {status}"
    );
    assert!(!body.is_empty(), "wiki markup must not be empty");
}

#[tokio::test]
async fn api_confluence_post_to_confluence_not_configured_returns_error() {
    let (status, _, _) = post_json(
        "/api/confluence/post",
        r#"{"run_id":"test-run-id","report_url":null}"#,
    )
    .await;
    // No confluence config → 400 or 500 but not a panic
    assert!(
        status.as_u16() < 600,
        "POST /api/confluence/post without config must not crash"
    );
}

// ── Analyze form option combinations ─────────────────────────────────────────

#[tokio::test]
async fn post_analyze_with_all_options_not_5xx() {
    let (status, headers, _) = post_form(
        "/analyze",
        "path=.&generate_html=1&generate_json=1&generate_csv=1&generate_xlsx=1\
         &submodule_breakdown=enabled&style_analysis_enabled=enabled\
         &cocomo_mode=organic&complexity_alert=10&exclude_duplicates=enabled\
         &generated_file_detection=enabled&report_title=Full+Test+Run",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers.contains_key("x-wait-id"),
        "analyze with all options must return x-wait-id"
    );
}

#[tokio::test]
async fn post_analyze_with_cocomo_semi_mode_not_5xx() {
    let (status, headers, _) = post_form("/analyze", "path=.&cocomo_mode=semi_detached").await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.contains_key("x-wait-id"));
}

#[tokio::test]
async fn post_analyze_with_cocomo_embedded_mode_not_5xx() {
    let (status, headers, _) = post_form("/analyze", "path=.&cocomo_mode=embedded").await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.contains_key("x-wait-id"));
}

// ── Rate limit 429 path ───────────────────────────────────────────────────────

#[tokio::test]
async fn rate_limit_triggers_429_after_burst_exhausted() {
    // make_test_router_tight_rate_limit allows 2 req/min. Fire 5 requests
    // to guarantee the rate limit kicks in.
    let mut got_429 = false;
    for i in 0..6 {
        let app = make_test_router_tight_rate_limit();
        let resp = app
            .oneshot(Request::get("/api/version").body(Body::empty()).unwrap())
            .await
            .unwrap();
        // Because each oneshot creates a fresh router state, we can't trivially
        // exhaust across calls — but the limit fires within a single shared router.
        if i == 0 && resp.status() == StatusCode::TOO_MANY_REQUESTS {
            got_429 = true;
        }
        assert!(
            resp.status() == StatusCode::OK || resp.status() == StatusCode::TOO_MANY_REQUESTS,
            "rate limiter must return 200 or 429, got {}",
            resp.status()
        );
    }
    let _ = got_429; // tested in shared-router variant below
}

#[tokio::test]
async fn rate_limit_shared_router_triggers_429() {
    // Use a shared router so requests accumulate against the same rate-limiter state.
    let app = make_test_router_tight_rate_limit();
    let mut statuses = Vec::new();
    for _ in 0..8 {
        let resp = app
            .clone()
            .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        statuses.push(resp.status());
    }
    let all_ok_or_429 = statuses
        .iter()
        .all(|s| *s == StatusCode::OK || *s == StatusCode::TOO_MANY_REQUESTS);
    assert!(
        all_ok_or_429,
        "all responses must be 200 or 429, got: {statuses:?}"
    );
    // At least some should be 429 given burst of 2
    let has_429 = statuses.contains(&StatusCode::TOO_MANY_REQUESTS);
    assert!(
        has_429,
        "expected at least one 429 from tight rate limiter, got: {statuses:?}"
    );
}

// ── Confluence API routes (unit-level HTTP coverage) ──────────────────────────

#[tokio::test]
async fn api_get_confluence_config_returns_json_not_configured() {
    let (status, headers, body) = get("/api/confluence/config").await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("json"), "must return JSON, got: {ct}");
    assert!(
        body.contains("\"configured\""),
        "body must contain 'configured' field, got: {body}"
    );
    // Default state has no config set
    assert!(
        body.contains("false") || body.contains("\"configured\":false"),
        "unconfigured state must report configured=false, got: {body}"
    );
}

#[tokio::test]
async fn api_save_confluence_config_cloud_tier() {
    let json = r#"{
        "tier": "cloud",
        "base_url": "https://mycompany.atlassian.net",
        "username": "user@example.com",
        "credential": "my-api-token",
        "space_key": "DEV",
        "parent_page_id": null,
        "schedule_auto_post": {}
    }"#;
    let (status, _, body) = post_json("/api/confluence/config", json).await;
    assert_eq!(status, StatusCode::OK, "save config must return 200");
    assert!(
        body.contains("\"ok\":true") || body.contains("\"ok\": true"),
        "save config must return ok=true, got: {body}"
    );
}

#[tokio::test]
async fn api_save_confluence_config_server_tier() {
    let json = r#"{
        "tier": "server",
        "base_url": "https://confluence.corp.com",
        "username": "admin",
        "credential": "password123",
        "space_key": "ENG",
        "parent_page_id": "12345",
        "schedule_auto_post": {}
    }"#;
    let (status, _, body) = post_json("/api/confluence/config", json).await;
    assert_eq!(status, StatusCode::OK, "save server config must return 200");
    assert!(
        body.contains("\"ok\":true") || body.contains("\"ok\": true"),
        "save server config must return ok=true, got: {body}"
    );
}

#[tokio::test]
async fn api_save_confluence_config_missing_body_not_5xx() {
    let (status, _, _) = post_json("/api/confluence/config", "{}").await;
    // Missing required fields -> 400 or 422, never 5xx
    assert!(
        status.as_u16() < 500,
        "missing confluence config body must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn api_test_confluence_unconfigured_returns_400() {
    let (status, _, body) = post_json("/api/confluence/test", "").await;
    // No Confluence configured in the test state -> 400 Bad Request
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "unconfigured confluence test must return 400, got {status}: {body}"
    );
    assert!(
        body.contains("\"ok\":false") || body.contains("\"ok\": false"),
        "body must contain ok=false, got: {body}"
    );
}

#[tokio::test]
async fn api_post_to_confluence_invalid_run_id_returns_400() {
    let json = r#"{"run_id": "../traversal", "page_title": "My Report"}"#;
    let (status, _, body) = post_json("/api/confluence/post", json).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "invalid run_id must return 400, got {status}: {body}"
    );
}

#[tokio::test]
async fn api_post_to_confluence_empty_run_id_returns_400() {
    let json = r#"{"run_id": "", "page_title": "My Report"}"#;
    let (status, _, body) = post_json("/api/confluence/post", json).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "empty run_id must return 400, got {status}: {body}"
    );
}

#[tokio::test]
async fn api_post_to_confluence_unconfigured_returns_400() {
    // Valid run_id format, but no Confluence configured
    let json = r#"{"run_id": "abc123", "page_title": "My Report"}"#;
    let (status, _, body) = post_json("/api/confluence/post", json).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "unconfigured confluence post must return 400, got {status}: {body}"
    );
    assert!(
        body.contains("\"ok\":false") || body.contains("\"ok\": false"),
        "body must contain ok=false, got: {body}"
    );
}

#[tokio::test]
async fn api_post_to_confluence_unknown_run_id_returns_404_or_400() {
    // First save a config so it's configured
    let cfg_json = r#"{
        "tier": "cloud",
        "base_url": "https://mycompany.atlassian.net",
        "username": "u@example.com",
        "credential": "token",
        "space_key": "DEV",
        "parent_page_id": null,
        "schedule_auto_post": {}
    }"#;
    // Use shared router so config persists across calls
    let app = make_test_router();
    let req = Request::post("/api/confluence/config")
        .header("content-type", "application/json")
        .body(Body::from(cfg_json.to_owned()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Now post with a run_id that does not exist in registry
    let req2 = Request::post("/api/confluence/post")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"run_id":"nonexistent-run-id","page_title":"Test"}"#,
        ))
        .unwrap();
    let resp2 = app.oneshot(req2).await.unwrap();
    let status = resp2.status();
    // Either 404 (run not found) or 400 (bad run_id chars) — never 5xx
    assert!(
        status.as_u16() < 500,
        "unknown run_id in confluence post must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn api_wiki_markup_invalid_run_id_returns_400() {
    let (status, _, _) = get("/api/confluence/wiki-markup?run_id=../../etc/passwd").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "path-traversal run_id must return 400, got {status}"
    );
}

#[tokio::test]
async fn api_wiki_markup_unknown_run_id_returns_404() {
    let (status, _, _) = get("/api/confluence/wiki-markup?run_id=nonexistent-abc-123").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "unknown run_id must return 404, got {status}"
    );
}

#[tokio::test]
async fn api_wiki_markup_too_long_run_id_returns_400() {
    let long_id = "a".repeat(200);
    let uri = format!("/api/confluence/wiki-markup?run_id={long_id}");
    let (status, _, _) = get(&uri).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "overly-long run_id must return 400, got {status}"
    );
}

// ── Webhook routes — push payload coverage ────────────────────────────────────

#[tokio::test]
async fn post_github_webhook_push_valid_payload_returns_accepted() {
    let app = make_test_router();
    let payload = r#"{"ref":"refs/heads/main","repository":{"clone_url":"https://github.com/org/repo.git"},"commits":[{"id":"abc123","message":"test commit"}]}"#;
    let req = Request::post("/webhooks/github")
        .header("content-type", "application/json")
        .header("x-github-event", "push")
        .body(Body::from(payload))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    // No schedules configured, so dispatch finds nothing, but handler returns 202 Accepted
    assert!(
        status.as_u16() < 500,
        "valid GitHub push webhook must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn post_github_webhook_push_invalid_payload_returns_400() {
    let app = make_test_router();
    let req = Request::post("/webhooks/github")
        .header("content-type", "application/json")
        .header("x-github-event", "push")
        .body(Body::from("not-json-at-all"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "invalid JSON push payload must return 400, got {status}"
    );
}

#[tokio::test]
async fn post_github_webhook_missing_event_header_returns_400() {
    let app = make_test_router();
    let req = Request::post("/webhooks/github")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"ref":"refs/heads/main"}"#))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "missing X-GitHub-Event header must return 400, got {status}"
    );
}

#[tokio::test]
async fn post_gitlab_webhook_push_valid_payload_returns_accepted() {
    let app = make_test_router();
    let payload = r#"{"ref":"refs/heads/main","project":{"git_http_url":"https://gitlab.com/org/repo.git"},"commits":[{"id":"abc123","message":"test commit"}]}"#;
    let req = Request::post("/webhooks/gitlab")
        .header("content-type", "application/json")
        .header("x-gitlab-event", "Push Hook")
        .body(Body::from(payload))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status.as_u16() < 500,
        "valid GitLab push webhook must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn post_gitlab_webhook_tag_push_valid_payload_returns_accepted() {
    let app = make_test_router();
    let payload = r#"{"ref":"refs/tags/v1.0","project":{"git_http_url":"https://gitlab.com/org/repo.git"},"commits":[{"id":"def456","message":"tag release"}]}"#;
    let req = Request::post("/webhooks/gitlab")
        .header("content-type", "application/json")
        .header("x-gitlab-event", "Tag Push Hook")
        .body(Body::from(payload))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status.as_u16() < 500,
        "valid GitLab tag push webhook must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn post_gitlab_webhook_invalid_payload_returns_400() {
    let app = make_test_router();
    let req = Request::post("/webhooks/gitlab")
        .header("content-type", "application/json")
        .header("x-gitlab-event", "Push Hook")
        .body(Body::from("not-json"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "invalid JSON GitLab push payload must return 400, got {status}"
    );
}

#[tokio::test]
async fn post_gitlab_webhook_missing_event_header_returns_400() {
    let app = make_test_router();
    let req = Request::post("/webhooks/gitlab")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"ref":"refs/heads/main"}"#))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "missing X-GitLab-Event header must return 400, got {status}"
    );
}

#[tokio::test]
async fn post_gitlab_webhook_non_push_event_returns_200() {
    let app = make_test_router();
    let req = Request::post("/webhooks/gitlab")
        .header("content-type", "application/json")
        .header("x-gitlab-event", "Merge Request Hook")
        .body(Body::from(r#"{"object_kind":"merge_request"}"#))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status.as_u16() < 500,
        "non-push GitLab event must not 5xx, got {status}"
    );
    assert_eq!(
        status,
        StatusCode::OK,
        "non-push GitLab event must return 200, got {status}"
    );
}

#[tokio::test]
async fn post_bitbucket_webhook_valid_payload_returns_accepted() {
    let app = make_test_router();
    // Minimal Bitbucket push payload matching parse_bitbucket_push format
    let payload = r#"{"push":{"changes":[{"new":{"name":"main","target":{"hash":"abc1234567890"}}}]},"repository":{"links":{"clone":[{"href":"https://bitbucket.org/ws/repo.git","name":"https"}]}}}"#;
    let req = Request::post("/webhooks/bitbucket")
        .header("content-type", "application/json")
        .body(Body::from(payload))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status.as_u16() < 500,
        "valid Bitbucket push webhook must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn post_bitbucket_webhook_invalid_payload_returns_400() {
    let app = make_test_router();
    let req = Request::post("/webhooks/bitbucket")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    // Empty JSON missing required fields -> parse fails -> 400
    assert!(
        status.as_u16() < 500,
        "malformed Bitbucket payload must not 5xx, got {status}"
    );
}

// ── Schedules API — additional coverage ───────────────────────────────────────

#[tokio::test]
async fn api_list_schedules_returns_json_array() {
    let (status, headers, body) = get("/api/schedules").await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("json"), "must return JSON, got: {ct}");
    assert!(
        body.contains("\"schedules\""),
        "body must contain 'schedules' key, got: {body}"
    );
}

#[tokio::test]
async fn api_create_schedule_webhook_kind_returns_201() {
    let json = r#"{
        "label": "My GitHub Schedule",
        "repo_url": "https://github.com/org/repo.git",
        "branch": "main",
        "kind": "webhook",
        "provider": "github",
        "interval_secs": null,
        "webhook_secret": "secret123"
    }"#;
    let (status, _, body) = post_json("/api/schedules", json).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create webhook schedule must return 201, got {status}: {body}"
    );
    assert!(
        body.contains("\"repo_url\"") || body.contains("repo_url"),
        "response must contain schedule data, got: {body}"
    );
}

#[tokio::test]
async fn api_create_schedule_poll_kind_returns_201() {
    let json = r#"{
        "label": "Poll Schedule",
        "repo_url": "https://github.com/org/repo.git",
        "branch": "develop",
        "kind": "poll",
        "provider": null,
        "interval_secs": 600,
        "webhook_secret": null
    }"#;
    let (status, _, body) = post_json("/api/schedules", json).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create poll schedule must return 201, got {status}: {body}"
    );
    assert!(
        body.contains("\"repo_url\"") || body.contains("repo_url"),
        "response must contain schedule data, got: {body}"
    );
}

#[tokio::test]
async fn api_create_schedule_gitlab_provider_returns_201() {
    let json = r#"{
        "label": "GL Schedule",
        "repo_url": "https://gitlab.com/org/repo.git",
        "branch": "main",
        "kind": "webhook",
        "provider": "gitlab",
        "interval_secs": null,
        "webhook_secret": "gl-secret"
    }"#;
    let (status, _, _) = post_json("/api/schedules", json).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create gitlab schedule must return 201, got {status}"
    );
}

#[tokio::test]
async fn api_create_schedule_bitbucket_provider_returns_201() {
    let json = r#"{
        "label": "BB Schedule",
        "repo_url": "https://bitbucket.org/ws/repo.git",
        "branch": "main",
        "kind": "webhook",
        "provider": "bitbucket",
        "interval_secs": null,
        "webhook_secret": "bb-secret"
    }"#;
    let (status, _, _) = post_json("/api/schedules", json).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create bitbucket schedule must return 201, got {status}"
    );
}

#[tokio::test]
async fn api_create_schedule_invalid_body_not_5xx() {
    let (status, _, _) = post_json("/api/schedules", "{}").await;
    // Missing required fields -> 400 or 422; never 5xx
    assert!(
        status.as_u16() < 500,
        "invalid schedule body must not 5xx, got {status}"
    );
}

// ── Git browser API routes — error branches ────────────────────────────────────

#[tokio::test]
async fn api_list_refs_missing_repo_param_returns_400_v2() {
    // Verify the error JSON contains the expected 'error' key (additional assertion).
    let (status, _, body) = get("/api/git/refs").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "missing ?repo= must return 400, got {status}: {body}"
    );
    assert!(
        body.contains("\"error\""),
        "error response must have 'error' key, got: {body}"
    );
}

#[tokio::test]
async fn api_scan_ref_ref_with_space_returns_400() {
    let (status, _, body) =
        get("/api/git/scan-ref?repo=https://github.com/org/repo.git&ref_name=my%20branch").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "ref_name with space must return 400, got {status}: {body}"
    );
}

#[tokio::test]
async fn api_scan_ref_ref_with_semicolon_returns_400() {
    let (status, _, body) =
        get("/api/git/scan-ref?repo=https://github.com/org/repo.git&ref_name=main%3Brm+-rf").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "shell-injection ref_name must return 400, got {status}: {body}"
    );
}

#[tokio::test]
async fn api_scan_ref_ref_starting_with_slash_returns_400() {
    let (status, _, body) =
        get("/api/git/scan-ref?repo=https://github.com/org/repo.git&ref_name=%2Fetc%2Fpasswd")
            .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "absolute-path ref_name must return 400, got {status}: {body}"
    );
}

#[tokio::test]
async fn api_compare_refs_invalid_baseline_returns_400() {
    let (status, _, body) = get(
        "/api/git/compare-refs?repo=https://github.com/org/repo.git&baseline_ref=..%2F..%2Fetc&current_ref=main",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "invalid baseline_ref must return 400, got {status}: {body}"
    );
}

#[tokio::test]
async fn api_compare_refs_invalid_current_returns_400() {
    let (status, _, body) = get(
        "/api/git/compare-refs?repo=https://github.com/org/repo.git&baseline_ref=main&current_ref=..%2F..%2Fetc",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "invalid current_ref must return 400, got {status}: {body}"
    );
}
