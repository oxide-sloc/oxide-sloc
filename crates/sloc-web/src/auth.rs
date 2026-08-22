// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>
#![allow(clippy::redundant_pub_crate)]

//! Authentication middleware and login form handlers.
//!
//! Provides Bearer-token / X-API-Key / session-cookie auth,
//! login lockout on repeated failures, and the GET/POST /auth/login routes.

use std::{
    net::{IpAddr, SocketAddr},
    sync::OnceLock,
    time::{Duration, Instant},
};

use axum::{
    body::Body,
    extract::{Form, Query, State},
    http::{HeaderMap, HeaderValue, Request, StatusCode, header},
    middleware::Next,
    response::{Html, IntoResponse, Response},
};

use crate::{AppState, CspNonce, LoginTemplate, audit};
use askama::Template as _;

// ── Trusted-proxy / client-IP resolution ───────────────────────────────────────

/// Trusted reverse-proxy addresses parsed once from `SLOC_TRUSTED_PROXIES`
/// (comma-separated IPs). Only when the direct peer is one of these do we believe
/// the `X-Forwarded-For` header — never trust XFF from an arbitrary client.
fn trusted_proxies() -> &'static [IpAddr] {
    static TP: OnceLock<Vec<IpAddr>> = OnceLock::new();
    TP.get_or_init(|| {
        std::env::var("SLOC_TRUSTED_PROXIES")
            .unwrap_or_default()
            .split(',')
            .filter_map(|s| s.trim().parse::<IpAddr>().ok())
            .collect()
    })
}

/// Resolve the real client IP for rate-limiting / lockout. When the transport peer
/// is a configured trusted proxy, walk `X-Forwarded-For` from right to left and
/// return the first address that is not itself a trusted proxy. Otherwise the
/// transport peer IP is authoritative.
fn client_ip_from(headers: &HeaderMap, peer_ip: IpAddr) -> IpAddr {
    if !trusted_proxies().contains(&peer_ip) {
        return peer_ip;
    }
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        for entry in xff.split(',').rev() {
            if let Ok(ip) = entry.trim().parse::<IpAddr>()
                && !trusted_proxies().contains(&ip)
            {
                return ip;
            }
        }
    }
    peer_ip
}

/// Whether session cookies must carry the `Secure` attribute. True when TLS is
/// terminated in-process, or when `SLOC_SECURE_COOKIES` is set (for deployments
/// behind a TLS-terminating reverse proxy).
fn secure_cookies(state: &AppState) -> bool {
    state.tls_enabled
        || std::env::var("SLOC_SECURE_COOKIES")
            .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// Session cookie name. The `__Host-` prefix (Secure + Path=/ + no Domain) is used
/// whenever cookies are Secure, giving the strongest browser binding; plain mode
/// falls back to the unprefixed name (the prefix *requires* Secure).
const fn session_cookie_name(secure: bool) -> &'static str {
    if secure {
        "__Host-sloc_session"
    } else {
        "sloc_session"
    }
}

// ── Middleware ────────────────────────────────────────────────────────────────

/// 503 response returned when server mode is active but no API key is configured.
/// Fails closed: browsers get an explanatory HTML page, API clients get a plain 503.
fn unconfigured_server_mode_response(req: &Request<Body>) -> Response {
    audit::record(
        "auth_denied_unconfigured",
        "denied",
        &[("path", req.uri().path())],
    );
    if is_browser_request(req) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Html(
                r#"<!doctype html><html><head><meta charset="utf-8">
<title>Authentication Required — OxideSLOC</title>
<style>body{font-family:system-ui,sans-serif;max-width:520px;margin:80px auto;padding:0 24px;color:#2f241c}
h1{color:#b85d33}p{line-height:1.6}code{background:#f3e9e0;padding:2px 6px;border-radius:4px}</style>
</head><body>
<h1>Authentication not configured</h1>
<p>This server is running in network mode but <code>SLOC_API_KEY</code> is not set.</p>
<p>Restart with <code>SLOC_API_KEY=&lt;secret&gt;</code> or <code>SLOC_API_KEYS=&lt;k1,k2&gt;</code>
to enable access.</p>
</body></html>"#
                    .to_owned(),
            ),
        )
            .into_response();
    }
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::WWW_AUTHENTICATE, "Bearer realm=\"oxide-sloc\"")],
        "503 Service Unavailable — set SLOC_API_KEY to enable server-mode access\n",
    )
        .into_response()
}

/// True for state-changing HTTP methods (the ones worth auditing on success).
const fn is_mutating(method: &axum::http::Method) -> bool {
    use axum::http::Method;
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

/// True for non-state-changing methods that a read-only credential may perform.
const fn is_safe_method(method: &axum::http::Method) -> bool {
    use axum::http::Method;
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// True if any provided credential constant-time-matches a configured API key.
/// Kept as a free function so its nested closures don't score against the caller.
fn any_key_matches(candidates: &[&Option<String>], keys: &[secrecy::SecretBox<String>]) -> bool {
    use secrecy::ExposeSecret;
    candidates.iter().filter_map(|o| o.as_deref()).any(|k| {
        keys.iter()
            .any(|expected| ct_eq(k, expected.expose_secret()))
    })
}

/// Whether the operator admin key (`SLOC_ADMIN_KEY`) is configured. Operator-only
/// controls fall back to the normal API-key gate when this is unset, so nothing is
/// locked away on deployments that don't opt into a separate admin credential.
#[must_use]
pub(crate) fn admin_key_configured() -> bool {
    std::env::var("SLOC_ADMIN_KEY").is_ok_and(|k| !k.trim().is_empty())
}

/// Whether this request presents the operator admin key. Checked (constant-time)
/// against `Authorization: Bearer …`, `X-Admin-Key`, and `X-Api-Key`. Returns false
/// when `SLOC_ADMIN_KEY` is unset, so callers must decide the unset-key policy.
#[must_use]
pub(crate) fn is_admin_request(headers: &header::HeaderMap) -> bool {
    let Ok(admin) = std::env::var("SLOC_ADMIN_KEY") else {
        return false;
    };
    let admin = admin.trim();
    if admin.is_empty() {
        return false;
    }
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);
    let x_admin = headers.get("x-admin-key").and_then(|v| v.to_str().ok());
    let x_api = headers.get("x-api-key").and_then(|v| v.to_str().ok());
    [bearer, x_admin, x_api]
        .into_iter()
        .flatten()
        .any(|candidate| ct_eq(candidate, admin))
}

/// Audit a successful authenticated request, but only for state-changing methods.
/// Auditing every GET (assets, polling) would flood the sink with no security value;
/// the who-did-what trail that matters is mutating requests.
fn audit_authenticated_success(req: &Request<Body>, peer_ip: IpAddr, session_valid: bool) {
    if !is_mutating(req.method()) {
        return;
    }
    let via = if session_valid { "session" } else { "api_key" };
    audit::record(
        "auth_success",
        "success",
        &[
            ("peer_ip", &peer_ip.to_string()),
            ("method", req.method().as_str()),
            ("path", req.uri().path()),
            ("via", via),
        ],
    );
}

/// Pull the three accepted credential carriers out of the request headers:
/// `(Bearer token, X-API-Key, session cookie)`.
fn extract_credentials(req: &Request<Body>) -> (Option<String>, Option<String>, Option<String>) {
    let auth_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned);
    let x_api_key = req
        .headers()
        .get("X-API-Key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let session_cookie = req
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(extract_session_cookie)
        .map(str::to_owned);
    (auth_header, x_api_key, session_cookie)
}

pub(crate) async fn require_api_key(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if state.api_keys.is_empty() {
        // In server mode with no API key configured every protected route would
        // be publicly accessible. Fail closed so operators must opt in — unless they
        // explicitly accepted an open server via SLOC_ALLOW_UNAUTHENTICATED=1.
        // Desktop mode (server_mode = false) keeps the open-by-default behaviour.
        if state.server_mode && !state.allow_unauthenticated {
            return unconfigured_server_mode_response(&req);
        }
        return next.run(req).await;
    }

    let keys = &state.api_keys;
    let conn_ip = req
        .extensions()
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .map_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), |c| c.0.ip());
    let peer_ip = client_ip_from(req.headers(), conn_ip);

    let (auth_header, x_api_key, session_cookie) = extract_credentials(&req);

    let session_valid = check_session_valid(session_cookie.as_deref(), &state);
    let any_credential_provided =
        auth_header.is_some() || x_api_key.is_some() || session_cookie.is_some();
    let valid = session_valid || any_key_matches(&[&auth_header, &x_api_key], keys);

    if valid {
        audit_authenticated_success(&req, peer_ip, session_valid);
        return next.run(req).await;
    }

    // Read-only credentials: authenticate for safe methods only. A read-only key
    // that matches but targets a state-changing method is rejected with 403 rather
    // than treated as an auth failure (it is a valid credential, wrong privilege).
    if !state.readonly_api_keys.is_empty()
        && any_key_matches(&[&auth_header, &x_api_key], &state.readonly_api_keys)
    {
        if is_safe_method(req.method()) {
            return next.run(req).await;
        }
        audit::record(
            "authz_denied",
            "denied",
            &[
                ("peer_ip", &peer_ip.to_string()),
                ("method", req.method().as_str()),
                ("path", req.uri().path()),
                ("role", "readonly"),
            ],
        );
        return (
            StatusCode::FORBIDDEN,
            "403 Forbidden — read-only credential cannot perform this action\n",
        )
            .into_response();
    }

    if state.rate_limiter.is_auth_locked_out(peer_ip) {
        return auth_lockout_response(&req, &state.rate_limiter, peer_ip);
    }

    if any_credential_provided {
        state.rate_limiter.record_auth_failure(peer_ip);
        audit::record(
            "auth_failure",
            "failure",
            &[
                ("peer_ip", &peer_ip.to_string()),
                ("method", req.method().as_str()),
                ("path", req.uri().path()),
            ],
        );
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer realm=\"oxide-sloc\"")],
            "401 Unauthorized\n",
        )
            .into_response();
    }

    // No credential — redirect browsers, plain 401 for API clients.
    if is_browser_request(&req) {
        let next_path = req.uri().path_and_query().map_or("/", |pq| pq.as_str());
        let next_path = sanitize_next(next_path);
        let login_url = format!("/auth/login?next={}", urlencode_path(next_path));
        let location = HeaderValue::from_str(&login_url)
            .unwrap_or_else(|_| HeaderValue::from_static("/auth/login"));
        let mut resp = StatusCode::FOUND.into_response();
        resp.headers_mut().insert(header::LOCATION, location);
        return resp;
    }
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer realm=\"oxide-sloc\"")],
        "401 Unauthorized\n",
    )
        .into_response()
}

// ── Session helpers ───────────────────────────────────────────────────────────

fn check_session_valid(token: Option<&str>, state: &AppState) -> bool {
    let Some(tok) = token else { return false };
    let now = Instant::now();
    let idle = crate::session_idle_timeout();
    let mut sessions = state
        .sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(sess) = sessions.get_mut(tok) {
        let absolute_ok = now < sess.absolute_expiry;
        // Idle timeout is opt-in; when unset every session passes the idle check.
        let idle_ok = idle.is_none_or(|d| now.duration_since(sess.last_seen) < d);
        if absolute_ok && idle_ok {
            // Sliding window: each authenticated request refreshes last-seen.
            sess.last_seen = now;
            return true;
        }
        sessions.remove(tok);
    }
    false
}

fn auth_lockout_response(
    req: &Request<Body>,
    rate_limiter: &crate::IpRateLimiter,
    peer_ip: IpAddr,
) -> Response {
    audit::record(
        "auth_lockout",
        "denied",
        &[("peer_ip", &peer_ip.to_string())],
    );
    let remaining = rate_limiter.auth_lockout_remaining_secs(peer_ip);
    let retry_after =
        HeaderValue::from_str(&remaining.to_string()).unwrap_or(HeaderValue::from_static("3600"));
    if is_browser_request(req) {
        let minutes = remaining.div_ceil(60).max(1);
        let s = if minutes == 1 { "" } else { "s" };
        let body = format!(
            r#"<!doctype html><html><head><meta charset="utf-8">
<title>Locked Out — OxideSLOC</title>
<style>body{{font-family:system-ui,sans-serif;max-width:520px;margin:80px auto;padding:0 24px;color:#2f241c}}
h1{{color:#b85d33}}p{{line-height:1.6}}code{{background:#f3e9e0;padding:2px 6px;border-radius:4px}}</style>
</head><body>
<h1>Too many failed sign-in attempts</h1>
<p>Access from your IP is temporarily locked. Lockout expires in approximately
<strong>{minutes} minute{s}</strong>.</p>
<p>To clear immediately, restart the server.</p>
<p>For trusted LAN testing, leave <code>SLOC_API_KEY</code> unset, or raise the
threshold via <code>SLOC_AUTH_LOCKOUT_FAILS</code> / <code>SLOC_AUTH_LOCKOUT_SECS</code>.</p>
</body></html>"#
        );
        let mut resp = (StatusCode::TOO_MANY_REQUESTS, Html(body)).into_response();
        resp.headers_mut().insert(header::RETRY_AFTER, retry_after);
        return resp;
    }
    let mut resp = (
        StatusCode::TOO_MANY_REQUESTS,
        format!("429 Too Many Requests — locked out, retry in {remaining}s\n"),
    )
        .into_response();
    resp.headers_mut().insert(header::RETRY_AFTER, retry_after);
    resp
}

fn ct_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

fn extract_session_cookie(cookie_header: &str) -> Option<&str> {
    // Accept both the plain and the `__Host-`-prefixed (Secure) cookie names so a
    // session survives regardless of which mode set it.
    cookie_header.split(';').find_map(|pair| {
        let pair = pair.trim();
        let (k, v) = pair.split_once('=')?;
        if matches!(k.trim(), "sloc_session" | "__Host-sloc_session") {
            Some(v.trim())
        } else {
            None
        }
    })
}

fn is_browser_request(req: &Request<Body>) -> bool {
    req.headers()
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains("text/html"))
}

/// Returns `raw` only if it is a safe same-origin relative path to redirect to
/// after login.  Any value that could cause a redirect loop or open redirect
/// (e.g. `/auth/login*`, `//host`, paths containing `://`) is replaced with `/`.
fn sanitize_next(raw: &str) -> &str {
    if raw.starts_with('/')
        && !raw.starts_with("//")
        && !raw.contains("://")
        && !raw.starts_with("/auth/")
    {
        raw
    } else {
        "/"
    }
}

fn urlencode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b'/'
            | b'?'
            | b'='
            | b'&'
            | b'#' => {
                out.push(b as char);
            }
            _ => {
                use std::fmt::Write as _;
                write!(&mut out, "%{b:02X}").ok();
            }
        }
    }
    out
}

// ── Login form handlers ────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub(crate) struct LoginQuery {
    next: Option<String>,
    error: Option<String>,
}

#[derive(serde::Deserialize)]
pub(crate) struct LoginFormData {
    key: String,
    next: Option<String>,
}

pub(crate) async fn auth_login_get(
    State(state): State<AppState>,
    Query(query): Query<LoginQuery>,
    axum::extract::Extension(CspNonce(csp_nonce)): axum::extract::Extension<CspNonce>,
) -> Response {
    if state.api_keys.is_empty() {
        let mut resp = StatusCode::FOUND.into_response();
        resp.headers_mut()
            .insert(header::LOCATION, HeaderValue::from_static("/"));
        return resp;
    }
    let has_error = query.error.as_deref() == Some("1");
    let next_url = query.next.as_deref().map_or("/", sanitize_next).to_string();
    let lockout_threshold = state.rate_limiter.auth_lockout_threshold;
    Html(
        LoginTemplate {
            csp_nonce,
            has_error,
            next_url,
            lockout_threshold,
        }
        .render()
        .unwrap_or_else(|e| format!("<pre>Template error: {e}</pre>")),
    )
    .into_response()
}

pub(crate) async fn auth_login_post(
    State(state): State<AppState>,
    axum::extract::ConnectInfo(peer_addr): axum::extract::ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(mut form): Form<LoginFormData>,
) -> Response {
    let peer_ip = client_ip_from(&headers, peer_addr.ip());
    let next_url = form
        .next
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("/");
    let safe_next = sanitize_next(next_url);

    // Move the submitted key into a zeroizing container so the plaintext credential
    // is wiped from memory as soon as the constant-time comparison completes, rather
    // than lingering in the form String until it happens to be overwritten.
    let submitted = secrecy::SecretBox::new(Box::new(std::mem::take(&mut form.key)));
    let valid = {
        use secrecy::ExposeSecret;
        state
            .api_keys
            .iter()
            .any(|expected| ct_eq(submitted.expose_secret(), expected.expose_secret()))
    };
    drop(submitted);

    if valid {
        const SESSION_SECS: u64 = 8 * 3600;
        let session_id = uuid::Uuid::new_v4().to_string();
        let now = Instant::now();
        let expiry = now + Duration::from_secs(SESSION_SECS);
        let mut sessions = state
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Evict expired sessions to prevent unbounded memory growth under sustained logins.
        sessions.retain(|_, s| s.absolute_expiry > now);
        sessions.insert(
            session_id.clone(),
            crate::SessionState {
                absolute_expiry: expiry,
                last_seen: now,
            },
        );
        drop(sessions);
        let secure = secure_cookies(&state);
        let secure_flag = if secure { "; Secure" } else { "" };
        let name = session_cookie_name(secure);
        let cookie_value = format!(
            "{name}={session_id}; Path=/; HttpOnly; SameSite=Strict; Max-Age={SESSION_SECS}{secure_flag}",
        );
        let location =
            HeaderValue::from_str(safe_next).unwrap_or_else(|_| HeaderValue::from_static("/"));
        let cookie_hv = HeaderValue::from_str(&cookie_value)
            .unwrap_or_else(|_| HeaderValue::from_static("sloc_session=; Path=/; HttpOnly"));
        let mut resp = StatusCode::FOUND.into_response();
        resp.headers_mut().insert(header::LOCATION, location);
        resp.headers_mut().insert(header::SET_COOKIE, cookie_hv);
        audit::record(
            "login_success",
            "success",
            &[("peer_ip", &peer_ip.to_string())],
        );
        resp
    } else {
        state.rate_limiter.record_auth_failure(peer_ip);
        audit::record(
            "login_failure",
            "failure",
            &[("peer_ip", &peer_ip.to_string()), ("path", "/auth/login")],
        );
        let error_url = format!("/auth/login?next={}&error=1", urlencode_path(safe_next));
        let location = HeaderValue::from_str(&error_url)
            .unwrap_or_else(|_| HeaderValue::from_static("/auth/login?error=1"));
        let mut resp = StatusCode::FOUND.into_response();
        resp.headers_mut().insert(header::LOCATION, location);
        resp
    }
}

// ── Logout handler ────────────────────────────────────────────────────────────

/// POST /auth/logout — invalidates the current session cookie immediately.
/// Redirects to /auth/login (or / when no API key is configured).
pub(crate) async fn auth_logout(State(state): State<AppState>, req: Request<Body>) -> Response {
    // Remove the session from the server-side store so the token is dead instantly.
    if let Some(tok) = req
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(extract_session_cookie)
    {
        let mut sessions = state
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        sessions.remove(tok);
    }

    // Expire the cookie on the client side regardless. We don't know which name was
    // set (plain vs `__Host-` Secure), so expire both variants.
    let expire_plain = "sloc_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT";
    let expire_host = "__Host-sloc_session=; Path=/; HttpOnly; SameSite=Strict; Secure; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT";

    let redirect_to = if state.api_keys.is_empty() {
        "/"
    } else {
        "/auth/login"
    };
    let location = HeaderValue::from_str(redirect_to).unwrap_or(HeaderValue::from_static("/"));

    let mut resp = StatusCode::FOUND.into_response();
    resp.headers_mut().insert(header::LOCATION, location);
    if let Ok(hv) = HeaderValue::from_str(expire_plain) {
        resp.headers_mut().append(header::SET_COOKIE, hv);
    }
    if let Ok(hv) = HeaderValue::from_str(expire_host) {
        resp.headers_mut().append(header::SET_COOKIE, hv);
    }
    audit::record("logout", "success", &[]);
    resp
}

// ── Consent acknowledgement ─────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub(crate) struct ConsentQuery {
    next: Option<String>,
}

/// GET /auth/consent — records acknowledgement of the notice-and-consent banner in
/// a session-scoped cookie and returns the user to their original destination.
/// Reachable without authentication and exempt from the consent gate itself.
pub(crate) async fn auth_consent_accept(
    State(state): State<AppState>,
    Query(query): Query<ConsentQuery>,
) -> Response {
    let next = query.next.as_deref().map_or("/", sanitize_next).to_owned();
    let secure = secure_cookies(&state);
    let secure_flag = if secure { "; Secure" } else { "" };
    // Session-scoped (no Max-Age): the banner is re-shown on each new browser session.
    let cookie = format!("sloc_consent=1; Path=/; HttpOnly; SameSite=Strict{secure_flag}");
    let location = HeaderValue::from_str(&next).unwrap_or_else(|_| HeaderValue::from_static("/"));
    let cookie_hv = HeaderValue::from_str(&cookie)
        .unwrap_or_else(|_| HeaderValue::from_static("sloc_consent=1; Path=/; HttpOnly"));
    let mut resp = StatusCode::FOUND.into_response();
    resp.headers_mut().insert(header::LOCATION, location);
    resp.headers_mut().insert(header::SET_COOKIE, cookie_hv);
    audit::record("consent_ack", "success", &[]);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── sanitize_next ────────────────────────────────────────────────────────

    #[test]
    fn sanitize_next_accepts_simple_path() {
        assert_eq!(sanitize_next("/dashboard"), "/dashboard");
    }

    #[test]
    fn sanitize_next_accepts_path_with_query() {
        assert_eq!(sanitize_next("/scan?path=."), "/scan?path=.");
    }

    #[test]
    fn sanitize_next_accepts_root() {
        assert_eq!(sanitize_next("/"), "/");
    }

    #[test]
    fn sanitize_next_accepts_nested_path() {
        assert_eq!(
            sanitize_next("/api/runs/some-id/status"),
            "/api/runs/some-id/status"
        );
    }

    #[test]
    fn sanitize_next_rejects_double_slash_open_redirect() {
        assert_eq!(sanitize_next("//evil.com/steal"), "/");
    }

    #[test]
    fn sanitize_next_rejects_absolute_url_with_scheme() {
        assert_eq!(sanitize_next("https://evil.com"), "/");
    }

    #[test]
    fn sanitize_next_rejects_javascript_scheme() {
        assert_eq!(sanitize_next("javascript://xss"), "/");
    }

    #[test]
    fn sanitize_next_rejects_exact_login_path() {
        assert_eq!(sanitize_next("/auth/login"), "/");
    }

    #[test]
    fn sanitize_next_rejects_login_path_with_query() {
        assert_eq!(sanitize_next("/auth/login?next=/foo"), "/");
    }

    #[test]
    fn sanitize_next_rejects_empty_string() {
        assert_eq!(sanitize_next(""), "/");
    }

    #[test]
    fn sanitize_next_rejects_relative_path_without_leading_slash() {
        assert_eq!(sanitize_next("dashboard"), "/");
    }

    // ── urlencode_path ───────────────────────────────────────────────────────

    #[test]
    fn urlencode_path_passes_through_safe_chars() {
        assert_eq!(urlencode_path("/scan?path=."), "/scan?path=.");
    }

    #[test]
    fn urlencode_path_preserves_alphanumeric_and_path_chars() {
        let s = "/abc/DEF/123?foo=bar&baz=qux#anchor";
        assert_eq!(urlencode_path(s), s);
    }

    #[test]
    fn urlencode_path_encodes_spaces() {
        assert_eq!(urlencode_path("/path with space"), "/path%20with%20space");
    }

    #[test]
    fn urlencode_path_encodes_plus_sign() {
        assert!(
            urlencode_path("/a+b").contains("%2B"),
            "'+' must be percent-encoded"
        );
    }

    #[test]
    fn urlencode_path_encodes_angle_brackets() {
        let encoded = urlencode_path("<script>");
        assert!(
            encoded.contains("%3C") && encoded.contains("%3E"),
            "angle brackets must be encoded"
        );
    }

    #[test]
    fn urlencode_path_empty_string() {
        assert_eq!(urlencode_path(""), "");
    }

    #[test]
    fn urlencode_path_preserves_tilde_and_underscore() {
        assert_eq!(urlencode_path("/~user_name"), "/~user_name");
    }

    // ── extract_session_cookie ───────────────────────────────────────────────

    #[test]
    fn extract_session_cookie_found_single() {
        assert_eq!(
            extract_session_cookie("sloc_session=abc123"),
            Some("abc123")
        );
    }

    #[test]
    fn extract_session_cookie_found_among_multiple() {
        assert_eq!(
            extract_session_cookie("other=val; sloc_session=mysession; another=x"),
            Some("mysession")
        );
    }

    #[test]
    fn extract_session_cookie_not_found() {
        assert_eq!(extract_session_cookie("other=value; unrelated=abc"), None);
    }

    #[test]
    fn extract_session_cookie_empty_header() {
        assert_eq!(extract_session_cookie(""), None);
    }

    #[test]
    fn extract_session_cookie_partial_name_no_match() {
        assert_eq!(extract_session_cookie("sloc_session_extra=abc"), None);
    }

    #[test]
    fn extract_session_cookie_first_wins() {
        assert_eq!(
            extract_session_cookie("sloc_session=first; sloc_session=second"),
            Some("first")
        );
    }

    #[test]
    fn extract_session_cookie_with_extra_whitespace() {
        assert_eq!(
            extract_session_cookie("  sloc_session  =  token_val  "),
            Some("token_val")
        );
    }

    // ── is_browser_request / is_mutating / session_cookie_name ────────────────

    fn req_with_accept(accept: Option<&str>) -> Request<Body> {
        let mut b = Request::builder().uri("/api/runs");
        if let Some(a) = accept {
            b = b.header(header::ACCEPT, a);
        }
        b.body(Body::empty()).unwrap()
    }

    #[test]
    fn is_browser_request_true_for_html_accept() {
        assert!(is_browser_request(&req_with_accept(Some("text/html,*/*"))));
    }

    #[test]
    fn is_browser_request_false_for_json_or_missing_accept() {
        assert!(!is_browser_request(&req_with_accept(Some(
            "application/json"
        ))));
        assert!(!is_browser_request(&req_with_accept(None)));
    }

    #[test]
    fn is_mutating_classifies_methods() {
        use axum::http::Method;
        for m in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            assert!(is_mutating(&m), "{m} should be mutating");
        }
        for m in [Method::GET, Method::HEAD, Method::OPTIONS] {
            assert!(!is_mutating(&m), "{m} should not be mutating");
        }
    }

    #[test]
    fn session_cookie_name_uses_host_prefix_when_secure() {
        assert_eq!(session_cookie_name(true), "__Host-sloc_session");
        assert_eq!(session_cookie_name(false), "sloc_session");
    }

    // ── unconfigured_server_mode_response (fail-closed 503) ───────────────────

    #[test]
    fn unconfigured_response_html_for_browser() {
        let req = req_with_accept(Some("text/html"));
        let resp = unconfigured_server_mode_response(&req);
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(ct.contains("text/html"), "browser must get HTML, got {ct}");
    }

    #[test]
    fn unconfigured_response_plain_for_api_client() {
        let req = req_with_accept(Some("application/json"));
        let resp = unconfigured_server_mode_response(&req);
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            resp.headers().contains_key(header::WWW_AUTHENTICATE),
            "API clients must receive a WWW-Authenticate challenge"
        );
    }

    // ── any_key_matches (constant-time credential comparison) ─────────────────

    #[test]
    fn any_key_matches_detects_present_and_absent_credentials() {
        use secrecy::SecretBox;
        let keys = vec![SecretBox::new(Box::new("s3cret".to_string()))];
        let good = Some("s3cret".to_string());
        let bad = Some("nope".to_string());
        let none: Option<String> = None;
        assert!(any_key_matches(&[&good, &none], &keys));
        assert!(!any_key_matches(&[&bad, &none], &keys));
        // No candidates at all → no match.
        assert!(!any_key_matches(&[&none], &keys));
    }

    #[test]
    fn admin_request_matches_only_configured_key() {
        use axum::http::HeaderMap;
        // SAFETY: only this test reads/writes SLOC_ADMIN_KEY in the auth unit tests,
        // and it is cleared before returning, so no other test observes it.
        unsafe {
            std::env::remove_var("SLOC_ADMIN_KEY");
        }
        let mut h = HeaderMap::new();
        h.insert("x-admin-key", "topsecret".parse().unwrap());
        // Unset ⇒ not configured, never admin (callers fall back to the API-key gate).
        assert!(!admin_key_configured());
        assert!(!is_admin_request(&h));

        unsafe {
            std::env::set_var("SLOC_ADMIN_KEY", "topsecret");
        }
        assert!(admin_key_configured());
        assert!(is_admin_request(&h), "x-admin-key must be accepted");

        let mut hb = HeaderMap::new();
        hb.insert(header::AUTHORIZATION, "Bearer topsecret".parse().unwrap());
        assert!(is_admin_request(&hb), "Bearer admin key must be accepted");

        let mut hw = HeaderMap::new();
        hw.insert("x-admin-key", "wrong".parse().unwrap());
        assert!(!is_admin_request(&hw), "wrong key must be rejected");

        unsafe {
            std::env::remove_var("SLOC_ADMIN_KEY");
        }
    }
}
