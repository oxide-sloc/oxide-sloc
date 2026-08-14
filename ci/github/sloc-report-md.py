#!/usr/bin/env python3
"""Render an oxide-sloc result (and optional diff) as Markdown.

Used for both the GitHub Actions job summary ($GITHUB_STEP_SUMMARY) and the
sticky PR comment, so they stay in sync. Prints Markdown to stdout.

Usage:
    python3 ci/github/sloc-report-md.py <result.json> [diff.json]
"""

import json
import sys

MARKER = "<!-- oxide-sloc-report -->"  # sticky-comment identity marker
_TABLE_SEP = "|---|--:|"  # right-aligned two-column Markdown table separator


def _fmt(n) -> str:
    try:
        v = int(n)
    except (TypeError, ValueError):
        return str(n)
    a = abs(v)
    if a >= 1_000_000:
        return f"{v / 1_000_000:.1f}".rstrip("0").rstrip(".") + "M"
    if a >= 10_000:
        return f"{round(v / 1000)}K"
    return f"{v:,}"


def _delta(n) -> str:
    try:
        v = int(n)
    except (TypeError, ValueError):
        return ""
    if v == 0:
        return "±0"
    return ("🟢 +" if v > 0 else "🔴 −") + _fmt(abs(v))


def _load(path):
    try:
        with open(path, encoding="utf-8") as fh:
            return json.load(fh)
    except (OSError, ValueError):
        return None


def _header_lines(tool) -> list:
    out = [MARKER, "## 📊 oxide-sloc report", ""]
    ver = tool.get("version", "")
    if ver:
        out.append(f"<sub>oxide-sloc v{ver}</sub>\n")
    return out


def _summary_lines(t) -> list:
    return [
        "| Metric | Value |",
        _TABLE_SEP,
        f"| Code lines | **{_fmt(t.get('code_lines', 0))}** |",
        f"| Comment lines | {_fmt(t.get('comment_lines', 0))} |",
        f"| Blank lines | {_fmt(t.get('blank_lines', 0))} |",
        f"| Files analyzed | {_fmt(t.get('files_analyzed', 0))} |",
        "",
    ]


def _delta_lines(diff) -> list:
    # Optional delta vs base branch, from `oxide-sloc diff` (ScanComparison JSON).
    if not isinstance(diff, dict):
        return []
    d = diff.get("summary", {}) if isinstance(diff.get("summary"), dict) else {}
    cl = d.get("code_lines_delta")
    if cl is None:
        return []
    return [
        "### Change vs base branch",
        "",
        "| Metric | Δ |",
        _TABLE_SEP,
        f"| Code lines | {_delta(cl)} |",
        f"| Comment lines | {_delta(d.get('comment_lines_delta', 0))} |",
        f"| Files | {_delta(d.get('files_analyzed_delta', 0))} |",
        "",
    ]


def _language_lines(data) -> list:
    # Top languages by code lines.
    langs = []
    for e in data.get("totals_by_language", []):
        if not isinstance(e, dict):
            continue
        name = e.get("language", "?")
        if isinstance(name, dict):
            name = name.get("display_name") or name.get("name") or "?"
        langs.append((str(name), int(e.get("code_lines", 0))))
    langs.sort(key=lambda x: x[1], reverse=True)
    if not langs:
        return []
    out = ["### Language breakdown", "", "| Language | Code lines |", _TABLE_SEP]
    for name, code in langs[:10]:
        out.append(f"| {name} | {_fmt(code)} |")
    out.append("")
    return out


def main() -> None:
    try:
        sys.stdout.reconfigure(encoding="utf-8")  # emoji-safe on any console
    except Exception:
        pass
    if len(sys.argv) < 2:
        sys.stderr.write("usage: sloc-report-md.py <result.json> [diff.json]\n")
        sys.exit(2)
    data = _load(sys.argv[1]) or {}
    diff = _load(sys.argv[2]) if len(sys.argv) >= 3 else None

    t = data.get("summary_totals", {})
    tool = data.get("tool", {}) if isinstance(data.get("tool"), dict) else {}
    out = _header_lines(tool)
    out += _summary_lines(t)
    out += _delta_lines(diff)
    out += _language_lines(data)
    print("\n".join(out))


if __name__ == "__main__":
    main()
