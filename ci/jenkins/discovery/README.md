# oxide-sloc Jenkins discovery

Opt-in, organization-neutral tooling that discovers repositories in a Bitbucket
project and keeps a small, managed Jenkins trigger job per repository/branch.
Each managed job only invokes the **central** `oxide-sloc` scan job (the root
`Jenkinsfile`); it never obtains or runs the scanner itself. This gives one
upgrade point, one security boundary for scan options, and consistent reports.

> This feature is isolated under `ci/jenkins/discovery/` and changes nothing
> about the central job's default behavior. See the operator guide in
> [`docs/jenkins-discovery.md`](../../../docs/jenkins-discovery.md).

## Flow

```
Bitbucket project REST API
        │
        ▼
discover_bitbucket.py ──▶ normalized inventory.json ──▶ seed-discovery.groovy
                                                              │ (Job DSL)
                                                              ▼
                                          managed trigger jobs under a folder
Bitbucket push webhook ──▶ dispatch_event.py ──▶ build the matching managed job
                                                              │
                                                              ▼
                                       central `oxide-sloc` job → artifacts
```

## Files

| File | Role |
|---|---|
| `inventory.py` | Normalized, secret-free entry schema + job-name/branch/filter helpers; deterministic dump/load. |
| `discover_bitbucket.py` | Server/DC + Cloud pagination clients (stdlib only); emits the inventory. `--offline-inventory` re-normalizes a transferred file with no HTTP. |
| `seed-discovery.groovy` | Job DSL seed: creates/updates folder-bounded managed jobs; fails closed on limits; `DRY_RUN`. |
| `job-template.groovy` | Renders one managed job that invokes the central scan; disabled-not-deleted lifecycle. |
| `dispatch_event.py` | Verifies + normalizes a push webhook and resolves it against the **approved** inventory (SHA-pinned). |
| `Jenkinsfile.discovery` | `DISCOVER` and `DISPATCH` control pipeline. |
| `fixtures/` | Checked-in Bitbucket REST + webhook payloads for offline tests. |
| `tests/` | `make jenkins-tests` guard suites (no network, no pip). |

## Running the tests

```bash
make jenkins-tests        # runs these alongside the other Jenkins guard suites
# or individually:
python3 ci/jenkins/discovery/tests/test_discover_bitbucket.py
python3 ci/jenkins/discovery/tests/test_job_template_guards.py
python3 ci/jenkins/discovery/tests/test_dispatch_event.py
```

## Security model (summary)

- A generated job carries only non-secret metadata (clone URL, Jenkins
  credential **id**, branch, policy revision) — never a token, key, or
  TLS-bypass flag. Discovery credentials come from Jenkins credentials and the
  `BITBUCKET_USER`/`BITBUCKET_TOKEN` environment at request time only.
- The dispatcher treats the webhook as hostile: the pushed branch/SHA are read
  from the payload, but the clone URL, credential id, and job to build are
  looked up in the approved inventory by immutable `repo_id` + `branch`. A
  forged or spoofed event cannot scan an arbitrary URL or build an arbitrary
  job. Only `main`, `develop`, and `release/*` are dispatched; everything else
  is ignored without failing the endpoint.
