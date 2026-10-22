#!/usr/bin/env python3
"""Static guard tests for ci/jenkins/pipeline-helpers.groovy.

These lock in fixes that are invisible to a compiler and easy to silently
regress during unrelated edits:

  P0: the bitbucketStatusNotify(...) call MUST be wrapped in `catch (Throwable ...)`,
      not `catch (Exception ...)` / an untyped `catch (e)`. When the
      bitbucket-build-status-notifier plugin is absent, Pipeline throws
      java.lang.NoSuchMethodError — a java.lang.Error, not an Exception — so an
      Exception-typed catch lets it escape, aborts post{always}, and skips the
      plugin-independent REST fallback (breaking the no-admin / Tier-3 path).

  P3: the bitbucketStatusNotify(...) call MUST NOT pass a buildUrl: argument (the
      installed plugin version rejects it with an "Unknown parameter" warning).

  P4: NO Groovy/Jenkinsfile source may contain a bare `\\u` that is not a valid
      four-hex-digit unicode escape. Groovy runs a unicode-escape preprocessing
      pass over the ENTIRE source (comments included) before lexing, so a Windows
      path like `\\usr\\bin` written with a single backslash in a comment aborts
      the whole pipeline with "Did not find four digit hex character code". Escape
      it by doubling the backslash (`\\\\usr`) or use forward slashes.

Run:  python3 ci/jenkins/tests/test-pipeline-helpers-guards.py
Exit code 0 = all guards hold.
"""

import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
HELPERS = os.path.abspath(os.path.join(HERE, "..", "pipeline-helpers.groovy"))
REPO_ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))

RESULTS = []


def check(name, cond, detail=""):
    RESULTS.append((name, bool(cond), detail))


def _groovy_sources():
    """Every file Jenkins/Groovy will compile: *.groovy plus any 'Jenkinsfile'."""
    skip_dirs = {".git", "target", "vendor", "toolchain", "node_modules", ".tools"}
    for root, dirs, files in os.walk(REPO_ROOT):
        dirs[:] = [d for d in dirs if d not in skip_dirs]
        for fname in files:
            if fname.endswith(".groovy") or fname == "Jenkinsfile":
                yield os.path.join(root, fname)


def _bad_unicode_escapes(src):
    """Return (line, col, snippet) for each bare `\\u` that is NOT a valid escape.

    A `\\u` is an *active* Groovy unicode escape only when preceded by an even
    number of backslashes (so the backslash forming `\\u` is unescaped). When it
    is active it must be followed by one-or-more `u` then exactly four hex digits;
    anything else is a hard compile error.
    """
    hits = []
    for m in re.finditer(r"\\+u+", src):
        run = m.group(0)
        n_back = len(run) - run.count("u")  # leading backslashes
        if n_back % 2 == 0:
            continue  # even backslashes => the `\\u` is escaped, not an escape
        after = src[m.end():m.end() + 4]
        if len(after) == 4 and all(c in "0123456789abcdefABCDEF" for c in after):
            continue  # valid \\uXXXX
        pos = m.start()
        line = src.count("\n", 0, pos) + 1
        col = pos - (src.rfind("\n", 0, pos))  # 1-based within line
        snippet = src[max(0, pos - 8):pos + 12].replace("\n", "\\n")
        hits.append((line, col, snippet))
    return hits


def main() -> None:
    with open(HELPERS, encoding="utf-8") as fh:
        src = fh.read()

    # Locate the bitbucketStatusNotify(...) call and the catch clause that
    # immediately follows its closing paren.
    call_idx = src.find("bitbucketStatusNotify(")
    check("bitbucketStatusNotify call present", call_idx != -1)

    if call_idx != -1:
        # The try/catch wrapping the call: find the next `} catch (...)` after it.
        tail = src[call_idx:call_idx + 1200]
        m = re.search(r"\}\s*catch\s*\(([^)]*)\)", tail)
        catch_arg = m.group(1) if m else ""
        check("P0: bitbucketStatusNotify wrapped in catch (Throwable ...)",
              "Throwable" in catch_arg,
              f"catch arg was: {catch_arg!r}")

        # The call args (between the first '(' and its matching close before the
        # catch) must not contain a buildUrl: parameter.
        call_block = tail.split("} catch", 1)[0]
        check("P3: no buildUrl: argument on bitbucketStatusNotify",
              "buildUrl" not in call_block,
              "buildUrl: still present in the plugin call")

    # P4: repo-wide — no bare `\u` unicode-escape hazard in any Groovy source.
    bad = []
    for path in _groovy_sources():
        with open(path, encoding="utf-8") as fh:
            for line, col, snippet in _bad_unicode_escapes(fh.read()):
                rel = os.path.relpath(path, REPO_ROOT).replace(os.sep, "/")
                bad.append(f"{rel}:{line}:{col}  …{snippet}…")
    check("P4: no bare '\\u' unicode-escape hazard in Groovy sources",
          not bad,
          "offending \\u (double the backslash or use '/'):\n        "
          + "\n        ".join(bad))

    print("=" * 78)
    print("pipeline-helpers.groovy - static guard tests")
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
