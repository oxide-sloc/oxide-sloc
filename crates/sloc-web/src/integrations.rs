// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>
//
// Combined Integrations page — webhook-based automated scanning and Confluence
// report publishing in a single tabbed view at /integrations.

use askama::Template;
use axum::{
    extract::State,
    response::{Html, IntoResponse},
};

use super::{AppState, CspNonce};

// ── template ──────────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(
    source = r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>OxideSLOC — Integrations</title>
  <link rel="icon" type="image/png" href="/images/logo/small-logo.png">
  <link rel="stylesheet" href="/static/app.css">
  <script src="/static/app.js"></script>
  <style nonce="{{ csp_nonce }}">
    :root{--radius:14px;--bg:#f5efe8;--surface:rgba(255,255,255,0.9);--surface-2:#fbf7f2;--line:#e6d0bf;--line-strong:#d8bfad;--text:#43342d;--muted:#7b675b;--muted-2:#7b675b;--nav:#b85d33;--nav-2:#7a371b;--oxide-2:#b85d33;--shadow:0 8px 24px rgba(77,44,20,0.10);}
    body.dark-theme{--bg:#1b1511;--surface:#261c17;--surface-2:#2d221d;--line:#524238;--text:#f5ece6;--muted:#c7b7aa;--muted-2:#c7b7aa;--shadow:0 8px 24px rgba(0,0,0,0.32);}
    *{box-sizing:border-box;}html,body{margin:0;min-height:100vh;font-family:Inter,ui-sans-serif,system-ui,-apple-system,sans-serif;background:var(--bg);color:var(--text);}body{display:flex;flex-direction:column;}
    .background-watermarks{position:fixed;inset:0;pointer-events:none;z-index:0;overflow:hidden;}
    .background-watermarks img{position:absolute;opacity:0.16;filter:blur(0.3px);user-select:none;max-width:none;}
    .code-particles{position:fixed;inset:0;pointer-events:none;z-index:0;overflow:hidden;}
    .code-particle{position:absolute;font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:11px;font-weight:600;color:var(--oxide-2);opacity:0;white-space:nowrap;user-select:none;animation:floatCode linear infinite;}
    @keyframes floatCode{0%{opacity:0;transform:translateY(0) rotate(var(--rot));}10%{opacity:var(--op);}85%{opacity:var(--op);}100%{opacity:0;transform:translateY(-200px) rotate(var(--rot));}}
    .top-nav{position:sticky;top:0;z-index:30;background:linear-gradient(180deg,var(--nav),var(--nav-2));border-bottom:1px solid rgba(255,255,255,0.12);box-shadow:0 4px 14px rgba(0,0,0,0.18);}
    .top-nav-inner{max-width:1720px;margin:0 auto;padding:4px 24px;min-height:56px;display:flex;align-items:center;gap:14px;}
    .brand{display:flex;align-items:center;gap:12px;text-decoration:none;}
    .brand-logo{width:36px;height:40px;object-fit:contain;}
    .brand-title{color:#fff;font-size:16px;font-weight:800;}.brand-sub{color:rgba(255,255,255,0.75);font-size:12px;}
    .nav-right{margin-left:auto;display:flex;align-items:center;gap:10px;}
    @media(max-width:1400px){.nav-right{gap:6px;}.nav-pill,.theme-toggle{padding:0 10px;}}
    @media(max-width:1150px){.nav-right{gap:4px;}.nav-pill,.nav-dropdown-btn,.theme-toggle{padding:0 8px;font-size:11px;min-height:34px;}.brand-sub{display:none;}.server-online-pill{width:34px;padding:0;justify-content:center;font-size:0;gap:0;min-height:34px;}}
    .nav-pill{display:inline-flex;align-items:center;min-height:34px;padding:0 14px;border-radius:999px;border:1px solid rgba(255,255,255,0.18);color:#fff;background:rgba(255,255,255,0.08);font-size:12px;font-weight:700;text-decoration:none;}
    .nav-pill:hover{background:rgba(255,255,255,0.18);}
    .nav-dropdown{position:relative;display:inline-flex;}.nav-dropdown-btn{cursor:pointer;background:rgba(255,255,255,0.08);border:1px solid rgba(255,255,255,0.18);color:#fff;border-radius:999px;padding:0 14px;min-height:34px;font-size:12px;font-weight:700;display:inline-flex;align-items:center;gap:6px;text-decoration:none;}.nav-dropdown-btn:hover,.nav-dropdown:focus-within .nav-dropdown-btn{background:rgba(255,255,255,0.18);}.nav-dropdown-menu{opacity:0;visibility:hidden;position:absolute;top:calc(100% + 8px);right:0;background:linear-gradient(180deg,var(--nav),var(--nav-2));border:1px solid rgba(255,255,255,0.15);border-radius:12px;min-width:165px;overflow:hidden;box-shadow:0 10px 28px rgba(0,0,0,0.28);z-index:100;transition:opacity 0.13s ease,visibility 0s ease 0.13s;}.nav-dropdown:hover .nav-dropdown-menu,.nav-dropdown:focus-within .nav-dropdown-menu{opacity:1;visibility:visible;transition:opacity 0.13s ease,visibility 0s ease 0s;}.nav-dropdown-menu a{display:flex;align-items:center;gap:9px;padding:11px 16px;color:rgba(255,255,255,0.92);text-decoration:none;font-size:12px;font-weight:700;border-bottom:1px solid rgba(255,255,255,0.10);}.nav-dropdown-menu a:last-child{border-bottom:none;}.nav-dropdown-menu a:hover{background:rgba(255,255,255,0.14);color:#fff;}.nav-dropdown-menu a svg{width:13px;height:13px;stroke:currentColor;fill:none;stroke-width:2;flex:0 0 auto;}
    .page{width:100%;max-width:1720px;margin:0 auto;padding:32px 24px 36px;position:relative;z-index:1;}
    @media (max-width:1920px) { .top-nav-inner { max-width:1500px; } .page { max-width:1500px; } }
    h1{font-size:26px;font-weight:850;margin:0 0 6px;letter-spacing:-0.03em;}
    .subtitle{color:var(--muted);font-size:14px;margin:0 0 24px;}
    .card{background:var(--surface);border:1px solid var(--line);border-radius:var(--radius);padding:24px;box-shadow:var(--shadow);margin-bottom:20px;}
    .card-title{font-size:15px;font-weight:800;margin:0 0 18px;}
    .form-row{display:grid;grid-template-columns:1fr 1fr;gap:14px;margin-bottom:14px;}
    @media(max-width:600px){.form-row{grid-template-columns:1fr;}}
    .form-group{display:flex;flex-direction:column;gap:5px;}
    label{font-size:12px;font-weight:700;color:var(--muted);}
    input,select{padding:9px 12px;border-radius:8px;border:1.5px solid var(--line-strong);background:var(--surface-2);color:var(--text);font-size:13px;width:100%;}
    input:focus,select:focus{outline:none;border-color:var(--oxide-2);}
    .btn{display:inline-flex;align-items:center;gap:7px;padding:9px 18px;border-radius:9px;border:none;cursor:pointer;font-size:13px;font-weight:700;transition:opacity 0.15s;}
    .btn:hover{opacity:0.85;}.btn-primary{background:var(--oxide-2);color:#fff;}.btn-secondary{background:var(--surface-2);color:var(--text);border:1.5px solid var(--line-strong);}.btn-danger{background:#dc2626;color:#fff;}.btn-sm{padding:5px 12px;font-size:12px;border-radius:7px;}
    .btn-row{display:flex;gap:10px;margin-top:16px;flex-wrap:wrap;}
    .status-msg{padding:10px 14px;border-radius:8px;font-size:13px;font-weight:600;margin-top:12px;display:none;}
    .status-ok{background:#dcfce7;color:#166534;display:block!important;}.status-err{background:#fee2e2;color:#991b1b;display:block!important;}
    body.dark-theme .status-ok{background:#14532d;color:#86efac;}body.dark-theme .status-err{background:#450a0a;color:#fca5a5;}
    /* Webhooks tab */
    .schedule-list{display:flex;flex-direction:column;gap:12px;}
    .sched-item{background:var(--surface-2);border:1px solid var(--line);border-radius:10px;padding:16px 20px;}
    .sched-header{display:flex;align-items:center;gap:10px;margin-bottom:8px;}
    .sched-label{font-size:14px;font-weight:800;}
    .sched-badge{font-size:10px;font-weight:700;padding:2px 9px;border-radius:999px;letter-spacing:.05em;}
    .badge-webhook{background:#dbeafe;color:#1d4ed8;}body.dark-theme .badge-webhook{background:#1e3a5f;color:#93c5fd;}
    .badge-poll{background:#dcfce7;color:#166534;}body.dark-theme .badge-poll{background:#14532d;color:#86efac;}
    .sched-meta{font-size:12px;color:var(--muted);display:flex;flex-direction:column;gap:4px;}
    .sched-secret{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:11px;color:var(--oxide-2);background:var(--surface);border:1px solid var(--line);border-radius:6px;padding:2px 7px;}
    .sched-actions{display:flex;gap:8px;margin-top:12px;}
    .url-row{display:flex;align-items:center;gap:8px;margin-bottom:8px;}
    .url-label{font-size:12px;font-weight:700;min-width:80px;color:var(--muted);}
    .url-box{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:11px;background:var(--surface-2);border:1px solid var(--line);border-radius:6px;padding:5px 10px;flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:var(--text);}
    .copy-btn{padding:4px 10px;font-size:11px;border-radius:6px;border:1px solid var(--line);background:var(--surface);cursor:pointer;font-weight:700;color:var(--muted);}
    .copy-btn:hover{background:var(--line);}
    .empty-state{text-align:center;padding:32px;color:var(--muted);font-size:14px;}
    .empty-rich{text-align:center;padding:40px 20px 32px;}
    .empty-rich-icon{width:52px;height:52px;border-radius:14px;background:var(--surface-2);border:2px solid var(--line);display:inline-flex;align-items:center;justify-content:center;margin-bottom:13px;}
    .empty-rich-icon svg{width:26px;height:26px;stroke:var(--muted);fill:none;stroke-width:1.5;}
    .empty-rich-title{font-size:14px;font-weight:700;color:var(--text);margin:0 0 5px;}
    .empty-rich-desc{font-size:12px;color:var(--muted);max-width:400px;margin:0 auto 16px;line-height:1.55;}
    .empty-rich-tips{display:flex;flex-direction:column;gap:7px;max-width:360px;margin:0 auto;text-align:left;}
    .empty-rich-tip{display:flex;align-items:flex-start;gap:9px;font-size:12px;color:var(--muted);line-height:1.5;background:var(--surface-2);border:1px solid var(--line);border-radius:8px;padding:9px 12px;}
    .empty-rich-tip svg{width:14px;height:14px;stroke:var(--oxide-2);fill:none;stroke-width:2;flex-shrink:0;margin-top:1px;}
    .empty-rich-tip strong{color:var(--text);}
    /* Confluence tab */
    .tier-radio-row{display:flex;gap:18px;margin-bottom:14px;}
    .tier-radio{display:flex;align-items:center;gap:7px;cursor:pointer;font-size:13px;font-weight:700;}
    .tier-radio input[type=radio]{width:auto;accent-color:var(--oxide-2);}
    .sched-auto-row{display:flex;align-items:center;justify-content:space-between;padding:10px 14px;border-radius:8px;background:var(--surface-2);border:1px solid var(--line);margin-bottom:8px;}
    .sched-auto-label{font-size:13px;font-weight:600;}
    .toggle-switch{position:relative;display:inline-block;width:40px;height:22px;}
    .toggle-switch input{opacity:0;width:0;height:0;}
    .toggle-slider{position:absolute;cursor:pointer;inset:0;background:var(--line-strong);border-radius:999px;transition:.2s;}
    .toggle-slider:before{content:'';position:absolute;height:16px;width:16px;left:3px;bottom:3px;background:#fff;border-radius:50%;transition:.2s;}
    input:checked+.toggle-slider{background:var(--oxide-2);}
    input:checked+.toggle-slider:before{transform:translateX(18px);}
    .label-row{display:flex;align-items:center;gap:5px;}
    .info-btn{position:relative;width:15px;height:15px;border-radius:50%;border:1.5px solid var(--muted);background:transparent;color:var(--muted);font-size:9px;font-weight:900;cursor:pointer;display:inline-flex;align-items:center;justify-content:center;padding:0;flex-shrink:0;line-height:1;}
    .info-btn:hover,.info-btn:focus{border-color:var(--oxide-2);color:var(--oxide-2);outline:none;}
    .info-tip{display:none;position:absolute;left:0;top:calc(100% + 6px);z-index:200;background:var(--surface);border:1px solid var(--line-strong);border-radius:8px;box-shadow:0 6px 18px rgba(0,0,0,0.16);padding:9px 11px;font-size:11.5px;font-weight:500;color:var(--text);line-height:1.55;width:320px;white-space:normal;pointer-events:none;}
    .info-tip::before{content:'';position:absolute;bottom:100%;left:8px;border:5px solid transparent;border-bottom-color:var(--line-strong);}
    .info-btn:hover .info-tip,.info-btn:focus .info-tip{display:block;}
    body.dark-theme .info-tip{background:var(--surface-2);box-shadow:0 6px 18px rgba(0,0,0,0.36);}
    /* Integrations module */
    .integrations-module{background:var(--surface);border:1px solid var(--line);border-radius:var(--radius);box-shadow:var(--shadow);margin-bottom:12px;}
    .integrations-header{padding:26px 28px 20px;}
    .integrations-header h1{margin-bottom:6px;}
    .integrations-header .subtitle{margin-bottom:0;}
    /* Tab bar */
    .tab-bar{display:flex;gap:8px;margin-top:16px;flex-wrap:wrap;}
    .tab-btn{display:inline-flex;align-items:center;gap:8px;padding:8px 27px;border-radius:999px;border:1.5px solid var(--line);background:var(--bg);color:var(--muted);font-size:13px;font-weight:700;cursor:pointer;transition:background 0.12s,color 0.12s,border-color 0.12s;}
    .tab-btn svg{width:16px;height:16px;stroke:currentColor;fill:none;stroke-width:2;flex:0 0 auto;}
    .tab-btn:hover{background:#efe6dc;color:var(--text);}
    body.dark-theme .tab-btn:hover{background:#372922;color:var(--text);}
    .tab-btn.active{background:var(--oxide-2);color:#fff;border-color:var(--oxide-2);}
    .tab-btn.active svg{stroke:#fff;}
    .tab-pane{display:none;}
    .tab-pane.active{display:block;}
    /* Shared misc */
    .status-dot{width:8px;height:8px;border-radius:999px;background:#26d768;box-shadow:0 0 0 4px rgba(38,215,104,0.14);flex:0 0 auto;}
    .server-status-wrap{position:relative;display:inline-flex;}.server-online-pill{cursor:default;gap:7px;}.server-status-tip{display:none;position:absolute;top:calc(100% + 10px);right:0;z-index:100;background:rgba(20,12,8,0.97);color:rgba(255,255,255,0.92);border-radius:10px;padding:10px 14px;font-size:12px;font-weight:500;line-height:1.55;white-space:nowrap;box-shadow:0 8px 24px rgba(0,0,0,0.32);pointer-events:none;border:1px solid rgba(255,255,255,0.10);}.server-status-tip::before{content:'';position:absolute;bottom:100%;right:18px;border:6px solid transparent;border-bottom-color:rgba(20,12,8,0.97);}.server-status-wrap:hover .server-status-tip,.server-status-wrap:focus-within .server-status-tip{display:block;}
    .site-footer{text-align:center;padding:12px 24px;font-size:13px;color:var(--muted);position:relative;z-index:1;}
    .site-footer a{color:var(--muted);}
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
        <div><div class="brand-title">OxideSLOC</div><div class="brand-sub">Integrations</div></div></a>
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
          <a href="/git-browser" class="nav-dropdown-btn sx-8c38ef73" >Git Browser <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="6 9 12 15 18 9"></polyline></svg></a>
          <div class="nav-dropdown-menu">
            <a href="/code-ownership"><svg viewBox="0 0 24 24"><path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2"></path><circle cx="9" cy="7" r="4"></circle><path d="M23 21v-2a4 4 0 0 0-3-3.87"></path><path d="M16 3.13a4 4 0 0 1 0 7.75"></path></svg>Code Ownership</a>
            <a href="/integrations"><svg viewBox="0 0 24 24"><path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z"></path></svg>Integrations</a>
          </div>
        </div>
        <div class="server-status-wrap" id="server-status-wrap">
          <div class="nav-pill server-online-pill" id="server-status-pill">
            <span class="status-dot" id="status-dot"></span>
            <span id="server-status-label">Server</span>
            <span class="sx-d60f2ef3" id="server-ping-ms" ></span>
          </div>
          <div class="server-status-tip">
            OxideSLOC is running — accessible on your network.
            <span class="sx-238af6bc" id="server-tip-ping" ></span>
          </div>
        </div>
        <button type="button" class="theme-toggle" id="settings-btn" aria-label="Color scheme" title="Color scheme settings">
          <svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.8"><circle cx="12" cy="12" r="3"></circle><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path></svg>
        </button>
        <button class="theme-toggle" id="theme-toggle" type="button" title="Toggle theme">
          <svg class="icon-moon" viewBox="0 0 24 24"><path d="M21 12.79A9 9 0 1 1 11.21 3a7 7 0 0 0 9.79 9.79z"/></svg>
          <svg class="icon-sun" viewBox="0 0 24 24"><circle cx="12" cy="12" r="5"/><line x1="12" y1="1" x2="12" y2="3"/><line x1="12" y1="21" x2="12" y2="23"/><line x1="4.22" y1="4.22" x2="5.64" y2="5.64"/><line x1="18.36" y1="18.36" x2="19.78" y2="19.78"/><line x1="1" y1="12" x2="3" y2="12"/><line x1="21" y1="12" x2="23" y2="12"/><line x1="4.22" y1="19.78" x2="5.64" y2="18.36"/><line x1="18.36" y1="5.64" x2="19.78" y2="4.22"/></svg>
        </button>
      </div>
    </div>
  </nav>

  <div class="page">
    <div class="integrations-module">
      <div class="integrations-header">
        <h1>Integrations</h1>
        <p class="subtitle">Connect OxideSLOC to external platforms — automate scanning via webhooks, or publish results directly to Atlassian Confluence.</p>
        <div class="tab-bar">
          <button class="tab-btn" data-tab="webhooks" type="button">
            <svg viewBox="0 0 24 24"><path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z"></path></svg>
            Webhooks
          </button>
          <button class="tab-btn" data-tab="confluence" type="button">
            <svg viewBox="0 0 24 24"><path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z"></path><path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"></path></svg>
            Confluence
          </button>
        </div>
      </div>
    </div>

    <!-- ── Webhooks tab ─────────────────────────────────────────────────────── -->
    <div class="tab-pane" data-tab="webhooks">
      <div class="card">
        <div class="card-title">Add Schedule</div>
        <div class="form-row">
          <div class="form-group">
            <div class="label-row"><label>Label</label><button type="button" class="info-btn" aria-label="About label">i<span class="info-tip">A friendly name for this schedule shown in the Active Schedules list, e.g. "My Repo — main".</span></button></div>
            <input id="fLabel" type="text" placeholder="My Repo — main"/>
          </div>
          <div class="form-group">
            <div class="label-row"><label>Type</label><button type="button" class="info-btn" aria-label="About schedule type">i<span class="info-tip">Webhook: triggered instantly by push events from GitHub, GitLab, or Bitbucket. Polling: checks the branch on a fixed interval and scans when a new commit is detected.</span></button></div>
            <select id="fKind">
              <option value="webhook">Webhook (GitHub / GitLab / Bitbucket)</option>
              <option value="poll">Polling (interval-based)</option>
            </select>
          </div>
        </div>
        <div class="form-row">
          <div class="form-group">
            <div class="label-row"><label>Repository URL</label><button type="button" class="info-btn" aria-label="About repository URL">i<span class="info-tip">The git clone URL of the repository to scan. Must be accessible from this machine, e.g. https://github.com/owner/repo.git or git@github.com:owner/repo.git.</span></button></div>
            <input id="fRepo" type="text" placeholder="https://github.com/owner/repo.git"/>
          </div>
          <div class="form-group">
            <div class="label-row"><label>Branch</label><button type="button" class="info-btn" aria-label="About branch">i<span class="info-tip">The branch to monitor for new commits. Only pushes or polls on this branch will trigger a scan. Defaults to "main".</span></button></div>
            <input id="fBranch" type="text" value="main" placeholder="main"/>
          </div>
        </div>
        <div class="form-row" id="providerRow">
          <div class="form-group">
            <div class="label-row"><label>Provider</label><button type="button" class="info-btn" aria-label="About provider">i<span class="info-tip">The git hosting platform sending webhook payloads. Each provider signs payloads differently — select the one that matches where your repository is hosted.</span></button></div>
            <select id="fProvider"><option value="github">GitHub</option><option value="gitlab">GitLab</option><option value="bitbucket">Bitbucket</option></select>
          </div>
        </div>
        <div class="form-row sx-6aa34d74" id="pollRow" >
          <div class="form-group">
            <div class="label-row"><label>Poll Interval (seconds, min 60)</label><button type="button" class="info-btn" aria-label="About poll interval">i<span class="info-tip">How often (in seconds) to check the branch for new commits. Minimum is 60 seconds. Recommended: 300 (5 min) for active repos, 900+ for slower ones.</span></button></div>
            <input id="fInterval" type="number" min="60" step="60" value="300"/>
          </div>
        </div>
        <div id="addStatus" class="status-msg"></div>
        <button class="btn btn-primary" id="addScheduleBtn" type="button">Add Schedule</button>
      </div>

      <div class="card">
        <div class="card-title">Active Schedules</div>
        <div id="scheduleList" class="schedule-list"><div class="empty-state">Loading…</div></div>
      </div>

      <div class="card">
        <div class="card-title">Webhook Endpoint URLs</div>
        <p class="sx-48175f5b" >Configure these URLs in your provider and use the secret shown on each schedule for HMAC verification.</p>
        <div class="url-row"><span class="url-label">GitHub</span><span id="urlGH" class="url-box">{{ server_url }}/webhooks/github</span><button class="copy-btn" type="button" data-copy-target="urlGH">Copy</button></div>
        <div class="url-row"><span class="url-label">GitLab</span><span id="urlGL" class="url-box">{{ server_url }}/webhooks/gitlab</span><button class="copy-btn" type="button" data-copy-target="urlGL">Copy</button></div>
        <div class="url-row"><span class="url-label">Bitbucket</span><span id="urlBB" class="url-box">{{ server_url }}/webhooks/bitbucket</span><button class="copy-btn" type="button" data-copy-target="urlBB">Copy</button></div>
      </div>
    </div>

    <!-- ── Confluence tab ──────────────────────────────────────────────────── -->
    <div class="tab-pane" data-tab="confluence">
      <div class="card">
        <div class="card-title">Connection Settings</div>
        <div class="tier-radio-row">
          <label class="tier-radio"><input type="radio" name="tier" id="tierCloud" value="cloud" checked> Cloud (atlassian.net)</label>
          <label class="tier-radio"><input type="radio" name="tier" id="tierServer" value="server"> Server / Data Center</label>
        </div>
        <div class="form-row">
          <div class="form-group">
            <div class="label-row"><label id="labelBaseUrl">Base URL</label><button type="button" class="info-btn" aria-label="About Base URL">i<span class="info-tip" id="tipBaseUrl">Your Atlassian site URL, e.g. https://yourcompany.atlassian.net</span></button></div>
            <input id="fBaseUrl" type="url" placeholder="https://mycompany.atlassian.net"/>
          </div>
          <div class="form-group">
            <div class="label-row"><label id="labelUsername">Email</label><button type="button" class="info-btn" aria-label="About username field">i<span class="info-tip" id="tipUsername">Your Atlassian account email address. Found in your Atlassian profile at id.atlassian.com.</span></button></div>
            <input id="fUsername" type="text" placeholder="you@example.com"/>
          </div>
        </div>
        <div class="form-row">
          <div class="form-group">
            <div class="label-row"><label id="labelCredential">API Token</label><button type="button" class="info-btn" aria-label="About token field">i<span class="info-tip" id="tipCredential">Your Atlassian API token. Create one at id.atlassian.com → Security → API tokens. Leave blank to keep the saved token.</span></button></div>
            <input id="fCredential" type="password" placeholder="Leave blank to keep existing token"/>
          </div>
          <div class="form-group">
            <div class="label-row"><label>Space Key</label><button type="button" class="info-btn" aria-label="About space key">i<span class="info-tip">The short key identifying a Confluence space, e.g. ENG or PROJ. Find it in Space Settings → Space Details.</span></button></div>
            <input id="fSpaceKey" type="text" placeholder="ENG"/>
          </div>
        </div>
        <div class="form-row">
          <div class="form-group">
            <div class="label-row"><label>Parent Page ID <span class="sx-41eb337d" >(optional)</span></label><button type="button" class="info-btn" aria-label="About parent page ID">i<span class="info-tip">Numeric ID of an existing Confluence page. Find it in the page URL: .../pages/123456/.... Leave blank to create at space root.</span></button></div>
            <input id="fParentId" type="text" placeholder="Leave blank to create at space root"/>
          </div>
        </div>
        <div id="connStatus" class="status-msg"></div>
        <div class="btn-row">
          <button class="btn btn-secondary" id="testBtn" type="button">Test Connection</button>
          <button class="btn btn-primary" id="saveBtn" type="button">Save Settings</button>
        </div>
      </div>

      <div class="card">
        <div class="card-title">Auto-Post on Scheduled Scan</div>
        <p class="sx-48175f5b" >When a webhook or polling schedule triggers a scan, automatically create or update the linked Confluence page.</p>
        <div id="schedAutoList"><div class="empty-state">Loading schedules…</div></div>
        <button class="btn btn-primary sx-e268d771" id="saveAutoPostBtn" type="button" >Save Auto-Post Settings</button>
      </div>

      <div class="card" id="manualPostCard">
        <div class="card-title">Manual Post</div>
        <p class="sx-48175f5b" >Post any saved scan result to Confluence right now. Enter the Run ID from the scan result page.</p>
        <div class="form-row">
          <div class="form-group">
            <div class="label-row"><label>Run ID</label><button type="button" class="info-btn" aria-label="About run ID">i<span class="info-tip">The UUID of a completed scan. Copy it from the scan result page URL (/runs/result/uuid) or from the View Reports list.</span></button></div>
            <input id="mRunId" type="text" placeholder="Paste run UUID from scan result page"/>
          </div>
          <div class="form-group">
            <div class="label-row"><label>Page Title</label><button type="button" class="info-btn" aria-label="About page title">i<span class="info-tip">Title of the Confluence page to create or update. If a page with this exact title already exists it will be updated in place.</span></button></div>
            <input id="mPageTitle" type="text" placeholder="OxideSLOC Report — my-repo"/>
          </div>
        </div>
        <div class="form-row">
          <div class="form-group">
            <div class="label-row"><label>Report URL <span class="sx-41eb337d" >(optional)</span></label><button type="button" class="info-btn" aria-label="About report URL">i<span class="info-tip">A link back to the full interactive oxide-sloc HTML report. Embedded as a hyperlink in the Confluence page. Leave blank to omit.</span></button></div>
            <input id="mReportUrl" type="url" placeholder="http://127.0.0.1:4317/runs/result/..."/>
          </div>
        </div>
        <div id="manualStatus" class="status-msg"></div>
        <div class="btn-row">
          <button class="btn btn-primary" id="manualPostBtn" type="button">Post to Confluence</button>
          <button class="btn btn-secondary" id="manualCopyBtn" type="button">Copy Wiki Markup</button>
        </div>
      </div>
    </div>
  </div>

  <script nonce="{{ csp_nonce }}">
  (function () {
    // ── Theme ─────────────────────────────────────────────────────────────────
    function applyTheme() { if (localStorage.getItem('sloc-theme') === 'dark') document.body.classList.add('dark-theme'); }
    function toggleTheme() { var d = document.body.classList.toggle('dark-theme'); localStorage.setItem('sloc-theme', d ? 'dark' : 'light'); }

    // ── Settings modal ────────────────────────────────────────────────────────
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

    // ── Tab switching ─────────────────────────────────────────────────────────
    function showTab(name) {
      document.querySelectorAll('.tab-btn').forEach(function(b) { b.classList.toggle('active', b.dataset.tab === name); });
      document.querySelectorAll('.tab-pane').forEach(function(p) { p.classList.toggle('active', p.dataset.tab === name); });
    }
    showTab('webhooks');
    document.querySelectorAll('.tab-btn').forEach(function(b) {
      b.addEventListener('click', function() { showTab(b.dataset.tab); });
    });

    // ── Shared helper ─────────────────────────────────────────────────────────
    function esc(s) { return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;'); }

    // ─────────────────────────────────────────────────────────────────────────
    // WEBHOOKS TAB
    // ─────────────────────────────────────────────────────────────────────────

    function onKindChange() {
      var poll = document.getElementById('fKind').value === 'poll';
      document.getElementById('pollRow').style.display = poll ? 'grid' : 'none';
      document.getElementById('providerRow').style.display = poll ? 'none' : 'grid';
    }

    function showAddStatus(msg, ok) {
      var el = document.getElementById('addStatus');
      el.style.display = 'block';
      el.className = 'status-msg ' + (ok ? 'status-ok' : 'status-err');
      el.textContent = msg;
    }

    async function addSchedule() {
      var kind = document.getElementById('fKind').value;
      var body = {
        label: document.getElementById('fLabel').value.trim() || 'Unnamed',
        repo_url: document.getElementById('fRepo').value.trim(),
        branch: document.getElementById('fBranch').value.trim() || 'main',
        kind: kind,
        provider: kind === 'webhook' ? document.getElementById('fProvider').value : null,
        interval_secs: kind === 'poll' ? parseInt(document.getElementById('fInterval').value, 10) : null,
      };
      if (!body.repo_url) { showAddStatus('Repository URL is required.', false); return; }
      var r = await fetch('/api/schedules', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
      var data = await r.json();
      if (r.ok) { showAddStatus('Schedule added.', true); loadWebhookSchedules(); }
      else { showAddStatus(data.error || 'Failed.', false); }
    }

    async function deleteSchedule(id) {
      if (!confirm('Delete this schedule?')) return;
      await fetch('/api/schedules?id=' + encodeURIComponent(id), { method: 'DELETE' });
      loadWebhookSchedules();
    }

    async function loadWebhookSchedules() {
      var r = await fetch('/api/schedules');
      if (!r.ok) return;
      var data = await r.json();
      var el = document.getElementById('scheduleList');
      var list = data.schedules || [];
      if (!list.length) {
        el.innerHTML = '<div class="empty-rich">'
          + '<div class="empty-rich-icon"><svg viewBox="0 0 24 24"><path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z"></path></svg></div>'
          + '<div class="empty-rich-title">No schedules configured yet</div>'
          + '<div class="empty-rich-desc">Add a webhook or polling schedule below to automatically trigger SLOC scans whenever code is pushed to your repository.</div>'
          + '<div class="empty-rich-tips">'
          + '<div class="empty-rich-tip"><svg viewBox="0 0 24 24"><polyline points="22 12 18 12 15 21 9 3 6 12 2 12"/></svg><span><strong>Webhook</strong> \u2014 GitHub, GitLab, or Bitbucket sends a push event and a scan triggers instantly.</span></div>'
          + '<div class="empty-rich-tip"><svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg><span><strong>Polling</strong> \u2014 OxideSLOC checks the branch on a fixed interval and scans when a new commit appears.</span></div>'
          + '</div>'
          + '</div>';
        return;
      }
      el.innerHTML = list.map(function (s) {
        var badge = s.kind === 'webhook' ? '<span class="sched-badge badge-webhook">Webhook</span>' : '<span class="sched-badge badge-poll">Poll</span>';
        var extra = s.interval_secs ? ' \u00b7 every ' + s.interval_secs + 's' : (s.provider && s.provider !== 'any' ? ' \u00b7 ' + esc(s.provider) : '');
        var rawSecret = s.webhook_secret || '';
        var maskedSecret = rawSecret.length > 4
          ? '••••••••' + esc(rawSecret.slice(-4))
          : rawSecret.length > 0 ? '••••' : '';
        var secret = maskedSecret ? '<div>Secret: <span class="sched-secret">' + maskedSecret + '</span></div>' : '';
        var last = s.last_scan_at ? 'Last scanned: ' + new Date(s.last_scan_at).toLocaleString() : 'Not yet scanned';
        return '<div class="sched-item">'
          + '<div class="sched-header">' + badge + '<span class="sched-label">' + esc(s.label) + '</span></div>'
          + '<div class="sched-meta"><div>' + esc(s.repo_url) + ' \u00b7 <strong>' + esc(s.branch) + '</strong>' + extra + '</div>' + secret + '<div>' + last + '</div></div>'
          + '<div class="sched-actions"><button class="btn btn-danger btn-sm" data-action="delete-schedule" data-id="' + esc(s.id) + '" type="button">Remove</button></div>'
          + '</div>';
      }).join('');
    }

    function copyUrl(id) { navigator.clipboard.writeText(document.getElementById(id).textContent.trim()); }

    // ─────────────────────────────────────────────────────────────────────────
    // CONFLUENCE TAB
    // ─────────────────────────────────────────────────────────────────────────

    function onTierChange() {
      var cloud = document.querySelector('input[name=tier]:checked').value === 'cloud';
      document.getElementById('labelUsername').textContent = cloud ? 'Email' : 'Username';
      document.getElementById('labelCredential').textContent = cloud ? 'API Token' : 'Password / PAT';
      document.getElementById('labelBaseUrl').textContent = cloud ? 'Base URL (atlassian.net)' : 'Base URL';
      document.getElementById('fUsername').placeholder = cloud ? 'you@example.com' : 'username (blank if using PAT)';
      var tb = document.getElementById('tipBaseUrl');
      if (tb) tb.textContent = cloud
        ? 'Your Atlassian site URL, e.g. https://yourcompany.atlassian.net.'
        : 'Root URL of your Confluence instance, e.g. https://confluence.corp.com.';
      var tu = document.getElementById('tipUsername');
      if (tu) tu.textContent = cloud
        ? 'Your Atlassian account email address. Found in your profile at id.atlassian.com.'
        : 'Your Confluence username. Leave blank if authenticating with a PAT.';
      var tc = document.getElementById('tipCredential');
      if (tc) tc.textContent = cloud
        ? 'Your Atlassian API token. Create one at id.atlassian.com \u2192 Security \u2192 API tokens. Leave blank to keep the saved token.'
        : 'Your Confluence password or PAT. Leave blank to keep the saved value.';
    }
    document.querySelectorAll('input[name=tier]').forEach(function(r) { r.addEventListener('change', onTierChange); });

    function showStatus(elId, msg, ok) {
      var el = document.getElementById(elId);
      el.textContent = msg;
      el.className = 'status-msg ' + (ok ? 'status-ok' : 'status-err');
    }

    async function loadConfluenceConfig() {
      var r = await fetch('/api/confluence/config');
      if (!r.ok) return;
      var d = await r.json();
      var cloud = d.tier !== 'server';
      document.getElementById(cloud ? 'tierCloud' : 'tierServer').checked = true;
      onTierChange();
      document.getElementById('fBaseUrl').value = d.base_url || '';
      document.getElementById('fUsername').value = d.username || '';
      document.getElementById('fSpaceKey').value = d.space_key || '';
      document.getElementById('fParentId').value = d.parent_page_id || '';
      if (d.api_token_set) {
        document.getElementById('fCredential').placeholder = '\u2022\u2022\u2022\u2022\u2022\u2022\u2022\u2022  (saved \u2014 leave blank to keep)';
      }
      return d;
    }

    function collectAutoPost() {
      var result = {};
      document.querySelectorAll('[data-sched-id]').forEach(function(cb) { result[cb.dataset.schedId] = cb.checked; });
      return result;
    }

    async function saveConfluenceConfig() {
      var btn = document.getElementById('saveBtn');
      btn.disabled = true;
      var autoPost = collectAutoPost();
      var body = {
        tier: document.querySelector('input[name=tier]:checked').value,
        base_url: document.getElementById('fBaseUrl').value.trim(),
        username: document.getElementById('fUsername').value.trim(),
        credential: document.getElementById('fCredential').value,
        space_key: document.getElementById('fSpaceKey').value.trim(),
        parent_page_id: document.getElementById('fParentId').value.trim() || null,
        schedule_auto_post: autoPost,
      };
      if (!body.base_url) { showStatus('connStatus', 'Base URL is required.', false); btn.disabled = false; return; }
      if (!body.space_key) { showStatus('connStatus', 'Space key is required.', false); btn.disabled = false; return; }
      var r = await fetch('/api/confluence/config', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
      var d = await r.json();
      if (d.ok) showStatus('connStatus', 'Settings saved.', true);
      else showStatus('connStatus', d.error || 'Save failed.', false);
      btn.disabled = false;
    }

    async function testConfluenceConnection() {
      var btn = document.getElementById('testBtn');
      btn.disabled = true;
      showStatus('connStatus', 'Testing\u2026', true);
      var r = await fetch('/api/confluence/test', { method: 'POST' });
      var d = await r.json();
      if (d.ok) showStatus('connStatus', 'Connection successful!', true);
      else showStatus('connStatus', 'Connection failed: ' + (d.error || 'Unknown error'), false);
      btn.disabled = false;
    }

    async function loadConfluenceSchedules(savedAutoPost) {
      var r = await fetch('/api/schedules');
      if (!r.ok) return;
      var d = await r.json();
      var list = d.schedules || [];
      var el = document.getElementById('schedAutoList');
      var saveBtn = document.getElementById('saveAutoPostBtn');
      if (!list.length) {
        el.innerHTML = '<div class="empty-state">No schedules configured yet. Add them in the <a href="#webhooks" id="goWebhooks">Webhooks tab</a>.</div>';
        var link = document.getElementById('goWebhooks');
        if (link) link.addEventListener('click', function(e) { e.preventDefault(); showTab('webhooks'); });
        return;
      }
      saveBtn.style.display = 'inline-flex';
      el.innerHTML = list.map(function(s) {
        var checked = (savedAutoPost && savedAutoPost[s.id]) ? 'checked' : '';
        return '<div class="sched-auto-row">'
          + '<span class="sched-auto-label">' + esc(s.label) + ' <small class="sx-b7896353" >\u00b7 ' + esc(s.branch) + '</small></span>'
          + '<label class="toggle-switch"><input type="checkbox" data-sched-id="' + esc(s.id) + '" ' + checked + '><span class="toggle-slider"></span></label></div>';
      }).join('');
    }

    async function saveAutoPost() {
      var btn = document.getElementById('saveAutoPostBtn');
      btn.disabled = true;
      var r = await fetch('/api/confluence/config');
      if (!r.ok) { btn.disabled = false; return; }
      var existing = await r.json();
      var body = {
        tier: existing.tier || 'cloud',
        base_url: existing.base_url || '',
        username: existing.username || '',
        credential: '',
        space_key: existing.space_key || '',
        parent_page_id: existing.parent_page_id || null,
        schedule_auto_post: collectAutoPost(),
      };
      var resp = await fetch('/api/confluence/config', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
      var d = await resp.json();
      if (d.ok) showStatus('connStatus', 'Auto-post settings saved.', true);
      else showStatus('connStatus', d.error || 'Save failed.', false);
      btn.disabled = false;
    }

    async function manualPost() {
      var btn = document.getElementById('manualPostBtn');
      btn.disabled = true;
      showStatus('manualStatus', 'Posting to Confluence\u2026', true);
      var body = {
        run_id: document.getElementById('mRunId').value.trim(),
        page_title: document.getElementById('mPageTitle').value.trim() || 'OxideSLOC Report',
        report_url: document.getElementById('mReportUrl').value.trim() || null,
      };
      if (!body.run_id) { showStatus('manualStatus', 'Run ID is required.', false); btn.disabled = false; return; }
      var r = await fetch('/api/confluence/post', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
      var d = await r.json();
      if (d.ok) showStatus('manualStatus', 'Posted! Page ID: ' + d.page_id, true);
      else showStatus('manualStatus', 'Error: ' + (d.error || 'Unknown error'), false);
      btn.disabled = false;
    }

    async function copyWikiMarkup() {
      var btn = document.getElementById('manualCopyBtn');
      var runId = document.getElementById('mRunId').value.trim();
      if (!runId) { showStatus('manualStatus', 'Enter a Run ID first.', false); return; }
      var r = await fetch('/api/confluence/wiki-markup?run_id=' + encodeURIComponent(runId));
      if (!r.ok) { showStatus('manualStatus', 'Could not load run (check Run ID).', false); return; }
      var text = await r.text();
      try {
        await navigator.clipboard.writeText(text);
        var orig = btn.textContent;
        btn.textContent = 'Copied!';
        setTimeout(function() { btn.textContent = orig; }, 2000);
      } catch(e) { showStatus('manualStatus', 'Clipboard write failed \u2014 check browser permissions.', false); }
    }

    // ── Background effects ────────────────────────────────────────────────────
    (function randomizeWatermarks() {
      var wms = Array.prototype.slice.call(document.querySelectorAll('.background-watermarks img'));
      if (!wms.length) return;
      var placed = [];
      function tooClose(top, left) { for (var i=0;i<placed.length;i++){if(Math.abs(placed[i][0]-top)<16&&Math.abs(placed[i][1]-left)<12)return true;} return false; }
      function pick(leftBand) { for (var attempt=0;attempt<50;attempt++){var top=Math.random()*88+2,left=leftBand?Math.random()*24+1:Math.random()*24+74;if(!tooClose(top,left)){placed.push([top,left]);return[top,left];}} var top=Math.random()*88+2,left=leftBand?Math.random()*24+1:Math.random()*24+74;placed.push([top,left]);return[top,left]; }
      var half = Math.floor(wms.length/2);
      wms.forEach(function(img,i){var pos=pick(i<half);var size=Math.floor(Math.random()*100+120);img.style.cssText='width:'+size+'px;top:'+pos[0].toFixed(1)+'%;left:'+pos[1].toFixed(1)+'%;transform:rotate('+(Math.random()*360).toFixed(1)+'deg);opacity:'+(Math.random()*0.08+0.12).toFixed(2)+';';});
    })();

    (function spawnCodeParticles() {
      var container = document.getElementById('code-particles');
      if (!container) return;
      var snippets = ['1,247 sloc','fn analyze()','code_lines','0 mixed','blanks: 312','// comment','pub fn run','use std::fs','Result<()>','let mut n = 0','git main','#[derive]','impl Scan','3,841 physical','files: 60','450 comments','cargo build','Ok(run)','Vec<String>','match lang','fn main() {','.rs .go .py','sloc_core','render_html','2,163 code'];
      for (var i=0;i<38;i++){(function(idx){var el=document.createElement('span');el.className='code-particle';el.textContent=snippets[idx%snippets.length];el.style.left=(Math.random()*94+2).toFixed(1)+'%';el.style.top=(Math.random()*88+6).toFixed(1)+'%';el.style.setProperty('--rot',(Math.random()*26-13).toFixed(1)+'deg');el.style.setProperty('--op',(Math.random()*0.09+0.06).toFixed(3));el.style.animationDuration=(Math.random()*10+9).toFixed(1)+'s';el.style.animationDelay='-'+(Math.random()*18).toFixed(1)+'s';container.appendChild(el);})(i);}
    })();

    // ── Event wiring ──────────────────────────────────────────────────────────
    document.getElementById('theme-toggle').addEventListener('click', toggleTheme);

    // Webhooks
    document.getElementById('fKind').addEventListener('change', onKindChange);
    document.getElementById('addScheduleBtn').addEventListener('click', addSchedule);
    document.addEventListener('click', function (e) {
      var copyBtn = e.target.closest('[data-copy-target]');
      if (copyBtn) { copyUrl(copyBtn.dataset.copyTarget); return; }
      var delBtn = e.target.closest('[data-action="delete-schedule"]');
      if (delBtn) { deleteSchedule(delBtn.dataset.id); }
    });

    // Confluence
    document.getElementById('testBtn').addEventListener('click', testConfluenceConnection);
    document.getElementById('saveBtn').addEventListener('click', saveConfluenceConfig);
    document.getElementById('saveAutoPostBtn').addEventListener('click', saveAutoPost);
    document.getElementById('manualPostBtn').addEventListener('click', manualPost);
    document.getElementById('manualCopyBtn').addEventListener('click', copyWikiMarkup);

    // ── Server ping ───────────────────────────────────────────────────────────
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

    // ── Init ──────────────────────────────────────────────────────────────────
    applyTheme();
    loadWebhookSchedules();
    loadConfluenceConfig().then(function(d) {
      loadConfluenceSchedules(d && d.schedule_auto_post);
    });
  })();
  </script>
  <footer class="site-footer">
    oxide-sloc v{{ version }} — local code analysis - metrics, history and reports
    &nbsp;·&nbsp; <em class="footer-mode sx-e01b0d98" id="footer-mode" >Mode: Local</em>
    &nbsp;·&nbsp; Built by <a href="https://github.com/NimaShafie" target="_blank" rel="noopener">Nima Shafie</a>
    &nbsp;·&nbsp; <a href="https://github.com/oxide-sloc/oxide-sloc" target="_blank" rel="noopener">View on GitHub</a>
    &nbsp;·&nbsp; <a href="https://www.gnu.org/licenses/agpl-3.0.html" target="_blank" rel="noopener">AGPL-3.0-or-later</a>
    &nbsp;·&nbsp; <a href="/report-bug" rel="noopener">Report a Bug</a> &nbsp;&middot;&nbsp; <a href="/api-docs" rel="noopener">REST API</a>
  </footer>
</body>
</html>"##,
    ext = "html"
)]
pub struct IntegrationsTemplate {
    pub csp_nonce: String,
    pub server_url: String,
    pub version: &'static str,
}

// ── handler ───────────────────────────────────────────────────────────────────

pub async fn integrations_handler(
    State(state): State<AppState>,
    axum::extract::Extension(CspNonce(csp_nonce)): axum::extract::Extension<CspNonce>,
) -> impl IntoResponse {
    let server_url = build_server_url(&state);
    let template = IntegrationsTemplate {
        csp_nonce,
        server_url,
        version: env!("CARGO_PKG_VERSION"),
    };
    Html(
        template
            .render()
            .unwrap_or_else(|e| format!("<pre>{e}</pre>")),
    )
}

fn build_server_url(state: &AppState) -> String {
    let addr = &state.base_config.web.bind_address;
    if state.tls_enabled {
        format!("https://{addr}")
    } else {
        format!("http://{addr}")
    }
}
