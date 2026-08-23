// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>
//
// Air-gap / connectivity detection for the web UI.
//
// oxide-sloc runs unchanged on both internet-connected and fully air-gapped hosts. A handful
// of footer links (the author's GitHub profile, "View on GitHub", the AGPL licence page) point
// at the public internet and are dead links on an air-gapped network. This module decides, per
// deployment, whether the UI should present itself in "offline" mode so the shared front-end
// script (`app.js`) can repoint or plain-text those links.
//
// Detection deliberately makes NO outbound network connection — an active probe would generate
// traffic that is undesirable (and often alarm-worthy) on a locked-down / air-gapped network.
// The posture is resolved, highest priority first, from:
//
//   1. Explicit override — the `SLOC_AIRGAP` env var, or the `[reporting] offline_mode` config
//      key. A truthy value forces offline, a falsy value forces online.
//   2. Build-origin heuristic — the git origin remote captured at build time
//      (`SLOC_BUILD_ORIGIN_URL`, via the "Report a Bug" target resolver). A public host
//      (github.com / gitlab.com / bitbucket.org) implies online; any other (internal) host
//      implies offline — the build came from an internal fork/mirror.
//   3. Default — online.

use axum::{Json, extract::State, response::IntoResponse};
use serde::Serialize;

use super::AppState;
use super::report_bug::{TargetSource, resolve_target};

/// Public git hosts. A build whose origin remote is one of these (or a subdomain of one) is
/// treated as internet-connected; anything else is assumed to be an internal, air-gapped
/// mirror or fork.
const PUBLIC_GIT_HOSTS: &[&str] = &["github.com", "gitlab.com", "bitbucket.org"];

/// Resolved connectivity posture for the current deployment, serialised to the front-end.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Connectivity {
    /// True when the UI should present itself as air-gapped (offline).
    pub(crate) offline: bool,
    /// Browsable repository URL this build came from (fork or upstream). Informational; the
    /// footer script only repoints links at it when `repo_reachable` is true.
    pub(crate) repo_url: String,
    /// True when `repo_url` is a reachable, deployment-local repository — a real (non-default)
    /// origin on a non-public host — so it is a usable repoint target for external footer links
    /// on an air-gapped host. False for the upstream default and for public git hosts (whose
    /// URLs are exactly the ones that are dead on an air-gapped network).
    pub(crate) repo_reachable: bool,
}

/// Parse a truthy / falsy override string. Returns `None` for empty or unrecognised values so
/// the next detection tier is consulted.
fn parse_override(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "offline" | "airgap" | "air-gapped" => Some(true),
        "0" | "false" | "no" | "off" | "online" => Some(false),
        _ => None,
    }
}

/// Extract the lower-cased host component of a normalised web URL
/// (`https://host:port/owner/repo` -> `host`).
fn host_of(web_url: &str) -> Option<String> {
    let after = web_url.split_once("://").map_or(web_url, |(_, rest)| rest);
    let host = after.split(['/', ':']).next()?;
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}

/// True when `host` is a public git host (exact match or a subdomain of one).
fn is_public_host(host: &str) -> bool {
    PUBLIC_GIT_HOSTS
        .iter()
        .any(|p| host == *p || host.ends_with(&format!(".{p}")))
}

/// Decide offline vs online from an explicit override and the build-origin host. Split out
/// from [`detect`] so the precedence is unit-testable without touching process env.
///
/// `is_default` marks that the origin fell back to the upstream default (no real origin was
/// detected), in which case the host heuristic is skipped — an ordinary online `cargo install`
/// user has no git origin and must not be misclassified as air-gapped.
fn decide_offline(explicit: Option<bool>, origin_host: Option<&str>, is_default: bool) -> bool {
    if let Some(forced) = explicit {
        return forced;
    }
    if !is_default && let Some(host) = origin_host {
        return !is_public_host(host);
    }
    false
}

/// Resolve the connectivity posture for this deployment. Performs no network I/O.
pub(crate) fn detect(state: &AppState) -> Connectivity {
    let explicit = std::env::var("SLOC_AIRGAP")
        .ok()
        .as_deref()
        .and_then(parse_override)
        .or(state.base_config.reporting.offline_mode);

    let target = resolve_target(state);
    let is_default = target.source() == TargetSource::Default;
    let host = host_of(target.web_url());
    let offline = decide_offline(explicit, host.as_deref(), is_default);
    // A repoint target is only useful when it is a real origin on a non-public (internal)
    // host — a public github.com/gitlab.com URL is exactly what's unreachable air-gapped.
    let repo_reachable = !is_default && host.as_deref().is_some_and(|h| !is_public_host(h));

    Connectivity {
        offline,
        repo_url: target.web_url().to_string(),
        repo_reachable,
    }
}

/// `GET /api/connectivity` — reports whether the UI should render in offline (air-gapped) mode
/// and the repository URL to repoint external links at. Public (no API-key auth) so the shared
/// footer script works on every page, including authenticated servers. The payload is
/// non-sensitive (a boolean plus a repository URL already shown on the "Report a Bug" page).
pub(crate) async fn connectivity_handler(State(state): State<AppState>) -> impl IntoResponse {
    Json(detect(&state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_wins_over_heuristic() {
        // Forced offline even though the origin is a public host.
        assert!(decide_offline(Some(true), Some("github.com"), false));
        // Forced online even though the origin is an internal host.
        assert!(!decide_offline(
            Some(false),
            Some("git.internal.corp"),
            false
        ));
    }

    #[test]
    fn public_origin_is_online() {
        assert!(!decide_offline(None, Some("github.com"), false));
        assert!(!decide_offline(None, Some("gitlab.com"), false));
        assert!(!decide_offline(None, Some("bitbucket.org"), false));
    }

    #[test]
    fn public_subdomain_is_online() {
        assert!(!decide_offline(None, Some("www.github.com"), false));
    }

    #[test]
    fn internal_origin_is_offline() {
        assert!(decide_offline(None, Some("git.internal.corp"), false));
        assert!(decide_offline(None, Some("bitbucket.instance2.com"), false));
    }

    #[test]
    fn default_fallback_stays_online() {
        // No real origin (upstream default) => never auto-classified as air-gapped.
        assert!(!decide_offline(None, Some("github.com"), true));
        assert!(!decide_offline(None, None, true));
    }

    #[test]
    fn parse_override_variants() {
        for t in ["1", "true", "YES", "on", "offline", "airgap", "air-gapped"] {
            assert_eq!(parse_override(t), Some(true), "{t}");
        }
        for f in ["0", "false", "NO", "off", "online"] {
            assert_eq!(parse_override(f), Some(false), "{f}");
        }
        for u in ["", "  ", "maybe", "2"] {
            assert_eq!(parse_override(u), None, "{u:?}");
        }
    }

    #[test]
    fn is_public_host_matches_hosts_and_subdomains() {
        assert!(is_public_host("github.com"));
        assert!(is_public_host("gitlab.com"));
        assert!(is_public_host("bitbucket.org"));
        assert!(is_public_host("www.github.com"));
        assert!(!is_public_host("git.internal.corp"));
        assert!(!is_public_host("github.com.evil.example")); // suffix, not subdomain
    }

    #[test]
    fn host_of_extracts_host() {
        assert_eq!(
            host_of("https://github.com/a/b").as_deref(),
            Some("github.com")
        );
        assert_eq!(
            host_of("http://gitlab.internal:2222/team/repo").as_deref(),
            Some("gitlab.internal")
        );
        assert_eq!(host_of("").as_deref(), None);
    }
}
