# oxide-sloc — Agent Instructions

oxide-sloc is a Rust-based SLOC analysis tool: CLI + localhost web UI + MCP server.
Build it, analyze code with it, and call its REST API or MCP tools directly from agents.

## Build

```bash
cargo build --workspace          # debug build (all 8 crates)
cargo build --release -p oxide-sloc  # release CLI binary
```

Requires Rust 1.95+. Vendored dependencies are in `vendor/` (reassembled + extracted from the split gzip parts `vendor.tar.gz.*`).
`.cargo/config.toml` redirects crate-io lookups to `vendor/` for offline builds.

CI gates (must all pass):
```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
cargo test --workspace
```

## CLI usage

```bash
# Analyze a directory
oxide-sloc analyze ./src --plain
oxide-sloc analyze ./src --per-file --json-out out/result.json --html-out out/report.html

# Re-render a saved result
oxide-sloc report result.json --pdf-out result.pdf

# Compare two results
oxide-sloc diff baseline.json current.json

# Start the web UI (default when no subcommand given)
oxide-sloc serve
oxide-sloc                    # same
```

## MCP server

The `sloc-mcp` binary exposes 7 tools over stdio MCP: `analyze_path`, `get_metrics_latest`,
`get_metrics_history`, `get_run_metrics`, `compare_runs`, `health_check`, `ingest_result`.

Config file: `mcp.json` (repo root). Full schemas: `docs/mcp/tool-definitions.json`.

## REST API

When the server is running (default `http://127.0.0.1:4317`):

```
GET  /api/metrics/latest       # latest scan metrics (JSON)
GET  /api/metrics/history      # time-series scan history
POST /api/ingest               # push an AnalysisRun JSON result
GET  /api/openapi.yaml         # full OpenAPI 3.1 spec
GET  /healthz                  # health ping
GET  /badge/{metric}           # SVG badge
```

Full OpenAPI spec: `docs/openapi.yaml`.

## Workspace layout

```
crates/
  sloc-cli/       # clap CLI binary entry point
  sloc-config/    # AppConfig TOML schema and enums
  sloc-core/      # analyze() — file discovery, decoding, aggregation
  sloc-git/       # Git CLI wrappers and webhook parsing
  sloc-languages/ # Language detection + lexical line analyzers (60 languages)
  sloc-report/    # HTML/PDF rendering
  sloc-web/       # Axum web server and REST API
  sloc-mcp/       # MCP server binary
```

## Key entry points

- `sloc_core::analyze(config)` — main analysis entry point, returns `AnalysisRun`
- `sloc_languages::analyze_text(language, text, options)` — per-file lexical analyzer
- `sloc_web::serve(config)` — starts the Axum server
- `mcp.json` — MCP server configuration for AI agent frameworks

## Coding conventions

- No `.unwrap()` or `.expect()` in library code — use `anyhow::Result` and `?`
- `max_width = 100` (rustfmt). Run `cargo fmt --all` before committing.
- Do not add `#[allow(...)]` without a comment explaining why.
- TOML, Markdown, and YAML are intentionally not analyzed (no SLOC metric applies).

## AI-native integrations

- **MCP**: `docs/mcp/tool-definitions.json` — ready-to-use MCP tool schemas
- **Function calling**: `docs/mcp/function-definitions.json` — OpenAI/Anthropic function schemas
- **REST API**: `docs/openapi.yaml` — full OpenAPI 3.1 spec
- **LLM context**: `docs/ai/llms.txt` (concise), `docs/ai/llms-full.txt` (extended)
