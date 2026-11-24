// job-template.groovy -- renders ONE managed oxide-sloc trigger job.
//
// This file is loaded by seed-discovery.groovy and exposes a single function,
// renderManagedScanJob(dsl, entry, cfg), used once per normalized inventory
// entry.  It intentionally contains no Bitbucket, scanner, or secret logic:
//
//   * A generated job NEVER obtains or runs the scanner binary itself.  It only
//     invokes the central `oxide-sloc` job (cfg.centralJob) -- the single place
//     that acquires/runs the tool and owns scan-option policy.
//   * A generated job stores ONLY non-secret metadata: the clone URL, the
//     Jenkins credential-id reference, the branch, and the policy revision.
//     No token, password, key, or TLS-bypass flag is ever rendered.
//   * Jobs are created ONLY under cfg.managedFolder; entry.job_name is already
//     folder-scoped and sanitized by inventory.py.
//   * Ineligible entries (enabled == false) are DISABLED with an explanatory
//     description, never deleted, so build history survives.

// Build the CPS pipeline script for a managed trigger job.  For a nightly run
// it passes the configured branch name as SCAN_REF; a DISPATCH event overrides
// SCAN_REF with the immutable pushed commit SHA (see dispatch_event.py).
static String pipelineScript(Map entry, Map cfg) {
    String cloneUrl = entry.clone_url
    String branch = entry.branch
    String credId = entry.credential_id ?: ''
    String central = cfg.centralJob
    String policyRev = cfg.policyRevision ?: 'unversioned'
    return """
pipeline {
  agent any
  options {
    disableConcurrentBuilds()
    timeout(time: ${cfg.jobTimeoutMinutes ?: 60}, unit: 'MINUTES')
  }
  parameters {
    // SCAN_REF defaults to the managed branch for nightly/manual runs; a push
    // dispatch supplies the exact commit SHA instead of this moving name.
    string(name: 'SCAN_REF', defaultValue: '${branch}', description: 'Branch or immutable commit SHA to scan')
  }
  triggers {
    cron('${cfg.nightlyCron}')
  }
  stages {
    stage('Invoke central oxide-sloc scan') {
      steps {
        script {
          def run = build job: '${central}',
            wait: true,
            propagate: true,
            parameters: [
              string(name: 'SCAN_REPO_URL', value: '${cloneUrl}'),
              string(name: 'SCAN_REF', value: params.SCAN_REF),
              string(name: 'SCAN_CREDENTIALS_ID', value: '${credId}'),
              string(name: 'REPORT_TITLE', value: "${entry.project}/${entry.repo_slug}@\${params.SCAN_REF}")
            ]
          currentBuild.description =
            "policy ${policyRev} -> <a href=\\"\${run.absoluteUrl}\\">central #\${run.number}</a>"
        }
      }
    }
  }
}
""".stripIndent()
}

// Render (create or update) the managed pipeline job for one inventory entry.
// Idempotent: re-running with the same entry yields the same job definition.
def renderManagedScanJob(dsl, Map entry, Map cfg) {
    String fullName = entry.job_name
    // Hard guard: refuse to render anything outside the managed folder.
    if (!fullName.startsWith(cfg.managedFolder + '/')) {
        throw new IllegalStateException("refusing to manage job outside folder: " + fullName)
    }
    boolean enabled = (entry.enabled != false)
    dsl.pipelineJob(fullName) {
        description(
            enabled
                ? "Managed by oxide-sloc discovery (policy ${cfg.policyRevision ?: 'unversioned'}). " +
                  "Repo ${entry.host}/${entry.project}/${entry.repo_slug}, branch ${entry.branch}."
                : "DISABLED by oxide-sloc discovery: inventory entry no longer eligible " +
                  "(archived/removed/out-of-policy). Preserved for build history; not deleted."
        )
        disabled(!enabled)
        if (enabled) {
            logRotator { numToKeep(30) }
        }
        definition {
            cps {
                sandbox(true)
                script(pipelineScript(entry, cfg))
            }
        }
    }
}

return this
