// Further coverage for reachable-but-previously-uncovered handler branches in
// sloc-web/src/lib.rs, driven through the real router:
//
//   * cleanup_runs_handler's expired-run deletion loop — only runs when the
//     registry actually holds an entry older than the cutoff (existing tests only
//     posted an empty registry, so the loop body never executed);
//   * locate_report_handler's "found HTML but no matching JSON / no registry entry"
//     error branch, plus the "folder contains a different scan" (run_id mismatch)
//     branch.

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
    SummaryTotals, ToolMetadata,
};
use sloc_languages::{Language, ParseMode, RawLineCounts};

// ── HTTP helpers ─────────────────────────────────────────────────────────────

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

/// POST a urlencoded form with `Accept: application/json`.
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

// ── AnalysisRun fixture ──────────────────────────────────────────────────────

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
        input_roots: vec!["/test/gap2".into()],
        summary_totals: SummaryTotals::default(),
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

fn file_record(path: &str, code: u64) -> FileRecord {
    FileRecord {
        path: format!("/test/gap2/{path}"),
        relative_path: path.into(),
        language: Some(Language::Rust),
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

const fn lang_summary(code: u64) -> LanguageSummary {
    LanguageSummary {
        language: Language::Rust,
        files: 1,
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

fn run_basic(id: &str, code: u64) -> AnalysisRun {
    let mut run = base_run(id);
    run.per_file_records = vec![file_record("src/lib.rs", code)];
    run.totals_by_language = vec![lang_summary(code)];
    run.summary_totals = SummaryTotals {
        files_considered: 1,
        files_analyzed: 1,
        code_lines: code,
        total_physical_lines: code + 2,
        ..SummaryTotals::default()
    };
    run
}

fn json_of(run: &AnalysisRun) -> String {
    serde_json::to_string(run).unwrap()
}

// ── cleanup_runs_handler expired-deletion loop ───────────────────────────────

#[tokio::test]
async fn cleanup_deletes_runs_older_than_cutoff() {
    let app = make_test_router();
    // Ingest a run whose timestamp is 90 days in the past. Its registry entry
    // inherits run.tool.timestamp_utc, so it is older than any positive cutoff.
    let mut old = run_basic("cleanup-old-1", 80);
    old.tool.timestamp_utc = Utc::now() - chrono::Duration::days(90);
    let (st, _) = post_json(app.clone(), "/api/ingest", &json_of(&old)).await;
    assert!(st.as_u16() < 500, "ingest must not 5xx");

    // Also ingest a fresh run that must survive the cleanup.
    let (st, _) = post_json(
        app.clone(),
        "/api/ingest",
        &json_of(&run_basic("cleanup-new-1", 40)),
    )
    .await;
    assert!(st.as_u16() < 500);

    // older_than_days=1 → the 90-day-old run is expired; the deletion loop runs
    // remove_dir_all on its real on-disk output dir and purges it from the registry.
    let (st, body) = post_json(app.clone(), "/api/runs/cleanup", r#"{"older_than_days":1}"#).await;
    assert_eq!(st, StatusCode::OK, "cleanup must return 200");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(
        v["deleted"].as_u64().unwrap_or(0) >= 1,
        "at least the 90-day-old run must be deleted, got: {body}"
    );
}

// ── locate_report_handler: HTML present but no JSON / no registry entry ───────

/// A scan folder that contains only an HTML report (no json/ subfolder), so the
/// locate handler finds the HTML but no result_*.json.
fn make_html_only_folder(run_id: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("html")).unwrap();
    std::fs::write(
        root.join("html").join(format!("report_{run_id}.html")),
        "<!doctype html><html><body>report</body></html>",
    )
    .unwrap();
    dir
}

#[tokio::test]
async fn locate_report_html_without_json_returns_error() {
    let scan = make_html_only_folder("nojson-001");
    let app = make_test_router();
    let body = format!(
        "file_path={}",
        pct_encode(scan.path().join("html").to_str().unwrap())
    );
    let (st, resp) = post_form_json(app.clone(), "/locate-report", &body).await;
    assert!(st.as_u16() < 500, "locate must not 5xx, got {st}");
    // The "no result_*.json found" error branch surfaces as a JSON failure.
    assert!(
        resp.contains("false") || resp.to_lowercase().contains("json"),
        "expected a no-JSON locate error, got: {resp}"
    );
}

/// A scan folder whose JSON records a run_id that differs from the caller's
/// expected_run_id → the "folder contains a different scan" branch.
fn make_scan_folder_with_json(actual_run_id: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("html")).unwrap();
    std::fs::create_dir_all(root.join("json")).unwrap();
    std::fs::write(
        root.join("html")
            .join(format!("report_{actual_run_id}.html")),
        "<!doctype html><html><body>r</body></html>",
    )
    .unwrap();
    let run = run_basic(actual_run_id, 60);
    std::fs::write(
        root.join("json")
            .join(format!("result_{actual_run_id}.json")),
        serde_json::to_string_pretty(&run).unwrap(),
    )
    .unwrap();
    dir
}

#[tokio::test]
async fn locate_report_mismatched_run_id_returns_different_scan_error() {
    let scan = make_scan_folder_with_json("actual-777");
    let app = make_test_router();
    // Ask to link folder as run "expected-999" — but the JSON says "actual-777".
    let body = format!(
        "file_path={}&expected_run_id=expected-999",
        pct_encode(scan.path().join("html").to_str().unwrap())
    );
    let (st, resp) = post_form_json(app.clone(), "/locate-report", &body).await;
    assert!(st.as_u16() < 500, "locate must not 5xx, got {st}");
    assert!(
        resp.to_lowercase().contains("different")
            || resp.contains("actual-777")
            || resp.contains("false"),
        "expected a different-scan / mismatch error, got: {resp}"
    );
}
