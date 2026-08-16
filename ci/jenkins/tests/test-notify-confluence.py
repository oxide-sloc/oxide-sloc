#!/usr/bin/env python3
"""Regression tests for ci/jenkins/notify-confluence.py.

Runs the REAL notify-confluence.py as a subprocess against an in-process fake
Confluence REST server (stdlib http.server — no network, no dependencies), and
asserts the behaviours the P0/P1 fixes and the attachment-idempotency hardening
must guarantee:

  (a) glob fallback populates the summary table when the slug-named result file
      is absent but some result_*.json exists                        [P1]
  (b) the "no scan result JSON found" warning prints when none exists [P1]
  (c) create -> re-publish (same title) yields an UPDATE (vN -> N+1),
      never a duplicate page
  (d) both the HTML and PDF reports are POSTed as attachments
  (e) 401 / blank base URL / unreachable host all exit 0 (never fail the build)
  (f) a 400 on the attachment create endpoint falls back to the per-attachment
      /data endpoint (idempotency against strict Confluence Server/DC)

Run:  python3 ci/jenkins/tests/test-notify-confluence.py
Exit code 0 = all assertions passed.
"""

import json
import os
import subprocess
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse, parse_qs

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "..", "notify-confluence.py")

_SAMPLE = {
    "tool": {"name": "sloc", "version": "1.6.0"},
    "summary_totals": {
        "code_lines": 3475,
        "comment_lines": 812,
        "blank_lines": 640,
        "files_analyzed": 96,
    },
}


class _State:
    """Mutable server state shared across requests within one server instance."""

    def __init__(self, fail_status=None, attach_first_400=False):
        self.fail_status = fail_status          # e.g. 401 -> every call returns it
        self.attach_first_400 = attach_first_400
        self.pages = {}                         # title -> {id, version}
        self.next_id = 1000
        self.created_bodies = []                # storage bodies from POST/PUT
        self.attachment_posts = []              # (path, filename-ish)
        self.attach_seen = set()                # filenames already create-POSTed
        self.puts = []                          # ids updated via PUT
        self.data_endpoint_hits = []            # attachment /data endpoint hits


class _Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):               # silence access logging
        pass

    @property
    def st(self) -> _State:
        return self.server.state             # type: ignore[attr-defined]

    def _send(self, code, obj=None):
        body = json.dumps(obj or {}).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _read_body(self) -> bytes:
        n = int(self.headers.get("Content-Length", 0) or 0)
        return self.rfile.read(n) if n else b""

    # ── GET: page search + attachment lookup ────────────────────────────────
    def do_GET(self):
        if self.st.fail_status:
            return self._send(self.st.fail_status)
        u = urlparse(self.path)
        q = parse_qs(u.query)
        if u.path.endswith("/child/attachment"):
            # idempotency lookup: return the existing attachment id
            name = (q.get("filename") or [""])[0]
            return self._send(200, {"results": [{"id": "att-1", "title": name}]})
        # content search by title
        title = (q.get("title") or [""])[0]
        page = self.st.pages.get(title)
        results = []
        if page:
            results = [{"id": page["id"], "title": title,
                        "version": {"number": page["version"]}}]
        return self._send(200, {"results": results})

    # ── POST: create page OR upload attachment ──────────────────────────────
    def do_POST(self):
        if self.st.fail_status:
            self._read_body()
            return self._send(self.st.fail_status)
        u = urlparse(self.path)
        raw = self._read_body()

        if u.path.endswith("/data"):
            # per-attachment data endpoint (idempotency fallback target)
            self.st.data_endpoint_hits.append(u.path)
            return self._send(200, {"results": [{"id": "att-1"}]})

        if u.path.endswith("/child/attachment"):
            # crude filename sniff from the multipart body
            text = raw.decode("utf-8", "replace")
            fname = ""
            marker = 'filename="'
            if marker in text:
                fname = text.split(marker, 1)[1].split('"', 1)[0]
            if self.st.attach_first_400 and fname not in self.st.attach_seen:
                self.st.attach_seen.add(fname)
                return self._send(400, {"message": "same file name as an existing attachment"})
            self.st.attachment_posts.append(fname)
            return self._send(200, {"results": [{"id": "att-1", "title": fname}]})

        # create page
        try:
            payload = json.loads(raw.decode("utf-8"))
        except ValueError:
            payload = {}
        title = payload.get("title", "")
        self.st.created_bodies.append(
            payload.get("body", {}).get("storage", {}).get("value", ""))
        pid = str(self.st.next_id)
        self.st.next_id += 1
        self.st.pages[title] = {"id": pid, "version": 1}
        return self._send(200, {"id": pid, "title": title, "version": {"number": 1}})

    # ── PUT: update existing page ───────────────────────────────────────────
    def do_PUT(self):
        if self.st.fail_status:
            self._read_body()
            return self._send(self.st.fail_status)
        raw = self._read_body()
        try:
            payload = json.loads(raw.decode("utf-8"))
        except ValueError:
            payload = {}
        title = payload.get("title", "")
        pid = self.path.rstrip("/").rsplit("/", 1)[-1]
        self.st.puts.append(pid)
        self.st.created_bodies.append(
            payload.get("body", {}).get("storage", {}).get("value", ""))
        ver = int(payload.get("version", {}).get("number", 2))
        self.st.pages[title] = {"id": pid, "version": ver}
        return self._send(200, {"id": pid, "title": title, "version": {"number": ver}})


def _start(state: _State):
    srv = HTTPServer(("127.0.0.1", 0), _Handler)
    srv.state = state                            # type: ignore[attr-defined]
    t = threading.Thread(target=srv.serve_forever, daemon=True)
    t.start()
    return srv, srv.server_address[1]


def _run(out_dir, slug, base_url, extra_env=None):
    env = {
        **os.environ,
        "CONFLUENCE_BASE_URL": base_url,
        "CONFLUENCE_SPACE_KEY": "DEV",
        "CONFLUENCE_TOKEN": "test-token",
        "CONFLUENCE_PAGE_TITLE": "oxide-sloc — test",
        "JOB_NAME": "oxide-sloc",
        "BUILD_NUMBER": "42",
    }
    env.update(extra_env or {})
    p = subprocess.run(
        [sys.executable, SCRIPT, out_dir, slug],
        capture_output=True, text=True, env=env,
    )
    return p


def _mk_out(with_result=True, result_name="result_project.json", with_reports=True):
    d = tempfile.mkdtemp(prefix="sloc-cf-")
    if with_result:
        with open(os.path.join(d, result_name), "w", encoding="utf-8") as fh:
            json.dump(_SAMPLE, fh)
    if with_reports:
        with open(os.path.join(d, "report_project.html"), "w", encoding="utf-8") as fh:
            fh.write("<html><body>report</body></html>")
        with open(os.path.join(d, "report_project.pdf"), "wb") as fh:
            fh.write(b"%PDF-1.4 fake")
    return d


RESULTS = []


def check(name, cond, detail=""):
    RESULTS.append((name, bool(cond), detail))


def _scenario_glob_fallback():
    # (a) glob fallback: result file is named result_project.json, slug is a
    #     DIFFERENT value so the slug-named file is absent — must still populate.
    st = _State()
    srv, port = _start(st)
    out = _mk_out(result_name="result_project.json")
    p = _run(out, "mismatched-slug", f"http://127.0.0.1:{port}")
    srv.shutdown()
    body = st.created_bodies[0] if st.created_bodies else ""
    check("a. glob fallback populates table when slug file absent",
          p.returncode == 0 and "3,475" in body,
          f"rc={p.returncode} body_has_metric={'3,475' in body}")


def _scenario_no_result_json():
    # (b) no result JSON at all -> warning printed, still exits 0.
    st = _State()
    srv, port = _start(st)
    out = _mk_out(with_result=False, with_reports=False)
    p = _run(out, "", f"http://127.0.0.1:{port}")
    srv.shutdown()
    check("b. warns when no scan result JSON found",
          p.returncode == 0 and "no scan result JSON found" in p.stdout,
          f"rc={p.returncode} out={p.stdout.strip()[:80]!r}")


def _scenario_republish_updates():
    # (c) create then re-publish same title -> update (PUT), never a 2nd create.
    st = _State()
    srv, port = _start(st)
    out = _mk_out()
    p1 = _run(out, "project", f"http://127.0.0.1:{port}")
    p2 = _run(out, "project", f"http://127.0.0.1:{port}")
    creates = st.next_id - 1000
    srv.shutdown()
    check("c. re-publish updates (PUT) instead of duplicating",
          p1.returncode == 0 and p2.returncode == 0 and creates == 1 and len(st.puts) == 1,
          f"creates={creates} puts={len(st.puts)}")
    check("c2. update log line mentions 'updated'",
          "updated" in p2.stdout, p2.stdout.strip()[:80])
    # (d) both HTML and PDF attachments POSTed.
    check("d. both HTML+PDF attachments uploaded",
          "oxide-sloc-report.html" in st.attachment_posts
          and "oxide-sloc-report.pdf" in st.attachment_posts,
          f"attachments={st.attachment_posts}")


def _scenario_non_fatal_failures():
    # (e1) 401 -> non-fatal exit 0.
    st = _State(fail_status=401)
    srv, port = _start(st)
    out = _mk_out()
    p = _run(out, "project", f"http://127.0.0.1:{port}")
    srv.shutdown()
    check("e1. HTTP 401 is non-fatal (exit 0)", p.returncode == 0, f"rc={p.returncode}")
    # (e2) blank base URL -> "not configured" skip, exit 0.
    out = _mk_out()
    p = _run(out, "project", "")
    check("e2. blank base URL exits 0 with 'not configured'",
          p.returncode == 0 and "not configured" in p.stdout,
          f"rc={p.returncode} out={p.stdout.strip()[:80]!r}")
    # (e3) unreachable host -> "unreachable ... skipping cleanly", exit 0.
    out = _mk_out()
    # 127.0.0.1:1 is a closed port -> connection refused.
    p = _run(out, "project", "http://127.0.0.1:1")
    check("e3. unreachable host exits 0 cleanly",
          p.returncode == 0 and "unreachable" in p.stdout,
          f"rc={p.returncode} out={p.stdout.strip()[:80]!r}")


def _scenario_attachment_data_fallback():
    # (f) attachment create returns 400 -> fall back to /data endpoint.
    st = _State(attach_first_400=True)
    srv, port = _start(st)
    out = _mk_out()
    p = _run(out, "project", f"http://127.0.0.1:{port}")
    srv.shutdown()
    check("f. 400 on attachment create falls back to /data endpoint",
          p.returncode == 0 and len(st.data_endpoint_hits) >= 2,
          f"data_hits={len(st.data_endpoint_hits)}")


def _report() -> bool:
    print("=" * 78)
    print("notify-confluence.py — regression tests")
    print("=" * 78)
    all_ok = True
    for name, ok, detail in RESULTS:
        all_ok = all_ok and ok
        line = f"[{'PASS' if ok else 'FAIL'}] {name}"
        if not ok and detail:
            line += f"\n        {detail}"
        print(line)
    print("-" * 78)
    print("OVERALL:", "ALL PASSED" if all_ok else "SOME FAILED")
    return all_ok


def main() -> None:
    try:
        sys.stdout.reconfigure(encoding="utf-8")
    except Exception:
        # Best-effort: older Python or a non-reconfigurable stream is fine as-is.
        pass
    _scenario_glob_fallback()
    _scenario_no_result_json()
    _scenario_republish_updates()
    _scenario_non_fatal_failures()
    _scenario_attachment_data_fallback()
    sys.exit(0 if _report() else 1)


if __name__ == "__main__":
    main()
