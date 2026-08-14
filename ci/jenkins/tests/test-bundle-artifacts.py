#!/usr/bin/env python3
"""Contract tests for ci/jenkins/bundle-artifacts.py and the per-repo trend CSV
schema in ci/jenkins/generate-trend-csv.py.

bundle-artifacts.py must:
  (a) produce BOTH oxide-sloc-report-<slug>.zip AND .tar.gz (corporate proxies
      block .zip; the .tar.gz is the fallback);
  (b) map flat CI files into the organized subfolders
      (html/ json/ csv/ pdf/ xlsx/ config/ dashboard/);
  (c) include a local-import/ tree when present (from `oxide-sloc bundle`);
  (d) skip missing inputs without failing;
  (e) build both with stdlib only (no `zip` binary) — verified implicitly since
      the test imports/opens them with zipfile/tarfile.

generate-trend-csv.py must:
  (f) write the 9-column header
      timestamp,build,code_lines,comment_lines,blank_lines,files_analyzed,repo_url,branch,commit
  (g) populate repo_url/branch/commit from SLOC_TREND_* env, normalizing repo_url
      (drop scheme + userinfo + trailing .git);
  (h) append rows to a PRE-EXISTING 6-column CSV without corrupting it (the new
      columns simply appear on the new row) — backward compatibility.

Run:  python3 ci/jenkins/tests/test-bundle-artifacts.py
Exit code 0 = all assertions passed.
"""

import csv
import json
import os
import subprocess
import sys
import tarfile
import tempfile
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
BUNDLE = os.path.abspath(os.path.join(HERE, "..", "bundle-artifacts.py"))
TREND = os.path.abspath(os.path.join(HERE, "..", "generate-trend-csv.py"))

RESULTS = []


def check(name, cond, detail=""):
    RESULTS.append((name, bool(cond), detail))


def _write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(text)


def _sample_result():
    return json.dumps({
        "tool": {"name": "sloc", "version": "1.6.0", "timestamp_utc": "2026-08-14T00:00:00Z"},
        "summary_totals": {
            "code_lines": 1234, "comment_lines": 200,
            "blank_lines": 90, "files_analyzed": 42,
        },
        "totals_by_language": [{"language": "Rust", "code_lines": 1234}],
        "git_remote_url": "https://github.com/acme/widget.git",
        "git_branch": "main",
        "git_commit_short": "abc1234",
    })


def test_bundle():
    slug = "widget_abc1234"
    with tempfile.TemporaryDirectory() as d:
        # Lay down a representative flat artifact set (some formats deliberately
        # ABSENT — pdf/xlsx/coverage — to exercise the skip-missing path).
        _write(os.path.join(d, f"result_{slug}.json"), _sample_result())
        _write(os.path.join(d, f"report_{slug}.html"), "<html>report</html>")
        _write(os.path.join(d, f"report_{slug}.css"), "body{}")
        _write(os.path.join(d, f"report_{slug}.csv"), "a,b\n1,2\n")
        _write(os.path.join(d, f"scan-config_{slug}.json"), "{}")
        _write(os.path.join(d, "per_language.csv"), "language,code_lines\nRust,1234\n")
        _write(os.path.join(d, "summary.csv"), "code_lines\n1234\n")
        _write(os.path.join(d, f"dashboard_{slug}.html"), "<html>dash</html>")
        _write(os.path.join(d, f"dashboard_{slug}.css"), ".x{}")
        _write(os.path.join(d, "sub_alpha.html"), "<html>sub</html>")
        # local-import/ tree (as `oxide-sloc bundle` would produce)
        _write(os.path.join(d, "local-import", "registry.json"), "[]")
        _write(os.path.join(d, "local-import", "run_x", "index.html"), "<html>li</html>")
        _write(os.path.join(d, "local-import", "run_x", "json", "result.json"), "{}")

        rc = subprocess.call([sys.executable, BUNDLE, d, slug])
        check("bundle: exit 0", rc == 0, f"exit={rc}")

        zip_path = os.path.join(d, f"oxide-sloc-report-{slug}.zip")
        tar_path = os.path.join(d, f"oxide-sloc-report-{slug}.tar.gz")
        check("bundle: .zip produced", os.path.isfile(zip_path))
        check("bundle: .tar.gz produced", os.path.isfile(tar_path))

        zip_names = set()
        if os.path.isfile(zip_path):
            with zipfile.ZipFile(zip_path) as zf:
                zip_names = set(zf.namelist())
        tar_names = set()
        if os.path.isfile(tar_path):
            with tarfile.open(tar_path) as tf:
                tar_names = set(tf.getnames())

        # (b) organized subfolders
        check("bundle: html/ mapping", f"html/report_{slug}.html" in zip_names, sorted(zip_names))
        check("bundle: json/ mapping", f"json/result_{slug}.json" in zip_names)
        check("bundle: csv/ mapping", f"csv/report_{slug}.csv" in zip_names)
        check("bundle: dashboard/ mapping", f"dashboard/dashboard_{slug}.html" in zip_names)
        check("bundle: config/ mirror", f"config/scan-config_{slug}.json" in zip_names)
        check("bundle: sub_*.html into html/", "html/sub_alpha.html" in zip_names)

        # (c) local-import tree preserved
        check("bundle: local-import registry", "local-import/registry.json" in zip_names)
        check("bundle: local-import nested run",
              "local-import/run_x/index.html" in zip_names)

        # (d) missing inputs skipped (no pdf/xlsx were written)
        check("bundle: absent pdf skipped",
              not any(n.startswith("pdf/") for n in zip_names))
        check("bundle: absent xlsx skipped",
              not any(n.startswith("xlsx/") for n in zip_names))

        # zip and tar carry the SAME member set (dual archive parity)
        check("bundle: zip == tar member set", zip_names == tar_names,
              f"zip-only={sorted(zip_names - tar_names)} tar-only={sorted(tar_names - zip_names)}")


def _run_trend(out_dir, slug, hist_file, env_extra):
    env = dict(os.environ)
    env["BUILD_NUMBER"] = env_extra.pop("BUILD_NUMBER", "7")
    env.update(env_extra)
    return subprocess.call([sys.executable, TREND, out_dir, slug, hist_file], env=env)


def test_trend_schema():
    slug = "widget_abc1234"
    with tempfile.TemporaryDirectory() as d:
        _write(os.path.join(d, f"result_{slug}.json"), _sample_result())
        hist = os.path.join(d, "history.csv")

        # (f) + (g): fresh CSV gets the 9-col header; repo_url normalized.
        rc = _run_trend(d, slug, hist, {
            "BUILD_NUMBER": "10",
            "SLOC_TREND_REPO_URL": "https://user:tok@github.com/acme/widget.git",
            "SLOC_TREND_BRANCH": "develop",
            "SLOC_TREND_COMMIT": "abc1234",
        })
        check("trend: exit 0 (fresh)", rc == 0, f"exit={rc}")

        with open(hist, newline="", encoding="utf-8") as fh:
            reader = csv.reader(fh)
            header = next(reader)
            rows = list(reader)
        expect_header = ["timestamp", "build", "code_lines", "comment_lines",
                         "blank_lines", "files_analyzed", "repo_url", "branch", "commit"]
        check("trend: 9-column header", header == expect_header, header)
        check("trend: one data row", len(rows) == 1, rows)
        if rows:
            row = dict(zip(header, rows[0]))
            check("trend: repo_url normalized (no scheme/userinfo/.git)",
                  row["repo_url"] == "github.com/acme/widget", row["repo_url"])
            check("trend: branch populated", row["branch"] == "develop", row["branch"])
            check("trend: commit populated", row["commit"] == "abc1234", row["commit"])

        # (h) backward compat: pre-existing 6-column CSV, then append a new row.
        legacy = os.path.join(d, "legacy.csv")
        _write(
            legacy,
            "timestamp,build,code_lines,comment_lines,blank_lines,files_analyzed\n"
            "2026-01-01T00:00:00Z,1,100,10,5,3\n",
        )
        rc = _run_trend(d, slug, legacy, {
            "BUILD_NUMBER": "2",
            "SLOC_TREND_REPO_URL": "git@github.com:acme/widget.git",
            "SLOC_TREND_BRANCH": "main",
            "SLOC_TREND_COMMIT": "def5678",
        })
        check("trend: exit 0 (append to legacy)", rc == 0, f"exit={rc}")
        with open(legacy, newline="", encoding="utf-8") as fh:
            all_rows = list(csv.reader(fh))
        # Old header untouched (still 6 cols), the OLD row still parses, and the
        # NEW row carries the 3 extra fields at the end.
        check("trend: legacy old header preserved (6 cols)",
              len(all_rows[0]) == 6, all_rows[0])
        check("trend: legacy old row preserved",
              all_rows[1][:6] == ["2026-01-01T00:00:00Z", "1", "100", "10", "5", "3"],
              all_rows[1])
        check("trend: legacy new row has 9 fields",
              len(all_rows[2]) == 9, all_rows[2])
        if len(all_rows) > 2 and len(all_rows[2]) == 9:
            check("trend: legacy new-row repo_url normalized",
                  all_rows[2][6] == "github.com/acme/widget", all_rows[2][6])


def main():
    test_bundle()
    test_trend_schema()

    print("=" * 78)
    print("bundle-artifacts.py + trend-csv schema — contract tests")
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
    sys.exit(0 if all_ok else 1)


if __name__ == "__main__":
    main()
