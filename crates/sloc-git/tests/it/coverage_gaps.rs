// SPDX-License-Identifier: AGPL-3.0-or-later
// Coverage-gap tests for sloc-git: real-repository ref/commit/worktree operations,
// clone-URL SSRF validation, and webhook helper/parse edge cases.

use std::path::Path;
use std::process::Command;

use sloc_git::ops::resolve_committish;
use sloc_git::{
    clone_or_fetch, create_worktree, destroy_worktree, get_sha, hmac_sha256_hex, list_commits,
    list_refs, parse_bitbucket_push, parse_gitlab_push,
};

// ── real-repository fixtures ──────────────────────────────────────────────────

/// True when a usable `git` executable is on PATH. Tests that need a repo skip
/// gracefully otherwise, matching the crate's best-effort convention.
fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "user.email=t@t", "-c", "user.name=Tester"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git command should run");
    assert!(
        status.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}

/// Build a small git repo with one commit on the default branch and a tag.
fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    run(p, &["init"]);
    std::fs::write(p.join("a.txt"), "hello\n").unwrap();
    run(p, &["add", "a.txt"]);
    run(p, &["commit", "-m", "initial commit"]);
    run(p, &["tag", "v1.0.0"]);
    dir
}

#[test]
fn list_refs_and_commits_on_real_repo() {
    if !git_available() {
        eprintln!("git not available — skipping");
        return;
    }
    let repo = init_repo();
    let p = repo.path();

    // list_refs exercises list_branches, list_tags, and list_commits together.
    let refs = list_refs(p).expect("list_refs should succeed");
    assert!(
        refs.tags.iter().any(|t| t.name == "v1.0.0"),
        "tag v1.0.0 should be listed: {:?}",
        refs.tags
    );
    assert!(
        !refs.recent_commits.is_empty(),
        "at least one recent commit expected"
    );

    // Direct list_commits with an explicit limit.
    let commits = list_commits(p, "HEAD", 5).expect("list_commits should succeed");
    assert_eq!(commits.len(), 1, "one commit in the fixture repo");
    assert_eq!(commits[0].subject, "initial commit");
    assert!(!commits[0].sha.is_empty(), "commit SHA must be populated");
}

#[test]
fn get_sha_and_resolve_committish_on_real_repo() {
    if !git_available() {
        eprintln!("git not available — skipping");
        return;
    }
    let repo = init_repo();
    let p = repo.path();

    let head = get_sha(p, "HEAD").expect("HEAD should resolve");
    assert_eq!(head.len(), 40, "full SHA should be 40 hex chars: {head}");

    // resolve_committish peels the tag to its commit.
    let tag_commit = resolve_committish(p, "v1.0.0").expect("tag should resolve");
    assert_eq!(tag_commit, head, "the tag points at HEAD in this fixture");

    // A ref that does not exist errors on every candidate spelling.
    let missing = resolve_committish(p, "no-such-ref");
    assert!(missing.is_err(), "unknown ref must error");
}

#[test]
fn create_and_destroy_worktree_on_real_repo() {
    if !git_available() {
        eprintln!("git not available — skipping");
        return;
    }
    let repo = init_repo();
    let p = repo.path();

    let wt_parent = tempfile::tempdir().unwrap();
    let wt = wt_parent.path().join("checkout");

    create_worktree(p, "HEAD", &wt).expect("worktree add should succeed");
    assert!(
        wt.join("a.txt").exists(),
        "checked-out file should be present in the worktree"
    );

    // destroy_worktree always returns Ok, even on a second call.
    destroy_worktree(p, &wt).expect("worktree remove should return Ok");
    destroy_worktree(p, &wt).expect("second remove is a no-op that still returns Ok");
}

// ── clone_or_fetch SSRF validation (no network required) ──────────────────────

#[test]
fn clone_or_fetch_rejects_plaintext_http_scheme() {
    let dir = tempfile::tempdir().unwrap();
    let err = clone_or_fetch("http://example.com/repo.git", &dir.path().join("x")).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("URL rejected") || msg.contains("permitted"),
        "http:// should be rejected: {msg}"
    );
}

#[test]
fn clone_or_fetch_rejects_loopback_host() {
    let dir = tempfile::tempdir().unwrap();
    // https scheme is allowed, but the loopback host is SSRF-blocked.
    let err = clone_or_fetch("https://127.0.0.1/repo.git", &dir.path().join("x")).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("loopback") || msg.contains("rejected"),
        "loopback host should be rejected: {msg}"
    );
}

#[test]
fn clone_or_fetch_rejects_metadata_host() {
    let dir = tempfile::tempdir().unwrap();
    let err = clone_or_fetch(
        "https://metadata.google.internal/repo.git",
        &dir.path().join("x"),
    )
    .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("rejected"), "metadata host rejected: {msg}");
}

// ── webhook helpers / parse edge cases ────────────────────────────────────────

#[test]
fn hmac_sha256_hex_is_deterministic_lowercase_hex() {
    let a = hmac_sha256_hex(b"key", b"message");
    let b = hmac_sha256_hex(b"key", b"message");
    assert_eq!(a, b, "same input yields the same digest");
    assert_eq!(a.len(), 64, "SHA-256 hex is 64 chars");
    assert!(
        a.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "digest must be lowercase hex: {a}"
    );
    // A different key changes the digest.
    assert_ne!(a, hmac_sha256_hex(b"other", b"message"));
}

#[test]
fn parse_gitlab_push_missing_repo_url_errors() {
    // No project.git_http_url → require_str error path.
    let body = br#"{"ref":"refs/heads/main","checkout_sha":"deadbeef"}"#;
    assert!(parse_gitlab_push(body).is_err());
}

#[test]
fn parse_bitbucket_push_missing_clone_url_errors() {
    // repository.links.clone lacks an https entry → extract_bitbucket_clone_url None.
    let body = br#"{
      "actor": {"display_name": "x"},
      "repository": {"links": {"clone": [{"name":"ssh","href":"git@bb:ws/r.git"}]}},
      "push": {"changes": [{"new": {"name":"main","target":{"hash":"abc"}}}]}
    }"#;
    assert!(parse_bitbucket_push(body).is_err());
}

#[test]
fn parse_bitbucket_push_missing_branch_name_errors() {
    // Valid clone url but no branch name → push.changes[0].new.name error path.
    let body = br#"{
      "repository": {"links": {"clone": [{"name":"https","href":"https://bb/ws/r.git"}]}},
      "push": {"changes": [{"new": {"target":{"hash":"abc"}}}]}
    }"#;
    assert!(parse_bitbucket_push(body).is_err());
}
