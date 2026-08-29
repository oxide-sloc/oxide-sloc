// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>

use std::cmp::Reverse;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Lightweight summary snapshot stored in the registry — avoids loading full JSON per entry.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScanSummarySnapshot {
    pub files_analyzed: u64,
    pub files_skipped: u64,
    pub total_physical_lines: u64,
    pub code_lines: u64,
    pub comment_lines: u64,
    pub blank_lines: u64,
    #[serde(default)]
    pub functions: u64,
    #[serde(default)]
    pub classes: u64,
    #[serde(default)]
    pub variables: u64,
    #[serde(default)]
    pub imports: u64,
    #[serde(default)]
    pub test_count: u64,
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
}

impl From<&crate::SummaryTotals> for ScanSummarySnapshot {
    /// Project the full per-run totals down to the lightweight registry/baseline snapshot.
    /// Centralises the field-by-field copy that callers (CLI baseline, web registry) would
    /// otherwise duplicate.
    fn from(t: &crate::SummaryTotals) -> Self {
        Self {
            files_analyzed: t.files_analyzed,
            files_skipped: t.files_skipped,
            total_physical_lines: t.total_physical_lines,
            code_lines: t.code_lines,
            comment_lines: t.comment_lines,
            blank_lines: t.blank_lines,
            functions: t.functions,
            classes: t.classes,
            variables: t.variables,
            imports: t.imports,
            test_count: t.test_count,
            coverage_lines_found: t.coverage_lines_found,
            coverage_lines_hit: t.coverage_lines_hit,
            coverage_functions_found: t.coverage_functions_found,
            coverage_functions_hit: t.coverage_functions_hit,
            coverage_branches_found: t.coverage_branches_found,
            coverage_branches_hit: t.coverage_branches_hit,
        }
    }
}

/// One entry in the scan registry — one per completed analysis run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryEntry {
    pub run_id: String,
    pub timestamp_utc: DateTime<Utc>,
    pub project_label: String,
    pub input_roots: Vec<String>,
    pub json_path: Option<PathBuf>,
    pub html_path: Option<PathBuf>,
    #[serde(default)]
    pub pdf_path: Option<PathBuf>,
    #[serde(default)]
    pub csv_path: Option<PathBuf>,
    #[serde(default)]
    pub xlsx_path: Option<PathBuf>,
    pub summary: ScanSummarySnapshot,
    /// Git branch active at scan time, if the project is a git repo.
    #[serde(default)]
    pub git_branch: Option<String>,
    /// Short git commit SHA active at scan time.
    #[serde(default)]
    pub git_commit: Option<String>,
    /// Full-length git commit SHA active at scan time (shown on hover).
    #[serde(default)]
    pub git_commit_long: Option<String>,
    /// Author of the last git commit at scan time.
    #[serde(default)]
    pub git_author: Option<String>,
    /// Comma-separated git tags pointing at HEAD at scan time.
    #[serde(default)]
    pub git_tags: Option<String>,
    /// Nearest ancestor release tag (output of `git describe --tags --abbrev=0`).
    #[serde(default)]
    pub git_nearest_tag: Option<String>,
    /// ISO 8601 author-date of the last git commit at scan time.
    #[serde(default)]
    pub git_commit_date: Option<String>,
    /// Operating system the scan ran on (e.g. "windows", "linux"). Lets pooled reports
    /// from different environments be told apart in the list/compare/trend views.
    #[serde(default)]
    pub scan_os: Option<String>,
    /// Hostname (or CI node name) the scan ran on.
    #[serde(default)]
    pub scan_host: Option<String>,
    /// User who initiated the scan.
    #[serde(default)]
    pub scan_user: Option<String>,
    /// Detected CI system name (e.g. "Jenkins", "GitHub Actions"), if the scan ran in CI.
    #[serde(default)]
    pub scan_ci: Option<String>,
}

impl RegistryEntry {
    /// Human-readable label for who/what produced this report. Mirrors the single-report
    /// page's "Scan by" chip so pooled reports from different environments/users are
    /// distinguishable in the list, compare, and trend views: the CI system name when the
    /// scan ran in CI, otherwise `user / host`, otherwise "unknown".
    #[must_use]
    pub fn performed_by(&self) -> String {
        if let Some(ci) = self.scan_ci.as_deref().filter(|s| !s.is_empty()) {
            return ci.to_string();
        }
        match (
            self.scan_user.as_deref().filter(|s| !s.is_empty()),
            self.scan_host.as_deref().filter(|s| !s.is_empty()),
        ) {
            (Some(u), Some(h)) => format!("{u} / {h}"),
            (Some(u), None) => u.to_string(),
            (None, Some(h)) => h.to_string(),
            (None, None) => "unknown".to_string(),
        }
    }
}

/// Persistent list of directories the user has chosen to watch for new reports.
/// Stored as `watched_dirs.json` adjacent to `registry.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct WatchedDirsStore {
    pub dirs: Vec<PathBuf>,
}

impl WatchedDirsStore {
    #[must_use]
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// # Errors
    ///
    /// Returns an error if the file cannot be written.
    pub fn save(&self, path: &Path) -> Result<()> {
        let path = crate::pathsafe::reject_traversal(path)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn add(&mut self, dir: PathBuf) {
        if !self.dirs.contains(&dir) {
            self.dirs.push(dir);
        }
    }

    pub fn remove(&mut self, dir: &Path) {
        self.dirs.retain(|d| d != dir);
    }
}

/// Persistent on-disk index of all past scans for this workspace.
/// Stored as `registry.json` adjacent to the scan output directories.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ScanRegistry {
    pub entries: Vec<RegistryEntry>,
}

impl ScanRegistry {
    /// Load from disk; returns an empty registry on missing file or parse error.
    #[must_use]
    pub fn load(registry_path: &Path) -> Self {
        std::fs::read_to_string(registry_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// # Errors
    ///
    /// Returns an error if the parent directory cannot be created or the file cannot be written.
    pub fn save(&self, registry_path: &Path) -> Result<()> {
        let registry_path = crate::pathsafe::reject_traversal(registry_path)?;
        if let Some(parent) = registry_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&registry_path, json)?;
        Ok(())
    }

    pub fn add_entry(&mut self, entry: RegistryEntry) {
        self.entries.retain(|e| e.run_id != entry.run_id);
        self.entries.push(entry);
        self.entries.sort_by_key(|e| Reverse(e.timestamp_utc));
    }

    /// All entries whose `input_roots` exactly match, newest first.
    #[must_use]
    pub fn entries_for_roots(&self, roots: &[String]) -> Vec<&RegistryEntry> {
        self.entries
            .iter()
            .filter(|e| e.input_roots == roots)
            .collect()
    }

    #[must_use]
    pub fn find_by_run_id(&self, run_id: &str) -> Option<&RegistryEntry> {
        self.entries.iter().find(|e| e.run_id == run_id)
    }

    /// Remove entries whose `json_path` no longer exists on disk.
    pub fn prune_stale(&mut self) {
        self.entries
            .retain(|e| e.json_path.as_ref().is_none_or(|p| p.exists()));
    }

    /// Remove every entry whose artifacts live under `dir` (the directory itself or any
    /// descendant). Used when a watched folder is un-watched so its linked reports leave the
    /// list too. Returns the number of entries removed.
    pub fn remove_entries_under(&mut self, dir: &Path) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| {
            let under = |p: &Option<PathBuf>| p.as_ref().is_some_and(|path| path.starts_with(dir));
            !(under(&e.json_path) || under(&e.html_path))
        });
        before - self.entries.len()
    }

    /// Keep only entries whose artifacts live under one of `roots` — the currently-watched
    /// folders plus the app's own output directory. Everything else (leftovers from folders
    /// that are no longer watched, or other external strays) is dropped. This enforces the
    /// invariant that the report list reflects exactly the watched folders and native scans.
    /// Returns the number of entries removed.
    pub fn retain_under_roots(&mut self, roots: &[PathBuf]) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| {
            let under = |p: &Option<PathBuf>| {
                p.as_ref()
                    .is_some_and(|path| roots.iter().any(|r| path.starts_with(r)))
            };
            under(&e.json_path) || under(&e.html_path)
        });
        before - self.entries.len()
    }
}

const fn default_interval_hours() -> u32 {
    24
}

/// Rules for automatic periodic cleanup of old scan runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanupPolicy {
    pub enabled: bool,
    /// Delete runs older than this many days. `None` disables age-based cleanup.
    #[serde(default)]
    pub max_age_days: Option<u32>,
    /// Keep only the N most recent runs; delete older ones. `None` disables count-based cleanup.
    #[serde(default)]
    pub max_run_count: Option<u32>,
    /// Cap the total on-disk size of retained scan artifacts, in megabytes. When the
    /// artifact tree exceeds this, the oldest runs are deleted until it fits again.
    /// `None` disables size-based cleanup. The `SLOC_MAX_DISK_MB` operator environment
    /// variable is a separate hard ceiling; when both are set the smaller wins.
    #[serde(default)]
    pub max_total_mb: Option<u64>,
    /// Hours between automatic cleanup passes (minimum 1, default 24).
    #[serde(default = "default_interval_hours")]
    pub interval_hours: u32,
}

/// Persisted store for the auto-cleanup policy and last-run metadata.
/// Stored as `cleanup_policy.json` adjacent to `registry.json`.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CleanupPolicyStore {
    pub policy: Option<CleanupPolicy>,
    /// When the background task last ran a cleanup pass.
    #[serde(default)]
    pub last_run_at: Option<DateTime<Utc>>,
    /// Number of runs deleted in the last cleanup pass.
    #[serde(default)]
    pub last_run_deleted: Option<u32>,
}

impl CleanupPolicyStore {
    #[must_use]
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// # Errors
    ///
    /// Returns an error if the file cannot be written.
    pub fn save(&self, path: &Path) -> Result<()> {
        let path = crate::pathsafe::reject_traversal(path)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(run_id: &str, json: &str) -> RegistryEntry {
        RegistryEntry {
            run_id: run_id.to_string(),
            timestamp_utc: Utc::now(),
            project_label: "proj".to_string(),
            input_roots: vec![],
            json_path: Some(PathBuf::from(json)),
            html_path: None,
            pdf_path: None,
            csv_path: None,
            xlsx_path: None,
            summary: ScanSummarySnapshot::default(),
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
    fn remove_entries_under_drops_only_matching_folder() {
        let mut reg = ScanRegistry::default();
        reg.entries
            .push(entry("a", "/watched/scans/run1/result.json"));
        reg.entries
            .push(entry("b", "/watched/scans/sub/json/result.json"));
        reg.entries.push(entry("c", "/other/place/result.json"));

        let removed = reg.remove_entries_under(Path::new("/watched/scans"));
        assert_eq!(removed, 2);
        assert_eq!(reg.entries.len(), 1);
        assert_eq!(reg.entries[0].run_id, "c");
    }

    #[test]
    fn remove_entries_under_no_match_is_noop() {
        let mut reg = ScanRegistry::default();
        reg.entries.push(entry("a", "/other/result.json"));
        assert_eq!(reg.remove_entries_under(Path::new("/watched")), 0);
        assert_eq!(reg.entries.len(), 1);
    }

    #[test]
    fn retain_under_roots_keeps_only_watched_and_native() {
        let mut reg = ScanRegistry::default();
        reg.entries
            .push(entry("native", "/app/out/web/run1/json/result.json"));
        reg.entries
            .push(entry("watched", "/watched/scans/run2/json/result.json"));
        reg.entries
            .push(entry("orphan", "/removed/folder/run3/json/result.json"));

        let roots = vec![
            PathBuf::from("/app/out/web"),
            PathBuf::from("/watched/scans"),
        ];
        let removed = reg.retain_under_roots(&roots);
        assert_eq!(removed, 1);
        assert_eq!(reg.entries.len(), 2);
        assert!(reg.entries.iter().all(|e| e.run_id != "orphan"));
    }

    #[test]
    fn retain_under_roots_empty_roots_clears_all() {
        let mut reg = ScanRegistry::default();
        reg.entries.push(entry("a", "/somewhere/result.json"));
        reg.entries.push(entry("b", "/elsewhere/result.json"));
        assert_eq!(reg.retain_under_roots(&[]), 2);
        assert!(reg.entries.is_empty());
    }
}
