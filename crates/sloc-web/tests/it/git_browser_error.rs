// Coverage for git_error_message's truncation branch (crates/sloc-web/src/git_browser.rs):
// when a git-operation error exceeds the 600-char cap, the API body is truncated
// with an ellipsis so the payload stays bounded. Driven through the real
// /api/git/refs handler; no network — an invalid-scheme URL is rejected by the
// clone-URL validator (with the URL echoed in the message) before any git op runs.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use sloc_web::make_test_router;
use tower::ServiceExt;

#[tokio::test]
async fn overlong_rejected_repo_url_error_is_truncated_with_ellipsis() {
    // >600 chars and an unsupported scheme → the validator bails with
    // "... permitted (got \"ftp://aaa…\")", a message longer than the cap.
    let repo = format!("ftp://{}", "a".repeat(800));
    let uri = format!("/api/git/refs?repo={repo}");

    let resp = make_test_router()
        .oneshot(Request::get(&uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&bytes);
    assert!(
        body.contains('\u{2026}'),
        "an over-long git error must be truncated with an ellipsis: {body}"
    );
    // Bounded: the JSON envelope plus the 600-char cap, not the full 800+ char URL.
    assert!(
        body.len() < 800,
        "payload stays bounded: {} bytes",
        body.len()
    );
}
