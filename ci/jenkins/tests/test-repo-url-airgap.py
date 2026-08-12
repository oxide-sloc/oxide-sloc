#!/usr/bin/env python3
"""Air-gap resilience guards for the Jenkins tooling-repo (REPO_URL) wiring.

The oxide-sloc Jenkins pipeline must never assume an internet URL. On an
air-gapped controller the job points at a LOCAL MIRROR and a no-parameter build
has to succeed without ever touching github.com. These static + functional guards
lock that in so it cannot silently regress during unrelated CI edits:

  1. Jenkinsfile — the REPO_URL build parameter has an EMPTY default (no baked-in
     internet URL) and the Checkout stage resolves
     param -> env.REPO_URL -> `checkout scm` (the job's own SCM). REPO_BRANCH is
     resolvable the same way.
  2. seed-job.groovy — no hardcoded github fallback; REPO_URL is REQUIRED (the
     script throws when it is unset) and the branch comes from a repoBranch var.
  3. render-job-config.sh — no silent github default; it ERRORS when REPO_URL is
     unset and substitutes both __REPO_URL__ and __REPO_BRANCH__.
  4. job-config.xml + job-config.xml.tmpl — carry the __REPO_URL__ / __REPO_BRANCH__
     placeholders and contain NO hardcoded oxide-sloc github URL.
  5. Functional: render-job-config.sh actually exits non-zero with REPO_URL unset,
     and renders a placeholder-free XML (with the given URL + branch) when set.
     Skipped only if `bash` is unavailable on this host.

Run:  python3 ci/jenkins/tests/test-repo-url-airgap.py
Exit code 0 = all guards hold.
"""

import os
import re
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
JENKINSFILE = os.path.join(ROOT, "Jenkinsfile")
SEED = os.path.join(ROOT, "ci", "jenkins", "seed-job.groovy")
RENDER = os.path.join(ROOT, "ci", "jenkins", "render-job-config.sh")
JOB_XML = os.path.join(ROOT, "ci", "jenkins", "job-config.xml")
JOB_TMPL = os.path.join(ROOT, "ci", "jenkins", "job-config.xml.tmpl")

# Any occurrence of the upstream github repo as a functional default is a
# regression — an air-gapped build must not depend on it.
GITHUB_HARDCODE = re.compile(r"github\.com/oxide-sloc/oxide-sloc")

RESULTS = []


def check(name, cond, detail=""):
    RESULTS.append((name, bool(cond), detail))


def read(path):
    with open(path, encoding="utf-8") as fh:
        return fh.read()


def test_jenkinsfile():
    src = read(JENKINSFILE)

    # The TOOL_REPO_URL parameter block (tooling repo) must default to an empty string.
    m = re.search(
        r"name:\s*'TOOL_REPO_URL'.*?defaultValue:\s*('([^']*)'|\"([^\"]*)\")",
        src, re.DOTALL)
    default = (m.group(2) or m.group(3) or "") if m else "<no TOOL_REPO_URL param>"
    check("Jenkinsfile: TOOL_REPO_URL param default is empty",
          m is not None and default == "",
          f"defaultValue was: {default!r}")

    # A TOOL_REPO_BRANCH parameter must exist (so */main is no longer hardcoded-only).
    check("Jenkinsfile: TOOL_REPO_BRANCH param exists",
          re.search(r"name:\s*'TOOL_REPO_BRANCH'", src) is not None)

    # Checkout resolves the env override (new TOOL_REPO_URL, with legacy REPO_URL
    # accepted as a back-compat fallback) and falls back to the job's own SCM.
    check("Jenkinsfile: Checkout reads env.TOOL_REPO_URL", "env.TOOL_REPO_URL" in src)
    check("Jenkinsfile: Checkout keeps legacy env.REPO_URL fallback", "env.REPO_URL" in src)
    check("Jenkinsfile: Checkout falls back to `checkout scm`",
          re.search(r"checkout\s+scm", src) is not None)

    # No hardcoded upstream github URL anywhere in the pipeline.
    check("Jenkinsfile: no hardcoded github.com/oxide-sloc",
          GITHUB_HARDCODE.search(src) is None,
          "found github.com/oxide-sloc in Jenkinsfile")


def test_seed():
    src = read(SEED)
    check("seed-job.groovy: no hardcoded github.com/oxide-sloc default",
          GITHUB_HARDCODE.search(src) is None,
          "found github.com/oxide-sloc in seed-job.groovy")
    # REPO_URL is required — the script throws when unset.
    check("seed-job.groovy: REPO_URL is required (throws when unset)",
          "throw new IllegalStateException" in src and "repoUrl?.trim()" in src)
    # Branch comes from a variable, not a bare literal.
    check("seed-job.groovy: branch() uses repoBranch variable",
          "branch(repoBranch)" in src)


def test_render():
    src = read(RENDER)
    check("render-job-config.sh: no silent github default",
          GITHUB_HARDCODE.search(src) is None,
          "found github.com/oxide-sloc in render-job-config.sh")
    check("render-job-config.sh: errors when REPO_URL is unset",
          'if [ -z "${REPO_URL:-}" ]; then' in src and "exit 1" in src)
    check("render-job-config.sh: substitutes __REPO_BRANCH__",
          "__REPO_BRANCH__" in src)


def test_job_xml():
    for label, path in [("job-config.xml", JOB_XML),
                        ("job-config.xml.tmpl", JOB_TMPL)]:
        src = read(path)
        check(f"{label}: uses __REPO_URL__ placeholder", "__REPO_URL__" in src)
        check(f"{label}: uses __REPO_BRANCH__ placeholder", "__REPO_BRANCH__" in src)
        check(f"{label}: no hardcoded github.com/oxide-sloc",
              GITHUB_HARDCODE.search(src) is None,
              f"found github.com/oxide-sloc in {label}")


def test_render_functional():
    """Actually run render-job-config.sh with and without REPO_URL."""
    bash = shutil.which("bash")
    if not bash:
        check("render-job-config.sh: functional run (SKIPPED — no bash)", True,
              "bash not on PATH; static checks still enforce the invariants")
        return

    # Isolate from any operator env / .env so the "unset" case is honest.
    base_env = {k: v for k, v in os.environ.items()
                if k not in ("REPO_URL", "REPO_BRANCH", "OXIDE_SLOC_ENV_FILE")}

    # (a) Unset REPO_URL must fail (exit != 0) and print no path on stdout.
    unset = subprocess.run([bash, RENDER], cwd=ROOT, env=base_env,
                           capture_output=True, text=True)
    check("render-job-config.sh: exits non-zero when REPO_URL unset",
          unset.returncode != 0,
          f"exit={unset.returncode}, stdout={unset.stdout!r}")
    check("render-job-config.sh: guidance mentions air-gap mirror on failure",
          "air-gapped local mirror" in unset.stderr or "file:///" in unset.stderr,
          f"stderr={unset.stderr!r}")

    # (b) With REPO_URL + REPO_BRANCH set, render a placeholder-free XML.
    url = "file:///srv/git/oxide-sloc.git"
    branch = "*/release"
    set_env = dict(base_env, REPO_URL=url, REPO_BRANCH=branch)
    done = subprocess.run([bash, RENDER], cwd=ROOT, env=set_env,
                          capture_output=True, text=True)
    check("render-job-config.sh: exits 0 when REPO_URL set",
          done.returncode == 0,
          f"exit={done.returncode}, stderr={done.stderr!r}")

    out_path = done.stdout.strip().splitlines()[-1] if done.stdout.strip() else ""
    rendered = ""
    if out_path:
        # Read through bash so a POSIX mktemp path (e.g. /tmp/... under git-bash on
        # Windows) resolves the same way it was written, regardless of which python
        # interpreter runs this test.
        cat = subprocess.run([bash, "-c", f"cat '{out_path}' && rm -f '{out_path}'"],
                             cwd=ROOT, env=set_env, capture_output=True, text=True)
        if cat.returncode == 0:
            rendered = cat.stdout
    check("render-job-config.sh: emits a rendered file path", bool(rendered),
          f"stdout path was: {out_path!r}")
    if rendered:
        check("render-job-config.sh: rendered URL substituted", url in rendered)
        check("render-job-config.sh: rendered branch substituted", branch in rendered)
        check("render-job-config.sh: no unsubstituted placeholders remain",
              "__REPO_URL__" not in rendered and "__REPO_BRANCH__" not in rendered)


def main():
    test_jenkinsfile()
    test_seed()
    test_render()
    test_job_xml()
    test_render_functional()

    print("=" * 78)
    print("Jenkins REPO_URL air-gap resilience guards")
    print("=" * 78)
    all_ok = True
    for name, ok, detail in RESULTS:
        all_ok = all_ok and ok
        line = f"[{'PASS' if ok else 'FAIL'}] {name}"
        if not ok and detail:
            line += f"\n        {detail}"
        print(line)
    print("-" * 78)
    print("OVERALL:", "ALL PASSED" if all_ok else "SOME FAILED")
    sys.exit(0 if all_ok else 1)


if __name__ == "__main__":
    main()
