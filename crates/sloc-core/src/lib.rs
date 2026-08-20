// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>
#![allow(clippy::multiple_crate_versions)]

pub mod baseline;
pub mod coverage;
pub mod delta;
pub mod history;
pub mod maintenance;
pub use baseline::{BaselineEntry, BaselineStore, check_against_baseline, resolve_baselines_path};
pub use coverage::{FileCoverage, aggregate_line_coverage, lookup_coverage, parse_lcov};
pub use delta::{
    FileChangeStatus, FileDelta, MultiFileDelta, MultiScanComparison, MultiScanPoint,
    ScanComparison, SummaryDelta, compute_delta, compute_multi_delta,
};
pub use history::{
    CleanupPolicy, CleanupPolicyStore, RegistryEntry, ScanRegistry, ScanSummarySnapshot,
    WatchedDirsStore,
};
pub use maintenance::{
    PrunePlan, PruneReport, PrunedRun, dir_size_bytes, execute_run_prune, plan_run_prune,
    resolve_output_root, resolve_registry_path, rotate_log, rotated_log_paths, run_output_dir,
};

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use encoding_rs::{UTF_16BE, UTF_16LE, WINDOWS_1252};
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use sloc_config::{
    AppConfig, BinaryFileBehavior, BlankInBlockCommentPolicy, ContinuationLinePolicy,
    FailureBehavior, MixedLinePolicy,
};
use sloc_languages::style::IndentStyle;
use sloc_languages::{
    AnalysisOptions, Language, LineCategory, ParseMode, RawLineCounts, StyleAnalysis,
    StyleLangScope, analyze_text, classify_physical_lines, detect_language, supported_languages,
};

// ── Detection sample sizes and thresholds ────────────────────────────────────

/// Maximum number of worker threads used for parallel file analysis.
const MAX_ANALYSIS_THREADS: usize = 16;
/// Fallback thread count when `available_parallelism` is unavailable.
const DEFAULT_ANALYSIS_THREADS: usize = 4;
/// Byte sample used to detect `@generated` markers.
const GENERATED_SAMPLE_BYTES: usize = 1024;
/// Byte sample used to detect minified files via line-length heuristic.
const MINIFIED_SAMPLE_BYTES: usize = 4096;
/// Longest line length above which a file is considered minified.
const MINIFIED_LINE_THRESHOLD: usize = 2000;
/// Byte sample used to detect binary files via null-byte scan.
const BINARY_SAMPLE_BYTES: usize = 8192;

/// Atomics shared between `analyze()` and the caller so the caller can poll scan progress.
pub struct ProgressCounters {
    /// Number of candidate files processed so far (incremented per file, across all threads).
    pub files_done: Arc<AtomicUsize>,
    /// Total candidate files discovered (set before parallel analysis begins).
    pub files_total: Arc<AtomicUsize>,
}

/// Three-way outcome for metadata-level policy checks.
enum MetadataPolicyOutcome {
    /// Skip this file — include the record in output.
    Skip(Box<FileRecord>),
    /// Exclude this file entirely — no record in output (include-glob miss).
    Exclude,
    /// Continue to content checks.
    Continue,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    AnalyzedExact,
    AnalyzedBestEffort,
    SkippedBinary,
    SkippedDecodeError,
    SkippedUnsupported,
    SkippedByPolicy,
    ErrorInternal,
}

/// COCOMO I (Basic) project mode — determines the a/b/c/d exponent coefficients.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CocomoMode {
    /// Small team, familiar domain. Effort = 2.4 × KSLOC^1.05.
    #[default]
    Organic,
    /// Mixed constraints. Effort = 3.0 × KSLOC^1.12.
    SemiDetached,
    /// Tight hardware/OS constraints. Effort = 3.6 × KSLOC^1.20.
    Embedded,
}

/// COCOMO I (Basic) cost-estimation result derived from total code SLOC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CocomoEstimate {
    pub mode: CocomoMode,
    /// Input: code lines in thousands (KSLOC).
    pub ksloc: f64,
    /// Estimated development effort in person-months.
    pub effort_person_months: f64,
    /// Estimated schedule duration in months.
    pub duration_months: f64,
    /// Average team size (effort ÷ duration).
    pub avg_staff: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EffectiveCounts {
    pub code_lines: u64,
    pub comment_lines: u64,
    pub blank_lines: u64,
    pub mixed_lines_separate: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolMetadata {
    pub name: String,
    pub version: String,
    pub run_id: String,
    pub timestamp_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentMetadata {
    pub operating_system: String,
    pub architecture: String,
    pub runtime_mode: String,
    pub initiator_username: String,
    pub initiator_hostname: String,
    /// CI system name when the scan runs inside a known CI environment (Jenkins,
    /// GitHub Actions, GitLab CI, …). `None` for interactive / local runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ci_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SummaryTotals {
    pub files_considered: u64,
    pub files_analyzed: u64,
    pub files_skipped: u64,
    pub total_physical_lines: u64,
    pub code_lines: u64,
    pub comment_lines: u64,
    pub blank_lines: u64,
    pub mixed_lines_separate: u64,
    #[serde(default)]
    pub functions: u64,
    #[serde(default)]
    pub classes: u64,
    #[serde(default)]
    pub variables: u64,
    /// C/C++ variable-scope breakdown (member/local/global) and object-like macro constants.
    #[serde(default)]
    pub variables_member: u64,
    #[serde(default)]
    pub variables_local: u64,
    #[serde(default)]
    pub variables_global: u64,
    #[serde(default)]
    pub macro_definitions: u64,
    #[serde(default)]
    pub imports: u64,
    #[serde(default)]
    pub test_count: u64,
    /// Lexically detected test assertion call lines across all analyzed files.
    #[serde(default)]
    pub test_assertion_count: u64,
    /// Lexically detected test suite / fixture / group declaration lines across all analyzed files.
    #[serde(default)]
    pub test_suite_count: u64,
    /// Aggregated from LCOV data when provided.
    #[serde(default)]
    pub coverage_lines_found: u64,
    #[serde(default)]
    pub coverage_lines_hit: u64,
    #[serde(default)]
    pub coverage_functions_found: u64,
    #[serde(default)]
    pub coverage_functions_hit: u64,
    #[serde(default)]
    pub coverage_branches_found: u64,
    #[serde(default)]
    pub coverage_branches_hit: u64,
    /// Sum of per-file cyclomatic complexity scores across all analyzed files.
    #[serde(default)]
    pub cyclomatic_complexity: u64,
    /// Total logical SLOC across files that support it; `None` if no files produced LSLOC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lsloc: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageSummary {
    pub language: Language,
    pub files: u64,
    pub total_physical_lines: u64,
    pub code_lines: u64,
    pub comment_lines: u64,
    pub blank_lines: u64,
    pub mixed_lines_separate: u64,
    #[serde(default)]
    pub functions: u64,
    #[serde(default)]
    pub classes: u64,
    #[serde(default)]
    pub variables: u64,
    /// C/C++ variable-scope breakdown (member/local/global) and object-like macro constants.
    #[serde(default)]
    pub variables_member: u64,
    #[serde(default)]
    pub variables_local: u64,
    #[serde(default)]
    pub variables_global: u64,
    #[serde(default)]
    pub macro_definitions: u64,
    #[serde(default)]
    pub imports: u64,
    #[serde(default)]
    pub test_count: u64,
    #[serde(default)]
    pub test_assertion_count: u64,
    #[serde(default)]
    pub test_suite_count: u64,
    #[serde(default)]
    pub coverage_lines_found: u64,
    #[serde(default)]
    pub coverage_lines_hit: u64,
    #[serde(default)]
    pub coverage_functions_found: u64,
    #[serde(default)]
    pub coverage_functions_hit: u64,
    #[serde(default)]
    pub coverage_branches_found: u64,
    #[serde(default)]
    pub coverage_branches_hit: u64,
    #[serde(default)]
    pub cyclomatic_complexity: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lsloc: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub path: String,
    pub relative_path: String,
    pub language: Option<Language>,
    pub size_bytes: u64,
    pub detected_encoding: Option<String>,
    pub raw_line_categories: RawLineCounts,
    pub effective_counts: EffectiveCounts,
    pub status: FileStatus,
    pub warnings: Vec<String>,
    pub generated: bool,
    pub minified: bool,
    pub vendor: bool,
    pub parse_mode: Option<ParseMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submodule: Option<String>,
    /// Line/function/branch coverage from an external LCOV file, when provided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<FileCoverage>,
    /// Lexical style-guide adherence analysis; `None` for unsupported languages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_analysis: Option<StyleAnalysis>,
    /// Cyclomatic complexity approximation for this file (sum of branch decision keywords).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cyclomatic_complexity: Option<u32>,
    /// Logical SLOC estimate; `None` when the language does not support lexical LSLOC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lsloc: Option<u32>,
    /// Git commit-count in the configured activity window that touched this file.
    /// `None` unless `analysis.activity_window_days` is set and the root is a git repo.
    /// Powers the hotspots view; distinct from the web layer's scan-to-scan churn rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_count: Option<u32>,
    /// ISO-8601 date of the most recent commit touching this file within the window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_commit_date: Option<String>,
    /// Per-author line ownership for this file (blame-based), sorted by total lines owned
    /// descending. `None` unless `analysis.attribution` is enabled and the root is a git repo.
    /// `author_id` indexes into `AnalysisRun::authors`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ownership: Option<Vec<FileOwnership>>,
    /// SHA-256 (first 8 bytes as u64) of raw file bytes — used for duplicate detection.
    /// Not serialized; consumed in-process during `assemble_run`.
    #[serde(skip)]
    pub content_hash: u64,
}

/// Per-language-family style aggregation within a `StyleSummary`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageStyleGroup {
    /// Display label, e.g. `"C / C++"`, `"Python"`, `"JavaScript"`.
    pub language_family: String,
    /// Number of files in this group.
    pub files_count: u32,
    /// Name of the guide with the highest average adherence.
    pub dominant_guide: String,
    /// Average adherence of the dominant guide (0-100).
    pub dominant_score_pct: u8,
    /// Most common indent style across the group.
    pub common_indent_style: String,
    /// Average guide adherence scores (guide name, 0-100) sorted descending.
    pub guide_avg_scores: Vec<(String, u8)>,
    /// Percentage of files (0-100) where ≤ 5 % of lines exceed the configured column threshold.
    pub line80_compliant_pct: u8,
    /// Same as `line80_compliant_pct` but named for the actual configured threshold.
    pub line_col_compliant_pct: u8,
}

/// Aggregate multi-language style-guide adherence across all analysed files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StyleSummary {
    /// Total files for which style data was produced.
    pub files_analyzed: u32,
    /// Most common indent style across *all* analysed files.
    pub common_indent_style: String,
    /// Percentage of all analysed files (0-100) with ≤ 5 % of lines over 80 chars (legacy, always 80).
    pub line80_compliant_pct: u8,
    /// Percentage of all analysed files (0-100) with ≤ 5 % of lines over `col_threshold` chars.
    pub line_col_compliant_pct: u8,
    /// Column-width threshold used for `line_col_compliant_pct` (from `analysis.style_col_threshold`).
    pub col_threshold: u16,
    /// Per-language-family breakdown, sorted by `files_count` descending.
    pub by_language: Vec<LanguageStyleGroup>,
}

/// Backward-compatible alias kept so that `sloc-report` and `sloc-web` can migrate
/// incrementally without a breaking change on the same release.
pub type CppStyleSummary = StyleSummary;

/// Per-submodule aggregated stats produced when `submodule_breakdown` is enabled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmoduleSummary {
    pub name: String,
    pub relative_path: String,
    pub files_analyzed: u64,
    pub total_physical_lines: u64,
    pub code_lines: u64,
    pub comment_lines: u64,
    pub blank_lines: u64,
    pub language_summaries: Vec<LanguageSummary>,
    /// Short commit SHA (7 chars) of the submodule's own HEAD at scan time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit_short: Option<String>,
    /// Full commit SHA of the submodule's own HEAD at scan time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit_long: Option<String>,
    /// Branch name active in the submodule at scan time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    /// Author of the submodule's most recent commit at scan time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit_author: Option<String>,
    /// ISO 8601 author-date of the submodule's most recent commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit_date: Option<String>,
    /// URL of the submodule's `origin` remote as recorded in its `.git/config`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_remote_url: Option<String>,
}

/// A raw `(name, email)` pair exactly as recorded in git history (post-`.mailmap`). Multiple
/// raw identities can be folded into one [`Author`] when they share a normalized email.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct RawIdentity {
    pub name: String,
    pub email: String,
}

/// Line tallies bucketed the same three ways as [`LineCategory`]. `total_lines` is the sum of
/// the three category counts (i.e. physical lines owned).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthorLineCounts {
    pub code_lines: u64,
    pub comment_lines: u64,
    pub blank_lines: u64,
    pub total_lines: u64,
}

impl AuthorLineCounts {
    fn add_category(&mut self, cat: LineCategory) {
        match cat {
            LineCategory::Code => self.code_lines += 1,
            LineCategory::Comment => self.comment_lines += 1,
            LineCategory::Blank => self.blank_lines += 1,
        }
        self.total_lines += 1;
    }

    fn add(&mut self, other: &AuthorLineCounts) {
        self.code_lines += other.code_lines;
        self.comment_lines += other.comment_lines;
        self.blank_lines += other.blank_lines;
        self.total_lines += other.total_lines;
    }
}

/// One author's ownership of a single file. `author_id` indexes into [`AnalysisRun::authors`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOwnership {
    pub author_id: u32,
    pub counts: AuthorLineCounts,
}

/// A resolved contributor: one human, possibly spanning several raw git identities that were
/// auto-merged because they share a normalized email. Cross-email merges (corporate login vs.
/// full name vs. last name) are a follow-up interactive step; Phase 1 only auto-merges the
/// high-confidence same-email case, which is always safe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Author {
    /// Stable index into [`AnalysisRun::authors`]; authors are ordered by code lines owned.
    pub id: u32,
    pub canonical_name: String,
    pub canonical_email: String,
    /// Every raw `(name, email)` pair folded into this author.
    pub aliases: Vec<RawIdentity>,
    /// Repo-wide roll-up across all files.
    pub counts: AuthorLineCounts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisRun {
    pub tool: ToolMetadata,
    pub environment: EnvironmentMetadata,
    pub effective_configuration: AppConfig,
    pub input_roots: Vec<String>,
    pub summary_totals: SummaryTotals,
    pub totals_by_language: Vec<LanguageSummary>,
    pub per_file_records: Vec<FileRecord>,
    pub skipped_file_records: Vec<FileRecord>,
    pub warnings: Vec<String>,
    /// Non-empty only when `discovery.submodule_breakdown` is enabled.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub submodule_summaries: Vec<SubmoduleSummary>,
    /// Short git commit SHA (7 chars) at scan time, if the project is a git repo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit_short: Option<String>,
    /// Full git commit SHA at scan time, if the project is a git repo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit_long: Option<String>,
    /// Git branch active at scan time, if the project is a git repo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    /// Author of the last git commit at scan time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit_author: Option<String>,
    /// Comma-separated git tags pointing at HEAD at scan time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_tags: Option<String>,
    /// Nearest ancestor release tag (output of `git describe --tags --abbrev=0`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_nearest_tag: Option<String>,
    /// ISO 8601 author-date of the last git commit at scan time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit_date: Option<String>,
    /// URL of the `origin` remote as recorded in `.git/config` at scan time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_remote_url: Option<String>,
    /// Multi-language style-guide adherence; `None` when no supported files were analysed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_summary: Option<StyleSummary>,
    /// COCOMO I (Basic) effort/schedule estimate derived from total code SLOC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cocomo: Option<CocomoEstimate>,
    /// Unique Lines of Code: count of distinct non-blank code lines across all analyzed files.
    #[serde(default)]
    pub uloc: u64,
    /// `DRYness` percentage: `uloc / total_code_lines × 100`. `None` when code lines = 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dryness_pct: Option<f32>,
    /// Groups of files with identical content (relative paths). Only non-singleton groups included.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub duplicate_groups: Vec<Vec<String>>,
    /// Number of duplicate files excluded from SLOC totals (when `exclude_duplicates` is set).
    #[serde(default)]
    pub duplicates_excluded: usize,
    /// Per-author code-ownership roll-up, ordered by code lines owned (descending). Non-empty
    /// only when `analysis.attribution` is enabled and the scan root is a git repo. Author
    /// indices are referenced by `FileRecord::ownership`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<Author>,
}

#[derive(Default)]
struct GitInfo {
    commit_short: Option<String>,
    commit_long: Option<String>,
    branch: Option<String>,
    author: Option<String>,
    tags: Option<String>,
    nearest_tag: Option<String>,
    commit_date: Option<String>,
    remote_url: Option<String>,
}

/// Return `true` if `dir` is itself the top level of a git repository — i.e. it
/// contains a `.git` directory, or a `.git` *file* (worktree/submodule pointer)
/// that resolves to a real gitdir. Tests `dir` only; does not walk up.
fn is_git_root(dir: &Path) -> bool {
    let candidate = dir.join(".git");
    if candidate.is_dir() {
        return true;
    }
    candidate.is_file() && resolve_git_file_pointer(&candidate, dir).is_some()
}

/// Locate the `.git` directory by walking up from `start`.
/// Handles plain repos, worktrees (`.git` is a file with `gitdir:` pointer), and
/// submodules. Returns `None` if no git repo is found.
fn find_git_dir(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        let candidate = dir.join(".git");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if candidate.is_file()
            && let Some(resolved) = resolve_git_file_pointer(&candidate, dir)
        {
            return Some(resolved);
        }
        current = dir.parent();
    }
    None
}

/// Resolve a `.git` *file* (worktree/submodule pointer) to the absolute path it
/// points to. Returns `None` if the file is unreadable or lacks a `gitdir:` line,
/// or if the resolved path is not an existing directory.
fn resolve_git_file_pointer(file: &Path, base_dir: &Path) -> Option<PathBuf> {
    let content = fs::read_to_string(file).ok()?;
    let ptr = content.trim().strip_prefix("gitdir: ")?;
    // Normalise forward-slash paths to the OS separator so that Path operations
    // (join, exists, canonicalize) work correctly on Windows.
    let ptr_native = ptr.replace('/', std::path::MAIN_SEPARATOR_STR);
    let resolved = if Path::new(&ptr_native).is_absolute() {
        PathBuf::from(&ptr_native)
    } else {
        base_dir.join(&ptr_native)
    };
    // canonicalize resolves ".." components and symlinks; fall back to the
    // un-canonicalized path if it fails (e.g. some Windows configurations
    // return a UNC "\\?\" prefix that confuses later path operations).
    let final_path = resolved.canonicalize().unwrap_or(resolved);
    if final_path.is_dir() {
        Some(final_path)
    } else {
        None
    }
}

/// Resolve a git ref name (e.g. `refs/heads/main`) to a full 40-char commit SHA.
/// Checks loose ref files first, then `packed-refs`.
fn resolve_ref(git_dir: &Path, refname: &str) -> Option<String> {
    // Build the OS-native path to the loose ref file by joining each
    // forward-slash component individually.  This produces the correct
    // separator on every platform without any manual replacement.
    let ref_path = refname
        .split('/')
        .fold(git_dir.to_path_buf(), |p, c| p.join(c));
    if ref_path.exists() {
        let sha = fs::read_to_string(&ref_path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| s.len() >= 40 && s.chars().all(|c| c.is_ascii_hexdigit()));
        if sha.is_some() {
            return sha;
        }
    }
    // Packed refs: each line is "<sha> <refname>" (lines starting with '#' are
    // comments; lines starting with '^' are peeled tag objects to skip).
    // str::lines() handles both \n and \r\n, so Windows line endings are fine.
    let packed = fs::read_to_string(git_dir.join("packed-refs")).ok()?;
    for line in packed.lines() {
        if line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        let mut cols = line.splitn(2, ' ');
        let sha = cols.next()?;
        let name = cols.next()?.trim();
        if name == refname {
            return Some(sha.to_string());
        }
    }
    None
}

/// Extract the URL value from a `url = <value>` git-config line, returning `None` if absent or empty.
fn parse_url_line(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("url")?;
    let rest = rest.trim_start_matches([' ', '\t']);
    let url = rest.strip_prefix('=')?.trim();
    if url.is_empty() { None } else { Some(url) }
}

/// Parse `.git/config` and return the URL of the `origin` remote, if present.
fn read_git_remote_url(git_dir: &Path) -> Option<String> {
    let config = fs::read_to_string(git_dir.join("config")).ok()?;
    let mut in_origin = false;
    for line in config.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_origin = trimmed == r#"[remote "origin"]"#;
        } else if in_origin && let Some(url) = parse_url_line(trimmed) {
            return Some(url.to_owned());
        }
    }
    None
}

/// Detect git metadata by reading `.git/` files directly — no `git` executable
/// needed. Falls back gracefully for detached HEADs, shallow clones, and missing
/// reflogs.
fn detect_git_for_run(project_path: &Path) -> GitInfo {
    // Resolve the CI branch early so it can fill in any gap in git metadata.
    let ci_branch = ci_branch_from_env();

    let Some(git_dir) = find_git_dir(project_path) else {
        // No .git directory (e.g. scanning a non-repo path in CI). Use whatever
        // the CI system tells us about the branch.
        return GitInfo {
            branch: ci_branch,
            ..GitInfo::default()
        };
    };

    let head_raw = match fs::read_to_string(git_dir.join("HEAD")) {
        Ok(s) => s.trim().to_string(),
        Err(_) => {
            return GitInfo {
                branch: ci_branch,
                ..GitInfo::default()
            };
        }
    };

    let (branch_from_head, commit_long) = head_raw.strip_prefix("ref: ").map_or_else(
        || {
            if head_raw.len() >= 40 && head_raw.chars().all(|c| c.is_ascii_hexdigit()) {
                // Detached HEAD — HEAD file is the commit SHA (common in CI checkouts).
                (None, Some(head_raw[..40].to_string()))
            } else {
                (None, None)
            }
        },
        |refname| {
            let branch = refname
                .strip_prefix("refs/heads/")
                .map(|b| b.trim().to_string());
            let sha = resolve_ref(&git_dir, refname.trim());
            (branch, sha)
        },
    );
    // Prefer the branch name derived from the HEAD ref; fall back to the CI
    // env var (covers detached-HEAD checkouts done by Jenkins, GitHub Actions, etc.).
    let branch = branch_from_head.or(ci_branch);

    let commit_short = commit_long
        .as_deref()
        .map(|s| s.chars().take(7).collect::<String>());

    let author = run_git_cmd(project_path, &["log", "-1", "--format=%an", "HEAD"]);
    let commit_date = run_git_cmd(project_path, &["log", "-1", "--format=%aI", "HEAD"]);
    let remote_url = read_git_remote_url(&git_dir);

    // Tags and nearest-tag still require git CLI — try it as a best-effort bonus
    // but don't block on it. If git isn't available these will simply be None.
    let tags = run_git_cmd(project_path, &["tag", "--points-at", "HEAD"]).map(|t| {
        t.lines()
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    });
    let nearest_tag = run_git_cmd(project_path, &["describe", "--tags", "--abbrev=0", "HEAD"]);

    GitInfo {
        commit_short,
        commit_long,
        branch,
        author,
        tags,
        nearest_tag,
        commit_date,
        remote_url,
    }
}

/// Run a git command as a best-effort supplemental source.
fn run_git_cmd(dir: &Path, args: &[&str]) -> Option<String> {
    // Try the bare name first (works when git is on PATH), then fall back to
    // absolute paths for service accounts that run with a stripped PATH.
    // Unix paths silently fail on Windows and vice-versa.
    let candidates: &[&str] = &[
        // Works on all platforms when git is on PATH
        "git",
        // Common Linux / macOS install locations
        "/usr/bin/git",
        "/usr/local/bin/git",
        "/opt/homebrew/bin/git",
        // Git for Windows default installation paths
        r"C:\Program Files\Git\cmd\git.exe",
        r"C:\Program Files\Git\bin\git.exe",
        r"C:\Program Files (x86)\Git\cmd\git.exe",
    ];
    for &exe in candidates {
        let result = std::process::Command::new(exe)
            .args(["-c", "safe.directory=*"])
            .args(args)
            .current_dir(dir)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if result.is_some() {
            return result;
        }
    }
    None
}

/// Per-file git activity (commit-count + last-change date) over `window_days`, computed
/// with a single `git log --name-status` pass. Keys are paths relative to `project_path`
/// (via `--relative`), matching `FileRecord::relative_path`. Best-effort: returns an empty
/// map when git is unavailable or the path is not a repository — a scan never fails on this.
fn detect_file_activity(
    project_path: &Path,
    window_days: u32,
) -> HashMap<String, (u32, Option<String>)> {
    let since = format!("--since={window_days} days ago");
    // `--relative` limits output to (and reports paths relative to) the scan directory, so
    // the keys line up with FileRecord::relative_path even when scanning a repo subdirectory.
    // %x00 prefixes each commit header with a NUL, distinguishing it from name-status lines.
    let out = run_git_cmd(
        project_path,
        &[
            "-c",
            "core.quotepath=false",
            "log",
            since.as_str(),
            "--no-merges",
            "--name-status",
            "--relative",
            "--pretty=format:%x00%aI",
        ],
    );
    out.map(|s| parse_activity_log(&s)).unwrap_or_default()
}

/// Parse `git log --name-status` output (NUL-prefixed commit headers) into a
/// path → (`commit_count`, `last_commit_date`) map. `git log` emits newest-first, so the
/// first time a path appears is its most recent change. Renames are attributed to the new path.
fn parse_activity_log(out: &str) -> HashMap<String, (u32, Option<String>)> {
    let mut map: HashMap<String, (u32, Option<String>)> = HashMap::new();
    let mut current_date: Option<String> = None;
    for line in out.lines() {
        if let Some(date) = line.strip_prefix('\u{0}') {
            let d = date.trim();
            current_date = (!d.is_empty()).then(|| d.to_owned());
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        // name-status line: "STATUS\tpath" or "Rxxx\told\tnew" / "Cxxx\told\tnew".
        let mut fields = line.split('\t');
        let status = fields.next().unwrap_or("");
        let path = if status.starts_with('R') || status.starts_with('C') {
            fields.next_back()
        } else {
            fields.next()
        };
        let Some(path) = path.map(str::trim).filter(|p| !p.is_empty()) else {
            continue;
        };
        let entry = map.entry(path.to_owned()).or_insert((0, None));
        entry.0 += 1;
        if entry.1.is_none() {
            entry.1.clone_from(&current_date);
        }
    }
    map
}

/// Blame every analyzed file under `root`, attribute each physical line to its last author,
/// bucket it as code / comment / blank, and roll the tallies up per resolved contributor.
///
/// Mutates each `FileRecord` in place (populating `ownership`) and returns the repo-wide author
/// list ordered by code lines owned (descending). Best-effort: files that fail to blame or read
/// are silently skipped, so a partial result is always safe. Runs one `git blame` per file — the
/// reason this whole pass is opt-in behind `analysis.attribution`.
fn attribute_ownership(root: &Path, records: &mut [FileRecord]) -> Vec<Author> {
    let mut resolver = AuthorResolver::default();

    for rec in records.iter_mut() {
        let Some(language) = rec.language else {
            continue;
        };
        // Blame reports the working-tree file; classify the same bytes so line numbers align.
        let Ok(bytes) = std::fs::read(&rec.path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        let categories = classify_physical_lines(language, &text);
        let blame = blame_line_identities(root, &rec.relative_path);
        if blame.is_empty() {
            continue;
        }

        // Zip by the shorter of the two — trailing-newline / CRLF quirks can differ by one line.
        let mut per_file: HashMap<u32, AuthorLineCounts> = HashMap::new();
        for (category, ident) in categories.iter().zip(blame.iter()) {
            let id = resolver.resolve(ident);
            per_file.entry(id).or_default().add_category(*category);
        }

        let mut ownership: Vec<FileOwnership> = per_file
            .into_iter()
            .map(|(author_id, counts)| {
                resolver.authors[author_id as usize].counts.add(&counts);
                FileOwnership { author_id, counts }
            })
            .collect();
        ownership.sort_by_key(|entry| std::cmp::Reverse(entry.counts.total_lines));
        rec.ownership = Some(ownership);
    }

    resolver.finish(records)
}

/// Accumulates raw git identities into deduplicated [`Author`]s keyed by normalized email.
#[derive(Default)]
struct AuthorResolver {
    authors: Vec<Author>,
    /// Normalized-email key → author index.
    key_to_id: HashMap<String, u32>,
    /// Parallel to `authors`: the set of raw identities already recorded, to avoid dup aliases.
    seen_aliases: Vec<HashSet<RawIdentity>>,
}

impl AuthorResolver {
    fn resolve(&mut self, ident: &RawIdentity) -> u32 {
        let key = normalize_email_key(ident);
        if let Some(&id) = self.key_to_id.get(&key) {
            if self.seen_aliases[id as usize].insert(ident.clone()) {
                self.authors[id as usize].aliases.push(ident.clone());
            }
            return id;
        }
        let id = self.authors.len() as u32;
        let canonical_name = if ident.name.trim().is_empty() {
            ident.email.clone()
        } else {
            ident.name.clone()
        };
        self.authors.push(Author {
            id,
            canonical_name,
            canonical_email: ident.email.clone(),
            aliases: vec![ident.clone()],
            counts: AuthorLineCounts::default(),
        });
        self.seen_aliases.push(HashSet::from([ident.clone()]));
        self.key_to_id.insert(key, id);
        id
    }

    /// Sort authors by code lines owned (descending), reassign stable ids, and remap the
    /// `author_id`s already stored on each `FileRecord::ownership` to match.
    fn finish(self, records: &mut [FileRecord]) -> Vec<Author> {
        let mut order: Vec<usize> = (0..self.authors.len()).collect();
        order.sort_by(|&a, &b| {
            self.authors[b]
                .counts
                .code_lines
                .cmp(&self.authors[a].counts.code_lines)
                .then_with(|| {
                    self.authors[a]
                        .canonical_name
                        .cmp(&self.authors[b].canonical_name)
                })
        });
        let mut remap = vec![0u32; self.authors.len()];
        for (new_id, &old) in order.iter().enumerate() {
            remap[old] = new_id as u32;
        }
        for rec in records.iter_mut() {
            if let Some(ownership) = rec.ownership.as_mut() {
                for entry in ownership.iter_mut() {
                    entry.author_id = remap[entry.author_id as usize];
                }
            }
        }
        let mut sorted: Vec<Author> = order.iter().map(|&old| self.authors[old].clone()).collect();
        for (new_id, author) in sorted.iter_mut().enumerate() {
            author.id = new_id as u32;
        }
        sorted
    }
}

/// The stable identity key used for the safe, high-confidence auto-merge: normalized email.
/// Same-email identities (differing only in display-name spelling/capitalization) collapse into
/// one author. Cross-email merges are deferred to the Phase-2 interactive step. When no usable
/// email is present the display name is used so unrelated anonymous commits don't all merge.
fn normalize_email_key(ident: &RawIdentity) -> String {
    let email = ident.email.trim().to_lowercase();
    if email.is_empty() || email == "not.committed.yet" || !email.contains('@') {
        return format!("name:{}", ident.name.trim().to_lowercase());
    }
    // Strip a `+tag` suffix from the local part (e.g. `user+work@host` → `user@host`).
    match email.split_once('@') {
        Some((local, domain)) => {
            let core = local.split('+').next().unwrap_or(local);
            format!("{core}@{domain}")
        }
        None => email,
    }
}

/// Run `git blame` on `rel` (relative to `root`) and return one raw identity per physical line,
/// in file order. `-w` ignores whitespace-only reblame, `-M`/`-C` follow moved/copied code so a
/// refactor doesn't misattribute ownership; the repo `.mailmap` is honoured by git. Returns an
/// empty vec on any failure (non-git path, shallow clone, unreadable file).
fn blame_line_identities(root: &Path, rel: &str) -> Vec<RawIdentity> {
    run_git_cmd(
        root,
        &["blame", "--line-porcelain", "-w", "-M", "-C", "--", rel],
    )
    .map(|out| parse_blame_porcelain(&out))
    .unwrap_or_default()
}

/// Parse `git blame --line-porcelain` output into one [`RawIdentity`] per source line. In
/// line-porcelain mode the `author` / `author-mail` headers repeat for every line and each
/// block terminates with a TAB-prefixed content line.
fn parse_blame_porcelain(out: &str) -> Vec<RawIdentity> {
    let mut identities = Vec::new();
    let mut name = String::new();
    let mut email = String::new();
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("author ") {
            name = rest.trim().to_owned();
        } else if let Some(rest) = line.strip_prefix("author-mail ") {
            email = rest
                .trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .to_owned();
        } else if line.starts_with('\t') {
            identities.push(RawIdentity {
                name: std::mem::take(&mut name),
                email: std::mem::take(&mut email),
            });
        }
    }
    identities
}

/// Return the name of the CI system if the process is running inside one.
fn detect_ci_system() -> Option<&'static str> {
    let ev = |k: &str| std::env::var(k).is_ok();
    let ev_true = |k: &str| std::env::var(k).as_deref() == Ok("true");
    if ev("JENKINS_URL") || ev("JENKINS_HOME") || ev("BUILD_URL") {
        return Some("Jenkins");
    }
    if ev_true("GITHUB_ACTIONS") {
        return Some("GitHub Actions");
    }
    if ev_true("GITLAB_CI") {
        return Some("GitLab CI");
    }
    if ev_true("CIRCLECI") {
        return Some("CircleCI");
    }
    if ev_true("TRAVIS") {
        return Some("Travis CI");
    }
    if ev_true("TF_BUILD") {
        return Some("Azure DevOps");
    }
    if ev("TEAMCITY_VERSION") {
        return Some("TeamCity");
    }
    None
}

/// Read the current branch name from well-known CI environment variables.
/// Called as a fallback when the git HEAD is detached (common in CI checkouts).
fn ci_branch_from_env() -> Option<String> {
    const VARS: &[&str] = &[
        "BRANCH_NAME",        // Jenkins Pipeline
        "GIT_BRANCH",         // Jenkins Freestyle (may carry "origin/<branch>")
        "GITHUB_REF_NAME",    // GitHub Actions
        "CI_COMMIT_BRANCH",   // GitLab CI
        "CIRCLE_BRANCH",      // CircleCI
        "TRAVIS_BRANCH",      // Travis CI
        "BUILD_SOURCEBRANCH", // Azure DevOps (may carry "refs/heads/<branch>")
    ];
    for &var in VARS {
        if let Ok(val) = std::env::var(var) {
            let val = val.trim();
            let val = val
                .strip_prefix("refs/heads/")
                .or_else(|| val.strip_prefix("origin/"))
                .unwrap_or(val);
            if !val.is_empty() && val != "HEAD" {
                return Some(val.to_string());
            }
        }
    }
    None
}

fn get_current_username() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "unknown".to_string())
}

fn non_empty_env(var: &str) -> Option<String> {
    let v = std::env::var(var).ok()?;
    if v.is_empty() { None } else { Some(v) }
}

fn is_jenkins_env() -> bool {
    std::env::var("JENKINS_URL").is_ok()
        || std::env::var("JENKINS_HOME").is_ok()
        || std::env::var("BUILD_URL").is_ok()
}

fn get_hostname() -> String {
    // In CI environments prefer a human-readable agent/runner identifier over
    // whatever hostname the container was assigned.
    if is_jenkins_env()
        && let Some(n) = non_empty_env("NODE_NAME")
    {
        return n;
    }
    if std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
        && let Some(r) = non_empty_env("RUNNER_NAME")
    {
        return r;
    }
    if std::env::var("GITLAB_CI").as_deref() == Ok("true")
        && let Some(r) = non_empty_env("CI_RUNNER_DESCRIPTION")
    {
        return r;
    }
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .or_else(|_| std::fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Walk a single directory root and collect file records into the output vectors.
#[allow(clippy::too_many_arguments)]
fn walk_root(
    root: &Path,
    config: &AppConfig,
    include_globs: Option<&GlobSet>,
    exclude_globs: Option<&GlobSet>,
    enabled_languages: Option<&BTreeSet<Language>>,
    seen_paths: &mut HashSet<PathBuf>,
    analyzed: &mut Vec<FileRecord>,
    skipped: &mut Vec<FileRecord>,
    warnings: &mut Vec<String>,
    cancel: Option<&AtomicBool>,
    progress: Option<&ProgressCounters>,
) -> Result<()> {
    let mut builder = WalkBuilder::new(root);
    builder
        .follow_links(config.discovery.follow_symlinks)
        .hidden(config.discovery.ignore_hidden_files)
        .ignore(config.discovery.honor_ignore_files)
        .parents(config.discovery.honor_ignore_files)
        .git_ignore(config.discovery.honor_ignore_files)
        .git_global(config.discovery.honor_ignore_files)
        .git_exclude(config.discovery.honor_ignore_files);

    let paths = collect_walk_paths(&builder, seen_paths, warnings);
    if paths.is_empty() {
        return Ok(());
    }

    if let Some(p) = progress {
        p.files_total.fetch_add(paths.len(), Ordering::Relaxed);
    }

    let chunk_results = run_parallel_analysis(
        &paths,
        root,
        config,
        include_globs,
        exclude_globs,
        enabled_languages,
        cancel,
        progress,
    )?;
    merge_chunk_results(chunk_results, analyzed, skipped, warnings)
}

fn collect_walk_paths(
    builder: &WalkBuilder,
    seen_paths: &mut HashSet<PathBuf>,
    warnings: &mut Vec<String>,
) -> Vec<PathBuf> {
    // build_parallel() walks the directory tree across multiple threads (work-stealing
    // internally), which is meaningfully faster for deeply nested repos with many directories.
    // We collect results via an MPSC channel so each walker thread sends without contention.
    let (tx, rx) = std::sync::mpsc::channel::<std::result::Result<PathBuf, String>>();

    builder.build_parallel().run(|| {
        let tx = tx.clone();
        Box::new(move |entry| {
            match entry {
                Err(e) => {
                    let _ = tx.send(Err(format!("discovery warning: {e}")));
                }
                Ok(e) => {
                    let path = e.into_path();
                    if !path.is_dir() {
                        let _ = tx.send(Ok(path));
                    }
                }
            }
            ignore::WalkState::Continue
        })
    });

    // Drop the sender that the outer scope holds; the per-thread clones were dropped when
    // run() returned (all threads finished). Dropping this last sender closes the channel.
    drop(tx);

    rx.into_iter()
        .filter_map(|msg| match msg {
            Ok(path) => {
                if seen_paths.insert(path.clone()) {
                    Some(path)
                } else {
                    None
                }
            }
            Err(warn) => {
                warnings.push(warn);
                None
            }
        })
        .collect()
}

/// Inner work loop executed by each analysis thread.
#[allow(clippy::too_many_arguments)]
fn worker_loop(
    paths: &[PathBuf],
    root: &Path,
    config: &AppConfig,
    include_globs: Option<&GlobSet>,
    exclude_globs: Option<&GlobSet>,
    enabled_languages: Option<&BTreeSet<Language>>,
    cancel: Option<&AtomicBool>,
    next_index: &AtomicUsize,
    files_done: Option<&AtomicUsize>,
) -> Vec<Result<Option<FileRecord>>> {
    let mut results = Vec::new();
    loop {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            results.push(Err(anyhow::anyhow!("analysis cancelled")));
            break;
        }
        let i = next_index.fetch_add(1, Ordering::Relaxed);
        if i >= paths.len() {
            break;
        }
        results.push(analyze_candidate_file(
            &paths[i],
            root,
            config,
            include_globs,
            exclude_globs,
            enabled_languages,
        ));
        if let Some(fd) = files_done {
            fd.fetch_add(1, Ordering::Relaxed);
        }
    }
    results
}

#[allow(clippy::too_many_arguments)]
fn run_parallel_analysis(
    paths: &[PathBuf],
    root: &Path,
    config: &AppConfig,
    include_globs: Option<&GlobSet>,
    exclude_globs: Option<&GlobSet>,
    enabled_languages: Option<&BTreeSet<Language>>,
    cancel: Option<&AtomicBool>,
    progress: Option<&ProgressCounters>,
) -> Result<Vec<Vec<Result<Option<FileRecord>>>>> {
    let thread_count = std::thread::available_parallelism().map_or(DEFAULT_ANALYSIS_THREADS, |n| {
        n.get().min(MAX_ANALYSIS_THREADS)
    });
    // Shared work-queue index: each thread atomically claims the next path to process.
    // This eliminates static-chunk load imbalance — threads that finish early immediately
    // pick up more work instead of sitting idle while one overloaded chunk finishes.
    let next_index = AtomicUsize::new(0);
    let files_done: Option<&AtomicUsize> = progress.map(|p| p.files_done.as_ref());

    std::thread::scope(|s| -> Result<Vec<Vec<Result<Option<FileRecord>>>>> {
        // IMPORTANT: collect ALL handles before joining any of them.
        // A lazy spawn-then-join chain would serialize threads one at a time.
        let mut handles = Vec::with_capacity(thread_count);
        for _ in 0..thread_count {
            handles.push(s.spawn(|| {
                worker_loop(
                    paths,
                    root,
                    config,
                    include_globs,
                    exclude_globs,
                    enabled_languages,
                    cancel,
                    &next_index,
                    files_done,
                )
            }));
        }
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .map_err(|_| anyhow::anyhow!("analysis thread panicked"))
            })
            .collect()
    })
}

fn merge_chunk_results(
    chunk_results: Vec<Vec<Result<Option<FileRecord>>>>,
    analyzed: &mut Vec<FileRecord>,
    skipped: &mut Vec<FileRecord>,
    warnings: &mut Vec<String>,
) -> Result<()> {
    for chunk in chunk_results {
        for result in chunk {
            if let Some(record) = result? {
                push_record(record, analyzed, skipped, warnings);
            }
        }
    }
    Ok(())
}

/// Label each analyzed file with its submodule and build per-submodule summaries.
fn process_submodules(config: &AppConfig, analyzed: &mut [FileRecord]) -> Vec<SubmoduleSummary> {
    let root = config.discovery.root_paths[0]
        .canonicalize()
        .unwrap_or_else(|_| config.discovery.root_paths[0].clone());
    let submodules = detect_submodules(&root);
    if submodules.is_empty() {
        return Vec::new();
    }

    for file in analyzed.iter_mut() {
        for (name, sub_path) in &submodules {
            let prefix = sub_path.to_string_lossy().replace('\\', "/");
            let rel = &file.relative_path;
            if rel == &prefix || rel.starts_with(&format!("{prefix}/")) {
                file.submodule = Some(name.clone());
                break;
            }
        }
    }

    build_submodule_summaries(analyzed, &submodules, &root)
}

/// Compute Basic COCOMO I cost estimate from total code SLOC.
#[allow(clippy::cast_precision_loss)] // COCOMO formula: line counts at f64 precision are sufficient
fn compute_cocomo(code_lines: u64, mode: CocomoMode) -> CocomoEstimate {
    let ksloc = code_lines as f64 / 1_000.0;
    let (a, b, c, d): (f64, f64, f64, f64) = match mode {
        CocomoMode::Organic => (2.4, 1.05, 2.5, 0.38),
        CocomoMode::SemiDetached => (3.0, 1.12, 2.5, 0.35),
        CocomoMode::Embedded => (3.6, 1.20, 2.5, 0.32),
    };
    let effort = a * ksloc.powf(b);
    let duration = c * effort.powf(d);
    let avg_staff = if duration > 0.0 {
        effort / duration
    } else {
        0.0
    };
    // Round to 2 decimal places for readability.
    CocomoEstimate {
        mode,
        ksloc: (ksloc * 100.0).round() / 100.0,
        effort_person_months: (effort * 100.0).round() / 100.0,
        duration_months: (duration * 100.0).round() / 100.0,
        avg_staff: (avg_staff * 100.0).round() / 100.0,
    }
}

/// Collect ULOC hashes across all analyzed files, compute ULOC and `DRYness`.
#[allow(clippy::cast_precision_loss)] // DRYness is a display percentage; f32 precision is adequate
fn compute_uloc(analyzed: &[FileRecord]) -> (u64, Option<f32>) {
    use std::collections::HashSet as StdHashSet;
    let mut unique: StdHashSet<u64> = StdHashSet::new();
    let mut total_code: u64 = 0;
    for record in analyzed {
        total_code += record.effective_counts.code_lines;
        for &hash in &record.raw_line_categories.code_line_hashes {
            unique.insert(hash);
        }
    }
    let uloc = unique.len() as u64;
    let dryness = if total_code > 0 {
        Some((uloc as f32 / total_code as f32) * 100.0)
    } else {
        None
    };
    (uloc, dryness)
}

/// Group files by content hash and return groups of duplicates (relative paths).
/// Only groups with ≥ 2 files are returned.
fn find_duplicate_groups(analyzed: &[FileRecord]) -> Vec<Vec<String>> {
    let mut by_hash: std::collections::HashMap<u64, Vec<&str>> = std::collections::HashMap::new();
    for record in analyzed {
        if record.content_hash != 0 {
            by_hash
                .entry(record.content_hash)
                .or_default()
                .push(&record.relative_path);
        }
    }
    let mut groups: Vec<Vec<String>> = by_hash
        .into_values()
        .filter(|v| v.len() >= 2)
        .map(|v| {
            let mut paths: Vec<String> = v.into_iter().map(str::to_owned).collect();
            paths.sort();
            paths
        })
        .collect();
    groups.sort_by(|a, b| a[0].cmp(&b[0]));
    groups
}

/// Assemble the final `AnalysisRun` from collected records and metadata.
fn assemble_run(
    config: &AppConfig,
    runtime_mode: &str,
    mut analyzed: Vec<FileRecord>,
    skipped: Vec<FileRecord>,
    warnings: Vec<String>,
    submodule_summaries: Vec<SubmoduleSummary>,
) -> AnalysisRun {
    let summary = build_summary(&analyzed, &skipped);
    let language_summaries = build_language_summaries(&analyzed);
    let col_threshold = config.analysis.style_col_threshold;
    let style_summary = build_style_summary(&analyzed, col_threshold);

    // Compute ULOC, DRYness, duplicates, and COCOMO from the aggregated records.
    let (uloc, dryness_pct) = compute_uloc(&analyzed);
    let duplicate_groups = find_duplicate_groups(&analyzed);
    let cocomo = if summary.code_lines > 0 {
        Some(compute_cocomo(summary.code_lines, CocomoMode::Organic))
    } else {
        None
    };

    let first_root = config
        .discovery
        .root_paths
        .first()
        .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()));
    let git = first_root
        .as_deref()
        .map(detect_git_for_run)
        .unwrap_or_default();

    // Per-file git activity for the hotspots view (on by default, single `git log` pass,
    // best-effort). A window of 0 (or None) disables it; a non-git path yields an empty result.
    let activity_window = config.analysis.activity_window_days.unwrap_or(0);
    if let (true, Some(root)) = (activity_window > 0, first_root.as_deref()) {
        let activity = detect_file_activity(root, activity_window);
        if !activity.is_empty() {
            for rec in &mut analyzed {
                if let Some((count, date)) = activity.get(&rec.relative_path) {
                    rec.commit_count = Some(*count);
                    rec.last_commit_date.clone_from(date);
                }
            }
        }
    }

    // Per-author code-ownership attribution (opt-in: one `git blame` per file). Best-effort —
    // a non-git path or blame failure yields an empty author list and leaves records untouched.
    let authors = if config.analysis.attribution {
        first_root
            .as_deref()
            .map(|root| attribute_ownership(root, &mut analyzed))
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let now = Utc::now();
    let run_id = {
        let uuid_suffix = Uuid::new_v4().simple().to_string();
        format!("{}-{}", now.format("%Y%m%d-%H%M"), uuid_suffix)
    };

    AnalysisRun {
        tool: ToolMetadata {
            name: "sloc".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            run_id,
            timestamp_utc: now,
        },
        environment: EnvironmentMetadata {
            operating_system: std::env::consts::OS.into(),
            architecture: std::env::consts::ARCH.into(),
            runtime_mode: runtime_mode.into(),
            initiator_username: get_current_username(),
            initiator_hostname: get_hostname(),
            ci_name: if is_jenkins_env() {
                Some(format!("Jenkins\t{}", get_hostname()))
            } else {
                detect_ci_system().map(str::to_string)
            },
        },
        effective_configuration: config.clone(),
        input_roots: config
            .discovery
            .root_paths
            .iter()
            .map(|p| path_to_string(p))
            .collect(),
        summary_totals: summary,
        totals_by_language: language_summaries,
        per_file_records: analyzed,
        skipped_file_records: skipped,
        warnings,
        submodule_summaries,
        git_commit_short: git.commit_short,
        git_commit_long: git.commit_long,
        git_branch: git.branch,
        git_commit_author: git.author,
        git_tags: git.tags,
        git_nearest_tag: git.nearest_tag,
        git_commit_date: git.commit_date,
        git_remote_url: git.remote_url,
        style_summary,
        cocomo,
        uloc,
        dryness_pct,
        duplicate_groups,
        duplicates_excluded: 0,
        authors,
    }
}

/// # Errors
///
/// Returns an error if the config is invalid, root paths cannot be walked, or any file
/// analysis step fails in a way that cannot be recovered from.
#[allow(clippy::too_many_lines)]
pub fn analyze(
    config: &AppConfig,
    runtime_mode: &str,
    cancel: Option<&AtomicBool>,
    progress: Option<&ProgressCounters>,
) -> Result<AnalysisRun> {
    config.validate()?;

    if config.discovery.root_paths.is_empty() {
        anyhow::bail!("no input paths were provided");
    }

    let include_globs = compile_globset(&config.discovery.include_globs)?;
    let exclude_globs = compile_globset(&config.discovery.exclude_globs)?;
    let enabled_languages = parse_enabled_languages(&config.analysis.enabled_languages)?;

    let mut analyzed = Vec::new();
    let mut skipped = Vec::new();
    let mut warnings = Vec::new();
    let mut seen_paths = HashSet::new();

    for root in &config.discovery.root_paths {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            anyhow::bail!("analysis cancelled");
        }

        let root = root.canonicalize().unwrap_or_else(|_| root.clone());

        if root.is_file() {
            if let Some(record) = analyze_candidate_file(
                &root,
                root.parent().unwrap_or_else(|| Path::new(".")),
                config,
                include_globs.as_ref(),
                exclude_globs.as_ref(),
                enabled_languages.as_ref(),
            )? {
                push_record(record, &mut analyzed, &mut skipped, &mut warnings);
            }
            continue;
        }

        let layout = detect_repository_layout(&root);
        if layout.has_multiple_repos() {
            warnings.push(format_multi_repo_warning(&layout));
        }

        walk_root(
            &root,
            config,
            include_globs.as_ref(),
            exclude_globs.as_ref(),
            enabled_languages.as_ref(),
            &mut seen_paths,
            &mut analyzed,
            &mut skipped,
            &mut warnings,
            cancel,
            progress,
        )?;
    }

    analyzed.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    skipped.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

    // Submodule detection: label each file with its submodule and build per-submodule summaries.
    let submodule_summaries = if config.discovery.submodule_breakdown {
        process_submodules(config, &mut analyzed)
    } else {
        Vec::new()
    };

    attach_coverage(config, &mut analyzed, &mut warnings);

    Ok(assemble_run(
        config,
        runtime_mode,
        analyzed,
        skipped,
        warnings,
        submodule_summaries,
    ))
}

fn attach_coverage(config: &AppConfig, analyzed: &mut [FileRecord], warnings: &mut Vec<String>) {
    let Some(cov_path) = coverage::resolve_coverage_file(config.analysis.coverage_file.as_deref())
    else {
        return;
    };
    tracing::debug!(path = %cov_path.display(), "loading coverage file");
    match fs::read_to_string(&cov_path) {
        Ok(content) => {
            let cov_map = coverage::parse_coverage_auto(&cov_path, &content);
            let mut matched: u32 = 0;
            let mut unmatched: u32 = 0;
            for record in analyzed.iter_mut() {
                record.coverage =
                    coverage::lookup_coverage(&cov_map, &record.relative_path).cloned();
                if record.coverage.is_some() {
                    matched += 1;
                } else {
                    unmatched += 1;
                }
            }
            tracing::debug!(
                path = %cov_path.display(),
                coverage_entries = cov_map.len(),
                files_matched = matched,
                files_unmatched = unmatched,
                "coverage attached"
            );
            if unmatched > 0 && matched == 0 {
                tracing::warn!(
                    path = %cov_path.display(),
                    "coverage file loaded but no source files could be matched — check that paths in the coverage report match the scanned directory"
                );
            }
        }
        Err(e) => {
            tracing::warn!(path = %cov_path.display(), error = %e, "coverage file could not be read");
            warnings.push(format!(
                "coverage file '{}' could not be read: {e}",
                cov_path.display()
            ));
        }
    }
}

fn push_record(
    record: FileRecord,
    analyzed: &mut Vec<FileRecord>,
    skipped: &mut Vec<FileRecord>,
    warnings: &mut Vec<String>,
) {
    warnings.extend(
        record
            .warnings
            .iter()
            .map(|warning| format!("{}: {warning}", record.relative_path)),
    );

    match record.status {
        FileStatus::AnalyzedExact | FileStatus::AnalyzedBestEffort => analyzed.push(record),
        _ => skipped.push(record),
    }
}

/// Convenience wrapper: build a boxed `Skip` outcome with a single-item warning message.
#[inline]
fn skip_with_reason(
    path: &Path,
    root: &Path,
    size: u64,
    reason: impl Into<String>,
) -> MetadataPolicyOutcome {
    MetadataPolicyOutcome::Skip(Box::new(skipped_record(
        path,
        root,
        size,
        FileStatus::SkippedByPolicy,
        vec![reason.into()],
    )))
}

/// Apply metadata-level policy checks (symlink, name, dir exclusion, size, globs, lockfile).
/// Returns `Skip(record)` to skip, `Exclude` to omit from output entirely (include-glob miss),
/// or `Continue` to proceed to content checks.
#[allow(clippy::too_many_arguments)]
fn check_metadata_policy(
    path: &Path,
    root: &Path,
    relative_path: &str,
    metadata: &fs::Metadata,
    config: &AppConfig,
    include_globs: Option<&GlobSet>,
    exclude_globs: Option<&GlobSet>,
) -> MetadataPolicyOutcome {
    let size = metadata.len();

    if metadata.file_type().is_symlink() && !config.discovery.follow_symlinks {
        return skip_with_reason(path, root, size, "symlink skipped by policy");
    }
    if file_name_eq(path, ".gitignore") {
        return skip_with_reason(path, root, size, ".gitignore is always excluded");
    }
    if is_excluded_dir_path(path, &config.discovery.excluded_directories) {
        return skip_with_reason(path, root, size, "path matched excluded directory setting");
    }
    if size > config.discovery.max_file_size_bytes {
        return skip_with_reason(
            path,
            root,
            size,
            format!(
                "file exceeded max_file_size_bytes ({})",
                config.discovery.max_file_size_bytes
            ),
        );
    }
    if let Some(globs) = include_globs
        && !globs.is_match(Path::new(relative_path))
        && !globs.is_match(path)
    {
        return MetadataPolicyOutcome::Exclude;
    }
    if let Some(globs) = exclude_globs
        && (globs.is_match(Path::new(relative_path)) || globs.is_match(path))
    {
        return skip_with_reason(path, root, size, "path matched exclude glob");
    }
    if is_known_lockfile(path) && !config.analysis.include_lockfiles {
        return skip_with_reason(path, root, size, "lockfile skipped by default policy");
    }

    MetadataPolicyOutcome::Continue
}

struct ContentPolicyResult {
    vendor: bool,
    generated: bool,
    minified: bool,
    skip_record: Option<FileRecord>,
}

/// Apply content-level policy checks (vendor, generated, minified).
/// `skip_record` is `Some` when the file should be skipped.
fn check_content_policy(
    path: &Path,
    root: &Path,
    size_bytes: u64,
    bytes: &[u8],
    config: &AppConfig,
) -> ContentPolicyResult {
    let vendor = is_vendor_path(path);
    if vendor && config.analysis.vendor_directory_detection {
        return ContentPolicyResult {
            vendor,
            generated: false,
            minified: false,
            skip_record: Some(skipped_record(
                path,
                root,
                size_bytes,
                FileStatus::SkippedByPolicy,
                vec!["vendor file skipped by policy".into()],
            )),
        };
    }

    let generated = config.analysis.generated_file_detection && looks_generated(path, bytes);
    if generated {
        return ContentPolicyResult {
            vendor,
            generated,
            minified: false,
            skip_record: Some(skipped_record(
                path,
                root,
                size_bytes,
                FileStatus::SkippedByPolicy,
                vec!["generated file skipped by policy".into()],
            )),
        };
    }

    let minified = config.analysis.minified_file_detection && looks_minified(path, bytes);
    if minified {
        return ContentPolicyResult {
            vendor,
            generated,
            minified,
            skip_record: Some(skipped_record(
                path,
                root,
                size_bytes,
                FileStatus::SkippedByPolicy,
                vec!["minified file skipped by policy".into()],
            )),
        };
    }

    ContentPolicyResult {
        vendor,
        generated,
        minified,
        skip_record: None,
    }
}

/// Decode file bytes to a UTF-8 string, handling binary detection and decode failures.
fn decode_file_contents(
    path: &Path,
    root: &Path,
    size_bytes: u64,
    bytes: &[u8],
    config: &AppConfig,
) -> Result<Option<(String, String, Vec<String>)>> {
    if is_binary(bytes) {
        return match config.analysis.binary_file_behavior {
            BinaryFileBehavior::Skip => Ok(None),
            BinaryFileBehavior::Fail => {
                anyhow::bail!("binary file encountered: {}", path.display())
            }
        };
    }

    match decode_bytes(bytes) {
        Ok(result) => Ok(Some(result)),
        Err(err) => match config.analysis.decode_failure_behavior {
            FailureBehavior::WarnSkip => {
                // Caller will handle the None as a SkippedDecodeError record.
                // We use a sentinel: return Ok(None) but encode the error into a field.
                // Instead, propagate as a skipped record via the caller.
                let _ = (path, root, size_bytes); // suppress unused warnings
                Err(anyhow::anyhow!("__decode_warn__: {err}"))
            }
            FailureBehavior::Fail => {
                anyhow::bail!("decode failure for {}: {err}", path.display())
            }
        },
    }
}

/// Result of resolving a candidate file's language: either a concrete language or a pre-built
/// skipped `FileRecord` (unsupported/undetected, or disabled by the enabled-languages policy).
enum LanguageOutcome {
    Resolved(Language),
    Skip(Box<FileRecord>),
}

/// Detect the file's language, apply the `.h` C→C++ reclassification, and enforce the
/// enabled-languages policy. Returns `LanguageOutcome::Skip` (with the ready-to-return record)
/// when the language is undetected/unsupported or disabled by configuration.
fn resolve_language(
    path: &Path,
    root: &Path,
    size_bytes: u64,
    text: &str,
    config: &AppConfig,
    enabled_languages: Option<&BTreeSet<Language>>,
) -> LanguageOutcome {
    let first_line = text.lines().next();
    let language = detect_language(
        path,
        first_line,
        &config.analysis.extension_overrides,
        config.analysis.shebang_detection,
    );

    let Some(mut language) = language else {
        return LanguageOutcome::Skip(Box::new(skipped_record(
            path,
            root,
            size_bytes,
            FileStatus::SkippedUnsupported,
            vec!["unsupported or undetected language".into()],
        )));
    };

    // The `.h` extension is ambiguous between C and C++; `detect_language` defaults it to C.
    // Reclassify as C++ when the file uses C++-only constructs (namespaces, classes, templates,
    // `std::`, …) so class/namespace and class-typed function signatures are counted correctly.
    if language == Language::C
        && path.extension().and_then(|e| e.to_str()) == Some("h")
        && sloc_languages::looks_like_cpp(text)
    {
        language = Language::Cpp;
    }

    if let Some(enabled) = enabled_languages
        && !enabled.contains(&language)
    {
        return LanguageOutcome::Skip(Box::new(skipped_record(
            path,
            root,
            size_bytes,
            FileStatus::SkippedByPolicy,
            vec![format!(
                "language {} disabled by configuration",
                language.display_name()
            )],
        )));
    }

    LanguageOutcome::Resolved(language)
}

#[allow(clippy::too_many_lines)]
fn analyze_candidate_file(
    path: &Path,
    root: &Path,
    config: &AppConfig,
    include_globs: Option<&GlobSet>,
    exclude_globs: Option<&GlobSet>,
    enabled_languages: Option<&BTreeSet<Language>>,
) -> Result<Option<FileRecord>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) => {
            return Ok(Some(skipped_record(
                path,
                root,
                0,
                FileStatus::ErrorInternal,
                vec![format!("failed to read metadata: {err}")],
            )));
        }
    };

    let relative_path = relative_path_string(path, root);

    // Metadata-level policy checks.
    match check_metadata_policy(
        path,
        root,
        &relative_path,
        &metadata,
        config,
        include_globs,
        exclude_globs,
    ) {
        MetadataPolicyOutcome::Skip(record) => return Ok(Some(*record)),
        MetadataPolicyOutcome::Exclude => return Ok(None),
        MetadataPolicyOutcome::Continue => {}
    }

    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            return Ok(Some(skipped_record(
                path,
                root,
                metadata.len(),
                FileStatus::ErrorInternal,
                vec![format!("failed to read file: {err}")],
            )));
        }
    };

    // Content-level policy checks (vendor, generated, minified).
    let content_policy = check_content_policy(path, root, metadata.len(), &bytes, config);
    if let Some(record) = content_policy.skip_record {
        return Ok(Some(record));
    }
    let (vendor, generated, minified) = (
        content_policy.vendor,
        content_policy.generated,
        content_policy.minified,
    );

    // Decode content, handling binary and decode failures.
    let (text, encoding, decode_warnings) =
        match decode_file_contents(path, root, metadata.len(), &bytes, config) {
            Ok(Some(result)) => result,
            Ok(None) => {
                return Ok(Some(skipped_record(
                    path,
                    root,
                    metadata.len(),
                    FileStatus::SkippedBinary,
                    vec!["binary file skipped by default".into()],
                )));
            }
            Err(err) => {
                let msg = err.to_string();
                if let Some(warn_msg) = msg.strip_prefix("__decode_warn__: ") {
                    return Ok(Some(skipped_record(
                        path,
                        root,
                        metadata.len(),
                        FileStatus::SkippedDecodeError,
                        vec![warn_msg.to_string()],
                    )));
                }
                return Err(err);
            }
        };

    let language =
        match resolve_language(path, root, metadata.len(), &text, config, enabled_languages) {
            LanguageOutcome::Resolved(language) => language,
            LanguageOutcome::Skip(record) => return Ok(Some(*record)),
        };

    let style_scope = match config.analysis.style_lang_scope.as_str() {
        "c_family" => StyleLangScope::CFamilyOnly,
        _ => StyleLangScope::All,
    };
    let ieee_opts = AnalysisOptions {
        blank_in_block_comment_as_comment: config.analysis.blank_in_block_comment_policy
            == BlankInBlockCommentPolicy::CountAsComment,
        collapse_continuation_lines: config.analysis.continuation_line_policy
            == ContinuationLinePolicy::CollapseToLogical,
        enable_style: config.analysis.style_analysis_enabled,
        style_lang_scope: style_scope,
    };
    let analysis = analyze_text(language, &text, ieee_opts);
    let effective_counts = compute_effective_counts(
        &analysis.raw,
        config.analysis.mixed_line_policy,
        config.analysis.python_docstrings_as_comments,
        config.analysis.count_compiler_directives,
    );

    let mut warnings = decode_warnings;
    warnings.extend(analysis.warnings.clone());

    // Compute a fast 64-bit content fingerprint for duplicate-file detection.
    let content_hash = {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut h = DefaultHasher::new();
        bytes.hash(&mut h);
        h.finish()
    };

    // Extract fields from analysis.raw before it is moved into FileRecord.
    let cyclomatic_complexity = if analysis.raw.cyclomatic_complexity > 0 {
        Some(analysis.raw.cyclomatic_complexity)
    } else {
        None
    };
    let lsloc = analysis.raw.lsloc;

    Ok(Some(FileRecord {
        path: path_to_string(path),
        relative_path,
        language: Some(language),
        size_bytes: metadata.len(),
        detected_encoding: Some(encoding),
        raw_line_categories: analysis.raw,
        effective_counts,
        status: match analysis.parse_mode {
            ParseMode::Lexical | ParseMode::TreeSitter => FileStatus::AnalyzedExact,
            ParseMode::LexicalBestEffort => FileStatus::AnalyzedBestEffort,
        },
        warnings,
        generated,
        minified,
        vendor,
        parse_mode: Some(analysis.parse_mode),
        submodule: None,
        coverage: None,
        style_analysis: analysis.style_analysis,
        cyclomatic_complexity,
        lsloc,
        commit_count: None,
        last_commit_date: None,
        ownership: None,
        content_hash,
    }))
}

const fn compute_effective_counts(
    raw: &RawLineCounts,
    mixed_line_policy: MixedLinePolicy,
    python_docstrings_as_comments: bool,
    count_compiler_directives: bool,
) -> EffectiveCounts {
    let mut effective = EffectiveCounts {
        code_lines: raw.code_only_lines,
        comment_lines: raw.single_comment_only_lines + raw.multi_comment_only_lines,
        blank_lines: raw.blank_only_lines,
        mixed_lines_separate: 0,
    };

    if python_docstrings_as_comments {
        effective.comment_lines += raw.docstring_comment_lines;
    } else {
        effective.code_lines += raw.docstring_comment_lines;
    }

    let mixed_total = raw.mixed_code_single_comment_lines + raw.mixed_code_multi_comment_lines;
    match mixed_line_policy {
        MixedLinePolicy::CodeOnly => effective.code_lines += mixed_total,
        MixedLinePolicy::CodeAndComment => {
            effective.code_lines += mixed_total;
            effective.comment_lines += mixed_total;
        }
        MixedLinePolicy::CommentOnly => effective.comment_lines += mixed_total,
        MixedLinePolicy::SeparateMixedCategory => effective.mixed_lines_separate += mixed_total,
    }

    // IEEE 1045-1992 §4.2: optionally exclude preprocessor/compiler directives from code SLOC.
    // compiler_directive_lines is a subset of code_only_lines, so subtract it directly.
    if !count_compiler_directives {
        effective.code_lines = effective
            .code_lines
            .saturating_sub(raw.compiler_directive_lines);
    }

    effective
}

fn build_summary(analyzed: &[FileRecord], skipped: &[FileRecord]) -> SummaryTotals {
    let mut summary = SummaryTotals {
        files_considered: (analyzed.len() + skipped.len()) as u64,
        files_analyzed: analyzed.len() as u64,
        files_skipped: skipped.len() as u64,
        ..Default::default()
    };

    for record in analyzed {
        summary.total_physical_lines += record.raw_line_categories.total_physical_lines;
        summary.code_lines += record.effective_counts.code_lines;
        summary.comment_lines += record.effective_counts.comment_lines;
        summary.blank_lines += record.effective_counts.blank_lines;
        summary.mixed_lines_separate += record.effective_counts.mixed_lines_separate;
        summary.functions += record.raw_line_categories.functions;
        summary.classes += record.raw_line_categories.classes;
        summary.variables += record.raw_line_categories.variables;
        summary.variables_member += record.raw_line_categories.variables_member;
        summary.variables_local += record.raw_line_categories.variables_local;
        summary.variables_global += record.raw_line_categories.variables_global;
        summary.macro_definitions += record.raw_line_categories.macro_definitions;
        summary.imports += record.raw_line_categories.imports;
        summary.test_count += record.raw_line_categories.test_count;
        summary.test_assertion_count += record.raw_line_categories.test_assertion_count;
        summary.test_suite_count += record.raw_line_categories.test_suite_count;
        summary.cyclomatic_complexity +=
            u64::from(record.raw_line_categories.cyclomatic_complexity);
        if let Some(lsloc) = record.raw_line_categories.lsloc {
            *summary.lsloc.get_or_insert(0) += u64::from(lsloc);
        }
        if let Some(cov) = &record.coverage {
            summary.coverage_lines_found += u64::from(cov.lines_found);
            summary.coverage_lines_hit += u64::from(cov.lines_hit);
            summary.coverage_functions_found += u64::from(cov.functions_found);
            summary.coverage_functions_hit += u64::from(cov.functions_hit);
            summary.coverage_branches_found += u64::from(cov.branches_found);
            summary.coverage_branches_hit += u64::from(cov.branches_hit);
        }
    }

    summary
}

/// Construct a zero-filled `LanguageSummary` for the given language.
const fn zeroed_summary(language: Language) -> LanguageSummary {
    LanguageSummary {
        language,
        files: 0,
        total_physical_lines: 0,
        code_lines: 0,
        comment_lines: 0,
        blank_lines: 0,
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

/// Accumulate all per-file counters from `record` into an existing `LanguageSummary`.
fn accumulate_record_into_summary(entry: &mut LanguageSummary, record: &FileRecord) {
    entry.files += 1;
    let r = &record.raw_line_categories;
    entry.total_physical_lines += r.total_physical_lines;
    entry.code_lines += record.effective_counts.code_lines;
    entry.comment_lines += record.effective_counts.comment_lines;
    entry.blank_lines += record.effective_counts.blank_lines;
    entry.mixed_lines_separate += record.effective_counts.mixed_lines_separate;
    entry.functions += r.functions;
    entry.classes += r.classes;
    entry.variables += r.variables;
    entry.variables_member += r.variables_member;
    entry.variables_local += r.variables_local;
    entry.variables_global += r.variables_global;
    entry.macro_definitions += r.macro_definitions;
    entry.imports += r.imports;
    entry.test_count += r.test_count;
    entry.test_assertion_count += r.test_assertion_count;
    entry.test_suite_count += r.test_suite_count;
    entry.cyclomatic_complexity += u64::from(r.cyclomatic_complexity);
    if let Some(lsloc) = r.lsloc {
        *entry.lsloc.get_or_insert(0) += u64::from(lsloc);
    }
    if let Some(cov) = &record.coverage {
        entry.coverage_lines_found += u64::from(cov.lines_found);
        entry.coverage_lines_hit += u64::from(cov.lines_hit);
        entry.coverage_functions_found += u64::from(cov.functions_found);
        entry.coverage_functions_hit += u64::from(cov.functions_hit);
        entry.coverage_branches_found += u64::from(cov.branches_found);
        entry.coverage_branches_hit += u64::from(cov.branches_hit);
    }
}

fn build_language_summaries(analyzed: &[FileRecord]) -> Vec<LanguageSummary> {
    let mut by_language: BTreeMap<Language, LanguageSummary> = BTreeMap::new();
    for record in analyzed {
        let Some(language) = record.language else {
            continue;
        };
        let entry = by_language
            .entry(language)
            .or_insert_with(|| zeroed_summary(language));
        accumulate_record_into_summary(entry, record);
    }
    by_language.into_values().collect()
}

fn skipped_record(
    path: &Path,
    root: &Path,
    size_bytes: u64,
    status: FileStatus,
    warnings: Vec<String>,
) -> FileRecord {
    FileRecord {
        path: path_to_string(path),
        relative_path: relative_path_string(path, root),
        language: None,
        size_bytes,
        detected_encoding: None,
        raw_line_categories: RawLineCounts::default(),
        effective_counts: EffectiveCounts::default(),
        status,
        warnings,
        generated: false,
        minified: false,
        vendor: false,
        parse_mode: None,
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

/// Normalize a raw lossy path string: strip any Windows verbatim / extended-length prefix,
/// then convert backslashes to forward slashes.
///
/// `std::fs::canonicalize()` on Windows returns extended-length paths that begin with
/// `\\?\` (or `\\?\UNC\` for UNC shares). Once backslashes are normalized to forward
/// slashes this would otherwise leak into report output as `//?/C:/...`, which looks
/// broken. This operates purely on the raw string, so it behaves identically on every
/// platform (the tests run on Linux too).
///
/// - `\\?\C:\foo`            -> `C:/foo`
/// - `\\?\UNC\server\share`  -> `//server/share`
/// - anything else           -> backslashes replaced with slashes, otherwise unchanged
fn normalize_path_str(raw: &str) -> String {
    if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
        // `\\?\UNC\server\share\...` denotes `\\server\share\...` -> `//server/share/...`
        format!("//{}", unc.replace('\\', "/"))
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        rest.replace('\\', "/")
    } else {
        raw.replace('\\', "/")
    }
}

fn relative_path_string(path: &Path, root: &Path) -> String {
    normalize_path_str(&path.strip_prefix(root).unwrap_or(path).to_string_lossy())
}

fn path_to_string(path: &Path) -> String {
    normalize_path_str(&path.to_string_lossy())
}

/// Summary of the git-repository shape under a selected scan root.
///
/// Used to warn when a user points oxide-sloc at a folder that holds several
/// *independent* repositories (e.g. a `projects/` directory of separate clones),
/// which silently conflates unrelated codebases and their git metrics. A single
/// repository that contains git *submodules* is legitimate and does not count as
/// "multiple repos".
#[derive(Debug, Clone, Default)]
pub struct RepositoryLayout {
    /// The scan root this layout was computed for.
    pub root: PathBuf,
    /// `true` when the root directory is itself the top of a git repository.
    pub root_is_repo: bool,
    /// Submodule paths declared in the root's `.gitmodules`, relative to `root`.
    pub submodule_paths: Vec<PathBuf>,
    /// Independent (non-submodule) repositories found beneath `root`, relative to `root`.
    pub nested_repos: Vec<PathBuf>,
}

impl RepositoryLayout {
    /// `true` when the selection spans more than one independent repository.
    ///
    /// If the root is itself a repo, any nested non-submodule repo is a foreign
    /// checkout vendored inside it. If the root is not a repo, two or more child
    /// repos means the user picked a parent-of-repos folder.
    #[must_use]
    pub const fn has_multiple_repos(&self) -> bool {
        if self.root_is_repo {
            !self.nested_repos.is_empty()
        } else {
            self.nested_repos.len() >= 2
        }
    }
}

/// Depth (below the root) at which the nested-repo scan stops descending.
const REPO_SCAN_MAX_DEPTH: usize = 6;
/// Upper bound on directories visited by the nested-repo scan; a partial result
/// is acceptable for a best-effort caution.
const REPO_SCAN_MAX_DIRS: usize = 4000;

/// Inspect `root` for independent git repositories nested beneath it.
///
/// Performs a bounded, prune-on-first-`.git` walk: once a directory is found to
/// be a repository (or a declared submodule) the scan does not descend into it,
/// so a repo's own submodules never register as independent repos. Best-effort
/// and infallible — IO errors are swallowed and simply yield a smaller result.
#[must_use]
pub fn detect_repository_layout(root: &Path) -> RepositoryLayout {
    let mut layout = RepositoryLayout {
        root: root.to_path_buf(),
        root_is_repo: is_git_root(root),
        submodule_paths: detect_submodules(root)
            .into_iter()
            .map(|(_, path)| path)
            .collect(),
        nested_repos: Vec::new(),
    };

    // Absolute paths of declared submodules, so we can prune them from the walk.
    let submodule_dirs: HashSet<PathBuf> = layout
        .submodule_paths
        .iter()
        .map(|rel| root.join(rel))
        .collect();

    // Stack of (dir, depth) to visit; start with the root's children (depth 1).
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    let mut visited = 0usize;

    while let Some((dir, depth)) = stack.pop() {
        if visited >= REPO_SCAN_MAX_DIRS {
            break;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let child = entry.path();
            // Skip non-directories and any `.git` directory without counting them.
            if !child.is_dir() || child.file_name().and_then(|n| n.to_str()) == Some(".git") {
                continue;
            }
            visited += 1;
            match classify_child(&child, &submodule_dirs, root) {
                ChildAction::RecordRepo(rel) => layout.nested_repos.push(rel),
                ChildAction::Recurse if depth + 1 < REPO_SCAN_MAX_DEPTH => {
                    stack.push((child, depth + 1));
                }
                ChildAction::Skip | ChildAction::Recurse => {}
            }
        }
    }

    layout.nested_repos.sort();
    layout
}

/// What the nested-repo walk should do with one candidate child directory.
enum ChildAction {
    /// Prune without recording (a declared submodule — not an independent repo).
    Skip,
    /// A nested independent repository at this root-relative path; record it and do not recurse.
    RecordRepo(PathBuf),
    /// Not a repo boundary — descend into it (subject to the depth bound).
    Recurse,
}

/// Classify one child directory during the nested-repo walk. Callers have already excluded
/// non-directories and `.git` directories.
fn classify_child(child: &Path, submodule_dirs: &HashSet<PathBuf>, root: &Path) -> ChildAction {
    if submodule_dirs.contains(child) {
        ChildAction::Skip
    } else if is_git_root(child) {
        ChildAction::RecordRepo(relative_path_buf(child, root))
    } else {
        ChildAction::Recurse
    }
}

/// Path of `path` relative to `root` (falling back to `path` itself), as a `PathBuf`.
fn relative_path_buf(path: &Path, root: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

/// Build the human-readable warning for a multi-repository selection.
fn format_multi_repo_warning(layout: &RepositoryLayout) -> String {
    const MAX_LISTED: usize = 5;
    let total = layout.nested_repos.len();
    let listed: Vec<String> = layout
        .nested_repos
        .iter()
        .take(MAX_LISTED)
        .map(|p| path_to_string(p))
        .collect();
    let mut joined = listed.join(", ");
    if total > MAX_LISTED {
        use std::fmt::Write as _;
        let _ = write!(joined, ", … and {} more", total - MAX_LISTED);
    }
    if layout.root_is_repo {
        format!(
            "This repository contains {total} nested git {} ({joined}) that are not registered \
             submodules. Their files are being counted as part of this project; if that is not \
             intended, exclude them or scan each repository separately.",
            if total == 1 {
                "repository"
            } else {
                "repositories"
            }
        )
    } else {
        format!(
            "The selected folder contains {total} independent git repositories ({joined}). \
             oxide-sloc analyzes one repository at a time — git metrics and totals are only \
             meaningful when the root is a single repository. Select one repository as the root \
             (submodules are fine).",
        )
    }
}

/// Parse `.gitmodules` in `root` and return `(name, relative_path)` for each submodule found.
#[must_use]
pub fn detect_submodules(root: &Path) -> Vec<(String, PathBuf)> {
    let gitmodules = root.join(".gitmodules");
    if !gitmodules.is_file() {
        return Vec::new();
    }
    let Ok(content) = fs::read_to_string(&gitmodules) else {
        return Vec::new();
    };

    let mut result = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_path: Option<PathBuf> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("[submodule \"") && trimmed.ends_with("\"]") {
            if let (Some(name), Some(path)) = (current_name.take(), current_path.take()) {
                result.push((name, path));
            }
            let name = trimmed["[submodule \"".len()..trimmed.len() - 2].to_string();
            current_name = Some(name);
        } else if let Some(rest) = trimmed.strip_prefix("path")
            && let Some(eq_pos) = rest.find('=')
        {
            let path_str = rest[eq_pos + 1..].trim();
            current_path = Some(PathBuf::from(path_str));
        }
    }
    if let (Some(name), Some(path)) = (current_name, current_path) {
        result.push((name, path));
    }

    result
}

fn build_submodule_summaries(
    analyzed: &[FileRecord],
    submodules: &[(String, PathBuf)],
    root: &Path,
) -> Vec<SubmoduleSummary> {
    submodules
        .iter()
        .map(|(name, path)| {
            let files: Vec<&FileRecord> = analyzed
                .iter()
                .filter(|f| f.submodule.as_deref() == Some(name.as_str()))
                .collect();

            let files_analyzed = files.len() as u64;
            let total_physical_lines = files
                .iter()
                .map(|f| f.raw_line_categories.total_physical_lines)
                .sum();
            let code_lines = files.iter().map(|f| f.effective_counts.code_lines).sum();
            let comment_lines = files.iter().map(|f| f.effective_counts.comment_lines).sum();
            let blank_lines = files.iter().map(|f| f.effective_counts.blank_lines).sum();
            let language_summaries = build_language_summaries_from_slice(&files);

            let git = detect_git_for_run(&root.join(path));

            SubmoduleSummary {
                name: name.clone(),
                relative_path: path.to_string_lossy().replace('\\', "/"),
                files_analyzed,
                total_physical_lines,
                code_lines,
                comment_lines,
                blank_lines,
                language_summaries,
                git_commit_short: git.commit_short,
                git_commit_long: git.commit_long,
                git_branch: git.branch,
                git_commit_author: git.author,
                git_commit_date: git.commit_date,
                git_remote_url: git.remote_url,
            }
        })
        .filter(|s| s.files_analyzed > 0)
        .collect()
}

/// Dominant indent label from vote counts.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn dominant_indent_label(files: &[&StyleAnalysis]) -> String {
    let mut votes = [0u32; 6];
    for f in files {
        let idx = match f.indent_style {
            IndentStyle::Tabs => 0,
            IndentStyle::Spaces2 => 1,
            IndentStyle::Spaces4 => 2,
            IndentStyle::Spaces8 => 3,
            IndentStyle::Mixed => 4,
            IndentStyle::Unknown => 5,
        };
        votes[idx] += 1;
    }
    let labels = ["Tabs", "2-Space", "4-Space", "8-Space", "Mixed", "\u{2014}"];
    labels[votes
        .iter()
        .enumerate()
        .max_by_key(|(_, v)| *v)
        .map_or(5, |(i, _)| i)]
    .to_string()
}

/// Line-80 compliance percentage for a slice of style analyses.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn line80_pct(files: &[&StyleAnalysis]) -> u8 {
    if files.is_empty() {
        return 0;
    }
    let compliant = files
        .iter()
        .filter(|f| f.total_lines == 0 || (f.lines_over_80 as f32 / f.total_lines as f32) <= 0.05)
        .count() as u32;
    ((compliant * 100) / files.len() as u32) as u8
}

/// Column-N compliance percentage using the configured threshold (80, 100, or 120).
/// Falls back to the 80-col bucket for any threshold ≤ 80.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn line_col_pct(files: &[&StyleAnalysis], threshold: u16) -> u8 {
    if files.is_empty() {
        return 0;
    }
    let compliant = files
        .iter()
        .filter(|f| {
            let over = if threshold <= 80 {
                f.lines_over_80
            } else if threshold <= 100 {
                f.lines_over_100
            } else {
                f.lines_over_120
            };
            f.total_lines == 0 || (over as f32 / f.total_lines as f32) <= 0.05
        })
        .count() as u32;
    ((compliant * 100) / files.len() as u32) as u8
}

/// Build a `LanguageStyleGroup` from a non-empty slice of `StyleAnalysis` for one family.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn build_language_group(
    family: &str,
    files: &[&StyleAnalysis],
    col_threshold: u16,
) -> LanguageStyleGroup {
    let count = files.len() as u32;

    // Collect every unique guide name across all files in this group.
    let mut all_names: Vec<String> = Vec::new();
    for f in files {
        for g in &f.guide_scores {
            if !all_names.contains(&g.name) {
                all_names.push(g.name.clone());
            }
        }
    }

    let mut guide_avg_scores: Vec<(String, u8)> = all_names
        .into_iter()
        .map(|name| {
            let sum: u32 = files
                .iter()
                .filter_map(|f| f.guide_scores.iter().find(|g| g.name == name))
                .map(|g| u32::from(g.score_pct))
                .sum();
            let avg = (sum / count) as u8;
            (name, avg)
        })
        .collect();
    guide_avg_scores.sort_by_key(|s| std::cmp::Reverse(s.1));

    let (dominant_guide, dominant_score_pct) = guide_avg_scores
        .first()
        .map(|(n, s)| (n.clone(), *s))
        .unwrap_or_default();

    let lcp = line_col_pct(files, col_threshold);
    LanguageStyleGroup {
        language_family: family.to_string(),
        files_count: count,
        dominant_guide,
        dominant_score_pct,
        common_indent_style: dominant_indent_label(files),
        guide_avg_scores,
        line80_compliant_pct: line80_pct(files),
        line_col_compliant_pct: lcp,
    }
}

/// Build aggregate multi-language style-guide adherence.
/// Returns `None` when no files had style data.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn build_style_summary(analyzed: &[FileRecord], col_threshold: u16) -> Option<StyleSummary> {
    let all_style: Vec<&StyleAnalysis> = analyzed
        .iter()
        .filter_map(|f| f.style_analysis.as_ref())
        .collect();

    if all_style.is_empty() {
        return None;
    }

    // Group by language_family.
    let mut families: std::collections::BTreeMap<&str, Vec<&StyleAnalysis>> =
        std::collections::BTreeMap::new();
    for sa in &all_style {
        families
            .entry(sa.language_family.as_str())
            .or_default()
            .push(sa);
    }

    let mut by_language: Vec<LanguageStyleGroup> = families
        .iter()
        .map(|(family, files)| build_language_group(family, files, col_threshold))
        .collect();
    by_language.sort_by_key(|g| std::cmp::Reverse(g.files_count));

    let files_analyzed = all_style.len() as u32;
    let common_indent_style = dominant_indent_label(&all_style);
    let line80_compliant_pct = line80_pct(&all_style);
    let line_col_compliant_pct = line_col_pct(&all_style, col_threshold);

    Some(StyleSummary {
        files_analyzed,
        common_indent_style,
        line80_compliant_pct,
        line_col_compliant_pct,
        col_threshold,
        by_language,
    })
}

fn build_language_summaries_from_slice(files: &[&FileRecord]) -> Vec<LanguageSummary> {
    let mut map: BTreeMap<String, LanguageSummary> = BTreeMap::new();
    for file in files {
        let Some(lang) = file.language else { continue };
        let entry = map
            .entry(lang.display_name().to_string())
            .or_insert_with(|| zeroed_summary(lang));
        accumulate_record_into_summary(entry, file);
    }
    map.into_values().collect()
}

fn file_name_eq(path: &Path, expected: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == expected)
}

fn is_excluded_dir_path(path: &Path, excluded_dirs: &[String]) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|part| excluded_dirs.iter().any(|excluded| excluded == part))
    })
}

fn is_vendor_path(path: &Path) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|part| matches!(part, "vendor" | "node_modules" | "packages"))
    })
}

fn is_known_lockfile(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name,
                "Cargo.lock"
                    | "package-lock.json"
                    | "yarn.lock"
                    | "pnpm-lock.yaml"
                    | "Pipfile.lock"
                    | "poetry.lock"
                    | "composer.lock"
            )
        })
}

fn looks_generated(path: &Path, bytes: &[u8]) -> bool {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if file_name.contains(".generated.") || file_name.contains(".g.") {
        return true;
    }

    let sample = String::from_utf8_lossy(&bytes[..bytes.len().min(GENERATED_SAMPLE_BYTES)])
        .to_ascii_lowercase();
    sample.contains("@generated") || sample.contains("generated by")
}

fn looks_minified(path: &Path, bytes: &[u8]) -> bool {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if file_name.contains(".min.") {
        return true;
    }

    let sample = String::from_utf8_lossy(&bytes[..bytes.len().min(MINIFIED_SAMPLE_BYTES)]);
    let longest_line = sample.lines().map(str::len).max().unwrap_or(0);
    let whitespace = sample.chars().filter(|c| c.is_whitespace()).count();
    longest_line > MINIFIED_LINE_THRESHOLD && whitespace * 100 < sample.len().max(1)
}

fn is_binary(bytes: &[u8]) -> bool {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF])
        || bytes.starts_with(&[0xFF, 0xFE])
        || bytes.starts_with(&[0xFE, 0xFF])
    {
        return false;
    }

    let sample = &bytes[..bytes.len().min(BINARY_SAMPLE_BYTES)];
    sample.contains(&0)
}

/// Decode a BOM-stripped UTF-16 byte slice using the given encoding.
/// Returns `(text, encoding_label, warnings)`.
fn decode_utf16_bom(
    bom_stripped: &[u8],
    encoding: &'static encoding_rs::Encoding,
    label: &str,
) -> (String, String, Vec<String>) {
    let (cow, _, had_errors) = encoding.decode(bom_stripped);
    let mut warnings = Vec::new();
    if had_errors {
        warnings.push(format!("{label} decode contained replacement characters"));
    }
    (cow.into_owned(), label.into(), warnings)
}

fn decode_bytes(bytes: &[u8]) -> std::result::Result<(String, String, Vec<String>), String> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        let text = String::from_utf8(bytes[3..].to_vec()).map_err(|err| err.to_string())?;
        return Ok((text, "utf-8-bom".into(), vec![]));
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return Ok(decode_utf16_bom(&bytes[2..], UTF_16LE, "utf-16le"));
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return Ok(decode_utf16_bom(&bytes[2..], UTF_16BE, "utf-16be"));
    }

    // Multiple statements in the else branch make map_or_else awkward here.
    #[allow(clippy::option_if_let_else)]
    if let Ok(text) = String::from_utf8(bytes.to_vec()) {
        Ok((text, "utf-8".into(), vec![]))
    } else {
        let (cow, _, had_errors) = WINDOWS_1252.decode(bytes);
        let mut warnings = vec!["decoded using windows-1252 fallback".into()];
        if had_errors {
            warnings.push("fallback decode contained replacement characters".into());
        }
        Ok((cow.into_owned(), "windows-1252".into(), warnings))
    }
}

fn compile_globset(patterns: &[String]) -> Result<Option<GlobSet>> {
    if patterns.is_empty() {
        return Ok(None);
    }

    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder
            .add(Glob::new(pattern).with_context(|| format!("invalid glob pattern: {pattern}"))?);
    }
    Ok(Some(
        builder.build().context("failed to compile glob filters")?,
    ))
}

fn parse_enabled_languages(enabled: &[String]) -> Result<Option<BTreeSet<Language>>> {
    if enabled.is_empty() {
        return Ok(None);
    }

    let supported = supported_languages();
    let mut set = BTreeSet::new();
    for name in enabled {
        let language = Language::from_name(name)
            .with_context(|| format!("unsupported language in config: {name}"))?;
        if !supported.contains(&language) {
            anyhow::bail!("language {name} is not supported in this build");
        }
        set.insert(language);
    }
    Ok(Some(set))
}

/// # Errors
///
/// Returns an error if serialization fails or the output file cannot be written.
pub fn write_json(run: &AnalysisRun, output_path: &Path) -> Result<()> {
    let json = serde_json::to_string_pretty(run).context("failed to serialize analysis run")?;
    fs::write(output_path, json)
        .with_context(|| format!("failed to write JSON output to {}", output_path.display()))
}

/// # Errors
///
/// Returns an error if the file cannot be read or the JSON cannot be parsed.
pub fn read_json(path: &Path) -> Result<AnalysisRun> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read result file {}", path.display()))?;
    serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse JSON result {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_path_str_strips_verbatim_drive_prefix() {
        assert_eq!(
            normalize_path_str(r"\\?\C:\jenkins-agent\repo\CMakeLists.txt"),
            "C:/jenkins-agent/repo/CMakeLists.txt"
        );
    }

    #[test]
    fn normalize_path_str_strips_verbatim_unc_prefix() {
        assert_eq!(
            normalize_path_str(r"\\?\UNC\server\share\proj\main.rs"),
            "//server/share/proj/main.rs"
        );
    }

    #[test]
    fn normalize_path_str_leaves_plain_paths_unchanged() {
        // Relative path with backslashes -> only slash normalization applies.
        assert_eq!(normalize_path_str(r"src\foo\bar.rs"), "src/foo/bar.rs");
        // Already-forward-slash path is untouched.
        assert_eq!(normalize_path_str("src/foo/bar.rs"), "src/foo/bar.rs");
        // Plain absolute drive path (no verbatim prefix) is untouched except slashes.
        assert_eq!(normalize_path_str(r"C:\foo\bar.rs"), "C:/foo/bar.rs");
    }

    #[test]
    fn effective_counts_respect_code_only_policy() {
        let raw = RawLineCounts {
            code_only_lines: 2,
            single_comment_only_lines: 1,
            mixed_code_single_comment_lines: 3,
            docstring_comment_lines: 2,
            ..RawLineCounts::default()
        };
        let counts = compute_effective_counts(&raw, MixedLinePolicy::CodeOnly, true, true);
        assert_eq!(counts.code_lines, 5);
        assert_eq!(counts.comment_lines, 3);
    }

    #[test]
    fn effective_counts_can_separate_mixed() {
        let raw = RawLineCounts {
            mixed_code_single_comment_lines: 2,
            mixed_code_multi_comment_lines: 1,
            ..RawLineCounts::default()
        };
        let counts =
            compute_effective_counts(&raw, MixedLinePolicy::SeparateMixedCategory, true, true);
        assert_eq!(counts.mixed_lines_separate, 3);
        assert_eq!(counts.code_lines, 0);
        assert_eq!(counts.comment_lines, 0);
    }

    #[test]
    fn windows_1252_fallback_decodes() {
        let bytes = vec![0x48, 0x65, 0x6c, 0x6c, 0x6f, 0x96, 0x57];
        let (text, encoding, warnings) = decode_bytes(&bytes).unwrap();
        assert_eq!(encoding, "windows-1252");
        // 0x96 in windows-1252 decodes to U+2013; assert via escape to keep the source ASCII.
        assert!(text.contains('\u{2013}'));
        assert!(!warnings.is_empty());
    }

    // ── Pure predicate tests ─────────────────────────────────────────────────

    #[test]
    fn is_binary_detects_null_byte() {
        let bytes = b"hello\x00world";
        assert!(is_binary(bytes));
    }

    #[test]
    fn is_binary_clean_text_is_not_binary() {
        let bytes = b"fn main() { println!(\"hello\"); }";
        assert!(!is_binary(bytes));
    }

    #[test]
    fn is_binary_utf8_bom_not_binary() {
        let bytes = b"\xef\xbb\xbffn main() {}";
        assert!(!is_binary(bytes));
    }

    #[test]
    fn looks_generated_at_generated_marker() {
        let bytes = b"// @generated by protoc-gen-rust\nfn foo() {}";
        assert!(looks_generated(Path::new("foo.rs"), bytes));
    }

    #[test]
    fn looks_generated_do_not_edit_marker() {
        // "Code generated by" triggers detection (contains the "generated by" substring).
        let bytes = b"// Code generated by build.rs. DO NOT EDIT.\nuse foo;";
        assert!(looks_generated(Path::new("foo.rs"), bytes));
        // @generated also triggers detection independently.
        let bytes2 = b"// @generated\nuse foo;";
        assert!(looks_generated(Path::new("foo.rs"), bytes2));
    }

    #[test]
    fn looks_generated_normal_file_not_generated() {
        let bytes = b"fn main() {\n    println!(\"hello\");\n}\n";
        assert!(!looks_generated(Path::new("main.rs"), bytes));
    }

    #[test]
    fn looks_minified_dot_min_filename() {
        let bytes = b"function a(){return 1}";
        assert!(looks_minified(Path::new("bundle.min.js"), bytes));
    }

    #[test]
    fn looks_minified_normal_file_not_minified() {
        let bytes = b"function hello() {\n    return 1;\n}\n";
        assert!(!looks_minified(Path::new("app.js"), bytes));
    }

    #[test]
    fn looks_minified_very_long_line() {
        let long_line: Vec<u8> = b"x".repeat(MINIFIED_LINE_THRESHOLD + 1);
        assert!(looks_minified(Path::new("app.js"), &long_line));
    }

    #[test]
    fn is_known_lockfile_cargo_lock() {
        assert!(is_known_lockfile(Path::new("Cargo.lock")));
    }

    #[test]
    fn is_known_lockfile_package_lock_json() {
        assert!(is_known_lockfile(Path::new("package-lock.json")));
    }

    #[test]
    fn is_known_lockfile_yarn_lock() {
        assert!(is_known_lockfile(Path::new("yarn.lock")));
    }

    #[test]
    fn is_known_lockfile_normal_file_is_not_lockfile() {
        assert!(!is_known_lockfile(Path::new("src/lib.rs")));
    }

    #[test]
    fn is_vendor_path_node_modules() {
        assert!(is_vendor_path(Path::new("node_modules/react/index.js")));
    }

    #[test]
    fn is_vendor_path_vendor_dir() {
        assert!(is_vendor_path(Path::new("vendor/anyhow/src/lib.rs")));
    }

    #[test]
    fn is_vendor_path_normal_src_is_not_vendor() {
        assert!(!is_vendor_path(Path::new("src/lib.rs")));
    }

    #[test]
    fn is_excluded_dir_path_matches_excluded() {
        let excluded = vec![".git".into(), "target".into()];
        assert!(is_excluded_dir_path(Path::new(".git/config"), &excluded));
    }

    #[test]
    fn is_excluded_dir_path_non_excluded_is_ok() {
        let excluded = vec![".git".into(), "target".into()];
        assert!(!is_excluded_dir_path(Path::new("src/main.rs"), &excluded));
    }

    #[test]
    fn decode_bytes_utf8_bom_stripped() {
        let bytes = b"\xef\xbb\xbffn main() {}";
        let (text, encoding, _) = decode_bytes(bytes).unwrap();
        // BOM is detected — encoding label includes "bom" indicator
        assert!(
            encoding.contains("utf-8"),
            "should be utf-8 variant, got {encoding}"
        );
        assert!(text.starts_with("fn"));
    }

    #[test]
    fn decode_bytes_plain_utf8() {
        let bytes = b"hello world";
        let (text, encoding, warnings) = decode_bytes(bytes).unwrap();
        assert_eq!(encoding, "utf-8");
        assert_eq!(text, "hello world");
        assert!(warnings.is_empty());
    }

    // ── UTF-16 BOM decoding ──────────────────────────────────────────────────

    #[test]
    fn decode_bytes_utf16le_bom() {
        // Encode "hi" as UTF-16 LE with BOM: FF FE 68 00 69 00
        let mut bytes = vec![0xFF, 0xFE];
        for ch in "hi\n".encode_utf16() {
            bytes.extend_from_slice(&ch.to_le_bytes());
        }
        let (text, encoding, _warnings) = decode_bytes(&bytes).unwrap();
        assert_eq!(encoding, "utf-16le");
        assert!(text.contains('h') && text.contains('i'));
    }

    #[test]
    fn decode_bytes_utf16be_bom() {
        // Encode "ok" as UTF-16 BE with BOM: FE FF 00 6F 00 6B
        let mut bytes = vec![0xFE, 0xFF];
        for ch in "ok\n".encode_utf16() {
            bytes.extend_from_slice(&ch.to_be_bytes());
        }
        let (text, encoding, _warnings) = decode_bytes(&bytes).unwrap();
        assert_eq!(encoding, "utf-16be");
        assert!(text.contains('o') && text.contains('k'));
    }

    #[test]
    fn is_binary_utf16le_bom_not_binary() {
        // UTF-16 LE BOM followed by null bytes — should NOT be binary
        let bytes = &[0xFF, 0xFE, 0x68, 0x00];
        assert!(!is_binary(bytes));
    }

    #[test]
    fn is_binary_utf16be_bom_not_binary() {
        let bytes = &[0xFE, 0xFF, 0x00, 0x68];
        assert!(!is_binary(bytes));
    }

    // ── MixedLinePolicy branches ─────────────────────────────────────────────

    #[test]
    fn effective_counts_code_and_comment_policy() {
        let raw = RawLineCounts {
            mixed_code_single_comment_lines: 3,
            mixed_code_multi_comment_lines: 2,
            ..RawLineCounts::default()
        };
        let counts = compute_effective_counts(&raw, MixedLinePolicy::CodeAndComment, true, true);
        // Both code and comment incremented by mixed_total (5)
        assert_eq!(counts.code_lines, 5);
        assert_eq!(counts.comment_lines, 5);
        assert_eq!(counts.mixed_lines_separate, 0);
    }

    #[test]
    fn effective_counts_comment_only_policy() {
        let raw = RawLineCounts {
            mixed_code_single_comment_lines: 4,
            mixed_code_multi_comment_lines: 1,
            ..RawLineCounts::default()
        };
        let counts = compute_effective_counts(&raw, MixedLinePolicy::CommentOnly, true, true);
        assert_eq!(counts.code_lines, 0);
        assert_eq!(counts.comment_lines, 5);
        assert_eq!(counts.mixed_lines_separate, 0);
    }

    #[test]
    fn effective_counts_docstrings_as_code_when_flag_false() {
        let raw = RawLineCounts {
            code_only_lines: 10,
            docstring_comment_lines: 3,
            ..RawLineCounts::default()
        };
        // python_docstrings_as_comments = false → docstrings counted as code
        let counts = compute_effective_counts(&raw, MixedLinePolicy::CodeOnly, false, true);
        assert_eq!(counts.code_lines, 13);
        assert_eq!(counts.comment_lines, 0);
    }

    #[test]
    fn effective_counts_exclude_compiler_directives() {
        let raw = RawLineCounts {
            code_only_lines: 10,
            compiler_directive_lines: 3,
            ..RawLineCounts::default()
        };
        // count_compiler_directives = false → subtract directive lines from code
        let counts = compute_effective_counts(&raw, MixedLinePolicy::CodeOnly, true, false);
        assert_eq!(counts.code_lines, 7);
    }

    #[test]
    fn effective_counts_directives_not_subtracted_below_zero() {
        let raw = RawLineCounts {
            code_only_lines: 2,
            compiler_directive_lines: 5, // more than code — saturating_sub
            ..RawLineCounts::default()
        };
        let counts = compute_effective_counts(&raw, MixedLinePolicy::CodeOnly, true, false);
        assert_eq!(counts.code_lines, 0); // saturated at 0
    }

    // ── COCOMO modes ─────────────────────────────────────────────────────────

    #[test]
    fn cocomo_organic_computes_positive_values() {
        let est = compute_cocomo(5_000, CocomoMode::Organic);
        assert!(est.ksloc > 0.0);
        assert!(est.effort_person_months > 0.0);
        assert!(est.duration_months > 0.0);
        assert!(est.avg_staff > 0.0);
        assert_eq!(est.mode, CocomoMode::Organic);
    }

    #[test]
    fn cocomo_semi_detached_computes_positive_values() {
        let est = compute_cocomo(20_000, CocomoMode::SemiDetached);
        assert!(est.ksloc > 0.0);
        assert!(est.effort_person_months > 0.0);
        assert!(est.duration_months > 0.0);
        assert_eq!(est.mode, CocomoMode::SemiDetached);
    }

    #[test]
    fn cocomo_embedded_computes_positive_values() {
        let est = compute_cocomo(100_000, CocomoMode::Embedded);
        assert!(est.effort_person_months > 0.0);
        assert_eq!(est.mode, CocomoMode::Embedded);
    }

    #[test]
    fn cocomo_zero_lines_produces_zero_effort() {
        let est = compute_cocomo(0, CocomoMode::Organic);
        assert!((est.ksloc).abs() < f64::EPSILON);
        // Zero KSLOC → effort = 2.4 * 0^1.05 = 0
        assert!((est.effort_person_months - 0.0).abs() < 0.01);
    }

    // ── parse_activity_log (git hotspots) ─────────────────────────────────────

    #[test]
    fn parse_activity_log_counts_and_dates_per_file() {
        let out = "\u{0}2024-03-02T10:00:00+00:00\n\
                   M\tsrc/a.rs\n\
                   A\tsrc/b.rs\n\
                   \u{0}2024-03-01T09:00:00+00:00\n\
                   M\tsrc/a.rs\n";
        let map = parse_activity_log(out);
        assert_eq!(map["src/a.rs"].0, 2, "a.rs touched in two commits");
        assert_eq!(map["src/b.rs"].0, 1, "b.rs touched once");
        // Newest-first: a.rs keeps the most recent date.
        assert_eq!(
            map["src/a.rs"].1.as_deref(),
            Some("2024-03-02T10:00:00+00:00")
        );
    }

    #[test]
    fn parse_activity_log_attributes_rename_to_new_path() {
        let out = "\u{0}2024-03-02T10:00:00+00:00\nR100\tsrc/old.rs\tsrc/new.rs\n";
        let map = parse_activity_log(out);
        assert_eq!(map["src/new.rs"].0, 1);
        assert!(!map.contains_key("src/old.rs"));
    }

    #[test]
    fn parse_activity_log_empty_is_empty() {
        assert!(parse_activity_log("").is_empty());
    }

    // ── Code-ownership attribution ────────────────────────────────────────────

    #[test]
    fn parse_blame_porcelain_extracts_one_identity_per_line() {
        let out = "\
abc123 1 1 2
author Nima Shafie
author-mail <nimzshafie@gmail.com>
author-time 1700000000
summary first
filename src/a.rs
\tfirst line of code
abc123 2 2
author Nima Shafie
author-mail <nimzshafie@gmail.com>
\tsecond line
def456 3 3
author Other Dev
author-mail <other@example.com>
\tthird line
";
        let ids = parse_blame_porcelain(out);
        assert_eq!(ids.len(), 3, "one identity per TAB-prefixed content line");
        assert_eq!(ids[0].name, "Nima Shafie");
        assert_eq!(ids[0].email, "nimzshafie@gmail.com");
        assert_eq!(ids[2].name, "Other Dev");
        assert_eq!(ids[2].email, "other@example.com");
    }

    #[test]
    fn parse_blame_porcelain_empty_is_empty() {
        assert!(parse_blame_porcelain("").is_empty());
    }

    #[test]
    fn normalize_email_key_merges_case_and_plus_tag() {
        let a = RawIdentity {
            name: "Nima Shafie".into(),
            email: "Nima@Example.COM".into(),
        };
        let b = RawIdentity {
            name: "nshafie".into(),
            email: "nima+work@example.com".into(),
        };
        assert_eq!(normalize_email_key(&a), normalize_email_key(&b));
    }

    #[test]
    fn normalize_email_key_distinct_emails_do_not_merge() {
        let a = RawIdentity {
            name: "Nima Shafie".into(),
            email: "nima@corp.example".into(),
        };
        let b = RawIdentity {
            name: "Nima Shafie".into(),
            email: "nima@personal.example".into(),
        };
        // Cross-email merges are a later interactive step, not an auto-merge.
        assert_ne!(normalize_email_key(&a), normalize_email_key(&b));
    }

    #[test]
    fn normalize_email_key_missing_email_falls_back_to_name() {
        let a = RawIdentity {
            name: "Anon Dev".into(),
            email: String::new(),
        };
        let b = RawIdentity {
            name: "anon dev".into(),
            email: "not.committed.yet".into(),
        };
        assert_eq!(normalize_email_key(&a), "name:anon dev");
        assert_eq!(normalize_email_key(&a), normalize_email_key(&b));
    }

    #[test]
    fn author_resolver_folds_same_email_and_orders_by_code() {
        let mut r = AuthorResolver::default();
        let id_a1 = r.resolve(&RawIdentity {
            name: "Nima Shafie".into(),
            email: "nima@example.com".into(),
        });
        let id_a2 = r.resolve(&RawIdentity {
            name: "nshafie".into(),
            email: "NIMA@example.com".into(),
        });
        let id_b = r.resolve(&RawIdentity {
            name: "Other".into(),
            email: "other@example.com".into(),
        });
        assert_eq!(id_a1, id_a2, "same email folds into one author");
        assert_ne!(id_a1, id_b);

        // Give author B more code so it should sort first after finish().
        r.authors[id_a1 as usize].counts.code_lines = 10;
        r.authors[id_b as usize].counts.code_lines = 50;
        let mut records: Vec<FileRecord> = Vec::new();
        let authors = r.finish(&mut records);
        assert_eq!(authors.len(), 2);
        assert_eq!(authors[0].canonical_email, "other@example.com");
        assert_eq!(authors[0].id, 0);
        assert_eq!(authors[1].aliases.len(), 2, "two spellings recorded");
    }

    // ── Path / git helpers ────────────────────────────────────────────────────

    #[test]
    fn parse_url_line_extracts_url() {
        assert_eq!(
            parse_url_line("url = https://example.com/repo.git"),
            Some("https://example.com/repo.git")
        );
    }

    #[test]
    fn parse_url_line_returns_none_for_non_url_key() {
        assert_eq!(
            parse_url_line("fetch = +refs/heads/*:refs/remotes/origin/*"),
            None
        );
    }

    #[test]
    fn parse_url_line_returns_none_for_empty_url() {
        assert_eq!(parse_url_line("url = "), None);
    }

    #[test]
    fn looks_generated_generated_filename_extension() {
        // Files with ".generated." in name are detected without reading bytes
        let bytes = b"// normal code\n";
        assert!(looks_generated(Path::new("schema.generated.ts"), bytes));
    }

    #[test]
    fn looks_generated_dot_g_extension() {
        let bytes = b"// normal code\n";
        assert!(looks_generated(Path::new("parser.g.cs"), bytes));
    }

    #[test]
    fn looks_minified_whitespace_ratio_is_ok() {
        // Low whitespace ratio but NOT over the line length threshold → not minified
        let normal = b"var x=1,y=2,z=3;\n";
        assert!(!looks_minified(Path::new("app.js"), normal));
    }

    #[test]
    fn is_known_lockfile_pnpm() {
        assert!(is_known_lockfile(Path::new("pnpm-lock.yaml")));
    }

    #[test]
    fn is_known_lockfile_pipfile() {
        assert!(is_known_lockfile(Path::new("Pipfile.lock")));
    }

    #[test]
    fn is_known_lockfile_poetry() {
        assert!(is_known_lockfile(Path::new("poetry.lock")));
    }

    #[test]
    fn is_known_lockfile_composer() {
        assert!(is_known_lockfile(Path::new("composer.lock")));
    }

    // ── relative_path_string and path_to_string ──────────────────────────────

    #[test]
    fn relative_path_string_strips_root_prefix() {
        let path = Path::new("/tmp/project/src/lib.rs");
        let root = Path::new("/tmp/project");
        let rel = relative_path_string(path, root);
        assert_eq!(rel, "src/lib.rs");
    }

    #[test]
    fn relative_path_string_falls_back_to_full_path() {
        // When path is not under root, fall back to path itself
        let path = Path::new("/other/dir/file.rs");
        let root = Path::new("/tmp/project");
        let rel = relative_path_string(path, root);
        // Should not panic; returns path representation
        assert!(!rel.is_empty());
    }

    // ── find_duplicate_groups ────────────────────────────────────────────────

    #[test]
    fn find_duplicate_groups_returns_empty_for_unique_hashes() {
        use sloc_languages::{Language, ParseMode, RawLineCounts};
        let make_rec = |hash: u64, path: &str| FileRecord {
            path: path.into(),
            relative_path: path.into(),
            language: Some(Language::Rust),
            size_bytes: 10,
            detected_encoding: Some("utf-8".into()),
            raw_line_categories: RawLineCounts::default(),
            effective_counts: EffectiveCounts::default(),
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
            content_hash: hash,
        };
        let analyzed = vec![make_rec(111, "a.rs"), make_rec(222, "b.rs")];
        let groups = find_duplicate_groups(&analyzed);
        assert!(groups.is_empty());
    }

    #[test]
    fn find_duplicate_groups_returns_group_for_same_hash() {
        use sloc_languages::{Language, ParseMode, RawLineCounts};
        let make_rec = |hash: u64, path: &str| FileRecord {
            path: path.into(),
            relative_path: path.into(),
            language: Some(Language::Rust),
            size_bytes: 10,
            detected_encoding: Some("utf-8".into()),
            raw_line_categories: RawLineCounts::default(),
            effective_counts: EffectiveCounts::default(),
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
            content_hash: hash,
        };
        let analyzed = vec![
            make_rec(999, "a.rs"),
            make_rec(999, "b.rs"),
            make_rec(123, "c.rs"),
        ];
        let groups = find_duplicate_groups(&analyzed);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 2);
    }

    #[test]
    fn find_duplicate_groups_ignores_zero_hash() {
        use sloc_languages::{Language, ParseMode, RawLineCounts};
        let make_rec = |hash: u64, path: &str| FileRecord {
            path: path.into(),
            relative_path: path.into(),
            language: Some(Language::Rust),
            size_bytes: 10,
            detected_encoding: Some("utf-8".into()),
            raw_line_categories: RawLineCounts::default(),
            effective_counts: EffectiveCounts::default(),
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
            content_hash: hash,
        };
        // hash=0 means "not computed" — must be excluded from duplicate detection
        let analyzed = vec![make_rec(0, "a.rs"), make_rec(0, "b.rs")];
        let groups = find_duplicate_groups(&analyzed);
        assert!(
            groups.is_empty(),
            "zero-hash files must not be grouped as duplicates"
        );
    }

    // ── detect_submodules ────────────────────────────────────────────────────

    #[test]
    fn detect_submodules_no_gitmodules_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let result = detect_submodules(dir.path());
        assert!(result.is_empty());
    }

    #[test]
    fn detect_submodules_parses_gitmodules_file() {
        let dir = tempfile::tempdir().unwrap();
        let content = "[submodule \"vendor/lib\"]\n\tpath = vendor/lib\n\turl = https://github.com/example/lib.git\n";
        std::fs::write(dir.path().join(".gitmodules"), content).unwrap();
        let result = detect_submodules(dir.path());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, "vendor/lib");
    }

    // ── write_json / read_json roundtrip ─────────────────────────────────────

    #[test]
    fn write_json_read_json_roundtrip() {
        use chrono::Utc;
        use sloc_config::AppConfig;
        use sloc_languages::{Language, ParseMode, RawLineCounts};
        let dir = tempfile::tempdir().unwrap();
        let run = AnalysisRun {
            tool: ToolMetadata {
                name: "sloc".into(),
                version: "0.0.1".into(),
                run_id: "test-roundtrip".into(),
                timestamp_utc: Utc::now(),
            },
            environment: EnvironmentMetadata {
                operating_system: "test".into(),
                architecture: "x86_64".into(),
                runtime_mode: "test".into(),
                initiator_username: "tester".into(),
                initiator_hostname: "testhost".into(),
                ci_name: None,
            },
            effective_configuration: AppConfig::default(),
            input_roots: vec!["/tmp/test".into()],
            summary_totals: SummaryTotals {
                files_analyzed: 1,
                code_lines: 5,
                ..SummaryTotals::default()
            },
            totals_by_language: vec![],
            per_file_records: vec![FileRecord {
                path: "a.rs".into(),
                relative_path: "a.rs".into(),
                language: Some(Language::Rust),
                size_bytes: 50,
                detected_encoding: Some("utf-8".into()),
                raw_line_categories: RawLineCounts {
                    code_only_lines: 5,
                    ..RawLineCounts::default()
                },
                effective_counts: EffectiveCounts {
                    code_lines: 5,
                    ..EffectiveCounts::default()
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
            }],
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
        };
        let json_path = dir.path().join("test.json");
        write_json(&run, &json_path).unwrap();
        let loaded = read_json(&json_path).unwrap();
        assert_eq!(loaded.summary_totals.files_analyzed, 1);
        assert_eq!(loaded.summary_totals.code_lines, 5);
        assert_eq!(loaded.git_commit_short.as_deref(), Some("abc1234"));
        assert_eq!(loaded.git_branch.as_deref(), Some("main"));
        assert_eq!(loaded.per_file_records.len(), 1);
    }

    // ── detect_ci_system ─────────────────────────────────────────────────────

    #[test]
    fn detect_ci_system_returns_none_without_env_vars() {
        // Remove known CI env vars so detection returns None
        for var in &[
            "JENKINS_URL",
            "JENKINS_HOME",
            "BUILD_URL",
            "GITHUB_ACTIONS",
            "GITLAB_CI",
            "CIRCLECI",
            "TRAVIS",
            "TF_BUILD",
            "TEAMCITY_VERSION",
        ] {
            // FIXME: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::remove_var(var) };
        }
        // Result depends on test runner env; just assert no panic
        let _ = detect_ci_system();
    }

    // ── resolve_git_file_pointer ──────────────────────────────────────────────

    #[test]
    fn resolve_git_file_pointer_valid_absolute_gitdir() {
        let dir = tempfile::tempdir().unwrap();
        // Create a real target directory (the "real" git dir)
        let real_git = dir.path().join("real.git");
        fs::create_dir_all(&real_git).unwrap();
        // Write a .git file pointing at the real git dir
        let git_file = dir.path().join(".git");
        fs::write(&git_file, format!("gitdir: {}\n", real_git.display())).unwrap();

        let result = resolve_git_file_pointer(&git_file, dir.path());
        // Should resolve to the real git dir (or its canonicalized form)
        assert!(
            result.is_some(),
            "should resolve a valid absolute gitdir pointer"
        );
        assert!(result.unwrap().is_dir());
    }

    #[test]
    fn resolve_git_file_pointer_missing_gitdir_prefix_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let git_file = dir.path().join(".git");
        fs::write(&git_file, "not a gitdir line\n").unwrap();
        assert!(resolve_git_file_pointer(&git_file, dir.path()).is_none());
    }

    #[test]
    fn resolve_git_file_pointer_unreadable_path_returns_none() {
        assert!(
            resolve_git_file_pointer(
                Path::new("/nonexistent/__sloc_test_git_file__"),
                Path::new("/nonexistent")
            )
            .is_none()
        );
    }

    #[test]
    fn resolve_git_file_pointer_nonexistent_target_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let git_file = dir.path().join(".git");
        fs::write(&git_file, "gitdir: /nonexistent/__sloc_fake_gitdir_xyz__\n").unwrap();
        // Target does not exist → returns None
        assert!(resolve_git_file_pointer(&git_file, dir.path()).is_none());
    }

    #[test]
    fn resolve_git_file_pointer_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        let real_git = dir.path().join("real_git_dir");
        fs::create_dir_all(&real_git).unwrap();
        let git_file = dir.path().join(".git");
        // Relative path — should be resolved relative to base_dir
        fs::write(&git_file, "gitdir: real_git_dir\n").unwrap();
        let result = resolve_git_file_pointer(&git_file, dir.path());
        assert!(result.is_some());
    }

    // ── resolve_ref ──────────────────────────────────────────────────────────

    #[test]
    fn resolve_ref_from_loose_file() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path();
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        let sha = "abc1234567890abcdef1234567890abcdef123456";
        fs::write(git_dir.join("refs/heads/main"), format!("{sha}\n")).unwrap();

        let result = resolve_ref(git_dir, "refs/heads/main");
        assert_eq!(result.as_deref(), Some(sha));
    }

    #[test]
    fn resolve_ref_from_packed_refs() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path();
        let sha = "def5678def5678def5678def5678def5678def56";
        fs::write(
            git_dir.join("packed-refs"),
            format!("# pack-refs with: peeled fully-peeled sorted\n{sha} refs/heads/feature\n"),
        )
        .unwrap();

        let result = resolve_ref(git_dir, "refs/heads/feature");
        assert_eq!(result.as_deref(), Some(sha));
    }

    #[test]
    fn resolve_ref_not_found_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let result = resolve_ref(dir.path(), "refs/heads/nonexistent-branch-xyz");
        assert!(result.is_none());
    }

    #[test]
    fn resolve_ref_packed_refs_skips_comment_and_peeled() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path();
        let sha = "aaa1111aaa1111aaa1111aaa1111aaa1111aaa11";
        fs::write(
            git_dir.join("packed-refs"),
            format!("# comment\n^peeled-object-sha\n{sha} refs/tags/v1.0\n"),
        )
        .unwrap();

        let result = resolve_ref(git_dir, "refs/tags/v1.0");
        assert_eq!(result.as_deref(), Some(sha));
    }

    #[test]
    fn resolve_ref_loose_sha_too_short_falls_through_to_packed() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path();
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        // Write an invalid (too short) SHA to the loose file
        fs::write(git_dir.join("refs/heads/main"), "short\n").unwrap();
        // No packed-refs → None
        let result = resolve_ref(git_dir, "refs/heads/main");
        assert!(result.is_none());
    }

    // ── read_git_remote_url ───────────────────────────────────────────────────

    #[test]
    fn read_git_remote_url_parses_origin_url() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(
            git_dir.join("config"),
            "[core]\n\trepositoryformatversion = 0\n[remote \"origin\"]\n\turl = https://github.com/org/repo.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n",
        )
        .unwrap();
        let url = read_git_remote_url(&git_dir);
        assert_eq!(url.as_deref(), Some("https://github.com/org/repo.git"));
    }

    #[test]
    fn read_git_remote_url_no_config_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        // No config file
        let url = read_git_remote_url(&git_dir);
        assert!(url.is_none());
    }

    // ── detect_git_for_run — HEAD edge cases ──────────────────────────────────

    #[test]
    fn detect_git_for_run_no_git_dir_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        // No .git directory or file
        let info = detect_git_for_run(dir.path());
        assert!(info.commit_long.is_none());
    }

    #[test]
    fn detect_git_for_run_unreadable_head_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        // .git directory exists but no HEAD file → read fails → early return
        let info = detect_git_for_run(dir.path());
        assert!(info.commit_long.is_none());
    }

    #[test]
    fn detect_git_for_run_detached_head_with_sha() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        // Exactly 40 hex chars — the code checks len >= 40 and takes [..40]
        let sha = "abc1234567890abcdef1234567890abcdef12345";
        fs::write(git_dir.join("HEAD"), sha).unwrap();
        let info = detect_git_for_run(dir.path());
        // Detached HEAD — commit_long should be the first 40 chars of HEAD
        assert_eq!(info.commit_long.as_deref(), Some(sha));
        assert_eq!(info.commit_short.as_deref(), Some("abc1234"));
    }

    #[test]
    fn detect_git_for_run_with_packed_ref() {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        // HEAD points to a ref resolved via packed-refs
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let sha = "deadbeef00000000000000000000000000000000";
        fs::write(
            git_dir.join("packed-refs"),
            format!("# pack-refs\n{sha} refs/heads/main\n"),
        )
        .unwrap();
        let info = detect_git_for_run(dir.path());
        assert_eq!(info.commit_long.as_deref(), Some(sha));
        assert_eq!(info.branch.as_deref(), Some("main"));
    }

    #[test]
    fn detect_git_for_run_reads_origin_remote_url() {
        // A .git/config with an [remote "origin"] section exercises
        // read_git_remote_url + parse_url_line, which the dir-only tests miss.
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        let sha = "deadbeef00000000000000000000000000000000";
        fs::write(git_dir.join("HEAD"), sha).unwrap();
        fs::write(
            git_dir.join("config"),
            "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = https://example.com/repo.git\n\tfetch = +refs/heads/*\n",
        )
        .unwrap();
        let info = detect_git_for_run(dir.path());
        assert_eq!(
            info.remote_url.as_deref(),
            Some("https://example.com/repo.git")
        );
    }

    #[test]
    fn detect_git_for_run_follows_git_file_worktree_pointer() {
        // A worktree/submodule uses a `.git` *file* (not dir) containing
        // `gitdir: <path>`. This drives find_git_dir's is_file branch and
        // resolve_git_file_pointer, resolving to the real git data directory.
        let tmp = tempfile::tempdir().unwrap();
        let gitdata = tmp.path().join("gitdata");
        fs::create_dir_all(&gitdata).unwrap();
        let sha = "abc1234567890abcdef1234567890abcdef12345";
        fs::write(gitdata.join("HEAD"), sha).unwrap();

        let project = tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        // Forward-slash path is normalised to the OS separator by the resolver.
        let pointer = format!("gitdir: {}\n", gitdata.to_string_lossy().replace('\\', "/"));
        fs::write(project.join(".git"), pointer).unwrap();

        let info = detect_git_for_run(&project);
        assert_eq!(info.commit_long.as_deref(), Some(sha));
    }

    // ── ci_branch_from_env ───────────────────────────────────────────────────

    // Note: ci_branch_from_env env-var tests share a mutex to avoid parallel interference.
    use std::sync::{Mutex, OnceLock};
    static CI_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    fn ci_env_lock() -> std::sync::MutexGuard<'static, ()> {
        CI_ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    fn clear_branch_env_vars() {
        for v in &[
            "BRANCH_NAME",
            "GIT_BRANCH",
            "GITHUB_REF_NAME",
            "CI_COMMIT_BRANCH",
            "CIRCLE_BRANCH",
            "TRAVIS_BRANCH",
            "BUILD_SOURCEBRANCH",
        ] {
            // FIXME: Audit that the environment access only happens in single-threaded code.
            unsafe { std::env::remove_var(v) };
        }
    }

    #[test]
    fn ci_branch_from_env_strips_refs_heads_prefix() {
        let _lock = ci_env_lock();
        clear_branch_env_vars();
        // Azure DevOps sets BUILD_SOURCEBRANCH = "refs/heads/main"
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("BUILD_SOURCEBRANCH", "refs/heads/my-branch") };
        let branch = ci_branch_from_env();
        clear_branch_env_vars();
        assert_eq!(branch.as_deref(), Some("my-branch"));
    }

    #[test]
    fn ci_branch_from_env_strips_origin_prefix() {
        let _lock = ci_env_lock();
        clear_branch_env_vars();
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("GIT_BRANCH", "origin/develop") };
        let branch = ci_branch_from_env();
        clear_branch_env_vars();
        assert_eq!(branch.as_deref(), Some("develop"));
    }

    #[test]
    fn ci_branch_from_env_returns_none_for_head() {
        let _lock = ci_env_lock();
        clear_branch_env_vars();
        // "HEAD" is filtered out; with no other vars, should return None
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("BRANCH_NAME", "HEAD") };
        let branch = ci_branch_from_env();
        clear_branch_env_vars();
        // HEAD value is filtered → None (or falls through to other vars, but all cleared)
        assert!(branch.is_none(), "HEAD should be filtered, got: {branch:?}");
    }

    // --- Multiple-repository detection -------------------------------------

    /// Create `dir/.git/` so `is_git_root(dir)` is true (plain repo marker).
    fn make_git_dir(dir: &Path) {
        fs::create_dir_all(dir.join(".git")).unwrap();
    }

    #[test]
    fn multi_repo_dir_warns() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for name in ["repo-a", "repo-b", "repo-c"] {
            make_git_dir(&root.join(name));
        }
        let layout = detect_repository_layout(root);
        assert!(!layout.root_is_repo);
        assert_eq!(layout.nested_repos.len(), 3);
        assert!(layout.has_multiple_repos());
    }

    #[test]
    fn repo_with_submodules_does_not_warn() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        make_git_dir(root);
        fs::write(
            root.join(".gitmodules"),
            "[submodule \"vendor/json\"]\n\tpath = vendor/json\n\turl = https://example/json.git\n\
             [submodule \"vendor/gtest\"]\n\tpath = vendor/gtest\n\turl = https://example/gtest.git\n",
        )
        .unwrap();
        // Submodule working trees carry a `.git` *file* pointer, but we prune them
        // by declared path regardless, so a plain marker dir is enough here.
        make_git_dir(&root.join("vendor/json"));
        make_git_dir(&root.join("vendor/gtest"));
        let layout = detect_repository_layout(root);
        assert!(layout.root_is_repo);
        assert!(layout.nested_repos.is_empty());
        assert!(!layout.has_multiple_repos());
    }

    #[test]
    fn format_multi_repo_warning_root_repo_singular_and_truncated() {
        // root_is_repo=true with a single nested repo → the singular "repository"
        // wording. The dir-layout tests never format this branch.
        let one = RepositoryLayout {
            root: PathBuf::from("/proj"),
            root_is_repo: true,
            submodule_paths: vec![],
            nested_repos: vec![PathBuf::from("vendor/foreign")],
        };
        let msg = format_multi_repo_warning(&one);
        assert!(
            msg.contains("1 nested git repository"),
            "singular wording: {msg}"
        );
        assert!(!msg.contains("repositories"), "must not pluralise: {msg}");

        // root_is_repo=true with more than MAX_LISTED (5) nested repos → the
        // "… and N more" truncation plus plural wording.
        let many = RepositoryLayout {
            root: PathBuf::from("/proj"),
            root_is_repo: true,
            submodule_paths: vec![],
            nested_repos: (0..7)
                .map(|i| PathBuf::from(format!("nested-{i}")))
                .collect(),
        };
        let msg = format_multi_repo_warning(&many);
        assert!(
            msg.contains("7 nested git repositories"),
            "plural wording: {msg}"
        );
        assert!(
            msg.contains("and 2 more"),
            "must truncate the listed set: {msg}"
        );
    }

    #[test]
    fn repo_with_vendored_foreign_repo_warns() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        make_git_dir(root); // root is a repo
        make_git_dir(&root.join("vendor/foreign")); // not a declared submodule
        let layout = detect_repository_layout(root);
        assert!(layout.root_is_repo);
        assert_eq!(layout.nested_repos, vec![PathBuf::from("vendor/foreign")]);
        assert!(layout.has_multiple_repos());
    }

    #[test]
    fn single_plain_dir_no_warn() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        let layout = detect_repository_layout(root);
        assert!(!layout.root_is_repo);
        assert!(layout.nested_repos.is_empty());
        assert!(!layout.has_multiple_repos());
    }

    #[test]
    fn analyze_surfaces_multi_repo_warning() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for name in ["repo-a", "repo-b"] {
            let repo = root.join(name);
            make_git_dir(&repo);
            fs::write(repo.join("main.rs"), "fn main() {}\n").unwrap();
        }
        let mut config = AppConfig::default();
        config.discovery.root_paths = vec![root.to_path_buf()];
        let run = analyze(&config, "analyze", None, None).unwrap();
        assert!(
            run.warnings
                .iter()
                .any(|w| w.contains("independent git repositories")),
            "expected multi-repo warning, got: {:?}",
            run.warnings
        );
    }
}
