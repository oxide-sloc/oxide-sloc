// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>

//! Batch / queue scanning in the web UI.
//!
//! `GET /batch` renders a form where the user pastes a list of project paths
//! (one per line, optional `, profile`); `POST /batch/run` scans each project
//! server-side, writes per-project reports plus a combined index under
//! `<output_root>/batch/<id>/`, and redirects to the served index. The index
//! and per-project artifacts are served (path-safely) by
//! `GET /batch-out/{id}/{*path}`.
//!
//! The pages are rendered with plain `include_str!` + token substitution rather
//! than Askama: this crate's `lib.rs` is large enough that adding more
//! `#[derive(Template)]` expansions overflows the compiler's stack on the pinned
//! toolchain. The self-contained HTML templates live in `templates/batch*.html`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::{
    Form,
    extract::{Path as AxumPath, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;

use sloc_config::AppConfig;
use sloc_core::{
    BatchEntry, BatchIndex, BatchItemResult, ScanSummarySnapshot, analyze, parse_batch_text,
    write_json,
};
use sloc_report::{render_batch_index_html, write_csv, write_html, write_xlsx};

use super::{AppState, CspNonce, path_within_allowed_roots, resolve_output_root};

const BATCH_PAGE_HTML: &str = include_str!("../templates/batch.html");
const BATCH_ERROR_HTML: &str = include_str!("../templates/batch_error.html");

// ── GET /batch ────────────────────────────────────────────────────────────────

pub async fn batch_page_handler(
    State(state): State<AppState>,
    axum::extract::Extension(CspNonce(csp_nonce)): axum::extract::Extension<CspNonce>,
) -> impl IntoResponse {
    let (label, tip, note) = if state.server_mode {
        (
            "Server",
            "OxideSLOC is running in server mode &mdash; accessible on your LAN.",
            "In server mode, every path must resolve under a configured scan root \
             (SLOC_ALLOWED_ROOTS); projects outside are reported as failed and skipped.",
        )
    } else {
        (
            "Local",
            "OxideSLOC is running locally &mdash; only accessible from this machine.",
            "Paths are scanned on this machine. Add a comma and a profile name after a path to \
             override the default profile for that project only.",
        )
    };
    let html = BATCH_PAGE_HTML
        .replace("__CSP_NONCE__", &csp_nonce)
        .replace("__VERSION__", env!("CARGO_PKG_VERSION"))
        .replace("__SERVER_LABEL__", label)
        .replace("__SERVER_TIP__", tip)
        .replace("__SERVER_NOTE__", note);
    Html(html)
}

// ── POST /batch/run ─────────────────────────────────────────────────────────--

#[derive(Debug, Deserialize)]
pub struct BatchRunForm {
    /// Newline-separated `path[, profile]` list from the textarea.
    projects: String,
    /// Default profile applied to entries that do not name their own.
    #[serde(default)]
    profile: Option<String>,
    /// Checkbox values ("on" when ticked). JSON is always written.
    #[serde(default)]
    fmt_html: Option<String>,
    #[serde(default)]
    fmt_csv: Option<String>,
    #[serde(default)]
    fmt_xlsx: Option<String>,
    #[serde(default)]
    jobs: Option<String>,
}

#[derive(Clone, Copy)]
struct BatchFormats {
    html: bool,
    csv: bool,
    xlsx: bool,
}

pub async fn batch_run_handler(
    State(state): State<AppState>,
    axum::extract::Extension(CspNonce(csp_nonce)): axum::extract::Extension<CspNonce>,
    Form(form): Form<BatchRunForm>,
) -> Response {
    let default_profile = form
        .profile
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let entries = match parse_batch_text(&form.projects, default_profile) {
        Ok(e) => e,
        Err(e) => {
            return batch_error_page(&format!("Could not read the project list: {e}"), &csp_nonce);
        }
    };

    let fmt = BatchFormats {
        html: form.fmt_html.is_some(),
        csv: form.fmt_csv.is_some(),
        xlsx: form.fmt_xlsx.is_some(),
    };
    let jobs = form
        .jobs
        .as_deref()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, 8);

    let id = uuid::Uuid::new_v4().to_string();
    let base = resolve_output_root(None).join("batch").join(&id);
    if let Err(e) = std::fs::create_dir_all(&base) {
        return batch_error_page(
            &format!("Could not create the output directory: {e}"),
            &csp_nonce,
        );
    }

    let allowed_roots = if state.server_mode {
        Some(state.base_config.discovery.allowed_scan_roots.clone())
    } else {
        None
    };

    let sem = Arc::new(tokio::sync::Semaphore::new(jobs));
    let mut set = tokio::task::JoinSet::new();
    for (idx, entry) in entries.into_iter().enumerate() {
        let Ok(permit) = Arc::clone(&sem).acquire_owned().await else {
            return batch_error_page(
                "The batch scheduler was shut down unexpectedly.",
                &csp_nonce,
            );
        };
        let base = base.clone();
        let allowed = allowed_roots.clone();
        set.spawn_blocking(move || {
            let _permit = permit;
            (idx, scan_one(&entry, &base, fmt, allowed.as_deref()))
        });
    }

    let mut collected: Vec<(usize, BatchItemResult)> = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(pair) => collected.push(pair),
            Err(_) => return batch_error_page("A batch scan task failed to complete.", &csp_nonce),
        }
    }
    collected.sort_by_key(|(idx, _)| *idx);
    let items: Vec<BatchItemResult> = collected.into_iter().map(|(_, item)| item).collect();

    let index = BatchIndex::new(env!("CARGO_PKG_VERSION"), items);
    if let Ok(json) = serde_json::to_string_pretty(&index) {
        let _ = std::fs::write(base.join("batch-index.json"), json);
    }
    if std::fs::write(
        base.join("batch-index.html"),
        render_batch_index_html(&index),
    )
    .is_err()
    {
        return batch_error_page("Could not write the batch index report.", &csp_nonce);
    }

    Redirect::to(&format!("/batch-out/{id}/batch-index.html")).into_response()
}

/// Scan a single project into `<base>/<label>/`, enforcing allowed roots when
/// `allowed` is `Some` (server mode). Never panics; failures become a failed item.
fn scan_one(
    entry: &BatchEntry,
    base: &Path,
    fmt: BatchFormats,
    allowed: Option<&[PathBuf]>,
) -> BatchItemResult {
    let outcome = (|| -> anyhow::Result<(ScanSummarySnapshot, Vec<String>)> {
        // Server mode: refuse paths outside the configured scan roots.
        if let Some(roots) = allowed {
            let canonical = std::fs::canonicalize(&entry.path)
                .map_err(|_| anyhow::anyhow!("path could not be resolved to a real directory"))?;
            if !path_within_allowed_roots(&canonical, roots) {
                anyhow::bail!(
                    "path is outside the server's allowed scan roots (SLOC_ALLOWED_ROOTS)"
                );
            }
        }

        let mut config = AppConfig::default();
        config.discovery.root_paths = vec![entry.path.clone()];
        if let Some(profile) = &entry.profile {
            config.apply_profile(profile)?;
        }
        config.validate()?;
        let run = analyze(&config, "batch", None, None)?;

        let dir = base.join(&entry.label);
        std::fs::create_dir_all(&dir)?;
        let mut artifacts = Vec::new();
        write_json(&run, &dir.join("result.json"))?;
        artifacts.push(format!("{}/result.json", entry.label));
        if fmt.html {
            write_html(&run, &dir.join("report.html"))?;
            artifacts.push(format!("{}/report.html", entry.label));
        }
        if fmt.csv {
            write_csv(&run, &dir.join("report.csv"))?;
            artifacts.push(format!("{}/report.csv", entry.label));
        }
        if fmt.xlsx {
            write_xlsx(&run, &dir.join("report.xlsx"))?;
            artifacts.push(format!("{}/report.xlsx", entry.label));
        }
        Ok((ScanSummarySnapshot::from(&run.summary_totals), artifacts))
    })();

    let (ok, error, summary, artifacts) = match outcome {
        Ok((summary, artifacts)) => (true, None, Some(summary), artifacts),
        Err(e) => (false, Some(e.to_string()), None, Vec::new()),
    };
    BatchItemResult {
        label: entry.label.clone(),
        path: entry.path.display().to_string(),
        profile: entry.profile.clone(),
        ok,
        error,
        summary,
        artifacts,
    }
}

// ── GET /batch-out/{*path} ──────────────────────────────────────────────────--

/// Serve a file produced by a batch run, scoped to `<output_root>/batch/`.
/// Path-safe: the resolved target must stay inside the batch output tree.
pub async fn batch_artifact_handler(AxumPath(tail): AxumPath<String>) -> Response {
    let root = resolve_output_root(None).join("batch");
    let candidate = root.join(&tail);

    // Reject traversal: the canonical target must stay under the batch root.
    let (Ok(canonical), Ok(root_canonical)) = (
        std::fs::canonicalize(&candidate),
        std::fs::canonicalize(&root),
    ) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !canonical.starts_with(&root_canonical) {
        return StatusCode::NOT_FOUND.into_response();
    }

    let bytes = match std::fs::read(&canonical) {
        Ok(b) => b,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let content_type = match canonical.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("json") => "application/json",
        Some("csv") => "text/csv",
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => "application/octet-stream",
    };
    ([(header::CONTENT_TYPE, content_type)], bytes).into_response()
}

/// Minimal full-chrome error page shared by the POST handler failure paths.
fn batch_error_page(message: &str, csp_nonce: &str) -> Response {
    let safe = message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let html = BATCH_ERROR_HTML
        .replace("__CSP_NONCE__", csp_nonce)
        .replace("__VERSION__", env!("CARGO_PKG_VERSION"))
        .replace("__MESSAGE__", &safe);
    (StatusCode::BAD_REQUEST, Html(html)).into_response()
}
