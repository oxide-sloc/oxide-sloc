#!/usr/bin/env python3
"""Guard against status-masking shell pipes in Jenkins Groovy and ci/ shell scripts.

Jenkins `sh` runs dash, where a pipeline's exit status is the *last* command's.
So `cmd | tail` reports 0 even when `cmd` failed, and a `sh(..., returnStatus:)`
check reads that 0 as success. This masking produced three green-but-broken
builds (fixed in 73410cb / 07f1acd) before it was caught. This linter fails CI on
the known-bad forms so the pattern cannot recur.

Rules (see ci-integration docs / commit history):
  1. A top-level pipe (`|`, not `||`, not inside `$(...)`/backticks) in an `sh`
     step, when the pipe can mask the status the pipeline actually cares about:
       (a) any top-level pipe in a `returnStatus:`/`returnStdout:` step — the
           captured value is the last command's, so the check always sees 0; or
       (b) a top-level pipe into `tail`/`head`/`tee` in any `sh` step — the
           truncation/duplication idioms silently swallow the upstream failure.
           A bare `… | tee out.txt` is the defect that let a compile error with
           no junit.xml produce a green build.
     Exempt when the step begins with a `#!/bin/bash` shebang and sets pipefail.
  2. `set -o pipefail` in an `sh` step that does NOT begin with `#!/bin/bash` —
     Jenkins `sh` is dash, which aborts with "set: Illegal option -o pipefail".
  3. Any `ci/**/*.sh` whose `set` options omit `pipefail`.

Fast (<2s), no Jenkins/cargo/network. Run: `python3 ci/lint-pipeline-shell.py`
(scans the repo) or pass explicit files. Regression coverage lives in
`ci/lint-pipeline-shell-test.py` (run by the wrapper before the scan).
"""
from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent  # repo root (ci/..)

# Groovy files that contain Jenkins `sh` steps. The SLOC test corpus
# (crates/**/tests/corpus) is intentionally excluded — it is analyzer input,
# not pipeline code.
GROOVY_GLOBS = ["Jenkinsfile", "testing/examples/jenkins/Jenkinsfile"]
MASKING_FILTERS = {"tail", "head", "tee"}


# ── string / paren scanning over Groovy source ───────────────────────────────
def _read_string(text: str, i: int):
    """text[i] is a quote. Return (raw_content, content_start_index, end_index)."""
    if text.startswith("'''", i) or text.startswith('"""', i):
        q = text[i : i + 3]
        j = text.find(q, i + 3)
        if j == -1:
            j = len(text)
        return text[i + 3 : j], i + 3, j + 3
    q = text[i]
    j = i + 1
    while j < len(text):
        c = text[j]
        if c == "\\" and j + 1 < len(text):
            j += 2
            continue
        if c == q:
            return text[i + 1 : j], i + 1, j + 1
        j += 1
    return text[i + 1 : j], i + 1, j


def _match_paren(text: str, i: int) -> int:
    """text[i] == '('. Return index just past the matching ')', skipping strings."""
    depth = 0
    j = i
    while j < len(text):
        c = text[j]
        if c in "'\"":
            _, _, j = _read_string(text, j)
            continue
        if c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                return j + 1
        j += 1
    return len(text)


def _sh_paren_step(text: str, k: int):
    """Handle the `sh(...)` paren-call form starting at the '(' at index k.

    Returns (script_text, script_abs_start, has_return) or None when the call has
    no string literal.
    """
    end = _match_paren(text, k)
    span = text[k:end]
    has_return = "returnStatus" in span or "returnStdout" in span
    # Locate the script string: prefer the value after a `script:` label, else the
    # first string literal in the call.
    search_from = k
    lbl = re.search(r"script\s*:", span)
    if lbl:
        search_from = k + lbl.end()
    j = search_from
    while j < end:
        if text[j] in "'\"":
            content, start, _ = _read_string(text, j)
            return content, start, has_return
        j += 1
    return None


def _iter_sh_steps(text: str):
    """Yield (script_text, script_abs_start, has_return) for each `sh` step."""
    for m in re.finditer(r"(?<![\w.$])sh(?![\w])", text):
        k = m.end()
        while k < len(text) and text[k] in " \t\r\n":
            k += 1
        if k >= len(text):
            continue
        if text[k] == "(":
            step = _sh_paren_step(text, k)
            if step is not None:
                yield step
        elif text[k] in "'\"":
            content, start, _ = _read_string(text, k)
            yield content, start, False


# ── shell-aware top-level pipe detection ─────────────────────────────────────
def _skip_quoted(script: str, i: int, n: int, quote: str) -> int:
    """Advance past a quoted region whose opening quote is at index i.

    Honours backslash escapes only inside double quotes (matching shell rules and
    the original inline state machine). Returns the index just after the closing
    quote, or n if the quote is unterminated.
    """
    i += 1
    while i < n:
        c = script[i]
        if quote == '"' and c == "\\" and i + 1 < n:
            i += 2
            continue
        if c == quote:
            return i + 1
        i += 1
    return i


def _is_double_pipe(script: str, i: int, n: int) -> bool:
    """True when the `|` at index i is part of a `||` operator."""
    return (i + 1 < n and script[i + 1] == "|") or (i > 0 and script[i - 1] == "|")


def _pipe_filter_word(script: str, i: int) -> str:
    """The command word immediately after the `|` at index i (or '')."""
    mm = re.match(r"\s*([\w./-]+)", script[i + 1 :])
    return mm.group(1) if mm else ""


def _top_level_pipes(script: str):
    """Return the filter word after each top-level `|` (skips ||, $(...), quotes)."""
    pipes = []
    i, n = 0, len(script)
    sub = 0
    while i < n:
        c = script[i]
        if c in "'\"`":
            i = _skip_quoted(script, i, n, c)
            continue
        if c == "$" and i + 1 < n and script[i + 1] == "(":
            sub += 1
            i += 2
            continue
        if c == ")" and sub > 0:
            sub -= 1
        elif c == "|":
            if _is_double_pipe(script, i, n):
                i += 1
                continue  # part of ||
            if sub == 0:
                pipes.append(_pipe_filter_word(script, i))
        i += 1
    return pipes


def _line_of(text: str, abs_off: int) -> int:
    return text.count("\n", 0, abs_off) + 1


# ── rule engine ──────────────────────────────────────────────────────────────
def _rule2_finding(path, text, script, start, starts_bash):
    """Rule 2: `set -o pipefail` in a non-bash sh step (dash aborts). Tuple or None."""
    if "pipefail" in script and not starts_bash:
        return (path, _line_of(text, start), 2,
                "`set -o pipefail` in an sh step without a #!/bin/bash shebang "
                "(Jenkins sh is dash and aborts on it)")
    return None


def _rule1_finding(path, text, script, start, has_return):
    """Rule 1: status-masking top-level pipe in an sh step. Tuple or None."""
    pipes = _top_level_pipes(script)
    if not pipes:
        return None
    masking = [f for f in pipes if f in MASKING_FILTERS]
    if not (has_return or masking):
        return None
    why = ("piped `sh` step under returnStatus/returnStdout"
           if has_return else "top-level pipe into `%s`" % masking[0])
    return (path, _line_of(text, start), 1,
            "status-masking pipe in sh step (%s); under dash the pipeline's "
            "exit status is the last command's. Add a #!/bin/bash shebang + "
            "`set -o pipefail`, or redirect to a log and check the status "
            "directly." % why)


def check_groovy(path: str, text: str, findings: list):
    for script, start, has_return in _iter_sh_steps(text):
        first = next((ln for ln in script.splitlines() if ln.strip()), "").strip()
        starts_bash = first.startswith("#!") and "bash" in first
        f2 = _rule2_finding(path, text, script, start, starts_bash)
        if f2:
            findings.append(f2)
        # Rule 1 is exempt when bash + pipefail.
        if starts_bash and "pipefail" in script:
            continue
        f1 = _rule1_finding(path, text, script, start, has_return)
        if f1:
            findings.append(f1)


def check_shell(path: str, text: str, findings: list):
    # Rule 3: a ci/ shell script whose `set` options omit pipefail.
    if not re.search(r"^\s*set\s+[-\w ]*\bpipefail\b", text, re.M) and not re.search(
        r"^\s*set\s+-o\s+pipefail\b", text, re.M
    ):
        findings.append(
            (path, 1, 3,
             "ci/ shell script does not `set -o pipefail` (masks failures in any "
             "pipeline; every ci/**/*.sh must set it)")
        )


def _default_targets():
    groovy = [p for p in GROOVY_GLOBS if (ROOT / p).is_file()]
    groovy += sorted(p.relative_to(ROOT).as_posix() for p in (ROOT / "ci").rglob("*.groovy"))
    shell = sorted(p.relative_to(ROOT).as_posix() for p in (ROOT / "ci").rglob("*.sh"))
    return groovy, shell


def _is_groovy(path: str) -> bool:
    return path.endswith(".groovy") or pathlib.Path(path).name == "Jenkinsfile"


def main(argv):
    findings: list = []
    if argv:
        for a in argv:
            p = pathlib.Path(a)
            text = p.read_text(encoding="utf-8", errors="replace")
            if _is_groovy(a):
                check_groovy(a, text, findings)
            elif a.endswith(".sh"):
                check_shell(a, text, findings)
    else:
        groovy, shell = _default_targets()
        for rel in groovy:
            check_groovy(rel, (ROOT / rel).read_text(encoding="utf-8", errors="replace"), findings)
        for rel in shell:
            check_shell(rel, (ROOT / rel).read_text(encoding="utf-8", errors="replace"), findings)

    findings.sort(key=lambda f: (f[0], f[1], f[2]))
    for path, line, rule, msg in findings:
        print(f"{path}:{line}: [rule {rule}] {msg}")
    if findings:
        print(f"\n{len(findings)} status-masking finding(s). See ci/lint-pipeline-shell.py.",
              file=sys.stderr)
        return 1
    print("pipeline-shell guard: no status-masking pipes found.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
