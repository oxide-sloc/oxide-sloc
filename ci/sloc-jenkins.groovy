// oxide-sloc Jenkins shared library step
//
// Place this file at vars/slocAnalyze.groovy in your Jenkins shared library,
// then call it from any Jenkinsfile after 'cargo install oxide-sloc' is on PATH.
//
// Usage:
//   @Library('your-shared-lib') _
//   pipeline {
//     agent any
//     stages {
//       stage('SLOC') {
//         steps {
//           slocAnalyze(
//             label: env.JOB_NAME,
//             // optional — push artifacts to Artifactory:
//             artifactRepoType: 'artifactory',
//             artifactRepoUrl:  'https://repo.example.com/artifactory/sloc-reports',
//             artifactRepoPath: "oxide-sloc/${env.JOB_NAME}/${env.BUILD_NUMBER}",
//           )
//         }
//       }
//     }
//   }
//
// Required:
//   SLOC_SERVER env var or `server` argument (base URL of oxide-sloc instance)
//
// Optional:
//   SLOC_API_KEY          env var or `apiKey` arg — bearer token for oxide-sloc server auth
//   artifactRepoType      repo backend: artifactory | nexus | nexus2 | s3 | minio | azure-blob
//                                        | gcs | gitlab-packages | github-packages | generic-http
//   artifactRepoUrl       base URL of the artifact repository
//   artifactRepoPath      path prefix (default: oxide-sloc/<job>/<build>)
//   artifactRepoExtra     provider-specific extra config (see ci/artifact-push.sh)
//   artifactRepoUser      username / access key ID (or set SLOC_ARTIFACT_REPO_USER env var)
//   artifactRepoPass      password / API token     (or set SLOC_ARTIFACT_REPO_PASS env var)
//   artifactPushHtml      true/false — also push report.html (requires generateHtml: true)
//   artifactPushPdf       true/false — also push report.pdf  (requires generatePdf: true)
//   generateHtml          true/false — produce an HTML report alongside JSON (default: false)
//   generatePdf           true/false — produce a PDF report  (default: false)
//
// How it works:
//   1. oxide-sloc analyze runs on the Jenkins agent — no network call to the server.
//   2. oxide-sloc send POSTs the result JSON to /api/ingest on your server.
//   3. (optional) ci/artifact-push.sh pushes JSON/HTML/PDF to the artifact repository.
//   4. The server renders HTML, stores the artifact, and the scan appears in /view-reports.

def call(Map args = [:]) {
    def server = args.server ?: env.SLOC_SERVER
    if (!server) {
        error('slocAnalyze: set the SLOC_SERVER environment variable or pass server: "http://..." as an argument')
    }

    def label    = args.label  ?: (env.JOB_NAME ?: 'jenkins-build')
    def apiKey   = args.apiKey ?: (env.SLOC_API_KEY ?: '')
    def scanPath = args.path   ?: '.'
    def jsonOut  = args.output ?: 'sloc-result.json'

    // ── HTML / PDF output ─────────────────────────────────────────────────────
    def generateHtml = args.containsKey('generateHtml') ? args.generateHtml : false
    def generatePdf  = args.containsKey('generatePdf')  ? args.generatePdf  : false
    def htmlOut = generateHtml ? jsonOut.replace('.json', '.html') : ''
    def pdfOut  = generatePdf  ? jsonOut.replace('.json', '.pdf')  : ''
    def htmlArg = generateHtml ? "--html-out '${htmlOut}'" : ''
    def pdfArg  = generatePdf  ? "--pdf-out  '${pdfOut}'"  : ''

    // ── Artifact repository ───────────────────────────────────────────────────
    def repoType  = args.artifactRepoType  ?: (env.ARTIFACT_REPO_TYPE  ?: '')
    def repoUrl   = args.artifactRepoUrl   ?: (env.ARTIFACT_REPO_URL   ?: '')
    def repoPath  = args.artifactRepoPath  ?: (env.ARTIFACT_REPO_PATH  ?:
                        "oxide-sloc/${env.JOB_NAME ?: 'build'}/${env.BUILD_NUMBER ?: '0'}")
    def repoExtra = args.artifactRepoExtra ?: (env.ARTIFACT_REPO_EXTRA ?: '')
    def repoUser  = args.artifactRepoUser  ?: (env.SLOC_ARTIFACT_REPO_USER ?: '')
    def repoPass  = args.artifactRepoPass  ?: (env.SLOC_ARTIFACT_REPO_PASS ?: '')
    def pushHtml  = args.containsKey('artifactPushHtml') ? args.artifactPushHtml : generateHtml
    def pushPdf   = args.containsKey('artifactPushPdf')  ? args.artifactPushPdf  : generatePdf
    def pushArtifacts = repoType && repoUrl && repoType != 'none'

    // ── Analyze ───────────────────────────────────────────────────────────────
    def encodedLabel = URLEncoder.encode(label as String, 'UTF-8')
    def tokenFlag = apiKey ? "--webhook-token '${apiKey}'" : ''

    sh """
        oxide-sloc analyze '${scanPath}' --json-out '${jsonOut}' ${htmlArg} ${pdfArg}
        oxide-sloc send '${jsonOut}' \\
            --webhook-url '${server}/api/ingest?label=${encodedLabel}' \\
            ${tokenFlag}
    """

    archiveArtifacts artifacts: jsonOut, allowEmptyArchive: true

    // ── Push to artifact repository ───────────────────────────────────────────
    if (pushArtifacts) {
        def validTypes = ['artifactory', 'nexus', 'nexus2', 's3', 'minio', 'azure-blob',
                          'gcs', 'gitlab-packages', 'github-packages', 'generic-http']
        if (!validTypes.contains(repoType)) {
            error("slocAnalyze: unknown artifactRepoType '${repoType}'. Valid: ${validTypes.join(', ')}")
        }

        def artifactFiles = ['result.json']
        if (pushHtml && generateHtml && htmlOut) artifactFiles << htmlOut.tokenize('/').last()
        if (pushPdf  && generatePdf  && pdfOut)  artifactFiles << pdfOut.tokenize('/').last()

        // Resolve artifact directory from the jsonOut path
        def artifactDir = jsonOut.contains('/')
            ? jsonOut.substring(0, jsonOut.lastIndexOf('/'))
            : env.WORKSPACE

        withEnv([
            "ARTIFACT_REPO_TYPE=${repoType}",
            "ARTIFACT_REPO_URL=${repoUrl}",
            "ARTIFACT_REPO_PATH=${repoPath}",
            "ARTIFACT_REPO_EXTRA=${repoExtra}",
            "ARTIFACT_DIR=${artifactDir}",
            "ARTIFACT_FILES=${artifactFiles.join(' ')}",
            "ARTIFACT_REPO_USER=${repoUser}",
            "ARTIFACT_REPO_PASS=${repoPass}",
        ]) {
            sh 'bash ci/artifact-push.sh'
        }
    }
}
