# sloc-mcp

MCP stdio server that exposes oxide-sloc analysis tools to AI agents (Claude Desktop,
Claude Code, and any other MCP-compatible host).

Speaks MCP protocol revision **2025-06-18** (negotiated down to `2025-03-26` / `2024-11-05`
for older clients). Tools carry behaviour annotations (`readOnlyHint`, `openWorldHint`, …) and
return both a human-readable text block and machine-readable `structuredContent` JSON.
`SLOC_MCP_ALLOWED_ROOTS` allowlists the directories `analyze_path` / `compare_runs` may read
(fail-closed; set `SLOC_MCP_UNRESTRICTED=1` for trusted local use).

## Tools

| Tool | Description |
|---|---|
| `analyze_path` | Run oxide-sloc on a local path; returns SLOC metrics |
| `get_metrics_latest` | Fetch latest scan from a running server |
| `get_metrics_history` | Fetch time-series scan history |
| `get_run_metrics` | Fetch metrics for a specific run_id |
| `compare_runs` | Diff two JSON result files |
| `health_check` | Verify server is reachable |
| `ingest_result` | Push a result into the server registry |

## Usage

```bash
cargo build -p sloc-mcp
```

Add to `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "oxide-sloc": {
      "command": "sloc-mcp",
      "env": { "SLOC_SERVER_URL": "http://127.0.0.1:4317" }
    }
  }
}
```

## Environment variables

| Variable | Description |
|---|---|
| `SLOC_SERVER_URL` | Base URL of a running oxide-sloc server |
| `SLOC_BIN` | Path to the oxide-sloc binary (default: `oxide-sloc`) |
| `SLOC_API_KEY` | Bearer token when server auth is enabled |
