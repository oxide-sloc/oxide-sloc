// Example: trigger an oxide-sloc scan from a modern Declarative Jenkins pipeline.
//
// The `post { success { ... } }` block is the success gate — it only runs when
// the whole pipeline succeeded. Store the shared HMAC secret as a Jenkins
// "Secret text" credential (id: oxide-sloc-secret) and the server URL wherever
// you keep such config.
//
// NOTE: for Jenkins triggering the oxide-sloc project ITSELF as a downstream
// Jenkins job (not a server), use the existing CHAIN_DOWNSTREAM_JOB parameter on
// the bundled Jenkinsfile instead — see docs/ci-integrations.md. This example is
// the server-webhook path, which also works cross-tool.
pipeline {
  agent any
  environment {
    OXIDE_SLOC_URL = 'https://sloc.internal.example.com'
  }
  stages {
    stage('Build') {
      steps { echo '…your real build…' }
    }
  }
  post {
    success {
      withCredentials([string(credentialsId: 'oxide-sloc-secret', variable: 'OXIDE_SLOC_SECRET')]) {
        // On a Windows agent, call trigger-oxide-sloc.ps1 via `powershell` instead.
        sh '''
          ci/downstream-trigger/trigger-oxide-sloc.sh \
            --repo "${GIT_URL}" \
            --branch "${BRANCH_NAME:-${GIT_BRANCH#origin/}}" \
            --commit "${GIT_COMMIT}" \
            --status success \
            --system jenkins \
            --job "${JOB_NAME}" \
            --build-id "${BUILD_NUMBER}" \
            --build-url "${BUILD_URL}"
        '''
      }
    }
  }
}
