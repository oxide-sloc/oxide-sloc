// oxide-sloc — Pipeline-of-Pipelines chaining demo (Job DSL seed)
//
// Stands up two lightweight demo jobs that exercise the oxide-sloc pipeline's
// existing upstream/downstream chaining (the DOWNSTREAM_JOB parameter +
// runPostSuccess's `build job:` trigger). No changes to the oxide-sloc job are
// needed — this only adds the partner jobs so the chain can be run and observed:
//
//     oxide-sloc-chain-upstream   (orchestrator)
//                 │  build job: 'oxide-sloc', wait: true, propagate: true
//                 ▼
//              oxide-sloc          (the real pipeline; DOWNSTREAM_JOB set)
//                 │  on success → build job: DOWNSTREAM_JOB, wait: false
//                 ▼
//     oxide-sloc-chain-downstream (consumer; receives UPSTREAM_JOB/BUILD/ARTIFACT_PATH)
//
// Setup (once):
//   1. Install the "Job DSL" plugin (already required by seed-job.groovy).
//   2. Create a Freestyle job (e.g. "oxide-sloc-chain-seed"), add a build step
//      "Process Job DSLs" → "Look on Filesystem" → this file. Run it once.
//      (Approve the script under Manage Jenkins → In-process Script Approval if
//      prompted.) It creates both demo jobs below.
//   3. The oxide-sloc pipeline job must already exist (seed-job.groovy).
//
// Run the test:
//   Build "oxide-sloc-chain-upstream" (optionally with SCAN_PATH). It triggers
//   oxide-sloc, waits for it, and oxide-sloc in turn triggers the downstream
//   consumer on success. See ci/jenkins/demo/README.md for expected output.
//
// Override the oxide-sloc job name (if you named it differently) by setting
// TARGET_JOB on the seed, or edit the default below.

def targetJob = (binding.hasVariable('TARGET_JOB') ? TARGET_JOB : System.getenv('TARGET_JOB')) ?: 'oxide-sloc'
def downstreamName = 'oxide-sloc-chain-downstream'
def upstreamName   = 'oxide-sloc-chain-upstream'

// ── Downstream consumer ──────────────────────────────────────────────────────
// Receives the parameters the oxide-sloc pipeline passes to DOWNSTREAM_JOB and
// proves the hand-off: prints them, sets a build description, and (if the Copy
// Artifact plugin is installed) copies the upstream build's archived artifacts.
pipelineJob(downstreamName) {
    description('Downstream consumer for the oxide-sloc chaining demo. ' +
               'Triggered automatically by the oxide-sloc pipeline via DOWNSTREAM_JOB.')
    logRotator { numToKeep(20) }
    // Declare the parameters at the JOB level so they exist on build #1 — the
    // upstream trigger passes them immediately, before any manual run.
    parameters {
        stringParam('CHAIN_UPSTREAM_JOB',   '', 'Name of the job that triggered this build')
        stringParam('CHAIN_UPSTREAM_BUILD', '', 'Build number of the upstream job')
        stringParam('ARTIFACT_PATH',  '', 'Artifact path passed from the upstream job')
    }
    definition {
        cps {
            sandbox(true)
            script('''
pipeline {
    agent any
    options { timestamps() }
    stages {
        stage('Receive chain hand-off') {
            steps {
                echo '=== oxide-sloc downstream consumer ==='
                echo "Triggered by upstream job : ${params.CHAIN_UPSTREAM_JOB}"
                echo "Upstream build number      : #${params.CHAIN_UPSTREAM_BUILD}"
                echo "Artifact path from upstream: ${params.ARTIFACT_PATH}"
                script {
                    if (params.CHAIN_UPSTREAM_JOB?.trim() && params.CHAIN_UPSTREAM_BUILD?.trim()) {
                        try {
                            copyArtifacts projectName: params.CHAIN_UPSTREAM_JOB,
                                          selector: specific(params.CHAIN_UPSTREAM_BUILD),
                                          optional: true,
                                          fingerprintArtifacts: true,
                                          target: 'from-upstream'
                            echo 'Copied upstream artifacts into ./from-upstream (Copy Artifact plugin present).'
                        } catch (Throwable t) {
                            echo "copyArtifacts skipped (install the 'Copy Artifact' plugin to enable): ${t.message}"
                        }
                        currentBuild.description = "downstream of ${params.CHAIN_UPSTREAM_JOB} #${params.CHAIN_UPSTREAM_BUILD}"
                    } else {
                        echo 'No upstream context set — run this through oxide-sloc-chain-upstream, not directly.'
                        currentBuild.description = 'run directly (no upstream context)'
                    }
                }
                echo '=== downstream consumer complete — chain verified ==='
            }
        }
    }
}
'''.stripIndent())
        }
    }
}

// ── Upstream orchestrator ────────────────────────────────────────────────────
// Triggers the real oxide-sloc pipeline with DOWNSTREAM_JOB pointed at the
// consumer above, waits for it (propagate: true adopts its result), and passes
// its own identity as UPSTREAM_JOB/UPSTREAM_BUILD so the *upstream* half of the
// chain is exercised too. SKIP_* flags keep the demo fast (scan + report only).
pipelineJob(upstreamName) {
    description('Upstream orchestrator for the oxide-sloc chaining demo. ' +
               'Run this to trigger oxide-sloc, which then triggers the downstream consumer.')
    logRotator { numToKeep(20) }
    parameters {
        stringParam('TARGET_JOB',     targetJob,          'The oxide-sloc pipeline job to trigger')
        stringParam('DOWNSTREAM_JOB', downstreamName,     'Job that oxide-sloc triggers on success')
        stringParam('SCAN_PATH',      'testing/fixtures/basic', 'Path oxide-sloc scans for this demo')
    }
    definition {
        cps {
            sandbox(true)
            script('''
pipeline {
    agent any
    options { timestamps() }
    stages {
        stage('Trigger oxide-sloc (downstream)') {
            steps {
                script {
                    echo "Triggering ${params.TARGET_JOB} with DOWNSTREAM_JOB=${params.DOWNSTREAM_JOB}"
                    def run = build job: params.TARGET_JOB,
                        parameters: [
                            string(name: 'SCAN_PATH',          value: params.SCAN_PATH),
                            string(name: 'CHAIN_DOWNSTREAM_JOB',     value: params.DOWNSTREAM_JOB),
                            booleanParam(name: 'SKIP_QUALITY_GATES', value: true),
                            booleanParam(name: 'SKIP_WEB_CHECK',     value: true),
                            string(name: 'CHAIN_UPSTREAM_JOB',       value: env.JOB_NAME),
                            string(name: 'CHAIN_UPSTREAM_BUILD',     value: env.BUILD_NUMBER)
                        ],
                        wait: true,
                        propagate: true
                    echo "${params.TARGET_JOB} #${run.number} finished with result ${run.result}"
                    echo "On success it fire-and-forget triggers ${params.DOWNSTREAM_JOB} — check that job's builds."
                    currentBuild.description = "ran ${params.TARGET_JOB} #${run.number} -> ${run.result}"
                }
            }
        }
    }
}
'''.stripIndent())
        }
    }
}

println "Created chaining demo jobs: '${upstreamName}' -> '${targetJob}' -> '${downstreamName}'"
