// seed-discovery.groovy -- Job DSL seed that turns a normalized inventory
// (produced by discover_bitbucket.py, or transferred in for an air-gap) into
// folder-bounded managed oxide-sloc trigger jobs.
//
// Invoked by the DISCOVER mode of Jenkinsfile.discovery as a "Process Job DSLs"
// build step.  It is deliberately boring and safe:
//
//   * It ONLY creates/updates jobs under MANAGED_FOLDER.  Configure this seed
//     step's "Action for removed jobs" = Disable (NOT Delete) so repositories
//     that leave the inventory are disabled, not destroyed.  This script also
//     renders enabled==false entries as disabled jobs defensively.
//   * It fails CLOSED on MAX_REPOS / MAX_BRANCHES_PER_REPO so a huge project
//     cannot silently flood the controller.
//   * DRY_RUN prints the planned folder/job set and creates nothing.
//   * No secret is read or rendered here; entries carry only a credential-id.

import groovy.json.JsonSlurper

// --- Configuration (env-driven; safe defaults) ------------------------------
String inventoryPath   = System.getenv('INVENTORY_PATH')   ?: 'inventory.json'
String managedFolder   = System.getenv('MANAGED_FOLDER')   ?: 'oxide-sloc-scans'
String centralJob      = System.getenv('CENTRAL_JOB')      ?: 'oxide-sloc'
String nightlyCron     = System.getenv('NIGHTLY_CRON')     ?: 'H H(1-4) * * *'
String policyRevision  = System.getenv('POLICY_REVISION')  ?: 'unversioned'
int    jobTimeoutMins  = (System.getenv('JOB_TIMEOUT_MINUTES') ?: '60').toInteger()
int    maxRepos        = (System.getenv('MAX_REPOS')            ?: '500').toInteger()
int    maxBranchesRepo = (System.getenv('MAX_BRANCHES_PER_REPO')?: '25').toInteger()
boolean dryRun         = (System.getenv('DRY_RUN') ?: 'false').toBoolean()

Map cfg = [
    managedFolder   : managedFolder,
    centralJob      : centralJob,
    nightlyCron     : nightlyCron,
    policyRevision  : policyRevision,
    jobTimeoutMinutes: jobTimeoutMins,
]

// --- Load + validate the inventory ------------------------------------------
def entries = new JsonSlurper().parse(new File(inventoryPath))
if (!(entries instanceof List)) {
    throw new IllegalStateException("inventory must be a JSON array: " + inventoryPath)
}

// Enforce bounds BEFORE creating anything (fail closed, actionable message).
def byRepo = entries.groupBy { it.repo_id }
if (byRepo.size() > maxRepos) {
    throw new IllegalStateException(
        "MAX_REPOS exceeded: ${byRepo.size()} repos > ${maxRepos}. " +
        "Narrow discovery filters or raise MAX_REPOS deliberately.")
}
byRepo.each { repoId, repoEntries ->
    if (repoEntries.size() > maxBranchesRepo) {
        throw new IllegalStateException(
            "MAX_BRANCHES_PER_REPO exceeded for repo ${repoId}: " +
            "${repoEntries.size()} > ${maxBranchesRepo}.")
    }
}

// --- Ensure the managed folder tree exists ----------------------------------
// Create each parent folder segment of every job (idempotent).
def folderPaths = [] as Set
entries.each { e ->
    def parts = (e.job_name as String).split('/') as List
    for (int i = 1; i < parts.size(); i++) {
        folderPaths << parts[0..<i].join('/')
    }
}
if (!dryRun) {
    folderPaths.sort().each { path -> folder(path) }
}

// --- Render jobs via the shared template ------------------------------------
def template = evaluate(readFileFromWorkspace('ci/jenkins/discovery/job-template.groovy'))

int created = 0
int disabledCount = 0
entries.each { entry ->
    if (!(entry.job_name as String).startsWith(managedFolder + '/')) {
        // Never touch anything outside the owned folder.
        throw new IllegalStateException("entry job_name escapes managed folder: " + entry.job_name)
    }
    if (dryRun) {
        println "[DRY_RUN] would manage ${entry.job_name} (enabled=${entry.enabled != false})"
        return
    }
    template.renderManagedScanJob(this, entry, cfg)
    if (entry.enabled == false) { disabledCount++ } else { created++ }
}

println "oxide-sloc discovery seed: ${created} active, ${disabledCount} disabled, " +
        "${byRepo.size()} repos, folder=${managedFolder}, dryRun=${dryRun}"
