# Server Deployment

> **Default install:** run `bash scripts/run.sh` for personal local use — it installs on first
> run automatically, then opens the web UI at `http://127.0.0.1:4317`.
> This guide is for hosting oxide-sloc persistently so **other users** can reach it.

---

## Local vs server mode

| | Local (default) | Server (`--server`) |
|---|---|---|
| Bind address default | `127.0.0.1:4317` | `0.0.0.0:4317` |
| Browser auto-open | yes | no |
| File picker | native OS dialog | browser directory upload |
| OS path opener | yes | toast message (not available) |
| Startup message | "local web UI" | "server" |

In server mode the native file picker is replaced by a browser-based directory
upload: users click **Browse**, a standard browser folder-picker opens on their
own machine, and the selected files are uploaded to the server's temp directory.
The path field is auto-populated with the server-side temp path, and the
uploaded files are deleted automatically after the scan completes.

The OS path opener (open folder in Explorer/Finder) cannot open a window on a
remote client's machine, so in server mode clicking it shows an informational
message instead.

The scan registry and report artifacts are shared across all sessions.

> **Network-facing binds always get server-mode protections.** If you bind to a
> non-loopback address (`0.0.0.0`, a LAN IP, or a hostname) via `SLOC_BIND` / `--bind`
> *without* `--server`, oxide-sloc automatically enables server mode anyway: it
> refuses to start without `SLOC_API_KEY` (unless `SLOC_ALLOW_UNAUTHENTICATED=1`),
> enforces the `SLOC_ALLOWED_ROOTS` scan-path allowlist, disables the desktop-only
> routes, and applies the tighter rate limit and CORS. Loopback binds keep the open
> single-user desktop model.

---

## Option A — Docker Compose (recommended)

```bash
# Build and start (survives reboots via restart: unless-stopped)
docker compose up -d

# Tail logs
docker compose logs -f

# Stop
docker compose down
```

The container runs with `--server` by default — binds to `0.0.0.0:4317`,
browser auto-open is suppressed, and the native file picker is replaced by
browser-based directory upload.

### Analyzing paths on the host

Mount any directory you want users to be able to analyze:

```yaml
volumes:
  - type: bind
    source: /path/to/project
    target: /repo
    read_only: true
```

Users then enter `/repo` in the path field of the web form.

> **Why `/repo` is scannable:** server mode is fail-closed on `SLOC_ALLOWED_ROOTS`.
> The shipped `docker-compose.yml` already sets `SLOC_ALLOWED_ROOTS: /repo`, which
> is what permits scanning that path. If you mount the project at a **different**
> target, update `SLOC_ALLOWED_ROOTS` to match (colon-separated for several) or the
> scan is rejected with HTTP 403.

---

## Option B — systemd (Linux bare-metal / VPS)

> **Before you can scan server-side paths:** server mode rejects every path scan
> with HTTP 403 until `SLOC_ALLOWED_ROOTS` lists the directories you want
> analyzable (colon-separated absolute paths). `install-systemd.sh` writes a
> commented `SLOC_ALLOWED_ROOTS=` line into `/etc/oxide-sloc/oxide-sloc.env` —
> uncomment it, set your paths, and `sudo systemctl restart oxide-sloc`. The UI
> loads and authenticates without it, but "Analyze &lt;path&gt;" will not run. The
> Browse / directory-upload flow works regardless.

### 1. Install the binary

```bash
# From a release archive:
tar xzf oxide-sloc-linux-x86_64.tar.gz
sudo install -m 755 oxide-sloc /usr/local/bin/oxide-sloc

# Or build from source:
cargo build --release -p oxide-sloc
sudo install -m 755 target/release/oxide-sloc /usr/local/bin/oxide-sloc
```

### 2. Create a dedicated user and working directory

```bash
sudo useradd --system --no-create-home --shell /usr/sbin/nologin oxide-sloc
sudo mkdir -p /opt/oxide-sloc/out
sudo chown -R oxide-sloc:oxide-sloc /opt/oxide-sloc
```

### 3. Optional: install a config file

```bash
sudo mkdir -p /etc/oxide-sloc
sudo cp deploy/server.toml /etc/oxide-sloc/config.toml
# Edit bind address, report title, etc. as needed.
```

To use it, change `ExecStart` in the service unit to add `--config`:

```
ExecStart=/usr/local/bin/oxide-sloc serve --server --config /etc/oxide-sloc/config.toml
```

### 4. Install and enable the service

**Preferred — use the installer script** (automates all steps above):

```bash
sudo bash scripts/internal/install-systemd.sh
```

To undo the install: `sudo bash scripts/internal/install-systemd.sh --uninstall`

**Manual steps (equivalent to what the script does):**

```bash
sudo cp deploy/oxide-sloc.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now oxide-sloc
```

```bash
sudo systemctl status oxide-sloc
sudo journalctl -u oxide-sloc -f
```

---

## Reverse proxy (Nginx example)

Put Nginx in front for HTTPS or a custom domain.
Use `bind_address = "127.0.0.1:4317"` in `server.toml` so the port is not directly internet-accessible.

```nginx
server {
    listen 443 ssl;
    server_name sloc.example.com;

    # ... ssl_certificate / ssl_certificate_key ...

    location / {
        proxy_pass http://127.0.0.1:4317;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }
}
```

---

## Security

### Authentication (`SLOC_API_KEYS` / `SLOC_API_KEY`)

Set `SLOC_API_KEYS` (a comma-separated list, the form `install-systemd.sh` and
`deploy/corp.env.example` write) — or the singular `SLOC_API_KEY` — to require a
bearer token on every request. When set, clients must supply one of:

- `Authorization: Bearer <key>` header
- `X-API-Key: <key>` header

`--server` mode is **fail-closed**: it refuses to start unless a key is configured
(or `SLOC_ALLOW_UNAUTHENTICATED=1` is explicitly set for a trusted, isolated
network). The installer generates a key on first install for exactly this reason.
In local mode auth is optional and the no-key warning is suppressed.

### TLS (`SLOC_TLS_CERT` / `SLOC_TLS_KEY`)

oxide-sloc can terminate TLS directly using PEM-encoded certificate and key files:

```bash
SLOC_TLS_CERT=/etc/oxide-sloc/server.crt \
SLOC_TLS_KEY=/etc/oxide-sloc/server.key \
oxide-sloc serve --server
```

When both are set the server prints `OxideSLOC server running at https://… (TLS)` and accepts HTTPS connections. When running in `--server` mode without TLS, a cleartext warning is logged at startup.

For production deployments a **reverse proxy** (Nginx, Caddy) with `bind_address = "127.0.0.1:4317"` is the preferred approach — TLS termination at the proxy layer avoids certificate management in the binary.

### Rate limiting

A sliding-window rate limiter enforces **60 requests per 60-second window per client IP** across all routes. Requests over the limit receive `HTTP 429 Too Many Requests`.

---

## Environment variables

| Variable | Purpose | Default |
|---|---|---|
| `OXIDE_SLOC_ROOT` | Working directory for output files and relative path resolution | current directory |
| `SLOC_BROWSER` | Path to Chromium-based browser for PDF export | auto-detected |
| `SLOC_BROWSER_NOSANDBOX` | Set to `1` to add `--no-sandbox` to Chromium args (required in Docker) | unset |
| `SLOC_REGISTRY_PATH` | Override path for `registry.json` | `<out-dir>/registry.json` |
| `SLOC_API_KEYS` | **Comma-separated** full-access API keys (server mode). This is what `install-systemd.sh` and `deploy/corp.env.example` write. | unset |
| `SLOC_API_KEY` | Singular alias — a single bearer token. Either variable is honored; `SLOC_API_KEYS` is preferred for rotation. | unset (no auth) |
| `SLOC_BIND` | Listen address (`ip:port`). Overridden by `--bind`; overrides the config file. | `0.0.0.0:4317` (server) / `127.0.0.1:4317` (local) |
| `SLOC_ALLOWED_ROOTS` | **Colon-separated** absolute paths the web UI may scan. **Server mode is fail-closed:** while unset, every server-side path scan is rejected with HTTP 403. Uploaded folders (Browse) always work regardless. | unset ⇒ no server-side path scanning |
| `SLOC_RATE_LIMIT` | Per-client-IP request budget per 60-second window | `120` (server) / `60` (local) |
| `SLOC_ALLOWED_ORIGINS` | Comma-separated origins allowed to make cross-origin browser API calls (CORS) | unset ⇒ same-origin only |
| `SLOC_TRUST_PROXY` | Set to `1` to trust `X-Forwarded-For` (behind a reverse proxy) so limits/logs are per-client | unset |
| `SLOC_AUTH_LOCKOUT_FAILS` | Failed auth attempts from one IP before a temporary lockout | `10` |
| `SLOC_AUTH_LOCKOUT_SECS` | Lockout duration in seconds after `SLOC_AUTH_LOCKOUT_FAILS` failures | `3600` |
| `SLOC_TLS_CERT` | Path to PEM certificate file for native TLS | unset |
| `SLOC_TLS_KEY` | Path to PEM private key file for native TLS | unset |
| `SLOC_ALLOWED_HOSTS` | Comma/space-separated `Host` header allowlist. When set (server mode), requests with any other `Host` are rejected `421` — blocks DNS-rebinding / Host-header injection. Health/metrics/webhook paths are exempt; the `SLOC_PUBLIC_URL` host is auto-trusted. | unset ⇒ any Host accepted |
| `SLOC_PUBLIC_URL` | Canonical URL clients use (e.g. `https://sloc.corp.local`). Printed at startup with a DNS-resolution check and the exact A-record / hosts-file line to add so LAN users can reach the server by name. | unset |
| `SLOC_ADMIN_KEY` | Separate operator admin credential. When set, `GET /api/admin/config` (effective-config view) requires it specifically, not a regular user key. | unset ⇒ falls back to the API-key gate |
| `SLOC_MAX_DISK_MB` | Hard ceiling (MB) on the total size of retained scan artifacts. A background guard deletes the oldest runs when the tree exceeds it, uploads are refused `507` when staging is full, and the UI cleanup policy can only tighten below it. | unset ⇒ no size cap |
| `SLOC_EXPORT_DIR` | Destination directory (a mounted network/SMB share or any path) for the `share` export target. | unset |
| `SLOC_EXPORT_GIT_REPO` | Repository URL for the `git` export target (artifacts are committed under a per-run subdirectory and pushed). Uses the same host-allowlist / credential / SSRF gates as git scanning. | unset |
| `SLOC_EXPORT_GIT_BRANCH` | Branch the `git` export target pushes to. | `oxide-sloc-reports` |
| `SLOC_EXPORT_AUTO` | Set to `1` to auto-publish each finished scan to the `share` target. | unset ⇒ manual export only |
| `SLOC_AIRGAP` | Force the web UI's air-gapped presentation. `1`/`true` marks the deployment offline: the shared footer repoints "View on GitHub" at this build's own repository (when it is a reachable internal fork) and renders the author / AGPL-licence links as plain text, since those point at the public internet; an "Air-gapped" pill appears next to the Local/Server status indicator. `0`/`false` forces the online presentation. Overrides the `[reporting] offline_mode` config key and the automatic detection. Detection never makes an outbound connection. | unset ⇒ auto-detect from the build's git origin (a non-public/internal host ⇒ offline) |
| `SLOC_BUG_REPORT_URL` | Repository / issue-tracker URL the "Report a Bug" page (and the offline footer repoint) targets. Air-gapped forks set this to their internal GitLab/Bitbucket repo. Setting it to an internal host also auto-flips the UI into offline mode. Overrides `[reporting] bug_report_url` and the build-time origin. | unset ⇒ build-time origin, then upstream default |
| `RUST_LOG` | Tracing log level (`info`, `debug`, `warn`) | `info` |

> The full menu of hardening knobs (mTLS, audit-log HMAC, CORS, frame-ancestors,
> read-only keys, session idle timeout) lives in
> [`deploy/corp.env.example`](../deploy/corp.env.example).

---

## Hosting on a corporate LAN / VLAN / VPN / air-gap

Everything below is configured by the **host operator** (environment / systemd unit /
config TOML). LAN users never touch these settings — the watched-folder, cleanup, disk,
export, and hostname controls are all operator-owned.

### Let users reach the server by hostname (not a bare IP)

oxide-sloc runs unprivileged, so it does **not** edit host DNS itself. Instead, set the
canonical URL and it validates + guides you:

```bash
SLOC_PUBLIC_URL=https://sloc.corp.local:4317
```

At startup the server prints whether `sloc.corp.local` resolves to this machine and, if
not, the exact record to add — either a DNS **A record** on your DNS server, or a
per-client `hosts` line (`C:\Windows\System32\drivers\etc\hosts` / `/etc/hosts`). Then
pin the names the server will answer to (defense against DNS-rebinding / Host-header
injection):

```bash
SLOC_ALLOWED_HOSTS=sloc.corp.local,10.20.0.5   # the SLOC_PUBLIC_URL host is auto-added
```

Requests whose `Host` header isn't on the list get `421 Misdirected Request`. Health,
metrics, and webhook endpoints are exempt so load-balancer probes keep working.

### Cap disk usage (garbage collection)

Bound how much disk retained artifacts can consume — a background guard deletes the
**oldest** runs when the tree exceeds the cap, and uploads are refused with `507` when
the staging area is full:

```bash
SLOC_MAX_DISK_MB=20000     # 20 GB hard ceiling
```

This is the operator ceiling. The in-UI auto-cleanup policy (age / count / size) can only
set an *equal or tighter* size cap — never loosen beyond `SLOC_MAX_DISK_MB`.

### Publish reports off-box

Point finished runs at a destination that survives host cleanup. Three targets, invoked
per run via `POST /api/runs/{run_id}/export?target=<share|git|confluence>`:

```bash
# 1) Mounted network / SMB share (simplest, air-gap friendly)
SLOC_EXPORT_DIR=/mnt/reports-share
SLOC_EXPORT_AUTO=1                       # optional: auto-publish every scan to the share

# 2) Git repository (committed under a per-run subdir, pushed to a branch)
SLOC_EXPORT_GIT_REPO=https://git.corp.local/team/sloc-reports.git
SLOC_EXPORT_GIT_BRANCH=oxide-sloc-reports   # default shown

# 3) Confluence — uses the existing Confluence integration (configure at /integrations)
```

Git export reuses the same host-allowlist, per-host credential resolution
(`SLOC_GIT_CRED_<HOST>`), and SSRF gates as git scanning, so it works the same on a
locked-down VLAN or through a corporate proxy.

### Operator-only config view

With `SLOC_ADMIN_KEY` set, `GET /api/admin/config` returns the effective security posture
(which controls are active, the disk ceiling, export target, hostname settings) and
requires that admin key specifically — a regular user key is not enough:

```bash
curl -H "X-Admin-Key: $SLOC_ADMIN_KEY" https://sloc.corp.local:4317/api/admin/config
```

### Air-gap notes

- No outbound internet is required: a plain `git clone` builds and runs offline (vendored
  crates + bundled toolchain). Export to a **share** or an **internal** git repo needs no
  external connectivity.
- Behind a TLS-inspecting proxy, the OS trust store is used automatically; set
  `HTTPS_PROXY` / `NO_PROXY` for git operations as usual.
- For a fully closed network, prefer `SLOC_EXPORT_DIR` (a mounted share) over git/Confluence.

---

## Health check

Three equivalent endpoints return `200 OK` with body `ok`:

| Endpoint | Use case |
|---|---|
| `GET /healthz` | Traditional; used by the Docker `HEALTHCHECK` directive |
| `GET /api/health` | Conventional REST API path; preferred for monitoring tools |
| `GET /api/version` | Returns `{"name":"oxide-sloc","version":"X.Y.Z"}` — use when the version is needed |

```bash
curl http://localhost:4317/healthz
curl http://localhost:4317/api/health
curl http://localhost:4317/api/version
```

The Docker image includes a `HEALTHCHECK` that polls `/healthz` every 30 seconds.
