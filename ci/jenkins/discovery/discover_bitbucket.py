#!/usr/bin/env python3
"""Discover Bitbucket repositories/branches and emit a normalized inventory.

Supports both Bitbucket flavours with the same code path:

  * **Server / Data Center** -- ``GET /rest/api/1.0/projects/{key}/repos`` and
    ``.../repos/{slug}/branches``, paginated with ``start`` / ``limit`` /
    ``isLastPage`` / ``nextPageStart``.
  * **Cloud** -- ``GET /2.0/repositories/{workspace}`` and
    ``.../{slug}/refs/branches``, paginated with the ``next`` URL.

The HTTP layer is a single injectable ``fetch_json(url, headers)`` callable so
the whole client is exercised offline against the checked-in fixtures in
``fixtures/`` -- no live controller, no third-party package (standard library
``urllib`` only, matching the other ci/jenkins helpers).  An ``--offline-inventory``
path short-circuits HTTP entirely and simply re-normalizes a transferred
inventory, which is the air-gapped validation / import workflow.

Credentials are read from the environment at request time and are **never**
written into the inventory, argv, or logs -- only the non-secret Jenkins
``credential_id`` reference is stored.

Usage
-----
    # Bitbucket Server / Data Center
    python3 discover_bitbucket.py --provider bitbucket-server \\
        --base-url https://git.example.invalid --project PROJ \\
        --credential-id bb-example-readonly --out inventory.json

    # Bitbucket Cloud
    python3 discover_bitbucket.py --provider bitbucket-cloud \\
        --workspace my-workspace --credential-id bb-cloud-readonly

    # Air-gapped: re-normalize a transferred inventory, no network
    python3 discover_bitbucket.py --offline-inventory inbox/inventory.json

Environment variables (optional, secret)
-----------------------------------------
BITBUCKET_USER / BITBUCKET_TOKEN   HTTP basic / bearer auth for the REST API.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import sys
import urllib.error
import urllib.request

import inventory as inv

PROVIDER_SERVER = "bitbucket-server"
PROVIDER_CLOUD = "bitbucket-cloud"


def _urllib_fetch_json(url: str, headers: dict) -> dict:
    """Default transport: fetch ``url`` with ``headers`` and parse JSON."""
    request = urllib.request.Request(url, headers=headers)
    with urllib.request.urlopen(request, timeout=30) as response:  # noqa: S310 (trusted base-url)
        return json.loads(response.read().decode("utf-8"))


def _auth_headers() -> dict:
    """Build auth headers from the environment, if present (never logged)."""
    token = os.environ.get("BITBUCKET_TOKEN", "")
    user = os.environ.get("BITBUCKET_USER", "")
    if user and token:
        raw = f"{user}:{token}".encode()
        return {"Authorization": "Basic " + base64.b64encode(raw).decode()}
    if token:
        return {"Authorization": f"Bearer {token}"}
    return {}


def _host_of(base_url: str) -> str:
    from urllib.parse import urlsplit

    return urlsplit(base_url).hostname or base_url


def _pick_clone_url(clone_links: list[dict], prefer_ssh: bool) -> str:
    """Choose an HTTPS or SSH clone URL from a provider ``links.clone`` array."""
    by_name = {}
    for link in clone_links or []:
        name = (link.get("name") or "").lower()
        href = link.get("href") or ""
        if name and href:
            by_name[name] = href
    order = ["ssh", "https", "http"] if prefer_ssh else ["https", "http", "ssh"]
    for name in order:
        if name in by_name:
            return by_name[name]
    # Fall back to the first available href.
    return next(iter(by_name.values()), "")


# --------------------------------------------------------------------------- #
# Bitbucket Server / Data Center
# --------------------------------------------------------------------------- #
def _iter_server_pages(fetch, base_url: str, path: str, headers: dict):
    """Yield each page dict from a Server/DC paginated endpoint."""
    start = 0
    while True:
        sep = "&" if "?" in path else "?"
        url = f"{base_url}{path}{sep}start={start}&limit=100"
        page = fetch(url, headers)
        yield page
        if page.get("isLastPage", True):
            return
        nxt = page.get("nextPageStart")
        if nxt is None:
            return
        start = nxt


def discover_server(cfg, fetch) -> list[dict]:
    headers = _auth_headers()
    host = _host_of(cfg.base_url)
    include = inv.compile_filter(cfg.include)
    exclude = inv.compile_filter(cfg.exclude)
    allowlist = set(cfg.allow) if cfg.allow else None
    denylist = set(cfg.deny) if cfg.deny else None
    entries: list[dict] = []
    repo_path = f"/rest/api/1.0/projects/{cfg.project}/repos"
    for page in _iter_server_pages(fetch, cfg.base_url, repo_path, headers):
        for repo in page.get("values", []):
            slug = repo.get("slug", "")
            name = repo.get("name", slug)
            if repo.get("archived") and not cfg.include_archived:
                continue
            if repo.get("origin") and not cfg.include_forks:
                continue
            if not inv.repo_included(
                name, include=include, exclude=exclude, allowlist=allowlist, denylist=denylist
            ):
                continue
            clone_url = _pick_clone_url(
                (repo.get("links") or {}).get("clone", []), cfg.prefer_ssh
            )
            branches_path = f"{repo_path}/{slug}/branches"
            for branch in _iter_server_branches(fetch, cfg.base_url, branches_path, headers):
                if not inv.branch_allowed(branch, cfg.branch_policy):
                    continue
                entries.append(
                    inv.make_entry(
                        provider=PROVIDER_SERVER,
                        host=host,
                        project=cfg.project,
                        repo_id=repo.get("id", slug),
                        repo_slug=slug,
                        clone_url=clone_url,
                        branch=branch,
                        credential_id=cfg.credential_id,
                        managed_folder=cfg.managed_folder,
                    )
                )
    return inv.normalize_inventory(entries)


def _iter_server_branches(fetch, base_url, path, headers):
    for page in _iter_server_pages(fetch, base_url, path, headers):
        for branch in page.get("values", []):
            display = branch.get("displayId") or branch.get("id", "")
            if display:
                yield display


# --------------------------------------------------------------------------- #
# Bitbucket Cloud
# --------------------------------------------------------------------------- #
def _iter_cloud_pages(fetch, url: str, headers: dict):
    """Yield each page dict from a Cloud paginated endpoint, following ``next``."""
    while url:
        page = fetch(url, headers)
        yield page
        url = page.get("next")


def discover_cloud(cfg, fetch) -> list[dict]:
    headers = _auth_headers()
    host = "bitbucket.org"
    include = inv.compile_filter(cfg.include)
    exclude = inv.compile_filter(cfg.exclude)
    allowlist = set(cfg.allow) if cfg.allow else None
    denylist = set(cfg.deny) if cfg.deny else None
    entries: list[dict] = []
    base = cfg.base_url.rstrip("/") if cfg.base_url else "https://api.bitbucket.org"
    repo_url = f"{base}/2.0/repositories/{cfg.workspace}?pagelen=100"
    for page in _iter_cloud_pages(fetch, repo_url, headers):
        for repo in page.get("values", []):
            slug = repo.get("slug") or repo.get("name", "")
            full = repo.get("full_name", slug)
            if repo.get("parent") and not cfg.include_forks:
                continue
            if not inv.repo_included(
                full, include=include, exclude=exclude, allowlist=allowlist, denylist=denylist
            ):
                continue
            clone_url = _pick_clone_url(
                (repo.get("links") or {}).get("clone", []), cfg.prefer_ssh
            )
            branches_url = f"{base}/2.0/repositories/{cfg.workspace}/{slug}/refs/branches?pagelen=100"
            for branch in _iter_cloud_branches(fetch, branches_url, headers):
                if not inv.branch_allowed(branch, cfg.branch_policy):
                    continue
                entries.append(
                    inv.make_entry(
                        provider=PROVIDER_CLOUD,
                        host=host,
                        project=cfg.workspace,
                        repo_id=repo.get("uuid", slug),
                        repo_slug=slug,
                        clone_url=clone_url,
                        branch=branch,
                        credential_id=cfg.credential_id,
                        managed_folder=cfg.managed_folder,
                    )
                )
    return inv.normalize_inventory(entries)


def _iter_cloud_branches(fetch, url, headers):
    for page in _iter_cloud_pages(fetch, url, headers):
        for branch in page.get("values", []):
            name = branch.get("name", "")
            if name:
                yield name


def discover(cfg, fetch=_urllib_fetch_json) -> list[dict]:
    """Run discovery for the configured provider and return a normalized inventory."""
    if cfg.offline_inventory:
        return inv.load_inventory(cfg.offline_inventory)
    if cfg.provider == PROVIDER_SERVER:
        return discover_server(cfg, fetch)
    if cfg.provider == PROVIDER_CLOUD:
        return discover_cloud(cfg, fetch)
    raise ValueError(f"unknown provider: {cfg.provider!r}")


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Discover Bitbucket repos into an inventory.")
    parser.add_argument("--provider", choices=(PROVIDER_SERVER, PROVIDER_CLOUD))
    parser.add_argument("--base-url", default="", help="Server/DC base URL or Cloud API base.")
    parser.add_argument("--project", default="", help="Bitbucket Server/DC project key.")
    parser.add_argument("--workspace", default="", help="Bitbucket Cloud workspace.")
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
    if not cfg.offline_inventory and not cfg.provider:
        print("error: --provider is required unless --offline-inventory is given", file=sys.stderr)
        return 2
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
