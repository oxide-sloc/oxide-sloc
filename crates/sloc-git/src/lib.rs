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
