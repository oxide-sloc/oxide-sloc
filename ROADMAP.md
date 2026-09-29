# Roadmap

This is the near-term roadmap for oxide-sloc. Items are roughly priority-ordered within each
section. Completed work ships as patch or minor releases; breaking changes target the next minor.

## Planned

- **tree-sitter adapters** — Enable the scaffolded tree-sitter grammars for Python and C/C++ by
  default in place of the hand-rolled lexical state machines, for more accurate comment / code
  classification, especially for nested constructs and multi-line strings.

- **SARIF export** — Emit SARIF output so results can be ingested by GitHub / GitLab code-scanning.

- **CSV / XLSX export parity in the web UI** — CSV and 4-sheet Excel export are currently
  CLI-only; surface the same exports from the web UI.

## Ideas / under consideration

- **`validate` drift auditing** — extend beyond the current config validation to a
  `oxide-sloc validate <result.json>` mode that re-counts from the original files and reports any
  drift, useful for auditing saved results against a live checkout.
- Silver / Gold OpenSSF Best Practices badge criteria
- WASM build target for browser-native analysis
- Language Server Protocol (LSP) integration for IDE inline metrics
- Plugin API for custom language analyzers

## Recently shipped

These were once roadmap items and are now complete:

- **PDF generation in the web UI** — headless-Chromium export plus a pure-Rust fallback
  (`/export/pdf`, `/api/runs/:id/pdf-status`).
- **Validation corpus + golden tests** — `crates/sloc-languages/tests/corpus/` + `golden.rs`.
- **IEEE 1045-1992 parameters in the web UI** — continuation-line, blank-in-block-comment, and
  compiler-directive controls in the step-2 scan form.
- **`validate` command** — validates the config file (paths, globs, logo, colour scheme).
- **Git-triggered scan registration** — webhook / polling scans register in the run registry and
  appear in `/view-reports`.
- **Code Ownership** — per-author `git blame` attribution, contributor leaderboard, hotspot
  ownership, and `.mailmap` identity merge/export.
- **Git Hotspots** — refactor ranking by `code lines × commit activity` (on by default).
- **COCOMO I, cyclomatic complexity, ULOC / DRYness, duplicate-file detection** — computed on
  every run.

---

Issues and discussion: <https://github.com/oxide-sloc/oxide-sloc/issues>
