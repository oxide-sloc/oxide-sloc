// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>

pub mod ops;
pub mod schedule;
pub mod webhook;

pub use ops::{
    clone_or_fetch, create_worktree, destroy_worktree, get_sha, host_allowlist_configured,
    is_local_repo_path, list_commits, list_refs, list_refs_local, normalize_git_url,
    open_local_repo, populate_submodules, publish_dir,
};
pub use schedule::{ScanSchedule, ScanScheduleKind, ScanScheduleProvider, ScheduleStore};
pub use webhook::{
    WebhookEvent, WebhookProvider, hmac_sha256_hex, parse_bitbucket_push, parse_github_push,
    parse_gitlab_push,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Reject a filesystem path that contains a parent-directory (`..`) segment, returning a
/// freshly-owned path rebuilt from the validated string. This crate is the leaf of the
/// workspace (it depends on no sibling crates), so it carries its own traversal barrier
/// rather than reusing `sloc_core::pathsafe`. Callers must use the returned value for the
/// subsequent filesystem operation so the check dominates the sink.
///
/// # Errors
/// Returns an error when the path contains a `..` component.
pub(crate) fn reject_traversal(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let text = path.to_string_lossy().into_owned();
    if text.contains("..") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refusing filesystem path with a parent-directory (`..`) segment",
        ));
    }
    Ok(std::path::PathBuf::from(text))
}

#[cfg(test)]
mod pathsafe_tests {
    use super::reject_traversal;
    use std::path::{Path, PathBuf};

    #[test]
    fn accepts_plain_path() {
        assert_eq!(
            reject_traversal(Path::new("clones/repo")).expect("plain path"),
            PathBuf::from("clones/repo")
        );
    }

    #[test]
    fn rejects_traversal() {
        assert!(reject_traversal(Path::new("clones/../../etc")).is_err());
        assert!(reject_traversal(Path::new(r"clones\..\secret")).is_err());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GitRefKind {
    Branch,
    Tag,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitRef {
    pub kind: GitRefKind,
    pub name: String,
    pub sha: String,
    pub date: Option<DateTime<Utc>>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCommit {
    pub sha: String,
    pub short_sha: String,
    pub author: String,
    pub date: DateTime<Utc>,
    pub subject: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoRefs {
    pub branches: Vec<GitRef>,
    pub tags: Vec<GitRef>,
    pub recent_commits: Vec<GitCommit>,
}
