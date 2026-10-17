// SPDX-License-Identifier: AGPL-3.0-or-later
// Tests for the machine-readable interchange exporters (SARIF, Code Climate, Bitbucket, cloc).

use chrono::Utc;
use sloc_config::AppConfig;
use sloc_core::{
    AnalysisRun, EffectiveCounts, EnvironmentMetadata, FileRecord, FileStatus, LanguageSummary,
    SummaryTotals, ToolMetadata,
};
use sloc_languages::{Language, ParseMode, RawLineCounts};
use sloc_report::{
    CodeScanOptions, write_bitbucket, write_cloc_json, write_codeclimate, write_sarif,
};

fn make_record(path: &str, lang: Language, code: u64, complexity: Option<u32>) -> FileRecord {
    FileRecord {
        path: path.into(),
        relative_path: path.into(),
        language: Some(lang),
        size_bytes: code * 30,
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
        cyclomatic_complexity: complexity,
        lsloc: None,
        commit_count: None,
        last_commit_date: None,
        ownership: None,
        content_hash: 0,
    }
}

const fn make_lang(lang: Language, files: u64, code: u64) -> LanguageSummary {
    LanguageSummary {
        language: lang,
        files,
        total_physical_lines: code + 2,
        code_lines: code,
        comment_lines: 1,
        blank_lines: 1,
        mixed_lines_separate: 0,
        functions: 0,
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

/// Run with one clean file, one high-complexity file (triggers a finding), and one file with a
/// per-file warning.
fn make_run() -> AnalysisRun {
    let mut warned = make_record("src/warn.rs", Language::Rust, 5, Some(1));
    warned.warnings = vec!["decoded as Windows-1252".into()];
    let hot = make_record("src/hot.cpp", Language::Cpp, 400, Some(90));
    let clean = make_record("src/clean.rs", Language::Rust, 10, Some(2));

    AnalysisRun {
        tool: ToolMetadata {
            name: "sloc".into(),
            version: "1.0.0".into(),
            run_id: "test-run".into(),
            timestamp_utc: Utc::now(),
        },
        environment: EnvironmentMetadata {
            operating_system: "linux".into(),
            architecture: "x86_64".into(),
            runtime_mode: "cli".into(),
            initiator_username: "test".into(),
            initiator_hostname: "host".into(),
            ci_name: None,
        },
        effective_configuration: AppConfig::default(),
        input_roots: vec!["/tmp/test".into()],
        summary_totals: SummaryTotals {
            files_considered: 3,
            files_analyzed: 3,
            total_physical_lines: 430,
            code_lines: 415,
            comment_lines: 3,
            blank_lines: 3,
            test_count: 4,
            cyclomatic_complexity: 93,
            ..SummaryTotals::default()
        },
        totals_by_language: vec![
            make_lang(Language::Rust, 2, 15),
            make_lang(Language::Cpp, 1, 400),
        ],
        per_file_records: vec![warned, hot, clean],
        skipped_file_records: vec![],
        warnings: vec![],
        submodule_summaries: vec![],
        git_commit_short: None,
        git_branch: None,
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

#[test]
fn sarif_is_valid_and_reports_findings() {
    let run = make_run();
    let tmp = tempfile::NamedTempFile::new().unwrap();
    write_sarif(&run, tmp.path(), CodeScanOptions::default()).unwrap();
    let text = std::fs::read_to_string(tmp.path()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();

    assert_eq!(v["version"], "2.1.0");
    assert_eq!(v["runs"][0]["tool"]["driver"]["name"], "oxide-sloc");
    let results = v["runs"][0]["results"].as_array().unwrap();
    // One high-complexity finding (hot.cpp) + one file warning.
    assert_eq!(results.len(), 2);
    assert!(
        results
            .iter()
            .any(|r| r["ruleId"] == "oxide-sloc/high-complexity")
    );
    assert!(
        results
            .iter()
            .any(|r| r["ruleId"] == "oxide-sloc/file-warning")
    );
    // The 400-line C++ file at cc=90 is > 2x the default threshold (25) → error level.
    let hot = results
        .iter()
        .find(|r| r["ruleId"] == "oxide-sloc/high-complexity")
        .unwrap();
    assert_eq!(hot["level"], "error");
    assert_eq!(
        hot["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
        "src/hot.cpp"
    );
}

#[test]
fn sarif_threshold_suppresses_findings() {
    let run = make_run();
    let tmp = tempfile::NamedTempFile::new().unwrap();
    // Raise the ceiling above the hottest file so only the warning finding remains.
    write_sarif(
        &run,
        tmp.path(),
        CodeScanOptions {
            complexity_threshold: 1000,
        },
    )
    .unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(tmp.path()).unwrap()).unwrap();
    let results = v["runs"][0]["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["ruleId"], "oxide-sloc/file-warning");
}

#[test]
fn codeclimate_has_required_gitlab_fields() {
    let run = make_run();
    let tmp = tempfile::NamedTempFile::new().unwrap();
    write_codeclimate(&run, tmp.path(), CodeScanOptions::default()).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(tmp.path()).unwrap()).unwrap();
    let issues = v.as_array().unwrap();
    assert_eq!(issues.len(), 2);
    for issue in issues {
        // GitLab Code Quality requires these fields on every issue.
        assert!(issue["description"].is_string());
        assert!(issue["fingerprint"].is_string());
        assert!(issue["severity"].is_string());
        assert!(issue["location"]["path"].is_string());
        assert!(issue["location"]["lines"]["begin"].is_number());
    }
    // Fingerprints must be unique so GitLab can track issues across MRs.
    let fp0 = issues[0]["fingerprint"].as_str().unwrap();
    let fp1 = issues[1]["fingerprint"].as_str().unwrap();
    assert_ne!(fp0, fp1);
}

#[test]
fn bitbucket_report_and_annotations() {
    let run = make_run();
    let tmp = tempfile::NamedTempFile::new().unwrap();
    write_bitbucket(&run, tmp.path(), CodeScanOptions::default()).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(tmp.path()).unwrap()).unwrap();
    assert_eq!(v["report"]["reporter"], "oxide-sloc");
    // A warning/error finding present ⇒ overall result FAILED.
    assert_eq!(v["report"]["result"], "FAILED");
    assert!(
        v["report"]["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["title"] == "Code lines")
    );
    let anns = v["annotations"].as_array().unwrap();
    assert_eq!(anns.len(), 2);
    assert!(
        anns.iter()
            .all(|a| a["path"].is_string() && a["external_id"].is_string())
    );
}

#[test]
fn bitbucket_passes_when_only_notes() {
    let mut run = make_run();
    // Drop the hot file so only the note-level warning remains.
    run.per_file_records
        .retain(|r| r.relative_path != "src/hot.cpp");
    let tmp = tempfile::NamedTempFile::new().unwrap();
    write_bitbucket(&run, tmp.path(), CodeScanOptions::default()).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(tmp.path()).unwrap()).unwrap();
    assert_eq!(v["report"]["result"], "PASSED");
}

#[test]
fn cloc_json_matches_cloc_shape() {
    let run = make_run();
    let tmp = tempfile::NamedTempFile::new().unwrap();
    write_cloc_json(&run, tmp.path()).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(tmp.path()).unwrap()).unwrap();

    assert!(
        v["header"]["cloc_version"]
            .as_str()
            .unwrap()
            .contains("oxide-sloc")
    );
    assert_eq!(v["header"]["n_files"], 3);
    // Per-language objects keyed by display name, plus the SUM roll-up.
    assert_eq!(v["Rust"]["code"], 15);
    assert_eq!(v["Rust"]["nFiles"], 2);
    assert_eq!(v["SUM"]["code"], 415);
    assert_eq!(v["SUM"]["nFiles"], 3);
}
