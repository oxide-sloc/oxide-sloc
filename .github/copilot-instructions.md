# GitHub Copilot Instructions — oxide-sloc

oxide-sloc is an 8-crate Rust workspace: IEEE 1045-1992 SLOC analysis, unit test detection,
HTML/PDF reports, REST API, and an MCP server for AI agent integration.

## Workspace crates

| Crate | Role |
|---|---|
| `sloc-cli` | clap CLI — `analyze`, `report`, `serve`, `diff`, `init`, `send` |
| `sloc-config` | `AppConfig` TOML schema and enums |
| `sloc-core` | `analyze()` entry point — file discovery, decoding, aggregation |
| `sloc-git` | Git CLI wrappers, webhook parsing |
| `sloc-languages` | Language detection + lexical line analyzers (60 languages) |
| `sloc-report` | Askama HTML templates + headless-Chromium PDF export |
| `sloc-web` | Axum web server, REST API, form handlers, artifact storage |
| `sloc-mcp` | MCP server binary (7 tools for AI agents) |

## Key types and entry points

- `sloc_core::analyze(config: &AppConfig) -> Result<AnalysisRun>` — main entry point
- `sloc_languages::analyze_text(lang, text, opts) -> RawFileAnalysis` — per-file lexer
- `sloc_web::serve(config: AppConfig) -> Result<()>` — starts the Axum server
- `AnalysisRun` — canonical serializable result (JSON in/out via `write_json` / `read_json`)
- `FileRecord` — per-file details including effective counts after policy application
- `AppConfig` → `DiscoveryConfig`, `AnalysisConfig`, `ReportingConfig`, `WebConfig`

## Coding conventions

- No `.unwrap()` or `.expect()` in library code — propagate with `anyhow::Result` / `?`
- `rustfmt.toml`: `edition = "2024"`, `max_width = 100`. Run `cargo fmt --all` before committing.
- No `#[allow(...)]` without a comment explaining why.
- No comments that explain *what* the code does — only *why* (non-obvious constraints or workarounds).
- Do not use `.unwrap()` or `.expect()` in library code — propagate errors.

## CI gates

All four must pass before merging:
```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
cargo test --workspace
```

## AI / MCP integration

- MCP config: `mcp.json` (repo root)
- MCP tool schemas: `docs/mcp/tool-definitions.json`
- OpenAI/Anthropic function schemas: `docs/mcp/function-definitions.json`
- OpenAPI 3.1 spec: `docs/openapi.yaml`
- LLM context files: `docs/ai/llms.txt`, `docs/ai/llms-full.txt`

## Supported Languages (60)

oxide-sloc intentionally excludes TOML, Markdown, and YAML — no meaningful SLOC metric applies.
Adding a language requires updating both `sloc-languages` (analyzer) and `sloc-config` (extension map).

## Offline-first build

The repo commits the split gzip parts `vendor.tar.gz.aa` / `.ab` / `.ac` (~115 MB total, ≤45 MB
each — gzip so stock Git Bash on Windows without `xz` can extract) plus `vendor.checksums.sha256`,
and split Rust toolchain archives, so the entire build works with no internet access.
`.cargo/config.toml` redirects crate lookups to `vendor/`. Do not add dependencies without running
`bash scripts/internal/update-vendor.sh` and committing the updated `vendor.tar.gz.*` parts and
`vendor.checksums.sha256` atomically.
