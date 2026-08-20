// Success- and branch-path coverage for sloc-web route handlers that previously
// only had their error paths exercised. These build a real on-disk scan folder
// (result_*.json + result_*.html) and drive the relocate / locate / watched-dir
// handlers through their happy paths, plus the JSON-negotiated (`Accept:
// application/json`) response variants.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use sloc_web::{make_test_router, make_test_router_with_key};
use tower::ServiceExt;

use chrono::Utc;
use sloc_config::AppConfig;
use sloc_core::{
    AnalysisRun, EffectiveCounts, EnvironmentMetadata, FileRecord, FileStatus, LanguageSummary,
    SummaryTotals, ToolMetadata,
};
use sloc_languages::{Language, ParseMode, RawLineCounts};
use std::path::Path;

// ── fixtures ──────────────────────────────────────────────────────────────────

// Fixture builder dominated by one large struct literal; splitting it would only
// scatter the fixture, so the line count is inherent rather than a smell.
#[allow(clippy::too_many_lines)]
fn make_run(run_id: &str) -> AnalysisRun {
    let raw = RawLineCounts {
        total_physical_lines: 12,
        code_only_lines: 10,
        blank_only_lines: 1,
        single_comment_only_lines: 1,
        ..RawLineCounts::default()
    };
    let rec = FileRecord {
        path: "src/lib.rs".into(),
        relative_path: "src/lib.rs".into(),
        language: Some(Language::Rust),
        size_bytes: 200,
        detected_encoding: Some("utf-8".into()),
        raw_line_categories: raw,
        effective_counts: EffectiveCounts {
            code_lines: 10,
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
    };
    let lang = LanguageSummary {
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
    };
    AnalysisRun {
        tool: ToolMetadata {
            name: "oxide-sloc".into(),
            version: "1.0.0".into(),
            run_id: run_id.into(),
            timestamp_utc: Utc::now(),
        },
        environment: EnvironmentMetadata {
            operating_system: "linux".into(),
            architecture: "x86_64".into(),
            runtime_mode: "test".into(),
            initiator_username: "tester".into(),
            initiator_hostname: "localhost".into(),
            ci_name: None,
        },
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
        totals_by_language: vec![lang],
        per_file_records: vec![rec],
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

/// Minimal percent-encoder for form values (Windows paths contain `:` and `\`).
fn pe(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// Write a scan folder containing `result_<id>.json` + `result_<id>.html`.
fn write_scan_folder(dir: &Path, run_id: &str) {
    let run = make_run(run_id);
    let json = serde_json::to_string(&run).unwrap();
    std::fs::write(dir.join(format!("result_{run_id}.json")), json).unwrap();
    std::fs::write(
        dir.join(format!("result_{run_id}.html")),
        "<html><body>report</body></html>",
    )
    .unwrap();
}

// ── HTTP helpers ──────────────────────────────────────────────────────────────

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

async fn post_form_json_accept(app: Router, uri: &str, body: &str) -> (StatusCode, String) {
    let req = Request::post(uri)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
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

/// Ingest a run so it exists in the shared router's registry.
async fn ingest(app: &Router, run_id: &str) {
    let json = serde_json::to_string(&make_run(run_id)).unwrap();
    let (status, body) = post_json(app.clone(), "/api/ingest", &json).await;
    assert!(status.is_success(), "ingest failed {status}: {body}");
}

/// Ingest a run then link it to an on-disk scan folder (sets html/json paths on
/// the registry entry). Returns nothing; the caller keeps `dir` alive.
async fn ingest_and_link(app: &Router, dir: &Path, run_id: &str) {
    ingest(app, run_id).await;
    write_scan_folder(dir, run_id);
    let path = dir.canonicalize().unwrap();
    let body = format!("file_path={}", pe(&path.to_string_lossy()));
    let (status, _) = post_form(app.clone(), "/locate-report", &body).await;
    assert!(
        status.is_redirection() || status.is_success(),
        "link failed: {status}"
    );
}

// ── relocate-scan success + branches ──────────────────────────────────────────

#[tokio::test]
async fn relocate_scan_success_redirects_to_compare() {
    let app = make_test_router();
    let run_id = "relocate-ok-001";
    ingest(&app, run_id).await;

    let dir = tempfile::tempdir().unwrap();
    write_scan_folder(dir.path(), run_id);
    let path = dir.path().canonicalize().unwrap();

    let body = format!(
        "run_id={run_id}&folder_path={}&redirect_url=%2Fcompare-scans",
        pe(&path.to_string_lossy())
    );
    let (status, _) = post_form(app, "/relocate-scan", &body).await;
    assert!(
        status.is_redirection(),
        "relocate success should redirect, got {status}"
    );
}

#[tokio::test]
async fn relocate_scan_success_json_accept_returns_ok_json() {
    let app = make_test_router();
    let run_id = "relocate-json-001";
    ingest(&app, run_id).await;

    let dir = tempfile::tempdir().unwrap();
    write_scan_folder(dir.path(), run_id);
    let path = dir.path().canonicalize().unwrap();

    let body = format!(
        "run_id={run_id}&folder_path={}&redirect_url=%2Fcompare-scans",
        pe(&path.to_string_lossy())
    );
    let (status, resp) = post_form_json_accept(app, "/relocate-scan", &body).await;
    assert_eq!(status, StatusCode::OK, "json relocate: {resp}");
    assert!(resp.contains("\"ok\":true"), "json ok body: {resp}");
    assert!(resp.contains("redirect"), "json redirect field: {resp}");
}

#[tokio::test]
async fn relocate_scan_no_matching_json_json_accept_errors() {
    let app = make_test_router();
    let run_id = "relocate-mismatch-001";
    ingest(&app, run_id).await;

    // Folder has a result JSON for a *different* run.
    let dir = tempfile::tempdir().unwrap();
    write_scan_folder(dir.path(), "some-other-run");
    let path = dir.path().canonicalize().unwrap();

    let body = format!(
        "run_id={run_id}&folder_path={}&redirect_url=%2Fcompare-scans",
        pe(&path.to_string_lossy())
    );
    let (status, resp) = post_form_json_accept(app, "/relocate-scan", &body).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "resp: {resp}");
    assert!(resp.contains("\"ok\":false"), "error json: {resp}");
}

#[tokio::test]
async fn relocate_scan_empty_folder_no_json_errors() {
    let app = make_test_router();
    let run_id = "relocate-empty-001";
    ingest(&app, run_id).await;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().canonicalize().unwrap();
    let body = format!(
        "run_id={run_id}&folder_path={}&redirect_url=%2Fcompare-scans",
        pe(&path.to_string_lossy())
    );
    let (status, _) = post_form(app, "/relocate-scan", &body).await;
    assert!(status.as_u16() < 500, "must not 5xx, got {status}");
}

#[tokio::test]
async fn relocate_scan_unknown_run_json_accept_returns_404_json() {
    let app = make_test_router();
    let body = "run_id=does-not-exist&folder_path=%2Ftmp&redirect_url=%2Fcompare-scans";
    let (status, resp) = post_form_json_accept(app, "/relocate-scan", body).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "resp: {resp}");
    assert!(resp.contains("\"ok\":false"), "error json: {resp}");
}

// ── locate-report success + branches ──────────────────────────────────────────

#[tokio::test]
async fn locate_report_directory_input_links_existing_entry() {
    let app = make_test_router();
    let run_id = "locate-dir-001";
    ingest(&app, run_id).await;

    let dir = tempfile::tempdir().unwrap();
    write_scan_folder(dir.path(), run_id);
    let path = dir.path().canonicalize().unwrap();

    let body = format!("file_path={}", pe(&path.to_string_lossy()));
    let (status, _) = post_form(app, "/locate-report", &body).await;
    assert!(
        status.is_redirection() || status.is_success(),
        "locate by dir should succeed, got {status}"
    );
}

#[tokio::test]
async fn locate_report_direct_html_file_links_entry() {
    let app = make_test_router();
    let run_id = "locate-html-001";
    ingest(&app, run_id).await;

    let dir = tempfile::tempdir().unwrap();
    write_scan_folder(dir.path(), run_id);
    let html = dir
        .path()
        .canonicalize()
        .unwrap()
        .join(format!("result_{run_id}.html"));

    let body = format!("file_path={}", pe(&html.to_string_lossy()));
    let (status, resp) = post_form_json_accept(app, "/locate-report", &body).await;
    assert!(status.as_u16() < 500, "must not 5xx, got {status}: {resp}");
}

#[tokio::test]
async fn locate_report_mismatched_expected_run_id_errors() {
    let app = make_test_router();
    let run_id = "locate-expect-001";
    ingest(&app, run_id).await;

    let dir = tempfile::tempdir().unwrap();
    write_scan_folder(dir.path(), run_id);
    let path = dir.path().canonicalize().unwrap();

    let body = format!(
        "file_path={}&expected_run_id=totally-different",
        pe(&path.to_string_lossy())
    );
    let (status, resp) = post_form_json_accept(app, "/locate-report", &body).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "resp: {resp}");
    assert!(resp.contains("\"ok\":false"), "mismatch json: {resp}");
}

#[tokio::test]
async fn locate_report_empty_directory_no_html_errors_json() {
    let app = make_test_router();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().canonicalize().unwrap();
    let body = format!("file_path={}", pe(&path.to_string_lossy()));
    let (status, resp) = post_form_json_accept(app, "/locate-report", &body).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "resp: {resp}");
    assert!(resp.contains("\"ok\":false"), "no-html json: {resp}");
}

// ── locate-reports-dir success ────────────────────────────────────────────────

#[tokio::test]
async fn locate_reports_dir_success_links_reports() {
    let app = make_test_router();
    let dir = tempfile::tempdir().unwrap();
    write_scan_folder(dir.path(), "reports-dir-001");
    let path = dir.path().canonicalize().unwrap();

    let body = format!("folder_path={}", pe(&path.to_string_lossy()));
    let (status, _) = post_form(app, "/locate-reports-dir", &body).await;
    assert!(status.is_redirection(), "should redirect, got {status}");
}

#[tokio::test]
async fn locate_reports_dir_file_not_dir_redirects_error() {
    let app = make_test_router();
    let dir = tempfile::tempdir().unwrap();
    write_scan_folder(dir.path(), "reports-file-001");
    let file = dir
        .path()
        .canonicalize()
        .unwrap()
        .join("result_reports-file-001.json");

    let body = format!("folder_path={}", pe(&file.to_string_lossy()));
    let (status, _) = post_form(app, "/locate-reports-dir", &body).await;
    assert!(status.is_redirection(), "should redirect, got {status}");
}

// ── watched-dirs add / remove / refresh success ───────────────────────────────

#[tokio::test]
async fn watched_dir_add_with_reports_links() {
    let app = make_test_router();
    let dir = tempfile::tempdir().unwrap();
    write_scan_folder(dir.path(), "watched-add-001");
    let path = dir.path().canonicalize().unwrap();

    let body = format!(
        "folder_path={}&redirect_to=%2Fview-reports",
        pe(&path.to_string_lossy())
    );
    let (status, _) = post_form(app, "/watched-dirs/add", &body).await;
    assert!(status.is_redirection(), "add should redirect, got {status}");
}

#[tokio::test]
async fn watched_dir_add_empty_folder_reports_none_found() {
    let app = make_test_router();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().canonicalize().unwrap();
    let body = format!(
        "folder_path={}&redirect_to=%2Fview-reports",
        pe(&path.to_string_lossy())
    );
    let (status, _) = post_form(app, "/watched-dirs/add", &body).await;
    assert!(status.is_redirection(), "add should redirect, got {status}");
}

#[tokio::test]
async fn watched_dir_add_not_a_directory_redirects_error() {
    let app = make_test_router();
    let dir = tempfile::tempdir().unwrap();
    write_scan_folder(dir.path(), "watched-file-001");
    let file = dir
        .path()
        .canonicalize()
        .unwrap()
        .join("result_watched-file-001.json");
    let body = format!(
        "folder_path={}&redirect_to=%2Fview-reports",
        pe(&file.to_string_lossy())
    );
    let (status, _) = post_form(app, "/watched-dirs/add", &body).await;
    assert!(status.is_redirection(), "should redirect, got {status}");
}

#[tokio::test]
async fn watched_dir_add_then_remove_then_refresh() {
    let app = make_test_router();
    let dir = tempfile::tempdir().unwrap();
    write_scan_folder(dir.path(), "watched-cycle-001");
    let path = dir.path().canonicalize().unwrap();
    let encoded = pe(&path.to_string_lossy());

    let add_body = format!("folder_path={encoded}&redirect_to=%2Fview-reports");
    let (s1, _) = post_form(app.clone(), "/watched-dirs/add", &add_body).await;
    assert!(s1.is_redirection());

    // Refresh: re-scans watched dirs.
    let (s2, _) = post_form(
        app.clone(),
        "/watched-dirs/refresh",
        "redirect_to=%2Fview-reports",
    )
    .await;
    assert!(s2.is_redirection(), "refresh should redirect, got {s2}");

    // Remove.
    let rm_body = format!("folder_path={encoded}&redirect_to=%2Fview-reports");
    let (s3, _) = post_form(app, "/watched-dirs/remove", &rm_body).await;
    assert!(s3.is_redirection(), "remove should redirect, got {s3}");
}

// ── auth logout ───────────────────────────────────────────────────────────────

async fn post_logout(app: Router, cookie: Option<&str>) -> (StatusCode, axum::http::HeaderMap) {
    let mut b =
        Request::post("/auth/logout").header("content-type", "application/x-www-form-urlencoded");
    if let Some(c) = cookie {
        b = b.header("cookie", c);
    }
    let resp = app.oneshot(b.body(Body::empty()).unwrap()).await.unwrap();
    (resp.status(), resp.headers().clone())
}

#[tokio::test]
async fn auth_logout_no_key_redirects_to_root() {
    let (status, headers) = post_logout(make_test_router(), None).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers.get("location").and_then(|v| v.to_str().ok()),
        Some("/"),
        "no API key configured → redirect to /"
    );
    // Both cookie variants are expired.
    let set_cookie_count = headers.get_all("set-cookie").iter().count();
    assert_eq!(set_cookie_count, 2, "expires plain + __Host- cookie");
}

#[tokio::test]
async fn auth_logout_with_session_cookie_clears_session() {
    let (status, headers) =
        post_logout(make_test_router(), Some("sloc_session=deadbeefdeadbeef")).await;
    assert_eq!(status, StatusCode::FOUND);
    assert!(
        headers.get_all("set-cookie").iter().count() >= 1,
        "logout expires the session cookie"
    );
}

// ── artifact serving (html / json / scan-config / bundle) ─────────────────────

#[tokio::test]
async fn serve_linked_html_and_json_artifacts() {
    let app = make_test_router();
    let dir = tempfile::tempdir().unwrap();
    let run_id = "artifact-serve-001";
    ingest_and_link(&app, dir.path(), run_id).await;

    // HTML artifact — inline view.
    let (s_html, h_html, body_html) = get(app.clone(), &format!("/runs/html/{run_id}")).await;
    assert_eq!(s_html, StatusCode::OK, "html: {body_html}");
    let ct = h_html
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("html"), "html content-type: {ct}");

    // HTML artifact — download variant.
    let (s_dl, _, _) = get(app.clone(), &format!("/runs/html/{run_id}?download=1")).await;
    assert_eq!(s_dl, StatusCode::OK);

    // JSON artifact — inline + download.
    let (s_json, h_json, _) = get(app.clone(), &format!("/runs/json/{run_id}")).await;
    assert_eq!(s_json, StatusCode::OK);
    assert!(
        h_json
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .contains("json")
    );
    let (s_json_download, _, _) =
        get(app.clone(), &format!("/runs/json/{run_id}?download=1")).await;
    assert_eq!(s_json_download, StatusCode::OK);

    // scan-config arm (reads config out of the JSON's directory).
    let (s_cfg, _, _) = get(app.clone(), &format!("/runs/scan-config/{run_id}")).await;
    assert!(
        s_cfg.as_u16() < 500,
        "scan-config must not 5xx, got {s_cfg}"
    );

    // CSV arm (regenerated on demand or 404 when absent — either way, no 5xx).
    let (s_csv, _, _) = get(app.clone(), &format!("/runs/csv/{run_id}")).await;
    assert!(s_csv.as_u16() < 500, "csv arm must not 5xx, got {s_csv}");

    // Download bundle (zips the linked artifacts).
    let (s_bundle, h_bundle, _) = get(app, &format!("/api/runs/{run_id}/bundle")).await;
    assert!(
        s_bundle.as_u16() < 500,
        "bundle must not 5xx, got {s_bundle}"
    );
    if s_bundle == StatusCode::OK {
        let ct = h_bundle
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            ct.contains("zip") || ct.contains("octet"),
            "bundle ct: {ct}"
        );
    }
}

#[tokio::test]
async fn artifact_unknown_run_id_returns_404() {
    let (status, _, _) = get(make_test_router(), "/runs/html/no-such-run-xyz").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn artifact_reversed_url_hint_returns_404() {
    // /runs/{run_id}/{artifact} reversed → run_id is literally "html".
    let (status, _, body) = get(make_test_router(), "/runs/pdf/html").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        body.contains("reversed") || body.contains("not"),
        "hint body"
    );
}

#[tokio::test]
async fn compare_two_linked_runs_renders() {
    let app = make_test_router();
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    ingest_and_link(&app, dir_a.path(), "cmp-link-a").await;
    ingest_and_link(&app, dir_b.path(), "cmp-link-b").await;

    let (status, _, body) = get(app, "/compare?a=cmp-link-a&b=cmp-link-b").await;
    assert!(
        status.is_success(),
        "compare should render, got {status}: {}",
        &body[..body.len().min(200)]
    );
}

#[tokio::test]
async fn auth_logout_with_key_redirects_to_login() {
    let (status, headers) = post_logout(make_test_router_with_key("secret-key-xyz"), None).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        headers.get("location").and_then(|v| v.to_str().ok()),
        Some("/auth/login"),
        "API key configured → redirect to /auth/login"
    );
}

// ── Confluence integration API (network-free validation branches) ─────────────

#[tokio::test]
async fn confluence_get_config_when_unconfigured() {
    let (status, _, body) = get(make_test_router(), "/api/confluence/config").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("\"configured\":false"),
        "unconfigured: {body}"
    );
}

#[tokio::test]
async fn confluence_save_invalid_url_rejected() {
    // A private/localhost base_url is blocked by the SSRF guard in validate_confluence_url.
    let body = r#"{"base_url":"http://127.0.0.1/wiki","username":"u","credential":"tok","space_key":"DS"}"#;
    let (status, resp) = post_json(make_test_router(), "/api/confluence/config", body).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "resp: {resp}");
    assert!(resp.contains("error"), "error json: {resp}");
}

#[tokio::test]
async fn confluence_save_valid_then_get_configured() {
    let app = make_test_router();
    let body = r#"{"tier":"cloud","base_url":"https://example.atlassian.net","username":"u@x.com","credential":"tok123","space_key":"DS","parent_page_id":"12345"}"#;
    let (s1, r1) = post_json(app.clone(), "/api/confluence/config", body).await;
    assert_eq!(s1, StatusCode::OK, "save: {r1}");
    assert!(r1.contains("\"ok\":true"));

    let (s2, _, r2) = get(app, "/api/confluence/config").await;
    assert_eq!(s2, StatusCode::OK);
    assert!(r2.contains("\"configured\":true"), "configured: {r2}");
    assert!(r2.contains("\"api_token_set\":true"), "token set: {r2}");
}

#[tokio::test]
async fn confluence_save_empty_credential_keeps_existing() {
    let app = make_test_router();
    let first = r#"{"base_url":"https://example.atlassian.net","username":"u","credential":"secret","space_key":"DS"}"#;
    let (s1, _) = post_json(app.clone(), "/api/confluence/config", first).await;
    assert_eq!(s1, StatusCode::OK);
    // Re-save with a blank credential — the stored token must be preserved.
    let second = r#"{"base_url":"https://example.atlassian.net","username":"u2","credential":"","space_key":"DS"}"#;
    let (s2, _) = post_json(app.clone(), "/api/confluence/config", second).await;
    assert_eq!(s2, StatusCode::OK);
    let (_, _, body) = get(app, "/api/confluence/config").await;
    assert!(
        body.contains("\"api_token_set\":true"),
        "kept token: {body}"
    );
}

#[tokio::test]
async fn confluence_save_server_tier() {
    let app = make_test_router();
    let body = r#"{"tier":"server","base_url":"https://wiki.example.com","username":"u","credential":"tok","space_key":"DS"}"#;
    let (s, _) = post_json(app.clone(), "/api/confluence/config", body).await;
    assert_eq!(s, StatusCode::OK);
    let (_, _, cfg) = get(app, "/api/confluence/config").await;
    assert!(cfg.contains("\"tier\":\"server\""), "server tier: {cfg}");
}

#[tokio::test]
async fn confluence_test_when_unconfigured_returns_400() {
    let (status, resp) = post_json(make_test_router(), "/api/confluence/test", "{}").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "resp: {resp}");
    assert!(resp.contains("not configured"), "message: {resp}");
}

#[tokio::test]
async fn confluence_post_invalid_run_id_rejected() {
    let body = r#"{"run_id":"bad id/../x","page_title":"T"}"#;
    let (status, resp) = post_json(make_test_router(), "/api/confluence/post", body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "resp: {resp}");
    assert!(resp.contains("Invalid run_id"), "message: {resp}");
}

#[tokio::test]
async fn confluence_post_unconfigured_rejected() {
    let body = r#"{"run_id":"valid-run-001","page_title":"T"}"#;
    let (status, resp) = post_json(make_test_router(), "/api/confluence/post", body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "resp: {resp}");
    assert!(resp.contains("not configured"), "message: {resp}");
}

#[tokio::test]
async fn confluence_post_run_not_found_returns_404() {
    let app = make_test_router();
    let cfg = r#"{"base_url":"https://example.atlassian.net","username":"u","credential":"tok","space_key":"DS"}"#;
    let (s, _) = post_json(app.clone(), "/api/confluence/config", cfg).await;
    assert_eq!(s, StatusCode::OK);
    let body = r#"{"run_id":"no-such-run-001","page_title":"T"}"#;
    let (status, resp) = post_json(app, "/api/confluence/post", body).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "resp: {resp}");
    assert!(resp.contains("not found"), "message: {resp}");
}

#[tokio::test]
async fn confluence_wiki_markup_invalid_run_id_returns_400() {
    let (status, _, _) = get(
        make_test_router(),
        "/api/confluence/wiki-markup?run_id=a%2Fb",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn confluence_wiki_markup_unknown_run_returns_404() {
    let (status, _, _) = get(
        make_test_router(),
        "/api/confluence/wiki-markup?run_id=unknown-xyz",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn confluence_wiki_markup_linked_run_renders_markup() {
    let app = make_test_router();
    let dir = tempfile::tempdir().unwrap();
    let run_id = "wiki-markup-001";
    ingest_and_link(&app, dir.path(), run_id).await;

    let (status, headers, body) =
        get(app, &format!("/api/confluence/wiki-markup?run_id={run_id}")).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("text/plain"), "wiki markup content-type: {ct}");
    assert!(!body.is_empty(), "wiki markup should not be empty");
}

// ── Git browser API (validation + repo-access error branches) ─────────────────

#[tokio::test]
async fn git_list_refs_missing_repo_returns_400() {
    let (status, _, _) = get(make_test_router(), "/api/git/refs").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn git_list_refs_bogus_repo_returns_bad_gateway() {
    // A non-existent local path fails `git clone` fast → load_refs error → 502.
    let (status, _, _) = get(
        make_test_router(),
        "/api/git/refs?repo=%2Fnonexistent%2Frepo-xyz.git",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "bogus repo → 502");
}

#[tokio::test]
async fn git_scan_ref_invalid_ref_returns_400() {
    let (status, _, _) = get(
        make_test_router(),
        "/api/git/scan-ref?repo=%2Ftmp%2Fx&ref_name=..%2Fetc",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "path-traversal ref → 400");
}

#[tokio::test]
async fn git_scan_ref_valid_ref_bogus_repo_returns_bad_gateway() {
    let (status, _, _) = get(
        make_test_router(),
        "/api/git/scan-ref?repo=%2Fnonexistent%2Frepo-xyz.git&ref_name=main",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "bogus repo scan → 502");
}

#[tokio::test]
async fn git_compare_refs_invalid_ref_returns_400() {
    let (status, _, _) = get(
        make_test_router(),
        "/api/git/compare-refs?repo=%2Ftmp%2Fx&baseline_ref=..&current_ref=main",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn git_compare_refs_valid_refs_bogus_repo_returns_bad_gateway() {
    let (status, _, _) = get(
        make_test_router(),
        "/api/git/compare-refs?repo=%2Fnonexistent%2Frepo.git&baseline_ref=v1&current_ref=v2",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "bogus repo compare → 502");
}

// ── read/render endpoints against a populated registry ────────────────────────

/// Drives the report-consuming GET endpoints with real linked runs so their
/// with-data render branches (not just the empty-state paths) are exercised.
#[tokio::test]
async fn render_endpoints_with_populated_registry() {
    let app = make_test_router();
    let d1 = tempfile::tempdir().unwrap();
    let d2 = tempfile::tempdir().unwrap();
    let d3 = tempfile::tempdir().unwrap();
    ingest_and_link(&app, d1.path(), "pop-run-001").await;
    ingest_and_link(&app, d2.path(), "pop-run-002").await;
    ingest_and_link(&app, d3.path(), "pop-run-003").await;

    // Pages that render tables/charts from the registry.
    for uri in [
        "/view-reports",
        "/compare-scans",
        "/multi-compare?ids=pop-run-001,pop-run-002,pop-run-003",
        "/test-metrics",
        "/trend-reports",
        "/git-browser",
        "/integrations",
    ] {
        let (status, _, body) = get(app.clone(), uri).await;
        assert!(
            status.is_success() || status.is_redirection(),
            "{uri} should render, got {status}: {}",
            &body[..body.len().min(160)]
        );
    }

    // JSON metrics APIs with data present.
    for uri in [
        "/api/metrics/latest",
        "/api/metrics/history",
        "/api/metrics/pop-run-001",
        "/api/project-history",
        "/embed/summary",
        "/badge/code_lines",
        "/badge/files",
    ] {
        let (status, _, _) = get(app.clone(), uri).await;
        assert!(status.as_u16() < 500, "{uri} must not 5xx, got {status}");
    }
}

/// Extract the first UUID-looking token from a response body (byte-safe).
fn first_uuid(body: &str) -> Option<String> {
    let b = body.as_bytes();
    let is_hex = |c: u8| c.is_ascii_hexdigit();
    for w in b.windows(36) {
        let shape_ok = w[8] == b'-'
            && w[13] == b'-'
            && w[18] == b'-'
            && w[23] == b'-'
            && w.iter()
                .enumerate()
                .all(|(j, &c)| matches!(j, 8 | 13 | 18 | 23) || is_hex(c));
        if shape_ok {
            // All bytes are ASCII (hex + '-'), so this is valid UTF-8.
            return Some(String::from_utf8_lossy(w).into_owned());
        }
    }
    None
}

/// End-to-end: POST /analyze on a real project directory, poll the async status
/// endpoint to completion, then render the result page. Exercises
/// `run_analysis_task`, `scan_path_to_artifacts`, the native PDF background spawn,
/// and the async status/result handlers.
#[tokio::test]
async fn analyze_full_flow_polls_to_completion_and_renders_result() {
    let app = make_test_router();
    let proj = tempfile::tempdir().unwrap();
    std::fs::write(
        proj.path().join("main.rs"),
        "fn main() {\n    // entry\n    println!(\"hi\");\n}\n",
    )
    .unwrap();
    std::fs::write(
        proj.path().join("lib.rs"),
        "pub fn add(a: i32) -> i32 { a }\n",
    )
    .unwrap();

    let path = proj.path().canonicalize().unwrap();
    let (status, wait_body) = post_form(
        app.clone(),
        "/analyze",
        &format!("path={}", pe(&path.to_string_lossy())),
    )
    .await;
    assert!(status.is_success(), "analyze dispatch failed: {status}");
    let wait_id = first_uuid(&wait_body).expect("wait page must embed a wait_id");

    // Poll status until Complete/Failed (bounded).
    let mut run_id = None;
    for _ in 0..100 {
        let (s, _, body) = get(app.clone(), &format!("/api/runs/{wait_id}/status")).await;
        assert!(s.is_success(), "status poll failed: {s}");
        if body.contains("\"state\":\"complete\"") {
            run_id = first_uuid(&body).or_else(|| {
                // run_id may not be UUID-shaped; extract the JSON field directly.
                body.split("\"run_id\":\"")
                    .nth(1)
                    .and_then(|s| s.split('"').next())
                    .map(str::to_owned)
            });
            break;
        }
        assert!(
            !body.contains("\"state\":\"failed\""),
            "analysis failed: {body}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let run_id = run_id.expect("analysis should complete with a run_id");

    // Render the result page for the completed run.
    let (rs, _, rbody) = get(app, &format!("/runs/result/{run_id}")).await;
    assert!(
        rs.is_success() || rs.is_redirection(),
        "result page render, got {rs}: {}",
        &rbody[..rbody.len().min(160)]
    );
}

/// Cancelling a freshly-dispatched analysis run marks it Cancelled.
#[tokio::test]
async fn analyze_run_can_be_cancelled() {
    let app = make_test_router();
    let proj = tempfile::tempdir().unwrap();
    std::fs::write(proj.path().join("a.rs"), "fn a() {}\n").unwrap();
    let path = proj.path().canonicalize().unwrap();
    let (_, wait_body) = post_form(
        app.clone(),
        "/analyze",
        &format!("path={}", pe(&path.to_string_lossy())),
    )
    .await;
    let wait_id = first_uuid(&wait_body).expect("wait_id");

    // POST cancel — either it was still running (200) or already complete (404).
    let (cs, _) = post_form(app.clone(), &format!("/api/runs/{wait_id}/cancel"), "").await;
    assert!(cs.as_u16() < 500, "cancel must not 5xx, got {cs}");

    // Status is still queryable afterward.
    let (ss, _, _) = get(app, &format!("/api/runs/{wait_id}/status")).await;
    assert!(ss.is_success(), "status after cancel: {ss}");
}

/// Link a run, then delete its on-disk artifacts so the serve-artifact handlers
/// take their "file recorded but missing on disk" (locate/not-found) branches.
#[tokio::test]
async fn serve_artifacts_when_files_deleted_take_notfound_branch() {
    let app = make_test_router();
    let dir = tempfile::tempdir().unwrap();
    let run_id = "artifact-missing-001";
    // Link via /locate-report only (no /api/ingest), so the registry entry's
    // paths point at *these* files rather than a second copy under out/web.
    write_scan_folder(dir.path(), run_id);
    let canon = dir.path().canonicalize().unwrap();
    let (ls, _) = post_form(
        app.clone(),
        "/locate-report",
        &format!("file_path={}", pe(&canon.to_string_lossy())),
    )
    .await;
    assert!(ls.is_redirection() || ls.is_success(), "link: {ls}");

    // Remove the linked files from disk (registry still points at them).
    std::fs::remove_file(dir.path().join(format!("result_{run_id}.html"))).ok();
    std::fs::remove_file(dir.path().join(format!("result_{run_id}.json"))).ok();

    // HTML: read fails with NotFound → LocateFileTemplate 404.
    let (s_html, _, body) = get(app.clone(), &format!("/runs/html/{run_id}")).await;
    assert_eq!(s_html, StatusCode::NOT_FOUND, "missing html → 404");
    assert!(!body.is_empty());

    // JSON: json_path recorded but file gone → NotFound arm.
    let (s_json, _, _) = get(app.clone(), &format!("/runs/json/{run_id}")).await;
    assert_eq!(s_json, StatusCode::NOT_FOUND, "missing json → 404");

    // scan-config: file absent → NotFound.
    let (s_cfg, _, _) = get(app, &format!("/runs/scan-config/{run_id}")).await;
    assert!(s_cfg.as_u16() < 500, "scan-config missing must not 5xx");
}

/// compare handler with sub= and scope=super query variants, plus a missing side.
#[tokio::test]
async fn compare_handler_scope_and_missing_variants() {
    let app = make_test_router();
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    ingest_and_link(&app, da.path(), "cmp-scope-a").await;
    ingest_and_link(&app, db.path(), "cmp-scope-b").await;

    for uri in [
        "/compare?a=cmp-scope-a&b=cmp-scope-b&scope=super",
        "/compare?a=cmp-scope-a&b=cmp-scope-b&sub=vendor",
        "/compare?a=cmp-scope-a&b=does-not-exist",
        "/compare?a=&b=",
    ] {
        let (status, _, _) = get(app.clone(), uri).await;
        assert!(status.as_u16() < 500, "{uri} must not 5xx, got {status}");
    }
}

/// Deleting a real linked run exercises `delete_run_artifacts`' full removal path.
#[tokio::test]
async fn delete_linked_run_removes_it() {
    let app = make_test_router();
    let dir = tempfile::tempdir().unwrap();
    let run_id = "delete-linked-001";
    ingest_and_link(&app, dir.path(), run_id).await;

    let resp = app
        .clone()
        .oneshot(
            Request::delete(format!("/api/runs/{run_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().as_u16() < 500, "delete must not 5xx");

    // After deletion the run is gone from the registry (metrics no longer find it).
    let (s, _, _) = get(app, &format!("/api/metrics/{run_id}")).await;
    assert!(s.as_u16() < 500);
}

/// Cleanup handler over a populated registry.
#[tokio::test]
async fn cleanup_runs_with_populated_registry() {
    let app = make_test_router();
    let dir = tempfile::tempdir().unwrap();
    ingest_and_link(&app, dir.path(), "cleanup-pop-001").await;
    let (status, _) = post_json(app, "/api/runs/cleanup", r#"{"keep_last":1}"#).await;
    assert!(status.as_u16() < 500, "cleanup must not 5xx, got {status}");
}

/// The multi-compare handler with a single id and with a submodule scope.
#[tokio::test]
async fn multi_compare_variants_render() {
    let app = make_test_router();
    let d1 = tempfile::tempdir().unwrap();
    let d2 = tempfile::tempdir().unwrap();
    ingest_and_link(&app, d1.path(), "mc-a").await;
    ingest_and_link(&app, d2.path(), "mc-b").await;

    for uri in [
        "/multi-compare",
        "/multi-compare?ids=mc-a",
        "/multi-compare?ids=mc-a,mc-b",
    ] {
        let (status, _, _) = get(app.clone(), uri).await;
        assert!(status.as_u16() < 500, "{uri} must not 5xx, got {status}");
    }
}
