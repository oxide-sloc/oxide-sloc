#!/usr/bin/env python3
"""Fixture-driven contract tests for inventory.py and discover_bitbucket.py.

Runs with no network and no third-party package: a fake ``fetch_json`` maps
request URLs onto the checked-in fixtures in ../fixtures.  Exit 0 == pass, so
it slots straight into ``make jenkins-tests`` alongside the other guard suites.
"""

from __future__ import annotations

import json
import os
import sys
from types import SimpleNamespace

HERE = os.path.dirname(os.path.abspath(__file__))
DISCOVERY = os.path.dirname(HERE)
FIXTURES = os.path.join(DISCOVERY, "fixtures")
sys.path.insert(0, DISCOVERY)

import discover_bitbucket as disc  # noqa: E402
import inventory as inv  # noqa: E402

PASSED = 0


def check(label: str, condition: bool) -> None:
    global PASSED
    if not condition:
        raise AssertionError(f"FAIL: {label}")
    PASSED += 1
    print(f"  ok: {label}")


def _load_fixture(name: str) -> dict:
    with open(os.path.join(FIXTURES, name), encoding="utf-8") as handle:
        return json.load(handle)


def _fake_fetch(mapping: dict):
    """Return a fetch(url, headers) that serves fixtures by URL substring."""

    def fetch(url: str, headers: dict) -> dict:
        for needle, fixture in mapping.items():
            if needle in url:
                return _load_fixture(fixture)
        raise AssertionError(f"unexpected URL in test: {url}")

    return fetch


def _server_cfg(**over) -> SimpleNamespace:
    base = dict(
        provider=disc.PROVIDER_SERVER,
        base_url="https://git.example.invalid",
        project="PROJ",
        workspace="",
        credential_id="bb-example-readonly",
        managed_folder=inv.DEFAULT_MANAGED_FOLDER,
        branch_policy=inv.DEFAULT_BRANCH_POLICY,
        include="",
        exclude="",
        allow=[],
        deny=[],
        include_archived=False,
        include_forks=False,
        prefer_ssh=False,
        offline_inventory="",
    )
    base.update(over)
    return SimpleNamespace(**base)


def _cloud_cfg(**over) -> SimpleNamespace:
    base = dict(
        provider=disc.PROVIDER_CLOUD,
        base_url="https://api.bitbucket.org",
        project="",
        workspace="my-workspace",
        credential_id="bb-cloud-readonly",
        managed_folder=inv.DEFAULT_MANAGED_FOLDER,
        branch_policy=inv.DEFAULT_BRANCH_POLICY,
        include="",
        exclude="",
        allow=[],
        deny=[],
        include_archived=False,
        include_forks=False,
        prefer_ssh=False,
        offline_inventory="",
    )
    base.update(over)
    return SimpleNamespace(**base)


def test_inventory_helpers() -> None:
    print("inventory helpers:")
    check(
        "scrub_url drops password",
        inv.scrub_url("https://user:secret@host/x.git") == "https://host/x.git",
    )
    check(
        "scrub_url keeps bare ssh user",
        inv.scrub_url("ssh://git@host:7999/x.git") == "ssh://git@host:7999/x.git",
    )
    check("branch_allowed main", inv.branch_allowed("main"))
    check("branch_allowed release/1.2", inv.branch_allowed("release/1.2"))
    check("branch_allowed rejects feature/x", not inv.branch_allowed("feature/x"))
    jn = inv.job_name("release/1.2", host="git.example.invalid", project="PROJ", repo_slug="widget")
    check(
        "job_name shape + branch slash sanitized",
        jn == "oxide-sloc-scans/git-example-invalid/PROJ/widget/release-1.2",
    )
    jn_a = inv.job_name("main", host="h", project="P", repo_slug="dup", repo_id="A1", collide=True)
    jn_b = inv.job_name("main", host="h", project="P", repo_slug="dup", repo_id="B2", collide=True)
    check("collision suffix makes names unique", jn_a != jn_b)
    try:
        inv.validate_entry({"token": "x"})
        check("validate_entry rejects secrets", False)
    except inv.InventoryError:
        check("validate_entry rejects secrets", True)


def test_server_discovery() -> None:
    print("server discovery:")
    fetch = _fake_fetch(
        {
            "/repos/widget/branches": "bitbucket-server-branches-page-1.json",
            "/projects/PROJ/repos": "bitbucket-server-repos-page-1.json",
        }
    )
    entries = disc.discover_server(_server_cfg(), fetch)
    branches = sorted(e["branch"] for e in entries)
    check("archived + fork repos filtered (only widget)", {e["repo_slug"] for e in entries} == {"widget"})
    check("branch policy applied", branches == ["develop", "main", "release/1.2"])
    widget_main = next(e for e in entries if e["branch"] == "main")
    check("https clone url chosen by default", widget_main["clone_url"].startswith("https://"))
    check("repo_id captured", widget_main["repo_id"] == "101")
    check(
        "job_name built from host/project/slug/branch",
        widget_main["job_name"] == "oxide-sloc-scans/git-example-invalid/PROJ/widget/main",
    )
    check("credential id is non-secret reference", widget_main["credential_id"] == "bb-example-readonly")
    check("no forbidden keys present", not any(k in widget_main for k in inv.FORBIDDEN_KEYS))

    ssh_entries = disc.discover_server(_server_cfg(prefer_ssh=True), fetch)
    ssh_main = next(e for e in ssh_entries if e["branch"] == "main")
    check("prefer_ssh selects ssh clone url", ssh_main["clone_url"].startswith("ssh://"))

    denied = disc.discover_server(_server_cfg(deny=["widget"]), fetch)
    check("denylist removes widget -> empty", denied == [])


def test_cloud_discovery() -> None:
    print("cloud discovery:")
    fetch = _fake_fetch(
        {
            "/refs/branches": "bitbucket-cloud-branches-page-1.json",
            "/2.0/repositories/my-workspace": "bitbucket-cloud-repos-page-1.json",
        }
    )
    entries = disc.discover_cloud(_cloud_cfg(), fetch)
    check("cloud fork filtered (only widget)", {e["repo_slug"] for e in entries} == {"widget"})
    check(
        "cloud branch policy applied",
        sorted(e["branch"] for e in entries) == ["main", "release/2.0"],
    )
    check("cloud host is bitbucket.org", all(e["host"] == "bitbucket.org" for e in entries))
    check("cloud provider tag", all(e["provider"] == disc.PROVIDER_CLOUD for e in entries))


def test_determinism_and_offline() -> None:
    print("determinism + offline load:")
    fetch = _fake_fetch(
        {
            "/repos/widget/branches": "bitbucket-server-branches-page-1.json",
            "/projects/PROJ/repos": "bitbucket-server-repos-page-1.json",
        }
    )
    entries = disc.discover_server(_server_cfg(), fetch)
    dump1 = inv.dump_inventory(entries)
    dump2 = inv.dump_inventory(list(reversed(entries)))
    check("dump is order-independent (deterministic)", dump1 == dump2)
    roundtrip = inv.load_inventory(dump1)
    check("offline load round-trips", inv.dump_inventory(roundtrip) == dump1)
    # discover() with --offline-inventory must not touch the network.
    cfg = _server_cfg(offline_inventory=dump1)
    offline = disc.discover(cfg, fetch=lambda u, h: (_ for _ in ()).throw(AssertionError("network!")))
    check("offline discover() avoids HTTP", inv.dump_inventory(offline) == dump1)


def main() -> int:
    test_inventory_helpers()
    test_server_discovery()
    test_cloud_discovery()
    test_determinism_and_offline()
    print(f"\nPASS - {PASSED} checks")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
