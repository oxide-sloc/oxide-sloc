# oxide-sloc API contract — embedding oxide-sloc in another project

This is the **stable, embeddable contract** for driving oxide-sloc from another
project: a CI pipeline, a dashboard, a service, or a plain shell job. Everything
here is a long-lived interface — code against it directly.

The machine-readable source of truth is the OpenAPI 3.1 spec:

- **In the repo:** [`openapi.yaml`](openapi.yaml)
- **Live from a running server:** `GET /api/openapi.yaml` (no auth) — its
  `info.version` is stamped from the running binary at request time, so it always
  reflects the deployed version.

Generate a typed client from either (`openapi-generator`, `oapi-codegen`,
`openapi-python-client`, …) rather than hand-rolling HTTP calls.

## Base URL & transport

| | |
|---|---|
| Default bind | `http://127.0.0.1:4317` (loopback) |
| Override | `SLOC_BIND=host:port` or `--bind host:port` when starting `oxide-sloc serve` |
| TLS | set `SLOC_TLS_CERT` / `SLOC_TLS_KEY` for native TLS termination |
| Content type | `application/json` for request/response bodies unless noted |

## Authentication

- When **`SLOC_API_KEY`** (or `SLOC_API_KEYS`) is set on the server, every endpoint
  requires `Authorization: Bearer <key>` **except** the always-public set:
  `/healthz`, `/readyz`, `/api/health`, `/api/version`, `/api/openapi.yaml`,
  `/badge/{metric}`, and the `/webhooks/*` receivers.
- In **server mode** (`serve --server`) a key is mandatory — the server refuses to
  start unauthenticated (set `SLOC_ALLOW_UNAUTHENTICATED=1` only on a trusted,
  isolated network).
- `/webhooks/*` are authenticated differently: each inbound event is signed
  **HMAC-SHA256** over the exact request body with the matching schedule's
  `webhook_secret` (see below). This lets a legacy upstream trigger a scan with
  nothing but `curl`, without holding the server API key.

## Versioning & compatibility policy

- `GET /api/version` → `{ name, version, git_sha, build_time }`. Poll it to learn
  the deployed version before relying on a feature.
- The API follows the crate's semver. Treat a **major** version change as
  potentially breaking; minor/patch add fields but do not remove them.
- Pin a reproducible tool version in a chain with the Jenkins `SCAN_TOOL_VERSION`
  parameter, or locally with `scripts/preflight.sh --require-version X.Y.Z`
  (see [Preflight](#preflight-before-you-integrate)).

## Endpoints you call from another project

### Health & discovery (public)

| Method | Path | Purpose |
|---|---|---|
| GET | `/healthz` | Liveness — `200 ok`. Container/systemd probe. |
| GET | `/readyz` | Readiness — `200` when it can persist state + write artifacts, else `503`. |
| GET | `/api/health` | Structured status + build info (`status: ok\|degraded`). |
| GET | `/api/version` | `{ name, version, git_sha, build_time }`. |
| GET | `/api/openapi.yaml` | This contract, machine-readable. |
| GET | `/badge/{metric}` | SVG badge (`code-lines`, `files`, `comment-lines`, `blank-lines`). |

### Metrics (bearer-auth when a key is set)

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/metrics/latest` | Metrics from the most recent scan (`404` if none). |
| GET | `/api/metrics/{run_id}` | Metrics for a specific run UUID. |
| GET | `/api/metrics/history?root=&limit=` | Time-ordered history for trend charts. |
| GET | `/api/metrics/submodules` | Per-submodule breakdown of the latest scan. |

### Ingest — push a headless scan result into a shared server

Run `oxide-sloc analyze --json-out result.json` anywhere, then POST the document:

```bash
curl -X POST "$OXIDE_SLOC_URL/api/ingest" \
  -H 'Content-Type: application/json' \
  -H "Authorization: Bearer $OXIDE_SLOC_API_KEY" \
  --data-binary @result.json
# → 200 { "run_id": "...", "view_url": "http://.../runs/result/..." }
```

The run then appears in `/view-reports` and is queryable via `/api/metrics/{run_id}`.

### Schedules — register what inbound events match against

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/schedules` | List configured schedules. |
| POST | `/api/schedules` | Create one (`label`, `repo_url`, `branch`, `kind`, `provider?`, `interval_secs?`, `webhook_secret?`). |
| DELETE | `/api/schedules?id=<uuid>` | Remove one. |

```bash
curl -X POST "$OXIDE_SLOC_URL/api/schedules" -H 'Content-Type: application/json' \
  -H "Authorization: Bearer $OXIDE_SLOC_API_KEY" \
  --data '{"label":"myapp","repo_url":"https://git.example.com/org/myapp.git",
           "branch":"main","kind":"webhook","provider":"any",
           "webhook_secret":"<SHARED_SECRET>"}'
```

### Webhooks — trigger a scan when an upstream build finishes (public, HMAC)

The provider-agnostic receiver is `POST /webhooks/ci`; GitHub/GitLab/Bitbucket
native push events go to `/webhooks/{github,gitlab,bitbucket}`. The body is signed
HMAC-SHA256 with the schedule's `webhook_secret`; an invalid signature returns the
same `202` as success (no oracle). See the
[downstream-trigger kit](../ci/downstream-trigger/README.md) for the full contract,
guards (success gate, idempotency, retry/backoff, fail-closed), and copy-paste
wiring for every CI.

```json
POST /webhooks/ci
X-Sloc-Signature: sha256=<hmac-hex>
{
  "repo_url": "https://git.example.com/org/myapp.git",
  "branch": "main",
  "commit_sha": "",
  "status": "success",
  "upstream": { "system": "jenkins", "job": "myapp-pipeline", "build_id": "1234" }
}
```

## Delivery from the CLI — the `send` subcommand

No server required: `oxide-sloc send result.json` pushes a finished scan outward.

| Target | Flags |
|---|---|
| HTTP webhook | `--webhook-url <URL>` (repeatable), `--webhook-token` / `SLOC_WEBHOOK_TOKEN` |
| Email (SMTP/TLS) | `--smtp-to`, `--smtp-from`, `--smtp-host`/`SLOC_SMTP_HOST`, … |
| Microsoft Teams | `--notify-teams <URL>`, `--report-url` |
| Confluence | `--confluence-url`, `--confluence-space`, `--confluence-token`/`SLOC_CONFLUENCE_TOKEN`, … |
| Pool into a server | `send … --webhook-url <serve>/api/ingest` (use `--allow-private-net` for RFC-1918 targets) |

## Preflight — before you integrate

Confirm a host can install/build/run oxide-sloc and is compatible with a target
server **before** wiring it in. The preflight is a single standalone file you can
copy into any project:

```bash
# POSIX hosts (Linux, macOS, Git Bash)
bash scripts/preflight.sh --mode run --server-url "$OXIDE_SLOC_URL" --require-version 1.6.24
```
```powershell
# Windows hosts without bash
pwsh scripts/preflight.ps1 -Mode run -ServerUrl $env:OXIDE_SLOC_URL -RequireVersion 1.6.24
```

It checks platform/arch, git & curl, mode prerequisites (extraction tooling or a
Rust toolchain), a PDF browser, the resolved binary version (with an optional hard
pin), and — with `--server-url` — server reachability, health, and version
compatibility. Exit `0` = ready, `1` = a hard prerequisite is missing. Add `--json`
for a machine-readable summary line.

## Jenkins: pin a reproducible tool version across a job chain

The bundled `Jenkinsfile` exposes `SCAN_TOOL_VERSION`. When set, the pipeline
asserts `oxide-sloc --version` after the Build stage and **fails on mismatch**, then
forwards the pin to `CHAIN_DOWNSTREAM_JOB` so an entire upstream/downstream chain
runs one locked version. See [CI Integrations](ci-integrations.md) and the
[downstream-trigger kit](../ci/downstream-trigger/README.md).
