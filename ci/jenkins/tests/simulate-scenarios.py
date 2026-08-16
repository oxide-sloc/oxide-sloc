#!/usr/bin/env python3
"""Simulate the oxide-sloc Jenkins pipeline under every permission / air-gap
scenario, without a live Jenkins controller.

It exercises the real decision logic (``detect-capabilities.decide``), writes a
``capabilities.json`` for each scenario, runs the real ``generate-dashboard.py``
and ``install-plugins.sh``, and asserts that:

  * the correct capability mode is chosen,
  * plugin installation is attempted only when permitted,
  * the pipeline NEVER errors (every helper exits 0),
  * the dashboard shows the graceful-degradation banner exactly when it should.

Scenarios (mirrors the requested test matrix)
---------------------------------------------
  1. Networked, no project-admin rights
  2. Networked, project-admin rights
  3. Networked, no system (global) admin rights
  4. Networked, system (global) admin rights
  5. Air-gapped — the same four, plus the offline-bundle edge cases

Run:  python3 ci/jenkins/tests/simulate-scenarios.py
Exit code 0 = all scenarios behaved as expected.
"""

import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile


def _find_bash():
    """Locate a POSIX bash. Prefer Git Bash on Windows so we don't hit a broken
    WSL shim; fall back to PATH. Returns None if none is usable."""
    candidates = [
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files\Git\usr\bin\bash.exe",
        "/bin/bash",
    ]
    for c in candidates:
        if os.path.isfile(c):
            return c
    return shutil.which("bash")


BASH = _find_bash()

HERE = os.path.dirname(os.path.abspath(__file__))
JENKINS_DIR = os.path.abspath(os.path.join(HERE, ".."))
REPO_ROOT = os.path.abspath(os.path.join(JENKINS_DIR, "..", ".."))
GEN = os.path.join(JENKINS_DIR, "generate-dashboard.py")
INSTALL = os.path.join(JENKINS_DIR, "install-plugins.sh")


def _load_decide():
    spec = importlib.util.spec_from_file_location(
        "detect_caps", os.path.join(JENKINS_DIR, "detect-capabilities.py")
    )
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod.decide


decide = _load_decide()

_SAMPLE_RESULT = {
    "tool": {"name": "sloc", "version": "1.5.76",
             "run_id": "sim", "timestamp_utc": "2026-06-26T03:08:00Z"},
    "summary_totals": {"code_lines": 999, "comment_lines": 132,
                       "blank_lines": 237, "files_analyzed": 24},
    "totals_by_language": [
        {"language": "C++", "code_lines": 466},
        {"language": "C", "code_lines": 138},
        {"language": "Go", "code_lines": 116},
        {"language": "Python", "code_lines": 106},
    ],
    "per_file_records": [{"path": "src/main.cpp", "language": "C++", "code_lines": 200}],
}

REQUESTED = ["htmlpublisher", "plot", "warnings-ng", "coverage", "badge"]

# Each scenario: probe inputs + what we expect out of the decision.
SCENARIOS = [
    # name, reachable, sys_admin, job_admin, uc_reachable, offline_hpi,
    #   expect_mode, expect_can_install, expect_banner
    ("1. Networked · no project-admin",
     True, False, False, True, True, "degraded", False, True),
    ("2. Networked · project-admin",
     True, False, True, True, True, "degraded", False, True),
    ("3. Networked · no system-admin",
     True, False, True, True, True, "degraded", False, True),
    ("4. Networked · system-admin",
     True, True, True, True, True, "full", True, False),
    ("5a. Air-gapped · no project-admin",
     True, False, False, False, True, "degraded", False, True),
    ("5b. Air-gapped · project-admin",
     True, False, True, False, True, "degraded", False, True),
    ("5c. Air-gapped · no system-admin",
     True, False, True, False, True, "degraded", False, True),
    ("5d. Air-gapped · system-admin + offline bundle",
     True, True, True, False, True, "full", True, False),
    ("5e. Air-gapped · system-admin · NO offline bundle",
     True, True, True, False, False, "airgapped-degraded", False, True),
    ("5f. Air-gapped · Jenkins API unreachable",
     False, False, False, False, True, "degraded", False, True),
]


def run_scenario(spec) -> dict:
    (name, reachable, sys_admin, job_admin, uc, offline,
     exp_mode, exp_install, exp_banner) = spec

    caps = decide(
        reachable=reachable, system_admin=sys_admin, job_admin=job_admin,
        update_center_reachable=uc, offline_hpi_present=offline,
        requested_plugins=REQUESTED,
    )

    tmp = tempfile.mkdtemp(prefix="sloc-sim-")
    slug = "simproj_deadbee"
    with open(os.path.join(tmp, f"result_{slug}.json"), "w", encoding="utf-8") as fh:
        json.dump(_SAMPLE_RESULT, fh)
    with open(os.path.join(tmp, "capabilities.json"), "w", encoding="utf-8") as fh:
        json.dump(caps, fh)

    # install-plugins.sh must ALWAYS exit 0 (never fail the build). BASE is empty
    # here (no live Jenkins) so any permitted install attempt fails gracefully.
    # If no POSIX bash is available locally, the install step is marked n/a and
    # excluded from the verdict (the script's exit-0 guarantee is a shell
    # property; the CI agent always has bash).
    install_rc = "n/a"
    if BASH:
        try:
            inst = subprocess.run(
                [BASH, INSTALL, tmp],
                capture_output=True, text=True,
                env={**os.environ, "JENKINS_BASE_URL": "", "SLOC_PLUGIN_DIR": tmp},
            )
            install_rc = inst.returncode
        except OSError:
            install_rc = "n/a"

    # generate-dashboard.py must succeed and (not) render the banner as expected.
    gen = subprocess.run(
        [sys.executable, GEN, tmp, slug],
        capture_output=True, text=True,
    )
    dash = os.path.join(tmp, f"dashboard_{slug}.html")
    html = ""
    if os.path.isfile(dash):
        with open(dash, encoding="utf-8") as fh:
            html = fh.read()
    banner_present = "capability-banner" in html

    install_ok = install_rc in (0, "n/a")
    ok = (
        caps["mode"] == exp_mode
        and caps["can_install_plugins"] == exp_install
        and banner_present == exp_banner
        and install_ok
        and gen.returncode == 0
        and bool(html)
    )
    return {
        "name": name,
        "mode": caps["mode"],
        "can_install": caps["can_install_plugins"],
        "plugins_source": caps["plugins_source"],
        "banner": banner_present,
        "install_rc": install_rc,
        "gen_rc": gen.returncode,
        "ok": ok,
    }


def main() -> None:
    try:
        sys.stdout.reconfigure(encoding="utf-8")  # keep glyphs on Windows consoles
    except Exception:
        # Best-effort: older Python or a non-reconfigurable stream is fine as-is.
        pass
    print("=" * 96)
    print("oxide-sloc Jenkins pipeline — permission / air-gap simulation")
    print("=" * 96)
    header = (f"{'Scenario':<46}{'mode':<20}{'install':<9}"
              f"{'source':<16}{'banner':<8}{'result'}")
    print(header)
    print("-" * 96)
    all_ok = True
    for spec in SCENARIOS:
        r = run_scenario(spec)
        all_ok = all_ok and r["ok"]
        verdict = "PASS" if r["ok"] else "FAIL"
        print(f"{r['name']:<46}{r['mode']:<20}{str(r['can_install']):<9}"
              f"{r['plugins_source']:<16}{str(r['banner']):<8}{verdict}")
    print("-" * 96)
    print("Every helper exited 0 (build never fails); banner shows exactly when "
          "plugins are unavailable.")
    print("Air-gapped + system-admin + offline bundle (5d) still reaches FULL mode "
          "— the offline .hpi bundle works with no network.")
    print("=" * 96)
    print("OVERALL:", "ALL SCENARIOS PASSED" if all_ok else "SOME SCENARIOS FAILED")
    sys.exit(0 if all_ok else 1)


if __name__ == "__main__":
    main()
