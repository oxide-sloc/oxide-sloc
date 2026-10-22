// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>

//! Batch / queue scanning: parse a list of projects to scan and model the
//! combined result index.
//!
//! A batch is declared one of two ways, auto-detected from the `--from` source:
//!
//! * **A directory** — every immediate subdirectory is treated as one project
//!   and scanned with the single default profile.
//! * **A manifest file** — either a plain-text list (one project per line,
//!   `path[, profile]`, `#` comments allowed) or a JSON array of objects
//!   `{ "path": "...", "profile": "...", "label": "...", "config": "..." }`.
//!
//! This module owns only the *parsing* and the serializable *index* model so it
//! can be reused by both the CLI (`oxide-sloc batch`) and the web UI; the actual
//! per-project analysis + artifact writing is orchestrated by the caller.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::history::ScanSummarySnapshot;

/// One project to scan in a batch, after the manifest has been resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchEntry {
    /// Directory to scan.
    pub path: PathBuf,
    /// Unique, filesystem-safe label used for the per-project output subdir.
    pub label: String,
    /// Optional named profile from the config file to apply to this project.
    pub profile: Option<String>,
    /// Optional per-project config (`.oxide-sloc.toml`) override.
    pub config: Option<PathBuf>,
}

/// Raw JSON manifest entry (deserialization shape only).
#[derive(Debug, Deserialize)]
struct JsonEntry {
    path: String,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    config: Option<String>,
}

/// Parse a batch manifest from a directory or a file.
///
/// `default_profile` applies to every entry that does not name its own profile
/// (directory sources always use it; file sources use it as the fallback).
pub fn parse_batch_manifest(from: &Path, default_profile: Option<&str>) -> Result<Vec<BatchEntry>> {
    let entries = if from.is_dir() {
        parse_directory(from, default_profile)?
    } else if from.is_file() {
        parse_file(from, default_profile)?
    } else {
        bail!(
            "batch source not found (expected a directory of projects or a manifest file): {}",
            from.display()
        );
    };
    if entries.is_empty() {
        bail!(
            "batch source produced no projects to scan: {}",
            from.display()
        );
    }
    Ok(dedupe_labels(entries))
}

/// Each immediate subdirectory of `root` becomes one project.
fn parse_directory(root: &Path, default_profile: Option<&str>) -> Result<Vec<BatchEntry>> {
    let mut out = Vec::new();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .with_context(|| format!("reading batch directory {}", root.display()))?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let label = label_from_path(&dir);
        out.push(BatchEntry {
            path: dir,
            label,
            profile: default_profile.map(str::to_owned),
            config: None,
        });
    }
    Ok(out)
}

/// Parse a manifest file: JSON array when it looks like JSON, else a text list.
fn parse_file(file: &Path, default_profile: Option<&str>) -> Result<Vec<BatchEntry>> {
    let raw = std::fs::read_to_string(file)
        .with_context(|| format!("reading batch manifest {}", file.display()))?;
    let trimmed = raw.trim_start();
    let is_json = trimmed.starts_with('[')
        || file
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"));
    if is_json {
        parse_json(&raw, default_profile)
    } else {
        parse_text(&raw, default_profile)
    }
}

fn parse_json(raw: &str, default_profile: Option<&str>) -> Result<Vec<BatchEntry>> {
    let items: Vec<JsonEntry> =
        serde_json::from_str(raw).context("parsing JSON batch manifest (expected an array)")?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let path = PathBuf::from(item.path);
        let label = item
            .label
            .filter(|s| !s.trim().is_empty())
            .map_or_else(|| label_from_path(&path), |s| sanitize_label(&s));
        out.push(BatchEntry {
            label,
            profile: item.profile.or_else(|| default_profile.map(str::to_owned)),
            config: item.config.map(PathBuf::from),
            path,
        });
    }
    Ok(out)
}

/// Parse a newline-separated project list (`path[, profile]` per line, `#`
/// comments allowed), de-duplicating labels. Convenience wrapper over the
/// text-manifest parser for callers that already hold the text (e.g. the web
/// UI's batch textarea).
pub fn parse_batch_text(text: &str, default_profile: Option<&str>) -> Result<Vec<BatchEntry>> {
    let entries = parse_text(text, default_profile)?;
    if entries.is_empty() {
        bail!("no projects were provided");
    }
    Ok(dedupe_labels(entries))
}

/// One entry per non-empty, non-comment line: `path[, profile]`.
fn parse_text(raw: &str, default_profile: Option<&str>) -> Result<Vec<BatchEntry>> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (path_str, profile) = match line.split_once(',') {
            Some((p, prof)) => {
                let prof = prof.trim();
                (
                    p.trim(),
                    if prof.is_empty() {
                        None
                    } else {
                        Some(prof.to_owned())
                    },
                )
            }
            None => (line, None),
        };
        let path = PathBuf::from(path_str);
        let label = label_from_path(&path);
        out.push(BatchEntry {
            label,
            profile: profile.or_else(|| default_profile.map(str::to_owned)),
            config: None,
            path,
        });
    }
    Ok(out)
}

/// Derive a label from a path's final component, sanitized for the filesystem.
fn label_from_path(path: &Path) -> String {
    let base = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty() && s != "." && s != "..")
        .unwrap_or_else(|| "project".to_owned());
    sanitize_label(&base)
}

/// Keep ASCII alphanumerics, `-`, `_`, `.`; collapse everything else to `-`.
fn sanitize_label(raw: &str) -> String {
    let mut s: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-').to_owned();
    if s.is_empty() {
        "project".to_owned()
    } else {
        s
    }
}

/// Ensure labels are unique by suffixing `-2`, `-3`, ... on collisions.
fn dedupe_labels(mut entries: Vec<BatchEntry>) -> Vec<BatchEntry> {
    let mut seen: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for entry in &mut entries {
        let count = seen.entry(entry.label.clone()).or_insert(0);
        *count += 1;
        if *count > 1 {
            entry.label = format!("{}-{}", entry.label, *count);
        }
    }
    entries
}

/// Result of scanning one project within a batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchItemResult {
    pub label: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<ScanSummarySnapshot>,
    /// Output artifacts written for this project, relative to the batch out dir.
    #[serde(default)]
    pub artifacts: Vec<String>,
}

/// Combined index for a whole batch run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchIndex {
    pub generated_at: DateTime<Utc>,
    pub tool_version: String,
    pub items: Vec<BatchItemResult>,
    /// Aggregate totals across every successful project.
    pub totals: ScanSummarySnapshot,
}

impl BatchIndex {
    /// Build an index from per-project results, aggregating successful totals.
    #[must_use]
    pub fn new(tool_version: impl Into<String>, items: Vec<BatchItemResult>) -> Self {
        let mut totals = ScanSummarySnapshot::default();
        for item in &items {
            if let Some(s) = &item.summary {
                totals.files_analyzed += s.files_analyzed;
                totals.files_skipped += s.files_skipped;
                totals.total_physical_lines += s.total_physical_lines;
                totals.code_lines += s.code_lines;
                totals.comment_lines += s.comment_lines;
                totals.blank_lines += s.blank_lines;
                totals.functions += s.functions;
                totals.classes += s.classes;
                totals.variables += s.variables;
                totals.imports += s.imports;
                totals.test_count += s.test_count;
            }
        }
        Self {
            generated_at: Utc::now(),
            tool_version: tool_version.into(),
            items,
            totals,
        }
    }

    /// Count of projects that scanned successfully.
    #[must_use]
    pub fn ok_count(&self) -> usize {
        self.items.iter().filter(|i| i.ok).count()
    }

    /// Count of projects that failed to scan.
    #[must_use]
    pub fn fail_count(&self) -> usize {
        self.items.iter().filter(|i| !i.ok).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_label_collapses_and_trims() {
        assert_eq!(sanitize_label("my repo!!"), "my-repo");
        assert_eq!(sanitize_label("a/b\\c"), "a-b-c");
        assert_eq!(sanitize_label("---"), "project");
        assert_eq!(sanitize_label("keep_dots.v2"), "keep_dots.v2");
    }

    #[test]
    fn text_manifest_parses_path_and_optional_profile() {
        let raw = "# a comment\n/proj/one\n/proj/two, strict\n\n  /proj/three ,full-scope\n";
        let entries = parse_text(raw, Some("default")).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path, PathBuf::from("/proj/one"));
        assert_eq!(entries[0].profile.as_deref(), Some("default"));
        assert_eq!(entries[1].profile.as_deref(), Some("strict"));
        assert_eq!(entries[2].profile.as_deref(), Some("full-scope"));
    }

    #[test]
    fn json_manifest_parses_objects() {
        let raw = r#"[
            {"path": "/proj/a", "profile": "strict", "label": "Proj A"},
            {"path": "/proj/b"}
        ]"#;
        let entries = parse_json(raw, Some("default")).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].label, "Proj-A");
        assert_eq!(entries[0].profile.as_deref(), Some("strict"));
        assert_eq!(entries[1].profile.as_deref(), Some("default"));
        assert_eq!(entries[1].label, "b");
    }

    #[test]
    fn labels_are_deduped() {
        let entries = dedupe_labels(vec![
            BatchEntry {
                path: "/x/proj".into(),
                label: "proj".into(),
                profile: None,
                config: None,
            },
            BatchEntry {
                path: "/y/proj".into(),
                label: "proj".into(),
                profile: None,
                config: None,
            },
        ]);
        assert_eq!(entries[0].label, "proj");
        assert_eq!(entries[1].label, "proj-2");
    }

    #[test]
    fn index_aggregates_only_successful() {
        let ok = BatchItemResult {
            label: "a".into(),
            path: "/a".into(),
            profile: None,
            ok: true,
            error: None,
            summary: Some(ScanSummarySnapshot {
                code_lines: 100,
                files_analyzed: 2,
                ..Default::default()
            }),
            artifacts: vec![],
        };
        let bad = BatchItemResult {
            label: "b".into(),
            path: "/b".into(),
            profile: None,
            ok: false,
            error: Some("boom".into()),
            summary: None,
            artifacts: vec![],
        };
        let index = BatchIndex::new("test", vec![ok, bad]);
        assert_eq!(index.totals.code_lines, 100);
        assert_eq!(index.ok_count(), 1);
        assert_eq!(index.fail_count(), 1);
    }
}
