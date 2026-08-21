// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>
//
// Git browser: browse branches/tags/commits of a local or remote repo and
// trigger scans or ref-to-ref comparisons directly from the web UI.

use std::path::Path;

use askama::Template;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Json},
};
use serde::Deserialize;

use sloc_git::{RepoRefs, clone_or_fetch, create_worktree, destroy_worktree, list_refs};

use sloc_report::render_html;

use super::{
    AppState, CspNonce, RunArtifacts, build_run_registry_entry, git_clone_dest,
    sanitize_project_label, scan_path_to_artifacts,
};

// ── query types ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct GitBrowserQuery {
    pub repo: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ScanRefQuery {
    pub repo: String,
    pub ref_name: String,
    pub label: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CompareRefsQuery {
    pub repo: String,
    pub baseline_ref: String,
    pub current_ref: String,
    pub label: Option<String>,
}

// ── template ──────────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(
    source = r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>OxideSLOC — Git Browser</title>
  <link rel="icon" type="image/png" href="/images/logo/small-logo.png">
  <style nonce="{{ csp_nonce }}">
    :root{--radius:14px;--bg:#f5efe8;--surface:rgba(255,255,255,0.9);--surface-2:#fbf7f2;--line:#e6d0bf;--line-strong:#d8bfad;--text:#43342d;--muted:#7b675b;--nav:#b85d33;--nav-2:#7a371b;--oxide:#d37a4c;--oxide-2:#b85d33;--accent-2:#2563eb;--shadow:0 8px 24px rgba(77,44,20,0.10);}
    body.dark-theme{--bg:#1b1511;--surface:#261c17;--surface-2:#2d221d;--line:#524238;--text:#f5ece6;--muted:#c7b7aa;--shadow:0 8px 24px rgba(0,0,0,0.32);}
    *{box-sizing:border-box;} html,body{margin:0;min-height:100vh;font-family:Inter,ui-sans-serif,system-ui,-apple-system,sans-serif;background:var(--bg);color:var(--text);} body{display:flex;flex-direction:column;}
    .background-watermarks{position:fixed;inset:0;pointer-events:none;z-index:0;overflow:hidden;}
    .background-watermarks img{position:absolute;opacity:0.16;filter:blur(0.3px);user-select:none;max-width:none;}
    .code-particles{position:fixed;inset:0;pointer-events:none;z-index:0;overflow:hidden;}
    .code-particle{position:absolute;font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:11px;font-weight:600;color:var(--oxide);opacity:0;white-space:nowrap;user-select:none;animation:floatCode linear infinite;}
    @keyframes floatCode{0%{opacity:0;transform:translateY(0) rotate(var(--rot));}10%{opacity:var(--op);}85%{opacity:var(--op);}100%{opacity:0;transform:translateY(-200px) rotate(var(--rot));}}
    .top-nav{position:sticky;top:0;z-index:30;background:linear-gradient(180deg,var(--nav),var(--nav-2));border-bottom:1px solid rgba(255,255,255,0.12);box-shadow:0 4px 14px rgba(0,0,0,0.18);}
    .top-nav-inner{max-width:1720px;margin:0 auto;padding:4px 24px;min-height:56px;display:flex;align-items:center;gap:14px;}
    .brand{display:flex;align-items:center;gap:12px;text-decoration:none;}
    .brand-logo{width:36px;height:40px;object-fit:contain;flex:0 0 auto;}
    .brand-title{color:#fff;font-size:16px;font-weight:800;}.brand-sub{color:rgba(255,255,255,0.75);font-size:12px;}
    .nav-right{margin-left:auto;display:flex;align-items:center;gap:10px;}
    @media (max-width: 1400px) { .nav-right { gap: 6px; } .nav-pill, .nav-dropdown-btn, .theme-toggle { padding: 0 10px; } }
    @media (max-width: 1150px) { .nav-right { gap: 4px; } .nav-pill, .nav-dropdown-btn, .theme-toggle { padding: 0 8px; font-size: 11px; min-height: 34px; } .brand-sub { display: none; } .server-online-pill { width: 34px; padding: 0; justify-content: center; font-size: 0; gap: 0; min-height: 34px; } }
    .nav-pill{display:inline-flex;align-items:center;min-height:34px;padding:0 14px;border-radius:999px;border:1px solid rgba(255,255,255,0.18);color:#fff;background:rgba(255,255,255,0.08);font-size:12px;font-weight:700;text-decoration:none;}
    .nav-pill:hover{background:rgba(255,255,255,0.18);}
    .status-dot{width:8px;height:8px;border-radius:999px;background:#26d768;box-shadow:0 0 0 4px rgba(38,215,104,0.14);flex:0 0 auto;}
    .server-status-wrap{position:relative;display:inline-flex;}.server-online-pill{cursor:default;gap:7px;}.server-status-tip{visibility:hidden;opacity:0;pointer-events:none;position:absolute;top:calc(100% + 10px);right:0;z-index:100;background:rgba(20,12,8,0.97);color:rgba(255,255,255,0.92);border-radius:10px;padding:10px 14px;font-size:12px;font-weight:500;line-height:1.55;white-space:nowrap;box-shadow:0 8px 24px rgba(0,0,0,0.32);border:1px solid rgba(255,255,255,0.10);transition:opacity 0.15s ease;}.server-status-tip::before{content:'';position:absolute;bottom:100%;right:18px;border:6px solid transparent;border-bottom-color:rgba(20,12,8,0.97);}.server-status-wrap:hover .server-status-tip{visibility:visible;opacity:1;pointer-events:auto;}
    .nav-dropdown{position:relative;display:inline-flex;}.nav-dropdown-btn{cursor:pointer;background:rgba(255,255,255,0.08);border:1px solid rgba(255,255,255,0.18);color:#fff;border-radius:999px;padding:0 14px;min-height:34px;font-size:12px;font-weight:700;display:inline-flex;align-items:center;gap:6px;text-decoration:none;}.nav-dropdown-btn:hover,.nav-dropdown:focus-within .nav-dropdown-btn{background:rgba(255,255,255,0.18);}.nav-dropdown-menu{opacity:0;visibility:hidden;position:absolute;top:calc(100% + 8px);right:0;background:linear-gradient(180deg,var(--nav),var(--nav-2));border:1px solid rgba(255,255,255,0.15);border-radius:12px;min-width:165px;overflow:hidden;box-shadow:0 10px 28px rgba(0,0,0,0.28);z-index:100;transition:opacity 0.13s ease,visibility 0s ease 0.13s;}.nav-dropdown:hover .nav-dropdown-menu,.nav-dropdown:focus-within .nav-dropdown-menu{opacity:1;visibility:visible;transition:opacity 0.13s ease,visibility 0s ease 0s;}.nav-dropdown-menu a{display:flex;align-items:center;gap:9px;padding:11px 16px;color:rgba(255,255,255,0.92);text-decoration:none;font-size:12px;font-weight:700;border-bottom:1px solid rgba(255,255,255,0.10);}.nav-dropdown-menu a:last-child{border-bottom:none;}.nav-dropdown-menu a:hover{background:rgba(255,255,255,0.14);color:#fff;}.nav-dropdown-menu a svg{width:13px;height:13px;stroke:currentColor;fill:none;stroke-width:2;flex:0 0 auto;}
    .page{width:100%;max-width:1720px;margin:0 auto;padding:32px 24px 36px;position:relative;z-index:1;}
    @media (max-width:1920px) { .top-nav-inner { max-width:1500px; } .page { max-width:1500px; } }
    h1{font-size:26px;font-weight:850;margin:0 0 6px;letter-spacing:-0.03em;}
    .subtitle{color:var(--muted);font-size:14px;margin:0 0 28px;}
    .card{background:var(--surface);border:1px solid var(--line);border-radius:var(--radius);padding:24px;box-shadow:var(--shadow);margin-bottom:20px;}
    .card-title{font-size:15px;font-weight:800;margin:0 0 16px;}
    .repo-bar{display:flex;gap:10px;align-items:center;}
    .repo-input{flex:1;padding:10px 14px;border-radius:9px;border:1.5px solid var(--line-strong);background:var(--surface-2);color:var(--text);font-size:14px;}
    .repo-input:focus{outline:none;border-color:var(--oxide);}
    .btn{display:inline-flex;align-items:center;gap:7px;padding:9px 18px;border-radius:9px;border:none;cursor:pointer;font-size:13px;font-weight:700;transition:opacity 0.15s;}
    .btn:hover{opacity:0.85;}.btn-primary{background:var(--oxide-2);color:#fff;}.btn-sm{padding:5px 12px;font-size:12px;border-radius:7px;}
    .btn-compare{background:linear-gradient(135deg,#7c3aed,#6d28d9);color:#fff;}
    .tabs{display:flex;gap:2px;border-bottom:2px solid var(--line);}
    .tab{padding:10px 20px;font-size:13px;font-weight:700;cursor:pointer;border-radius:8px 8px 0 0;border:none;background:none;color:var(--muted);}
    .tab.active{background:var(--oxide-2);color:#fff;}
    .tab-pane{display:none;padding-top:16px;}.tab-pane.active{display:block;}
    .ref-table{width:100%;border-collapse:collapse;font-size:13px;table-layout:fixed;}
    .ref-table col.col-check{width:40px;}
    .ref-table col.col-name{width:22%;}
    .ref-table col.col-sha{width:88px;}
    .ref-table col.col-date{width:92px;}
    .ref-table col.col-msg{width:auto;}
    .ref-table col.col-actions{width:96px;}
    .ref-table th{text-align:left;padding:8px 12px;color:var(--muted);font-weight:700;font-size:11px;text-transform:uppercase;letter-spacing:.06em;border-bottom:2px solid var(--line);}
    .ref-table td{padding:9px 12px;border-bottom:1px solid var(--line);vertical-align:middle;}
    .ref-table th:first-child,.ref-table td:first-child{padding-left:10px;padding-right:4px;}
    .ref-table th:last-child,.ref-table td:last-child{text-align:right;padding-right:16px;}
    .ref-table td.col-msg-cell{overflow:hidden;text-overflow:ellipsis;white-space:nowrap;}
    .ref-table tbody tr{cursor:pointer;}.ref-table tr:hover td{background:var(--surface-2);}
    .sha-badge{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:11px;background:var(--surface-2);border:1px solid var(--line);border-radius:5px;padding:2px 7px;color:var(--muted);}
    .kind-badge{font-size:10px;font-weight:700;padding:2px 8px;border-radius:999px;letter-spacing:.05em;}
    .kind-branch{background:#dcfce7;color:#166534;}.kind-tag{background:#ede9fe;color:#5b21b6;}
    body.dark-theme .kind-branch{background:#14532d;color:#86efac;}body.dark-theme .kind-tag{background:#2e1065;color:#c4b5fd;}
    .btn-scan{width:62px;white-space:nowrap;overflow:hidden;justify-content:center;}
    .compare-check{width:16px;height:16px;cursor:pointer;accent-color:var(--oxide);}
    .compare-bar{display:flex;align-items:center;gap:12px;padding:12px 16px;background:var(--surface-2);border-radius:10px;border:1px solid var(--line);margin-top:14px;}
    .status-msg{padding:12px 16px;border-radius:9px;font-size:13px;font-weight:600;margin-top:12px;}
    .status-ok{background:#dcfce7;color:#166534;border:1px solid #86efac;}
    .status-err{background:#fee2e2;color:#991b1b;border:1px solid #fca5a5;}
    body.dark-theme .status-ok{background:#14532d;color:#86efac;border-color:#166534;}
    body.dark-theme .status-err{background:#450a0a;color:#fca5a5;border-color:#991b1b;}
    .spinner{display:inline-block;width:14px;height:14px;border:2px solid rgba(255,255,255,0.3);border-top-color:#fff;border-radius:50%;animation:spin 0.7s linear infinite;vertical-align:middle;flex-shrink:0;}
    @keyframes spin{to{transform:rotate(360deg);}}
    .skeleton-panel{background:var(--surface);border:1px solid var(--line);border-radius:var(--radius);padding:24px;box-shadow:var(--shadow);margin-bottom:20px;}
    .loading-info{display:flex;align-items:center;gap:10px;padding:0 0 18px;font-size:13px;font-weight:700;color:var(--oxide);}
    .loading-spinner{display:inline-block;width:16px;height:16px;border:2.5px solid rgba(211,122,76,0.25);border-top-color:var(--oxide);border-radius:50%;animation:spin 0.75s linear infinite;flex-shrink:0;}
    .sk{border-radius:6px;background:linear-gradient(90deg,var(--surface-2) 0%,var(--line) 50%,var(--surface-2) 100%);background-size:400% 100%;animation:shimmer 1.5s ease infinite;}
    @keyframes shimmer{0%{background-position:100% 0;}100%{background-position:-100% 0;}}
    .sk-tabs{display:flex;gap:4px;margin-bottom:16px;border-bottom:2px solid var(--line);padding-bottom:0;}
    .sk-tab{height:38px;width:90px;border-radius:8px 8px 0 0;}
    .sk-row{display:flex;align-items:center;gap:10px;padding:10px 12px;border-bottom:1px solid var(--line);}
    .sk-sm{height:13px;width:55px;}.sk-md{height:13px;width:110px;}.sk-lg{height:13px;flex:1;min-width:60px;}.sk-badge{height:20px;width:54px;border-radius:999px;}.sk-btn{height:26px;width:52px;border-radius:7px;}
    .date-cell{color:var(--muted);font-size:12px;}
    .panel-hidden{display:none !important;}
    .empty-state{text-align:center;padding:44px 12px;color:var(--muted);font-size:13px;font-style:italic;}
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
    /* ── Page header ── */
    .page-header{display:flex;align-items:flex-start;gap:16px;}
    .page-header-icon{flex:0 0 auto;width:44px;height:44px;border-radius:11px;background:linear-gradient(135deg,var(--oxide),var(--nav-2));display:flex;align-items:center;justify-content:center;}
    .page-header-icon svg{width:22px;height:22px;stroke:#fff;fill:none;stroke-width:2;}
    .page-header-body{flex:1;min-width:0;}
    .page-header-title{font-size:24px;font-weight:900;margin:0 0 5px;letter-spacing:-0.03em;color:var(--text);}
    .page-header-sub{font-size:13px;color:var(--muted);margin:0 0 11px;line-height:1.55;}
    .page-header-chips{display:flex;flex-wrap:wrap;gap:7px;}
    .page-header-chip{display:inline-flex;align-items:center;gap:5px;background:var(--surface-2);border:1px solid var(--line);color:var(--muted);font-size:11px;font-weight:700;padding:3px 9px;border-radius:999px;}
    .page-header-chip svg{width:10px;height:10px;stroke:currentColor;fill:none;stroke-width:2.5;flex:0 0 auto;}
    /* ── Workflow steps ── */
    .how-row{display:grid;grid-template-columns:1fr auto 1fr auto 1fr;gap:10px;align-items:center;margin-bottom:20px;}
    .how-step{background:var(--surface);border:1px solid var(--line);border-radius:var(--radius);padding:15px 16px;display:flex;align-items:center;gap:13px;box-shadow:var(--shadow);cursor:default;transition:transform 0.18s ease,box-shadow 0.18s ease,border-color 0.18s ease;}
    .how-step:hover{transform:translateY(-3px);box-shadow:0 8px 22px rgba(0,0,0,0.13);border-color:var(--oxide);}
    .how-step-num{flex:0 0 auto;width:30px;height:30px;border-radius:50%;background:linear-gradient(135deg,var(--oxide),var(--nav-2));color:#fff;font-size:13px;font-weight:900;display:flex;align-items:center;justify-content:center;transition:transform 0.18s ease;}
    .how-step:hover .how-step-num{transform:scale(1.12);}
    .how-step-body{min-width:0;}
    .how-step-label{font-size:13px;font-weight:800;color:var(--text);}
    .how-step-desc{font-size:11px;color:var(--muted);margin-top:2px;}
    .how-arrow{color:var(--muted);font-size:20px;font-weight:200;text-align:center;padding:0 2px;opacity:0.6;}
    @media(max-width:700px){.how-row{grid-template-columns:1fr;}.how-arrow{display:none;}}
    /* ── URL card ── */
    .fetch-card-header{display:flex;align-items:flex-start;gap:13px;margin-bottom:16px;}
    .fetch-card-icon{flex:0 0 auto;width:40px;height:40px;border-radius:10px;background:linear-gradient(135deg,var(--oxide),var(--nav-2));display:flex;align-items:center;justify-content:center;}
    .fetch-card-icon svg{width:19px;height:19px;stroke:#fff;fill:none;stroke-width:2;}
    .fetch-card-title{font-size:15px;font-weight:800;margin:0 0 2px;}
    .fetch-card-desc{font-size:12px;color:var(--muted);margin:0;}
    .provider-row{display:flex;gap:7px;flex-wrap:wrap;margin-bottom:13px;}
    .provider-pill{display:inline-flex;align-items:center;gap:5px;padding:4px 10px;border-radius:999px;font-size:11px;font-weight:700;border:1.5px solid transparent;user-select:none;}
    .pp-github{background:#24292e12;color:#24292e;border-color:#24292e28;}body.dark-theme .pp-github{background:#ffffff10;color:#cdd9e5;border-color:#ffffff22;}
    .pp-gitlab{background:#fc6d2612;color:#b84800;border-color:#fc6d2630;}body.dark-theme .pp-gitlab{background:#fc6d2612;color:#fc9c6a;border-color:#fc6d2640;}
    .pp-bitbucket{background:#0052cc12;color:#003d99;border-color:#0052cc28;}body.dark-theme .pp-bitbucket{background:#0052cc15;color:#5b8fc9;border-color:#0052cc35;}
    .input-with-icon{position:relative;flex:1;min-width:0;}
    .input-with-icon .repo-input{width:100%;}
    .input-icon-prefix{position:absolute;left:11px;top:50%;transform:translateY(-50%);pointer-events:none;color:var(--muted);}
    .input-icon-prefix svg{width:15px;height:15px;stroke:currentColor;fill:none;stroke-width:2;}
    .repo-input-padded{padding-left:34px;}
    .card.fetch-card{padding-bottom:0;}
    .fetch-footer{margin:14px -24px 0;padding:12px 18px 12px 16px;background:var(--surface-2);border-top:1px solid var(--line);border-radius:0 0 var(--radius) var(--radius);font-size:12px;color:var(--muted);line-height:1.5;display:flex;gap:10px;align-items:flex-start;}
    .fetch-footer-icon{flex-shrink:0;margin-top:1px;color:var(--oxide);opacity:0.75;}
    .fetch-footer-icon svg{display:block;width:15px;height:15px;}
    .fetch-footer-body{flex:1;}
    .fetch-footer-body span+span{display:block;margin-top:4px;}
    .site-footer{text-align:center;padding:12px 24px;font-size:13px;color:var(--muted);position:relative;z-index:1;}
    .site-footer a{color:var(--muted);}
    /* ── Tabs with icons ── */
    .tab-inner{display:inline-flex;align-items:center;gap:6px;}
    .tab-inner svg{width:13px;height:13px;stroke:currentColor;fill:none;stroke-width:2;opacity:0.65;flex:0 0 auto;}
    .tab.active .tab-inner svg{opacity:1;}
    .tab{display:inline-flex;align-items:center;}
    .tab-count{background:rgba(255,255,255,0.22);color:rgba(255,255,255,0.9);font-size:10px;font-weight:800;padding:1px 6px;border-radius:999px;line-height:1;min-width:18px;text-align:center;display:none;margin-left:3px;vertical-align:middle;}
    .tab-count.has-data{display:inline-flex;align-items:center;justify-content:center;}
    .tab:not(.active) .tab-count{background:rgba(0,0,0,0.09);color:var(--muted);}
    body.dark-theme .tab:not(.active) .tab-count{background:rgba(255,255,255,0.09);color:var(--muted);}
    /* ── Ref panel header (loaded repo) ── */
    .ref-panel-topbar{display:none;align-items:center;justify-content:space-between;padding:0 4px 14px;border-bottom:1px solid var(--line);margin-bottom:0;}
    .ref-panel-topbar.visible{display:flex;}
    .ref-panel-repo-label{font-size:11px;font-weight:700;color:var(--muted);text-transform:uppercase;letter-spacing:.06em;margin-bottom:2px;}
    .ref-panel-repo-url{font-size:12px;font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;color:var(--text);overflow:hidden;text-overflow:ellipsis;white-space:nowrap;max-width:480px;}
    .ref-panel-badge{display:inline-flex;align-items:center;gap:5px;background:#dcfce7;color:#166534;border:1px solid #86efac;font-size:11px;font-weight:700;padding:4px 10px;border-radius:999px;}
    body.dark-theme .ref-panel-badge{background:#14532d;color:#86efac;border-color:#166534;}
    .ref-panel-badge svg{width:11px;height:11px;stroke:currentColor;fill:none;stroke-width:2.5;}
    /* ── Rich empty state ── */
    .empty-rich{text-align:center;padding:52px 20px 42px;}
    .empty-rich-icon{width:52px;height:52px;border-radius:14px;background:var(--surface-2);border:2px solid var(--line);display:inline-flex;align-items:center;justify-content:center;margin-bottom:13px;}
    .empty-rich-icon svg{width:26px;height:26px;stroke:var(--muted);fill:none;stroke-width:1.5;}
    .empty-rich-title{font-size:14px;font-weight:700;color:var(--text);margin:0 0 5px;}
    .empty-rich-desc{font-size:12px;color:var(--muted);max-width:340px;margin:0 auto;line-height:1.5;}
    /* ── Compare bar ── */
    .compare-bar{display:flex;align-items:center;gap:12px;padding:13px 18px;background:linear-gradient(135deg,rgba(124,58,237,0.07),rgba(99,40,217,0.05));border-radius:10px;border:1.5px solid rgba(124,58,237,0.22);margin-top:16px;}
    .compare-refs-label{display:flex;align-items:center;gap:7px;font-size:13px;color:var(--muted);flex:1;min-width:0;flex-wrap:wrap;}
    .compare-refs-label svg{width:13px;height:13px;stroke:var(--muted);fill:none;stroke-width:2;flex-shrink:0;}
    .compare-ref-tag{background:rgba(124,58,237,0.1);color:#5b21b6;border:1px solid rgba(124,58,237,0.22);border-radius:5px;padding:2px 8px;font-size:12px;font-weight:700;font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;white-space:nowrap;overflow:hidden;max-width:160px;text-overflow:ellipsis;display:inline-block;vertical-align:middle;}
    body.dark-theme .compare-ref-tag{background:rgba(167,139,250,0.1);color:#c4b5fd;border-color:rgba(167,139,250,0.22);}
    .compare-vs{font-size:11px;font-weight:600;color:var(--muted);}
    /* ── Pagination ── */
    .pag-bar{display:flex;align-items:center;gap:6px;padding:10px 14px;border-top:1px solid var(--line);flex-wrap:wrap;min-height:46px;}
    .pag-info{font-size:12px;color:var(--muted);white-space:nowrap;margin-right:auto;}
    .pag-size-wrap{display:flex;align-items:center;gap:5px;font-size:12px;color:var(--muted);white-space:nowrap;}
    .pag-size-select{font-size:12px;padding:3px 6px;border-radius:6px;border:1.5px solid var(--line-strong);background:var(--surface-2);color:var(--text);cursor:pointer;outline:none;}
    .pag-size-select:focus{border-color:var(--oxide);}
    .pag-nav{display:flex;align-items:center;gap:3px;}
    .pag-btn{min-width:30px;height:28px;padding:0 9px;border-radius:6px;border:1.5px solid var(--line);background:var(--surface-2);color:var(--text);font-size:12px;font-weight:700;cursor:pointer;display:inline-flex;align-items:center;justify-content:center;transition:background 0.12s,color 0.12s,border-color 0.12s;}
    .pag-btn:hover:not(:disabled):not(.active){background:var(--surface);border-color:var(--oxide);color:var(--oxide);}
    .pag-btn.active{background:var(--oxide-2);color:#fff;border-color:var(--oxide-2);}
    .pag-btn:disabled{opacity:0.35;cursor:default;}
    .pag-ellipsis{font-size:13px;color:var(--muted);padding:0 3px;line-height:28px;}
    /* ── Fetch error state (shown in table bodies on failed load) ── */
    .fetch-error-state{text-align:center;padding:44px 20px 38px;}
    .fetch-error-icon-wrap{display:inline-flex;width:52px;height:52px;border-radius:14px;background:rgba(220,38,38,0.08);border:2px solid rgba(220,38,38,0.2);align-items:center;justify-content:center;margin-bottom:13px;}
    .fetch-error-icon-wrap svg{width:26px;height:26px;stroke:#dc2626;fill:none;stroke-width:1.5;}
    body.dark-theme .fetch-error-icon-wrap{background:rgba(220,38,38,0.12);border-color:rgba(220,38,38,0.3);}
    body.dark-theme .fetch-error-icon-wrap svg{stroke:#f87171;}
    .fetch-error-title{font-size:14px;font-weight:700;color:var(--text);margin-bottom:9px;}
    .fetch-error-msg{font-size:12px;color:#991b1b;font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;background:#fee2e2;border:1px solid #fca5a5;border-radius:7px;padding:9px 13px;max-width:560px;margin:0 auto 12px;text-align:left;white-space:pre-wrap;word-break:break-all;line-height:1.55;}
    body.dark-theme .fetch-error-msg{color:#fca5a5;background:#450a0a;border-color:#991b1b;}
    .fetch-error-hints{font-size:12px;color:var(--muted);max-width:560px;margin:0 auto;text-align:left;background:var(--surface-2);border:1px solid var(--line);border-radius:7px;padding:10px 13px;line-height:1.6;}
    .fetch-error-hints code{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:11px;background:var(--line);padding:1px 5px;border-radius:4px;word-break:break-all;}
    .fetch-error-hints b{color:var(--text);}
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
        <div><div class="brand-title">OxideSLOC</div><div class="brand-sub">Git Browser</div></div></a>
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
          <a href="/git-browser" class="nav-dropdown-btn" style="background:rgba(255,255,255,0.22);">Git Browser <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="6 9 12 15 18 9"></polyline></svg></a>
          <div class="nav-dropdown-menu">
            <a href="/integrations"><svg viewBox="0 0 24 24"><path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z"></path></svg>Integrations</a>
          </div>
        </div>
        <div class="server-status-wrap" id="server-status-wrap">
          <div class="nav-pill server-online-pill" id="server-status-pill">
            <span class="status-dot" id="status-dot"></span>
            <span id="server-status-label">Server</span>
            <span id="server-ping-ms" style="margin-left:5px;opacity:0.75;font-size:10px;"></span>
          </div>
          <div class="server-status-tip">
            OxideSLOC is running — accessible on your network.
            <span id="server-tip-ping" style="display:block;margin-top:4px;font-size:11px;opacity:0.75;"></span>
          </div>
        </div>
        <button type="button" class="theme-toggle" id="settings-btn" aria-label="Color scheme" title="Color scheme settings">
          <svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.8"><circle cx="12" cy="12" r="3"></circle><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path></svg>
        </button>
        <button class="theme-toggle" id="theme-toggle" title="Toggle theme" type="button">
          <svg class="icon-moon" viewBox="0 0 24 24"><path d="M21 12.79A9 9 0 1 1 11.21 3a7 7 0 0 0 9.79 9.79z"/></svg>
          <svg class="icon-sun" viewBox="0 0 24 24"><circle cx="12" cy="12" r="5"/><line x1="12" y1="1" x2="12" y2="3"/><line x1="12" y1="21" x2="12" y2="23"/><line x1="4.22" y1="4.22" x2="5.64" y2="5.64"/><line x1="18.36" y1="18.36" x2="19.78" y2="19.78"/><line x1="1" y1="12" x2="3" y2="12"/><line x1="21" y1="12" x2="23" y2="12"/><line x1="4.22" y1="19.78" x2="5.64" y2="18.36"/><line x1="18.36" y1="5.64" x2="19.78" y2="4.22"/></svg>
        </button>
      </div>
    </div>
  </nav>
  <div class="page">
    <div class="card">
      <div class="page-header">
        <div class="page-header-icon" aria-hidden="true">
          <svg viewBox="0 0 24 24"><line x1="6" y1="3" x2="6" y2="15"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M18 9a9 9 0 0 1-9 9"/><circle cx="6" cy="6" r="3"/></svg>
        </div>
        <div class="page-header-body">
          <h1 class="page-header-title">Git Browser</h1>
          <p class="page-header-sub">Browse branches, tags, commits, and releases from any GitHub, GitLab, or Bitbucket repository — then run a point-in-time SLOC scan or compare any two refs side-by-side.</p>
          <div class="page-header-chips">
            <span class="page-header-chip"><svg viewBox="0 0 24 24"><polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/></svg>Branches &amp; Tags</span>
            <span class="page-header-chip"><svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg>Commit History</span>
            <span class="page-header-chip"><svg viewBox="0 0 24 24"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>Point-in-time SLOC</span>
            <span class="page-header-chip"><svg viewBox="0 0 24 24"><line x1="18" y1="20" x2="18" y2="10"/><line x1="12" y1="20" x2="12" y2="4"/><line x1="6" y1="20" x2="6" y2="14"/></svg>Side-by-side Compare</span>
          </div>
        </div>
      </div>
    </div>
    <div class="how-row">
      <div class="how-step">
        <div class="how-step-num">1</div>
        <div class="how-step-body"><div class="how-step-label">Paste a URL</div><div class="how-step-desc">GitHub, GitLab, Bitbucket, or a local path</div></div>
      </div>
      <div class="how-arrow">&#8594;</div>
      <div class="how-step">
        <div class="how-step-num">2</div>
        <div class="how-step-body"><div class="how-step-label">Browse refs</div><div class="how-step-desc">Explore branches, tags, or commits</div></div>
      </div>
      <div class="how-arrow">&#8594;</div>
      <div class="how-step">
        <div class="how-step-num">3</div>
        <div class="how-step-body"><div class="how-step-label">Scan or compare</div><div class="how-step-desc">Get SLOC counts or diff two refs</div></div>
      </div>
    </div>
    <div class="card fetch-card" style="overflow:hidden;">
      <div class="fetch-card-header">
        <div class="fetch-card-icon" aria-hidden="true">
          <svg viewBox="0 0 24 24"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="17 8 12 3 7 8"/><line x1="12" y1="3" x2="12" y2="15"/></svg>
        </div>
        <div>
          <div class="fetch-card-title">Load from Version Control</div>
          <p class="fetch-card-desc">Paste a repository URL, then click <strong>Fetch</strong> to browse its branches, tags, and commits.</p>
        </div>
      </div>
      <div class="provider-row">
        <span class="provider-pill pp-github"><svg viewBox="0 0 24 24" width="12" height="12" fill="currentColor"><path d="M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12"/></svg>GitHub</span>
        <span class="provider-pill pp-gitlab"><svg viewBox="0 0 24 24" width="12" height="12" fill="currentColor"><path d="M22.65 14.39L12 22.13 1.35 14.39a.84.84 0 0 1-.3-.94l1.22-3.78 2.44-7.51A.42.42 0 0 1 4.82 2a.43.43 0 0 1 .58 0 .42.42 0 0 1 .11.18l2.44 7.49h8.1l2.44-7.51A.42.42 0 0 1 18.6 2a.43.43 0 0 1 .58 0 .42.42 0 0 1 .11.18l2.44 7.51L23 13.45a.84.84 0 0 1-.35.94z"/></svg>GitLab</span>
        <span class="provider-pill pp-bitbucket"><svg viewBox="0 0 24 24" width="12" height="12" fill="currentColor"><path d="M.778 1.213a.768.768 0 0 0-.768.892l3.263 19.81c.084.5.515.868 1.022.873H19.95a.772.772 0 0 0 .77-.646l3.27-20.03a.768.768 0 0 0-.768-.891zM14.52 15.53H9.522L8.17 8.466h7.561z"/></svg>Bitbucket</span>
      </div>
      <div class="repo-bar">
        <div class="input-with-icon">
          <div class="input-icon-prefix" aria-hidden="true"><svg viewBox="0 0 24 24"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg></div>
          <input id="repoInput" class="repo-input repo-input-padded" type="text"
                 placeholder="https://github.com/owner/repo  ·  https://gitlab.com/group/repo"
                 value="{{ repo_url }}" />
        </div>
        <button class="btn btn-primary" id="loadBtn" type="button">
          <span id="loadSpinner" class="spinner panel-hidden"></span>
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" style="flex-shrink:0"><polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/></svg>
          Fetch
        </button>
      </div>
      <div id="statusMsg" style="display:none" class="status-msg"></div>
      <div class="fetch-footer">
        <span class="fetch-footer-icon" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/></svg></span>
        <span class="fetch-footer-body">
          <span>First fetch clones the repository — this may take 15-30 seconds for large repos. Subsequent fetches for the same URL are instant (cached). Browse URLs (e.g. <code style="font-family:ui-monospace,monospace;font-size:11px">/projects/PROJ/repos/REPO/browse</code>) are automatically converted to git clone URLs.</span>
          <span>Public repos work without credentials; for private repos, configure your SSH or HTTPS credentials in git before fetching. Corporate proxy/VPN certificates are trusted automatically from your OS certificate store — no extra setup. Behind an outbound proxy, set <code style="font-family:ui-monospace,monospace;font-size:11px">HTTPS_PROXY</code>/<code style="font-family:ui-monospace,monospace;font-size:11px">HTTP_PROXY</code>.</span>
        </span>
      </div>
    </div>
    <div id="loadingPanel" class="skeleton-panel panel-hidden">
      <div class="loading-info">
        <span class="loading-spinner"></span>
        Loading repository…
      </div>
      <div class="sk-tabs">
        <div class="sk sk-tab"></div>
        <div class="sk sk-tab" style="width:60px;"></div>
        <div class="sk sk-tab" style="width:80px;"></div>
      </div>
      <div class="sk-row"><div class="sk sk-sm"></div><div class="sk sk-badge"></div><div class="sk sk-md"></div><div class="sk sk-md"></div><div class="sk sk-lg"></div><div class="sk sk-btn"></div></div>
      <div class="sk-row" style="opacity:0.75;"><div class="sk sk-sm"></div><div class="sk sk-badge"></div><div class="sk sk-md"></div><div class="sk sk-md"></div><div class="sk sk-lg"></div><div class="sk sk-btn"></div></div>
      <div class="sk-row" style="opacity:0.55;"><div class="sk sk-sm"></div><div class="sk sk-badge"></div><div class="sk sk-md"></div><div class="sk sk-md"></div><div class="sk sk-lg"></div><div class="sk sk-btn"></div></div>
      <div class="sk-row" style="opacity:0.35;"><div class="sk sk-sm"></div><div class="sk sk-badge"></div><div class="sk sk-md"></div><div class="sk sk-md"></div><div class="sk sk-lg"></div><div class="sk sk-btn"></div></div>
    </div>
    <div id="refPanel" class="card">
      <div class="ref-panel-topbar" id="refPanelTopbar">
        <div>
          <div class="ref-panel-repo-label">Loaded repository</div>
          <div class="ref-panel-repo-url" id="refPanelRepo"></div>
        </div>
        <span class="ref-panel-badge"><svg viewBox="0 0 24 24"><polyline points="20 6 9 17 4 12"/></svg>Ready</span>
      </div>
      <div class="tabs">
        <button class="tab active" data-tab="branches" type="button"><span class="tab-inner"><svg viewBox="0 0 24 24"><line x1="6" y1="3" x2="6" y2="15"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M18 9a9 9 0 0 1-9 9"/><circle cx="6" cy="6" r="3"/></svg>Branches</span><span class="tab-count" id="branchCount"></span></button>
        <button class="tab" data-tab="tags" type="button"><span class="tab-inner"><svg viewBox="0 0 24 24"><path d="M20.59 13.41l-7.17 7.17a2 2 0 0 1-2.83 0L2 12V2h10l8.59 8.59a2 2 0 0 1 0 2.82z"/><line x1="7" y1="7" x2="7.01" y2="7"/></svg>Tags / Releases</span><span class="tab-count" id="tagCount"></span></button>
        <button class="tab" data-tab="commits" type="button"><span class="tab-inner"><svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg>Commits</span><span class="tab-count" id="commitCount"></span></button>
      </div>
      <div id="tab-branches" class="tab-pane active">
        <table class="ref-table">
          <colgroup><col class="col-check"><col class="col-name"><col class="col-sha"><col class="col-date"><col class="col-msg"><col class="col-actions"></colgroup>
          <thead><tr><th></th><th>Branch</th><th>SHA</th><th>Date</th><th>Message</th><th>Actions</th></tr></thead>
          <tbody id="branchBody"><tr><td colspan="6"><div class="empty-rich"><div class="empty-rich-icon"><svg viewBox="0 0 24 24"><line x1="6" y1="3" x2="6" y2="15"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M18 9a9 9 0 0 1-9 9"/><circle cx="6" cy="6" r="3"/></svg></div><div class="empty-rich-title">No repository loaded</div><div class="empty-rich-desc">Paste a repository URL above and click Fetch to browse branches.</div></div></td></tr></tbody>
        </table>
        <div id="branchPag"></div>
      </div>
      <div id="tab-tags" class="tab-pane">
        <table class="ref-table">
          <colgroup><col class="col-check"><col class="col-name"><col class="col-sha"><col class="col-date"><col class="col-msg"><col class="col-actions"></colgroup>
          <thead><tr><th></th><th>Tag / Release</th><th>SHA</th><th>Date</th><th>Message</th><th>Actions</th></tr></thead>
          <tbody id="tagBody"><tr><td colspan="6"><div class="empty-rich"><div class="empty-rich-icon"><svg viewBox="0 0 24 24"><path d="M20.59 13.41l-7.17 7.17a2 2 0 0 1-2.83 0L2 12V2h10l8.59 8.59a2 2 0 0 1 0 2.82z"/><line x1="7" y1="7" x2="7.01" y2="7"/></svg></div><div class="empty-rich-title">No repository loaded</div><div class="empty-rich-desc">Paste a repository URL above and click Fetch to browse tags and releases.</div></div></td></tr></tbody>
        </table>
        <div id="tagPag"></div>
      </div>
      <div id="tab-commits" class="tab-pane">
        <table class="ref-table">
          <colgroup><col class="col-check"><col class="col-name"><col class="col-sha"><col class="col-date"><col class="col-msg"><col class="col-actions"></colgroup>
          <thead><tr><th></th><th>Author</th><th>SHA</th><th>Date</th><th>Subject</th><th>Actions</th></tr></thead>
          <tbody id="commitBody"><tr><td colspan="6"><div class="empty-rich"><div class="empty-rich-icon"><svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg></div><div class="empty-rich-title">No repository loaded</div><div class="empty-rich-desc">Paste a repository URL above and click Fetch to browse recent commits.</div></div></td></tr></tbody>
        </table>
        <div id="commitPag"></div>
      </div>
      <div class="compare-bar" id="compareBar" style="display:none">
        <div class="compare-refs-label">
          <svg viewBox="0 0 24 24"><line x1="18" y1="20" x2="18" y2="10"/><line x1="12" y1="20" x2="12" y2="4"/><line x1="6" y1="20" x2="6" y2="14"/></svg>
          Compare: <span class="compare-ref-tag" id="compareA">—</span> <span class="compare-vs">vs</span> <span class="compare-ref-tag" id="compareB">—</span>
        </div>
        <button class="btn btn-compare btn-sm" id="compareBtn" type="button">
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" style="flex-shrink:0"><polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/></svg>
          Compare Refs
        </button>
        <button class="btn btn-sm" id="clearBtn" style="background:var(--line);color:var(--text);" type="button">Clear</button>
      </div>
    </div>
  </div>
  <script nonce="{{ csp_nonce }}">
    (function () {
      var compareA = null, compareB = null, currentRepo = {{ repo_url_json|safe }};

      // ── Pagination state ──────────────────────────────────────────────────────
      var allBranches = [], allTags = [], allCommits = [];
      var pagState = {
        branches: { page: 1, size: 15 },
        tags:     { page: 1, size: 15 },
        commits:  { page: 1, size: 15 },
      };

      function applyTheme() { if (localStorage.getItem('sloc-theme') === 'dark') document.body.classList.add('dark-theme'); }
      function toggleTheme() { var d = document.body.classList.toggle('dark-theme'); localStorage.setItem('sloc-theme', d ? 'dark' : 'light'); }

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

      function showTab(name) {
        var names = ['branches', 'tags', 'commits'];
        document.querySelectorAll('.tab').forEach(function (t, i) { t.classList.toggle('active', names[i] === name); });
        document.querySelectorAll('.tab-pane').forEach(function (p) { p.classList.remove('active'); });
        document.getElementById('tab-' + name).classList.add('active');
      }

      function showStatus(msg, ok) {
        var el = document.getElementById('statusMsg');
        el.style.display = 'block';
        el.className = 'status-msg ' + (ok ? 'status-ok' : 'status-err');
        el.textContent = msg;
      }

      function resetLoadingState() {
        document.getElementById('loadingPanel').classList.add('panel-hidden');
        document.getElementById('loadSpinner').classList.add('panel-hidden');
        document.getElementById('loadBtn').disabled = false;
        document.getElementById('statusMsg').style.display = 'none';
      }

      // ── Error display helpers ─────────────────────────────────────────────────

      function getErrorHints(errMsg, inputUrl) {
        var lower = inputUrl.toLowerCase();
        var errLower = errMsg.toLowerCase();

        // Bitbucket Server/Data Center browse URL
        if (lower.includes('/projects/') && lower.includes('/repos/')) {
          var m = inputUrl.match(/^(https?:\/\/[^/]+)(\/[^/]*)\/projects\/([^/]+)\/repos\/([^/]+)/i);
          if (m) {
            var derived = m[1] + m[2] + '/scm/' + m[3].toLowerCase() + '/' + m[4].replace(/\.git$/i, '') + '.git';
            return '<b>Bitbucket Server/Data Center URL detected.</b> The browse URL was automatically converted to: <code>' + esc(derived) + '</code><br>If the fetch failed, verify the repository exists and you have access.';
          }
          return '<b>Bitbucket Server/Data Center URL detected.</b> Browse URLs are automatically converted to the git clone format (<code>/scm/PROJECT/repo.git</code>). If the fetch still fails, the derived URL may be wrong for your deployment.';
        }

        // Bitbucket Cloud browse URL
        if (lower.includes('bitbucket.org') && lower.includes('/src/')) {
          return '<b>Bitbucket Cloud browse URL.</b> The <code>/src/\u2026</code> suffix was stripped. If the fetch failed, check that the repository is public or that your git credentials are configured.';
        }

        // GitLab browse URL
        if (lower.includes('/-/')) {
          var repoUrl = inputUrl.replace(/\/-\/.*$/, '').replace(/\.git$/, '') + '.git';
          return '<b>GitLab browse URL detected.</b> Converted to: <code>' + esc(repoUrl) + '</code>';
        }

        // GitHub browse URL
        if ((lower.includes('github.com') || lower.includes('.github.com')) && (lower.includes('/tree/') || lower.includes('/blob/'))) {
          return '<b>GitHub browse URL detected.</b> The branch/path suffix was stripped to produce a clone URL.';
        }

        // SSL / certificate errors
        if (errLower.includes('ssl') || errLower.includes('certificate') || errLower.includes('tls')) {
          return '<b>SSL certificate error.</b> Corporate proxy/VPN certificates are trusted automatically from your OS certificate store (Windows uses schannel). If this still fails, the signing CA is not installed in your system trust store — add it there (the durable fix), or as a last resort start oxide-sloc with <code>SLOC_GIT_SSL_NO_VERIFY=1</code>.';
        }

        // Authentication errors
        if (errLower.includes('authentication failed') || errLower.includes('could not read username') || errLower.includes('403') || errLower.includes('401')) {
          return '<b>Authentication required.</b> Configure HTTPS credentials in your <a href="https://git-scm.com/docs/git-credential-store" target="_blank" rel="noopener">git credential store</a>, or use an SSH URL (<code>git@host:project/repo.git</code>).';
        }

        // Repository not found
        if (errLower.includes('not found') || errLower.includes('does not exist') || errLower.includes('404') || errLower.includes('repository') && errLower.includes('not')) {
          return '<b>Repository not found.</b> Verify the URL is correct and the repository is accessible from this machine. For private repos, ensure your credentials are configured in git.';
        }

        // Timeout (stalled proxy / VPN)
        if (errLower.includes('timed out') || errLower.includes('timeout')) {
          return '<b>Fetch timed out.</b> The remote did not respond in time — usually a slow or blocking proxy/VPN on a corporate network. git honors the <code>HTTPS_PROXY</code>/<code>HTTP_PROXY</code> environment variables; set them if your network requires a proxy. Raise the ceiling with <code>SLOC_GIT_TIMEOUT=&lt;seconds&gt;</code> (default 300) before starting oxide-sloc.';
        }

        // Connection errors
        if (errLower.includes('could not resolve host') || errLower.includes('failed to connect') || errLower.includes('connection refused') || errLower.includes('network')) {
          return '<b>Network error.</b> Check that the host is reachable from this machine. For internal corporate repos, ensure you are connected to the correct network or VPN, and set <code>HTTPS_PROXY</code>/<code>HTTP_PROXY</code> if a proxy is required.';
        }

        return '';
      }

      function buildErrorTableHtml(errMsg, inputUrl) {
        var hints = getErrorHints(errMsg, inputUrl);
        var short = errMsg.length > 400 ? errMsg.slice(0, 397) + '\u2026' : errMsg;
        return '<tr><td colspan="6"><div class="fetch-error-state">'
          + '<div class="fetch-error-icon-wrap"><svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/></svg></div>'
          + '<div class="fetch-error-title">Repository fetch failed</div>'
          + '<div class="fetch-error-msg">' + esc(short) + '</div>'
          + (hints ? '<div class="fetch-error-hints">' + hints + '</div>' : '')
          + '</div></td></tr>';
      }

      function showFetchError(errMsg, inputUrl) {
        var html = buildErrorTableHtml(errMsg, inputUrl);
        ['branchBody', 'tagBody', 'commitBody'].forEach(function (id) {
          var el = document.getElementById(id); if (el) el.innerHTML = html;
        });
        ['branchPag', 'tagPag', 'commitPag'].forEach(function (id) {
          var el = document.getElementById(id); if (el) el.innerHTML = '';
        });
        setTabCount('branchCount', 0);
        setTabCount('tagCount', 0);
        setTabCount('commitCount', 0);
        var topbar = document.getElementById('refPanelTopbar');
        if (topbar) topbar.classList.remove('visible');
        document.getElementById('refPanel').classList.remove('panel-hidden');
        // Short summary near the input as well
        var brief = errMsg.length > 100 ? errMsg.slice(0, 97) + '\u2026' : errMsg;
        showStatus(brief, false);
      }

      async function fetchRefs() {
        var repo = document.getElementById('repoInput').value.trim();
        if (!repo) { showStatus('Enter a GitHub, GitLab, or Bitbucket URL.', false); return; }
        currentRepo = repo;
        document.getElementById('statusMsg').style.display = 'none';
        document.getElementById('loadSpinner').classList.remove('panel-hidden');
        document.getElementById('loadBtn').disabled = true;
        document.getElementById('refPanel').classList.add('panel-hidden');
        document.getElementById('loadingPanel').classList.remove('panel-hidden');

        var elapsed = 0;
        var loadingMsg = document.querySelector('#loadingPanel .loading-info');
        var baseText = ' Fetching repository\u2026';
        var timer = setInterval(function () {
          elapsed++;
          var hint = elapsed >= 10 ? ' (first fetch may take 15\u201330s)' : '';
          if (loadingMsg) loadingMsg.lastChild.textContent = baseText + ' ' + elapsed + 's' + hint;
        }, 1000);

        // Guard against a backend that never responds (e.g. a git subprocess wedged on an
        // auth prompt): abort after 90s so the spinner can't loop forever and the user gets
        // a recoverable error in-page instead of having to refresh.
        var ctrl = new AbortController();
        var abortTimer = setTimeout(function () { ctrl.abort(); }, 90000);
        try {
          var r = await fetch('/api/git/refs?' + new URLSearchParams({ repo: repo }), { signal: ctrl.signal });
          var data = await r.json();
          if (!r.ok) { showFetchError(data.error || 'Failed to load repository.', repo); return; }
          renderRefs(data);
          document.getElementById('refPanel').classList.remove('panel-hidden');
          document.getElementById('statusMsg').style.display = 'none';
        } catch (e) {
          if (e && e.name === 'AbortError') {
            showFetchError('Fetch timed out after 90s. The repository may be very large, unreachable, or require credentials that git is waiting on. Verify the URL and access, then try again.', repo);
          } else {
            showFetchError('Network error: ' + e.message, repo);
          }
        }
        finally {
          clearTimeout(abortTimer);
          clearInterval(timer);
          if (loadingMsg) loadingMsg.lastChild.textContent = baseText;
          resetLoadingState();
        }
      }

      function renderRefs(data) {
        allBranches = data.branches || [];
        allTags     = data.tags || [];
        allCommits  = data.recent_commits || [];
        pagState.branches.page = 1;
        pagState.tags.page     = 1;
        pagState.commits.page  = 1;
        renderPage('branches');
        renderPage('tags');
        renderPage('commits');
        clearCompare();
        setTabCount('branchCount', allBranches.length);
        setTabCount('tagCount',    allTags.length);
        setTabCount('commitCount', allCommits.length);
        var topbar = document.getElementById('refPanelTopbar');
        var repoEl = document.getElementById('refPanelRepo');
        if (topbar && repoEl) { repoEl.textContent = currentRepo; topbar.classList.add('visible'); }
      }

      // ── Pagination helpers ────────────────────────────────────────────────────

      function renderPage(tab) {
        var items  = tab === 'branches' ? allBranches : tab === 'tags' ? allTags : allCommits;
        var state  = pagState[tab];
        var total  = items.length;
        var start  = (state.page - 1) * state.size;
        var end    = Math.min(start + state.size, total);
        var slice  = items.slice(start, end);
        var bodyId = tab === 'branches' ? 'branchBody' : tab === 'tags' ? 'tagBody' : 'commitBody';
        var pagId  = tab === 'branches' ? 'branchPag'  : tab === 'tags' ? 'tagPag'  : 'commitPag';

        if (tab === 'commits') {
          renderCommitSlice(bodyId, slice);
        } else {
          renderRefSlice(bodyId, slice, tab === 'branches' ? 'branch' : 'tag');
        }
        renderPagBar(pagId, tab, total, state.page, state.size, start, end);
      }

      function renderRefSlice(tbodyId, items, kind) {
        if (!items.length) {
          document.getElementById(tbodyId).innerHTML = '<tr><td colspan="6"><div class="empty-state">No entries found.</div></td></tr>';
          return;
        }
        document.getElementById(tbodyId).innerHTML = items.map(function (it) {
          var d = it.date ? new Date(it.date).toLocaleDateString() : '';
          var badge = kind === 'branch'
            ? '<span class="kind-badge kind-branch">branch</span>'
            : '<span class="kind-badge kind-tag">tag</span>';
          var n = esc(it.name), m = esc(it.message || ''), s = esc(it.sha.slice(0, 8));
          var ref = esc(it.name);
          return '<tr>'
            + '<td><input type="checkbox" class="compare-check" data-ref="' + ref + '"></td>'
            + '<td>' + badge + ' ' + n + '</td>'
            + '<td><span class="sha-badge">' + s + '</span></td>'
            + '<td class="date-cell">' + d + '</td>'
            + '<td class="col-msg-cell" title="' + m + '">' + m + '</td>'
            + '<td><button class="btn btn-primary btn-sm btn-scan" data-action="scan-ref" data-ref="' + ref + '" type="button">Scan</button></td>'
            + '</tr>';
        }).join('');
      }

      function renderCommitSlice(tbodyId, commits) {
        if (!commits.length) {
          document.getElementById(tbodyId).innerHTML = '<tr><td colspan="6"><div class="empty-state">No entries found.</div></td></tr>';
          return;
        }
        document.getElementById(tbodyId).innerHTML = commits.map(function (c) {
          var d = c.date ? new Date(c.date).toLocaleDateString() : '';
          var sha = esc(c.sha), s = esc(c.short_sha), a = esc(c.author), sub = esc(c.subject);
          return '<tr>'
            + '<td><input type="checkbox" class="compare-check" data-ref="' + sha + '"></td>'
            + '<td>' + a + '</td>'
            + '<td><span class="sha-badge">' + s + '</span></td>'
            + '<td class="date-cell">' + d + '</td>'
            + '<td class="col-msg-cell" title="' + sub + '">' + sub + '</td>'
            + '<td><button class="btn btn-primary btn-sm btn-scan" data-action="scan-ref" data-ref="' + sha + '" type="button">Scan</button></td>'
            + '</tr>';
        }).join('');
      }

      function renderPagBar(containerId, tab, total, page, size, start, end) {
        var el = document.getElementById(containerId);
        if (!el) return;
        if (total <= size && total > 0) {
          // All rows fit on one page — show a simple count, no nav needed
          el.innerHTML = '<div class="pag-bar"><span class="pag-info">Showing all ' + total + ' entr' + (total === 1 ? 'y' : 'ies') + '</span>'
            + '<div class="pag-size-wrap">Per page: <select class="pag-size-select" data-tab="' + tab + '">'
            + pagSizeOptions(size) + '</select></div></div>';
          return;
        }
        if (total === 0) { el.innerHTML = ''; return; }

        var totalPages = Math.ceil(total / size);
        var from = start + 1;
        var html = '<div class="pag-bar">';
        html += '<span class="pag-info">Showing ' + from + '\u2013' + end + ' of ' + total + '</span>';
        html += '<div class="pag-size-wrap">Per page: <select class="pag-size-select" data-tab="' + tab + '">' + pagSizeOptions(size) + '</select></div>';
        html += '<div class="pag-nav">';
        html += '<button class="pag-btn" data-tab="' + tab + '" data-page="' + (page - 1) + '"' + (page <= 1 ? ' disabled' : '') + '>&#8592; Prev</button>';
        pageRange(page, totalPages).forEach(function (p) {
          if (p === '\u2026') {
            html += '<span class="pag-ellipsis">\u2026</span>';
          } else {
            html += '<button class="pag-btn' + (p === page ? ' active' : '') + '" data-tab="' + tab + '" data-page="' + p + '">' + p + '</button>';
          }
        });
        html += '<button class="pag-btn" data-tab="' + tab + '" data-page="' + (page + 1) + '"' + (page >= totalPages ? ' disabled' : '') + '>Next &#8594;</button>';
        html += '</div></div>';
        el.innerHTML = html;
      }

      function pagSizeOptions(current) {
        return [15, 25, 50, 100].map(function (s) {
          return '<option value="' + s + '"' + (s === current ? ' selected' : '') + '>' + s + '</option>';
        }).join('');
      }

      function pageRange(current, total) {
        if (total <= 7) {
          var arr = [];
          for (var i = 1; i <= total; i++) arr.push(i);
          return arr;
        }
        if (current <= 4) {
          return [1, 2, 3, 4, 5, '\u2026', total];
        }
        if (current >= total - 3) {
          return [1, '\u2026', total - 4, total - 3, total - 2, total - 1, total];
        }
        return [1, '\u2026', current - 1, current, current + 1, '\u2026', total];
      }

      function setTabCount(id, n) {
        var el = document.getElementById(id);
        if (!el) return;
        el.textContent = n;
        el.classList.toggle('has-data', n > 0);
      }

      function toggleCompare(ref, cb) {
        if (cb.checked) {
          if (!compareA) { compareA = ref; }
          else if (!compareB && ref !== compareA) { compareB = ref; }
          else { cb.checked = false; return; }
        } else {
          if (compareA === ref) compareA = null;
          else if (compareB === ref) compareB = null;
        }
        var bar = document.getElementById('compareBar');
        bar.style.display = (compareA || compareB) ? 'flex' : 'none';
        document.getElementById('compareA').textContent = compareA ? short(compareA) : '\u2014';
        document.getElementById('compareB').textContent = compareB ? short(compareB) : '\u2014';
      }

      function clearCompare() {
        compareA = null; compareB = null;
        document.querySelectorAll('.compare-check').forEach(function (c) { c.checked = false; });
        document.getElementById('compareBar').style.display = 'none';
      }

      function short(r) { return r.length > 12 ? r.slice(0, 8) + '\u2026' : r; }

      function scanRef(refName) {
        if (!currentRepo) return;
        var params = new URLSearchParams({ git_repo: currentRepo, git_ref: refName });
        window.location.href = '/scan?' + params.toString();
      }

      async function runCompare() {
        if (!compareA || !compareB) { showStatus('Select exactly two refs.', false); return; }
        showStatus('Scanning both refs\u2026', true);
        var r = await fetch('/api/git/compare-refs?' + new URLSearchParams({ repo: currentRepo, baseline_ref: compareA, current_ref: compareB }));
        var data = await r.json();
        if (r.ok && data.compare_url) { window.location.href = data.compare_url; }
        else { showStatus(data.error || 'Compare failed.', false); }
      }

      function esc(s) { return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;'); }

      // ── Event wiring ──────────────────────────────────────────────────────────
      document.getElementById('theme-toggle').addEventListener('click', toggleTheme);
      document.getElementById('loadBtn').addEventListener('click', fetchRefs);
      document.getElementById('repoInput').addEventListener('keydown', function (e) { if (e.key === 'Enter') fetchRefs(); });
      document.getElementById('compareBtn').addEventListener('click', runCompare);
      document.getElementById('clearBtn').addEventListener('click', clearCompare);

      document.querySelectorAll('.tab').forEach(function (btn) {
        btn.addEventListener('click', function () { showTab(btn.dataset.tab); });
      });

      // Event delegation for dynamically-injected table rows and pagination controls
      document.getElementById('refPanel').addEventListener('click', function (e) {
        // Pagination page buttons
        var pb = e.target.closest('.pag-btn:not(:disabled)');
        if (pb && pb.dataset.tab && pb.dataset.page) {
          var tab = pb.dataset.tab, pg = Number(pb.dataset.page);
          if (!isNaN(pg) && pg >= 1) {
            pagState[tab].page = pg;
            renderPage(tab);
          }
          return;
        }
        // Scan button
        var btn = e.target.closest('[data-action="scan-ref"]');
        if (btn) { scanRef(btn.dataset.ref); return; }
        // Clicking anywhere on a row (outside the checkbox or scan button) toggles the checkbox
        if (e.target.closest('.compare-check') || e.target.closest('.btn-scan')) return;
        var row = e.target.closest('tr');
        if (!row) return;
        var cb = row.querySelector('.compare-check');
        if (cb) { cb.checked = !cb.checked; toggleCompare(cb.dataset.ref, cb); }
      });
      document.getElementById('refPanel').addEventListener('change', function (e) {
        // Page-size selector
        var sel = e.target.closest('.pag-size-select');
        if (sel && sel.dataset.tab) {
          pagState[sel.dataset.tab].size = Number(sel.value);
          pagState[sel.dataset.tab].page = 1;
          renderPage(sel.dataset.tab);
          return;
        }
        var cb = e.target.closest('.compare-check');
        if (cb) toggleCompare(cb.dataset.ref, cb);
      });

      // ── Background effects ────────────────────────────────────────────────────
      (function randomizeWatermarks() {
        var wms = Array.prototype.slice.call(document.querySelectorAll('.background-watermarks img'));
        if (!wms.length) return;
        var placed = [];
        function tooClose(top, left) {
          for (var i = 0; i < placed.length; i++) {
            if (Math.abs(placed[i][0] - top) < 16 && Math.abs(placed[i][1] - left) < 12) return true;
          }
          return false;
        }
        function pick(leftBand) {
          for (var attempt = 0; attempt < 50; attempt++) {
            var top = Math.random() * 88 + 2, left = leftBand ? Math.random() * 24 + 1 : Math.random() * 24 + 74;
            if (!tooClose(top, left)) { placed.push([top, left]); return [top, left]; }
          }
          var top = Math.random() * 88 + 2, left = leftBand ? Math.random() * 24 + 1 : Math.random() * 24 + 74;
          placed.push([top, left]); return [top, left];
        }
        var half = Math.floor(wms.length / 2);
        wms.forEach(function (img, i) {
          var pos = pick(i < half);
          var size = Math.floor(Math.random() * 100 + 120);
          img.style.cssText = 'width:' + size + 'px;top:' + pos[0].toFixed(1) + '%;left:' + pos[1].toFixed(1) + '%;transform:rotate(' + (Math.random() * 360).toFixed(1) + 'deg);opacity:' + (Math.random() * 0.08 + 0.12).toFixed(2) + ';';
        });
      })();

      (function spawnCodeParticles() {
        var container = document.getElementById('code-particles');
        if (!container) return;
        var snippets = ['1,247 sloc','fn analyze()','code_lines','0 mixed','blanks: 312','// comment','pub fn run','use std::fs','Result<()>','let mut n = 0','git main','#[derive]','impl Scan','3,841 physical','files: 60','450 comments','cargo build','Ok(run)','Vec<String>','match lang','fn main() {','.rs .go .py','sloc_core','render_html','2,163 code'];
        for (var i = 0; i < 38; i++) {
          (function (idx) {
            var el = document.createElement('span');
            el.className = 'code-particle';
            el.textContent = snippets[idx % snippets.length];
            el.style.cssText = 'left:' + (Math.random() * 94 + 2).toFixed(1) + '%;top:' + (Math.random() * 88 + 6).toFixed(1) + '%;--rot:' + (Math.random() * 26 - 13).toFixed(1) + 'deg;--op:' + (Math.random() * 0.09 + 0.06).toFixed(3) + ';animation-duration:' + (Math.random() * 10 + 9).toFixed(1) + 's;animation-delay:-' + (Math.random() * 18).toFixed(1) + 's;';
            container.appendChild(el);
          })(i);
        }
      })();

      // Before the page enters BFCache, reset loading state so it's never
      // cached mid-load and shown to the user on back-navigation.
      window.addEventListener('pagehide', resetLoadingState);
      // Also reset on pageshow to cover fresh loads and BFCache restores.
      window.addEventListener('pageshow', resetLoadingState);

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
        var fm = document.getElementById('footer-mode');
        if (fm) { var isServer = location.hostname !== 'localhost' && location.hostname !== '127.0.0.1' && location.hostname !== '[::1]'; fm.textContent = 'Mode: ' + (isServer ? 'Network Server' : 'Local'); }
      })();

      applyTheme();
    })();
  </script>
  <footer class="site-footer">
    oxide-sloc v{{ version }} — local code analysis - metrics, history and reports
    &nbsp;·&nbsp; <em class="footer-mode" id="footer-mode" style="font-style:italic;font-weight:700;color:var(--oxide);">Mode: Local</em>
    &nbsp;·&nbsp; Built by <a href="https://github.com/NimaShafie" target="_blank" rel="noopener">Nima Shafie</a>
    &nbsp;·&nbsp; <a href="https://github.com/oxide-sloc/oxide-sloc" target="_blank" rel="noopener">View on GitHub</a>
    &nbsp;·&nbsp; <a href="https://www.gnu.org/licenses/agpl-3.0.html" target="_blank" rel="noopener">AGPL-3.0-or-later</a>
    &nbsp;·&nbsp; <a href="/report-bug" rel="noopener">Report a Bug</a> &nbsp;&middot;&nbsp; <a href="/api-docs" rel="noopener">REST API</a>
  </footer>
</body>
</html>"##,
    ext = "html"
)]
pub struct GitBrowserTemplate {
    pub csp_nonce: String,
    pub repo_url: String,
    pub repo_url_json: String,
    pub version: &'static str,
}

// ── handlers ──────────────────────────────────────────────────────────────────

pub async fn git_browser_handler(
    State(_state): State<AppState>,
    axum::extract::Extension(CspNonce(csp_nonce)): axum::extract::Extension<CspNonce>,
    Query(q): Query<GitBrowserQuery>,
) -> impl IntoResponse {
    let repo_url = q.repo.unwrap_or_default();
    let repo_url_json = serde_json::to_string(&repo_url).unwrap_or_else(|_| "\"\"".to_owned());
    let template = GitBrowserTemplate {
        csp_nonce,
        repo_url,
        repo_url_json,
        version: env!("CARGO_PKG_VERSION"),
    };
    Html(
        template
            .render()
            .unwrap_or_else(|e| format!("<pre>{e}</pre>")),
    )
}

pub async fn api_list_refs(
    State(state): State<AppState>,
    Query(q): Query<GitBrowserQuery>,
) -> impl IntoResponse {
    let Some(repo) = q.repo else {
        return json_error(StatusCode::BAD_REQUEST, "missing ?repo=");
    };
    let clones_dir = state.git_clones_dir.clone();
    match tokio::task::spawn_blocking(move || load_refs(&repo, &clones_dir)).await {
        Ok(Ok(refs)) => (StatusCode::OK, Json(serde_json::json!(refs))).into_response(),
        Ok(Err(e)) => {
            tracing::warn!(event = "git_api_error", "load_refs failed: {e:#}");
            json_error(StatusCode::BAD_GATEWAY, &git_error_message(&e))
        }
        Err(e) => {
            tracing::error!(event = "git_task_panic", "load_refs task panicked: {e}");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
        }
    }
}

/// Allow the same character set git itself accepts for ref names, plus a conservative length cap.
/// Blocks path traversal via `..`, absolute paths, and shell-special characters.
fn is_valid_git_ref(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 256
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '@' | '+'))
        && !s.starts_with('/')
        && !s.contains("..")
}

pub async fn api_scan_ref(
    State(state): State<AppState>,
    Query(q): Query<ScanRefQuery>,
) -> impl IntoResponse {
    if !is_valid_git_ref(&q.ref_name) {
        return json_error(StatusCode::BAD_REQUEST, "invalid ref_name");
    }
    let clones_dir = state.git_clones_dir.clone();
    let base_config = state.base_config.clone();
    let label = q
        .label
        .clone()
        .unwrap_or_else(|| make_label(&q.repo, &q.ref_name));
    let label_for_reg = label.clone();
    let repo = q.repo.clone();
    let ref_name = q.ref_name.clone();

    match tokio::task::spawn_blocking(move || {
        run_ref_scan(&repo, &ref_name, &clones_dir, &base_config, &label)
    })
    .await
    {
        Ok(Ok((run_id, html_url, artifacts, run))) => {
            register_scan(&state, &run_id, &label_for_reg, artifacts, &run).await;
            (
                StatusCode::OK,
                Json(serde_json::json!({ "run_id": run_id, "html_url": html_url })),
            )
                .into_response()
        }
        Ok(Err(e)) => {
            tracing::warn!(event = "git_api_error", "ref scan failed: {e:#}");
            json_error(StatusCode::BAD_GATEWAY, &git_error_message(&e))
        }
        Err(e) => {
            tracing::error!(event = "git_task_panic", "ref scan task panicked: {e}");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
        }
    }
}

pub async fn api_compare_refs(
    State(state): State<AppState>,
    Query(q): Query<CompareRefsQuery>,
) -> impl IntoResponse {
    if !is_valid_git_ref(&q.baseline_ref) || !is_valid_git_ref(&q.current_ref) {
        return json_error(StatusCode::BAD_REQUEST, "invalid ref name").into_response();
    }
    let clones_dir = state.git_clones_dir.clone();
    let base_config = state.base_config.clone();
    let label = q
        .label
        .clone()
        .unwrap_or_else(|| make_label(&q.repo, "compare"));
    let label_for_reg = label.clone();
    let repo = q.repo.clone();
    let baseline_ref = q.baseline_ref.clone();
    let current_ref = q.current_ref.clone();
    let baseline_ref_label = baseline_ref.clone();
    let current_ref_label = current_ref.clone();

    match tokio::task::spawn_blocking(move || {
        run_compare_refs(
            &repo,
            &baseline_ref,
            &current_ref,
            &clones_dir,
            &base_config,
            &label,
        )
    })
    .await
    {
        Ok(Ok((b_id, b_arts, b_run, c_id, c_arts, c_run))) => {
            let b_label = format!("{label_for_reg} ({baseline_ref_label})");
            let c_label = format!("{label_for_reg} ({current_ref_label})");
            register_scan(&state, &b_id, &b_label, b_arts, &b_run).await;
            register_scan(&state, &c_id, &c_label, c_arts, &c_run).await;
            let url = format!("/compare?a={b_id}&b={c_id}");
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "baseline_run_id": b_id, "current_run_id": c_id, "compare_url": url
                })),
            )
                .into_response()
        }
        Ok(Err(e)) => {
            tracing::warn!(event = "git_api_error", "ref compare failed: {e:#}");
            json_error(StatusCode::BAD_GATEWAY, &git_error_message(&e))
        }
        Err(e) => {
            tracing::error!(event = "git_task_panic", "ref compare task panicked: {e}");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
        }
    }
}

async fn register_scan(
    state: &AppState,
    run_id: &str,
    label: &str,
    artifacts: RunArtifacts,
    run: &sloc_core::AnalysisRun,
) {
    let project_label = sanitize_project_label(label);
    {
        let mut map = state.artifacts.lock().await;
        map.insert(run_id.to_owned(), artifacts.clone());
    }
    {
        let entry = build_run_registry_entry(run, run_id, &project_label, &artifacts);
        let mut reg = state.registry.lock().await;
        reg.add_entry(entry);
        let _ = reg.save(&state.registry_path);
    }
}

// ── core logic (runs in spawn_blocking) ───────────────────────────────────────

fn load_refs(repo: &str, clones_dir: &Path) -> anyhow::Result<RepoRefs> {
    let dest = git_clone_dest(repo, clones_dir);
    clone_or_fetch(repo, &dest)?;
    list_refs(&dest)
}

fn run_ref_scan(
    repo: &str,
    ref_name: &str,
    clones_dir: &Path,
    base_config: &sloc_config::AppConfig,
    label: &str,
) -> anyhow::Result<(String, String, RunArtifacts, sloc_core::AnalysisRun)> {
    let dest = git_clone_dest(repo, clones_dir);
    clone_or_fetch(repo, &dest)?;
    let wt_path = clones_dir.join(format!("wt-{}", uuid::Uuid::new_v4().simple()));
    create_worktree(&dest, ref_name, &wt_path)?;
    let result = scan_path_to_artifacts(&wt_path, base_config, label);
    let _ = destroy_worktree(&dest, &wt_path);
    let (run_id, artifacts, mut run) = result?;
    // Worktrees at a tag/commit are in detached HEAD; git branch --show-current
    // returns empty. Use the ref_name we checked out instead.
    if run.git_branch.is_none() {
        run.git_branch = Some(ref_name.to_owned());
        if let Some(html_path) = &artifacts.html_path
            && let Ok(html) = render_html(&run)
        {
            let _ = std::fs::write(html_path, html);
        }
    }
    let html_url = format!("/runs/html/{run_id}");
    Ok((run_id, html_url, artifacts, run))
}

fn run_compare_refs(
    repo: &str,
    baseline_ref: &str,
    current_ref: &str,
    clones_dir: &Path,
    base_config: &sloc_config::AppConfig,
    label: &str,
) -> anyhow::Result<(
    String,
    RunArtifacts,
    sloc_core::AnalysisRun,
    String,
    RunArtifacts,
    sloc_core::AnalysisRun,
)> {
    let dest = git_clone_dest(repo, clones_dir);
    clone_or_fetch(repo, &dest)?;

    let b_label = format!("{label} ({baseline_ref})");
    let c_label = format!("{label} ({current_ref})");

    let wt_a = clones_dir.join(format!("wt-{}", uuid::Uuid::new_v4().simple()));
    create_worktree(&dest, baseline_ref, &wt_a)?;
    let b_result = scan_path_to_artifacts(&wt_a, base_config, &b_label);
    let _ = destroy_worktree(&dest, &wt_a);
    let (b_id, b_arts, mut b_run) = b_result?;
    if b_run.git_branch.is_none() {
        b_run.git_branch = Some(baseline_ref.to_owned());
        if let Some(p) = &b_arts.html_path
            && let Ok(html) = render_html(&b_run)
        {
            let _ = std::fs::write(p, html);
        }
    }

    let wt_b = clones_dir.join(format!("wt-{}", uuid::Uuid::new_v4().simple()));
    create_worktree(&dest, current_ref, &wt_b)?;
    let c_result = scan_path_to_artifacts(&wt_b, base_config, &c_label);
    let _ = destroy_worktree(&dest, &wt_b);
    let (c_id, c_arts, mut c_run) = c_result?;
    if c_run.git_branch.is_none() {
        c_run.git_branch = Some(current_ref.to_owned());
        if let Some(p) = &c_arts.html_path
            && let Ok(html) = render_html(&c_run)
        {
            let _ = std::fs::write(p, html);
        }
    }

    Ok((b_id, b_arts, b_run, c_id, c_arts, c_run))
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_label(repo: &str, ref_name: &str) -> String {
    let base = repo
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit('/')
        .next()
        .unwrap_or("repo");
    let ref_safe: String = ref_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{base}_at_{ref_safe}_sloc")
}

fn json_error(status: StatusCode, msg: &str) -> axum::response::Response {
    (status, Json(serde_json::json!({ "error": msg }))).into_response()
}

/// Render a git-operation error for the API response body. The Git Browser UI classifies
/// this text (timeout, SSL/certificate, authentication, DNS, SSRF-block, …) to show
/// targeted remediation hints, so the real message must reach the client instead of
/// collapsing to a single opaque string that no hint can match. The full cause chain is
/// still logged separately on the `git_api_error` event; length is capped here to keep the
/// payload bounded.
fn git_error_message(e: &anyhow::Error) -> String {
    const MAX_CHARS: usize = 600;
    let msg = format!("{e:#}");
    if msg.chars().count() > MAX_CHARS {
        let mut truncated: String = msg.chars().take(MAX_CHARS).collect();
        truncated.push('\u{2026}');
        truncated
    } else {
        msg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_valid_git_ref ─────────────────────────────────────────────────────

    #[test]
    fn valid_ref_simple_branch_name() {
        assert!(is_valid_git_ref("main"));
    }

    #[test]
    fn valid_ref_develop_branch() {
        assert!(is_valid_git_ref("develop"));
    }

    #[test]
    fn valid_ref_with_slash() {
        assert!(is_valid_git_ref("feature/my-branch"));
    }

    #[test]
    fn valid_ref_tag_with_dots() {
        assert!(is_valid_git_ref("v1.0.0"));
    }

    #[test]
    fn valid_ref_sha_hex_string() {
        assert!(is_valid_git_ref("a1b2c3d4e5f678901234567890abcdef01234567"));
    }

    #[test]
    fn valid_ref_with_at_sign() {
        assert!(is_valid_git_ref("refs@HEAD"));
    }

    #[test]
    fn valid_ref_with_plus() {
        assert!(is_valid_git_ref("refs/tags/v1.0+patch"));
    }

    #[test]
    fn valid_ref_exactly_256_chars() {
        let r = "a".repeat(256);
        assert!(is_valid_git_ref(&r), "256-char ref must be valid");
    }

    #[test]
    fn valid_ref_with_underscore() {
        assert!(is_valid_git_ref("release_candidate_1"));
    }

    #[test]
    fn valid_ref_with_hyphen() {
        assert!(is_valid_git_ref("my-feature-branch"));
    }

    #[test]
    fn valid_ref_nested_path() {
        assert!(is_valid_git_ref("refs/heads/main"));
    }

    #[test]
    fn invalid_ref_empty_string() {
        assert!(!is_valid_git_ref(""));
    }

    #[test]
    fn invalid_ref_257_chars_too_long() {
        let r = "a".repeat(257);
        assert!(!is_valid_git_ref(&r), "257-char ref must be invalid");
    }

    #[test]
    fn invalid_ref_starts_with_slash() {
        assert!(!is_valid_git_ref("/etc/passwd"));
    }

    #[test]
    fn invalid_ref_contains_double_dot() {
        assert!(!is_valid_git_ref("branch..other"));
    }

    #[test]
    fn invalid_ref_path_traversal_attempt() {
        assert!(!is_valid_git_ref("../../etc/passwd"));
    }

    #[test]
    fn invalid_ref_contains_space() {
        assert!(!is_valid_git_ref("branch name"));
    }

    #[test]
    fn invalid_ref_semicolon_injection() {
        assert!(!is_valid_git_ref("branch;rm -rf /"));
    }

    #[test]
    fn invalid_ref_backtick_injection() {
        assert!(!is_valid_git_ref("branch`cmd`"));
    }

    #[test]
    fn invalid_ref_dollar_sign() {
        assert!(!is_valid_git_ref("$PATH"));
    }

    #[test]
    fn invalid_ref_angle_bracket() {
        assert!(!is_valid_git_ref("<script>alert(1)</script>"));
    }

    #[test]
    fn invalid_ref_newline() {
        assert!(!is_valid_git_ref("branch\ninjected"));
    }

    #[test]
    fn invalid_ref_null_byte() {
        assert!(!is_valid_git_ref("branch\0name"));
    }

    #[test]
    fn invalid_ref_pipe_char() {
        assert!(!is_valid_git_ref("branch|cmd"));
    }

    #[test]
    fn invalid_ref_exclamation_mark() {
        assert!(!is_valid_git_ref("branch!"));
    }

    // ── make_label ───────────────────────────────────────────────────────────

    #[test]
    fn make_label_simple_repo_url() {
        assert_eq!(
            make_label("https://github.com/owner/myrepo", "main"),
            "myrepo_at_main_sloc"
        );
    }

    #[test]
    fn make_label_repo_url_with_git_extension() {
        assert_eq!(
            make_label("https://github.com/owner/myrepo.git", "v1.0"),
            "myrepo_at_v1.0_sloc"
        );
    }

    #[test]
    fn make_label_trailing_slash_stripped() {
        assert_eq!(
            make_label("https://github.com/owner/myrepo/", "main"),
            "myrepo_at_main_sloc"
        );
    }

    #[test]
    fn make_label_ref_slash_becomes_underscore() {
        let label = make_label("https://github.com/owner/repo", "feature/my-branch");
        assert_eq!(label, "repo_at_feature_my-branch_sloc");
    }

    #[test]
    fn make_label_at_sign_in_ref_becomes_underscore() {
        let label = make_label("https://github.com/owner/repo", "v1@tag");
        assert_eq!(label, "repo_at_v1_tag_sloc");
    }

    #[test]
    fn make_label_plus_in_ref_becomes_underscore() {
        let label = make_label("https://github.com/owner/repo", "v1.0+patch");
        assert_eq!(label, "repo_at_v1.0_patch_sloc");
    }

    #[test]
    fn make_label_compare_ref_name() {
        assert_eq!(
            make_label("https://github.com/owner/repo.git", "compare"),
            "repo_at_compare_sloc"
        );
    }

    #[test]
    fn make_label_short_sha() {
        let label = make_label("https://github.com/owner/repo", "a1b2c3d4");
        assert_eq!(label, "repo_at_a1b2c3d4_sloc");
    }

    #[test]
    fn make_label_gitlub_style_path() {
        assert_eq!(
            make_label("https://gitlab.com/group/subgroup/myproject.git", "main"),
            "myproject_at_main_sloc"
        );
    }

    #[test]
    fn make_label_empty_repo_uses_rsplit_fallback() {
        let label = make_label("", "main");
        assert_eq!(label, "_at_main_sloc");
    }

    // ── json_error ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn json_error_not_found_status() {
        use http_body_util::BodyExt;
        let resp = json_error(StatusCode::NOT_FOUND, "resource missing");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&bytes);
        assert!(body.contains("resource missing"));
        assert!(body.contains("error"));
    }

    #[tokio::test]
    async fn json_error_bad_request_status() {
        use http_body_util::BodyExt;
        let resp = json_error(StatusCode::BAD_REQUEST, "invalid ref");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&bytes);
        assert!(body.contains("invalid ref"));
    }

    #[tokio::test]
    async fn json_error_bad_gateway_status() {
        use http_body_util::BodyExt;
        let resp = json_error(StatusCode::BAD_GATEWAY, "repo unreachable");
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&bytes);
        assert!(body.contains("repo unreachable"));
    }

    #[tokio::test]
    async fn json_error_internal_server_error_status() {
        use http_body_util::BodyExt;
        let resp = json_error(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error");
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8_lossy(&bytes);
        assert!(body.contains("Internal server error"));
    }

    // ── is_valid_git_ref — additional valid patterns ─────────────────────────

    #[test]
    fn valid_ref_refs_tags_path() {
        assert!(is_valid_git_ref("refs/tags/v2.0.0"));
    }

    #[test]
    fn valid_ref_commit_sha_40_chars() {
        let sha = "abcdef0123456789abcdef0123456789abcdef01";
        assert_eq!(sha.len(), 40);
        assert!(is_valid_git_ref(sha));
    }

    #[test]
    fn valid_ref_single_char() {
        assert!(is_valid_git_ref("a"));
    }

    #[test]
    fn valid_ref_feature_with_ticket_number() {
        assert!(is_valid_git_ref("feature/JIRA-1234"));
    }

    // ── is_valid_git_ref — additional invalid patterns ────────────────────────

    #[test]
    fn invalid_ref_double_dot_at_start() {
        assert!(!is_valid_git_ref("..main"));
    }

    #[test]
    fn invalid_ref_double_dot_at_end() {
        assert!(!is_valid_git_ref("main.."));
    }

    #[test]
    fn invalid_ref_backslash() {
        assert!(!is_valid_git_ref("branch\\name"));
    }

    #[test]
    fn invalid_ref_asterisk() {
        assert!(!is_valid_git_ref("branch*glob"));
    }

    #[test]
    fn invalid_ref_question_mark() {
        assert!(!is_valid_git_ref("branch?name"));
    }

    #[test]
    fn invalid_ref_colon() {
        assert!(!is_valid_git_ref("branch:ref"));
    }

    #[test]
    fn invalid_ref_caret() {
        assert!(!is_valid_git_ref("HEAD^1"));
    }

    #[test]
    fn invalid_ref_tilde() {
        assert!(!is_valid_git_ref("HEAD~2"));
    }

    #[test]
    fn invalid_ref_left_bracket() {
        assert!(!is_valid_git_ref("branch[0]"));
    }

    // ── make_label — edge cases ───────────────────────────────────────────────

    #[test]
    fn make_label_only_git_extension() {
        // repo URL that is just ".git" after stripping
        let label = make_label("ssh://git@host/.git", "main");
        assert!(!label.is_empty());
        assert!(label.ends_with("_sloc"));
    }

    #[test]
    fn make_label_numeric_in_ref() {
        let label = make_label("https://github.com/org/repo", "release-2024-12-31");
        assert_eq!(label, "repo_at_release-2024-12-31_sloc");
    }

    #[test]
    fn make_label_special_chars_in_ref_become_underscore() {
        // Braces are not in the alphanumeric / - . set, so they become _
        let label = make_label("https://github.com/org/repo", "branch{name}");
        assert!(label.contains("branch"), "must contain 'branch'");
        assert!(!label.contains('{'), "curly braces must be replaced");
        assert!(!label.contains('}'), "curly braces must be replaced");
    }

    #[test]
    fn make_label_dot_in_ref_preserved() {
        let label = make_label("https://github.com/org/repo.git", "v1.2.3");
        assert_eq!(label, "repo_at_v1.2.3_sloc");
    }

    #[test]
    fn make_label_hyphen_in_ref_preserved() {
        let label = make_label("https://github.com/org/repo", "hot-fix-123");
        assert_eq!(label, "repo_at_hot-fix-123_sloc");
    }
}
