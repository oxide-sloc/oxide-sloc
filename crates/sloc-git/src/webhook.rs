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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookEvent {
    pub provider: WebhookProvider,
    pub repo_url: String,
    pub branch: String,
    pub commit_sha: String,
    pub pusher: Option<String>,
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
    let branch = strip_refs_heads(ref_str);
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
    let branch = strip_refs_heads(ref_str);
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
    Ok(WebhookEvent {
        provider: WebhookProvider::Bitbucket,
        repo_url,
        branch,
        commit_sha,
        pusher,
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

fn strip_refs_heads(r: &str) -> String {
    r.strip_prefix("refs/heads/").unwrap_or(r).to_owned()
}

fn extract_bitbucket_clone_url(v: &serde_json::Value) -> Option<String> {
    v["repository"]["links"]["clone"]
        .as_array()
        .and_then(|arr| arr.iter().find(|e| e["name"] == "https"))
        .and_then(|e| e["href"].as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
