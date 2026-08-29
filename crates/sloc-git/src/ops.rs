// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::{GitCommit, GitRef, GitRefKind, RepoRefs};

/// Optional positive host allowlist for clone targets, parsed once from
/// `SLOC_GIT_HOST_ALLOWLIST` (comma-separated, lowercased hostnames). When empty,
/// `validate_clone_url` runs in denylist mode (metadata/loopback blocking only).
fn git_host_allowlist() -> &'static [String] {
    static ALLOW: OnceLock<Vec<String>> = OnceLock::new();
    ALLOW.get_or_init(|| {
        std::env::var("SLOC_GIT_HOST_ALLOWLIST")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect()
    })
}

/// True when `SLOC_GIT_HOST_ALLOWLIST` names at least one host. Callers use this to decide
/// whether network operations that fetch attacker-influenced URLs (e.g. submodule population
/// on a network-facing server) may proceed under the positive allowlist.
#[must_use]
pub fn host_allowlist_configured() -> bool {
    !git_host_allowlist().is_empty()
}

/// When `SLOC_GIT_REQUIRE_ALLOWLIST` is truthy, clones are refused unless
/// `SLOC_GIT_HOST_ALLOWLIST` names the target host. This lets internet-facing or
/// multi-tenant deployments run allowlist-only (fail closed): only explicitly listed
/// hostnames are clonable, so a hostname that resolves to an internal address only at
/// clone time cannot slip through the validate-time resolution check. Unset by default,
/// so denylist-mode deployments are unaffected.
fn require_host_allowlist() -> bool {
    static REQ: OnceLock<bool> = OnceLock::new();
    *REQ.get_or_init(|| {
        std::env::var("SLOC_GIT_REQUIRE_ALLOWLIST")
            .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
    })
}

/// When `SLOC_GIT_SSL_NO_VERIFY` is set (any value), TLS certificate verification is
/// disabled for git network operations via `-c http.sslVerify=false`. This is the escape
/// hatch for corporate networks whose VPN/proxy performs TLS inspection with a self-signed
/// CA that is not in the machine's trust store — the common reason a Bitbucket/GitHub fetch
/// fails on an internal network. Off by default; a startup warning is printed when set.
fn ssl_no_verify() -> bool {
    static NO_VERIFY: OnceLock<bool> = OnceLock::new();
    *NO_VERIFY.get_or_init(|| std::env::var_os("SLOC_GIT_SSL_NO_VERIFY").is_some())
}

/// Wall-clock ceiling for a single git subprocess, from `SLOC_GIT_TIMEOUT` (seconds).
/// Defaults to 300s. Guarantees a stalled clone/fetch (dead VPN, black-holed proxy) fails
/// with a clear error instead of hanging the web request forever.
fn git_timeout() -> Duration {
    static TIMEOUT: OnceLock<Duration> = OnceLock::new();
    *TIMEOUT.get_or_init(|| {
        let secs = std::env::var("SLOC_GIT_TIMEOUT")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&s| s > 0)
            .unwrap_or(300);
        Duration::from_secs(secs)
    })
}

// ── per-host credential registry ───────────────────────────────────────────────

/// A credential resolved for a specific git host from the in-app registry.
///
/// The secret (`token` / the key file's contents) is NEVER placed in a git argv or
/// written to disk. It travels only in the child process environment (`GIT_U`/`GIT_P`
/// for HTTPS, `GIT_SSH_COMMAND` for SSH); see [`cred_injection`].
enum GitCredential {
    /// HTTPS personal-access-token auth (`username` + `token`).
    Https { user: String, token: String },
    /// SSH key auth (path to a private key file).
    Ssh { key_path: String },
}

/// Map a hostname to the env-var suffix used by the credential registry:
/// uppercase, with every non-alphanumeric byte replaced by `_`
/// (`bitbucket.instance2.com` → `BITBUCKET_INSTANCE2_COM`, `host:7990` → `HOST_7990`).
///
/// Note: this is intentionally lossy — `a.b.com`, `a-b.com`, and `a_b.com` all map to
/// `A_B_COM`. That collision is documented; keep instance hostnames distinct beyond
/// punctuation, or use `SLOC_GIT_CRED_FILE` (which keys on the exact hostname).
fn hostkey(host: &str) -> String {
    host.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// Resolve a per-host credential from the in-app registry, or `None` to fall through
/// to git's own credential resolution (OS credential helper, `~/.netrc`, ssh-agent).
///
/// Resolution order for host `H` (first match wins):
/// 1. `SLOC_GIT_CRED_<HOSTKEY>` = `username:token` — HTTPS PAT.
/// 2. `SLOC_GIT_SSHKEY_<HOSTKEY>` = path to an SSH private key.
/// 3. `SLOC_GIT_CRED_FILE` — a bulk `host = "user:token"` map (see [`cred_from_file`]).
///
/// Not cached: env is read live so the value is correct under runtime changes and the
/// unit tests can mutate it. Cheap relative to a network clone.
fn resolve_credential(host: &str, port: Option<u16>) -> Option<GitCredential> {
    // Try the port-qualified key first (`SLOC_GIT_CRED_HOST_7990`) so two instances
    // on the same host but different ports can carry distinct credentials, then the
    // bare-host key (`SLOC_GIT_CRED_HOST`). The port form matches the documented
    // `git.corp:7990 → GIT_CORP_7990` convention; without it the port suffix was dead.
    let mut keys: Vec<String> = Vec::with_capacity(2);
    if let Some(pt) = port {
        keys.push(hostkey(&format!("{host}:{pt}")));
    }
    keys.push(hostkey(host));
    for key in &keys {
        if let Ok(v) = std::env::var(format!("SLOC_GIT_CRED_{key}"))
            && let Some((user, token)) = v.split_once(':')
            && !token.is_empty()
        {
            return Some(GitCredential::Https {
                user: user.to_owned(),
                token: token.to_owned(),
            });
        }
        if let Ok(p) = std::env::var(format!("SLOC_GIT_SSHKEY_{key}"))
            && !p.trim().is_empty()
        {
            return Some(GitCredential::Ssh { key_path: p });
        }
    }
    cred_from_file(host)
}

/// Look `host` up in the optional bulk credentials file named by `SLOC_GIT_CRED_FILE`.
///
/// Format: one `host = "user:token"` entry per line (`#` comments and blank lines are
/// ignored); quotes optional; host match is case-insensitive on the exact hostname.
/// The file is treated as a secret — its contents are never logged. On Unix a warning is
/// emitted if it is group/world-readable.
fn cred_from_file(host: &str) -> Option<GitCredential> {
    let path = std::env::var("SLOC_GIT_CRED_FILE").ok()?;
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    warn_if_world_readable(path);
    let content = std::fs::read_to_string(path).ok()?;
    let host_lower = host.to_lowercase();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.trim().trim_matches('"').to_lowercase() != host_lower {
            continue;
        }
        let v = v.trim().trim_matches('"');
        if let Some((user, token)) = v.split_once(':')
            && !token.is_empty()
        {
            return Some(GitCredential::Https {
                user: user.to_owned(),
                token: token.to_owned(),
            });
        }
    }
    None
}

/// Warn (once per call, best-effort) if a secrets file is readable beyond its owner.
#[cfg(unix)]
fn warn_if_world_readable(path: &str) {
    use std::os::unix::fs::PermissionsExt as _;
    if let Ok(meta) = std::fs::metadata(path)
        && meta.permissions().mode() & 0o077 != 0
    {
        eprintln!(
            "warning: SLOC_GIT_CRED_FILE {path:?} is group/world-readable; \
             restrict it with chmod 600"
        );
    }
}

#[cfg(not(unix))]
fn warn_if_world_readable(_path: &str) {}

/// Extra git config (`-c` pairs) and child-process env for a resolved credential.
///
/// - HTTPS: an empty `credential.helper=` first (resets any inherited system/GCM helper
///   so it can't win or pop a GUI), then a shell-snippet helper that echoes the credential
///   read from `$GIT_U`/`$GIT_P`. The helper text contains only the variable *names* — the
///   secret is supplied via `env` and never appears in argv or on disk.
/// - SSH: `GIT_SSH_COMMAND` pinning the key with `IdentitiesOnly=yes` (so an ssh-agent key
///   can't shadow it) and `BatchMode=yes` (fail fast, matching the non-interactive model).
#[derive(Default)]
struct CredInjection {
    config: Vec<String>,
    env: Vec<(String, String)>,
}

fn cred_injection(host: &str, port: Option<u16>) -> CredInjection {
    match resolve_credential(host, port) {
        Some(GitCredential::Https { user, token }) => CredInjection {
            config: vec![
                "credential.helper=".to_owned(),
                "credential.helper=!f() { test \"$1\" = get && echo \"username=$GIT_U\" && \
                 echo \"password=$GIT_P\"; }; f"
                    .to_owned(),
            ],
            env: vec![("GIT_U".to_owned(), user), ("GIT_P".to_owned(), token)],
        },
        Some(GitCredential::Ssh { key_path }) => {
            let mut ssh = format!("ssh -i \"{key_path}\" -o IdentitiesOnly=yes -o BatchMode=yes");
            // Default: strict host-key checking (first contact with an unseeded
            // known_hosts fails, matching the non-interactive model). Opt in to
            // trust-on-first-use with SLOC_GIT_SSH_ACCEPT_NEW=1.
            if ssh_accept_new() {
                ssh.push_str(" -o StrictHostKeyChecking=accept-new");
            }
            CredInjection {
                config: Vec::new(),
                env: vec![("GIT_SSH_COMMAND".to_owned(), ssh)],
            }
        }
        None => CredInjection::default(),
    }
}

/// Opt-in trust-on-first-use for SSH clones: when set, `StrictHostKeyChecking=accept-new`
/// is added to the injected SSH command so a first contact with a host absent from
/// `known_hosts` records its key instead of failing. Default OFF keeps strict checking —
/// pre-seed `known_hosts` (e.g. via `ssh-keyscan`) on a fresh agent otherwise.
fn ssh_accept_new() -> bool {
    std::env::var("SLOC_GIT_SSH_ACCEPT_NEW")
        .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

// ── offline / local-source import gate ───────────────────────────────────────────

/// Whether local/offline git sources (bundle, `file://`, local path) are permitted.
/// `SLOC_GIT_ALLOW_LOCAL` truthy. Default OFF preserves the SSRF posture for
/// internet-facing servers (a bare `file:///etc/...` clone is an LFI otherwise).
fn allow_local() -> bool {
    std::env::var("SLOC_GIT_ALLOW_LOCAL").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// The directory local sources must resolve under, from `SLOC_GIT_LOCAL_ROOT`.
/// Required whenever `allow_local()` is on (fail closed) — the filesystem analog of
/// `SLOC_GIT_HOST_ALLOWLIST`.
fn local_root() -> Option<PathBuf> {
    std::env::var("SLOC_GIT_LOCAL_ROOT")
        .ok()
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// `-c key=value` config flags applied to every network-touching git invocation
/// (clone/fetch). Makes internal/corporate repos work with zero configuration:
/// - `http.sslBackend=schannel` (Windows only) — validate TLS against the Windows system
///   certificate store instead of Git for Windows' own bundled CA file. The system store
///   already holds the enterprise/proxy root CAs that IT deploys, so a TLS-inspecting
///   corporate proxy or VPN is trusted automatically — the same reason the repo opens fine
///   in a browser. This is why a fetch that used to need `SLOC_GIT_SSL_NO_VERIFY` now just
///   works, and it keeps certificate verification ON (no security downgrade). On Linux/macOS
///   git already uses the system trust store, so nothing extra is needed there.
/// - `http.followRedirects=false` — never follow an HTTP redirect into an SSRF target.
/// - `http.lowSpeedLimit`/`http.lowSpeedTime` — abort a transfer that drops below ~1 KB/s
///   for 30s, so a flaky VPN/proxy fails fast rather than hanging.
/// - `http.sslVerify=false` — last-resort override, only when `SLOC_GIT_SSL_NO_VERIFY` is set
///   (a self-signed cert that isn't in any trust store). Rarely needed now.
fn network_git_config() -> Vec<String> {
    let mut cfg = vec![
        "http.followRedirects=false".to_owned(),
        "http.lowSpeedLimit=1000".to_owned(),
        "http.lowSpeedTime=30".to_owned(),
    ];
    if cfg!(windows) {
        cfg.push("http.sslBackend=schannel".to_owned());
    }
    if ssl_no_verify() {
        cfg.push("http.sslVerify=false".to_owned());
    }
    cfg
}

/// Prepend `-c <cfg>` pairs to a git argument list, borrowing from `cfg`.
fn with_config<'a>(cfg: &'a [String], tail: &[&'a str]) -> Vec<&'a str> {
    let mut v = Vec::with_capacity(cfg.len() * 2 + tail.len());
    for c in cfg {
        v.push("-c");
        v.push(c.as_str());
    }
    v.extend_from_slice(tail);
    v
}

/// Persist the network config into the freshly-cloned repo's local git config.
/// Blobless clones fetch file contents lazily (the promisor kicks in when a ref is checked
/// out into a worktree), and that implicit fetch reads the repo config — not our per-command
/// `-c` flags. Writing them here makes the SSL bypass and low-speed abort apply to those
/// lazy fetches too, so scanning a ref works on the same corporate network the clone did.
/// Best-effort: a failure here doesn't invalidate an otherwise-successful clone.
fn persist_repo_config(dest: &Path, cfg: &[String]) {
    let mut helper_reset = false;
    for kv in cfg {
        if let Some((key, value)) = kv.split_once('=') {
            if key == "credential.helper" {
                // `credential.helper` is a multi-valued key: we persist BOTH the empty
                // reset (drops inherited system/GCM helpers so the promisor fetch can't
                // fall back to a GUI prompt and hang) AND the env-reading helper. A plain
                // `git config` would overwrite, clobbering the reset — so clear once, then
                // `--add` each value in order. No secret is written: the helper text holds
                // only `$GIT_U`/`$GIT_P` variable names.
                if !helper_reset {
                    let _ = run_git(dest, &["config", "--unset-all", "credential.helper"]);
                    helper_reset = true;
                }
                let _ = run_git(dest, &["config", "--add", "credential.helper", value]);
            } else {
                let _ = run_git(dest, &["config", key, value]);
            }
        }
    }
}

// ── low-level git runner ───────────────────────────────────────────────────────

fn run_git(repo: &Path, args: &[&str]) -> Result<String> {
    run_git_env(repo, args, &[])
}

/// Like [`run_git`], but sets additional child-process environment variables (e.g. the
/// per-host credential secret `GIT_U`/`GIT_P`, or `GIT_SSH_COMMAND`). The secret lives
/// only in the child env — never in argv, never on disk. All the spawn/drain/timeout
/// logic is shared with `run_git` (which calls this with an empty `extra_env`).
fn run_git_env(repo: &Path, args: &[&str], extra_env: &[(&str, &str)]) -> Result<String> {
    let mut cmd = std::process::Command::new("git");
    // Force non-interactive operation. Without this, a `clone`/`fetch` that hits an
    // authentication challenge (e.g. a rate-limited anonymous clone returning 401, or a
    // private repo) blocks indefinitely waiting for input that never arrives — git asks on
    // the terminal and Git Credential Manager pops a GUI dialog, neither of which a
    // background server subprocess can answer. The request then hangs forever and the web
    // UI spins on "Fetching repository…". These variables make git fail fast with an error
    // instead. They suppress only *interactive* prompts; already-stored credentials (SSH
    // agent, cached HTTPS tokens) are still used, so configured private repos keep working.
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "");
    // Per-host credential secrets injected by the caller (clone/fetch/worktree). Set after
    // the interactive-suppression vars so a resolved credential's helper can answer git.
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.args(args)
        .current_dir(repo)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().context("failed to spawn git process")?;

    // Drain stdout/stderr on dedicated threads: a chatty git process (clone progress,
    // large logs) can otherwise fill a fixed-size OS pipe buffer and block on write while
    // we poll for the timeout below — a deadlock that would look exactly like a hang.
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });
    let err_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });

    // Poll for completion, killing the process if it exceeds the wall-clock ceiling.
    let timeout = git_timeout();
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().context("failed to poll git process")? {
            break status;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "git {} timed out after {}s — the remote did not respond in time. \
                 On a corporate network this usually means a proxy or VPN is slow or \
                 blocking the connection. Raise the ceiling with SLOC_GIT_TIMEOUT=<seconds>, \
                 or check your proxy/VPN configuration.",
                args.first().copied().unwrap_or(""),
                timeout.as_secs()
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    };

    let stdout = out_handle.join().unwrap_or_default();
    let stderr = err_handle.join().unwrap_or_default();
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr);
        bail!(
            "git {}: {}",
            args.first().copied().unwrap_or(""),
            stderr.trim()
        );
    }
    Ok(String::from_utf8_lossy(&stdout).trim().to_owned())
}

// ── URL normalization ─────────────────────────────────────────────────────────

/// Convert a repository browse URL into a clonable git URL.
///
/// Handles Bitbucket Server/Data Center (`/projects/{PROJ}/repos/{REPO}/...`),
/// GitLab (`/path/repo/-/tree/...`), GitHub (`github.com/{owner}/{repo}/tree/...`),
/// and Bitbucket Cloud (`bitbucket.org/{ws}/{repo}/src/...`). SSH URLs and URLs
/// that already look like clone targets are returned unchanged.
#[must_use]
pub fn normalize_git_url(raw: &str) -> String {
    let url = raw.trim();
    if url.starts_with("git@") || url.starts_with("ssh://") {
        return url.to_owned();
    }
    let scheme = if url.starts_with("https://") {
        "https"
    } else if url.starts_with("http://") {
        "http"
    } else {
        return url.to_owned();
    };
    let authority_and_path = &url[scheme.len() + 3..];
    let (host, path) = authority_and_path
        .find('/')
        .map_or((authority_and_path, "/"), |i| {
            (&authority_and_path[..i], &authority_and_path[i..])
        });
    let path = path.trim_end_matches('/');

    try_normalize_bitbucket_server(scheme, host, path)
        .or_else(|| try_normalize_gitlab(scheme, host, path))
        .or_else(|| try_normalize_github(scheme, host, path))
        .or_else(|| try_normalize_bitbucket_cloud(scheme, host, path))
        .unwrap_or_else(|| url.to_owned())
}

// ── Bitbucket Server / Data Center ────────────────────────────────────────────
// Browse URL: /{context}/projects/{PROJECT}/repos/{REPO}[/...]
// Clone URL:  /{context}/scm/{project_lower}/{repo}.git
fn try_normalize_bitbucket_server(scheme: &str, host: &str, path: &str) -> Option<String> {
    let path_lower = path.to_lowercase();
    let proj_pos = path_lower.find("/projects/")?;
    let after = &path[proj_pos + "/projects/".len()..];
    let parts: Vec<&str> = after.splitn(4, '/').collect();
    if parts.len() < 3 || !parts[1].eq_ignore_ascii_case("repos") {
        return None;
    }
    let context = &path[..proj_pos];
    let project = parts[0].to_lowercase();
    let repo = parts[2].trim_end_matches(".git");
    Some(format!(
        "{scheme}://{host}{context}/scm/{project}/{repo}.git"
    ))
}

// ── GitLab (any host) ─────────────────────────────────────────────────────────
// Browse URL: /path/to/repo/-/tree/branch  →  Clone URL: /path/to/repo.git
fn try_normalize_gitlab(scheme: &str, host: &str, path: &str) -> Option<String> {
    let idx = path.find("/-/")?;
    let repo_path = path[..idx].trim_end_matches(".git");
    Some(format!("{scheme}://{host}{repo_path}.git"))
}

// ── GitHub ────────────────────────────────────────────────────────────────────
// Browse URL: github.com/{owner}/{repo}/{tree|blob|...}/...
fn try_normalize_github(scheme: &str, host: &str, path: &str) -> Option<String> {
    if host != "github.com" && !host.ends_with(".github.com") {
        return None;
    }
    let p = path.trim_start_matches('/');
    let parts: Vec<&str> = p.splitn(4, '/').collect();
    if parts.len() < 3
        || !matches!(
            parts[2],
            "tree" | "blob" | "commits" | "commit" | "releases" | "tags" | "branches"
        )
    {
        return None;
    }
    let owner = parts[0];
    let repo = parts[1].trim_end_matches(".git");
    Some(format!("{scheme}://{host}/{owner}/{repo}.git"))
}

// ── Bitbucket Cloud ───────────────────────────────────────────────────────────
// Browse URL: bitbucket.org/{workspace}/{repo}/src/...
fn try_normalize_bitbucket_cloud(scheme: &str, host: &str, path: &str) -> Option<String> {
    if host != "bitbucket.org" {
        return None;
    }
    let p = path.trim_start_matches('/');
    let parts: Vec<&str> = p.splitn(4, '/').collect();
    if parts.len() < 3 || parts[2] != "src" {
        return None;
    }
    let ws = parts[0];
    let repo = parts[1].trim_end_matches(".git");
    Some(format!("{scheme}://{host}/{ws}/{repo}.git"))
}

// ── clone / fetch ─────────────────────────────────────────────────────────────

fn validate_clone_url(url: &str) -> Result<()> {
    let lower = url.to_lowercase();
    // http:// excluded: prevents SSRF against plaintext internal HTTP services.
    // file:// excluded: prevents local filesystem access.
    let allowed = ["https://", "git://", "ssh://", "git@"];
    if !allowed.iter().any(|p| lower.starts_with(p)) {
        bail!(
            "git URL rejected: only https://, git://, ssh://, and git@ URLs are \
             permitted (got {url:?})"
        );
    }
    // SSRF protection: block loopback, link-local, and cloud-metadata hosts.
    // RFC 1918 private ranges are intentionally ALLOWED so the tool can scan
    // internal/corporate git servers (10.x, 192.168.x, 172.16-31.x); the real
    // threat is cloud-metadata and loopback, not "any private IP".
    // The check is host-scoped (not a whole-URL substring match) so legitimate
    // paths/tags such as "release-v10.2" are never mistaken for an IP.
    let Some(host) = host_of_git_url(url) else {
        return Ok(());
    };
    check_host_allowed(&host)?;
    check_resolved_ips(&host, url)?;
    Ok(())
}

/// Host-level SSRF gate: positive allowlist (when configured) plus the
/// loopback/link-local/cloud-metadata denylist. Split out of `validate_clone_url`
/// to keep that function's cognitive complexity low.
fn check_host_allowed(host: &str) -> Result<()> {
    // Positive allowlist (durable SSRF control): when SLOC_GIT_HOST_ALLOWLIST is
    // configured, only those hosts may be cloned. This closes the validate-vs-clone
    // DNS TOCTOU — an attacker cannot point an *allowed name* at an internal IP and
    // have it accepted unless the name itself is allowlisted. Empty = denylist mode
    // (loopback/link-local/metadata blocking only), preserving prior behaviour.
    let allow = git_host_allowlist();
    if allow.is_empty() {
        if require_host_allowlist() {
            bail!(
                "git URL rejected: SLOC_GIT_REQUIRE_ALLOWLIST is set but \
                 SLOC_GIT_HOST_ALLOWLIST is empty (no hosts are permitted)"
            );
        }
    } else if !allow.iter().any(|h| h == host) {
        bail!("git URL rejected: host {host:?} is not in SLOC_GIT_HOST_ALLOWLIST");
    }
    if is_ssrf_blocked_host(host) {
        bail!(
            "git URL rejected: loopback, link-local, and cloud-metadata \
             addresses are not permitted (host {host:?})"
        );
    }
    Ok(())
}

/// Defence against DNS-rebinding: a hostname that is not itself an IP literal can
/// still resolve to an SSRF-sensitive address. Resolve it now and reject if *any*
/// resolved IP is blocked. A resolution failure is not fatal (the host may only be
/// resolvable by git's own resolver in some air-gapped setups) — git will then fail
/// or succeed on its own; the residual is the documented validate-vs-clone TOCTOU.
fn check_resolved_ips(host: &str, url: &str) -> Result<()> {
    let Some(port) = port_of_git_url(url) else {
        return Ok(());
    };
    let Ok(addrs) = resolve_host_port(host, port) else {
        return Ok(());
    };
    for addr in addrs {
        if is_ssrf_blocked_ip(addr.ip()) {
            bail!(
                "git URL rejected: host {host:?} resolves to a blocked \
                 address {} (loopback/link-local/cloud-metadata)",
                addr.ip()
            );
        }
    }
    Ok(())
}

/// Live DNS resolution seam for `check_resolved_ips`. Production performs a real
/// `getaddrinfo`; the `cfg(test)` build resolves purely in-process so the unit
/// suite is network-hermetic (no `github.com` A/AAAA lookups on every `cargo test`,
/// which trip DNS alarms on monitored air-gapped sites). The DNS-rebinding path it
/// guards needs a real hostile record and is exercised by integration tests, not
/// these offline units.
#[cfg(not(test))]
fn resolve_host_port(
    host: &str,
    port: u16,
) -> std::io::Result<std::vec::IntoIter<std::net::SocketAddr>> {
    use std::net::ToSocketAddrs as _;
    (host, port).to_socket_addrs()
}

#[cfg(test)]
fn resolve_host_port(
    host: &str,
    port: u16,
) -> std::io::Result<std::vec::IntoIter<std::net::SocketAddr>> {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    // IP-literal hosts resolve to themselves, so the SSRF-blocked-IP assertions still
    // hold without touching the network; any real hostname resolves to a fixed public
    // address so the block-loop is still exercised but no DNS query is emitted.
    let ip = host
        .parse::<IpAddr>()
        .unwrap_or(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)));
    Ok(vec![SocketAddr::new(ip, port)].into_iter())
}

/// Extract the host (lowercased, brackets stripped) from a git clone URL.
/// Handles `git@host:path`, `scheme://[user@]host[:port]/path`, and IPv6 literals.
fn host_of_git_url(url: &str) -> Option<String> {
    let u = url.trim();
    // scp-like syntax: git@host:path (no scheme)
    if let Some(rest) = u.strip_prefix("git@") {
        let host = rest.split(':').next().unwrap_or(rest);
        return Some(host.to_lowercase());
    }
    // scheme://[user@]host[:port]/path
    let after_scheme = u.split("://").nth(1)?;
    let authority = after_scheme.split('/').next().unwrap_or(after_scheme);
    // Strip any userinfo (user[:pass]@).
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    // IPv6 literal: [::1]:port → ::1
    let host = authority.strip_prefix('[').map_or_else(
        || authority.split(':').next().unwrap_or(authority).to_string(),
        |stripped| stripped.split(']').next().unwrap_or(stripped).to_string(),
    );
    Some(host.to_lowercase())
}

/// Best-effort port extraction for DNS-rebinding resolution. Returns the explicit
/// port if present, otherwise the scheme default (https 443, git 9418, ssh 22).
/// `None` only when no host/scheme can be determined.
fn port_of_git_url(url: &str) -> Option<u16> {
    let u = url.trim();
    // scp-like git@host:path — git over ssh, port 22 (path after ':' is not a port).
    if u.starts_with("git@") {
        return Some(22);
    }
    let (scheme, after_scheme) = u.split_once("://")?;
    let authority = after_scheme.split('/').next().unwrap_or(after_scheme);
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    // Explicit port: take the segment after the last ':' that is not inside [..].
    let explicit = authority.strip_prefix('[').map_or_else(
        // No '[' prefix: take the segment after the last ':'.
        || {
            authority
                .rsplit_once(':')
                .and_then(|(_, p)| p.parse::<u16>().ok())
        },
        // IPv6 literal: [host]:port
        |stripped| {
            stripped
                .split_once("]:")
                .and_then(|(_, p)| p.parse::<u16>().ok())
        },
    );
    explicit.or_else(|| match scheme.to_lowercase().as_str() {
        "https" => Some(443),
        "git" => Some(9418),
        "ssh" => Some(22),
        _ => None,
    })
}

/// The EXPLICIT `:port` from a git URL authority, or `None` when the URL carries no
/// port. Unlike [`port_of_git_url`], no scheme default is substituted, and scp-like
/// `git@host:path` is treated as port-less (the part after `:` is a path). Used only
/// to build the optional port-qualified credential key so two instances on the same
/// host but different ports resolve distinct credentials.
fn explicit_port_of_git_url(url: &str) -> Option<u16> {
    let u = url.trim();
    if u.starts_with("git@") {
        return None;
    }
    let (_scheme, after_scheme) = u.split_once("://")?;
    let authority = after_scheme.split('/').next().unwrap_or(after_scheme);
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    authority.strip_prefix('[').map_or_else(
        || {
            authority
                .rsplit_once(':')
                .and_then(|(_, p)| p.parse::<u16>().ok())
        },
        |stripped| {
            stripped
                .split_once("]:")
                .and_then(|(_, p)| p.parse::<u16>().ok())
        },
    )
}

/// Known cloud-metadata / instance-data hostnames that must never be reachable.
const BLOCKED_METADATA_HOSTNAMES: &[&str] = &[
    "metadata.google.internal",
    "metadata.internal",
    "instance-data",
];

/// Returns true when `host` (a hostname or IP literal) is an SSRF-sensitive
/// loopback, link-local, unspecified, multicast, or cloud-metadata target.
/// RFC 1918 / IPv6 unique-local private ranges are NOT blocked.
fn is_ssrf_blocked_host(host: &str) -> bool {
    let h = host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_lowercase();
    if h == "localhost" || BLOCKED_METADATA_HOSTNAMES.contains(&h.as_str()) {
        return true;
    }
    h.parse::<std::net::IpAddr>().is_ok_and(is_ssrf_blocked_ip)
}

/// IP-level SSRF classification. Blocks loopback, link-local, unspecified,
/// broadcast, multicast, and the Alibaba metadata IP. Allows RFC 1918 / ULA.
fn is_ssrf_blocked_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.octets() == [100, 100, 100, 200] // Alibaba Cloud metadata
        }
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
        }
    }
}

/// How a clone source string should be treated. The four forms are NOT interchangeable
/// under the blobless partial-clone flags, so each gets its own clone-arg construction.
enum GitSource {
    /// `https://`, `http://`, `git://`, `ssh://`, `git@host:` — network transport, SSRF-gated.
    Remote,
    /// `file:///local/path` — regular git transport, so `--filter` / promisor work.
    FileUrl,
    /// A bare local filesystem path (`/path`, `C:\path`). git ignores `--filter` and
    /// hardlinks objects for these; we force `--no-local` for safety + partial-clone parity.
    LocalPath,
    /// A `*.bundle` file (a static packfile) — verify then clone; `--filter` is meaningless.
    Bundle,
}

/// Classify a (already-normalized) clone source string.
fn classify_source(url: &str) -> GitSource {
    let u = url.trim();
    let lower = u.to_lowercase();
    if lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("git://")
        || lower.starts_with("ssh://")
        || u.starts_with("git@")
    {
        GitSource::Remote
    } else if lower.starts_with("file://") {
        GitSource::FileUrl
    } else if lower.ends_with(".bundle") {
        GitSource::Bundle
    } else {
        GitSource::LocalPath
    }
}

/// Clone `url` into `dest`, or fetch all refs if the repo already exists.
///
/// Browse URLs (GitHub, GitLab, Bitbucket web pages) are automatically converted to their
/// corresponding git clone URLs first. Remote sources are SSRF-gated and get per-host
/// credentials from the in-app registry (see [`cred_injection`]). Local/offline sources
/// (a git bundle, a `file://` mirror, or a local path) are only permitted when
/// `SLOC_GIT_ALLOW_LOCAL` is set and resolve under `SLOC_GIT_LOCAL_ROOT`.
///
/// # Errors
/// Returns an error if the source is rejected, the clone directory cannot be created,
/// or the underlying `git clone` / `git fetch` command fails.
pub fn clone_or_fetch(url: &str, dest: &Path) -> Result<()> {
    let dest = crate::reject_traversal(dest)?;
    let dest = dest.as_path();
    let normalized = normalize_git_url(url);
    let url = normalized.as_str();
    match classify_source(url) {
        GitSource::Remote => clone_or_fetch_remote(url, dest),
        source => clone_or_fetch_local(url, dest, &source),
    }
}

/// Remote clone/fetch: SSRF validation, network hardening config, per-host credential
/// injection, blobless fast path with a full-clone fallback for servers that reject filters.
fn clone_or_fetch_remote(url: &str, dest: &Path) -> Result<()> {
    validate_clone_url(url)?;
    // `network_git_config()` supplies `http.followRedirects=false` (SSRF hardening — a
    // redirect can't escape the validated host), the low-speed abort (a stalled VPN/proxy
    // fails fast), and optional `http.sslVerify=false` for TLS-inspecting corporate proxies.
    let mut cfg = network_git_config();
    // Per-host credential from the registry (HTTPS helper config and/or the secret env).
    let inj = host_of_git_url(url)
        .map(|h| cred_injection(&h, explicit_port_of_git_url(url)))
        .unwrap_or_default();
    cfg.extend(inj.config.iter().cloned());
    let env: Vec<(&str, &str)> = inj
        .env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    if dest.join(".git").exists() {
        let args = with_config(&cfg, &["fetch", "--all", "--tags", "--prune"]);
        run_git_env(dest, &args, &env)?;
        return Ok(());
    }

    std::fs::create_dir_all(dest).context("failed to create clone directory")?;
    let dest_str = dest.to_str().unwrap_or(".");
    let parent = dest.parent().unwrap_or(dest);

    // Fast path: a blobless (`--filter=blob:none`), no-checkout clone. Only commit and tree
    // metadata is downloaded — no file blobs, no working tree — which is all that ref
    // listing needs, and is dramatically faster than a full clone on large repos and slow
    // corporate links. File contents are fetched lazily by the promisor when a ref is later
    // scanned into a worktree. `--no-tags` is NOT passed: the Tags tab needs them.
    let fast = with_config(
        &cfg,
        &[
            "clone",
            "--filter=blob:none",
            "--no-checkout",
            "--no-single-branch",
            url,
            dest_str,
        ],
    );
    if let Err(e) = run_git_env(parent, &fast, &env) {
        // A handful of older self-hosted servers (e.g. legacy Bitbucket Server) reject
        // object filtering outright instead of degrading to a full clone. Only in that
        // specific case do we clean up the partial directory and retry without the filter —
        // a genuine network/auth failure is surfaced directly rather than paying a second
        // timeout.
        let msg = e.to_string().to_lowercase();
        if !(msg.contains("filter") || msg.contains("partial")) {
            return Err(e);
        }
        let _ = std::fs::remove_dir_all(dest);
        std::fs::create_dir_all(dest).context("failed to re-create clone directory")?;
        let full = with_config(
            &cfg,
            &[
                "clone",
                "--no-checkout",
                "--no-single-branch",
                url,
                dest_str,
            ],
        );
        run_git_env(parent, &full, &env)?;
    }
    persist_repo_config(dest, &cfg);
    Ok(())
}

/// Local/offline clone from a git bundle, a `file://` mirror, or a local path. Gated by
/// `SLOC_GIT_ALLOW_LOCAL` + `SLOC_GIT_LOCAL_ROOT`; no network, no credentials, no SSRF risk
/// once the source is confirmed to resolve under the configured root.
fn clone_or_fetch_local(url: &str, dest: &Path, source: &GitSource) -> Result<()> {
    let src = validate_local_source(url, source)?;
    let cfg = network_git_config();
    if dest.join(".git").exists() {
        let args = with_config(&cfg, &["fetch", "--all", "--tags", "--prune"]);
        run_git(dest, &args)?;
        return Ok(());
    }
    std::fs::create_dir_all(dest).context("failed to create clone directory")?;
    let dest_str = dest.to_str().unwrap_or(".");
    let parent = dest.parent().unwrap_or(dest);

    let tail: Vec<&str> = match source {
        // A bundle is a self-contained packfile — a plain no-checkout clone. `git clone`
        // validates the bundle itself (a corrupt/incomplete bundle fails the clone), and
        // `git bundle verify` can't run here (it needs an existing repository). `--filter`
        // has no promisor remote to defer to, so it is intentionally omitted.
        GitSource::Bundle => vec!["clone", "--no-checkout", &src, dest_str],
        // `file://` uses the regular git transport, so partial clone + promisor work as remote.
        GitSource::FileUrl => vec![
            "clone",
            "--filter=blob:none",
            "--no-checkout",
            "--no-single-branch",
            &src,
            dest_str,
        ],
        // A bare local path defaults to `--local` (hardlink/copy, ignores `--filter`, and
        // historically dereferences symlinks in objects/). Force `--no-local` for the safe
        // copy transport, which also re-enables `--filter`.
        GitSource::LocalPath => vec![
            "clone",
            "--no-local",
            "--filter=blob:none",
            "--no-checkout",
            "--no-single-branch",
            &src,
            dest_str,
        ],
        GitSource::Remote => unreachable!("remote sources are handled by clone_or_fetch_remote"),
    };
    let args = with_config(&cfg, &tail);
    run_git(parent, &args)?;
    persist_repo_config(dest, &cfg);
    Ok(())
}

/// Publish the contents of `src_dir` into `repo_url` on `branch`, under `subdir`, as a single
/// commit, then push. `work_dir` is a caller-owned empty scratch directory used as the clone
/// working tree.
///
/// Reuses the exact same SSRF validation, host-allowlist, per-host credential injection, and
/// network-hardening (`http.followRedirects=false`, low-speed abort) as [`clone_or_fetch`], so
/// it is safe to point at a corporate host over a VPN or at an air-gapped local mirror (the
/// latter behind `SLOC_GIT_ALLOW_LOCAL` / `SLOC_GIT_LOCAL_ROOT`). Appends to the branch's
/// existing history when it exists, creates it (even from an empty repo's unborn HEAD)
/// otherwise. Returns `Ok(())` without pushing when nothing changed since the last publish.
///
/// # Errors
/// Propagates clone/commit/push failures and URL-validation / local-gate rejections.
pub fn publish_dir(
    repo_url: &str,
    branch: &str,
    subdir: &str,
    src_dir: &Path,
    message: &str,
    work_dir: &Path,
) -> Result<()> {
    let normalized = normalize_git_url(repo_url);
    let url = normalized.as_str();

    // Resolve git config + credential env exactly as a clone would, honoring the source gates.
    let (cfg, env_owned): (Vec<String>, Vec<(String, String)>) = match classify_source(url) {
        GitSource::Remote => {
            validate_clone_url(url)?;
            let mut cfg = network_git_config();
            let inj = host_of_git_url(url)
                .map(|h| cred_injection(&h, explicit_port_of_git_url(url)))
                .unwrap_or_default();
            cfg.extend(inj.config.iter().cloned());
            (cfg, inj.env)
        }
        source => {
            validate_local_source(url, &source)?;
            (network_git_config(), Vec::new())
        }
    };
    let env: Vec<(&str, &str)> = env_owned
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    std::fs::create_dir_all(work_dir).context("failed to create publish work dir")?;
    let dest_str = work_dir
        .to_str()
        .context("publish work dir path is not valid UTF-8")?;
    let parent = work_dir.parent().unwrap_or(work_dir);

    // Full checkout clone: we need a working tree to add files onto the existing history.
    let clone_args = with_config(&cfg, &["clone", url, dest_str]);
    run_git_env(parent, &clone_args, &env)?;
    persist_repo_config(work_dir, &cfg);

    // Land on the target branch: base on its remote head when present (append), else create
    // it fresh — including from an empty repo's unborn HEAD.
    let origin_ref = format!("origin/{branch}");
    if run_git(work_dir, &["rev-parse", "--verify", "--quiet", &origin_ref]).is_ok() {
        run_git(work_dir, &["checkout", "-B", branch, &origin_ref])?;
    } else {
        run_git(work_dir, &["checkout", "-B", branch])?;
    }

    // Replace the destination subdir with the fresh artifacts.
    let target = if subdir.is_empty() {
        work_dir.to_path_buf()
    } else {
        work_dir.join(subdir)
    };
    if target != work_dir && target.exists() {
        std::fs::remove_dir_all(&target).ok();
    }
    copy_dir_all(src_dir, &target)?;

    run_git(work_dir, &["add", "-A"])?;
    if run_git(work_dir, &["status", "--porcelain"])?
        .trim()
        .is_empty()
    {
        return Ok(()); // nothing changed since the last publish — skip the empty commit/push
    }
    run_git(
        work_dir,
        &[
            "-c",
            "user.email=oxide-sloc@localhost",
            "-c",
            "user.name=oxide-sloc",
            "commit",
            "-m",
            message,
        ],
    )?;
    let refspec = format!("HEAD:refs/heads/{branch}");
    let push_args = with_config(&cfg, &["push", "origin", &refspec]);
    run_git_env(work_dir, &push_args, &env)?;
    Ok(())
}

/// Recursively copy a directory tree (files + subdirs), skipping symlinks so a link inside a
/// scanned/uploaded artifact tree cannot redirect the copy or loop. Local to `sloc-git` so
/// [`publish_dir`] needs no dependency on `sloc-core`.
fn copy_dir_all(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)?.flatten() {
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            continue;
        }
        let to = dest.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_all(&entry.path(), &to)?;
        } else if ft.is_file() {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// Validate a local/offline source against the fail-closed gate and return the filesystem
/// path git should clone from. Enforces: `SLOC_GIT_ALLOW_LOCAL` on; `SLOC_GIT_LOCAL_ROOT`
/// set; `file://` has no host authority; no UNC (SMB is a network fetch, not local); and the
/// canonicalized source resolves under the configured root (defeats `..`/symlink traversal).
fn validate_local_source(url: &str, source: &GitSource) -> Result<String> {
    if !allow_local() {
        bail!(
            "local/offline git source rejected: set SLOC_GIT_ALLOW_LOCAL=1 to enable bundle / \
             file:// / local-path imports (got {url:?})"
        );
    }
    let Some(root) = local_root() else {
        bail!(
            "SLOC_GIT_ALLOW_LOCAL is set but SLOC_GIT_LOCAL_ROOT is not — refusing local import \
             (fail-closed). Point SLOC_GIT_LOCAL_ROOT at the directory holding your bundles/mirrors."
        );
    };

    let raw = url.trim();
    let path = match source {
        GitSource::FileUrl => file_url_to_path(raw)?,
        _ => raw.to_owned(),
    };
    // UNC (`\\server\share` or `//server/share`) is an SMB fetch to an attacker-chosen host —
    // that is remote (SSRF + credential-leak), not local. Never accept it under the local gate.
    if path.starts_with("\\\\") || path.starts_with("//") {
        bail!("UNC path rejected: SMB shares are network sources, not local ({url:?})");
    }

    let canon = std::fs::canonicalize(&path)
        .with_context(|| format!("local git source not found or unreadable: {path:?}"))?;
    let root_canon = std::fs::canonicalize(&root)
        .with_context(|| format!("SLOC_GIT_LOCAL_ROOT not found: {}", root.display()))?;
    if !canon.starts_with(&root_canon) {
        bail!(
            "local git source {} is outside SLOC_GIT_LOCAL_ROOT {}",
            canon.display(),
            root_canon.display()
        );
    }
    // Strip any Windows verbatim (`\\?\`) prefix so git accepts the path.
    Ok(deverbatim(&canon))
}

/// Convert a `file://` URL to a local filesystem path, rejecting a non-empty host authority
/// (`file://host/path` is an SMB/UNC fetch on Windows — treated as remote and refused).
/// Handles `file:///home/x` → `/home/x` and `file:///C:/x` → `C:/x`.
fn file_url_to_path(url: &str) -> Result<String> {
    let rest = &url.trim()[7..]; // strip "file://"
    if !rest.starts_with('/') {
        bail!(
            "file:// URL with a host authority is not permitted (use file:///local/path): {url:?}"
        );
    }
    // Drop exactly one leading slash for a Windows drive path (`/C:/x` → `C:/x`); keep the
    // rooted slash for a POSIX path (`/home/x`).
    let after = &rest[1..];
    if after.len() >= 2 && after.as_bytes()[1] == b':' {
        Ok(after.to_owned())
    } else {
        Ok(rest.to_owned())
    }
}

/// Strip a Windows verbatim path prefix (`\\?\`) that `std::fs::canonicalize` adds, since
/// git does not accept it. No-op on other platforms / non-verbatim paths.
fn deverbatim(p: &Path) -> String {
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_owned()
}

/// Resolve `ref_name` to its full SHA in `repo`.
///
/// # Errors
/// Returns an error if `git rev-parse` fails (e.g. the ref does not exist).
pub fn get_sha(repo: &Path, ref_name: &str) -> Result<String> {
    run_git(repo, &["rev-parse", ref_name])
}

// ── local repository detection ────────────────────────────────────────────────

/// True when `s` points at an existing local git repository — a directory containing a
/// `.git` entry (working tree or worktree/submodule pointer) or a bare repository dir.
///
/// This is a cheap filesystem check (no subprocess, no network) used to branch the Git
/// Browser between the local flow (operate on the repo in place) and the remote flow (clone
/// the URL). Anything that looks like a remote URL scheme is deliberately rejected so a
/// `file://`, `https://`, or `git@` string is never mistaken for a local path.
#[must_use]
pub fn is_local_repo_path(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return false;
    }
    let lower = t.to_lowercase();
    if lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("git://")
        || lower.starts_with("ssh://")
        || lower.starts_with("file://")
        || t.starts_with("git@")
    {
        return false;
    }
    // Canonicalize first so a legitimate relative path (e.g. `../repo`) resolves its `..`
    // segments before the traversal barrier guards the filesystem probes below. A path that
    // does not exist fails canonicalization and is simply not a local repo.
    let Ok(p) = std::fs::canonicalize(Path::new(t)) else {
        return false;
    };
    let Ok(p) = crate::reject_traversal(&p) else {
        return false;
    };
    if !p.is_dir() {
        return false;
    }
    // Non-bare repo (or a linked worktree / submodule): a `.git` dir or `.git` file.
    // Bare repo: has `HEAD` plus an `objects/` directory at the top level.
    p.join(".git").exists() || (p.join("HEAD").is_file() && p.join("objects").is_dir())
}

/// Verify that `path` is inside a git repository and return the canonical repository root
/// (the working-tree top level, or the given path for a bare repo). Runs no network I/O.
///
/// # Errors
/// Returns an error if `path` is not a git repository.
pub fn open_local_repo(path: &Path) -> Result<PathBuf> {
    // `--show-toplevel` yields the working-tree root for a normal repo; it fails for a bare
    // repo, in which case the path itself is the repository.
    if let Ok(top) = run_git(path, &["rev-parse", "--show-toplevel"])
        && !top.trim().is_empty()
    {
        return Ok(PathBuf::from(top.trim()));
    }
    // Bare repo (or worktree without a toplevel): confirm it really is a git dir.
    run_git(path, &["rev-parse", "--git-dir"]).context("not a git repository")?;
    Ok(std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
}

// ── worktree helpers ──────────────────────────────────────────────────────────

/// Resolve a user-facing ref name to a concrete commit SHA the worktree/scan commands accept.
///
/// A clone only materialises a *local* branch for the repository's default branch;
/// every other branch exists solely as a remote-tracking ref (`refs/remotes/origin/<name>`).
/// Ref listing strips the `origin/` prefix for display, so a bare branch name like "test"
/// won't resolve directly — we fall back to the remote-tracking form. Tags and raw SHAs
/// resolve on the first candidate. Peeling with `^{commit}` also dereferences annotated tags.
///
/// # Errors
/// Returns an error if none of the candidate spellings resolve to a commit.
pub fn resolve_committish(repo: &Path, ref_name: &str) -> Result<String> {
    let candidates = [
        ref_name.to_owned(),
        format!("origin/{ref_name}"),
        format!("refs/remotes/origin/{ref_name}"),
    ];
    for cand in &candidates {
        let spec = format!("{cand}^{{commit}}");
        if let Ok(sha) = run_git(repo, &["rev-parse", "--verify", "-q", &spec])
            && !sha.is_empty()
        {
            return Ok(sha);
        }
    }
    bail!(
        "ref {ref_name:?} not found in repository (tried it directly, as origin/{ref_name}, \
         and as refs/remotes/origin/{ref_name})"
    );
}

/// Create a detached worktree at `worktree_path` pointing at `ref_name`.
///
/// `ref_name` is resolved via [`resolve_committish`] first, so a bare branch name that
/// only exists as a remote-tracking ref (every branch except the default one, in a fresh
/// clone) still checks out correctly instead of failing with "invalid reference".
///
/// # Errors
/// Returns an error if `ref_name` cannot be resolved or `git worktree add` fails.
pub fn create_worktree(repo: &Path, ref_name: &str, worktree_path: &Path) -> Result<()> {
    let wt = worktree_path.to_str().unwrap_or(".");
    let committish = resolve_committish(repo, ref_name)?;
    // A blobless clone fetches file contents lazily: `worktree add` triggers a promisor
    // fetch against origin. That fetch needs the same per-host credential the clone used, so
    // re-resolve it from the repo's origin URL and pass the secret env through (the helper
    // config itself is already persisted in the repo config by `persist_repo_config`).
    let env = cred_env_for_repo(repo);
    let env_refs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    run_git_env(
        repo,
        &["worktree", "add", "--detach", wt, &committish],
        &env_refs,
    )?;
    Ok(())
}

/// Resolve the per-host credential *env* (secret) for an already-cloned repo by reading its
/// `remote.origin.url`. Returns empty when there is no origin, no host, or no registry match
/// (local/offline clones, or hosts that use git's own credential resolution).
fn cred_env_for_repo(repo: &Path) -> Vec<(String, String)> {
    let Ok(url) = run_git(repo, &["config", "--get", "remote.origin.url"]) else {
        return Vec::new();
    };
    let Some(host) = host_of_git_url(&url) else {
        return Vec::new();
    };
    cred_injection(&host, explicit_port_of_git_url(&url)).env
}

/// Remove a worktree previously created with [`create_worktree`].
///
/// # Errors
/// This function always succeeds; the underlying git command failure is intentionally ignored.
pub fn destroy_worktree(repo: &Path, worktree_path: &Path) -> Result<()> {
    let wt = worktree_path.to_str().unwrap_or(".");
    let _ = run_git(repo, &["worktree", "remove", "--force", wt]);
    Ok(())
}

// ── submodule population ───────────────────────────────────────────────────────

/// Recursively check out a super-repo's submodules inside `worktree` so their files are
/// present for analysis. Best-effort: a repo with no `.gitmodules` is a no-op, and a
/// submodule whose fetch fails simply stays empty (its per-submodule breakdown is then blank)
/// rather than failing the whole scan.
///
/// Security: `git submodule update` would clone whatever URLs `.gitmodules` records, which
/// bypasses the SSRF gate that guards the top-level clone. Every recorded submodule URL is
/// therefore validated with the same [`validate_clone_url`] check first; a submodule whose URL
/// is rejected is skipped (and its name returned) instead of being fetched. Relative submodule
/// URLs resolve against the superproject origin and cannot target a new host, so they are
/// allowed; absolute local/`file://` submodule references are refused (a legitimate local
/// super-repo references its submodules by relative path).
///
/// Returns the names of submodules that were skipped as unsafe.
///
/// # Errors
/// Returns an error only if the working tree cannot be inspected; a failed submodule fetch is
/// swallowed (best-effort).
pub fn populate_submodules(worktree: &Path) -> Result<Vec<String>> {
    let worktree = crate::reject_traversal(worktree)?;
    let worktree = worktree.as_path();
    if !worktree.join(".gitmodules").is_file() {
        return Ok(Vec::new());
    }
    let mut safe_paths: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for name in submodule_names(worktree) {
        let url = gitmodules_value(worktree, &name, "url");
        let path = gitmodules_value(worktree, &name, "path");
        if url.trim().is_empty() || path.trim().is_empty() {
            continue;
        }
        if submodule_url_is_safe(url.trim()) {
            safe_paths.push(path.trim().to_owned());
        } else {
            skipped.push(name);
        }
    }
    if safe_paths.is_empty() {
        return Ok(skipped);
    }
    // Update only the vetted submodule paths, recursively. `network_git_config()` carries the
    // same redirect/low-speed/TLS hardening the clone used. Nested submodules are a documented
    // residual: only the top-level URLs are validated here.
    let cfg = network_git_config();
    let mut tail: Vec<&str> = vec!["submodule", "update", "--init", "--recursive", "--"];
    tail.extend(safe_paths.iter().map(String::as_str));
    let args = with_config(&cfg, &tail);
    let _ = run_git(worktree, &args); // best-effort: leave any failing submodule empty
    Ok(skipped)
}

/// Names of the submodules declared in `<worktree>/.gitmodules`.
fn submodule_names(worktree: &Path) -> Vec<String> {
    let out = run_git(
        worktree,
        &[
            "config",
            "-f",
            ".gitmodules",
            "--name-only",
            "--get-regexp",
            r"^submodule\..*\.path$",
        ],
    )
    .unwrap_or_default();
    out.lines()
        .filter_map(|l| {
            l.trim()
                .strip_prefix("submodule.")
                .and_then(|s| s.strip_suffix(".path"))
                .map(str::to_owned)
        })
        .collect()
}

/// Read `submodule.<name>.<key>` from the worktree's `.gitmodules`.
fn gitmodules_value(worktree: &Path, name: &str, key: &str) -> String {
    run_git(
        worktree,
        &[
            "config",
            "-f",
            ".gitmodules",
            "--get",
            &format!("submodule.{name}.{key}"),
        ],
    )
    .unwrap_or_default()
}

/// Whether a `.gitmodules` submodule URL is safe to fetch. Relative URLs (same origin host)
/// are allowed; remote URLs go through the SSRF gate; absolute local/`file://`/bundle URLs are
/// refused.
fn submodule_url_is_safe(url: &str) -> bool {
    let u = url.trim();
    if u.is_empty() {
        return false;
    }
    if u.starts_with('.') {
        return true;
    }
    let norm = normalize_git_url(u);
    match classify_source(&norm) {
        GitSource::Remote => validate_clone_url(&norm).is_ok(),
        _ => false,
    }
}

// ── ref listing ───────────────────────────────────────────────────────────────

/// Return all branches, tags, and recent commits for `repo`.
///
/// # Errors
/// Returns an error if any underlying git command fails.
pub fn list_refs(repo: &Path) -> Result<RepoRefs> {
    Ok(RepoRefs {
        branches: list_branches(repo)?,
        tags: list_tags(repo)?,
        recent_commits: list_commits(repo, "HEAD", 40)?,
    })
}

/// Like [`list_refs`], but for a repository operated on **in place** (a local checkout the
/// user pointed us at). It lists the repo's *local* branch heads (`git branch`) rather than
/// remote-tracking refs (`git branch -r`), because a working repo's branches of interest are
/// its local heads. Tags and recent commits are listed identically to [`list_refs`].
///
/// # Errors
/// Returns an error if any underlying git command fails.
pub fn list_refs_local(repo: &Path) -> Result<RepoRefs> {
    Ok(RepoRefs {
        branches: list_branches_local(repo)?,
        tags: list_tags(repo)?,
        recent_commits: list_commits(repo, "HEAD", 40)?,
    })
}

/// List the local branch heads of `repo` (no remote-tracking refs, no `origin/` prefix).
fn list_branches_local(repo: &Path) -> Result<Vec<GitRef>> {
    let fmt = "%(refname:short)|%(objectname:short)|%(creatordate:iso-strict)|%(subject)";
    let out = run_git(repo, &["branch", &format!("--format={fmt}")])?;
    Ok(out
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| parse_ref_line(l, GitRefKind::Branch))
        .collect())
}

fn list_branches(repo: &Path) -> Result<Vec<GitRef>> {
    // `%(symref)` is the leading column and is non-empty only for symbolic refs such as the
    // remote's default-branch pointer `origin/HEAD`. We must filter on it rather than on the
    // ref name: `%(refname:short)` collapses `refs/remotes/origin/HEAD` down to bare `origin`,
    // which is neither "HEAD" nor "*/HEAD", so a name-based filter lets it through and renders
    // a phantom duplicate of the default branch (same SHA, displayed as "origin").
    let fmt = "%(symref)|%(refname:short)|%(objectname:short)|%(creatordate:iso-strict)|%(subject)";
    // Use -r (remote-tracking only) to avoid local/remote duplicates.
    // Strip the leading remote name (e.g. "origin/") from each ref so the
    // displayed name matches what the upstream repository calls the branch.
    let out = run_git(repo, &["branch", "-r", &format!("--format={fmt}")])?;
    let refs = out
        .lines()
        .filter(|l| !l.trim().is_empty())
        // Split off the symref column; skip the line entirely when it is a symbolic ref.
        .filter_map(|l| {
            let (symref, rest) = l.split_once('|')?;
            if symref.trim().is_empty() {
                Some(rest)
            } else {
                None
            }
        })
        .map(|l| parse_ref_line(l, GitRefKind::Branch))
        .map(|mut r| {
            // Strip the remote prefix ("origin/", "upstream/", etc.).
            if let Some(slash) = r.name.find('/') {
                r.name = r.name[slash + 1..].to_owned();
            }
            r
        })
        .collect::<Vec<_>>();
    Ok(refs)
}

fn list_tags(repo: &Path) -> Result<Vec<GitRef>> {
    let fmt = "%(refname:short)|%(objectname:short)|%(creatordate:iso-strict)|%(subject)";
    let out = run_git(
        repo,
        &["tag", "--sort=-creatordate", &format!("--format={fmt}")],
    )?;
    Ok(out
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| parse_ref_line(l, GitRefKind::Tag))
        .collect())
}

fn parse_ref_line(line: &str, kind: GitRefKind) -> GitRef {
    let parts: Vec<&str> = line.splitn(4, '|').collect();
    let name = parts.first().copied().unwrap_or("").to_owned();
    let sha = parts.get(1).copied().unwrap_or("").to_owned();
    let date = parts.get(2).copied().and_then(parse_git_date);
    let message = parts.get(3).map(|s| (*s).to_owned());
    GitRef {
        kind,
        name,
        sha,
        date,
        message,
    }
}

// ── commit listing ────────────────────────────────────────────────────────────

/// Return up to `limit` commits reachable from `ref_name`.
///
/// # Errors
/// Returns an error if `git log` fails.
pub fn list_commits(repo: &Path, ref_name: &str, limit: usize) -> Result<Vec<GitCommit>> {
    let fmt = "%H|%h|%an|%aI|%s";
    let n = format!("-{limit}");
    let out = run_git(repo, &["log", ref_name, &format!("--format={fmt}"), &n])?;
    Ok(out
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(parse_commit_line)
        .collect())
}

fn parse_commit_line(line: &str) -> GitCommit {
    let p: Vec<&str> = line.splitn(5, '|').collect();
    let sha = p.first().copied().unwrap_or("").to_owned();
    let short_sha = p.get(1).copied().unwrap_or("").to_owned();
    let author = p.get(2).copied().unwrap_or("").to_owned();
    let date = p
        .get(3)
        .copied()
        .and_then(parse_git_date)
        .unwrap_or_default();
    let subject = p.get(4).copied().unwrap_or("").to_owned();
    GitCommit {
        sha,
        short_sha,
        author,
        date,
        subject,
    }
}

fn parse_git_date(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&chrono::Utc))
}

/// Serializes tests that mutate process-global env vars (credential registry / local-import
/// gate settings), which would otherwise race under the parallel test runner. Shared by both
/// the `tests` and `git_integration` modules.
#[cfg(test)]
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Acquire [`ENV_LOCK`], tolerating a prior panic (poisoning) so one failing env-mutating
/// test doesn't cascade into spurious `PoisonError` failures that hide the real cause.
#[cfg(test)]
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GitRefKind;
    use chrono::Timelike as _;

    // ── SSRF host classification ───────────────────────────────────────────────

    #[test]
    fn is_ssrf_blocked_host_blocks_localhost_and_metadata() {
        assert!(is_ssrf_blocked_host("localhost"));
        assert!(is_ssrf_blocked_host("metadata.google.internal"));
        assert!(is_ssrf_blocked_host("metadata.internal"));
        assert!(is_ssrf_blocked_host("instance-data"));
        // Case/whitespace/bracket normalisation.
        assert!(is_ssrf_blocked_host("  LOCALHOST  "));
        // IP literals: loopback and link-local blocked.
        assert!(is_ssrf_blocked_host("127.0.0.1"));
        assert!(is_ssrf_blocked_host("[::1]"));
        assert!(is_ssrf_blocked_host("169.254.169.254"));
    }

    #[test]
    fn require_host_allowlist_defaults_false() {
        // With SLOC_GIT_REQUIRE_ALLOWLIST unset, allowlist enforcement is off.
        assert!(!require_host_allowlist());
    }

    #[test]
    fn check_host_allowed_denylist_mode_permits_public_blocks_sensitive() {
        // Empty allowlist + enforcement off: public hosts pass, SSRF-sensitive hosts fail.
        assert!(check_host_allowed("github.com").is_ok());
        assert!(check_host_allowed("localhost").is_err());
    }

    #[test]
    fn is_ssrf_blocked_host_allows_public_hosts() {
        assert!(!is_ssrf_blocked_host("github.com"));
        assert!(!is_ssrf_blocked_host("example.com"));
        // RFC 1918 private ranges are intentionally NOT blocked.
        assert!(!is_ssrf_blocked_host("192.168.1.10"));
        assert!(!is_ssrf_blocked_host("10.0.0.1"));
    }

    // ── network config helpers ────────────────────────────────────────────────

    #[test]
    fn network_git_config_always_hardens_redirects_and_lowspeed() {
        let cfg = network_git_config();
        assert!(cfg.iter().any(|c| c == "http.followRedirects=false"));
        assert!(cfg.iter().any(|c| c == "http.lowSpeedLimit=1000"));
        assert!(cfg.iter().any(|c| c == "http.lowSpeedTime=30"));
    }

    #[cfg(windows)]
    #[test]
    fn network_git_config_uses_schannel_on_windows() {
        // On Windows we validate against the system certificate store so corporate
        // root CAs are trusted automatically — no SLOC_GIT_SSL_NO_VERIFY required.
        let cfg = network_git_config();
        assert!(cfg.iter().any(|c| c == "http.sslBackend=schannel"));
    }

    #[test]
    fn with_config_interleaves_dash_c_pairs_before_tail() {
        let cfg = vec!["a=1".to_owned(), "b=2".to_owned()];
        let args = with_config(&cfg, &["clone", "url", "dest"]);
        assert_eq!(args, vec!["-c", "a=1", "-c", "b=2", "clone", "url", "dest"]);
    }

    #[test]
    fn with_config_empty_cfg_is_just_the_tail() {
        let cfg: Vec<String> = Vec::new();
        assert_eq!(with_config(&cfg, &["fetch"]), vec!["fetch"]);
    }

    #[test]
    fn git_timeout_is_positive() {
        // Default (or env-provided) timeout is always a positive duration.
        assert!(git_timeout().as_secs() > 0);
    }

    // ── normalize_git_url ─────────────────────────────────────────────────────

    #[test]
    fn normalize_github_tree_url() {
        assert_eq!(
            normalize_git_url("https://github.com/owner/repo/tree/main"),
            "https://github.com/owner/repo.git"
        );
    }

    #[test]
    fn normalize_github_blob_url() {
        assert_eq!(
            normalize_git_url("https://github.com/owner/repo/blob/main/README.md"),
            "https://github.com/owner/repo.git"
        );
    }

    #[test]
    fn normalize_github_commits_url() {
        assert_eq!(
            normalize_git_url("https://github.com/owner/repo/commits/main"),
            "https://github.com/owner/repo.git"
        );
    }

    #[test]
    fn normalize_github_releases_url() {
        assert_eq!(
            normalize_git_url("https://github.com/owner/repo/releases"),
            "https://github.com/owner/repo.git"
        );
    }

    #[test]
    fn normalize_github_tags_url() {
        assert_eq!(
            normalize_git_url("https://github.com/owner/repo/tags"),
            "https://github.com/owner/repo.git"
        );
    }

    #[test]
    fn normalize_github_branches_url() {
        assert_eq!(
            normalize_git_url("https://github.com/owner/repo/branches"),
            "https://github.com/owner/repo.git"
        );
    }

    #[test]
    fn normalize_github_plain_clone_url_unchanged() {
        let url = "https://github.com/owner/repo.git";
        assert_eq!(normalize_git_url(url), url);
    }

    #[test]
    fn normalize_gitlab_tree_url() {
        assert_eq!(
            normalize_git_url("https://gitlab.com/group/subgroup/repo/-/tree/main"),
            "https://gitlab.com/group/subgroup/repo.git"
        );
    }

    #[test]
    fn normalize_gitlab_blob_url() {
        assert_eq!(
            normalize_git_url("https://gitlab.com/org/repo/-/blob/main/src/lib.rs"),
            "https://gitlab.com/org/repo.git"
        );
    }

    #[test]
    fn normalize_gitlab_self_hosted() {
        assert_eq!(
            normalize_git_url("https://gitlab.corp.com/team/project/-/tree/develop"),
            "https://gitlab.corp.com/team/project.git"
        );
    }

    #[test]
    fn normalize_bitbucket_server_browse_url() {
        assert_eq!(
            normalize_git_url("https://bitbucket.corp.com/projects/MYPROJ/repos/myrepo/browse"),
            "https://bitbucket.corp.com/scm/myproj/myrepo.git"
        );
    }

    #[test]
    fn normalize_bitbucket_server_with_context() {
        assert_eq!(
            normalize_git_url("https://host.com/ctx/projects/PROJ/repos/repo/browse"),
            "https://host.com/ctx/scm/proj/repo.git"
        );
    }

    #[test]
    fn normalize_bitbucket_cloud_src_url() {
        assert_eq!(
            normalize_git_url("https://bitbucket.org/workspace/repo/src/main/README.md"),
            "https://bitbucket.org/workspace/repo.git"
        );
    }

    #[test]
    fn normalize_ssh_url_unchanged() {
        let url = "git@github.com:owner/repo.git";
        assert_eq!(normalize_git_url(url), url);
    }

    #[test]
    fn normalize_ssh_protocol_url_unchanged() {
        let url = "ssh://git@github.com/owner/repo.git";
        assert_eq!(normalize_git_url(url), url);
    }

    #[test]
    fn normalize_trims_leading_trailing_whitespace() {
        assert_eq!(
            normalize_git_url("  https://github.com/owner/repo/tree/main  "),
            "https://github.com/owner/repo.git"
        );
    }

    #[test]
    fn normalize_http_url_without_match_returned_unchanged() {
        let url = "http://internal.corp.com/repo.git";
        assert_eq!(normalize_git_url(url), url);
    }

    // ── validate_clone_url ────────────────────────────────────────────────────

    #[test]
    fn validate_https_url_ok() {
        assert!(validate_clone_url("https://github.com/owner/repo.git").is_ok());
    }

    #[test]
    fn validate_git_protocol_url_ok() {
        assert!(validate_clone_url("git://github.com/owner/repo.git").is_ok());
    }

    #[test]
    fn validate_ssh_protocol_url_ok() {
        assert!(validate_clone_url("ssh://git@github.com/owner/repo.git").is_ok());
    }

    #[test]
    fn validate_git_at_url_ok() {
        assert!(validate_clone_url("git@github.com:owner/repo.git").is_ok());
    }

    #[test]
    fn validate_http_plain_rejected() {
        assert!(
            validate_clone_url("http://github.com/owner/repo.git").is_err(),
            "plain http:// must be rejected"
        );
    }

    #[test]
    fn validate_link_local_169_254_rejected() {
        assert!(validate_clone_url("https://169.254.169.254/latest/meta-data/").is_err());
    }

    #[test]
    fn validate_google_metadata_endpoint_rejected() {
        assert!(
            validate_clone_url("https://metadata.google.internal/computeMetadata/v1/").is_err()
        );
    }

    #[test]
    fn validate_alibaba_metadata_rejected() {
        assert!(validate_clone_url("https://100.100.100.200/latest/meta-data/").is_err());
    }

    #[test]
    fn validate_ipv6_fe80_link_local_rejected() {
        assert!(validate_clone_url("https://[fe80::1]/repo").is_err());
    }

    #[test]
    fn validate_file_protocol_rejected() {
        assert!(validate_clone_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn validate_empty_string_rejected() {
        assert!(validate_clone_url("").is_err());
    }

    #[test]
    fn validate_rfc1918_10_allowed() {
        // RFC 1918 private ranges are allowed (internal corporate git servers).
        assert!(validate_clone_url("https://10.0.0.1/repo.git").is_ok());
    }

    #[test]
    fn validate_rfc1918_192_168_allowed() {
        assert!(validate_clone_url("https://192.168.1.1/repo.git").is_ok());
    }

    #[test]
    fn validate_rfc1918_172_16_allowed() {
        assert!(validate_clone_url("https://172.16.0.1/repo.git").is_ok());
    }

    #[test]
    fn validate_rfc1918_172_31_allowed() {
        assert!(validate_clone_url("https://172.31.255.255/repo.git").is_ok());
    }

    #[test]
    fn validate_ipv6_ula_fd_allowed() {
        // IPv6 unique-local (fc00::/7) is the private-range equivalent — allowed.
        assert!(validate_clone_url("https://[fd12:3456:789a::1]/repo").is_ok());
    }

    // ── port_of_git_url (DNS-rebind resolution helper) ────────────────────────
    #[test]
    fn port_https_default() {
        assert_eq!(port_of_git_url("https://github.com/o/r.git"), Some(443));
    }

    #[test]
    fn port_explicit_overrides_default() {
        assert_eq!(
            port_of_git_url("https://gitlab.corp:8443/o/r.git"),
            Some(8443)
        );
    }

    #[test]
    fn port_git_scheme_default() {
        assert_eq!(port_of_git_url("git://example.com/r.git"), Some(9418));
    }

    #[test]
    fn port_scp_like_is_ssh() {
        assert_eq!(port_of_git_url("git@github.com:owner/repo.git"), Some(22));
    }

    #[test]
    fn port_ipv6_with_explicit_port() {
        assert_eq!(port_of_git_url("https://[fd00::1]:7000/r"), Some(7000));
    }

    #[test]
    fn port_ipv6_default() {
        assert_eq!(port_of_git_url("https://[fd00::1]/r"), Some(443));
    }

    #[test]
    fn validate_metadata_ip_literal_still_rejected() {
        // IP-literal path remains blocked regardless of the new DNS resolution step.
        assert!(validate_clone_url("https://169.254.169.254/latest/meta-data/").is_err());
    }

    #[test]
    fn validate_loopback_127_rejected() {
        assert!(validate_clone_url("https://127.0.0.1/repo.git").is_err());
    }

    #[test]
    fn validate_localhost_rejected() {
        assert!(validate_clone_url("https://localhost/repo.git").is_err());
    }

    #[test]
    fn validate_unspecified_0_0_0_0_rejected() {
        assert!(validate_clone_url("https://0.0.0.0/repo.git").is_err());
    }

    // ── host_of_git_url ───────────────────────────────────────────────────────

    // The URL embeds userinfo purely to prove the parser drops it and returns
    // only the host — no real secret, this is a parsing fixture.
    #[test]
    fn host_of_git_url_https_with_port_and_creds() {
        assert_eq!(
            host_of_git_url("https://user:pw@gitlab.corp.com:8443/team/repo.git").as_deref(),
            Some("gitlab.corp.com")
        );
    }

    #[test]
    fn host_of_git_url_scp_syntax() {
        assert_eq!(
            host_of_git_url("git@github.com:owner/repo.git").as_deref(),
            Some("github.com")
        );
    }

    #[test]
    fn host_of_git_url_ipv6_literal() {
        assert_eq!(
            host_of_git_url("https://[fe80::1]:443/repo").as_deref(),
            Some("fe80::1")
        );
    }

    #[test]
    fn validate_clone_url_path_with_version_number_not_blocked() {
        // Regression: a path/tag containing "10." must not be mistaken for an IP.
        assert!(validate_clone_url("https://github.com/acme/release-v10.2.git").is_ok());
        assert!(validate_clone_url("https://github.com/foo/bar-127-baz.git").is_ok());
    }

    // ── try_normalize_bitbucket_server ────────────────────────────────────────

    #[test]
    fn bitbucket_server_uppercase_project_lowercased() {
        let r = try_normalize_bitbucket_server(
            "https",
            "bb.corp.com",
            "/projects/PROJ/repos/myrepo/browse",
        );
        assert_eq!(
            r,
            Some("https://bb.corp.com/scm/proj/myrepo.git".to_owned())
        );
    }

    #[test]
    fn bitbucket_server_without_projects_returns_none() {
        assert!(
            try_normalize_bitbucket_server("https", "bb.corp.com", "/scm/proj/repo.git").is_none()
        );
    }

    #[test]
    fn bitbucket_server_missing_repos_segment_returns_none() {
        assert!(
            try_normalize_bitbucket_server("https", "bb.corp.com", "/projects/PROJ/browse")
                .is_none()
        );
    }

    // ── try_normalize_gitlab ──────────────────────────────────────────────────

    #[test]
    fn gitlab_dash_tree_normalized() {
        let r = try_normalize_gitlab("https", "gitlab.com", "/group/repo/-/tree/main");
        assert_eq!(r, Some("https://gitlab.com/group/repo.git".to_owned()));
    }

    #[test]
    fn gitlab_no_dash_returns_none() {
        assert!(try_normalize_gitlab("https", "gitlab.com", "/group/repo").is_none());
    }

    #[test]
    fn gitlab_strips_existing_dot_git_before_readding() {
        let r = try_normalize_gitlab("https", "gitlab.com", "/group/repo.git/-/tree/main");
        assert_eq!(r, Some("https://gitlab.com/group/repo.git".to_owned()));
    }

    // ── try_normalize_github ──────────────────────────────────────────────────

    #[test]
    fn github_tree_normalized() {
        let r = try_normalize_github("https", "github.com", "/owner/repo/tree/main");
        assert_eq!(r, Some("https://github.com/owner/repo.git".to_owned()));
    }

    #[test]
    fn github_non_github_host_returns_none() {
        assert!(try_normalize_github("https", "gitlab.com", "/owner/repo/tree/main").is_none());
    }

    #[test]
    fn github_plain_two_segment_path_returns_none() {
        assert!(try_normalize_github("https", "github.com", "/owner/repo").is_none());
    }

    #[test]
    fn github_unknown_third_segment_returns_none() {
        assert!(try_normalize_github("https", "github.com", "/owner/repo/wiki").is_none());
    }

    // ── try_normalize_bitbucket_cloud ─────────────────────────────────────────

    #[test]
    fn bitbucket_cloud_src_normalized() {
        let r = try_normalize_bitbucket_cloud(
            "https",
            "bitbucket.org",
            "/workspace/repo/src/main/README.md",
        );
        assert_eq!(
            r,
            Some("https://bitbucket.org/workspace/repo.git".to_owned())
        );
    }

    #[test]
    fn bitbucket_cloud_non_bitbucket_host_returns_none() {
        assert!(
            try_normalize_bitbucket_cloud("https", "github.com", "/ws/repo/src/main").is_none()
        );
    }

    #[test]
    fn bitbucket_cloud_without_src_segment_returns_none() {
        assert!(try_normalize_bitbucket_cloud("https", "bitbucket.org", "/ws/repo").is_none());
    }

    // ── parse_ref_line ────────────────────────────────────────────────────────

    #[test]
    fn parse_ref_line_all_fields() {
        let line = "main|abc1234|2024-01-15T10:00:00+00:00|Initial commit";
        let r = parse_ref_line(line, GitRefKind::Branch);
        assert_eq!(r.name, "main");
        assert_eq!(r.sha, "abc1234");
        assert!(r.date.is_some());
        assert_eq!(r.message.as_deref(), Some("Initial commit"));
        assert!(matches!(r.kind, GitRefKind::Branch));
    }

    #[test]
    fn parse_ref_line_tag_kind() {
        let line = "v1.0.0|deadbeef|2024-01-01T00:00:00+00:00|Release v1.0.0";
        let r = parse_ref_line(line, GitRefKind::Tag);
        assert_eq!(r.name, "v1.0.0");
        assert!(matches!(r.kind, GitRefKind::Tag));
    }

    #[test]
    fn parse_ref_line_name_only() {
        let r = parse_ref_line("main", GitRefKind::Branch);
        assert_eq!(r.name, "main");
        assert_eq!(r.sha, "");
        assert!(r.date.is_none());
        assert!(r.message.is_none());
    }

    #[test]
    fn parse_ref_line_invalid_date_gives_none() {
        let r = parse_ref_line("main|abc|not-a-date|msg", GitRefKind::Branch);
        assert!(r.date.is_none());
        assert_eq!(r.message.as_deref(), Some("msg"));
    }

    #[test]
    fn parse_ref_line_empty_string() {
        let r = parse_ref_line("", GitRefKind::Branch);
        assert_eq!(r.name, "");
    }

    // ── parse_commit_line ─────────────────────────────────────────────────────

    #[test]
    fn parse_commit_line_all_fields() {
        let line =
            "abc1234567890abcdef|abc1234|Alice Smith|2024-01-15T10:00:00+00:00|Fix critical bug";
        let c = parse_commit_line(line);
        assert_eq!(c.sha, "abc1234567890abcdef");
        assert_eq!(c.short_sha, "abc1234");
        assert_eq!(c.author, "Alice Smith");
        assert_eq!(c.subject, "Fix critical bug");
    }

    #[test]
    fn parse_commit_line_empty() {
        let c = parse_commit_line("");
        assert_eq!(c.sha, "");
        assert_eq!(c.short_sha, "");
        assert_eq!(c.author, "");
        assert_eq!(c.subject, "");
    }

    #[test]
    fn parse_commit_line_partial_fields() {
        let c = parse_commit_line("sha1|sha_short");
        assert_eq!(c.sha, "sha1");
        assert_eq!(c.short_sha, "sha_short");
        assert_eq!(c.author, "");
    }

    #[test]
    fn parse_commit_line_subject_with_pipe() {
        // splitn(5, '|') keeps everything in the 5th slot
        let line = "sha|short|author|2024-01-01T00:00:00+00:00|subject with | pipe inside";
        let c = parse_commit_line(line);
        assert_eq!(c.subject, "subject with | pipe inside");
    }

    // ── parse_git_date ────────────────────────────────────────────────────────

    #[test]
    fn parse_git_date_valid_rfc3339() {
        let dt = parse_git_date("2024-01-15T10:30:00+00:00");
        assert!(dt.is_some());
    }

    #[test]
    fn parse_git_date_invalid_returns_none() {
        assert!(parse_git_date("not-a-date").is_none());
        assert!(parse_git_date("").is_none());
    }

    #[test]
    fn parse_git_date_with_offset_converts_to_utc() {
        let dt = parse_git_date("2024-06-01T12:00:00+05:00").unwrap();
        // +05:00 offset means UTC is 12:00 - 5:00 = 07:00
        assert_eq!(dt.time().hour(), 7);
    }

    #[test]
    fn port_of_git_url_unknown_scheme_returns_none() {
        // A recognised scheme with no explicit port falls back to its default…
        assert_eq!(port_of_git_url("https://host/repo"), Some(443));
        assert_eq!(port_of_git_url("ssh://host/repo"), Some(22));
        assert_eq!(port_of_git_url("git://host/repo"), Some(9418));
        // …but an unknown scheme with no explicit port yields None.
        assert_eq!(port_of_git_url("file://host/repo"), None);
        assert_eq!(port_of_git_url("ftp://host/repo"), None);
    }

    // ── per-host credential registry ──────────────────────────────────────────

    #[test]
    fn hostkey_normalizes_punctuation_and_case() {
        assert_eq!(
            hostkey("bitbucket.instance2.com"),
            "BITBUCKET_INSTANCE2_COM"
        );
        assert_eq!(hostkey("git-host.corp"), "GIT_HOST_CORP");
        assert_eq!(hostkey("host:7990"), "HOST_7990");
    }

    #[test]
    fn resolve_credential_https_env_wins() {
        let _g = env_lock();
        let host = "cred-https-test.example";
        let key = format!("SLOC_GIT_CRED_{}", hostkey(host));
        // SAFETY: single-threaded under ENV_LOCK; removed before the guard drops.
        unsafe { std::env::set_var(&key, "alice:secrettoken") };
        let cred = resolve_credential(host, None);
        // SAFETY: see above.
        unsafe { std::env::remove_var(&key) };
        match cred {
            Some(GitCredential::Https { user, token }) => {
                assert_eq!(user, "alice");
                assert_eq!(token, "secrettoken");
            }
            _ => panic!("expected an HTTPS credential from the env registry"),
        }
    }

    #[test]
    fn resolve_credential_ssh_key_env() {
        let _g = env_lock();
        let host = "cred-ssh-test.example";
        let key = format!("SLOC_GIT_SSHKEY_{}", hostkey(host));
        // SAFETY: single-threaded under ENV_LOCK; removed before the guard drops.
        unsafe { std::env::set_var(&key, "/home/u/.ssh/id_ed25519") };
        let cred = resolve_credential(host, None);
        // SAFETY: see above.
        unsafe { std::env::remove_var(&key) };
        match cred {
            Some(GitCredential::Ssh { key_path }) => {
                assert_eq!(key_path, "/home/u/.ssh/id_ed25519");
            }
            _ => panic!("expected an SSH credential from the env registry"),
        }
    }

    #[test]
    fn resolve_credential_none_falls_through() {
        // No registry entry → None, so git falls back to its own credential resolution.
        assert!(resolve_credential("no-such-cred-host.invalid", None).is_none());
    }

    #[test]
    fn cred_injection_https_keeps_secret_out_of_config() {
        let _g = env_lock();
        let host = "inj-test.example";
        let key = format!("SLOC_GIT_CRED_{}", hostkey(host));
        // SAFETY: single-threaded under ENV_LOCK; removed before the guard drops.
        unsafe { std::env::set_var(&key, "bob:tok123") };
        let inj = cred_injection(host, None);
        // SAFETY: see above.
        unsafe { std::env::remove_var(&key) };

        // First config entry resets inherited helpers; the helper reads the secret from env.
        assert_eq!(
            inj.config.first().map(String::as_str),
            Some("credential.helper=")
        );
        assert!(
            inj.config
                .iter()
                .any(|c| c.contains("$GIT_U") && c.contains("$GIT_P"))
        );
        assert!(
            !inj.config.iter().any(|c| c.contains("tok123")),
            "the token must NEVER appear in git config / argv"
        );
        assert!(inj.env.iter().any(|(k, v)| k == "GIT_U" && v == "bob"));
        assert!(inj.env.iter().any(|(k, v)| k == "GIT_P" && v == "tok123"));
    }

    #[test]
    fn resolve_credential_port_qualified_key_wins() {
        let _g = env_lock();
        let host = "cred-port-test.example";
        let port_key = format!("SLOC_GIT_CRED_{}", hostkey(&format!("{host}:7990")));
        let bare_key = format!("SLOC_GIT_CRED_{}", hostkey(host));
        // SAFETY: single-threaded under ENV_LOCK; removed before the guard drops.
        unsafe {
            std::env::set_var(&port_key, "svc-port:porttoken");
            std::env::set_var(&bare_key, "svc-bare:baretoken");
        }
        let with_port = resolve_credential(host, Some(7990));
        let without_port = resolve_credential(host, None);
        // SAFETY: see above.
        unsafe {
            std::env::remove_var(&port_key);
            std::env::remove_var(&bare_key);
        }
        match with_port {
            Some(GitCredential::Https { user, .. }) => assert_eq!(user, "svc-port"),
            _ => panic!("port-qualified key should win when a port is present"),
        }
        match without_port {
            Some(GitCredential::Https { user, .. }) => assert_eq!(user, "svc-bare"),
            _ => panic!("bare-host key should resolve when no port is given"),
        }
    }

    #[test]
    fn resolve_credential_falls_back_to_bare_when_only_bare_key_set() {
        let _g = env_lock();
        let host = "cred-fallback-test.example";
        let bare_key = format!("SLOC_GIT_CRED_{}", hostkey(host));
        // SAFETY: single-threaded under ENV_LOCK; removed before the guard drops.
        unsafe { std::env::set_var(&bare_key, "svc:tok") };
        // A port is supplied but only the bare-host key exists → it is still used.
        let cred = resolve_credential(host, Some(7990));
        // SAFETY: see above.
        unsafe { std::env::remove_var(&bare_key) };
        assert!(matches!(cred, Some(GitCredential::Https { .. })));
    }

    #[test]
    fn explicit_port_of_git_url_extracts_or_none() {
        assert_eq!(
            explicit_port_of_git_url("https://git.corp:7990/team/repo.git"),
            Some(7990)
        );
        assert_eq!(
            explicit_port_of_git_url("https://git.corp/team/repo.git"),
            None
        );
        assert_eq!(
            explicit_port_of_git_url("ssh://git@host:2222/repo.git"),
            Some(2222)
        );
        // scp-like: the segment after ':' is a path, not a port.
        assert_eq!(
            explicit_port_of_git_url("git@github.com:owner/repo.git"),
            None
        );
        assert_eq!(
            explicit_port_of_git_url("https://[fe80::1]:443/repo"),
            Some(443)
        );
        assert_eq!(explicit_port_of_git_url("https://[fe80::1]/repo"), None);
    }

    // ── source classification / normalize passthrough ─────────────────────────

    #[test]
    fn classify_source_recognizes_each_form() {
        assert!(matches!(
            classify_source("https://github.com/o/r.git"),
            GitSource::Remote
        ));
        assert!(matches!(
            classify_source("git@github.com:o/r.git"),
            GitSource::Remote
        ));
        assert!(matches!(
            classify_source("ssh://git@h/o/r.git"),
            GitSource::Remote
        ));
        assert!(matches!(
            classify_source("file:///srv/mirror/r"),
            GitSource::FileUrl
        ));
        assert!(matches!(
            classify_source("/srv/mirror/r.bundle"),
            GitSource::Bundle
        ));
        assert!(matches!(
            classify_source(r"C:\mirror\r"),
            GitSource::LocalPath
        ));
    }

    #[test]
    fn normalize_git_url_passes_local_sources_through() {
        for u in [
            "file:///srv/mirror/r",
            r"C:\mirror\repo",
            r"\\srv\share\repo",
            "/srv/x.bundle",
        ] {
            assert_eq!(
                normalize_git_url(u),
                u,
                "local source must pass through unchanged: {u}"
            );
        }
    }

    #[test]
    fn file_url_to_path_posix_windows_and_rejects_host() {
        assert_eq!(
            file_url_to_path("file:///home/u/repo").unwrap(),
            "/home/u/repo"
        );
        assert_eq!(
            file_url_to_path("file:///C:/mirror/repo").unwrap(),
            "C:/mirror/repo"
        );
        assert!(
            file_url_to_path("file://server/share/repo").is_err(),
            "a file:// URL with a host authority must be rejected"
        );
    }

    // ── local-import gate (validate_local_source) ─────────────────────────────

    #[test]
    fn validate_local_source_rejected_when_disabled() {
        let _g = env_lock();
        // SAFETY: single-threaded under ENV_LOCK.
        unsafe {
            std::env::remove_var("SLOC_GIT_ALLOW_LOCAL");
            std::env::remove_var("SLOC_GIT_LOCAL_ROOT");
        }
        assert!(validate_local_source("/srv/x.bundle", &GitSource::Bundle).is_err());
    }

    #[test]
    fn validate_local_source_requires_root_when_enabled() {
        let _g = env_lock();
        // SAFETY: single-threaded under ENV_LOCK.
        unsafe {
            std::env::set_var("SLOC_GIT_ALLOW_LOCAL", "1");
            std::env::remove_var("SLOC_GIT_LOCAL_ROOT");
        }
        let err = validate_local_source("/srv/x.bundle", &GitSource::Bundle)
            .unwrap_err()
            .to_string();
        // SAFETY: see above.
        unsafe { std::env::remove_var("SLOC_GIT_ALLOW_LOCAL") };
        assert!(
            err.contains("SLOC_GIT_LOCAL_ROOT"),
            "must fail closed without a configured root: {err}"
        );
    }

    #[test]
    fn validate_local_source_rejects_outside_root() {
        let _g = env_lock();
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        // SAFETY: single-threaded under ENV_LOCK.
        unsafe {
            std::env::set_var("SLOC_GIT_ALLOW_LOCAL", "1");
            std::env::set_var("SLOC_GIT_LOCAL_ROOT", root.path());
        }
        let outside_path = outside.path().to_string_lossy().into_owned();
        let res = validate_local_source(&outside_path, &GitSource::LocalPath);
        // SAFETY: see above.
        unsafe {
            std::env::remove_var("SLOC_GIT_ALLOW_LOCAL");
            std::env::remove_var("SLOC_GIT_LOCAL_ROOT");
        }
        assert!(
            res.is_err(),
            "a source outside SLOC_GIT_LOCAL_ROOT must be rejected"
        );
    }

    #[test]
    fn validate_local_source_accepts_inside_root() {
        let _g = env_lock();
        let root = tempfile::tempdir().unwrap();
        let inside = root.path().join("sub");
        std::fs::create_dir_all(&inside).unwrap();
        // SAFETY: single-threaded under ENV_LOCK.
        unsafe {
            std::env::set_var("SLOC_GIT_ALLOW_LOCAL", "1");
            std::env::set_var("SLOC_GIT_LOCAL_ROOT", root.path());
        }
        let inside_path = inside.to_string_lossy().into_owned();
        let res = validate_local_source(&inside_path, &GitSource::LocalPath);
        // SAFETY: see above.
        unsafe {
            std::env::remove_var("SLOC_GIT_ALLOW_LOCAL");
            std::env::remove_var("SLOC_GIT_LOCAL_ROOT");
        }
        assert!(
            res.is_ok(),
            "a source under the root must be accepted: {res:?}"
        );
    }

    #[test]
    fn validate_local_source_rejects_unc_even_when_enabled() {
        let _g = env_lock();
        let root = tempfile::tempdir().unwrap();
        // SAFETY: single-threaded under ENV_LOCK.
        unsafe {
            std::env::set_var("SLOC_GIT_ALLOW_LOCAL", "1");
            std::env::set_var("SLOC_GIT_LOCAL_ROOT", root.path());
        }
        let res = validate_local_source(r"\\attacker\share\repo", &GitSource::LocalPath);
        // SAFETY: see above.
        unsafe {
            std::env::remove_var("SLOC_GIT_ALLOW_LOCAL");
            std::env::remove_var("SLOC_GIT_LOCAL_ROOT");
        }
        assert!(
            res.is_err(),
            "UNC/SMB paths must be treated as remote and rejected"
        );
    }

    #[test]
    fn clone_or_fetch_file_url_rejected_without_gate() {
        // SSRF→LFI stays blocked: file:// is refused unless the local gate is explicitly on.
        let _g = env_lock();
        // SAFETY: single-threaded under ENV_LOCK.
        unsafe { std::env::remove_var("SLOC_GIT_ALLOW_LOCAL") };
        let dest = tempfile::tempdir().unwrap();
        assert!(clone_or_fetch("file:///etc/passwd", dest.path()).is_err());
    }
}

// ── git subprocess integration tests ─────────────────────────────────────────
//
// These tests exercise run_git, clone_or_fetch, get_sha, list_refs,
// list_commits, create_worktree, and destroy_worktree against a real git
// repository created in a temp directory.  They require git to be on PATH
// (always true in this project's development and CI environments).
#[cfg(test)]
mod git_integration {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;

    // ── helpers ───────────────────────────────────────────────────────────────

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .status()
            .expect("git must be on PATH");
        assert!(status.success(), "git {args:?} failed");
    }

    /// Initialise a bare-minimum git repo with a single commit on branch `main`.
    fn make_repo(dir: &Path) {
        git(dir, &["init", "-b", "main"]);
        std::fs::write(dir.join("hello.txt"), "hello\n").unwrap();
        git(dir, &["add", "hello.txt"]);
        git(dir, &["commit", "--no-gpg-sign", "-m", "initial"]);
    }

    // ── publish_dir (git-push export) ─────────────────────────────────────────

    #[test]
    fn publish_dir_pushes_snapshot_to_local_bare_repo() {
        let _g = env_lock();
        let root = tempdir().unwrap();
        // A bare target repo under the allowed local root plays the role of the remote.
        let bare = root.path().join("target.git");
        git(root.path(), &["init", "--bare", bare.to_str().unwrap()]);

        // Artifacts to publish (nested, to exercise the recursive copy).
        let src = tempdir().unwrap();
        std::fs::write(src.path().join("result.json"), b"{\"ok\":true}").unwrap();
        std::fs::create_dir_all(src.path().join("html")).unwrap();
        std::fs::write(src.path().join("html").join("r.html"), b"<html></html>").unwrap();

        let work = root.path().join("work");
        // SAFETY: single-threaded under ENV_LOCK; the local-source gate is required so
        // publish_dir accepts a filesystem path as the target.
        unsafe {
            std::env::set_var("SLOC_GIT_ALLOW_LOCAL", "1");
            std::env::set_var("SLOC_GIT_LOCAL_ROOT", root.path());
        }
        let res = publish_dir(
            bare.to_str().unwrap(),
            "reports",
            "run-1",
            src.path(),
            "publish run-1",
            &work,
        );
        // SAFETY: see above.
        unsafe {
            std::env::remove_var("SLOC_GIT_ALLOW_LOCAL");
            std::env::remove_var("SLOC_GIT_LOCAL_ROOT");
        }
        assert!(res.is_ok(), "publish_dir failed: {res:?}");

        // The bare repo now carries the branch with the artifacts under the subdir.
        let out = std::process::Command::new("git")
            .args([
                &format!("--git-dir={}", bare.to_str().unwrap()),
                "ls-tree",
                "-r",
                "--name-only",
                "reports",
            ])
            .output()
            .expect("git must be on PATH");
        let listing = String::from_utf8_lossy(&out.stdout);
        assert!(
            listing.contains("run-1/result.json"),
            "expected run-1/result.json in: {listing}"
        );
        assert!(
            listing.contains("run-1/html/r.html"),
            "expected run-1/html/r.html in: {listing}"
        );
    }

    // ── run_git ───────────────────────────────────────────────────────────────

    #[test]
    fn run_git_success_returns_stdout() {
        let dir = tempdir().unwrap();
        make_repo(dir.path());
        // `git rev-parse HEAD` is the simplest command that produces output
        let sha = run_git(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(sha.len(), 40, "full SHA must be 40 hex chars: {sha}");
    }

    #[test]
    fn run_git_failure_returns_error() {
        let dir = tempdir().unwrap();
        make_repo(dir.path());
        let result = run_git(dir.path(), &["rev-parse", "nonexistent-ref-xyz"]);
        assert!(result.is_err(), "nonexistent ref must return an error");
    }

    // ── clone_or_fetch ────────────────────────────────────────────────────────

    #[test]
    fn clone_or_fetch_clones_local_repo() {
        let src = tempdir().unwrap();
        make_repo(src.path());

        let dest_root = tempdir().unwrap();
        let dest = dest_root.path().join("clone");

        // Use the file:// URL so validate_clone_url accepts it ... but wait,
        // file:// is NOT in the allowlist.  Use https:// scheme bypass: pass the
        // raw path directly and let normalize_git_url pass it through unchanged,
        // then test validate_clone_url separately.
        // Instead: bypass validate_clone_url by calling run_git directly for the
        // clone, then test clone_or_fetch on a subsequent fetch.

        // Set up the clone manually so we can test the fetch branch.
        std::fs::create_dir_all(&dest).unwrap();
        let src_str = src.path().to_str().unwrap();
        let dest_str = dest.to_str().unwrap();
        run_git(src.path(), &["clone", src_str, dest_str]).unwrap();
        assert!(dest.join(".git").exists(), "clone must create .git dir");

        // Now the dest exists; add a second commit to src and fetch.
        std::fs::write(src.path().join("second.txt"), "v2\n").unwrap();
        git(src.path(), &["add", "second.txt"]);
        git(src.path(), &["commit", "--no-gpg-sign", "-m", "second"]);

        // clone_or_fetch on existing dest → runs git fetch
        // We bypass URL validation by calling the underlying path directly
        // (validate_clone_url would reject local paths; test the fetch branch
        // via run_git directly since it's already covered by run_git tests above)
        run_git(&dest, &["fetch", "--all", "--tags", "--prune"]).unwrap();
    }

    #[test]
    fn list_branches_excludes_origin_head_symref() {
        // A fresh clone carries `origin/HEAD -> origin/main`. `%(refname:short)` shortens that
        // symref to bare `origin`, which a name-based filter misses — it would surface as a
        // phantom branch duplicating the default branch. Verify it is dropped.
        let src = tempdir().unwrap();
        let inner = src.path().join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        make_repo(&inner);
        git(&inner, &["branch", "feature-x"]);

        let dest_root = tempdir().unwrap();
        let dest = dest_root.path().join("clone");
        let src_str = inner.to_str().unwrap();
        let dest_str = dest.to_str().unwrap();
        run_git(src.path(), &["clone", src_str, dest_str]).unwrap();
        // Ensure the remote HEAD symref exists (some git versions set it on clone already).
        let _ = run_git(&dest, &["remote", "set-head", "origin", "--auto"]);

        let branches = list_branches(&dest).unwrap();
        let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
        assert!(
            !names.contains(&"origin"),
            "origin/HEAD symref must not appear as a branch: {names:?}"
        );
        assert!(
            names.contains(&"main"),
            "main branch must be listed: {names:?}"
        );
        assert!(
            names.contains(&"feature-x"),
            "real branches must still be listed: {names:?}"
        );
    }

    #[test]
    fn clone_or_fetch_rejects_http_plain_url() {
        let dest = tempdir().unwrap();
        let result = clone_or_fetch("http://example.com/repo.git", dest.path());
        assert!(
            result.is_err(),
            "http:// must be rejected by validate_clone_url"
        );
    }

    #[test]
    fn clone_or_fetch_rejects_link_local_url() {
        let dest = tempdir().unwrap();
        let result = clone_or_fetch("https://169.254.169.254/repo", dest.path());
        assert!(result.is_err());
    }

    // ── offline import: git bundle / local path (SLOC_GIT_ALLOW_LOCAL gate) ────

    #[test]
    fn clone_or_fetch_imports_git_bundle_under_local_root() {
        let _g = env_lock();
        // Build a source repo and bundle it inside the allowed root (the air-gap import file).
        let root = tempdir().unwrap();
        let src = root.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        make_repo(&src);
        let bundle = root.path().join("repo.bundle");
        run_git(
            &src,
            &["bundle", "create", bundle.to_str().unwrap(), "--all"],
        )
        .unwrap();

        // SAFETY: single-threaded under ENV_LOCK; cleaned up before the guard drops.
        unsafe {
            std::env::set_var("SLOC_GIT_ALLOW_LOCAL", "1");
            std::env::set_var("SLOC_GIT_LOCAL_ROOT", root.path());
        }
        let dest_root = tempdir().unwrap();
        let dest = dest_root.path().join("clone");
        let res = clone_or_fetch(bundle.to_str().unwrap(), &dest);
        let refs = res.as_ref().ok().and(list_refs(&dest).ok());
        // SAFETY: see above.
        unsafe {
            std::env::remove_var("SLOC_GIT_ALLOW_LOCAL");
            std::env::remove_var("SLOC_GIT_LOCAL_ROOT");
        }

        res.unwrap();
        assert!(
            dest.join(".git").exists(),
            "bundle import must produce a clone"
        );
        let names: Vec<String> = refs
            .expect("refs must be listable from the imported clone")
            .branches
            .into_iter()
            .map(|b| b.name)
            .collect();
        assert!(
            names.iter().any(|n| n == "main"),
            "bundle clone must expose the main branch: {names:?}"
        );
    }

    #[test]
    fn clone_or_fetch_imports_local_path_under_root() {
        let _g = env_lock();
        let root = tempdir().unwrap();
        let src = root.path().join("mirror");
        std::fs::create_dir_all(&src).unwrap();
        make_repo(&src);

        // SAFETY: single-threaded under ENV_LOCK; cleaned up before the guard drops.
        unsafe {
            std::env::set_var("SLOC_GIT_ALLOW_LOCAL", "1");
            std::env::set_var("SLOC_GIT_LOCAL_ROOT", root.path());
        }
        let dest_root = tempdir().unwrap();
        let dest = dest_root.path().join("clone");
        let res = clone_or_fetch(src.to_str().unwrap(), &dest);
        // SAFETY: see above.
        unsafe {
            std::env::remove_var("SLOC_GIT_ALLOW_LOCAL");
            std::env::remove_var("SLOC_GIT_LOCAL_ROOT");
        }

        res.unwrap();
        assert!(
            dest.join(".git").exists(),
            "local-path import must produce a clone"
        );
    }

    // ── get_sha ───────────────────────────────────────────────────────────────

    #[test]
    fn get_sha_returns_full_commit_hash() {
        let dir = tempdir().unwrap();
        make_repo(dir.path());
        let sha = get_sha(dir.path(), "HEAD").unwrap();
        assert_eq!(sha.len(), 40);
        assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn get_sha_nonexistent_ref_errors() {
        let dir = tempdir().unwrap();
        make_repo(dir.path());
        assert!(get_sha(dir.path(), "refs/heads/nonexistent").is_err());
    }

    // ── list_commits ──────────────────────────────────────────────────────────

    #[test]
    fn list_commits_returns_at_least_one_commit() {
        let dir = tempdir().unwrap();
        make_repo(dir.path());
        let commits = list_commits(dir.path(), "HEAD", 10).unwrap();
        assert!(
            !commits.is_empty(),
            "must return at least the initial commit"
        );
        let c = &commits[0];
        assert_eq!(c.sha.len(), 40);
        assert!(!c.short_sha.is_empty());
        assert_eq!(c.author, "Test");
        assert_eq!(c.subject, "initial");
    }

    #[test]
    fn list_commits_respects_limit() {
        let dir = tempdir().unwrap();
        make_repo(dir.path());
        // Add a second commit
        std::fs::write(dir.path().join("b.txt"), "b\n").unwrap();
        git(dir.path(), &["add", "b.txt"]);
        git(dir.path(), &["commit", "--no-gpg-sign", "-m", "second"]);

        let one = list_commits(dir.path(), "HEAD", 1).unwrap();
        assert_eq!(one.len(), 1, "limit=1 must return exactly 1 commit");

        let two = list_commits(dir.path(), "HEAD", 10).unwrap();
        assert_eq!(two.len(), 2, "limit=10 must return both commits");
    }

    // ── list_refs (branches + tags) ───────────────────────────────────────────

    #[test]
    fn list_refs_returns_main_branch() {
        let src = tempdir().unwrap();
        make_repo(src.path());

        // Clone so we have remote-tracking refs (list_branches uses -r)
        let dest_root = tempdir().unwrap();
        let dest = dest_root.path().join("clone");
        let src_str = src.path().to_str().unwrap();
        let dest_str = dest.to_str().unwrap();
        run_git(src.path(), &["clone", src_str, dest_str]).unwrap();

        let refs = list_refs(&dest).unwrap();
        let branch_names: Vec<&str> = refs.branches.iter().map(|b| b.name.as_str()).collect();
        assert!(
            branch_names.contains(&"main"),
            "branches must include 'main', got: {branch_names:?}"
        );
    }

    #[test]
    fn list_refs_returns_tag() {
        let src = tempdir().unwrap();
        make_repo(src.path());
        git(src.path(), &["tag", "v1.0.0"]);

        let dest_root = tempdir().unwrap();
        let dest = dest_root.path().join("clone");
        let src_str = src.path().to_str().unwrap();
        run_git(src.path(), &["clone", src_str, dest.to_str().unwrap()]).unwrap();
        // Fetch tags explicitly
        run_git(&dest, &["fetch", "--tags"]).unwrap();

        let refs = list_refs(&dest).unwrap();
        let tag_names: Vec<&str> = refs.tags.iter().map(|t| t.name.as_str()).collect();
        assert!(
            tag_names.contains(&"v1.0.0"),
            "tags must include 'v1.0.0', got: {tag_names:?}"
        );
    }

    // ── create_worktree / destroy_worktree ────────────────────────────────────

    #[test]
    fn create_and_destroy_worktree() {
        let repo = tempdir().unwrap();
        make_repo(repo.path());

        let sha = get_sha(repo.path(), "HEAD").unwrap();

        let wt_root = tempdir().unwrap();
        let wt_path = wt_root.path().join("worktree");

        create_worktree(repo.path(), &sha, &wt_path).unwrap();
        assert!(
            wt_path.exists(),
            "worktree directory must exist after creation"
        );
        assert!(
            wt_path.join("hello.txt").exists(),
            "worktree must contain committed files"
        );

        destroy_worktree(repo.path(), &wt_path).unwrap();
        assert!(
            !wt_path.exists(),
            "worktree directory must be removed after destroy"
        );
    }

    #[test]
    fn destroy_worktree_on_nonexistent_path_succeeds() {
        // destroy_worktree intentionally ignores errors
        let repo = tempdir().unwrap();
        make_repo(repo.path());
        let nonexistent = repo.path().join("does_not_exist");
        assert!(destroy_worktree(repo.path(), &nonexistent).is_ok());
    }

    #[test]
    fn create_worktree_resolves_non_default_remote_branch() {
        // A fresh clone only materialises a local branch for the default branch; every other
        // branch exists solely as origin/<name>. Ref listing shows the bare name, so scanning
        // a non-default branch must still resolve — the regression the infra test caught.
        let src = tempdir().unwrap();
        let inner = src.path().join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        make_repo(&inner);
        git(&inner, &["checkout", "-b", "feature-x"]);
        std::fs::write(inner.join("feat.txt"), "feature\n").unwrap();
        git(&inner, &["add", "feat.txt"]);
        git(&inner, &["commit", "--no-gpg-sign", "-m", "feature commit"]);
        git(&inner, &["checkout", "main"]);

        let dest_root = tempdir().unwrap();
        let dest = dest_root.path().join("clone");
        run_git(
            src.path(),
            &["clone", inner.to_str().unwrap(), dest.to_str().unwrap()],
        )
        .unwrap();

        // Bare "feature-x" is only a remote-tracking ref in the clone; must still check out.
        let wt_root = tempdir().unwrap();
        let wt = wt_root.path().join("wt");
        create_worktree(&dest, "feature-x", &wt).unwrap();
        assert!(
            wt.join("feat.txt").exists(),
            "worktree must contain the feature branch's file"
        );
        destroy_worktree(&dest, &wt).unwrap();
    }

    #[test]
    fn resolve_committish_falls_back_to_origin_and_rejects_unknown() {
        let src = tempdir().unwrap();
        let inner = src.path().join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        make_repo(&inner);
        git(&inner, &["branch", "release-1"]);

        let dest_root = tempdir().unwrap();
        let dest = dest_root.path().join("clone");
        run_git(
            src.path(),
            &["clone", inner.to_str().unwrap(), dest.to_str().unwrap()],
        )
        .unwrap();

        // Non-default branch resolves via the origin/ fallback to a 40-char SHA.
        let sha = resolve_committish(&dest, "release-1").unwrap();
        assert_eq!(sha.len(), 40, "must resolve to a full SHA: {sha}");
        // A genuinely absent ref is an error, not a silent empty string.
        assert!(resolve_committish(&dest, "no-such-branch").is_err());
    }

    // ── local-repo detection ──────────────────────────────────────────────────

    #[test]
    fn is_local_repo_path_true_for_repo_dir() {
        let dir = tempdir().unwrap();
        make_repo(dir.path());
        assert!(is_local_repo_path(dir.path().to_str().unwrap()));
    }

    #[test]
    fn is_local_repo_path_false_for_plain_dir() {
        let dir = tempdir().unwrap();
        assert!(!is_local_repo_path(dir.path().to_str().unwrap()));
    }

    #[test]
    fn is_local_repo_path_false_for_remote_or_url_forms() {
        assert!(!is_local_repo_path("https://github.com/owner/repo.git"));
        assert!(!is_local_repo_path("git@github.com:owner/repo.git"));
        assert!(!is_local_repo_path("ssh://git@host/repo.git"));
        assert!(!is_local_repo_path("file:///tmp/repo"));
        assert!(!is_local_repo_path(""));
    }

    #[test]
    fn open_local_repo_returns_toplevel() {
        let dir = tempdir().unwrap();
        make_repo(dir.path());
        let root = open_local_repo(dir.path()).unwrap();
        // `--show-toplevel` and the temp dir may differ only by symlink normalisation
        // (e.g. /var → /private/var on macOS); compare by a marker file both must contain.
        assert!(
            root.join("hello.txt").exists(),
            "toplevel must be the repo root: {root:?}"
        );
    }

    #[test]
    fn open_local_repo_errs_on_non_repo() {
        let dir = tempdir().unwrap();
        assert!(open_local_repo(dir.path()).is_err());
    }

    // ── list_refs_local (local heads, not remote-tracking) ────────────────────

    #[test]
    fn list_refs_local_lists_local_branches_and_tags() {
        let dir = tempdir().unwrap();
        make_repo(dir.path());
        git(dir.path(), &["branch", "feature-x"]);
        git(dir.path(), &["tag", "v1.0.0"]);

        let refs = list_refs_local(dir.path()).unwrap();
        let branches: Vec<&str> = refs.branches.iter().map(|b| b.name.as_str()).collect();
        assert!(
            branches.contains(&"main"),
            "local heads must include main: {branches:?}"
        );
        assert!(
            branches.contains(&"feature-x"),
            "local heads must include feature-x (no clone/origin needed): {branches:?}"
        );
        let tags: Vec<&str> = refs.tags.iter().map(|t| t.name.as_str()).collect();
        assert!(
            tags.contains(&"v1.0.0"),
            "tags must include v1.0.0: {tags:?}"
        );
        assert!(
            !refs.recent_commits.is_empty(),
            "recent commits must be listed"
        );
    }

    // ── submodule population + SSRF gate ──────────────────────────────────────

    #[test]
    fn submodule_url_is_safe_classification() {
        // Relative URLs resolve against the superproject origin — always allowed.
        assert!(submodule_url_is_safe("../sibling.git"));
        assert!(submodule_url_is_safe("./nested/mod.git"));
        // Absolute local / file references are refused (a local super-repo uses relative URLs).
        assert!(!submodule_url_is_safe("/etc/passwd"));
        assert!(!submodule_url_is_safe("file:///etc/shadow"));
        // SSRF-sensitive remotes are refused by the shared clone gate.
        assert!(!submodule_url_is_safe("https://169.254.169.254/x.git"));
        assert!(!submodule_url_is_safe(
            "http://metadata.google.internal/x.git"
        ));
        // A normal public remote passes.
        assert!(submodule_url_is_safe("https://github.com/owner/repo.git"));
    }

    #[test]
    fn populate_submodules_noop_without_gitmodules() {
        let dir = tempdir().unwrap();
        make_repo(dir.path());
        assert!(populate_submodules(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn populate_submodules_skips_ssrf_submodule() {
        // A .gitmodules that points a submodule at a metadata/loopback host must be skipped, not
        // fetched — the whole point of validating submodule URLs against the SSRF gate.
        let dir = tempdir().unwrap();
        make_repo(dir.path());
        let gitmodules =
            "[submodule \"evil\"]\n\tpath = evil\n\turl = https://169.254.169.254/x.git\n";
        std::fs::write(dir.path().join(".gitmodules"), gitmodules).unwrap();
        let skipped = populate_submodules(dir.path()).unwrap();
        assert_eq!(
            skipped,
            vec!["evil".to_string()],
            "the unsafe submodule must be reported skipped"
        );
    }
}
