// End-to-end coverage for the `oxide-sloc verify-audit` subcommand
// (crates/sloc-cli/src/main.rs::run_verify_audit), driven through the real
// compiled binary. Asserts the observable contract — exit codes and messages —
// for the argument-resolution and failure branches. The happy (intact-chain)
// path is already covered by the audit-module unit tests; here we exercise the
// CLI wiring that those cannot reach.

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxide-sloc"))
}

#[test]
fn verify_audit_without_log_path_errors() {
    // No positional path and no SLOC_AUDIT_LOG → clap/anyhow context error, non-zero.
    let out = bin()
        .arg("verify-audit")
        .env_remove("SLOC_AUDIT_LOG")
        .env_remove("SLOC_AUDIT_HMAC_KEY")
        .output()
        .expect("run binary");
    assert!(!out.status.success(), "missing log path must fail");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("audit log path"),
        "should explain the missing log path: {err}"
    );
}

#[test]
fn verify_audit_without_key_errors() {
    // A path but no --key and no SLOC_AUDIT_HMAC_KEY → key-resolution error.
    let dir = std::env::temp_dir().join(format!("sloc-cli-va-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("audit.jsonl");
    std::fs::write(&log, "{}\n").unwrap();

    let out = bin()
        .arg("verify-audit")
        .arg(&log)
        .env_remove("SLOC_AUDIT_HMAC_KEY")
        .output()
        .expect("run binary");
    assert!(!out.status.success(), "missing key must fail");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("key"), "should explain the missing key: {err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verify_audit_missing_file_reports_failure_exit_2() {
    // A nonexistent log with a key → verify_audit_file read error → FAIL, exit 2.
    let missing = std::env::temp_dir().join(format!("sloc-cli-nope-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&missing);

    let out = bin()
        .arg("verify-audit")
        .arg(&missing)
        .args(["--key", "some-key"])
        .output()
        .expect("run binary");
    assert_eq!(out.status.code(), Some(2), "tamper/read failure exits 2");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("FAIL"), "failure line expected: {err}");
}

#[test]
fn verify_audit_unchained_log_reports_bad_line_exit_2() {
    // A JSON object with no `mac` is not a chained record → first bad line = 1, exit 2.
    let dir = std::env::temp_dir().join(format!("sloc-cli-va2-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("audit.jsonl");
    std::fs::write(&log, "{\"event\":\"x\",\"outcome\":\"success\"}\n").unwrap();

    let out = bin()
        .arg("verify-audit")
        .arg(&log)
        .args(["--key", "some-key"])
        .output()
        .expect("run binary");
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("FAIL") && err.contains(":1"),
        "should point at the first bad line: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
