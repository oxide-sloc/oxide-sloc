#!/usr/bin/env python3
"""Upsert an oxide-sloc summary page in Confluence with a link to the report.

Fully opt-in: if the base URL or token is absent it prints a note and exits 0 —
it never fails the build. Standard library only (urllib), so it works on an
air-gapped controller (where it will simply no-op because the endpoint is
unreachable). Supports Confluence Server/Data Center and Cloud REST.

Usage
-----
    python3 ci/jenkins/notify-confluence.py <output-dir> [project-slug]

Environment
-----------
CONFLUENCE_BASE_URL   e.g. https://confluence.example.com  (Server/DC)
                      or   https://your-domain.atlassian.net/wiki  (Cloud)
CONFLUENCE_TOKEN      bearer token (Server/DC PAT) — OR set CONFLUENCE_USER +
                      CONFLUENCE_TOKEN for Cloud basic auth (email:api-token)
CONFLUENCE_SPACE_KEY  space to write into (required)
CONFLUENCE_PARENT_ID  optional parent page id
CONFLUENCE_PAGE_TITLE optional page title (default: "oxide-sloc — <job/slug>")
REPORT_URL            link to the published report (shown on the page)
BUILD_URL / JOB_NAME / BUILD_NUMBER  Jenkins context (optional)

Auth-scheme matrix
------------------
    Target                 Credential                Scheme   Set CONFLUENCE_USER?
    Confluence CLOUD       API token                 Basic    YES (account email)
    Confluence SERVER/DC   Personal Access Token     Bearer   no

SSRF note
---------
Unlike the web app (which enforces an SSRF allow/deny policy on user-supplied
Atlassian URLs), this CI script performs NO SSRF filtering. It runs in a trusted
CI context where CONFLUENCE_BASE_URL is an operator-set job parameter, not
attacker-controlled input, so blocking loopback/link-local/metadata hosts is
neither needed nor desirable (self-hosted Server on a private LAN is valid).

TODO (Cloud v1 deprecation)
---------------------------
This script uses the v1 `/rest/api/content` REST surface for BOTH Server/DC and
Cloud. It still works against Cloud today, but Atlassian has deprecated the Cloud
v1 content REST API in favour of v2 (`/wiki/api/v2/...`). The web app already
speaks v2 for Cloud (see crates/sloc-web/src/confluence.rs); align this CI helper
with v2 for Cloud when convenient. Server/DC continues to use v1.
"""

import base64
import json
import os
import sys
import urllib.error
import urllib.request
from typing import Optional

_TIMEOUT = 10
_JSON_CONTENT_TYPE = "application/json"


def _auth_header() -> Optional[str]:
    token = os.environ.get("CONFLUENCE_TOKEN", "").strip()
    if not token:
        return None
    user = os.environ.get("CONFLUENCE_USER", "").strip()
    if user:  # Cloud basic auth: email:api-token
        raw = f"{user}:{token}".encode("utf-8")
        return "Basic " + base64.b64encode(raw).decode("ascii")
    return f"Bearer {token}"  # Server/DC personal access token


def _req(method: str, url: str, auth: str, body: Optional[dict] = None):
    data = json.dumps(body).encode("utf-8") if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("Authorization", auth)
    req.add_header("Content-Type", _JSON_CONTENT_TYPE)
    req.add_header("Accept", _JSON_CONTENT_TYPE)
    try:
        with urllib.request.urlopen(req, timeout=_TIMEOUT) as resp:  # nosec B310 - configured http(s) Confluence API URL; errors caught below
            payload = resp.read().decode("utf-8")
            return resp.status, json.loads(payload) if payload else {}
    except urllib.error.HTTPError as e:
        return e.code, None
    except (OSError, ValueError):
        # urllib.error.URLError is a subclass of OSError, so it is caught here too.
        return None, None


def _fmt(n) -> str:
    try:
        return f"{int(n):,}"
    except (TypeError, ValueError):
        return str(n)


_BOUNDARY = "oxidesloc7f3c1a9b2e5d4680boundary"


def _multipart_body(filename: str, content_type: str, data: bytes) -> bytes:
    """Hand-assembled multipart/form-data body for a Confluence attachment."""
    pre = (
        f"--{_BOUNDARY}\r\n"
        f'Content-Disposition: form-data; name="file"; filename="{filename}"\r\n'
        f"Content-Type: {content_type}\r\n\r\n"
    ).encode("utf-8")
    post = (
        f"\r\n--{_BOUNDARY}\r\n"
        'Content-Disposition: form-data; name="minorEdit"\r\n\r\n'
        "true\r\n"
        f"--{_BOUNDARY}--\r\n"
    ).encode("utf-8")
    return pre + data + post


def _multipart_post(url: str, auth: str, filename: str, ctype: str, data: bytes):
    """POST a multipart/form-data attachment body. Returns (status, None) on an
    HTTP response (any code), or (None, None) when the endpoint is unreachable."""
    req = urllib.request.Request(
        url,
        data=_multipart_body(filename, ctype, data),
        method="POST",
    )
    req.add_header("Authorization", auth)
    req.add_header("X-Atlassian-Token", "nocheck")
    req.add_header("Content-Type", f"multipart/form-data; boundary={_BOUNDARY}")
    req.add_header("Accept", _JSON_CONTENT_TYPE)
    try:
        with urllib.request.urlopen(req, timeout=_TIMEOUT) as resp:  # nosec B310 - configured http(s) Confluence API URL; errors caught below
            return resp.status, None
    except urllib.error.HTTPError as e:
        return e.code, None
    except (OSError, ValueError):
        # urllib.error.URLError is a subclass of OSError, so it is caught here too.
        return None, None


def _find_attachment_id(api: str, pid: str, auth: str, name: str) -> Optional[str]:
    """Return the id of an existing attachment named `name` on page `pid`, or None."""
    from urllib.parse import quote
    url = f"{api}/{pid}/child/attachment?filename={quote(name)}&limit=50"
    status, found = _req("GET", url, auth)
    if status is None or not isinstance(found, dict):
        return None
    for att in found.get("results", []):
        if att.get("title") == name and att.get("id"):
            return att["id"]
    return None


def _attach(api: str, pid: str, auth: str, out_dir: str) -> None:
    """Best-effort: upload the HTML/PDF report as page attachments.

    Idempotency: re-posting the same filename to `/child/attachment` updates the
    attachment to a new version on many Confluence versions — but some Server/DC
    versions REJECT the re-POST with 400 ("Cannot add a new attachment with same
    file name as an existing attachment"). In that case we look up the existing
    attachment id and POST the new bytes to `/child/attachment/{id}/data`, which
    always creates a new version. Never fails the build.
    """
    import glob as _glob

    targets = []
    for pattern, ctype, name in (
        ("*.html", "text/html", "oxide-sloc-report.html"),
        ("*.pdf", "application/pdf", "oxide-sloc-report.pdf"),
    ):
        matches = sorted(_glob.glob(os.path.join(out_dir, pattern)))
        if matches:
            targets.append((matches[0], ctype, name))

    for path, ctype, name in targets:
        try:
            with open(path, "rb") as fh:
                data = fh.read()
        except OSError:
            continue

        status, _ = _multipart_post(f"{api}/{pid}/child/attachment", auth, name, ctype, data)

        # 400 on the create endpoint usually means the filename already exists on
        # a Confluence version that won't update-via-re-POST. Fall back to the
        # per-attachment data endpoint to push a new version.
        if status == 400:
            att_id = _find_attachment_id(api, pid, auth, name)
            if att_id:
                status, _ = _multipart_post(
                    f"{api}/{pid}/child/attachment/{att_id}/data",
                    auth, name, ctype, data,
                )

        if status is None:
            print(f"notify-confluence: attachment '{name}' skipped (unreachable) — non-fatal.")
        elif 200 <= status < 300:
            print(f"notify-confluence: attachment '{name}' uploaded.")
        else:
            print(f"notify-confluence: attachment '{name}' upload returned {status} — non-fatal.")


def _storage_body(data: dict, report_url: str) -> str:
    """Confluence 'storage' (XHTML) body summarising the scan."""
    t = data.get("summary_totals", {})
    tool = data.get("tool", {}) if isinstance(data.get("tool"), dict) else {}
    job = os.environ.get("JOB_NAME", "")
    build = os.environ.get("BUILD_NUMBER", "")
    rows = "".join(
        f"<tr><th>{k}</th><td>{_fmt(v)}</td></tr>"
        for k, v in [
            ("Code lines", t.get("code_lines", 0)),
            ("Comment lines", t.get("comment_lines", 0)),
            ("Blank lines", t.get("blank_lines", 0)),
            ("Files analyzed", t.get("files_analyzed", 0)),
        ]
    )
    link = (
        f'<p><a href="{report_url}">Open the full oxide-sloc report</a></p>'
        if report_url else ""
    )
    meta = f"oxide-sloc v{tool.get('version', '')}"
    if job:
        meta += f" · job {job}" + (f" #{build}" if build else "")
    return (
        f"<p>{meta}</p>"
        f"<table><tbody>{rows}</tbody></table>"
        f"{link}"
        "<p><em>Updated automatically by the oxide-sloc CI pipeline.</em></p>"
    )


def _load_scan_result(out_dir: str, slug: str) -> dict:
    """Load the scan-result JSON for the summary table.

    Prefers ``result_<slug>.json`` but ALWAYS falls back to any ``result_*.json``
    — if the scan wrote its output under a name that drifts from the SLOC_PROJECT
    slug, looking only for result_<slug>.json would silently publish an all-zeros
    table. Returns {} (and warns) when nothing loadable is found.
    """
    import glob as _glob

    candidates = []
    if slug:
        candidates.append(os.path.join(out_dir, f"result_{slug}.json"))
    candidates += sorted(_glob.glob(os.path.join(out_dir, "result_*.json")))
    seen = set()
    candidates = [c for c in candidates if not (c in seen or seen.add(c))]
    for c in candidates:
        try:
            with open(c, encoding="utf-8") as fh:
                return json.load(fh)
        except (OSError, ValueError):
            continue
    print(f"notify-confluence: warning — no scan result JSON found in '{out_dir}'; "
          "summary table will be empty.")
    return {}


def _find_page_results(api: str, space: str, title: str, auth: str):
    """GET existing page(s) by title within the space. Returns the results list,
    or None when Confluence is unreachable (caller skips cleanly)."""
    from urllib.parse import quote
    q = f"{api}?spaceKey={quote(space)}&title={quote(title)}&expand=version"
    status, found = _req("GET", q, auth)
    if status is None:
        return None
    return (found or {}).get("results", []) if isinstance(found, dict) else []


def _build_common(title: str, space: str, body_storage: str) -> dict:
    common = {
        "type": "page",
        "title": title,
        "space": {"key": space},
        "body": {"storage": {"value": body_storage, "representation": "storage"}},
    }
    parent = os.environ.get("CONFLUENCE_PARENT_ID", "").strip()
    if parent:
        common["ancestors"] = [{"id": parent}]
    return common


def _upsert_page(api: str, title: str, common: dict, results: list, auth: str):
    """Create the page, or update the first existing match. Returns (ok, page_id)."""
    if results:
        page = results[0]
        pid = page.get("id")
        ver = int(page.get("version", {}).get("number", 1)) + 1
        common["version"] = {"number": ver}
        st, _ = _req("PUT", f"{api}/{pid}", auth, common)
        ok = st is not None and 200 <= st < 300
        print(f"notify-confluence: {'updated' if ok else 'update failed ('+str(st)+') for'} "
              f"page '{title}' (v{ver}).")
        return ok, pid
    st, created = _req("POST", api, auth, common)
    ok = st is not None and 200 <= st < 300
    pid = (created or {}).get("id") if isinstance(created, dict) else None
    print(f"notify-confluence: {'created' if ok else 'create failed ('+str(st)+') for'} "
          f"page '{title}'.")
    return ok, pid


def main() -> None:
    out_dir = sys.argv[1] if len(sys.argv) >= 2 else "."
    slug = sys.argv[2] if len(sys.argv) >= 3 else ""

    base = os.environ.get("CONFLUENCE_BASE_URL", "").strip().rstrip("/")
    space = os.environ.get("CONFLUENCE_SPACE_KEY", "").strip()
    auth = _auth_header()
    if not base or not space or not auth:
        print("notify-confluence: not configured "
              "(need CONFLUENCE_BASE_URL, CONFLUENCE_SPACE_KEY, CONFLUENCE_TOKEN) - skipping.")
        return

    data = _load_scan_result(out_dir, slug)

    title = os.environ.get("CONFLUENCE_PAGE_TITLE", "").strip() or (
        f"oxide-sloc — {os.environ.get('JOB_NAME', slug) or 'report'}"
    )
    report_url = os.environ.get("REPORT_URL", os.environ.get("BUILD_URL", "")).strip()
    body_storage = _storage_body(data, report_url)

    api = f"{base}/rest/api/content"
    results = _find_page_results(api, space, title, auth)
    if results is None:
        print("notify-confluence: Confluence unreachable (air-gapped?) - skipping cleanly.")
        return

    common = _build_common(title, space, body_storage)
    ok, pid = _upsert_page(api, title, common, results, auth)

    # Best-effort: embed the full HTML/PDF report as attachments on the page.
    if ok and pid:
        _attach(api, pid, auth, out_dir)


if __name__ == "__main__":
    main()
