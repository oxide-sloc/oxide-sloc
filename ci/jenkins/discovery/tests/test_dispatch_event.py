#!/usr/bin/env python3
"""Fixture-driven tests for the push-event dispatcher.

Covers the happy paths (main/develop/release are SHA-pinned and dispatched) and
the security/negative paths the feature issue requires: feature branches, tags,
deletes, malformed payloads, duplicate deliveries, forged/unlisted repos, and a
spoofed clone URL that must NOT be trusted.  No network, no pip.
"""

from __future__ import annotations

import hashlib
import hmac
import json
import os
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
DISCOVERY = os.path.dirname(HERE)
FIXTURES = os.path.join(DISCOVERY, "fixtures")
sys.path.insert(0, DISCOVERY)

import dispatch_event as dsp  # noqa: E402
import inventory as inv  # noqa: E402

PASSED = 0


def check(label: str, condition: bool) -> None:
    global PASSED
    if not condition:
        raise AssertionError(f"FAIL: {label}")
    PASSED += 1
    print(f"  ok: {label}")


def _fixture(name: str) -> dict:
    with open(os.path.join(FIXTURES, name), encoding="utf-8") as handle:
        return json.load(handle)


def _inventory() -> list[dict]:
    """Approved inventory: widget main/develop/release-1.2 on the Server host."""
    base = dict(
        provider="bitbucket-server",
        host="git.example.invalid",
        project="PROJ",
        repo_id="101",
        repo_slug="widget",
        clone_url="https://git.example.invalid/scm/PROJ/widget.git",
        credential_id="bb-example-readonly",
    )
    return inv.normalize_inventory(
        [inv.make_entry(branch=b, **base) for b in ("main", "develop", "release/1.2")]
    )


def _dispatch_one(fixture_name: str) -> dict | None:
    entries = _inventory()
    results = dsp.normalize(_fixture(fixture_name))
    for result in results:
        trusted = dsp.resolve(result, entries)
        if trusted:
            return trusted
    return None


def test_accept_and_sha_pin() -> None:
    print("accepted pushes are SHA-pinned + inventory-resolved:")
    main = _dispatch_one("bitbucket-push-main.json")
    check("main push dispatches", main is not None)
    check(
        "SCAN_REF is the pushed SHA, not the branch name",
        main["central_params"]["SCAN_REF"] == "a" * 40,
    )
    check(
        "job name resolved from inventory",
        main["job_name"] == "oxide-sloc-scans/git-example-invalid/PROJ/widget/main",
    )
    develop = _dispatch_one("bitbucket-push-develop.json")
    check("develop push dispatches with its SHA", develop["central_params"]["SCAN_REF"] == "b" * 40)
    release = _dispatch_one("bitbucket-push-release.json")
    check("release/1.2 push dispatches", release is not None)
    check("release SHA-pinned", release["central_params"]["SCAN_REF"] == "c" * 40)


def test_ignored_classes() -> None:
    print("out-of-policy / non-push refs are ignored:")
    check("feature branch not dispatched", _dispatch_one("bitbucket-push-ignored.json") is None)

    tag = {
        "eventKey": "repo:refs_changed",
        "repository": {"slug": "widget", "id": 101, "project": {"key": "PROJ"}, "links": {}},
        "changes": [{"ref": {"displayId": "v1.0", "type": "TAG"}, "toHash": "a" * 40, "type": "ADD"}],
    }
    check("tag ref ignored", not dsp.normalize(tag)[0].accepted)

    delete = {
        "eventKey": "repo:refs_changed",
        "repository": {"slug": "widget", "id": 101, "project": {"key": "PROJ"}, "links": {}},
        "changes": [{"ref": {"displayId": "main", "type": "BRANCH"}, "toHash": "0" * 40, "type": "DELETE"}],
    }
    check("delete ref ignored", not dsp.normalize(delete)[0].accepted)

    check("malformed payload ignored (no raise)", not dsp.normalize({"junk": 1})[0].accepted)


def test_security_boundary() -> None:
    print("webhook cannot scan arbitrary URL or build arbitrary job:")
    check("forged/unlisted repo id not dispatched", _dispatch_one("bitbucket-push-forged.json") is None)

    # Spoofed clone URL in the payload must be ignored; inventory URL is used.
    spoofed = _dispatch_one("bitbucket-push-spoofed-url.json")
    check("spoofed push still matches inventory repo_id", spoofed is not None)
    check(
        "dispatch uses inventory clone URL, not the payload's",
        spoofed["central_params"]["SCAN_REPO_URL"] == "https://git.example.invalid/scm/PROJ/widget.git",
    )
    check(
        "attacker URL never reaches dispatch",
        "attacker.invalid" not in json.dumps(spoofed),
    )


def test_cloud_provider() -> None:
    print("Bitbucket Cloud push normalization:")
    results = dsp.normalize(_fixture("bitbucket-push-cloud-main.json"))
    check("cloud provider detected + accepted", results[0].accepted)
    check("cloud commit sha captured", results[0].fields["commit_sha"] == "0123456789abcdef0123456789abcdef01234567")


def test_signature_verification() -> None:
    print("webhook verification:")
    body = b'{"x":1}'
    secret = "s3cr3t"
    good = "sha256=" + hmac.new(secret.encode(), body, hashlib.sha256).hexdigest()
    check("valid HMAC accepted", dsp.verify_signature(secret, body, good))
    check("tampered HMAC rejected", not dsp.verify_signature(secret, body, good[:-1] + "0"))
    check("missing signature rejected when secret set", not dsp.verify_signature(secret, body, ""))
    check("plain shared token accepted", dsp.verify_signature("tok", body, "tok"))
    check("no secret configured -> accepted", dsp.verify_signature("", body, ""))


def test_cooldown() -> None:
    print("duplicate-delivery cooldown:")
    with tempfile.TemporaryDirectory() as state:
        first = dsp.seen_recently(state, "101", "a" * 40, 300, now=1000.0)
        second = dsp.seen_recently(state, "101", "a" * 40, 300, now=1010.0)
        later = dsp.seen_recently(state, "101", "a" * 40, 300, now=2000.0)
        check("first delivery not a duplicate", not first)
        check("immediate redelivery suppressed", second)
        check("delivery after cooldown allowed again", not later)
        check("cooldown <= 0 disables suppression", not dsp.seen_recently(state, "101", "a" * 40, 0))


def main() -> int:
    test_accept_and_sha_pin()
    test_ignored_classes()
    test_security_boundary()
    test_cloud_provider()
    test_signature_verification()
    test_cooldown()
    print(f"\nPASS - {PASSED} checks")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
