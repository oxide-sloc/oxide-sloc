"""oxide-sloc CI build dashboard generator.

Reads CI output artifacts from <output-dir> and writes a single self-contained
HTML file with inline SVG charts and CSS.  Requires only Python 3 standard library —
no pip installs, no external JS libraries, and no Jenkins plugins beyond the core
``archiveArtifacts`` step.

Usage
-----
    python3 ci/jenkins/generate-dashboard.py <output-dir> [project-slug] [history-file]

Arguments
---------
output-dir
    Directory that contains the CI output artifacts produced by the Jenkinsfile.
    Typically the value of the ``OUTPUT_SUBDIR`` pipeline parameter (e.g. ``ci-out``).

project-slug
    Optional.  If omitted the script auto-detects it from the first
    ``result_*.json`` file found in <output-dir>.

Inputs (all relative to <output-dir>)
--------------------------------------
result_<slug>.json
    SLOC metrics produced by ``oxide-sloc analyze --json-out``.  Required.
test-results/junit.xml
    JUnit XML produced by cargo-nextest.  Optional.
coverage/lcov.info
    LCOV coverage data produced by cargo-llvm-cov.  Optional.

Output
------
<output-dir>/dashboard_<slug>.html
    Self-contained HTML file.  Path is printed to stdout on success.

Environment variables read
--------------------------
BUILD_NUMBER   Jenkins build number shown in the header.
BUILD_URL      Jenkins build URL — used for the "← Back to Jenkins build" link.
JOB_NAME       Jenkins job name shown in the header.
SCAN_PATH      Path that was scanned (shown in the header).
"""

import html
import json
import os
import re
import shutil
import sys
from datetime import datetime, timedelta, timezone
from glob import glob
from typing import Optional
import csv

# XML parsing uses defusedxml exclusively — it hardens against entity expansion (billion
# laughs), external-entity resolution, and DTD retrieval. If defusedxml is unavailable we
# skip parsing report XML rather than fall back to a parser that does not harden entities.
try:
    import defusedxml.ElementTree as ET
    _XML_SAFE = True
except ImportError:  # pragma: no cover
    ET = None  # type: ignore[assignment]
    _XML_SAFE = False

_MAX_XML_BYTES = 10 * 1024 * 1024  # 10 MB — guard against oversized JUnit/coverage files


# ---------------------------------------------------------------------------
# Timestamp handling — render in the timezone the oxide-sloc app uses (PT by
# default), mirroring the web UI's fmt_la_time / is_pacific_dst logic so the CI
# dashboard agrees with the interactive report. Standard library only — no
# tzdata dependency required for the default Pacific path.
# ---------------------------------------------------------------------------

def _parse_iso_utc(raw) -> Optional[datetime]:
    """Parse an RFC 3339 / ISO-8601 timestamp (e.g. ``2026-06-26T03:08:00Z``) to
    a timezone-aware UTC datetime. Returns None if it can't be parsed."""
    if not isinstance(raw, str) or not raw.strip():
        return None
    s = raw.strip().replace("Z", "+00:00")
    # Trim sub-second precision beyond microseconds (Rust may emit nanoseconds).
    s = re.sub(r"(\.\d{6})\d+", r"\1", s)
    try:
        dt = datetime.fromisoformat(s)
    except ValueError:
        return None
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return dt.astimezone(timezone.utc)


def _nth_sunday_utc(year: int, month: int, n: int, hour: int) -> datetime:
    """UTC datetime of the nth Sunday of a month at the given hour."""
    first = datetime(year, month, 1, tzinfo=timezone.utc)
    # datetime.weekday(): Monday=0 … Sunday=6
    first_sunday = 1 + (6 - first.weekday()) % 7
    day = first_sunday + (n - 1) * 7
    return datetime(year, month, day, hour, 0, 0, tzinfo=timezone.utc)


def _pacific(dt_utc: datetime):
    """Return (utc_offset, label) for US Pacific time at the given UTC instant.

    DST starts the 2nd Sunday of March at 02:00 PST (10:00 UTC) and ends the
    1st Sunday of November at 02:00 PDT (09:00 UTC) — matches sloc-web.
    """
    y = dt_utc.year
    start = _nth_sunday_utc(y, 3, 2, 10)
    end = _nth_sunday_utc(y, 11, 1, 9)
    if start <= dt_utc < end:
        return timedelta(hours=-7), "PDT"
    return timedelta(hours=-8), "PST"


def fmt_local(dt_utc: datetime) -> str:
    """Format a UTC datetime in the app's configured timezone.

    Defaults to US Pacific (PT). Set ``SLOC_REPORT_TZ`` to an IANA name (e.g.
    ``America/New_York``) to override; falls back to Pacific if zoneinfo or the
    tzdata for that zone is unavailable.
    """
    tz_name = os.environ.get("SLOC_REPORT_TZ", "").strip()
    if tz_name and tz_name != "America/Los_Angeles":
        try:
            from zoneinfo import ZoneInfo  # Python 3.9+

            local = dt_utc.astimezone(ZoneInfo(tz_name))
            return local.strftime("%Y-%m-%d %H:%M %Z")
        except Exception:
            pass  # fall through to Pacific
    off, label = _pacific(dt_utc)
    return (dt_utc + off).strftime(f"%Y-%m-%d %H:%M {label}")


def _strip_verbatim_prefix(path: str) -> str:
    """Strip a Windows verbatim/UNC path prefix so a portable path is shown.

    Handles ``\\\\?\\C:\\...`` and ``//?/C:/...`` (extended-length prefix) plus the
    UNC form ``//?/UNC/server/share`` → ``//server/share``. Older result JSON
    (produced before the Rust side cleaned the absolute path at the source) can
    still carry these; this keeps the dashboard rendering clean regardless.
    """
    if not path:
        return path
    p = path.replace("\\", "/")
    # //?/UNC/server/share  ->  //server/share
    low = p.lower()
    if low.startswith("//?/unc/"):
        return "//" + p[len("//?/unc/"):]
    if low.startswith("//?/"):
        return p[len("//?/"):]
    return p


def _display_file_path(rec: dict) -> str:
    """Portable display path for a per-file record.

    Prefer ``relative_path`` (portable, no absolute-path leak). Fall back to the
    basename of the absolute ``path`` when ``relative_path`` is missing/empty.
    A leading Windows verbatim/UNC prefix is stripped defensively in either case.
    """
    rel = str(rec.get("relative_path", "") or "").strip()
    if rel:
        return _strip_verbatim_prefix(rel).lstrip("/")
    raw = _strip_verbatim_prefix(str(rec.get("path", "") or "").strip())
    # Basename of the (cleaned) absolute path.
    return raw.rstrip("/").split("/")[-1] if raw else ""


def _norm_repo_url(url) -> str:
    """Normalize a repo URL for cross-run comparison.

    Strips a trailing ``.git``, any ``user:token@`` / ``user@`` userinfo, a URL
    scheme, and a trailing slash, then lower-cases the result. This lets rows that
    differ only by credentials, protocol, or a ``.git`` suffix match as the same
    project. Returns "" for a blank/None input.
    """
    if not url:
        return ""
    s = str(url).strip()
    if not s:
        return ""
    # Drop scheme (https://, ssh://, git://, file://, ...).
    s = re.sub(r"^[A-Za-z][A-Za-z0-9+.\-]*://", "", s)
    # Drop scp-like git@host: — keep host:path but remove the leading userinfo.
    s = re.sub(r"^[^@/]+@", "", s)
    # Normalize scp-style "host:path" to "host/path" so it matches the URL form.
    s = s.replace(":", "/", 1) if (":" in s and "/" not in s.split(":", 1)[0]) else s
    s = re.sub(r"\.git$", "", s)
    s = s.rstrip("/")
    return s.lower()


def _repo_name(url) -> str:
    """Human repo name (last path segment, .git stripped) from a URL. "" if blank."""
    if not url:
        return ""
    s = str(url).strip()
    s = re.sub(r"\.git$", "", s)
    s = re.sub(r"[/:]+$", "", s)
    seg = re.split(r"[/:]", s)
    return seg[-1] if seg and seg[-1] else ""


def _delta_color(delta: float) -> str:
    """Colour for a signed delta: green up, red down, neutral brown at zero."""
    if delta > 0:
        return "#2a6846"
    if delta < 0:
        return "#b23030"
    return "#8a6a5a"


# ---------------------------------------------------------------------------
# Number formatting (matches the CLAUDE.md canonical spec)
# ---------------------------------------------------------------------------

def fmt(n: int) -> str:
    """Compact number formatter.

    < 1,000          → as-is with Python locale comma (e.g. "2", "947")
    1,000 - 9,999    → comma-separated (e.g. "1,247")
    10,000 - 999,999 → integer K  (e.g. "15K", "100K")
    >= 1,000,000     → one-decimal M, trailing .0 stripped (e.g. "3.8M")
    """
    v = int(n)
    a = abs(v)
    if a >= 1_000_000:
        m = v / 1_000_000
        s = f"{m:.1f}M"
        return s.replace(".0M", "M")
    if a >= 10_000:
        return f"{round(v / 1_000)}K"
    return f"{v:,}"


def fmt_full(n: int) -> str:
    """Full precision comma-formatted number for tooltips."""
    return f"{int(n):,}"


# ---------------------------------------------------------------------------
# SVG helpers
# ---------------------------------------------------------------------------

# Ten-colour oxide palette (warm browns / terracottas)
_OXIDE_PALETTE = [
    "#b04a00", "#d4691c", "#a03800", "#cc5800",
    "#e07038", "#8a3800", "#e89060", "#c04818",
    "#f0a070", "#943000",
]


def svg_hbar(data: list) -> str:
    """Horizontal SVG bar chart.

    Parameters
    ----------
    data : list of (label, value) tuples, sorted descending by value.

    Returns
    -------
    Plain SVG string (no surrounding <div>).
    """
    bar_h = 26
    gap = 7
    label_w = 140
    bar_max = 340
    count_w = 90
    total_w = label_w + bar_max + count_w + 16  # 16px padding

    if not data:
        return '<svg width="1" height="1"></svg>'

    max_val = max(v for _, v in data) or 1
    total_val = sum(v for _, v in data) or 1
    row_h = bar_h + gap
    svg_h = row_h * len(data) + gap

    rows = []
    hits = []
    for i, (label, value) in enumerate(data):
        colour = _OXIDE_PALETTE[i % len(_OXIDE_PALETTE)]
        y = i * row_h + gap

        # Truncate long labels
        disp_label = label if len(label) <= 18 else label[:17] + "…"
        bar_w = int(bar_max * value / max_val)
        pct = value / total_val * 100
        # Tooltip text — e.g. "cpp = 603 code lines (73.0% of total)". Used by
        # both the JS interactive tooltip (data-tip) and the native <title> fallback.
        tip = f"{label} = {fmt_full(value)} code lines ({pct:.1f}% of total)"

        rows.append(
            f'  <!-- {html.escape(label)} -->'
            f'\n  <text x="{label_w - 6}" y="{y + bar_h - 8}"'
            f' text-anchor="end" fill="#2d1a0e" font-size="13" font-family="system-ui,sans-serif"'
            f' font-weight="600">{html.escape(disp_label)}</text>'
            f'\n  <rect class="bar-rect" data-i="{i}" x="{label_w}" y="{y}"'
            f' width="{bar_w}" height="{bar_h}" rx="4" fill="{colour}"/>'
            f'\n  <text x="{label_w + bar_w + 8}" y="{y + bar_h - 8}"'
            f' fill="#5a3820" font-size="12" font-family="system-ui,sans-serif"'
            f' font-weight="700">{html.escape(fmt(value))}</text>'
        )

        # Full-row transparent hit target so hovering ANYWHERE on the row (label,
        # bar, or count) triggers the tooltip + bar "pop" animation — essential for
        # tiny bars whose painted width rounds to ~0 (e.g. "c = 1"). data-i links
        # it to its .bar-rect; data-tip drives the JS tooltip; <title> is the
        # no-JS fallback. pointer-events="all" makes the transparent fill hoverable.
        hits.append(
            f'\n  <rect class="bar-hit" data-i="{i}" data-tip="{html.escape(tip)}"'
            f' x="0" y="{i * row_h}" width="{total_w}"'
            f' height="{row_h}" fill="#000000" fill-opacity="0" pointer-events="all">'
            f'<title>{html.escape(tip)}</title></rect>'
        )

    # Hit targets are appended last so they sit on top of every painted element.
    body = "\n".join(rows) + "\n" + "\n".join(hits)
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{total_w}" height="{svg_h}"'
        f' role="img" aria-label="Language breakdown bar chart">'
        f'\n{body}\n</svg>'
    )


def svg_progress(pct: float, w: int = 300, h: int = 22) -> str:
    """SVG progress bar for a coverage percentage.

    Fill colour:
      >= 70 %  → #2a6846 (green)
      >= 40 %  → #d4851c (amber)
      < 40 %   → #b23030 (red)
    """
    if pct >= 70.0:
        fill = "#2a6846"
    elif pct >= 40.0:
        fill = "#d4851c"
    else:
        fill = "#b23030"

    fill_w = max(0, min(w, int(w * pct / 100)))
    label = f"{pct:.1f}%"

    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}"'
        f' role="progressbar" aria-valuenow="{pct:.1f}" aria-valuemin="0" aria-valuemax="100">'
        f'\n  <rect width="{w}" height="{h}" rx="6" fill="#e8ddd8"/>'
        f'\n  <rect width="{fill_w}" height="{h}" rx="6" fill="{fill}"/>'
        f'\n  <text x="{w // 2}" y="{h - 6}" text-anchor="middle"'
        f' fill="#ffffff" font-size="12" font-family="system-ui,sans-serif"'
        f' font-weight="700">{html.escape(label)}</text>'
        f'\n</svg>'
    )


def svg_sparkline(points: list, width: int = 380, height: int = 80) -> str:
    """SVG line sparkline for build-over-build trend data.

    points: list of (build_number, value) pairs, oldest first.
    """
    if len(points) < 2:
        return ""

    values = [v for _, v in points]
    builds = [b for b, _ in points]
    min_v = min(values)
    max_v = max(values)
    value_range = max_v - min_v or 1

    pad_l, pad_r, pad_t, pad_b = 12, 12, 10, 20
    w = width - pad_l - pad_r
    h = height - pad_t - pad_b
    n = len(points)

    def px(i: int, v: int):
        x = pad_l + int(w * i / (n - 1))
        y = pad_t + int(h * (1.0 - (v - min_v) / value_range))
        return x, y

    coords = [px(i, v) for i, v in enumerate(values)]
    path_d = " ".join(f"{'M' if i == 0 else 'L'}{x},{y}" for i, (x, y) in enumerate(coords))
    area_d = (
        path_d
        + f" L{coords[-1][0]},{pad_t + h} L{coords[0][0]},{pad_t + h} Z"
    )

    max_i = values.index(max_v)
    min_i = values.index(min_v)

    # Visible per-point dots (green = peak, red = trough, oxide = latest/other),
    # each tagged data-i so the JS can grow the exact point on hover.
    visible_dots = ""
    for i, (x, y) in enumerate(coords):
        if i == n - 1:
            col, r = "#b04a00", 4
        elif i == max_i:
            col, r = "#2a6846", 3
        elif i == min_i:
            col, r = "#b23030", 3
        else:
            col, r = "#b04a00", 3
        visible_dots += (
            f'\n  <circle class="spark-pt" data-i="{i}" cx="{x}" cy="{y}"'
            f' r="{r}" fill="{col}"/>'
        )

    # Transparent, generously sized hit-target dots at every build. data-i links
    # each to its visible .spark-pt (grown on hover); data-tip drives the JS
    # tooltip; <title> is the no-JS fallback. pointer-events="all" is essential:
    # a fully transparent fill is "not painted" under visiblePainted, so without
    # it the circle would receive no hover events at all.
    hover_dots = ""
    for i, (x, y) in enumerate(coords):
        b = builds[i]
        v = values[i]
        note = ""
        if i > 0:
            d = v - values[i - 1]
            note = f" ({'+' if d > 0 else ''}{fmt_full(d)} vs #{builds[i - 1]})"
        tip = f"Build #{b} = {fmt_full(v)} code lines{note}"
        hover_dots += (
            f'\n  <circle class="spark-dot" data-i="{i}" data-tip="{html.escape(tip)}"'
            f' cx="{x}" cy="{y}" r="9" fill="#b04a00"'
            f' fill-opacity="0" pointer-events="all">'
            f'<title>{html.escape(tip)}</title></circle>'
        )

    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}"'
        f' role="img" aria-label="SLOC trend over last {n} builds">'
        f'\n  <path d="{area_d}" fill="#b04a00" fill-opacity="0.07"/>'
        f'\n  <path d="{path_d}" fill="none" stroke="#b04a00" stroke-width="2"'
        f' stroke-linejoin="round" stroke-linecap="round"/>'
        f"{visible_dots}"
        f"{hover_dots}"
        f'\n  <text x="{pad_l}" y="{height - 3}" font-size="10" fill="#8a6a5a"'
        f' font-family="system-ui,sans-serif">#{html.escape(str(builds[0]))}</text>'
        f'\n  <text x="{width - pad_r}" y="{height - 3}" font-size="10" fill="#8a6a5a"'
        f' text-anchor="end" font-family="system-ui,sans-serif">#{html.escape(str(builds[-1]))}</text>'
        f'\n</svg>'
    )


# ---------------------------------------------------------------------------
# HTML component helpers
# ---------------------------------------------------------------------------

def chip(
    val_html: str,
    label: str,
    sub: Optional[str] = None,
    tip: Optional[str] = None,
) -> str:
    """Render a stat chip card.

    Parameters
    ----------
    val_html
        The large value shown at the top.  May contain intentional inline HTML
        such as ``<span style="color:green">OK</span>`` — NOT escaped here.
    label
        Short uppercase label shown below the value.  Will be html-escaped.
    sub
        Optional small supplementary text below the label.  Will be html-escaped.
    tip
        Optional description shown in a hover tooltip explaining the metric.
        Will be html-escaped.
    """
    sub_html = (
        f'<div class="chip-sub">{html.escape(str(sub))}</div>' if sub is not None else ""
    )
    tip_html = (
        f'<div class="stat-chip-tip">{html.escape(str(tip))}</div>' if tip else ""
    )
    return (
        f'<div class="stat-chip">'
        f'<div class="stat-chip-val">{val_html}</div>'
        f'<div class="stat-chip-label">{html.escape(str(label))}</div>'
        f'{sub_html}'
        f'{tip_html}'
        f'</div>'
    )


# ---------------------------------------------------------------------------
# Input parsers
# ---------------------------------------------------------------------------

def parse_junit(path: str) -> Optional[dict]:
    """Parse a JUnit XML file.

    Handles both ``<testsuites>`` wrapper root and bare ``<testsuite>`` root.

    Returns a dict with keys: tests, failures, errors, skipped, passed, time.
    Returns None on any error (missing file, parse error, etc.).
    """
    if not os.path.isfile(path):
        return None
    if os.path.getsize(path) > _MAX_XML_BYTES:
        return None
    if not _XML_SAFE:
        sys.stderr.write(
            "defusedxml is not installed; skipping JUnit XML parse. "
            "Run 'pip install defusedxml' to enable it.\n"
        )
        return None
    try:
        tree = ET.parse(path)
        root = tree.getroot()

        def _int(el, attr):
            try:
                return int(el.get(attr, 0))
            except (ValueError, TypeError):
                return 0

        def _float(el, attr):
            try:
                return float(el.get(attr, 0.0))
            except (ValueError, TypeError):
                return 0.0

        if root.tag == "testsuites":
            # Aggregate across all <testsuite> children
            tests = failures = errors = skipped = 0
            time_total = 0.0
            for suite in root.findall("testsuite"):
                tests    += _int(suite, "tests")
                failures += _int(suite, "failures")
                errors   += _int(suite, "errors")
                skipped  += _int(suite, "skipped")
                time_total += _float(suite, "time")
        elif root.tag == "testsuite":
            tests    = _int(root, "tests")
            failures = _int(root, "failures")
            errors   = _int(root, "errors")
            skipped  = _int(root, "skipped")
            time_total = _float(root, "time")
        else:
            return None

        passed = max(0, tests - failures - errors - skipped)
        return {
            "tests":    tests,
            "failures": failures,
            "errors":   errors,
            "skipped":  skipped,
            "passed":   passed,
            "time":     time_total,
        }
    except Exception:
        return None


def parse_lcov(path: str) -> tuple:
    """Parse an lcov.info file.

    Accumulates LF: (lines found) and LH: (lines hit) across all source files.

    Returns (hit, found).  Both values are 0 if the file is missing or unreadable.
    """
    if not os.path.isfile(path):
        return (0, 0)
    try:
        hit = 0
        found = 0
        with open(path, encoding="utf-8", errors="replace") as fh:
            for line in fh:
                line = line.strip()
                if line.startswith("LF:"):
                    try:
                        found += int(line[3:])
                    except ValueError:
                        continue
                elif line.startswith("LH:"):
                    try:
                        hit += int(line[3:])
                    except ValueError:
                        continue
        return (hit, found)
    except Exception:
        return (0, 0)


# In-page download helper. Fetches an artifact same-origin and saves it via a
# blob: URL. A blob: URL is a first-party, potentially-trustworthy origin, so a
# download triggered from it is NOT subject to Chrome/Brave "insecure download
# blocked" — the rule that fires when a .zip/.xlsx is fetched over plain HTTP
# (exactly what the htmlpublisher "Zip" button does). If this script can't run
# (strict CSP), the anchors' href attributes remain a working fallback.
# ASCII-only, no async/await — safe under the trend-reports JS rules.
_DOWNLOAD_JS = """(function () {
  function saveBlob(url, name) {
    fetch(url, { credentials: 'same-origin' })
      .then(function (r) { if (!r.ok) { throw new Error(String(r.status)); } return r.blob(); })
      .then(function (blob) {
        var u = URL.createObjectURL(blob);
        var a = document.createElement('a');
        a.href = u;
        a.download = name || url.split('/').pop() || 'download';
        document.body.appendChild(a);
        a.click();
        setTimeout(function () { URL.revokeObjectURL(u); a.parentNode && a.parentNode.removeChild(a); }, 1500);
      })
      .catch(function () { window.location.href = url; });
  }
  document.addEventListener('DOMContentLoaded', function () {
    var links = document.querySelectorAll('a[data-dl]');
    for (var i = 0; i < links.length; i++) {
      links[i].addEventListener('click', function (e) {
        e.preventDefault();
        saveBlob(this.getAttribute('href'), this.getAttribute('download') || '');
      });
    }
  });
})();

// Interactive chart hover: an instant styled tooltip (the number/name) plus a
// "pop" animation of the hovered bar / trend point. Works on any [data-tip]
// element and grows its partner (.bar-rect or .spark-pt with the same data-i in
// the same SVG). ASCII-only, no async - safe under the trend-reports JS rules.
(function () {
  var tip = null;
  function ensureTip() {
    if (!tip) {
      tip = document.createElement('div');
      tip.className = 'sloc-tip';
      tip.setAttribute('aria-hidden', 'true');
      document.body.appendChild(tip);
    }
    return tip;
  }
  function place(e) {
    if (!tip) { return; }
    var pad = 14;
    var x = e.clientX + pad;
    var y = e.clientY + pad;
    var w = tip.offsetWidth;
    var h = tip.offsetHeight;
    if (x + w > window.innerWidth - 8) { x = e.clientX - w - pad; }
    if (y + h > window.innerHeight - 8) { y = e.clientY - h - pad; }
    tip.style.left = x + 'px';
    tip.style.top = y + 'px';
  }
  function partner(el) {
    var svg = el.ownerSVGElement;
    if (!svg) { return null; }
    var i = el.getAttribute('data-i');
    if (i === null) { return null; }
    return svg.querySelector('.bar-rect[data-i="' + i + '"], .spark-pt[data-i="' + i + '"]');
  }
  function bind(el) {
    el.addEventListener('mouseenter', function (e) {
      var t = ensureTip();
      t.textContent = el.getAttribute('data-tip') || '';
      t.classList.add('show');
      var p = partner(el);
      if (p) { p.classList.add('pop'); }
      place(e);
    });
    el.addEventListener('mousemove', place);
    el.addEventListener('mouseleave', function () {
      if (tip) { tip.classList.remove('show'); }
      var p = partner(el);
      if (p) { p.classList.remove('pop'); }
    });
  }
  document.addEventListener('DOMContentLoaded', function () {
    var els = document.querySelectorAll('[data-tip]');
    for (var i = 0; i < els.length; i++) { bind(els[i]); }
  });
})();
"""


def _reset_dir(path: str) -> None:
    """Create an empty directory, removing any previous contents."""
    if os.path.isdir(path):
        shutil.rmtree(path, ignore_errors=True)
    os.makedirs(path, exist_ok=True)


def _copy_into(src_dir: str, names: list, dst_dir: str) -> None:
    """Copy each existing file in ``names`` from src_dir into dst_dir (flat)."""
    for name in names:
        src = os.path.join(src_dir, name)
        if os.path.isfile(src):
            shutil.copyfile(src, os.path.join(dst_dir, os.path.basename(name)))


def parse_trend_history(path: str) -> list:
    """Parse the persistent per-job trend CSV written by the Jenkinsfile.

    Each row (current schema):
      timestamp, build, code_lines, comment_lines, blank_lines, files_analyzed,
      repo_url, branch, commit

    The last three columns were appended to the schema later, so older CSVs that
    stop after ``files_analyzed`` still parse — the new fields default to "".
    Returns a list of dicts sorted oldest-first.  Returns [] on any error.
    """
    if not os.path.isfile(path):
        return []
    try:
        rows = []
        with open(path, newline="", encoding="utf-8") as fh:
            reader = csv.DictReader(fh)
            for row in reader:
                try:
                    rows.append({
                        "timestamp":      row.get("timestamp", ""),
                        "build":          int(row.get("build", 0)),
                        "code_lines":     int(row.get("code_lines", 0)),
                        "comment_lines":  int(row.get("comment_lines", 0)),
                        "blank_lines":    int(row.get("blank_lines", 0)),
                        "files_analyzed": int(row.get("files_analyzed", 0)),
                        # New columns — backward compatible (empty when absent).
                        "repo_url":       str(row.get("repo_url", "") or ""),
                        "branch":         str(row.get("branch", "") or ""),
                        "commit":         str(row.get("commit", "") or ""),
                    })
                except (ValueError, KeyError):
                    continue
        return rows
    except Exception:
        return []


# ---------------------------------------------------------------------------
# Main dashboard generator
# ---------------------------------------------------------------------------

def generate(out_dir: str, slug: Optional[str] = None) -> None:
    """Read CI artifacts from out_dir and write dashboard_<slug>.html."""
    out_dir = os.path.abspath(out_dir)

    # ── Auto-detect slug ────────────────────────────────────────────────────
    if slug is None:
        candidates = glob(os.path.join(out_dir, "result_*.json"))
        if not candidates:
            print(
                f"ERROR: No result_*.json found in {out_dir}. "
                "Pass a project-slug argument or run the SLOC analysis first.",
                file=sys.stderr,
            )
            sys.exit(1)
        candidates.sort()
        basename = os.path.basename(candidates[0])           # result_myproject.json
        slug = basename[len("result_"):-len(".json")]

    # ── Load SLOC JSON ──────────────────────────────────────────────────────
    json_path = os.path.join(out_dir, f"result_{slug}.json")
    if not os.path.isfile(json_path):
        print(
            f"ERROR: {json_path} not found. Run oxide-sloc analyze with --json-out first.",
            file=sys.stderr,
        )
        sys.exit(1)

    with open(json_path, encoding="utf-8") as fh:
        data = json.load(fh)

    # oxide-sloc result.json structure:
    #   data["tool"]              → {name, version, run_id, timestamp_utc}
    #   data["summary_totals"]    → aggregate counts
    #   data["totals_by_language"] → list of per-language dicts
    #       each entry: {"language": str|dict, "code_lines": int, ...}
    tool_meta = data.get("tool") if isinstance(data.get("tool"), dict) else {}
    # Version: prefer the value baked into the scan result so the dashboard shows
    # the exact oxide-sloc build that produced it; fall back to the env override.
    app_version = str(tool_meta.get("version") or os.environ.get("SLOC_VERSION", "")).strip()
    # Timestamp: use the scan's own instant, not "now", so re-generating the
    # dashboard from archived artifacts keeps the original time.
    scan_dt = _parse_iso_utc(tool_meta.get("timestamp_utc")) or datetime.now(timezone.utc)

    totals = data.get("summary_totals", {})
    code_lines     = int(totals.get("code_lines",    0))
    comment_lines  = int(totals.get("comment_lines", 0))
    blank_lines    = int(totals.get("blank_lines",   0))
    files_analyzed = int(totals.get("files_analyzed", 0))

    # Per-language breakdown
    lang_rows = []        # (name, code_lines) for bar chart
    lang_table_rows = []  # full row dicts for table
    for entry in data.get("totals_by_language", []):
        if not isinstance(entry, dict):
            continue
        name = entry.get("language", "Unknown")
        if isinstance(name, dict):
            name = name.get("display_name") or name.get("name") or str(name)
        code = int(entry.get("code_lines", 0))
        lang_rows.append((str(name), code))
        lang_table_rows.append({
            "name":           str(name),
            "code_lines":     code,
            "comment_lines":  int(entry.get("comment_lines", 0)),
            "blank_lines":    int(entry.get("blank_lines",   0)),
            "files_analyzed": int(entry.get("files_analyzed", 0)),
        })

    lang_rows.sort(key=lambda x: x[1], reverse=True)
    lang_table_rows.sort(key=lambda x: x["code_lines"], reverse=True)
    top_lang = lang_rows[0][0] if lang_rows else None

    # Top files by code lines
    top_files = []
    for rec in data.get("per_file_records", []):
        if not isinstance(rec, dict):
            continue
        lang = rec.get("language", "")
        if isinstance(lang, dict):
            lang = lang.get("display_name") or lang.get("name") or str(lang)
        top_files.append({
            # Portable relative path (falls back to basename of the absolute path;
            # a Windows verbatim //?/ prefix is stripped defensively for old JSON).
            "path":       _display_file_path(rec),
            "language":   str(lang),
            "code_lines": int(rec.get("code_lines", 0)),
        })
    top_files.sort(key=lambda x: x["code_lines"], reverse=True)
    top_files = top_files[:20]

    # ── Load test results ───────────────────────────────────────────────────
    junit_path = os.path.join(out_dir, "test-results", "junit.xml")
    junit = parse_junit(junit_path)

    # ── Load coverage ───────────────────────────────────────────────────────
    lcov_path = os.path.join(out_dir, "coverage", "lcov.info")
    cov_hit, cov_found = parse_lcov(lcov_path)
    cov_pct = round(cov_hit / cov_found * 100, 1) if cov_found > 0 else None

    # ── Load trend history ──────────────────────────────────────────────────
    # Path supplied as SLOC_HISTORY_FILE env var (set by Jenkinsfile) or 3rd arg.
    history_file  = os.environ.get("SLOC_HISTORY_FILE", "")
    trend_history = parse_trend_history(history_file) if history_file else []

    # ── Determine THIS run's repo identity ──────────────────────────────────
    # Used to (a) compute the "Code Lines Δ" chip only against the most recent
    # PREVIOUS row of the SAME repo (never across unrelated projects the same
    # Jenkins job scanned), and (b) drive the per-project "View B" trend below.
    # Source order: the SLOC_TREND_REPO_URL env the pipeline exports (the
    # effective scanned repo URL) → the scan result's own git_remote_url.
    cur_repo_url_raw = (os.environ.get("SLOC_TREND_REPO_URL", "").strip()
                        or str(data.get("git_remote_url", "") or "").strip())
    cur_repo_norm = _norm_repo_url(cur_repo_url_raw)
    cur_repo_name = _repo_name(cur_repo_url_raw) or slug
    cur_branch    = str(data.get("git_branch", "") or "").strip()

    # Rows in history that belong to THIS project (normalized-URL match). When the
    # current repo URL is unknown (empty), we cannot filter reliably — treat the
    # per-project view as "all rows" so the dashboard still renders a trend.
    if cur_repo_norm:
        same_repo_history = [
            r for r in trend_history if _norm_repo_url(r.get("repo_url", "")) == cur_repo_norm
        ]
    else:
        same_repo_history = list(trend_history)

    # Distinct normalized repos seen in this Jenkins job's history (for the
    # "mixed projects" warning on the all-builds view).
    distinct_repos = sorted({
        _norm_repo_url(r.get("repo_url", "")) for r in trend_history
        if _norm_repo_url(r.get("repo_url", ""))
    })
    n_distinct_repos = len(distinct_repos)

    # ── Load CI capabilities (plugin-install permission / degradation state) ──
    capabilities = None
    caps_path = os.path.join(out_dir, "capabilities.json")
    if os.path.isfile(caps_path):
        try:
            with open(caps_path, encoding="utf-8") as fh:
                capabilities = json.load(fh)
        except (OSError, ValueError):
            capabilities = None

    # ── Environment ─────────────────────────────────────────────────────────
    build_number = os.environ.get("BUILD_NUMBER", "")
    build_url    = os.environ.get("BUILD_URL", "")
    job_name     = os.environ.get("JOB_NAME", "")
    scan_path    = os.environ.get("SCAN_PATH", "")
    # Render in the app's timezone (Pacific by default) to match the web report.
    timestamp    = fmt_local(scan_dt)

    # ── Build chip sections ─────────────────────────────────────────────────

    # Delta from the most recent PREVIOUS build OF THE SAME REPO. Comparing the
    # last two rows of the raw job history would mix unrelated projects (this one
    # Jenkins job can scan many repos), so filter to same_repo_history first. The
    # last row of same_repo_history is THIS build; index [-2] is its same-repo
    # predecessor. If there is no prior same-repo row, show "n/a (first scan)".
    delta_chip_html = ""
    if len(same_repo_history) >= 2:
        t_prev_build = same_repo_history[-2]
        delta = code_lines - t_prev_build["code_lines"]
        sign = "+" if delta > 0 else ""
        delta_col = _delta_color(delta)
        delta_chip_html = chip(
            f'<span style="color:{delta_col}" title="vs build #{t_prev_build["build"]}">'
            f"{sign}{fmt(delta)}</span>",
            "Code Lines Δ",
            f"vs build #{t_prev_build['build']}",
            tip=(
                f"Net change in code lines since build #{t_prev_build['build']} "
                f"(the previous scan of {html.escape(cur_repo_name)}) — positive means "
                "the codebase grew, negative means it shrank. Compared same-repo only, "
                "so unrelated projects this job scanned never skew the delta."
            ),
        )
    else:
        delta_chip_html = chip(
            '<span style="color:#8a6a5a" title="first scan of this repository">n/a</span>',
            "Code Lines Δ",
            "first scan of this repo",
            tip=(
                "No previous scan of this repository is recorded in this Jenkins "
                "job's history yet, so there is nothing to compare against. The "
                "delta appears from the second scan of the same repo onward."
            ),
        )

    # SLOC summary chips
    sloc_chips = "".join([
        chip(
            f'<span title="{fmt_full(code_lines)}">{fmt(code_lines)}</span>',
            "Code Lines",
            f"exact: {fmt_full(code_lines)}",
            tip=(
                "Source lines of code: physical lines containing executable code, "
                "excluding blank and comment-only lines."
            ),
        ),
        chip(
            f'<span title="{fmt_full(comment_lines)}">{fmt(comment_lines)}</span>',
            "Comment Lines",
            f"exact: {fmt_full(comment_lines)}",
            tip=(
                "Lines that are entirely comments or documentation (line comments, "
                "block comments, and docstrings)."
            ),
        ),
        chip(
            f'<span title="{fmt_full(blank_lines)}">{fmt(blank_lines)}</span>',
            "Blank Lines",
            f"exact: {fmt_full(blank_lines)}",
            tip="Empty or whitespace-only lines used for visual spacing.",
        ),
        chip(
            f'<span title="{fmt_full(files_analyzed)}">{fmt(files_analyzed)}</span>',
            "Files Analyzed",
            f"top: {top_lang}" if top_lang else None,
            tip=(
                "Number of source files that were detected, decoded, and counted "
                + (f"in this scan. Most code is in {top_lang}." if top_lang
                   else "in this scan.")
            ),
        ),
        delta_chip_html,
    ])
    sloc_strip_class = "summary-strip summary-strip-5" if delta_chip_html else "summary-strip"

    # Language chart
    lang_chart = svg_hbar(lang_rows[:20])
    lang_caption = (
        f"{len(lang_rows)} language{'s' if len(lang_rows) != 1 else ''} detected"
        " · hover a bar for its exact count and share of the total"
    )

    # Per-language metrics table
    if lang_table_rows:
        _tbl_rows = "\n".join(
            f"<tr>"
            f"<td>{html.escape(r['name'])}</td>"
            f"<td class='num'>{fmt(r['code_lines'])}</td>"
            f"<td class='num'>{fmt(r['comment_lines'])}</td>"
            f"<td class='num'>{fmt(r['blank_lines'])}</td>"
            f"<td class='num'>{fmt(r['files_analyzed'])}</td>"
            f"</tr>"
            for r in lang_table_rows
        )
        lang_table_html = f"""
<table class="data-table" style="margin-top:18px">
  <thead>
    <tr>
      <th>Language</th>
      <th class="num">Code</th>
      <th class="num">Comments</th>
      <th class="num">Blank</th>
      <th class="num">Files</th>
    </tr>
  </thead>
  <tbody>
    {_tbl_rows}
  </tbody>
</table>"""
    else:
        lang_table_html = ""

    # Top files section
    if top_files:
        _file_rows = "\n".join(
            f"<tr>"
            f'<td class="trunc" title="{html.escape(f["path"])}">{html.escape(f["path"])}</td>'
            f"<td>{html.escape(f['language'])}</td>"
            f"<td class='num'>{fmt(f['code_lines'])}</td>"
            f"</tr>"
            for f in top_files
        )
        files_section = f"""
<div class="card">
  <div class="card-title">Top Files by Code Lines</div>
  <table class="data-table">
    <thead>
      <tr>
        <th>Path</th>
        <th>Language</th>
        <th class="num">Code Lines</th>
      </tr>
    </thead>
    <tbody>
      {_file_rows}
    </tbody>
  </table>
</div>"""
    else:
        files_section = ""

    # Test results section
    if junit is not None:
        pass_colour = "#2a6846" if junit["failures"] == 0 and junit["errors"] == 0 else "#b23030"
        status_text = (
            "All tests passed."
            if junit["failures"] == 0 and junit["errors"] == 0
            else f"{junit['failures']} failed, {junit['errors']} error(s)."
        )
        test_chips = "".join([
            chip(fmt(junit["tests"]), "Total Tests",
                 tip="Total number of test cases executed by cargo-nextest."),
            chip(
                f'<span style="color:{pass_colour}">{fmt(junit["passed"])}</span>',
                "Passed",
                tip="Tests that completed successfully.",
            ),
            chip(
                f'<span style="color:#b23030">{fmt(junit["failures"])}</span>',
                "Failed",
                tip="Tests that ran but did not meet their assertions.",
            ),
            chip(
                f'<span style="color:#b23030">{fmt(junit["errors"])}</span>',
                "Errors",
                tip="Tests that could not complete due to a runtime error.",
            ),
            chip(fmt(junit["skipped"]), "Skipped",
                 tip="Tests that were ignored or filtered out of this run."),
        ])
        time_str = f"{junit['time']:.2f}s" if junit["time"] > 0 else ""
        test_section = f"""
<div class="card">
  <div class="card-title">Test Results</div>
  <div class="summary-strip summary-strip-5">{test_chips}</div>
  <p class="status-msg" style="color:{pass_colour}">
    {html.escape(status_text)}
    {(' <span class="muted">(' + html.escape(time_str) + ')</span>') if time_str else ''}
  </p>
</div>"""
    else:
        test_section = """
<div class="card">
  <div class="card-title">Test Results</div>
  <p class="muted-msg">JUnit XML not found at <code>test-results/junit.xml</code>.<br>
  Set <code>TEST_RUNNER = cargo-nextest</code> and <code>PUBLISH_TEST_RESULTS = true</code>
  in the pipeline parameters to enable test reporting.</p>
</div>"""

    # Coverage section
    if cov_pct is not None:
        cov_bar = svg_progress(cov_pct)
        coverage_section = f"""
<div class="card">
  <div class="card-title">Code Coverage</div>
  <div class="cov-bar">{cov_bar}</div>
  <p class="cov-detail">
    {html.escape(f"{cov_pct:.1f}%")} line coverage
    &nbsp;&middot;&nbsp;
    {html.escape(fmt_full(cov_hit))} / {html.escape(fmt_full(cov_found))} lines hit
  </p>
</div>"""
    else:
        coverage_section = """
<div class="card">
  <div class="card-title">Code Coverage</div>
  <p class="muted-msg">Coverage data not found at <code>coverage/lcov.info</code>.<br>
  Enable <code>COVERAGE_STANDALONE = true</code> in the pipeline parameters to
  generate LCOV coverage data.</p>
</div>"""

    # ── CI capabilities banner ──────────────────────────────────────────────
    # When richer visualization plugins could not be installed (no system-admin
    # rights, or an air-gapped controller with no offline bundle), show a calm
    # warning banner explaining it — never an error. When plugins are active, or
    # no capability probe ran, show nothing (or a subtle confirmation).
    capability_banner = ""
    if isinstance(capabilities, dict) and capabilities.get("degraded"):
        reason = str(capabilities.get("degraded_reason", "")).strip() or (
            "Enhanced Jenkins visualization plugins could not be enabled for this "
            "build; the native oxide-sloc dashboard is shown instead."
        )
        requested = [str(p) for p in capabilities.get("requested_plugins", []) if p]
        plugin_list = ""
        if requested:
            chips = "".join(
                f'<span class="plugin-chip">{html.escape(p)}</span>' for p in requested
            )
            plugin_list = (
                '<div class="banner-plugins"><span class="banner-plugins-label">'
                "Plugins that would add richer views:</span>"
                f'<div class="plugin-chip-row">{chips}</div></div>'
            )
        capability_banner = f"""
<div class="capability-banner" role="status">
  <div class="capability-banner-icon" aria-hidden="true">&#9888;</div>
  <div class="capability-banner-body">
    <div class="capability-banner-title">Running in native mode &mdash; enhanced plugins not enabled</div>
    <p class="capability-banner-text">{html.escape(reason)}</p>
    {plugin_list}
  </div>
</div>"""

    # ── Report & Exports section ────────────────────────────────────────────
    # Link to the sibling artifacts so the dashboard is the single entry point:
    # open the full interactive report or download any export from one place.
    # Every href is a plain filename resolved relative to this HTML, so the links
    # work identically in the Jenkins-published view and in the downloaded bundle.
    export_defs = [
        (f"report_{slug}.html", "Full HTML report", "Interactive per-file report", "open"),
        (f"report_{slug}.pdf", "PDF", "Print-ready PDF export", "download"),
        (f"report_{slug}.xlsx", "Excel workbook", "Per-language + per-file sheets", "download"),
        (f"report_{slug}.csv", "CSV", "Per-file metrics as CSV", "download"),
        (f"result_{slug}.json", "JSON", "Raw machine-readable result", "download"),
    ]
    export_links = []
    for fname, label, desc, action in export_defs:
        if not os.path.isfile(os.path.join(out_dir, fname)):
            continue
        # Download links carry data-dl + a download attr so the in-page blob
        # helper (dashboard_<slug>.js) saves them via a blob: URL — that counts
        # as a first-party/secure origin, so Chrome/Brave do NOT apply the
        # "insecure download blocked" rule that fires on plain-HTTP .zip/.xlsx
        # downloads (the htmlpublisher "Zip" button hits exactly that block).
        # If the script is unavailable (strict CSP), the href is the fallback.
        if action == "open":
            attrs = ' target="_blank" rel="noopener"'
            verb = "Open"
        else:
            attrs = f' download="{html.escape(fname)}" data-dl="1"'
            verb = "Download"
        export_links.append(
            f'<a class="export-link" href="{html.escape(fname)}"{attrs}'
            f' title="{verb} {html.escape(desc.lower())}">'
            f'<span class="export-link-label">{html.escape(label)}</span>'
            f'<span class="export-link-desc">{html.escape(desc)}</span>'
            f'<span class="export-link-verb">{verb} &#8594;</span>'
            f"</a>"
        )
    if export_links:
        downloads_section = f"""
<div class="card">
  <div class="card-title">Report &amp; Exports</div>
  <p class="export-hint">Use these links instead of the top-right <strong>Zip</strong>
     button &mdash; they save directly and avoid the browser's
     &ldquo;insecure download blocked&rdquo; warning on plain-HTTP servers.</p>
  <div class="export-grid">
    {''.join(export_links)}
  </div>
</div>"""
    else:
        downloads_section = ""

    # ── Trend sparkline section (TWO views) ─────────────────────────────────
    # A single per-Jenkins-job history CSV can interleave rows from DIFFERENT
    # repositories (this one job scans many projects), so a naive all-rows
    # sparkline can jump wildly between unrelated codebases. We render two views
    # in one card with a dependency-free inline toggle:
    #
    #   View B "This project only (<repo>)" — DEFAULT. Rows whose normalized
    #          repo_url matches this run's repo. Apples-to-apples.
    #   View A "All builds of this Jenkins job" — every row, with a prominent
    #          warning that it may mix N distinct repositories.
    #
    # The toggle is two radio-driven <label>s + CSS sibling selectors — no JS
    # required, matching the existing dashboard's dependency-free style.
    def _trend_delta_line(hist):
        pts = [(r["build"], r["code_lines"]) for r in hist]
        last_h = hist[-1]
        prev_h = hist[-2]
        d = last_h["code_lines"] - prev_h["code_lines"]
        sign = "+" if d > 0 else ""
        col = _delta_color(d)
        return (
            f'<p class="trend-delta" style="color:{col}">'
            f'{html.escape(f"{sign}{fmt(d)}")} code lines since build #{prev_h["build"]}'
            f'&nbsp;&middot;&nbsp;'
            f'<span style="color:#8a6a5a;font-weight:400">range: '
            f'{html.escape(fmt(min(v for _, v in pts)))} &ndash; '
            f'{html.escape(fmt(max(v for _, v in pts)))}</span></p>'
        )

    # View B (per-project) — the trustworthy trend. Available when this repo has
    # >= 2 rows of its own history.
    view_b_html = ""
    if len(same_repo_history) >= 2:
        b_points   = [(r["build"], r["code_lines"]) for r in same_repo_history]
        b_spark    = svg_sparkline(b_points)
        b_caption  = (
            f"Filtered to <strong>{html.escape(cur_repo_name)}</strong>"
            + (f" &middot; branch <code>{html.escape(cur_branch)}</code>" if cur_branch else "")
            + f" &middot; {len(same_repo_history)} scan"
            + ("s" if len(same_repo_history) != 1 else "")
            + " of this project. Apples-to-apples: only this repository's builds."
        )
        view_b_html = f"""
    <div class="sparkline-wrap">{b_spark}</div>
    <p class="chart-hint">{b_caption}</p>
    {_trend_delta_line(same_repo_history)}"""
    elif len(same_repo_history) == 1 or cur_repo_norm:
        view_b_html = f"""
    <p class="muted-msg">Only one recorded scan of
    <strong>{html.escape(cur_repo_name)}</strong> so far &mdash; a per-project trend
    line appears once this repository has been scanned at least twice by this job.</p>"""

    # View A (all builds of this Jenkins job) — may mix projects.
    view_a_html = ""
    if len(trend_history) >= 2:
        a_points = [(r["build"], r["code_lines"]) for r in trend_history]
        a_spark  = svg_sparkline(a_points)
        mixed_note = ""
        if n_distinct_repos > 1:
            mixed_note = (
                f'<p class="trend-mixed-warn">&#9888; Mixed projects &mdash; this '
                f"Jenkins job has scanned <strong>{n_distinct_repos}</strong> distinct "
                "repositories. Points below can jump between unrelated codebases; use "
                "the <em>This project only</em> view above for a like-for-like trend.</p>"
            )
        view_a_html = f"""
    {mixed_note}
    <div class="sparkline-wrap">{a_spark}</div>
    <p class="chart-hint">Every build of this Jenkins job, regardless of which repository was scanned.</p>
    {_trend_delta_line(trend_history)}"""

    # Assemble the card only if there is at least one view to show.
    if view_a_html or view_b_html:
        # If only one repo exists in history, A == B; say so instead of a toggle.
        single_repo = (n_distinct_repos <= 1)
        if single_repo:
            body = view_b_html or view_a_html
            note = ""
            if view_b_html and view_a_html:
                note = ('<p class="chart-hint">This Jenkins job has only ever scanned this '
                        "one repository, so the all-builds and per-project trends are identical.</p>")
            trend_section = f"""
<div class="card">
  <div class="card-title">Code Lines Trend</div>
  {note}
  {body}
</div>"""
        else:
            trend_section = f"""
<div class="card">
  <div class="card-title">Code Lines Trend</div>
  <div class="trend-toggle">
    <input type="radio" name="trend-view" id="trend-view-b" class="trend-radio" checked>
    <input type="radio" name="trend-view" id="trend-view-a" class="trend-radio">
    <div class="trend-tabs">
      <label for="trend-view-b" class="trend-tab trend-tab-b">This project only ({html.escape(cur_repo_name)})</label>
      <label for="trend-view-a" class="trend-tab trend-tab-a">All builds of this Jenkins job</label>
    </div>
    <div class="trend-panel trend-panel-b">
      {view_b_html or '<p class="muted-msg">No per-project history yet.</p>'}
    </div>
    <div class="trend-panel trend-panel-a">
      {view_a_html or '<p class="muted-msg">No job-wide history yet.</p>'}
    </div>
  </div>
</div>"""
    else:
        trend_section = ""

    # ── Header meta ─────────────────────────────────────────────────────────
    header_meta_parts = []
    if app_version:
        header_meta_parts.append(
            f'<span title="oxide-sloc application version">oxide-sloc '
            f"v{html.escape(app_version)}</span>"
        )
    if job_name:
        header_meta_parts.append(f"<span>Job: {html.escape(job_name)}</span>")
    if build_number:
        header_meta_parts.append(f"<span>Build #{html.escape(build_number)}</span>")
    if scan_path:
        header_meta_parts.append(f"<span>Scan path: <code>{html.escape(scan_path)}</code></span>")
    header_meta_parts.append(
        f'<span title="Scan time, shown in US Pacific time">{html.escape(timestamp)}</span>'
    )

    back_link = ""
    if build_url:
        back_link = (
            f'<a class="back-link" href="{html.escape(build_url)}">'
            f"&#8592; Back to Jenkins build</a>"
        )

    header_meta_html = "".join(header_meta_parts)

    # ── Assemble the full HTML page ─────────────────────────────────────────
    page_title = f"oxide-sloc Graphical Report — {slug}"

    html_out = f"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{html.escape(page_title)}</title>
<style>
/* ── Reset & base ──────────────────────────────────────────────────────── */
*, *::before, *::after {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{
  font-family: system-ui, -apple-system, "Segoe UI", sans-serif;
  background: #f9f5f0;
  color: #2d1a0e;
  min-height: 100vh;
}}

/* ── Header ────────────────────────────────────────────────────────────── */
.site-header {{
  background: linear-gradient(180deg, #3a2010 0%, #2d1a0e 100%);
  border-bottom: 2px solid rgba(224,112,56,0.55);
  box-shadow: 0 4px 22px rgba(0,0,0,0.38);
  color: #f9f5f0;
  padding: 0 32px;
}}
.site-header-row {{
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 14px 0 10px;
  flex-wrap: nowrap;
}}
.site-header .brand {{
  font-size: 21px;
  font-weight: 900;
  color: #e07038;
  letter-spacing: -0.02em;
  white-space: nowrap;
  flex-shrink: 0;
  text-shadow: 0 0 20px rgba(224,112,56,0.4);
}}
.header-div {{
  width: 1px;
  height: 22px;
  background: rgba(255,255,255,0.22);
  flex-shrink: 0;
}}
.site-header .dash-title {{
  font-size: 15px;
  font-weight: 600;
  color: rgba(249,245,240,0.88);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
}}
.header-fill {{ flex: 1; }}
.site-header .meta {{
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
  padding-bottom: 12px;
}}
.site-header .meta > span {{
  background: rgba(255,255,255,0.08);
  border: 1px solid rgba(255,255,255,0.14);
  border-radius: 20px;
  padding: 3px 11px;
  font-size: 11px;
  color: #c8a882;
  white-space: nowrap;
}}
.site-header .meta code {{
  background: rgba(255,255,255,0.12);
  border-radius: 3px;
  padding: 1px 5px;
  font-size: 10px;
  color: #d4a060;
  font-family: ui-monospace, "Cascadia Code", monospace;
}}
.back-link {{
  display: inline-flex;
  align-items: center;
  gap: 6px;
  background: rgba(224,112,56,0.18);
  border: 1px solid rgba(224,112,56,0.5);
  border-radius: 8px;
  padding: 7px 16px;
  color: #f09060;
  text-decoration: none;
  font-size: 13px;
  font-weight: 700;
  white-space: nowrap;
  flex-shrink: 0;
  transition: background 0.18s ease, box-shadow 0.18s ease;
}}
.back-link:hover {{
  background: rgba(224,112,56,0.32);
  box-shadow: 0 0 14px rgba(224,112,56,0.28);
}}

/* ── Main layout ───────────────────────────────────────────────────────── */
.main {{
  max-width: 860px;
  margin: 32px auto;
  padding: 0 20px;
  display: flex;
  flex-direction: column;
  gap: 24px;
}}

/* ── Cards ─────────────────────────────────────────────────────────────── */
.card {{
  background: #fff;
  border: 1px solid #e0d8d0;
  border-radius: 12px;
  padding: 22px 24px;
}}
.card-title {{
  font-size: 13px;
  font-weight: 800;
  text-transform: uppercase;
  letter-spacing: 0.08em;
  color: #8a6a5a;
  margin-bottom: 16px;
}}

/* ── Stat chips ─────────────────────────────────────────────────────────── */
.summary-strip {{
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: 12px;
  margin-bottom: 8px;
}}
.summary-strip-5 {{
  grid-template-columns: repeat(5, 1fr);
}}
.stat-chip {{
  position: relative;
  background: #f9f5f0;
  border: 1px solid #e0d8d0;
  border-radius: 12px;
  padding: 14px 16px;
  cursor: default;
  transition: transform 0.2s ease, box-shadow 0.2s ease;
}}
.stat-chip:hover {{
  transform: translateY(-3px);
  box-shadow: 0 8px 24px rgba(77,44,20,0.15);
}}
.stat-chip-val {{
  font-size: 20px;
  font-weight: 900;
  color: #b04a00;
}}
.stat-chip-label {{
  font-size: 11px;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.07em;
  color: #8a6a5a;
  margin-top: 4px;
}}
.chip-sub {{
  font-size: 11px;
  color: #8a6a5a;
  margin-top: 3px;
}}
/* Hover description tooltip for stat chips */
.stat-chip-tip {{
  position: absolute;
  bottom: calc(100% + 10px);
  left: 50%;
  transform: translateX(-50%) translateY(4px);
  width: max-content;
  max-width: 240px;
  background: #2d1a0e;
  color: #f9f5f0;
  padding: 9px 12px;
  border-radius: 8px;
  font-size: 11px;
  font-weight: 500;
  line-height: 1.45;
  text-transform: none;
  letter-spacing: normal;
  box-shadow: 0 10px 28px rgba(0,0,0,0.28);
  pointer-events: none;
  opacity: 0;
  transition: opacity 0.18s ease, transform 0.18s ease;
  z-index: 30;
}}
.stat-chip-tip::after {{
  content: "";
  position: absolute;
  top: 100%;
  left: 50%;
  transform: translateX(-50%);
  border: 6px solid transparent;
  border-top-color: #2d1a0e;
}}
.stat-chip:hover .stat-chip-tip {{
  opacity: 1;
  transform: translateX(-50%) translateY(0);
}}

/* ── Chart hover affordances ─────────────────────────────────────────────── */
.chart-hint {{
  font-size: 11px;
  color: #a08876;
  margin-top: 6px;
  font-style: italic;
}}
.bar-rect {{
  cursor: pointer;
  transform-box: fill-box;
  transform-origin: left center;
  transition: transform 0.16s ease, filter 0.16s ease;
}}
/* Bar "pops out" when its row is hovered (driven by JS on the hit target). */
.bar-rect.pop {{
  transform: scaleY(1.18);
  filter: brightness(1.12) drop-shadow(0 3px 6px rgba(0,0,0,0.28));
}}
.bar-hit {{ cursor: pointer; }}
.spark-dot {{ cursor: pointer; }}
/* Trend point grows/highlights when hovered (driven by JS on the hit dot). */
.spark-pt {{
  transform-box: fill-box;
  transform-origin: center;
  transition: transform 0.16s ease;
}}
.spark-pt.pop {{ transform: scale(2.6); }}

/* Instant styled hover tooltip shared by the trend + language charts. */
.sloc-tip {{
  position: fixed;
  z-index: 9999;
  pointer-events: none;
  background: #2d1a0e;
  color: #f9f5f0;
  padding: 7px 11px;
  border-radius: 8px;
  font-size: 12px;
  font-weight: 600;
  font-family: Inter, system-ui, -apple-system, "Segoe UI", sans-serif;
  box-shadow: 0 8px 24px rgba(0,0,0,0.32);
  white-space: nowrap;
  max-width: 340px;
  opacity: 0;
  transform: translateY(4px);
  transition: opacity 0.12s ease, transform 0.12s ease;
}}
.sloc-tip.show {{ opacity: 1; transform: translateY(0); }}

/* ── Report & Exports ────────────────────────────────────────────────────── */
.export-hint {{
  font-size: 12px;
  color: #8a6a5a;
  margin: -6px 0 14px;
  line-height: 1.5;
}}
.export-hint strong {{ color: #b04a00; }}
.export-grid {{
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(190px, 1fr));
  gap: 12px;
}}
.export-link {{
  display: flex;
  flex-direction: column;
  gap: 3px;
  background: #f9f5f0;
  border: 1px solid #e0d8d0;
  border-radius: 10px;
  padding: 12px 14px;
  text-decoration: none;
  transition: transform 0.15s ease, box-shadow 0.15s ease, border-color 0.15s ease;
}}
.export-link:hover {{
  transform: translateY(-3px);
  border-color: #d4691c;
  box-shadow: 0 8px 22px rgba(176,74,0,0.16);
}}
.export-link-label {{ font-size: 14px; font-weight: 800; color: #b04a00; }}
.export-link-desc {{ font-size: 11px; color: #8a6a5a; }}
.export-link-verb {{
  font-size: 11px;
  font-weight: 700;
  color: #d4691c;
  margin-top: 2px;
}}

/* ── CI capabilities / degradation banner ────────────────────────────────── */
.capability-banner {{
  display: flex;
  gap: 14px;
  align-items: flex-start;
  background: #fff8ec;
  border: 1px solid #e8c37a;
  border-left: 5px solid #d4a017;
  border-radius: 12px;
  padding: 16px 20px;
}}
.capability-banner-icon {{
  font-size: 22px;
  line-height: 1;
  color: #b8860b;
  flex-shrink: 0;
  margin-top: 1px;
}}
.capability-banner-title {{
  font-size: 14px;
  font-weight: 800;
  color: #8a5a00;
  margin-bottom: 4px;
}}
.capability-banner-text {{
  font-size: 13px;
  line-height: 1.55;
  color: #6a5030;
}}
.banner-plugins {{ margin-top: 10px; }}
.banner-plugins-label {{
  font-size: 11px;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.06em;
  color: #a07c3a;
}}
.plugin-chip-row {{
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 6px;
}}
.plugin-chip {{
  background: #f3e3c2;
  border: 1px solid #e0c78a;
  border-radius: 20px;
  padding: 2px 10px;
  font-size: 11px;
  font-weight: 600;
  color: #7a5a1a;
  font-family: ui-monospace, "Cascadia Code", monospace;
}}

/* ── Language chart ─────────────────────────────────────────────────────── */
.lang-chart-wrap {{
  overflow-x: auto;
}}
.lang-caption {{
  font-size: 12px;
  color: #8a6a5a;
  margin-top: 8px;
}}

/* ── Coverage ───────────────────────────────────────────────────────────── */
.cov-bar {{ margin-bottom: 8px; }}
.cov-detail {{
  font-size: 13px;
  color: #5a3820;
  font-weight: 600;
}}

/* ── Status messages ────────────────────────────────────────────────────── */
.status-msg {{
  font-size: 14px;
  font-weight: 700;
  margin-top: 8px;
}}
.muted {{ color: #8a6a5a; font-weight: 400; }}
.muted-msg {{
  color: #8a6a5a;
  font-size: 13px;
  line-height: 1.6;
}}
.muted-msg code {{
  background: #f0e8e0;
  border-radius: 3px;
  padding: 1px 5px;
  font-size: 12px;
}}

/* ── Footer ─────────────────────────────────────────────────────────────── */
.site-footer {{
  text-align: center;
  font-size: 12px;
  color: #8a6a5a;
  padding: 24px 20px 32px;
  border-top: 1px solid #e0d8d0;
  margin-top: 8px;
}}
.site-footer a {{
  color: #b04a00;
  text-decoration: none;
}}
.site-footer a:hover {{ text-decoration: underline; }}

/* ── Trend sparkline ────────────────────────────────────────────────────── */
.sparkline-wrap {{ overflow-x: auto; margin-bottom: 8px; }}
.trend-delta {{ font-size: 13px; font-weight: 700; margin-top: 4px; line-height: 1.5; }}
.trend-mixed-warn {{
  font-size: 12px;
  font-weight: 600;
  line-height: 1.5;
  color: #8a5a00;
  background: #fff8ec;
  border: 1px solid #e8c37a;
  border-left: 4px solid #d4a017;
  border-radius: 8px;
  padding: 9px 12px;
  margin-bottom: 10px;
}}
.trend-mixed-warn strong {{ color: #b04a00; }}
/* Dependency-free tab toggle for the dual trend view (radio + sibling CSS). */
.trend-radio {{ position: absolute; opacity: 0; pointer-events: none; }}
.trend-tabs {{
  display: flex;
  gap: 6px;
  margin-bottom: 14px;
  flex-wrap: wrap;
}}
.trend-tab {{
  cursor: pointer;
  font-size: 12px;
  font-weight: 700;
  padding: 6px 14px;
  border-radius: 20px;
  border: 1px solid #e0d8d0;
  background: #f9f5f0;
  color: #8a6a5a;
  user-select: none;
  transition: background 0.15s ease, color 0.15s ease, border-color 0.15s ease;
}}
.trend-tab:hover {{ border-color: #d4691c; color: #b04a00; }}
.trend-panel {{ display: none; }}
/* View B is the default (checked) tab; View A shows when its radio is checked. */
#trend-view-b:checked ~ .trend-tabs .trend-tab-b,
#trend-view-a:checked ~ .trend-tabs .trend-tab-a {{
  background: #b04a00;
  border-color: #b04a00;
  color: #fff;
}}
#trend-view-b:checked ~ .trend-panel-b {{ display: block; }}
#trend-view-a:checked ~ .trend-panel-a {{ display: block; }}

/* ── Data tables ─────────────────────────────────────────────────────────── */
.data-table {{
  width: 100%;
  border-collapse: collapse;
  font-size: 13px;
}}
.data-table th {{
  text-align: left;
  font-size: 11px;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.07em;
  color: #8a6a5a;
  padding: 6px 10px 8px;
  border-bottom: 2px solid #e0d8d0;
}}
.data-table td {{
  padding: 7px 10px;
  border-bottom: 1px solid #f0e8e0;
  color: #2d1a0e;
}}
.data-table tr:last-child td {{ border-bottom: none; }}
.data-table .num {{
  text-align: right;
  font-variant-numeric: tabular-nums;
  font-weight: 600;
}}
.data-table .trunc {{
  max-width: 320px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}}

/* ── Responsive ─────────────────────────────────────────────────────────── */
@media (max-width: 640px) {{
  .summary-strip {{ grid-template-columns: repeat(2, 1fr); }}
  .summary-strip-5 {{ grid-template-columns: repeat(2, 1fr); }}
  .site-header {{ padding: 0 16px; }}
  .site-header-row {{ padding: 12px 0 8px; gap: 10px; }}
  .site-header .meta {{ padding-bottom: 10px; }}
  .main {{ margin: 16px auto; }}
}}
</style>
</head>
<body>

<!-- ── Header ──────────────────────────────────────────────────────────── -->
<header class="site-header">
  <div class="site-header-row">
    <span class="brand">oxide-sloc</span>
    <span class="header-div"></span>
    <span class="dash-title">Graphical Report &mdash; {html.escape(slug)}</span>
    <div class="header-fill"></div>
    {back_link}
  </div>
  <div class="meta">
    {header_meta_html}
  </div>
</header>

<!-- ── Main content ────────────────────────────────────────────────────── -->
<main class="main">

  <!-- CI capabilities notice (only when plugins were unavailable) -->
  {capability_banner}

  <!-- SLOC Summary (code lines + delta when trend history available) -->
  <div class="card">
    <div class="card-title">SLOC Summary</div>
    <div class="{sloc_strip_class}">
      {sloc_chips}
    </div>
  </div>

  <!-- Build Trend (shown before language breakdown so the delta is prominent) -->
  {trend_section}

  <!-- Language Breakdown -->
  <div class="card">
    <div class="card-title">Language Breakdown</div>
    <div class="lang-chart-wrap">
      {lang_chart}
    </div>
    <p class="lang-caption">{html.escape(lang_caption)}</p>
    {lang_table_html}
  </div>

  <!-- Top Files -->
  {files_section}

  <!-- Test Results -->
  {test_section}

  <!-- Code Coverage -->
  {coverage_section}

  <!-- Report & Exports (single entry point to open / download everything) -->
  {downloads_section}

</main>

<!-- ── Footer ──────────────────────────────────────────────────────────── -->
<footer class="site-footer">
  oxide-sloc{(' v' + html.escape(app_version)) if app_version else ''} Graphical Report
  &nbsp;&middot;&nbsp;
  generated {html.escape(timestamp)} &nbsp;&middot;&nbsp;
  <a href="https://github.com/oxide-sloc/oxide-sloc" target="_blank" rel="noopener">
    oxide-sloc on GitHub
  </a>
</footer>

</body>
</html>
"""

    # ── Extract inline CSS to external file (Jenkins CSP fix) ──────────────
    # Jenkins's default artifact-viewer CSP blocks <style> tags but permits
    # external stylesheets from 'self'.  Writing the CSS to a separate file
    # and referencing it via <link> means the report renders correctly without
    # requiring credentials or a custom init.groovy.d CSP override.
    css_match = re.search(r"<style>([\s\S]*?)</style>", html_out)
    if css_match:
        css_filename = f"dashboard_{slug}.css"
        css_path = os.path.join(out_dir, css_filename)
        with open(css_path, "w", encoding="utf-8") as fh:
            fh.write(css_match.group(1))
        html_out = html_out.replace(
            css_match.group(0),
            f'<link rel="stylesheet" href="{css_filename}">',
            1,
        )

    # ── Write the in-page download helper as an external script (CSP-safe) ──
    # An external script from 'self' is permitted by Jenkins' relaxed CSP,
    # whereas inline scripts are not. Referenced right before </body>.
    js_filename = f"dashboard_{slug}.js"
    with open(os.path.join(out_dir, js_filename), "w", encoding="utf-8") as fh:
        fh.write(_DOWNLOAD_JS)
    html_out = html_out.replace(
        "</body>", f'<script src="{js_filename}"></script>\n</body>', 1
    )

    # ── Write output file ───────────────────────────────────────────────────
    out_path = os.path.join(out_dir, f"dashboard_{slug}.html")
    with open(out_path, "w", encoding="utf-8") as fh:
        fh.write(html_out)

    # ── Curated single-entry-point bundles ─────────────────────────────────
    # Jenkins' htmlpublisher zips the entire reportDir, so publishing from the
    # full output dir yields a cluttered zip with no obvious file to open. We
    # instead lay down two self-contained bundles whose root file is always
    # index.html, and point publishHTML at them:
    #   ci-report/   → this dashboard as index.html (summary + exports)
    #   html-report/ → the full interactive oxide-sloc report as index.html
    # Each carries the export artifacts so the downloaded zip opens and exports
    # offline. Every internal link is a bare filename, so it resolves in both
    # the Jenkins-served view and the extracted zip.
    export_assets = [
        f"report_{slug}.pdf",
        f"report_{slug}.xlsx",
        f"report_{slug}.csv",
        f"result_{slug}.json",
    ]
    sub_reports = sorted(
        os.path.basename(p) for p in glob(os.path.join(out_dir, "sub_*.html"))
    )

    # 1. CI dashboard bundle — the dashboard is the entry point.
    ci_dir = os.path.join(out_dir, "ci-report")
    _reset_dir(ci_dir)
    with open(os.path.join(ci_dir, "index.html"), "w", encoding="utf-8") as fh:
        fh.write(html_out)
    _copy_into(
        out_dir,
        [
            f"dashboard_{slug}.css",
            f"dashboard_{slug}.js",
            f"report_{slug}.html",
            f"report_{slug}.css",
            f"report_{slug}.js",
            *export_assets,
            *sub_reports,
        ],
        ci_dir,
    )

    # 2. Full interactive report bundle — only when the report was generated.
    report_html = os.path.join(out_dir, f"report_{slug}.html")
    if os.path.isfile(report_html):
        html_dir = os.path.join(out_dir, "html-report")
        _reset_dir(html_dir)
        shutil.copyfile(report_html, os.path.join(html_dir, "index.html"))
        _copy_into(
            out_dir,
            [f"report_{slug}.css", f"report_{slug}.js", *export_assets, *sub_reports],
            html_dir,
        )

    print(out_path)


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def main() -> None:
    if len(sys.argv) < 2:
        print(
            "Usage: python3 ci/jenkins/generate-dashboard.py <output-dir> [project-slug] [history-file]",
            file=sys.stderr,
        )
        sys.exit(1)

    out_dir = sys.argv[1]
    slug    = sys.argv[2] if len(sys.argv) >= 3 else None

    # 3rd arg overrides SLOC_HISTORY_FILE env var
    if len(sys.argv) >= 4 and sys.argv[3]:
        os.environ["SLOC_HISTORY_FILE"] = sys.argv[3]

    if not os.path.isdir(out_dir):
        print(f"ERROR: output directory does not exist: {out_dir}", file=sys.stderr)
        sys.exit(1)

    generate(out_dir, slug)


if __name__ == "__main__":
    main()
