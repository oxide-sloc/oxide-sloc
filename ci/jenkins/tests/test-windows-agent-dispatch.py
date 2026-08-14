#!/usr/bin/env python3
"""Static guards for the Windows-agent portability wiring (Problem 1).

These lock in the cross-platform shell dispatch and OS-aware environment so they
cannot silently regress during unrelated CI edits. They are static (no live
Jenkins / no Groovy compiler needed) but cover the load-bearing invariants:

  pipeline-helpers.groovy
    * resolveBash() exists, returns null on Unix, skips the WSL System32 shim,
      honours SLOC_BASH, and probes the known Git-for-Windows locations.
    * shx()/shxStdout()/shxStatus() exist and DO NOT use the fragile
      `bash -lc "<inline>"` form — they must write a temp .sh script and run it
      by path, normalising CRLF to LF.
    * initEnv() sets Unix values byte-identical to the old environment{} block,
      and on Windows PREPENDS the cache bin dir to the inherited PATH (never
      replaces it), normalises HOME<-USERPROFILE, and points BINARY/ARTIFACT_PATH
      at the .exe. Windows hardening (F4/F5/F8): cache root resolves
      SLOC_CACHE_DIR -> HOME -> USERPROFILE -> WORKSPACE (no C:\\Users\\Default),
      CARGO_HOME/RUSTUP_HOME/PATH are exported in forward-slash (POSIX) form, the
      MinGW gcc dir (<gitroot>\\mingw64\\bin) is prepended to PATH, and WS_POSIX /
      HISTORY_HOME_POSIX are exported for bash-facing path construction.
    * pyBin() provides a python3 -> python -> py (SLOC_PY override) fallback.
    * EVERY shell step goes through shx/shxStdout/shxStatus — no bare `sh '...'`
      / `sh("...")` call sites remain (the three inside the helper definitions
      themselves are the Unix branch and are allowed).

  Jenkinsfile
    * agent honours params.AGENT_LABEL; the AGENT_LABEL parameter exists.
    * no OS-dependent vars remain in a declarative environment{} block (they
      moved to initEnv()); h.initEnv() is invoked.
    * no bare `sh` steps remain — all dispatch via h.shx / h.shxStdout / etc.

Run:  python3 ci/jenkins/tests/test-windows-agent-dispatch.py
Exit code 0 = all guards hold.
"""

import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
HELPERS = os.path.join(ROOT, "ci", "jenkins", "pipeline-helpers.groovy")
JENKINSFILE = os.path.join(ROOT, "Jenkinsfile")

RESULTS = []


def check(name, cond, detail=""):
    RESULTS.append((name, bool(cond), detail))


def read(path):
    with open(path, encoding="utf-8") as fh:
        return fh.read()


def strip_line_comments(src):
    """Drop /* */ block comments and // line comments so call-site scans don't
    trip on prose. Coarse but sufficient: our positive checks look for code
    tokens and the negative `sh` scan runs against this stripped text. Newlines
    inside block comments are preserved so reported line numbers stay accurate."""
    # Strip /* ... */ block comments, preserving newline count for line numbers.
    def _blank(m):
        return "".join(ch if ch == "\n" else " " for ch in m.group(0))
    src = re.sub(r"/\*.*?\*/", _blank, src, flags=re.DOTALL)
    out = []
    for line in src.splitlines():
        idx = line.find("//")
        out.append(line[:idx] if idx != -1 else line)
    return "\n".join(out)


def _check_resolve_bash(src):
    check("helpers: resolveBash() defined", "def resolveBash()" in src)
    check("helpers: resolveBash returns null on Unix",
          re.search(r"def resolveBash\(\)\s*\{\s*if \(isUnix\(\)\)\s*\{\s*return null",
                    src) is not None)
    check("helpers: resolveBash honours SLOC_BASH override",
          "env.SLOC_BASH" in src)
    check("helpers: resolveBash skips the WSL System32 shim",
          "system32" in src.lower() and "where bash" in src.lower())
    # resolveBash builds candidate paths from Git/PortableGit folder ROOTS via a
    # bashesUnder() helper (<root>\bin\bash.exe + <root>\usr\bin\bash.exe), so the
    # probe list is no longer one literal string. Verify: the system Git root is
    # probed, the <root>\bin\bash.exe layout is derived, and the no-admin
    # fallbacks (per-user LocalAppData + a staged PortableGit folder) are covered.
    check("helpers: resolveBash probes known Git-for-Windows paths",
          r"Program Files\\Git" in src
          and (r"\\bin\\bash.exe" in src or r"\\usr\\bin\\bash.exe" in src))
    check("helpers: resolveBash offers a no-install PortableGit path",
          "SLOC_PORTABLE_GIT" in src and "PortableGit" in src,
          "must honour SLOC_PORTABLE_GIT and probe a staged PortableGit folder")
    check("helpers: resolveBash auto-detects the per-user (no-admin) Git install",
          "LOCALAPPDATA" in src,
          "must probe %LOCALAPPDATA%\\Programs\\Git (per-user install, no admin)")


def _check_shx(src, code):
    check("helpers: shx() defined", "def shx(String cmd)" in src)
    check("helpers: shxStdout() defined", "def shxStdout(String cmd)" in src)
    check("helpers: shxStatus() defined", "def shxStatus(String cmd)" in src)
    check("helpers: noBashError() actionable message mentions AGENT_LABEL / SLOC_BASH",
          "AGENT_LABEL" in src and "SLOC_BASH" in src)

    # CRITICAL: shx must NOT use the fragile `bash -lc "<inline>"` form. Scan the
    # comment-stripped code for an actual invocation (`-lc "` / `-lc \"`), so the
    # header comment that *documents* the ban does not trip the guard.
    lc_hits = re.findall(r'-lc\s*[\\"]', code)
    check("helpers: shx does NOT use `bash -lc` inline form",
          not lc_hits,
          "found a `-lc \"...\"` invocation — must write a temp script and run by path")
    # shx must write a temp script (writeFile) and run it by path.
    check("helpers: shx writes a temp script via writeFile",
          "writeFile" in src and "writeShxScript" in src)
    check("helpers: temp-script uniqueness derived from a counter + BUILD_NUMBER",
          "shxCounter" in src and "BUILD_NUMBER" in src)
    check("helpers: temp-script CRLF normalised to LF before writeFile",
          ".replace('\\r\\n', '\\n')" in src or ".replace('\\r', '\\n')" in src)
    check("helpers: temp-script cleaned up in a finally",
          "finally" in src and "del /q" in src)


def _check_init_env(src, code):
    check("helpers: initEnv() defined", "def initEnv()" in src)
    # Unix values must be byte-identical to the old environment{} block.
    check("helpers: initEnv Unix PATH byte-identical to old environment block",
          '"${home}/.rust-cache/cargo/bin:/usr/local/bin:/usr/bin:/bin"' in src)
    check("helpers: initEnv Unix BINARY has no .exe",
          '"${env.WORKSPACE}/target/release/oxide-sloc"' in src)
    # F4: Windows cache root resolution must prefer SLOC_CACHE_DIR, then
    # HOME -> USERPROFILE -> WORKSPACE, and MUST NOT use the read-only
    # 'C:\\Users\\Default' fallback anymore.
    check("helpers: initEnv Windows honours SLOC_CACHE_DIR override",
          "env.SLOC_CACHE_DIR" in src)
    # Scan comment-stripped code so the explanatory comment that *documents* the
    # removal (it names the old path) does not trip the guard.
    check("helpers: initEnv Windows falls back to WORKSPACE (writable), not Default",
          "C:\\\\Users\\\\Default" not in code,
          "the read-only 'C:\\Users\\Default' cache fallback must be removed")
    check("helpers: initEnv normalises HOME<-USERPROFILE on Windows",
          "env.USERPROFILE" in src)
    # F4: CARGO_HOME/RUSTUP_HOME/PATH must be exported in FORWARD-SLASH (POSIX) form
    # on Windows so bash consumers (check-disk-space.sh du/find) don't choke.
    check("helpers: initEnv Windows CARGO_HOME is POSIX (forward-slash) form",
          '.rust-cache/cargo"' in src and 'cacheRootPosix' in src,
          "Windows CARGO_HOME must be '<root>/.rust-cache/cargo' (forward slashes)")
    check("helpers: initEnv Windows PATH prepends cache bin in POSIX form, keeps inherited",
          re.search(r"env\.PATH\s*=\s*\"\$\{cacheRootPosix\}/\.rust-cache/cargo/bin;.*\$\{env\.PATH\}\"",
                    src) is not None,
          "Windows PATH must prepend '<root>/.rust-cache/cargo/bin;...;${env.PATH}'")
    check("helpers: initEnv Windows BINARY carries .exe",
          "oxide-sloc.exe" in src)
    # F5: MinGW gcc/ld must be prepended to PATH (derived from resolveBash's Git root)
    # so the non-login cargo build finds cc/gcc/ld for the windows-gnu target.
    check("helpers: initEnv Windows prepends MinGW gcc dir (mingw64\\bin) to PATH",
          "mingw64\\\\bin" in src and "resolveBash()" in src,
          "must prepend <gitroot>\\mingw64\\bin (and usr\\bin) derived from resolveBash()")
    # F8: POSIX-normalised WORKSPACE / history-home exported for bash-facing paths.
    check("helpers: initEnv exports WS_POSIX for bash-facing path construction",
          "env.WS_POSIX" in src)
    check("helpers: initEnv exports a POSIX history-home",
          "env.HISTORY_HOME_POSIX" in src)


def _check_py_bin(src):
    check("helpers: pyBin() defined with python3->python->py fallback",
          "def pyBin()" in src and "command -v python3" in src
          and "command -v python " in src and "command -v py" in src)
    check("helpers: pyBin honours SLOC_PY override", "env.SLOC_PY" in src)


def _find_bare_sh(code):
    # The only allowed `sh`/`sh(` tokens are the Unix branch inside
    # shx/shxStdout/shxStatus.
    bare = []
    for i, line in enumerate(code.splitlines(), 1):
        s = line.strip()
        if s == "sh cmd":
            continue  # shx() Unix branch
        if s.startswith("return sh(returnStdout: true, script: cmd)"):
            continue  # shxStdout() Unix branch
        if s.startswith("return sh(returnStatus: true, script: cmd)"):
            continue  # shxStatus() Unix branch
        if re.search(r"(^|[^\w.])sh\s+['\"]", line) or re.search(r"(^|[^\w.])sh\s*\(", line):
            bare.append((i, s))
    return bare


def _check_call_sites(code):
    # No bare `sh` call sites outside the helper definitions.
    bare = _find_bare_sh(code)
    check("helpers: no bare `sh` call sites remain (all via shx/shxStdout/shxStatus)",
          not bare,
          "; ".join(f"L{n}:{t[:50]}" for n, t in bare))

    # No hardcoded `python3 ci/` invocations — those must go through pyBin().
    py_hardcodes = [(i, l.strip()) for i, l in enumerate(code.splitlines(), 1)
                    if "python3 ci/" in l]
    check("helpers: no hardcoded `python3 ci/...` (all via ${pyBin()})",
          not py_hardcodes,
          "; ".join(f"L{n}" for n, _ in py_hardcodes))


def test_helpers():
    src = read(HELPERS)
    code = strip_line_comments(src)
    _check_resolve_bash(src)
    _check_shx(src, code)
    _check_init_env(src, code)
    _check_py_bin(src)
    _check_call_sites(code)


def test_jenkinsfile():
    src = read(JENKINSFILE)
    code = strip_line_comments(src)

    check("Jenkinsfile: AGENT_LABEL parameter exists",
          re.search(r"name:\s*'AGENT_LABEL'", src) is not None)
    check("Jenkinsfile: agent honours params.AGENT_LABEL",
          re.search(r"agent\s*\{\s*label\s+\"\$\{params\.AGENT_LABEL", src) is not None)

    # The old OS-dependent environment{} vars must be gone (moved to initEnv()).
    check("Jenkinsfile: no POSIX-only PATH in a declarative environment block",
          "/usr/local/bin:/usr/bin:/bin" not in code,
          "the OS-dependent PATH must live in initEnv(), not environment{}")
    check("Jenkinsfile: CARGO_HOME no longer set in environment{}",
          not re.search(r"^\s*CARGO_HOME\s*=", code, re.MULTILINE))
    check("Jenkinsfile: h.initEnv() is invoked", "h.initEnv()" in src)

    # No bare `sh` steps — everything dispatches through the helper.
    bare = [(i, l.strip()) for i, l in enumerate(code.splitlines(), 1)
            if re.search(r"(^|[^\w.])sh\s+['\"]", l) or re.search(r"(^|[^\w.])sh\s*\(", l)]
    check("Jenkinsfile: no bare `sh` steps remain (all via h.shx/h.shxStdout/etc.)",
          not bare,
          "; ".join(f"L{n}:{t[:50]}" for n, t in bare))

    # F3: timestamps() is a parse-time Timestamper plugin hard-dep — must be gone
    # from options{} (a missing options directive can't be try/catch'd, like ansiColor).
    check("Jenkinsfile: timestamps() removed from options{} (parse-time plugin dep)",
          not re.search(r"^\s*timestamps\(\)", code, re.MULTILINE),
          "timestamps() must be removed for minimal-controller compatibility")

    # F5: the source build must unset CC (and CXX) so the airgap-devkit gcc can't
    # conflict with the MinGW gcc the windows-gnu target links with. The build shx
    # body now lives in runBuild() in pipeline-helpers.groovy (the Jenkinsfile Build
    # stage just delegates to h.runBuild()), so assert the invariant there.
    helpers_src = read(HELPERS)
    check("helpers: runBuild source path unsets CC/CXX",
          re.search(r"unset\s+CC\s+CXX", helpers_src) is not None,
          "the source-build shx body in runBuild() must start with `unset CC CXX`")

    # F7: cleanWs() must catch Throwable (a missing ws-cleanup plugin throws
    # NoSuchMethodError, a java.lang.Error, which catch(Exception) would let escape).
    check("Jenkinsfile: cleanWs() catches Throwable, not Exception",
          re.search(r"cleanWs\(\)\s*\n\s*\}\s*catch\s*\(\s*Throwable", code) is not None,
          "cleanWs must be guarded by catch (Throwable ...)")


def main():
    test_helpers()
    test_jenkinsfile()

    print("=" * 82)
    print("Jenkins Windows-agent portability guards (resolveBash / shx / initEnv)")
    print("=" * 82)
    all_ok = True
    for name, ok, detail in RESULTS:
        all_ok = all_ok and ok
        line = f"[{'PASS' if ok else 'FAIL'}] {name}"
        if not ok and detail:
            line += f"\n        {detail}"
        print(line)
    print("-" * 82)
    print("OVERALL:", "ALL PASSED" if all_ok else "SOME FAILED")
    sys.exit(0 if all_ok else 1)


if __name__ == "__main__":
    main()
