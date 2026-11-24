#!/usr/bin/env python3
"""Static safety guards for the discovery Job DSL (seed + template).

A Job DSL seed cannot be unit-run without a live Jenkins, so these are static
guards that lock in the security and lifecycle invariants the feature issue
requires and that are easy to silently regress during unrelated edits:

  * generated jobs only ever invoke the central job -- never embed scanner
    source, a toolchain build, tokens, or TLS-bypass flags;
  * jobs are folder-bounded and disabled (not deleted) when ineligible;
  * the seed fails closed on MAX_REPOS / MAX_BRANCHES_PER_REPO and honours
    DRY_RUN, and the nightly cron keeps its spread-load default;
  * job names are stable and folder-scoped (cross-checked against inventory.py).

Exit 0 == all guards hold.
"""

from __future__ import annotations

import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DISCOVERY = os.path.dirname(HERE)
sys.path.insert(0, DISCOVERY)

import inventory as inv  # noqa: E402

TEMPLATE = os.path.join(DISCOVERY, "job-template.groovy")
SEED = os.path.join(DISCOVERY, "seed-discovery.groovy")

PASSED = 0


def check(label: str, condition: bool) -> None:
    global PASSED
    if not condition:
        raise AssertionError(f"FAIL: {label}")
    PASSED += 1
    print(f"  ok: {label}")


def _read(path: str) -> str:
    with open(path, encoding="utf-8") as handle:
        return handle.read()


def test_template_guards() -> None:
    print("job-template.groovy guards:")
    src = _read(TEMPLATE)
    check("serializes the central scan via build job", "build job: '${central}'" in src)
    check("concurrent builds disabled", "disableConcurrentBuilds()" in src)
    check("per-run timeout present", "timeout(time:" in src)
    check("renders disabled state for ineligible entries", "disabled(!enabled)" in src)
    check("refuses jobs outside managed folder", "startsWith(cfg.managedFolder" in src)
    check("passes SCAN_REF (SHA or branch) to central", "name: 'SCAN_REF'" in src)
    # Generated jobs must NOT embed the scanner or any build logic.
    for bad in ("cargo", "oxide-sloc analyze", "BUILD_MODE", "use-prebuilt"):
        check(f"no embedded scanner logic: {bad!r}", bad not in src)
    # No secrets or TLS bypass may ever be rendered into a generated job.
    for bad in ("BITBUCKET_TOKEN", "SSL_NO_VERIFY", "sslNoVerify", "--insecure", "PRIVATE_KEY"):
        check(f"no secret / TLS-bypass token: {bad!r}", bad not in src)
    check("no hardcoded public git host", "github.com" not in src and "bitbucket.org/" not in src)
    check("no job deletion from the template", "deleteJob" not in src)


def test_seed_guards() -> None:
    print("seed-discovery.groovy guards:")
    src = _read(SEED)
    check("nightly cron keeps spread-load default", "H H(1-4) * * *" in src)
    check("MAX_REPOS enforced", "MAX_REPOS exceeded" in src)
    check("MAX_BRANCHES_PER_REPO enforced", "MAX_BRANCHES_PER_REPO exceeded" in src)
    check("limits fail closed (throw)", "throw new IllegalStateException" in src)
    check("DRY_RUN honoured", "DRY_RUN" in src and "[DRY_RUN]" in src)
    check("managed-folder escape guard", "escapes managed folder" in src)
    check("loads shared template, no duplicate scanner", "job-template.groovy" in src)
    check("never deletes jobs (disable-not-delete)", "deleteJob" not in src)


def test_no_bad_unicode_escapes() -> None:
    """Groovy unicode-preprocesses the whole source; a bare `\\u` aborts compile."""
    print("groovy unicode-escape safety:")
    bad = re.compile(r"(?<!\\)(?:\\\\)*\\u(?!u*[0-9A-Fa-f]{4})")
    for path in (TEMPLATE, SEED):
        offenders = [i + 1 for i, line in enumerate(_read(path).splitlines()) if bad.search(line)]
        check(f"no invalid \\u escape in {os.path.basename(path)}", not offenders)


def test_name_stability() -> None:
    print("job-name stability (cross-checked with inventory.py):")
    kw = dict(
        provider="bitbucket-server",
        host="git.example.invalid",
        project="PROJ",
        repo_id="101",
        repo_slug="widget",
        clone_url="https://git.example.invalid/scm/PROJ/widget.git",
        branch="release/1.2",
        credential_id="cred",
    )
    e1 = inv.make_entry(**kw)
    e2 = inv.make_entry(**kw)
    check("entry rendering is deterministic", e1 == e2)
    check(
        "job_name is folder-scoped + slash-sanitized",
        e1["job_name"] == "oxide-sloc-scans/git-example-invalid/PROJ/widget/release-1.2",
    )
    check("job_name starts with the managed folder", e1["job_name"].startswith(inv.DEFAULT_MANAGED_FOLDER + "/"))


def main() -> int:
    test_template_guards()
    test_seed_guards()
    test_no_bad_unicode_escapes()
    test_name_stability()
    print(f"\nPASS - {PASSED} checks")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
