// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>

//! Disk-hygiene primitives for reclaiming space taken by old scan artifacts and
//! log files.
//!
//! These functions are deliberately auth-free and side-effect-transparent: they
//! operate directly on the on-disk registry and output tree, so the same logic
//! can drive the operator's `oxide-sloc prune` CLI command, the web UI's
//! auto-cleanup policy, and the audit-log rotation. The trust boundary is the
//! host: whoever can run the binary or reach these files already owns them, so
//! no separate "admin" credential is required — the shell (for the CLI) and the
//! existing API-key gate (for the web surface) are the authority.
//!
//! Nothing here deletes source code, configuration, or anything outside the
//! resolved output root / configured log path.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::history::{RegistryEntry, ScanRegistry};

// ── Output-root / registry resolution (shared by CLI + web) ─────────────────────

/// Workspace root used to anchor the default output tree.
///
/// `OXIDE_SLOC_ROOT` wins when it names a real directory (Docker / systemd / CI);
/// otherwise the current working directory is used, matching the web server's resolution.
#[must_use]
pub fn workspace_root() -> PathBuf {
    if let Ok(root) = std::env::var("OXIDE_SLOC_ROOT") {
        let p = PathBuf::from(root);
        if p.is_dir() {
            return p;
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Resolve the artifact output root.
///
/// Mirrors the web server so the CLI prunes exactly the tree the server writes to.
/// An explicit `raw` path (e.g. a CLI `--output-dir`) overrides; a relative value
/// is anchored at [`workspace_root`].
#[must_use]
pub fn resolve_output_root(raw: Option<&str>) -> PathBuf {
    let value = raw.unwrap_or("out/web").trim();
    let path = if value.is_empty() {
        PathBuf::from("out/web")
    } else {
        PathBuf::from(value)
    };
    if path.is_absolute() {
        path
    } else {
        workspace_root().join(path)
    }
}

/// Path to the scan registry for a given output root. Honours `SLOC_REGISTRY_PATH`
/// so a shared-drive deployment and its CLI agree on the same index.
#[must_use]
pub fn resolve_registry_path(output_root: &Path) -> PathBuf {
    std::env::var("SLOC_REGISTRY_PATH")
        .map_or_else(|_| output_root.join("registry.json"), PathBuf::from)
}

// ── Sizing ──────────────────────────────────────────────────────────────────────

/// Total size in bytes of a directory tree. Best-effort: unreadable entries are
/// skipped rather than erroring, so a partial permission failure still yields a
/// useful estimate.
#[must_use]
pub fn dir_size_bytes(path: &Path) -> u64 {
    fn walk(path: &Path, acc: &mut u64) {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                walk(&entry.path(), acc);
            } else if let Ok(meta) = entry.metadata() {
                *acc += meta.len();
            }
        }
    }
    let mut total = 0;
    if path.is_file() {
        return std::fs::metadata(path).map_or(0, |m| m.len());
    }
    walk(path, &mut total);
    total
}

/// Recursively copy the file tree at `src` into `dest`, creating `dest` and any
/// intermediate directories. Returns `(files_copied, bytes_written)`.
///
/// Symlinks are **not** followed (a link is skipped, not dereferenced) so a link
/// inside a scanned/uploaded tree cannot redirect the copy outside `src` or loop.
/// Used to publish a run's artifacts to an operator-configured export directory
/// (e.g. a mounted network share), so it must be robust against hostile trees.
///
/// # Errors
/// Returns the first I/O error encountered while creating `dest` or copying a file.
pub fn copy_tree(src: &Path, dest: &Path) -> std::io::Result<(usize, u64)> {
    std::fs::create_dir_all(dest)?;
    let mut files = 0usize;
    let mut bytes = 0u64;
    // Iterative DFS over (src_dir, dest_dir) pairs.
    let mut stack = vec![(src.to_path_buf(), dest.to_path_buf())];
    while let Some((from, to)) = stack.pop() {
        for entry in std::fs::read_dir(&from)?.flatten() {
            let ft = entry.file_type()?;
            if ft.is_symlink() {
                continue;
            }
            let target = to.join(entry.file_name());
            if ft.is_dir() {
                std::fs::create_dir_all(&target)?;
                stack.push((entry.path(), target));
            } else if ft.is_file() {
                let n = std::fs::copy(entry.path(), &target)?;
                files += 1;
                bytes = bytes.saturating_add(n);
            }
        }
    }
    Ok((files, bytes))
}

/// Derive the on-disk output directory that holds a run's artifacts from its registry entry.
///
/// Handles both the current layout (files nested in `html/` `json/` `pdf/` `excel/`
/// subfolders — go up two levels) and the older flat layout (go up one level).
/// Returns `None` when the entry stored no paths.
#[must_use]
pub fn run_output_dir(entry: &RegistryEntry) -> Option<PathBuf> {
    let p = entry
        .html_path
        .as_ref()
        .or(entry.json_path.as_ref())
        .or(entry.pdf_path.as_ref())
        .or(entry.csv_path.as_ref())
        .or(entry.xlsx_path.as_ref())?;
    let parent = p.parent()?;
    let parent_name = parent.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if matches!(parent_name, "html" | "json" | "pdf" | "excel") {
        parent.parent().map(PathBuf::from)
    } else {
        Some(parent.to_path_buf())
    }
}

// ── Run pruning ─────────────────────────────────────────────────────────────────

/// One run selected for removal, with the disk it will reclaim.
#[derive(Debug, Clone)]
pub struct PrunedRun {
    pub run_id: String,
    pub project_label: String,
    pub timestamp_utc: DateTime<Utc>,
    pub output_dir: Option<PathBuf>,
    pub bytes: u64,
}

/// The set of runs a prune would remove, computed without touching disk.
#[derive(Debug, Clone, Default)]
pub struct PrunePlan {
    pub runs: Vec<PrunedRun>,
    pub total_bytes: u64,
}

impl PrunePlan {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }
}

/// Compute which runs to delete under the given retention rules, newest-kept.
///
/// * `older_than_days` — delete runs whose timestamp is older than N days.
/// * `keep_last` — keep only the N most-recent runs, delete the rest.
///
/// A run is selected if it matches *either* rule (union). With neither rule set
/// the plan is empty — pruning is always opt-in about what "old" means. Registry
/// entries are assumed newest-first (as maintained by [`ScanRegistry::add_entry`]).
#[must_use]
pub fn plan_run_prune(
    reg: &ScanRegistry,
    older_than_days: Option<u32>,
    keep_last: Option<u32>,
) -> PrunePlan {
    use std::collections::HashSet;

    // Work on a timestamp-sorted (newest-first) view so `keep_last` is stable even
    // if the on-disk registry was hand-edited out of order.
    let mut ordered: Vec<&RegistryEntry> = reg.entries.iter().collect();
    ordered.sort_by_key(|e| std::cmp::Reverse(e.timestamp_utc));

    let mut selected: HashSet<&str> = HashSet::new();
    if let Some(days) = older_than_days {
        let cutoff = Utc::now() - chrono::Duration::days(i64::from(days));
        for e in &ordered {
            if e.timestamp_utc < cutoff {
                selected.insert(e.run_id.as_str());
            }
        }
    }
    if let Some(keep) = keep_last {
        for e in ordered.iter().skip(keep as usize) {
            selected.insert(e.run_id.as_str());
        }
    }

    let mut runs = Vec::new();
    let mut total_bytes = 0u64;
    for e in &ordered {
        if !selected.contains(e.run_id.as_str()) {
            continue;
        }
        let output_dir = run_output_dir(e);
        let bytes = output_dir.as_deref().map_or(0, dir_size_bytes);
        total_bytes += bytes;
        runs.push(PrunedRun {
            run_id: e.run_id.clone(),
            project_label: e.project_label.clone(),
            timestamp_utc: e.timestamp_utc,
            output_dir,
            bytes,
        });
    }
    PrunePlan { runs, total_bytes }
}

/// Outcome of executing a [`PrunePlan`].
#[derive(Debug, Clone, Default)]
pub struct PruneReport {
    pub deleted_runs: usize,
    pub bytes_freed: u64,
    /// Runs whose on-disk directory could not be removed (path + error string).
    pub failures: Vec<(String, String)>,
}

/// Delete the artifacts named by `plan` and drop their entries from `reg`.
///
/// The registry is mutated in place; the caller is responsible for persisting it
/// via [`ScanRegistry::save`]. Directory-removal failures are collected rather
/// than aborting so one locked run does not block reclaiming the rest.
#[must_use]
pub fn execute_run_prune(reg: &mut ScanRegistry, plan: &PrunePlan) -> PruneReport {
    use std::collections::HashSet;

    let mut report = PruneReport::default();
    let mut removed_ids: HashSet<String> = HashSet::new();

    for run in &plan.runs {
        if let Some(dir) = &run.output_dir
            && dir.exists()
            && let Err(e) = std::fs::remove_dir_all(dir)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            report
                .failures
                .push((dir.display().to_string(), e.to_string()));
            continue;
        }
        report.bytes_freed += run.bytes;
        report.deleted_runs += 1;
        removed_ids.insert(run.run_id.clone());
    }

    reg.entries.retain(|e| !removed_ids.contains(&e.run_id));
    report
}

// ── Log rotation ────────────────────────────────────────────────────────────────

/// Rotate `path` when it exceeds `max_bytes`, keeping up to `keep` compressed-by-age
/// generations (`path.1`, `path.2`, …). Returns `Ok(true)` when a rotation happened.
///
/// Rotation is size-triggered and lossless up to `keep`: `path.(keep)` is dropped,
/// each `path.N` shifts to `path.(N+1)`, the live file becomes `path.1`, and a
/// fresh empty live file is implied (the caller re-creates it on next append).
/// `keep == 0` simply truncates the oversized file, retaining no history.
///
/// # Errors
///
/// Returns an error if a rename or removal fails. A missing live file or a file
/// under the threshold is a no-op that returns `Ok(false)`.
pub fn rotate_log(path: &Path, max_bytes: u64, keep: u32) -> anyhow::Result<bool> {
    let Ok(meta) = std::fs::metadata(path) else {
        return Ok(false); // no live file yet
    };
    if meta.len() <= max_bytes {
        return Ok(false);
    }

    if keep == 0 {
        // No history retained: just clear the file in place.
        std::fs::write(path, b"")?;
        return Ok(true);
    }

    // Drop the oldest generation, then shift each remaining one down by one.
    let gen_path = |n: u32| -> PathBuf {
        let mut s = path.as_os_str().to_owned();
        s.push(format!(".{n}"));
        PathBuf::from(s)
    };
    let oldest = gen_path(keep);
    if oldest.exists() {
        std::fs::remove_file(&oldest)?;
    }
    for n in (1..keep).rev() {
        let from = gen_path(n);
        if from.exists() {
            std::fs::rename(&from, gen_path(n + 1))?;
        }
    }
    std::fs::rename(path, gen_path(1))?;
    Ok(true)
}

/// Every rotated generation of `path` that currently exists on disk (`path.1`, …),
/// scanning until the first gap. Used by the CLI to report and remove log history.
#[must_use]
pub fn rotated_log_paths(path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut n = 1u32;
    loop {
        let mut s = path.as_os_str().to_owned();
        s.push(format!(".{n}"));
        let p = PathBuf::from(s);
        if p.exists() {
            out.push(p);
            n += 1;
        } else {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{RegistryEntry, ScanRegistry, ScanSummarySnapshot};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// Tests that mutate process-global environment variables must hold this lock
    /// for their whole duration so the parallel test runner cannot observe each
    /// other's `set_var`/`remove_var` changes.
    fn env_lock() -> MutexGuard<'static, ()> {
        static ENV_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
        ENV_MUTEX
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn copy_tree_copies_nested_files_and_counts() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(src.path().join("json")).unwrap();
        std::fs::write(src.path().join("json").join("result.json"), b"{\"a\":1}").unwrap();
        std::fs::write(src.path().join("top.txt"), b"hello").unwrap();

        let dest_root = dst.path().join("run-123");
        let (files, bytes) = copy_tree(src.path(), &dest_root).unwrap();

        assert_eq!(files, 2);
        assert_eq!(bytes, 7 + 5);
        assert!(dest_root.join("json").join("result.json").is_file());
        assert!(dest_root.join("top.txt").is_file());
        assert_eq!(
            std::fs::read(dest_root.join("json").join("result.json")).unwrap(),
            b"{\"a\":1}"
        );
    }

    #[test]
    fn copy_tree_empty_source_creates_dest() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let dest_root = dst.path().join("out");
        let (files, bytes) = copy_tree(src.path(), &dest_root).unwrap();
        assert_eq!((files, bytes), (0, 0));
        assert!(dest_root.is_dir());
    }

    fn entry(run_id: &str, age_days: i64, root: &Path) -> RegistryEntry {
        let dir = root.join(run_id);
        std::fs::create_dir_all(dir.join("json")).unwrap();
        std::fs::write(dir.join("json").join("result.json"), b"{}").unwrap();
        RegistryEntry {
            run_id: run_id.to_owned(),
            timestamp_utc: Utc::now() - chrono::Duration::days(age_days),
            project_label: format!("proj-{run_id}"),
            input_roots: vec![],
            json_path: Some(dir.join("json").join("result.json")),
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

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("sloc_maint_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn run_output_dir_handles_nested_layout() {
        let root = tmp();
        let e = entry("abc", 0, &root);
        // json is in <root>/abc/json/result.json → output dir is <root>/abc
        assert_eq!(run_output_dir(&e), Some(root.join("abc")));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn plan_selects_by_age() {
        let root = tmp();
        let mut reg = ScanRegistry::default();
        reg.entries.push(entry("old", 40, &root));
        reg.entries.push(entry("new", 1, &root));

        let plan = plan_run_prune(&reg, Some(30), None);
        assert_eq!(plan.runs.len(), 1);
        assert_eq!(plan.runs[0].run_id, "old");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn plan_selects_by_keep_last() {
        let root = tmp();
        let mut reg = ScanRegistry::default();
        for (i, id) in ["a", "b", "c", "d"].iter().enumerate() {
            reg.entries
                .push(entry(id, i64::try_from(i).unwrap(), &root)); // a newest .. d oldest
        }
        let plan = plan_run_prune(&reg, None, Some(2));
        let ids: Vec<_> = plan.runs.iter().map(|r| r.run_id.clone()).collect();
        assert_eq!(plan.runs.len(), 2, "keep 2, delete the 2 oldest");
        assert!(ids.contains(&"c".to_owned()) && ids.contains(&"d".to_owned()));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn execute_removes_dirs_and_entries() {
        let root = tmp();
        let mut reg = ScanRegistry::default();
        reg.entries.push(entry("gone", 40, &root));
        reg.entries.push(entry("kept", 1, &root));

        let plan = plan_run_prune(&reg, Some(30), None);
        let report = execute_run_prune(&mut reg, &plan);

        assert_eq!(report.deleted_runs, 1);
        assert!(report.failures.is_empty());
        assert!(!root.join("gone").exists(), "artifacts removed");
        assert!(root.join("kept").exists(), "recent run untouched");
        assert_eq!(reg.entries.len(), 1);
        assert_eq!(reg.entries[0].run_id, "kept");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn empty_plan_when_no_rules() {
        let root = tmp();
        let mut reg = ScanRegistry::default();
        reg.entries.push(entry("x", 100, &root));
        assert!(plan_run_prune(&reg, None, None).is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rotate_log_shifts_generations() {
        let root = tmp();
        let log = root.join("audit.log");
        std::fs::write(&log, vec![b'x'; 100]).unwrap();

        // Under threshold: no-op.
        assert!(!rotate_log(&log, 1000, 3).unwrap());
        // Over threshold: rotates to audit.log.1, live file gone (recreated by caller).
        assert!(rotate_log(&log, 50, 3).unwrap());
        assert!(log.with_extension("log.1").exists());
        assert!(!log.exists());

        // Second rotation shifts .1 → .2.
        std::fs::write(&log, vec![b'y'; 100]).unwrap();
        assert!(rotate_log(&log, 50, 3).unwrap());
        assert!(log.with_extension("log.1").exists());
        assert!(log.with_extension("log.2").exists());
        assert_eq!(rotated_log_paths(&log).len(), 2);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rotate_log_keep_zero_truncates() {
        let root = tmp();
        let log = root.join("audit.log");
        std::fs::write(&log, vec![b'x'; 100]).unwrap();
        assert!(rotate_log(&log, 10, 0).unwrap());
        assert!(log.exists());
        assert_eq!(std::fs::metadata(&log).unwrap().len(), 0);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn workspace_root_prefers_env_dir_then_falls_back() {
        let _guard = env_lock();
        let dir = tmp();
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("OXIDE_SLOC_ROOT", &dir) };
        assert_eq!(workspace_root(), dir);
        // A non-existent path is ignored → falls back to CWD (a real dir).
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("OXIDE_SLOC_ROOT", dir.join("does-not-exist")) };
        assert!(workspace_root().is_dir());
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("OXIDE_SLOC_ROOT") };
        assert!(workspace_root().is_dir());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_output_root_handles_absolute_relative_and_default() {
        let _guard = env_lock();
        let dir = tmp();
        // Absolute path is returned verbatim.
        let abs = dir.join("art");
        assert_eq!(resolve_output_root(Some(abs.to_str().unwrap())), abs);
        // Empty/whitespace falls back to the default relative tree under the root.
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("OXIDE_SLOC_ROOT", &dir) };
        assert_eq!(resolve_output_root(Some("   ")), dir.join("out/web"));
        assert_eq!(resolve_output_root(None), dir.join("out/web"));
        // A relative override is anchored at the workspace root.
        assert_eq!(
            resolve_output_root(Some("custom/out")),
            dir.join("custom/out")
        );
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("OXIDE_SLOC_ROOT") };
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_registry_path_honours_env_override() {
        let _guard = env_lock();
        let dir = tmp();
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("SLOC_REGISTRY_PATH") };
        assert_eq!(resolve_registry_path(&dir), dir.join("registry.json"));
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var("SLOC_REGISTRY_PATH", dir.join("shared.json")) };
        assert_eq!(resolve_registry_path(&dir), dir.join("shared.json"));
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var("SLOC_REGISTRY_PATH") };
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dir_size_bytes_counts_files_and_handles_single_file() {
        let root = tmp();
        std::fs::write(root.join("a.txt"), vec![b'x'; 10]).unwrap();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub").join("b.txt"), vec![b'y'; 5]).unwrap();
        assert_eq!(dir_size_bytes(&root), 15);
        // A path to a single file returns just that file's length.
        assert_eq!(dir_size_bytes(&root.join("a.txt")), 10);
        // A non-existent path is a best-effort zero.
        assert_eq!(dir_size_bytes(&root.join("nope")), 0);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn run_output_dir_handles_flat_layout_and_missing_paths() {
        let root = tmp();
        // Flat layout: json sits directly under the run dir (no html/json subfolder).
        let mut e = entry("flat", 0, &root);
        e.json_path = Some(root.join("flat").join("result.json"));
        assert_eq!(run_output_dir(&e), Some(root.join("flat")));
        // No stored paths at all → None.
        e.json_path = None;
        assert_eq!(run_output_dir(&e), None);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn execute_run_prune_records_failure_for_locked_dir() {
        let root = tmp();
        let mut reg = ScanRegistry::default();
        reg.entries.push(entry("target", 40, &root));
        let mut plan = plan_run_prune(&reg, Some(30), None);
        // Point the plan at a *file* masquerading as the output dir so
        // remove_dir_all fails with a non-NotFound error, exercising the
        // failure-collection branch without racing on OS file locks.
        let bogus = root.join("target").join("json").join("result.json");
        plan.runs[0].output_dir = Some(bogus);
        let report = execute_run_prune(&mut reg, &plan);
        assert_eq!(report.deleted_runs, 0);
        assert_eq!(report.failures.len(), 1);
        assert!(reg.entries.iter().any(|e| e.run_id == "target"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rotate_log_drops_oldest_generation_at_keep_cap() {
        let root = tmp();
        let log = root.join("audit.log");
        // Pre-seed the max number of generations we keep.
        std::fs::write(&log, vec![b'x'; 100]).unwrap();
        std::fs::write(log.with_extension("log.1"), b"g1").unwrap();
        std::fs::write(log.with_extension("log.2"), b"g2").unwrap();
        // keep=2: the oldest (.2) is dropped, .1 shifts to .2, live becomes .1.
        assert!(rotate_log(&log, 50, 2).unwrap());
        assert!(log.with_extension("log.1").exists());
        assert!(log.with_extension("log.2").exists());
        assert!(!log.with_extension("log.3").exists());
        std::fs::remove_dir_all(&root).ok();
    }
}
