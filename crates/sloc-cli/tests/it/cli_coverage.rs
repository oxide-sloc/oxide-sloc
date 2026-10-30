// SPDX-License-Identifier: AGPL-3.0-or-later
// End-to-end coverage for the `oxide-sloc` binary's analyze/report/init/validate
// subcommands, driven through the real CLI. These invocations exercise the large
// terminal-rendering and artifact-writing surface of crates/sloc-cli/src/main.rs
// (run_analyze, write_outputs, write_scan_config, run_report, run_init,
// run_validate, and the print_* summary helpers) that unit tests cannot reach
// because those functions are private to the binary crate. Coverage from the
// spawned child process is captured by cargo-llvm-cov.

use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxide-sloc"))
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

/// A unique temp directory per test, cleaned first.
fn scratch(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("sloc-cli-cov-{}-{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// Populate a small multi-language source tree so the analyzer produces a
/// non-trivial run (multiple languages, comments, a duplicate file).
fn seed_sources(dir: &Path) {
    write(
        &dir.join("lib.rs"),
        "// a rust file\nfn foo() -> i32 {\n    let x = 1; // inline\n    x + 2\n}\n\n#[test]\nfn t() { assert_eq!(foo(), 3); }\n",
    );
    write(
        &dir.join("app.py"),
        "\"\"\"module docstring\"\"\"\nimport sys\n\n\ndef main():\n    # comment\n    return len(sys.argv)\n",
    );
    write(
        &dir.join("main.c"),
        "#include <stdio.h>\n/* block comment */\nint main(void) {\n    printf(\"hi\\n\");\n    return 0;\n}\n",
    );
    write(
        &dir.join("script.js"),
        "// js\nfunction add(a, b) {\n  return a + b; // sum\n}\nexport { add };\n",
    );
    // A duplicate of lib.rs content under a different name for dup detection.
    write(
        &dir.join("dup.rs"),
        "// a rust file\nfn foo() -> i32 {\n    let x = 1; // inline\n    x + 2\n}\n\n#[test]\nfn t() { assert_eq!(foo(), 3); }\n",
    );
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn git(dir: &Path, args: &[&str]) {
    let _ = Command::new("git")
        .args(["-c", "user.email=t@t", "-c", "user.name=Tester"])
        .args(args)
        .current_dir(dir)
        .output();
}

/// Default (coloured-capable, non-TTY) analyze output exercises print_summary
/// and its language/style/submodule table helpers.
#[test]
fn analyze_default_terminal_output() {
    let root = scratch("default");
    let src = root.join("src");
    seed_sources(&src);

    let out = bin().args(["analyze"]).arg(&src).output().unwrap();
    assert!(out.status.success(), "analyze should exit 0");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("Rust") || text.to_lowercase().contains("code"),
        "summary should mention a language / code totals: {text}"
    );
}

/// Per-file output + every artifact format in a single run covers write_outputs
/// (json/html/csv/xlsx/pure-Rust pdf), write_scan_config and print_per_file_table.
#[test]
fn analyze_writes_all_artifacts_and_per_file() {
    let root = scratch("artifacts");
    let src = root.join("src");
    seed_sources(&src);

    let json = root.join("r.json");
    let html = root.join("r.html");
    let csv = root.join("r.csv");
    let xlsx = root.join("r.xlsx");
    let pdf = root.join("r.pdf");
    let sc = root.join("scan-config.json");

    let status = bin()
        .args(["analyze"])
        .arg(&src)
        .arg("--per-file")
        .arg("--report-title")
        .arg("Coverage Fixture")
        .arg("--json-out")
        .arg(&json)
        .arg("--html-out")
        .arg(&html)
        .arg("--csv-out")
        .arg(&csv)
        .arg("--xlsx-out")
        .arg(&xlsx)
        .arg("--pdf-out")
        .arg(&pdf)
        .arg("--scan-config-out")
        .arg(&sc)
        .status()
        .unwrap();
    assert!(status.success(), "analyze with artifacts should succeed");
    for f in [&json, &html, &csv, &xlsx, &pdf, &sc] {
        assert!(f.is_file(), "expected artifact {f:?} to be written");
    }
    // The JSON must be re-readable by the `report` subcommand below.
    let body = std::fs::read_to_string(&json).unwrap();
    assert!(body.contains("summary_totals") || body.contains("files_analyzed"));
}

/// Plain (machine) mode routes through print_plain_summary.
#[test]
fn analyze_plain_mode() {
    let root = scratch("plain");
    let src = root.join("src");
    seed_sources(&src);
    let out = bin()
        .args(["analyze"])
        .arg(&src)
        .arg("--plain")
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains('='), "plain output is key=value: {text}");
}

/// Quiet mode suppresses the summary but still writes artifacts (log_written /
/// write_outputs quiet branch).
#[test]
fn analyze_quiet_still_writes_json() {
    let root = scratch("quiet");
    let src = root.join("src");
    seed_sources(&src);
    let json = root.join("q.json");
    let out = bin()
        .args(["analyze"])
        .arg(&src)
        .arg("--quiet")
        .arg("--json-out")
        .arg(&json)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(json.is_file(), "quiet run must still write JSON");
}

/// --fail-below with an impossibly high threshold triggers the exit-code-3 gate
/// in check_exit_conditions.
#[test]
fn analyze_fail_below_threshold_exits_3() {
    let root = scratch("failbelow");
    let src = root.join("src");
    seed_sources(&src);
    let out = bin()
        .args(["analyze"])
        .arg(&src)
        .arg("--fail-below")
        .arg("100000000")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(3),
        "code lines below threshold must exit 3"
    );
}

/// Attaching a coverage report exercises the coverage-file ingestion path and the
/// print_test_coverage / semantic-metrics rendering.
#[test]
fn analyze_with_coverage_file() {
    let root = scratch("coverage");
    let src = root.join("src");
    seed_sources(&src);
    // Minimal LCOV referencing lib.rs so lookup attaches per-file coverage.
    let lcov = root.join("cov.info");
    write(
        &lcov,
        "TN:\nSF:lib.rs\nDA:2,1\nDA:3,1\nDA:4,0\nLF:3\nLH:2\nend_of_record\n",
    );
    let out = bin()
        .args(["analyze"])
        .arg(&src)
        .arg("--coverage-file")
        .arg(&lcov)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "analyze --coverage-file should succeed"
    );
}

/// The `report` subcommand re-renders a saved JSON to every artifact format
/// (run_report).
#[test]
fn report_rerenders_saved_json() {
    let root = scratch("report");
    let src = root.join("src");
    seed_sources(&src);
    let json = root.join("run.json");
    assert!(
        bin()
            .args(["analyze"])
            .arg(&src)
            .arg("--quiet")
            .arg("--json-out")
            .arg(&json)
            .status()
            .unwrap()
            .success()
    );

    let html = root.join("out.html");
    let csv = root.join("out.csv");
    let xlsx = root.join("out.xlsx");
    let pdf = root.join("out.pdf");
    let status = bin()
        .args(["report"])
        .arg(&json)
        .arg("--html-out")
        .arg(&html)
        .arg("--csv-out")
        .arg(&csv)
        .arg("--xlsx-out")
        .arg(&xlsx)
        .arg("--pdf-out")
        .arg(&pdf)
        .status()
        .unwrap();
    assert!(status.success(), "report should re-render successfully");
    for f in [&html, &csv, &xlsx, &pdf] {
        assert!(f.is_file(), "report must write {f:?}");
    }
}

/// `report` on a missing/garbage input surfaces an error (non-zero exit).
#[test]
fn report_bad_input_fails() {
    let root = scratch("reportbad");
    let bad = root.join("nope.json");
    write(&bad, "{ not valid json");
    let out = bin().args(["report"]).arg(&bad).output().unwrap();
    assert!(!out.status.success(), "garbage JSON must fail report");
}

/// `init` writes a config template, refuses to overwrite, then honours --force.
#[test]
fn init_writes_then_requires_force() {
    let root = scratch("init");
    let cfg = root.join("oxide-sloc.toml");

    assert!(bin().args(["init"]).arg(&cfg).status().unwrap().success());
    assert!(cfg.is_file(), "init must write the config file");
    let first = std::fs::read_to_string(&cfg).unwrap();
    assert!(first.contains("[discovery]"), "template has [discovery]");

    // Second init without --force must fail (file exists).
    let out = bin().args(["init"]).arg(&cfg).output().unwrap();
    assert!(
        !out.status.success(),
        "init without --force must refuse to overwrite"
    );

    // With --force it succeeds again.
    assert!(
        bin()
            .args(["init"])
            .arg(&cfg)
            .arg("--force")
            .status()
            .unwrap()
            .success(),
        "init --force must overwrite"
    );
}

/// `validate` checks a generated config and reports (run_validate).
#[test]
fn validate_generated_config() {
    let root = scratch("validate");
    let cfg = root.join("cfg.toml");
    assert!(bin().args(["init"]).arg(&cfg).status().unwrap().success());
    let out = bin()
        .args(["validate"])
        .arg("--config")
        .arg(&cfg)
        .output()
        .unwrap();
    // Whether it passes or reports issues, it must run to completion (not crash)
    // and emit some diagnostic output.
    assert!(
        out.status.code().is_some(),
        "validate must terminate with an exit code"
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!combined.is_empty(), "validate should emit diagnostics");
}

/// Analyzing a real git checkout drives the git-metadata + activity attachment
/// path inside run_analyze, and baseline save/enforce round-trips.
#[test]
fn analyze_git_repo_baseline_roundtrip() {
    if !git_available() {
        eprintln!("git not available — skipping");
        return;
    }
    let root = scratch("baseline");
    let src = root.join("repo");
    seed_sources(&src);
    git(&src, &["init"]);
    git(&src, &["add", "."]);
    git(&src, &["commit", "-m", "seed"]);

    // Save a baseline (stored under out/baselines.json relative to OXIDE_SLOC_ROOT).
    let save = bin()
        .args(["analyze"])
        .arg(&src)
        .arg("--quiet")
        .arg("--set-baseline")
        .arg("main")
        .env("OXIDE_SLOC_ROOT", &root)
        .status()
        .unwrap();
    assert!(save.success(), "saving a baseline should succeed");

    // Enforce against it with no growth allowed — unchanged tree must pass.
    let enforce = bin()
        .args(["analyze"])
        .arg(&src)
        .arg("--quiet")
        .arg("--fail-above-baseline")
        .arg("main")
        .env("OXIDE_SLOC_ROOT", &root)
        .status()
        .unwrap();
    assert!(
        enforce.success(),
        "an unchanged tree must not exceed its baseline"
    );
}

/// A non-existent scan path is tolerated by discovery (zero files) and still
/// runs to a clean exit — exercising the empty-run rendering branch.
#[test]
fn analyze_missing_path_runs_with_zero_files() {
    let root = scratch("missing");
    let out = bin()
        .args(["analyze"])
        .arg(root.join("does-not-exist"))
        .arg("--plain")
        .output()
        .unwrap();
    assert!(
        out.status.code().is_some(),
        "analyze must terminate with an exit code"
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("=0") || text.is_empty(),
        "a missing path yields an empty/zero-file summary: {text}"
    );
}

/// `diff` with every artifact flag covers run_diff's json/csv/xlsx writers,
/// including write_diff_xlsx.
#[test]
fn diff_writes_json_csv_xlsx_artifacts() {
    let root = scratch("diffart");
    let a = root.join("a");
    let b = root.join("b");
    seed_sources(&a);
    seed_sources(&b);
    // Make b larger so the delta is non-trivial.
    write(&b.join("more.rs"), "pub fn added() -> u8 {\n    42\n}\n");

    let aj = root.join("a.json");
    let bj = root.join("b.json");
    for (src, out) in [(&a, &aj), (&b, &bj)] {
        assert!(
            bin()
                .args(["analyze"])
                .arg(src)
                .arg("--quiet")
                .arg("--json-out")
                .arg(out)
                .status()
                .unwrap()
                .success()
        );
    }

    let dj = root.join("delta.json");
    let dc = root.join("delta.csv");
    let dx = root.join("delta.xlsx");
    let status = bin()
        .args(["diff"])
        .arg(&aj)
        .arg(&bj)
        .arg("--json-out")
        .arg(&dj)
        .arg("--csv-out")
        .arg(&dc)
        .arg("--xlsx-out")
        .arg(&dx)
        .status()
        .unwrap();
    assert!(status.success(), "diff with artifacts should succeed");
    for f in [&dj, &dc, &dx] {
        assert!(f.is_file(), "diff must write {f:?}");
    }
}

/// A tight SLOC budget triggers the exit-code-4 gate (check_budget, both the
/// total and per-language branches).
#[test]
fn analyze_fail_on_budget_exits_4() {
    let root = scratch("budget");
    let src = root.join("src");
    seed_sources(&src);
    let cfg = root.join("budget.toml");
    write(
        &cfg,
        "[analysis.budget]\ntotal_max = 1\n\n[analysis.budget.per_language]\nrust = 1\n",
    );
    let out = bin()
        .args(["analyze"])
        .arg(&src)
        .arg("--config")
        .arg(&cfg)
        .arg("--fail-on-budget")
        .arg("--quiet")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(4),
        "exceeding the SLOC budget must exit 4"
    );
}

/// A zero complexity ceiling flags any file with computed cyclomatic complexity
/// (apply_complexity_gate, exit 6).
#[test]
fn analyze_max_complexity_exits_6() {
    let root = scratch("complexity");
    let src = root.join("src");
    // A function with several branches so cyclomatic complexity is clearly > 0.
    write(
        &src.join("branchy.rs"),
        "pub fn classify(n: i32) -> &'static str {\n    if n < 0 {\n        \"neg\"\n    } else if n == 0 {\n        \"zero\"\n    } else if n < 10 {\n        \"small\"\n    } else {\n        match n % 2 {\n            0 => \"even\",\n            _ => \"odd\",\n        }\n    }\n}\n",
    );
    let out = bin()
        .args(["analyze"])
        .arg(&src)
        .arg("--max-complexity")
        .arg("0")
        .arg("--quiet")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(6),
        "a file over the complexity ceiling must exit 6"
    );
}

/// Growth beyond a saved baseline triggers the exit-code-5 gate
/// (check_against_baseline / --fail-above-baseline).
#[test]
fn analyze_fail_above_baseline_exits_5_on_growth() {
    let root = scratch("grow");
    let src = root.join("src");
    seed_sources(&src);

    // Save the baseline for the small tree.
    assert!(
        bin()
            .args(["analyze"])
            .arg(&src)
            .arg("--quiet")
            .arg("--set-baseline")
            .arg("main")
            .env("OXIDE_SLOC_ROOT", &root)
            .status()
            .unwrap()
            .success()
    );

    // Grow the tree substantially, then enforce with no allowed growth.
    let mut big = String::new();
    for i in 0..50 {
        big.push_str(&format!("pub fn f{i}() -> i32 {{\n    {i}\n}}\n"));
    }
    write(&src.join("grown.rs"), &big);

    let out = bin()
        .args(["analyze"])
        .arg(&src)
        .arg("--quiet")
        .arg("--fail-above-baseline")
        .arg("main")
        .arg("--max-delta-pct")
        .arg("0")
        .env("OXIDE_SLOC_ROOT", &root)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(5),
        "code growth beyond the baseline must exit 5"
    );
}
