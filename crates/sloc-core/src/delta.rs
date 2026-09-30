// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::{AnalysisRun, FileRecord};

#[derive(Debug, Serialize)]
pub struct SummaryDelta {
    pub baseline_run_id: String,
    pub current_run_id: String,
    pub baseline_timestamp: DateTime<Utc>,
    pub current_timestamp: DateTime<Utc>,
    pub baseline_files: u64,
    pub current_files: u64,
    pub files_analyzed_delta: i64,
    pub baseline_code: u64,
    pub current_code: u64,
    pub code_lines_delta: i64,
    pub baseline_comments: u64,
    pub current_comments: u64,
    pub comment_lines_delta: i64,
    pub blank_lines_delta: i64,
    pub total_lines_delta: i64,
    /// Lines hit delta (positive = more covered). `None` if neither run has coverage data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage_lines_hit_delta: Option<i64>,
    /// Line coverage percentage delta (positive = improved). `None` if neither run has coverage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage_line_pct_delta: Option<f64>,
    /// Baseline line coverage percentage. `None` if baseline had no coverage data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline_coverage_line_pct: Option<f64>,
    /// Current line coverage percentage. `None` if current has no coverage data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_coverage_line_pct: Option<f64>,
}

#[derive(Debug, Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeStatus {
    Added,
    Removed,
    Modified,
    Unchanged,
}

#[derive(Debug, Serialize)]
pub struct FileDelta {
    pub relative_path: String,
    pub language: Option<String>,
    pub status: FileChangeStatus,
    pub baseline_code: i64,
    pub current_code: i64,
    pub code_delta: i64,
    pub baseline_comment: i64,
    pub current_comment: i64,
    pub comment_delta: i64,
    pub baseline_blank: i64,
    pub current_blank: i64,
    pub blank_delta: i64,
    pub total_delta: i64,
    /// Distinct code lines present in the current scan but not the baseline (multiset difference
    /// of per-line content hashes). Unlike `code_delta` this counts real churn, so a file that
    /// swaps N lines for N others reports `added_lines = removed_lines = N` while `code_delta = 0`.
    /// `None` when per-line hashes are unavailable on either side (e.g. a pre-Tier-2 run JSON).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added_lines: Option<i64>,
    /// Distinct code lines present in the baseline but not the current scan. See `added_lines`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed_lines: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ScanComparison {
    pub summary: SummaryDelta,
    pub file_deltas: Vec<FileDelta>,
    pub files_added: usize,
    pub files_removed: usize,
    pub files_modified: usize,
    pub files_unchanged: usize,
    /// Total files across both scans (`files_added + files_removed + files_modified + files_unchanged`).
    pub files_total: usize,
}

/// Per-line hashes are trustworthy for a file only when they are populated, or when the file
/// genuinely has zero code lines (an empty hash slice is then correct, not merely unpersisted).
/// A file with code lines but an empty hash slice came from a pre-Tier-2 JSON and is not usable.
fn hashes_usable(record: &FileRecord) -> bool {
    !record.raw_line_categories.code_line_hashes.is_empty()
        || record.effective_counts.code_lines == 0
}

/// Multiset difference of two per-line code-hash slices: `(added, removed)` where `added` counts
/// hashes present in `curr` beyond their `base` multiplicity and `removed` counts the reverse.
fn code_line_churn(base: &[u64], curr: &[u64]) -> (i64, i64) {
    let mut counts: HashMap<u64, i64> = HashMap::new();
    for &h in base {
        *counts.entry(h).or_default() += 1;
    }
    for &h in curr {
        *counts.entry(h).or_default() -= 1;
    }
    let mut removed = 0i64;
    let mut added = 0i64;
    for &v in counts.values() {
        if v > 0 {
            removed += v;
        } else {
            added += -v;
        }
    }
    (added, removed)
}

fn build_modified(record: &FileRecord, base: &FileRecord, lang: Option<String>) -> FileDelta {
    let curr = &record.effective_counts;
    let base_counts = &base.effective_counts;
    let code_delta = curr.code_lines.cast_signed() - base_counts.code_lines.cast_signed();
    let comment_delta = curr.comment_lines.cast_signed() - base_counts.comment_lines.cast_signed();
    let blank_delta = curr.blank_lines.cast_signed() - base_counts.blank_lines.cast_signed();

    // Per-line churn (multiset diff) when both sides carry usable line hashes.
    let (added_lines, removed_lines) = if hashes_usable(base) && hashes_usable(record) {
        let (a, r) = code_line_churn(
            &base.raw_line_categories.code_line_hashes,
            &record.raw_line_categories.code_line_hashes,
        );
        (Some(a), Some(r))
    } else {
        (None, None)
    };

    // Counts differing is a definite modification. When counts match, fall back to the whole-file
    // content hash (Tier 2) to catch equal-count edits (line swaps, reformatting), then finally to
    // the code-line churn — any of these signals a real change the raw counts would have hidden.
    let counts_equal = code_delta == 0 && comment_delta == 0 && blank_delta == 0;
    let content_changed = base.content_hash != 0
        && record.content_hash != 0
        && base.content_hash != record.content_hash;
    let churn_nonzero =
        added_lines.is_some_and(|a| a != 0) || removed_lines.is_some_and(|r| r != 0);
    let status = if !counts_equal || content_changed || churn_nonzero {
        FileChangeStatus::Modified
    } else {
        FileChangeStatus::Unchanged
    };

    FileDelta {
        relative_path: record.relative_path.clone(),
        language: lang,
        status,
        baseline_code: base_counts.code_lines.cast_signed(),
        current_code: curr.code_lines.cast_signed(),
        code_delta,
        baseline_comment: base_counts.comment_lines.cast_signed(),
        current_comment: curr.comment_lines.cast_signed(),
        comment_delta,
        baseline_blank: base_counts.blank_lines.cast_signed(),
        current_blank: curr.blank_lines.cast_signed(),
        blank_delta,
        total_delta: code_delta + comment_delta + blank_delta,
        added_lines,
        removed_lines,
    }
}

fn build_added(record: &FileRecord, lang: Option<String>) -> FileDelta {
    let curr = &record.effective_counts;
    let total = (curr.code_lines + curr.comment_lines + curr.blank_lines).cast_signed();
    // Every code line in a newly added file is an added line.
    let (added_lines, removed_lines) = if hashes_usable(record) {
        let n =
            i64::try_from(record.raw_line_categories.code_line_hashes.len()).unwrap_or(i64::MAX);
        (Some(n), Some(0))
    } else {
        (None, None)
    };
    FileDelta {
        relative_path: record.relative_path.clone(),
        language: lang,
        status: FileChangeStatus::Added,
        baseline_code: 0,
        current_code: curr.code_lines.cast_signed(),
        code_delta: curr.code_lines.cast_signed(),
        baseline_comment: 0,
        current_comment: curr.comment_lines.cast_signed(),
        comment_delta: curr.comment_lines.cast_signed(),
        baseline_blank: 0,
        current_blank: curr.blank_lines.cast_signed(),
        blank_delta: curr.blank_lines.cast_signed(),
        total_delta: total,
        added_lines,
        removed_lines,
    }
}

fn build_removed(base: &FileRecord, lang: Option<String>) -> FileDelta {
    let base_counts = &base.effective_counts;
    let total = (base_counts.code_lines + base_counts.comment_lines + base_counts.blank_lines)
        .cast_signed();
    // Every code line in a removed file is a removed line.
    let (added_lines, removed_lines) = if hashes_usable(base) {
        let n = i64::try_from(base.raw_line_categories.code_line_hashes.len()).unwrap_or(i64::MAX);
        (Some(0), Some(n))
    } else {
        (None, None)
    };
    FileDelta {
        relative_path: base.relative_path.clone(),
        language: lang,
        status: FileChangeStatus::Removed,
        baseline_code: base_counts.code_lines.cast_signed(),
        current_code: 0,
        code_delta: -(base_counts.code_lines.cast_signed()),
        baseline_comment: base_counts.comment_lines.cast_signed(),
        current_comment: 0,
        comment_delta: -(base_counts.comment_lines.cast_signed()),
        baseline_blank: base_counts.blank_lines.cast_signed(),
        current_blank: 0,
        blank_delta: -(base_counts.blank_lines.cast_signed()),
        total_delta: -total,
        added_lines,
        removed_lines,
    }
}

#[allow(clippy::cast_precision_loss)]
fn coverage_line_pct(hit: u64, found: u64) -> Option<f64> {
    if found == 0 {
        None
    } else {
        let pct = (hit as f64 / found as f64) * 100.0;
        Some((pct * 10.0).round() / 10.0)
    }
}

#[must_use]
#[allow(clippy::too_many_lines)]
pub fn compute_delta(baseline: &AnalysisRun, current: &AnalysisRun) -> ScanComparison {
    let baseline_map: HashMap<&str, &FileRecord> = baseline
        .per_file_records
        .iter()
        .map(|f| (f.relative_path.as_str(), f))
        .collect();

    let current_paths: HashSet<&str> = current
        .per_file_records
        .iter()
        .map(|f| f.relative_path.as_str())
        .collect();

    let mut file_deltas: Vec<FileDelta> = Vec::new();

    for record in &current.per_file_records {
        let path = record.relative_path.as_str();
        let lang = record.language.map(|l| l.display_name().to_string());
        if let Some(base) = baseline_map.get(path) {
            file_deltas.push(build_modified(record, base, lang));
        } else {
            file_deltas.push(build_added(record, lang));
        }
    }

    for record in &baseline.per_file_records {
        if !current_paths.contains(record.relative_path.as_str()) {
            let lang = record.language.map(|l| l.display_name().to_string());
            file_deltas.push(build_removed(record, lang));
        }
    }

    file_deltas.sort_by(|a, b| {
        const fn order(s: FileChangeStatus) -> u8 {
            match s {
                FileChangeStatus::Modified => 0,
                FileChangeStatus::Added => 1,
                FileChangeStatus::Removed => 2,
                FileChangeStatus::Unchanged => 3,
            }
        }
        order(a.status)
            .cmp(&order(b.status))
            .then(a.relative_path.cmp(&b.relative_path))
    });

    let files_added = file_deltas
        .iter()
        .filter(|f| f.status == FileChangeStatus::Added)
        .count();
    let files_removed = file_deltas
        .iter()
        .filter(|f| f.status == FileChangeStatus::Removed)
        .count();
    let files_modified = file_deltas
        .iter()
        .filter(|f| f.status == FileChangeStatus::Modified)
        .count();
    let files_unchanged = file_deltas
        .iter()
        .filter(|f| f.status == FileChangeStatus::Unchanged)
        .count();

    let s = &current.summary_totals;
    let b = &baseline.summary_totals;

    let baseline_cov_pct = coverage_line_pct(b.coverage_lines_hit, b.coverage_lines_found);
    let current_cov_pct = coverage_line_pct(s.coverage_lines_hit, s.coverage_lines_found);
    let coverage_lines_hit_delta = if b.coverage_lines_found > 0 || s.coverage_lines_found > 0 {
        Some(s.coverage_lines_hit.cast_signed() - b.coverage_lines_hit.cast_signed())
    } else {
        None
    };
    let coverage_line_pct_delta = match (baseline_cov_pct, current_cov_pct) {
        (Some(base_pct), Some(cur_pct)) => Some(((cur_pct - base_pct) * 10.0).round() / 10.0),
        (None, Some(cur_pct)) => Some(cur_pct),
        _ => None,
    };

    ScanComparison {
        summary: SummaryDelta {
            baseline_run_id: baseline.tool.run_id.clone(),
            current_run_id: current.tool.run_id.clone(),
            baseline_timestamp: baseline.tool.timestamp_utc,
            current_timestamp: current.tool.timestamp_utc,
            baseline_files: b.files_analyzed,
            current_files: s.files_analyzed,
            files_analyzed_delta: s.files_analyzed.cast_signed() - b.files_analyzed.cast_signed(),
            baseline_code: b.code_lines,
            current_code: s.code_lines,
            code_lines_delta: s.code_lines.cast_signed() - b.code_lines.cast_signed(),
            baseline_comments: b.comment_lines,
            current_comments: s.comment_lines,
            comment_lines_delta: s.comment_lines.cast_signed() - b.comment_lines.cast_signed(),
            blank_lines_delta: s.blank_lines.cast_signed() - b.blank_lines.cast_signed(),
            total_lines_delta: s
                .total_physical_lines
                .cast_signed()
                .wrapping_sub(b.total_physical_lines.cast_signed()),
            coverage_lines_hit_delta,
            coverage_line_pct_delta,
            baseline_coverage_line_pct: baseline_cov_pct,
            current_coverage_line_pct: current_cov_pct,
        },
        file_deltas,
        files_added,
        files_removed,
        files_modified,
        files_unchanged,
        files_total: files_added + files_removed + files_modified + files_unchanged,
    }
}

// ── Multi-point comparison ─────────────────────────────────────────────────────

/// Summary metrics snapshot for one scan in a multi-point timeline.
#[derive(Debug, Serialize)]
pub struct MultiScanPoint {
    pub run_id: String,
    pub timestamp: DateTime<Utc>,
    pub git_commit: Option<String>,
    pub git_branch: Option<String>,
    pub git_tags: Option<String>,
    pub git_nearest_tag: Option<String>,
    pub code_lines: i64,
    pub comment_lines: i64,
    pub blank_lines: i64,
    pub files_analyzed: i64,
    pub test_count: i64,
    pub coverage_line_pct: Option<f64>,
}

/// Per-file code counts across N scan points.
#[derive(Debug, Serialize)]
pub struct MultiFileDelta {
    pub relative_path: String,
    pub language: Option<String>,
    /// Code lines at each scan (`None` = file absent at that point).
    pub code_per_scan: Vec<Option<i64>>,
    /// Delta from previous scan; index 0 is always `None`.
    pub code_delta_per_scan: Vec<Option<i64>>,
    /// `"added"` | `"removed"` | `"modified"` | `"unchanged"`
    pub overall_status: String,
    /// Code delta from first presence to last presence.
    pub total_code_delta: i64,
}

#[derive(Debug, Serialize)]
pub struct MultiScanComparison {
    pub points: Vec<MultiScanPoint>,
    /// One `ScanComparison` per consecutive pair (length = N − 1).
    pub sequential_deltas: Vec<ScanComparison>,
    /// Overall delta from the first scan to the last scan.
    pub total_delta: SummaryDelta,
    /// All files × all scan points, sorted modified → added → removed → unchanged.
    pub file_matrix: Vec<MultiFileDelta>,
}

/// Per-scan deltas for a single file path: delta[0] is always None (no previous), delta[i] is
/// code[i] - code[i-1], or None when both sides are absent.
fn sequential_code_deltas(code_per_scan: &[Option<i64>]) -> Vec<Option<i64>> {
    let mut deltas = vec![None];
    for i in 1..code_per_scan.len() {
        if code_per_scan[i - 1].is_some() || code_per_scan[i].is_some() {
            let prev = code_per_scan[i - 1].unwrap_or(0);
            let curr = code_per_scan[i].unwrap_or(0);
            deltas.push(Some(curr - prev));
        } else {
            deltas.push(None);
        }
    }
    deltas
}

/// Classify a file's lifetime across scans as "added", "removed", "modified", or "unchanged".
fn classify_file_status(code_per_scan: &[Option<i64>]) -> &'static str {
    let n = code_per_scan.len();
    let first_idx = code_per_scan.iter().position(Option::is_some);
    let last_idx = code_per_scan.iter().rposition(Option::is_some);
    match (first_idx, last_idx) {
        (Some(f), Some(l)) if f > 0 && l == n - 1 => "added",
        (Some(f), Some(l)) if f == 0 && l < n - 1 => "removed",
        (Some(f), Some(l)) => {
            let first_val = code_per_scan[f].unwrap_or(0);
            if code_per_scan[f..=l]
                .iter()
                .all(|v| v.is_none_or(|x| x == first_val))
            {
                "unchanged"
            } else {
                "modified"
            }
        }
        _ => "unchanged",
    }
}

/// Net code-line change from the first scan where the file appeared to the last.
fn net_code_delta(code_per_scan: &[Option<i64>]) -> i64 {
    let first_idx = code_per_scan.iter().position(Option::is_some);
    let last_idx = code_per_scan.iter().rposition(Option::is_some);
    match (first_idx, last_idx) {
        (Some(f), Some(l)) => code_per_scan[l].unwrap_or(0) - code_per_scan[f].unwrap_or(0),
        _ => 0,
    }
}

/// Compute a multi-point timeline comparison.
///
/// `runs` must be sorted chronologically (oldest first) and contain at least 2 elements.
///
/// # Panics
///
/// Panics if `runs` contains fewer than 2 elements.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn compute_multi_delta(runs: &[&AnalysisRun]) -> MultiScanComparison {
    assert!(
        runs.len() >= 2,
        "compute_multi_delta requires at least 2 runs"
    );

    // Union of all file paths across every run.
    let all_paths: BTreeSet<String> = runs
        .iter()
        .flat_map(|r| r.per_file_records.iter().map(|f| f.relative_path.clone()))
        .collect();

    // Per-run lookup: path → FileRecord.
    let run_maps: Vec<HashMap<&str, &FileRecord>> = runs
        .iter()
        .map(|r| {
            r.per_file_records
                .iter()
                .map(|f| (f.relative_path.as_str(), f))
                .collect()
        })
        .collect();

    // Build the file matrix.
    let mut file_matrix: Vec<MultiFileDelta> = all_paths
        .into_iter()
        .map(|path| {
            let code_per_scan: Vec<Option<i64>> = run_maps
                .iter()
                .map(|m| {
                    m.get(path.as_str())
                        .map(|r| r.effective_counts.code_lines.cast_signed())
                })
                .collect();
            let code_delta_per_scan = sequential_code_deltas(&code_per_scan);
            let overall_status = classify_file_status(&code_per_scan).to_string();
            let total_code_delta = net_code_delta(&code_per_scan);
            let language = run_maps.iter().find_map(|m| {
                m.get(path.as_str())
                    .and_then(|r| r.language)
                    .map(|l| l.display_name().to_string())
            });
            MultiFileDelta {
                relative_path: path,
                language,
                code_per_scan,
                code_delta_per_scan,
                overall_status,
                total_code_delta,
            }
        })
        .collect();

    // Sort: modified → added → removed → unchanged, then path.
    file_matrix.sort_by(|a, b| {
        const fn status_order(s: &str) -> u8 {
            match s.as_bytes() {
                b"modified" => 0,
                b"added" => 1,
                b"removed" => 2,
                _ => 3,
            }
        }
        status_order(&a.overall_status)
            .cmp(&status_order(&b.overall_status))
            .then(a.relative_path.cmp(&b.relative_path))
    });

    // Sequential deltas (N - 1 pairs).
    let sequential_deltas: Vec<ScanComparison> = (0..runs.len() - 1)
        .map(|i| compute_delta(runs[i], runs[i + 1]))
        .collect();

    // Overall first-to-last delta.
    let total_delta = compute_delta(runs[0], runs[runs.len() - 1]).summary;

    // Build scan-point summaries.
    let points: Vec<MultiScanPoint> = runs
        .iter()
        .map(|r| {
            let s = &r.summary_totals;
            let coverage_line_pct = if s.coverage_lines_found > 0 {
                Some(
                    ((s.coverage_lines_hit as f64 / s.coverage_lines_found as f64 * 1000.0)
                        .round())
                        / 10.0,
                )
            } else {
                None
            };
            MultiScanPoint {
                run_id: r.tool.run_id.clone(),
                timestamp: r.tool.timestamp_utc,
                git_commit: r.git_commit_short.clone(),
                git_branch: r.git_branch.clone(),
                git_tags: r.git_tags.clone(),
                git_nearest_tag: r.git_nearest_tag.clone(),
                code_lines: s.code_lines.cast_signed(),
                comment_lines: s.comment_lines.cast_signed(),
                blank_lines: s.blank_lines.cast_signed(),
                files_analyzed: s.files_analyzed.cast_signed(),
                test_count: s.test_count.cast_signed(),
                coverage_line_pct,
            }
        })
        .collect();

    MultiScanComparison {
        points,
        sequential_deltas,
        total_delta,
        file_matrix,
    }
}
