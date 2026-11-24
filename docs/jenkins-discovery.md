# Jenkins project discovery and managed scans

This guide covers the **opt-in** discovery feature: a Jenkins-operated way to
find repositories in a Bitbucket project and keep a small, managed trigger job
per repository/branch that invokes the central `oxide-sloc` scan job. It changes
nothing about the central job itself — if you do not set it up, nothing happens.

For the single-job integration (point Jenkins at one repo, or have a repo
trigger Jenkins), see [`ci/jenkins/INTEGRATION.md`](../ci/jenkins/INTEGRATION.md).
Discovery sits on top of that central job; it does not replace it.

## When to use it

Use discovery when you want **zero-touch** scanning across many existing
repositories without adding a `Jenkinsfile` to any of them. If every repository
already carries a `Jenkinsfile`, a plain Bitbucket multibranch/organization
folder may suit you better; discovery is the baseline for arbitrary projects.

## Architecture

```
Bitbucket project REST API
        │
        ▼
oxide-sloc-discover (scheduled seed)  ──creates/updates──▶  managed trigger jobs
        │                                                    under an owned folder
Bitbucket push webhook ──▶ oxide-sloc-dispatch ──▶ central oxide-sloc job → artifacts
```

- **Central job** (`oxide-sloc`, root `Jenkinsfile`) is the only job that
  obtains and runs the scanner. One upgrade point, one security boundary.
- **Managed trigger jobs** live only under a folder you own
  (default `oxide-sloc-scans/<host>/<project>/<repo>/<branch>`). They schedule a
  nightly scan and are invoked by the dispatcher on an approved push.

## Two modes of `Jenkinsfile.discovery`

### DISCOVER

Builds (or loads) a normalized inventory and runs the Job DSL seed to
create/update managed jobs. Run it on a schedule (e.g. `H/30 * * * *`) and on
demand after a new repository is created.

- **Online:** set `PROVIDER`, `BASE_URL`/`PROJECT` (Server/DC) or
  `WORKSPACE_SLUG` (Cloud), `BITBUCKET_CREDENTIALS` (a Jenkins user/token
  credential for the REST API), and `CREDENTIAL_ID` (the credential id stored on
  generated jobs so the central job can clone).
- **Air-gapped:** produce the inventory on a connected machine
  (`discover_bitbucket.py … --out inventory.json`), transfer it, and set
  `OFFLINE_INVENTORY` to its path. No HTTP is performed.

Configure the seed step's **Action for removed jobs = Disable** (the pipeline
already passes `removedJobAction: 'DISABLE'`): a repository that leaves the
inventory is disabled with an explanation, never deleted, so build history
survives.

### DISPATCH

Handles one validated Bitbucket push webhook (typically via the Generic Webhook
Trigger plugin). It verifies the event, normalizes it, resolves it against the
**approved** inventory, and triggers the matching managed job with the exact
pushed commit SHA. It never trusts a clone URL or job name from the payload.

## Configuration reference

All values are pipeline parameters or environment variables — nothing
organization-specific is hardcoded.

| Name | Default | Purpose |
|---|---|---|
| `MODE` | `DISCOVER` | `DISCOVER` (seed) or `DISPATCH` (event). |
| `MANAGED_FOLDER` | `oxide-sloc-scans` | Folder discovery exclusively owns. |
| `CENTRAL_JOB` | `oxide-sloc` | Central scan job the managed jobs invoke. |
| `BRANCH_POLICY` | `^(main\|develop\|release/.+)$` | Allowed refs; validated at startup. |
| `NIGHTLY_CRON` | `H H(1-4) * * *` | Nightly schedule on each managed job (Jenkins staggers by name). |
| `POLICY_REVISION` | *(empty)* | Opaque tag stored on jobs for auditing. |
| `PROVIDER` | `bitbucket-server` | `bitbucket-server` or `bitbucket-cloud`. |
| `BASE_URL` / `PROJECT` | *(empty)* | Server/DC base URL and project key. |
| `WORKSPACE_SLUG` | *(empty)* | Cloud workspace. |
| `CREDENTIAL_ID` | *(empty)* | Jenkins credential id stored on generated jobs. |
| `BITBUCKET_CREDENTIALS` | *(empty)* | Jenkins user/token credential for the REST API. |
| `OFFLINE_INVENTORY` | *(empty)* | Air-gap: path to a transferred inventory. |
| `PREFER_SSH` | `false` | Prefer SSH clone URLs. |
| `DRY_RUN` | `false` | Plan only; create nothing, trigger nothing. |
| `MAX_REPOS` | `500` | Fail closed above this many repositories. |
| `MAX_BRANCHES_PER_REPO` | `25` | Fail closed above this many branches per repo. |
| `EVENT_PAYLOAD` / `EVENT_SIGNATURE` | *(empty)* | DISPATCH: raw push JSON + signature/token header. |
| `COOLDOWN_SECONDS` | `120` | Collapse duplicate deliveries per repo_id+SHA. |

Discovery-client environment (secret, request-time only):
`BITBUCKET_USER`, `BITBUCKET_TOKEN`. Webhook verification secret:
`WEBHOOK_SECRET` (HMAC `sha256=…` or a shared token).

## CLI usage (outside Jenkins, for testing/air-gap)

```bash
# Build an inventory from Bitbucket Server/DC
BITBUCKET_USER=svc BITBUCKET_TOKEN=*** \
python3 ci/jenkins/discovery/discover_bitbucket.py \
  --provider bitbucket-server --base-url https://git.example.invalid \
  --project PROJ --credential-id bb-example-readonly --out inventory.json

# Re-normalize a transferred inventory on an air-gapped host (no network)
python3 ci/jenkins/discovery/discover_bitbucket.py \
  --offline-inventory inbox/inventory.json --out inventory.json

# Normalize + resolve a push payload against the approved inventory
python3 ci/jenkins/discovery/dispatch_event.py \
  --payload event.json --inventory inventory.json
```

## Pilot / migration

1. **Dry run first.** Run DISCOVER with `DRY_RUN=true` and review the planned
   folder/job set and the archived `inventory.json`.
2. **Narrow scope.** Start with name include/allowlist filters and a small
   `MAX_REPOS`, so the first real pass creates a handful of jobs.
3. **Enable nightly only.** Let the nightly schedule run for a day before wiring
   webhooks.
4. **Add DISPATCH.** Configure the Generic Webhook Trigger and `WEBHOOK_SECRET`,
   then point a single repository's Bitbucket webhook at it and confirm a push
   to `main` starts exactly one SHA-pinned scan.
5. **Widen.** Relax filters / raise limits deliberately as you gain confidence.

## Security notes

- Generated jobs store only non-secret metadata (clone URL, Jenkins credential
  **id**, branch, policy revision). No token, key, or TLS-bypass flag is ever
  rendered into a job.
- The dispatcher is the security boundary: it resolves every event against the
  approved inventory by immutable `repo_id` + `branch`, so a forged/spoofed
  webhook cannot cause an arbitrary-URL scan or build an arbitrary job. Deletes,
  tags, PR events, unknown repos, and out-of-policy branches are ignored without
  failing the endpoint.
- Keep the whole feature opt-in and bounded (`MAX_REPOS`,
  `MAX_BRANCHES_PER_REPO`, `MAX_CONCURRENT_SCANS` via a Jenkins lock/throttle,
  `COOLDOWN_SECONDS`) so a large project cannot overwhelm the controller.
