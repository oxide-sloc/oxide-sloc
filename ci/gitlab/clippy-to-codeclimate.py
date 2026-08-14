#!/usr/bin/env python3
"""Convert `cargo clippy --message-format=json` output into GitLab Code Quality.

GitLab renders a "Code Quality" widget in the merge-request diff from a
CodeClimate-format JSON artifact declared via `artifacts:reports:codequality`.
This turns every clippy diagnostic into an inline finding on the exact line it
was raised, so reviewers see lint results in the diff without opening the job
log. It is the GitLab-native analogue of the Jenkins warnings-ng "recordIssues"
step.

The strict `-D warnings` gate stays in the separate `clippy` job; this converter
feeds the non-gating `codequality` job, so it must never fail the build — it
emits an empty array `[]` on any malformed input rather than erroring out.

Input : newline-delimited JSON from `cargo clippy --message-format=json`
        (a path, or `-` / omitted for stdin).
Output: a CodeClimate JSON array (a path, or `-` / omitted for stdout).

Usage:
    cargo clippy --workspace --all-targets --all-features --message-format=json \
        | python3 ci/gitlab/clippy-to-codeclimate.py - gl-code-quality.json

Standard library only (matches ci/github/sloc-to-sarif.py).
"""

import hashlib
import json
import sys

# clippy `level` → CodeClimate `severity`. GitLab accepts exactly:
#   info | minor | major | critical | blocker
# Anything outside this set makes GitLab silently drop the finding.
_SEVERITY = {
    "error": "major",
    "warning": "minor",
    "note": "info",
    "help": "info",
    "failure-note": "info",
}


def _read(path):
    if path in (None, "-"):
        return sys.stdin.read()
    with open(path, "r", encoding="utf-8", errors="replace") as fh:
        return fh.read()


def _primary_span(message):
    """Return the primary span (the one clippy underlines), or the first span."""
    spans = message.get("spans") or []
    for span in spans:
        if span.get("is_primary"):
            return span
    return spans[0] if spans else None


def _rel_path(file_name):
    """Repo-relative path with any leading './' stripped.

    GitLab matches Code Quality findings to the diff by `location.path`; an
    absolute or './'-prefixed path fails to match and the finding disappears.
    cargo emits workspace-relative paths already, so we only normalise slashes
    and the leading dot.
    """
    path = (file_name or "").replace("\\", "/")
    while path.startswith("./"):
        path = path[2:]
    return path


def _parse_record(line):
    """Parse one NDJSON line; return the record only if it's a compiler-message."""
    line = line.strip()
    if not line:
        return None
    try:
        record = json.loads(line)
    except ValueError:
        return None  # non-JSON progress lines, etc.
    if record.get("reason") != "compiler-message":
        return None
    return record


def _finding_from_message(message):
    """Build a Code Climate finding dict from a compiler message, or None."""
    severity = _SEVERITY.get(message.get("level", ""))
    if severity is None:
        return None  # skip non-diagnostic levels we don't map
    span = _primary_span(message)
    if not span or not span.get("file_name"):
        return None  # summary notes ("aborting due to N errors") have no span
    text = message.get("message", "").strip()
    if not text:
        return None
    check_name = (message.get("code") or {}).get("code") or "clippy"
    path = _rel_path(span["file_name"])
    begin = int(span.get("line_start", 1))
    # Non-cryptographic dedup key for identical findings; not a security digest.
    fingerprint = hashlib.sha1(
        f"{path}:{begin}:{check_name}:{text}".encode("utf-8"),
        usedforsecurity=False,
    ).hexdigest()
    return {
        "description": text,
        "check_name": check_name,
        "fingerprint": fingerprint,
        "severity": severity,
        "location": {"path": path, "lines": {"begin": begin}},
    }


def convert(raw):
    findings = []
    seen = set()  # de-dupe identical (path, line, check, message) fingerprints
    for line in raw.splitlines():
        record = _parse_record(line)
        if record is None:
            continue
        finding = _finding_from_message(record.get("message") or {})
        if finding is None or finding["fingerprint"] in seen:
            continue
        seen.add(finding["fingerprint"])
        findings.append(finding)
    return findings


def main():
    in_path = sys.argv[1] if len(sys.argv) > 1 else "-"
    out_path = sys.argv[2] if len(sys.argv) > 2 else "-"

    try:
        raw = _read(in_path)
        findings = convert(raw)
    except Exception as exc:  # never fail the build on this non-gating step
        sys.stderr.write(f"clippy-to-codeclimate: {exc}; emitting empty report\n")
        findings = []

    payload = json.dumps(findings, indent=2)
    if out_path in (None, "-"):
        sys.stdout.write(payload + "\n")
    else:
        with open(out_path, "w", encoding="utf-8") as fh:
            fh.write(payload + "\n")
    sys.stderr.write(f"clippy-to-codeclimate: {len(findings)} finding(s)\n")


if __name__ == "__main__":
    main()
