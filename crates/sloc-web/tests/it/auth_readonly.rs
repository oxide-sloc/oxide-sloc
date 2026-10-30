// Coverage for the read-only-credential branch of the auth middleware
// (crates/sloc-web/src/auth.rs): a read-only key authenticates safe methods
// (GET/HEAD/OPTIONS) but is rejected with 403 on state-changing methods, while a
// full-access key is accepted for both. These assert the actual auth/authz
// outcomes, not just that lines execute.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use sloc_web::make_test_router_with_readonly_key;
use tower::ServiceExt;

const FULL: &str = "full-access-key-0123456789";
const RO: &str = "read-only-key-0123456789";

#[tokio::test]
async fn readonly_key_authenticates_a_safe_get() {
    // A read-only key on a safe (GET) request is accepted → the protected page renders.
    let app = make_test_router_with_readonly_key(FULL, RO);
    let resp = app
        .oneshot(
            Request::get("/api-docs")
                .header("authorization", format!("Bearer {RO}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "read-only key must authenticate a safe GET"
    );
}

#[tokio::test]
async fn readonly_key_via_x_api_key_header_also_authenticates_get() {
    // Exercises the X-API-Key credential source (not just Authorization: Bearer).
    let app = make_test_router_with_readonly_key(FULL, RO);
    let resp = app
        .oneshot(
            Request::get("/api-docs")
                .header("x-api-key", RO)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn readonly_key_is_forbidden_on_a_state_changing_post() {
    // A valid read-only key on a mutating method is a 403 (valid credential, wrong
    // privilege) — NOT a 401. The middleware rejects before the handler runs.
    let app = make_test_router_with_readonly_key(FULL, RO);
    let resp = app
        .oneshot(
            Request::post("/analyze")
                .header("x-api-key", RO)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("path=."))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "read-only credential must be 403 on a POST"
    );
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&bytes);
    assert!(
        body.to_lowercase().contains("read-only"),
        "403 body should explain the read-only restriction: {body}"
    );
}

#[tokio::test]
async fn full_access_key_passes_auth_on_a_post() {
    // The full-access key is NOT limited to safe methods: a POST must clear the auth
    // middleware (it may still be rejected downstream by the handler, but never with
    // 401/403 from the auth layer).
    let app = make_test_router_with_readonly_key(FULL, RO);
    let resp = app
        .oneshot(
            Request::post("/analyze")
                .header("authorization", format!("Bearer {FULL}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("path=."))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_ne!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "full-access key must not be method-restricted"
    );
}

#[tokio::test]
async fn no_credential_is_rejected() {
    // With keys configured, an unauthenticated request is 401 (API client) or a
    // redirect to login (browser) — never served, never 5xx.
    let app = make_test_router_with_readonly_key(FULL, RO);
    let resp = app
        .oneshot(Request::get("/api-docs").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::UNAUTHORIZED || status.is_redirection(),
        "no credential must be 401 or redirect, got {status}"
    );
}
