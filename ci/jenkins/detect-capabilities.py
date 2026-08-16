#!/usr/bin/env python3
"""Detect what an oxide-sloc CI build is *allowed* to do on this Jenkins.

Plugin installation is a system-level operation gated by Jenkins' own
authorization (Overall/Administer), which a SAML / Keycloak / SSO deployment
maps roles onto.  Job-level publishing only needs Job/Configure.  This script
probes the Jenkins REST API with the build's own credentials, decides which
capabilities are available, and writes ``capabilities.json`` into the output
directory.  The pipeline reads that file to decide whether to install the
richer visualization plugins, and the dashboard reads it to show a friendly
warning banner when features had to be skipped — never an error.

Everything here is best-effort: any failure downgrades capabilities rather than
failing the build.  Standard library only (urllib), so it runs on an air-gapped
controller with no pip installs.

Usage
-----
    python3 ci/jenkins/detect-capabilities.py <output-dir>

Environment variables
----------------------
JENKINS_BASE_URL   Base URL of the controller (e.g. http://ci:8080).  If unset
                   it is derived from BUILD_URL by stripping the /job/... tail.
JENKINS_USER       Username for API auth (optional; anonymous if unset).
JENKINS_AUTH_TOKEN API token / password for API auth (optional).
JOB_NAME           Jenkins job name, used for the job-level permission probe.
SLOC_PLUGIN_DIR    Directory holding offline .hpi bundles
                   (default: ci/resources/plugins).
SLOC_REQUIRED_PLUGINS  Comma/space list overriding the manifest's plugin ids.
"""

import base64
import json
import os
import sys
import urllib.error
import urllib.request
from typing import Optional

# Visualization / reporting plugins that meaningfully enrich the SLOC report.
# The canonical manifest (ci/jenkins/plugins.txt) also lists core pipeline
# plugins (workflow-aggregator, git, ...) that are always required; those are
# NOT "enrichment" and are excluded so the degradation banner only advertises
# plugins that actually add richer views.
_ENRICHMENT_PLUGINS = [
    "htmlpublisher",
    "plot",
    "warnings-ng",
    "junit",
    "coverage",
    "badge",
    "dashboard-view",
    "build-monitor-plugin",
    "bitbucket-build-status-notifier",
]

_PROBE_TIMEOUT = 6  # seconds — short so an unreachable controller degrades fast


def _repo_root() -> str:
    here = os.path.dirname(os.path.abspath(__file__))
    return os.path.abspath(os.path.join(here, "..", ".."))


def _manifest_path() -> str:
    """Canonical Jenkins plugin manifest (shared with bundle-jenkins-plugins.sh)."""
    return os.environ.get(
        "SLOC_PLUGIN_MANIFEST", os.path.join(_repo_root(), "ci", "jenkins", "plugins.txt")
    )


def read_manifest(_unused: Optional[str] = None) -> list:
    """Return the enrichment plugin short-names declared in ci/jenkins/plugins.txt.

    The manifest lists ``id  # trailing comment`` (and section-header comment
    lines), so we strip inline comments and take the first token, then intersect
    with the enrichment set. Falls back to the enrichment set if unreadable.
    """
    override = os.environ.get("SLOC_REQUIRED_PLUGINS", "").replace(",", " ").split()
    if override:
        return [p.split(":")[0].strip() for p in override if p.strip()]
    declared = set()
    try:
        with open(_manifest_path(), encoding="utf-8") as fh:
            for line in fh:
                line = line.split("#", 1)[0].strip()  # drop inline/whole-line comments
                if not line:
                    continue
                declared.add(line.split()[0].split(":")[0].strip())
    except OSError:
        return list(_ENRICHMENT_PLUGINS)
    ordered = [p for p in _ENRICHMENT_PLUGINS if p in declared]
    return ordered or list(_ENRICHMENT_PLUGINS)


def offline_plugins_available(_unused: Optional[str] = None) -> bool:
    """True if the committed offline plugin bundle exists (either the
    jenkins-plugins.tar.xz archive at the repo root, or loose .hpi files in
    ci/jenkins/plugins/). Mirrors install-jenkins-plugins.sh's two source modes."""
    root = _repo_root()
    if os.path.isfile(os.path.join(root, "jenkins-plugins.tar.xz")):
        return True
    loose = os.environ.get("SLOC_PLUGIN_DIR", os.path.join(root, "ci", "jenkins", "plugins"))
    try:
        return any(n.endswith((".hpi", ".jpi")) for n in os.listdir(loose))
    except OSError:
        return False


def probe_http(url: str, user: str, token: str, timeout: int = _PROBE_TIMEOUT):
    """Return the HTTP status code for a GET, or None on a connection error.

    A 200/403 tells us about *permission*; None tells us about *reachability*
    (an air-gapped or offline controller).  4xx that isn't 403 is treated as
    'reachable but not permitted'.
    """
    req = urllib.request.Request(url, method="GET")
    if user and token:
        raw = f"{user}:{token}".encode("utf-8")
        req.add_header("Authorization", "Basic " + base64.b64encode(raw).decode("ascii"))
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:  # nosec B310 - constructed http(s) probe URL; other/unreachable schemes handled by except below
            return resp.status
    except urllib.error.HTTPError as e:
        return e.code
    except (OSError, ValueError):
        # urllib.error.URLError is a subclass of OSError, so it is caught here too.
        return None


# ---------------------------------------------------------------------------
# Pure decision logic — separated so the simulation harness can exercise every
# permission / air-gap scenario without a live controller.
# ---------------------------------------------------------------------------

def decide(
    *,
    reachable: bool,
    system_admin: bool,
    job_admin: bool,
    update_center_reachable: bool,
    offline_hpi_present: bool,
    requested_plugins: list,
) -> dict:
    """Turn raw probe results into a capability + degradation decision.

    Rules
    -----
    * Installing plugins requires system (Overall/Administer) permission.
    * With that permission, plugins can be installed if EITHER the update
      centre is reachable OR an offline .hpi bundle is present — the latter is
      the whole point of committing plugins for air-gapped controllers.
    * Anything less degrades to the native (plugin-free) dashboard, which is
      always fully functional. Job-admin still lets us publish reports and set
      the build description.
    """
    if update_center_reachable:
        plugins_source = "update-center"
    elif offline_hpi_present:
        plugins_source = "offline-bundle"
    else:
        plugins_source = "none"
    can_install_plugins = bool(
        reachable and system_admin and plugins_source != "none"
    )

    if not reachable:
        mode = "degraded"
        reason = (
            "The Jenkins API could not be reached with the build credentials, so "
            "elevated capabilities were assumed unavailable. The native oxide-sloc "
            "dashboard is shown; no functionality is lost."
        )
    elif can_install_plugins:
        mode = "full"
        reason = ""
    elif not system_admin:
        scope = "project-admin" if job_admin else "read-only"
        mode = "degraded"
        reason = (
            "The build's Jenkins API token lacks Overall/Administer permission "
            "(or is empty / not configured), which is required to install "
            "controller plugins. Richer visualization plugins were skipped and the "
            "native oxide-sloc dashboard is shown instead. Make sure the "
            "'jenkins-api-token' credential holds a valid admin token — or, if the "
            "plugins are already installed on the controller, this is only about "
            "the token, not the plugins."
            + (" This account does have project (Job/Configure) rights."
               if job_admin else "")
            + f" (detected API scope: {scope})"
        )
    else:  # system_admin but no way to obtain the plugins
        mode = "airgapped-degraded"
        reason = (
            "This build has system permission, but the plugin update centre is "
            "unreachable (air-gapped) and no offline plugin bundle was found under "
            "ci/resources/plugins. Commit the .hpi bundle (see that folder's README) "
            "to enable the enhanced plugins offline. The native dashboard is shown."
        )

    return {
        "reachable": reachable,
        "system_admin": system_admin,
        "job_admin": job_admin,
        "update_center_reachable": update_center_reachable,
        "offline_hpi_present": offline_hpi_present,
        "plugins_source": plugins_source,
        "can_install_plugins": can_install_plugins,
        "requested_plugins": list(requested_plugins),
        # Populated by install-plugins.sh after an install attempt.
        "installed_plugins": [],
        "mode": mode,
        "degraded": mode != "full",
        "degraded_reason": reason,
    }


def assess() -> dict:
    """Probe the live controller and return the capability decision."""
    base = os.environ.get("JENKINS_BASE_URL", "").rstrip("/")
    if not base:
        build_url = os.environ.get("BUILD_URL", "")
        base = build_url.split("/job/")[0].rstrip("/") if build_url else ""
    user = os.environ.get("JENKINS_USER", "")
    token = os.environ.get("JENKINS_AUTH_TOKEN", "")
    job = os.environ.get("JOB_NAME", "")
    requested = read_manifest()
    offline_present = offline_plugins_available()

    if not base:
        # No controller URL at all — treat as unreachable and degrade cleanly.
        return decide(
            reachable=False, system_admin=False, job_admin=False,
            update_center_reachable=False, offline_hpi_present=offline_present,
            requested_plugins=requested,
        )

    # /me/api/json → reachability + authenticated identity.
    me = probe_http(f"{base}/me/api/json", user, token)
    reachable = me is not None
    # /pluginManager/api/json requires Overall/Administer → system admin probe.
    pm = probe_http(f"{base}/pluginManager/api/json?depth=0", user, token)
    system_admin = pm == 200
    # Job config read requires Job/Configure → project-admin probe.
    job_admin = False
    if job:
        job_path = "/job/".join([""] + job.split("/"))  # folder-safe
        cfg = probe_http(f"{base}{job_path}/config.xml", user, token)
        job_admin = cfg == 200
    # Update-centre reachability: the controller's own UC metadata endpoint.
    uc = probe_http(f"{base}/updateCenter/api/json?depth=0", user, token)
    update_center_reachable = uc == 200 and reachable

    return decide(
        reachable=reachable,
        system_admin=system_admin,
        job_admin=job_admin,
        update_center_reachable=update_center_reachable,
        offline_hpi_present=offline_present,
        requested_plugins=requested,
    )


def main() -> None:
    out_dir = sys.argv[1] if len(sys.argv) >= 2 else "."
    try:
        os.makedirs(out_dir, exist_ok=True)
    except OSError:
        # Best-effort: dir may be unwritable; the write below surfaces any real error.
        pass
    caps = assess()
    path = os.path.join(out_dir, "capabilities.json")
    try:
        with open(path, "w", encoding="utf-8") as fh:
            json.dump(caps, fh, indent=2)
    except OSError as e:
        sys.stderr.write(f"Could not write {path}: {e}\n")
    # Human-readable one-liner for the console log.
    print(
        f"CI capabilities: mode={caps['mode']} "
        f"system_admin={caps['system_admin']} job_admin={caps['job_admin']} "
        f"plugins_source={caps['plugins_source']} "
        f"can_install_plugins={caps['can_install_plugins']}"
    )


if __name__ == "__main__":
    main()
