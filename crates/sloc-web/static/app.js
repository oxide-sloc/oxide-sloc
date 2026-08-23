// Applies dynamic, data-driven styles without inline `style="…"` attributes, so the
// Content-Security-Policy can forbid `style-src 'unsafe-inline'`.
//
// Any element carrying `data-sx-style="prop:val;prop:val"` has those declarations
// applied via the CSSOM property API (`style.setProperty`), which — unlike a parsed
// inline style attribute, `style.cssText`, or `setAttribute('style', …)` — is NOT
// governed by the CSP style-src directive. Values that must stay runtime-computed
// (chart bar widths, per-series colors, tree depth) travel in the data attribute.
(function () {
  "use strict";
  function applyOne(el) {
    var spec = el.getAttribute("data-sx-style");
    if (!spec) return;
    var decls = spec.split(";");
    for (var i = 0; i < decls.length; i++) {
      var d = decls[i];
      if (!d) continue;
      var c = d.indexOf(":");
      if (c < 0) continue;
      var prop = d.slice(0, c).trim();
      var val = d.slice(c + 1).trim();
      if (prop) el.style.setProperty(prop, val);
    }
    el.removeAttribute("data-sx-style");
  }
  // Apply within a subtree (root inclusive). Exposed so code that injects markup at
  // runtime can style freshly-inserted nodes synchronously (no flash, no observer lag).
  function apply(root) {
    if (!root) root = document;
    if (root.nodeType === 1 && root.hasAttribute("data-sx-style")) applyOne(root);
    var list = root.querySelectorAll ? root.querySelectorAll("[data-sx-style]") : [];
    for (var i = 0; i < list.length; i++) applyOne(list[i]);
  }
  window.sxApply = apply;
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", function () { apply(document); });
  } else {
    apply(document);
  }

  // ── Self-contained exports ────────────────────────────────────────────────
  // The HTML/PDF/print export builders serialize the live DOM into a standalone
  // document. That document references /static/app.css via <link>, which is
  // unreachable once downloaded, so the utility classes would lose their styles.
  // Cache the utility CSS and expose a helper that swaps the <link> for an inline
  // <style>, making an exported document fully self-contained. (Applied
  // data-sx-style values are already inline on the elements — setProperty writes
  // to the element's own style — so those survive serialization on their own.)
  window.__sxCss = "";
  try {
    fetch("/static/app.css").then(function (r) { return r.text(); })
      .then(function (t) { window.__sxCss = t; }).catch(function () {});
  } catch (e) { /* fetch unavailable: exports fall back to <link> */ }
  window.sxSelfContain = function (html) {
    if (!window.__sxCss) return html;
    return html.replace(
      /<link\b[^>]*href="\/static\/app\.css"[^>]*>/,
      "<style>" + window.__sxCss + "</style>"
    );
  };
  // Safety net for markup inserted after load that isn't styled via an explicit
  // sxApply() call (keeps behaviour correct even if a call site is missed).
  if (typeof MutationObserver === "function") {
    new MutationObserver(function (muts) {
      for (var i = 0; i < muts.length; i++) {
        var added = muts[i].addedNodes;
        for (var j = 0; j < added.length; j++) {
          if (added[j].nodeType === 1) apply(added[j]);
        }
      }
    }).observe(document.documentElement, { childList: true, subtree: true });
  }
})();

// ── Air-gap / connectivity footer handling ──────────────────────────────────
// Several footer links point at the public internet (the author's profile, "View on
// GitHub", the AGPL licence page) and are dead links on an air-gapped network. The
// CSP forbids the browser from probing the internet directly, so ask the server
// (same-origin) whether this deployment is air-gapped, then repoint or plain-text
// those links. Runs on every page because this script is loaded everywhere.
(function () {
  "use strict";
  // Replace a dead <a> with its own text, preserving the credit without a broken link.
  function plainText(a) {
    if (!a || !a.parentNode) return;
    var span = document.createElement("span");
    span.textContent = a.textContent;
    a.parentNode.replaceChild(span, a);
  }

  function normalizeFooters(info) {
    if (!info || !info.offline) return;
    document.body.classList.add("sloc-offline");
    var repo = info.repo_reachable && info.repo_url ? info.repo_url : "";
    var links = document.querySelectorAll(".site-footer a");
    for (var i = 0; i < links.length; i++) {
      var a = links[i];
      var href = a.getAttribute("href") || "";
      if (!href || href.charAt(0) === "/") continue; // same-origin links stay clickable
      // "View on GitHub" points at the upstream oxide-sloc repo. Repoint it at the repo
      // this build actually came from when that is a reachable fork/mirror; otherwise drop it.
      var isViewRepo =
        /^https?:\/\/(www\.)?github\.com\/oxide-sloc(\/|$)/i.test(href) ||
        /view on github/i.test(a.textContent || "");
      if (isViewRepo) {
        if (repo) {
          a.setAttribute("href", repo);
          a.setAttribute("title", "Repository this build came from");
          a.textContent = "View repository";
        } else {
          plainText(a);
        }
        continue;
      }
      // Author profile, licence page, or any other internet-only link: plain-text it.
      plainText(a);
    }
    addAirgapBadge();
  }

  // Surface the air-gapped posture next to the "Local / Server" status pill in the nav.
  function addAirgapBadge() {
    var wrap = document.getElementById("server-status-wrap");
    if (!wrap || !wrap.parentNode || document.getElementById("airgap-pill")) return;
    var pill = document.createElement("span");
    pill.id = "airgap-pill";
    pill.className = "nav-pill"; // reuse the shared nav-pill style present on every page
    pill.textContent = "Air-gapped";
    pill.setAttribute(
      "title",
      "No internet access for this deployment — external links are disabled"
    );
    wrap.parentNode.insertBefore(pill, wrap);
  }

  function run(info) {
    if (document.readyState === "loading") {
      document.addEventListener("DOMContentLoaded", function () { normalizeFooters(info); });
    } else {
      normalizeFooters(info);
    }
  }

  try {
    fetch("/api/connectivity", { cache: "no-store" })
      .then(function (r) { return r.ok ? r.json() : null; })
      .then(function (info) { run(info); })
      .catch(function () { /* endpoint unreachable: leave links as-is */ });
  } catch (e) { /* fetch unavailable */ }
})();
