// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>
//
// "Report a Bug" page (`/report-bug`). Lets a user file a bug against the repository this
// build came from. Two audiences, one page:
//
//   * Networked users of the upstream project — the page builds a pre-filled "New Issue" URL
//     for the detected tracker (GitHub / GitLab) and opens it in a new tab. No token stored,
//     no secrets: the user reviews and submits on the tracker itself.
//   * Air-gapped forks — the same form instead produces a portable Markdown "report bundle",
//     stamped with the fork's own origin remote, that the user carries across the air-gap to
//     their internal maintainer to file on the internal tracker.
//
// The target repository is resolved (highest priority first) from the `SLOC_BUG_REPORT_URL`
// env var, the `[reporting] bug_report_url` config key, the origin remote captured at build
// time (`build.rs` -> `SLOC_BUILD_ORIGIN_URL`), and finally the default upstream project.

use askama::Template;
use axum::{
    extract::State,
    response::{Html, IntoResponse},
};

use super::{AppState, CspNonce};

/// Default upstream repository used when nothing else identifies where this build came from.
const DEFAULT_REPO: &str = "https://github.com/oxide-sloc/oxide-sloc";

/// Which hosting platform the target repo lives on. Drives the "New Issue" URL shape and
/// whether pre-filling the issue form is supported at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BugTrackerKind {
    GitHub,
    GitLab,
    Bitbucket,
    Other,
}

impl BugTrackerKind {
    /// Machine slug consumed by the page JS to pick a pre-fill URL template.
    fn slug(self) -> &'static str {
        match self {
            BugTrackerKind::GitHub => "github",
            BugTrackerKind::GitLab => "gitlab",
            BugTrackerKind::Bitbucket => "bitbucket",
            BugTrackerKind::Other => "other",
        }
    }

    /// Human label shown in the UI (e.g. the "Open new issue on GitHub" button).
    fn label(self) -> &'static str {
        match self {
            BugTrackerKind::GitHub => "GitHub",
            BugTrackerKind::GitLab => "GitLab",
            BugTrackerKind::Bitbucket => "Bitbucket",
            BugTrackerKind::Other => "the repository",
        }
    }

    /// Whether we can build a pre-filled "New Issue" URL for this host. GitHub and GitLab both
    /// accept title/body query parameters; Bitbucket and unknown hosts do not, so those users
    /// rely on the downloadable report bundle instead.
    fn prefill_supported(self) -> bool {
        matches!(self, BugTrackerKind::GitHub | BugTrackerKind::GitLab)
    }
}

/// How the target repository was determined — surfaced to the user so they understand where a
/// report will go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TargetSource {
    /// `SLOC_BUG_REPORT_URL` env var or `[reporting] bug_report_url` config key.
    Configured,
    /// The origin remote captured at build time.
    BuildOrigin,
    /// Fallback to the upstream project.
    Default,
}

impl TargetSource {
    fn label(self) -> &'static str {
        match self {
            TargetSource::Configured => "configured for this deployment",
            TargetSource::BuildOrigin => "detected from this build's origin remote",
            TargetSource::Default => "the upstream oxide-sloc project",
        }
    }
}

/// Fully resolved bug-report target: a browsable repo URL, the raw remote string (kept verbatim
/// so a report is stamped with the exact fork it came from), the host kind, and how we found it.
pub(crate) struct ResolvedTarget {
    web_url: String,
    raw: String,
    kind: BugTrackerKind,
    source: TargetSource,
}

/// Pick the raw target string and record where it came from. Split out from [`resolve_target`]
/// so the precedence logic is unit-testable without touching process env.
fn choose_source(
    env_url: Option<String>,
    config_url: Option<String>,
    build_origin: Option<&str>,
) -> (String, TargetSource) {
    let clean = |s: String| {
        let t = s.trim().to_string();
        if t.is_empty() { None } else { Some(t) }
    };
    if let Some(u) = env_url.and_then(clean) {
        return (u, TargetSource::Configured);
    }
    if let Some(u) = config_url.and_then(clean) {
        return (u, TargetSource::Configured);
    }
    if let Some(u) = build_origin
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
    {
        return (u, TargetSource::BuildOrigin);
    }
    (DEFAULT_REPO.to_string(), TargetSource::Default)
}

/// Resolve the effective target from env, config, and the build-time origin.
pub(crate) fn resolve_target(state: &AppState) -> ResolvedTarget {
    let env_url = std::env::var("SLOC_BUG_REPORT_URL").ok();
    let config_url = state.base_config.reporting.bug_report_url.clone();
    let build_origin = option_env!("SLOC_BUILD_ORIGIN_URL");

    let (raw, source) = choose_source(env_url, config_url, build_origin);
    let web_url = to_web_url(&raw);
    let kind = kind_of(&web_url);
    ResolvedTarget {
        web_url,
        raw,
        kind,
        source,
    }
}

/// Strip a trailing `.git` and any trailing slash from a repo path/URL fragment.
fn strip_git(s: &str) -> String {
    let s = s.trim_end_matches('/');
    s.strip_suffix(".git")
        .unwrap_or(s)
        .trim_end_matches('/')
        .to_string()
}

/// Normalise any git remote form (scp-like `git@host:owner/repo.git`, `ssh://`, `https://`,
/// or a bare `host/owner/repo`) into a browsable web URL. SSH/scp forms become `https://`;
/// an `http://` scheme is preserved for internal hosts that only serve plaintext.
fn to_web_url(raw: &str) -> String {
    let s = raw.trim().trim_end_matches('/');
    if s.is_empty() {
        return DEFAULT_REPO.to_string();
    }

    if !s.contains("://") {
        // scp-like: [user@]host:owner/repo(.git)
        if let Some(at) = s.find('@') {
            let after = &s[at + 1..];
            if let Some(colon) = after.find(':') {
                let host = &after[..colon];
                let path = after[colon + 1..].trim_start_matches('/');
                return format!("https://{host}/{}", strip_git(path));
            }
        }
        // Bare `host/owner/repo` with no scheme and no user@.
        return format!("https://{}", strip_git(s));
    }

    // Scheme present: split it off, drop any userinfo, separate host[:port] from the path.
    let (scheme, after) = s.split_once("://").unwrap_or(("https", s));
    let after = after.rsplit('@').next().unwrap_or(after);
    let (hostport, path) = match after.find('/') {
        Some(i) => (&after[..i], &after[i + 1..]),
        None => (after, ""),
    };
    let host = hostport.split(':').next().unwrap_or(hostport);
    let web_scheme = if scheme.eq_ignore_ascii_case("http") {
        "http"
    } else {
        "https"
    };
    if path.is_empty() {
        format!("{web_scheme}://{host}")
    } else {
        format!("{web_scheme}://{host}/{}", strip_git(path))
    }
}

/// Classify the host from a normalised web URL.
fn kind_of(web_url: &str) -> BugTrackerKind {
    let l = web_url.to_ascii_lowercase();
    if l.contains("github") {
        BugTrackerKind::GitHub
    } else if l.contains("gitlab") {
        BugTrackerKind::GitLab
    } else if l.contains("bitbucket") {
        BugTrackerKind::Bitbucket
    } else {
        BugTrackerKind::Other
    }
}

// ── handler ─────────────────────────────────────────────────────────────────────

pub(crate) async fn report_bug_handler(
    State(state): State<AppState>,
    axum::extract::Extension(CspNonce(csp_nonce)): axum::extract::Extension<CspNonce>,
) -> impl IntoResponse {
    let target = resolve_target(&state);
    let template = ReportBugTemplate {
        csp_nonce,
        version: env!("CARGO_PKG_VERSION"),
        repo_web_url: target.web_url,
        repo_raw: target.raw,
        host_kind: target.kind.slug(),
        host_label: target.kind.label(),
        prefill_supported: target.kind.prefill_supported(),
        source_label: target.source.label(),
        platform: format!("{} / {}", std::env::consts::OS, std::env::consts::ARCH),
    };
    Html(
        template
            .render()
            .unwrap_or_else(|e| format!("<pre>{e}</pre>")),
    )
}

// ── template ────────────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(
    source = r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>OxideSLOC — Report a Bug</title>
  <link rel="icon" type="image/png" href="/images/logo/small-logo.png">
  <link rel="stylesheet" href="/static/app.css">
  <script src="/static/app.js"></script>
  <style nonce="{{ csp_nonce }}">
    :root{--radius:14px;--bg:#f5efe8;--surface:rgba(255,255,255,0.9);--surface-2:#fbf7f2;--line:#e6d0bf;--line-strong:#d8bfad;--text:#43342d;--muted:#7b675b;--muted-2:#7b675b;--nav:#b85d33;--nav-2:#7a371b;--oxide:#c45c10;--oxide-2:#b85d33;--shadow:0 8px 24px rgba(77,44,20,0.10);}
    body.dark-theme{--bg:#1b1511;--surface:#261c17;--surface-2:#2d221d;--line:#524238;--line-strong:#6b5548;--text:#f5ece6;--muted:#c7b7aa;--muted-2:#c7b7aa;--shadow:0 8px 24px rgba(0,0,0,0.32);}
    *{box-sizing:border-box;}html,body{margin:0;min-height:100vh;font-family:Inter,ui-sans-serif,system-ui,-apple-system,Segoe UI,Roboto,sans-serif;background:var(--bg);color:var(--text);}body{display:flex;flex-direction:column;}
    .background-watermarks{position:fixed;inset:0;pointer-events:none;z-index:0;overflow:hidden;}
    .background-watermarks img{position:absolute;opacity:0.16;filter:blur(0.3px);user-select:none;max-width:none;}
    .code-particles{position:fixed;inset:0;pointer-events:none;z-index:0;overflow:hidden;}
    .code-particle{position:absolute;font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:11px;font-weight:600;color:var(--oxide-2);opacity:0;white-space:nowrap;user-select:none;animation:floatCode linear infinite;}
    @keyframes floatCode{0%{opacity:0;transform:translateY(0) rotate(var(--rot));}10%{opacity:var(--op);}85%{opacity:var(--op);}100%{opacity:0;transform:translateY(-200px) rotate(var(--rot));}}
    .top-nav{position:sticky;top:0;z-index:30;background:linear-gradient(180deg,var(--nav),var(--nav-2));border-bottom:1px solid rgba(255,255,255,0.12);box-shadow:0 4px 14px rgba(0,0,0,0.18);}
    .top-nav-inner{max-width:1720px;margin:0 auto;padding:4px 24px;min-height:56px;display:flex;align-items:center;gap:14px;}
    .brand{display:flex;align-items:center;gap:12px;text-decoration:none;}
    .brand-logo{width:36px;height:40px;object-fit:contain;}
    .brand-title{color:#fff;font-size:16px;font-weight:800;}.brand-sub{color:rgba(255,255,255,0.75);font-size:12px;}
    .nav-right{margin-left:auto;display:flex;align-items:center;gap:10px;}
    @media(max-width:1150px){.nav-right{gap:4px;}.nav-pill,.nav-dropdown-btn,.theme-toggle{padding:0 8px;font-size:11px;min-height:34px;}.brand-sub{display:none;}.server-online-pill{width:34px;padding:0;justify-content:center;font-size:0;gap:0;min-height:34px;}}
    .nav-pill{display:inline-flex;align-items:center;min-height:34px;padding:0 14px;border-radius:999px;border:1px solid rgba(255,255,255,0.18);color:#fff;background:rgba(255,255,255,0.08);font-size:12px;font-weight:700;text-decoration:none;}
    .nav-pill:hover{background:rgba(255,255,255,0.18);}
    .nav-dropdown{position:relative;display:inline-flex;}.nav-dropdown-btn{cursor:pointer;background:rgba(255,255,255,0.08);border:1px solid rgba(255,255,255,0.18);color:#fff;border-radius:999px;padding:0 14px;min-height:34px;font-size:12px;font-weight:700;display:inline-flex;align-items:center;gap:6px;text-decoration:none;}.nav-dropdown-btn:hover,.nav-dropdown:focus-within .nav-dropdown-btn{background:rgba(255,255,255,0.18);}.nav-dropdown-menu{opacity:0;visibility:hidden;position:absolute;top:calc(100% + 8px);right:0;background:linear-gradient(180deg,var(--nav),var(--nav-2));border:1px solid rgba(255,255,255,0.15);border-radius:12px;min-width:165px;overflow:hidden;box-shadow:0 10px 28px rgba(0,0,0,0.28);z-index:100;transition:opacity 0.13s ease,visibility 0s ease 0.13s;}.nav-dropdown:hover .nav-dropdown-menu,.nav-dropdown:focus-within .nav-dropdown-menu{opacity:1;visibility:visible;transition:opacity 0.13s ease,visibility 0s ease 0s;}.nav-dropdown-menu a{display:flex;align-items:center;gap:9px;padding:11px 16px;color:rgba(255,255,255,0.92);text-decoration:none;font-size:12px;font-weight:700;border-bottom:1px solid rgba(255,255,255,0.10);}.nav-dropdown-menu a:last-child{border-bottom:none;}.nav-dropdown-menu a:hover{background:rgba(255,255,255,0.14);color:#fff;}.nav-dropdown-menu a svg{width:13px;height:13px;stroke:currentColor;fill:none;stroke-width:2;flex:0 0 auto;}
    .page{width:100%;max-width:900px;margin:0 auto;padding:32px 24px 36px;position:relative;z-index:1;}
    h1{font-size:26px;font-weight:850;margin:0 0 6px;letter-spacing:-0.03em;}
    .subtitle{color:var(--muted);font-size:14px;margin:0 0 22px;line-height:1.6;}
    .card{background:var(--surface);border:1px solid var(--line);border-radius:var(--radius);padding:24px;box-shadow:var(--shadow);margin-bottom:20px;}
    .card-title{font-size:15px;font-weight:800;margin:0 0 16px;}
    .form-group{display:flex;flex-direction:column;gap:5px;margin-bottom:16px;}
    label{font-size:12px;font-weight:700;color:var(--muted);}
    .field-hint{font-size:11px;color:var(--muted-2);font-weight:500;}
    input,textarea{padding:9px 12px;border-radius:8px;border:1.5px solid var(--line-strong);background:var(--surface-2);color:var(--text);font-size:13px;width:100%;font-family:inherit;}
    textarea{resize:vertical;min-height:96px;line-height:1.5;}
    input:focus,textarea:focus{outline:none;border-color:var(--oxide-2);}
    .btn{display:inline-flex;align-items:center;gap:8px;padding:10px 20px;border-radius:9px;border:none;cursor:pointer;font-size:13px;font-weight:700;text-decoration:none;transition:opacity 0.15s,transform 0.15s;}
    .btn:hover{opacity:0.9;transform:translateY(-1px);}
    .btn svg{width:16px;height:16px;stroke:currentColor;fill:none;stroke-width:2;flex:0 0 auto;}
    .btn-primary{background:var(--oxide-2);color:#fff;}
    .btn-secondary{background:var(--surface-2);color:var(--text);border:1.5px solid var(--line-strong);}
    .btn-row{display:flex;gap:12px;margin-top:8px;flex-wrap:wrap;}
    .status-msg{padding:10px 14px;border-radius:8px;font-size:13px;font-weight:600;margin-top:14px;display:none;}
    .status-ok{background:#dcfce7;color:#166534;display:block!important;}.status-err{background:#fee2e2;color:#991b1b;display:block!important;}
    body.dark-theme .status-ok{background:#14532d;color:#86efac;}body.dark-theme .status-err{background:#450a0a;color:#fca5a5;}
    .target-card{display:flex;align-items:flex-start;gap:14px;background:var(--surface-2);border:1px solid var(--line);border-radius:12px;padding:16px 18px;margin-bottom:20px;}
    .target-icon{width:38px;height:38px;border-radius:10px;background:var(--surface);border:1px solid var(--line);display:inline-flex;align-items:center;justify-content:center;flex:0 0 auto;}
    .target-icon svg{width:20px;height:20px;stroke:var(--oxide-2);fill:none;stroke-width:1.8;}
    .target-body{min-width:0;}
    .target-label{font-size:11px;font-weight:800;text-transform:uppercase;letter-spacing:.07em;color:var(--muted-2);margin-bottom:3px;}
    .target-url{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:13px;font-weight:700;color:var(--oxide-2);word-break:break-all;}
    .target-src{font-size:12px;color:var(--muted);margin-top:4px;}
    .diag-box{background:var(--surface-2);border:1px solid var(--line);border-radius:8px;padding:12px 14px;font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:12px;color:var(--muted);line-height:1.7;white-space:pre-wrap;word-break:break-word;}
    .airgap-note{font-size:12.5px;color:var(--muted);line-height:1.6;margin:14px 0 0;padding-top:14px;border-top:1px solid var(--line);}
    .airgap-note strong{color:var(--text);}
    .airgap-note code{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:11px;background:var(--surface-2);padding:1px 5px;border-radius:5px;}
    .rb-drop{border:1.5px dashed var(--line-strong);border-radius:10px;background:var(--surface-2);padding:18px;text-align:center;cursor:pointer;transition:border-color .15s,background .15s;}
    .rb-drop:hover,.rb-drop.drag{border-color:var(--oxide-2);background:var(--surface);}
    .rb-drop-text{font-size:13px;color:var(--muted);}
    .rb-drop-text strong{color:var(--oxide-2);font-weight:800;}
    .rb-drop-hint{font-size:11px;color:var(--muted-2);margin-top:4px;}
    .rb-thumbs{display:grid;grid-template-columns:repeat(auto-fill,minmax(96px,1fr));gap:10px;margin-top:12px;}
    .rb-thumbs:empty{display:none;}
    .rb-thumb{position:relative;border:1px solid var(--line);border-radius:8px;overflow:hidden;background:var(--surface-2);aspect-ratio:1/1;}
    .rb-thumb img{width:100%;height:100%;object-fit:cover;display:block;}
    .rb-thumb-name{position:absolute;bottom:0;left:0;right:0;font-size:9px;font-weight:600;color:#fff;background:rgba(0,0,0,0.55);padding:2px 4px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;}
    .rb-thumb-remove{position:absolute;top:3px;right:3px;width:20px;height:20px;border-radius:50%;border:none;background:rgba(0,0,0,0.6);color:#fff;cursor:pointer;font-size:13px;font-weight:800;line-height:1;display:flex;align-items:center;justify-content:center;padding:0;}
    .rb-thumb-remove:hover{background:#b23030;}
    .status-dot{width:8px;height:8px;border-radius:999px;background:#26d768;box-shadow:0 0 0 4px rgba(38,215,104,0.14);flex:0 0 auto;}
    .server-status-wrap{position:relative;display:inline-flex;}.server-online-pill{cursor:default;gap:7px;}.server-status-tip{display:none;position:absolute;top:calc(100% + 10px);right:0;z-index:100;background:rgba(20,12,8,0.97);color:rgba(255,255,255,0.92);border-radius:10px;padding:10px 14px;font-size:12px;font-weight:500;line-height:1.55;white-space:nowrap;box-shadow:0 8px 24px rgba(0,0,0,0.32);pointer-events:none;border:1px solid rgba(255,255,255,0.10);}.server-status-wrap:hover .server-status-tip{display:block;}
    .site-footer{text-align:center;padding:12px 24px;font-size:13px;color:var(--muted);position:relative;z-index:1;}
    .site-footer a{color:var(--muted);}
    .theme-toggle{width:34px;height:34px;display:flex;align-items:center;justify-content:center;border-radius:999px;border:1px solid rgba(255,255,255,0.18);background:rgba(255,255,255,0.08);cursor:pointer;}
    .theme-toggle svg{width:16px;height:16px;stroke:#fff;fill:none;stroke-width:1.8;}
    .theme-toggle .icon-sun{display:none;}body.dark-theme .theme-toggle .icon-sun{display:block;}body.dark-theme .theme-toggle .icon-moon{display:none;}
    .settings-modal{position:fixed;z-index:9999;background:var(--surface);border:1px solid var(--line-strong);border-radius:14px;box-shadow:0 12px 36px rgba(0,0,0,0.22);min-width:240px;max-width:300px;opacity:0;pointer-events:none;transform:translateY(-8px) scale(0.97);transition:opacity 0.18s ease,transform 0.18s ease;overflow:hidden;}
    .settings-modal.open{opacity:1;pointer-events:auto;transform:translateY(0) scale(1);}
    .settings-modal-header{display:flex;align-items:center;justify-content:space-between;padding:14px 16px 10px;border-bottom:1px solid var(--line);font-size:13px;font-weight:800;color:var(--text);}
    .settings-close{background:none;border:none;cursor:pointer;padding:4px;color:var(--muted-2);display:flex;align-items:center;border-radius:6px;}
    .settings-close svg{width:16px;height:16px;stroke:currentColor;fill:none;stroke-width:2.5;}
    .settings-modal-body{padding:14px 16px 16px;}
    .settings-modal-label{font-size:11px;font-weight:800;text-transform:uppercase;letter-spacing:0.08em;color:var(--muted-2);margin-bottom:10px;}
    .scheme-grid{display:grid;grid-template-columns:repeat(5,1fr);gap:8px;}
    .scheme-swatch{display:flex;flex-direction:column;align-items:center;gap:5px;background:none;border:1.5px solid var(--line);border-radius:10px;cursor:pointer;padding:7px 4px 6px;transition:border-color 0.15s ease,transform 0.12s ease;}
    .scheme-swatch:hover{border-color:var(--line-strong);transform:translateY(-1px);}
    .scheme-swatch.active{border-color:#6f9bff;box-shadow:0 0 0 2px rgba(111,155,255,0.25);}
    .scheme-preview{width:28px;height:28px;border-radius:7px;flex-shrink:0;}
    .scheme-label{font-size:9px;font-weight:700;color:var(--muted-2);white-space:nowrap;}
  </style>
</head>
<body>
  <div class="background-watermarks" aria-hidden="true">
    <img src="/images/logo/logo-text.png" alt=""><img src="/images/logo/logo-text.png" alt="">
    <img src="/images/logo/logo-text.png" alt=""><img src="/images/logo/logo-text.png" alt="">
    <img src="/images/logo/logo-text.png" alt=""><img src="/images/logo/logo-text.png" alt="">
    <img src="/images/logo/logo-text.png" alt=""><img src="/images/logo/logo-text.png" alt="">
    <img src="/images/logo/logo-text.png" alt=""><img src="/images/logo/logo-text.png" alt="">
    <img src="/images/logo/logo-text.png" alt=""><img src="/images/logo/logo-text.png" alt="">
  </div>
  <div class="code-particles" id="code-particles" aria-hidden="true"></div>
  <nav class="top-nav">
    <div class="top-nav-inner">
      <a class="brand" href="/"><img class="brand-logo" src="/images/logo/small-logo.png" alt="">
        <div><div class="brand-title">OxideSLOC</div><div class="brand-sub">Report a Bug</div></div></a>
      <div class="nav-right">
        <a class="nav-pill" href="/">Home</a>
        <div class="nav-dropdown">
          <a href="/view-reports" class="nav-dropdown-btn">View Reports <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="6 9 12 15 18 9"></polyline></svg></a>
          <div class="nav-dropdown-menu">
            <a href="/trend-reports"><svg viewBox="0 0 24 24"><polyline points="23 6 13.5 15.5 8.5 10.5 1 18"></polyline><polyline points="17 6 23 6 23 12"></polyline></svg>Trend Reports</a>
          </div>
        </div>
        <a class="nav-pill" href="/compare-scans">Compare Scans</a>
        <a class="nav-pill" href="/test-metrics">Test Metrics</a>
        <div class="nav-dropdown">
          <a href="/git-browser" class="nav-dropdown-btn">Git Browser <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="6 9 12 15 18 9"></polyline></svg></a>
          <div class="nav-dropdown-menu">
            <a href="/code-ownership"><svg viewBox="0 0 24 24"><path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2"></path><circle cx="9" cy="7" r="4"></circle><path d="M23 21v-2a4 4 0 0 0-3-3.87"></path><path d="M16 3.13a4 4 0 0 1 0 7.75"></path></svg>Code Ownership</a>
            <a href="/integrations"><svg viewBox="0 0 24 24"><path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z"></path></svg>Integrations</a>
          </div>
        </div>
        <div class="server-status-wrap" id="server-status-wrap">
          <div class="nav-pill server-online-pill" id="server-status-pill">
            <span class="status-dot" id="status-dot"></span>
            <span id="server-status-label">Server</span>
            <span class="sx-d60f2ef3" id="server-ping-ms" ></span>
          </div>
          <div class="server-status-tip">
            OxideSLOC is running — accessible on your network.
            <span class="sx-238af6bc" id="server-tip-ping" ></span>
          </div>
        </div>
        <button type="button" class="theme-toggle" id="settings-btn" aria-label="Color scheme" title="Color scheme settings">
          <svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.8"><circle cx="12" cy="12" r="3"></circle><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path></svg>
        </button>
        <button class="theme-toggle" id="theme-toggle" type="button" title="Toggle theme">
          <svg class="icon-moon" viewBox="0 0 24 24"><path d="M21 12.79A9 9 0 1 1 11.21 3a7 7 0 0 0 9.79 9.79z"/></svg>
          <svg class="icon-sun" viewBox="0 0 24 24"><circle cx="12" cy="12" r="5"/><line x1="12" y1="1" x2="12" y2="3"/><line x1="12" y1="21" x2="12" y2="23"/><line x1="4.22" y1="4.22" x2="5.64" y2="5.64"/><line x1="18.36" y1="18.36" x2="19.78" y2="19.78"/><line x1="1" y1="12" x2="3" y2="12"/><line x1="21" y1="12" x2="23" y2="12"/><line x1="4.22" y1="19.78" x2="5.64" y2="18.36"/><line x1="18.36" y1="5.64" x2="19.78" y2="4.22"/></svg>
        </button>
      </div>
    </div>
  </nav>

  <div class="page"
       id="rb-cfg"
       data-repo="{{ repo_web_url }}"
       data-raw="{{ repo_raw }}"
       data-kind="{{ host_kind }}"
       data-hostlabel="{{ host_label }}"
       data-source="{{ source_label }}"
       data-version="{{ version }}"
       data-platform="{{ platform }}">
    <h1>Report a Bug</h1>
    <p class="subtitle">Found something wrong? Describe it below. Fields marked with an asterisk are required. Your report is assembled entirely in your browser — nothing is sent anywhere until you choose an action.</p>

    <div class="target-card">
      <span class="target-icon" aria-hidden="true"><svg viewBox="0 0 24 24"><path d="M9 19c-5 1.5-5-2.5-7-3m14 6v-3.87a3.37 3.37 0 0 0-.94-2.61c3.14-.35 6.44-1.54 6.44-7A5.44 5.44 0 0 0 20 4.77 5.07 5.07 0 0 0 19.91 1S18.73.65 16 2.48a13.38 13.38 0 0 0-7 0C6.27.65 5.09 1 5.09 1A5.07 5.07 0 0 0 5 4.77a5.44 5.44 0 0 0-1.5 3.78c0 5.42 3.3 6.61 6.44 7A3.37 3.37 0 0 0 9 18.13V22"></path></svg></span>
      <div class="target-body">
        <div class="target-label">Reports for this build go to</div>
        <a class="target-url" id="target-url" href="{{ repo_web_url }}" target="_blank" rel="noopener">{{ repo_web_url }}</a>
        <div class="target-src">Target {{ source_label }}.</div>
      </div>
    </div>

    <div class="card">
      <div class="form-group">
        <label for="rbTitle">Summary <span class="sx-61745c28" >*</span></label>
        <input id="rbTitle" type="text" maxlength="160" placeholder="Short one-line summary of the problem"/>
      </div>
      <div class="form-group">
        <label for="rbDesc">What happened? <span class="sx-61745c28" >*</span></label>
        <textarea id="rbDesc" placeholder="Steps to reproduce, what you expected, and what actually happened."></textarea>
      </div>
      <div class="form-group">
        <label for="rbErrors">Error codes / messages <span class="field-hint">(optional)</span></label>
        <textarea id="rbErrors" placeholder="Paste any error codes, stack traces, or console output here."></textarea>
      </div>

      <div class="form-group">
        <label>Screenshots / images <span class="field-hint">(optional — drag, paste, or click to add)</span></label>
        <div class="rb-drop" id="rbDrop" tabindex="0" role="button" aria-label="Add images">
          <div class="rb-drop-text"><strong>Click to choose</strong>, drag images here, or paste from the clipboard</div>
          <div class="rb-drop-hint">PNG, JPEG, GIF, or WebP — up to 6 images, 5 MB each</div>
          <input id="rbFile" type="file" accept="image/png,image/jpeg,image/gif,image/webp" multiple hidden>
        </div>
        <div class="rb-thumbs" id="rbThumbs"></div>
      </div>

      <div class="form-group">
        <label>Diagnostics automatically included <span class="field-hint">(review before sending)</span></label>
        <div class="diag-box" id="rbDiag">Loading…</div>
      </div>

      <div id="rbStatus" class="status-msg"></div>

      <div class="btn-row">
        {% if prefill_supported %}
        <button class="btn btn-primary" id="rbOpenBtn" type="button">
          <svg viewBox="0 0 24 24"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"></path><polyline points="15 3 21 3 21 9"></polyline><line x1="10" y1="14" x2="21" y2="3"></line></svg>
          Open a new issue on {{ host_label }}
        </button>
        {% endif %}
        <button class="btn btn-secondary" id="rbDownloadBtn" type="button">
          <svg viewBox="0 0 24 24"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path><polyline points="7 10 12 15 17 10"></polyline><line x1="12" y1="15" x2="12" y2="3"></line></svg>
          Download report bundle
        </button>
      </div>

      <p class="airgap-note">
        {% if prefill_supported %}
        <strong>Online:</strong> "Open a new issue" pre-fills the {{ host_label }} form in a new tab. Any images you attach are downloaded at the same time so you can drag them straight into the {{ host_label }} comment box, which uploads them for you — pre-filled issue URLs cannot carry image data themselves.<br>
        {% endif %}
        <strong>Air-gapped:</strong> "Download report bundle" saves a portable report stamped with this build's origin remote — a single Markdown file, or a <code>.zip</code> (the report plus an <code>images/</code> folder) when you attach images. Carry it across the air-gap to your OxideSLOC maintainer, who can file it on the internal tracker. To point this page at your own internal repository, set the <code>SLOC_BUG_REPORT_URL</code> environment variable (or the <code>[reporting] bug_report_url</code> config key).
      </p>
    </div>
  </div>

  <script nonce="{{ csp_nonce }}">
  (function () {
    function applyTheme() { if (localStorage.getItem('sloc-theme') === 'dark') document.body.classList.add('dark-theme'); }
    function toggleTheme() { var d = document.body.classList.toggle('dark-theme'); localStorage.setItem('sloc-theme', d ? 'dark' : 'light'); }

    // ── Settings modal (nav colour scheme) ────────────────────────────────────
    (function() {
      var S=[{n:'Classic',a:'#b85d33',b:'#7a371b'},{n:'Navy',a:'#283790',b:'#1e1e24'},{n:'Ember',a:'#ce5d3d',b:'#1e1e24'},{n:'Ocean',a:'#1f439b',b:'#1e1e24'},{n:'Royal',a:'#003184',b:'#1e1e24'}];
      function ap(s){document.documentElement.style.setProperty('--nav',s.a);document.documentElement.style.setProperty('--nav-2',s.b);try{localStorage.setItem('sloc-ns',JSON.stringify(s));}catch(e){}document.querySelectorAll('.scheme-swatch').forEach(function(x){x.classList.toggle('active',x.dataset.n===s.n);});}
      try{var sv=JSON.parse(localStorage.getItem('sloc-ns'));if(sv&&sv.a){ap(sv);}else{ap(S[0]);}}catch(e){ap(S[0]);}
      var btn=document.getElementById('settings-btn');if(!btn)return;
      var m=document.createElement('div');m.id='settings-modal';m.className='settings-modal';
      m.innerHTML='<div class="settings-modal-header"><span>Appearance</span><button type="button" class="settings-close" id="settings-close" aria-label="Close"><svg viewBox="0 0 24 24"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg></button></div><div class="settings-modal-body"><div class="settings-modal-label">Navigation color scheme</div><div class="scheme-grid" id="scheme-grid"></div></div>';
      document.body.appendChild(m);
      var g=document.getElementById('scheme-grid');
      if(g)S.forEach(function(s){var el=document.createElement('button');el.type='button';el.className='scheme-swatch';el.dataset.n=s.n;el.title=s.n;var p=document.createElement('div');p.className='scheme-preview';p.style.background='linear-gradient(135deg,'+s.a+','+s.b+')';var l=document.createElement('span');l.className='scheme-label';l.textContent=s.n;el.appendChild(p);el.appendChild(l);try{var c=JSON.parse(localStorage.getItem('sloc-ns'));if(c&&c.n===s.n)el.classList.add('active');}catch(e){}el.addEventListener('click',function(){ap(s);});g.appendChild(el);});
      var cl=document.getElementById('settings-close');
      btn.addEventListener('click',function(e){e.stopPropagation();var r=btn.getBoundingClientRect();m.style.top=(r.bottom+6)+'px';m.style.right=(window.innerWidth-r.right)+'px';m.classList.toggle('open');});
      if(cl)cl.addEventListener('click',function(){m.classList.remove('open');});
      document.addEventListener('click',function(e){if(!m.contains(e.target)&&e.target!==btn)m.classList.remove('open');});
    })();

    // ── Background effects ────────────────────────────────────────────────────
    (function randomizeWatermarks() {
      var wms = Array.prototype.slice.call(document.querySelectorAll('.background-watermarks img'));
      if (!wms.length) return;
      var placed = [];
      function tooClose(top, left) { for (var i=0;i<placed.length;i++){if(Math.abs(placed[i][0]-top)<16&&Math.abs(placed[i][1]-left)<12)return true;} return false; }
      function pick(leftBand) { for (var attempt=0;attempt<50;attempt++){var top=Math.random()*88+2,left=leftBand?Math.random()*24+1:Math.random()*24+74;if(!tooClose(top,left)){placed.push([top,left]);return[top,left];}} var top=Math.random()*88+2,left=leftBand?Math.random()*24+1:Math.random()*24+74;placed.push([top,left]);return[top,left]; }
      var half = Math.floor(wms.length/2);
      wms.forEach(function(img,i){var pos=pick(i<half);var size=Math.floor(Math.random()*100+120);img.style.cssText='width:'+size+'px;top:'+pos[0].toFixed(1)+'%;left:'+pos[1].toFixed(1)+'%;transform:rotate('+(Math.random()*360).toFixed(1)+'deg);opacity:'+(Math.random()*0.08+0.12).toFixed(2)+';';});
    })();

    (function spawnCodeParticles() {
      var container = document.getElementById('code-particles');
      if (!container) return;
      var snippets = ['1,247 sloc','fn analyze()','code_lines','0 mixed','blanks: 312','// comment','pub fn run','use std::fs','Result<()>','let mut n = 0','git main','#[derive]','impl Scan','3,841 physical','files: 60','450 comments','cargo build','Ok(run)','Vec<String>','match lang','fn main() {','.rs .go .py','sloc_core','render_html','2,163 code'];
      for (var i=0;i<38;i++){(function(idx){var el=document.createElement('span');el.className='code-particle';el.textContent=snippets[idx%snippets.length];el.style.cssText='left:'+(Math.random()*94+2).toFixed(1)+'%;top:'+(Math.random()*88+6).toFixed(1)+'%;--rot:'+(Math.random()*26-13).toFixed(1)+'deg;--op:'+(Math.random()*0.09+0.06).toFixed(3)+';animation-duration:'+(Math.random()*10+9).toFixed(1)+'s;animation-delay:-'+(Math.random()*18).toFixed(1)+'s;';container.appendChild(el);})(i);}
    })();

    // ── Server ping ───────────────────────────────────────────────────────────
    (function() {
      var dot = document.getElementById('status-dot');
      var pingEl = document.getElementById('server-ping-ms');
      var tipEl = document.getElementById('server-tip-ping');
      var lbl = document.getElementById('server-status-label');
      var isServer = location.hostname !== 'localhost' && location.hostname !== '127.0.0.1' && location.hostname !== '[::1]';
      if (lbl) lbl.textContent = isServer ? 'Server' : 'Local';
      function setDot(ms) {
        if (!dot) return;
        if (ms < 100) { dot.style.background = '#26d768'; dot.style.boxShadow = '0 0 0 4px rgba(38,215,104,0.14)'; }
        else if (ms < 300) { dot.style.background = '#f5a623'; dot.style.boxShadow = '0 0 0 4px rgba(245,166,35,0.14)'; }
        else { dot.style.background = '#e05c5c'; dot.style.boxShadow = '0 0 0 4px rgba(224,92,92,0.14)'; }
      }
      function doPing() {
        var t0 = performance.now();
        fetch('/healthz', { cache: 'no-store' })
          .then(function() { var ms = Math.round(performance.now() - t0); if (pingEl) pingEl.textContent = ms + 'ms'; if (tipEl) tipEl.textContent = 'Server latency: ' + ms + ' ms'; setDot(ms); })
          .catch(function() { if (pingEl) pingEl.textContent = ''; if (tipEl) tipEl.textContent = ''; if (dot) { dot.style.background = '#e05c5c'; dot.style.boxShadow = '0 0 0 4px rgba(224,92,92,0.14)'; } });
      }
      doPing();
      setInterval(doPing, 5000);
    })();

    // ── Report a Bug logic ────────────────────────────────────────────────────
    var cfg = document.getElementById('rb-cfg');
    var repo = cfg.dataset.repo;
    var kind = cfg.dataset.kind;
    var hostLabel = cfg.dataset.hostlabel;
    var version = cfg.dataset.version;
    var platform = cfg.dataset.platform;
    var rawOrigin = cfg.dataset.raw;
    var srcLabel = cfg.dataset.source;

    var titleEl = document.getElementById('rbTitle');
    var descEl = document.getElementById('rbDesc');
    var errEl = document.getElementById('rbErrors');
    var statusEl = document.getElementById('rbStatus');
    var diagEl = document.getElementById('rbDiag');

    var reportedAt = new Date().toISOString();
    var userAgent = navigator.userAgent || 'unknown';
    diagEl.textContent =
      'oxide-sloc version: ' + version + '\n' +
      'Platform: ' + platform + '\n' +
      'Browser: ' + userAgent + '\n' +
      'Reported at: ' + reportedAt + '\n' +
      'Source repository: ' + rawOrigin;

    function showStatus(msg, ok) {
      statusEl.textContent = msg;
      statusEl.className = 'status-msg ' + (ok ? 'status-ok' : 'status-err');
    }

    function currentTitle() { return (titleEl.value || '').trim(); }

    // ── Image attachments (kept in-browser as raw bytes until an action is taken) ─
    var MAX_IMAGES = 6, MAX_BYTES = 5 * 1024 * 1024;
    var OK_TYPES = { 'image/png': 'png', 'image/jpeg': 'jpg', 'image/gif': 'gif', 'image/webp': 'webp' };
    var attachments = []; // { name, type, data (Uint8Array), url (object URL for preview) }
    var dropEl = document.getElementById('rbDrop');
    var fileEl = document.getElementById('rbFile');
    var thumbsEl = document.getElementById('rbThumbs');

    function baseName(n, ext, idx) {
      var base = (n || '').replace(/\.[^.]*$/, '').toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
      if (!base) base = 'image-' + (idx + 1);
      return base.slice(0, 40) + '.' + ext;
    }

    function uniqueName(name) {
      var used = {}; attachments.forEach(function(a) { used[a.name] = true; });
      if (!used[name]) return name;
      var dot = name.lastIndexOf('.'), stem = name.slice(0, dot), ext = name.slice(dot), i = 2;
      while (used[stem + '-' + i + ext]) i++;
      return stem + '-' + i + ext;
    }

    function renderThumbs() {
      thumbsEl.innerHTML = '';
      attachments.forEach(function(a, i) {
        var t = document.createElement('div'); t.className = 'rb-thumb';
        var img = document.createElement('img'); img.src = a.url; img.alt = a.name;
        var nm = document.createElement('div'); nm.className = 'rb-thumb-name'; nm.textContent = a.name;
        var rm = document.createElement('button'); rm.type = 'button'; rm.className = 'rb-thumb-remove';
        rm.setAttribute('aria-label', 'Remove ' + a.name); rm.textContent = '\u00d7';
        rm.addEventListener('click', function(e) { e.stopPropagation(); URL.revokeObjectURL(a.url); attachments.splice(i, 1); renderThumbs(); });
        t.appendChild(img); t.appendChild(nm); t.appendChild(rm); thumbsEl.appendChild(t);
      });
    }

    function addFile(file) {
      var ext = OK_TYPES[file.type];
      if (!ext) { showStatus('Skipped ' + (file.name || 'an item') + ': only PNG, JPEG, GIF, or WebP images are allowed.', false); return; }
      if (file.size > MAX_BYTES) { showStatus('Skipped ' + (file.name || 'an image') + ': larger than 5 MB.', false); return; }
      if (attachments.length >= MAX_IMAGES) { showStatus('You can attach at most ' + MAX_IMAGES + ' images.', false); return; }
      var idx = attachments.length;
      var reader = new FileReader();
      reader.onload = function() {
        var data = new Uint8Array(reader.result);
        var name = uniqueName(baseName(file.name, ext, idx));
        attachments.push({ name: name, type: file.type, data: data, url: URL.createObjectURL(new Blob([data], { type: file.type })) });
        renderThumbs();
      };
      reader.readAsArrayBuffer(file);
    }

    function addFiles(list) { for (var i = 0; i < list.length; i++) addFile(list[i]); }

    dropEl.addEventListener('click', function() { fileEl.click(); });
    dropEl.addEventListener('keydown', function(e) { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); fileEl.click(); } });
    fileEl.addEventListener('change', function() { addFiles(fileEl.files); fileEl.value = ''; });
    dropEl.addEventListener('dragover', function(e) { e.preventDefault(); dropEl.classList.add('drag'); });
    dropEl.addEventListener('dragleave', function() { dropEl.classList.remove('drag'); });
    dropEl.addEventListener('drop', function(e) { e.preventDefault(); dropEl.classList.remove('drag'); if (e.dataTransfer && e.dataTransfer.files) addFiles(e.dataTransfer.files); });
    // Clipboard paste anywhere on the page. Text paste carries no files, so this is a no-op then.
    document.addEventListener('paste', function(e) { if (e.clipboardData && e.clipboardData.files && e.clipboardData.files.length) addFiles(e.clipboardData.files); });

    // ── Minimal client-side ZIP (store method + CRC-32, no dependencies) ─────────
    var CRC_T = (function() {
      var t = [];
      for (var n = 0; n < 256; n++) { var c = n; for (var k = 0; k < 8; k++) c = (c & 1) ? (0xEDB88320 ^ (c >>> 1)) : (c >>> 1); t[n] = c >>> 0; }
      return t;
    })();
    function crc32(u8) { var c = 0xFFFFFFFF; for (var i = 0; i < u8.length; i++) c = CRC_T[(c ^ u8[i]) & 0xFF] ^ (c >>> 8); return (c ^ 0xFFFFFFFF) >>> 0; }
    function buildZip(entries) {
      var enc = new TextEncoder(), d = new Date();
      var dtime = ((d.getHours() & 0x1f) << 11) | ((d.getMinutes() & 0x3f) << 5) | ((d.getSeconds() >> 1) & 0x1f);
      var ddate = (((d.getFullYear() - 1980) & 0x7f) << 9) | (((d.getMonth() + 1) & 0x0f) << 5) | (d.getDate() & 0x1f);
      function u16(v) { return [v & 0xFF, (v >>> 8) & 0xFF]; }
      function u32(v) { return [v & 0xFF, (v >>> 8) & 0xFF, (v >>> 16) & 0xFF, (v >>> 24) & 0xFF]; }
      var parts = [], central = [], offset = 0;
      entries.forEach(function(e) {
        var nameB = enc.encode(e.name), data = e.data, crc = crc32(data), sz = data.length;
        var lfh = [].concat(u32(0x04034b50), u16(20), u16(0x0800), u16(0), u16(dtime), u16(ddate), u32(crc), u32(sz), u32(sz), u16(nameB.length), u16(0));
        var head = new Uint8Array(lfh.length + nameB.length); head.set(lfh, 0); head.set(nameB, lfh.length);
        parts.push(head); parts.push(data);
        var cd = [].concat(u32(0x02014b50), u16(20), u16(20), u16(0x0800), u16(0), u16(dtime), u16(ddate), u32(crc), u32(sz), u32(sz), u16(nameB.length), u16(0), u16(0), u16(0), u16(0), u32(0), u32(offset));
        var cdr = new Uint8Array(cd.length + nameB.length); cdr.set(cd, 0); cdr.set(nameB, cd.length);
        central.push(cdr);
        offset += head.length + sz;
      });
      var cdSize = central.reduce(function(a, c) { return a + c.length; }, 0);
      var eocd = [].concat(u32(0x06054b50), u16(0), u16(0), u16(entries.length), u16(entries.length), u32(cdSize), u32(offset), u16(0));
      var all = parts.concat(central); all.push(new Uint8Array(eocd));
      return new Blob(all, { type: 'application/zip' });
    }

    function triggerDownload(blob, name) {
      var a = document.createElement('a'), url = URL.createObjectURL(blob);
      a.href = url; a.download = name; document.body.appendChild(a); a.click(); document.body.removeChild(a);
      setTimeout(function() { URL.revokeObjectURL(url); }, 1500);
    }

    function validate() {
      if (!currentTitle()) { showStatus('Please add a short summary before sending.', false); titleEl.focus(); return false; }
      if (!(descEl.value || '').trim()) { showStatus('Please describe what happened before sending.', false); descEl.focus(); return false; }
      return true;
    }

    // Issue body (Markdown). mode 'github' emits a drag-in placeholder for screenshots (the
    // pre-fill URL cannot carry image bytes); mode 'bundle' references the zipped image files.
    function buildBody(mode) {
      var desc = (descEl.value || '').trim();
      var errs = (errEl.value || '').trim();
      var lines = [];
      lines.push('## What happened');
      lines.push(desc || '(none provided)');
      lines.push('');
      if (errs) { lines.push('## Error codes / messages'); lines.push('```'); lines.push(errs); lines.push('```'); lines.push(''); }
      if (attachments.length) {
        lines.push('## Screenshots');
        if (mode === 'github') {
          lines.push('<!-- Drag the ' + attachments.length + ' downloaded image file(s) into this box to attach them. -->');
        } else {
          attachments.forEach(function(a) { lines.push('![' + a.name + '](images/' + a.name + ')'); });
        }
        lines.push('');
      }
      lines.push('## Environment');
      lines.push('- oxide-sloc version: ' + version);
      lines.push('- Platform: ' + platform);
      lines.push('- Browser: ' + userAgent);
      lines.push('- Reported at: ' + reportedAt);
      lines.push('- Source repository: ' + rawOrigin);
      return lines.join('\n');
    }

    function buildBundle() {
      var header = '# oxide-sloc bug report\n\n' +
        '**Summary:** ' + currentTitle() + '\n\n' +
        '> Target repository: ' + repo + ' (' + srcLabel + ')\n\n';
      var footer = '\n\n---\n' +
        'Generated by the oxide-sloc "Report a Bug" page. Transfer this file to your oxide-sloc ' +
        'maintainer, who can attach it to an issue on ' + repo + '.\n';
      return header + buildBody('bundle') + footer;
    }

    function issueUrl() {
      var t = encodeURIComponent(currentTitle());
      var b = encodeURIComponent(buildBody('github'));
      if (kind === 'github') { return repo + '/issues/new?labels=bug&title=' + t + '&body=' + b; }
      if (kind === 'gitlab') { return repo + '/-/issues/new?issue[title]=' + t + '&issue[description]=' + b; }
      return repo;
    }

    var openBtn = document.getElementById('rbOpenBtn');
    if (openBtn) {
      openBtn.addEventListener('click', function() {
        if (!validate()) return;
        window.open(issueUrl(), '_blank', 'noopener');
        if (attachments.length) {
          // Download each image so the user can drag them into the tracker's comment box.
          attachments.forEach(function(a, i) {
            setTimeout(function() { triggerDownload(new Blob([a.data], { type: a.type }), a.name); }, i * 350);
          });
          showStatus('Opened a pre-filled issue on ' + hostLabel + ' and downloaded ' + attachments.length +
            ' image(s). Drag the downloaded files into the ' + hostLabel + ' comment box to attach them, then submit.', true);
        } else {
          showStatus('Opened a pre-filled issue on ' + hostLabel + ' in a new tab. Review and submit it there.', true);
        }
      });
    }

    function slugTitle() {
      var s = currentTitle().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
      return s ? s.slice(0, 40) : 'report';
    }

    document.getElementById('rbDownloadBtn').addEventListener('click', function() {
      if (!validate()) return;
      var stamp = reportedAt.replace(/[:.]/g, '-');
      var base = 'oxide-sloc-bugreport-' + slugTitle() + '-' + stamp;
      if (attachments.length) {
        var entries = [{ name: 'report.md', data: new TextEncoder().encode(buildBundle()) }];
        attachments.forEach(function(a) { entries.push({ name: 'images/' + a.name, data: a.data }); });
        triggerDownload(buildZip(entries), base + '.zip');
        showStatus('Saved ' + base + '.zip (report + ' + attachments.length +
          ' image(s)). Transfer it to your oxide-sloc maintainer to file the report.', true);
      } else {
        triggerDownload(new Blob([buildBundle()], { type: 'text/markdown;charset=utf-8' }), base + '.md');
        showStatus('Saved ' + base + '.md. Transfer it to your oxide-sloc maintainer to file the report.', true);
      }
    });

    document.getElementById('theme-toggle').addEventListener('click', toggleTheme);
    applyTheme();
  })();
  </script>
  <footer class="site-footer">
    oxide-sloc v{{ version }} — local code metrics workbench &nbsp;·&nbsp;
    Built by <a href="https://github.com/NimaShafie" target="_blank" rel="noopener">Nima Shafie</a>
    &nbsp;·&nbsp; <a href="https://github.com/oxide-sloc/oxide-sloc" target="_blank" rel="noopener">View on GitHub</a>
    &nbsp;·&nbsp; <a href="https://www.gnu.org/licenses/agpl-3.0.html" target="_blank" rel="noopener">AGPL-3.0-or-later</a>
    &nbsp;·&nbsp; <a href="/report-bug" rel="noopener">Report a Bug</a>
    &nbsp;·&nbsp; <a href="/api-docs" rel="noopener">REST API</a>
  </footer>
</body>
</html>"##,
    ext = "html"
)]
struct ReportBugTemplate {
    csp_nonce: String,
    version: &'static str,
    repo_web_url: String,
    repo_raw: String,
    host_kind: &'static str,
    host_label: &'static str,
    prefill_supported: bool,
    source_label: &'static str,
    platform: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scp_like_url_becomes_https() {
        assert_eq!(
            to_web_url("git@github.com:oxide-sloc/oxide-sloc.git"),
            "https://github.com/oxide-sloc/oxide-sloc"
        );
    }

    #[test]
    fn https_url_strips_dot_git_and_slash() {
        assert_eq!(
            to_web_url("https://github.com/oxide-sloc/oxide-sloc.git/"),
            "https://github.com/oxide-sloc/oxide-sloc"
        );
    }

    #[test]
    fn ssh_url_with_port_and_userinfo() {
        assert_eq!(
            to_web_url("ssh://git@gitlab.internal:2222/team/oxide-sloc.git"),
            "https://gitlab.internal/team/oxide-sloc"
        );
    }

    #[test]
    fn http_scheme_is_preserved_for_internal_hosts() {
        assert_eq!(
            to_web_url("http://bitbucket.corp/scm/dev/oxide-sloc.git"),
            "http://bitbucket.corp/scm/dev/oxide-sloc"
        );
    }

    #[test]
    fn host_classification() {
        assert_eq!(kind_of("https://github.com/a/b"), BugTrackerKind::GitHub);
        assert_eq!(kind_of("https://gitlab.corp/a/b"), BugTrackerKind::GitLab);
        assert_eq!(
            kind_of("https://bitbucket.corp/a/b"),
            BugTrackerKind::Bitbucket
        );
        assert_eq!(kind_of("https://git.example/a/b"), BugTrackerKind::Other);
    }

    #[test]
    fn prefill_only_for_github_and_gitlab() {
        assert!(BugTrackerKind::GitHub.prefill_supported());
        assert!(BugTrackerKind::GitLab.prefill_supported());
        assert!(!BugTrackerKind::Bitbucket.prefill_supported());
        assert!(!BugTrackerKind::Other.prefill_supported());
    }

    #[test]
    fn env_overrides_config_and_origin() {
        let (url, src) = choose_source(
            Some("https://gitlab.corp/team/fork".to_string()),
            Some("https://config.example/x".to_string()),
            Some("git@github.com:oxide-sloc/oxide-sloc.git"),
        );
        assert_eq!(url, "https://gitlab.corp/team/fork");
        assert_eq!(src, TargetSource::Configured);
    }

    #[test]
    fn config_used_when_env_absent() {
        let (url, src) = choose_source(
            None,
            Some("https://config.example/x".to_string()),
            Some("git@github.com:oxide-sloc/oxide-sloc.git"),
        );
        assert_eq!(url, "https://config.example/x");
        assert_eq!(src, TargetSource::Configured);
    }

    #[test]
    fn build_origin_used_when_no_override() {
        let (url, src) = choose_source(None, None, Some("git@github.com:acme/oxide-sloc-fork.git"));
        assert_eq!(url, "git@github.com:acme/oxide-sloc-fork.git");
        assert_eq!(src, TargetSource::BuildOrigin);
    }

    #[test]
    fn blank_values_fall_through_to_default() {
        let (url, src) = choose_source(Some("   ".to_string()), Some(String::new()), None);
        assert_eq!(url, DEFAULT_REPO);
        assert_eq!(src, TargetSource::Default);
    }

    #[test]
    fn tracker_kind_slug_and_label_cover_all_variants() {
        for (kind, slug, label) in [
            (BugTrackerKind::GitHub, "github", "GitHub"),
            (BugTrackerKind::GitLab, "gitlab", "GitLab"),
            (BugTrackerKind::Bitbucket, "bitbucket", "Bitbucket"),
            (BugTrackerKind::Other, "other", "the repository"),
        ] {
            assert_eq!(kind.slug(), slug);
            assert_eq!(kind.label(), label);
        }
    }

    #[test]
    fn target_source_label_covers_all_variants() {
        assert!(TargetSource::Configured.label().contains("configured"));
        assert!(TargetSource::BuildOrigin.label().contains("origin"));
        assert!(TargetSource::Default.label().contains("upstream"));
    }

    #[test]
    fn bare_host_path_without_scheme_becomes_https() {
        assert_eq!(
            to_web_url("github.com/oxide-sloc/oxide-sloc"),
            "https://github.com/oxide-sloc/oxide-sloc"
        );
    }

    #[test]
    fn scheme_url_without_path_keeps_host_only() {
        assert_eq!(to_web_url("https://example.com/"), "https://example.com");
    }

    #[test]
    fn empty_raw_falls_back_to_default_repo() {
        assert_eq!(to_web_url("   "), DEFAULT_REPO);
    }
}
