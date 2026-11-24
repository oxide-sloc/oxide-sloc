#!/usr/bin/env python3
"""Normalize and safely dispatch a Bitbucket push webhook to a managed scan.

This is the event half of discovery.  A webhook is HOSTILE input, so the
dispatcher is built around one rule: **the payload may say what happened, but it
may never say what we do about it.**  Concretely:

  * The pushed branch and commit SHA are read from the payload.
  * The clone URL, credential id, and Jenkins job to trigger are looked up in
    the approved discovery inventory by the provider's immutable repository id.
    A clone URL or job name supplied in the webhook is ignored.  This stops a
    forged webhook from scanning an arbitrary URL or building an arbitrary job.
  * Only refs matching the branch policy (default ``^(main|develop|release/.+)$``)
    are dispatched.  Deletes, tags, PR events, unknown repos, and malformed
    bodies are ignored WITHOUT failing the webhook endpoint.
  * Each accepted push is SHA-pinned: SCAN_REF is the exact pushed commit, never
    a moving branch name.
  * Optional HMAC / shared-token verification runs BEFORE parsing.
  * A repo-id+SHA cooldown collapses duplicate deliveries.

Standard library only; exercised offline against checked-in fixtures.

Exit codes: 0 = handled (dispatched or deliberately ignored); 2 = usage;
3 = signature/verification failure (the one case a caller should treat as an
authentication error).
"""

from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import os
import sys
import time
from urllib.parse import urlsplit

import inventory as inv

PROVIDER_SERVER = "bitbucket-server"
PROVIDER_CLOUD = "bitbucket-cloud"
_ZERO_HASH = "0" * 40


class DispatchResult:
    """Outcome of normalizing one ref change in a push payload."""

    def __init__(self, accepted: bool, reason: str = "", **fields):
        self.accepted = accepted
        self.reason = reason
        self.fields = fields

    def as_dict(self) -> dict:
        out = {"accepted": self.accepted, "reason": self.reason}
        out.update(self.fields)
        return out


# --------------------------------------------------------------------------- #
# Webhook verification (runs before parsing)
# --------------------------------------------------------------------------- #
def verify_signature(secret: str, raw_body: bytes, header_value: str) -> bool:
    """Verify a webhook body.

    Supports two provider conventions, both constant-time compared:
      * HMAC-SHA256 as ``sha256=<hex>`` (Bitbucket Cloud ``X-Hub-Signature``);
      * a plain shared token equal to ``secret`` (Jenkins Generic Webhook
        Trigger token / Bitbucket Server secret).
    An empty configured secret means "verification disabled" -> accepted.
    """
    if not secret:
        return True
    if not header_value:
        return False
    value = header_value.strip()
    if value.startswith("sha256="):
        expected = hmac.new(secret.encode(), raw_body, hashlib.sha256).hexdigest()
        return hmac.compare_digest(expected, value.split("=", 1)[1])
    return hmac.compare_digest(secret, value)


# --------------------------------------------------------------------------- #
# Provider detection + normalization
# --------------------------------------------------------------------------- #
def detect_provider(payload: dict) -> str | None:
    if not isinstance(payload, dict):
        return None
    if isinstance(payload.get("push"), dict) and "repository" in payload:
        return PROVIDER_CLOUD
    if "changes" in payload and "repository" in payload:
        return PROVIDER_SERVER
    return None


def _host_from_clone(links: dict) -> str:
    for link in (links or {}).get("clone", []):
        href = link.get("href") or ""
        host = urlsplit(href).hostname
        if host:
            return host
    return ""


def _normalize_server(payload: dict, pattern: str) -> list[DispatchResult]:
    repo = payload.get("repository") or {}
    repo_id = str(repo.get("id", ""))
    slug = repo.get("slug", "")
    project = (repo.get("project") or {}).get("key", "")
    host = _host_from_clone(repo.get("links") or {})
    results = []
    for change in payload.get("changes", []):
        ref = change.get("ref") or {}
        ref_type = (ref.get("type") or "").upper()
        branch = ref.get("displayId", "")
        to_hash = change.get("toHash", "")
        ctype = (change.get("type") or "").upper()
        results.append(
            _decide(
                provider=PROVIDER_SERVER,
                host=host,
                project=project,
                repo_id=repo_id,
                repo_slug=slug,
                branch=branch,
                commit_sha=to_hash,
                is_tag=(ref_type == "TAG"),
                is_delete=(ctype == "DELETE" or to_hash == _ZERO_HASH),
                pattern=pattern,
            )
        )
    return results


def _normalize_cloud(payload: dict, pattern: str) -> list[DispatchResult]:
    repo = payload.get("repository") or {}
    repo_id = str(repo.get("uuid", ""))
    slug = repo.get("slug") or repo.get("name", "")
    project = (repo.get("workspace") or {}).get("slug", "")
    host = "bitbucket.org"
    results = []
    for change in (payload.get("push") or {}).get("changes", []):
        new = change.get("new")
        if new is None:  # branch/tag deleted
            results.append(DispatchResult(False, "deleted ref ignored", provider=PROVIDER_CLOUD))
            continue
        new_type = (new.get("type") or "").lower()
        branch = new.get("name", "")
        commit = ((new.get("target") or {}).get("hash")) or ""
        results.append(
            _decide(
                provider=PROVIDER_CLOUD,
                host=host,
                project=project,
                repo_id=repo_id,
                repo_slug=slug,
                branch=branch,
                commit_sha=commit,
                is_tag=(new_type != "branch"),
                is_delete=False,
                pattern=pattern,
            )
        )
    return results


def _decide(*, provider, host, project, repo_id, repo_slug, branch, commit_sha,
            is_tag, is_delete, pattern) -> DispatchResult:
    ident = dict(
        provider=provider,
        host=host,
        project=project,
        repo_id=repo_id,
        repo_slug=repo_slug,
        branch=branch,
        commit_sha=commit_sha,
        event_type="push",
    )
    if is_delete:
        return DispatchResult(False, "deleted ref ignored", **ident)
    if is_tag:
        return DispatchResult(False, "tag ref ignored", **ident)
    if not branch or not commit_sha:
        return DispatchResult(False, "malformed ref change", **ident)
    if not inv.branch_allowed(branch, pattern):
        return DispatchResult(False, f"branch '{branch}' outside policy", **ident)
    return DispatchResult(True, "", **ident)


def normalize(payload: dict, pattern: str = inv.DEFAULT_BRANCH_POLICY) -> list[DispatchResult]:
    """Normalize a push payload into per-ref dispatch decisions (never raises)."""
    provider = detect_provider(payload)
    if provider == PROVIDER_SERVER:
        return _normalize_server(payload, pattern)
    if provider == PROVIDER_CLOUD:
        return _normalize_cloud(payload, pattern)
    return [DispatchResult(False, "unrecognized or malformed payload")]


# --------------------------------------------------------------------------- #
# Inventory-backed resolution (the security boundary)
# --------------------------------------------------------------------------- #
def resolve(result: DispatchResult, entries: list[dict]) -> dict | None:
    """Map an accepted decision to a TRUSTED dispatch using only the inventory.

    The clone URL, credential id, and job name come from the approved inventory
    entry matched by immutable ``repo_id`` + ``branch`` -- never from the
    webhook.  Returns ``None`` for an unlisted repo/branch (forged or not yet
    discovered), which the caller treats as "ignore, do not dispatch".
    """
    if not result.accepted:
        return None
    repo_id = result.fields.get("repo_id")
    branch = result.fields.get("branch")
    for entry in entries:
        if entry.get("repo_id") == repo_id and entry.get("branch") == branch:
            if entry.get("enabled") is False:
                return None
            return {
                "job_name": entry["job_name"],
                "central_params": {
                    "SCAN_REPO_URL": entry["clone_url"],
                    "SCAN_REF": result.fields["commit_sha"],  # SHA-pinned
                    "SCAN_CREDENTIALS_ID": entry.get("credential_id", ""),
                },
            }
    return None


# --------------------------------------------------------------------------- #
# Duplicate-delivery cooldown (repo_id + commit SHA)
# --------------------------------------------------------------------------- #
def seen_recently(state_dir: str, repo_id: str, sha: str, cooldown: int, now=None) -> bool:
    """True if this (repo_id, sha) fired within ``cooldown`` seconds; else record it.

    Uses one small marker file per key so it survives across separate dispatcher
    invocations in a pipeline without a database.  ``cooldown <= 0`` disables.
    """
    if cooldown <= 0 or not state_dir:
        return False
    now = time.time() if now is None else now
    os.makedirs(state_dir, exist_ok=True)
    key = hashlib.sha256(f"{repo_id}\n{sha}".encode()).hexdigest()
    marker = os.path.join(state_dir, key)
    if os.path.exists(marker) and (now - os.path.getmtime(marker)) < cooldown:
        return True
    with open(marker, "w", encoding="utf-8") as handle:
        handle.write(str(now))
    os.utime(marker, (now, now))
    return False


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Normalize + dispatch a Bitbucket push event.")
    parser.add_argument("--payload", default="-", help="Payload JSON file, or '-' for stdin.")
    parser.add_argument("--inventory", required=True, help="Approved discovery inventory JSON.")
    parser.add_argument("--branch-policy", default=inv.DEFAULT_BRANCH_POLICY)
    parser.add_argument("--secret", default=os.environ.get("WEBHOOK_SECRET", ""))
    parser.add_argument("--signature", default="", help="Signature/token header value to verify.")
    parser.add_argument("--state-dir", default="", help="Cooldown marker directory.")
    parser.add_argument("--cooldown-seconds", type=int, default=0)
    return parser


def main(argv=None) -> int:
    cfg = _build_parser().parse_args(argv)
    raw = sys.stdin.buffer.read() if cfg.payload == "-" else open(cfg.payload, "rb").read()

    if not verify_signature(cfg.secret, raw, cfg.signature):
        print(json.dumps({"accepted": False, "reason": "signature verification failed"}))
        return 3

    try:
        payload = json.loads(raw.decode("utf-8"))
    except (ValueError, UnicodeDecodeError):
        print(json.dumps({"accepted": False, "reason": "malformed JSON payload"}))
        return 0  # do not fail the webhook endpoint

    entries = inv.load_inventory(cfg.inventory)
    dispatches, ignored = [], []
    for result in normalize(payload, cfg.branch_policy):
        trusted = resolve(result, entries)
        if trusted is None:
            ignored.append(result.reason or "no matching approved inventory entry")
            continue
        repo_id = result.fields["repo_id"]
        sha = result.fields["commit_sha"]
        if seen_recently(cfg.state_dir, repo_id, sha, cfg.cooldown_seconds):
            ignored.append(f"duplicate delivery suppressed for {repo_id}@{sha[:10]}")
            continue
        dispatches.append(trusted)

    print(json.dumps({"dispatches": dispatches, "ignored": ignored}, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
