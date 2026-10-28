//! Integration tests for the TLS server-config builder and the TLS-enforcement
//! gate that `serve()` uses when `SLOC_TLS_CERT` / `SLOC_TLS_KEY` are set.
//!
//! These exercise `build_tls_config` / `client_cert_verifier` / `require_tls`
//! through the thin `*_for_test` wrappers exposed from the crate. Fixtures live
//! under `tests/fixtures/tls/` (throwaway self-signed certs, valid to 2126).
//!
//! All assertions run in a single `#[test]` so the few that mutate process-global
//! environment variables (`SLOC_TLS_CLIENT_CA`, `SLOC_REQUIRE_TLS`, `SLOC_HARDENED`)
//! never race another test thread in this binary.

use sloc_web::{require_tls_for_test, validate_tls_pair_for_test};

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/tls/{name}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn tls_config_builder_and_gate() {
    let cert = fixture("server.crt");
    let key = fixture("server.key.pem");
    let ca = fixture("ca.crt");

    // 1. A valid PEM cert+key pair builds a usable rustls server config.
    assert!(
        validate_tls_pair_for_test(&cert, &key).is_ok(),
        "valid cert+key pair should build a TLS config"
    );

    // 2. Missing files are a clean error, not a panic.
    assert!(
        validate_tls_pair_for_test("/no/such/cert.pem", "/no/such/key.pem").is_err(),
        "missing cert/key files must error"
    );

    // 3. A cert file passed where a key is expected (not a PEM private key) errors.
    assert!(
        validate_tls_pair_for_test(&cert, &ca).is_err(),
        "a certificate used as the key must error"
    );

    // 4. Mutual TLS: with SLOC_TLS_CLIENT_CA pointing at a valid CA bundle the
    //    builder still succeeds (it wires up a client-cert verifier).
    //    SAFETY: single-threaded within this test; set then clear the var.
    unsafe {
        std::env::set_var("SLOC_TLS_CLIENT_CA", &ca);
    }
    assert!(
        validate_tls_pair_for_test(&cert, &key).is_ok(),
        "mutual-TLS config with a valid client CA should build"
    );
    // A non-existent client CA path must surface as an error.
    unsafe {
        std::env::set_var("SLOC_TLS_CLIENT_CA", "/no/such/ca.pem");
    }
    assert!(
        validate_tls_pair_for_test(&cert, &key).is_err(),
        "a missing client CA bundle must error"
    );
    unsafe {
        std::env::remove_var("SLOC_TLS_CLIENT_CA");
    }

    // 5. The TLS-enforcement gate: off by default, on when either knob is set.
    unsafe {
        std::env::remove_var("SLOC_REQUIRE_TLS");
        std::env::remove_var("SLOC_HARDENED");
    }
    assert!(!require_tls_for_test(), "TLS enforcement is off by default");

    unsafe {
        std::env::set_var("SLOC_REQUIRE_TLS", "1");
    }
    assert!(
        require_tls_for_test(),
        "SLOC_REQUIRE_TLS=1 must enable TLS enforcement"
    );
    unsafe {
        std::env::remove_var("SLOC_REQUIRE_TLS");
        std::env::set_var("SLOC_HARDENED", "1");
    }
    assert!(
        require_tls_for_test(),
        "SLOC_HARDENED=1 must enable TLS enforcement"
    );
    unsafe {
        std::env::remove_var("SLOC_HARDENED");
    }
}
