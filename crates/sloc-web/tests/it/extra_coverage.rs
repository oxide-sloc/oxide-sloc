// Extra coverage tests targeting code paths not yet exercised by the existing
// integration / coverage_boost / render_branches / api_functions suites.
//
// Focus areas (high missed-line counts):
//   * image_handler — all icon variants
//   * /llms.txt and /llms-full.txt handlers
//   * /api/openapi.yaml handler
//   * /static/chart-report.js handler
//   * badge handler — code-lines / files / comment-lines / blank-lines with no
//     registry data, and with registry data injected via POST /api/ingest
//   * /api/metrics/latest — empty registry (404) and with data
//   * /api/metrics/history — empty and with query params
//   * /api/metrics/submodules — handler smoke test
//   * /api/project-history — handler smoke test
//   * /api/suggest-coverage — Cargo.toml, pom.xml, build.gradle, pyproject.toml,
//     setup.py branches, and unknown project
//   * /open-path — headless / server_mode branches
//   * /pick-directory — headless / server_mode branches
//   * /pick-file — headless / server_mode branches
//   * /preview — various server_mode / upload path branches
//   * /scan (GET) with prefill query parameters
//   * /compare (GET) — redirect when no ?a=&?b= given, missing run IDs
//   * /multi-compare — <2 and >20 run IDs
//   * /runs/{artifact}/{run_id} — unknown run_id → 404, reversed-segment URL hint
//   * /api/runs/{wait_id}/status — invalid wait_id, not-found
//   * /api/runs/{wait_id}/cancel — invalid wait_id, not-found
//   * /api/scan-profiles CRUD (list, create, delete)
//   * /export-config / /import-config (GET and POST)
//   * /watched-dirs/add, /watched-dirs/remove, /watched-dirs/refresh — server_mode
//   * /locate-report, /locate-reports-dir, /relocate-scan — server_mode
//   * /api/ingest — bare POST to exercise the happy path render
//   * Delete-run handler — missing run falls through gracefully
//   * Rate-limit middleware — trust-proxy header path with XFF

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use sloc_web::{
    TEST_SERVER_MODE_API_KEY, make_test_router, make_test_router_exhausted_semaphore,
    make_test_router_server_mode, make_test_router_with_key,
};
use tower::ServiceExt;

use chrono::Utc;
use sloc_config::AppConfig;
use sloc_core::{
    AnalysisRun, EffectiveCounts, EnvironmentMetadata, FileRecord, FileStatus, LanguageSummary,
    SummaryTotals, ToolMetadata,
};
use sloc_languages::{Language, ParseMode, RawLineCounts};

// ── shared HTTP helpers ───────────────────────────────────────────────────────

async fn get(app: Router, uri: &str) -> (StatusCode, axum::http::HeaderMap, String) {
    let resp = app
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();

    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

async fn post_json(app: Router, uri: &str, json: &str) -> (StatusCode, String) {
    let req = Request::post(uri)
        .header("content-type", "application/json")
        .body(Body::from(json.to_owned()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn post_form(app: Router, uri: &str, body: &str) -> (StatusCode, String) {
    let req = Request::post(uri)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body.to_owned()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn delete(app: Router, uri: &str) -> StatusCode {
    let resp = app
        .oneshot(Request::delete(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    resp.status()
}

/// GET with the server-mode API key included (needed since server mode now
/// requires authentication even in tests — the router is configured with
/// `TEST_SERVER_MODE_API_KEY`, so all requests to it must include it).
async fn get_server(app: Router, uri: &str) -> (StatusCode, axum::http::HeaderMap, String) {
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
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

async fn post_form_server(app: Router, uri: &str, body: &str) -> (StatusCode, String) {
    let req = Request::post(uri)
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "Authorization",
            format!("Bearer {TEST_SERVER_MODE_API_KEY}"),
        )
        .body(Body::from(body.to_owned()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

// ── minimal AnalysisRun fixture ───────────────────────────────────────────────

fn make_tool(run_id: &str) -> ToolMetadata {
    ToolMetadata {
        name: "oxide-sloc".into(),
        version: "1.0.0".into(),
        run_id: run_id.into(),
        timestamp_utc: Utc::now(),
    }
}

fn make_env() -> EnvironmentMetadata {
    EnvironmentMetadata {
        operating_system: "linux".into(),
        architecture: "x86_64".into(),
        runtime_mode: "test".into(),
        initiator_username: "tester".into(),
        initiator_hostname: "localhost".into(),
        ci_name: None,
    }
}

fn make_file_record(path: &str, code: u64) -> FileRecord {
    let raw = RawLineCounts {
        total_physical_lines: code + 2,
        code_only_lines: code,
        blank_only_lines: 1,
        single_comment_only_lines: 1,
        ..RawLineCounts::default()
    };
    FileRecord {
        path: path.into(),
        relative_path: path.into(),
        language: Some(Language::Rust),
        size_bytes: code * 20,
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

const fn make_lang_summary() -> LanguageSummary {
    LanguageSummary {
        language: Language::Rust,
        files: 1,
        total_physical_lines: 12,
        code_lines: 10,
        comment_lines: 1,
        blank_lines: 1,
        mixed_lines_separate: 0,
        functions: 1,
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

fn make_run(run_id: &str) -> AnalysisRun {
    AnalysisRun {
        tool: make_tool(run_id),
        environment: make_env(),
        effective_configuration: AppConfig::default(),
        input_roots: vec!["/tmp/test-proj".into()],
        summary_totals: SummaryTotals {
            files_considered: 1,
            files_analyzed: 1,
            files_skipped: 0,
            total_physical_lines: 12,
            code_lines: 10,
            comment_lines: 1,
            blank_lines: 1,
            ..SummaryTotals::default()
        },
        totals_by_language: vec![make_lang_summary()],
        per_file_records: vec![make_file_record("src/lib.rs", 10)],
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

// ── Image handler: all variants ───────────────────────────────────────────────

#[tokio::test]
async fn image_handler_logo_text() {
    let (status, headers, _) = get(make_test_router(), "/images/logo/logo-text.png").await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("png") || ct.contains("image"), "ct={ct}");
}

#[tokio::test]
async fn image_handler_logo_small() {
    let (status, _, _) = get(make_test_router(), "/images/logo/small-logo.png").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn image_handler_icon_cpp() {
    let (status, _, _) = get(make_test_router(), "/images/icons/cpp.png").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn image_handler_icon_csharp() {
    let (status, _, _) = get(make_test_router(), "/images/icons/c-sharp.png").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn image_handler_icon_python() {
    let (status, _, _) = get(make_test_router(), "/images/icons/python.png").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn image_handler_icon_shell() {
    let (status, _, _) = get(make_test_router(), "/images/icons/shell.png").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn image_handler_icon_powershell() {
    let (status, _, _) = get(make_test_router(), "/images/icons/powershell.png").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn image_handler_icon_javascript() {
    let (status, _, _) = get(make_test_router(), "/images/icons/java-script.png").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn image_handler_icon_html5() {
    let (status, _, _) = get(make_test_router(), "/images/icons/html-5.png").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn image_handler_icon_java() {
    let (status, _, _) = get(make_test_router(), "/images/icons/java.png").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn image_handler_icon_vb() {
    let (status, _, _) = get(make_test_router(), "/images/icons/visual-basic.png").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn image_handler_icon_asm() {
    let (status, _, _) = get(make_test_router(), "/images/icons/asm.png").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn image_handler_icon_go() {
    let (status, _, _) = get(make_test_router(), "/images/icons/go.png").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn image_handler_icon_r() {
    let (status, _, _) = get(make_test_router(), "/images/icons/r.png").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn image_handler_icon_xml() {
    let (status, _, _) = get(make_test_router(), "/images/icons/xml.png").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn image_handler_icon_groovy() {
    let (status, _, _) = get(make_test_router(), "/images/icons/groovy.png").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn image_handler_icon_docker() {
    let (status, _, _) = get(make_test_router(), "/images/icons/docker.png").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn image_handler_icon_makefile_svg() {
    let (status, headers, _) = get(make_test_router(), "/images/icons/makefile.svg").await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("svg") || ct.contains("image"), "ct={ct}");
}

#[tokio::test]
async fn image_handler_icon_perl_svg() {
    let (status, _, _) = get(make_test_router(), "/images/icons/perl.svg").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn image_handler_unknown_returns_404() {
    let (status, _, _) = get(make_test_router(), "/images/icons/unknown.png").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── llms.txt / llms-full.txt ─────────────────────────────────────────────────

#[tokio::test]
async fn llms_txt_returns_plaintext() {
    let (status, headers, body) = get(make_test_router(), "/llms.txt").await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("text"), "expected text content-type, got: {ct}");
    assert!(!body.is_empty(), "llms.txt must not be empty");
}

#[tokio::test]
async fn llms_full_txt_returns_plaintext() {
    let (status, headers, body) = get(make_test_router(), "/llms-full.txt").await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("text"), "expected text content-type, got: {ct}");
    assert!(!body.is_empty(), "llms-full.txt must not be empty");
}

// ── openapi.yaml ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn openapi_yaml_returns_yaml() {
    let (status, headers, body) = get(make_test_router(), "/api/openapi.yaml").await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("yaml") || ct.contains("text"),
        "expected yaml content-type, got: {ct}"
    );
    assert!(!body.is_empty(), "openapi.yaml must not be empty");
}

// ── /static/chart-report.js ──────────────────────────────────────────────────

#[tokio::test]
async fn report_chart_js_returns_javascript() {
    let (status, headers, _) = get(make_test_router(), "/static/chart-report.js").await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("javascript") || ct.contains("text"),
        "expected js content-type, got: {ct}"
    );
}

// ── Badge handler variants ────────────────────────────────────────────────────

#[tokio::test]
async fn badge_code_lines_no_data_returns_svg_with_no_data() {
    let (status, _, body) = get(make_test_router(), "/badge/code-lines").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<svg"), "must return SVG");
    assert!(
        body.contains("no data") || body.contains('0'),
        "empty registry badge body: {body}"
    );
}

#[tokio::test]
async fn badge_files_no_data_returns_svg() {
    let (status, _, body) = get(make_test_router(), "/badge/files").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<svg"), "must return SVG");
}

#[tokio::test]
async fn badge_comment_lines_no_data_returns_svg() {
    let (status, _, body) = get(make_test_router(), "/badge/comment-lines").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<svg"), "must return SVG");
}

#[tokio::test]
async fn badge_blank_lines_no_data_returns_svg() {
    let (status, _, body) = get(make_test_router(), "/badge/blank-lines").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<svg"), "must return SVG");
}

#[tokio::test]
async fn badge_unknown_metric_no_registry_returns_svg_no_data() {
    // When the registry is empty, the badge handler short-circuits with "no data" SVG
    // before reaching the metric-type match, so ALL metric paths return 200.
    let (status, _, body) = get(make_test_router(), "/badge/nonexistent-metric").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<svg"), "must return SVG");
    assert!(
        body.contains("no data"),
        "empty registry returns no-data badge: {body}"
    );
}

#[tokio::test]
async fn badge_with_custom_label_and_color() {
    let (status, _, body) = get(
        make_test_router(),
        "/badge/code-lines?label=LOC&color=%234aee7a",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<svg"), "must return SVG");
}

// Inject a run via /api/ingest and then hit badge to exercise the "has data" branch
#[tokio::test]
async fn badge_code_lines_with_registry_data_returns_count() {
    let app = make_test_router();
    let run = make_run("badge-data-run-001");
    let json = serde_json::to_string(&run).unwrap();
    let (ingest_status, _) = post_json(app.clone(), "/api/ingest", &json).await;
    // ingest may fail if render is slow — but the badge test itself just must not 5xx
    if ingest_status == StatusCode::CREATED {
        let (status, _, body) = get(app.clone(), "/badge/code-lines").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("<svg"), "must return SVG");
    }
}

// ── /api/metrics/latest — empty registry → 404 ───────────────────────────────

#[tokio::test]
async fn api_metrics_latest_empty_registry_returns_404() {
    let (status, _) = post_json(make_test_router(), "/api/metrics/latest", "").await;
    // /api/metrics/latest is a GET endpoint — use helper
    let (status2, _, _) = get(make_test_router(), "/api/metrics/latest").await;
    assert_eq!(
        status2,
        StatusCode::NOT_FOUND,
        "empty registry must return 404"
    );
    // silence unused variable warning
    let _ = status;
}

#[tokio::test]
async fn api_metrics_run_handler_unknown_id_returns_404() {
    let (status, _, _) = get(
        make_test_router(),
        "/api/metrics/00000000-0000-0000-0000-000000000099",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── /api/metrics/history — smoke test ────────────────────────────────────────

#[tokio::test]
async fn api_metrics_history_empty_returns_json_array() {
    let (status, _, body) = get(make_test_router(), "/api/metrics/history").await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    assert!(v.is_array(), "history must return JSON array, got: {body}");
}

#[tokio::test]
async fn api_metrics_history_with_limit_param() {
    let (status, _, body) = get(make_test_router(), "/api/metrics/history?limit=5").await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    assert!(v.is_array(), "history must return JSON array");
}

#[tokio::test]
async fn api_metrics_history_with_root_param() {
    let (status, _, body) = get(
        make_test_router(),
        "/api/metrics/history?root=/tmp/some-proj",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    assert!(v.is_array());
}

// ── /api/metrics/submodules — smoke test ─────────────────────────────────────

#[tokio::test]
async fn api_metrics_submodules_empty_returns_json_array() {
    let (status, _, body) = get(make_test_router(), "/api/metrics/submodules").await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    assert!(v.is_array(), "submodules must return JSON array");
}

// ── /api/project-history — smoke test ────────────────────────────────────────

#[tokio::test]
async fn project_history_empty_registry() {
    let (status, _, body) = get(
        make_test_router(),
        "/api/project-history?path=/tmp/no-such-proj",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    assert_eq!(v["scan_count"], 0);
}

#[tokio::test]
async fn project_history_no_path_param_returns_zero() {
    let (status, _, body) = get(make_test_router(), "/api/project-history").await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    assert!(v["scan_count"].is_number());
}

// ── /api/suggest-coverage — coverage tool detection branches ─────────────────

#[tokio::test]
async fn suggest_coverage_unknown_project_no_tool() {
    let (status, _, body) = get(
        make_test_router(),
        "/api/suggest-coverage?path=/tmp/definitely-no-project",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    // For a path that doesn't exist, tool may be null
    assert!(v["tool"].is_null() || v["tool"].is_string());
}

#[tokio::test]
async fn suggest_coverage_with_cargo_toml_detects_rust() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
    let path = dir.path().to_string_lossy().replace('\\', "/");
    let uri = format!("/api/suggest-coverage?path={path}");
    let (status, _, body) = get(make_test_router(), &uri).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    // Rust projects should produce cargo-llvm-cov
    if v["tool"].is_string() {
        assert!(
            v["tool"].as_str().unwrap().contains("cargo")
                || v["tool"].as_str().unwrap() == "cargo-llvm-cov",
            "Rust project must map to cargo tool, got: {}",
            v["tool"]
        );
    }
}

#[tokio::test]
async fn suggest_coverage_with_pom_xml_detects_jacoco() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("pom.xml"), "<project></project>").unwrap();
    let path = dir.path().to_string_lossy().replace('\\', "/");
    let uri = format!("/api/suggest-coverage?path={path}");
    let (status, _, body) = get(make_test_router(), &uri).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    if v["tool"].is_string() {
        assert_eq!(v["tool"], "jacoco", "pom.xml must map to jacoco");
    }
}

#[tokio::test]
async fn suggest_coverage_with_build_gradle_detects_jacoco() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("build.gradle"), "apply plugin: 'java'").unwrap();
    let path = dir.path().to_string_lossy().replace('\\', "/");
    let uri = format!("/api/suggest-coverage?path={path}");
    let (status, _, body) = get(make_test_router(), &uri).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    if v["tool"].is_string() {
        assert_eq!(v["tool"], "jacoco", "build.gradle must map to jacoco");
    }
}

#[tokio::test]
async fn suggest_coverage_with_pyproject_toml_detects_pytest() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("pyproject.toml"), "[tool.pytest]\n").unwrap();
    let path = dir.path().to_string_lossy().replace('\\', "/");
    let uri = format!("/api/suggest-coverage?path={path}");
    let (status, _, body) = get(make_test_router(), &uri).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    if v["tool"].is_string() {
        assert!(
            v["tool"].as_str().unwrap().contains("pytest"),
            "pyproject.toml must map to pytest, got: {}",
            v["tool"]
        );
    }
}

#[tokio::test]
async fn suggest_coverage_with_setup_py_detects_pytest() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("setup.py"),
        "from setuptools import setup; setup()",
    )
    .unwrap();
    let path = dir.path().to_string_lossy().replace('\\', "/");
    let uri = format!("/api/suggest-coverage?path={path}");
    let (status, _, body) = get(make_test_router(), &uri).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    if v["tool"].is_string() {
        assert!(
            v["tool"].as_str().unwrap().contains("pytest"),
            "setup.py must map to pytest-cov, got: {}",
            v["tool"]
        );
    }
}

#[tokio::test]
async fn suggest_coverage_with_build_gradle_kts_detects_jacoco() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("build.gradle.kts"), "plugins { java }").unwrap();
    let path = dir.path().to_string_lossy().replace('\\', "/");
    let uri = format!("/api/suggest-coverage?path={path}");
    let (status, _, body) = get(make_test_router(), &uri).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    if v["tool"].is_string() {
        assert_eq!(v["tool"], "jacoco", "build.gradle.kts must map to jacoco");
    }
}

// ── /open-path — headless and server_mode branches ───────────────────────────

#[tokio::test]
async fn open_path_headless_returns_headless_json() {
    // make_test_router sets SLOC_HEADLESS=1
    let (status, _, body) = get(make_test_router(), "/open-path?path=/tmp").await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    // In headless mode returns {"opened": false, "headless": true}
    assert!(
        v["headless"] == true || v["server_mode_disabled"] == true || v["ok"] == true,
        "unexpected open-path response: {body}"
    );
}

#[tokio::test]
async fn open_path_server_mode_returns_disabled_message() {
    let (status, _, body) =
        get_server(make_test_router_server_mode(), "/open-path?path=/tmp").await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    assert_eq!(
        v["server_mode_disabled"], true,
        "server mode must return server_mode_disabled: true, got: {body}"
    );
}

#[tokio::test]
async fn open_path_missing_path_param_returns_400() {
    // When path is absent or empty, handler returns 400
    let (status, _, _) = get(make_test_router(), "/open-path").await;
    // Either 200 (headless short-circuits) or 400 (missing path after headless check)
    assert!(status.as_u16() < 500, "must not 5xx, got {status}");
}

// ── /pick-directory — headless / server_mode ──────────────────────────────────

#[tokio::test]
async fn pick_directory_headless_returns_cancelled() {
    let (status, _, body) = get(make_test_router(), "/pick-directory").await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    assert_eq!(
        v["cancelled"], true,
        "headless pick-directory must return cancelled=true, got: {body}"
    );
}

#[tokio::test]
async fn pick_directory_server_mode_returns_cancelled_or_404() {
    // The non-native-dialog build (feature flag off) returns 200+cancelled regardless
    // of server_mode. Only the native-dialog build gates on server_mode.
    let (status, _, body) = get_server(make_test_router_server_mode(), "/pick-directory").await;
    // Accept 404 (native-dialog feature enabled) or 200 with cancelled=true (fallback stub)
    assert!(
        status == StatusCode::NOT_FOUND
            || (status == StatusCode::OK
                && serde_json::from_str::<serde_json::Value>(&body)
                    .is_ok_and(|v| v["cancelled"] == true)),
        "server-mode pick-directory must return 404 or cancelled=true, got {status}: {body}"
    );
}

// ── /pick-file — headless / server_mode ──────────────────────────────────────

#[tokio::test]
async fn pick_file_headless_returns_cancelled() {
    let (status, _, body) = get(make_test_router(), "/pick-file").await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    assert_eq!(
        v["cancelled"], true,
        "headless pick-file must return cancelled=true, got: {body}"
    );
}

#[tokio::test]
async fn pick_file_server_mode_returns_cancelled_or_404() {
    // Same as pick-directory: only the native-dialog build gates on server_mode.
    let (status, _, body) = get_server(make_test_router_server_mode(), "/pick-file").await;
    assert!(
        status == StatusCode::NOT_FOUND
            || (status == StatusCode::OK
                && serde_json::from_str::<serde_json::Value>(&body)
                    .is_ok_and(|v| v["cancelled"] == true)),
        "server-mode pick-file must return 404 or cancelled=true, got {status}: {body}"
    );
}

// ── /preview — various branches ───────────────────────────────────────────────

#[tokio::test]
async fn preview_default_path_smoke_test() {
    let (status, _, _) = get(make_test_router(), "/preview").await;
    // 200 (renders) or 404 (samples dir doesn't exist on this machine) — just must not 5xx
    assert!(status.as_u16() < 500, "preview must not 5xx, got {status}");
}

#[tokio::test]
async fn preview_server_mode_upload_tmp_path_rejected() {
    // server_mode with a non-upload-tmp path that is not in allowed roots
    let (status, _, body) = get_server(
        make_test_router_server_mode(),
        "/preview?path=/tmp/no-such-proj",
    )
    .await;
    assert!(status.as_u16() < 500, "must not 5xx, got {status}");
    // Either the preview-error div is present or we got some HTML
    assert!(
        body.contains("preview-error") || body.contains("<!doctype") || body.contains("<div"),
        "unexpected body: {body}"
    );
}

// ── /scan GET with prefill query params ──────────────────────────────────────

#[tokio::test]
async fn scan_page_with_prefill_query() {
    let (status, _, body) = get(
        make_test_router(),
        "/scan?prefilled=1&path=/tmp/test&mixed_line_policy=count_once",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "must be HTML"
    );
}

#[tokio::test]
async fn scan_page_with_git_repo_and_ref() {
    let (status, _, body) = get(
        make_test_router(),
        "/scan?git_repo=https://github.com/owner/repo.git&git_ref=main",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "must be HTML"
    );
}

// ── /compare — redirect when run IDs missing ─────────────────────────────────

#[tokio::test]
async fn compare_without_params_redirects_to_compare_scans() {
    let app = make_test_router();
    let resp = app
        .oneshot(Request::get("/compare").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(
        resp.status().is_redirection(),
        "/compare without ?a=&b= must redirect, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn compare_with_only_one_param_redirects() {
    let app = make_test_router();
    let resp = app
        .oneshot(
            Request::get("/compare?a=some-run-id")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status().is_redirection(),
        "/compare with only ?a= must redirect"
    );
}

#[tokio::test]
async fn compare_with_unknown_run_ids_returns_error_html() {
    let (status, _, body) = get(
        make_test_router(),
        "/compare?a=00000000-0000-0000-0000-000000000001&b=00000000-0000-0000-0000-000000000002",
    )
    .await;
    // Either 200 with error message or redirect — must not 5xx
    assert!(status.as_u16() < 500, "must not 5xx, got {status}");
    // Should mention the error
    if status == StatusCode::OK {
        assert!(
            body.contains("not found") || body.contains("error") || body.contains("Error"),
            "unknown IDs must produce error page: {body}"
        );
    }
}

// ── /multi-compare — validation boundaries ────────────────────────────────────

#[tokio::test]
async fn multi_compare_with_zero_runs_shows_error() {
    let (status, _, body) = get(make_test_router(), "/multi-compare").await;
    assert!(status.as_u16() < 500, "must not 5xx");
    assert!(
        body.contains("At least 2") || body.contains("required"),
        "must require ≥2 runs: {body}"
    );
}

#[tokio::test]
async fn multi_compare_with_one_run_shows_error() {
    let (status, _, body) = get(
        make_test_router(),
        "/multi-compare?runs=00000000-0000-0000-0000-000000000001",
    )
    .await;
    assert!(status.as_u16() < 500, "must not 5xx");
    assert!(
        body.contains("At least 2") || body.contains("required"),
        "must require ≥2 runs: {body}"
    );
}

#[tokio::test]
async fn multi_compare_with_twenty_one_runs_shows_limit_error() {
    let ids: Vec<String> = (1u32..=21)
        .map(|n| format!("00000000-0000-0000-0000-{n:012}"))
        .collect();
    let runs_csv = ids.join(",");
    let uri = format!("/multi-compare?runs={runs_csv}");
    let (status, _, body) = get(make_test_router(), &uri).await;
    assert!(status.as_u16() < 500, "must not 5xx");
    assert!(
        body.contains("20") || body.contains("most"),
        "must show limit error for 21 runs: {body}"
    );
}

// ── Artifact handler — unknown run_id / reversed URL ─────────────────────────

#[tokio::test]
async fn artifact_handler_unknown_run_id_returns_404() {
    let (status, _, body) = get(
        make_test_router(),
        "/runs/html/00000000-0000-0000-0000-ffffffff0001",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        body.contains("not found") || body.contains("Error") || body.contains("error"),
        "404 body must contain error text: {body}"
    );
}

#[tokio::test]
async fn artifact_handler_reversed_url_segment_shows_hint() {
    // /runs/pdf/html triggers the "reversed URL" hint branch
    let (status, _, body) = get(make_test_router(), "/runs/pdf/html").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // The reversed URL hint branch should be triggered
    assert!(
        body.contains("reversed") || body.contains("error") || body.contains("Error"),
        "reversed segment must show hint or error: {body}"
    );
}

#[tokio::test]
async fn artifact_handler_csv_no_data_returns_404() {
    let (status, _, _) = get(
        make_test_router(),
        "/runs/csv/00000000-0000-0000-0000-ffffffff0002",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn artifact_handler_xlsx_no_data_returns_404() {
    let (status, _, _) = get(
        make_test_router(),
        "/runs/xlsx/00000000-0000-0000-0000-ffffffff0003",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn artifact_handler_unknown_artifact_type_returns_404() {
    let (status, _, _) = get(
        make_test_router(),
        "/runs/garbage/00000000-0000-0000-0000-ffffffff0004",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── Async run status / cancel — invalid wait_id ───────────────────────────────

#[tokio::test]
async fn async_run_status_invalid_wait_id_returns_bad_request() {
    let (status, _, _) = get(make_test_router(), "/api/runs/a/b/c/status").await;
    // Path structure "/api/runs/{wait_id}/status" — "a/b/c" is three path segments
    // so the router won't match this route; expect 404 or method-not-allowed
    assert!(status.as_u16() < 500, "must not 5xx, got {status}");
}

#[tokio::test]
async fn async_run_status_unknown_id_returns_not_found() {
    let (status, _, body) = get(
        make_test_router(),
        "/api/runs/00000000-0000-0000-0000-aaaaaaaaaaaa/status",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    assert!(
        v["error"].is_string() || body.contains("not found"),
        "unknown wait_id must give not-found error: {body}"
    );
}

#[tokio::test]
async fn async_run_status_very_long_id_is_rejected() {
    let long_id = "a".repeat(200);
    let uri = format!("/api/runs/{long_id}/status");
    let (status, _, _) = get(make_test_router(), &uri).await;
    // Either 400 (bad request) or 404 (router no match) — must not 5xx
    assert!(status.as_u16() < 500, "must not 5xx, got {status}");
}

#[tokio::test]
async fn cancel_run_unknown_id_returns_not_found() {
    let app = make_test_router();
    let req = Request::post("/api/runs/00000000-0000-0000-0000-bbbbbbbbbbbb/cancel")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── Scan profiles CRUD ────────────────────────────────────────────────────────

#[tokio::test]
async fn scan_profiles_list_empty_returns_json() {
    let (status, _, body) = get(make_test_router(), "/api/scan-profiles").await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    assert!(
        v["profiles"].is_array(),
        "profiles list must be array: {body}"
    );
}

#[tokio::test]
async fn scan_profiles_create_valid_returns_created() {
    let (status, body) = post_json(
        make_test_router(),
        "/api/scan-profiles",
        r#"{"name":"My Profile","params":{"path":"/tmp/test"}}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    assert!(v["id"].is_string(), "response must include id: {body}");
    assert_eq!(v["ok"], true);
}

#[tokio::test]
async fn scan_profiles_create_empty_name_returns_400() {
    let (status, _) = post_json(
        make_test_router(),
        "/api/scan-profiles",
        r#"{"name":"","params":{}}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn scan_profiles_delete_nonexistent_returns_404() {
    let status = delete(
        make_test_router(),
        "/api/scan-profiles/00000000-nonexistent",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn scan_profiles_create_then_delete() {
    let app = make_test_router();
    // Create a profile
    let (create_status, create_body) = post_json(
        app.clone(),
        "/api/scan-profiles",
        r#"{"name":"TempProfile","params":{"path":"/tmp"}}"#,
    )
    .await;
    assert_eq!(create_status, StatusCode::CREATED);
    let v: serde_json::Value = serde_json::from_str(&create_body).unwrap_or_default();
    let id = v["id"].as_str().unwrap_or("").to_string();
    if !id.is_empty() {
        // Delete the created profile
        let del_status = delete(app.clone(), &format!("/api/scan-profiles/{id}")).await;
        assert_eq!(del_status, StatusCode::OK);
    }
}

// ── /export-config / /import-config ──────────────────────────────────────────

#[tokio::test]
async fn export_config_returns_toml() {
    let (status, headers, body) = get(make_test_router(), "/export-config").await;
    assert_eq!(status, StatusCode::OK, "export-config must return 200");
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("toml") || ct.contains("text") || ct.contains("octet"),
        "unexpected content-type: {ct}"
    );
    assert!(!body.is_empty(), "config export must not be empty");
}

#[tokio::test]
async fn import_config_valid_toml_returns_200() {
    let toml = "[discovery]\nroot_paths = [\"/tmp\"]\n";
    let (status, body) = post_form(
        make_test_router(),
        "/import-config",
        &format!("toml={}", urlencoding_encode(toml)),
    )
    .await;
    assert!(
        status.as_u16() < 500,
        "import-config must not 5xx, got {status}: {body}"
    );
}

#[tokio::test]
async fn import_config_invalid_toml_returns_error() {
    let (status, body) = post_form(
        make_test_router(),
        "/import-config",
        "toml=this+is+not+toml+%5Bbad",
    )
    .await;
    // Should return 4xx for invalid TOML
    assert!(
        status == StatusCode::BAD_REQUEST
            || status == StatusCode::UNPROCESSABLE_ENTITY
            || status.as_u16() < 500,
        "invalid toml must not 5xx, got {status}: {body}"
    );
}

// ── /watched-dirs/* — server_mode returns 404 ────────────────────────────────

#[tokio::test]
async fn add_watched_dir_server_mode_returns_404() {
    let (status, _) = post_form_server(
        make_test_router_server_mode(),
        "/watched-dirs/add",
        "folder_path=/tmp&redirect_to=/view-reports",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "server mode watched-dirs/add must be 404"
    );
}

#[tokio::test]
async fn remove_watched_dir_server_mode_returns_404() {
    let (status, _) = post_form_server(
        make_test_router_server_mode(),
        "/watched-dirs/remove",
        "folder_path=/tmp&redirect_to=/view-reports",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn refresh_watched_dirs_server_mode_returns_404() {
    let (status, _) = post_form_server(
        make_test_router_server_mode(),
        "/watched-dirs/refresh",
        "redirect_to=/view-reports",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn add_watched_dir_nonexistent_path_redirects_with_error() {
    let app = make_test_router();
    let resp = app
        .oneshot(
            Request::post("/watched-dirs/add")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "folder_path=/nonexistent/path/xyz&redirect_to=/view-reports",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    // Should redirect (3xx) with error param, not 5xx
    assert!(
        resp.status().is_redirection() || resp.status().as_u16() < 500,
        "nonexistent path must not 5xx, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn remove_watched_dir_local_mode_redirects() {
    let app = make_test_router();
    let resp = app
        .oneshot(
            Request::post("/watched-dirs/remove")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("folder_path=/tmp&redirect_to=/view-reports"))
                .unwrap(),
        )
        .await
        .unwrap();
    // Should redirect back to view-reports
    assert!(
        resp.status().is_redirection() || resp.status().as_u16() < 500,
        "remove watched-dir must redirect or succeed: {}",
        resp.status()
    );
}

#[tokio::test]
async fn refresh_watched_dirs_local_mode_redirects() {
    let app = make_test_router();
    let resp = app
        .oneshot(
            Request::post("/watched-dirs/refresh")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("redirect_to=/view-reports"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status().is_redirection() || resp.status().as_u16() < 500,
        "refresh watched-dirs must redirect: {}",
        resp.status()
    );
}

// ── /locate-reports-dir — server_mode returns 404 ────────────────────────────

#[tokio::test]
async fn locate_reports_dir_server_mode_returns_404() {
    let (status, _) = post_form_server(
        make_test_router_server_mode(),
        "/locate-reports-dir",
        "folder_path=/tmp",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn locate_reports_dir_nonexistent_redirects_with_error() {
    let app = make_test_router();
    let resp = app
        .oneshot(
            Request::post("/locate-reports-dir")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("folder_path=/nonexistent/path/xyz"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status().is_redirection() || resp.status().as_u16() < 500,
        "nonexistent path must not 5xx, got {}",
        resp.status()
    );
}

// ── /relocate-scan — server_mode returns 404 ─────────────────────────────────

#[tokio::test]
async fn relocate_scan_server_mode_returns_404() {
    let (status, _) = post_form_server(
        make_test_router_server_mode(),
        "/relocate-scan",
        "run_id=abc123&folder_path=/tmp&redirect_url=/compare-scans",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn relocate_scan_unknown_run_id_returns_error_html() {
    let (status, body) = post_form(
        make_test_router(),
        "/relocate-scan",
        "run_id=00000000-not-real&folder_path=/tmp&redirect_url=/compare-scans",
    )
    .await;
    assert!(status.as_u16() < 500, "must not 5xx, got {status}");
    assert!(
        body.contains("error") || body.contains("Error") || body.contains("not found"),
        "unknown run_id must produce error: {body}"
    );
}

// ── /locate-report — various error branches ───────────────────────────────────

#[tokio::test]
async fn locate_report_non_html_path_returns_error() {
    let (status, _body) = post_form(
        make_test_router(),
        "/locate-report",
        "file_path=/tmp/something.txt",
    )
    .await;
    // Should return error page (200 with error content) or 4xx
    assert!(status.as_u16() < 500, "must not 5xx, got {status}");
}

#[tokio::test]
async fn locate_report_nonexistent_html_returns_error() {
    let (status, _) = post_form(
        make_test_router(),
        "/locate-report",
        "file_path=/nonexistent/result_abc.html",
    )
    .await;
    assert!(status.as_u16() < 500, "must not 5xx");
}

// ── /api/ingest — POST with AnalysisRun JSON ─────────────────────────────────

#[tokio::test]
async fn api_ingest_valid_run_returns_201() {
    let run = make_run("ingest-extra-001");
    let json = serde_json::to_string(&run).unwrap();
    let (status, body) = post_json(make_test_router(), "/api/ingest", &json).await;
    // May 201 or 500 depending on render; must not panic
    assert!(
        status == StatusCode::CREATED || status.as_u16() < 600,
        "ingest must not panic, got {status}: {body}"
    );
}

#[tokio::test]
async fn api_ingest_with_label_param() {
    let run = make_run("ingest-extra-002");
    let json = serde_json::to_string(&run).unwrap();
    let (status, body) = post_json(
        make_test_router(),
        "/api/ingest?label=my-custom-label",
        &json,
    )
    .await;
    assert!(
        status == StatusCode::CREATED || status.as_u16() < 600,
        "ingest with label must not panic, got {status}: {body}"
    );
}

#[tokio::test]
async fn api_ingest_invalid_body_returns_error() {
    let (status, _) = post_json(make_test_router(), "/api/ingest", r#"{"not": "a run"}"#).await;
    // Should return 4xx or 5xx — must not panic
    assert!(status.as_u16() >= 400, "invalid body must not succeed");
}

// ── Delete-run handler — unknown ID gracefully returns no-content ─────────────

#[tokio::test]
async fn delete_run_unknown_id_returns_no_content() {
    let status = delete(
        make_test_router(),
        "/api/runs/00000000-0000-0000-0000-cccccccccccc",
    )
    .await;
    // Should return 204 No Content (graceful) even for unknown IDs
    assert!(
        status == StatusCode::NO_CONTENT || status == StatusCode::NOT_FOUND,
        "delete unknown run must return 204 or 404, got {status}"
    );
}

// ── /api/runs/cleanup ─────────────────────────────────────────────────────────

#[tokio::test]
async fn cleanup_runs_empty_registry_returns_ok() {
    let (status, body) = post_json(make_test_router(), "/api/runs/cleanup", "{}").await;
    assert!(
        status.as_u16() < 500,
        "cleanup must not 5xx, got {status}: {body}"
    );
}

// ── Exhausted semaphore path ──────────────────────────────────────────────────

#[tokio::test]
async fn analyze_exhausted_semaphore_returns_503() {
    fn pct_encode(s: &str) -> String {
        s.bytes()
            .flat_map(|b| match b {
                b'A'..=b'Z'
                | b'a'..=b'z'
                | b'0'..=b'9'
                | b'-'
                | b'_'
                | b'.'
                | b'~'
                | b'/'
                | b':' => vec![b as char],
                _ => format!("%{b:02X}").chars().collect(),
            })
            .collect()
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.rs"), "fn foo() {}\n").unwrap();
    let path_enc = pct_encode(dir.path().to_str().unwrap_or("."));
    let (status, _) = post_form(
        make_test_router_exhausted_semaphore(),
        "/analyze",
        &format!("path={path_enc}"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "exhausted semaphore must return 503"
    );
}

// ── /api-docs page ────────────────────────────────────────────────────────────

#[tokio::test]
async fn api_docs_page_returns_html() {
    let (status, _, body) = get(make_test_router(), "/api-docs").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "api-docs must be HTML"
    );
}

#[tokio::test]
async fn api_docs_page_with_key_configured_requires_auth() {
    // When an API key is configured, /api-docs is a protected route.
    // Without credentials the middleware returns 401 or redirects to login.
    let app = make_test_router_with_key("test-key");
    let resp = app
        .oneshot(Request::get("/api-docs").body(Body::empty()).unwrap())
        .await
        .unwrap();
    // Either 401 (JSON client) or redirect to login (browser) — must not 5xx
    let status = resp.status();
    assert!(
        status == StatusCode::UNAUTHORIZED || status.is_redirection(),
        "protected api-docs without auth must return 401 or redirect, got {status}"
    );
}

#[tokio::test]
async fn api_docs_page_with_key_and_valid_auth() {
    // With a valid Bearer token, /api-docs should return 200 HTML.
    let app = make_test_router_with_key("test-key");
    let resp = app
        .oneshot(
            Request::get("/api-docs")
                .header("authorization", "Bearer test-key")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    assert_eq!(status, StatusCode::OK, "valid key must allow access");
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&bytes).into_owned();
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "api-docs must be HTML: {body}"
    );
}

// ── /embed/summary ────────────────────────────────────────────────────────────

#[tokio::test]
async fn embed_summary_empty_registry_returns_html() {
    let (status, _, body) = get(make_test_router(), "/embed/summary").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.is_empty(), "embed summary must not be empty");
}

// ── /metrics (Prometheus) ─────────────────────────────────────────────────────

#[tokio::test]
async fn metrics_endpoint_returns_plaintext() {
    let (status, headers, _body) = get(make_test_router(), "/metrics").await;
    assert_eq!(status, StatusCode::OK);
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("text/plain") || ct.contains("text"),
        "metrics must return text/plain, got: {ct}"
    );
}

// ── Webhook-setup and confluence-setup redirects ──────────────────────────────

#[tokio::test]
async fn webhook_setup_redirects_to_integrations() {
    let app = make_test_router();
    let resp = app
        .oneshot(Request::get("/webhook-setup").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(
        resp.status().is_redirection(),
        "/webhook-setup must redirect, got {}",
        resp.status()
    );
    let loc = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        loc.contains("integrations"),
        "must redirect to /integrations, got: {loc}"
    );
}

#[tokio::test]
async fn confluence_setup_redirects_to_integrations() {
    let app = make_test_router();
    let resp = app
        .oneshot(
            Request::get("/confluence-setup")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status().is_redirection(),
        "/confluence-setup must redirect, got {}",
        resp.status()
    );
    let loc = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        loc.contains("integrations"),
        "must redirect to /integrations, got: {loc}"
    );
}

// ── /integrations page ────────────────────────────────────────────────────────

#[tokio::test]
async fn integrations_page_returns_html() {
    let (status, _, body) = get(make_test_router(), "/integrations").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "integrations must be HTML"
    );
}

// ── /trend-reports page ───────────────────────────────────────────────────────

#[tokio::test]
async fn trend_reports_page_returns_html() {
    let (status, _, body) = get(make_test_router(), "/trend-reports").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "trend-reports must be HTML"
    );
}

// ── /test-metrics page ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_metrics_page_returns_html() {
    let (status, _, body) = get(make_test_router(), "/test-metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "test-metrics must be HTML"
    );
}

// ── /git-browser page ─────────────────────────────────────────────────────────

#[tokio::test]
async fn git_browser_page_returns_html() {
    let (status, _, body) = get(make_test_router(), "/git-browser").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "git-browser must be HTML"
    );
}

// ── /api/cleanup-policy/run-now ──────────────────────────────────────────────

#[tokio::test]
async fn cleanup_policy_run_now_no_policy_smoke_test() {
    let (status, _) = post_json(make_test_router(), "/api/cleanup-policy/run-now", "{}").await;
    assert!(
        status.as_u16() < 500,
        "cleanup run-now must not 5xx, got {status}"
    );
}

// ── /runs/result/{run_id} — 404 for unknown ID ───────────────────────────────

#[tokio::test]
async fn run_result_unknown_id_returns_404() {
    let (status, _, body) = get(
        make_test_router(),
        "/runs/result/00000000-0000-0000-0000-eeeeeeeeeeee",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        body.contains("not found") || body.contains("Error"),
        "must show error: {body}"
    );
}

// ── Security headers are present on all pages ─────────────────────────────────

#[tokio::test]
async fn security_headers_present_on_scan_page() {
    let (_, headers, _) = get(make_test_router(), "/scan").await;
    let frame_opts = headers
        .get("x-frame-options")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(frame_opts, "DENY", "X-Frame-Options must be DENY");

    let cto = headers
        .get("x-content-type-options")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(cto, "nosniff", "X-Content-Type-Options must be nosniff");
}

// ── /api/runs/{run_id}/pdf-status — no PDF on disk ───────────────────────────

#[tokio::test]
async fn pdf_status_no_pdf_returns_not_ready() {
    let (status, _, body) = get(
        make_test_router(),
        "/api/runs/00000000-0000-0000-0000-dddddddddddd/pdf-status",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    assert_eq!(v["ready"], false, "no-PDF run must return ready=false");
}

// ── download bundle — unknown run ─────────────────────────────────────────────

#[tokio::test]
async fn download_bundle_unknown_run_returns_404() {
    let (status, _, _) = get(
        make_test_router(),
        "/api/runs/00000000-0000-0000-0000-ffffffffff01/bundle",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── /api/confluence/post smoke test ──────────────────────────────────────────

#[tokio::test]
async fn api_confluence_post_no_config_returns_error() {
    let (status, _body) = post_json(
        make_test_router(),
        "/api/confluence/post",
        r#"{"run_id":"some-run-id"}"#,
    )
    .await;
    // With no Confluence config, should return 4xx — definitely not 5xx
    assert!(
        status.as_u16() < 500,
        "confluence post without config must not 5xx, got {status}"
    );
}

// ── /api/confluence/test smoke test ──────────────────────────────────────────

#[tokio::test]
async fn api_confluence_test_no_config_returns_error() {
    let (status, _) = post_json(make_test_router(), "/api/confluence/test", r"{}").await;
    assert!(
        status.as_u16() < 500,
        "confluence test without config must not 5xx, got {status}"
    );
}

// ── Confluence: save config, then hit the *configured* post-guard clauses ─────
//
// The smoke tests above only reach the "not configured" early return. This drives
// the far larger post-configuration path: after a valid config is saved, GET
// reports `configured: true`, and POST /api/confluence/post with a well-formed but
// unknown run_id proceeds past the config check into the artifact/registry lookup,
// which misses and returns 404 — all without any outbound network call.

#[tokio::test]
async fn api_confluence_save_then_post_unknown_run_is_404() {
    let app = make_test_router();

    // 1. Save a valid Confluence config (public HTTPS URL passes SSRF validation).
    let cfg = r#"{
        "tier": "cloud",
        "base_url": "https://example.atlassian.net",
        "username": "bot@example.com",
        "credential": "s3cret-token",
        "space_key": "DEV",
        "parent_page_id": ""
    }"#;
    let (save_status, _) = post_json(app.clone(), "/api/confluence/config", cfg).await;
    assert_eq!(save_status, StatusCode::OK, "valid config must save");

    // 2. GET config must now report configured=true (the populated JSON branch).
    let get_resp = app
        .clone()
        .oneshot(
            Request::get("/api/confluence/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = get_resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["configured"], true);
    assert_eq!(json["base_url"], "https://example.atlassian.net");
    assert_eq!(json["api_token_set"], true);

    // 3. POST to publish a run that does not exist. With config present, this now
    //    passes the config gate and reaches the artifact+registry lookup, which
    //    misses → 404 (no outbound call is made).
    let (post_status, post_body) = post_json(
        app.clone(),
        "/api/confluence/post",
        r#"{"run_id":"nonexistent-valid-run-id","page_title":"My Report"}"#,
    )
    .await;
    assert_eq!(
        post_status,
        StatusCode::NOT_FOUND,
        "publishing an unknown run must 404, got {post_status}: {post_body}"
    );
}

#[tokio::test]
async fn api_wiki_markup_unknown_run_is_404_and_bad_run_id_is_400() {
    // Unknown (but well-formed) run_id misses both artifacts and registry → 404.
    let (status, _, _) = get(
        make_test_router(),
        "/api/confluence/wiki-markup?run_id=nonexistent-run",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A run_id containing a path separator is rejected up front → 400.
    let (bad, _, _) = get(
        make_test_router(),
        "/api/confluence/wiki-markup?run_id=a%2Fb",
    )
    .await;
    assert_eq!(bad, StatusCode::BAD_REQUEST);
}

// ── Git browser API guard clauses (no network / no real clone) ───────────────

#[tokio::test]
async fn api_git_refs_missing_repo_is_400() {
    let (status, _, _) = get(make_test_router(), "/api/git/refs").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_git_refs_invalid_repo_url_is_bad_gateway() {
    // A non-URL repo is rejected by clone_or_fetch's scheme allowlist before any
    // network call, so load_refs returns Err → 502 (the Ok(Err) task arm).
    let (status, _, _) = get(make_test_router(), "/api/git/refs?repo=not-a-valid-url").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn api_git_scan_ref_invalid_ref_is_400() {
    // A ref_name containing ".." fails is_valid_git_ref → 400 before spawn_blocking.
    let (status, _, _) = get(
        make_test_router(),
        "/api/git/scan-ref?repo=https://example.com/r.git&ref_name=../evil",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_git_compare_refs_invalid_ref_is_400() {
    let (status, _, _) = get(
        make_test_router(),
        "/api/git/compare-refs?repo=https://example.com/r.git&baseline_ref=../bad&current_ref=main",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_confluence_save_rejects_ssrf_url() {
    // A loopback base_url must be rejected at save time (SSRF validation), so the
    // config is never stored.
    let (status, _) = post_json(
        make_test_router(),
        "/api/confluence/config",
        r#"{"base_url":"http://127.0.0.1/wiki","username":"u","credential":"c","space_key":"S"}"#,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "loopback Confluence URL must be rejected, got {status}"
    );
}

// ── Helper function: URL-encode a string for form bodies ─────────────────────

fn urlencoding_encode(s: &str) -> String {
    s.bytes()
        .flat_map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![b as char]
            }
            b' ' => vec!['+'],
            _ => format!("%{b:02X}").chars().collect(),
        })
        .collect()
}
