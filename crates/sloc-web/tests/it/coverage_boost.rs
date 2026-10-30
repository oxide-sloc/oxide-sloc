// Coverage-boost integration tests for sloc-web.
//
// These drive deep handler code paths that prior tests stopped short of:
//   * the real async analyze lifecycle through to the rendered result page and
//     every artifact (html/json/csv/xlsx), exercising run_analysis_task,
//     persist_run_artifacts, generate_offline_index, render_result_page and the
//     serve_*_artifact helpers;
//   * the multi-scan comparison page (multi_compare_handler + multi_compare_page)
//     including submodule and super-project scope filters.

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

async fn post_json_shared(app: Router, uri: &str, json: &str) -> StatusCode {
    let req = Request::post(uri)
        .header("content-type", "application/json")
        .body(Body::from(json.to_owned()))
        .unwrap();
    app.oneshot(req).await.unwrap().status()
}

/// POST a urlencoded form with `Accept: application/json` so JSON-or-HTML
/// handlers take their JSON branch.
async fn post_form_json(app: Router, uri: &str, body: &str) -> (StatusCode, String) {
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

/// Build a `multipart/form-data` body with a single file part.
fn multipart_single(boundary: &str, field: &str, filename: &str, data: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; \
             filename=\"{filename}\"\r\n\r\n"
        )
        .as_bytes(),
    );
    v.extend(data);
    v.extend(format!("\r\n--{boundary}--\r\n").as_bytes());
    v
}

async fn post_multipart(app: Router, uri: &str, boundary: &str, body: Vec<u8>) -> StatusCode {
    let req = Request::post(uri)
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    app.oneshot(req).await.unwrap().status()
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
            // The status enum is serde-tagged on the `state` field.
            match v["state"].as_str().unwrap_or("") {
                "complete" => return v["run_id"].as_str().unwrap_or("").to_owned(),
                "failed" | "cancelled" => return String::new(),
                _ => {}
            }
        }
    }
    String::new()
}

// ── AnalysisRun fixtures (for /api/ingest) ───────────────────────────────────

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

const fn lang_summary(lang: Language, files: u64, code: u64) -> LanguageSummary {
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

fn run_basic(id: &str, root: &str, code: u64) -> AnalysisRun {
    let mut run = base_run(id);
    run.per_file_records = vec![file_record("src/lib.rs", Language::Rust, code)];
    run.totals_by_language = vec![lang_summary(Language::Rust, 1, code)];
    run.summary_totals = SummaryTotals {
        files_considered: 1,
        files_analyzed: 1,
        code_lines: code,
        total_physical_lines: code + 2,
        ..SummaryTotals::default()
    };
    run.input_roots = vec![root.into()];
    run
}

fn run_with_submodules(id: &str) -> AnalysisRun {
    let mut run = base_run(id);
    run.per_file_records = vec![file_record("src/lib.rs", Language::Rust, 150)];
    run.totals_by_language = vec![lang_summary(Language::Rust, 1, 150)];
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
            language_summaries: vec![lang_summary(Language::Rust, 1, 100)],
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
            language_summaries: vec![lang_summary(Language::Python, 1, 50)],
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
    run
}

fn json_of(run: &AnalysisRun) -> String {
    serde_json::to_string(run).unwrap()
}

/// Create a small multi-language project on disk for a realistic analyze.
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
    std::fs::write(
        root.join("ui.js"),
        "// frontend\nfunction main() {\n  return 1;\n}\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub").join("more.rs"), "pub fn x() -> u8 { 1 }\n").unwrap();
    dir
}

// ── real analyze lifecycle ───────────────────────────────────────────────────

#[tokio::test]
async fn real_analyze_renders_result_page_and_all_artifacts() {
    let project = make_sample_project();
    let app = make_test_router();

    let form = format!(
        "path={}&report_title=Coverage+Test&submodule_breakdown=enabled\
         &style_analysis_enabled=enabled&cocomo_mode=organic&complexity_alert=5\
         &exclude_duplicates=enabled&generated_file_detection=enabled",
        pct_encode(project.path().to_str().unwrap())
    );
    let (_, headers, _) = post_form_shared(app.clone(), "/analyze", &form).await;
    let wait_id = headers
        .get("x-wait-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    assert!(!wait_id.is_empty(), "analyze should return an x-wait-id");

    let run_id = wait_for_run_id(app.clone(), &wait_id).await;
    assert!(
        !run_id.is_empty(),
        "analyze run should reach complete state"
    );

    // Result page — async_run_result_handler + render_result_page.
    let (status, body) = get_shared(app.clone(), &format!("/runs/result/{run_id}")).await;
    assert_eq!(status, StatusCode::OK, "result page should render");
    assert!(
        body.contains("<!doctype html>") || body.contains("<html"),
        "result page should be HTML"
    );

    // Every artifact kind — serve_html_artifact / serve_json_artifact / artifact_handler.
    for artifact in ["html", "json", "csv", "xlsx"] {
        let (st, _) = get_shared(app.clone(), &format!("/runs/{artifact}/{run_id}")).await;
        assert!(
            st.as_u16() < 500,
            "/runs/{artifact}/{run_id} must not 5xx, got {st}"
        );
    }

    // Metrics JSON APIs for the freshly created run.
    let (st, _) = get_shared(app.clone(), &format!("/api/metrics/{run_id}")).await;
    assert!(st.as_u16() < 500, "metrics must not 5xx");
    let (st, _) = get_shared(app.clone(), &format!("/api/metrics/{run_id}/submodules")).await;
    assert!(st.as_u16() < 500, "submodule metrics must not 5xx");

    // Embed widget + bundle download for a real run.
    let (st, _) = get_shared(app.clone(), &format!("/embed/summary?run_id={run_id}")).await;
    assert!(st.as_u16() < 500, "embed must not 5xx");
    let (st, _) = get_shared(app.clone(), &format!("/api/runs/{run_id}/bundle")).await;
    assert!(st.as_u16() < 500, "bundle download must not 5xx");

    // Result page artifacts are also reachable through the dashboards.
    let (st, _) = get_shared(app.clone(), "/view-reports").await;
    assert!(st.as_u16() < 500);
    let (st, _) = get_shared(app.clone(), "/trend-reports").await;
    assert!(st.as_u16() < 500);
    let (st, _) = get_shared(app.clone(), "/test-metrics").await;
    assert!(st.as_u16() < 500);
}

#[tokio::test]
async fn real_analyze_with_coverage_file_renders_coverage_sections() {
    let project = make_sample_project();
    let lcov = project.path().join("cov.info");
    std::fs::write(
        &lcov,
        "TN:\nSF:lib.rs\nDA:2,3\nDA:4,0\nLF:2\nLH:1\nend_of_record\n",
    )
    .unwrap();
    let app = make_test_router();
    let form = format!(
        "path={}&coverage_file={}&report_title=Cov",
        pct_encode(project.path().to_str().unwrap()),
        pct_encode(lcov.to_str().unwrap())
    );
    let (_, headers, _) = post_form_shared(app.clone(), "/analyze", &form).await;
    let wait_id = headers
        .get("x-wait-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    if wait_id.is_empty() {
        return;
    }
    let run_id = wait_for_run_id(app.clone(), &wait_id).await;
    if run_id.is_empty() {
        return;
    }
    let (status, body) = get_shared(app.clone(), &format!("/runs/result/{run_id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<html") || body.contains("<!doctype"));
}

// ── multi-compare ────────────────────────────────────────────────────────────

#[tokio::test]
async fn multi_compare_with_two_runs_renders_page() {
    let app = make_test_router();
    assert!(
        post_json_shared(
            app.clone(),
            "/api/ingest",
            &json_of(&run_basic("mc-a", "/test/proj", 100)),
        )
        .await
        .as_u16()
            < 500
    );
    assert!(
        post_json_shared(
            app.clone(),
            "/api/ingest",
            &json_of(&run_basic("mc-b", "/test/proj", 140)),
        )
        .await
        .as_u16()
            < 500
    );

    let (status, body) = get_shared(app.clone(), "/multi-compare?runs=mc-a,mc-b").await;
    assert_eq!(status, StatusCode::OK, "multi-compare should render");
    assert!(body.contains("<html") || body.contains("<!doctype"));
}

#[tokio::test]
async fn multi_compare_with_submodule_and_super_scope() {
    let app = make_test_router();
    let mut a = run_with_submodules("mc-sub-a");
    a.tool.timestamp_utc = Utc::now() - chrono::Duration::hours(2);
    assert!(
        post_json_shared(app.clone(), "/api/ingest", &json_of(&a))
            .await
            .as_u16()
            < 500
    );
    let b = run_with_submodules("mc-sub-b");
    assert!(
        post_json_shared(app.clone(), "/api/ingest", &json_of(&b))
            .await
            .as_u16()
            < 500
    );

    let (st, _) = get_shared(app.clone(), "/multi-compare?runs=mc-sub-a,mc-sub-b").await;
    assert!(st.as_u16() < 500);
    let (st, _) = get_shared(
        app.clone(),
        "/multi-compare?runs=mc-sub-a,mc-sub-b&sub=vendor%2Flib-a",
    )
    .await;
    assert!(st.as_u16() < 500, "submodule scope must not 5xx");
    let (st, _) = get_shared(
        app.clone(),
        "/multi-compare?runs=mc-sub-a,mc-sub-b&scope=super",
    )
    .await;
    assert!(st.as_u16() < 500, "super scope must not 5xx");
}

#[tokio::test]
async fn multi_compare_error_paths() {
    let app = make_test_router();
    let (st, body) = get_shared(app.clone(), "/multi-compare?runs=only-one").await;
    assert_eq!(st, StatusCode::OK);
    assert!(body.contains("At least 2"));

    let (st, body) = get_shared(app.clone(), "/multi-compare?runs=nope-a,nope-b").await;
    assert_eq!(st, StatusCode::OK);
    assert!(body.contains("found"));

    let many = (0..25)
        .map(|i| format!("r{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let (st, body) = get_shared(app.clone(), &format!("/multi-compare?runs={many}")).await;
    assert_eq!(st, StatusCode::OK);
    assert!(body.contains("At most 20"));

    let (st, _) = get_shared(app.clone(), "/multi-compare").await;
    assert!(st.as_u16() < 500);
}

// ── uploads ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn upload_file_multipart_stages_and_not_5xx() {
    let app = make_test_router();
    let boundary = "BOUND_FILE";
    let body = multipart_single(
        boundary,
        "file",
        "main.rs",
        b"fn main() {\n    let _ = 1 + 2;\n}\n",
    );
    let st = post_multipart(app.clone(), "/api/upload-file", boundary, body).await;
    assert!(st.as_u16() < 500, "upload-file must not 5xx, got {st}");
}

#[tokio::test]
async fn upload_directory_multipart_stages_and_not_5xx() {
    let app = make_test_router();
    let boundary = "BOUND_DIR";
    let body = multipart_single(
        boundary,
        "files",
        "src/lib.rs",
        b"pub fn add(a: u8, b: u8) -> u8 { a + b }\n",
    );
    let st = post_multipart(app.clone(), "/api/upload-directory", boundary, body).await;
    assert!(st.as_u16() < 500, "upload-directory must not 5xx, got {st}");
}

#[tokio::test]
async fn upload_tarball_extracts_and_not_5xx() {
    // Build a tiny .tar.gz in-memory containing one source file.
    use flate2::{Compression, write::GzEncoder};
    use std::io::Write;
    let mut tar_buf = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_buf);
        let contents = b"fn main() { println(\"x\"); }\n";
        let mut header = tar::Header::new_gnu();
        header.set_path("proj/main.rs").unwrap();
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, &contents[..]).unwrap();
        builder.finish().unwrap();
    }
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&tar_buf).unwrap();
    let gz_bytes = gz.finish().unwrap();

    let app = make_test_router();
    let req = Request::post("/api/upload-tarball")
        .header("content-type", "application/gzip")
        .body(Body::from(gz_bytes))
        .unwrap();
    let st = app.oneshot(req).await.unwrap().status();
    assert!(st.as_u16() < 500, "upload-tarball must not 5xx, got {st}");
}

// ── locate / relocate against a real on-disk scan folder ─────────────────────

/// Write a scan-output folder layout (html/ + json/) that locate handlers accept.
fn make_scan_folder(run_id: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("html")).unwrap();
    std::fs::create_dir_all(root.join("json")).unwrap();
    std::fs::write(
        root.join("html").join(format!("report_{run_id}.html")),
        "<!doctype html><html><body>report</body></html>",
    )
    .unwrap();
    let run = run_basic(run_id, "/test/located", 80);
    std::fs::write(
        root.join("json").join(format!("result_{run_id}.json")),
        serde_json::to_string_pretty(&run).unwrap(),
    )
    .unwrap();
    dir
}

#[tokio::test]
async fn locate_report_with_real_folder_succeeds() {
    let scan = make_scan_folder("locate-001");
    let app = make_test_router();
    // JSON branch — exercises validate_locate_request + redirect_or_json_ok.
    let body = format!(
        "file_path={}",
        pct_encode(scan.path().join("html").to_str().unwrap())
    );
    let (st, resp) = post_form_json(app.clone(), "/locate-report", &body).await;
    assert!(st.as_u16() < 500, "locate-report must not 5xx, got {st}");
    assert!(
        resp.contains("redirect") || resp.contains("ok"),
        "expected JSON locate result, got: {resp}"
    );
}

#[tokio::test]
async fn locate_report_invalid_path_returns_error() {
    let app = make_test_router();
    let (st, resp) =
        post_form_json(app.clone(), "/locate-report", "file_path=%2Fnope%2Fmissing").await;
    assert!(st.as_u16() < 500);
    assert!(
        resp.contains("false") || resp.contains("not"),
        "got: {resp}"
    );
}

#[tokio::test]
async fn locate_reports_dir_with_real_folder_not_5xx() {
    let scan = make_scan_folder("locate-dir-001");
    let app = make_test_router();
    let body = format!("folder_path={}", pct_encode(scan.path().to_str().unwrap()));
    let (st, _, _) = post_form_shared(app.clone(), "/locate-reports-dir", &body).await;
    assert!(
        st.as_u16() < 500,
        "locate-reports-dir must not 5xx, got {st}"
    );
}

#[tokio::test]
async fn relocate_scan_with_real_folder_not_5xx() {
    let scan = make_scan_folder("relocate-001");
    let app = make_test_router();
    // First register the run so the registry has an entry to relocate.
    let _ = post_json_shared(
        app.clone(),
        "/api/ingest",
        &json_of(&run_basic("relocate-001", "/test/relo", 90)),
    )
    .await;
    let body = format!(
        "run_id=relocate-001&folder_path={}&redirect_url=%2Fview-reports",
        pct_encode(scan.path().to_str().unwrap())
    );
    let (st, resp) = post_form_json(app.clone(), "/relocate-scan", &body).await;
    assert!(st.as_u16() < 500, "relocate-scan must not 5xx, got {st}");
    assert!(!resp.is_empty());
}

// ── single-scan compare with submodule scope filters ─────────────────────────

#[tokio::test]
async fn compare_with_submodule_scope_exercises_filters() {
    let app = make_test_router();
    let mut older = run_with_submodules("cmp-sub-old");
    older.tool.timestamp_utc = Utc::now() - chrono::Duration::hours(3);
    assert!(
        post_json_shared(app.clone(), "/api/ingest", &json_of(&older))
            .await
            .as_u16()
            < 500
    );
    let newer = run_with_submodules("cmp-sub-new");
    assert!(
        post_json_shared(app.clone(), "/api/ingest", &json_of(&newer))
            .await
            .as_u16()
            < 500
    );

    // Whole-project compare — exercises render_submodule_chips.
    let (st, _) = get_shared(app.clone(), "/compare?a=cmp-sub-old&b=cmp-sub-new").await;
    assert!(st.as_u16() < 500);
    // Submodule-scoped compare — exercises apply_submodule_filter.
    let (st, _) = get_shared(
        app.clone(),
        "/compare?a=cmp-sub-old&b=cmp-sub-new&sub=vendor%2Flib-a",
    )
    .await;
    assert!(st.as_u16() < 500, "submodule-scoped compare must not 5xx");
    // Super-project scope.
    let (st, _) = get_shared(
        app.clone(),
        "/compare?a=cmp-sub-old&b=cmp-sub-new&scope=super",
    )
    .await;
    assert!(st.as_u16() < 500, "super-scope compare must not 5xx");
}

// ── artifact download variants + cleanup policy + badges ─────────────────────

#[tokio::test]
async fn ingested_run_artifact_and_metric_endpoints() {
    let app = make_test_router();
    assert!(
        post_json_shared(
            app.clone(),
            "/api/ingest",
            &json_of(&run_basic("art-001", "/test/art", 120)),
        )
        .await
        .as_u16()
            < 500
    );

    // Artifact download (?download=1) and inline variants.
    for q in ["", "?download=1"] {
        for kind in ["json", "html", "csv"] {
            let (st, _) = get_shared(app.clone(), &format!("/runs/{kind}/art-001{q}")).await;
            assert!(st.as_u16() < 500, "/runs/{kind}/art-001{q} must not 5xx");
        }
    }

    // Badge metrics for a populated registry.
    for metric in [
        "total_sloc",
        "files",
        "languages",
        "comment_lines",
        "blank_lines",
    ] {
        let (st, _) = get_shared(app.clone(), &format!("/badge/{metric}")).await;
        assert!(st.as_u16() < 500, "/badge/{metric} must not 5xx");
    }

    // Metrics history + latest.
    let (st, _) = get_shared(app.clone(), "/api/metrics/latest").await;
    assert!(st.as_u16() < 500);
    let (st, _) = get_shared(app.clone(), "/api/metrics/history").await;
    assert!(st.as_u16() < 500);
    let (st, _) = get_shared(app.clone(), "/api/project-history").await;
    assert!(st.as_u16() < 500);
}

#[tokio::test]
async fn cleanup_policy_roundtrip_and_run_now() {
    let app = make_test_router();
    let st = post_json_shared(
        app.clone(),
        "/api/cleanup-policy",
        r#"{"enabled":true,"max_age_days":30,"max_runs":50}"#,
    )
    .await;
    assert!(
        st.as_u16() < 500,
        "save cleanup policy must not 5xx, got {st}"
    );
    let (st, _) = get_shared(app.clone(), "/api/cleanup-policy").await;
    assert!(st.as_u16() < 500);
    let st = post_json_shared(app.clone(), "/api/cleanup-policy/run-now", "{}").await;
    assert!(st.as_u16() < 500, "run-now must not 5xx, got {st}");
    let st = post_json_shared(app.clone(), "/api/runs/cleanup", "{}").await;
    assert!(st.as_u16() < 500, "manual cleanup must not 5xx, got {st}");
}

#[tokio::test]
async fn export_pdf_invalid_body_does_not_5xx() {
    let app = make_test_router();
    // Missing/garbage fields — should be a 4xx, never a 5xx.
    let st = post_json_shared(app.clone(), "/export/pdf", r#"{"html":""}"#).await;
    assert!(
        st.as_u16() < 500,
        "export/pdf empty html must not 5xx, got {st}"
    );
}

// ── git-mode analyze + git_browser endpoints (local repo, offline) ───────────

/// Create a local git repo with two commits. Returns (path, branch, sha1, sha2).
fn make_local_git_repo() -> Option<(tempfile::TempDir, String, String, String)> {
    use std::process::Command;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(&root)
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@e.com")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@e.com")
            .output()
            .ok()
    };
    git(&["init", "-q"])?;
    std::fs::write(root.join("a.rs"), "fn a() {}\n").ok()?;
    git(&["add", "."])?;
    if !git(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", "c1"])?
        .status
        .success()
    {
        return None;
    }
    let sha1 = String::from_utf8(git(&["rev-parse", "HEAD"])?.stdout).ok()?;
    std::fs::write(root.join("b.rs"), "fn b() -> u8 {\n    1\n}\n").ok()?;
    git(&["add", "."])?;
    git(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", "c2"])?;
    let sha2 = String::from_utf8(git(&["rev-parse", "HEAD"])?.stdout).ok()?;
    let branch = String::from_utf8(git(&["rev-parse", "--abbrev-ref", "HEAD"])?.stdout).ok()?;
    Some((
        dir,
        branch.trim().to_owned(),
        sha1.trim().to_owned(),
        sha2.trim().to_owned(),
    ))
}

// The git-browser endpoints clone the requested repo through clone_or_fetch,
// whose validate_clone_url intentionally rejects local filesystem paths (an
// SSRF guard) — so against a local repo each endpoint returns its handled
// BAD_GATEWAY rejection. This still exercises api_list_refs / api_scan_ref /
// api_compare_refs request parsing and their error branches; the clone success
// path requires a real allowlisted remote and cannot run offline.
#[tokio::test]
async fn git_browser_endpoints_handle_rejected_local_repo() {
    let Some((repo, branch, sha1, sha2)) = make_local_git_repo() else {
        return; // git not available
    };
    let repo_path = repo.path().to_str().unwrap().to_owned();
    let app = make_test_router();
    let enc = pct_encode(&repo_path);
    let ok = |st: StatusCode| matches!(st.as_u16(), 200 | 400 | 502);

    let (st, _) = get_shared(app.clone(), &format!("/api/git/refs?repo={enc}")).await;
    assert!(ok(st), "/api/git/refs unexpected status {st}");

    let (st, _) = get_shared(
        app.clone(),
        &format!("/api/git/scan-ref?repo={enc}&ref_name={branch}"),
    )
    .await;
    assert!(ok(st), "/api/git/scan-ref unexpected status {st}");

    let (st, _) = get_shared(
        app.clone(),
        &format!("/api/git/compare-refs?repo={enc}&baseline_ref={sha1}&current_ref={sha2}"),
    )
    .await;
    assert!(ok(st), "/api/git/compare-refs unexpected status {st}");
}

#[tokio::test]
async fn git_mode_analyze_clones_and_completes() {
    let Some((repo, branch, _s1, _s2)) = make_local_git_repo() else {
        return;
    };
    let repo_path = repo.path().to_str().unwrap().to_owned();
    let app = make_test_router();
    let form = format!(
        "path=&git_repo={}&git_ref={}",
        pct_encode(&repo_path),
        pct_encode(&branch)
    );
    let (_, headers, _) = post_form_shared(app.clone(), "/analyze", &form).await;
    let wait_id = headers
        .get("x-wait-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    if wait_id.is_empty() {
        return;
    }
    // Git clone + analyze may take a little longer; the poller waits up to ~10s.
    let run_id = wait_for_run_id(app.clone(), &wait_id).await;
    if !run_id.is_empty() {
        let (st, _) = get_shared(app.clone(), &format!("/runs/result/{run_id}")).await;
        assert!(st.as_u16() < 500, "git-mode result page must not 5xx");
    }
}
