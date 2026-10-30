// Coverage for the error branches of sloc_web::verify_audit_file
// (crates/sloc-web/src/audit.rs). The intact-chain and tampered-MAC / broken-prev
// paths are covered by the audit module's own unit tests; these exercise the
// remaining failure modes through the public API, asserting the exact report.

use sloc_web::verify_audit_file;

fn tmp_log(name: &str, contents: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sloc-audit-verify-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, contents).unwrap();
    p
}

#[test]
fn missing_file_is_not_ok_and_explains_read_error() {
    let missing =
        std::env::temp_dir().join(format!("sloc-audit-missing-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&missing);
    let r = verify_audit_file(&missing, "key");
    assert!(!r.ok);
    assert_eq!(r.records, 0);
    assert!(
        r.detail
            .as_deref()
            .unwrap_or_default()
            .contains("cannot read"),
        "detail: {:?}",
        r.detail
    );
}

#[test]
fn empty_file_verifies_with_zero_records() {
    // Blank lines are skipped; an all-whitespace log is a vacuously intact chain.
    let p = tmp_log("empty.jsonl", "\n   \n\n");
    let r = verify_audit_file(&p, "key");
    assert!(r.ok, "empty log verifies: {r:?}");
    assert_eq!(r.records, 0);
}

#[test]
fn non_json_line_is_flagged_at_its_line_number() {
    let p = tmp_log("notjson.jsonl", "not a json object\n");
    let r = verify_audit_file(&p, "key");
    assert!(!r.ok);
    assert_eq!(r.first_bad_line, Some(1));
    assert!(
        r.detail
            .as_deref()
            .unwrap_or_default()
            .contains("not a JSON object")
    );
}

#[test]
fn record_without_mac_is_rejected() {
    let p = tmp_log("nomac.jsonl", "{\"prev\":\"genesis\",\"event\":\"x\"}\n");
    let r = verify_audit_file(&p, "key");
    assert!(!r.ok);
    assert_eq!(r.first_bad_line, Some(1));
    assert!(r.detail.as_deref().unwrap_or_default().contains("mac"));
}

#[test]
fn record_without_prev_link_is_rejected() {
    let p = tmp_log("noprev.jsonl", "{\"mac\":\"deadbeef\",\"event\":\"x\"}\n");
    let r = verify_audit_file(&p, "key");
    assert!(!r.ok);
    assert_eq!(r.first_bad_line, Some(1));
    assert!(r.detail.as_deref().unwrap_or_default().contains("prev"));
}
