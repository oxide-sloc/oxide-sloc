#!/usr/bin/env python3
"""
Submit one or more files to VirusTotal and print scan results.

Usage (Linux / macOS / Git Bash):
    VT_API_KEY=<key> python3 scripts/internal/vt-scan.py <file> [<file2> ...]

Usage (PowerShell):
    $env:VT_API_KEY="<key>"; python3 scripts/internal/vt-scan.py <file>

Build a local binary first if you don't have one:
    cargo build --release
    # binary lands at target/release/oxide-sloc  (or oxide-sloc.exe on Windows)

Exit codes:
    0  all scans completed (may still have detections — check output)
    1  missing API key or no files given
    2  one or more uploads failed
"""

import json
import os
import sys
import time
import urllib.error
import urllib.request

POLL_INTERVAL = 30   # seconds between status checks
POLL_ATTEMPTS = 10   # max attempts per file (~5 min total)
UPLOAD_DELAY  = 16   # seconds between uploads (free tier: 4 req/min)


def vt_request(method, url, files=None, api_key=""):
    """Minimal multipart/JSON HTTP helper using only stdlib.

    Only HTTPS destinations are permitted, so the request cannot be redirected to a
    local file or other non-network scheme.
    """
    if not url.lower().startswith("https://"):
        raise ValueError(f"refusing non-HTTPS VirusTotal URL: {url!r}")
    headers = {"x-apikey": api_key}

    if files:
        boundary = "----VTBoundary0xideSLOC"
        body_parts = []
        for field, (filename, file_bytes, content_type) in files.items():
            part = (
                f"--{boundary}\r\n"
                f'Content-Disposition: form-data; name="{field}"; filename="{filename}"\r\n'
                f"Content-Type: {content_type}\r\n\r\n"
            ).encode() + file_bytes + b"\r\n"
            body_parts.append(part)
        body = b"".join(body_parts) + f"--{boundary}--\r\n".encode()
        headers["Content-Type"] = f"multipart/form-data; boundary={boundary}"
        req = urllib.request.Request(url, data=body, headers=headers, method=method)
    else:
        req = urllib.request.Request(url, headers=headers, method=method)

    try:
        with urllib.request.urlopen(req, timeout=60) as resp:  # nosec B310 - URL is validated HTTPS-only above (non-HTTPS raises ValueError); errors caught below
            return json.loads(resp.read().decode())
    except urllib.error.HTTPError as exc:
        body = exc.read().decode(errors="replace")
        raise RuntimeError(f"HTTP {exc.code}: {body[:400]}") from exc


def scan_file(path, api_key):
    name = os.path.basename(path)
    print(f"\n{'='*60}")
    print(f"File : {name}")
    print(f"Path : {path}")
    print(f"Size : {os.path.getsize(path):,} bytes")

    with open(path, "rb") as fh:
        data = fh.read()

    print("Uploading to VirusTotal...", flush=True)
    try:
        resp = vt_request(
            "POST",
            "https://www.virustotal.com/api/v3/files",
            files={"file": (name, data, "application/octet-stream")},
            api_key=api_key,
        )
    except RuntimeError as exc:
        print(f"  ERROR: upload failed - {exc}", file=sys.stderr)
        return False

    analysis_id = resp.get("data", {}).get("id", "")
    if not analysis_id:
        print("  ERROR: no analysis ID in response", file=sys.stderr)
        return False

    report_url = f"https://www.virustotal.com/gui/analysis/{analysis_id}"
    print(f"Analysis ID : {analysis_id}")
    print(f"Live report : {report_url}")
    print(f"Polling every {POLL_INTERVAL}s (max {POLL_ATTEMPTS} attempts)...", flush=True)

    stats = None
    for attempt in range(1, POLL_ATTEMPTS + 1):
        time.sleep(POLL_INTERVAL)
        try:
            poll = vt_request(
                "GET",
                f"https://www.virustotal.com/api/v3/analyses/{analysis_id}",
                api_key=api_key,
            )
        except RuntimeError as exc:
            print(f"  Attempt {attempt}/{POLL_ATTEMPTS}: poll error - {exc}", file=sys.stderr)
            continue

        status = poll.get("data", {}).get("attributes", {}).get("status", "unknown")
        print(f"  Attempt {attempt}/{POLL_ATTEMPTS}: {status}", flush=True)
        if status == "completed":
            stats = poll["data"]["attributes"].get("stats", {})
            break

    if stats is None:
        print(f"  Scan did not complete within {POLL_ATTEMPTS * POLL_INTERVAL}s.")
        print(f"  Check the live report manually: {report_url}")
        return True

    malicious   = stats.get("malicious",   0)
    suspicious  = stats.get("suspicious",  0)
    undetected  = stats.get("undetected",  0)
    harmless    = stats.get("harmless",    0)
    total       = malicious + suspicious + undetected + harmless

    print()
    print(f"  Malicious  : {malicious:>4}  /  {total}")
    print(f"  Suspicious : {suspicious:>4}  /  {total}")
    print(f"  Undetected : {undetected:>4}  /  {total}")
    print(f"  Harmless   : {harmless:>4}  /  {total}")
    print(f"  Report     : {report_url}")

    if malicious > 0 or suspicious > 0:
        print()
        print("  NOTE: One or more engines flagged this binary.")
        print("  False positives are common for freshly compiled Rust binaries.")
        print("  Open the report URL above and click 'Is this a false positive?'")
        print("  to submit a dispute to the flagging vendor(s).")

    return True


def main():
    # Windows consoles default to cp1252, which cannot encode some glyphs; force
    # UTF-8 so a print never aborts the scan. Best-effort.
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except Exception:
            pass
    api_key = os.environ.get("VT_API_KEY", "")
    if not api_key:
        print("ERROR: VT_API_KEY environment variable is not set.", file=sys.stderr)
        print()
        print("  Linux/macOS:  VT_API_KEY=<key> python3 scripts/internal/vt-scan.py <file>")
        print("  PowerShell:   $env:VT_API_KEY='<key>'; python3 scripts/internal/vt-scan.py <file>")
        sys.exit(1)

    files = sys.argv[1:]
    if not files:
        print("ERROR: no files specified.", file=sys.stderr)
        print()
        print("Usage: python3 scripts/internal/vt-scan.py <file> [<file2> ...]")
        print()
        print("Build a binary first:")
        print("  cargo build --release")
        print("  # binary: target/release/oxide-sloc  (or oxide-sloc.exe on Windows)")
        sys.exit(1)

    missing = [f for f in files if not os.path.isfile(f)]
    if missing:
        for f in missing:
            print(f"ERROR: file not found: {f}", file=sys.stderr)
        sys.exit(1)

    print(f"VirusTotal scan - {len(files)} file(s)")
    print(f"Timestamp: {time.strftime('%Y-%m-%d %H:%M:%S UTC', time.gmtime())}")

    any_failed = False
    for i, path in enumerate(files):
        ok = scan_file(path, api_key)
        if not ok:
            any_failed = True
        if i < len(files) - 1:
            print(f"\nWaiting {UPLOAD_DELAY}s before next upload (free-tier rate limit)...", flush=True)
            time.sleep(UPLOAD_DELAY)

    print(f"\n{'='*60}")
    print("Done.")
    sys.exit(2 if any_failed else 0)


if __name__ == "__main__":
    main()
