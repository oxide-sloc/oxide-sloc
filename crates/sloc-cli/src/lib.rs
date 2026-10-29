// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Nima Shafie <nimzshafie@gmail.com>

//! # oxide-sloc
//!
//! **Cross-platform source line analysis tool** implementing IEEE 1045-1992 SLOC
//! metrics, unit-test detection, coverage reporting, and CI/CD enforcement — with
//! a localhost web UI, rich HTML/PDF reports, and deep Git integration.
//!
//! ## Overview
//!
//! `oxide-sloc` is a binary crate — its primary interface is the `oxide-sloc`
//! executable. All analytical logic lives in focused library crates published
//! separately on crates.io for programmatic use:
//!
//! | Crate | Purpose |
//! |---|---|
//! | [`sloc-config`](https://docs.rs/sloc-config) | Configuration schema and TOML parsing |
//! | [`sloc-languages`](https://docs.rs/sloc-languages) | Language detection and lexical line analyzers |
//! | [`sloc-core`](https://docs.rs/sloc-core) | File discovery, decoding, and aggregation engine |
//! | [`sloc-report`](https://docs.rs/sloc-report) | HTML rendering and PDF export |
//! | [`sloc-git`](https://docs.rs/sloc-git) | Git CLI wrappers and webhook parsing |
//! | [`sloc-web`](https://docs.rs/sloc-web) | Axum web server and form handlers |
//!
//! ## Installation
//!
//! ```text
//! cargo install oxide-sloc
//! ```
//!
//! Or clone the repository and run `bash scripts/run.sh` for a fully offline,
//! no-Rust-required build (all Rust crates and the toolchain are vendored in the
//! repository — no internet connection or pre-installed Rust required).
//!
//! ## Quick Start
//!
//! ```text
//! # Start the localhost web UI  →  http://127.0.0.1:4317
//! oxide-sloc
//!
//! # Scan a directory, print a summary
//! oxide-sloc analyze ./my-repo --plain
//!
//! # Produce JSON + HTML + PDF reports in one pass
//! oxide-sloc analyze ./my-repo \
//!     --json-out result.json \
//!     --html-out result.html \
//!     --pdf-out  result.pdf
//!
//! # Re-render a saved JSON result without re-scanning
//! oxide-sloc report result.json --html-out result.html --pdf-out result.pdf
//!
//! # Compare two saved scans and show the delta
//! oxide-sloc diff baseline.json current.json
//!
//! # Generate a starter config file
//! oxide-sloc init
//! ```
//!
//! ## Subcommands
//!
//! | Subcommand | Description |
//! |---|---|
//! | *(none)* | Alias for `serve` — starts the web UI |
//! | `analyze` | Scan one or more directories and produce metrics |
//! | `report` | Re-render a saved JSON result as HTML / PDF / CSV / XLSX |
//! | `diff` | Compare two JSON results and show the delta |
//! | `serve` | Start the Axum web UI (port 4317 by default) |
//! | `init` | Write a starter `.oxide-sloc.toml` config file |
//! | `validate` | Validate config file paths and glob patterns |
//! | `send` | Deliver a report via SMTP, webhook, Microsoft Teams, or Confluence |
//! | `git-scan` | Clone a repo and scan it at a specific branch / tag / SHA |
//! | `git-compare` | Scan two git refs and produce a diff report |
//! | `watch` | Poll a branch and scan on every new commit |
//! | `pr-comment` | Post an SLOC diff as a GitHub / GitLab PR comment |
//! | `completions` | Print a shell completion script (bash / zsh / fish / …) |
//!
//! ## Output formats
//!
//! All formats are produced by the `analyze` subcommand via output flags.
//! The `report` subcommand can re-render any of them from a saved JSON result.
//!
//! | Flag | Format | Notes |
//! |---|---|---|
//! | `--json-out` | JSON | Full `AnalysisRun` struct; machine-readable and round-trippable |
//! | `--html-out` | HTML | Self-contained, inline CSS, light/dark theme, charts |
//! | `--pdf-out` | PDF | Rendered via headless Chromium (Chrome/Edge/Brave/Vivaldi/Opera) |
//! | `--csv-out` | CSV | Per-language summary; suitable for spreadsheets |
//! | `--xlsx-out` | Excel | Full workbook with per-language and per-file sheets |
//! | `--plain` | key=value | Machine-readable terminal output for shell scripting |
//!
//! ## CI/CD Integration
//!
//! ### GitHub Actions / Jenkins — basic scan
//!
//! ```text
//! oxide-sloc analyze . \
//!     --json-out sloc.json \
//!     --html-out sloc.html \
//!     --fail-on-warnings \
//!     --fail-below 1000
//! ```
//!
//! ### SLOC budget enforcement
//!
//! Define per-language ceilings in `.oxide-sloc.toml` and add `--fail-on-budget`
//! to your CI step.  Exit code 4 when any threshold is exceeded.
//!
//! ```toml
//! [analysis.budget]
//! total_max = 200000   # hard ceiling across all languages
//! rust      = 120000
//! typescript = 60000
//! ```
//!
//! ### Baseline tracking
//!
//! ```text
//! # Save the current scan as a named snapshot
//! oxide-sloc analyze . --json-out sloc.json --set-baseline main
//!
//! # On the next PR — fail if code grew more than 5 %
//! oxide-sloc analyze . --json-out sloc.json \
//!     --fail-above-baseline main --max-delta-pct 5
//! ```
//!
//! ### PR diff comment (GitHub / GitLab)
//!
//! Posts an Adaptive Card-style comment to the pull request with code-line deltas.
//!
//! ```text
//! oxide-sloc pr-comment current.json \
//!     --baseline baseline.json \
//!     --provider github \
//!     --repo owner/repo \
//!     --pr-number 42 \
//!     --token "$GITHUB_TOKEN" \
//!     --report-url "https://ci.example.com/sloc.html"
//! ```
//!
//! ### LCOV coverage overlay
//!
//! Attach per-file coverage data produced by `cargo-llvm-cov`, `gcov`, or any
//! LCOV-compatible tool:
//!
//! ```text
//! oxide-sloc analyze . --coverage-file lcov.info --html-out sloc.html
//! ```
//!
//! ### Exit codes
//!
//! | Code | Meaning |
//! |---|---|
//! | 0 | Success |
//! | 1 | Analysis or I/O error |
//! | 2 | `--fail-on-warnings`: one or more warnings emitted |
//! | 3 | `--fail-below N`: code lines fell below the threshold |
//! | 4 | `--fail-on-budget`: a budget ceiling was exceeded |
//! | 5 | `--fail-above-baseline`: code grew beyond the allowed delta |
//!
//! ## Configuration
//!
//! Generate a starter config with `oxide-sloc init`, then edit as needed.
//! The config file is loaded automatically when `.oxide-sloc.toml` exists in the
//! current directory; pass `--config <path>` to override.
//!
//! ```toml
//! [discovery]
//! root_paths          = ["."]
//! exclude_globs       = ["**/node_modules/**", "**/target/**"]
//! honor_ignore_files  = true   # respect .gitignore / .ignore
//! follow_symlinks     = false
//! submodule_breakdown = false  # per-submodule stats in output
//!
//! [analysis]
//! enabled_languages = []         # empty = all 60 languages
//! mixed_line_policy = "code-only"
//! python_docstrings_as_comments = true
//! generated_file_detection      = true
//! vendor_directory_detection    = true
//!
//! # IEEE 1045-1992 counting parameters
//! continuation_line_policy      = "each-physical-line"
//! blank_in_block_comment_policy = "count-as-comment"
//! count_compiler_directives     = true
//!
//! [reporting]
//! report_title  = "SLOC Report"
//! theme         = "auto"         # auto | light | dark
//! company_name  = "Acme Corp"    # optional branding
//! accent_color  = "#3b82f6"      # optional hex colour
//!
//! [web]
//! bind_address = "127.0.0.1:4317"
//! server_mode  = false           # true = bind 0.0.0.0, LAN mode
//! ```
//!
//! ### Named profiles
//!
//! Override any config section per invocation with `--profile <name>`. Define
//! profiles under `[profiles.NAME]` (the singular `[profile.NAME]` is also accepted):
//!
//! ```toml
//! [profiles.frontend]
//! [profiles.frontend.discovery]
//! root_paths    = ["frontend"]
//! exclude_globs = ["**/node_modules/**", "**/dist/**"]
//! [profiles.frontend.analysis]
//! enabled_languages = ["TypeScript", "JavaScript", "CSS"]
//! ```
//!
//! ## Web UI
//!
//! Running `oxide-sloc` with no arguments (or `oxide-sloc serve`) starts the
//! Axum web UI at `http://127.0.0.1:4317`.
//!
//! - **Step 1** — pick a directory with the native file picker, choose output
//!   formats, configure discovery and analysis options.
//! - **Step 2** — review the live analysis results, download JSON/HTML/PDF/CSV/XLSX.
//! - **View Reports** — browse all past runs with trend charts.
//! - **Compare Scans** — side-by-side delta between any two saved runs.
//! - **Git Tools** — webhook configuration and repository integration.
//!
//! For LAN / multi-user access:
//!
//! ```text
//! oxide-sloc serve --server          # binds 0.0.0.0:4317
//! oxide-sloc serve --bind 0.0.0.0:8080
//! ```
//!
//! ## Delivery (`send` subcommand)
//!
//! Ship a saved analysis result to one or more destinations in a single call:
//!
//! ```text
//! oxide-sloc send result.json \
//!     --smtp-to team@example.com \
//!     --smtp-from ci@example.com \
//!     --smtp-host mail.example.com \
//!     --notify-teams "$TEAMS_WEBHOOK_URL" \
//!     --webhook-url  "$DASHBOARD_URL"
//! ```
//!
//! | Destination | Flag(s) |
//! |---|---|
//! | Email (SMTP/TLS) | `--smtp-to`, `--smtp-from`, `--smtp-host` |
//! | HTTP webhook | `--webhook-url`, `--webhook-token` |
//! | Microsoft Teams | `--notify-teams` |
//! | Confluence | `--confluence-url`, `--confluence-space`, `--confluence-page-title` |
//!
//! ## Git operations
//!
//! Scan a remote repository at a specific ref without a manual clone:
//!
//! ```text
//! # Scan the v2.0.0 tag of a remote repo
//! oxide-sloc git-scan https://github.com/org/repo --git-ref v2.0.0 \
//!     --json-out v2.json --html-out v2.html
//!
//! # Compare two tags
//! oxide-sloc git-compare https://github.com/org/repo v1.0.0 v2.0.0 \
//!     --json-out delta.json
//!
//! # Watch a branch and scan on every new commit (poll every 5 min)
//! oxide-sloc watch https://github.com/org/repo \
//!     --branch main --interval 300 --output-dir scans/
//! ```
//!
//! ## Environment variables
//!
//! | Variable | Purpose |
//! |---|---|
//! | `SLOC_BIND` | Override web bind address (lower priority than `--bind`) |
//! | `SLOC_API_KEY` | Enable bearer-token auth on the web server |
//! | `SLOC_ALLOWED_ROOTS` | Colon-separated list of directories the web UI may scan |
//! | `SLOC_TLS_CERT` / `SLOC_TLS_KEY` | Enable native TLS on the web server |
//! | `SLOC_BROWSER` | Override the browser binary used for PDF export |
//! | `SLOC_BROWSER_NOSANDBOX` | Set to `1` to pass `--no-sandbox` (required in Docker) |
//! | `SLOC_COVERAGE_FILE` | Path to an LCOV `.info` file (same as `--coverage-file`) |
//! | `SLOC_SMTP_HOST` / `SLOC_SMTP_USER` / `SLOC_SMTP_PASS` | SMTP credentials |
//! | `SLOC_WEBHOOK_TOKEN` | Bearer token for webhook delivery |
//! | `SLOC_ALLOW_PRIVATE_WEBHOOK` | Set to `1` to allow HTTP and private-IP webhooks |
//! | `SLOC_VCS_API_URL` / `SLOC_VCS_REPO` / `SLOC_VCS_TOKEN` | VCS API for `pr-comment` |
//! | `SLOC_PR_NUMBER` | PR number for `pr-comment` |
//! | `SLOC_CONFLUENCE_URL` / `SLOC_CONFLUENCE_USER` / `SLOC_CONFLUENCE_TOKEN` | Confluence credentials |
//! | `SLOC_CONFLUENCE_SPACE` | Confluence space key |
//! | `RUST_LOG` | Tracing log level (e.g. `RUST_LOG=debug`) |
//!
//! ## Supported languages
//!
//! 60 languages detected by file extension and shebang:
//!
//! Ada, Assembly, Awk, C, C++, C#, Clojure, `CMake`, Crystal, CSS, D, Dart,
//! Dockerfile, Elixir, Elm, Erlang, F#, Fortran, GLSL/HLSL, Go, GraphQL, Groovy,
//! Haskell, HCL/Terraform, HTML, Java, JavaScript, Julia, Kotlin, Lisp/Scheme,
//! Lua, Makefile, Nim, Nix, Objective-C, OCaml, Pascal/Delphi, Perl, PHP,
//! PowerShell, Protocol Buffers, Python, R, Ruby, Rust, Scala, SCSS/Sass,
//! Shell (bash/sh/zsh/ksh), Solidity, SQL, Svelte, Swift, Tcl, TypeScript,
//! Verilog/SystemVerilog, VHDL, Visual Basic, Vue, XML/SVG, and Zig.
//!
//! TOML, Markdown, and YAML are intentionally excluded — no meaningful SLOC
//! metric applies to them.
//!
//! Extension-to-language mappings can be overridden per project:
//!
//! ```toml
//! [analysis.extension_overrides]
//! "h" = "cpp"    # treat .h files as C++ instead of C
//! ```
//!
//! ## IEEE 1045-1992 compliance
//!
//! The counting engine implements the IEEE 1045-1992 standard.  Three parameters
//! let you choose the counting convention that matches your organisation's policy:
//!
//! | Parameter | Values | Default |
//! |---|---|---|
//! | `continuation_line_policy` | `each-physical-line` / `collapse-to-logical` | `each-physical-line` |
//! | `blank_in_block_comment_policy` | `count-as-comment` / `count-as-blank` | `count-as-comment` |
//! | `count_compiler_directives` | `true` / `false` | `true` |
//!
//! CLI flags (`--continuation-line-policy`, `--blank-in-block-comment-policy`,
//! `--no-count-compiler-directives`) override the config file for a single run.
//!
//! ## See also
//!
//! - [Repository & full documentation](https://github.com/oxide-sloc/oxide-sloc)
//! - [Changelog](https://github.com/oxide-sloc/oxide-sloc/releases)
//! - [`sloc-core`](https://docs.rs/sloc-core) — programmatic analysis API
//! - [`sloc-config`](https://docs.rs/sloc-config) — configuration schema
//! - [`sloc-languages`](https://docs.rs/sloc-languages) — per-language analyzers
