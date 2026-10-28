#!/usr/bin/env bash
# Generate a self-signed TLS server certificate for oxide-sloc HTTPS.
#
# Creates a local root CA and a leaf server certificate (extendedKeyUsage =
# serverAuth) with Subject Alternative Names for localhost, this machine's
# hostname, and every routable LAN IP, so browsers and curl can verify the
# connection. Intended for air-gapped / LAN hosting where no public CA is
# reachable; for internet-facing deployments use a CA-issued cert instead.
#
# Output (default $HOME/.oxide-sloc/tls/, never the repo):
#   server.crt   leaf certificate (what SLOC_TLS_CERT points at)
#   server.key   leaf private key (0600; what SLOC_TLS_KEY points at)
#   ca.crt       root CA certificate — distribute this to clients to trust
#
# Usage:
#   bash scripts/internal/gen-tls-cert.sh                 # auto-detect SANs
#   bash scripts/internal/gen-tls-cert.sh --san 10.0.0.8 --san sloc.corp.local
#   bash scripts/internal/gen-tls-cert.sh --out /etc/oxide-sloc/tls --days 825
#   bash scripts/internal/gen-tls-cert.sh --force         # overwrite existing
#
# Extra SANs may also be passed as a comma-separated SLOC_TLS_SAN env var
# (serve-server.sh uses this to inject the LAN IPs it already classified).
#
# The config-file idiom (no `-addext`) is used so this works on RHEL 8's
# OpenSSL 1.1.1 as well as the OpenSSL bundled with Git Bash on Windows.
set -euo pipefail

OUT_DIR="${HOME:-.}/.oxide-sloc/tls"
DAYS=825           # <= 825 keeps leaf certs within modern browser acceptance
FORCE=false
EXTRA_SANS=()

# Collect extra SANs from the environment first (comma-separated), then flags.
if [[ -n "${SLOC_TLS_SAN:-}" ]]; then
    IFS=',' read -r -a _env_sans <<< "$SLOC_TLS_SAN"
    for s in "${_env_sans[@]}"; do
        s="${s// /}"
        [[ -n "$s" ]] && EXTRA_SANS+=("$s")
    done
fi

while [[ $# -gt 0 ]]; do
    case "$1" in
        --out)   OUT_DIR="$2"; shift 2 ;;
        --out=*) OUT_DIR="${1#--out=}"; shift ;;
        --days)   DAYS="$2"; shift 2 ;;
        --days=*) DAYS="${1#--days=}"; shift ;;
        --san)   EXTRA_SANS+=("$2"); shift 2 ;;
        --san=*) EXTRA_SANS+=("${1#--san=}"); shift ;;
        --force) FORCE=true; shift ;;
        --help|-h)
            sed -n '2,33p' "$0"
            exit 0
            ;;
        *) printf 'gen-tls-cert.sh: unknown argument: %s\n' "$1" >&2; exit 2 ;;
    esac
done

if ! command -v openssl &>/dev/null; then
    printf 'ERROR: openssl not found.\n' >&2
    printf '  openssl ships with Git Bash on Windows and is preinstalled on RHEL/Linux.\n' >&2
    printf '  Install it, or provide SLOC_TLS_CERT / SLOC_TLS_KEY from an external CA.\n' >&2
    exit 1
fi

SERVER_CRT="$OUT_DIR/server.crt"
SERVER_KEY="$OUT_DIR/server.key"
CA_CRT="$OUT_DIR/ca.crt"
CA_KEY="$OUT_DIR/ca.key"

# Idempotent: if a non-expired cert already exists, reuse it unless --force. The
# `-checkend` test returns success when the cert is valid for at least one more
# hour, so a launcher can call this every start without regenerating each time.
if [[ "$FORCE" != true && -f "$SERVER_CRT" && -f "$SERVER_KEY" ]]; then
    if openssl x509 -in "$SERVER_CRT" -checkend 3600 &>/dev/null; then
        printf 'Reusing existing certificate: %s\n' "$SERVER_CRT" >&2
        printf '%s\n' "$SERVER_CRT"   # stdout: cert path, for callers to capture
        exit 0
    fi
fi

mkdir -p "$OUT_DIR"
chmod 700 "$OUT_DIR" 2>/dev/null || true   # no-op on Windows NTFS, harmless

# ── Build the SAN list ───────────────────────────────────────────────────────
HOSTNAME_FQDN="$(hostname 2>/dev/null || echo localhost)"
declare -a SAN_LINES=()
_dns_idx=0
_ip_idx=0
add_dns() { SAN_LINES+=("DNS.$((++_dns_idx)) = $1"); }
add_ip()  { SAN_LINES+=("IP.$((++_ip_idx)) = $1"); }

add_dns "localhost"
[[ -n "$HOSTNAME_FQDN" && "$HOSTNAME_FQDN" != "localhost" ]] && add_dns "$HOSTNAME_FQDN"
add_ip "127.0.0.1"
add_ip "::1"

# Auto-detect routable IPv4 addresses when none were supplied explicitly.
if [[ ${#EXTRA_SANS[@]} -eq 0 ]]; then
    if command -v ip &>/dev/null; then
        while IFS= read -r _a; do
            [[ -n "$_a" ]] && EXTRA_SANS+=("$_a")
        done < <(ip -o -4 addr show 2>/dev/null | awk '{print $4}' | cut -d/ -f1 \
                 | grep -vE '^(127\.|169\.254\.)' || true)
    elif command -v hostname &>/dev/null; then
        while IFS= read -r _a; do
            [[ -n "$_a" ]] && EXTRA_SANS+=("$_a")
        done < <(hostname -I 2>/dev/null | tr ' ' '\n' | grep -vE '^(127\.|$)' || true)
    fi
fi

# Classify each extra SAN as IP or DNS and append (dedup against what we have).
is_ipv4() { [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; }
for entry in "${EXTRA_SANS[@]:-}"; do
    [[ -z "$entry" ]] && continue
    if is_ipv4 "$entry"; then
        printf '%s\n' "${SAN_LINES[@]}" | grep -qxF "IP.$((_ip_idx)) = $entry" && continue
        add_ip "$entry"
    else
        add_dns "$entry"
    fi
done

SAN_BLOCK="$(printf '%s\n' "${SAN_LINES[@]}")"

# ── OpenSSL config (config-file idiom; no -addext for 1.1.1 compatibility) ─────
CA_CNF="$OUT_DIR/.ca.cnf"
LEAF_CNF="$OUT_DIR/.leaf.cnf"

cat > "$CA_CNF" <<EOF
[ req ]
default_bits       = 4096
prompt             = no
distinguished_name = dn
x509_extensions    = v3_ca

[ dn ]
CN = OxideSLOC Local TLS Root CA
O  = oxide-sloc
C  = US

[ v3_ca ]
basicConstraints     = critical,CA:TRUE,pathlen:0
keyUsage             = critical,keyCertSign,cRLSign
subjectKeyIdentifier = hash
EOF

cat > "$LEAF_CNF" <<EOF
[ req ]
default_bits       = 2048
prompt             = no
distinguished_name = dn
req_extensions     = v3_leaf

[ dn ]
CN = ${HOSTNAME_FQDN}
O  = oxide-sloc
C  = US

[ v3_leaf ]
basicConstraints       = CA:FALSE
keyUsage               = critical,digitalSignature,keyEncipherment
extendedKeyUsage       = serverAuth
subjectKeyIdentifier   = hash
subjectAltName         = @alt_names

[ alt_names ]
${SAN_BLOCK}
EOF

printf 'Generating self-signed TLS certificate (%s days)...\n' "$DAYS" >&2
printf '  SANs:\n' >&2
printf '    %s\n' "${SAN_LINES[@]}" >&2

umask 077

# 1) Root CA
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:4096 -out "$CA_KEY" 2>/dev/null
openssl req -new -x509 -config "$CA_CNF" -key "$CA_KEY" -out "$CA_CRT" -days "$DAYS" 2>/dev/null

# 2) Leaf server cert signed by the CA (SAN carried via -extfile/-extensions)
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$SERVER_KEY" 2>/dev/null
openssl req -new -config "$LEAF_CNF" -key "$SERVER_KEY" -out "$OUT_DIR/.server.csr" 2>/dev/null
openssl x509 -req \
    -in "$OUT_DIR/.server.csr" \
    -CA "$CA_CRT" -CAkey "$CA_KEY" -CAcreateserial \
    -out "$SERVER_CRT" \
    -days "$DAYS" \
    -extfile "$LEAF_CNF" -extensions v3_leaf 2>/dev/null

chmod 600 "$SERVER_KEY" "$CA_KEY" 2>/dev/null || true
rm -f "$OUT_DIR/.server.csr" "$CA_CNF" "$LEAF_CNF" "$OUT_DIR/.srl" "$CA_CRT.srl" 2>/dev/null || true

EXPIRY="$(openssl x509 -in "$SERVER_CRT" -noout -enddate 2>/dev/null | cut -d= -f2)"
{
    printf '\n'
    printf 'TLS certificate ready:\n'
    printf '  SLOC_TLS_CERT = %s\n' "$SERVER_CRT"
    printf '  SLOC_TLS_KEY  = %s\n' "$SERVER_KEY"
    printf '  CA (trust on clients) = %s\n' "$CA_CRT"
    printf '  Expires: %s\n' "$EXPIRY"
    printf '\n'
    printf 'Clients verify with:  curl --cacert %s https://<host>:<port>/healthz\n' "$CA_CRT"
    printf '\n'
} >&2

# stdout: the cert path, so a caller can `CERT=$(bash gen-tls-cert.sh ...)`.
printf '%s\n' "$SERVER_CRT"
