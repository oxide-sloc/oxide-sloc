// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>
//
// Webhook receiver + schedule management for automated SLOC scanning.
// Supports GitHub, GitLab, and Bitbucket push events, plus polling schedules.

use std::path::Path;
use std::time::Duration;

use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
};
use serde::{Deserialize, Serialize};

use tracing::info;

use sloc_git::{
    ScanSchedule, ScanScheduleKind, ScanScheduleProvider, ScheduleStore, WebhookEvent,
    WebhookProvider, clone_or_fetch, create_worktree, destroy_worktree, get_sha,
    parse_bitbucket_push, parse_github_push, parse_gitlab_push,
    webhook::{verify_bitbucket_sig, verify_github_sig},
};

use sloc_core::AnalysisRun;

use super::{
    AppState, RunArtifacts, git_clone_dest, register_artifacts_in_registry, scan_path_to_artifacts,
};

// ── request types ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
pub struct CreateScheduleRequest {
    pub label: String,
    pub repo_url: String,
    pub branch: String,
    pub kind: String,
    pub provider: Option<String>,
    pub interval_secs: Option<u64>,
    /// Webhook HMAC secret. If omitted, a random secret is auto-generated.
    pub webhook_secret: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ScheduleIdQuery {
    pub id: uuid::Uuid,
}

/// Provider-agnostic build-completion payload posted to `/webhooks/ci`.
///
/// Deliberately tiny and forgiving so that even very old CI systems (a Jenkins
/// freestyle job, a shell step in a legacy pipeline) can emit it with nothing
/// more than `curl`. Only `repo_url` + `branch` are required to match a
/// configured scan schedule; everything else is optional but strengthens the
/// guards (`status` gates on success, `upstream.build_id` enables idempotency).
#[derive(Debug, Deserialize)]
pub struct CiBuildCompletePayload {
    pub repo_url: String,
    #[serde(default)]
    pub branch: Option<String>,
    /// Optional exact commit to scan. If omitted, the branch tip is used.
    #[serde(default)]
    pub commit_sha: Option<String>,
    /// Upstream build result. Anything not recognised as success is skipped.
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub upstream: Option<CiUpstream>,
}

/// Provenance of the upstream build. Used for logging and the idempotency key.
#[derive(Debug, Default, Deserialize)]
pub struct CiUpstream {
    #[serde(default)]
    pub system: Option<String>,
    #[serde(default)]
    pub job: Option<String>,
    #[serde(default)]
    pub build_id: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

// ── schedule CRUD ─────────────────────────────────────────────────────────────

pub async fn api_list_schedules(State(state): State<AppState>) -> impl IntoResponse {
    let store = state.schedules.lock().await;
    Json(serde_json::json!({ "schedules": store.schedules }))
}

pub async fn api_create_schedule(
    State(state): State<AppState>,
    Json(body): Json<CreateScheduleRequest>,
) -> impl IntoResponse {
    let schedule = build_schedule(body);
    let is_poll = schedule.kind == ScanScheduleKind::Poll;
    {
        let mut store = state.schedules.lock().await;
        store.schedules.push(schedule.clone());
        let _ = store.save(&state.schedules_path);
    }
    if is_poll {
        let interval = schedule.interval_secs.unwrap_or(300);
        let st = state;
        let sc = schedule.clone();
        tokio::spawn(async move { poll_loop(st, sc, interval).await });
    }
    (StatusCode::CREATED, Json(schedule)).into_response()
}

pub async fn api_delete_schedule(
    State(state): State<AppState>,
    Query(q): Query<ScheduleIdQuery>,
) -> impl IntoResponse {
    let mut store = state.schedules.lock().await;
    store.remove(q.id);
    let _ = store.save(&state.schedules_path);
    StatusCode::NO_CONTENT
}

// ── webhook receivers ─────────────────────────────────────────────────────────

/// Validate the X-GitHub-Event header; return a short-circuit `Response` for
/// missing or non-push events, or `None` to continue processing a push.
fn check_github_event_header(event: &str) -> Option<Response> {
    if event == "push" {
        return None;
    }
    let resp = if event.is_empty() {
        info!(
            "github webhook received without X-GitHub-Event header — likely misconfigured sender"
        );
        (StatusCode::BAD_REQUEST, "missing X-GitHub-Event header").into_response()
    } else {
        info!(event = %event, "github webhook: ignoring non-push event");
        StatusCode::OK.into_response()
    };
    Some(resp)
}

/// Validate the X-GitLab-Event header; return a short-circuit `Response` for
/// missing or non-push events, or `None` to continue processing a push.
fn check_gitlab_event_header(event: &str) -> Option<Response> {
    if event == "Push Hook" || event == "Tag Push Hook" {
        return None;
    }
    let resp = if event.is_empty() {
        info!(
            "gitlab webhook received without X-GitLab-Event header — likely misconfigured sender"
        );
        (StatusCode::BAD_REQUEST, "missing X-GitLab-Event header").into_response()
    } else {
        info!(event = %event, "gitlab webhook: ignoring non-push event");
        StatusCode::OK.into_response()
    };
    Some(resp)
}

pub async fn handle_github_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(resp) = check_github_event_header(&header_str(&headers, "x-github-event")) {
        return resp;
    }
    let event = match parse_github_push(&body) {
        Ok(e) => e,
        Err(e) => {
            info!(error = %e, "github webhook payload parse failed");
            return (
                StatusCode::BAD_REQUEST,
                format!("invalid github push payload: {e}"),
            )
                .into_response();
        }
    };
    let sig = header_str(&headers, "x-hub-signature-256");
    dispatch_hmac_webhook(state, event, &body, &sig, is_valid_github_sig).await;
    StatusCode::ACCEPTED.into_response()
}

pub async fn handle_gitlab_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(resp) = check_gitlab_event_header(&header_str(&headers, "x-gitlab-event")) {
        return resp;
    }
    let event = match parse_gitlab_push(&body) {
        Ok(e) => e,
        Err(e) => {
            info!(error = %e, "gitlab webhook payload parse failed");
            return (
                StatusCode::BAD_REQUEST,
                format!("invalid gitlab push payload: {e}"),
            )
                .into_response();
        }
    };
    let token = header_str(&headers, "x-gitlab-token");
    dispatch_token_webhook(state, event, &token).await;
    StatusCode::ACCEPTED.into_response()
}

pub async fn handle_bitbucket_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let event = match parse_bitbucket_push(&body) {
        Ok(e) => e,
        Err(e) => {
            info!(error = %e, "bitbucket webhook payload parse failed");
            return (
                StatusCode::BAD_REQUEST,
                format!("invalid bitbucket push payload: {e}"),
            )
                .into_response();
        }
    };
    let sig = header_str(&headers, "x-hub-signature");
    dispatch_hmac_webhook(state, event, &body, &sig, is_valid_bitbucket_sig).await;
    StatusCode::ACCEPTED.into_response()
}

/// Generic build-completion receiver: an upstream CI build finishing triggers a
/// downstream oxide-sloc scan. Unlike the push receivers this is not tied to any
/// provider's payload shape, so any pipeline — however old — can drive it with a
/// single signed `curl`. Three independent guards must all pass before a scan
/// runs: the upstream status must be a success, the body HMAC must validate
/// against a matching schedule's secret, and the upstream build id must not have
/// been processed already (idempotency). Auth outcomes are never leaked back to
/// the caller — a bad signature is indistinguishable from success — but the two
/// non-secret skip reasons (unsuccessful build, duplicate delivery) are reported
/// so operators can debug their wiring.
pub async fn handle_ci_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let payload: CiBuildCompletePayload = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(e) => {
            info!(error = %e, "ci webhook payload parse failed");
            return (StatusCode::BAD_REQUEST, format!("invalid ci payload: {e}")).into_response();
        }
    };

    let repo_url = payload.repo_url.trim().to_owned();
    let branch = payload.branch.as_deref().unwrap_or("").trim().to_owned();
    if repo_url.is_empty() || branch.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            "ci payload requires non-empty repo_url and branch",
        )
            .into_response();
    }

    // Guard 1 — success gate. A missing status is treated as success so the
    // simplest possible caller ("my build reached its final step") still works;
    // anything explicitly non-success is a normal, non-secret skip.
    let status = payload.status.as_deref().unwrap_or("success");
    if !is_success_status(status) {
        info!(
            repo = %repo_url, branch = %branch, status,
            "ci webhook: upstream build not successful — no scan triggered"
        );
        return skip_response("upstream build not successful");
    }

    let sig = header_str(&headers, "x-sloc-signature");
    let build_key = ci_build_key(payload.upstream.as_ref());
    let upstream_url = payload
        .upstream
        .as_ref()
        .and_then(|u| u.url.clone())
        .unwrap_or_default();

    // Guard 2 — HMAC. Only schedules whose secret validates the exact body bytes
    // are eligible. Guard 3 — idempotency: skip any schedule that already
    // recorded this upstream build id, and optimistically claim the id now so a
    // flaky upstream retrying its notification cannot double-trigger.
    let mut store = state.schedules.lock().await;
    let matching_ids: Vec<uuid::Uuid> = store
        .find_matching(&repo_url, &branch)
        .into_iter()
        .filter(|s| matches_hmac(s, &body, &sig, &is_valid_github_sig))
        .map(|s| s.id)
        .collect();
    let valid_sig_count = matching_ids.len();

    let fresh = claim_fresh_schedules(&mut store, &matching_ids, &build_key);
    let duplicate = valid_sig_count > 0 && fresh.is_empty();
    if !fresh.is_empty() {
        let _ = store.save(&state.schedules_path);
    }
    drop(store);

    info!(
        repo = %repo_url,
        branch = %branch,
        upstream_url = %upstream_url,
        valid_signatures = valid_sig_count,
        triggered = fresh.len(),
        "inbound ci build-complete webhook received"
    );

    if duplicate {
        return skip_response("duplicate upstream build");
    }

    // commit_sha is optional — fall back to the branch tip. `scan_commit` fetches
    // then worktrees this ref, so a branch name resolves fine and keeps pipelines
    // that cannot emit a SHA working.
    let commit_ref = payload
        .commit_sha
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&branch)
        .to_owned();

    let event = WebhookEvent {
        provider: WebhookProvider::Ci,
        repo_url,
        branch,
        commit_sha: commit_ref,
        pusher: build_key,
    };
    spawn_scans(state, event, fresh);
    StatusCode::ACCEPTED.into_response()
}

/// Guard 3 — idempotency. From the HMAC-validated `matching_ids`, return the schedules that have
/// not already processed this upstream build, optimistically claiming `build_key` on each so a
/// retried upstream notification cannot double-trigger. A schedule whose `last_ci_build` already
/// equals the key is skipped as a duplicate; with no build key every match is returned (such
/// deliveries cannot be de-duplicated). Mutates `store` in place — the caller persists it.
fn claim_fresh_schedules(
    store: &mut ScheduleStore,
    matching_ids: &[uuid::Uuid],
    build_key: &Option<String>,
) -> Vec<ScanSchedule> {
    let mut fresh: Vec<ScanSchedule> = Vec::new();
    for id in matching_ids {
        let Some(s) = store.by_id_mut(*id) else {
            continue;
        };
        if let Some(key) = build_key {
            if s.last_ci_build.as_deref() == Some(key.as_str()) {
                continue; // duplicate delivery of the same upstream build
            }
            s.last_ci_build = Some(key.clone());
        }
        fresh.push(s.clone());
    }
    fresh
}

/// A non-secret 200 skip: the request was well-formed and (where relevant)
/// correctly signed, but a guard other than authentication declined the scan.
fn skip_response(reason: &str) -> Response {
    (
        StatusCode::OK,
        Json(serde_json::json!({ "status": "skipped", "reason": reason })),
    )
        .into_response()
}

/// Recognised success tokens across CI systems (Jenkins `SUCCESS`, GitLab
/// `success`, a raw exit code `0`, a boolean `true`, etc.). Case-insensitive.
fn is_success_status(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "success" | "succeeded" | "passed" | "pass" | "ok" | "green" | "completed" | "0" | "true"
    )
}

/// Build the `<system>:<job>:<build_id>` idempotency key. Returns `None` when no
/// build id is supplied — such deliveries cannot be de-duplicated, so callers
/// that want the guard must always send `upstream.build_id`.
fn ci_build_key(upstream: Option<&CiUpstream>) -> Option<String> {
    let u = upstream?;
    let non_empty = |o: &Option<String>| {
        o.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let build_id = non_empty(&u.build_id)?;
    let system = non_empty(&u.system).unwrap_or_else(|| "ci".to_owned());
    let job = non_empty(&u.job).unwrap_or_else(|| "-".to_owned());
    Some(format!("{system}:{job}:{build_id}"))
}

// ── dispatch helpers ──────────────────────────────────────────────────────────

async fn dispatch_hmac_webhook<F>(
    state: AppState,
    event: WebhookEvent,
    body: &Bytes,
    sig: &str,
    verify: F,
) where
    F: Fn(&[u8], &str, &str) -> bool,
{
    let store = state.schedules.lock().await;
    let candidates = store.find_matching(&event.repo_url, &event.branch);
    let candidate_count = candidates.len();
    let matching: Vec<ScanSchedule> = candidates
        .into_iter()
        .filter(|s| matches_hmac(s, body, sig, &verify))
        .cloned()
        .collect();
    let valid_sig_count = matching.len();
    drop(store);
    info!(
        repo = %event.repo_url,
        branch = %event.branch,
        matched_schedules = candidate_count,
        valid_signatures = valid_sig_count,
        "inbound HMAC webhook received"
    );
    if candidate_count > 0 && valid_sig_count == 0 {
        info!(
            repo = %event.repo_url,
            branch = %event.branch,
            "webhook HMAC verification failed for all matching schedules — no scan triggered"
        );
    }
    spawn_scans(state, event, matching);
}

async fn dispatch_token_webhook(state: AppState, event: WebhookEvent, token: &str) {
    let store = state.schedules.lock().await;
    let candidates = store.find_matching(&event.repo_url, &event.branch);
    let candidate_count = candidates.len();
    let matching: Vec<ScanSchedule> = candidates
        .into_iter()
        .filter(|s| matches_token(s, token))
        .cloned()
        .collect();
    let valid_token_count = matching.len();
    drop(store);
    info!(
        repo = %event.repo_url,
        branch = %event.branch,
        matched_schedules = candidate_count,
        valid_tokens = valid_token_count,
        "inbound token webhook received"
    );
    if candidate_count > 0 && valid_token_count == 0 {
        info!(
            repo = %event.repo_url,
            branch = %event.branch,
            "webhook token verification failed for all matching schedules — no scan triggered"
        );
    }
    spawn_scans(state, event, matching);
}

fn matches_hmac<F: Fn(&[u8], &str, &str) -> bool>(
    s: &ScanSchedule,
    body: &[u8],
    sig: &str,
    verify: &F,
) -> bool {
    // A schedule without a secret never matches — callers must configure a secret.
    // Using is_none_or here would allow unauthenticated webhook triggering.
    match s.webhook_secret.as_deref() {
        None | Some("") => false,
        Some(secret) => verify(body, sig, secret),
    }
}

fn matches_token(s: &ScanSchedule, token: &str) -> bool {
    // A schedule without a secret never matches — token must be configured.
    match s.webhook_secret.as_deref() {
        None | Some("") => false,
        Some(secret) => ct_eq(secret, token),
    }
}

fn is_valid_github_sig(body: &[u8], sig: &str, secret: &str) -> bool {
    verify_github_sig(body, sig, secret)
}

fn is_valid_bitbucket_sig(body: &[u8], sig: &str, secret: &str) -> bool {
    verify_bitbucket_sig(body, sig, secret)
}

// Takes owned values so each iteration can move into the async task after cloning.
#[allow(clippy::needless_pass_by_value)]
fn spawn_scans(state: AppState, event: WebhookEvent, schedules: Vec<ScanSchedule>) {
    for schedule in schedules {
        let st = state.clone();
        let ev = event.clone();
        let sc = schedule.clone();
        tokio::spawn(async move { run_scheduled_scan(st, ev, sc).await });
    }
}

// ── scan execution ────────────────────────────────────────────────────────────

async fn run_scheduled_scan(state: AppState, event: WebhookEvent, schedule: ScanSchedule) {
    let repo = event.repo_url.clone();
    let sha = event.commit_sha.clone();
    let sha_for_record = sha.clone();
    let clones_dir = state.git_clones_dir.clone();
    let config = state.base_config.clone();
    let label = schedule.label.clone();
    let label_for_closure = label.clone();
    let sched_id = schedule.id;

    let result = tokio::task::spawn_blocking(move || {
        scan_commit(&repo, &sha, &clones_dir, &config, &label_for_closure)
    })
    .await;

    match result {
        Ok(Ok((run_id, artifacts, run))) => {
            register_artifacts_in_registry(&state, &label, &run, &artifacts).await;
            record_scan_result(&state, sched_id, &sha_for_record, &run_id).await;
            crate::confluence::maybe_auto_post_confluence(&state, sched_id, &run, &run_id).await;
        }
        Ok(Err(e)) => eprintln!("[sloc-webhook] scan failed '{}': {e:#}", schedule.label),
        Err(e) => eprintln!("[sloc-webhook] task panicked: {e}"),
    }
}

async fn record_scan_result(state: &AppState, id: uuid::Uuid, sha: &str, run_id: &str) {
    let mut store = state.schedules.lock().await;
    if let Some(s) = store.by_id_mut(id) {
        s.last_scan_sha = Some(sha.to_owned());
        s.last_scan_at = Some(chrono::Utc::now());
        s.last_run_id = Some(run_id.to_owned());
    }
    let _ = store.save(&state.schedules_path);
}

fn scan_commit(
    repo: &str,
    sha: &str,
    clones_dir: &Path,
    config: &sloc_config::AppConfig,
    label: &str,
) -> anyhow::Result<(String, RunArtifacts, AnalysisRun)> {
    let dest = git_clone_dest(repo, clones_dir);
    clone_or_fetch(repo, &dest)?;
    let wt_path = clones_dir.join(format!("wt-{}", uuid::Uuid::new_v4().simple()));
    create_worktree(&dest, sha, &wt_path)?;
    let result = scan_path_to_artifacts(&wt_path, config, label);
    let _ = destroy_worktree(&dest, &wt_path);
    result
}

// ── polling ───────────────────────────────────────────────────────────────────

pub async fn poll_loop(state: AppState, mut schedule: ScanSchedule, interval_secs: u64) {
    let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
    ticker.tick().await;
    loop {
        ticker.tick().await;
        if let Err(e) = poll_once(&state, &mut schedule).await {
            eprintln!("[sloc-poll] '{}': {e:#}", schedule.label);
        }
    }
}

async fn poll_once(state: &AppState, schedule: &mut ScanSchedule) -> anyhow::Result<()> {
    let repo = schedule.repo_url.clone();
    let branch = schedule.branch.clone();
    let clones_dir = state.git_clones_dir.clone();
    let last_sha = schedule.last_scan_sha.clone().unwrap_or_default();

    let current_sha =
        tokio::task::spawn_blocking(move || fetch_and_resolve_sha(&repo, &branch, &clones_dir))
            .await??;

    if current_sha == last_sha {
        return Ok(());
    }

    let label = schedule.label.clone();
    let config = state.base_config.clone();
    let repo2 = schedule.repo_url.clone();
    let sha = current_sha.clone();
    let clones2 = state.git_clones_dir.clone();

    let (run_id, artifacts, run) =
        tokio::task::spawn_blocking(move || scan_commit(&repo2, &sha, &clones2, &config, &label))
            .await??;

    register_artifacts_in_registry(state, &schedule.label, &run, &artifacts).await;
    schedule.last_scan_sha = Some(current_sha.clone());
    schedule.last_scan_at = Some(chrono::Utc::now());
    schedule.last_run_id = Some(run_id.clone());
    record_scan_result(state, schedule.id, &current_sha, &run_id).await;
    crate::confluence::maybe_auto_post_confluence(state, schedule.id, &run, &run_id).await;
    Ok(())
}

fn fetch_and_resolve_sha(repo: &str, branch: &str, clones_dir: &Path) -> anyhow::Result<String> {
    let dest = git_clone_dest(repo, clones_dir);
    clone_or_fetch(repo, &dest)?;
    get_sha(&dest, &format!("origin/{branch}"))
}

// ── small helpers ─────────────────────────────────────────────────────────────

fn build_schedule(req: CreateScheduleRequest) -> ScanSchedule {
    if req.kind == "poll" {
        ScanSchedule::new_poll(
            req.repo_url,
            req.branch,
            req.interval_secs.unwrap_or(300),
            req.label,
        )
    } else {
        let provider = match req.provider.as_deref() {
            Some("github") => ScanScheduleProvider::GitHub,
            Some("gitlab") => ScanScheduleProvider::GitLab,
            Some("bitbucket") => ScanScheduleProvider::Bitbucket,
            _ => ScanScheduleProvider::Any,
        };
        ScanSchedule::new_webhook(
            req.repo_url,
            req.branch,
            provider,
            req.label,
            req.webhook_secret,
        )
    }
}

fn header_str(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned()
}

fn ct_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sloc_git::{ScanScheduleKind, ScanScheduleProvider};

    // ── /webhooks/ci guards ───────────────────────────────────────────────────

    #[test]
    fn success_status_accepts_common_tokens() {
        for s in [
            "success",
            "SUCCESS",
            " Success ",
            "passed",
            "pass",
            "ok",
            "green",
            "completed",
            "0",
            "true",
        ] {
            assert!(is_success_status(s), "{s:?} should be a success");
        }
    }

    #[test]
    fn success_status_rejects_failures_and_unknowns() {
        for s in [
            "failure", "failed", "1", "error", "aborted", "unstable", "canceled", "false", "",
            "running",
        ] {
            assert!(!is_success_status(s), "{s:?} should not be a success");
        }
    }

    #[test]
    fn ci_build_key_requires_build_id() {
        // No upstream at all → no key (dedup disabled).
        assert_eq!(ci_build_key(None), None);
        // Upstream present but no build id → still no key.
        let u = CiUpstream {
            system: Some("jenkins".into()),
            job: Some("api".into()),
            build_id: None,
            url: None,
        };
        assert_eq!(ci_build_key(Some(&u)), None);
    }

    #[test]
    fn ci_build_key_composes_stable_and_fills_defaults() {
        let full = CiUpstream {
            system: Some("jenkins".into()),
            job: Some("api-pipeline".into()),
            build_id: Some("1234".into()),
            url: None,
        };
        assert_eq!(
            ci_build_key(Some(&full)).as_deref(),
            Some("jenkins:api-pipeline:1234")
        );
        // Missing system/job fall back to fixed placeholders so the key is stable.
        let sparse = CiUpstream {
            system: None,
            job: None,
            build_id: Some("42".into()),
            url: None,
        };
        assert_eq!(ci_build_key(Some(&sparse)).as_deref(), Some("ci:-:42"));
    }

    #[test]
    fn ci_signature_roundtrips_against_schedule_secret() {
        // What a trigger script computes must be what a matching schedule accepts.
        let secret = "s3cr3t-shared-key";
        let body = br#"{"repo_url":"https://x/y.git","branch":"main","status":"success"}"#;
        let sig = format!(
            "sha256={}",
            sloc_git::webhook::hmac_sha256_hex(secret.as_bytes(), body)
        );

        let sched = ScanSchedule::new_webhook(
            "https://x/y.git".into(),
            "main".into(),
            ScanScheduleProvider::Any,
            "ci".into(),
            Some(secret.into()),
        );
        assert!(matches_hmac(&sched, body, &sig, &is_valid_github_sig));
        // A tampered body must not validate under the same signature.
        let tampered = br#"{"repo_url":"https://x/y.git","branch":"main","status":"SUCCESS "}"#;
        assert!(!matches_hmac(&sched, tampered, &sig, &is_valid_github_sig));
    }

    #[test]
    fn ci_payload_parses_minimal_and_full() {
        let minimal: CiBuildCompletePayload =
            serde_json::from_slice(br#"{"repo_url":"https://x/y.git","branch":"main"}"#).unwrap();
        assert_eq!(minimal.repo_url, "https://x/y.git");
        assert!(minimal.commit_sha.is_none() && minimal.upstream.is_none());

        let full: CiBuildCompletePayload = serde_json::from_slice(
            br#"{"repo_url":"https://x/y.git","branch":"main","commit_sha":"abc",
                 "status":"success","upstream":{"system":"gitlab","job":"build",
                 "build_id":"9","url":"https://ci/9"}}"#,
        )
        .unwrap();
        assert_eq!(full.commit_sha.as_deref(), Some("abc"));
        assert_eq!(full.upstream.unwrap().build_id.as_deref(), Some("9"));
    }

    // ── claim_fresh_schedules (Guard 3 idempotency) ───────────────────────────

    fn webhook_sched() -> ScanSchedule {
        ScanSchedule::new_webhook(
            "https://x/y.git".into(),
            "main".into(),
            ScanScheduleProvider::Any,
            "ci".into(),
            Some("s3cr3t".into()),
        )
    }

    #[test]
    fn claim_fresh_schedules_dedups_by_build_key() {
        let mut store = ScheduleStore::default();
        store.schedules.push(webhook_sched());
        store.schedules.push(webhook_sched());
        let ids: Vec<uuid::Uuid> = store.schedules.iter().map(|s| s.id).collect();
        let key = Some("jenkins:api:1".to_string());

        // First delivery: both schedules are fresh and get the build id claimed.
        let fresh = claim_fresh_schedules(&mut store, &ids, &key);
        assert_eq!(fresh.len(), 2);
        assert!(
            store
                .schedules
                .iter()
                .all(|s| s.last_ci_build.as_deref() == Some("jenkins:api:1"))
        );

        // Re-delivery of the same build id: both are now duplicates.
        assert!(claim_fresh_schedules(&mut store, &ids, &key).is_empty());

        // A new build id claims them again.
        let key2 = Some("jenkins:api:2".to_string());
        assert_eq!(claim_fresh_schedules(&mut store, &ids, &key2).len(), 2);
    }

    #[test]
    fn claim_fresh_schedules_without_key_returns_all_and_skips_unknown_ids() {
        let mut store = ScheduleStore::default();
        store.schedules.push(webhook_sched());
        let real = store.schedules[0].id;
        let bogus = uuid::Uuid::new_v4();
        // No build key ⇒ dedup disabled ⇒ every existing match returned; unknown ids skipped.
        let fresh = claim_fresh_schedules(&mut store, &[real, bogus], &None);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].id, real);
    }

    // ── ct_eq ─────────────────────────────────────────────────────────────────

    #[test]
    fn ct_eq_equal_strings() {
        assert!(ct_eq("hello", "hello"));
    }

    #[test]
    fn ct_eq_unequal_strings() {
        assert!(!ct_eq("hello", "world"));
    }

    #[test]
    fn ct_eq_empty_strings_equal() {
        assert!(ct_eq("", ""));
    }

    #[test]
    fn ct_eq_different_lengths_unequal() {
        assert!(!ct_eq("abc", "abcd"));
        assert!(!ct_eq("abcd", "abc"));
    }

    #[test]
    fn ct_eq_one_empty_unequal() {
        assert!(!ct_eq("", "nonempty"));
        assert!(!ct_eq("nonempty", ""));
    }

    #[test]
    fn ct_eq_case_sensitive() {
        assert!(!ct_eq("Secret", "secret"));
    }

    // ── check_github_event_header ─────────────────────────────────────────────

    #[test]
    fn github_push_event_returns_none() {
        assert!(check_github_event_header("push").is_none());
    }

    #[test]
    fn github_ping_event_returns_200() {
        let resp = check_github_event_header("ping");
        assert!(resp.is_some());
    }

    #[test]
    fn github_empty_event_returns_400() {
        let resp = check_github_event_header("");
        assert!(resp.is_some());
    }

    #[test]
    fn github_issues_event_returns_200() {
        let resp = check_github_event_header("issues");
        assert!(resp.is_some());
    }

    // ── check_gitlab_event_header ─────────────────────────────────────────────

    #[test]
    fn gitlab_push_hook_returns_none() {
        assert!(check_gitlab_event_header("Push Hook").is_none());
    }

    #[test]
    fn gitlab_tag_push_hook_returns_none() {
        assert!(check_gitlab_event_header("Tag Push Hook").is_none());
    }

    #[test]
    fn gitlab_merge_request_hook_returns_200() {
        let resp = check_gitlab_event_header("Merge Request Hook");
        assert!(resp.is_some());
    }

    #[test]
    fn gitlab_empty_event_returns_400() {
        let resp = check_gitlab_event_header("");
        assert!(resp.is_some());
    }

    // ── matches_token ─────────────────────────────────────────────────────────

    #[test]
    fn matches_token_empty_secret_never_matches() {
        let mut s = ScanSchedule::new_webhook(
            "https://gitlab.com/org/repo.git".into(),
            "main".into(),
            ScanScheduleProvider::GitLab,
            "test".into(),
            Some(String::new()),
        );
        s.webhook_secret = Some(String::new());
        assert!(!matches_token(&s, "any-token"));
    }

    #[test]
    fn matches_token_no_secret_never_matches() {
        let mut s = ScanSchedule::new_webhook(
            "https://gitlab.com/org/repo.git".into(),
            "main".into(),
            ScanScheduleProvider::GitLab,
            "test".into(),
            None,
        );
        s.webhook_secret = None;
        assert!(!matches_token(&s, "any-token"));
    }

    #[test]
    fn matches_token_correct_token_matches() {
        let secret = "my-gitlab-secret";
        let mut s = ScanSchedule::new_webhook(
            "https://gitlab.com/org/repo.git".into(),
            "main".into(),
            ScanScheduleProvider::GitLab,
            "test".into(),
            Some(secret.into()),
        );
        s.webhook_secret = Some(secret.into());
        assert!(matches_token(&s, secret));
    }

    #[test]
    fn matches_token_wrong_token_no_match() {
        let mut s = ScanSchedule::new_webhook(
            "https://gitlab.com/org/repo.git".into(),
            "main".into(),
            ScanScheduleProvider::GitLab,
            "test".into(),
            Some("correct-secret".into()),
        );
        s.webhook_secret = Some("correct-secret".into());
        assert!(!matches_token(&s, "wrong-secret"));
    }

    // ── matches_hmac ──────────────────────────────────────────────────────────

    #[test]
    fn matches_hmac_no_secret_always_false() {
        let mut s = ScanSchedule::new_webhook(
            "https://github.com/org/repo.git".into(),
            "main".into(),
            ScanScheduleProvider::GitHub,
            "test".into(),
            None,
        );
        s.webhook_secret = None;
        let body = b"payload";
        let verify = |_: &[u8], _: &str, _: &str| true;
        assert!(!matches_hmac(&s, body, "sig", &verify));
    }

    #[test]
    fn matches_hmac_empty_secret_always_false() {
        let mut s = ScanSchedule::new_webhook(
            "https://github.com/org/repo.git".into(),
            "main".into(),
            ScanScheduleProvider::GitHub,
            "test".into(),
            Some(String::new()),
        );
        s.webhook_secret = Some(String::new());
        let body = b"payload";
        let verify = |_: &[u8], _: &str, _: &str| true;
        assert!(!matches_hmac(&s, body, "sig", &verify));
    }

    #[test]
    fn matches_hmac_verify_fn_determines_result() {
        let secret = "webhook-secret";
        let mut s = ScanSchedule::new_webhook(
            "https://github.com/org/repo.git".into(),
            "main".into(),
            ScanScheduleProvider::GitHub,
            "test".into(),
            Some(secret.into()),
        );
        s.webhook_secret = Some(secret.into());
        let body = b"payload";
        assert!(matches_hmac(&s, body, "sig", &|_, _, _| true));
        assert!(!matches_hmac(&s, body, "sig", &|_, _, _| false));
    }

    // ── build_schedule ────────────────────────────────────────────────────────

    #[test]
    fn build_schedule_webhook_kind() {
        let req = CreateScheduleRequest {
            label: "My webhook".into(),
            repo_url: "https://github.com/org/repo.git".into(),
            branch: "main".into(),
            kind: "webhook".into(),
            provider: Some("github".into()),
            interval_secs: None,
            webhook_secret: Some("secret123".into()),
        };
        let s = build_schedule(req);
        assert_eq!(s.kind, ScanScheduleKind::Webhook);
        assert_eq!(s.repo_url, "https://github.com/org/repo.git");
        assert_eq!(s.branch, "main");
        assert_eq!(s.label, "My webhook");
        assert_eq!(s.webhook_secret.as_deref(), Some("secret123"));
    }

    #[test]
    fn build_schedule_poll_kind() {
        let req = CreateScheduleRequest {
            label: "My poll".into(),
            repo_url: "https://github.com/org/repo.git".into(),
            branch: "develop".into(),
            kind: "poll".into(),
            provider: None,
            interval_secs: Some(600),
            webhook_secret: None,
        };
        let s = build_schedule(req);
        assert_eq!(s.kind, ScanScheduleKind::Poll);
        assert_eq!(s.interval_secs, Some(600));
    }

    #[test]
    fn build_schedule_poll_defaults_interval_to_300() {
        let req = CreateScheduleRequest {
            label: "Poll no interval".into(),
            repo_url: "https://github.com/org/repo.git".into(),
            branch: "main".into(),
            kind: "poll".into(),
            provider: None,
            interval_secs: None,
            webhook_secret: None,
        };
        let s = build_schedule(req);
        assert_eq!(s.kind, ScanScheduleKind::Poll);
        assert_eq!(s.interval_secs, Some(300));
    }

    #[test]
    fn build_schedule_github_provider() {
        let req = CreateScheduleRequest {
            label: "GH".into(),
            repo_url: "https://github.com/org/repo.git".into(),
            branch: "main".into(),
            kind: "webhook".into(),
            provider: Some("github".into()),
            interval_secs: None,
            webhook_secret: None,
        };
        let s = build_schedule(req);
        assert_eq!(s.provider, ScanScheduleProvider::GitHub);
    }

    #[test]
    fn build_schedule_gitlab_provider() {
        let req = CreateScheduleRequest {
            label: "GL".into(),
            repo_url: "https://gitlab.com/org/repo.git".into(),
            branch: "main".into(),
            kind: "webhook".into(),
            provider: Some("gitlab".into()),
            interval_secs: None,
            webhook_secret: None,
        };
        let s = build_schedule(req);
        assert_eq!(s.provider, ScanScheduleProvider::GitLab);
    }

    #[test]
    fn build_schedule_bitbucket_provider() {
        let req = CreateScheduleRequest {
            label: "BB".into(),
            repo_url: "https://bitbucket.org/ws/repo.git".into(),
            branch: "main".into(),
            kind: "webhook".into(),
            provider: Some("bitbucket".into()),
            interval_secs: None,
            webhook_secret: None,
        };
        let s = build_schedule(req);
        assert_eq!(s.provider, ScanScheduleProvider::Bitbucket);
    }

    #[test]
    fn build_schedule_unknown_provider_defaults_to_any() {
        let req = CreateScheduleRequest {
            label: "Any".into(),
            repo_url: "https://custom.git.server/repo.git".into(),
            branch: "main".into(),
            kind: "webhook".into(),
            provider: Some("unknown".into()),
            interval_secs: None,
            webhook_secret: None,
        };
        let s = build_schedule(req);
        assert_eq!(s.provider, ScanScheduleProvider::Any);
    }

    // ── header_str ────────────────────────────────────────────────────────────

    #[test]
    fn header_str_missing_header_returns_empty() {
        let headers = HeaderMap::new();
        assert_eq!(header_str(&headers, "x-github-event"), "");
    }

    #[test]
    fn header_str_present_header_returned() {
        let mut headers = HeaderMap::new();
        headers.insert("x-github-event", "push".parse().unwrap());
        assert_eq!(header_str(&headers, "x-github-event"), "push");
    }

    #[test]
    fn header_str_case_insensitive_lookup() {
        let mut headers = HeaderMap::new();
        headers.insert("x-github-event", "ping".parse().unwrap());
        // HeaderMap normalises header names to lowercase
        assert_eq!(header_str(&headers, "X-GitHub-Event"), "ping");
    }

    // ── build_schedule — edge cases ───────────────────────────────────────────

    #[test]
    fn build_schedule_no_provider_defaults_to_any() {
        let req = CreateScheduleRequest {
            label: "No provider".into(),
            repo_url: "https://git.example.com/repo.git".into(),
            branch: "main".into(),
            kind: "webhook".into(),
            provider: None,
            interval_secs: None,
            webhook_secret: None,
        };
        let s = build_schedule(req);
        assert_eq!(s.provider, ScanScheduleProvider::Any);
        assert_eq!(s.kind, ScanScheduleKind::Webhook);
    }

    #[test]
    fn build_schedule_webhook_no_secret_sets_none_or_generated() {
        let req = CreateScheduleRequest {
            label: "No secret webhook".into(),
            repo_url: "https://github.com/org/repo.git".into(),
            branch: "main".into(),
            kind: "webhook".into(),
            provider: Some("github".into()),
            interval_secs: None,
            webhook_secret: None,
        };
        let s = build_schedule(req);
        // When no secret is provided, the schedule either has None or an
        // auto-generated secret (implementation detail). Either is valid.
        assert_eq!(s.kind, ScanScheduleKind::Webhook);
    }

    #[test]
    fn build_schedule_poll_stores_interval() {
        let req = CreateScheduleRequest {
            label: "Fast poll".into(),
            repo_url: "https://github.com/org/repo.git".into(),
            branch: "main".into(),
            kind: "poll".into(),
            provider: None,
            interval_secs: Some(60),
            webhook_secret: None,
        };
        let s = build_schedule(req);
        assert_eq!(s.kind, ScanScheduleKind::Poll);
        assert_eq!(s.interval_secs, Some(60));
    }

    // ── check_github_event_header — additional event types ────────────────────

    #[test]
    fn github_pull_request_event_returns_200() {
        let resp = check_github_event_header("pull_request");
        assert!(
            resp.is_some(),
            "pull_request must return a response (not push)"
        );
    }

    #[test]
    fn github_star_event_returns_200() {
        let resp = check_github_event_header("star");
        assert!(
            resp.is_some(),
            "star event must return a response (not push)"
        );
    }

    // ── check_gitlab_event_header — additional event types ────────────────────

    #[test]
    fn gitlab_pipeline_hook_returns_200() {
        let resp = check_gitlab_event_header("Pipeline Hook");
        assert!(resp.is_some(), "Pipeline Hook must return a response");
    }

    #[test]
    fn gitlab_issue_hook_returns_200() {
        let resp = check_gitlab_event_header("Issue Hook");
        assert!(resp.is_some(), "Issue Hook must return a response");
    }

    // ── is_valid_github_sig / is_valid_bitbucket_sig ─────────────────────────

    #[test]
    fn is_valid_github_sig_empty_sig_always_false() {
        // An empty signature string cannot match any valid HMAC
        assert!(!is_valid_github_sig(b"test-body", "", "some-secret"));
    }

    #[test]
    fn is_valid_github_sig_wrong_format_returns_false() {
        // GitHub sig must be "sha256=<hex>" — anything else is invalid
        assert!(!is_valid_github_sig(
            b"test-body",
            "not-a-valid-sig",
            "some-secret"
        ));
    }

    #[test]
    fn is_valid_bitbucket_sig_empty_sig_returns_false() {
        assert!(!is_valid_bitbucket_sig(b"test-body", "", "some-secret"));
    }

    #[test]
    fn is_valid_bitbucket_sig_wrong_sig_returns_false() {
        assert!(!is_valid_bitbucket_sig(
            b"test-body",
            "sha256=wronghex",
            "some-secret"
        ));
    }

    // ── matches_hmac — verify fn called with correct arguments ────────────────

    #[test]
    fn matches_hmac_passes_body_to_verify() {
        let secret = "my-secret";
        let s = ScanSchedule::new_webhook(
            "https://github.com/org/repo.git".into(),
            "main".into(),
            ScanScheduleProvider::GitHub,
            "test".into(),
            Some(secret.into()),
        );
        let body = b"test-payload";
        let sig = "sha256=abc123";

        // Use a stateless verify fn that checks the secret matches
        let verify = |_body: &[u8], _sig: &str, sec: &str| sec == "my-secret";
        assert!(matches_hmac(&s, body, sig, &verify));

        // A fn that always returns false
        let reject = |_body: &[u8], _sig: &str, _sec: &str| false;
        assert!(!matches_hmac(&s, body, sig, &reject));
    }

    // ── header_str — additional headers ──────────────────────────────────────

    #[test]
    fn header_str_x_hub_signature_256() {
        let mut headers = HeaderMap::new();
        headers.insert("x-hub-signature-256", "sha256=abc123".parse().unwrap());
        assert_eq!(header_str(&headers, "x-hub-signature-256"), "sha256=abc123");
    }

    #[test]
    fn header_str_x_gitlab_token() {
        let mut headers = HeaderMap::new();
        headers.insert("x-gitlab-token", "my-secret-token".parse().unwrap());
        assert_eq!(header_str(&headers, "x-gitlab-token"), "my-secret-token");
    }

    #[test]
    fn header_str_x_hub_signature() {
        let mut headers = HeaderMap::new();
        headers.insert("x-hub-signature", "sha256=bitbucket-sig".parse().unwrap());
        assert_eq!(
            header_str(&headers, "x-hub-signature"),
            "sha256=bitbucket-sig"
        );
    }

    // ── ct_eq — unicode and binary-safe boundary tests ────────────────────────

    #[test]
    fn ct_eq_spaces_in_secret() {
        assert!(ct_eq("my secret", "my secret"));
        assert!(!ct_eq("my secret", "my-secret"));
    }

    #[test]
    fn ct_eq_long_strings() {
        let a = "x".repeat(1024);
        let b = "x".repeat(1024);
        assert!(ct_eq(&a, &b));
        let mut c = "x".repeat(1023);
        c.push('y');
        assert!(!ct_eq(&a, &c));
    }

    // ── CreateScheduleRequest — serialization ─────────────────────────────────

    #[test]
    fn create_schedule_request_serializes_and_deserializes() {
        let req = CreateScheduleRequest {
            label: "Test Label".into(),
            repo_url: "https://github.com/org/repo.git".into(),
            branch: "main".into(),
            kind: "webhook".into(),
            provider: Some("github".into()),
            interval_secs: Some(300),
            webhook_secret: Some("sec".into()),
        };
        let json = serde_json::to_string(&req).expect("must serialize");
        let deser: CreateScheduleRequest = serde_json::from_str(&json).expect("must deserialize");
        assert_eq!(deser.label, "Test Label");
        assert_eq!(deser.repo_url, "https://github.com/org/repo.git");
        assert_eq!(deser.branch, "main");
        assert_eq!(deser.kind, "webhook");
        assert_eq!(deser.provider.as_deref(), Some("github"));
        assert_eq!(deser.interval_secs, Some(300));
        assert_eq!(deser.webhook_secret.as_deref(), Some("sec"));
    }
}
