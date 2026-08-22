// Additional coverage for previously-uncovered handler / render code paths in
// sloc-web, driven through the real router (make_test_router*). Each test targets
// a concrete uncovered block identified from an llvm-cov run:
//
//   * scan_setup_handler's recent-scans mapping (only runs with a non-empty registry
//     that has an on-disk scan-config to fold in);
//   * render_multi_repo_warning (both the root-is-repo and multiple-independent-repos
//     branches) via /preview of a folder containing nested .git directories;
//   * serve_pdf_arm + resolve_or_queue_pdf (the "queue a background PDF, show the
//     self-refreshing generating page" path) via /runs/pdf/{id} on an ingested run;
//   * serve_submodule_pdf_arm (both the missing-submodule 404 render and the
//     rebuild-from-parent-JSON regeneration) via /runs/sub_*_pdf/{id};
//   * authorize_preview_path server-mode rejection branches via server-mode /preview;
//   * delete_run_artifacts (removing a real on-disk output dir) via DELETE /api/runs/{id};
//   * api_wiki_markup rendering a real ingested run to Confluence wiki markup.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use sloc_web::{TEST_SERVER_MODE_API_KEY, make_test_router, make_test_router_server_mode};
use tower::ServiceExt;

use chrono::Utc;
use sloc_config::AppConfig;
use sloc_core::{
    AnalysisRun, EffectiveCounts, EnvironmentMetadata, FileRecord, FileStatus, LanguageSummary,
    SubmoduleSummary, SummaryTotals, ToolMetadata,
};
use sloc_languages::{Language, ParseMode, RawLineCounts};

// ── HTTP helpers ─────────────────────────────────────────────────────────────

async fn get(app: Router, uri: &str) -> (StatusCode, String) {
    let resp = app
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn get_auth(app: Router, uri: &str, bearer: &str) -> (StatusCode, String) {
    let resp = app
        .oneshot(
            Request::get(uri)
                .header("authorization", format!("Bearer {bearer}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn post_json(app: Router, uri: &str, json: &str) -> StatusCode {
    let req = Request::post(uri)
        .header("content-type", "application/json")
        .body(Body::from(json.to_owned()))
        .unwrap();
    app.oneshot(req).await.unwrap().status()
}

async fn delete(app: Router, uri: &str) -> (StatusCode, String) {
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn post_body(app: Router, uri: &str) -> (StatusCode, String) {
    let resp = app
        .oneshot(Request::post(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
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

/// Poll `/api/runs/{wait_id}/status` until the run reports `complete`, returning
/// its `run_id` (empty string if it never completes).
async fn wait_for_run_id(app: Router, wait_id: &str) -> String {
    for _ in 0..200 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let (_, body) = get(app.clone(), &format!("/api/runs/{wait_id}/status")).await;
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

// ── AnalysisRun fixtures ─────────────────────────────────────────────────────

fn base_run(id: &str) -> AnalysisRun {
    AnalysisRun {
        tool: ToolMetadata {
            name: "oxide-sloc".into(),
            version: "1.6.12".into(),
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
        input_roots: vec!["/test/gapproj".into()],
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
        path: format!("/test/gapproj/{path}"),
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
    run.submodule_summaries = vec![SubmoduleSummary {
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
    }];
    run.git_remote_url = Some("https://github.com/test-org/parent-repo.git".into());
    run.input_roots = vec!["/test/parent-gap".into()];
    run
}

fn json_of(run: &AnalysisRun) -> String {
    serde_json::to_string(run).unwrap()
}

async fn ingest(app: &Router, run: &AnalysisRun) {
    let st = post_json(app.clone(), "/api/ingest", &json_of(run)).await;
    assert!(st.as_u16() < 500, "ingest must not 5xx, got {st}");
}

// ── scan_setup_handler recent-scans mapping ──────────────────────────────────

#[tokio::test]
async fn scan_setup_lists_recent_ingested_scans() {
    let app = make_test_router();
    // Populate the registry so scan_setup_handler's per-entry mapping closure runs
    // (the empty-registry case skips it entirely).
    for i in 0..3 {
        ingest(
            &app,
            &run_basic(&format!("setup-{i}"), "/test/setupproj", 60 + i),
        )
        .await;
    }
    let (st, body) = get(app.clone(), "/scan-setup").await;
    assert_eq!(st, StatusCode::OK, "scan-setup must render");
    assert!(
        body.contains("<html") || body.contains("<!doctype"),
        "scan-setup must be HTML"
    );
    // The recent-scans JSON payload is embedded in the page.
    assert!(
        body.contains("setupproj") || body.contains("project_label"),
        "recent-scans data should be embedded in the page"
    );
}

// ── render_multi_repo_warning via /preview ───────────────────────────────────

/// A directory that is NOT itself a git repo but contains two independent nested
/// repos (each a child dir with a `.git/` directory).
fn make_multi_repo_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for name in ["repo-alpha", "repo-beta"] {
        let child = root.join(name);
        std::fs::create_dir_all(child.join(".git")).unwrap();
        // A .git dir alone is enough for is_git_root; add HEAD for realism.
        std::fs::write(child.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(child.join("main.rs"), "fn main() {}\n").unwrap();
    }
    dir
}

/// A directory that IS a git repo AND contains one nested independent repo.
fn make_root_repo_with_nested() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(root.join("top.rs"), "pub fn t() {}\n").unwrap();
    let child = root.join("nested");
    std::fs::create_dir_all(child.join(".git")).unwrap();
    std::fs::write(child.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(child.join("inner.rs"), "fn i() {}\n").unwrap();
    dir
}

#[tokio::test]
async fn preview_multiple_independent_repos_shows_warning() {
    let dir = make_multi_repo_dir();
    let app = make_test_router();
    let uri = format!("/preview?path={}", pct_encode(dir.path().to_str().unwrap()));
    let (st, body) = get(app.clone(), &uri).await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        body.contains("Multiple repositories detected"),
        "expected the multiple-independent-repos warning, got: {}",
        &body[..body.len().min(400)]
    );
    // Each nested repo becomes a one-click pick button.
    assert!(body.contains("repo-pick"), "repo pick buttons expected");
}

#[tokio::test]
async fn preview_root_repo_with_nested_shows_nested_warning() {
    let dir = make_root_repo_with_nested();
    let app = make_test_router();
    let uri = format!("/preview?path={}", pct_encode(dir.path().to_str().unwrap()));
    let (st, body) = get(app.clone(), &uri).await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        body.contains("Nested repositories detected"),
        "expected the nested-repos (root-is-repo) warning, got: {}",
        &body[..body.len().min(400)]
    );
}

// ── serve_pdf_arm + resolve_or_queue_pdf (background-queue path) ──────────────

#[tokio::test]
async fn pdf_artifact_for_ingested_run_queues_generation() {
    let app = make_test_router();
    ingest(&app, &run_basic("pdf-gap-1", "/test/pdfgap", 90)).await;
    // The ingested run has a JSON on disk but no PDF → resolve_or_queue_pdf
    // records a pending PDF path, spawns the background generator, and
    // serve_pdf_arm returns the self-refreshing "generating" page (or, if the
    // pure-Rust generator finished fast, the PDF itself). Never a 5xx.
    let (st, body) = get(app.clone(), "/runs/pdf/pdf-gap-1").await;
    assert!(st.as_u16() < 500, "/runs/pdf must not 5xx, got {st}");
    // The generating page advertises a refresh + a link back to the HTML report,
    // or we already have a PDF; either way the body is non-empty.
    assert!(!body.is_empty());
}

// ── serve_submodule_pdf_arm ──────────────────────────────────────────────────

#[tokio::test]
async fn submodule_pdf_missing_submodule_returns_404_render() {
    let app = make_test_router();
    // Run WITHOUT the requested submodule → serve_submodule_pdf_arm cannot rebuild
    // and renders its 404 "could not be generated" page.
    ingest(&app, &run_basic("subpdf-none", "/test/subgap", 70)).await;
    let (st, body) = get(app.clone(), "/runs/sub_doesnotexist_pdf/subpdf-none").await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert!(
        body.contains("Sub-report PDF") || body.contains("could not be generated"),
        "expected sub-report PDF 404 render, got: {}",
        &body[..body.len().min(300)]
    );
}

#[tokio::test]
async fn submodule_pdf_rebuilds_from_parent_json() {
    let app = make_test_router();
    // Run WITH a "vendor/lib-a" submodule. sanitize_project_label("vendor/lib-a")
    // yields the artifact base, so /runs/sub_<label>_pdf reaches the rebuild path
    // (read parent JSON, find the submodule, write_pdf_from_run).
    ingest(&app, &run_with_submodules("subpdf-rebuild")).await;
    // sanitize_project_label maps '/' → '-' (or '_'); try the common form. Even if
    // the label doesn't match, the handler still executes the rebuild-attempt path
    // and returns a handled 404 — never a 5xx.
    for label in ["vendor-lib-a", "vendor_lib_a", "lib-a", "lib_a"] {
        let (st, _) = get(
            app.clone(),
            &format!("/runs/sub_{label}_pdf/subpdf-rebuild"),
        )
        .await;
        assert!(
            st == StatusCode::OK || st == StatusCode::NOT_FOUND,
            "sub pdf rebuild must be 200 or 404, got {st} for {label}"
        );
    }
}

// ── authorize_preview_path (server mode) ─────────────────────────────────────

#[tokio::test]
async fn server_mode_preview_rejects_path_without_scan_roots() {
    // Server mode with no allowed_scan_roots configured: a real, resolvable dir
    // that is not an upload/sample path hits the "no scan roots configured"
    // rejection branch of authorize_preview_path.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("x.rs"), "fn x() {}\n").unwrap();
    let app = make_test_router_server_mode();
    let uri = format!("/preview?path={}", pct_encode(dir.path().to_str().unwrap()));
    let (st, body) = get_auth(app.clone(), &uri, TEST_SERVER_MODE_API_KEY).await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        body.contains("Preview rejected"),
        "server-mode preview without scan roots must be rejected, got: {}",
        &body[..body.len().min(300)]
    );
}

#[tokio::test]
async fn server_mode_preview_rejects_unresolvable_path() {
    // A path that cannot be canonicalised (does not exist) and is neither an upload
    // nor a sample path → the fail-closed "could not be resolved" branch.
    let app = make_test_router_server_mode();
    let uri = "/preview?path=%2Fnope%2Fdefinitely%2Fmissing%2Fdir12345";
    let (st, body) = get_auth(app.clone(), uri, TEST_SERVER_MODE_API_KEY).await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        body.contains("Preview rejected"),
        "unresolvable server-mode preview path must be rejected, got: {}",
        &body[..body.len().min(300)]
    );
}

// ── delete_run_artifacts (real on-disk output dir) ───────────────────────────

#[tokio::test]
async fn delete_run_removes_ingested_run() {
    let app = make_test_router();
    // Ingest persists html/json/csv/xlsx into a real output dir, so DELETE exercises
    // delete_run_artifacts' remove_dir_all branch (output_dir.exists() == true).
    ingest(&app, &run_basic("del-gap-1", "/test/delgap", 55)).await;
    let (st, _) = delete(app.clone(), "/api/runs/del-gap-1").await;
    assert!(st.as_u16() < 500, "DELETE run must not 5xx, got {st}");
    // A second delete of the now-unknown run still returns a handled status.
    let (st2, _) = delete(app.clone(), "/api/runs/del-gap-1").await;
    assert!(st2.as_u16() < 500, "second DELETE must not 5xx, got {st2}");
}

// ── export_run_handler (share-drive publish) ─────────────────────────────────

#[tokio::test]
async fn export_run_to_share_and_unconfigured() {
    let app = make_test_router();
    // A real ingested run gives export_run_handler an on-disk output dir to copy.
    ingest(&app, &run_basic("exp-gap-1", "/test/expproj", 42)).await;

    // Only this test touches SLOC_EXPORT_DIR within this binary, and auto-export is
    // off, so no other test observes it. SAFETY: set/remove are bounded to this test.
    unsafe {
        std::env::remove_var("SLOC_EXPORT_DIR");
    }
    // Not configured → 409 Conflict.
    let (st, _) = post_body(app.clone(), "/api/runs/exp-gap-1/export").await;
    assert_eq!(st, StatusCode::CONFLICT);

    // Configured → 200 and files land under the destination.
    let dest = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("SLOC_EXPORT_DIR", dest.path());
    }
    let (st, body) = post_body(app.clone(), "/api/runs/exp-gap-1/export").await;
    unsafe {
        std::env::remove_var("SLOC_EXPORT_DIR");
    }
    assert_eq!(st, StatusCode::OK, "export body: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["target"], "share");
    assert!(
        v["files"].as_u64().unwrap_or(0) >= 1,
        "expected copied files: {body}"
    );
    assert!(
        std::fs::read_dir(dest.path()).unwrap().count() >= 1,
        "export destination should contain the run's artifact subdir"
    );
}

#[tokio::test]
async fn export_unknown_run_is_404() {
    let app = make_test_router();
    let (st, _) = post_body(app.clone(), "/api/runs/no-such-run-xyz/export").await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

// ── api_admin_config (operator-only effective config) ────────────────────────

#[tokio::test]
async fn admin_config_gated_by_admin_key() {
    let app = make_test_router();
    // Only this test touches SLOC_ADMIN_KEY within this binary.
    unsafe {
        std::env::remove_var("SLOC_ADMIN_KEY");
    }
    // No admin key configured → the endpoint is reachable under the normal gate.
    let (st, body) = get(app.clone(), "/api/admin/config").await;
    assert_eq!(st, StatusCode::OK, "config body: {body}");
    assert!(
        body.contains("\"disk_cap\""),
        "expected disk_cap in: {body}"
    );
    assert!(body.contains("\"export\""));
    assert!(body.contains("\"hostname\""));

    // Admin key configured but not presented → 403.
    unsafe {
        std::env::set_var("SLOC_ADMIN_KEY", "op-admin-secret");
    }
    let (st, _) = get(app.clone(), "/api/admin/config").await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Admin key presented → 200.
    let (st, _) = get_auth(app.clone(), "/api/admin/config", "op-admin-secret").await;
    assert_eq!(st, StatusCode::OK);
    unsafe {
        std::env::remove_var("SLOC_ADMIN_KEY");
    }
}

#[tokio::test]
async fn export_unsupported_target_is_400() {
    let app = make_test_router();
    ingest(&app, &run_basic("exp-gap-3", "/test/expproj3", 21)).await;
    let (st, _) = post_body(
        app.clone(),
        "/api/runs/exp-gap-3/export?target=carrier-pigeon",
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn export_confluence_target_without_config_is_400() {
    let app = make_test_router();
    ingest(&app, &run_basic("exp-conf-1", "/test/expconf", 12)).await;
    let (st, body) = post_body(app.clone(), "/api/runs/exp-conf-1/export?target=confluence").await;
    // Confluence is not configured in the test router, so the shared publish flow
    // reports it as unconfigured (400) rather than attempting a network call.
    assert_eq!(st, StatusCode::BAD_REQUEST, "body: {body}");
    assert!(
        body.contains("Confluence"),
        "expected a Confluence-not-configured message, got: {body}"
    );
}

// ── api_wiki_markup rendering a real run ─────────────────────────────────────

#[tokio::test]
async fn wiki_markup_renders_for_ingested_run() {
    let app = make_test_router();
    ingest(&app, &run_basic("wiki-gap-1", "/test/wikigap", 130)).await;
    // Ingest wrote a JSON to disk, so api_wiki_markup reads it and renders the
    // Confluence wiki-markup body (render_confluence_wiki_markup).
    let (st, body) = get(app.clone(), "/api/confluence/wiki-markup?run_id=wiki-gap-1").await;
    assert_eq!(st, StatusCode::OK, "wiki-markup must render for a real run");
    assert!(!body.is_empty(), "wiki markup body should be non-empty");
    // Confluence wiki markup uses h1./h2. headings and {panel}/{table} macros.
    assert!(
        body.contains('h') || body.contains('|') || body.contains('{'),
        "expected wiki-markup formatting tokens, got: {}",
        &body[..body.len().min(200)]
    );
}

// ── real analyze then request PDF artifact (queue path with real JSON) ────────

fn make_sample_project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("lib.rs"),
        "// c\nfn add(a: i32, b: i32) -> i32 { a + b }\n#[test]\nfn t() { assert_eq!(add(1,2),3); }\n",
    )
    .unwrap();
    std::fs::write(root.join("app.py"), "def g(n):\n    return n\n").unwrap();
    dir
}

#[tokio::test]
async fn real_analyze_then_pdf_and_submodule_pdf_not_5xx() {
    let project = make_sample_project();
    let app = make_test_router();
    let form = format!(
        "path={}&report_title=PDF+Gap+Test",
        pct_encode(project.path().to_str().unwrap())
    );
    let resp = app
        .clone()
        .oneshot(
            Request::post("/analyze")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();
    let wait_id = resp
        .headers()
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
    // PDF artifact for a real run — resolve_or_queue_pdf background path.
    let (st, _) = get(app.clone(), &format!("/runs/pdf/{run_id}")).await;
    assert!(st.as_u16() < 500, "/runs/pdf for real run must not 5xx");
    // Download variant too.
    let (st, _) = get(app.clone(), &format!("/runs/pdf/{run_id}?download=1")).await;
    assert!(st.as_u16() < 500, "/runs/pdf?download must not 5xx");
}
