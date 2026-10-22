# Local Automation

How to drive oxide-sloc from the shell: run the web server once, then interact
with it (or run one-shot scans) from any number of other terminals, and scan a
whole queue of projects in one command.

All of this works as an unprivileged local user with no network access.

---

## TL;DR

```bash
# Terminal 1 - start the server once, leave it running
bash scripts/run.sh                       # http://127.0.0.1:4317 (local mode, no API key)

# Terminal 2..N - fast one-shot scans (do not touch the server)
bash scripts/run.sh analyze ./some-repo --plain

# Terminal 2..N - scan a whole queue from a text file
bash scripts/run.sh batch --from repos.txt --out-dir out/batch --jobs 4 -H

# Terminal 2..N - talk to the running server over HTTP
curl -s http://127.0.0.1:4317/api/metrics/latest
```

---

## 1. Long-running server + many client terminals

`bash scripts/run.sh` with no subcommand launches the binary in `serve` mode,
binding `http://127.0.0.1:4317`. In **local mode** (the default, no `--host`)
there is **no API key** required, so other terminals can hit the API
immediately.

It is a normal Axum HTTP server, so it accepts concurrent connections from any
number of clients at once. You do not need to do anything special to "enable"
multi-window use - just leave the server running in Terminal 1 and open as many
client terminals as you like.

### Starting the server

```bash
bash scripts/run.sh                 # localhost only  -> http://127.0.0.1:4317
bash scripts/run.sh --host          # LAN server mode -> http://0.0.0.0:4317 (requires SLOC_API_KEY)
```

> **LAN mode is fail-closed.** `--host` (server mode) refuses to start without
> `SLOC_API_KEY` / `SLOC_API_KEYS`. Set a key, or - only on a trusted isolated
> network - `export SLOC_ALLOW_UNAUTHENTICATED=1`. Local mode has no such gate.

### Querying the running server

The JSON API is the automation surface. A few of the most useful endpoints:

| Method | Path | Purpose |
|---|---|---|
| `GET`  | `/api/health`, `/healthz`, `/readyz` | Liveness / readiness |
| `GET`  | `/api/version` | Running version |
| `GET`  | `/api/metrics/latest` | Most recent run's metrics |
| `GET`  | `/api/metrics/{run_id}` | A specific run's metrics |
| `GET`  | `/api/metrics/history` | All runs, newest first |
| `GET`  | `/api/metrics/churn` | Scan-to-scan churn |
| `POST` | `/api/ingest?label=<name>` | Ingest an `AnalysisRun` JSON into the UI |
| `GET`  | `/api/git/scan-ref?repo=<path>&ref_name=<ref>` | Scan a git ref server-side |
| `GET`  | `/api/openapi.yaml` | Full live OpenAPI spec (self-stamped to the running version) |

```bash
curl -s http://127.0.0.1:4317/api/metrics/latest
curl -s "http://127.0.0.1:4317/api/git/scan-ref?repo=.&ref_name=main"
curl -s http://127.0.0.1:4317/api/openapi.yaml       # discover everything else
```

PowerShell equivalents use `Invoke-RestMethod` or `curl.exe`:

```powershell
Invoke-RestMethod http://127.0.0.1:4317/api/metrics/latest
```

### Getting scan results into the web UI (ingest)

`POST /analyze` is the **browser form** handler and returns HTML, so it is
awkward to script. For automation, run the scan with the CLI (below) to produce
JSON, then push that JSON to the running server with `POST /api/ingest`. The run
then appears in **View Reports**, **Compare Scans**, and **Trend Reports**.

```bash
# Git Bash
bash scripts/run.sh analyze ./some-repo --json-out out/r.json
curl -s -X POST "http://127.0.0.1:4317/api/ingest?label=some-repo" \
     -H "Content-Type: application/json" --data-binary @out/r.json
```

```powershell
# PowerShell
bash scripts/run.sh analyze ./some-repo --json-out out/r.json
Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:4317/api/ingest?label=some-repo" `
  -ContentType "application/json" -InFile out/r.json
```

---

## 2. One-shot CLI scans

`run.sh` forwards any known subcommand straight to the binary and does **not**
start the server, so each invocation is fully independent - run as many in
parallel, across as many terminals, as you want.

```bash
bash scripts/run.sh analyze ./some-repo --plain
bash scripts/run.sh analyze ./some-repo --per-file --json-out out/r.json --html-out out/r.html
bash scripts/run.sh analyze ./some-repo --per-file --activity-window 90 --html-out out/hotspots.html
```

This path is **not** subject to the server's concurrent-analysis cap (see
Gotchas), so it is the right choice for heavy parallel automation.

---

## 3. Batch / queue scanning (a whole list of repos at once)

When you have many repos or directories to scan, put them in a queue and let
`batch` walk the whole list in one command. It writes a per-project report plus
a combined `batch-index.html` / `batch-index.json`.

### CLI

```bash
bash scripts/run.sh batch --from repos.txt --out-dir out/batch --jobs 4 -H
```

`--from` accepts any of three queue formats (auto-detected):

**(a) A text file - one project per line.** `#` comments and blank lines are
ignored. Add `, <profile>` after a path to override the default profile for that
one entry:

```text
# repos.txt - one project per line, optional ", profile"
./services/api
./services/web
../shared/libcommon, strict
/abs/path/to/another/repo
```

**(b) A directory of projects** - every immediate subdirectory becomes one
project:

```bash
bash scripts/run.sh batch --from ~/code/all-my-repos --jobs 4 -H
```

**(c) A JSON manifest** - array of objects for full per-entry control
(`path` required; `profile`, `label`, `config` optional):

```json
[
  { "path": "./services/api", "label": "API", "profile": "strict" },
  { "path": "./services/web", "label": "Web" },
  { "path": "/abs/path/repo", "config": "./configs/repo.toml" }
]
```

#### `batch` flags

| Flag | Default | Meaning |
|---|---|---|
| `--from <PATH>` | (required) | Directory of projects, or a text/JSON manifest |
| `--out-dir <DIR>` | `out/batch` | Where per-project reports + the index are written |
| `--profile <NAME>` | none | Default config profile for entries that do not name their own |
| `--json` | on if no other format chosen | Write a JSON result per project |
| `-H`, `--html` | off | Write an HTML report per project |
| `-c`, `--csv` | off | Write a CSV summary per project |
| `-x`, `--xlsx` | off | Write an Excel workbook per project |
| `--jobs <N>` | `1` | Max projects scanned concurrently |
| `-q`, `--quiet` | off | Suppress the per-project progress line |

JSON is always produced (even when you pick only HTML/CSV/XLSX) so the index and
any later re-render still work. Failures are captured per project and reported
in the index - one bad repo does not abort the batch.

When it finishes, open `out/batch/batch-index.html` for the combined view.

### Web UI

The same capability is available in the browser at **`/batch`**: paste the
project list (one `path[, profile]` per line) into the textarea, pick the output
formats, and submit. The server scans each project, writes per-project reports
plus a combined index under `<output_root>/batch/<id>/`, and redirects you to
the served index.

> In **server mode** every path must resolve under a configured scan root
> (`SLOC_ALLOWED_ROOTS`); projects outside are reported as failed and skipped.

---

## Gotchas

- **Do not run `run.sh` a second time.** On launch it runs `free_port()`, which
  kills any `oxide-sloc` process and whatever owns port 4317. A second `run.sh`
  would kill your server. Client terminals using `curl` / `Invoke-RestMethod` /
  `run.sh analyze` / `run.sh batch` are fine - only re-launching the *server*
  form of `run.sh` is the problem.
- **HTTP `/analyze` has a concurrency cap.** The browser analyze path is bounded
  by a semaphore and returns `503 "all analysis slots in use"` when saturated.
  CLI scans (`analyze`, `batch`) are not subject to it - prefer them for heavy
  parallel work, then `POST /api/ingest` the JSON if you want it in the UI.
- **Rate limit** in local mode is 600 requests/min per IP (120 in server mode),
  overridable with `SLOC_RATE_LIMIT=<rpm>`. Plenty for scripted loops.
- **Windows:** `run.sh` needs Git Bash. Your client terminals can be PowerShell
  (`Invoke-RestMethod` / `curl.exe`) or Git Bash - either works.

---

## Environment knobs for automation

| Variable | Effect |
|---|---|
| `SLOC_BIND` | Override bind address (e.g. `127.0.0.1:8080`) without editing the config |
| `SLOC_RATE_LIMIT` | Requests/min per IP |
| `SLOC_API_KEY` / `SLOC_API_KEYS` | Enable bearer-token auth (required for `--host`) |
| `SLOC_ALLOW_UNAUTHENTICATED=1` | Run an open server on a trusted isolated network |
| `SLOC_ALLOWED_ROOTS` | Restrict which directories the server may scan (colon-separated) |
| `OXIDE_SLOC_ROOT` | Root for the output/artifact tree (set automatically by `run.sh`) |
| `RUST_LOG` | Tracing verbosity (e.g. `RUST_LOG=debug`) |
