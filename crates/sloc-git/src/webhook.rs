// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>

use anyhow::Result;
use serde::{Deserialize, Serialize};

// ── types ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WebhookProvider {
    GitHub,
    GitLab,
    Bitbucket,
    /// Provider-agnostic build-completion trigger (`/webhooks/ci`). Fired by an
    /// upstream CI build finishing rather than by a git push, so it carries no
    /// native provider payload — any CI system (incl. very old pipelines) can
    /// post a small signed JSON body.
    Ci,
}

/// What kind of repository activity triggered a webhook. Lets a schedule fire
/// only on the events an operator opted into (e.g. "PR merged into main" or
/// "a release was published") rather than on every push.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TriggerEvent {
    /// A branch push (ordinary commit).
    Push,
    /// A pull / merge request merged into its target branch.
    PrMerge,
    /// A tag was pushed.
    Tag,
    /// A release was published.
    Release,
}

impl TriggerEvent {
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Push => "Commit push",
            Self::PrMerge => "PR / MR merged",
            Self::Tag => "Tag pushed",
            Self::Release => "Release published",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookEvent {
    pub provider: WebhookProvider,
    pub repo_url: String,
    pub branch: String,
    pub commit_sha: String,
    pub pusher: Option<String>,
    /// The activity that produced this event (push by default for back-compat).
    #[serde(default = "default_push_event")]
    pub event: TriggerEvent,
}

const fn default_push_event() -> TriggerEvent {
    TriggerEvent::Push
}

// ── HMAC-SHA256 verification ──────────────────────────────────────────────────

/// Verify a GitHub-style `sha256=<hex>` HMAC-SHA256 signature.
/// Returns `false` for any malformed input rather than erroring.
#[must_use]
pub fn verify_github_sig(body: &[u8], sig_header: &str, secret: &str) -> bool {
    use ring::hmac;

    let Some(hex_sig) = sig_header.strip_prefix("sha256=") else {
        return false;
    };
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let computed = hmac::sign(&key, body);
    let expected_hex = bytes_to_hex(computed.as_ref());
    constant_eq_str(&expected_hex, hex_sig)
}

/// Bitbucket uses the same HMAC-SHA256 scheme as GitHub.
#[must_use]
pub fn verify_bitbucket_sig(body: &[u8], sig_header: &str, secret: &str) -> bool {
    verify_github_sig(body, sig_header, secret)
}

/// Compute the HMAC-SHA256 of `msg` keyed by `secret`, returned as a lowercase
/// hex string. Shared helper so other crates can build keyed integrity chains
/// without taking their own crypto dependency.
#[must_use]
pub fn hmac_sha256_hex(secret: &[u8], msg: &[u8]) -> String {
    use ring::hmac;
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
    let tag = hmac::sign(&key, msg);
    bytes_to_hex(tag.as_ref())
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            write!(s, "{b:02x}").expect("write to String is infallible");
            s
        })
}

fn constant_eq_str(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

// ── payload parsers ───────────────────────────────────────────────────────────

/// Parse a GitHub `push` webhook payload.
///
/// # Errors
/// Returns an error if the body is not valid JSON or required fields are missing.
pub fn parse_github_push(body: &[u8]) -> Result<WebhookEvent> {
    let v: serde_json::Value = serde_json::from_slice(body)?;
    let repo_url = require_str(&v, &["repository", "clone_url"], "repository.clone_url")?;
    let ref_str = v["ref"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing field: ref"))?;
    let (branch, is_tag) = strip_ref(ref_str);
    let commit_sha = v["after"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing field: after"))?
        .to_owned();
    let pusher = v["pusher"]["name"].as_str().map(str::to_owned);
    Ok(WebhookEvent {
        provider: WebhookProvider::GitHub,
        repo_url,
        branch,
        commit_sha,
        pusher,
        event: if is_tag {
            TriggerEvent::Tag
        } else {
            TriggerEvent::Push
        },
    })
}

/// Parse a GitHub `pull_request` webhook payload. Only a *merged* PR (action
/// `closed` with `merged == true`) yields a `PrMerge` event; anything else
/// returns an error so the caller can ignore it.
///
/// # Errors
/// Returns an error if the body is not valid JSON, the PR was not merged, or
/// required fields are missing.
pub fn parse_github_pull_request(body: &[u8]) -> Result<WebhookEvent> {
    let v: serde_json::Value = serde_json::from_slice(body)?;
    let action = v["action"].as_str().unwrap_or_default();
    let merged = v["pull_request"]["merged"].as_bool().unwrap_or(false);
    if action != "closed" || !merged {
        anyhow::bail!("pull_request event is not a merge (action={action}, merged={merged})");
    }
    let repo_url = require_str(&v, &["repository", "clone_url"], "repository.clone_url")?;
    // The target branch the PR merged into (scan reflects the post-merge state).
    let branch = require_str(
        &v,
        &["pull_request", "base", "ref"],
        "pull_request.base.ref",
    )?;
    let commit_sha = require_str(
        &v,
        &["pull_request", "merge_commit_sha"],
        "pull_request.merge_commit_sha",
    )?;
    let pusher = v["pull_request"]["user"]["login"]
        .as_str()
        .map(str::to_owned);
    Ok(WebhookEvent {
        provider: WebhookProvider::GitHub,
        repo_url,
        branch,
        commit_sha,
        pusher,
        event: TriggerEvent::PrMerge,
    })
}

/// Parse a GitHub `release` webhook payload. Only a `published` release yields a
/// `Release` event; the scanned ref is the release tag.
///
/// # Errors
/// Returns an error if the body is not valid JSON, the release was not
/// published, or required fields are missing.
pub fn parse_github_release(body: &[u8]) -> Result<WebhookEvent> {
    let v: serde_json::Value = serde_json::from_slice(body)?;
    let action = v["action"].as_str().unwrap_or_default();
    if action != "published" {
        anyhow::bail!("release event is not a publish (action={action})");
    }
    let repo_url = require_str(&v, &["repository", "clone_url"], "repository.clone_url")?;
    let tag = require_str(&v, &["release", "tag_name"], "release.tag_name")?;
    let pusher = v["release"]["author"]["login"].as_str().map(str::to_owned);
    Ok(WebhookEvent {
        provider: WebhookProvider::GitHub,
        repo_url,
        branch: tag.clone(),
        commit_sha: tag,
        pusher,
        event: TriggerEvent::Release,
    })
}

/// Parse a GitLab `push` webhook payload.
///
/// # Errors
/// Returns an error if the body is not valid JSON or required fields are missing.
pub fn parse_gitlab_push(body: &[u8]) -> Result<WebhookEvent> {
    let v: serde_json::Value = serde_json::from_slice(body)?;
    let repo_url = require_str(&v, &["project", "git_http_url"], "project.git_http_url")?;
    let ref_str = v["ref"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing field: ref"))?;
    let (branch, is_tag) = strip_ref(ref_str);
    let commit_sha = v["checkout_sha"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing field: checkout_sha"))?
        .to_owned();
    let pusher = v["user_username"].as_str().map(str::to_owned);
    Ok(WebhookEvent {
        provider: WebhookProvider::GitLab,
        repo_url,
        branch,
        commit_sha,
        pusher,
        event: if is_tag {
            TriggerEvent::Tag
        } else {
            TriggerEvent::Push
        },
    })
}

/// Parse a Bitbucket Server / Cloud `push` webhook payload.
///
/// # Errors
/// Returns an error if the body is not valid JSON or required fields are missing.
pub fn parse_bitbucket_push(body: &[u8]) -> Result<WebhookEvent> {
    let v: serde_json::Value = serde_json::from_slice(body)?;
    let repo_url = extract_bitbucket_clone_url(&v)
        .ok_or_else(|| anyhow::anyhow!("missing field: repository.links.clone[https].href"))?;
    let push = &v["push"]["changes"][0]["new"];
    let branch = push["name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing field: push.changes[0].new.name"))?
        .to_owned();
    let commit_sha = push["target"]["hash"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing field: push.changes[0].new.target.hash"))?
        .to_owned();
    let pusher = v["actor"]["display_name"].as_str().map(str::to_owned);
    // Bitbucket marks tag refs with new.type == "tag".
    let is_tag = push["type"].as_str() == Some("tag");
    Ok(WebhookEvent {
        provider: WebhookProvider::Bitbucket,
        repo_url,
        branch,
        commit_sha,
        pusher,
        event: if is_tag {
            TriggerEvent::Tag
        } else {
            TriggerEvent::Push
        },
    })
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn require_str(v: &serde_json::Value, path: &[&str], field: &str) -> Result<String> {
    let s = path
        .iter()
        .fold(v, |cur, key| &cur[key])
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing field: {field}"))?;
    Ok(s.to_owned())
}

/// Strip a git ref down to its short name and report whether it was a tag.
/// `refs/heads/main` -> ("main", false); `refs/tags/v1.2` -> ("v1.2", true).
fn strip_ref(r: &str) -> (String, bool) {
    if let Some(tag) = r.strip_prefix("refs/tags/") {
        (tag.to_owned(), true)
    } else {
        (r.strip_prefix("refs/heads/").unwrap_or(r).to_owned(), false)
    }
}

fn extract_bitbucket_clone_url(v: &serde_json::Value) -> Option<String> {
    v["repository"]["links"]["clone"]
        .as_array()
        .and_then(|arr| arr.iter().find(|e| e["name"] == "https"))
        .and_then(|e| e["href"].as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_push_to_branch_is_push_event() {
        let body = br#"{"ref":"refs/heads/main","after":"abc123",
            "repository":{"clone_url":"https://github.com/o/r.git"},"pusher":{"name":"nima"}}"#;
        let e = parse_github_push(body).unwrap();
        assert_eq!(e.event, TriggerEvent::Push);
        assert_eq!(e.branch, "main");
    }

    #[test]
    fn github_push_to_tag_is_tag_event() {
        let body = br#"{"ref":"refs/tags/v1.2.0","after":"abc123",
            "repository":{"clone_url":"https://github.com/o/r.git"}}"#;
        let e = parse_github_push(body).unwrap();
        assert_eq!(e.event, TriggerEvent::Tag);
        assert_eq!(e.branch, "v1.2.0");
    }

    #[test]
    fn github_merged_pr_yields_prmerge_on_base_branch() {
        let body = br#"{"action":"closed","repository":{"clone_url":"https://github.com/o/r.git"},
            "pull_request":{"merged":true,"base":{"ref":"develop"},
            "merge_commit_sha":"deadbeef","user":{"login":"nima"}}}"#;
        let e = parse_github_pull_request(body).unwrap();
        assert_eq!(e.event, TriggerEvent::PrMerge);
        assert_eq!(e.branch, "develop");
        assert_eq!(e.commit_sha, "deadbeef");
    }

    #[test]
    fn github_unmerged_pr_close_is_rejected() {
        let body = br#"{"action":"closed","repository":{"clone_url":"https://github.com/o/r.git"},
            "pull_request":{"merged":false,"base":{"ref":"main"},"merge_commit_sha":null}}"#;
        assert!(parse_github_pull_request(body).is_err());
    }

    #[test]
    fn github_published_release_yields_release_on_tag() {
        let body =
            br#"{"action":"published","repository":{"clone_url":"https://github.com/o/r.git"},
            "release":{"tag_name":"v2.0.0","author":{"login":"nima"}}}"#;
        let e = parse_github_release(body).unwrap();
        assert_eq!(e.event, TriggerEvent::Release);
        assert_eq!(e.branch, "v2.0.0");
        assert_eq!(e.commit_sha, "v2.0.0");
    }

    #[test]
    fn github_draft_release_is_rejected() {
        let body = br#"{"action":"created","repository":{"clone_url":"https://github.com/o/r.git"},
            "release":{"tag_name":"v2.0.0"}}"#;
        assert!(parse_github_release(body).is_err());
    }
}
