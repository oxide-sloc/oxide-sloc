# oxide-sloc CI — upstream/downstream chaining test

This demonstrates and tests the oxide-sloc pipeline's **Pipeline-of-Pipelines**
chaining, which is already built into the current job configuration:

- The Jenkinsfile exposes `CHAIN_UPSTREAM_JOB`, `CHAIN_UPSTREAM_BUILD`, and
  `CHAIN_DOWNSTREAM_JOB` parameters.
- On success, `runPostSuccess()` (in `ci/jenkins/pipeline-helpers.groovy`)
  fires `build job: params.CHAIN_DOWNSTREAM_JOB` — fire-and-forget (`wait: false,
  propagate: false`) — passing `CHAIN_UPSTREAM_JOB`, `CHAIN_UPSTREAM_BUILD`, and
  `ARTIFACT_PATH` down to it.

No change to the `oxide-sloc` job is required. The seed here only adds the two
**partner** jobs so the chain can actually be run and observed.

```
oxide-sloc-chain-upstream          (orchestrator you start)
        │   build job: 'oxide-sloc', wait: true, propagate: true
        │   passes CHAIN_DOWNSTREAM_JOB, CHAIN_UPSTREAM_JOB, CHAIN_UPSTREAM_BUILD, BUILD_MODE
        ▼
     oxide-sloc                     (the real pipeline)
        │   post { success } → build job: CHAIN_DOWNSTREAM_JOB, wait: false
        │   passes CHAIN_UPSTREAM_JOB=oxide-sloc, CHAIN_UPSTREAM_BUILD=#N, ARTIFACT_PATH
        ▼
oxide-sloc-chain-downstream         (consumer; prints the hand-off, copies artifacts)
```

## 1. Create the demo jobs

Prerequisites: the `oxide-sloc` pipeline job already exists (via
`ci/jenkins/seed-job.groovy`), and the **Job DSL** plugin is installed.

1. New Item → **Freestyle project** → name it `oxide-sloc-chain-seed`.
2. Add build step **Process Job DSLs** → *Look on Filesystem* →
   `ci/jenkins/demo/seed-chain-demo.groovy`.
   (If your pipeline job isn't named `oxide-sloc`, add a String parameter
   `TARGET_JOB` to the seed with the correct name.)
3. **Save** and **Build** the seed once. If prompted, approve it under
   *Manage Jenkins → In-process Script Approval*.

This creates `oxide-sloc-chain-upstream` and `oxide-sloc-chain-downstream`.

## 2. Run the test

Build **`oxide-sloc-chain-upstream`** (Build with Parameters — the defaults are
fine; `SCAN_PATH` defaults to `testing/fixtures/basic`).

It leaves `RUN_QUALITY_GATES` and `RUN_WEB_HEALTHCHECK` at their defaults (both off)
so the demo is fast: on the default `BUILD_MODE=prebuilt` path oxide-sloc extracts the
committed binary, scans, and reports, skipping fmt/lint/test and the web health check.

## 3. What you should see (chain verified)

**`oxide-sloc-chain-upstream` console:**
```
Triggering oxide-sloc with DOWNSTREAM_JOB=oxide-sloc-chain-downstream
Starting building: oxide-sloc #<N>
...
oxide-sloc #<N> finished with result SUCCESS
On success it fire-and-forget triggers oxide-sloc-chain-downstream — check that job's builds.
```

**`oxide-sloc` build #N:** runs normally; its `post { success }` triggers the
downstream job. (Its build page description shows the SLOC summary; the trigger
is fire-and-forget so oxide-sloc does not wait for the consumer.)

**`oxide-sloc-chain-downstream` — a new build appears automatically:**
```
=== oxide-sloc downstream consumer ===
Triggered by upstream job : oxide-sloc
Upstream build number      : #<N>
Artifact path from upstream: <workspace>/target/release/oxide-sloc
Copied upstream artifacts into ./from-upstream (Copy Artifact plugin present).
=== downstream consumer complete — chain verified ===
```
Its build description reads `downstream of oxide-sloc #<N>`.

If the **Copy Artifact** plugin isn't installed you'll see
`copyArtifacts skipped (install the 'Copy Artifact' plugin to enable): …`
instead of the copy line — the chain still verifies; only the artifact copy is
optional.

## Verifying the two directions independently

- **Downstream trigger** (oxide-sloc → consumer): run `oxide-sloc` directly with
  `CHAIN_DOWNSTREAM_JOB = oxide-sloc-chain-downstream`. A downstream build appears on
  success.
- **Upstream trigger** (orchestrator → oxide-sloc): that's what
  `oxide-sloc-chain-upstream` does; `CHAIN_UPSTREAM_JOB`/`CHAIN_UPSTREAM_BUILD` show up in
  the oxide-sloc build's parameters.

## Cleanup

Delete `oxide-sloc-chain-upstream`, `oxide-sloc-chain-downstream`, and
`oxide-sloc-chain-seed` when done. They do not affect the main `oxide-sloc` job.
