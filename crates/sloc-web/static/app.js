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
