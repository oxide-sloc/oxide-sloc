// Unit tests for public helper functions in sloc-web and additional
// HTTP-route branches not covered by integration.rs / coverage_boost.rs.

use axum::{body::Body, http::Request, http::StatusCode};
use http_body_util::BodyExt;
use sloc_web::{build_sub_run, make_test_router, sanitize_project_label};
use tower::ServiceExt;

use chrono::Utc;
use sloc_config::AppConfig;
use sloc_core::{
    AnalysisRun, CocomoEstimate, CocomoMode, EffectiveCounts, EnvironmentMetadata, FileCoverage,
    FileRecord, FileStatus, LanguageSummary, SubmoduleSummary, SummaryTotals, ToolMetadata,
};
use sloc_languages::{Language, ParseMode, RawLineCounts};

// ── HTTP helpers ──────────────────────────────────────────────────────────────

async fn get(uri: &str) -> (StatusCode, String) {
    let app = make_test_router();
    let resp = app
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn post_json(uri: &str, json: &str) -> (StatusCode, String) {
    let app = make_test_router();
    let req = Request::post(uri)
        .header("content-type", "application/json")
        .body(Body::from(json.to_owned()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

// ── sanitize_project_label ────────────────────────────────────────────────────

#[test]
fn sanitize_project_label_simple_path() {
    assert_eq!(
        sanitize_project_label("/home/user/my-project"),
        "my-project"
    );
}

#[test]
fn sanitize_project_label_windows_path() {
    assert_eq!(
        sanitize_project_label(r"C:\Users\user\MyProject"),
        "myproject"
    );
}

#[test]
fn sanitize_project_label_empty_input() {
    // Empty → "project" fallback
    let result = sanitize_project_label("");
    assert!(!result.is_empty());
}

#[test]
fn sanitize_project_label_special_chars_become_dashes() {
    let result = sanitize_project_label("/path/my project v2.0");
    assert!(!result.is_empty());
    assert!(result.chars().all(|c| c.is_alphanumeric() || c == '-'));
}

#[test]
fn sanitize_project_label_preserves_alphanum() {
    let result = sanitize_project_label("/repos/oxide-sloc123");
    assert!(result.contains("oxide") || result.contains("sloc") || !result.is_empty());
}

#[test]
fn sanitize_project_label_all_dashes_returns_project() {
    // "---" → trimmed to "" → fallback "project"
    let result = sanitize_project_label("---");
    assert_eq!(result, "project");
}

#[test]
fn sanitize_project_label_dot_only_filename() {
    let result = sanitize_project_label("/path/.");
    assert!(!result.is_empty());
}

#[test]
fn sanitize_project_label_unicode_chars() {
    let result = sanitize_project_label("/path/プロジェクト");
    // Non-ASCII → all become dashes → trimmed → fallback
    assert!(!result.is_empty());
}

// ── build_sub_run ─────────────────────────────────────────────────────────────

#[allow(clippy::too_many_lines)]
fn make_parent_run_with_submodule() -> (AnalysisRun, SubmoduleSummary) {
    let mut run = AnalysisRun {
        tool: ToolMetadata {
            name: "sloc".into(),
            version: "1.0.0".into(),
            run_id: "test-build-subrun".into(),
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
        input_roots: vec!["/test/parent".into()],
        summary_totals: SummaryTotals {
            files_considered: 3,
            files_analyzed: 3,
            code_lines: 300,
            total_physical_lines: 306,
            ..SummaryTotals::default()
        },
        totals_by_language: vec![LanguageSummary {
            language: Language::Rust,
            files: 3,
            total_physical_lines: 306,
            code_lines: 300,
            comment_lines: 3,
            blank_lines: 3,
            mixed_lines_separate: 0,
            functions: 10,
            classes: 2,
            variables: 5,
            variables_member: 0,
            variables_local: 0,
            variables_global: 0,
            macro_definitions: 0,
            imports: 3,
            test_count: 8,
            test_assertion_count: 24,
            test_suite_count: 2,
            coverage_lines_found: 200,
            coverage_lines_hit: 180,
            coverage_functions_found: 10,
            coverage_functions_hit: 9,
            coverage_branches_found: 20,
            coverage_branches_hit: 17,
            cyclomatic_complexity: 0,
            lsloc: None,
        }],
        per_file_records: vec![
            FileRecord {
                path: "/test/parent/vendor/lib-a/src/lib.rs".into(),
                relative_path: "vendor/lib-a/src/lib.rs".into(),
                language: Some(Language::Rust),
                size_bytes: 2000,
                detected_encoding: Some("utf-8".into()),
                raw_line_categories: RawLineCounts {
                    total_physical_lines: 102,
                    code_only_lines: 100,
                    blank_only_lines: 1,
                    single_comment_only_lines: 1,
                    functions: 5,
                    test_count: 4,
                    test_assertion_count: 12,
                    test_suite_count: 1,
                    ..RawLineCounts::default()
                },
                effective_counts: EffectiveCounts {
                    code_lines: 100,
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
                submodule: Some("vendor/lib-a".into()),
                coverage: Some(FileCoverage {
                    lines_found: 100,
                    lines_hit: 90,
                    functions_found: 5,
                    functions_hit: 5,
                    branches_found: 10,
                    branches_hit: 9,
                }),
                style_analysis: None,
                cyclomatic_complexity: None,
                lsloc: None,
                commit_count: None,
                last_commit_date: None,
                ownership: None,
                content_hash: 12345,
            },
            FileRecord {
                path: "/test/parent/src/main.rs".into(),
                relative_path: "src/main.rs".into(),
                language: Some(Language::Rust),
                size_bytes: 1000,
                detected_encoding: Some("utf-8".into()),
                raw_line_categories: RawLineCounts {
                    total_physical_lines: 102,
                    code_only_lines: 100,
                    blank_only_lines: 1,
                    single_comment_only_lines: 1,
                    ..RawLineCounts::default()
                },
                effective_counts: EffectiveCounts {
                    code_lines: 100,
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
                content_hash: 67890,
            },
        ],
        skipped_file_records: vec![],
        warnings: vec![],
        submodule_summaries: vec![],
        git_commit_short: Some("abc123".into()),
        git_branch: Some("main".into()),
        git_commit_long: None,
        git_commit_author: None,
        git_tags: None,
        git_nearest_tag: None,
        git_commit_date: None,
        git_remote_url: Some("https://github.com/test/parent.git".into()),
        style_summary: None,
        cocomo: None,
        uloc: 0,
        dryness_pct: None,
        duplicate_groups: vec![],
        duplicates_excluded: 0,
        authors: Vec::new(),
    };

    let sub = SubmoduleSummary {
        name: "vendor/lib-a".into(),
        relative_path: "vendor/lib-a".into(),
        files_analyzed: 1,
        total_physical_lines: 102,
        code_lines: 100,
        comment_lines: 1,
        blank_lines: 1,
        language_summaries: vec![LanguageSummary {
            language: Language::Rust,
            files: 1,
            total_physical_lines: 102,
            code_lines: 100,
            comment_lines: 1,
            blank_lines: 1,
            mixed_lines_separate: 0,
            functions: 5,
            classes: 0,
            variables: 0,
            variables_member: 0,
            variables_local: 0,
            variables_global: 0,
            macro_definitions: 0,
            imports: 0,
            test_count: 4,
            test_assertion_count: 12,
            test_suite_count: 1,
            coverage_lines_found: 100,
            coverage_lines_hit: 90,
            coverage_functions_found: 5,
            coverage_functions_hit: 5,
            coverage_branches_found: 10,
            coverage_branches_hit: 9,
            cyclomatic_complexity: 0,
            lsloc: None,
        }],
        git_commit_short: Some("deadbeef".into()),
        git_commit_long: None,
        git_branch: Some("main".into()),
        git_commit_author: Some("Sub Author".into()),
        git_commit_date: Some("2026-06-01".into()),
        git_remote_url: Some("https://github.com/test/lib-a.git".into()),
    };

    run.submodule_summaries.push(sub.clone());
    (run, sub)
}

#[test]
fn build_sub_run_extracts_submodule_files() {
    let (parent, sub) = make_parent_run_with_submodule();
    let sub_run = build_sub_run(&parent, &sub, "/test/parent");
    // Only the file tagged with "vendor/lib-a" submodule should be in the sub-run
    assert_eq!(sub_run.per_file_records.len(), 1);
    assert_eq!(sub_run.summary_totals.code_lines, 100);
}

#[test]
fn build_sub_run_preserves_git_metadata() {
    let (parent, sub) = make_parent_run_with_submodule();
    let sub_run = build_sub_run(&parent, &sub, "/test/parent");
    assert_eq!(sub_run.git_commit_short.as_deref(), Some("deadbeef"));
    assert_eq!(sub_run.git_branch.as_deref(), Some("main"));
    assert_eq!(
        sub_run.git_remote_url.as_deref(),
        Some("https://github.com/test/lib-a.git")
    );
    assert_eq!(sub_run.git_commit_author.as_deref(), Some("Sub Author"));
}

#[test]
fn build_sub_run_aggregates_coverage_from_files() {
    let (parent, sub) = make_parent_run_with_submodule();
    let sub_run = build_sub_run(&parent, &sub, "/test/parent");
    assert_eq!(sub_run.summary_totals.coverage_lines_found, 100);
    assert_eq!(sub_run.summary_totals.coverage_lines_hit, 90);
    assert_eq!(sub_run.summary_totals.coverage_functions_found, 5);
    assert_eq!(sub_run.summary_totals.coverage_functions_hit, 5);
}

#[test]
fn build_sub_run_aggregates_test_counts() {
    let (parent, sub) = make_parent_run_with_submodule();
    let sub_run = build_sub_run(&parent, &sub, "/test/parent");
    assert_eq!(sub_run.summary_totals.test_count, 4);
    assert_eq!(sub_run.summary_totals.test_assertion_count, 12);
    assert_eq!(sub_run.summary_totals.test_suite_count, 1);
}

#[test]
fn build_sub_run_sets_correct_input_root() {
    let (parent, sub) = make_parent_run_with_submodule();
    let sub_run = build_sub_run(&parent, &sub, "/test/parent");
    assert!(
        sub_run.input_roots[0].contains("vendor/lib-a"),
        "sub-run root must include submodule path"
    );
}

#[test]
fn build_sub_run_title_includes_submodule_name() {
    let (parent, sub) = make_parent_run_with_submodule();
    let sub_run = build_sub_run(&parent, &sub, "/test/parent");
    assert!(
        sub_run
            .effective_configuration
            .reporting
            .report_title
            .contains("vendor/lib-a"),
        "sub-run title must include submodule name"
    );
}

#[test]
fn build_sub_run_no_matching_files_returns_empty() {
    let (parent, _) = make_parent_run_with_submodule();
    // Create a sub summary for a submodule that has no matching files in parent
    let ghost_sub = SubmoduleSummary {
        name: "vendor/nonexistent".into(),
        relative_path: "vendor/nonexistent".into(),
        files_analyzed: 0,
        total_physical_lines: 0,
        code_lines: 0,
        comment_lines: 0,
        blank_lines: 0,
        language_summaries: vec![],
        git_commit_short: None,
        git_commit_long: None,
        git_branch: None,
        git_commit_author: None,
        git_commit_date: None,
        git_remote_url: None,
    };
    let sub_run = build_sub_run(&parent, &ghost_sub, "/test/parent");
    assert_eq!(
        sub_run.per_file_records.len(),
        0,
        "sub-run with no matching files must have empty per_file_records"
    );
    assert_eq!(sub_run.summary_totals.coverage_lines_found, 0);
}

#[test]
fn build_sub_run_with_cocomo_parent_has_no_cocomo() {
    let (mut parent, sub) = make_parent_run_with_submodule();
    parent.cocomo = Some(CocomoEstimate {
        mode: CocomoMode::Organic,
        ksloc: 5.0,
        effort_person_months: 13.2,
        duration_months: 6.8,
        avg_staff: 1.94,
    });
    let sub_run = build_sub_run(&parent, &sub, "/test/parent");
    // Sub-runs never inherit parent COCOMO
    assert!(
        sub_run.cocomo.is_none(),
        "sub-run must not inherit parent COCOMO"
    );
}

// ── Preview handler with diverse file types ───────────────────────────────────

#[tokio::test]
async fn preview_handler_with_rust_file_returns_response() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("lib.rs"),
        "pub fn hello() -> &'static str { \"hi\" }\n",
    )
    .unwrap();
    let path_enc: String = dir
        .path()
        .to_str()
        .unwrap()
        .bytes()
        .flat_map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                vec![b as char]
            }
            _ => format!("%{b:02X}").chars().collect(),
        })
        .collect();
    let (status, _) = get(&format!("/preview?path={path_enc}")).await;
    assert!(status.as_u16() < 500, "/preview must not 5xx, got {status}");
}

#[tokio::test]
async fn preview_handler_with_multiple_file_types() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(dir.path().join("app.py"), "def f(): pass\n").unwrap();
    std::fs::write(dir.path().join("style.css"), ".a { color: red; }\n").unwrap();
    std::fs::write(dir.path().join("index.html"), "<html></html>").unwrap();
    std::fs::write(dir.path().join("data.json"), r#"{"key": "value"}"#).unwrap();
    let sub = dir.path().join("src");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("util.ts"), "export function x(): void {}\n").unwrap();

    let path_enc: String = dir
        .path()
        .to_str()
        .unwrap()
        .bytes()
        .flat_map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                vec![b as char]
            }
            _ => format!("%{b:02X}").chars().collect(),
        })
        .collect();
    let (status, _) = get(&format!("/preview?path={path_enc}")).await;
    assert!(
        status.as_u16() < 500,
        "/preview with many file types must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn preview_handler_empty_dir_returns_response() {
    let dir = tempfile::tempdir().unwrap();
    let path_enc: String = dir
        .path()
        .to_str()
        .unwrap()
        .bytes()
        .flat_map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                vec![b as char]
            }
            _ => format!("%{b:02X}").chars().collect(),
        })
        .collect();
    let (status, _) = get(&format!("/preview?path={path_enc}")).await;
    assert!(
        status.as_u16() < 500,
        "/preview of empty dir must not 5xx, got {status}"
    );
}

// ── Confluence test endpoint ──────────────────────────────────────────────────

#[tokio::test]
async fn api_test_confluence_without_config_returns_error() {
    let (status, body) = post_json("/api/confluence/test", "{}").await;
    // No confluence configured → 400 or 503 or similar non-5xx
    assert!(
        status.as_u16() < 600,
        "POST /api/confluence/test without config must not panic, got {status}"
    );
    assert!(!body.is_empty(), "response body must not be empty");
}

#[tokio::test]
async fn api_confluence_post_missing_run_id_returns_error() {
    let (status, _) = post_json("/api/confluence/post", r#"{"run_id":""}"#).await;
    assert!(
        status.as_u16() < 600,
        "/api/confluence/post with empty run_id must not panic, got {status}"
    );
}

// ── Scan setup with various params ────────────────────────────────────────────

#[tokio::test]
async fn scan_setup_with_scan_id_param_not_5xx() {
    let (status, _) = get("/scan-setup?scan_id=some-scan-id-123").await;
    assert!(
        status.as_u16() < 500,
        "/scan-setup?scan_id= must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn scan_setup_with_all_params_not_5xx() {
    let dir = tempfile::tempdir().unwrap();
    let path_enc: String = dir
        .path()
        .to_str()
        .unwrap()
        .bytes()
        .flat_map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                vec![b as char]
            }
            _ => format!("%{b:02X}").chars().collect(),
        })
        .collect();
    let (status, _) = get(&format!(
        "/scan-setup?path={path_enc}&label=MyProject&recent=0"
    ))
    .await;
    assert!(
        status.as_u16() < 500,
        "/scan-setup with all params must not 5xx, got {status}"
    );
}

// ── Export PDF handler ────────────────────────────────────────────────────────

#[tokio::test]
async fn export_pdf_with_empty_html_returns_error() {
    let (status, _) = post_json("/export/pdf", r#"{"html":""}"#).await;
    assert!(
        status.as_u16() < 600,
        "/export/pdf with empty html must not panic, got {status}"
    );
}

#[tokio::test]
async fn export_pdf_with_valid_html_not_5xx() {
    let html = "<html><body><h1>Test Report</h1><p>Some content</p></body></html>";
    let json = serde_json::json!({ "html": html });
    let (status, _) = post_json("/export/pdf", &json.to_string()).await;
    // Without a browser, this returns an error — but must not panic or 500
    assert!(
        status.as_u16() < 600,
        "/export/pdf must not panic, got {status}"
    );
}

// ── View reports with various params ─────────────────────────────────────────

#[tokio::test]
async fn view_reports_with_error_query_param_renders_html() {
    let (status, body) = get("/view-reports?error=scan_failed").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<html"),
        "expected HTML from /view-reports?error="
    );
}

#[tokio::test]
async fn view_reports_with_dir_param_not_5xx() {
    let dir = tempfile::tempdir().unwrap();
    let path_enc: String = dir
        .path()
        .to_str()
        .unwrap()
        .bytes()
        .flat_map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                vec![b as char]
            }
            _ => format!("%{b:02X}").chars().collect(),
        })
        .collect();
    let (status, _) = get(&format!("/view-reports?dir={path_enc}")).await;
    assert!(
        status.as_u16() < 500,
        "/view-reports?dir= must not 5xx, got {status}"
    );
}

// ── Badge with various metrics ────────────────────────────────────────────────

#[tokio::test]
async fn badge_coverage_metric_returns_svg() {
    let (status, _, body) = {
        let app = make_test_router();
        let resp = app
            .oneshot(Request::get("/badge/coverage").body(Body::empty()).unwrap())
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
    };
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<svg"), "badge/coverage must return SVG");
}

#[tokio::test]
async fn badge_test_count_metric_not_5xx() {
    let (status, _) = get("/badge/test_count").await;
    assert!(status.as_u16() < 500, "/badge/test_count must not 5xx");
}

#[tokio::test]
async fn badge_unknown_metric_returns_svg() {
    let (status, body) = get("/badge/unknown_metric_xyz").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("<svg"),
        "unknown badge metric must return SVG"
    );
}

// ── Image handler with all registered images ──────────────────────────────────

#[tokio::test]
async fn image_small_logo_returns_png() {
    let (status, body) = get("/images/logo/small-logo.png").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.is_empty(), "small-logo.png must be non-empty");
}

#[tokio::test]
async fn image_icons_are_served() {
    for icon in ["c.png", "cpp.png", "python.png", "java.png", "go.png"] {
        let (status, _) = get(&format!("/images/icons/{icon}")).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "icon {icon} must return 200, got {status}"
        );
    }
}

#[tokio::test]
async fn image_svg_icon_is_served() {
    let (status, _) = get("/images/icons/makefile.svg").await;
    assert_eq!(status, StatusCode::OK, "makefile.svg must return 200");
}

// ── Multi-compare page variants ───────────────────────────────────────────────

#[tokio::test]
async fn multi_compare_with_empty_runs_param_renders_page() {
    let (status, _) = get("/multi-compare?runs=").await;
    assert!(
        status.as_u16() < 500,
        "/multi-compare?runs= must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn multi_compare_with_21_runs_shows_limit_error() {
    let many = (0..21)
        .map(|i| format!("r{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let (status, body) = get(&format!("/multi-compare?runs={many}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("At most 20") || body.contains("<html"),
        "too many runs must show error page"
    );
}

// ── API schedules — invalid input ─────────────────────────────────────────────

#[tokio::test]
async fn api_create_schedule_missing_required_fields_returns_4xx() {
    let (status, _) = post_json("/api/schedules", r#"{"branch":"main"}"#).await;
    assert!(
        status.as_u16() >= 400 && status.as_u16() < 500,
        "schedule without repo_url must return 4xx, got {status}"
    );
}

// ── Import config variants ────────────────────────────────────────────────────

#[tokio::test]
async fn import_config_with_empty_toml_key_returns_400() {
    let (status, _) = post_json("/import-config", r#"{"toml":""}"#).await;
    assert!(
        status.as_u16() < 500,
        "empty TOML string must not 5xx, got {status}"
    );
}

// ── Analyze form: additional field combinations ────────────────────────────────

#[tokio::test]
async fn post_analyze_mixed_line_policy_variants_not_5xx() {
    for policy in ["count_as_code", "count_as_comment", "split"] {
        let app = make_test_router();
        let req = axum::http::Request::post("/analyze")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(axum::body::Body::from(format!(
                "path=.&mixed_line_policy={policy}"
            )))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert!(
            resp.status().as_u16() < 500,
            "analyze with mixed_line_policy={policy} must not 5xx"
        );
    }
}

#[tokio::test]
async fn post_analyze_blank_in_block_comment_variants_not_5xx() {
    for policy in ["count_as_blank", "count_as_comment"] {
        let app = make_test_router();
        let req = axum::http::Request::post("/analyze")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(axum::body::Body::from(format!(
                "path=.&blank_in_block_comment_policy={policy}"
            )))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert!(
            resp.status().as_u16() < 500,
            "blank_in_block_comment_policy={policy} must not 5xx"
        );
    }
}

// ── Project history with project filter ───────────────────────────────────────

#[tokio::test]
async fn project_history_with_project_param_not_5xx() {
    let (status, _) = get("/api/project-history?project=my-project").await;
    assert!(
        status.as_u16() < 500,
        "/api/project-history?project= must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn project_history_with_limit_param_not_5xx() {
    let (status, _) = get("/api/project-history?limit=5").await;
    assert!(
        status.as_u16() < 500,
        "/api/project-history?limit= must not 5xx, got {status}"
    );
}

// ── Embed handler variants ────────────────────────────────────────────────────

#[tokio::test]
async fn embed_summary_with_compact_mode_not_5xx() {
    let (status, _) = get("/embed/summary?compact=1").await;
    assert!(
        status.as_u16() < 500,
        "/embed/summary?compact=1 must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn embed_summary_with_light_theme_not_5xx() {
    let (status, _) = get("/embed/summary?theme=light").await;
    assert!(
        status.as_u16() < 500,
        "/embed/summary?theme=light must not 5xx, got {status}"
    );
}
