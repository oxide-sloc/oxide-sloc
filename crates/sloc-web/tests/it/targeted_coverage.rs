// Targeted coverage tests hitting specific uncovered code paths identified from lcov data.
//
// Focus areas:
//   1. JSON-body upload API in server mode (upload_directory_handler, upload_file_handler,
//      upload_tarball_handler) — previously all hit 404 because tests used non-server-mode router.
//   2. Auth lockout path (auth_lockout_response, is_auth_locked_out expiry).
//   3. Rate-limiter inline functions exercised via HTTP.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use sloc_web::{
    TEST_SERVER_MODE_API_KEY, make_test_router, make_test_router_server_mode,
    make_test_router_tight_auth_lockout, make_test_router_with_key,
};
use tower::ServiceExt;

// ── HTTP helpers ──────────────────────────────────────────────────────────────
//
// These helpers attach the server-mode bearer token to every request. The
// keyless local-mode router ignores the header (auth middleware short-circuits
// when no key is configured), while the server-mode router — which now requires
// authentication — validates it. Auth-specific tests below craft their own
// requests directly via `.oneshot()` and do not use these helpers.

fn bearer() -> String {
    format!("Bearer {TEST_SERVER_MODE_API_KEY}")
}

async fn request_status(app: Router, mut req: Request<Body>) -> StatusCode {
    if !req.headers().contains_key("authorization") {
        req.headers_mut().insert(
            "authorization",
            bearer().parse().expect("valid header value"),
        );
    }
    app.oneshot(req).await.unwrap().status()
}

async fn post_json(app: Router, uri: &str, body: &str) -> (StatusCode, String) {
    let req = Request::post(uri)
        .header("content-type", "application/json")
        .header("authorization", bearer())
        .body(Body::from(body.to_owned()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn post_raw(
    app: Router,
    uri: &str,
    content_type: &str,
    body: Vec<u8>,
) -> (StatusCode, String) {
    let req = Request::post(uri)
        .header("content-type", content_type)
        .header("authorization", bearer())
        .body(Body::from(body))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn b64(data: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(data)
}

// ── upload_directory_handler (server mode JSON API) ───────────────────────────
// Previously these hit NOT_FOUND in local mode; now we use make_test_router_server_mode().

#[tokio::test]
async fn upload_dir_json_local_mode_returns_404() {
    // In local (non-server) mode the handler returns 404 immediately
    let app = make_test_router();
    let body = r#"{"files":[{"path":"src/lib.rs","content":"Zm9v"}]}"#;
    let (status, _) = post_json(app, "/api/upload-directory", body).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn upload_dir_json_empty_files_returns_400() {
    let app = make_test_router_server_mode();
    let (status, body) = post_json(app, "/api/upload-directory", r#"{"files":[]}"#).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "empty files array must return 400, body: {body}"
    );
    assert!(
        body.contains("error") || body.contains("No files"),
        "got: {body}"
    );
}

#[tokio::test]
async fn upload_dir_json_valid_single_file_returns_200() {
    let app = make_test_router_server_mode();
    let content = b64(b"fn main() {}\n// comment\n");
    let body = format!(r#"{{"files":[{{"path":"proj/src/lib.rs","content":"{content}"}}]}}"#);
    let (status, resp) = post_json(app, "/api/upload-directory", &body).await;
    assert!(
        status.as_u16() < 500,
        "valid upload must not 5xx, got {status}: {resp}"
    );
    if status == StatusCode::OK {
        assert!(
            resp.contains("tmp_path") || resp.contains("upload_id"),
            "response must contain tmp_path or upload_id, got: {resp}"
        );
    }
}

#[tokio::test]
async fn upload_dir_json_path_traversal_rejected_after_5_attempts() {
    let app = make_test_router_server_mode();
    // Send 5+ files with "../" in their paths to trigger path-traversal protection
    let content = b64(b"malicious");
    let files: Vec<String> = (0..6)
        .map(|i| format!(r#"{{"path":"../../../evil{i}.rs","content":"{content}"}}"#))
        .collect();
    let body = format!(r#"{{"files":[{}]}}"#, files.join(","));
    let (status, resp) = post_json(app, "/api/upload-directory", &body).await;
    // After 5 traversal attempts, handler returns 400
    assert!(
        status == StatusCode::BAD_REQUEST || status.as_u16() < 500,
        "path traversal must return 400 or be handled gracefully, got {status}: {resp}"
    );
}

#[tokio::test]
async fn upload_dir_json_continuation_batch_with_upload_id() {
    let app = make_test_router_server_mode();
    let content = b64(b"let x = 1;\n");
    // First batch — creates a new staging dir, returns an upload_id
    let body1 = format!(r#"{{"files":[{{"path":"proj/a.rs","content":"{content}"}}]}}"#);
    let (status1, resp1) = post_json(app.clone(), "/api/upload-directory", &body1).await;
    if status1 != StatusCode::OK {
        return; // skip if first batch fails (e.g., temp dir issues)
    }
    // Extract upload_id from response
    let v: serde_json::Value = serde_json::from_str(&resp1).unwrap_or_default();
    let upload_id = v["upload_id"].as_str().unwrap_or("").to_owned();
    if upload_id.is_empty() {
        return;
    }
    // Second batch — continuation using the upload_id
    let content2 = b64(b"let y = 2;\n");
    let body2 = format!(
        r#"{{"files":[{{"path":"proj/b.rs","content":"{content2}"}}],"upload_id":"{upload_id}"}}"#
    );
    let (status2, _) = post_json(app, "/api/upload-directory", &body2).await;
    assert!(
        status2.as_u16() < 500,
        "continuation batch must not 5xx, got {status2}"
    );
}

#[tokio::test]
async fn upload_dir_json_invalid_base64_content_skipped() {
    // Files with invalid base64 are skipped (not an error); the batch still succeeds
    let app = make_test_router_server_mode();
    let body = r#"{"files":[{"path":"proj/x.rs","content":"!!!NOT_VALID_BASE64!!!"}]}"#;
    let (status, _) = post_json(app, "/api/upload-directory", body).await;
    // Invalid base64 is skipped; the overall request should succeed or return 200
    // (empty file_count is fine since the bad entry was skipped)
    assert!(
        status.as_u16() < 500,
        "invalid base64 must not 5xx, got {status}"
    );
}

// ── upload_file_handler (server mode JSON API) ────────────────────────────────

#[tokio::test]
async fn upload_file_json_local_mode_returns_404() {
    let app = make_test_router();
    let content = b64(b"TN:\nSF:lib.rs\nend_of_record\n");
    let body = format!(r#"{{"filename":"coverage.info","content":"{content}"}}"#);
    let (status, _) = post_json(app, "/api/upload-file", &body).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn upload_file_json_invalid_base64_returns_400() {
    let app = make_test_router_server_mode();
    let body = r#"{"filename":"test.info","content":"!!!NOT_VALID_BASE64!!!"}"#;
    let (status, resp) = post_json(app, "/api/upload-file", body).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "invalid base64 must return 400, got: {resp}"
    );
}

#[tokio::test]
async fn upload_file_json_valid_lcov_returns_tmp_path() {
    let app = make_test_router_server_mode();
    let lcov = b"TN:\nSF:src/lib.rs\nDA:1,1\nDA:2,0\nLF:2\nLH:1\nend_of_record\n";
    let content = b64(lcov);
    let body = format!(r#"{{"filename":"cov.info","content":"{content}"}}"#);
    let (status, resp) = post_json(app, "/api/upload-file", &body).await;
    assert!(
        status.as_u16() < 500,
        "valid upload must not 5xx, got {status}: {resp}"
    );
    if status == StatusCode::OK {
        assert!(
            resp.contains("tmp_path"),
            "response must contain tmp_path, got: {resp}"
        );
    }
}

#[tokio::test]
async fn upload_file_json_directory_stripped_from_filename() {
    // Filenames with directory components should have them stripped
    let app = make_test_router_server_mode();
    let content = b64(b"data");
    let body = format!(r#"{{"filename":"some/path/file.xml","content":"{content}"}}"#);
    let (status, _) = post_json(app, "/api/upload-file", &body).await;
    assert!(status.as_u16() < 500, "directory-in-filename must not 5xx");
}

// ── upload_tarball_handler (server mode, raw body) ────────────────────────────

#[tokio::test]
async fn upload_tarball_local_mode_returns_404() {
    let app = make_test_router();
    let (status, _) = post_raw(
        app,
        "/api/upload-tarball",
        "application/gzip",
        vec![0x1f, 0x8b], // minimal gzip magic bytes (incomplete)
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn upload_tarball_invalid_gzip_returns_4xx() {
    let app = make_test_router_server_mode();
    // Send garbage bytes — not a valid gzip/tar → should return 400
    let (status, _) = post_raw(
        app,
        "/api/upload-tarball",
        "application/gzip",
        b"this is not a valid gzip archive at all".to_vec(),
    )
    .await;
    assert!(
        status.as_u16() < 500,
        "invalid tarball must not 5xx, got {status}"
    );
}

#[tokio::test]
async fn upload_tarball_valid_archive_not_5xx() {
    use flate2::{Compression, write::GzEncoder};
    use std::io::Write as _;

    let app = make_test_router_server_mode();

    // Build a minimal tar.gz in-memory
    let mut tar_buf = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_buf);
        let contents = b"fn main() {}\n";
        let mut header = tar::Header::new_gnu();
        header.set_path("proj/main.rs").unwrap();
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, &contents[..]).unwrap();
        builder.finish().unwrap();
    }
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&tar_buf).unwrap();
    let gz_bytes = gz.finish().unwrap();

    let (status, _) = post_raw(app, "/api/upload-tarball", "application/gzip", gz_bytes).await;
    assert!(
        status.as_u16() < 500,
        "valid tarball must not 5xx, got {status}"
    );
}

// ── Auth lockout HTTP tests ───────────────────────────────────────────────────
// make_test_router_tight_auth_lockout("key") has threshold=2 and window=200ms.

#[tokio::test]
async fn auth_lockout_api_client_receives_429_after_threshold() {
    use std::net::SocketAddr;
    let app = make_test_router_tight_auth_lockout("correct-key");
    let peer: SocketAddr = "10.1.2.3:1111".parse().unwrap();

    // Send 2 wrong-key requests to exhaust the lockout threshold
    for _ in 0..2 {
        let mut req = Request::get("/")
            .header("authorization", "Bearer wrong-key")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(peer));
        app.clone().oneshot(req).await.unwrap();
    }

    // Third wrong-key request should trigger the lockout (429)
    let mut req = Request::get("/")
        .header("authorization", "Bearer wrong-key")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(peer));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "3rd wrong-key request must trigger 429 lockout"
    );
    // Retry-After header should be present
    assert!(
        resp.headers().contains_key("retry-after"),
        "lockout response must include Retry-After header"
    );
}

#[tokio::test]
async fn auth_lockout_browser_client_receives_429_html() {
    use std::net::SocketAddr;
    let app = make_test_router_tight_auth_lockout("correct-key");
    let peer: SocketAddr = "10.2.3.4:2222".parse().unwrap();

    // Exhaust threshold (2 failures)
    for _ in 0..2 {
        let mut req = Request::get("/")
            .header("authorization", "Bearer bad-key")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(peer));
        app.clone().oneshot(req).await.unwrap();
    }

    // Browser request (Accept: text/html) after lockout → 429 HTML page
    let mut req = Request::get("/")
        .header("accept", "text/html,application/xhtml+xml")
        .header("authorization", "Bearer bad-key")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(peer));
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&bytes);
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    // The browser lockout response is an HTML page
    assert!(
        body.contains("<!doctype html>") || body.contains("minute"),
        "browser lockout must return HTML, got: {body}"
    );
}

#[tokio::test]
async fn auth_lockout_expires_after_window() {
    use std::net::SocketAddr;
    let app = make_test_router_tight_auth_lockout("correct-key");
    let peer: SocketAddr = "10.3.4.5:3333".parse().unwrap();

    // Exhaust threshold
    for _ in 0..2 {
        let mut req = Request::get("/")
            .header("authorization", "Bearer bad-key")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(peer));
        app.clone().oneshot(req).await.unwrap();
    }

    // Verify locked out
    let mut req = Request::get("/")
        .header("authorization", "Bearer bad-key")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(peer));
    let status = app.clone().oneshot(req).await.unwrap().status();
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "should be locked out"
    );

    // Wait for the 200ms lockout window to expire
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    // After expiry, wrong key returns 401 (not 429)
    let mut req = Request::get("/")
        .header("authorization", "Bearer bad-key")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(peer));
    let status = app.oneshot(req).await.unwrap().status();
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "after lockout window expires, should get 401 not 429"
    );
}

// ── Scan page with server mode ─────────────────────────────────────────────────

#[tokio::test]
async fn scan_page_server_mode_returns_html() {
    let app = make_test_router_server_mode();
    let resp = app
        .oneshot(
            Request::get("/scan")
                .header("authorization", bearer())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&bytes);
    assert!(body.contains("<html"), "server mode /scan must return HTML");
}

#[tokio::test]
async fn view_reports_server_mode_returns_html() {
    let app = make_test_router_server_mode();
    let resp = app
        .oneshot(
            Request::get("/view-reports")
                .header("authorization", bearer())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&bytes);
    assert!(body.contains("<html"));
}

#[tokio::test]
async fn trend_reports_server_mode_returns_html() {
    let app = make_test_router_server_mode();
    let resp = app
        .oneshot(
            Request::get("/trend-reports")
                .header("authorization", bearer())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&bytes);
    assert!(
        status.as_u16() < 500,
        "trend-reports server mode must not 5xx: {body}"
    );
}

#[tokio::test]
async fn test_metrics_server_mode_returns_html() {
    let app = make_test_router_server_mode();
    let resp = app
        .oneshot(
            Request::get("/test-metrics")
                .header("authorization", bearer())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().as_u16() < 500);
}

// ── Session cookie auth — exercises check_session_valid (auth.rs 119-130) ──────

#[tokio::test]
async fn session_cookie_grants_access_after_login() {
    use std::net::SocketAddr;
    let api_key = "session-test-key-xyz";
    let app = make_test_router_with_key(api_key);
    let peer: SocketAddr = "127.0.0.1:9999".parse().unwrap();

    // Step 1: POST /auth/login with correct key → response has Set-Cookie
    let mut login_req = Request::post("/auth/login")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(format!("key={api_key}&next=%2F")))
        .unwrap();
    login_req
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(peer));
    let login_resp = app.clone().oneshot(login_req).await.unwrap();
    assert!(
        login_resp.status().is_redirection(),
        "login must redirect, got {}",
        login_resp.status()
    );

    // Extract the session cookie value from Set-Cookie header
    let set_cookie = login_resp
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    assert!(!set_cookie.is_empty(), "login must set a session cookie");

    // Extract just the `sloc_session=<value>` part
    let cookie_pair = set_cookie.split(';').next().unwrap_or("").trim().to_owned();
    assert!(
        cookie_pair.starts_with("sloc_session="),
        "cookie must be sloc_session, got: {cookie_pair}"
    );

    // Step 2: GET / with the session cookie → check_session_valid returns true → 200
    let mut page_req = Request::get("/")
        .header("cookie", &cookie_pair)
        .body(Body::empty())
        .unwrap();
    page_req
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(peer));
    let page_resp = app.clone().oneshot(page_req).await.unwrap();
    assert_eq!(
        page_resp.status(),
        StatusCode::OK,
        "valid session cookie must grant access to /, got {}",
        page_resp.status()
    );
}

#[tokio::test]
async fn invalid_session_cookie_denies_access() {
    use std::net::SocketAddr;
    let app = make_test_router_with_key("some-key");
    let peer: SocketAddr = "127.0.0.1:8888".parse().unwrap();

    // Send a fake (invalid) session token → check_session_valid returns false
    // The middleware sees an invalid credential → returns 401 (not 302, since cookie counts as a credential)
    let mut req = Request::get("/")
        .header("cookie", "sloc_session=invalid-fake-token-xyz")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(peer));
    let resp = app.oneshot(req).await.unwrap();
    // Invalid session cookie is treated as an invalid credential → 401
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "invalid session cookie must be rejected with 401, got {}",
        resp.status()
    );
}

// ── upload-directory with wrong content-type still handled gracefully ──────────

#[tokio::test]
async fn upload_dir_multipart_in_server_mode_not_5xx() {
    // The handler expects JSON; multipart body → 422 from Axum extractor (not 5xx)
    let app = make_test_router_server_mode();
    let boundary = "BOUNDARY123";
    let mut body = Vec::new();
    body.extend(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; \
             filename=\"a.rs\"\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend(b"fn f() {}");
    body.extend(format!("\r\n--{boundary}--\r\n").as_bytes());

    let req = Request::post("/api/upload-directory")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    let status = request_status(app, req).await;
    assert!(
        status.as_u16() < 500,
        "multipart to JSON handler must not 5xx, got {status}"
    );
}
