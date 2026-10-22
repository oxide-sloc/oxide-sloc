#!/usr/bin/env python3
"""Extract inline CSS and JS from an oxide-sloc HTML report into companion files.

Usage: python3 extract-report-assets.py <html-file>

Writes <basename>.css and <basename>.js alongside the HTML, then rewrites the
HTML to reference them via <link> / <script src>.  This lets the SLOC Report
render correctly under Jenkins's default Content Security Policy (which blocks
unsafe-inline styles and scripts) without requiring CSP relaxation via the
Script Console or init.groovy.d.

The original oxide-sloc report is intentionally self-contained (all CSS and JS
inline) so it can be shared as a single file.  This script creates a
Jenkins-compatible sidecar version only; it does not change how oxide-sloc
generates reports.
"""

import os
import re
import sys


def _force_utf8_console() -> None:
    # Jenkins on Windows defaults stdout/stderr to cp1252, which cannot encode
    # report glyphs; force UTF-8 so a print never aborts the build. Best-effort:
    # older Python or a non-reconfigurable stream is fine as-is.
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except Exception:
            pass


def main() -> None:
    _force_utf8_console()
    if len(sys.argv) != 2:
        print(f"Usage: {sys.argv[0]} <html-file>", file=sys.stderr)
        sys.exit(1)

    html_path = sys.argv[1]
    if not os.path.isfile(html_path):
        print(f"File not found: {html_path}", file=sys.stderr)
        sys.exit(1)

    base = os.path.splitext(html_path)[0]
    css_path = base + ".css"
    js_path = base + ".js"
    css_name = os.path.basename(css_path)
    js_name = os.path.basename(js_path)

    with open(html_path, "r", encoding="utf-8") as fh:
        html = fh.read()

    css_chunks: list[str] = []
    js_chunks: list[str] = []
    css_link_inserted = False

    def _replace_style(m: re.Match) -> str:
        nonlocal css_link_inserted
        css_chunks.append(m.group(1))
        if css_link_inserted:
            return ""
        css_link_inserted = True
        return f'<link rel="stylesheet" href="{css_name}">'

    def _replace_script(m: re.Match) -> str:
        content = m.group(1).strip()
        if content:
            js_chunks.append(content)
        return ""

    html = re.sub(
        r"<style[^>]*>(.*?)</style>", _replace_style, html, flags=re.DOTALL | re.IGNORECASE
    )
    html = re.sub(
        r"<script[^>]*>(.*?)</script>", _replace_script, html, flags=re.DOTALL | re.IGNORECASE
    )

    if js_chunks:
        html = html.replace("</body>", f'<script src="{js_name}"></script>\n</body>', 1)

    if css_chunks:
        with open(css_path, "w", encoding="utf-8") as fh:
            fh.write("\n".join(css_chunks))
        print(f"CSS -> {css_path} ({os.path.getsize(css_path):,} bytes)")

    if js_chunks:
        with open(js_path, "w", encoding="utf-8") as fh:
            fh.write("\n".join(js_chunks))
        print(f"JS  -> {js_path} ({os.path.getsize(js_path):,} bytes)")

    with open(html_path, "w", encoding="utf-8") as fh:
        fh.write(html)
    print(f"HTML updated: {html_path}")


if __name__ == "__main__":
    main()
