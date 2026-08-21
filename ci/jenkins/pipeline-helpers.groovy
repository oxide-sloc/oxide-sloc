// oxide-sloc Jenkins pipeline helpers
//
// Loaded by the canonical Jenkinsfile via:
//   h = load 'ci/jenkins/pipeline-helpers.groovy'
//
// Each function compiles as a separate class method, giving it its own
// 64 KB JVM bytecode budget and keeping the main WorkflowScript class
// well under the MethodTooLargeException threshold.
//
// Add new helper functions here rather than in the Jenkinsfile.

// ── Cross-platform shell dispatch ────────────────────────────────────────────
//
// The pipeline keeps its POSIX .sh scripts as the single source of truth and
// runs them unchanged on BOTH Linux and Windows agents. On Unix the `sh` step
// works natively. On Windows there is no /bin/sh, so we route the SAME command
// body through a Git-Bash `bash.exe` via a `bat` step. This is the only place
// that knows about the OS split — every other step calls shx()/shxStatus()/
// shxStdout() so the command bodies stay identical across platforms.
//
// The Windows path does NOT use `bash -lc "<inline>"` (fragile with %, ", `,
// $, and heredocs). Instead each command body is written to a temp .sh file in
// the workspace via writeFile (CRLF normalised to LF) and executed by path, then
// removed. See writeShxScript()/shx() below.
//
// resolveBash() and shx() are kept as separate methods so each compiles into its
// own class method and stays well under the JVM 64 KB per-method bytecode limit.

// Locate a POSIX bash on a Windows agent. Returns the absolute path, or null if
// none is found. On Unix returns null (the native `sh` step is used instead, so
// no bash discovery is needed). Discovery order:
//   1. SLOC_BASH env override (explicit operator escape hatch), if it exists.
//   2. SLOC_PORTABLE_GIT env — a *no-install* PortableGit folder root; we probe
//      <root>\\bin\\bash.exe and <root>\\usr\\bin\\bash.exe under it.
//   3. `where bash` on PATH, skipping the WSL System32 shim (that bash.exe is a
//      launcher for a Linux distro, not a usable POSIX shell for our scripts).
//   4. Known Git-for-Windows locations, INCLUDING no-admin ones: a PortableGit
//      folder staged in the workspace / common tools dirs, and the per-user
//      LocalAppData install (%LOCALAPPDATA%\Programs\Git — no admin required).
//
// The no-install path matters: installing Git for Windows system-wide needs
// admin and is a non-starter on locked-down agents. A PortableGit ZIP (or the
// per-user LocalAppData install) needs neither an installer nor admin — extract
// a folder, and this resolver finds bash.exe (and the bundled MinGW gcc/ld that
// initEnv() puts on PATH for the windows-gnu build) inside it.
def resolveBash() {
    if (isUnix()) { return null }

    // Cache the resolution for the duration of the build so we don't shell out
    // to `where` on every single step.
    if (env.SLOC_RESOLVED_BASH?.trim()) {
        return env.SLOC_RESOLVED_BASH.trim()
    }

    // Given a Git/PortableGit folder ROOT, the two bash.exe layouts under it.
    // Pure string construction (no pipeline steps) so it is CPS-safe to call.
    def bashesUnder = { String root ->
        if (!root?.trim()) { return [] }
        def r = root.trim().replaceAll('[\\\\/]+$', '')
        return ["${r}\\bin\\bash.exe", "${r}\\usr\\bin\\bash.exe"]
    }

    // 1. Explicit bash.exe override.
    def override = env.SLOC_BASH?.trim()
    if (override && fileExists(override)) {
        env.SLOC_RESOLVED_BASH = override
        return override
    }

    // 2. Explicit PortableGit folder root (no-install escape hatch).
    for (def p in bashesUnder(env.SLOC_PORTABLE_GIT?.trim())) {
        if (fileExists(p)) {
            env.SLOC_RESOLVED_BASH = p
            return p
        }
    }

    // 3. `where bash` — prefer a Git bash over the WSL System32 launcher.
    def hits = ''
    try {
        hits = bat(returnStdout: true, script: '@where bash 2>NUL').trim()
    } catch (Throwable t) {
        hits = ''
    }
    def gitHit = null
    def anyHit = null
    hits.readLines().each { line ->
        def p = line.trim()
        if (!p) { return }
        if (anyHit == null) { anyHit = p }
        def lower = p.toLowerCase()
        // Skip the WSL shim (lives under System32\bash.exe) — it is not a POSIX
        // shell we can run .sh scripts through.
        if (lower.contains('system32')) { return }
        if (gitHit == null) { gitHit = p }
    }
    def picked = gitHit ?: anyHit
    if (picked && fileExists(picked)) {
        env.SLOC_RESOLVED_BASH = picked
        return picked
    }

    // 3b. Derive bash from a `git` on PATH. A locked-down agent frequently has Git
    //     for Windows installed to a NON-standard folder (so the fixed roots below
    //     miss it) yet still exposes git on PATH for SCM. git.exe lives at
    //     <root>\cmd\git.exe (also \mingw64\bin\ or \bin\); strip that suffix to get
    //     <root> and probe bash.exe under it. This is the common cause of a bare
    //     "no POSIX shell" on an agent that clearly HAS Git.
    def gitHits = ''
    try {
        gitHits = bat(returnStdout: true, script: '@where git 2>NUL').trim()
    } catch (Throwable t) {
        gitHits = ''
    }
    for (def line in gitHits.readLines()) {
        def g = line.trim()
        if (!g) { continue }
        def lower = g.toLowerCase()
        // The WSL/System32 shim is a git launcher, not a Git-for-Windows root.
        if (lower.contains('\\system32\\')) { continue }
        def root = null
        for (def marker in ['\\cmd\\git.exe', '\\mingw64\\bin\\git.exe',
                            '\\mingw32\\bin\\git.exe', '\\bin\\git.exe']) {
            def idx = lower.lastIndexOf(marker)
            if (idx >= 0) { root = g.substring(0, idx); break }
        }
        if (!root) { continue }
        for (def b in bashesUnder(root)) {
            if (fileExists(b)) {
                env.SLOC_RESOLVED_BASH = b
                return b
            }
        }
    }

    // 4. Known Git-for-Windows locations. System-wide installs first, then the
    //    no-admin fallbacks: a staged PortableGit folder (workspace/.tools or a
    //    common tools dir) and the per-user LocalAppData install.
    def ws  = env.WORKSPACE?.trim()
    def up  = env.USERPROFILE?.trim()
    def lad = env.LOCALAPPDATA?.trim()
    def roots = []
    roots << 'C:\\Program Files\\Git'
    roots << 'C:\\Program Files (x86)\\Git'
    if (lad) { roots << "${lad}\\Programs\\Git" }        // per-user, no admin
    if (ws)  { roots << "${ws}\\.tools\\PortableGit" }   // staged in workspace
    if (up)  { roots << "${up}\\PortableGit" }
    roots << 'C:\\Tools\\PortableGit'
    roots << 'C:\\PortableGit'
    def candidates = []
    for (def r in roots) { candidates.addAll(bashesUnder(r)) }
    for (def c in candidates) {
        if (fileExists(c)) {
            env.SLOC_RESOLVED_BASH = c
            return c
        }
    }

    // 5. Last resort: auto-stage a no-install PortableGit if an operator has merely
    //    dropped a PortableGit-*.7z.exe / .zip in the workspace or a common tools dir.
    //    This lets a truly bash-free, air-gapped Windows agent bootstrap itself with
    //    NO admin, NO installer, and NO network. Tried at most once per build.
    def staged = stagePortableGitFallback()
    if (staged && fileExists(staged)) {
        env.SLOC_RESOLVED_BASH = staged
        return staged
    }
    return null
}

// Auto-stage a PortableGit archive discovered on the agent, so a truly bash-free
// Windows box can bootstrap itself when an operator has dropped a
// PortableGit-*.7z.exe / .zip in the workspace (or set SLOC_PORTABLE_GIT_ARCHIVE).
// Runs the native, bash-free stage-portable-git.ps1 in -AutoDiscover mode and
// returns the path to the staged bash.exe, or null. Kept as its own method so it
// gets its own JVM bytecode budget, and guarded to run at most once per build.
def stagePortableGitFallback() {
    if (isUnix()) { return null }
    if (env.SLOC_BASH_STAGE_TRIED == '1') { return null }
    env.SLOC_BASH_STAGE_TRIED = '1'
    def ws = env.WORKSPACE?.trim()
    if (!ws) { return null }
    def dest = "${ws}\\.tools\\PortableGit\\bin\\bash.exe"
    try {
        def rc = bat(returnStatus: true,
            script: 'powershell -NoProfile -ExecutionPolicy Bypass ' +
                    '-File ci\\jenkins\\stage-portable-git.ps1 -AutoDiscover')
        if (rc == 0 && fileExists(dest)) {
            echo "Auto-staged PortableGit at ${dest} (no admin, no installer, no network)."
            return dest
        }
    } catch (Throwable t) {
        echo "PortableGit auto-stage skipped: ${t.message}"
    }
    return null
}

// The actionable error shown on a Windows agent with no usable POSIX shell.
def noBashError() {
    error('No POSIX shell found on this Windows agent, and no PortableGit archive ' +
          'was available to auto-stage. No admin/system install is required — provide ' +
          'a portable Git Bash once (no installer, no admin) by ANY of: (1) drop a ' +
          'PortableGit-*.7z.exe (or .zip) into the job workspace — or set ' +
          'SLOC_PORTABLE_GIT_ARCHIVE to its path — and re-run; the pipeline stages it ' +
          'automatically. (2) Run `powershell -File ci/jenkins/stage-portable-git.ps1 ' +
          '<PortableGit-*.7z.exe>` yourself to extract it into ' +
          '<workspace>\\.tools\\PortableGit. (3) Point SLOC_PORTABLE_GIT at an already-' +
          'extracted PortableGit folder, or SLOC_BASH at a bash.exe. A per-user Git ' +
          'install under %LOCALAPPDATA%\\Programs\\Git, and any `git` on PATH, are also ' +
          'auto-detected. Alternatively pin this job to a Linux agent via the ' +
          'AGENT_LABEL parameter.')
}

// Per-build monotonically-increasing counter used to name temp script files
// uniquely. There is no Math.random()/new Date() available under the Groovy
// sandbox, so uniqueness is derived from BUILD_NUMBER + this static counter.
@groovy.transform.Field int shxCounter = 0

// Write a command body to a unique temp script in the workspace and return the
// workspace-relative path. The body is written VERBATIM (no `set -e...` is
// prepended — many existing bodies already start with their own `set` line, and
// double-setting would change behaviour). CRLF is normalised to LF so Git Bash
// on Windows never chokes on a `\r` at the end of a `set -o pipefail` line.
def writeShxScript(String cmd) {
    shxCounter += 1
    def name = ".shx-${env.BUILD_NUMBER ?: '0'}-${shxCounter}.sh"
    // Strip CR so the script is pure-LF regardless of how the Groovy string was
    // authored or checked out (Windows Git may have introduced CRLF).
    def body = cmd.replace('\r\n', '\n').replace('\r', '\n')
    writeFile file: name, text: body
    return name
}

// Run a shell command. On Unix: native `sh` (unchanged). On Windows: the SAME
// body is written to a temp .sh file and executed by Git Bash via a `bat` step
// — NOT `bash -lc "<inline>"`, which is fragile with %, ", `, $, and heredocs.
// The temp file is removed in a finally so a workspace never accumulates them.
def shx(String cmd) {
    if (isUnix()) {
        sh cmd
        return
    }
    def bash = resolveBash()
    if (!bash) { noBashError() }
    def tmp = writeShxScript(cmd)
    try {
        bat "\"${bash}\" \"${tmp}\""
    } finally {
        bat "if exist \"${tmp}\" del /q \"${tmp}\""
    }
}

// Stdout-returning variant of shx (mirrors sh(returnStdout: true)). Returns the
// captured stdout as a String (NOT trimmed — callers .trim() as today). The
// leading `@` suppresses cmd.exe echoing the command line into the captured
// output, so callers get only the script's own stdout.
def shxStdout(String cmd) {
    if (isUnix()) {
        return sh(returnStdout: true, script: cmd)
    }
    def bash = resolveBash()
    if (!bash) { noBashError() }
    def tmp = writeShxScript(cmd)
    try {
        return bat(returnStdout: true, script: "@\"${bash}\" \"${tmp}\"")
    } finally {
        bat "@if exist \"${tmp}\" del /q \"${tmp}\""
    }
}

// Status-returning variant of shx (mirrors sh(returnStatus: true)). Returns the
// process exit code as an int.
def shxStatus(String cmd) {
    if (isUnix()) {
        return sh(returnStatus: true, script: cmd)
    }
    def bash = resolveBash()
    if (!bash) { noBashError() }
    def tmp = writeShxScript(cmd)
    try {
        return bat(returnStatus: true, script: "\"${bash}\" \"${tmp}\"")
    } finally {
        bat "@if exist \"${tmp}\" del /q \"${tmp}\""
    }
}

// ── OS-aware environment initialization ──────────────────────────────────────
//
// The declarative environment{} block cannot branch on isUnix(), so the
// OS-dependent variables (CARGO_HOME / RUSTUP_HOME / PATH / BINARY /
// ARTIFACT_PATH) are set here from an early script{} stage. Unix values are
// IDENTICAL to what the environment{} block used to set; Windows PREPENDS the
// cache bin dir to the inherited PATH (so System32 / Git / Python / cargo all
// survive) and points BINARY/ARTIFACT_PATH at the .exe.
def initEnv() {
    if (isUnix()) {
        def home = env.HOME
        env.CARGO_HOME  = "${home}/.rust-cache/cargo"
        env.RUSTUP_HOME = "${home}/.rust-cache/rustup"
        env.PATH        = "${home}/.rust-cache/cargo/bin:/usr/local/bin:/usr/bin:/bin"
        env.BINARY        = "${env.WORKSPACE}/target/release/oxide-sloc"
        env.ARTIFACT_PATH = "${env.WORKSPACE}/target/release/oxide-sloc"
        // POSIX-normalised mirrors of WORKSPACE / history-home for bash-facing path
        // construction. On Unix these are byte-identical to the native values, so
        // callers can uniformly use WS_POSIX / HISTORY_HOME_POSIX with no branching.
        env.WS_POSIX           = env.WORKSPACE
        env.HISTORY_HOME_POSIX = env.HOME
    } else {
        // Cache root resolution. A Windows Jenkins service account frequently runs
        // as a locked-down user whose HOME/USERPROFILE points at a read-only or
        // ACL-restricted profile (and the old 'C:\\Users\\Default' fallback is NEVER
        // writable). Resolve with an explicit operator override first, then the
        // usual profile vars, then WORKSPACE — which the agent guarantees is
        // writable — so the persistent Rust cache always lands somewhere usable:
        //   SLOC_CACHE_DIR (escape hatch) -> HOME -> USERPROFILE -> WORKSPACE
        def cacheRoot = (env.SLOC_CACHE_DIR?.trim()
                            ?: env.HOME?.trim()
                            ?: env.USERPROFILE?.trim()
                            ?: env.WORKSPACE)
        // Export CARGO_HOME/RUSTUP_HOME and the PATH-prepended cargo bin dir in
        // FORWARD-SLASH (POSIX) form, exactly as BINARY is normalised below. Git
        // Bash and cargo both accept 'C:/Users/...'; backslash values were mangled
        // by the coreutils (du/find) inside check-disk-space.sh and diverged from
        // install-rust-cache.sh's "$HOME/.rust-cache" layout. POSIX form fixes both.
        def cacheRootPosix = cacheRoot?.replace('\\', '/')
        env.CARGO_HOME  = "${cacheRootPosix}/.rust-cache/cargo"
        env.RUSTUP_HOME = "${cacheRootPosix}/.rust-cache/rustup"

        // MinGW gcc/ld for the x86_64-pc-windows-gnu target. shx() runs NON-login
        // bash (/etc/profile is not sourced), so /mingw64/bin is not injected and
        // the cargo build would fail linking with "linker 'cc' not found". Derive
        // the Git install root from the resolved bash.exe and PREPEND its
        // mingw64\bin + usr\bin to PATH (Windows ';'-separated) so gcc/ld resolve.
        //
        // Derive the root by LAYOUT, not by a '\Git\' name marker: bash.exe lives
        // at <root>\\bin\\bash.exe or <root>\\usr\\bin\\bash.exe under EVERY Git-for-
        // Windows layout — system, per-user (…\Programs\Git), and a PortableGit
        // folder (whose name is 'PortableGit', so a '\Git\' marker would miss it).
        // Guarded: only when a Windows bash path was actually resolved.
        def gccPrefix = ''
        def bashPath = resolveBash()
        if (bashPath) {
            def norm = bashPath.replace('/', '\\')
            def lower = norm.toLowerCase()
            def gitRoot = null
            if (lower.endsWith('\\usr\\bin\\bash.exe')) {
                gitRoot = norm.substring(0, norm.length() - '\\usr\\bin\\bash.exe'.length())
            } else if (lower.endsWith('\\bin\\bash.exe')) {
                gitRoot = norm.substring(0, norm.length() - '\\bin\\bash.exe'.length())
            } else {
                // Fallback: the legacy '\Git\' marker (system-install layouts).
                def marker = '\\Git\\'
                def idx = lower.lastIndexOf(marker.toLowerCase())
                if (idx >= 0) { gitRoot = norm.substring(0, idx + marker.length() - 1) }
            }
            if (gitRoot) {
                gccPrefix = "${gitRoot}\\mingw64\\bin;${gitRoot}\\usr\\bin;"
            }
        }
        // PREPEND the cache cargo bin dir (POSIX form) + the MinGW/usr dirs to the
        // INHERITED PATH — never replace it, or System32/Git/Python/cargo vanish.
        // Windows path separator is ';'.
        env.PATH = "${cacheRootPosix}/.rust-cache/cargo/bin;${gccPrefix}${env.PATH}"

        // BINARY/ARTIFACT_PATH are consumed INSIDE bash (shx / run-web-check.sh
        // reference "${BINARY}"). A POSIX-style forward-slash path WITH the .exe
        // suffix is what bash resolves correctly on Windows, so use that form.
        // WORKSPACE on Windows uses backslashes — convert to forward slashes.
        def wsPosix = env.WORKSPACE?.replace('\\', '/')
        env.BINARY        = "${wsPosix}/target/release/oxide-sloc.exe"
        env.ARTIFACT_PATH = "${wsPosix}/target/release/oxide-sloc.exe"
        // POSIX-normalised WORKSPACE + history-home for bash-facing path building.
        // Backslash WORKSPACE/HOME interpolated raw into bash bodies (mkdir -p, cp,
        // test -s, git -C, python) produced mixed 'C:\...workspace/ci-out' paths that
        // coreutils/git mangle. Callers that feed a path into shx use these instead.
        env.WS_POSIX           = wsPosix
        env.HISTORY_HOME_POSIX = cacheRootPosix
    }
    env.RUST_LOG = 'warn'
}

// ── Python interpreter resolution ────────────────────────────────────────────
//
// The pipeline shells out to several .py helpers (dashboard, trend CSV, build
// summary, Confluence notify). Corporate / air-gapped agents frequently have NO
// system Python, or one too old, so a bundled portable CPython 3.14 ships in the
// repo under python/ (see python/README.md). Resolution order (first that works):
//   1. SLOC_PY override           — explicit operator escape hatch.
//   2. Bundled portable Python    — extracted on demand by ci/jenkins/setup-python.sh
//      (offline, checksum-verified). This is the air-gap default: no system Python
//      needed at all.
//   3. system python3 → python → py — on hosts that already have a modern Python.
// Resolved once and cached on env for the rest of the build. Callers use
// "${h.pyBin()} script.py ...". Every .py call in the pipeline is best-effort
// (guarded / "|| true"), so if none of these resolve the build still succeeds —
// only the optional dashboard/trend extras are skipped.
def pyBin() {
    if (env.SLOC_RESOLVED_PY?.trim()) {
        return env.SLOC_RESOLVED_PY.trim()
    }
    // 1. Explicit operator escape hatch.
    def override = env.SLOC_PY?.trim()
    if (override) {
        env.SLOC_RESOLVED_PY = override
        return override
    }
    // 2. Bundled portable Python — extract (idempotent, offline) and capture the
    //    interpreter path from setup-python.sh's LAST stdout line. A non-zero exit
    //    (no bundle for this platform) is expected on unbundled arches; fall through.
    if (!env.SLOC_PY_BUNDLE_TRIED) {
        env.SLOC_PY_BUNDLE_TRIED = '1'
        try {
            def out = shxStdout('bash ci/jenkins/setup-python.sh')
            def interp = out?.readLines()?.findAll { it?.trim() }?.last()?.trim()
            if (interp && (interp.endsWith('python.exe') || interp.endsWith('/python3'))) {
                // Verify it actually runs before committing to it.
                def ok = shxStatus("'${interp}' --version >/dev/null 2>&1") == 0
                if (ok) {
                    echo "Using bundled portable Python: ${interp}"
                    env.SLOC_RESOLVED_PY = interp
                    return interp
                }
            }
        } catch (Throwable t) {
            echo "Bundled Python setup skipped (${t.message}); falling back to system Python."
        }
    }
    // 3. Probe python3 → python → py, printing the first that runs. Runs through
    // shx so Git Bash resolves it on Windows and /bin/sh on Linux.
    def picked = ''
    try {
        picked = shxStdout('''
            if command -v python3 >/dev/null 2>&1; then echo python3
            elif command -v python >/dev/null 2>&1; then echo python
            elif command -v py >/dev/null 2>&1; then echo py
            else echo python3
            fi
        ''').trim().readLines().findAll { it?.trim() }.last()?.trim()
    } catch (Throwable t) {
        picked = ''
    }
    def resolved = picked ?: 'python3'
    env.SLOC_RESOLVED_PY = resolved
    return resolved
}

def runSetup() {
    // Disk preflight FIRST: fail fast on a low-space volume and prune the
    // persistent Rust cache if it has grown past its cap, so a build never dies
    // mid-run with ENOSPC and the cache that survives cleanWs() stays bounded.
    shx 'bash ci/jenkins/check-disk-space.sh'

    shx 'bash ci/jenkins/setup-toolchain.sh'

    // Air-gap: pin RUSTUP_TOOLCHAIN on the pipeline env so EVERY downstream stage
    // (fmt, clippy, build, test, coverage) uses the exact bundled toolchain name
    // resolved by setup-toolchain.sh. rust-toolchain.toml pins the "1.97" channel,
    // but the committed bundle installs 1.97.0-<triple>; without this pin an
    // offline rustup tries to sync the "1.97" channel manifest from
    // static.rust-lang.org and aborts ("connection reset by peer"). setup-toolchain.sh
    // records the concrete name in .rust-toolchain-name; each shx() is a fresh
    // shell so its own export cannot carry — read it here and set it on env.
    try {
        def tcName = shxStdout('cat .rust-toolchain-name 2>/dev/null || true')
            .readLines().findAll { it?.trim() }.last()?.trim()
        if (tcName) {
            env.RUSTUP_TOOLCHAIN = tcName
            echo "Pinned RUSTUP_TOOLCHAIN=${tcName} for all downstream stages (air-gap)."
        }
    } catch (Throwable t) {
        echo "Could not pin RUSTUP_TOOLCHAIN from .rust-toolchain-name: ${t.message}"
    }

    shx 'bash ci/jenkins/setup-vendor.sh'

    // ── Artifact-viewer CSP (OPTIONAL — the build succeeds without it) ─────────
    //
    // Relaxing the CSP only affects the styling/interactivity of the INTERACTIVE
    // artifact viewer on very old controllers. It is genuinely optional:
    //   * extract-report-assets.py externalises the report's inline CSS/JS so the
    //     HTML report renders under Jenkins' DEFAULT CSP with no relaxation, and
    //   * modern Jenkins (2.387.x+) serves CSP report-only (non-blocking).
    // So this whole block is best-effort and NEVER fails the build. One in-pipeline
    // tier, then the permanent out-of-band fix, in order of preference:
    //   1. Script Console REST API     — needs the OPTIONAL 'jenkins-api-token'
    //   2. init.groovy.d/relax-csp.groovy — the permanent, credential-free,
    //      sandbox-proof fix (bash ci/jenkins/preflight.sh --install-csp)
    //
    // A direct in-pipeline System.setProperty is deliberately NOT attempted. A
    // Pipeline-from-SCM job is always Groovy-sandboxed (see job-config.xml), so the
    // static call is rejected AND the script-security plugin logs its own alarming
    // "Scripts not permitted to use staticMethod java.lang.System setProperty" line
    // at the point of rejection — which a surrounding try/catch canNOT suppress. That
    // noise would appear on 100% of runs for zero benefit. The sandbox-OFF niche
    // where a direct set would actually work is covered by the init.groovy.d tier.
    def RELAXED_CSP = "default-src 'self'; style-src 'self' 'unsafe-inline'; " +
                      "img-src 'self' data: blob:; script-src 'self' 'unsafe-inline'; " +
                      "font-src 'self' data:;"
    def cspSet = false

    // Tier 1 — Script Console REST API, using the OPTIONAL 'jenkins-api-token'
    // credential. A missing credential is the EXPECTED path, not an error: the
    // withCredentials binding throws "credentials entry ... not found" when the
    // token is absent, which we classify as the quiet path and route to the calm
    // summary below (never a raw exception dump). Any OTHER error still gets a
    // short note. This runs quietly under the sandbox — no static Jenkins API
    // calls are made from Groovy (those would be RejectedAccessException anyway).
    if (!cspSet) {
        try {
            withCredentials([string(credentialsId: 'jenkins-api-token',
                                    variable:      'JEN_API_TOK')]) {
                def base = (env.BUILD_URL ?: '').replaceAll('/job/.*', '').replaceAll('/+$', '')
                if (env.JEN_API_TOK?.trim() && base) {
                    withEnv(["SLOC_JENKINS_BASE=${base}",
                             "SLOC_CSP=${RELAXED_CSP}",
                             // Derive the API user from the same place runArchivePublish
                             // does — JENKINS_API_USER, defaulting to admin — rather than
                             // hardcoding 'admin'.
                             "SLOC_JENKINS_USER=${env.JENKINS_API_USER ?: 'admin'}"]) {
                        // Report success ONLY when the Script Console POST returns 200.
                        // Use the exact concat(...,":",...) xpath (the "::" form is
                        // rejected with HTTP 403) and split the crumb on the FIRST ':'
                        // only (crumb values never contain ':').
                        def cspStatus = shxStatus('''
                            CRUMB=$(curl -fsS -u "${SLOC_JENKINS_USER}:${JEN_API_TOK}" \
                                "${SLOC_JENKINS_BASE}/crumbIssuer/api/xml?xpath=concat(//crumbRequestField,%22:%22,//crumb)" \
                                2>/dev/null || echo "")
                            FIELD="${CRUMB%%:*}"
                            CRUMB_VAL="${CRUMB#*:}"
                            GROOVY="System.setProperty(\"hudson.model.DirectoryBrowserSupport.CSP\",\"${SLOC_CSP}\")"
                            # A modern LTS exempts API-token POSTs from CSRF, so proceed
                            # even when the crumb fetch returned nothing; only the HTTP
                            # code decides success.
                            if [ -n "${FIELD}" ] && [ -n "${CRUMB_VAL}" ]; then
                                CODE=$(curl -sS -o /dev/null -w '%{http_code}' \
                                    -u "${SLOC_JENKINS_USER}:${JEN_API_TOK}" \
                                    -H "${FIELD}: ${CRUMB_VAL}" \
                                    --data-urlencode "script=${GROOVY}" \
                                    "${SLOC_JENKINS_BASE}/scriptText" 2>/dev/null || echo 000)
                            else
                                CODE=$(curl -sS -o /dev/null -w '%{http_code}' \
                                    -u "${SLOC_JENKINS_USER}:${JEN_API_TOK}" \
                                    --data-urlencode "script=${GROOVY}" \
                                    "${SLOC_JENKINS_BASE}/scriptText" 2>/dev/null || echo 000)
                            fi
                            if [ "${CODE}" = "200" ]; then
                                echo "Artifact-viewer CSP relaxed (Script Console API)."
                                exit 0
                            fi
                            echo "Script Console CSP relaxation returned HTTP ${CODE} (not applied)."
                            exit 1
                        ''')
                        if (cspStatus == 0) { cspSet = true }
                    }
                }
            }
        } catch (Throwable ex) {
            // A "credentials entry ... not found" is the expected/quiet path —
            // stay silent and let the friendly summary below explain the
            // (optional) situation. Any OTHER error is worth a short note.
            def msg = ex.message ?: ''
            if (!(msg.contains('not found') || msg.contains('Could not find') ||
                  msg.contains('CredentialNotFoundException'))) {
                echo "CSP auto-relax via API skipped: ${msg}"
            }
        }
    }

    // Single, calm INFO summary when the in-pipeline API tier did not apply the CSP.
    // This is expected on a sandboxed Pipeline-from-SCM job with no api token —
    // it is optional and non-fatal, so say so plainly (no exception dump).
    if (!cspSet) {
        echo 'CSP auto-relax skipped (Groovy sandbox active and no ' +
             "'jenkins-api-token' credential). This is OPTIONAL and non-fatal — " +
             'HTML reports still render (inline CSS/JS is externalised by ' +
             'extract-report-assets.py, and modern Jenkins serves CSP report-only). ' +
             'For interactive artifact-viewer styling on older Jenkins, install ' +
             'ci/jenkins/init.groovy.d/relax-csp.groovy (permanent, no credentials, ' +
             'sandbox-proof) or run: bash ci/jenkins/preflight.sh --install-csp.'
    }
}

// Obtain the oxide-sloc scanner binary for this build. Two modes, decided in the
// Jenkinsfile "Load helpers" stage and carried on env.SLOC_NEEDS_SOURCE:
//   PREBUILT (default, fast) — extract the committed dist/ binary into
//     target/release/. No Rust toolchain, no vendor archive, no compile, no
//     network. This is what makes a standard scan finish in a couple of minutes.
//   SOURCE — compile from the vendored crates with the air-gapped toolchain. Used
//     for release verification and whenever RUN_QUALITY_GATES / RUN_COVERAGE is on
//     (those need the toolchain to lint / instrument the workspace).
def runBuild() {
    if (env.SLOC_NEEDS_SOURCE == 'true') {
        // unset CC/CXX inside the build shell: on Windows a stray gcc from another
        // toolkit can leak in via CC and conflict with the MinGW-w64 gcc that the
        // x86_64-pc-windows-gnu target links with (documented "unset CC on Windows"
        // rule). initEnv() has already prepended <gitroot>\mingw64\bin to PATH so
        // cc/gcc/ld resolve. A best-effort `gcc --version` (never fatal) makes a
        // missing linker diagnosable in the build log.
        shx '''
            unset CC CXX
            gcc --version 2>/dev/null | head -1 || echo "gcc not on PATH (link may fail on the windows-gnu target)"
            cargo build --release -p oxide-sloc
        '''
    } else {
        // Fast path: extract the prebuilt binary from dist/ into target/release/.
        // use-prebuilt-binary.sh mirrors scripts/internal/install.sh dist extraction
        // (prefers the .tar.gz for locked-down hosts) and verifies the binary runs.
        shx 'bash ci/jenkins/use-prebuilt-binary.sh'
        // Confirm BINARY (set by initEnv, .exe on Windows) is now present.
        shx 'test -x "${BINARY}" || test -s "${BINARY}"'
        shx '"${BINARY}" --version'
    }
}

def runUnitTests() {
    // POSIX-form workspace for all bash-facing paths (see initEnv/WS_POSIX). On Unix
    // WS_POSIX == WORKSPACE so behaviour is unchanged; on Windows it swaps the
    // backslash workspace for forward slashes so coreutils in the shx body are happy.
    def outDir     = "${env.WS_POSIX}/${params.OUTPUT_SUBDIR}"
    def resultsDir = "${outDir}/test-results"
    shx "mkdir -p '${resultsDir}'"

    def useNextest = false
    if (params.TEST_RUNNER == 'cargo-nextest') {
        def alreadyInstalled = shxStatus(
            'cargo nextest --version >/dev/null 2>&1'
        ) == 0
        if (!alreadyInstalled) {
            echo 'cargo-nextest not found — attempting offline install from vendor...'
            // Redirect to a log rather than piping to tail: Jenkins `sh` is dash,
            // which has no `pipefail`, so a pipeline's exit status is tail's
            // (always 0) and `installed` would be unconditionally true — making
            // the unstable() branch below dead code.
            //
            // `cargo install cargo-nextest --offline` alone can never resolve from
            // vendor/: cargo install runs in a temp dir and does NOT pick up the
            // workspace .cargo/config.toml source replacement, so it looks for the
            // crate in registry 'crates-io' and fails. Instead copy the vendored
            // crate out of vendor/ and install it by --path, which resolves its
            // dependency tree against the workspace vendored-sources (cwd here is
            // the workspace, where setup-vendor.sh wrote .cargo/config.toml). This
            // mirrors the verified-working GitLab CI path. Do NOT add --locked to
            // the --path install: nextest's bundled lock wants chrono 0.4.44 while
            // the vendor set carries 0.4.45 by design (see ci/tools/Cargo.toml).
            // The `--locked` online fallback is a no-op offline (it just fails and
            // the unstable() branch fires) but lets a networked agent recover.
            def installed = shxStatus('''
                    {
                        if [ -d vendor/cargo-nextest ]; then
                            rm -rf .ci-tools && mkdir -p .ci-tools
                            cp -r vendor/cargo-nextest .ci-tools/
                            # .ci-tools/ lives INSIDE the oxide-sloc checkout, so cargo
                            # would auto-adopt the copied crate into the parent workspace
                            # (whose members do not list it) and abort with "current
                            # package believes it's in a workspace when it's not" BEFORE
                            # resolving anything. Append an empty [workspace] table so the
                            # copied crate is its own workspace root and installs from vendor/.
                            printf '\n[workspace]\n' >> .ci-tools/cargo-nextest/Cargo.toml
                            cargo install --offline --path .ci-tools/cargo-nextest \
                                || cargo install --locked cargo-nextest
                        else
                            cargo install --locked cargo-nextest
                        fi
                    } > .nextest-install.log 2>&1
                ''') == 0
            shx 'tail -5 .nextest-install.log 2>/dev/null || true'
            if (installed) {
                echo 'cargo-nextest installed from vendor successfully.'
                useNextest = true
            } else {
                // TEST_RUNNER explicitly requested cargo-nextest but it is not on
                // PATH and the offline install failed. Do NOT degrade silently to a
                // green build with no JUnit report — mark the build UNSTABLE so the
                // regression is visible (yellow, notified), while still running the
                // cargo test fallback below so the suite is not skipped entirely.
                unstable('cargo-nextest was requested (TEST_RUNNER=cargo-nextest) but is unavailable and the offline install failed — falling back to cargo test with NO JUnit report. Bake cargo-nextest into the agent image to restore the Tests report.')
                echo '  JUnit XML output will not be produced this run.'
                echo '  To enable: cargo install --locked cargo-nextest  on the agent (then re-run install-rust-cache.sh).'
                echo '  Falling back to cargo test (build marked UNSTABLE).'
            }
        } else {
            useNextest = true
        }
    }

    if (useNextest) {
        def failFastFlag = params.TEST_FAIL_FAST ? '--fail-fast' : '--no-fail-fast'
        // Capture nextest's status instead of letting `| tee` swallow it (a bash
        // shebang + pipefail is required — dash rejects `set -o pipefail`). The
        // status is acted on AFTER the junit move/publish below, so a red or
        // aborted run still gets its report collected.
        def testStatus = shxStatus("""#!/bin/bash
            set -o pipefail
            cargo nextest run --workspace ${failFastFlag} --profile ci \
                2>&1 | tee '${resultsDir}/nextest-output.txt'
        """)
        // nextest writes JUnit XML into the profile store dir
        // (target/nextest/ci/junit.xml), NOT the workspace root. Move from there;
        // fall back to a search in case a custom CARGO_TARGET_DIR relocates it.
        shx """
            if [ -f target/nextest/ci/junit.xml ]; then
                mv -f target/nextest/ci/junit.xml '${resultsDir}/junit.xml'
            else
                found=\$(find . -path '*/nextest/ci/junit.xml' -print -quit 2>/dev/null)
                [ -n "\$found" ] && mv -f "\$found" '${resultsDir}/junit.xml' || true
            fi
        """
        if (params.PUBLISH_TEST_RESULTS) {
            // Guarded so a controller WITHOUT the JUnit plugin still passes — the
            // junit.xml is archived regardless, so nothing is lost but the sidebar
            // "Test Result" view. Throwable (not Exception): a missing step throws
            // NoSuchMethodError, which is an Error.
            try {
                junit testResults:          "${params.OUTPUT_SUBDIR}/test-results/junit.xml",
                      allowEmptyResults:    true,
                      skipPublishingChecks: false
            } catch (Throwable t) {
                echo "junit publish skipped (JUnit plugin not installed): ${t.message}"
            }
        }
        // Decide only after the report is collected. junit() flags a red suite on
        // its own, but a nextest run that dies WITHOUT writing junit.xml (compile
        // error, missing test binary, OOM) leaves nothing to parse — this keeps
        // that case off green.
        if (testStatus != 0) {
            unstable("cargo nextest exited ${testStatus}")
        }
    } else {
        def failFastFlag = params.TEST_FAIL_FAST ? '' : '--no-fail-fast'
        shx "cargo test --workspace ${failFastFlag}"
    }
}

def runCoverage() {
    // POSIX-form workspace (WS_POSIX == WORKSPACE on Unix). outDir/coverageDir flow
    // into shx bodies (mkdir -p, generate-coverage.sh, cargo llvm-cov) and into
    // fileExists (which accepts forward slashes on Windows), so the POSIX form is safe.
    def outDir      = "${env.WS_POSIX}/${params.OUTPUT_SUBDIR}"
    def coverageDir = "${outDir}/coverage"
    shx "mkdir -p '${coverageDir}'"

    shx '''
        if ! cargo llvm-cov --version >/dev/null 2>&1; then
            if [ -d vendor/cargo-llvm-cov ]; then
                echo "Installing cargo-llvm-cov from vendor (offline)..."
                # Registry-form `cargo install --offline cargo-llvm-cov` ignores the
                # workspace vendored-source replacement and fails with "could not find
                # 'cargo-llvm-cov' in registry 'crates-io'". Copy the vendored crate out
                # and install by --path so its dependency tree resolves against vendor/
                # (mirrors the verified-working cargo-nextest offline install above).
                mkdir -p .ci-tools
                cp -r vendor/cargo-llvm-cov .ci-tools/
                # .ci-tools/ lives INSIDE the oxide-sloc checkout, so cargo would
                # auto-adopt the copied crate into the parent workspace (whose members
                # do not list it) and abort with "current package believes it's in a
                # workspace when it's not" BEFORE resolving from vendor/. Append an empty
                # [workspace] table so the copied crate is its own workspace root.
                printf '\n[workspace]\n' >> .ci-tools/cargo-llvm-cov/Cargo.toml
                cargo install --offline --path .ci-tools/cargo-llvm-cov \
                    > .llvmcov-install.log 2>&1 \
                    || echo "cargo-llvm-cov offline install failed; see the install log below."
                # Surface the log in the console — cleanWs deletes it post-build, so a
                # failure would otherwise be undiagnosable (mirrors the nextest block).
                tail -20 .llvmcov-install.log 2>/dev/null || true
            else
                echo "cargo-llvm-cov is not vendored under vendor/ — skipping offline install."
            fi
        fi
        # Announce a fallback ONLY for a tool that is actually present, so the console
        # never claims a fallback it cannot run. generate-coverage.sh does the real work
        # and reports the final skip when neither tool is available.
        if cargo llvm-cov --version >/dev/null 2>&1; then
            rustup component add llvm-tools 2>/dev/null || true
        elif cargo tarpaulin --version >/dev/null 2>&1; then
            echo "cargo-llvm-cov unavailable; using cargo-tarpaulin fallback."
        else
            echo "Neither cargo-llvm-cov nor cargo-tarpaulin available; coverage will be skipped."
        fi
    '''

    shx "bash ci/sonar/generate-coverage.sh '${coverageDir}'"

    shx """
        if cargo llvm-cov --version >/dev/null 2>&1; then
            echo "==> Generating HTML report with cargo-llvm-cov"
            cargo llvm-cov --workspace --all-features \
                --html --output-dir '${coverageDir}/html'
        elif command -v genhtml >/dev/null 2>&1; then
            echo "==> Generating HTML report with genhtml (lcov fallback)"
            if ! genhtml '${coverageDir}/lcov.info' \
                    --output-directory '${coverageDir}/html' \
                    --legend \
                    --branch-coverage \
                    --title 'oxide-sloc coverage' \
                    > '${coverageDir}/genhtml.log' 2>&1; then
                echo "WARNING: genhtml failed — HTML coverage report not generated."
            fi
            tail -20 '${coverageDir}/genhtml.log' 2>/dev/null || true
        else
            echo "HTML coverage report: skipped (install cargo-llvm-cov or lcov package)."
        fi
    """

    def threshold = params.COVERAGE_THRESHOLD?.trim()?.isDouble()
                        ? params.COVERAGE_THRESHOLD.trim().toDouble()
                        : (params.COVERAGE_THRESHOLD?.trim()?.isInteger()
                            ? params.COVERAGE_THRESHOLD.trim().toInteger() as Double
                            : 0.0)

    def lcovFile      = "${params.OUTPUT_SUBDIR}/coverage/lcov.info"

    // Feed the Jenkins Coverage view from LCOV only. cargo-llvm-cov's Cobertura
    // XML (sonar-coverage.xml) contains a duplicate element that the Coverage
    // plugin's CoberturaParser rejects, which threw and dropped the whole view;
    // lcov.info already provides line/branch/function coverage. sonar-coverage.xml
    // is still generated + archived for SonarQube — it's just not fed to Jenkins.
    def tools = []
    if (fileExists("${env.WORKSPACE}/${lcovFile}")) {
        tools << [parser: 'LCOV', pattern: lcovFile]
    }

    if (tools.isEmpty()) {
        echo 'WARNING: No coverage data files found — recordCoverage skipped.'
    } else {
        def gates = []
        if (threshold > 0) {
            gates << [threshold: threshold, metric: 'LINE',
                      baseline: 'PROJECT', criticality: 'FAILURE']
            gates << [threshold: threshold * 0.7, metric: 'BRANCH',
                      baseline: 'PROJECT', criticality: 'UNSTABLE']
        }
        // Discover a reference build so the Coverage view shows a delta/trend vs.
        // the previous build. Guarded: it needs the git-forensics plugin and is
        // harmless (just no delta) when absent.
        try {
            discoverReferenceBuild()
        } catch (Throwable t) {
            echo "discoverReferenceBuild skipped (git-forensics plugin not installed): ${t.message}"
        }
        // Guarded: without the Coverage plugin the lcov/cobertura files are still
        // archived; only the "Coverage" trend view is skipped. id 'coverage' →
        // conventional /job/<job>/<n>/coverage/ URL. sourceDirectories points the
        // source-painting at the Rust tree (crates/) instead of the Java default.
        try {
            recordCoverage(
                tools:               tools,
                id:                  'coverage',
                name:                'Coverage',
                sourceDirectories:   [[path: 'crates']],
                sourceCodeRetention: 'EVERY_BUILD',
                qualityGates:        gates
            )
        } catch (Throwable t) {
            // Could be a missing Coverage plugin OR a parser error — log the real
            // reason rather than assuming the plugin is absent.
            echo "recordCoverage skipped (Coverage plugin missing, or report parse error): ${t.message}"
        }
    }

    if (fileExists("${coverageDir}/html/index.html")) {
        publishHtmlSafe([
            allowMissing         : false,
            alwaysLinkToLastBuild: true,
            keepAll              : true,
            reportDir            : "${params.OUTPUT_SUBDIR}/coverage/html",
            reportFiles          : 'index.html',
            reportName           : 'Coverage Source',
        ])
    } else {
        echo 'Annotated HTML source report not available for this run.'
    }
}

// Publish an HTML report, degrading cleanly when the HTML Publisher plugin is
// absent: the report dir is already archived by archiveArtifacts, so without the
// plugin you simply lose the left-sidebar link, not the report. Throwable, not
// Exception — a missing publishHTML step throws NoSuchMethodError (an Error).
def publishHtmlSafe(Map target) {
    try {
        publishHTML(target: target)
    } catch (Throwable t) {
        echo "publishHTML '${target.reportName}' skipped (HTML Publisher plugin not installed); " +
             "the report is still available under Build Artifacts. (${t.message})"
    }
}

def runAnalyze() {
    def allowedPolicies = ['code-only', 'code-and-comment', 'comment-only', 'separate-mixed-category']
    def allowedPresets  = ['none', 'default', 'strict', 'full-scope']
    if (!allowedPolicies.contains(params.MIXED_LINE_POLICY)) {
        error("Invalid MIXED_LINE_POLICY value: ${params.MIXED_LINE_POLICY}")
    }
    if (!allowedPresets.contains(params.CI_PRESET)) {
        error("Invalid CI_PRESET value: ${params.CI_PRESET}")
    }
    if (params.OUTPUT_SUBDIR && !(params.OUTPUT_SUBDIR ==~ /^[a-zA-Z0-9_\-\/]+$/)) {
        error("OUTPUT_SUBDIR contains invalid characters: ${params.OUTPUT_SUBDIR}")
    }
    def safeGlob = /^[a-zA-Z0-9_\-\.\*\?\[\]\/\\]+$/
    if (params.INCLUDE_GLOBS) {
        params.INCLUDE_GLOBS.tokenize(',').each { g ->
            if (!(g.trim() ==~ safeGlob)) { error("INCLUDE_GLOBS contains invalid pattern: ${g.trim()}") }
        }
    }
    if (params.EXCLUDE_GLOBS) {
        params.EXCLUDE_GLOBS.tokenize(',').each { g ->
            if (!(g.trim() ==~ safeGlob)) { error("EXCLUDE_GLOBS contains invalid pattern: ${g.trim()}") }
        }
    }
    if (params.ENABLED_LANGUAGES) {
        params.ENABLED_LANGUAGES.tokenize(',').each { l ->
            if (!(l.trim() ==~ /^[a-zA-Z0-9\+\#]+$/)) {
                error("ENABLED_LANGUAGES contains invalid value: ${l.trim()}")
            }
        }
    }
    if (params.ACTIVITY_WINDOW?.trim() && !(params.ACTIVITY_WINDOW.trim() ==~ /^[0-9]{1,4}$/)) {
        error("ACTIVITY_WINDOW must be a number of days (0-3650): ${params.ACTIVITY_WINDOW}")
    }

    // POSIX-form workspace for bash-facing paths (WS_POSIX == WORKSPACE on Unix).
    def outDir = "${env.WS_POSIX}/${params.OUTPUT_SUBDIR}"
    shx "mkdir -p '${outDir}'"

    // scanRoot is where the project-under-analysis lives: the workspace root for a
    // self-scan, or ./_target when SCAN_REPO_URL was checked out (set in Checkout).
    // All scan commands and git-based features resolve against it. Use the POSIX
    // workspace form as the self-scan default so `git -C '${scanRoot}'` in the shx
    // bodies below gets forward slashes on Windows. SCAN_ROOT (set in Checkout) is
    // compared against WS_POSIX so scanningExternal is decided on the same form.
    def scanRoot         = env.SCAN_ROOT?.trim()?.replace('\\', '/') ?: env.WS_POSIX
    def scanningExternal = scanRoot != env.WS_POSIX

    // Resolve SCAN_PATH against scanRoot ONLY when scanning an external project, so a
    // plain self-scan keeps its exact previous behaviour (relative path, cwd=WORKSPACE).
    def effScan = params.SCAN_PATH?.trim() ?: '.'
    if (scanningExternal) {
        effScan = effScan.startsWith('/') ? effScan : "${scanRoot}/${effScan}"
        // Fail fast with a helpful message when a SCAN_PATH subtree does not exist
        // in the target repo (a blank SCAN_PATH scans the whole repo and never trips this).
        if (!fileExists(effScan)) {
            error("SCAN_PATH '${params.SCAN_PATH}' was not found in the target repo (${scanRoot}). " +
                  "Leave SCAN_PATH BLANK to scan the whole repo, or set an existing subtree like 'src'.")
        }
    }

    // Derive a project slug that matches the local-run naming convention:
    //   {repo-name}_{short-sha}  (e.g. my-project_a78a632)
    // Name comes from the last path segment of the scanned repo's URL (strip .git).
    // When scanning an external project that is SCAN_REPO_URL; otherwise the tooling
    // repo — TOOL_REPO_URL if the operator set it, else the URL the job actually checked out
    // from (SLOC_REPO_URL_EFFECTIVE, set in the Checkout stage). This keeps the self-scan
    // slug stable (repo-name_shortsha) even when TOOL_REPO_URL is blank and the pipeline fell
    // back to `checkout scm` on an air-gapped controller.
    def slugSource = scanningExternal
                        ? params.SCAN_REPO_URL
                        : (params.TOOL_REPO_URL?.trim() ?: env.SLOC_REPO_URL_EFFECTIVE)
    def repoSlug = (slugSource?.trim()
                        ? slugSource.trim()
                              .replaceAll(/\.git$/, '')
                              .replaceAll(/.*[\/:]/, '')
                              .replaceAll(/[^a-zA-Z0-9_\-]/, '-')
                              .replaceAll(/-+/, '-')
                              .replaceAll(/^-|-$/, '')
                        : '') ?: 'project'
    // Short SHA: from GIT_COMMIT (the tooling checkout) for a self-scan, or from the
    // target checkout's HEAD when scanning an external project.
    def shortSha
    if (scanningExternal) {
        shortSha = shxStdout("git -C '${scanRoot}' rev-parse --short HEAD 2>/dev/null || echo unknown").trim()
    } else {
        def rawSha = env.GIT_COMMIT?.trim() ?: ''
        shortSha = (rawSha.length() >= 7)
                        ? rawSha[0..6]
                        : shxStdout('git rev-parse --short HEAD').trim()
    }
    def projectSlug = "${repoSlug}_${shortSha}"
    env.SLOC_PROJECT = projectSlug

    // ── Effective report title ──────────────────────────────────────────────
    // The static default ('oxide-sloc CI Report') does not identify the scanned
    // repo. When the operator LEFT the title at its default (or blank), derive a
    // project-scoped title "<RepoName> CI SLOC Report" where <RepoName> is the
    // human repo name (last URL path segment, .git stripped, ORIGINAL case/hyphens
    // preserved — NOT the sanitized slug and NOT the short SHA). A custom title is
    // honoured verbatim. Note the new convention adds "SLOC" vs the old "CI Report".
    def defaultTitle = 'oxide-sloc CI Report'
    def rawTitle     = params.REPORT_TITLE?.trim() ?: ''
    def repoName     = (slugSource?.trim()
                            ? slugSource.trim()
                                  .replaceAll(/\.git$/, '')
                                  .replaceAll(/[\/:]+$/, '')
                                  .replaceAll(/.*[\/:]/, '')
                            : '') ?: repoSlug
    def effectiveTitle = (!rawTitle || rawTitle == defaultTitle)
                            ? "${repoName} CI SLOC Report"
                            : rawTitle
    env.SLOC_REPORT_TITLE_EFFECTIVE = effectiveTitle

    def configArg    = (params.CI_PRESET != 'none')
                        ? "--config 'ci/sloc-ci-${params.CI_PRESET}.toml'"
                        : ''
    def jsonArg      = "--json-out  '${outDir}/result_${projectSlug}.json'"
    def csvArg       = "--csv-out   '${outDir}/report_${projectSlug}.csv'"
    def xlsxArg      = "--xlsx-out  '${outDir}/report_${projectSlug}.xlsx'"
    def htmlArg      = params.REPORT_HTML       ? "--html-out '${outDir}/report_${projectSlug}.html'" : ''
    def pdfArg       = params.REPORT_PDF        ? "--pdf-out  '${outDir}/report_${projectSlug}.pdf'"  : ''
    def docArg       = params.DOCSTRINGS_AS_CODE  ? '--python-docstrings-as-code'        : ''
    def symlinkArg   = params.FOLLOW_SYMLINKS     ? '--follow-symlinks'                  : ''
    def noIgnoreArg  = params.NO_IGNORE_FILES     ? '--no-ignore-files'                  : ''
    def submodArg    = params.SUBMODULE_BREAKDOWN ? '--submodule-breakdown'              : ''
    def styleColArg  = (params.STYLE_COL_THRESHOLD?.trim() && params.STYLE_COL_THRESHOLD.trim() != '80')
                        ? "--style-col-threshold '${params.STYLE_COL_THRESHOLD.trim()}'"
                        : ''
    // Git Hotspots window. On by default (the binary uses 90 when omitted); only emit the flag
    // when the operator picked a non-default value, including 0 to disable.
    def activityArg  = (params.ACTIVITY_WINDOW?.trim() && params.ACTIVITY_WINDOW.trim() != '90')
                        ? "--activity-window '${params.ACTIVITY_WINDOW.trim()}'"
                        : ''
    // Outputs that mirror the web UI artifact layout: scan-config JSON and
    // per-submodule HTML reports (sub_<name>.html in the output directory).
    def scanConfigArg = "--scan-config-out '${outDir}/scan-config_${projectSlug}.json'"
    def subHtmlArg    = params.SUBMODULE_BREAKDOWN
                        ? "--sub-html-out-dir '${outDir}'"
                        : ''

    def includeArgs = params.INCLUDE_GLOBS
        ? params.INCLUDE_GLOBS.tokenize(',').collect { "--include-glob '${it.trim()}'" }.join(' ')
        : ''
    def excludeArgs = params.EXCLUDE_GLOBS
        ? params.EXCLUDE_GLOBS.tokenize(',').collect { "--exclude-glob '${it.trim()}'" }.join(' ')
        : ''
    def langArgs    = params.ENABLED_LANGUAGES
        ? params.ENABLED_LANGUAGES.tokenize(',').collect { "--enabled-language '${it.trim()}'" }.join(' ')
        : ''

    // a. Quick plain summary
    withEnv(["SCAN_PATH=${effScan}"]) {
        shx '''
            "${BINARY}" analyze "${SCAN_PATH}" --plain ''' + configArg + '''
        '''
    }

    // b. Main artifact run — JSON, CSV, XLSX always written; HTML and PDF are optional.
    // Pass --git-branch explicitly so the report shows the correct branch name even
    // when Jenkins performs a detached-HEAD checkout (env.GIT_BRANCH is empty or is
    // the TOOLING repo's branch on an external scan). Robust resolution:
    //
    //   EXTERNAL scan (SCAN_REPO_URL set): the target was checked out into
    //     ./_target from the SCAN_REF param. Prefer SCAN_REF (the ref the pipeline
    //     actually asked for; defaults to "main" — see the Checkout stage), else
    //     ask the target checkout directly (`git -C <scanRoot> rev-parse
    //     --abbrev-ref HEAD`). A detached-HEAD checkout returns the literal "HEAD";
    //     when that happens, fall back to SCAN_REF.
    //   SELF scan: prefer env.BRANCH_NAME (Jenkins multibranch), then env.GIT_BRANCH,
    //     stripping origin/ and refs/heads/ prefixes.
    //
    // Only pass --git-branch when the resolved value is concrete (non-empty and not
    // the detached-HEAD sentinel "HEAD"); the binary leaves git_branch as detected
    // otherwise. A bare SHA or tag SCAN_REF is still passed through so the report
    // shows the ref that was scanned rather than "not detected".
    def stripRef = { String s ->
        (s ?: '').replaceAll('^origin/', '').replaceAll('^refs/heads/', '')
                 .replaceAll('^refs/tags/', '').trim()
    }
    def branchName = ''
    if (scanningExternal) {
        def scanRef = params.SCAN_REF?.trim() ?: ''
        def detected = shxStdout(
            "git -C '${scanRoot}' rev-parse --abbrev-ref HEAD 2>/dev/null || echo ''"
        ).trim().readLines().findAll { it?.trim() }.with { it ? it.last().trim() : '' }
        if (detected && detected != 'HEAD') {
            branchName = stripRef(detected)
        } else {
            branchName = stripRef(scanRef)
        }
    } else {
        def selfRaw = env.BRANCH_NAME?.trim() ?: env.GIT_BRANCH?.trim() ?: ''
        branchName = stripRef(selfRaw)
    }
    def branchArg = (branchName && branchName != 'HEAD') ? "--git-branch '${branchName}'" : ''
    // Per-author code-ownership metrics (git blame). Opt-in on the CLI; ON by default
    // here. Blame walks full history, so the scanned checkout must NOT be shallow —
    // the Jenkinsfile Checkout stage pins CloneOption(shallow:false, depth:0) for this.
    def attributionArg = params.RUN_ATTRIBUTION ? '--attribution' : ''
    // Export the resolved branch + the commit for the persistent trend CSV (Change 4).
    env.SLOC_TREND_BRANCH = branchName
    env.SLOC_TREND_COMMIT = env.GIT_COMMIT?.trim() ?: shortSha
    // Effective scanned repo URL for the per-repo trend history + dashboard delta.
    // Mirrors slugSource: SCAN_REPO_URL for an external scan, else the tooling repo
    // URL the job was built from. Normalization happens in the Python consumers.
    env.SLOC_TREND_REPO_URL = (slugSource?.trim() ?: '')

    withEnv([
        "SCAN_PATH=${effScan}",
        "REPORT_TITLE=${effectiveTitle}",
        "MIXED_LINE_POLICY=${params.MIXED_LINE_POLICY}",
    ]) {
        shx '''
            "${BINARY}" analyze "${SCAN_PATH}" \
                --report-title "${REPORT_TITLE}" \
                --mixed-line-policy "${MIXED_LINE_POLICY}" \
                ''' + "${configArg} ${docArg} ${symlinkArg} ${noIgnoreArg} ${submodArg} ${styleColArg} ${activityArg}" + ''' \
                ''' + "${langArgs} ${includeArgs} ${excludeArgs} ${branchArg} ${attributionArg}" + ''' \
                ''' + "${jsonArg} ${csvArg} ${xlsxArg} ${htmlArg} ${pdfArg}" + ''' \
                ''' + "${scanConfigArg} ${subHtmlArg}" + '''
        '''
    }

    shx "test -s '${outDir}/result_${projectSlug}.json'"
    shx "test -s '${outDir}/report_${projectSlug}.csv'"
    shx "test -s '${outDir}/report_${projectSlug}.xlsx'"
    if (params.REPORT_HTML) { shx "test -s '${outDir}/report_${projectSlug}.html'" }

    // b2. Central-pool ingest push — cross-environment report pooling. When
    // POOL_INGEST_URL is set, POST the verified JSON to <URL>/api/ingest on a shared
    // oxide-sloc `serve` instance so this run's report pools with reports from other
    // environments/agents. Best-effort: a failed push logs a warning and never fails
    // the build. The API key (when a credential is configured) is bound to
    // SLOC_WEBHOOK_TOKEN in the environment and read there by `send` — it is never
    // placed on the command line, so it stays out of argv and the console log. The
    // trailing slash of the base URL is trimmed in-shell (${POOL_INGEST_URL%/}), and
    // --allow-private-net permits an RFC-1918/intranet pool server.
    if (params.POOL_INGEST_URL?.trim()) {
        def resultJson = "${outDir}/result_${projectSlug}.json"
        def doPush = { ->
            try {
                withEnv(["POOL_INGEST_URL=${params.POOL_INGEST_URL.trim()}"]) {
                    shx '''
                        "${BINARY}" send "''' + resultJson + '''" \
                            --webhook-url "${POOL_INGEST_URL%/}/api/ingest" \
                            --allow-private-net
                    '''
                }
            } catch (err) {
                echo "WARNING: central-pool ingest push to ${params.POOL_INGEST_URL.trim()} " +
                     "failed (non-fatal, scan artifacts preserved): ${err.message}"
            }
        }
        def credId = params.POOL_INGEST_TOKEN_CREDENTIAL?.trim()
        if (credId) {
            // Optional Secret Text credential → SLOC_WEBHOOK_TOKEN (masked, env-only).
            withCredentials([string(credentialsId: credId, variable: 'SLOC_WEBHOOK_TOKEN')]) {
                doPush()
            }
        } else {
            doPush()
        }
    }

    // c. Per-file breakdown — a self-test pass (console only, no artifact). Gated
    // behind RUN_ANALYZE_SELFTEST: it re-walks the whole repo just to exercise the
    // --per-file path, which is pure overhead on a normal metrics scan.
    if (params.RUN_ANALYZE_SELFTEST) {
        withEnv(["SCAN_PATH=${effScan}"]) {
            shx '''
                "${BINARY}" analyze "${SCAN_PATH}" --per-file --plain ''' + configArg + '''
            '''
        }
    }

    // d. HTML content sanity checks — grep for the EFFECTIVE title actually passed
    // to --report-title (the auto-derived "<Repo> CI SLOC Report" when the operator
    // left REPORT_TITLE at its default), not the raw param, or the grep would miss.
    if (params.REPORT_HTML) {
        withEnv(["REPORT_TITLE=${effectiveTitle}"]) {
            shx '''
                grep -q 'OxideSLOC' "''' + outDir + '''/report_''' + projectSlug + '''.html"
                grep -qF "${REPORT_TITLE}" "''' + outDir + '''/report_''' + projectSlug + '''.html"
            '''
        }
    }

    // e. Extract inline CSS/JS so the report renders under Jenkins's default CSP.
    if (params.REPORT_HTML) {
        shx "${pyBin()} ci/jenkins/extract-report-assets.py '${outDir}/report_${projectSlug}.html' || true"
    }

    // f. Mixed-line policy matrix — spot-checks all four policies. Four more full-repo
    // walks whose output is discarded; gated behind RUN_ANALYZE_SELFTEST for the same
    // reason as the per-file pass above.
    if (params.RUN_ANALYZE_SELFTEST) {
        for (def policy in ['code-only', 'code-and-comment', 'comment-only', 'separate-mixed-category']) {
            withEnv(["SCAN_PATH=${effScan}"]) {
                shx '''
                    "${BINARY}" analyze "${SCAN_PATH}" --plain --mixed-line-policy ''' + policy + '''
                '''
            }
        }
    }
}

def runArchivePublish() {
    // POSIX-form paths for the .py/.sh helpers that consume them via shx (WS_POSIX
    // == WORKSPACE on Unix). The history home is the writable, POSIX-normalised
    // cache root from initEnv (never a backslash / read-only profile path).
    def outDir   = "${env.WS_POSIX}/${params.OUTPUT_SUBDIR}"
    def proj     = env.SLOC_PROJECT ?: 'project'
    def jobSlug  = (env.JOB_NAME?.replaceAll('[^a-zA-Z0-9_\\-]', '_') ?: 'oxide-sloc')
    def histFile = "${env.HISTORY_HOME_POSIX}/.oxide-sloc-history/${jobSlug}.csv"

    shx "${pyBin()} ci/jenkins/generate-trend-csv.py '${outDir}' '${proj}' '${histFile}'"

    // Detect what this build is allowed to do (system-admin ⇒ may install the
    // richer visualization plugins; otherwise degrade to the native dashboard).
    // The auth-API probe uses the 'jenkins-api-token' credential when present;
    // every step here is best-effort and never fails the build.
    try {
        def base = (env.BUILD_URL ?: '').replaceAll('/job/.*', '').replaceAll('/+$', '')
        // 'jenkins-api-token' is a secret-text credential (a token); the API user
        // defaults to 'admin' to match runSetup, overridable via JENKINS_API_USER.
        withCredentials([string(credentialsId: 'jenkins-api-token',
                                variable: 'SLOC_JTOKEN')]) {
            // The token must be read as $SLOC_JTOKEN INSIDE sh — a withCredentials
            // binding is injected into the shell env (masked) but is NOT visible via
            // ${env.SLOC_JTOKEN} in Groovy. Interpolating it in Groovy sent an empty
            // token (so the probe ran anonymously and the dashboard wrongly showed
            // "degraded / read-only") and would also defeat Jenkins' secret masking.
            withEnv(["JENKINS_BASE_URL=${base}",
                     "JENKINS_USER=${env.JENKINS_API_USER ?: 'admin'}"]) {
                shx "JENKINS_AUTH_TOKEN=\"\$SLOC_JTOKEN\" ${pyBin()} ci/jenkins/detect-capabilities.py '${outDir}' || true"
                shx "JENKINS_AUTH_TOKEN=\"\$SLOC_JTOKEN\" bash ci/jenkins/install-plugins.sh '${outDir}' || true"
            }
        }
    } catch (Exception ex) {
        // No 'jenkins-api-token' credential — still probe anonymously so the
        // dashboard can show the correct (degraded) banner.
        echo "Capability probe running without API credentials: ${ex.message}"
        def base = (env.BUILD_URL ?: '').replaceAll('/job/.*', '').replaceAll('/+$', '')
        withEnv(["JENKINS_BASE_URL=${base}"]) {
            shx "${pyBin()} ci/jenkins/detect-capabilities.py '${outDir}' || true"
        }
    }

    // Generate the dashboard (and the curated ci-report/ + html-report/ bundles)
    // BEFORE archiving so the bundle layout is present for both archive + publish.
    // The dashboard reads SLOC_TREND_REPO_URL (set in runAnalyze) to compute the
    // per-repo delta + per-project trend view; env vars set on env{} in an earlier
    // stage are already exported into this shx shell, so no extra wiring is needed.
    def dashboardBuilt = false
    try {
        shx "${pyBin()} ci/jenkins/generate-dashboard.py '${outDir}' '${proj}' '${histFile}'"
        dashboardBuilt = fileExists("${outDir}/ci-report/index.html")
    } catch (Exception ex) {
        echo "generate-dashboard.py did not run (Python 3 unavailable or script error): ${ex.message}"
    }

    // ── Local-import bundle via the `oxide-sloc bundle` CLI (best-effort) ────
    // Emit a folder mirroring the LOCAL web-UI run layout (run_dir with
    // html/json/excel/pdf subfolders + index.html, plus a registry.json). A user
    // unzips local-import/ into their own oxide-sloc out/web/ directory to use the
    // Scan Delta / Compare pages on this run locally. Wrapped so an older binary
    // WITHOUT the `bundle` subcommand does NOT fail the build.
    try {
        shx "\"\${BINARY}\" bundle '${outDir}/result_${proj}.json' --out-dir '${outDir}/local-import' " +
            "|| echo 'bundle subcommand unavailable; skipping local-import'"
    } catch (Exception ex) {
        echo "local-import bundle skipped: ${ex.message}"
    }

    // ── Organized dual-archive bundle: BOTH .zip AND .tar.gz ────────────────
    // Package the flat artifacts into an organized subfolder tree (html/ json/
    // csv/ pdf/ xlsx/ config/ dashboard/ + local-import/) and emit BOTH archives.
    // The .tar.gz is the fallback for corporate browsers/proxies that block .zip
    // downloads from Jenkins (Git Bash tar + 7-Zip open it). Built with the
    // bundled Python (zipfile + tarfile) so it needs no `zip` binary. Best-effort.
    try {
        shx "${pyBin()} ci/jenkins/bundle-artifacts.py '${outDir}' '${proj}' || true"
    } catch (Exception ex) {
        echo "Artifact bundling (zip + tar.gz) skipped: ${ex.message}"
    }

    // Archive everything except the curated bundle dirs — those are duplicated
    // copies published separately below, so keeping them out keeps the archive lean.
    archiveArtifacts artifacts: "${params.OUTPUT_SUBDIR}/**",
        excludes: "${params.OUTPUT_SUBDIR}/ci-report/**,${params.OUTPUT_SUBDIR}/html-report/**",
        fingerprint: true,
        allowEmptyArchive: true

    // Zip names come from the report name (htmlpublisher sanitises it), so we use
    // project-scoped, space/em-dash-free names → e.g. OxideSLOC_CI_Report_<proj>.zip.
    // reportFiles is always index.html, so the zip has one obvious entry point.
    if (dashboardBuilt) {
        publishHtmlSafe([
            allowMissing         : true,
            alwaysLinkToLastBuild: true,
            keepAll              : true,
            reportDir            : "${params.OUTPUT_SUBDIR}/ci-report",
            reportFiles          : "index.html",
            reportName           : "OxideSLOC_CI_Report_${proj}",
        ])
    }

    if (params.REPORT_HTML && fileExists("${outDir}/html-report/index.html")) {
        publishHtmlSafe([
            allowMissing         : false,
            alwaysLinkToLastBuild: true,
            keepAll              : true,
            reportDir            : "${params.OUTPUT_SUBDIR}/html-report",
            reportFiles          : "index.html",
            reportName           : "OxideSLOC_HTML_Report_${proj}",
        ])
    }

    try {
        if (fileExists("${env.WORKSPACE}/${params.OUTPUT_SUBDIR}/summary.csv")) {
            plot(
                csvFileName: 'sloc-trend-summary.csv',
                csvSeries:   [[file: "${params.OUTPUT_SUBDIR}/summary.csv",
                               url: '', displayTableFlag: false,
                               inclusionFlag: 'OFF', exclusionValues: '']],
                group:       'SLOC Trends',
                title:       'SLOC Totals Over Time',
                style:       'line',
                yaxis:       'Lines',
                numBuilds:   '50',
                keepRecords: true,
                useDescr:    true
            )
        }
        if (fileExists("${env.WORKSPACE}/${params.OUTPUT_SUBDIR}/per_language.csv")) {
            plot(
                csvFileName: 'sloc-trend-per-language.csv',
                csvSeries:   [[file: "${params.OUTPUT_SUBDIR}/per_language.csv",
                               url: '', displayTableFlag: false,
                               inclusionFlag: 'OFF', exclusionValues: '']],
                group:       'SLOC Trends',
                title:       'Per-Language Code Lines',
                style:       'bar',
                yaxis:       'Code Lines',
                numBuilds:   '50',
                keepRecords: true,
                useDescr:    true
            )
        }
        if (params.RUN_COVERAGE &&
                fileExists("${env.WORKSPACE}/${params.OUTPUT_SUBDIR}/coverage.csv")) {
            plot(
                csvFileName: 'sloc-trend-coverage.csv',
                csvSeries:   [[file: "${params.OUTPUT_SUBDIR}/coverage.csv",
                               url: '', displayTableFlag: false,
                               inclusionFlag: 'OFF', exclusionValues: '']],
                group:       'SLOC Trends',
                title:       'Line Coverage % Over Time',
                style:       'line',
                yaxis:       'Coverage %',
                numBuilds:   '50',
                keepRecords: true,
                useDescr:    true
            )
        }
        if (fileExists("${env.WORKSPACE}/${params.OUTPUT_SUBDIR}/style_analysis.csv")) {
            plot(
                csvFileName: 'sloc-trend-style.csv',
                csvSeries:   [[file: "${params.OUTPUT_SUBDIR}/style_analysis.csv",
                               url: '', displayTableFlag: false,
                               inclusionFlag: 'OFF', exclusionValues: '']],
                group:       'SLOC Trends',
                title:       'Code Style N-Col Compliance % Over Time',
                style:       'line',
                yaxis:       'Compliance %',
                numBuilds:   '50',
                keepRecords: true,
                useDescr:    true
            )
        }
    } catch (Throwable ex) {
        // Throwable, not Exception: a missing plot step throws NoSuchMethodError
        // (an Error). Without the plugin the trend CSVs are still archived.
        echo "Plot trend charts skipped (install the 'plot' plugin to enable): ${ex.message}"
    }
}

def runPushArtifacts() {
    def allowedTypes = [
        'artifactory', 'nexus', 'nexus2', 's3', 'minio', 'azure-blob', 'generic-http'
    ]
    if (!allowedTypes.contains(params.ARTIFACT_REPO_TYPE)) {
        error("Invalid ARTIFACT_REPO_TYPE: ${params.ARTIFACT_REPO_TYPE}")
    }

    // POSIX-form paths — outDir/binaryPath/junitPath/srcPath all flow into shx `cp`
    // (and ARTIFACT_DIR into artifact-push.sh); WS_POSIX == WORKSPACE on Unix.
    def outDir = "${env.WS_POSIX}/${params.OUTPUT_SUBDIR}"

    def repoPath = params.ARTIFACT_REPO_PATH
        .replace('${JOB_NAME}',     env.JOB_NAME    ?: 'unknown-job')
        .replace('${BUILD_NUMBER}', env.BUILD_NUMBER ?: '0')

    def filesToPush = []
    def proj = env.SLOC_PROJECT ?: 'project'
    if (params.ARTIFACT_PUSH_JSON)                           filesToPush << "result_${proj}.json"
    if (params.ARTIFACT_PUSH_CSV)                            filesToPush << "report_${proj}.csv"
    if (params.ARTIFACT_PUSH_XLSX)                           filesToPush << "report_${proj}.xlsx"
    if (params.ARTIFACT_PUSH_HTML && params.REPORT_HTML)   filesToPush << "report_${proj}.html"
    if (params.ARTIFACT_PUSH_PDF  && params.REPORT_PDF)    filesToPush << "report_${proj}.pdf"

    if (params.ARTIFACT_PUSH_BINARY) {
        def binaryName = isUnix() ? 'oxide-sloc' : 'oxide-sloc.exe'
        def binaryPath = "${env.WS_POSIX}/target/release/${binaryName}"
        if (fileExists(binaryPath)) {
            shx "cp '${binaryPath}' '${outDir}/${binaryName}'"
            filesToPush << binaryName
        } else {
            echo "WARNING: binary not found at ${binaryPath} — skipping binary push."
        }
    }

    if (params.ARTIFACT_PUSH_JUNIT
            && params.PUBLISH_TEST_RESULTS
            && params.TEST_RUNNER == 'cargo-nextest') {
        def junitPath = "${env.WS_POSIX}/test-results/junit.xml"
        if (fileExists(junitPath)) {
            shx "cp '${junitPath}' '${outDir}/junit.xml'"
            filesToPush << 'junit.xml'
        } else {
            echo "WARNING: junit.xml not found at ${junitPath} — skipping junit push."
        }
    }

    if (params.ARTIFACT_PUSH_COVERAGE && params.RUN_COVERAGE) {
        [['coverage/lcov.info', 'lcov.info'],
         ['coverage/sonar-coverage.xml', 'sonar-coverage.xml']].each { src, dst ->
            def srcPath = "${env.WS_POSIX}/${src}"
            if (fileExists(srcPath)) {
                shx "cp '${srcPath}' '${outDir}/${dst}'"
                filesToPush << dst
            }
        }
    }

    if (params.ARTIFACT_PUSH_DIFF && params.COMPARE_REF?.trim()) {
        ['diff.json', 'diff.csv'].each { f ->
            if (fileExists("${outDir}/${f}")) filesToPush << f
        }
    }

    if (filesToPush.isEmpty()) {
        echo 'No artifact files selected for push — skipping.'
        return
    }

    try {
        withCredentials([
            string(credentialsId: 'SLOC_ARTIFACT_REPO_USER', variable: 'SLOC_AR_USER'),
            string(credentialsId: 'SLOC_ARTIFACT_REPO_PASS', variable: 'SLOC_AR_PASS'),
        ]) {
            withEnv([
                "ARTIFACT_REPO_TYPE=${params.ARTIFACT_REPO_TYPE}",
                "ARTIFACT_REPO_URL=${params.ARTIFACT_REPO_URL}",
                "ARTIFACT_REPO_PATH=${repoPath}",
                "ARTIFACT_REPO_EXTRA=${params.ARTIFACT_REPO_EXTRA ?: ''}",
                "ARTIFACT_DIR=${outDir}",
                "ARTIFACT_FILES=${filesToPush.join(' ')}",
                "ARTIFACT_REPO_USER=${env.SLOC_AR_USER ?: ''}",
                "ARTIFACT_REPO_PASS=${env.SLOC_AR_PASS ?: ''}",
                "ARTIFACT_GENERATE_MANIFEST=${params.ARTIFACT_GENERATE_MANIFEST}",
            ]) {
                shx 'bash ci/artifact-push.sh'
            }
        }
    } catch (Exception ex) {
        echo "WARNING: Artifact repo credentials (SLOC_ARTIFACT_REPO_USER / SLOC_ARTIFACT_REPO_PASS) not found in Jenkins credential store — attempting unauthenticated push. (${ex.message})"
        withEnv([
            "ARTIFACT_REPO_TYPE=${params.ARTIFACT_REPO_TYPE}",
            "ARTIFACT_REPO_URL=${params.ARTIFACT_REPO_URL}",
            "ARTIFACT_REPO_PATH=${repoPath}",
            "ARTIFACT_REPO_EXTRA=${params.ARTIFACT_REPO_EXTRA ?: ''}",
            "ARTIFACT_DIR=${outDir}",
            "ARTIFACT_FILES=${filesToPush.join(' ')}",
            "ARTIFACT_REPO_USER=",
            "ARTIFACT_REPO_PASS=",
            "ARTIFACT_GENERATE_MANIFEST=${params.ARTIFACT_GENERATE_MANIFEST}",
        ]) {
            shx 'bash ci/artifact-push.sh'
        }
    }
}

def runGitRefCompare() {
    withEnv([
        "GIT_REF=${params.COMPARE_REF}",
        "COMPARE_TO_REF=${params.COMPARE_BASELINE_REF}",
        "COMPARE_TO_PREV_TAG=${params.COMPARE_PREV_TAG}",
        "OUTPUT_SUBDIR=${params.OUTPUT_SUBDIR}",
        // POSIX-normalise both the scan root and the workspace so the bash body's
        // git -C / analyze / diff get forward-slash paths on Windows. WS_POSIX
        // overrides the ambient (backslash) $WORKSPACE that the agent injects.
        "SCAN_ROOT=${(env.SCAN_ROOT ?: env.WORKSPACE)?.replace('\\', '/')}",
        "WS_POSIX=${env.WS_POSIX}",
    ]) {
        shx '''
            OUT="${WS_POSIX}/${OUTPUT_SUBDIR}"
            # Honour the OS-aware BINARY exported by initEnv (carries .exe on
            # Windows); fall back to the POSIX default when it is unset.
            BINARY="${BINARY:-${WS_POSIX}/target/release/oxide-sloc}"
            # Git operations target the scanned repo (workspace root for a self-scan,
            # ./_target when SCAN_REPO_URL was checked out). Use the POSIX workspace.
            REPO="${SCAN_ROOT:-$WS_POSIX}"

            # Resolve baseline ref
            if [ "${COMPARE_TO_PREV_TAG:-false}" = "true" ]; then
                CURRENT_TAG=$(git -C "${REPO}" tag --sort=-version:refname | head -1)
                BASELINE_TAG=$(git -C "${REPO}" tag --sort=-version:refname | grep -v "^${CURRENT_TAG}$" | head -1)
                BASELINE_REF="${BASELINE_TAG}"
                echo "Auto-detected previous tag: ${BASELINE_REF} (current: ${CURRENT_TAG})"
            else
                BASELINE_REF="${COMPARE_TO_REF}"
            fi

            if [ -z "${BASELINE_REF}" ]; then
                echo "No baseline ref found — skipping comparison."
                exit 0
            fi

            # Determine what was scanned as "current"
            CURRENT_JSON="${OUT}/ref-scan.json"
            if [ ! -f "${CURRENT_JSON}" ]; then
                CURRENT_JSON="${OUT}/result_${SLOC_PROJECT:-project}.json"
            fi
            if [ ! -f "${CURRENT_JSON}" ]; then
                echo "No current scan JSON found — cannot compare."
                exit 1
            fi

            echo "=== Scanning baseline: ${BASELINE_REF} ==="
            WT_BASE="${WS_POSIX}/.wt-baseline"
            git -C "${REPO}" worktree add --detach "${WT_BASE}" "${BASELINE_REF}"

            "${BINARY}" analyze "${WT_BASE}" \
                --json-out  "${OUT}/baseline-scan.json" \
                --report-title "Baseline: ${BASELINE_REF}" \
                --plain

            git -C "${REPO}" worktree remove --force "${WT_BASE}" || true

            echo "=== Computing diff ==="
            "${BINARY}" diff \
                "${OUT}/baseline-scan.json" \
                "${CURRENT_JSON}" \
                --json-out "${OUT}/diff.json" \
                --csv-out  "${OUT}/diff.csv" \
                --plain
        '''
    }
}

def runPostSuccess() {
    try {
        // POSIX-form (WS_POSIX == WORKSPACE on Unix). outDir feeds both Jenkins
        // steps (readJSON/readFile/fileExists accept forward slashes) and shx
        // bodies (the lcov awk + build-summary.py), so POSIX is correct for both.
        def outDir = "${env.WS_POSIX}/${params.OUTPUT_SUBDIR}"
        def proj   = env.SLOC_PROJECT
                        ?: (params.SCAN_PATH?.trim()?.split('[/\\\\]') as List)?.last()
                        ?: 'project'
        def result = readJSON file: "${outDir}/result_${proj}.json"
        def t      = result.summary_totals

        // A human label for the build row: the SCAN_PATH subtree if set, else the
        // target repo name (external scan), else "whole repo" (self-scan). SCAN_PATH
        // is commonly blank now (blank = whole repo), so never show an empty label.
        def scanLabel = params.SCAN_PATH?.trim()
        if (!scanLabel) {
            def tRepo = params.SCAN_REPO_URL?.trim()
            scanLabel = tRepo
                ? tRepo.replaceAll(/\.git$/, '').replaceAll(/.*[\/:]/, '')
                : 'whole repo'
        }

        def fmtN = { n ->
            long v = n as long
            long a = Math.abs(v)
            if (a >= 1_000_000L) {
                String s = String.format('%.1f', v / 1_000_000.0) + 'M'
                return s.replace('.0M', 'M')
            }
            if (a >= 10_000L) return "${Math.round(v / 1_000.0d)}K"
            return String.format('%,d', v)
        }
        // Plain one-line fallback (used if the rich generator can't run).
        def desc = "${fmtN(t.code_lines)} code · " +
                   "${fmtN(t.comment_lines)} cmts · " +
                   "${fmtN(t.blank_lines)} blank · " +
                   "${fmtN(t.files_analyzed)} files | ${scanLabel}"

        // Capture the sub-metrics so the rich summary generator can lay them out.
        def styleStr = ''
        def testsStr = ''
        def covStr   = ''

        def ss = result.style_summary
        if (ss) {
            def colThreshold = ss.col_threshold ?: 80
            def colPct       = ss.line_col_compliant_pct ?: 0
            styleStr = "${ss.common_indent_style} · ${colPct}% ${colThreshold}-col"
            desc += " · Style: ${styleStr}"
        }

        def junitPath = "${outDir}/test-results/junit.xml"
        if (fileExists(junitPath)) {
            try {
                def junitXml   = readFile(junitPath)
                def tm = junitXml =~ /tests="(\d+)"/
                def fm = junitXml =~ /failures="(\d+)"/
                def em = junitXml =~ /errors="(\d+)"/
                def totalTests = tm ? (tm[0][1] as long) : 0L
                def failCount  = fm ? (fm[0][1] as long) : 0L
                def errCount   = em ? (em[0][1] as long) : 0L
                def passCount  = Math.max(0L, totalTests - failCount - errCount)
                def testStatus = (failCount == 0 && errCount == 0) ? 'OK' : "FAIL(${fmtN(failCount)})"
                testsStr = "${passCount}/${totalTests}"
                desc += " · ${testsStr} tests ${testStatus}"
            } catch (Exception ex) {
                echo "Could not parse JUnit XML for description: ${ex.message}"
            }
        }

        def lcovPath = "${outDir}/coverage/lcov.info"
        if (fileExists(lcovPath)) {
            try {
                def pct = shxStdout("""
                        TOTAL=\$(grep -E '^LF:' '${lcovPath}' | awk -F: '{s+=\$2} END{print s+0}')
                        HIT=\$(grep -E '^LH:' '${lcovPath}'   | awk -F: '{s+=\$2} END{print s+0}')
                        [ "\${TOTAL}" -gt 0 ] && \
                            awk "BEGIN { printf \\"%.1f\\", (\${HIT}/\${TOTAL})*100 }" || echo "N/A"
                    """).trim()
                if (pct != 'N/A') {
                    covStr = pct
                    desc += " · ${pct}% cov"
                }
            } catch (Exception ex) {
                echo "Could not read coverage for description: ${ex.message}"
            }
        }

        // Rich, multi-line Unicode bar-chart description + HTML summary panel.
        // build-summary.py renders both; the .txt is set as the description
        // (renders under the default Plain-text formatter — newlines + block
        // glyphs, no HTML needed) and the .html feeds the badge summary box.
        def richDesc = desc
        try {
            withEnv(["SLOC_SCAN_PATH=${scanLabel}",
                     "SLOC_TESTS=${testsStr}",
                     "SLOC_COV=${covStr}",
                     "SLOC_STYLE=${styleStr}"]) {
                shx """
                    ${pyBin()} ci/jenkins/build-summary.py \
                        '${outDir}/result_${proj}.json' '${outDir}' \
                        --scan-path "\${SLOC_SCAN_PATH}" \
                        --tests "\${SLOC_TESTS}" \
                        --coverage "\${SLOC_COV}" \
                        --style "\${SLOC_STYLE}" >/dev/null || true
                """
            }
            if (fileExists("${outDir}/build-description.txt")) {
                richDesc = readFile(file: "${outDir}/build-description.txt")
            }
        } catch (Exception ex) {
            echo "Rich build summary generation skipped: ${ex.message}"
        }

        currentBuild.description = richDesc
        currentBuild.displayName = "#${env.BUILD_NUMBER} — ${scanLabel}"

        // A boxed HTML summary panel (metric table + coloured language bars) on
        // the build page, via the badge plugin's createSummary. Catch Throwable,
        // not Exception: a missing pipeline step (no badge plugin) throws
        // NoSuchMethodError (an Error), which would otherwise flip a successful
        // build to FAILURE. Degrades to the Unicode description above.
        try {
            if (fileExists("${outDir}/build-summary.html")) {
                def panel = readFile(file: "${outDir}/build-summary.html")
                createSummary(icon: 'symbol-analytics-outline plugin-ionicons-api',
                              text: panel)
            }
        } catch (Throwable ignore) {
            // badge/summary plugin not installed — non-fatal by design.
        }

        // Compact headline badge on the run row itself (also badge-plugin gated).
        try {
            addBadge(icon: 'symbol-analytics-outline plugin-ionicons-api',
                     text: "oxide-sloc: ${fmtN(t.code_lines)} code · " +
                           "${fmtN(t.files_analyzed)} files")
        } catch (Throwable ignore) {
            // badge plugin not installed — non-fatal by design.
        }
    } catch (Throwable ex) {
        echo "Could not set build metadata: ${ex.message}"
    }
    if (currentBuild.result == 'UNSTABLE') {
        echo 'Build UNSTABLE — artifacts and reports archived; review the stage warnings above.'
    } else {
        echo 'All stages passed. Artifacts and reports archived.'
    }

    // Only chain a downstream job on a clean SUCCESS. runPostSuccess is also invoked
    // from post{unstable} (to populate the build row), where triggering downstream
    // would be wrong.
    if (params.CHAIN_DOWNSTREAM_JOB?.trim()
            && (currentBuild.result == null || currentBuild.result == 'SUCCESS')) {
        // Pass the chaining context forward. Names match this job's own inputs
        // (CHAIN_UPSTREAM_JOB / CHAIN_UPSTREAM_BUILD) so an oxide-sloc -> oxide-sloc
        // chain works; ARTIFACT_PATH is a generic pointer the downstream job defines.
        build job: params.CHAIN_DOWNSTREAM_JOB,
              parameters: [
                  string(name: 'CHAIN_UPSTREAM_JOB',   value: env.JOB_NAME),
                  string(name: 'CHAIN_UPSTREAM_BUILD',  value: env.BUILD_NUMBER),
                  string(name: 'ARTIFACT_PATH',         value: env.ARTIFACT_PATH ?: '')
              ],
              wait: false,
              propagate: false
    }
}

def runBitbucketNotify() {
    def result = currentBuild.result ?: 'SUCCESS'
    def state  = result == 'SUCCESS' ? 'SUCCESSFUL' :
                 result == 'FAILURE' ? 'FAILED' : 'STOPPED'

    // 1. Plugin path — used when the bitbucket-build-status-notifier plugin is
    //    installed (i.e. a system-admin build was able to enable it).
    if (env.BITBUCKET_SOURCE_BRANCH || env.GIT_COMMIT) {
        try {
            // Note: no buildUrl: parameter — the installed plugin version does not
            // accept it (it derives the link from the build) and passing it logs a
            // "WARNING: Unknown parameter(s) found ... buildUrl" line on every build.
            bitbucketStatusNotify(
                buildState: state,
                buildKey:   env.JOB_NAME,
                buildName:  "oxide-sloc CI #${env.BUILD_NUMBER}"
            )
        } catch (Throwable e) {
            // An absent plugin makes Pipeline throw java.lang.NoSuchMethodError (a
            // java.lang.Error, NOT an Exception), so catch Throwable — an untyped
            // `catch (e)` compiles to catch(Exception) and would let the Error escape,
            // aborting post{always} and skipping the REST fallback entirely.
            echo "Bitbucket status notify via plugin skipped (plugin not installed): ${e.message}"
        }
    }

    // 2. Plugin-independent path — post the status + a link to the published
    //    report to Bitbucket, and upsert a Confluence summary page. Both are
    //    fully opt-in via credentials and no-op (exit 0) when unconfigured, so
    //    they work with or without the plugins and never fail the build.
    def proj      = env.SLOC_PROJECT ?: 'project'
    def reportUrl = env.BUILD_URL ? "${env.BUILD_URL}OxideSLOC_CI_Report_${proj}/" : ''
    // POSIX-form for the shx body that runs notify-confluence.py (WS_POSIX == WORKSPACE on Unix).
    def outDir    = "${env.WS_POSIX}/${params.OUTPUT_SUBDIR}"
    def bbState   = result == 'FAILURE' ? 'FAILED' : 'SUCCESSFUL'

    try {
        withCredentials([string(credentialsId: 'bitbucket-build-token',
                                variable: 'SLOC_BB_TOKEN')]) {
            // Read the token as $SLOC_BB_TOKEN inside sh (masked) — a withCredentials
            // binding is not visible via ${env.SLOC_BB_TOKEN} in Groovy, so
            // interpolating it here would pass an empty token and the notifier would
            // silently treat itself as "not configured" and skip.
            withEnv(["BITBUCKET_BASE_URL=${env.BITBUCKET_BASE_URL ?: ''}",
                     "BITBUCKET_WORKSPACE=${env.BITBUCKET_WORKSPACE ?: ''}",
                     "BITBUCKET_REPO=${env.BITBUCKET_REPO ?: ''}",
                     // BITBUCKET_USER set ⇒ notify-bitbucket.sh uses Basic auth
                     // (Cloud app passwords). Blank ⇒ Bearer (access token / Server PAT).
                     "BITBUCKET_USER=${env.BITBUCKET_USER ?: ''}",
                     "GIT_COMMIT=${env.GIT_COMMIT ?: ''}",
                     "BUILD_KEY=${env.JOB_NAME ?: 'oxide-sloc'}",
                     "BUILD_NAME=oxide-sloc CI #${env.BUILD_NUMBER}",
                     "REPORT_URL=${reportUrl}"]) {
                shx "BITBUCKET_TOKEN=\"\$SLOC_BB_TOKEN\" bash ci/jenkins/notify-bitbucket.sh ${bbState} || true"
            }
        }
    } catch (Exception ex) {
        echo "Bitbucket direct notify skipped (no 'bitbucket-build-token' credential): ${ex.message}"
    }

    try {
        withCredentials([string(credentialsId: 'confluence-api-token',
                                variable: 'SLOC_CF_TOKEN')]) {
            // Same masking rule: read $SLOC_CF_TOKEN inside sh, not via Groovy env.
            withEnv(["CONFLUENCE_BASE_URL=${env.CONFLUENCE_BASE_URL ?: ''}",
                     "CONFLUENCE_USER=${env.CONFLUENCE_USER ?: ''}",
                     "CONFLUENCE_SPACE_KEY=${env.CONFLUENCE_SPACE_KEY ?: ''}",
                     "CONFLUENCE_PARENT_ID=${env.CONFLUENCE_PARENT_ID ?: ''}",
                     "CONFLUENCE_PAGE_TITLE=${env.CONFLUENCE_PAGE_TITLE ?: ''}",
                     "REPORT_URL=${reportUrl}"]) {
                shx "CONFLUENCE_TOKEN=\"\$SLOC_CF_TOKEN\" ${pyBin()} ci/jenkins/notify-confluence.py '${outDir}' '${proj}' || true"
            }
        }
    } catch (Exception ex) {
        echo "Confluence notify skipped (no 'confluence-api-token' credential): ${ex.message}"
    }
}

return this
