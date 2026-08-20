// Tests for `sloc_web::bundle_run` — the shared entry point behind the CLI `bundle`
// command. Verifies the run directory + `result_<stem>.json` + `registry.json` are all
// created, and that the registry holds one entry whose `json_path` points at the written
// result file. This is the exact contract the local Compare / "Scan Delta" page relies on.

use chrono::Utc;
use sloc_config::AppConfig;
use sloc_core::{
    AnalysisRun, EnvironmentMetadata, ScanRegistry, SummaryTotals, ToolMetadata, read_json,
};

fn fixture_run(run_id: &str) -> AnalysisRun {
    AnalysisRun {
        tool: ToolMetadata {
            name: "oxide-sloc".into(),
            version: "test".into(),
            run_id: run_id.into(),
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
        input_roots: vec!["/tmp/myproject".into()],
        summary_totals: SummaryTotals {
            files_considered: 1,
            files_analyzed: 1,
            code_lines: 42,
            ..SummaryTotals::default()
        },
        totals_by_language: vec![],
        per_file_records: vec![],
        skipped_file_records: vec![],
        warnings: vec![],
        submodule_summaries: vec![],
        git_commit_short: Some("abc1234".into()),
        git_commit_long: Some("abc1234def5678".into()),
        git_branch: Some("main".into()),
        git_commit_author: Some("Tester".into()),
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

#[test]
fn bundle_run_writes_layout_and_registers_entry() {
    let out_root = tempfile::tempdir().expect("tempdir");
    let run = fixture_run("run-1234");

    let run_dir = sloc_web::bundle_run(&run, out_root.path(), "run-1234", None)
        .expect("bundle_run should succeed");

    // The run directory follows the web convention: <project_label>_<run_id>.
    assert!(
        run_dir.is_dir(),
        "run dir must exist: {}",
        run_dir.display()
    );
    assert_eq!(
        run_dir.file_name().and_then(|s| s.to_str()),
        Some("myproject_run-1234"),
        "run dir name must be <project_label>_<run_id>"
    );

    // result_<stem>.json — stem is <label>_<git_commit_short>.
    let result_json = run_dir.join("json").join("result_myproject_abc1234.json");
    assert!(
        result_json.is_file(),
        "result JSON must exist at {}",
        result_json.display()
    );
    // It must round-trip as a valid AnalysisRun.
    let reloaded = read_json(&result_json).expect("result JSON must be a valid AnalysisRun");
    assert_eq!(reloaded.tool.run_id, "run-1234");

    // HTML report present too.
    assert!(
        run_dir
            .join("html")
            .join("report_myproject_abc1234.html")
            .is_file(),
        "HTML report must exist"
    );

    // registry.json created at the output root with exactly one entry pointing at the JSON.
    let registry_path = out_root.path().join("registry.json");
    assert!(registry_path.is_file(), "registry.json must exist");
    let registry = ScanRegistry::load(&registry_path);
    assert_eq!(registry.entries.len(), 1, "registry must hold one entry");
    let entry = &registry.entries[0];
    assert_eq!(entry.run_id, "run-1234");
    assert_eq!(entry.project_label, "myproject");
    assert_eq!(
        entry.json_path.as_deref(),
        Some(result_json.as_path()),
        "registry json_path must point at the written result file"
    );
    assert_eq!(entry.git_commit.as_deref(), Some("abc1234"));
    assert_eq!(entry.git_branch.as_deref(), Some("main"));
}

#[test]
fn bundle_run_label_override_and_generated_run_id() {
    let out_root = tempfile::tempdir().expect("tempdir");
    // No git commit -> file stem is just the label; explicit label overrides derivation.
    let mut run = fixture_run("");
    run.git_commit_short = None;

    let run_dir = sloc_web::bundle_run(&run, out_root.path(), "explicit-id", Some("My Service"))
        .expect("bundle_run should succeed");

    assert_eq!(
        run_dir.file_name().and_then(|s| s.to_str()),
        Some("my-service_explicit-id"),
        "label is sanitized and joined with the run id"
    );
    let result_json = run_dir.join("json").join("result_my-service.json");
    assert!(
        result_json.is_file(),
        "result JSON stem must be the sanitized label when no commit is known"
    );

    let registry = ScanRegistry::load(&out_root.path().join("registry.json"));
    assert_eq!(registry.entries.len(), 1);
    assert_eq!(registry.entries[0].run_id, "explicit-id");
}
