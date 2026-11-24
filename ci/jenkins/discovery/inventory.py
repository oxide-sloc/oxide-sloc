#!/usr/bin/env python3
"""Normalized inventory model shared by oxide-sloc Jenkins discovery.

Discovery, the Job DSL seed, and the push-event dispatcher all speak the same
small, deterministic JSON document: a flat list of repository/branch entries.
Keeping the schema in one place means the three stages can be developed and
tested independently, and the whole feature can be exercised offline from a
checked-in inventory file with no Bitbucket controller in the loop.

Design rules enforced here:

  * **Standard library only.**  This runs on an air-gapped Jenkins controller
    with no pip installs, exactly like the other ci/jenkins helpers.
  * **Deterministic output.**  ``dump_inventory`` sorts entries and keys so a
    re-run with the same upstream state produces byte-identical JSON; the seed
    job can diff it and stay idempotent.
  * **No secrets.**  Entries carry a Jenkins *credential id* (a non-secret
    reference), never a token, password, key, or authorization header.  URLs
    are scrubbed of any embedded userinfo before they enter an entry.

The schema (``REQUIRED_KEYS``) mirrors the example in the feature issue::

    {
      "provider": "bitbucket-server",
      "host": "git.example.invalid",
      "project": "PROJ",
      "repo_id": "immutable-provider-id",
      "repo_slug": "widget",
      "clone_url": "https://git.example.invalid/scm/PROJ/widget.git",
      "branch": "main",
      "credential_id": "jenkins-credential-id",
      "job_name": "oxide-sloc-scans/git-example-invalid/PROJ/widget/main",
      "enabled": true
    }
"""

from __future__ import annotations

import json
import re
from urllib.parse import urlsplit, urlunsplit

# Default branch policy: the three branch classes the feature targets.  The
# dispatcher and discovery both honour an override, but validate against this
# shape so a misconfigured regex cannot silently widen the surface.
DEFAULT_BRANCH_POLICY = r"^(main|develop|release/.+)$"

# The managed Jenkins folder that discovery exclusively owns.  Jobs are never
# created or modified outside it.
DEFAULT_MANAGED_FOLDER = "oxide-sloc-scans"

REQUIRED_KEYS = (
    "provider",
    "host",
    "project",
    "repo_id",
    "repo_slug",
    "clone_url",
    "branch",
    "credential_id",
    "job_name",
    "enabled",
)

# Keys that must never appear in an inventory entry — a defensive guard so a
# provider-adapter change cannot leak a secret into an artifact.
FORBIDDEN_KEYS = (
    "token",
    "password",
    "secret",
    "authorization",
    "private_key",
    "ssh_key",
    "access_token",
)


class InventoryError(ValueError):
    """Raised when an inventory document or entry is malformed."""


def scrub_url(url: str) -> str:
    """Strip any ``user:pass@`` userinfo from a URL so it is safe to store/log.

    A clone URL may legitimately carry a username (``git@host``); we keep a
    bare username but always drop an embedded password, and we never keep a
    ``user:token`` pair.  Credentials belong in a Jenkins credential id, not in
    the stored clone URL.
    """
    if not url:
        return url
    parts = urlsplit(url)
    netloc = parts.netloc
    if "@" in netloc:
        userinfo, hostport = netloc.rsplit("@", 1)
        user = userinfo.split(":", 1)[0]
        # Drop a bare git-over-ssh style user only if it carried a secret.
        netloc = f"{user}@{hostport}" if user and ":" not in userinfo else hostport
    return urlunsplit((parts.scheme, netloc, parts.path, parts.query, parts.fragment))


def sanitize_segment(value: str, *, lower: bool = False) -> str:
    """Make one path segment safe for a Jenkins job/folder name.

    Alphanumerics, ``.``, ``_`` and ``-`` survive; everything else (including
    the ``/`` in ``release/1.2``) collapses to a single ``-``.  Leading and
    trailing separators are trimmed so names never start or end with ``-``.
    Host segments are lower-cased; project/slug/branch keep their original case
    to match the schema example (``PROJ`` stays ``PROJ``).
    """
    if lower:
        value = value.lower()
    value = re.sub(r"[^A-Za-z0-9._-]+", "-", value)
    value = re.sub(r"-{2,}", "-", value).strip("-.")
    return value or "x"


def sanitize_host(value: str) -> str:
    """Sanitize a hostname for a job path: lower-case, dots/ports -> ``-``.

    Hosts become a single flat segment (``git.example.invalid`` ->
    ``git-example-invalid``) so the folder tree is one level per host, not one
    level per DNS label.
    """
    value = re.sub(r"[^A-Za-z0-9-]+", "-", value.lower())
    return re.sub(r"-{2,}", "-", value).strip("-") or "x"


def job_name(
    branch: str,
    *,
    host: str,
    project: str,
    repo_slug: str,
    repo_id: str = "",
    managed_folder: str = DEFAULT_MANAGED_FOLDER,
    collide: bool = False,
) -> str:
    """Build the stable, folder-scoped job name for a repo/branch pair.

    Shape: ``<folder>/<host>/<project>/<slug>/<branch>``.  When two distinct
    repositories sanitize to the same slug, ``collide=True`` appends a short,
    deterministic suffix derived from the immutable ``repo_id`` so the names
    stay unique without depending on discovery order.
    """
    host_seg = sanitize_host(host)
    proj_seg = sanitize_segment(project)
    slug_seg = sanitize_segment(repo_slug)
    if collide and repo_id:
        suffix = re.sub(r"[^A-Za-z0-9]+", "", repo_id)[-6:] or "id"
        slug_seg = f"{slug_seg}-{suffix}"
    branch_seg = sanitize_segment(branch)
    return "/".join((managed_folder, host_seg, proj_seg, slug_seg, branch_seg))


def make_entry(
    *,
    provider: str,
    host: str,
    project: str,
    repo_id: str,
    repo_slug: str,
    clone_url: str,
    branch: str,
    credential_id: str = "",
    enabled: bool = True,
    managed_folder: str = DEFAULT_MANAGED_FOLDER,
    collide: bool = False,
) -> dict:
    """Construct a single normalized, secret-free inventory entry."""
    return {
        "provider": provider,
        "host": host,
        "project": project,
        "repo_id": str(repo_id),
        "repo_slug": repo_slug,
        "clone_url": scrub_url(clone_url),
        "branch": branch,
        "credential_id": credential_id,
        "job_name": job_name(
            branch,
            host=host,
            project=project,
            repo_slug=repo_slug,
            repo_id=repo_id,
            managed_folder=managed_folder,
            collide=collide,
        ),
        "enabled": bool(enabled),
    }


def branch_allowed(branch: str, pattern: str = DEFAULT_BRANCH_POLICY) -> bool:
    """True when ``branch`` matches the (anchored) branch policy regex."""
    return re.match(pattern, branch) is not None


def compile_filter(pattern: str | None) -> re.Pattern | None:
    """Compile an optional include/exclude regex, or return ``None``."""
    return re.compile(pattern) if pattern else None


def repo_included(
    name: str,
    *,
    include: re.Pattern | None = None,
    exclude: re.Pattern | None = None,
    allowlist: set[str] | None = None,
    denylist: set[str] | None = None,
) -> bool:
    """Apply name allow/deny lists and include/exclude regexes, in that order.

    An explicit denylist always wins.  An explicit allowlist, when present,
    restricts to its members.  Include/exclude regexes then refine the result.
    """
    if denylist and name in denylist:
        return False
    if allowlist is not None and name not in allowlist:
        return False
    if include is not None and not include.search(name):
        return False
    if exclude is not None and exclude.search(name):
        return False
    return True


def _entry_sort_key(entry: dict) -> tuple:
    return (
        entry.get("host", ""),
        entry.get("project", ""),
        entry.get("repo_slug", ""),
        entry.get("branch", ""),
    )


def validate_entry(entry: dict) -> None:
    """Raise :class:`InventoryError` if an entry is missing keys or leaks a secret."""
    missing = [k for k in REQUIRED_KEYS if k not in entry]
    if missing:
        raise InventoryError(f"entry missing keys: {', '.join(missing)}")
    leaked = [k for k in entry if k.lower() in FORBIDDEN_KEYS]
    if leaked:
        raise InventoryError(f"entry contains forbidden secret-bearing keys: {', '.join(leaked)}")


def normalize_inventory(entries: list[dict]) -> list[dict]:
    """Validate, de-duplicate by ``job_name``, and sort deterministically."""
    seen: dict[str, dict] = {}
    for entry in entries:
        validate_entry(entry)
        seen[entry["job_name"]] = entry
    return sorted(seen.values(), key=_entry_sort_key)


def dump_inventory(entries: list[dict]) -> str:
    """Serialize to deterministic (sorted, stable) JSON with a trailing newline."""
    normalized = normalize_inventory(entries)
    return json.dumps(normalized, indent=2, sort_keys=True) + "\n"


def load_inventory(source) -> list[dict]:
    """Load an offline inventory from a path, file object, JSON string, or list.

    This is the air-gap path: the seed job and dispatcher can consume an
    inventory produced on a connected machine and transferred in, with no HTTP.
    """
    if isinstance(source, list):
        data = source
    else:
        if hasattr(source, "read"):
            text = source.read()
        elif isinstance(source, str) and source.lstrip()[:1] in "[{":
            text = source
        else:
            with open(source, encoding="utf-8") as handle:
                text = handle.read()
        data = json.loads(text)
    if not isinstance(data, list):
        raise InventoryError("inventory document must be a JSON array of entries")
    return normalize_inventory(data)
