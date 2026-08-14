#!/usr/bin/env python3
"""Bundle flat CI output artifacts into an ORGANIZED tree, packaged as BOTH .zip
and .tar.gz.

Why two archives?  Some corporate browsers / proxies BLOCK .zip downloads from
Jenkins outright; the .tar.gz is the fallback that gets through (Git Bash `tar`
and 7-Zip both open it). This mirrors the repo's dist/ dual-archive convention.

Why build them here (not with the `zip` binary)?  The `zip` CLI is frequently
absent on locked-down Windows/Linux agents. Python's stdlib ``zipfile`` +
``tarfile`` are always available with the bundled portable CPython, so both
archives are produced without any external tool.

Usage
-----
    python3 ci/jenkins/bundle-artifacts.py <out_dir> <project_slug>

Produces, inside <out_dir>:
    oxide-sloc-report-<project_slug>.zip
    oxide-sloc-report-<project_slug>.tar.gz

Both contain the same ORGANIZED tree, mapping the flat CI files into subfolders:

    html/        report_<slug>.html, report_<slug>.css, report_<slug>.js, sub_*.html
    json/        result_<slug>.json, scan-config_<slug>.json, capabilities.json
    csv/         report_<slug>.csv, per_language.csv, summary.csv,
                 style_analysis.csv, coverage.csv
    pdf/         report_<slug>.pdf
    xlsx/        report_<slug>.xlsx
    config/      scan-config_<slug>.json (also mirrored under json/)
    dashboard/   dashboard_<slug>.html, dashboard_<slug>.css, dashboard_<slug>.js
    local-import/  the `oxide-sloc bundle` local web-UI import tree, if present
                   (unzip it into your oxide-sloc out/web/ directory to use
                   Scan Delta / Compare on the run locally)

Only files that actually exist are added; missing inputs are silently skipped so
a minimal scan (no PDF/XLSX/coverage) still bundles cleanly.
"""
import os
import sys
import tarfile
import zipfile


def _submodule_html_plan(out_dir: str):
    """Per-submodule HTML reports (sub_<name>.html) → html/."""
    plan = []
    entries = sorted(os.listdir(out_dir)) if os.path.isdir(out_dir) else []
    for entry in entries:
        if entry.startswith("sub_") and entry.endswith(".html"):
            src = os.path.join(out_dir, entry)
            if os.path.isfile(src):
                plan.append((src, f"html/{entry}"))
    return plan


def _local_import_plan(out_dir: str):
    """local-import/ tree (from `oxide-sloc bundle`) preserved under local-import/."""
    li_root = os.path.join(out_dir, "local-import")
    if not os.path.isdir(li_root):
        return []
    plan = []
    for root, _dirs, files in os.walk(li_root):
        for f in files:
            src = os.path.join(root, f)
            rel = os.path.relpath(src, li_root).replace(os.sep, "/")
            plan.append((src, f"local-import/{rel}"))
    return plan


def _plan(out_dir: str, slug: str):
    """Return a list of (src_abs, arcname) for every existing flat artifact.

    arcname is the path INSIDE the archive (organized subfolder + basename).
    Directory trees (local-import/) are expanded to their individual files so
    both zipfile and tarfile get a flat, deterministic member list.
    """
    # Flat file → subfolder mapping. A file may map into more than one folder
    # (e.g. scan-config lives under both json/ and config/).
    single = [
        # html/
        (f"report_{slug}.html", "html"),
        (f"report_{slug}.css", "html"),
        (f"report_{slug}.js", "html"),
        # json/
        (f"result_{slug}.json", "json"),
        (f"scan-config_{slug}.json", "json"),
        ("capabilities.json", "json"),
        # csv/
        (f"report_{slug}.csv", "csv"),
        ("per_language.csv", "csv"),
        ("summary.csv", "csv"),
        ("style_analysis.csv", "csv"),
        ("coverage.csv", "csv"),
        # pdf/
        (f"report_{slug}.pdf", "pdf"),
        # xlsx/
        (f"report_{slug}.xlsx", "xlsx"),
        # config/ (scan-config mirrored here as well as under json/)
        (f"scan-config_{slug}.json", "config"),
        # dashboard/
        (f"dashboard_{slug}.html", "dashboard"),
        (f"dashboard_{slug}.css", "dashboard"),
        (f"dashboard_{slug}.js", "dashboard"),
    ]

    plan = []
    for name, folder in single:
        src = os.path.join(out_dir, name)
        if os.path.isfile(src):
            plan.append((src, f"{folder}/{os.path.basename(name)}"))

    plan += _submodule_html_plan(out_dir)
    plan += _local_import_plan(out_dir)
    return plan


def _write_zip(zip_path: str, plan) -> None:
    with zipfile.ZipFile(zip_path, "w", compression=zipfile.ZIP_DEFLATED) as zf:
        seen = set()
        for src, arc in plan:
            if arc in seen:
                continue
            seen.add(arc)
            zf.write(src, arcname=arc)


def _write_tar(tar_path: str, plan) -> None:
    with tarfile.open(tar_path, "w:gz") as tf:
        seen = set()
        for src, arc in plan:
            if arc in seen:
                continue
            seen.add(arc)
            tf.add(src, arcname=arc, recursive=False)


def main() -> int:
    if len(sys.argv) < 3:
        print("Usage: bundle-artifacts.py <out_dir> <project_slug>", file=sys.stderr)
        return 1
    out_dir = os.path.abspath(sys.argv[1])
    slug = sys.argv[2]
    if not os.path.isdir(out_dir):
        print(f"ERROR: output directory does not exist: {out_dir}", file=sys.stderr)
        return 1

    plan = _plan(out_dir, slug)
    if not plan:
        print(f"No artifacts found to bundle in {out_dir} — skipping.", file=sys.stderr)
        # Not an error: a scan with no outputs still lets the build proceed.
        return 0

    zip_path = os.path.join(out_dir, f"oxide-sloc-report-{slug}.zip")
    tar_path = os.path.join(out_dir, f"oxide-sloc-report-{slug}.tar.gz")

    _write_zip(zip_path, plan)
    _write_tar(tar_path, plan)

    print(f"Bundled {len(plan)} file(s) into:")
    print(f"  {zip_path}")
    print(f"  {tar_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
