#!/usr/bin/env python3
"""Fixture-driven contract tests for discover_gitlab.py.

A fake ``fetch_json`` maps request URLs onto the checked-in fixtures in
../fixtures, so the whole client runs with no network and no pip. Exit 0 ==
pass, so it slots straight into ``make jenkins-tests``.
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

import discover_gitlab as disc  # noqa: E402
import inventory as inv  # noqa: E402

PASSED = 0


def check(label: str, condition: bool) -> None:
    global PASSED
    if not condition:
        raise AssertionError(f"FAIL: {label}")
    PASSED += 1
    print(f"  ok: {label}")


def _load_fixture(name: str):
    with open(os.path.join(FIXTURES, name), encoding="utf-8") as handle:
        return json.load(handle)


def _fake_fetch(mapping: dict):
    def fetch(url: str, headers: dict):
        for needle, fixture in mapping.items():
            if needle in url:
                return _load_fixture(fixture)
        raise AssertionError(f"unexpected URL in test: {url}")

    return fetch


def _cfg(**over) -> SimpleNamespace:
    base = dict(
        base_url="",
        group="mygroup",
        credential_id="gl-group-readonly",
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


def _fetch() -> object:
    return _fake_fetch(
        {
            "/repository/branches": "gitlab-branches-page-1.json",
            "/groups/mygroup/projects": "gitlab-projects-page-1.json",
        }
    )


def test_discovery() -> None:
    print("gitlab discovery:")
    entries = disc.discover_gitlab(_cfg(), _fetch())
    check("fork project filtered (only widget)", {e["repo_slug"] for e in entries} == {"widget"})
    check(
        "branch policy applied",
        sorted(e["branch"] for e in entries) == ["develop", "main", "release/3.0"],
    )
    main = next(e for e in entries if e["branch"] == "main")
    check("https clone url chosen by default", main["clone_url"].startswith("https://"))
    check("immutable numeric project id as repo_id", main["repo_id"] == "201")
    check("host is gitlab.com", main["host"] == "gitlab.com")
    check("provider tag", all(e["provider"] == "gitlab" for e in entries))
    check(
        "job_name built from host/namespace/slug/branch",
        main["job_name"] == "oxide-sloc-scans/gitlab-com/mygroup/widget/main",
    )
    check("credential id is non-secret reference", main["credential_id"] == "gl-group-readonly")
    check("no forbidden keys present", not any(k in main for k in inv.FORBIDDEN_KEYS))


def test_prefer_ssh_keeps_host() -> None:
    print("gitlab prefer-ssh:")
    entries = disc.discover_gitlab(_cfg(prefer_ssh=True), _fetch())
    main = next(e for e in entries if e["branch"] == "main")
    check("prefer_ssh selects scp-form ssh url", main["clone_url"].startswith("git@gitlab.com:"))
    check("host still resolved from web url", main["host"] == "gitlab.com")


def test_determinism_and_offline() -> None:
    print("gitlab determinism + offline load:")
    entries = disc.discover_gitlab(_cfg(), _fetch())
    dump1 = inv.dump_inventory(entries)
    dump2 = inv.dump_inventory(list(reversed(entries)))
    check("dump is order-independent (deterministic)", dump1 == dump2)
    cfg = _cfg(offline_inventory=dump1)
    offline = disc.discover(cfg, fetch=lambda u, h: (_ for _ in ()).throw(AssertionError("network!")))
    check("offline discover() avoids HTTP", inv.dump_inventory(offline) == dump1)


def main() -> int:
    test_discovery()
    test_prefer_ssh_keeps_host()
    test_determinism_and_offline()
    print(f"\nPASS - {PASSED} checks")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
