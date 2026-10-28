#!/usr/bin/env bats
# Behavior tests for scripts/serve-server.sh and scripts/internal/gen-tls-cert.sh.
#
# Each test runs the launcher inside a throwaway "fake repo" (a temp dir laid out
# like the real one) with a STUB binary that records its argv + the SLOC_* env it
# was launched with, then exits 0 — so we exercise argument parsing, the auth gate,
# and the TLS setup without ever starting a real server. HOME is redirected to a
# temp dir so generated certs never touch the developer's profile.
#
# Run: bats scripts/tests/serve-server.bats   (install: https://bats-core.readthedocs.io)

setup() {
    REPO="$BATS_TEST_TMPDIR/repo"
    mkdir -p "$REPO/scripts/internal"
    cp "$BATS_TEST_DIRNAME/../serve-server.sh" "$REPO/scripts/serve-server.sh"
    cp "$BATS_TEST_DIRNAME/../internal/gen-tls-cert.sh" "$REPO/scripts/internal/gen-tls-cert.sh"

    # Stub binary: record argv + relevant env, then exit so the launcher returns.
    cat > "$REPO/oxide-sloc" <<'STUB'
#!/usr/bin/env bash
d="$(cd "$(dirname "$0")" && pwd)"
printf '%s\n' "$*" > "$d/stub.args"
env | grep -E '^SLOC_(TLS|API)' > "$d/stub.env" || true
exit 0
STUB
    chmod +x "$REPO/oxide-sloc"
    # Windows (Git Bash) looks for oxide-sloc.exe; provide both names.
    cp "$REPO/oxide-sloc" "$REPO/oxide-sloc.exe"

    export HOME="$BATS_TEST_TMPDIR/home"
    mkdir -p "$HOME"
    # Start from a clean auth environment for the fail-closed gate tests.
    unset SLOC_API_KEY SLOC_API_KEYS SLOC_ALLOW_UNAUTHENTICATED
    unset SLOC_TLS_CERT SLOC_TLS_KEY SLOC_TLS_CLIENT_CA SLOC_REQUIRE_TLS SLOC_HARDENED
}

launcher() { bash "$REPO/scripts/serve-server.sh" "$@"; }

@test "--help exits 0 and documents the TLS flags" {
    run launcher --help
    [ "$status" -eq 0 ]
    [[ "$output" == *"Serve HTTPS"* ]]
    [[ "$output" == *"--tls-generate"* ]]
    [[ "$output" == *"--mtls"* ]]
}

@test "refuses to start unauthenticated on a network bind without acknowledgement" {
    run launcher --quiet --port 24401
    [ "$status" -eq 1 ]
    [[ "$output" == *"Refusing to start an UNAUTHENTICATED server"* ]]
    [ ! -f "$REPO/stub.args" ]   # never reached the launch
}

@test "--no-auth --i-understand-no-auth launches and forces an all-interfaces bind" {
    run launcher --no-auth --i-understand-no-auth --quiet --port 24402
    [ "$status" -eq 0 ]
    [[ "$(cat "$REPO/stub.args")" == *"--bind 0.0.0.0:24402"* ]]
}

@test "--with-auth generates and exports an API key" {
    run launcher --with-auth --quiet --port 24403
    [ "$status" -eq 0 ]
    grep -q '^SLOC_API_KEY=' "$REPO/stub.env"
}

@test "--port overrides the bind port" {
    run launcher --with-auth --quiet --port 24404
    [ "$status" -eq 0 ]
    [[ "$(cat "$REPO/stub.args")" == *":24404"* ]]
}

@test "--tls without a certificate errors with guidance" {
    run launcher --with-auth --tls --quiet --port 24405
    [ "$status" -eq 1 ]
    [[ "$output" == *"needs a certificate"* ]]
    [ ! -f "$REPO/stub.args" ]
}

@test "--tls --tls-generate creates a cert and exports SLOC_TLS_CERT/KEY" {
    run launcher --with-auth --tls --tls-generate --quiet --port 24406
    [ "$status" -eq 0 ]
    grep -q '^SLOC_TLS_CERT=' "$REPO/stub.env"
    grep -q '^SLOC_TLS_KEY=' "$REPO/stub.env"
    [ -f "$HOME/.oxide-sloc/tls/server.crt" ]
    [ -f "$HOME/.oxide-sloc/tls/server.key" ]
    [ -f "$HOME/.oxide-sloc/tls/ca.crt" ]
}

@test "SLOC_REQUIRE_TLS without --tls is refused up front" {
    export SLOC_API_KEY=preset-key SLOC_REQUIRE_TLS=1
    run launcher --quiet --port 24407
    [ "$status" -eq 1 ]
    [[ "$output" == *"TLS is not enabled"* ]]
}

@test "gen-tls-cert.sh produces a serverAuth cert with SANs" {
    out="$BATS_TEST_TMPDIR/certs"
    run bash "$REPO/scripts/internal/gen-tls-cert.sh" --out "$out" --san 10.9.8.7 --force
    [ "$status" -eq 0 ]
    [ -f "$out/server.crt" ]
    run openssl x509 -in "$out/server.crt" -noout -ext extendedKeyUsage
    [[ "$output" == *"TLS Web Server Authentication"* ]]
    run openssl x509 -in "$out/server.crt" -noout -ext subjectAltName
    [[ "$output" == *"10.9.8.7"* ]]
    [[ "$output" == *"localhost"* ]]
}
