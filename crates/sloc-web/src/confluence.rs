// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>
//
// Atlassian Confluence integration — Cloud (API v2) and Server/DC (API v1).

use std::collections::HashMap;
use std::path::Path;

use axum::{
    extract::{Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Json, Response},
};
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use sloc_core::read_json;
use sloc_report::{render_confluence_storage, render_confluence_wiki_markup};

use super::{AppState, recover_artifacts_from_registry};

// ── URL validation ────────────────────────────────────────────────────────────

/// Validate a Confluence base URL to prevent SSRF.
/// Only https:// and http:// are accepted; private/reserved IP ranges and
/// cloud metadata endpoints are blocked regardless of scheme.
fn validate_confluence_url(url: &str) -> Result<(), String> {
    let url = url.trim();
    if url.is_empty() {
        return Ok(()); // Empty URL: no connection will be made, save is a clear/reset.
    }
    let lower = url.to_lowercase();
    if !lower.starts_with("https://") && !lower.starts_with("http://") {
        return Err(format!(
            "Confluence URL must start with https:// or http:// (got {url:?})"
        ));
    }
    // Extract the host portion (between "://" and the first "/" or end of string).
    let after_scheme = lower
        .find("://")
        .map_or(lower.as_str(), |i| &lower[i + 3..]);
    // Strip credentials (user:pass@host) if present.
    let after_creds = after_scheme
        .find('@')
        .map_or(after_scheme, |i| &after_scheme[i + 1..]);
    // Strip path/query, leaving host[:port] (or "[ipv6]:port").
    let authority = after_creds.split('/').next().unwrap_or(after_creds);
    // Strip the port. For bracketed IPv6 literals ("[::1]:8090") keep the bracketed
    // host intact rather than splitting on the IPv6 colons.
    let host = authority.strip_prefix('[').map_or_else(
        || authority.split(':').next().unwrap_or(authority),
        |stripped| stripped.split(']').next().unwrap_or(stripped),
    );

    if is_blocked_confluence_host(host) {
        return Err(format!(
            "Confluence URL host {host:?} resolves to a private or reserved address; \
             SSRF protection prevents connecting to it"
        ));
    }
    Ok(())
}

/// Cloud-metadata / instance-data hostnames that must never be reachable.
const BLOCKED_METADATA_HOSTNAMES: &[&str] = &[
    "metadata.google.internal",
    "metadata.internal",
    "instance-data",
];

/// Returns true when `host` is an SSRF-sensitive loopback, link-local,
/// unspecified, multicast, or cloud-metadata target. RFC 1918 / IPv6 unique-local
/// private ranges are intentionally NOT blocked, so self-hosted Confluence
/// Server/DC on a corporate network remains reachable. IP literals are parsed
/// (not prefix-matched) so hostnames like `fd-corp.com` are never false-blocked.
fn is_blocked_confluence_host(host: &str) -> bool {
    let h = host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_lowercase();
    if h == "localhost" || BLOCKED_METADATA_HOSTNAMES.contains(&h.as_str()) {
        return true;
    }
    match h.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => {
            v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.octets() == [100, 100, 100, 200] // Alibaba Cloud metadata
        }
        Ok(std::net::IpAddr::V6(v6)) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
        }
        Err(_) => false,
    }
}

// ── config structs ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConfluenceTier {
    /// Atlassian Cloud — uses REST API v2 at /wiki/api/v2/
    #[default]
    Cloud,
    /// Self-hosted Confluence Server or Data Center — uses REST API v1 at /rest/api/
    Server,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConfluenceConfig {
    pub tier: ConfluenceTier,
    /// Base URL, e.g. "<https://mycompany.atlassian.net>" or "<https://confluence.corp.com>"
    pub base_url: String,
    /// Cloud: Atlassian account email. Server: username (blank if using a PAT).
    pub username: String,
    /// Cloud: API token. Server: password or Personal Access Token.
    #[serde(skip)]
    pub credential: String,
    pub space_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_page_id: Option<String>,
    /// per-schedule auto-post override; key = schedule UUID string, value = enabled
    #[serde(default)]
    pub schedule_auto_post: HashMap<String, bool>,
}

impl ConfluenceConfig {
    fn is_cloud_url(&self) -> bool {
        self.base_url.to_lowercase().contains(".atlassian.net")
    }

    fn effective_tier(&self) -> &ConfluenceTier {
        if self.tier == ConfluenceTier::Cloud || self.is_cloud_url() {
            &ConfluenceTier::Cloud
        } else {
            &ConfluenceTier::Server
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConfluenceConfigStore {
    pub config: Option<ConfluenceConfig>,
}

impl ConfluenceConfigStore {
    pub fn load(path: &Path) -> Self {
        let mut store: Self = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if let Some(ref mut cfg) = store.config
            && let Ok(token) = std::env::var("SLOC_CONFLUENCE_TOKEN")
        {
            cfg.credential = token;
        }
        store
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn is_configured(&self) -> bool {
        self.config
            .as_ref()
            .is_some_and(|c| !c.base_url.is_empty() && !c.credential.is_empty())
    }
}

// ── Confluence API client ─────────────────────────────────────────────────────

pub struct ConfluenceClient {
    client: reqwest::Client,
    base_url: String,
    auth_header: String,
    tier: ConfluenceTier,
    space_key: String,
    parent_page_id: Option<String>,
}

struct PageSummary {
    id: String,
    version_number: u32,
}

impl ConfluenceClient {
    pub fn new(config: &ConfluenceConfig) -> Self {
        let auth_header = if config.username.is_empty() {
            // Server PAT — Bearer token
            format!("Bearer {}", config.credential)
        } else {
            let raw = format!("{}:{}", config.username, config.credential);
            let encoded = base64::engine::general_purpose::STANDARD.encode(raw.as_bytes());
            format!("Basic {encoded}")
        };

        Self {
            client: reqwest::Client::new(),
            base_url: config.base_url.trim_end_matches('/').to_owned(),
            auth_header,
            tier: config.effective_tier().clone(),
            space_key: config.space_key.clone(),
            parent_page_id: config.parent_page_id.clone(),
        }
    }

    async fn find_space_id(&self) -> anyhow::Result<String> {
        let url = format!(
            "{}/wiki/api/v2/spaces?keys={}&limit=1",
            self.base_url, self.space_key
        );
        let resp: serde_json::Value = self
            .client
            .get(&url)
            .header("Authorization", &self.auth_header)
            .header("Accept", "application/json")
            .send()
            .await?
            .json()
            .await?;
        resp["results"][0]["id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("Confluence space '{}' not found", self.space_key))
    }

    /// GET a Confluence search endpoint and project the first result into a
    /// `PageSummary`. Shared by the Cloud and Server variants, which differ only in the
    /// query URL they build.
    async fn fetch_first_page_summary(&self, url: &str) -> anyhow::Result<Option<PageSummary>> {
        let resp: serde_json::Value = self
            .client
            .get(url)
            .header("Authorization", &self.auth_header)
            .header("Accept", "application/json")
            .send()
            .await?
            .json()
            .await?;
        let results = resp["results"].as_array();
        if results.is_none_or(std::vec::Vec::is_empty) {
            return Ok(None);
        }
        let page = &resp["results"][0];
        let id = page["id"].as_str().unwrap_or("").to_owned();
        // Confluence version numbers are small; truncation is not possible in practice.
        #[allow(clippy::cast_possible_truncation)]
        let ver = page["version"]["number"].as_u64().unwrap_or(1) as u32;
        Ok(Some(PageSummary {
            id,
            version_number: ver,
        }))
    }

    async fn find_page_cloud(
        &self,
        space_id: &str,
        title: &str,
    ) -> anyhow::Result<Option<PageSummary>> {
        let enc = urlencoding_encode(title);
        let url = format!(
            "{}/wiki/api/v2/pages?spaceId={}&title={}&limit=1&expand=version",
            self.base_url, space_id, enc
        );
        self.fetch_first_page_summary(&url).await
    }

    async fn find_page_server(&self, title: &str) -> anyhow::Result<Option<PageSummary>> {
        let enc = urlencoding_encode(title);
        let url = format!(
            "{}/rest/api/content?spaceKey={}&title={}&type=page&expand=version&limit=1",
            self.base_url, self.space_key, enc
        );
        self.fetch_first_page_summary(&url).await
    }

    async fn create_cloud(
        &self,
        space_id: &str,
        title: &str,
        body_html: &str,
    ) -> anyhow::Result<String> {
        let mut payload = serde_json::json!({
            "spaceId": space_id,
            "title": title,
            "body": { "representation": "storage", "value": body_html }
        });
        if let Some(parent_id) = &self.parent_page_id {
            payload["parentId"] = serde_json::Value::String(parent_id.clone());
        }
        self.post_create_page(
            format!("{}/wiki/api/v2/pages", self.base_url),
            &payload,
            "Cloud",
        )
        .await
    }

    /// POST a page-create payload and return the new page id. Shared by the Cloud and
    /// Server variants, which differ only in the endpoint URL, request payload, and the
    /// label used in the error message.
    async fn post_create_page(
        &self,
        url: String,
        payload: &serde_json::Value,
        label: &str,
    ) -> anyhow::Result<String> {
        let resp = self
            .client
            .post(url)
            .header("Authorization", &self.auth_header)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .json(payload)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Confluence {label} create failed (HTTP {status}): {body}");
        }
        let created: serde_json::Value = resp.json().await?;
        Ok(created["id"].as_str().unwrap_or("").to_owned())
    }

    async fn create_server(&self, title: &str, body_html: &str) -> anyhow::Result<String> {
        let mut payload = serde_json::json!({
            "type": "page",
            "space": { "key": self.space_key },
            "title": title,
            "body": { "storage": { "value": body_html, "representation": "storage" } }
        });
        if let Some(parent_id) = &self.parent_page_id {
            payload["ancestors"] = serde_json::json!([{ "id": parent_id }]);
        }
        self.post_create_page(
            format!("{}/rest/api/content", self.base_url),
            &payload,
            "Server",
        )
        .await
    }

    async fn update_cloud(
        &self,
        page_id: &str,
        ver: u32,
        title: &str,
        body_html: &str,
    ) -> anyhow::Result<()> {
        let payload = serde_json::json!({
            "version": { "number": ver + 1 },
            "title": title,
            "body": { "representation": "storage", "value": body_html }
        });
        self.put_update_page(
            format!("{}/wiki/api/v2/pages/{page_id}", self.base_url),
            &payload,
            "Cloud",
        )
        .await
    }

    /// PUT a page-update payload. Shared by the Cloud and Server variants, which differ
    /// only in the endpoint URL, request payload, and error-message label.
    async fn put_update_page(
        &self,
        url: String,
        payload: &serde_json::Value,
        label: &str,
    ) -> anyhow::Result<()> {
        let resp = self
            .client
            .put(url)
            .header("Authorization", &self.auth_header)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .json(payload)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Confluence {label} update failed (HTTP {status}): {body}");
        }
        Ok(())
    }

    async fn update_server(
        &self,
        page_id: &str,
        ver: u32,
        title: &str,
        body_html: &str,
    ) -> anyhow::Result<()> {
        let payload = serde_json::json!({
            "version": { "number": ver + 1 },
            "type": "page",
            "title": title,
            "space": { "key": self.space_key },
            "body": { "storage": { "value": body_html, "representation": "storage" } }
        });
        self.put_update_page(
            format!("{}/rest/api/content/{page_id}", self.base_url),
            &payload,
            "Server",
        )
        .await
    }

    pub async fn test_connection(&self) -> anyhow::Result<()> {
        let url = match self.tier {
            ConfluenceTier::Cloud => {
                format!("{}/wiki/api/v2/spaces?limit=1", self.base_url)
            }
            ConfluenceTier::Server => {
                format!("{}/rest/api/space?limit=1", self.base_url)
            }
        };
        let resp = self
            .client
            .get(&url)
            .header("Authorization", &self.auth_header)
            .header("Accept", "application/json")
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            anyhow::bail!("Confluence connection test failed (HTTP {status})");
        }
        Ok(())
    }

    /// Upload a file as an attachment on an existing page. Uses the v1 REST
    /// attachment endpoint (identical shape on Cloud — under `/wiki` — and
    /// Server/DC). Re-posting the same filename updates the attachment to a new
    /// version rather than erroring, so repeated publishes stay idempotent.
    ///
    /// The multipart/form-data body is hand-assembled so we do not need reqwest's
    /// `multipart` feature (the workspace builds reqwest with default features off
    /// for the air-gapped vendor set).
    async fn upload_attachment(
        &self,
        page_id: &str,
        filename: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> anyhow::Result<()> {
        let url = match self.tier {
            ConfluenceTier::Cloud => {
                format!(
                    "{}/wiki/rest/api/content/{page_id}/child/attachment",
                    self.base_url
                )
            }
            ConfluenceTier::Server => {
                format!(
                    "{}/rest/api/content/{page_id}/child/attachment",
                    self.base_url
                )
            }
        };

        let boundary = "oxidesloc7f3c1a9b2e5d4680boundary";
        let mut body: Vec<u8> = Vec::with_capacity(bytes.len() + 512);
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
        // minorEdit=true avoids notifying page watchers on every re-upload.
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(b"Content-Disposition: form-data; name=\"minorEdit\"\r\n\r\n");
        body.extend_from_slice(b"true\r\n");
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

        let resp = self
            .client
            .post(&url)
            .header("Authorization", &self.auth_header)
            // Required by Confluence to accept a cross-origin-style multipart upload.
            .header("X-Atlassian-Token", "nocheck")
            .header("Accept", "application/json")
            .header(
                "Content-Type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(body)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Confluence attachment upload failed (HTTP {status}): {text}");
        }
        Ok(())
    }
}

/// Best-effort: attach the finished HTML/PDF report files to a Confluence page.
/// Failures are logged and swallowed — the summary page (table + report link) is
/// the primary artifact; the embedded attachments are a convenience so the full
/// report is viewable inside Confluence without linking back to the server.
pub async fn attach_reports(
    client: &ConfluenceClient,
    page_id: &str,
    html_path: Option<&Path>,
    pdf_path: Option<&Path>,
) {
    for (path, name, ctype) in [
        (html_path, "oxide-sloc-report.html", "text/html"),
        (pdf_path, "oxide-sloc-report.pdf", "application/pdf"),
    ] {
        let Some(p) = path else { continue };
        match std::fs::read(p) {
            Ok(bytes) => {
                if let Err(e) = client.upload_attachment(page_id, name, ctype, &bytes).await {
                    tracing::warn!("Confluence attachment '{name}' skipped: {e:#}");
                }
            }
            Err(e) => tracing::warn!("Confluence attachment '{name}' unreadable ({p:?}): {e}"),
        }
    }
}

/// Create or update a Confluence page with the SLOC scan result. Returns the
/// page ID of the created or updated page.
pub async fn post_to_confluence(
    client: &ConfluenceClient,
    run: &sloc_core::AnalysisRun,
    page_title: &str,
    report_url: Option<&str>,
) -> anyhow::Result<String> {
    let body_html = render_confluence_storage(run, report_url);

    match client.tier {
        ConfluenceTier::Cloud => {
            let space_id = client.find_space_id().await?;
            match client.find_page_cloud(&space_id, page_title).await? {
                None => client.create_cloud(&space_id, page_title, &body_html).await,
                Some(existing) => {
                    client
                        .update_cloud(
                            &existing.id,
                            existing.version_number,
                            page_title,
                            &body_html,
                        )
                        .await?;
                    Ok(existing.id)
                }
            }
        }
        ConfluenceTier::Server => match client.find_page_server(page_title).await? {
            None => client.create_server(page_title, &body_html).await,
            Some(existing) => {
                client
                    .update_server(
                        &existing.id,
                        existing.version_number,
                        page_title,
                        &body_html,
                    )
                    .await?;
                Ok(existing.id)
            }
        },
    }
}

// ── request / response types ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SaveConfluenceConfig {
    pub tier: Option<String>,
    pub base_url: String,
    pub username: String,
    /// Blank means "keep existing credential"
    pub credential: String,
    pub space_key: String,
    pub parent_page_id: Option<String>,
    #[serde(default)]
    pub schedule_auto_post: HashMap<String, bool>,
}

#[derive(Debug, Deserialize)]
pub struct PostToConfluenceRequest {
    pub run_id: String,
    pub page_title: String,
    pub report_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RunIdQuery {
    pub run_id: String,
}

// ── route handlers ────────────────────────────────────────────────────────────

pub async fn api_get_confluence_config(State(state): State<AppState>) -> impl IntoResponse {
    let store = state.confluence.lock().await;
    Json(store.config.as_ref().map_or_else(
        || {
            serde_json::json!({
                "configured": false,
                "tier": "cloud",
                "base_url": "",
                "username": "",
                "api_token_set": false,
                "space_key": "",
                "parent_page_id": null,
                "schedule_auto_post": {}
            })
        },
        |c| {
            serde_json::json!({
                "configured": true,
                "tier": if c.tier == ConfluenceTier::Cloud { "cloud" } else { "server" },
                "base_url": c.base_url,
                "username": c.username,
                "api_token_set": !c.credential.is_empty(),
                "space_key": c.space_key,
                "parent_page_id": c.parent_page_id,
                "schedule_auto_post": c.schedule_auto_post
            })
        },
    ))
}

pub async fn api_save_confluence_config(
    State(state): State<AppState>,
    Json(body): Json<SaveConfluenceConfig>,
) -> Response {
    if let Err(msg) = validate_confluence_url(&body.base_url) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": msg })),
        )
            .into_response();
    }

    let tier = match body.tier.as_deref() {
        Some("server") => ConfluenceTier::Server,
        _ => ConfluenceTier::Cloud,
    };

    let mut store = state.confluence.lock().await;
    let existing_credential = store
        .config
        .as_ref()
        .map(|c| c.credential.clone())
        .unwrap_or_default();

    let credential = if body.credential.is_empty() {
        existing_credential
    } else {
        body.credential.clone()
    };

    store.config = Some(ConfluenceConfig {
        tier,
        base_url: body.base_url.trim_end_matches('/').to_owned(),
        username: body.username.clone(),
        credential,
        space_key: body.space_key.clone(),
        parent_page_id: body.parent_page_id.clone().filter(|s| !s.is_empty()),
        schedule_auto_post: body.schedule_auto_post.clone(),
    });
    let _ = store.save(&state.confluence_path);
    drop(store);
    Json(serde_json::json!({ "ok": true })).into_response()
}

pub async fn api_test_confluence(State(state): State<AppState>) -> Response {
    let config = {
        let store = state.confluence.lock().await;
        store.config.clone()
    };
    let Some(config) = config else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": "Confluence is not configured." })),
        )
            .into_response();
    };
    // Defense in depth: re-validate the stored URL before any outbound request,
    // in case the config was written to disk directly, bypassing save-time validation.
    if let Err(msg) = validate_confluence_url(&config.base_url) {
        tracing::warn!("Confluence connection test blocked: {msg}");
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "ok": false, "error": msg })),
        )
            .into_response();
    }
    let client = ConfluenceClient::new(&config);
    match client.test_connection().await {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => {
            tracing::warn!("Confluence connection test failed: {e:#}");
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "ok": false, "error": "Connection test failed." })),
            )
                .into_response()
        }
    }
}

// Linear chain of request-validation guard clauses, each returning a distinct JSON error
// response; splitting it would scatter the validation flow without reducing complexity.
#[allow(
    clippy::too_many_lines,
    reason = "sequential request-validation guard clauses"
)]
pub async fn api_post_to_confluence(
    State(state): State<AppState>,
    Json(body): Json<PostToConfluenceRequest>,
) -> Response {
    if !run_id_is_valid(&body.run_id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": "Invalid run_id" })),
        )
            .into_response();
    }
    publish_run(
        &state,
        &body.run_id,
        &body.page_title,
        body.report_url.as_deref(),
    )
    .await
}

/// Validate a run id used to look up artifacts: non-empty, bounded, and restricted to a
/// safe alphabet so it cannot traverse paths or be abused as a lookup key.
fn run_id_is_valid(run_id: &str) -> bool {
    !run_id.is_empty()
        && run_id.len() <= 128
        && run_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Core "publish a run to Confluence" flow, shared by the dedicated Confluence endpoint
/// and the unified run-export action (`POST /api/runs/{id}/export?target=confluence`).
/// Resolves the stored Confluence config and the run's artifacts, re-validates the URL,
/// creates/updates the page, and attaches the HTML/PDF reports. Callers must validate
/// `run_id` first (see `run_id_is_valid`).
pub(crate) async fn publish_run(
    state: &AppState,
    run_id: &str,
    page_title: &str,
    report_url: Option<&str>,
) -> Response {
    let config = {
        let store = state.confluence.lock().await;
        store.config.clone()
    };
    let Some(config) = config else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "ok": false,
                "error": "Confluence is not configured. Visit /confluence-setup first."
            })),
        )
            .into_response();
    };

    // Locate artifacts
    let artifacts = {
        let map = state.artifacts.lock().await;
        map.get(run_id).cloned()
    };
    let artifacts = if let Some(a) = artifacts {
        a
    } else {
        let reg = state.registry.lock().await;
        match reg.find_by_run_id(run_id) {
            Some(entry) => recover_artifacts_from_registry(entry),
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "ok": false,
                        "error": "Run not found in scan history."
                    })),
                )
                    .into_response();
            }
        }
    };

    let json_path = match &artifacts.json_path {
        Some(p) => p.clone(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "ok": false,
                    "error": "No JSON result saved for this run."
                })),
            )
                .into_response();
        }
    };

    let run = match read_json(&json_path) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "ok": false,
                    "error": format!("Could not load scan result: {e}")
                })),
            )
                .into_response();
        }
    };

    // Defense in depth: re-validate the stored URL before publishing.
    if let Err(msg) = validate_confluence_url(&config.base_url) {
        tracing::warn!("Confluence publish blocked for run '{}': {msg}", run_id);
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "ok": false, "error": msg })),
        )
            .into_response();
    }
    let client = ConfluenceClient::new(&config);

    match post_to_confluence(&client, &run, page_title, report_url).await {
        Ok(page_id) => {
            attach_reports(
                &client,
                &page_id,
                artifacts.html_path.as_deref(),
                artifacts.pdf_path.as_deref(),
            )
            .await;
            Json(serde_json::json!({ "ok": true, "page_id": page_id })).into_response()
        }
        Err(e) => {
            tracing::warn!("Confluence publish failed for run '{}': {e:#}", run_id);
            (
                StatusCode::BAD_GATEWAY,
                Json(
                    serde_json::json!({ "ok": false, "error": "Failed to publish to Confluence." }),
                ),
            )
                .into_response()
        }
    }
}

pub async fn api_wiki_markup(
    State(state): State<AppState>,
    Query(q): Query<RunIdQuery>,
) -> Response {
    if q.run_id.len() > 128 || q.run_id.contains('/') || q.run_id.contains('\\') {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let artifacts = {
        let map = state.artifacts.lock().await;
        map.get(&q.run_id).cloned()
    };
    let artifacts = if let Some(a) = artifacts {
        a
    } else {
        let reg = state.registry.lock().await;
        match reg.find_by_run_id(&q.run_id) {
            Some(entry) => recover_artifacts_from_registry(entry),
            None => return StatusCode::NOT_FOUND.into_response(),
        }
    };

    let json_path = match &artifacts.json_path {
        Some(p) => p.clone(),
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    let Ok(run) = read_json(&json_path) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };

    let markup = render_confluence_wiki_markup(&run);
    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        markup,
    )
        .into_response()
}

// ── auto-post hook (called from git_webhook.rs) ────────────────────────────────

pub async fn maybe_auto_post_confluence(
    state: &AppState,
    sched_id: uuid::Uuid,
    run: &sloc_core::AnalysisRun,
    run_id: &str,
) {
    let (config, enabled) = {
        let store = state.confluence.lock().await;
        let en = store
            .config
            .as_ref()
            .and_then(|c| c.schedule_auto_post.get(&sched_id.to_string()))
            .copied()
            .unwrap_or(false);
        (store.config.clone(), en)
    };

    if !enabled {
        return;
    }
    let Some(config) = config else { return };

    // Defense in depth: re-validate the stored URL before auto-posting.
    if let Err(msg) = validate_confluence_url(&config.base_url) {
        tracing::warn!("Confluence auto-post skipped for schedule {sched_id}: {msg}");
        return;
    }
    let client = ConfluenceClient::new(&config);
    let title = format!(
        "OxideSLOC — {} ({})",
        run.effective_configuration.reporting.report_title,
        &run_id[..run_id.len().min(8)]
    );

    // Build a server URL for the report link using bind_address from config
    let bind = &state.base_config.web.bind_address;
    let proto = if state.tls_enabled { "https" } else { "http" };
    let report_url = format!("{proto}://{bind}/runs/result/{run_id}");

    match post_to_confluence(&client, run, &title, Some(&report_url)).await {
        Ok(page_id) => {
            // Look up the run's saved artifacts to attach the full report files.
            let artifacts = {
                let map = state.artifacts.lock().await;
                map.get(run_id).cloned()
            };
            let artifacts = if let Some(a) = artifacts {
                Some(a)
            } else {
                let reg = state.registry.lock().await;
                reg.find_by_run_id(run_id)
                    .map(recover_artifacts_from_registry)
            };
            if let Some(a) = artifacts {
                attach_reports(
                    &client,
                    &page_id,
                    a.html_path.as_deref(),
                    a.pdf_path.as_deref(),
                )
                .await;
            }
        }
        Err(e) => tracing::warn!("Confluence auto-post failed for schedule {sched_id}: {e:#}"),
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn urlencoding_encode(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                write!(out, "{b:02X}").expect("write to String is infallible");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // ── validate_confluence_url / is_blocked_confluence_host ──────────────────

    #[test]
    fn validate_confluence_url_empty_is_ok() {
        // Empty URL = clear/reset, allowed.
        assert!(validate_confluence_url("").is_ok());
        assert!(validate_confluence_url("   ").is_ok());
    }

    #[test]
    fn validate_confluence_url_public_https_ok() {
        assert!(validate_confluence_url("https://mycompany.atlassian.net").is_ok());
        assert!(validate_confluence_url("https://confluence.example.com/wiki").is_ok());
    }

    #[test]
    fn validate_confluence_url_internal_private_ok() {
        // Self-hosted Confluence on a corporate network must remain reachable.
        assert!(validate_confluence_url("https://confluence.corp.internal").is_ok());
        assert!(validate_confluence_url("https://10.0.0.5:8090").is_ok());
        assert!(validate_confluence_url("http://192.168.1.10/wiki").is_ok());
    }

    #[test]
    fn validate_confluence_url_rejects_non_http_scheme() {
        assert!(validate_confluence_url("file:///etc/passwd").is_err());
        assert!(validate_confluence_url("gopher://x/").is_err());
    }

    #[test]
    fn validate_confluence_url_rejects_metadata_and_loopback() {
        assert!(validate_confluence_url("http://169.254.169.254/latest/meta-data/").is_err());
        assert!(validate_confluence_url("http://metadata.google.internal/").is_err());
        assert!(validate_confluence_url("http://100.100.100.200/").is_err());
        assert!(validate_confluence_url("http://127.0.0.1:8090/").is_err());
        assert!(validate_confluence_url("http://localhost/wiki").is_err());
        assert!(validate_confluence_url("http://[::1]/wiki").is_err());
    }

    #[test]
    fn is_blocked_confluence_host_no_false_positive_on_fc_fd_names() {
        // Regression: hostname prefix "fc"/"fd" must NOT be treated as IPv6 ULA.
        assert!(!is_blocked_confluence_host("fc.example.com"));
        assert!(!is_blocked_confluence_host("fd-corp.example.com"));
        assert!(!is_blocked_confluence_host("fe80-server.example.com"));
    }

    #[test]
    fn is_blocked_confluence_host_blocks_ipv6_link_local_literal() {
        assert!(is_blocked_confluence_host("fe80::1"));
        assert!(is_blocked_confluence_host("[fe80::1]"));
    }

    #[test]
    fn is_blocked_confluence_host_allows_ipv6_ula_literal() {
        // IPv6 unique-local is the private-range equivalent — allowed.
        assert!(!is_blocked_confluence_host("fd12:3456:789a::1"));
    }

    // ── ConfluenceTier default ────────────────────────────────────────────────

    #[test]
    fn tier_default_is_cloud() {
        assert_eq!(ConfluenceTier::default(), ConfluenceTier::Cloud);
    }

    // ── ConfluenceConfig::is_cloud_url / effective_tier ───────────────────────

    #[test]
    fn effective_tier_atlassian_net_is_cloud() {
        let cfg = ConfluenceConfig {
            tier: ConfluenceTier::Server,
            base_url: "https://mycompany.atlassian.net".into(),
            ..Default::default()
        };
        assert_eq!(cfg.effective_tier(), &ConfluenceTier::Cloud);
    }

    #[test]
    fn effective_tier_server_url_is_server() {
        let cfg = ConfluenceConfig {
            tier: ConfluenceTier::Server,
            base_url: "https://confluence.corp.com".into(),
            ..Default::default()
        };
        assert_eq!(cfg.effective_tier(), &ConfluenceTier::Server);
    }

    #[test]
    fn effective_tier_cloud_flag_overrides_non_cloud_url() {
        let cfg = ConfluenceConfig {
            tier: ConfluenceTier::Cloud,
            base_url: "https://confluence.corp.com".into(),
            ..Default::default()
        };
        // tier field is Cloud, so effective_tier must also be Cloud
        assert_eq!(cfg.effective_tier(), &ConfluenceTier::Cloud);
    }

    // ── ConfluenceConfigStore::is_configured ──────────────────────────────────

    #[test]
    fn is_configured_no_config_returns_false() {
        let store = ConfluenceConfigStore { config: None };
        assert!(!store.is_configured());
    }

    #[test]
    fn is_configured_empty_base_url_returns_false() {
        let store = ConfluenceConfigStore {
            config: Some(ConfluenceConfig {
                base_url: String::new(),
                credential: "token".into(),
                ..Default::default()
            }),
        };
        assert!(!store.is_configured());
    }

    #[test]
    fn is_configured_empty_credential_returns_false() {
        let store = ConfluenceConfigStore {
            config: Some(ConfluenceConfig {
                base_url: "https://mycompany.atlassian.net".into(),
                credential: String::new(),
                ..Default::default()
            }),
        };
        assert!(!store.is_configured());
    }

    #[test]
    fn is_configured_both_fields_set_returns_true() {
        let store = ConfluenceConfigStore {
            config: Some(ConfluenceConfig {
                base_url: "https://mycompany.atlassian.net".into(),
                credential: "my-api-token".into(),
                ..Default::default()
            }),
        };
        assert!(store.is_configured());
    }

    // ── ConfluenceConfigStore::save / load round-trip ─────────────────────────

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("confluence.json");

        let original = ConfluenceConfigStore {
            config: Some(ConfluenceConfig {
                tier: ConfluenceTier::Server,
                base_url: "https://confluence.corp.com".into(),
                username: "alice".into(),
                credential: "secret".into(),
                space_key: "DEV".into(),
                parent_page_id: Some("12345".into()),
                schedule_auto_post: std::collections::HashMap::new(),
            }),
        };

        original.save(&path).expect("save must succeed");

        // credential is #[serde(skip)] so it won't be in the file
        let loaded = ConfluenceConfigStore::load(&path);
        assert!(loaded.config.is_some());
        let cfg = loaded.config.as_ref().unwrap();
        assert_eq!(cfg.base_url, "https://confluence.corp.com");
        assert_eq!(cfg.username, "alice");
        assert_eq!(cfg.space_key, "DEV");
        assert_eq!(cfg.parent_page_id.as_deref(), Some("12345"));
        // credential is skipped in serialization
        assert!(cfg.credential.is_empty());
    }

    #[test]
    fn load_nonexistent_file_returns_default() {
        let store = ConfluenceConfigStore::load(std::path::Path::new(
            "/nonexistent/__sloc_test_confluence__.json",
        ));
        assert!(store.config.is_none());
        assert!(!store.is_configured());
    }

    #[test]
    fn save_empty_store_then_load() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("empty.json");
        let store = ConfluenceConfigStore { config: None };
        store.save(&path).expect("save empty store must succeed");
        let loaded = ConfluenceConfigStore::load(&path);
        assert!(loaded.config.is_none());
    }

    // ── urlencoding_encode ────────────────────────────────────────────────────

    #[test]
    fn encode_alphanumeric_unchanged() {
        assert_eq!(urlencoding_encode("HelloWorld123"), "HelloWorld123");
    }

    #[test]
    fn encode_unreserved_chars_unchanged() {
        assert_eq!(urlencoding_encode("-_.~"), "-_.~");
    }

    #[test]
    fn encode_space_becomes_plus() {
        assert_eq!(urlencoding_encode("hello world"), "hello+world");
    }

    #[test]
    fn encode_slash_percent_encoded() {
        assert_eq!(urlencoding_encode("a/b"), "a%2Fb");
    }

    #[test]
    fn encode_ampersand_percent_encoded() {
        assert_eq!(urlencoding_encode("a&b"), "a%26b");
    }

    #[test]
    fn encode_empty_string() {
        assert_eq!(urlencoding_encode(""), "");
    }

    #[test]
    fn encode_mixed_content() {
        let result = urlencoding_encode("My Report 2024/01");
        assert!(result.contains("My+Report+2024"));
        assert!(result.contains("%2F01"));
    }

    // ── ConfluenceConfig::is_cloud_url ────────────────────────────────────────

    #[test]
    fn is_cloud_url_atlassian_net_is_true() {
        let cfg = ConfluenceConfig {
            base_url: "https://acme.atlassian.net".into(),
            ..Default::default()
        };
        assert!(cfg.is_cloud_url());
    }

    #[test]
    fn is_cloud_url_non_atlassian_is_false() {
        let cfg = ConfluenceConfig {
            base_url: "https://confluence.example.com".into(),
            ..Default::default()
        };
        assert!(!cfg.is_cloud_url());
    }

    #[test]
    fn is_cloud_url_atlassian_net_case_insensitive() {
        let cfg = ConfluenceConfig {
            base_url: "https://COMPANY.ATLASSIAN.NET".into(),
            ..Default::default()
        };
        assert!(cfg.is_cloud_url(), "should be case-insensitive");
    }

    #[test]
    fn is_cloud_url_empty_string_is_false() {
        let cfg = ConfluenceConfig {
            base_url: String::new(),
            ..Default::default()
        };
        assert!(!cfg.is_cloud_url());
    }

    // ── ConfluenceConfigStore::save / load — parent_page_id None ─────────────

    #[test]
    fn save_and_load_no_parent_page_id() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("no_parent.json");

        let original = ConfluenceConfigStore {
            config: Some(ConfluenceConfig {
                tier: ConfluenceTier::Cloud,
                base_url: "https://co.atlassian.net".into(),
                username: "u@example.com".into(),
                credential: String::new(),
                space_key: "DEV".into(),
                parent_page_id: None,
                schedule_auto_post: std::collections::HashMap::new(),
            }),
        };
        original.save(&path).expect("save must succeed");
        let loaded = ConfluenceConfigStore::load(&path);
        let cfg = loaded.config.as_ref().expect("config must be present");
        assert!(cfg.parent_page_id.is_none());
    }

    // ── ConfluenceClient::new — empty username uses Bearer auth ──────────────

    #[test]
    fn client_new_empty_username_uses_bearer() {
        // Install TLS provider so reqwest::Client::new() doesn't panic.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let cfg = ConfluenceConfig {
            tier: ConfluenceTier::Server,
            base_url: "https://confluence.corp.com".into(),
            username: String::new(), // empty → Bearer PAT
            credential: "my-pat-token".into(),
            space_key: "DEV".into(),
            parent_page_id: None,
            schedule_auto_post: std::collections::HashMap::new(),
        };
        // Should not panic
        let _client = ConfluenceClient::new(&cfg);
    }

    // ── urlencoding_encode — additional special chars ─────────────────────────

    #[test]
    fn encode_question_mark_percent_encoded() {
        let result = urlencoding_encode("key?value");
        assert!(result.contains("%3F"), "? must become %3F, got: {result}");
    }

    #[test]
    fn encode_percent_sign_percent_encoded() {
        let result = urlencoding_encode("100%");
        assert!(result.contains("%25"), "% must become %25, got: {result}");
    }

    #[test]
    fn encode_tab_character_percent_encoded() {
        let result = urlencoding_encode("a\tb");
        assert!(result.contains("%09"), "tab must become %09, got: {result}");
    }

    #[test]
    fn encode_newline_percent_encoded() {
        let result = urlencoding_encode("line1\nline2");
        assert!(
            result.contains("%0A"),
            "newline must become %0A, got: {result}"
        );
    }

    // ── ConfluenceConfigStore schedule_auto_post ──────────────────────────────

    #[test]
    fn schedule_auto_post_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sched_auto.json");

        let mut auto_post = std::collections::HashMap::new();
        auto_post.insert("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_owned(), true);
        auto_post.insert("11111111-2222-3333-4444-555555555555".to_owned(), false);

        let original = ConfluenceConfigStore {
            config: Some(ConfluenceConfig {
                tier: ConfluenceTier::Cloud,
                base_url: "https://acme.atlassian.net".into(),
                username: "x@example.com".into(),
                credential: String::new(),
                space_key: "PRJ".into(),
                parent_page_id: None,
                schedule_auto_post: auto_post.clone(),
            }),
        };
        original.save(&path).expect("save must succeed");
        let loaded = ConfluenceConfigStore::load(&path);
        let cfg = loaded.config.as_ref().expect("config must be present");
        assert_eq!(cfg.schedule_auto_post.len(), 2);
        assert_eq!(
            cfg.schedule_auto_post
                .get("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
            Some(&true)
        );
        assert_eq!(
            cfg.schedule_auto_post
                .get("11111111-2222-3333-4444-555555555555"),
            Some(&false)
        );
    }
}

// ── HTTP-mocked tests for ConfluenceClient ────────────────────────────────────
//
// Uses a minimal Axum router on a random port (tokio TcpListener) as the mock
// Confluence server. No extra deps — axum and tokio are already in [dependencies].

#[cfg(test)]
mod http_tests {
    use super::*;
    use axum::{Json, Router, routing};
    use std::net::SocketAddr;

    fn setup_tls() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    fn cloud_cfg(base_url: &str) -> ConfluenceConfig {
        ConfluenceConfig {
            tier: ConfluenceTier::Cloud,
            base_url: base_url.to_owned(),
            username: "user@example.com".to_owned(),
            credential: "api-token-123".to_owned(),
            space_key: "MYSPACE".to_owned(),
            parent_page_id: None,
            schedule_auto_post: HashMap::default(),
        }
    }

    fn server_cfg(base_url: &str) -> ConfluenceConfig {
        ConfluenceConfig {
            tier: ConfluenceTier::Server,
            base_url: base_url.to_owned(),
            username: "admin".to_owned(),
            credential: "password".to_owned(),
            space_key: "MYSPACE".to_owned(),
            parent_page_id: None,
            schedule_auto_post: HashMap::default(),
        }
    }

    /// Spin up a throw-away Axum app on an ephemeral port, return the bound address.
    async fn start_mock(app: Router) -> SocketAddr {
        // Install a TLS crypto provider so reqwest::Client::new() doesn't panic.
        setup_tls();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        addr
    }

    #[tokio::test]
    async fn test_connection_cloud_ok() {
        let app = Router::new().route(
            "/wiki/api/v2/spaces",
            routing::get(|| async { Json(serde_json::json!({"results": []})) }),
        );
        let addr = start_mock(app).await;
        let client = ConfluenceClient::new(&cloud_cfg(&format!("http://{addr}")));
        assert!(
            client.test_connection().await.is_ok(),
            "test_connection should succeed with 200"
        );
    }

    #[tokio::test]
    async fn test_connection_cloud_unauthorized() {
        use axum::http::StatusCode;
        let app = Router::new().route(
            "/wiki/api/v2/spaces",
            routing::get(|| async { StatusCode::UNAUTHORIZED }),
        );
        let addr = start_mock(app).await;
        let client = ConfluenceClient::new(&cloud_cfg(&format!("http://{addr}")));
        assert!(
            client.test_connection().await.is_err(),
            "test_connection should fail with 401"
        );
    }

    #[tokio::test]
    async fn test_connection_server_ok() {
        let app = Router::new().route(
            "/rest/api/space",
            routing::get(|| async { Json(serde_json::json!({"results": []})) }),
        );
        let addr = start_mock(app).await;
        let client = ConfluenceClient::new(&server_cfg(&format!("http://{addr}")));
        assert!(
            client.test_connection().await.is_ok(),
            "server test_connection should succeed"
        );
    }

    #[tokio::test]
    async fn test_connection_server_unauthorized() {
        use axum::http::StatusCode;
        let app = Router::new().route(
            "/rest/api/space",
            routing::get(|| async { StatusCode::UNAUTHORIZED }),
        );
        let addr = start_mock(app).await;
        let client = ConfluenceClient::new(&server_cfg(&format!("http://{addr}")));
        assert!(
            client.test_connection().await.is_err(),
            "server test_connection should fail with 401"
        );
    }

    #[tokio::test]
    async fn find_space_id_cloud_returns_id() {
        let app = Router::new().route(
            "/wiki/api/v2/spaces",
            routing::get(|| async {
                Json(serde_json::json!({"results": [{"id": "12345", "key": "MYSPACE"}]}))
            }),
        );
        let addr = start_mock(app).await;
        let client = ConfluenceClient::new(&cloud_cfg(&format!("http://{addr}")));
        let space_id = client
            .find_space_id()
            .await
            .expect("should return space id");
        assert_eq!(space_id, "12345");
    }

    #[tokio::test]
    async fn find_space_id_cloud_not_found_returns_error() {
        let app = Router::new().route(
            "/wiki/api/v2/spaces",
            routing::get(|| async { Json(serde_json::json!({"results": []})) }),
        );
        let addr = start_mock(app).await;
        let client = ConfluenceClient::new(&cloud_cfg(&format!("http://{addr}")));
        assert!(
            client.find_space_id().await.is_err(),
            "should error when space not found"
        );
    }

    #[tokio::test]
    async fn find_page_cloud_not_found_returns_none() {
        let app = Router::new().route(
            "/wiki/api/v2/pages",
            routing::get(|| async { Json(serde_json::json!({"results": []})) }),
        );
        let addr = start_mock(app).await;
        let client = ConfluenceClient::new(&cloud_cfg(&format!("http://{addr}")));
        let result = client
            .find_page_cloud("99", "My Report")
            .await
            .expect("should not error");
        assert!(result.is_none(), "should return None when page not found");
    }

    #[tokio::test]
    async fn find_page_cloud_found_returns_summary() {
        let app = Router::new().route(
            "/wiki/api/v2/pages",
            routing::get(|| async {
                Json(serde_json::json!({"results": [{"id": "777", "version": {"number": 3}}]}))
            }),
        );
        let addr = start_mock(app).await;
        let client = ConfluenceClient::new(&cloud_cfg(&format!("http://{addr}")));
        let result = client
            .find_page_cloud("99", "My Report")
            .await
            .expect("should not error");
        let ps = result.expect("should find the page");
        assert_eq!(ps.id, "777");
        assert_eq!(ps.version_number, 3);
    }

    #[tokio::test]
    async fn find_page_server_not_found_returns_none() {
        let app = Router::new().route(
            "/rest/api/content",
            routing::get(|| async { Json(serde_json::json!({"results": []})) }),
        );
        let addr = start_mock(app).await;
        let client = ConfluenceClient::new(&server_cfg(&format!("http://{addr}")));
        let result = client
            .find_page_server("My Report")
            .await
            .expect("should not error");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn find_page_server_found_returns_summary() {
        let app = Router::new().route(
            "/rest/api/content",
            routing::get(|| async {
                Json(serde_json::json!({"results": [{"id": "888", "version": {"number": 2}}]}))
            }),
        );
        let addr = start_mock(app).await;
        let client = ConfluenceClient::new(&server_cfg(&format!("http://{addr}")));
        let result = client
            .find_page_server("My Report")
            .await
            .expect("should not error");
        let ps = result.expect("should find the page");
        assert_eq!(ps.id, "888");
        assert_eq!(ps.version_number, 2);
    }

    #[tokio::test]
    async fn create_cloud_page_success() {
        let app = Router::new().route(
            "/wiki/api/v2/pages",
            routing::post(|| async { Json(serde_json::json!({"id": "555"})) }),
        );
        let addr = start_mock(app).await;
        let client = ConfluenceClient::new(&cloud_cfg(&format!("http://{addr}")));
        let id = client
            .create_cloud("99", "My Report", "<p>content</p>")
            .await
            .expect("create should succeed");
        assert_eq!(id, "555");
    }

    #[tokio::test]
    async fn create_cloud_page_failure_returns_error() {
        use axum::http::StatusCode;
        let app = Router::new().route(
            "/wiki/api/v2/pages",
            routing::post(|| async { StatusCode::FORBIDDEN }),
        );
        let addr = start_mock(app).await;
        let client = ConfluenceClient::new(&cloud_cfg(&format!("http://{addr}")));
        assert!(
            client
                .create_cloud("99", "My Report", "<p>content</p>")
                .await
                .is_err(),
            "create should fail with 403"
        );
    }

    #[tokio::test]
    async fn create_server_page_success() {
        let app = Router::new().route(
            "/rest/api/content",
            routing::post(|| async { Json(serde_json::json!({"id": "444"})) }),
        );
        let addr = start_mock(app).await;
        let client = ConfluenceClient::new(&server_cfg(&format!("http://{addr}")));
        let id = client
            .create_server("My Report", "<p>content</p>")
            .await
            .expect("server create should succeed");
        assert_eq!(id, "444");
    }

    #[tokio::test]
    async fn update_cloud_page_success() {
        let app = Router::new().route(
            "/wiki/api/v2/pages/{id}",
            routing::put(|| async { Json(serde_json::json!({"id": "777"})) }),
        );
        let addr = start_mock(app).await;
        let client = ConfluenceClient::new(&cloud_cfg(&format!("http://{addr}")));
        assert!(
            client
                .update_cloud("777", 3, "My Report", "<p>updated</p>")
                .await
                .is_ok(),
            "update_cloud should succeed"
        );
    }

    #[tokio::test]
    async fn update_server_page_success() {
        let app = Router::new().route(
            "/rest/api/content/{id}",
            routing::put(|| async { Json(serde_json::json!({"id": "888"})) }),
        );
        let addr = start_mock(app).await;
        let client = ConfluenceClient::new(&server_cfg(&format!("http://{addr}")));
        assert!(
            client
                .update_server("888", 2, "My Report", "<p>updated</p>")
                .await
                .is_ok(),
            "update_server should succeed"
        );
    }

    /// Minimal `AnalysisRun` for `post_to_confluence` (`render_confluence_storage` input).
    fn tiny_run() -> sloc_core::AnalysisRun {
        sloc_core::AnalysisRun {
            tool: sloc_core::ToolMetadata {
                name: "oxide-sloc".into(),
                version: "1.5.66".into(),
                run_id: "conf-001".into(),
                timestamp_utc: chrono::Utc::now(),
            },
            environment: sloc_core::EnvironmentMetadata {
                operating_system: "linux".into(),
                architecture: "x86_64".into(),
                runtime_mode: "test".into(),
                initiator_username: "tester".into(),
                initiator_hostname: "ci".into(),
                ci_name: None,
            },
            effective_configuration: sloc_config::AppConfig::default(),
            input_roots: vec!["/test/proj".into()],
            summary_totals: sloc_core::SummaryTotals {
                files_considered: 1,
                files_analyzed: 1,
                code_lines: 42,
                ..Default::default()
            },
            totals_by_language: vec![],
            per_file_records: vec![],
            skipped_file_records: vec![],
            warnings: vec![],
            submodule_summaries: vec![],
            git_commit_short: None,
            git_commit_long: None,
            git_branch: None,
            git_commit_author: None,
            git_tags: None,
            git_nearest_tag: None,
            git_commit_date: None,
            git_remote_url: None,
            style_summary: None,
            cocomo: None,
            uloc: 0,
            dryness_pct: None,
            duplicate_groups: vec![],
            duplicates_excluded: 0,
            authors: Vec::new(),
        }
    }

    #[tokio::test]
    async fn post_to_confluence_cloud_creates_new_page() {
        // spaces lookup → page search (empty) → create.
        let app = Router::new()
            .route(
                "/wiki/api/v2/spaces",
                routing::get(|| async { Json(serde_json::json!({"results": [{"id": "sp1"}]})) }),
            )
            .route(
                "/wiki/api/v2/pages",
                routing::get(|| async { Json(serde_json::json!({"results": []})) })
                    .post(|| async { Json(serde_json::json!({"id": "page-new"})) }),
            );
        let addr = start_mock(app).await;
        let client = ConfluenceClient::new(&cloud_cfg(&format!("http://{addr}")));
        let id = post_to_confluence(&client, &tiny_run(), "My Report", Some("/runs/result/x"))
            .await
            .expect("cloud post should create a page");
        assert_eq!(id, "page-new");
    }

    #[tokio::test]
    async fn post_to_confluence_server_updates_existing_page() {
        // page search (found) → update.
        let app = Router::new()
            .route(
                "/rest/api/content",
                routing::get(|| async {
                    Json(serde_json::json!({"results": [{"id": "p9", "version": {"number": 4}}]}))
                }),
            )
            .route(
                #[allow(clippy::literal_string_with_formatting_args)]
                "/rest/api/content/{id}",
                routing::put(|| async { Json(serde_json::json!({"id": "p9"})) }),
            );
        let addr = start_mock(app).await;
        let client = ConfluenceClient::new(&server_cfg(&format!("http://{addr}")));
        let id = post_to_confluence(&client, &tiny_run(), "My Report", None)
            .await
            .expect("server post should update the page");
        assert_eq!(id, "p9");
    }

    // ── the two previously-uncovered post_to_confluence branches ──────────────

    #[tokio::test]
    async fn post_to_confluence_cloud_updates_existing_page() {
        // spaces lookup → page search (found) → update (PUT).
        let app = Router::new()
            .route(
                "/wiki/api/v2/spaces",
                routing::get(|| async { Json(serde_json::json!({"results": [{"id": "sp1"}]})) }),
            )
            .route(
                "/wiki/api/v2/pages",
                routing::get(|| async {
                    Json(serde_json::json!({"results": [{"id": "pg7", "version": {"number": 3}}]}))
                }),
            )
            .route(
                #[allow(clippy::literal_string_with_formatting_args)]
                "/wiki/api/v2/pages/{id}",
                routing::put(|| async { Json(serde_json::json!({"id": "pg7"})) }),
            );
        let addr = start_mock(app).await;
        let client = ConfluenceClient::new(&cloud_cfg(&format!("http://{addr}")));
        let id = post_to_confluence(&client, &tiny_run(), "My Report", None)
            .await
            .expect("cloud post should update the existing page");
        assert_eq!(id, "pg7");
    }

    #[tokio::test]
    async fn post_to_confluence_server_creates_new_page() {
        // page search (empty) → create (POST).
        let app = Router::new().route(
            "/rest/api/content",
            routing::get(|| async { Json(serde_json::json!({"results": []})) })
                .post(|| async { Json(serde_json::json!({"id": "srv-new"})) }),
        );
        let addr = start_mock(app).await;
        let client = ConfluenceClient::new(&server_cfg(&format!("http://{addr}")));
        let id = post_to_confluence(&client, &tiny_run(), "My Report", None)
            .await
            .expect("server post should create a new page");
        assert_eq!(id, "srv-new");
    }

    // ── handler-level coverage via the defense-in-depth re-validation path ────
    //
    // The API handlers re-validate the stored base_url before any outbound
    // request (guarding against a config written directly to disk). Seeding a
    // loopback URL directly into the store drives each handler through all of its
    // request-processing logic (config load, artifact/registry lookup, JSON read)
    // and out via the SSRF 422 branch — without weakening the guard or needing a
    // live Confluence server.

    fn loopback_cfg(auto_post: HashMap<String, bool>) -> ConfluenceConfig {
        ConfluenceConfig {
            tier: ConfluenceTier::Cloud,
            base_url: "https://127.0.0.1".to_owned(), // passes save-time shape, blocked at request time
            username: "u@x.com".to_owned(),
            credential: "tok".to_owned(),
            space_key: "DS".to_owned(),
            parent_page_id: None,
            schedule_auto_post: auto_post,
        }
    }

    #[tokio::test]
    async fn api_test_confluence_revalidates_stored_url() {
        use axum::extract::State;
        setup_tls();
        let state = crate::test_app_state("conf_didp_test");
        state.confluence.lock().await.config = Some(loopback_cfg(HashMap::new()));
        let resp = api_test_confluence(State(state)).await;
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "stored loopback URL must be rejected at test time"
        );
    }

    #[tokio::test]
    async fn api_post_to_confluence_processes_run_then_revalidates() {
        use axum::Json as AxumJson;
        use axum::extract::State;
        setup_tls();
        let state = crate::test_app_state("conf_didp_post");

        // Seed a registry entry whose JSON is a real on-disk AnalysisRun.
        let dir = tempfile::tempdir().unwrap();
        let jp = dir.path().join("result_conf-001.json");
        std::fs::write(&jp, serde_json::to_string(&tiny_run()).unwrap()).unwrap();
        let entry = crate::registry_entry_from_run(&tiny_run(), jp.clone(), jp.clone());
        state.registry.lock().await.add_entry(entry);
        state.confluence.lock().await.config = Some(loopback_cfg(HashMap::new()));

        let resp = api_post_to_confluence(
            State(state),
            AxumJson(PostToConfluenceRequest {
                run_id: "conf-001".to_owned(),
                page_title: "My Report".to_owned(),
                report_url: Some("/runs/result/conf-001".to_owned()),
            }),
        )
        .await;
        // Reached the JSON read + defense-in-depth re-validation → 422.
        assert_eq!(resp.status(), axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn maybe_auto_post_confluence_enabled_revalidates() {
        setup_tls();
        let state = crate::test_app_state("conf_didp_auto");
        let sched = uuid::Uuid::new_v4();
        let mut ap = HashMap::new();
        ap.insert(sched.to_string(), true);
        state.confluence.lock().await.config = Some(loopback_cfg(ap));
        // Enabled + loopback URL → runs through the auto-post body and returns
        // after the SSRF re-validation (must not panic).
        maybe_auto_post_confluence(&state, sched, &tiny_run(), "conf-001").await;
    }

    #[tokio::test]
    async fn maybe_auto_post_confluence_disabled_is_noop() {
        setup_tls();
        let state = crate::test_app_state("conf_didp_auto_off");
        let sched = uuid::Uuid::new_v4();
        // schedule_auto_post empty → not enabled → early return.
        state.confluence.lock().await.config = Some(loopback_cfg(HashMap::new()));
        maybe_auto_post_confluence(&state, sched, &tiny_run(), "conf-001").await;
    }

    // ── attachment upload ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn upload_attachment_server_success() {
        let app = Router::new().route(
            #[allow(clippy::literal_string_with_formatting_args)]
            "/rest/api/content/{id}/child/attachment",
            routing::post(|| async { Json(serde_json::json!({"results": [{"id": "att1"}]})) }),
        );
        let addr = start_mock(app).await;
        let client = ConfluenceClient::new(&server_cfg(&format!("http://{addr}")));
        assert!(
            client
                .upload_attachment("p1", "report.html", "text/html", b"<p>x</p>")
                .await
                .is_ok(),
            "server attachment upload should succeed on 200"
        );
    }

    #[tokio::test]
    async fn upload_attachment_cloud_forbidden_errors() {
        use axum::http::StatusCode;
        let app = Router::new().route(
            #[allow(clippy::literal_string_with_formatting_args)]
            "/wiki/rest/api/content/{id}/child/attachment",
            routing::post(|| async { StatusCode::FORBIDDEN }),
        );
        let addr = start_mock(app).await;
        let client = ConfluenceClient::new(&cloud_cfg(&format!("http://{addr}")));
        assert!(
            client
                .upload_attachment("p1", "report.pdf", "application/pdf", b"%PDF-1.4")
                .await
                .is_err(),
            "cloud attachment upload should error on 403"
        );
    }

    #[tokio::test]
    async fn attach_reports_uploads_present_files_and_skips_missing() {
        let app = Router::new().route(
            #[allow(clippy::literal_string_with_formatting_args)]
            "/rest/api/content/{id}/child/attachment",
            routing::post(|| async { Json(serde_json::json!({"results": [{"id": "att"}]})) }),
        );
        let addr = start_mock(app).await;
        let client = ConfluenceClient::new(&server_cfg(&format!("http://{addr}")));
        let dir = tempfile::tempdir().unwrap();
        let html = dir.path().join("report.html");
        std::fs::write(&html, "<p>hi</p>").unwrap();
        // HTML present → uploaded; PDF None → silently skipped. Must not panic.
        attach_reports(&client, "p1", Some(&html), None).await;
    }
}
