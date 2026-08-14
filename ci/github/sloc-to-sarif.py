#!/usr/bin/env python3
"""Convert an oxide-sloc result.json into SARIF 2.1.0 for GitHub code scanning.

Surfaces one actionable, per-file signal oxide-sloc computes:

  * high complexity density  → warning (>= warn) / note (>= note)

so it shows up inline in the PR "Files changed" tab via upload-sarif. Standard
library only. (A file-size "hotspot" note used to be emitted too, but it only
restated the obvious — big files are big — without a maintainability signal, so
it was dropped.)

Scope: only production Rust source under ``crates/<crate>/src`` is considered —
the same source set SonarQube analyzes (``sonar.sources=crates`` with tests,
benches, and examples classified as test code). Shell/CI/build helper scripts,
docs tooling, and test files are intentionally *not* complexity-gated: their
branch counts are dominated by sequential setup/dispatch, not maintainability
risk, so gating them only produces noise.

Complexity is scored by *density* — cyclomatic complexity per code line — not by
the whole-file branch-keyword sum. The raw sum is dominated by file size (a big
but flat file trips it), which is just a proxy for line count, not complexity.
Density measures how branch-dense the code actually is, independent of length, so
this rule flags genuinely convoluted files while large-but-linear files are not
flagged at all. A minimum code-line gate avoids flagging tiny utility files
where a couple of branches yield a high ratio. Function-level complexity is
enforced separately by SonarQube's rust:S3776 cognitive-complexity rule.

Usage:
    python3 ci/github/sloc-to-sarif.py <result.json> <out.sarif> \
        [--dens-warn 0.30] [--dens-note 0.20] [--min-code 200]
"""

import json
import sys

TOOL = {
    "driver": {
        "name": "oxide-sloc",
        "informationUri": "https://github.com/oxide-sloc/oxide-sloc",
        "rules": [
            {
                "id": "high-cyclomatic-complexity",
                "name": "HighCyclomaticComplexity",
                "shortDescription": {"text": "High complexity density"},
                "fullDescription": {"text":
                    "This file has a high cyclomatic-complexity density "
                    "(branch-decision points per code line). Dense branching is "
                    "harder to test and maintain; consider simplifying it."},
                "defaultConfiguration": {"level": "warning"},
                "helpUri": "https://en.wikipedia.org/wiki/Cyclomatic_complexity",
            },
        ],
    }
}


def _arg(flag, default, cast=int):
    if flag in sys.argv:
        try:
            return cast(sys.argv[sys.argv.index(flag) + 1])
        except (ValueError, IndexError):
            return default
    return default


# Path segments that mark a Rust file as test/bench/example code rather than
# production source (mirrors sonar.test.inclusions in ci/sonar).
_NON_PRODUCTION_SEGMENTS = ("/tests/", "/benches/", "/examples/")


def _is_production_source(uri):
    """True only for production Rust source under crates/<crate>/src.

    Excludes shell/CI/build scripts, docs tooling, and test/bench/example code —
    the same scope SonarQube gates. Keeps the complexity signal focused on code
    a reviewer would actually be asked to split up.
    """
    path = uri.replace("\\", "/")
    if not path.endswith(".rs"):
        return False
    if not (path.startswith("crates/") and "/src/" in path):
        return False
    if path.endswith("_test.rs") or "/test_" in path:
        return False
    return not any(seg in path for seg in _NON_PRODUCTION_SEGMENTS)


def _result(rule_id, level, uri, message):
    return {
        "ruleId": rule_id,
        "level": level,
        "message": {"text": message},
        "locations": [{
            "physicalLocation": {
                "artifactLocation": {"uri": uri},
                "region": {"startLine": 1},
            }
        }],
        # Stable fingerprint so GitHub dedupes the finding across runs.
        "partialFingerprints": {"oxideSlocFinding": f"{rule_id}:{uri}"},
    }


def _finding_for_record(rec, dens_warn, dens_note, min_code):
    """Return a SARIF result dict for an over-threshold per-file record, else None."""
    if not isinstance(rec, dict):
        return None
    uri = rec.get("relative_path") or rec.get("path") or ""
    if not uri or not _is_production_source(uri):
        return None
    cc = rec.get("cyclomatic_complexity")
    code = (rec.get("effective_counts") or {}).get("code_lines", 0)
    if not (isinstance(cc, int) and isinstance(code, int) and code >= min_code):
        return None
    density = cc / code
    if density >= dens_warn:
        return _result(
            "high-cyclomatic-complexity", "warning", uri,
            f"Complexity density {density:.2f} branches/line "
            f"(~{cc} over {code:,} code lines; threshold {dens_warn:.2f}). "
            "Consider simplifying the branch-heavy logic.")
    if density >= dens_note:
        return _result(
            "high-cyclomatic-complexity", "note", uri,
            f"Complexity density {density:.2f} branches/line "
            f"(~{cc} over {code:,} code lines; threshold {dens_note:.2f}).")
    return None


def main() -> None:
    if len(sys.argv) < 3:
        sys.stderr.write("usage: sloc-to-sarif.py <result.json> <out.sarif>\n")
        sys.exit(2)
    src, dst = sys.argv[1], sys.argv[2]
    # Complexity-density thresholds (branches per code line) — see module docstring.
    dens_warn = _arg("--dens-warn", 0.30, float)
    dens_note = _arg("--dens-note", 0.20, float)
    min_code = _arg("--min-code", 200)

    with open(src, encoding="utf-8") as fh:
        data = json.load(fh)

    results = []
    for rec in data.get("per_file_records", []):
        finding = _finding_for_record(rec, dens_warn, dens_note, min_code)
        if finding is not None:
            results.append(finding)

    sarif = {
        "version": "2.1.0",
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "runs": [{"tool": TOOL, "results": results}],
    }
    with open(dst, "w", encoding="utf-8") as fh:
        json.dump(sarif, fh, indent=2)
    print(f"sloc-to-sarif: wrote {len(results)} finding(s) to {dst}")


if __name__ == "__main__":
    main()
