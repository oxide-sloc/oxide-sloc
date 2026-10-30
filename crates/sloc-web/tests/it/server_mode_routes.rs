//! Regression tests pinning the attack-surface contract for the desktop-only
//! routes in server mode. These routes drive native OS dialogs / the OS file
//! manager, which must never be reachable on a network-facing (server-mode)
//! deployment. The handlers already gate on `state.server_mode`; this locks that
//! behaviour in so a future refactor can't silently re-expose them.

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use sloc_web::{TEST_SERVER_MODE_API_KEY, make_test_router_server_mode};
use tower::ServiceExt;

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn authed(path: &str) -> Request<Body> {
    Request::get(path)
        .header(
            header::AUTHORIZATION,
            format!("Bearer {TEST_SERVER_MODE_API_KEY}"),
        )
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn open_path_is_disabled_in_server_mode() {
    // Not feature-gated: always refuses to launch an OS file manager in server mode.
    let resp = make_test_router_server_mode()
        .oneshot(authed("/open-path?path=/etc"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("server_mode_disabled"),
        "open-path must be disabled in server mode, got: {body}"
    );
}

#[tokio::test]
async fn pick_directory_never_opens_a_dialog_in_server_mode() {
    let resp = make_test_router_server_mode()
        .oneshot(authed("/pick-directory?kind=project"))
        .await
        .unwrap();
    // Inert either way: 404 when the native-dialog feature is on (route hidden in
    // server mode), or a no-op `cancelled` response when it is compiled out.
    let status = resp.status();
    let body = body_string(resp).await;
    assert!(
        status == StatusCode::NOT_FOUND || body.contains("\"cancelled\":true"),
        "pick-directory must never open a native dialog in server mode (status {status}, body {body})"
    );
}

#[tokio::test]
async fn pick_file_never_opens_a_dialog_in_server_mode() {
    let resp = make_test_router_server_mode()
        .oneshot(authed("/pick-file"))
        .await
        .unwrap();
    let status = resp.status();
    let body = body_string(resp).await;
    assert!(
        status == StatusCode::NOT_FOUND || body.contains("\"cancelled\":true"),
        "pick-file must never open a native dialog in server mode (status {status}, body {body})"
    );
}
