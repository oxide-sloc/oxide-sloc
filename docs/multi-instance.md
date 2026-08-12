# Scanning multiple git instances (multi-instance & cross-network)

oxide-sloc is built to scan code that lives on a **different** git host than the one hosting the
tool or the pipeline — including across corporate networks, proxies, VLANs, subnet segmentation,
and fully air-gapped environments.

> **The scenario this solves.** Your tooling / pipeline definition lives on
> `bitbucket.instance1.com`. The code you want to scan lives on `bitbucket.instance2.com` (and
> maybe `github.enterprise.instance3.com`, and an internal GitLab, and …). Each instance may need
> its own credentials, and some may be reachable only over the internal network — or not reachable
> at all, requiring an offline copy.

Everything below works identically whether you run oxide-sloc **locally (CLI)**, **in server
mode**, or **from Jenkins/CI**.

---

## Table of contents

1. [What already works out of the box](#what-already-works-out-of-the-box)
2. [Per-host credentials (the credential registry)](#per-host-credentials-the-credential-registry)
3. [Corporate networks: proxies, VLANs, subnets, TLS inspection](#corporate-networks-proxies-vlans-subnets-tls-inspection)
4. [Air-gapped instances](#air-gapped-instances)
   - [Case A — reachable on the internal network](#case-a--reachable-on-the-internal-network)
   - [Case B — fully disconnected (offline import)](#case-b--fully-disconnected-offline-import)
5. [Running it three ways](#running-it-three-ways)
   - [Locally (CLI)](#locally-cli)
   - [Server mode](#server-mode)
   - [Jenkins / CI](#jenkins--ci)
6. [Environment variable reference](#environment-variable-reference)
7. [Security model](#security-model)

---

## What already works out of the box

- **Any host, any layout.** Browse URLs for GitHub, GitLab, Bitbucket Server/Data Center, and
  Bitbucket Cloud — including self-hosted enterprise instances on any hostname — are normalized to
  a clone URL automatically. SSH (`git@host:…`, `ssh://…`) and `git://` are also accepted.
- **Internal IPs are allowed.** RFC 1918 / IPv6 ULA private addresses (10.x, 192.168.x,
  172.16-31.x, `fd00::/8`) are permitted, so an internal instance on a corporate VLAN clones
  directly. Only loopback, link-local, and cloud-metadata targets are blocked.
- **All three run modes accept a remote target** — CLI `git-scan` / `git-compare` / `watch`, the
  web **Git Browser** (`/git-browser`, `/api/git/*`) and the `git_repo` scan field, and the
  Jenkins `SCAN_REPO_URL` parameter (kept fully separate from the tooling `TOOL_REPO_URL`).

The one thing you must supply for a **private** instance is credentials — see the next section.

---

## Per-host credentials (the credential registry)

oxide-sloc resolves a credential **per hostname** at clone/fetch time. This is what lets a single
run authenticate to `instance1` with token A and `instance2` with token B.

Secrets are supplied through the **environment** (or an optional secrets file) and are **never**
placed on a git command line, never embedded in a URL, and never written to disk — the git
credential helper reads them from the child-process environment at the moment of the fetch.

### Convention

For a host `H`, form `HOSTKEY` by upper-casing the hostname and replacing every non-alphanumeric
character with `_`:

| Hostname | HOSTKEY |
|---|---|
| `bitbucket.instance2.com` | `BITBUCKET_INSTANCE2_COM` |
| `github.enterprise.acme` | `GITHUB_ENTERPRISE_ACME` |
| `git.corp:7990` | `GIT_CORP_7990` |

Then set **one** of:

```bash
# HTTPS personal-access-token (PAT) — value is "username:token"
export SLOC_GIT_CRED_BITBUCKET_INSTANCE2_COM="svc-scanner:ATBB…token…"

# …or an SSH private key for that host
export SLOC_GIT_SSHKEY_GIT_CORP="/home/scanner/.ssh/id_ed25519_corp"
```

**Ports.** When the clone URL carries an explicit port (e.g.
`https://git.corp:7990/team/repo.git`), the **port-qualified** key
(`SLOC_GIT_CRED_GIT_CORP_7990`) is tried first, then the **bare-host** key
(`SLOC_GIT_CRED_GIT_CORP`). Use the port form only when two instances share a
hostname but differ by port; otherwise the bare-host key applies to every port.

Provider username hints for the HTTPS form:

| Provider | Username to use |
|---|---|
| GitHub / GitHub Enterprise | any (e.g. your username) with a PAT as the token |
| GitLab | `oauth2` with a PAT |
| Bitbucket Cloud | `x-token-auth` with an app password / API token |
| Bitbucket Server / Data Center | your username with an HTTP access token |

### Bulk file (many instances)

If you have many instances, point `SLOC_GIT_CRED_FILE` at a file (keep it `chmod 600`, never commit
it) with one `host = "user:token"` entry per line:

```ini
# /etc/oxide-sloc/git-creds  (chmod 600)
bitbucket.instance2.com = "svc-scanner:ATBB-token-a"
github.enterprise.acme  = "svc-scanner:ghp-token-b"
```

### Precedence & fallback

For each host, resolution is: `SLOC_GIT_CRED_<HOSTKEY>` → `SLOC_GIT_SSHKEY_<HOSTKEY>` →
`SLOC_GIT_CRED_FILE`. If **none** matches, oxide-sloc falls back to git's own resolution (OS
credential helper / Git Credential Manager, `~/.netrc`, `ssh-agent`) exactly as before — so
existing setups keep working with no changes.

> **HOSTKEY collision note.** The mapping is lossy: `a.b.com`, `a-b.com`, and `a_b.com` all map to
> `A_B_COM`. If your instances differ only by punctuation, use `SLOC_GIT_CRED_FILE` (it keys on the
> exact hostname).

### SSH host-key verification (fresh agents)

SSH clones use `BatchMode=yes` and **strict** host-key checking, so a first contact with a host
that isn't yet in `known_hosts` fails with an opaque `Host key verification failed` (Jenkins) or
`could not read from remote repository` / "check access rights" (CLI) — the key was never the
problem. On a fresh agent, seed the host key once before scanning:

```bash
ssh-keyscan -p 22 git.corp >> ~/.ssh/known_hosts   # use -p <port> for a non-standard SSH port
```

On Jenkins, configure **Manage Jenkins → Security → Git Host Key Verification Configuration**
(e.g. "Accept first connection" or a known-hosts file) instead.

If you accept trust-on-first-use, set `SLOC_GIT_SSH_ACCEPT_NEW=1` — oxide-sloc then adds
`StrictHostKeyChecking=accept-new` to its SSH command so the first connection records the host key
instead of failing. The default stays strict; only opt in on trusted networks.

---

## Corporate networks: proxies, VLANs, subnets, TLS inspection

A **single system proxy** covers all instances — git honours the standard variables, and oxide-sloc
does nothing to override them:

```bash
export HTTPS_PROXY=http://proxy.corp:3128
export HTTP_PROXY=http://proxy.corp:3128
# Bypass the proxy for internal VLAN instances (direct LAN route):
export NO_PROXY=bitbucket.instance2.com,10.0.0.0/8,192.168.0.0/16,.corp
```

- **VLAN / subnet segmentation.** As long as the machine (or Jenkins agent) running oxide-sloc has
  a network route to the instance, it clones directly. Internal RFC 1918 addresses are allowed.
  Put internal hosts in `NO_PROXY` so they take the direct LAN route rather than the egress proxy.
- **TLS-inspecting proxies.** On Windows, oxide-sloc validates against the Windows system
  certificate store (`http.sslBackend=schannel`), so an enterprise root CA that IT already deploys
  is trusted automatically — the same reason the repo opens in a browser. On Linux, install the
  corporate root CA into the system trust store. Only as a last resort (a self-signed cert in no
  trust store) set `SLOC_GIT_SSL_NO_VERIFY=1`.
- **Slow / flaky links.** A stalled clone aborts fast (low-speed guard) and the whole git call has
  a wall-clock ceiling — raise it with `SLOC_GIT_TIMEOUT=<seconds>` on very large repos or slow
  VPNs.
- **Pin reachable instances.** For an internet-facing or multi-tenant deployment, restrict which
  hosts may be cloned with `SLOC_GIT_HOST_ALLOWLIST=bitbucket.instance2.com,github.enterprise.acme`
  and enforce it with `SLOC_GIT_REQUIRE_ALLOWLIST=1` (fail-closed).

---

## Air-gapped instances

### Case A — reachable on the internal network

If the instance is on the corporate network (internal DNS / RFC 1918) with no internet, this is
just the normal remote path: set the per-host credential and clone directly over the LAN. Use
`NO_PROXY` so internal hosts skip any egress proxy. Nothing special is required.

### Case B — fully disconnected (offline import)

When the target cannot be reached at all, bring the code in as an offline copy and scan that. This
is **off by default** (a bare `file:///…` clone would otherwise be a local-file-read risk on a
network-facing server) and is fail-closed: you must enable it **and** confine it to a directory.

```bash
export SLOC_GIT_ALLOW_LOCAL=1
export SLOC_GIT_LOCAL_ROOT=/srv/oxide-sloc/imports   # sources must resolve under here
```

Supported offline sources (all must sit under `SLOC_GIT_LOCAL_ROOT`):

| Form | Example | How it's made |
|---|---|---|
| **git bundle** | `/srv/oxide-sloc/imports/widget.bundle` | on a connected machine: `git bundle create widget.bundle --all` |
| **`file://` mirror** | `file:///srv/oxide-sloc/imports/widget.git` | copy a bare/normal clone into the import dir |
| **local path** | `/srv/oxide-sloc/imports/widget` | copy a checkout into the import dir |

A git bundle is usually the best air-gap carrier: it is a single file that preserves full history
and branches/tags, so the resulting scan (and `git-compare` between refs) behaves like the real
repo.

Rejected on purpose: `file://host/…` and UNC `\\server\share` — those are SMB **network** fetches
to an arbitrary host, so they are treated as remote and refused under the local gate.

---

## Running it three ways

### Locally (CLI)

```bash
# Private remote instance — credential comes from the env registry, resolved by hostname.
export SLOC_GIT_CRED_BITBUCKET_INSTANCE2_COM="svc-scanner:ATBB-token"
oxide-sloc git-scan https://bitbucket.instance2.com/scm/proj/widget.git --git-ref main \
  --html-out out/widget.html

# Offline bundle import.
oxide-sloc git-scan /mnt/usb/widget.bundle \
  --allow-local --local-root /mnt/usb --json-out out/widget.json

# Compare two refs on a private instance.
oxide-sloc git-compare https://bitbucket.instance2.com/scm/proj/widget.git v1.0 v1.1
```

`--allow-local` / `--local-root` are available on `git-scan`, `git-compare`, and `watch`. An
explicit `SLOC_GIT_*` env var always wins over the flag.

### Server mode

Set the registry (and, if you need offline import, the local gate) in the **server's** environment
before starting it — credentials then apply to every scan the server runs, per host:

```bash
export SLOC_API_KEY=…                                   # server mode requires auth
export SLOC_GIT_CRED_BITBUCKET_INSTANCE2_COM="svc-scanner:ATBB-token"
export SLOC_GIT_CRED_GITHUB_ENTERPRISE_ACME="svc:ghp-token"
# Optional: pin the instances that may be scanned.
export SLOC_GIT_HOST_ALLOWLIST="bitbucket.instance2.com,github.enterprise.acme"
export SLOC_GIT_REQUIRE_ALLOWLIST=1
oxide-sloc serve --server
```

Users then paste `https://bitbucket.instance2.com/scm/proj/widget.git` into the Git Browser or the
scan form and it authenticates automatically. Offline import stays **off** in server mode unless
you explicitly set `SLOC_GIT_ALLOW_LOCAL=1` **and** `SLOC_GIT_LOCAL_ROOT` (fail-closed).

The same two non-secret gate settings can live in the config TOML instead of the environment:

```toml
[git]
allow_local = true
local_root = "/srv/oxide-sloc/imports"
```

(Secrets are **never** read from the TOML — only from the environment or `SLOC_GIT_CRED_FILE`.)

### Jenkins / CI

The pipeline already separates the tooling repo (`TOOL_REPO_URL`) from the project being scanned
(`SCAN_REPO_URL`), so scanning a different instance than the one hosting the pipeline is the
normal case. Three credential options:

1. **Multi-instance in one run (idiomatic).** Set the multi-line `SCAN_GIT_CREDENTIALS` parameter,
   one `host=jenkins-credentials-id` per line. The pipeline auto-selects the Jenkins credential whose
   host matches `SCAN_REPO_URL` (falling back to the single `SCAN_CREDENTIALS_ID`), so **one job can
   scan many GitHub/Bitbucket/GitLab instances**, each credential injected natively by the git plugin
   — never in an env var, argv, or the URL. This mirrors the app-side per-host
   `SLOC_GIT_CRED_<HOSTKEY>` registry described above.
2. **Single Jenkins credential.** Set `SCAN_CREDENTIALS_ID` to one Jenkins credential
   (username+token, or SSH key). Jenkins injects it during the target checkout. Best when the whole
   fan-out is one instance per run.
3. **Per-host env registry (app does the clone).** Expose the tokens as Jenkins secrets and
   map them to `SLOC_GIT_CRED_<HOSTKEY>` in the environment for the scan step — handy when the app
   itself does the clone (webhook-triggered scans).

For an **air-gapped agent**, enable offline import: check the `SCAN_ALLOW_LOCAL` parameter and set
`SCAN_LOCAL_ROOT` (mapped to `SLOC_GIT_ALLOW_LOCAL` / `SLOC_GIT_LOCAL_ROOT`) to the workspace/artifact
directory that holds the imported bundle, then pass that bundle path as `SCAN_REPO_URL` (Case B above).

See [ci-integrations.md](ci-integrations.md) and [airgap.md](airgap.md) for full pipeline detail.

---

## Environment variable reference

| Variable | Purpose |
|---|---|
| `SLOC_GIT_CRED_<HOSTKEY>` | HTTPS `username:token` for a host (PAT). |
| `SLOC_GIT_SSHKEY_<HOSTKEY>` | Path to an SSH private key for a host. |
| `SLOC_GIT_CRED_FILE` | Bulk `host = "user:token"` secrets file (chmod 600; never commit). |
| `SLOC_GIT_ALLOW_LOCAL` | `1`/`true` to permit offline import (bundle / `file://` / local path). Default off. |
| `SLOC_GIT_LOCAL_ROOT` | Directory offline sources must resolve under. Required when `SLOC_GIT_ALLOW_LOCAL` is on. |
| `SLOC_GIT_HOST_ALLOWLIST` | Comma-separated hosts permitted for clone (SSRF control). |
| `SLOC_GIT_REQUIRE_ALLOWLIST` | `1`/`true` to fail-closed unless the host is allowlisted. |
| `SLOC_GIT_SSL_NO_VERIFY` | Last-resort: disable TLS verification (self-signed CA not in any trust store). |
| `SLOC_GIT_TIMEOUT` | Per-git-subprocess wall-clock ceiling in seconds (default 300). |
| `HTTPS_PROXY` / `HTTP_PROXY` / `NO_PROXY` | Standard proxy controls, honoured by git. |

---

## Security model

- **Secrets never leak into argv or disk.** The HTTPS credential is answered by an ephemeral git
  credential helper that reads the token from the child-process environment; the helper text and
  the repo's persisted config contain only the variable *names*, so `ps` / Task Manager and
  `.git/config` never show the token. Credentials are deliberately **not** embedded in the clone
  URL (which git would persist to `remote.origin.url`).
- **Offline import is fail-closed.** Disabled by default; when enabled it requires
  `SLOC_GIT_LOCAL_ROOT` and rejects any source that canonicalizes outside it (defeating `..` and
  symlink traversal), plus all UNC / `file://host` SMB fetches. This prevents a network-facing
  server from being coerced into reading arbitrary local files.
- **SSRF hardening is unchanged.** Remote clones still block loopback / link-local /
  cloud-metadata, refuse HTTP→internal redirects, and can be pinned to an explicit host allowlist.
