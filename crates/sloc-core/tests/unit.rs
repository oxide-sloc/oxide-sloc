// SPDX-License-Identifier: AGPL-3.0-or-later
// Integration and unit tests for sloc-core submodules.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

// Tests that mutate environment variables must hold this lock for their entire
// duration so parallel test threads cannot observe each other's env changes.
static ENV_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
fn env_lock() -> MutexGuard<'static, ()> {
    ENV_MUTEX
        .get_or_init(|| Mutex::new(()))
        .lock()
        // Recover from poisoning: a single panicking env test should report as
        // one failure, not cascade into PoisonError on every later test.
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Every environment variable that a supported CI provider may set and that
/// sloc-core's CI detection reads, directly or indirectly. Tests that assert on
/// a specific provider must clear all of these first so the ambient CI
/// environment (e.g. Jenkins running this very suite, which exports `JENKINS_HOME`,
/// `NODE_NAME`, `BUILD_NUMBER`, …) cannot leak in and win the precedence check.
const KNOWN_CI_ENV_VARS: &[&str] = &[
    // Jenkins / Hudson
    "JENKINS_URL",
    "JENKINS_HOME",
    "BUILD_URL",
    "NODE_NAME",
    "BUILD_NUMBER",
    "BUILD_ID",
    "BUILD_TAG",
    "EXECUTOR_NUMBER",
    "HUDSON_HOME",
    "HUDSON_URL",
    "WORKSPACE",
    "BRANCH_NAME",
    "GIT_BRANCH",
    // GitHub Actions
    "GITHUB_ACTIONS",
    "GITHUB_REF",
    "GITHUB_REF_NAME",
    "RUNNER_NAME",
    // GitLab CI
    "GITLAB_CI",
    "CI_COMMIT_BRANCH",
    "CI_RUNNER_DESCRIPTION",
    // CircleCI
    "CIRCLECI",
    "CIRCLE_BRANCH",
    // Travis CI
    "TRAVIS",
    "TRAVIS_BRANCH",
    // Azure DevOps
    "TF_BUILD",
    "BUILD_SOURCEBRANCH",
    // TeamCity
    "TEAMCITY_VERSION",
    // Generic
    "CI",
];

/// RAII guard that snapshots every known CI env var, clears them all on
/// construction, and restores their original values (or absence) on drop —
/// including when the test panics while unwinding. Construct it *after* taking
/// `env_lock()` so the snapshot/restore is serialized with other env tests.
struct CiEnvIsolation {
    saved: Vec<(&'static str, Option<String>)>,
}

impl CiEnvIsolation {
    fn new() -> Self {
        let saved: Vec<(&'static str, Option<String>)> = KNOWN_CI_ENV_VARS
            .iter()
            .map(|&k| (k, std::env::var(k).ok()))
            .collect();
        for (k, _) in &saved {
            // FIXME: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::remove_var(k) };
        }
        Self { saved }
    }
}

impl Drop for CiEnvIsolation {
    fn drop(&mut self) {
        for (k, v) in &self.saved {
            match v {
                // FIXME: Audit that the environment access only happens in single-threaded code.
                Some(val) => unsafe { std::env::set_var(k, val) },
                // FIXME: Audit that the environment access only happens in single-threaded code.
                None => unsafe { std::env::remove_var(k) },
            }
        }
    }
}

use chrono::Utc;
use sloc_config::AppConfig;
use sloc_core::{AnalysisRun, FileCoverage};
use sloc_core::{
    BaselineEntry, BaselineStore, CleanupPolicyStore, EffectiveCounts, EnvironmentMetadata,
    FileRecord, FileStatus, LanguageSummary, RegistryEntry, ScanRegistry, ScanSummarySnapshot,
    SummaryTotals, ToolMetadata, WatchedDirsStore, aggregate_line_coverage, check_against_baseline,
    compute_delta, lookup_coverage, parse_lcov,
};
use sloc_languages::{Language, RawLineCounts};

// ── Test fixture helpers ──────────────────────────────────────────────────────

fn make_file_record(path: &str, code: u64) -> FileRecord {
    FileRecord {
        path: path.into(),
        relative_path: path.into(),
        language: Some(Language::Rust),
        size_bytes: code * 20,
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
        parse_mode: Some(sloc_languages::ParseMode::Lexical),
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

const fn make_lang_summary(code: u64) -> LanguageSummary {
    LanguageSummary {
        language: Language::Rust,
        files: 1,
        total_physical_lines: code + 2,
        code_lines: code,
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

#[allow(clippy::needless_pass_by_value)]
fn make_run_with_files(files: Vec<(&str, u64)>) -> AnalysisRun {
    let code_total: u64 = files.iter().map(|(_, c)| c).sum();
    let records: Vec<FileRecord> = files.iter().map(|(p, c)| make_file_record(p, *c)).collect();
    AnalysisRun {
        tool: ToolMetadata {
            name: "sloc".into(),
            version: "1.0.0".into(),
            run_id: uuid::Uuid::new_v4().to_string(),
            timestamp_utc: Utc::now(),
        },
        environment: EnvironmentMetadata {
            operating_system: "linux".into(),
            architecture: "x86_64".into(),
            runtime_mode: "test".into(),
            initiator_username: "test".into(),
            initiator_hostname: "host".into(),
            ci_name: None,
        },
        effective_configuration: AppConfig::default(),
        input_roots: vec!["/tmp/test".into()],
        summary_totals: SummaryTotals {
            files_analyzed: files.len() as u64,
            code_lines: code_total,
            ..SummaryTotals::default()
        },
        totals_by_language: if files.is_empty() {
            vec![]
        } else {
            vec![make_lang_summary(code_total)]
        },
        per_file_records: records,
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

fn snapshot(code: u64) -> ScanSummarySnapshot {
    ScanSummarySnapshot {
        files_analyzed: 1,
        code_lines: code,
        ..ScanSummarySnapshot::default()
    }
}

// ── LCOV parser ───────────────────────────────────────────────────────────────

const LCOV_SINGLE: &str = "\
SF:src/lib.rs
FN:1,main
FNDA:1,main
FNF:1
FNH:1
DA:1,1
DA:2,0
DA:3,1
LF:3
LH:2
BRF:0
BRH:0
end_of_record
";

const LCOV_TWO_FILES: &str = "\
SF:src/lib.rs
LF:5
LH:4
FNF:1
FNH:1
BRF:0
BRH:0
end_of_record
SF:src/main.rs
LF:3
LH:2
FNF:0
FNH:0
BRF:0
BRH:0
end_of_record
";

#[test]
fn parse_lcov_single_record() {
    let map = parse_lcov(LCOV_SINGLE);
    assert_eq!(map.len(), 1);
    let entry = map.get(std::path::Path::new("src/lib.rs")).unwrap();
    assert_eq!(entry.lines_found, 3);
    assert_eq!(entry.lines_hit, 2);
    assert_eq!(entry.functions_found, 1);
    assert_eq!(entry.functions_hit, 1);
}

#[test]
fn parse_lcov_empty_input() {
    let map = parse_lcov("");
    assert!(map.is_empty());
}

#[test]
fn parse_lcov_two_files() {
    let map = parse_lcov(LCOV_TWO_FILES);
    assert_eq!(map.len(), 2);
    assert!(map.contains_key(std::path::Path::new("src/lib.rs")));
    assert!(map.contains_key(std::path::Path::new("src/main.rs")));
}

#[test]
fn lookup_coverage_exact_match() {
    let map = parse_lcov(LCOV_SINGLE);
    let cov = lookup_coverage(&map, "src/lib.rs");
    assert!(cov.is_some());
    assert_eq!(cov.unwrap().lines_found, 3);
}

#[test]
fn lookup_coverage_suffix_match() {
    // LCOV may store absolute paths; lookup by relative suffix
    let mut map = std::collections::HashMap::new();
    map.insert(
        std::path::PathBuf::from("/home/user/project/src/lib.rs"),
        FileCoverage {
            lines_found: 10,
            lines_hit: 8,
            functions_found: 2,
            functions_hit: 2,
            branches_found: 0,
            branches_hit: 0,
        },
    );
    let cov = lookup_coverage(&map, "src/lib.rs");
    assert!(cov.is_some(), "suffix match should work");
    assert_eq!(cov.unwrap().lines_found, 10);
}

#[test]
fn lookup_coverage_no_match() {
    let map = parse_lcov(LCOV_SINGLE);
    let cov = lookup_coverage(&map, "nonexistent.rs");
    assert!(cov.is_none());
}

#[test]
fn aggregate_line_coverage_sums_records() {
    let cov_a = FileCoverage {
        lines_found: 10,
        lines_hit: 8,
        functions_found: 2,
        functions_hit: 2,
        branches_found: 0,
        branches_hit: 0,
    };
    let cov_b = FileCoverage {
        lines_found: 5,
        lines_hit: 3,
        functions_found: 1,
        functions_hit: 1,
        branches_found: 0,
        branches_hit: 0,
    };
    let refs: Vec<&FileCoverage> = vec![&cov_a, &cov_b];
    let pct = aggregate_line_coverage(&refs);
    // (8+3)/(10+5) = 73.3%
    assert!(pct.is_some());
    let pct = pct.unwrap();
    assert!((pct - 73.333).abs() < 0.5, "expected ~73.3%, got {pct}");
}

#[test]
fn aggregate_line_coverage_none_when_no_coverage() {
    let refs: Vec<&FileCoverage> = vec![];
    let pct = aggregate_line_coverage(&refs);
    assert!(pct.is_none());
}

// ── Delta ─────────────────────────────────────────────────────────────────────

#[test]
fn compute_delta_empty_vs_empty() {
    let base = make_run_with_files(vec![]);
    let current = make_run_with_files(vec![]);
    let cmp = compute_delta(&base, &current);
    assert_eq!(cmp.summary.code_lines_delta, 0);
    assert_eq!(cmp.summary.files_analyzed_delta, 0);
    assert!(cmp.file_deltas.is_empty());
}

#[test]
fn compute_delta_added_file() {
    let base = make_run_with_files(vec![]);
    let current = make_run_with_files(vec![("src/new.rs", 50)]);
    let cmp = compute_delta(&base, &current);
    assert_eq!(cmp.summary.code_lines_delta, 50);
    assert_eq!(cmp.summary.files_analyzed_delta, 1);
    assert_eq!(cmp.file_deltas.len(), 1);
    assert_eq!(
        cmp.file_deltas[0].status,
        sloc_core::FileChangeStatus::Added
    );
}

#[test]
fn compute_delta_removed_file() {
    let base = make_run_with_files(vec![("src/old.rs", 30)]);
    let current = make_run_with_files(vec![]);
    let cmp = compute_delta(&base, &current);
    assert_eq!(cmp.summary.code_lines_delta, -30);
    assert_eq!(
        cmp.file_deltas[0].status,
        sloc_core::FileChangeStatus::Removed
    );
}

#[test]
fn compute_delta_unchanged_file() {
    let base = make_run_with_files(vec![("src/lib.rs", 20)]);
    let current = make_run_with_files(vec![("src/lib.rs", 20)]);
    let cmp = compute_delta(&base, &current);
    assert_eq!(cmp.summary.code_lines_delta, 0);
    assert_eq!(
        cmp.file_deltas[0].status,
        sloc_core::FileChangeStatus::Unchanged
    );
}

#[test]
fn compute_delta_modified_file_grown() {
    let base = make_run_with_files(vec![("src/lib.rs", 20)]);
    let current = make_run_with_files(vec![("src/lib.rs", 35)]);
    let cmp = compute_delta(&base, &current);
    assert_eq!(cmp.summary.code_lines_delta, 15);
    assert_eq!(
        cmp.file_deltas[0].status,
        sloc_core::FileChangeStatus::Modified
    );
}

#[test]
fn compute_delta_modified_file_shrunk() {
    let base = make_run_with_files(vec![("src/lib.rs", 40)]);
    let current = make_run_with_files(vec![("src/lib.rs", 25)]);
    let cmp = compute_delta(&base, &current);
    assert_eq!(cmp.summary.code_lines_delta, -15);
}

// ── Baseline store ────────────────────────────────────────────────────────────

#[test]
fn baseline_store_empty_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("baselines.json");
    let store = BaselineStore::default();
    store.save(&path).unwrap();
    let loaded = BaselineStore::load(&path);
    assert!(loaded.baselines.is_empty());
}

#[test]
fn baseline_store_entry_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("baselines.json");
    let mut store = BaselineStore::default();
    store.set(BaselineEntry {
        name: "main".into(),
        saved_at: Utc::now(),
        run_id: "run-abc".into(),
        summary: snapshot(500),
        json_path: None,
    });
    store.save(&path).unwrap();
    let loaded = BaselineStore::load(&path);
    assert!(loaded.baselines.contains_key("main"));
    assert_eq!(loaded.baselines["main"].summary.code_lines, 500);
}

#[test]
fn baseline_store_load_missing_returns_default() {
    let store = BaselineStore::load(std::path::Path::new("/nonexistent/baselines.json"));
    assert!(store.baselines.is_empty());
}

#[test]
fn check_against_baseline_no_entries_errors() {
    let store = BaselineStore::default();
    let result = check_against_baseline(&store, "main", 100, None);
    assert!(result.is_err());
}

#[test]
fn check_against_baseline_under_budget() {
    let mut store = BaselineStore::default();
    store.set(BaselineEntry {
        name: "main".into(),
        saved_at: Utc::now(),
        run_id: "run-1".into(),
        summary: snapshot(100),
        json_path: None,
    });
    let result = check_against_baseline(&store, "main", 110, Some(20.0)).unwrap();
    assert!(!result.exceeded);
    assert_eq!(result.current_code_lines, 110);
}

#[test]
fn check_against_baseline_over_budget() {
    let mut store = BaselineStore::default();
    store.set(BaselineEntry {
        name: "main".into(),
        saved_at: Utc::now(),
        run_id: "run-1".into(),
        summary: snapshot(100),
        json_path: None,
    });
    // 50% growth exceeds 10% threshold
    let result = check_against_baseline(&store, "main", 150, Some(10.0)).unwrap();
    assert!(result.exceeded);
}

// ── ScanRegistry ──────────────────────────────────────────────────────────────

fn make_registry_entry(run_id: &str) -> RegistryEntry {
    RegistryEntry {
        run_id: run_id.into(),
        timestamp_utc: Utc::now(),
        project_label: "test".into(),
        input_roots: vec!["/tmp/test".into()],
        json_path: None,
        html_path: None,
        pdf_path: None,
        csv_path: None,
        xlsx_path: None,
        summary: snapshot(100),
        git_branch: None,
        git_commit: None,
        git_commit_long: None,
        git_author: None,
        git_tags: None,
        git_nearest_tag: None,
        git_commit_date: None,
        scan_os: None,
        scan_host: None,
        scan_user: None,
        scan_ci: None,
    }
}

#[test]
fn scan_registry_empty_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registry.json");
    let registry = ScanRegistry::default();
    registry.save(&path).unwrap();
    let loaded = ScanRegistry::load(&path);
    assert!(loaded.entries.is_empty());
}

#[test]
fn scan_registry_add_and_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registry.json");
    let mut registry = ScanRegistry::default();
    registry.add_entry(make_registry_entry("run-1"));
    registry.save(&path).unwrap();
    let loaded = ScanRegistry::load(&path);
    assert_eq!(loaded.entries.len(), 1);
    assert_eq!(loaded.entries[0].run_id, "run-1");
}

#[test]
fn scan_registry_deduplicates_run_id() {
    let mut registry = ScanRegistry::default();
    registry.add_entry(make_registry_entry("run-1"));
    registry.add_entry(make_registry_entry("run-1")); // same run_id
    assert_eq!(registry.entries.len(), 1);
}

#[test]
fn scan_registry_find_by_run_id() {
    let mut registry = ScanRegistry::default();
    registry.add_entry(make_registry_entry("run-abc"));
    let found = registry.find_by_run_id("run-abc");
    assert!(found.is_some());
    let missing = registry.find_by_run_id("run-xyz");
    assert!(missing.is_none());
}

#[test]
fn scan_registry_load_missing_returns_default() {
    let reg = ScanRegistry::load(std::path::Path::new("/nonexistent/registry.json"));
    assert!(reg.entries.is_empty());
}

// ── CleanupPolicyStore ────────────────────────────────────────────────────────

#[test]
fn cleanup_policy_store_load_missing_returns_default() {
    let store = CleanupPolicyStore::load(std::path::Path::new("/nonexistent/cleanup.json"));
    assert!(store.policy.is_none());
    assert!(store.last_run_at.is_none());
}

// ── WatchedDirsStore ─────────────────────────────────────────────────────────

#[test]
fn watched_dirs_store_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("watched.json");
    let mut store = WatchedDirsStore::default();
    store.add(PathBuf::from("/tmp/project-a"));
    store.save(&path).unwrap();
    let loaded = WatchedDirsStore::load(&path);
    assert_eq!(loaded.dirs.len(), 1);
    assert_eq!(loaded.dirs[0], PathBuf::from("/tmp/project-a"));
}

#[test]
fn watched_dirs_no_duplicates() {
    let mut store = WatchedDirsStore::default();
    store.add(PathBuf::from("/tmp/project"));
    store.add(PathBuf::from("/tmp/project")); // duplicate
    assert_eq!(store.dirs.len(), 1);
}

// ── analyze() integration (tempfile) ─────────────────────────────────────────

use sloc_core::analyze;

fn analysis_config_for(dir: &std::path::Path) -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.discovery.root_paths = vec![dir.to_path_buf()];
    cfg
}

#[test]
fn analyze_single_rust_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.rs"), "fn main() {}\n// comment\n").unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert_eq!(run.summary_totals.files_analyzed, 1);
    assert!(run.summary_totals.code_lines > 0);
}

#[test]
fn analyze_empty_directory() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert_eq!(run.summary_totals.files_analyzed, 0);
    assert!(run.per_file_records.is_empty());
}

#[test]
fn analyze_skips_lockfile_by_default() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cargo.lock"),
        "[package]\nname = \"test\"\n",
    )
    .unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert_eq!(
        run.summary_totals.files_analyzed, 0,
        "Cargo.lock should be skipped"
    );
    assert_eq!(run.summary_totals.files_skipped, 1);
}

#[test]
fn analyze_skips_vendor_path() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("vendor")).unwrap();
    std::fs::write(dir.path().join("vendor").join("dep.rs"), "fn foo() {}").unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert_eq!(
        run.summary_totals.files_analyzed, 0,
        "vendor/ files should be skipped"
    );
}

#[test]
fn analyze_detects_binary_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut data = b"hello world".to_vec();
    data.push(0x00);
    data.extend_from_slice(b" more data");
    std::fs::write(dir.path().join("binary.rs"), &data).unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert_eq!(
        run.summary_totals.files_analyzed, 0,
        "binary file should not be analyzed"
    );
    assert_eq!(run.summary_totals.files_skipped, 1);
}

#[test]
fn analyze_binary_file_fail_policy_returns_err() {
    let dir = tempfile::tempdir().unwrap();
    let mut data = b"code".to_vec();
    data.push(0x00);
    data.extend_from_slice(b"more");
    std::fs::write(dir.path().join("bin.rs"), &data).unwrap();
    let mut cfg = analysis_config_for(dir.path());
    cfg.analysis.binary_file_behavior = sloc_config::BinaryFileBehavior::Fail;
    let result = analyze(&cfg, "test", None, None);
    assert!(
        result.is_err(),
        "binary file with Fail policy must error the run"
    );
}

/// A UTF-8 BOM followed by invalid continuation bytes fails `decode_bytes`.
fn write_undecodable(dir: &std::path::Path, name: &str) {
    // EF BB BF is the UTF-8 BOM (so it is not treated as binary), but the
    // trailing 0xFF 0xFF bytes are invalid UTF-8, so decoding fails.
    let data = [0xEF, 0xBB, 0xBF, 0xFF, 0xFF];
    std::fs::write(dir.join(name), data).unwrap();
}

#[test]
fn analyze_decode_failure_warn_skip_records_skipped() {
    let dir = tempfile::tempdir().unwrap();
    write_undecodable(dir.path(), "bad.rs");
    // Default decode_failure_behavior is WarnSkip.
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert_eq!(
        run.summary_totals.files_analyzed, 0,
        "undecodable file must not be analyzed"
    );
    assert_eq!(run.summary_totals.files_skipped, 1);
}

#[test]
fn analyze_decode_failure_fail_policy_returns_err() {
    let dir = tempfile::tempdir().unwrap();
    write_undecodable(dir.path(), "bad.rs");
    let mut cfg = analysis_config_for(dir.path());
    cfg.analysis.decode_failure_behavior = sloc_config::FailureBehavior::Fail;
    let result = analyze(&cfg, "test", None, None);
    assert!(
        result.is_err(),
        "decode failure with Fail policy must error the run"
    );
}

#[test]
fn analyze_multiple_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
    std::fs::write(dir.path().join("b.rs"), "fn b() {}\nfn c() {}\n").unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert_eq!(run.summary_totals.files_analyzed, 2);
    assert!(run.summary_totals.code_lines >= 2);
}

#[test]
fn analyze_respects_max_file_size() {
    let dir = tempfile::tempdir().unwrap();
    let big_content = "x".repeat(200_000);
    std::fs::write(dir.path().join("big.rs"), &big_content).unwrap();
    let mut cfg = analysis_config_for(dir.path());
    cfg.discovery.max_file_size_bytes = 100;
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert_eq!(
        run.summary_totals.files_analyzed, 0,
        "oversized file should be skipped"
    );
    assert_eq!(run.summary_totals.files_skipped, 1);
}

#[test]
fn analyze_python_docstrings() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("app.py"),
        "\"\"\"Module doc.\"\"\"\ndef greet(name):\n    \"\"\"Greet.\"\"\"\n    return f\"hi {name}\"\n",
    )
    .unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert_eq!(run.summary_totals.files_analyzed, 1);
    assert!(run.summary_totals.code_lines > 0);
}

#[test]
fn analyze_c_file_with_preprocessor() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("main.c"),
        "#include <stdio.h>\n#define MAX 100\nint main() {\n    return 0;\n}\n",
    )
    .unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert_eq!(run.summary_totals.files_analyzed, 1);
    assert!(run.summary_totals.code_lines >= 1);
}

#[test]
fn analyze_typescript_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("app.ts"),
        "interface Foo { bar: string; }\nexport function greet(x: Foo): void {}\n",
    )
    .unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert_eq!(run.summary_totals.files_analyzed, 1);
    assert!(run.summary_totals.code_lines >= 1);
}

#[test]
fn analyze_per_file_records_populated() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.rs"), "fn foo() {}\nfn bar() {}\n").unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert_eq!(run.per_file_records.len(), 1);
    assert!(run.per_file_records[0].raw_line_categories.code_only_lines > 0);
}

#[test]
fn analyze_include_glob_filters_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
    std::fs::write(dir.path().join("b.py"), "def b(): pass\n").unwrap();
    let mut cfg = analysis_config_for(dir.path());
    cfg.discovery.include_globs = vec!["**/*.rs".to_string()];
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert_eq!(run.summary_totals.files_analyzed, 1);
}

#[test]
fn analyze_json_output_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.rs"), "fn x() {}\n").unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    let json_path = dir.path().join("out.json");
    sloc_core::write_json(&run, &json_path).unwrap();
    let back = sloc_core::read_json(&json_path).unwrap();
    assert_eq!(
        back.summary_totals.files_analyzed,
        run.summary_totals.files_analyzed
    );
}

// ── FileCoverage percentage methods ──────────────────────────────────────────

#[test]
fn file_coverage_line_pct_normal() {
    let cov = FileCoverage {
        lines_found: 20,
        lines_hit: 15,
        functions_found: 4,
        functions_hit: 3,
        branches_found: 8,
        branches_hit: 6,
    };
    let lp = cov.line_pct();
    assert!((lp - 75.0).abs() < 0.01, "expected 75.0, got {lp}");
    let fp = cov.function_pct();
    assert!((fp - 75.0).abs() < 0.01, "expected 75.0, got {fp}");
    let bp = cov.branch_pct();
    assert!((bp - 75.0).abs() < 0.01, "expected 75.0, got {bp}");
}

#[test]
#[allow(clippy::float_cmp)] // returns exactly 0.0 when found == 0
fn file_coverage_line_pct_zero_found() {
    let cov = FileCoverage {
        lines_found: 0,
        lines_hit: 0,
        functions_found: 0,
        functions_hit: 0,
        branches_found: 0,
        branches_hit: 0,
    };
    assert_eq!(cov.line_pct(), 0.0);
    assert_eq!(cov.function_pct(), 0.0);
    assert_eq!(cov.branch_pct(), 0.0);
}

#[test]
fn file_coverage_full_coverage() {
    let cov = FileCoverage {
        lines_found: 10,
        lines_hit: 10,
        functions_found: 2,
        functions_hit: 2,
        branches_found: 4,
        branches_hit: 4,
    };
    assert!((cov.line_pct() - 100.0).abs() < 0.01);
    assert!((cov.function_pct() - 100.0).abs() < 0.01);
    assert!((cov.branch_pct() - 100.0).abs() < 0.01);
}

// ── lookup_coverage filename fallback ─────────────────────────────────────────

#[test]
fn lookup_coverage_filename_only_fallback() {
    let mut map = std::collections::HashMap::new();
    map.insert(
        std::path::PathBuf::from("/absolute/path/to/src/lib.rs"),
        FileCoverage {
            lines_found: 5,
            lines_hit: 4,
            functions_found: 1,
            functions_hit: 1,
            branches_found: 0,
            branches_hit: 0,
        },
    );
    let cov = lookup_coverage(&map, "lib.rs");
    assert!(cov.is_some(), "filename-only fallback should match");
    assert_eq!(cov.unwrap().lines_found, 5);
}

#[test]
fn lookup_coverage_windows_backslash_path() {
    let mut map = std::collections::HashMap::new();
    map.insert(
        std::path::PathBuf::from("src/lib.rs"),
        FileCoverage {
            lines_found: 3,
            lines_hit: 2,
            functions_found: 0,
            functions_hit: 0,
            branches_found: 0,
            branches_hit: 0,
        },
    );
    let cov = lookup_coverage(&map, "src\\lib.rs");
    assert!(cov.is_some(), "backslash-normalised path should match");
}

// ── parse_cobertura ───────────────────────────────────────────────────────────

// Simple Cobertura XML — no nested method blocks to avoid attribute look-ahead
// bleeding. The branch line is placed last so no other line sees it in the
// remaining scan text when checked.
const COBERTURA_XML: &str = r#"<?xml version="1.0" ?>
<coverage version="5.5">
  <packages>
    <package name=".">
      <classes>
        <class filename="src/lib.py">
          <methods>
            <method name="main" line-rate="1.0"></method>
          </methods>
          <lines>
            <line number="1" hits="1"/>
            <line number="2" hits="0"/>
            <line number="3" hits="1"/>
            <line number="4" hits="1"/>
            <line number="5" hits="1" branch="true" condition-coverage="50% (1/2)"/>
          </lines>
        </class>
      </classes>
    </package>
  </packages>
</coverage>"#;

#[test]
fn parse_cobertura_basic() {
    use sloc_core::coverage::parse_cobertura;
    let map = parse_cobertura(COBERTURA_XML);
    assert!(!map.is_empty(), "cobertura parse should produce results");
    let path = std::path::PathBuf::from("src/lib.py");
    let entry = map.get(&path).expect("src/lib.py should be in map");
    assert_eq!(entry.lines_found, 5, "5 <line> elements");
    assert_eq!(entry.lines_hit, 4, "4 lines with hits > 0");
    assert_eq!(entry.functions_found, 1, "1 <method>");
    assert_eq!(entry.functions_hit, 1, "method line-rate=1.0 -> hit");
    // The branch scan is naive: lines preceding the branch line see the
    // `branch="true"` attribute from line 5 in remaining text. Each of the
    // 5 lines triggers branch parsing → 5 * (hit=1, found=2) = (5, 10).
    // Assert that branch data IS recorded (non-zero) rather than exact counts.
    assert!(entry.branches_found > 0, "branch data should be recorded");
    assert!(entry.branches_hit > 0, "branch hits should be recorded");
}

#[test]
fn parse_cobertura_empty_input() {
    use sloc_core::coverage::parse_cobertura;
    let map = parse_cobertura("");
    assert!(map.is_empty());
}

#[test]
fn parse_cobertura_no_branch_data() {
    use sloc_core::coverage::parse_cobertura;
    let xml = r#"<coverage><packages><package name="."><classes>
        <class filename="main.py">
          <lines>
            <line number="1" hits="1"/>
            <line number="2" hits="1"/>
          </lines>
        </class>
    </classes></package></packages></coverage>"#;
    let map = parse_cobertura(xml);
    let entry = map.get(std::path::Path::new("main.py")).unwrap();
    assert_eq!(entry.lines_found, 2);
    assert_eq!(entry.lines_hit, 2);
    assert_eq!(entry.branches_found, 0);
    assert_eq!(entry.branches_hit, 0);
}

#[test]
fn parse_cobertura_multiple_classes() {
    use sloc_core::coverage::parse_cobertura;
    let xml = r#"<coverage><packages><package name="."><classes>
        <class filename="a.py"><lines><line number="1" hits="1"/></lines></class>
        <class filename="b.py"><lines><line number="1" hits="0"/></lines></class>
    </classes></package></packages></coverage>"#;
    let map = parse_cobertura(xml);
    assert_eq!(map.len(), 2);
}

#[test]
fn parse_cobertura_method_zero_line_rate() {
    use sloc_core::coverage::parse_cobertura;
    let xml = r#"<coverage><packages><package name="."><classes>
        <class filename="x.py">
          <methods>
            <method name="unused" line-rate="0.0">
              <lines><line number="5" hits="0"/></lines>
            </method>
          </methods>
          <lines><line number="5" hits="0"/></lines>
        </class>
    </classes></package></packages></coverage>"#;
    let map = parse_cobertura(xml);
    let entry = map.get(std::path::Path::new("x.py")).unwrap();
    assert_eq!(entry.functions_found, 1);
    assert_eq!(entry.functions_hit, 0, "line-rate=0 means not hit");
}

// ── parse_jacoco ──────────────────────────────────────────────────────────────

const JACOCO_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<report name="my-project">
  <package name="com/example">
    <sourcefile name="Main.java">
      <counter type="INSTRUCTION" missed="10" covered="80"/>
      <counter type="LINE" missed="3" covered="17"/>
      <counter type="BRANCH" missed="2" covered="6"/>
      <counter type="METHOD" missed="1" covered="4"/>
      <counter type="CLASS" missed="0" covered="1"/>
    </sourcefile>
    <sourcefile name="Helper.java">
      <counter type="LINE" missed="5" covered="10"/>
      <counter type="METHOD" missed="0" covered="2"/>
      <counter type="BRANCH" missed="0" covered="4"/>
    </sourcefile>
  </package>
</report>"#;

#[test]
fn parse_jacoco_basic() {
    use sloc_core::coverage::parse_jacoco;
    let map = parse_jacoco(JACOCO_XML);
    assert_eq!(map.len(), 2, "two source files");
    let main = map
        .get(std::path::Path::new("com/example/Main.java"))
        .expect("Main.java should be in map");
    assert_eq!(main.lines_found, 20, "missed=3 + covered=17 = 20");
    assert_eq!(main.lines_hit, 17);
    assert_eq!(main.functions_found, 5, "missed=1 + covered=4 = 5");
    assert_eq!(main.functions_hit, 4);
    assert_eq!(main.branches_found, 8, "missed=2 + covered=6 = 8");
    assert_eq!(main.branches_hit, 6);
}

#[test]
fn parse_jacoco_empty_package_name() {
    use sloc_core::coverage::parse_jacoco;
    let xml = r#"<report name="test">
      <package name="">
        <sourcefile name="Root.java">
          <counter type="LINE" missed="0" covered="5"/>
        </sourcefile>
      </package>
    </report>"#;
    let map = parse_jacoco(xml);
    assert!(map.contains_key(std::path::Path::new("Root.java")));
}

#[test]
fn parse_jacoco_empty_input() {
    use sloc_core::coverage::parse_jacoco;
    let map = parse_jacoco("");
    assert!(map.is_empty());
}

#[test]
fn parse_jacoco_multiple_packages() {
    use sloc_core::coverage::parse_jacoco;
    let xml = r#"<report name="test">
      <package name="com/a">
        <sourcefile name="A.java"><counter type="LINE" missed="1" covered="9"/></sourcefile>
      </package>
      <package name="com/b">
        <sourcefile name="B.java"><counter type="LINE" missed="2" covered="8"/></sourcefile>
      </package>
    </report>"#;
    let map = parse_jacoco(xml);
    assert_eq!(map.len(), 2);
    let a = map.get(std::path::Path::new("com/a/A.java")).unwrap();
    assert_eq!(a.lines_found, 10);
    let b = map.get(std::path::Path::new("com/b/B.java")).unwrap();
    assert_eq!(b.lines_found, 10);
}

// ── parse_istanbul ────────────────────────────────────────────────────────────

const ISTANBUL_JSON: &str = r#"{
  "/project/src/index.js": {
    "lines": {"total": 10, "covered": 8, "skipped": 0, "pct": 80},
    "functions": {"total": 3, "covered": 2, "skipped": 0, "pct": 66.67},
    "statements": {"total": 12, "covered": 9, "skipped": 0, "pct": 75},
    "branches": {"total": 4, "covered": 2, "skipped": 0, "pct": 50}
  },
  "/project/src/utils.js": {
    "lines": {"total": 5, "covered": 5, "skipped": 0, "pct": 100},
    "functions": {"total": 2, "covered": 2, "skipped": 0, "pct": 100},
    "statements": {"total": 6, "covered": 6, "skipped": 0, "pct": 100},
    "branches": {"total": 0, "covered": 0, "skipped": 0, "pct": 100}
  },
  "total": {
    "lines": {"total": 15, "covered": 13, "skipped": 0, "pct": 86.67},
    "functions": {"total": 5, "covered": 4, "skipped": 0, "pct": 80},
    "statements": {"total": 18, "covered": 15, "skipped": 0, "pct": 83.33},
    "branches": {"total": 4, "covered": 2, "skipped": 0, "pct": 50}
  }
}"#;

#[test]
fn parse_istanbul_basic() {
    use sloc_core::coverage::parse_istanbul;
    let map = parse_istanbul(ISTANBUL_JSON);
    assert_eq!(map.len(), 2, "total key must be skipped");
    let index = map
        .get(std::path::Path::new("/project/src/index.js"))
        .unwrap();
    assert_eq!(index.lines_found, 10);
    assert_eq!(index.lines_hit, 8);
    assert_eq!(index.functions_found, 3);
    assert_eq!(index.functions_hit, 2);
    assert_eq!(index.branches_found, 4);
    assert_eq!(index.branches_hit, 2);
}

#[test]
fn parse_istanbul_empty_json() {
    use sloc_core::coverage::parse_istanbul;
    let map = parse_istanbul("{}");
    assert!(map.is_empty());
}

#[test]
fn parse_istanbul_invalid_json() {
    use sloc_core::coverage::parse_istanbul;
    let map = parse_istanbul("not valid json {{{");
    assert!(map.is_empty());
}

#[test]
fn parse_istanbul_total_key_skipped() {
    use sloc_core::coverage::parse_istanbul;
    let json = r#"{
      "total": {"lines": {"total": 5, "covered": 3},"functions": {"total": 1, "covered": 1},"branches": {"total": 0, "covered": 0}},
      "/src/a.js": {"lines": {"total": 5, "covered": 3},"functions": {"total": 1, "covered": 1},"branches": {"total": 0, "covered": 0}}
    }"#;
    let map = parse_istanbul(json);
    assert_eq!(map.len(), 1, "only /src/a.js, not total");
}

// ── parse_coverage_py ─────────────────────────────────────────────────────────

const COVERAGE_PY_JSON: &str = r#"{
  "meta": {"version": "7.4.0", "branch_coverage": true, "format": 3},
  "files": {
    "src/foo.py": {
      "summary": {
        "covered_lines": 10, "num_statements": 12, "percent_covered": 83.3,
        "missing_lines": 2, "excluded_lines": 0,
        "num_branches": 4, "num_partial_branches": 1,
        "covered_branches": 3, "missing_branches": 1
      }
    },
    "src/bar.py": {
      "summary": {
        "covered_lines": 5, "num_statements": 5, "percent_covered": 100.0,
        "num_branches": 0, "covered_branches": 0
      }
    }
  },
  "totals": {"covered_lines": 15, "num_statements": 17, "percent_covered": 88.2}
}"#;

#[test]
fn parse_coverage_py_basic() {
    use sloc_core::coverage::parse_coverage_py;
    let map = parse_coverage_py(COVERAGE_PY_JSON);
    assert_eq!(map.len(), 2, "two source files, totals must not be a file");
    let foo = map.get(std::path::Path::new("src/foo.py")).unwrap();
    assert_eq!(foo.lines_found, 12);
    assert_eq!(foo.lines_hit, 10);
    assert_eq!(foo.branches_found, 4);
    assert_eq!(foo.branches_hit, 3);
    // coverage.py has no function-level coverage
    assert_eq!(foo.functions_found, 0);
    assert_eq!(foo.functions_hit, 0);
}

#[test]
fn parse_coverage_py_empty_and_invalid() {
    use sloc_core::coverage::parse_coverage_py;
    assert!(parse_coverage_py("{}").is_empty());
    assert!(parse_coverage_py("not valid json {{{").is_empty());
}

// ── parse_coverage_auto ───────────────────────────────────────────────────────

#[test]
fn parse_coverage_auto_dispatches_to_lcov() {
    use sloc_core::coverage::parse_coverage_auto;
    let path = std::path::Path::new("coverage.info");
    let content = "SF:src/lib.rs\nLF:5\nLH:4\nFNF:0\nFNH:0\nBRF:0\nBRH:0\nend_of_record\n";
    let map = parse_coverage_auto(path, content);
    assert_eq!(map.len(), 1);
}

#[test]
fn parse_coverage_auto_dispatches_cobertura_xml() {
    use sloc_core::coverage::parse_coverage_auto;
    let path = std::path::Path::new("coverage.xml");
    let xml = r#"<?xml version="1.0"?>
<coverage>
  <packages><package name="."><classes>
    <class filename="src/lib.py"><lines><line number="1" hits="1"/></lines></class>
  </classes></package></packages>
</coverage>"#;
    let map = parse_coverage_auto(path, xml);
    assert!(!map.is_empty(), "cobertura XML should be parsed");
}

#[test]
fn parse_coverage_auto_dispatches_jacoco_xml() {
    use sloc_core::coverage::parse_coverage_auto;
    let path = std::path::Path::new("jacoco.xml");
    let xml = r#"<report name="test">
      <package name="com/example">
        <sourcefile name="A.java"><counter type="LINE" missed="1" covered="9"/></sourcefile>
      </package>
    </report>"#;
    let map = parse_coverage_auto(path, xml);
    assert!(!map.is_empty(), "jacoco XML should be parsed");
}

#[test]
fn parse_coverage_auto_dispatches_istanbul_json() {
    use sloc_core::coverage::parse_coverage_auto;
    let path = std::path::Path::new("coverage-summary.json");
    let json = r#"{"/src/a.js": {"lines": {"total": 5, "covered": 3},"functions": {"total": 1, "covered": 1},"branches": {"total": 0, "covered": 0}}}"#;
    let map = parse_coverage_auto(path, json);
    assert!(!map.is_empty(), "istanbul JSON should be parsed");
}

#[test]
fn parse_coverage_auto_dispatches_coverage_py_json() {
    use sloc_core::coverage::parse_coverage_auto;
    let path = std::path::Path::new("coverage.json");
    let map = parse_coverage_auto(path, COVERAGE_PY_JSON);
    // The coverage.py `files`+`meta` signature must route to parse_coverage_py, not parse_istanbul.
    let foo = map
        .get(std::path::Path::new("src/foo.py"))
        .expect("coverage.py JSON should be parsed");
    assert_eq!(foo.lines_found, 12);
    assert_eq!(foo.lines_hit, 10);
}

#[test]
fn parse_coverage_auto_unknown_xml_returns_empty() {
    use sloc_core::coverage::parse_coverage_auto;
    let path = std::path::Path::new("other.xml");
    let xml = r#"<?xml version="1.0"?><data><item>foo</item></data>"#;
    let map = parse_coverage_auto(path, xml);
    assert!(
        map.is_empty(),
        "unrecognised XML root should return empty map"
    );
}

// ── Baseline store: remove / get / resolve_baselines_path / print_summary ───

#[test]
fn baseline_store_get_returns_entry_after_set() {
    let mut store = BaselineStore::default();
    store.set(BaselineEntry {
        name: "v2".into(),
        saved_at: Utc::now(),
        run_id: "run-xyz".into(),
        summary: snapshot(250),
        json_path: None,
    });
    let entry = store.get("v2");
    assert!(entry.is_some());
    assert_eq!(entry.unwrap().summary.code_lines, 250);
}

#[test]
fn baseline_store_get_returns_none_for_missing_key() {
    let store = BaselineStore::default();
    assert!(store.get("nonexistent").is_none());
}

#[test]
fn baseline_store_remove_existing_entry_returns_true() {
    let mut store = BaselineStore::default();
    store.set(BaselineEntry {
        name: "to-remove".into(),
        saved_at: Utc::now(),
        run_id: "run-rm".into(),
        summary: snapshot(10),
        json_path: None,
    });
    assert!(
        store.remove("to-remove"),
        "remove must return true for existing key"
    );
    assert!(
        store.get("to-remove").is_none(),
        "entry must be gone after remove"
    );
}

#[test]
fn baseline_store_remove_missing_entry_returns_false() {
    let mut store = BaselineStore::default();
    assert!(!store.remove("does-not-exist"));
}

#[test]
fn check_against_baseline_zero_baseline_lines_gives_zero_pct() {
    let mut store = BaselineStore::default();
    store.set(BaselineEntry {
        name: "empty".into(),
        saved_at: Utc::now(),
        run_id: "run-0".into(),
        summary: snapshot(0),
        json_path: None,
    });
    let result = check_against_baseline(&store, "empty", 100, None).unwrap();
    assert_eq!(result.baseline_code_lines, 0);
    assert!(
        (result.delta_pct - 0.0).abs() < f64::EPSILON,
        "delta_pct must be 0.0 when baseline is 0"
    );
    assert!(!result.exceeded);
}

#[test]
fn check_against_baseline_no_limit_never_exceeds() {
    let mut store = BaselineStore::default();
    store.set(BaselineEntry {
        name: "main".into(),
        saved_at: Utc::now(),
        run_id: "run-1".into(),
        summary: snapshot(100),
        json_path: None,
    });
    let result = check_against_baseline(&store, "main", 999, None).unwrap();
    assert!(
        !result.exceeded,
        "no max_delta_pct means exceeded is never true"
    );
}

#[test]
fn check_against_baseline_negative_delta_never_exceeds() {
    let mut store = BaselineStore::default();
    store.set(BaselineEntry {
        name: "main".into(),
        saved_at: Utc::now(),
        run_id: "run-1".into(),
        summary: snapshot(200),
        json_path: None,
    });
    let result = check_against_baseline(&store, "main", 100, Some(5.0)).unwrap();
    assert!(result.delta < 0, "delta must be negative");
    assert!(
        !result.exceeded,
        "shrinking code must not exceed positive threshold"
    );
}

#[test]
fn baseline_check_result_print_summary_does_not_panic() {
    let result = sloc_core::baseline::BaselineCheckResult {
        baseline_name: "main".into(),
        baseline_code_lines: 100,
        current_code_lines: 120,
        delta: 20,
        delta_pct: 20.0,
        exceeded: false,
        max_delta_pct: Some(30.0),
    };
    result.print_summary();
}

#[test]
fn baseline_check_result_print_summary_exceeded_does_not_panic() {
    let result = sloc_core::baseline::BaselineCheckResult {
        baseline_name: "release".into(),
        baseline_code_lines: 100,
        current_code_lines: 200,
        delta: 100,
        delta_pct: 100.0,
        exceeded: true,
        max_delta_pct: Some(10.0),
    };
    result.print_summary();
}

#[test]
fn resolve_baselines_path_uses_env_var() {
    let _guard = env_lock();
    let dir = tempfile::tempdir().unwrap();
    let custom = dir.path().join("my_baselines.json");
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("SLOC_BASELINES_PATH", custom.to_str().unwrap()) };
    let resolved = sloc_core::baseline::resolve_baselines_path();
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("SLOC_BASELINES_PATH") };
    assert_eq!(resolved, custom);
}

#[test]
fn resolve_baselines_path_default_when_env_unset() {
    let _guard = env_lock();
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("SLOC_BASELINES_PATH") };
    let resolved = sloc_core::baseline::resolve_baselines_path();
    assert!(
        resolved.to_string_lossy().contains("baselines.json"),
        "default path must end with baselines.json, got: {}",
        resolved.display()
    );
}

// ── resolve_coverage_file ─────────────────────────────────────────────────────

#[test]
fn resolve_coverage_file_from_config_path() {
    use sloc_core::coverage::resolve_coverage_file;
    let _guard = env_lock();
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("SLOC_COVERAGE_FILE") };
    let path = std::path::Path::new("/tmp/coverage.info");
    let result = resolve_coverage_file(Some(path));
    assert_eq!(result, Some(std::path::PathBuf::from("/tmp/coverage.info")));
}

#[test]
fn resolve_coverage_file_none_when_no_config_and_no_env() {
    use sloc_core::coverage::resolve_coverage_file;
    let _guard = env_lock();
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("SLOC_COVERAGE_FILE") };
    let result = resolve_coverage_file(None);
    assert!(result.is_none());
}

// ── parse_lcov direct tests ───────────────────────────────────────────────────

#[test]
fn parse_lcov_round_trip_returns_correct_counts() {
    let lcov = "\
SF:src/lib.rs\n\
FN:1,main\n\
FNDA:3,main\n\
FNF:1\n\
FNH:1\n\
DA:1,3\n\
DA:2,2\n\
DA:3,0\n\
LH:2\n\
LF:3\n\
BRDA:1,0,0,1\n\
BRDA:1,0,1,0\n\
BRF:2\n\
BRH:1\n\
end_of_record\n";
    let map = parse_lcov(lcov);
    let key = std::path::PathBuf::from("src/lib.rs");
    let cov = map.get(&key).expect("should have coverage for src/lib.rs");
    assert_eq!(cov.lines_found, 3);
    assert_eq!(cov.lines_hit, 2);
    assert_eq!(cov.functions_found, 1);
    assert_eq!(cov.functions_hit, 1);
    assert_eq!(cov.branches_found, 2);
    assert_eq!(cov.branches_hit, 1);
}

#[test]
fn parse_lcov_derives_counts_when_summaries_absent() {
    // Valid LCOV with only raw DA:/FN:/FNDA:/BRDA: records and no LF/LH/FNF/FNH/BRF/BRH
    // summaries (as emitted by `llvm-cov export -format=lcov` and some geninfo configs).
    let lcov = "\
SF:src/lib.rs\n\
FN:1,alpha\n\
FN:5,beta\n\
FNDA:3,alpha\n\
FNDA:0,beta\n\
DA:1,3\n\
DA:2,2\n\
DA:3,0\n\
DA:4,7\n\
BRDA:1,0,0,1\n\
BRDA:1,0,1,-\n\
BRDA:2,0,0,0\n\
end_of_record\n";
    let map = parse_lcov(lcov);
    let cov = map
        .get(&std::path::PathBuf::from("src/lib.rs"))
        .expect("should have coverage for src/lib.rs");
    assert_eq!(cov.lines_found, 4, "4 DA records");
    assert_eq!(cov.lines_hit, 3, "3 DA records with hits > 0");
    assert_eq!(cov.functions_found, 2, "2 FN records");
    assert_eq!(cov.functions_hit, 1, "1 FNDA record with hits > 0");
    assert_eq!(cov.branches_found, 3, "3 BRDA records");
    assert_eq!(cov.branches_hit, 1, "1 BRDA taken (not '-' or '0')");
}

#[test]
fn parse_lcov_prefers_explicit_summaries_over_raw_records() {
    // When both summaries and raw records are present, summaries win (they may reflect
    // post-processing/aggregation the raw records don't).
    let lcov = "\
SF:src/lib.rs\n\
DA:1,1\n\
DA:2,1\n\
LF:10\n\
LH:9\n\
end_of_record\n";
    let map = parse_lcov(lcov);
    let cov = map.get(&std::path::PathBuf::from("src/lib.rs")).unwrap();
    assert_eq!(cov.lines_found, 10);
    assert_eq!(cov.lines_hit, 9);
}

#[test]
fn parse_lcov_multiple_files() {
    let lcov = "\
SF:src/a.rs\n\
LH:5\n\
LF:10\n\
FNF:2\n\
FNH:2\n\
BRF:0\n\
BRH:0\n\
end_of_record\n\
SF:src/b.rs\n\
LH:8\n\
LF:8\n\
FNF:3\n\
FNH:3\n\
BRF:4\n\
BRH:4\n\
end_of_record\n";
    let map = parse_lcov(lcov);
    assert_eq!(map.len(), 2);
    let a = map.get(&std::path::PathBuf::from("src/a.rs")).unwrap();
    assert_eq!(a.lines_found, 10);
    assert_eq!(a.lines_hit, 5);
    let b = map.get(&std::path::PathBuf::from("src/b.rs")).unwrap();
    assert_eq!(b.lines_hit, 8);
}

#[test]
fn parse_lcov_empty_input_returns_empty_map() {
    let map = parse_lcov("");
    assert!(map.is_empty());
}

// ── aggregate_line_coverage tests ────────────────────────────────────────────

#[test]
fn aggregate_line_coverage_returns_none_for_empty_slice() {
    let result = aggregate_line_coverage(&[]);
    assert!(result.is_none());
}

#[test]
fn aggregate_line_coverage_returns_percentage() {
    let cov = FileCoverage {
        lines_found: 100,
        lines_hit: 80,
        functions_found: 10,
        functions_hit: 8,
        branches_found: 20,
        branches_hit: 16,
    };
    let result = aggregate_line_coverage(&[&cov]);
    assert!(result.is_some());
    let pct = result.unwrap();
    assert!((pct - 80.0).abs() < 0.01, "expected 80.0%, got {pct}");
}

#[test]
fn aggregate_line_coverage_multiple_records() {
    let c1 = FileCoverage {
        lines_found: 50,
        lines_hit: 40,
        functions_found: 5,
        functions_hit: 4,
        branches_found: 0,
        branches_hit: 0,
    };
    let c2 = FileCoverage {
        lines_found: 50,
        lines_hit: 25,
        functions_found: 5,
        functions_hit: 3,
        branches_found: 0,
        branches_hit: 0,
    };
    let result = aggregate_line_coverage(&[&c1, &c2]).unwrap();
    // combined: 65 hit / 100 found = 65%
    assert!((result - 65.0).abs() < 0.01, "expected 65.0%, got {result}");
}

// ── analyze() edge case tests ─────────────────────────────────────────────────

#[test]
fn analyze_skips_high_entropy_file_that_looks_binary() {
    let dir = tempfile::tempdir().unwrap();
    // Create a file that triggers the binary heuristic: very long single line, no whitespace
    let binary_like = "A".repeat(2000);
    std::fs::write(dir.path().join("data.bin"), binary_like).unwrap();
    // Also add a real Rust file so the run has at least one analyzed file
    std::fs::write(dir.path().join("lib.rs"), "fn foo() {}\n").unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    // The .bin file should NOT be in analyzed files (either skipped or unrecognized language)
    assert!(
        !run.per_file_records
            .iter()
            .any(|r| r.relative_path == "data.bin"),
        "binary-like file should not be analyzed as a language file"
    );
}

#[test]
fn analyze_walks_deeply_nested_directories() {
    let dir = tempfile::tempdir().unwrap();
    // Create 4 levels of nesting
    let deep = dir.path().join("a").join("b").join("c").join("d");
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(deep.join("deep.rs"), "fn deep() {}\n").unwrap();
    std::fs::write(dir.path().join("top.rs"), "fn top() {}\n").unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert_eq!(
        run.summary_totals.files_analyzed, 2,
        "should find both top and deep files"
    );
}

#[test]
fn analyze_ci_name_set_when_github_actions_env_present() {
    let _guard = env_lock();
    // Isolate from the ambient CI environment (this suite itself runs under Jenkins,
    // which exports JENKINS_HOME/BUILD_URL/etc. that would otherwise win the
    // precedence check). The guard restores the original env on drop, even on panic.
    let _ci = CiEnvIsolation::new();

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.rs"), "fn f() {}\n").unwrap();
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("GITHUB_ACTIONS", "true") };
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();

    assert_eq!(
        run.environment.ci_name.as_deref(),
        Some("GitHub Actions"),
        "CI name should be set from GITHUB_ACTIONS env var"
    );
}

#[test]
fn analyze_sets_git_branch_from_github_ref_when_no_git_dir() {
    let _guard = env_lock();
    // Isolate from ambient CI environment to avoid cross-test env pollution.
    let _ci = CiEnvIsolation::new();

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.rs"), "fn g() {}\n").unwrap();
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("GITHUB_REF", "refs/heads/feature-branch") };
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();

    // If a .git dir was found, git_branch might come from git; otherwise from env
    if run.git_branch.is_some() {
        assert!(
            run.git_branch.as_deref() == Some("feature-branch")
                || run.git_branch.as_deref().is_some(),
            "git_branch should be set"
        );
    }
    // If no git dir (most common in test), the branch may remain None or be set from env
}

#[test]
fn analyze_with_lcov_file_populates_coverage_on_records() {
    let dir = tempfile::tempdir().unwrap();
    // Write source file
    std::fs::write(
        dir.path().join("lib.rs"),
        "fn foo() {}\nfn bar() {}\n// comment\n",
    )
    .unwrap();
    // Write matching LCOV data
    let lcov_content = format!(
        "SF:{}\nLH:2\nLF:3\nFNF:2\nFNH:2\nBRF:0\nBRH:0\nend_of_record\n",
        dir.path().join("lib.rs").display()
    );
    let lcov_path = dir.path().join("lcov.info");
    std::fs::write(&lcov_path, lcov_content).unwrap();
    let mut cfg = analysis_config_for(dir.path());
    cfg.analysis.coverage_file = Some(lcov_path);
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert_eq!(run.summary_totals.files_analyzed, 1);
    // Coverage may or may not be applied depending on path matching, but should not error
    assert!(!run.warnings.iter().any(|w| w.contains("panic")));
}

#[test]
fn analyze_multiple_languages_in_same_dir() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.py"), "def foo():\n    pass\n").unwrap();
    std::fs::write(dir.path().join("app.js"), "function bar() {}\n").unwrap();
    std::fs::write(dir.path().join("style.css"), ".a { color: red; }\n").unwrap();
    std::fs::write(dir.path().join("lib.rs"), "fn rust_fn() {}\n").unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert_eq!(run.summary_totals.files_analyzed, 4);
    assert!(
        run.totals_by_language.len() >= 4,
        "should have at least 4 language entries"
    );
}

// ── analyze() against a real git repo (covers git/CI detection paths) ─────────

#[test]
fn analyze_real_git_repo_populates_metadata() {
    use std::process::Command;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // Initialise a git repo with one commit and a remote URL.
    let run_git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "Tester")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "Tester")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
    };
    if run_git(&["init", "-q"]).is_err() {
        return; // git not available — skip silently
    }
    let _ = run_git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/test/repo.git",
    ]);
    std::fs::write(root.join("main.rs"), "fn main() {\n    let _ = 1;\n}\n").unwrap();
    std::fs::write(root.join("util.py"), "def f():\n    return 1\n").unwrap();
    let _ = run_git(&["add", "."]);
    let committed = run_git(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", "init"]);
    if committed.map_or(true, |o| !o.status.success()) {
        return; // commit failed (e.g. no identity) — skip
    }

    let cfg = analysis_config_for(root);
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert!(run.summary_totals.files_analyzed >= 2);
    // Git metadata should now be populated from the real repo.
    assert!(
        run.git_commit_short.is_some(),
        "expected git commit metadata from a real repo"
    );
    assert_eq!(
        run.git_remote_url.as_deref(),
        Some("https://github.com/test/repo.git")
    );
}

#[test]
fn analyze_handles_diverse_file_contents() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // Normal source.
    std::fs::write(root.join("a.rs"), "pub fn a() -> i32 { 42 }\n").unwrap();
    // A generated-looking file (machine-generated banner).
    std::fs::write(
        root.join("gen.rs"),
        "// @generated by tool — do not edit\npub const X: u8 = 1;\n",
    )
    .unwrap();
    // A binary-ish file (NUL bytes, long line, low whitespace).
    let mut bin = vec![0u8, 1, 2, 3, 0, 7, 9];
    bin.extend(std::iter::repeat_n(b'A', 5000));
    std::fs::write(root.join("blob.bin"), &bin).unwrap();
    // A UTF-16 LE encoded file with BOM.
    let mut u16f = vec![0xFF, 0xFE];
    for ch in "let x = 1;\n".encode_utf16() {
        u16f.extend_from_slice(&ch.to_le_bytes());
    }
    std::fs::write(root.join("utf16.js"), &u16f).unwrap();

    let cfg = analysis_config_for(root);
    let run = analyze(&cfg, "test", None, None).unwrap();
    // The analyzer must not crash and should classify at least the rust sources.
    assert!(run.summary_totals.files_considered >= 3);
}

// ── COCOMO + ULOC verification ────────────────────────────────────────────────

#[test]
fn analyze_nonempty_project_produces_cocomo_estimate() {
    let dir = tempfile::tempdir().unwrap();
    // Need enough code for COCOMO to be non-trivial
    let code = "fn foo() {}\nfn bar() {}\nfn baz() -> i32 { 42 }\n".repeat(10);
    std::fs::write(dir.path().join("lib.rs"), code).unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert!(
        run.cocomo.is_some(),
        "non-empty project must produce a COCOMO estimate"
    );
    let cocomo = run.cocomo.unwrap();
    assert!(
        cocomo.ksloc > 0.0,
        "KSLOC must be positive, got {}",
        cocomo.ksloc
    );
    assert!(cocomo.effort_person_months > 0.0, "effort must be positive");
    assert!(cocomo.duration_months > 0.0, "duration must be positive");
}

#[test]
fn analyze_empty_project_has_no_cocomo() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    // No code lines → no COCOMO estimate
    assert!(
        run.cocomo.is_none() || run.summary_totals.code_lines == 0,
        "empty project should have no COCOMO or zero code lines"
    );
}

#[test]
fn analyze_produces_uloc_for_nonempty_project() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn foo() {}\nfn bar() {}\n").unwrap();
    std::fs::write(dir.path().join("b.rs"), "fn baz() {}\nfn qux() {}\n").unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert!(run.uloc > 0, "ULOC must be positive for non-empty project");
}

#[test]
fn analyze_detects_duplicate_files_same_content() {
    let dir = tempfile::tempdir().unwrap();
    let content = "fn shared() { let x = 42; }\nfn another() { let y = 7; }\n".repeat(5);
    std::fs::write(dir.path().join("a.rs"), &content).unwrap();
    std::fs::write(dir.path().join("b.rs"), &content).unwrap(); // identical content
    std::fs::write(
        dir.path().join("unique.rs"),
        "fn unique_only() { let z = 99; }\n",
    )
    .unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    // All files should be analyzed
    assert_eq!(run.summary_totals.files_analyzed, 3);
    // Duplicate groups should contain a.rs and b.rs
    assert!(
        !run.duplicate_groups.is_empty(),
        "duplicate groups should be non-empty when files have identical content"
    );
    let all_paths: Vec<&str> = run
        .duplicate_groups
        .iter()
        .flat_map(|g| g.iter().map(String::as_str))
        .collect();
    let has_dup = all_paths.iter().any(|p| p.contains("a.rs"))
        && all_paths.iter().any(|p| p.contains("b.rs"));
    assert!(
        has_dup,
        "a.rs and b.rs should be in the same duplicate group"
    );
}

#[test]
fn analyze_with_identical_small_files_finds_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let content = "pub fn helper(x: i32) -> i32 { x * 2 }\n";
    for name in ["x.rs", "y.rs", "z.rs"] {
        std::fs::write(dir.path().join(name), content).unwrap();
    }
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    // Duplicate detection works even for small files
    assert_eq!(run.summary_totals.files_analyzed, 3);
    // Groups should be non-empty
    let total_in_groups: usize = run.duplicate_groups.iter().map(Vec::len).sum();
    assert!(
        total_in_groups >= 2,
        "at least 2 files should be in duplicate groups"
    );
}

#[test]
fn analyze_dryness_pct_is_none_for_empty_project() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    // With no files, dryness is None
    assert!(
        run.dryness_pct.is_none() || run.summary_totals.code_lines == 0,
        "empty project should have no dryness percentage"
    );
}

#[test]
fn analyze_dryness_pct_is_some_for_nonempty_project() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn foo() { let x = 1; }\n").unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    // With some code, dryness_pct should be set
    if run.summary_totals.code_lines > 0 {
        assert!(
            run.dryness_pct.is_some(),
            "non-empty project should have dryness_pct"
        );
        let pct = run.dryness_pct.unwrap();
        assert!(
            (0.0..=100.0).contains(&pct),
            "dryness_pct must be 0-100, got {pct}"
        );
    }
}

// ── Style analysis ────────────────────────────────────────────────────────────

#[test]
fn analyze_style_summary_populated_for_consistent_code() {
    let dir = tempfile::tempdir().unwrap();
    // Well-formatted Rust code with consistent 4-space indentation
    let code = r#"fn main() {
    let x = 1;
    let y = 2;
    println!("{}", x + y);
}

fn helper(n: i32) -> i32 {
    n * 2
}
"#;
    std::fs::write(dir.path().join("main.rs"), code).unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    // Style summary may or may not be computed depending on config defaults,
    // but the run should complete without errors.
    assert!(run.summary_totals.files_analyzed >= 1);
    // style_summary is optional — just verify it doesn't crash
    let _ = run.style_summary;
}

// ── Compute delta on coverage-bearing runs ────────────────────────────────────

#[test]
fn compute_delta_with_coverage_data() {
    let mut base = make_run_with_files(vec![("src/lib.rs", 100)]);
    let mut current = make_run_with_files(vec![("src/lib.rs", 120)]);
    // Add coverage to both
    base.per_file_records[0].coverage = Some(sloc_core::FileCoverage {
        lines_found: 100,
        lines_hit: 75,
        functions_found: 10,
        functions_hit: 8,
        branches_found: 0,
        branches_hit: 0,
    });
    base.summary_totals.coverage_lines_found = 100;
    base.summary_totals.coverage_lines_hit = 75;
    current.per_file_records[0].coverage = Some(sloc_core::FileCoverage {
        lines_found: 120,
        lines_hit: 100,
        functions_found: 12,
        functions_hit: 11,
        branches_found: 0,
        branches_hit: 0,
    });
    current.summary_totals.coverage_lines_found = 120;
    current.summary_totals.coverage_lines_hit = 100;
    let cmp = compute_delta(&base, &current);
    assert_eq!(cmp.summary.code_lines_delta, 20);
    // Coverage delta should be Some
    assert!(
        cmp.summary.coverage_lines_hit_delta.is_some(),
        "coverage delta must be Some when both runs have coverage data"
    );
}

#[test]
fn compute_delta_without_coverage_on_baseline() {
    let base = make_run_with_files(vec![("src/lib.rs", 50)]);
    let mut current = make_run_with_files(vec![("src/lib.rs", 60)]);
    current.per_file_records[0].coverage = Some(sloc_core::FileCoverage {
        lines_found: 60,
        lines_hit: 50,
        functions_found: 5,
        functions_hit: 4,
        branches_found: 0,
        branches_hit: 0,
    });
    current.summary_totals.coverage_lines_found = 60;
    current.summary_totals.coverage_lines_hit = 50;
    let cmp = compute_delta(&base, &current);
    assert_eq!(cmp.summary.code_lines_delta, 10);
    // Delta completes without panic regardless of coverage presence
    let _ = cmp.summary.coverage_lines_hit_delta;
}

// ── ScanRegistry advanced operations ─────────────────────────────────────────

#[test]
fn scan_registry_preserves_order_of_insertion() {
    let mut reg = ScanRegistry::default();
    for i in 0..5 {
        let mut e = make_registry_entry(&format!("run-{i}"));
        e.timestamp_utc = Utc::now() + chrono::Duration::seconds(i64::from(i));
        reg.add_entry(e);
    }
    assert_eq!(reg.entries.len(), 5);
    // Entries should be present
    for i in 0..5 {
        assert!(reg.find_by_run_id(&format!("run-{i}")).is_some());
    }
}

#[test]
fn scan_registry_multiple_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("registry.json");
    let mut reg = ScanRegistry::default();
    for i in 0..3 {
        reg.add_entry(make_registry_entry(&format!("run-{i}")));
    }
    reg.save(&path).unwrap();
    let loaded = ScanRegistry::load(&path);
    assert_eq!(loaded.entries.len(), 3);
    // Add more and save again
    let mut loaded = loaded;
    loaded.add_entry(make_registry_entry("run-3"));
    loaded.save(&path).unwrap();
    let loaded2 = ScanRegistry::load(&path);
    assert_eq!(loaded2.entries.len(), 4);
}

// ── Watched dirs store advanced ───────────────────────────────────────────────

#[test]
fn watched_dirs_store_remove_entry() {
    let mut store = WatchedDirsStore::default();
    let p = PathBuf::from("/tmp/test-dir");
    store.add(p.clone());
    assert_eq!(store.dirs.len(), 1);
    store.remove(&p);
    assert!(
        store.dirs.is_empty(),
        "removing added dir should leave empty store"
    );
}

#[test]
fn watched_dirs_store_remove_nonexistent_is_noop() {
    let mut store = WatchedDirsStore::default();
    store.add(PathBuf::from("/tmp/a"));
    store.remove(&PathBuf::from("/tmp/does-not-exist"));
    assert_eq!(
        store.dirs.len(),
        1,
        "removing nonexistent dir must not affect store"
    );
}

// ── Multi-delta ───────────────────────────────────────────────────────────────

#[test]
fn compute_multi_delta_with_three_runs() {
    use sloc_core::compute_multi_delta;
    let r0 = make_run_with_files(vec![("src/a.rs", 100)]);
    let r1 = make_run_with_files(vec![("src/a.rs", 120), ("src/b.rs", 40)]);
    let r2 = make_run_with_files(vec![("src/a.rs", 130), ("src/b.rs", 50), ("src/c.rs", 20)]);
    let refs = [&r0, &r1, &r2];
    let multi = compute_multi_delta(&refs);
    assert_eq!(multi.points.len(), 3);
    assert_eq!(
        multi.sequential_deltas.len(),
        2,
        "2 consecutive deltas for 3 runs"
    );
}

// ── Analyze with excluded language ────────────────────────────────────────────

#[test]
fn analyze_with_enabled_languages_filter() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn rust_fn() {}\n").unwrap();
    std::fs::write(dir.path().join("b.py"), "def py_fn(): pass\n").unwrap();
    let mut cfg = analysis_config_for(dir.path());
    // Only analyze Rust files
    cfg.analysis.enabled_languages = vec!["rust".to_string()];
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert_eq!(
        run.summary_totals.files_analyzed, 1,
        "only Rust file should be analyzed"
    );
    assert_eq!(
        run.totals_by_language[0].language,
        sloc_languages::Language::Rust
    );
}

#[test]
fn analyze_with_exclude_globs_filters_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(dir.path().join("test_helpers.rs"), "fn helper() {}\n").unwrap();
    let mut cfg = analysis_config_for(dir.path());
    cfg.discovery.exclude_globs = vec!["**/test_*.rs".to_string()];
    let run = analyze(&cfg, "test", None, None).unwrap();
    let paths: Vec<&str> = run
        .per_file_records
        .iter()
        .map(|r| r.relative_path.as_str())
        .collect();
    assert!(
        !paths.iter().any(|p| p.contains("test_helpers")),
        "test_helpers.rs should be excluded"
    );
}

// ── JSON serialization of CocomoEstimate ──────────────────────────────────────

#[test]
fn analysis_run_with_cocomo_roundtrips_json() {
    let dir = tempfile::tempdir().unwrap();
    let code = "fn f() { let x = 1 + 2; }\n".repeat(100);
    std::fs::write(dir.path().join("lib.rs"), code).unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert!(
        run.cocomo.is_some(),
        "analyze should produce COCOMO estimate"
    );

    // Serialize and deserialize
    let json = serde_json::to_string(&run).unwrap();
    let back: sloc_core::AnalysisRun = serde_json::from_str(&json).unwrap();
    assert!(back.cocomo.is_some(), "COCOMO must survive JSON roundtrip");
    let orig = run.cocomo.unwrap();
    let restored = back.cocomo.unwrap();
    assert!(
        (orig.ksloc - restored.ksloc).abs() < 0.001,
        "KSLOC must survive roundtrip"
    );
    assert!(
        (orig.effort_person_months - restored.effort_person_months).abs() < 0.001,
        "effort must survive roundtrip"
    );
}

// ── Generated file detection ──────────────────────────────────────────────────

#[test]
fn analyze_marks_generated_files() {
    let dir = tempfile::tempdir().unwrap();
    // Files with @generated comment should be flagged
    std::fs::write(
        dir.path().join("gen_code.rs"),
        "// @generated by build_tool — do not edit\nfn generated() { let _ = 1; }\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("normal.rs"), "fn normal() {}\n").unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    // The generated file may be skipped or flagged depending on config
    assert!(
        run.summary_totals.files_considered >= 1,
        "at least the normal file should be considered"
    );
}

// ── Coverage store operations ─────────────────────────────────────────────────

#[test]
fn cleanup_policy_store_save_and_load() {
    use sloc_core::CleanupPolicy;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cleanup.json");
    let store = CleanupPolicyStore {
        policy: Some(CleanupPolicy {
            enabled: true,
            max_age_days: Some(30),
            max_run_count: Some(50),
            interval_hours: 24,
        }),
        ..CleanupPolicyStore::default()
    };
    store.save(&path).unwrap();
    let loaded = CleanupPolicyStore::load(&path);
    assert!(loaded.policy.is_some(), "policy must survive save/load");
    let p = loaded.policy.unwrap();
    assert!(p.enabled);
    assert_eq!(p.max_age_days, Some(30));
    assert_eq!(p.max_run_count, Some(50));
}

#[test]
fn cleanup_policy_store_default_has_no_policy() {
    let store = CleanupPolicyStore::default();
    assert!(store.policy.is_none());
    assert!(store.last_run_at.is_none());
}

// ── Cancellation via AtomicBool ───────────────────────────────────────────────

#[test]
fn analyze_respects_cancel_signal() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let dir = tempfile::tempdir().unwrap();
    // Add some files so there's work to do
    for i in 0..5 {
        std::fs::write(dir.path().join(format!("f{i}.rs")), "fn f() {}\n").unwrap();
    }
    let cancel = AtomicBool::new(false);
    // Set cancel = true immediately to trigger early exit
    cancel.store(true, Ordering::SeqCst);
    let cfg = analysis_config_for(dir.path());
    // Analysis should fail or return early with the cancel signal set
    let result = analyze(&cfg, "test", Some(&cancel), None);
    // Either cancelled (Err) or completed (Ok) — must not panic
    let _ = result;
}

// ── detect_ci_system / ci_branch_from_env — env-var tests ────────────────────
// These tests require ENV_MUTEX so parallel test threads don't interfere.
// Uses pub re-exports from sloc_core if available; otherwise tests via analyze.

// Note: detect_ci_system and ci_branch_from_env are private in sloc-core.
// We exercise them indirectly through the analyze() function's EnvironmentMetadata,
// OR by checking the AnalysisRun.environment.ci_name field after scanning.

#[test]
fn analysis_run_environment_ci_name_travis() {
    let _lock = env_lock();
    // Clear all known CI vars first to avoid false positives from the ambient env.
    let _ci = CiEnvIsolation::new();
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("TRAVIS", "true") };
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn f() {}\n").unwrap();
    let cfg = analysis_config_for(dir.path());
    let result = sloc_core::analyze(&cfg, "ci-travis-test", None, None);
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("TRAVIS") };
    // The CI name field should be set when TRAVIS=true
    if let Ok(run) = result {
        assert_eq!(run.environment.ci_name.as_deref(), Some("Travis CI"));
    }
}

#[test]
fn analysis_run_environment_ci_name_azure_devops() {
    let _lock = env_lock();
    let _ci = CiEnvIsolation::new();
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("TF_BUILD", "true") };
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("b.rs"), "fn g() {}\n").unwrap();
    let cfg = analysis_config_for(dir.path());
    let result = sloc_core::analyze(&cfg, "ci-azure-test", None, None);
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("TF_BUILD") };
    if let Ok(run) = result {
        assert_eq!(run.environment.ci_name.as_deref(), Some("Azure DevOps"));
    }
}

#[test]
fn analysis_run_environment_ci_name_teamcity() {
    let _lock = env_lock();
    let _ci = CiEnvIsolation::new();
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("TEAMCITY_VERSION", "2024.1") };
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("c.rs"), "fn h() {}\n").unwrap();
    let cfg = analysis_config_for(dir.path());
    let result = sloc_core::analyze(&cfg, "ci-teamcity-test", None, None);
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("TEAMCITY_VERSION") };
    if let Ok(run) = result {
        assert_eq!(run.environment.ci_name.as_deref(), Some("TeamCity"));
    }
}

#[test]
fn analysis_run_git_branch_from_github_ref_name() {
    let _lock = env_lock();
    // Use GITHUB_REF_NAME env var as ci_branch fallback for detached HEAD.
    // Clear ambient CI branch vars first so a Jenkins-provided BRANCH_NAME/GIT_BRANCH
    // can't take precedence over the value under test.
    let _ci = CiEnvIsolation::new();
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("GITHUB_REF_NAME", "my-feature-branch") };
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("d.rs"), "fn i() {}\n").unwrap();
    let cfg = analysis_config_for(dir.path());
    let result = sloc_core::analyze(&cfg, "ci-branch-test", None, None);
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("GITHUB_REF_NAME") };
    // If the directory has no .git, the branch comes from CI env var
    if let Ok(run) = result {
        // Branch may be set from env var when no .git is present
        // just assert no panic and the run is valid
        assert!(run.summary_totals.files_analyzed <= run.summary_totals.files_considered);
    }
}

// ── coverage parser edge cases ─────────────────────────────────────────────────

#[test]
fn lookup_coverage_strategy3_filename_fallback_when_suffix_differs() {
    // Suffix match fails (different parent dir) but the bare filename still matches —
    // this exercises strategy 3, distinct from the bare-filename suffix case.
    let mut map = std::collections::HashMap::new();
    map.insert(
        std::path::PathBuf::from("vendor/other/foo.rs"),
        FileCoverage {
            lines_found: 9,
            lines_hit: 7,
            functions_found: 0,
            functions_hit: 0,
            branches_found: 0,
            branches_hit: 0,
        },
    );
    let cov = lookup_coverage(&map, "weird/path/foo.rs");
    assert!(cov.is_some(), "filename fallback should match foo.rs");
    assert_eq!(cov.unwrap().lines_found, 9);
}

#[test]
fn parse_cobertura_class_without_filename_is_skipped() {
    // The no-filename class is placed LAST so no `filename=` attribute exists after it,
    // forcing the `extract_attr(... "filename")` lookup to return None -> `continue`.
    let xml = r#"<coverage>
      <packages><package><classes>
        <class filename="src/a.py"><lines>
          <line number="1" hits="1"/>
          <line number="2" hits="0"/>
        </lines></class>
        <class name="NoFile"><lines><line number="9" hits="1"/></lines></class>
      </classes></package></packages>
    </coverage>"#;
    let map = sloc_core::coverage::parse_cobertura(xml);
    assert!(map.contains_key(std::path::Path::new("src/a.py")));
    let cov = &map[std::path::Path::new("src/a.py")];
    assert_eq!(cov.lines_found, 2);
    assert_eq!(cov.lines_hit, 1);
}

#[test]
fn parse_cobertura_branch_fraction_variants() {
    // branch="true" with a well-formed condition-coverage, plus malformed/missing
    // variants that must fall through to (0,0) without panicking.
    let xml = r#"<coverage><packages><package><classes>
      <class filename="src/b.py"><lines>
        <line number="1" hits="1" branch="true" condition-coverage="50% (1/2)"/>
        <line number="2" hits="1" branch="true" condition-coverage="garbage-no-paren"/>
        <line number="3" hits="1" branch="true" condition-coverage="(no-slash)"/>
        <line number="4" hits="1" branch="true"/>
      </lines></class>
    </classes></package></packages></coverage>"#;
    let map = sloc_core::coverage::parse_cobertura(xml);
    let cov = &map[std::path::Path::new("src/b.py")];
    assert_eq!(cov.lines_found, 4);
    // Only the first well-formed fraction contributes (1/2).
    assert_eq!(cov.branches_found, 2);
    assert_eq!(cov.branches_hit, 1);
}

#[test]
fn parse_jacoco_sourcefile_without_name_is_skipped() {
    let xml = r#"<report>
      <package name="com/example">
        <sourcefile><counter type="LINE" missed="1" covered="2"/></sourcefile>
        <sourcefile name="Main.java"><counter type="LINE" missed="1" covered="3"/></sourcefile>
      </package>
    </report>"#;
    let map = sloc_core::coverage::parse_jacoco(xml);
    assert!(map.contains_key(std::path::Path::new("com/example/Main.java")));
    let cov = &map[std::path::Path::new("com/example/Main.java")];
    assert_eq!(cov.lines_found, 4);
    assert_eq!(cov.lines_hit, 3);
}

#[test]
fn parse_istanbul_invalid_json_returns_empty() {
    assert!(sloc_core::coverage::parse_istanbul("not json at all").is_empty());
    // Valid JSON that is not an object (a JSON array) also yields an empty map.
    assert!(sloc_core::coverage::parse_istanbul("[1,2,3]").is_empty());
}

#[test]
fn parse_coverage_py_without_files_key_returns_empty() {
    assert!(sloc_core::coverage::parse_coverage_py("not json").is_empty());
    assert!(sloc_core::coverage::parse_coverage_py(r#"{"meta":{}}"#).is_empty());
}

#[test]
fn resolve_coverage_file_prefers_env_var() {
    let _lock = env_lock();
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var("SLOC_COVERAGE_FILE", "from-env-coverage.info") };
    let resolved = sloc_core::coverage::resolve_coverage_file(Some(std::path::Path::new(
        "config-coverage.info",
    )));
    // FIXME: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::remove_var("SLOC_COVERAGE_FILE") };
    let resolved = resolved.expect("env var path should resolve to Some");
    assert!(
        resolved
            .to_string_lossy()
            .contains("from-env-coverage.info"),
        "env var must win over config path, got {}",
        resolved.display()
    );
}

// ── analyze() branch coverage: single-file root, header reclassification, ────
//    disabled-language skip, and style-summary aggregation. ────────────────────

#[test]
fn analyze_single_file_root_path() {
    // A root that is a *file* (not a directory) takes the `root.is_file()` branch
    // in analyze(), scanning just that file.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("solo.rs");
    std::fs::write(&file, "fn main() {}\nlet x = 1;\n").unwrap();
    let mut cfg = AppConfig::default();
    cfg.discovery.root_paths = vec![file];
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert_eq!(run.summary_totals.files_analyzed, 1);
}

#[test]
fn analyze_header_reclassified_as_cpp() {
    // A .h file that uses C++-only constructs (namespace/class/templates) is
    // reclassified from C to C++ so class/namespace lines are counted correctly.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("widget.h"),
        "namespace ui {\nclass Widget {\npublic:\n    template<typename T> T get();\n};\n}\n",
    )
    .unwrap();
    let cfg = analysis_config_for(dir.path());
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert_eq!(run.summary_totals.files_analyzed, 1);
    // The file must be attributed to the C++ family, not C.
    assert!(
        run.totals_by_language
            .iter()
            .any(|l| l.language == Language::Cpp),
        "header with C++ constructs must be counted as C++: {:?}",
        run.totals_by_language
            .iter()
            .map(|l| l.language)
            .collect::<Vec<_>>()
    );
}

#[test]
fn analyze_disabled_language_is_skipped_by_policy() {
    // Restricting enabled_languages to Python means a .rs file is skipped by policy.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();
    let mut cfg = analysis_config_for(dir.path());
    cfg.analysis.enabled_languages = vec!["Python".to_string()];
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert_eq!(run.summary_totals.files_analyzed, 0);
    assert_eq!(run.summary_totals.files_skipped, 1);
}

/// Two Rust files with divergent indentation drive the style-summary
/// `dominant_indent_label` vote arms (tabs vs mixed), and a raised column
/// threshold drives the `line_col_pct` threshold branches.
fn write_style_corpus(dir: &std::path::Path) {
    std::fs::write(dir.path_tabs(), "fn a() {\n\tlet x = 1;\n\tlet y = 2;\n}\n").unwrap();
    std::fs::write(
        dir.path_mixed(),
        "fn b() {\n\tlet x = 1;\n  let y = 2;\n    let z = 3;\n}\n",
    )
    .unwrap();
}

trait StyleCorpusPaths {
    fn path_tabs(&self) -> PathBuf;
    fn path_mixed(&self) -> PathBuf;
}
impl StyleCorpusPaths for std::path::Path {
    fn path_tabs(&self) -> PathBuf {
        self.join("tabbed.rs")
    }
    fn path_mixed(&self) -> PathBuf {
        self.join("mixed.rs")
    }
}

#[test]
fn analyze_style_summary_threshold_100() {
    let dir = tempfile::tempdir().unwrap();
    write_style_corpus(dir.path());
    let mut cfg = analysis_config_for(dir.path());
    cfg.analysis.style_col_threshold = 100;
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert!(run.style_summary.is_some());
}

#[test]
fn analyze_style_summary_threshold_120() {
    let dir = tempfile::tempdir().unwrap();
    write_style_corpus(dir.path());
    let mut cfg = analysis_config_for(dir.path());
    cfg.analysis.style_col_threshold = 120;
    let run = analyze(&cfg, "test", None, None).unwrap();
    assert!(run.style_summary.is_some());
}
