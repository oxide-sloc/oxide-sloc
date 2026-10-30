// End-to-end coverage for the `oxide-sloc diff` subcommand
// (crates/sloc-cli/src/main.rs::run_diff / print_diff_summary), driven through the
// real binary. Analyzes two directories to JSON, then diffs them in both the plain
// (machine) and default (coloured) output modes — covering both files_total lines
// of print_diff_summary. Asserts observable stdout, not just exit status.

use std::path::Path;
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

#[test]
fn diff_summary_reports_files_total_in_plain_and_coloured_modes() {
    let root = std::env::temp_dir().join(format!("sloc-diff-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let a_src = root.join("a");
    let b_src = root.join("b");
    write(&a_src.join("lib.rs"), "fn foo() {}\n");
    // b differs from a so the delta has non-trivial file counts.
    write(&b_src.join("lib.rs"), "fn foo() {}\nfn bar() {}\n");
    write(&b_src.join("extra.rs"), "pub fn added() {}\n");

    let a_json = root.join("a.json");
    let b_json = root.join("b.json");
    for (dir, out) in [(&a_src, &a_json), (&b_src, &b_json)] {
        let status = bin()
            .args(["analyze"])
            .arg(dir)
            .arg("--plain")
            .arg("--json-out")
            .arg(out)
            .status()
            .expect("run analyze");
        assert!(status.success(), "analyze {dir:?} should succeed");
        assert!(out.is_file(), "analyze must write {out:?}");
    }

    // Plain mode → machine-readable key=value lines, including files_total=.
    let plain = bin()
        .args(["diff"])
        .arg(&a_json)
        .arg(&b_json)
        .arg("--plain")
        .output()
        .expect("run diff --plain");
    assert!(plain.status.success(), "diff --plain should succeed");
    let plain_out = String::from_utf8_lossy(&plain.stdout);
    assert!(
        plain_out.contains("files_total="),
        "plain diff must emit files_total=: {plain_out}"
    );

    // Default (coloured) mode → the human summary, which prints "... total=N".
    let pretty = bin()
        .args(["diff"])
        .arg(&a_json)
        .arg(&b_json)
        .output()
        .expect("run diff");
    assert!(pretty.status.success(), "diff should succeed");
    let pretty_out = String::from_utf8_lossy(&pretty.stdout);
    assert!(
        pretty_out.contains("SLOC Diff") && pretty_out.contains("total="),
        "human diff must include the Files … total= line: {pretty_out}"
    );

    let _ = std::fs::remove_dir_all(&root);
}
