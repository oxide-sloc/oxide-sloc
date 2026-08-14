# Downstream triggers — run an oxide-sloc scan when any build finishes

This directory lets **any upstream CI build** — Jenkins, GitHub Actions, GitLab
CI, Bitbucket Pipelines, or a plain cron/shell job — kick off an oxide-sloc scan
of the repo it just built, the moment that build completes. It is designed to
work **even when the upstream pipeline is ancient**: no plugins, no modern CI
syntax, and no cooperation from the upstream tool beyond running one shell line.

```
┌─────────────────┐   build finishes   ┌──────────────────────────┐
│ upstream build  │ ─────────────────▶ │ trigger-oxide-sloc.sh/.ps1│
│ (any CI, any    │   (final step)     │  guards + signs + posts   │
│  age)           │                    └────────────┬─────────────┘
└─────────────────┘                                 │
                                    server mode  ▼        ▼ dispatch mode
                       POST /webhooks/ci (signed)      kick a downstream
                       to `oxide-sloc serve`           CI pipeline that
                              │                        runs `oxide-sloc analyze`
                              ▼
                       clone @ ref → scan → artifacts
```

## Contents

| File | What it is |
|---|---|
| `trigger-oxide-sloc.sh` | Portable POSIX-sh trigger (Linux, macOS, Git Bash). The primitive every pipeline calls. |
| `trigger-oxide-sloc.ps1` | Windows PowerShell twin (PowerShell 5.1+; legacy Windows Jenkins nodes). |
| `examples/` | Copy-paste wiring for GitHub, GitLab, Bitbucket, Jenkins (declarative **and** legacy freestyle). |
| `tests/test-trigger-guards.sh` | Offline behavioural test of every guard (stubbed `curl`). |
| `tests/integration-live.sh` | Live end-to-end test against a real `oxide-sloc serve`. |

## Two modes

The trigger supports two delivery models — you can use either or both:

1. **`server` (default)** — POST a signed *build-complete* event to a long-lived
   `oxide-sloc serve` instance at `POST /webhooks/ci`. The server clones the ref
   and scans it. Best when you already run the web UI/server.
2. **`dispatch`** — start a *downstream CI pipeline* (`github` | `gitlab` |
   `jenkins` | `bitbucket`) that itself runs `oxide-sloc analyze`. Best when you
   have no running server and want the scan to happen as a normal CI job.

## The guards (why this is reliable)

Because the guards live in the **caller**, they hold no matter how old the
upstream pipeline is:

| Guard | Where | Behaviour |
|---|---|---|
| **Success gate** | script + server | A non-success `--status` is skipped locally (no call) and, if it ever reaches the server, is skipped there too. |
| **HMAC signing** | script → server | The exact JSON body is signed HMAC-SHA256 with a shared secret; the server only scans if the signature matches a configured schedule. |
| **Idempotency** | script → server | A stable `<system>:<job>:<build_id>` key de-duplicates retries — a flaky upstream that fires the notification twice triggers **one** scan. |
| **Retry + backoff** | script | Transient 5xx/network errors retry with exponential backoff; a 4xx fails fast (no wasted retries). |
| **Fail-closed** | script | Missing/blank required config exits non-zero *before* any network call — never silently no-ops. |
| **No auth oracle** | server | A bad signature returns the same generic `202` as success, so the endpoint cannot be used to probe which repos/secrets exist. |

## Setup (server mode) — 3 steps

1. **Run the server** and register a scan schedule for the repo, with a shared
   secret. Either use the Integrations page (`/integrations`) or the API:

   ```bash
   curl -X POST "$OXIDE_SLOC_URL/api/schedules" -H 'Content-Type: application/json' \
     --data '{"label":"myapp","repo_url":"https://git.example.com/org/myapp.git",
              "branch":"main","kind":"webhook","provider":"any",
              "webhook_secret":"<SHARED_SECRET>"}'
   ```

   The `repo_url` + `branch` are how inbound events are matched to a schedule;
   `webhook_secret` is the HMAC key the trigger must sign with.

2. **Give the upstream build the secret and URL** (as masked CI secrets):
   `OXIDE_SLOC_URL` and `OXIDE_SLOC_SECRET`.

3. **Add the trigger to the end of the upstream build.** See `examples/` for
   your platform. The one-liner is always:

   ```sh
   ci/downstream-trigger/trigger-oxide-sloc.sh \
     --repo "$REPO_URL" --branch "$BRANCH" --commit "$COMMIT" \
     --status success --system <ci> --job "$JOB" --build-id "$BUILD_ID"
   ```

## The `/webhooks/ci` contract

- **Method / path:** `POST /webhooks/ci` (public route; secured per-schedule by
  HMAC, not by the server API key). 512 KB body cap.
- **Header:** `X-Sloc-Signature: sha256=<hex>` — HMAC-SHA256 of the **exact**
  request body using the schedule's `webhook_secret`.
- **Body:**

  ```json
  {
    "repo_url": "https://git.example.com/org/myapp.git",
    "branch": "main",
    "commit_sha": "",                       // optional; blank = branch tip
    "status": "success",                    // non-success values are skipped
    "upstream": {
      "system": "jenkins",
      "job": "myapp-pipeline",
      "build_id": "1234",                   // enables de-duplication
      "url": "https://ci.example.com/job/1234"
    }
  }
  ```

- **Responses:**
  - `202 Accepted` — event accepted (a scan was spawned, **or** nothing matched /
    the signature was invalid — deliberately indistinguishable).
  - `200 OK {"status":"skipped","reason":...}` — a **non-secret** guard declined:
    `"upstream build not successful"` or `"duplicate upstream build"`.
  - `400 Bad Request` — malformed JSON, or missing `repo_url` / `branch`.

Recognised success tokens (case-insensitive): `success`, `succeeded`, `passed`,
`pass`, `ok`, `green`, `completed`, `0`, `true`.

## Very outdated pipelines

The trigger is intentionally the lowest common denominator:

- **`/bin/sh`**, not bash — runs on dash, busybox, old AIX/Solaris shells.
- HMAC via **`openssl`** with a **`python`** fallback — one is always present.
- **Legacy Jenkins freestyle:** add one "Execute shell" step —
  see `examples/jenkins-freestyle-post-build.sh`. No pipeline, Groovy, or
  plugins required.
- **Old Windows nodes:** `trigger-oxide-sloc.ps1` targets Windows PowerShell 5.1
  (bundled with Windows) and needs no modules or admin rights.
- If the upstream cannot even check out this repo, copy the single script file
  into the job — it has no dependencies of its own.

## Cross-platform parity

| Capability | Jenkins | GitHub | GitLab | Bitbucket |
|---|:--:|:--:|:--:|:--:|
| Fire on upstream **build finished** | ✅ post / freestyle | ✅ `workflow_run` | ✅ stage order | ✅ step / after-script |
| Success-only gate | ✅ | ✅ | ✅ | ✅ |
| Signed server trigger (`/webhooks/ci`) | ✅ | ✅ | ✅ | ✅ |
| Dispatch a downstream pipeline | ✅ remote build | ✅ `repository_dispatch` | ✅ trigger token | ✅ pipeline API |
| Idempotent (build-id de-dupe) | ✅ | ✅ | ✅ | ✅ |
| Works with legacy/outdated config | ✅ freestyle | ✅ any workflow | ✅ shell runner | ✅ minimal step |

> Jenkins also has a native job-to-job path: the bundled `Jenkinsfile` exposes
> `CHAIN_DOWNSTREAM_JOB` to chain the oxide-sloc job into a larger pipeline.
> The trigger here is the tool-agnostic complement that gives every other CI the
> same reach.

## Testing

```bash
# Offline guard behaviour (no server, no network — stubbed curl):
bash ci/downstream-trigger/tests/test-trigger-guards.sh

# Live end-to-end against a real server (needs a built binary, git, curl, python):
bash ci/downstream-trigger/tests/integration-live.sh
```

Both are green in this repo; the live test exercises accept → duplicate-skip →
non-success-skip → wrong-secret (no scan) → recorded run-id.

## Script options

Every flag has an `OXIDE_SLOC_*` environment equivalent (e.g. `--url` ↔
`OXIDE_SLOC_URL`). Run `trigger-oxide-sloc.sh --help` for the full list. Common:

| Flag | Meaning |
|---|---|
| `--mode server\|dispatch` | delivery model (default `server`) |
| `--url` | server base URL, or the dispatch endpoint |
| `--secret` | shared HMAC secret (server mode) |
| `--repo` / `--branch` / `--commit` | what was built (commit optional) |
| `--status` | upstream result; non-success is skipped |
| `--system` / `--job` / `--build-id` / `--build-url` | upstream provenance; `build-id` enables de-dupe |
| `--retries` / `--timeout` | resilience knobs (defaults 4 / 30s) |
| `--dispatch github\|gitlab\|jenkins\|bitbucket` + `--token` | dispatch-mode target + auth |
| `--always` | ignore the success gate |
| `--dry-run` | print what would be sent; make no network call |
