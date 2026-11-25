#!/usr/bin/env python3
"""Discover GitHub repositories/branches and emit a normalized inventory.

Supports GitHub.com and GitHub Enterprise Server with the same code path:

  * **Repos** -- ``GET /orgs/{org}/repos`` (or ``/users/{user}/repos``),
    paginated with ``?per_page=100&page=N`` until a short/empty page.
  * **Branches** -- ``GET /repos/{owner}/{repo}/branches``, same pagination.

The HTTP layer is a single injectable ``fetch_json(url, headers)`` callable so
the whole client is exercised offline against the checked-in fixtures -- no live
controller, no third-party package (standard library ``urllib`` only, matching
the other ci/jenkins helpers). An ``--offline-inventory`` path short-circuits
HTTP entirely and simply re-normalizes a transferred inventory, which is the
air-gapped validation / import workflow.

The immutable ``repo_id`` stored in every entry is GitHub's numeric repository
id (``repository.id``) -- the SAME field the push-event dispatcher reads from a
webhook payload, so an inventory entry and an inbound push resolve to each other
by a value a forged webhook cannot change.

Credentials are read from the environment at request time and are **never**
written into the inventory, argv, or logs -- only the non-secret Jenkins
``credential_id`` reference is stored.

Usage
-----
    # GitHub.com organization
    python3 discover_github.py --org my-org \\
        --credential-id gh-org-readonly --out inventory.json

    # GitHub Enterprise Server
    python3 discover_github.py --base-url https://ghe.example.com/api/v3 \\
        --org my-org --credential-id gh-ghe-readonly

    # A user's repos instead of an org
    python3 discover_github.py --org octocat --owner-type user

    # Air-gapped: re-normalize a transferred inventory, no network
    python3 discover_github.py --offline-inventory inbox/inventory.json

Environment variables (optional, secret)
-----------------------------------------
GITHUB_TOKEN   Bearer auth for the REST API (a PAT or an app token).
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.error
import urllib.request
from urllib.parse import urlsplit

import inventory as inv

PROVIDER = "github"
_DEFAULT_API = "https://api.github.com"


def _urllib_fetch_json(url: str, headers: dict):
    """Default transport: fetch ``url`` with ``headers`` and parse JSON."""
    request = urllib.request.Request(url, headers=headers)
    with urllib.request.urlopen(request, timeout=30) as response:  # noqa: S310 (trusted base-url)
        return json.loads(response.read().decode("utf-8"))


def _auth_headers() -> dict:
    """Build auth headers from the environment, if present (never logged)."""
    token = os.environ.get("GITHUB_TOKEN", "")
    headers = {
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "oxide-sloc-discovery",
    }
    if token:
        headers["Authorization"] = f"Bearer {token}"
    return headers


def _host_of(url: str) -> str:
    """Return the git host for display, mapping the API host back to the web host.

    ``api.github.com`` -> ``github.com``; a GHE ``ghe.example.com/api/v3`` base
    -> ``ghe.example.com``. A repo ``clone_url`` host is preferred when present.
    """
    host = urlsplit(url).hostname or url
    if host == "api.github.com":
        return "github.com"
    return host


def _iter_pages(fetch, base_path: str, headers: dict):
    """Yield each item from a ``?per_page=100&page=N`` paginated list endpoint."""
    page = 1
    while True:
        sep = "&" if "?" in base_path else "?"
        url = f"{base_path}{sep}per_page=100&page={page}"
        items = fetch(url, headers)
        if not isinstance(items, list) or not items:
            return
        yield from items
        if len(items) < 100:
            return
        page += 1


def _clone_url(repo: dict, prefer_ssh: bool) -> str:
    if prefer_ssh:
        return repo.get("ssh_url") or repo.get("clone_url") or ""
    return repo.get("clone_url") or repo.get("ssh_url") or ""


def discover_github(cfg, fetch) -> list[dict]:
    headers = _auth_headers()
    api = (cfg.base_url or _DEFAULT_API).rstrip("/")
    include = inv.compile_filter(cfg.include)
    exclude = inv.compile_filter(cfg.exclude)
    allowlist = set(cfg.allow) if cfg.allow else None
    denylist = set(cfg.deny) if cfg.deny else None
    scope = "users" if cfg.owner_type == "user" else "orgs"
    entries: list[dict] = []
    repos_path = f"{api}/{scope}/{cfg.org}/repos?type=all"
    for repo in _iter_pages(fetch, repos_path, headers):
        slug = repo.get("name", "")
        full = repo.get("full_name", slug)
        if repo.get("archived") and not cfg.include_archived:
            continue
        if repo.get("fork") and not cfg.include_forks:
            continue
        if not inv.repo_included(
            full, include=include, exclude=exclude, allowlist=allowlist, denylist=denylist
        ):
            continue
        clone_url = _clone_url(repo, cfg.prefer_ssh)
        # Derive the host from the web URL (always scheme://host), not the clone
        # URL, so SSH scp-form URLs (git@host:…) still yield the right host.
        host = _host_of(repo.get("html_url") or repo.get("clone_url") or api)
        branches_path = f"{api}/repos/{cfg.org}/{slug}/branches"
        for branch in _iter_branches(fetch, branches_path, headers):
            if not inv.branch_allowed(branch, cfg.branch_policy):
                continue
            entries.append(
                inv.make_entry(
                    provider=PROVIDER,
                    host=host,
                    project=cfg.org,
                    repo_id=repo.get("id", full),
                    repo_slug=slug,
                    clone_url=clone_url,
                    branch=branch,
                    credential_id=cfg.credential_id,
                    managed_folder=cfg.managed_folder,
                )
            )
    return inv.normalize_inventory(entries)


def _iter_branches(fetch, path, headers):
    for branch in _iter_pages(fetch, path, headers):
        name = branch.get("name", "")
        if name:
            yield name


def discover(cfg, fetch=_urllib_fetch_json) -> list[dict]:
    """Run discovery and return a normalized inventory."""
    if cfg.offline_inventory:
        return inv.load_inventory(cfg.offline_inventory)
    if not cfg.org:
        raise ValueError("--org is required unless --offline-inventory is given")
    return discover_github(cfg, fetch)


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Discover GitHub repos into an inventory.")
    parser.add_argument("--base-url", default="", help="GHE API base (default: api.github.com).")
    parser.add_argument("--org", default="", help="GitHub organization (or user with --owner-type).")
    parser.add_argument("--owner-type", choices=("org", "user"), default="org")
    parser.add_argument("--credential-id", default="", help="Jenkins credential id (non-secret).")
    parser.add_argument("--managed-folder", default=inv.DEFAULT_MANAGED_FOLDER)
    parser.add_argument("--branch-policy", default=inv.DEFAULT_BRANCH_POLICY)
    parser.add_argument("--include", default="", help="Include-name regex.")
    parser.add_argument("--exclude", default="", help="Exclude-name regex.")
    parser.add_argument("--allow", action="append", default=[], help="Allowlist name (repeatable).")
    parser.add_argument("--deny", action="append", default=[], help="Denylist name (repeatable).")
    parser.add_argument("--include-archived", action="store_true")
    parser.add_argument("--include-forks", action="store_true")
    parser.add_argument("--prefer-ssh", action="store_true", help="Prefer SSH clone URLs.")
    parser.add_argument("--offline-inventory", default="", help="Re-normalize this file; no HTTP.")
    parser.add_argument("--out", default="", help="Write inventory here (default: stdout).")
    return parser


def main(argv=None) -> int:
    cfg = _build_parser().parse_args(argv)
    try:
        entries = discover(cfg)
    except (urllib.error.URLError, inv.InventoryError, ValueError) as exc:
        print(f"discovery failed: {exc}", file=sys.stderr)
        return 1
    text = inv.dump_inventory(entries)
    if cfg.out:
        with open(cfg.out, "w", encoding="utf-8") as handle:
            handle.write(text)
        print(f"wrote {len(entries)} inventory entries to {cfg.out}", file=sys.stderr)
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
