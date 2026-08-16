#!/usr/bin/env python3
"""Render a rich Jenkins build summary from an oxide-sloc result.json.

Writes two artifacts the pipeline consumes in post{success}:

  <out>/build-description.txt
      A multi-line, Unicode bar-chart description set as currentBuild.description.
      Uses only newlines + Unicode block glyphs, so it renders identically under
      Jenkins' Plain-text markup formatter (the default) — no HTML, no plugin,
      no system config required. This is the guaranteed-visible upgrade over the
      old single plain line.

  <out>/build-summary.html
      A styled HTML panel (metric table + coloured language bars + test/coverage
      chips) for the badge plugin's createSummary() box on the build page. Shown
      only when the badge plugin is installed; degrades away cleanly otherwise.

Standard library only.

Usage:
    python3 ci/jenkins/build-summary.py <result.json> <out-dir> \
        [--scan-path P] [--tests "128/130"] [--coverage 82.4] [--style "4-Space · 73% 80-col"]
"""

import html
import json
import os
import sys

# Oxide palette (matches the dashboard / CLAUDE.md canon).
_PALETTE = ["#c45c10", "#2a6846", "#d4a017", "#4472c4", "#805099",
            "#2e75b6", "#70ad47", "#e07038", "#9e480e", "#156082"]
_BAR_CELLS = 22  # width of the Unicode bar in cells


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


def _arg(flag, default=None):
    if flag in sys.argv:
        i = sys.argv.index(flag)
        if i + 1 < len(sys.argv):
            return sys.argv[i + 1]
    return default


def _languages(data):
    rows = []
    for e in data.get("totals_by_language", []):
        if not isinstance(e, dict):
            continue
        name = e.get("language", "?")
        if isinstance(name, dict):
            name = name.get("display_name") or name.get("name") or "?"
        rows.append((str(name), int(e.get("code_lines", 0))))
    rows.sort(key=lambda x: x[1], reverse=True)
    return rows


def _unicode_bar(value, max_value, cells=_BAR_CELLS):
    if max_value <= 0:
        return "░" * cells
    filled = int(round(cells * value / max_value))
    filled = max(1 if value > 0 else 0, min(cells, filled))
    return "█" * filled + "░" * (cells - filled)


def build_description(data, scan_path, tests, coverage, style):
    """Plain-text, emoji-free description.

    Jenkins renders this in a proportional font, so we never rely on
    space-padded columns to line up *before* variable-width text. The language
    chart puts a FIXED-width bar first (every bar is the same number of block
    glyphs), so the value + label that follow it start at the same x on every
    row regardless of the page font.
    """
    t = data.get("summary_totals", {})
    tool = data.get("tool", {}) if isinstance(data.get("tool"), dict) else {}
    langs = _languages(data)
    maxv = langs[0][1] if langs else 0

    title = f"oxide-sloc  -  {scan_path}" if scan_path else "oxide-sloc report"
    lines = [title, "-" * 60]

    lines.append(
        "Totals:   "
        f"{_fmt(t.get('code_lines', 0))} code, "
        f"{_fmt(t.get('comment_lines', 0))} comments, "
        f"{_fmt(t.get('blank_lines', 0))} blank, "
        f"{_fmt(t.get('files_analyzed', 0))} files"
    )
    if style:
        lines.append(f"Style:    {style}")
    quality = []
    if tests:
        quality.append(f"tests {tests}")
    if coverage not in (None, "", "N/A"):
        quality.append(f"coverage {coverage}%")
    if quality:
        lines.append("Quality:  " + ", ".join(quality))
    if tool.get("version"):
        lines.append(f"Version:  oxide-sloc {tool['version']}")

    if langs:
        top = langs[:8]
        cw = max(len(_fmt(v)) for _, v in top)
        lines.append("")
        lines.append("Languages by code lines")
        for name, code in top:
            lines.append(f"  {_unicode_bar(code, maxv)}  {_fmt(code).rjust(cw)}  {name}")
        if len(langs) > len(top):
            lines.append(f"  ... and {len(langs) - len(top)} more")
    return "\n".join(lines)


def build_html(data, scan_path, tests, coverage, style):
    t = data.get("summary_totals", {})
    tool = data.get("tool", {}) if isinstance(data.get("tool"), dict) else {}
    langs = _languages(data)
    maxv = langs[0][1] if langs else 1

    def esc(x):
        return html.escape(str(x))

    metric_cells = "".join(
        f'<td style="padding:6px 14px;text-align:center">'
        f'<div style="font-size:20px;font-weight:800;color:#c45c10">{_fmt(v)}</div>'
        f'<div style="font-size:10px;text-transform:uppercase;letter-spacing:.06em;'
        f'color:#8a6a5a">{esc(lbl)}</div></td>'
        for lbl, v in [
            ("Code", t.get("code_lines", 0)),
            ("Comments", t.get("comment_lines", 0)),
            ("Blank", t.get("blank_lines", 0)),
            ("Files", t.get("files_analyzed", 0)),
        ]
    )

    bars = []
    for i, (name, code) in enumerate(langs[:8]):
        colour = _PALETTE[i % len(_PALETTE)]
        pct = code / maxv * 100 if maxv else 0
        bars.append(
            '<div style="display:flex;align-items:center;gap:8px;margin:3px 0;font-size:12px">'
            f'<span style="width:88px;font-weight:600;color:#2d1a0e;text-align:right">{esc(name)}</span>'
            f'<span style="flex:0 0 220px;background:#efe6dd;border-radius:4px;overflow:hidden">'
            f'<span style="display:block;height:13px;width:{pct:.1f}%;background:{colour}"></span></span>'
            f'<span style="color:#5a3820;font-weight:700">{_fmt(code)}</span></div>'
        )

    chips = []
    if style:
        chips.append(f"<b>Style</b> {esc(style)}")
    if tests:
        chips.append(f"<b>Tests</b> {esc(tests)}")
    if coverage not in (None, "", "N/A"):
        chips.append(f"<b>Coverage</b> {esc(coverage)}%")
    chip_row = (
        '<div style="margin:8px 0;font-size:12px;color:#5a3820">'
        + "  &middot;  ".join(chips) + "</div>"
    ) if chips else ""

    ver = f" · v{esc(tool['version'])}" if tool.get("version") else ""
    return (
        '<div style="font-family:Inter,system-ui,Segoe UI,sans-serif;max-width:640px">'
        f'<div style="font-weight:800;color:#c45c10;font-size:14px;margin-bottom:6px">'
        f'oxide-sloc — {esc(scan_path or "report")}{ver}</div>'
        '<table style="border-collapse:collapse;background:#faf6f1;border:1px solid #e0d8d0;'
        f'border-radius:10px;margin-bottom:8px"><tr>{metric_cells}</tr></table>'
        f'{chip_row}'
        '<div style="font-size:11px;font-weight:700;text-transform:uppercase;letter-spacing:.06em;'
        'color:#8a6a5a;margin:6px 0 2px">Language breakdown</div>'
        + "".join(bars) +
        "</div>"
    )


def main() -> None:
    try:
        sys.stdout.reconfigure(encoding="utf-8")
    except Exception:
        # Best-effort: older Python or a non-reconfigurable stream is fine as-is.
        pass
    if len(sys.argv) < 3:
        sys.stderr.write("usage: build-summary.py <result.json> <out-dir> [--scan-path ..]\n")
        sys.exit(2)
    src, out_dir = sys.argv[1], sys.argv[2]
    scan_path = _arg("--scan-path", "")
    tests = _arg("--tests", "")
    coverage = _arg("--coverage", "")
    style = _arg("--style", "")

    with open(src, encoding="utf-8") as fh:
        data = json.load(fh)

    os.makedirs(out_dir, exist_ok=True)
    desc = build_description(data, scan_path, tests, coverage, style)
    html_panel = build_html(data, scan_path, tests, coverage, style)
    with open(os.path.join(out_dir, "build-description.txt"), "w", encoding="utf-8") as fh:
        fh.write(desc)
    with open(os.path.join(out_dir, "build-summary.html"), "w", encoding="utf-8") as fh:
        fh.write(html_panel)
    print(desc)


if __name__ == "__main__":
    main()
