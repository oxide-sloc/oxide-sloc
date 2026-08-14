#!/usr/bin/env python3
"""Write trend CSVs for the Jenkins Plot plugin and update persistent build history.

Usage: generate-trend-csv.py <out_dir> <project_slug> <history_file>

Persistent history schema (per-job CSV at <history_file>):
    timestamp,build,code_lines,comment_lines,blank_lines,files_analyzed,repo_url,branch,commit

The last three columns — repo_url, branch, commit — were appended to the original
six-column schema so each row records which repository/branch/commit produced it.
They are appended at the END so pre-existing six-column CSVs remain parseable (the
dashboard reads the new columns with an empty-string default when absent).

repo_url/branch/commit are populated from the environment variables the Jenkins
pipeline exports before calling this script:
    SLOC_TREND_REPO_URL   effective scanned repo URL (SCAN_REPO_URL for an external
                          scan, else the tooling repo the job was built from)
    SLOC_TREND_BRANCH     the resolved branch name for the scanned checkout
    SLOC_TREND_COMMIT     the scanned commit (env.GIT_COMMIT / target HEAD SHA)

repo_url is normalized (trailing .git stripped, any user:token@ / user@ userinfo
and URL scheme removed) so downstream same-repo comparison is credential-agnostic.
"""
import csv
import json
import os
import re
import sys
import time


def _norm_repo_url(url: str) -> str:
    """Normalize a repo URL for cross-run comparison (mirrors generate-dashboard.py).

    Strips a URL scheme, any ``user:token@`` / ``user@`` userinfo, a trailing
    ``.git``, and a trailing slash, then lower-cases. Returns "" for a blank input.
    """
    if not url:
        return ""
    s = str(url).strip()
    if not s:
        return ""
    s = re.sub(r"^[A-Za-z][A-Za-z0-9+.\-]*://", "", s)   # drop scheme
    s = re.sub(r"^[^@/]+@", "", s)                        # drop userinfo
    # Normalize scp-style "host:path" to "host/path" so it matches the URL form.
    if ":" in s and "/" not in s.split(":", 1)[0]:
        s = s.replace(":", "/", 1)
    s = re.sub(r"\.git$", "", s)
    return s.rstrip("/").lower()


def _csv_field(value: str) -> str:
    """Quote a CSV field if it contains a comma, quote, or newline (RFC 4180)."""
    s = "" if value is None else str(value)
    if any(c in s for c in (",", '"', "\n", "\r")):
        return '"' + s.replace('"', '""') + '"'
    return s


def main():
    if len(sys.argv) < 4:
        print("Usage: generate-trend-csv.py <out_dir> <project_slug> <history_file>")
        sys.exit(1)

    out = sys.argv[1]
    proj = sys.argv[2]
    history_file = sys.argv[3]

    # ── SLOC summary CSV ─────────────────────────────────────────────────────
    result_path = os.path.join(out, f"result_{proj}.json")
    totals = None
    if os.path.exists(result_path):
        with open(result_path) as f:
            data = json.load(f)
        totals = data["summary_totals"]

        with open(os.path.join(out, "summary.csv"), "w", newline="") as f:
            w = csv.writer(f)
            w.writerow(["code_lines", "comment_lines", "blank_lines", "files_analyzed"])
            w.writerow([
                totals["code_lines"], totals["comment_lines"],
                totals["blank_lines"], totals["files_analyzed"],
            ])

        langs = data.get("totals_by_language", [])
        with open(os.path.join(out, "per_language.csv"), "w", newline="") as f:
            w = csv.writer(f)
            w.writerow(["language", "code_lines"])
            for lang in langs:
                display = lang.get("language", {})
                name = display if isinstance(display, str) else str(display)
                w.writerow([name, lang["code_lines"]])

        print("SLOC trend CSVs written to:", out)

        style_summary = data.get("style_summary")
        if style_summary:
            col_threshold = style_summary.get("col_threshold", 80)
            with open(os.path.join(out, "style_analysis.csv"), "w", newline="") as f:
                w = csv.writer(f)
                w.writerow([
                    "style_files_analyzed",
                    f"{col_threshold}_col_compliant_pct",
                    "style_language_groups",
                ])
                w.writerow([
                    style_summary.get("files_analyzed", 0),
                    style_summary.get("line_col_compliant_pct", 0),
                    len(style_summary.get("by_language", [])),
                ])
            print(f"Style trend CSV: {col_threshold}-col compliant={style_summary.get('line_col_compliant_pct', 0)}%")
        else:
            print("No style_summary in result.json — skipping style CSV")
    else:
        print("result.json not found — skipping SLOC CSV generation")

    # ── Coverage CSV ─────────────────────────────────────────────────────────
    lcov_path = os.path.join(out, "coverage", "lcov.info")
    if os.path.exists(lcov_path):
        total = hit = 0
        with open(lcov_path) as f:
            for line in f:
                line = line.strip()
                if line.startswith("LF:"):
                    total += int(line[3:])
                elif line.startswith("LH:"):
                    hit += int(line[3:])
        pct = round(hit / total * 100, 1) if total > 0 else 0.0
        with open(os.path.join(out, "coverage.csv"), "w", newline="") as f:
            w = csv.writer(f)
            w.writerow(["line_coverage_pct"])
            w.writerow([pct])
        print(f"Coverage trend CSV: {pct}% line coverage ({hit}/{total} lines hit)")
    else:
        print("lcov.info not found — skipping coverage CSV")

    # ── Persistent trend history ─────────────────────────────────────────────
    if history_file and totals and os.path.exists(result_path):
        ts = time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())
        build_num = os.environ.get('BUILD_NUMBER', '0')
        hist_dir = os.path.dirname(history_file)
        if hist_dir:
            os.makedirs(hist_dir, exist_ok=True)

        # Repo identity for this row. repo_url/branch/commit come from the env the
        # pipeline exports; repo_url falls back to the result JSON's own
        # git_remote_url, commit to git_commit_(short|long), branch to git_branch —
        # so a self-scan (no external SCAN_REPO_URL) still records identity.
        repo_url = os.environ.get('SLOC_TREND_REPO_URL', '').strip()
        if not repo_url:
            repo_url = str(data.get('git_remote_url', '') or '').strip()
        repo_url = _norm_repo_url(repo_url)
        branch = os.environ.get('SLOC_TREND_BRANCH', '').strip()
        if not branch:
            branch = str(data.get('git_branch', '') or '').strip()
        commit = os.environ.get('SLOC_TREND_COMMIT', '').strip()
        if not commit:
            commit = str(data.get('git_commit_short') or data.get('git_commit_long') or '').strip()

        # New 9-column header. Appended columns keep old 6-column CSVs parseable.
        header = ('timestamp,build,code_lines,comment_lines,blank_lines,'
                  'files_analyzed,repo_url,branch,commit\n')
        if not os.path.exists(history_file):
            with open(history_file, 'w') as hf:
                hf.write(header)
        with open(history_file, 'a') as hf:
            hf.write(
                f"{ts},{build_num},{totals['code_lines']},"
                f"{totals['comment_lines']},{totals['blank_lines']},"
                f"{totals['files_analyzed']},"
                f"{_csv_field(repo_url)},{_csv_field(branch)},{_csv_field(commit)}\n"
            )
        with open(history_file) as hf:
            lines = hf.readlines()
        if len(lines) > 51:
            with open(history_file, 'w') as hf:
                hf.write(lines[0])
                hf.writelines(lines[-50:])
        print(f"Trend history updated: {history_file} ({len(lines)} entries)")


if __name__ == '__main__':
    main()
