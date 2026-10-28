#!/usr/bin/env bash
# oxide-sloc LAN server launcher
#
# Starts oxide-sloc in server mode: binds to 0.0.0.0:4317 so every device on
# your local network can reach it.  Separate from run.sh (which defaults to
# localhost-only).
#
# Usage (the common case needs NO flags — just run it):
#   bash scripts/serve-server.sh                 # start LAN server (Windows: auto-opens
#                                                #   the firewall on first run; Linux:
#                                                #   detects the firewall and prints the
#                                                #   exact command / use --open-firewall)
#   bash scripts/serve-server.sh --port 8080
#   bash scripts/serve-server.sh --with-auth     # generate an API key; all endpoints require it
#   bash scripts/serve-server.sh --no-auth       # explicit no-auth (same as default)
#   bash scripts/serve-server.sh --quiet         # suppress startup banner (scripted launches)
#   bash scripts/serve-server.sh --open-firewall # force-(re)create the firewall rule now
#   SLOC_API_KEY=mysecret bash scripts/serve-server.sh  # use a pre-set key
#
# Windows firewall / LAN access — "just works" after a one-time setup:
#   Letting OTHER machines reach the server needs an inbound Windows Firewall rule,
#   and creating that rule requires admin ONCE (a Windows security boundary — there
#   is no zero-admin / per-launch way to open an inbound port). So on the FIRST run
#   this script installs a PERSISTENT all-profiles/all-subnets rule via a single UAC
#   prompt. Every launch after that finds the rule and runs with no flags, no admin,
#   and no UAC. Decline the prompt and it won't ask again; the server still serves
#   this machine, and --open-firewall retries later.
#
# What this does vs. run.sh --host:
#   - Dedicated entrypoint — purpose is obvious from the filename
#   - Defaults to no authentication for trusted-LAN use; use --with-auth or set
#     SLOC_API_KEY to require authentication
#   - Binds all interfaces (0.0.0.0) and prints one recommended LAN URL plus each
#     interface's subnet/gateway so multi-homed / VLAN / VPN hosts are unambiguous
#   - Windows only: auto-enables the inbound firewall rule on first run (one-time
#     admin prompt). On Linux the firewall is detected and left to you — the banner
#     prints the exact firewall-cmd/ufw/iptables command, or pass --open-firewall
#     to run it via sudo.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
SLOC_PORT=4317

# Detect Windows (Git Bash / MSYS2 / Cygwin)
if [[ -n "${WINDIR+x}" ]] || [[ "${OSTYPE:-}" == msys* ]] || [[ "${OSTYPE:-}" == cygwin* ]]; then
    PLATFORM=windows
    EXE="$REPO_ROOT/oxide-sloc.exe"
else
    PLATFORM=linux
    EXE="$REPO_ROOT/oxide-sloc"
fi

# Parse flags
OPEN_FIREWALL=false
# --no-auth is the default; accepted explicitly for UX. The real gate is UNDERSTAND_NO_AUTH.
NO_AUTH=false
WITH_AUTH=false
QUIET=false
UNDERSTAND_NO_AUTH=false
TLS=false            # --tls: serve HTTPS (drives the binary's SLOC_TLS_CERT/KEY support)
TLS_GENERATE=false   # --tls-generate: create a self-signed cert on the fly
MTLS_CA=""           # --mtls <ca.pem>: require client certs (SLOC_TLS_CLIENT_CA)
# NO_AUTH is assigned by --no-auth below but inert; the gate is UNDERSTAND_NO_AUTH (see note above).
# shellcheck disable=SC2034
for arg in "$@"; do
    case "$arg" in
        --open-firewall) OPEN_FIREWALL=true ;;
        --no-auth) NO_AUTH=true ;;        # explicit; also the default
        --with-auth) WITH_AUTH=true ;;
        --i-understand-no-auth) UNDERSTAND_NO_AUTH=true ;;
        --quiet) QUIET=true ;;
        --tls) TLS=true ;;
        --tls-generate) TLS=true; TLS_GENERATE=true ;;
        --mtls) TLS=true ;;   # next arg (CA path) consumed below
        --mtls=*) MTLS_CA="${arg#--mtls=}"; TLS=true ;;
        --port) ;;            # next arg consumed below
        --port=*) SLOC_PORT="${arg#--port=}" ;;
        --help|-h)
            printf 'Usage: bash scripts/serve-server.sh [options]\n\n'
            printf 'Options:\n'
            printf '  --port <n>              Bind on port n (default: 4317)\n'
            printf '  --with-auth             Generate a session API key (all endpoints require it)\n'
            printf '  --tls                   Serve HTTPS. Uses SLOC_TLS_CERT / SLOC_TLS_KEY if set;\n'
            printf '                          otherwise add --tls-generate.\n'
            printf '  --tls-generate          Create (and reuse) a self-signed cert with this host'\''s\n'
            printf '                          LAN SANs under ~/.oxide-sloc/tls/. Implies --tls.\n'
            printf '  --mtls <ca.pem>         Require clients to present a cert signed by <ca.pem>\n'
            printf '                          (mutual TLS). Implies --tls.\n'
            printf '  --no-auth               Run without authentication (see --i-understand-no-auth)\n'
            printf '  --i-understand-no-auth  Required to actually run --no-auth on a network (non-\n'
            printf '                          loopback) bind. An open, unauthenticated server on a\n'
            printf '                          shared LAN is a deliberate choice — this is the ack.\n'
            printf '  --open-firewall         Force-(re)create the LAN firewall rule now. Normally NOT\n'
            printf '                          needed: Windows auto-enables it on first run (one-time\n'
            printf '                          UAC). Linux: firewall-cmd (zone-aware) / ufw / iptables.\n'
            printf '  --quiet                 Suppress startup banner\n'
            printf '  --help                  Show this help\n\n'
            printf 'Environment:\n'
            printf '  SLOC_API_KEY=<key>       Use a pre-set API key instead of generating one\n'
            printf '  SLOC_ADVERTISE_HOST=<h>  Pin the host/IP shown to clients (skip auto-detect)\n'
            printf '  SLOC_ALLOW_UNAUTHENTICATED=1  Same effect as --i-understand-no-auth\n\n'
            printf 'Secure one-shot (TLS + auth + rate-limit + CORS): see deploy/corp.env.example\n'
            printf 'For persistent install (survives reboots): bash scripts/internal/install-systemd.sh\n'
            exit 0
            ;;
        *) ;;
    esac
done
_prev=""
for arg in "$@"; do
    [[ "$_prev" == "--port" ]] && SLOC_PORT="$arg"
    [[ "$_prev" == "--mtls" ]] && { MTLS_CA="$arg"; TLS=true; }
    _prev="$arg"
done

# ── API key setup ──────────────────────────────────────────────────────────────
# `serve --server` is FAIL-CLOSED: it refuses to start unless either an API key is
# set OR SLOC_ALLOW_UNAUTHENTICATED=1 is explicitly exported. This script binds a
# NETWORK (non-loopback) address, so running unauthenticated exposes an open server
# to the whole segment — that must be a deliberate choice, never a silent default.
# So the no-auth path requires an explicit acknowledgement (--i-understand-no-auth,
# or a pre-set SLOC_ALLOW_UNAUTHENTICATED=1); otherwise we refuse with guidance.
# When a key IS in use we unset any inherited SLOC_ALLOW_UNAUTHENTICATED so it can't
# quietly disable the auth middleware and make the key meaningless.
_no_auth_mode=false
if [[ -n "${SLOC_API_KEY:-}" || -n "${SLOC_API_KEYS:-}" ]]; then
    # A key is set in the environment (single SLOC_API_KEY or comma-list
    # SLOC_API_KEYS — the binary honors either, and so does the corp.env profile).
    # Honor it regardless of flags; export whichever was provided.
    [[ -n "${SLOC_API_KEY:-}" ]]  && export SLOC_API_KEY
    [[ -n "${SLOC_API_KEYS:-}" ]] && export SLOC_API_KEYS
    unset SLOC_ALLOW_UNAUTHENTICATED
    _key_generated=false
elif [[ "$WITH_AUTH" == true ]]; then
    # --with-auth: generate a fresh session key.
    if command -v openssl &>/dev/null; then
        SLOC_API_KEY="$(openssl rand -hex 32)"
    else
        # Fallback: timestamp-based pseudo-random (not cryptographic, but avoids no-key warning)
        SLOC_API_KEY="$(date +%s%N 2>/dev/null || date +%s)_$(od -A n -t x4 -N 8 /dev/urandom 2>/dev/null | tr -d ' \n' || echo "nokey")"
    fi
    export SLOC_API_KEY
    unset SLOC_ALLOW_UNAUTHENTICATED
    _key_generated=true
else
    # No key requested → no-auth mode. Require an explicit acknowledgement before
    # opening an unauthenticated server on a network address.
    unset SLOC_API_KEY
    _no_auth_mode=true
    _key_generated=false
    if [[ "$UNDERSTAND_NO_AUTH" == true || -n "${SLOC_ALLOW_UNAUTHENTICATED:-}" ]]; then
        export SLOC_ALLOW_UNAUTHENTICATED=1
    else
        printf '\n' >&2
        printf 'Refusing to start an UNAUTHENTICATED server on a network address (0.0.0.0:%s).\n' "$SLOC_PORT" >&2
        printf 'Anyone on this segment could reach and scan directories. Pick one:\n\n' >&2
        printf '  Recommended — enable auth:\n' >&2
        printf '    bash scripts/serve-server.sh --with-auth\n\n' >&2
        printf '  Secure profile (TLS + auth + rate-limit + CORS):\n' >&2
        printf '    cp deploy/corp.env.example deploy/corp.env   # edit, then:\n' >&2
        printf '    set -a; . deploy/corp.env; set +a; bash scripts/serve-server.sh\n\n' >&2
        printf '  I understand, run it open anyway (trusted/isolated LAN only):\n' >&2
        printf '    bash scripts/serve-server.sh --no-auth --i-understand-no-auth\n\n' >&2
        printf '  Localhost only (no acknowledgement needed):\n' >&2
        printf '    %s serve            # binds 127.0.0.1\n\n' "$(basename "$EXE")" >&2
        exit 1
    fi
fi

# ── TLS / HTTPS setup ────────────────────────────────────────────────────────
# The binary already terminates TLS with rustls (TLS 1.2/1.3, modern AEAD suites
# only) and does mutual TLS when SLOC_TLS_CLIENT_CA is set — all gated on the
# SLOC_TLS_CERT / SLOC_TLS_KEY env vars. This launcher just resolves a cert and
# exports those vars before launch. Default stays HTTP so trusted-LAN use is
# unchanged; HTTPS is opt-in via --tls / --tls-generate / --mtls.
SCHEME=http
TLS_CA_PATH=""   # path to the CA clients should trust (self-signed path only)

# Comma-separated routable IPs, fed to gen-tls-cert.sh as extra SANs so the
# generated cert verifies when reached by LAN IP (not just hostname/localhost).
get_lan_ips_csv() {
    classify_ips | awk -F '\t' '$1=="lan" || $1=="vpn" {print $3}' \
        | paste -sd, - 2>/dev/null || true
}

setup_tls() {
    # Honor the binary's fail-closed TLS gate (SLOC_REQUIRE_TLS / SLOC_HARDENED
    # refuse a cleartext non-loopback bind) up front with friendly guidance,
    # rather than letting the Rust layer bail with a raw error.
    if [[ "$TLS" != true && ( -n "${SLOC_REQUIRE_TLS:-}" || -n "${SLOC_HARDENED:-}" ) ]]; then
        printf '\n' >&2
        printf 'SLOC_REQUIRE_TLS / SLOC_HARDENED is set, but TLS is not enabled.\n' >&2
        printf 'A network-facing cleartext bind is refused in this mode. Enable HTTPS:\n\n' >&2
        printf '  bash scripts/serve-server.sh --with-auth --tls --tls-generate\n' >&2
        printf '  # or bring your own certificate:\n' >&2
        printf '  SLOC_TLS_CERT=/path/server.crt SLOC_TLS_KEY=/path/server.key \\\n' >&2
        printf '    bash scripts/serve-server.sh --with-auth --tls\n\n' >&2
        exit 1
    fi
    [[ "$TLS" != true ]] && return 0

    # Mutual TLS: require clients to present a cert chaining to the given CA.
    if [[ -n "$MTLS_CA" ]]; then
        if [[ ! -f "$MTLS_CA" ]]; then
            printf 'ERROR: --mtls CA file not found: %s\n' "$MTLS_CA" >&2
            exit 1
        fi
        export SLOC_TLS_CLIENT_CA="$MTLS_CA"
    fi

    if [[ -n "${SLOC_TLS_CERT:-}" && -n "${SLOC_TLS_KEY:-}" ]]; then
        # Bring-your-own certificate (e.g. from a corporate / public CA).
        if [[ ! -f "$SLOC_TLS_CERT" || ! -f "$SLOC_TLS_KEY" ]]; then
            printf 'ERROR: SLOC_TLS_CERT / SLOC_TLS_KEY is set but a file is missing:\n' >&2
            printf '  cert: %s\n  key:  %s\n' "$SLOC_TLS_CERT" "$SLOC_TLS_KEY" >&2
            exit 1
        fi
        export SLOC_TLS_CERT SLOC_TLS_KEY
    elif [[ "$TLS_GENERATE" == true ]]; then
        # Generate (or reuse a still-valid) self-signed cert with LAN SANs.
        local sans cert
        sans="$(get_lan_ips_csv)"
        cert="$(SLOC_TLS_SAN="$sans" bash "$SCRIPT_DIR/internal/gen-tls-cert.sh" | tail -1)"
        if [[ -z "$cert" || ! -f "$cert" ]]; then
            printf 'ERROR: could not generate a TLS certificate.\n' >&2
            printf '  Run it directly for details: bash scripts/internal/gen-tls-cert.sh\n' >&2
            exit 1
        fi
        export SLOC_TLS_CERT="$cert"
        export SLOC_TLS_KEY="${cert%/*}/server.key"
        TLS_CA_PATH="${cert%/*}/ca.crt"
    else
        printf '\n--tls needs a certificate. Either:\n' >&2
        printf '  - set SLOC_TLS_CERT and SLOC_TLS_KEY to your PEM files, or\n' >&2
        printf '  - add --tls-generate to create a self-signed cert automatically.\n\n' >&2
        exit 1
    fi
    SCHEME=https
    # A cleartext SLOC_REQUIRE_TLS gate is now satisfied; nothing else to do.
}

# ── Detect & classify local IPv4 addresses ─────────────────────────────────────
# Emits one TAB-separated record per address:
#     category <TAB> alias <TAB> ip <TAB> prefixlen <TAB> gateway
#   category ∈ { lan, vpn, virtual }
#     lan     = ordinary NIC on a routed subnet — what other hosts should use
#     vpn     = tunnel adapter — reachable ONLY by other clients on that same VPN
#     virtual = Hyper-V / Docker / WSL / VMware switch — not reachable off this box
#   prefixlen = subnet mask length (e.g. 24 for /24); "-" if unknown
#   gateway   = default-gateway on that interface, or "-" if none. An interface
#               WITH a gateway is the one routers/other-VLAN peers can reach.
# This is the whole point on a corporate network: a multi-homed box has several
# IPs across different subnets/VLANs/VPNs, and only some are actually routable to
# the peer that's trying to connect. We surface the subnet + gateway so you can
# match a local address to the network your colleague is actually on.
classify_ips() {
    if [[ "$PLATFORM" == linux ]]; then
        if command -v ip &>/dev/null; then
            # Build an iface→gateway map from the routing table, then walk addrs.
            local -A _gw=()
            local _if _nh
            while read -r _if _nh; do
                [[ -n "$_if" && -z "${_gw[$_if]:-}" ]] && _gw[$_if]="$_nh"
            done < <(ip -o -4 route show default 2>/dev/null | sed -n 's/.*dev \([^ ]*\).*via \([0-9.]*\).*/\1 \2/p;s/^default via \([0-9.]*\) dev \([^ ]*\).*/\2 \1/p')
            local _ln iface cidr addr plen
            while IFS= read -r _ln; do
                iface="$(awk '{print $2}' <<< "$_ln")"
                cidr="$(awk '{print $4}' <<< "$_ln")"
                addr="${cidr%/*}"; plen="${cidr#*/}"
                [[ -z "$addr" || "$addr" == 127.* ]] && continue
                local cat=lan
                if [[ "$iface" =~ ^(docker|br-|veth|virbr|cni|flannel|podman|vmnet|vboxnet) ]]; then cat=virtual; fi
                if [[ "$iface" =~ ^(tun|tap|wg|ppp|nordlynx) ]]; then cat=vpn; fi
                printf '%s\t%s\t%s\t%s\t%s\n' "$cat" "$iface" "$addr" "$plen" "${_gw[$iface]:--}"
            done < <(ip -o -4 addr show 2>/dev/null)
        else
            # No ip(8): fall back to a bare address list with unknown subnet/gw.
            local ip
            while IFS= read -r ip; do
                [[ -z "$ip" ]] && continue
                printf 'lan\t-\t%s\t-\t-\n' "$ip"
            done < <(hostname -I 2>/dev/null | tr ' ' '\n' | grep -v '^$' | grep -v '^127\.' | grep -v '^::')
        fi
    else
        # Windows: classify by adapter description/alias so VPN tunnels and Hyper-V
        # switches are labelled correctly regardless of which RFC-1918 range they use,
        # and attach the per-interface default gateway from the routing table.
        powershell -NoProfile -Command '
            $vpnPat  = "VPN|AnyConnect|GlobalProtect|PANGP|WireGuard|OpenVPN|TAP-|Tunnel|Pulse|Juniper|Zscaler|WAN Miniport|SoftEther|FortiClient|Sophos|Cisco Secure|Netskope"
            $virtPat = "Hyper-V|vEthernet|Default Switch|Docker|WSL|VirtualBox|VMware|Loopback"
            $gws = @{}
            Get-NetRoute -DestinationPrefix "0.0.0.0/0" -ErrorAction SilentlyContinue | ForEach-Object {
                if (-not $gws.ContainsKey([int]$_.InterfaceIndex)) { $gws[[int]$_.InterfaceIndex] = $_.NextHop }
            }
            Get-NetIPAddress -AddressFamily IPv4 |
              Where-Object { $_.IPAddress -notmatch "^127\." -and $_.IPAddress -notmatch "^169\.254\." } |
              ForEach-Object {
                $idx = [int]$_.InterfaceIndex
                $alias = $_.InterfaceAlias
                $ad = (Get-NetAdapter -InterfaceIndex $idx -ErrorAction SilentlyContinue).InterfaceDescription
                $cat = "lan"
                if ($alias -match $virtPat -or $ad -match $virtPat) { $cat = "virtual" }
                if ($alias -match $vpnPat  -or $ad -match $vpnPat)  { $cat = "vpn" }
                $gw = $gws[$idx]; if (-not $gw -or $gw -eq "0.0.0.0") { $gw = "-" }
                "$cat`t$alias`t$($_.IPAddress)`t$($_.PrefixLength)`t$gw"
              }
        ' 2>/dev/null | tr -d '\r' || true
    fi
}

# Best address for other hosts to use. Priority:
#   1. a physical-LAN address that both is on the default route AND has a gateway
#   2. any physical-LAN address that has a gateway (routable off-subnet)
#   3. any physical-LAN address
# Under a full-tunnel VPN the default route is the tunnel (which local-LAN peers
# cannot use), so we intentionally prefer a gateway-bearing lan address over it.
get_primary_ip() {
    local classified lan_gw lan_any route_ip
    classified="$(classify_ips)"
    lan_gw="$(awk -F '\t' '$1=="lan" && $5!="-" {print $3}' <<< "$classified")"
    lan_any="$(awk -F '\t' '$1=="lan" {print $3}' <<< "$classified")"
    if [[ "$PLATFORM" == linux ]]; then
        command -v ip &>/dev/null && \
            route_ip="$(ip route get 1.1.1.1 2>/dev/null | grep -oP 'src \K\S+' | head -1)"
    else
        route_ip="$(powershell -NoProfile -Command \
            '(Find-NetRoute -RemoteIPAddress 1.1.1.1 -ErrorAction SilentlyContinue | Select-Object -First 1).IPAddress' \
            2>/dev/null | tr -d '\r' | grep -v '^$' | head -1)"
    fi
    if [[ -n "${route_ip:-}" ]] && grep -qxF "$route_ip" <<< "$lan_gw" 2>/dev/null; then
        printf '%s\n' "$route_ip"
    elif [[ -n "$lan_gw" ]]; then
        head -1 <<< "$lan_gw"
    else
        head -1 <<< "$lan_any"
    fi
}

# ── Firewall preflight ─────────────────────────────────────────────────────────
FIREWALL_STATUS="unknown"
FIREWALL_FIX=""
WIN_FW_RULE_NAME=""   # set by check_firewall_windows

# Windows: is there an enabled inbound Allow rule covering our port?
# Sets FIREWALL_STATUS / FIREWALL_FIX. The active NIC on a corporate network is
# very often on the "Public" profile, which drops unsolicited inbound by default —
# so without an explicit rule LAN peers get a silent connection-refused/timeout.
check_firewall_windows() {
    WIN_FW_RULE_NAME="oxide-sloc-${SLOC_PORT}"
    local out state prof scope
    # Report three things: (1) is our inbound Allow rule present & enabled,
    # (2) the active network profile, and (3) the rule's RemoteAddress scope —
    # a rule scoped to "LocalSubnet" (what the Windows app-prompt often creates)
    # silently blocks peers on OTHER VLANs/subnets even though it looks "open".
    out="$(powershell -NoProfile -Command "
        \$name = '${WIN_FW_RULE_NAME}'
        \$rule = Get-NetFirewallRule -DisplayName \$name -ErrorAction SilentlyContinue |
            Where-Object { \$_.Enabled -eq 'True' -and \$_.Direction -eq 'Inbound' -and \$_.Action -eq 'Allow' } |
            Select-Object -First 1
        \$prof = (Get-NetConnectionProfile -ErrorAction SilentlyContinue |
            Sort-Object -Property @{Expression = { \$_.IPv4Connectivity -eq 'Internet' }} -Descending |
            Select-Object -First 1).NetworkCategory
        if (\$rule) {
            \$ra = (\$rule | Get-NetFirewallAddressFilter -ErrorAction SilentlyContinue).RemoteAddress
            \$scope = if (\$ra -eq 'Any' -or \$ra -eq '*' -or -not \$ra) { 'any' } else { 'scoped' }
            \"open|\$prof|\$scope\"
        } else { \"blocked|\$prof|-\" }
    " 2>/dev/null | tr -d '\r' | grep -v '^$' | head -1)"
    state="$(cut -d'|' -f1 <<< "$out")"
    prof="$(cut -d'|' -f2 <<< "$out")"
    scope="$(cut -d'|' -f3 <<< "$out")"
    [[ -z "$prof" ]] && prof="unknown"
    if [[ "$state" == "open" && "$scope" == "scoped" ]]; then
        # Rule exists but is subnet-scoped — the classic cross-VLAN failure.
        FIREWALL_STATUS="PARTIAL (rule '${WIN_FW_RULE_NAME}' is subnet-scoped; other VLANs/subnets blocked; active profile: ${prof})"
        FIREWALL_FIX="netsh advfirewall firewall set rule name=${WIN_FW_RULE_NAME} new remoteip=any"
    elif [[ "$state" == "open" ]]; then
        FIREWALL_STATUS="open (inbound rule '${WIN_FW_RULE_NAME}' present, all remote hosts; active profile: ${prof})"
        FIREWALL_FIX=""
    else
        FIREWALL_STATUS="BLOCKED (no inbound rule; active profile: ${prof})"
        FIREWALL_FIX="netsh advfirewall firewall add rule name=${WIN_FW_RULE_NAME} dir=in action=allow protocol=TCP localport=${SLOC_PORT} profile=any"
    fi
}

# Windows: (re)create the inbound Allow rule for ALL profiles (Domain/Private/
# Public) with remoteip=any, via a single UAC-elevated netsh call. We DELETE any
# existing same-named rule first so a stale subnet-scoped rule can't linger and
# keep blocking cross-VLAN peers. Creating firewall rules requires admin — hence
# the one UAC prompt. The delete+add run in one elevated shell so it's one prompt.
open_firewall_windows() {
    if [[ -z "$FIREWALL_FIX" ]]; then
        printf '  --open-firewall: port %s/tcp is already open to all remote hosts.\n' "$SLOC_PORT"
        return
    fi
    printf '  (Re)creating Windows Firewall inbound rule for TCP %s\n' "$SLOC_PORT"
    printf '    profiles: all (Domain/Private/Public), remote hosts: any (all subnets/VLANs).\n'
    printf '  A UAC prompt will appear — approve it to open the port for LAN hosts.\n'
    # cmd /c chains the delete (ignored if absent) and add in one elevated process.
    local cmdline
    cmdline="netsh advfirewall firewall delete rule name=${WIN_FW_RULE_NAME} >nul 2>&1 & netsh advfirewall firewall add rule name=${WIN_FW_RULE_NAME} dir=in action=allow protocol=TCP localport=${SLOC_PORT} profile=any remoteip=any"
    powershell -NoProfile -Command "
        Start-Process -FilePath 'cmd.exe' -Verb RunAs -Wait -ArgumentList '/c','${cmdline}'
    " 2>/dev/null || {
        printf '  Could not add the rule automatically. Run this once in an elevated CMD:\n'
        printf '    netsh advfirewall firewall add rule name=%s dir=in action=allow protocol=TCP localport=%s profile=any remoteip=any\n' \
            "$WIN_FW_RULE_NAME" "$SLOC_PORT"
    }
    FIREWALL_FIX=""
    check_firewall_windows
}

# One-time, per-user marker: set only if the user declined the firewall UAC, so we
# don't re-prompt on every launch. Lives in the user profile, never in the repo.
AUTO_LAN_MARKER="${HOME:-$REPO_ROOT}/.oxide-sloc/lan-setup-declined"

# Windows default path: make LAN access "just work" with no flags. On the FIRST
# launch (no rule yet) we do the one-time elevated setup — a single UAC prompt
# that installs a PERSISTENT all-profiles/all-subnets rule. Every launch after
# that finds the rule and runs completely silently: no flags, no admin, no UAC.
# If the user declines the prompt (or has no admin at all) we record that and
# never nag again — the server still runs for localhost/same-machine, and the
# banner explains how to enable LAN later.
ensure_lan_access_windows() {
    check_firewall_windows
    # Rule already present and unscoped → LAN access is on. Nothing to do, silent.
    [[ -z "$FIREWALL_FIX" ]] && return 0
    # Rule missing or subnet-scoped. Honor a previous decline unless the user
    # explicitly asked to (re)open it this time with --open-firewall.
    if [[ -f "$AUTO_LAN_MARKER" && "$OPEN_FIREWALL" != true ]]; then
        return 0
    fi
    if [[ "$OPEN_FIREWALL" != true ]]; then
        printf '  First run: enabling LAN access with a one-time Windows Firewall rule.\n'
        printf '  Approve the admin prompt once — you will not be asked again.\n'
    fi
    open_firewall_windows
    # Still blocked afterwards ⇒ the prompt was declined or admin is unavailable.
    # Remember it so we stop prompting; they can retry later with --open-firewall.
    if [[ -n "$FIREWALL_FIX" && "$OPEN_FIREWALL" != true ]]; then
        mkdir -p "$(dirname "$AUTO_LAN_MARKER")" 2>/dev/null || true
        : > "$AUTO_LAN_MARKER" 2>/dev/null || true
        printf '  LAN access left off (no rule created). The server still runs for this\n'
        printf '  machine; run "bash scripts/serve-server.sh --open-firewall" to try again.\n'
    elif [[ -z "$FIREWALL_FIX" ]]; then
        # Success — clear any stale decline marker so state is consistent.
        rm -f "$AUTO_LAN_MARKER" 2>/dev/null || true
    fi
}

# If --open-firewall is set and a blocking rule was detected, run the fix.
maybe_open_firewall() {
    [[ "$OPEN_FIREWALL" != true ]] && return
    if [[ "$PLATFORM" == windows ]]; then
        open_firewall_windows
        return
    fi
    if [[ -z "$FIREWALL_FIX" ]]; then
        if [[ "$FIREWALL_STATUS" == "no managed firewall detected" ]]; then
            printf '  --open-firewall: No supported firewall tool found (firewall-cmd, ufw, iptables).\n'
            printf '  Open port %s/tcp manually.\n' "$SLOC_PORT"
        fi
        return
    fi
    printf '  Opening port %s/tcp via sudo...\n' "$SLOC_PORT"
    if sudo -n true 2>/dev/null; then
        eval "$FIREWALL_FIX"
    else
        sudo bash -c "$FIREWALL_FIX"
    fi
    # Re-evaluate after the fix so the banner shows the updated status.
    FIREWALL_FIX=""
    check_firewall
}

# Firewall preflight dispatch used by both launch paths. Windows gets the
# auto-enable-once behavior (no flags needed); Linux keeps the sudo-based flow.
prepare_firewall() {
    if [[ "$PLATFORM" == windows ]]; then
        ensure_lan_access_windows
    else
        check_firewall
        maybe_open_firewall
    fi
}

check_firewall() {
    if [[ "$PLATFORM" == windows ]]; then
        check_firewall_windows
        return
    fi
    # Detect firewalld via reliable signals; avoid firewall-cmd --state which
    # exits 253 ("Authorization failed") for unprivileged users on RHEL/polkit.
    if command -v firewall-cmd &>/dev/null \
       && { systemctl is-active --quiet firewalld 2>/dev/null \
            || [ -S /run/firewalld/private ] \
            || pgrep -x firewalld >/dev/null 2>&1; }; then
        # Determine the zone that actually owns the primary-route interface, rather
        # than assuming the default zone. On a multi-zone host (e.g. enp6s18 in
        # "public" + a docker bridge in its own zone) the port must be opened in the
        # zone bound to the interface that carries LAN traffic, or the rule is inert.
        local fw_iface fw_zone
        fw_iface="$(ip route get 1.1.1.1 2>/dev/null | grep -oP 'dev \K\S+' | head -1)"
        if [[ -n "$fw_iface" ]]; then
            fw_zone="$(firewall-cmd --get-zone-of-interface="$fw_iface" 2>/dev/null \
                       || sudo -n firewall-cmd --get-zone-of-interface="$fw_iface" 2>/dev/null)"
        fi
        # Fall back to the default zone if the interface isn't explicitly bound.
        [[ -z "${fw_zone:-}" || "$fw_zone" == "no zone" ]] && \
            fw_zone="$(firewall-cmd --get-default-zone 2>/dev/null || echo public)"
        local fw_zflag="--zone=${fw_zone}"
        if firewall-cmd "$fw_zflag" --query-port="${SLOC_PORT}/tcp" &>/dev/null 2>&1 \
           || sudo -n firewall-cmd "$fw_zflag" --query-port="${SLOC_PORT}/tcp" &>/dev/null 2>&1; then
            FIREWALL_STATUS="open (firewalld, zone: ${fw_zone})"
        else
            FIREWALL_STATUS="BLOCKED (firewalld active; port not open in zone '${fw_zone}'${fw_iface:+ on ${fw_iface}})"
            FIREWALL_FIX="sudo firewall-cmd ${fw_zflag} --add-port=${SLOC_PORT}/tcp --permanent && sudo firewall-cmd --reload"
        fi
    elif command -v ufw &>/dev/null && ufw status 2>/dev/null | grep -q "Status: active"; then
        if ufw status | grep -qE "^${SLOC_PORT}(/tcp)?[[:space:]]+ALLOW"; then
            FIREWALL_STATUS="open (ufw)"
        else
            FIREWALL_STATUS="BLOCKED (ufw active, port not permitted)"
            FIREWALL_FIX="sudo ufw allow ${SLOC_PORT}/tcp"
        fi
    elif command -v iptables &>/dev/null; then
        if iptables -C INPUT -p tcp --dport "${SLOC_PORT}" -j ACCEPT &>/dev/null 2>&1 \
           || sudo -n iptables -C INPUT -p tcp --dport "${SLOC_PORT}" -j ACCEPT &>/dev/null 2>&1; then
            FIREWALL_STATUS="open (iptables)"
        else
            FIREWALL_STATUS="iptables present — port ${SLOC_PORT}/tcp not explicitly allowed"
            FIREWALL_FIX="sudo iptables -A INPUT -p tcp --dport ${SLOC_PORT} -j ACCEPT"
        fi
    else
        FIREWALL_STATUS="no managed firewall detected"
    fi
}

# ── Kill any process already holding the port ──────────────────────────────────
free_port() {
    if [[ "$PLATFORM" == windows ]]; then
        powershell -NoProfile -Command "
            Get-Process -Name 'oxide-sloc' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
            \$conn = Get-NetTCPConnection -LocalPort $SLOC_PORT -ErrorAction SilentlyContinue
            if (\$conn) {
                \$conn | Select-Object -ExpandProperty OwningProcess | Sort-Object -Unique |
                    ForEach-Object { Stop-Process -Id \$_ -Force -ErrorAction SilentlyContinue }
            }
        " 2>/dev/null || true
        local tries=0
        while netstat -ano 2>/dev/null | grep -qE ":${SLOC_PORT}[[:space:]].*LISTENING"; do
            (( tries++ )) && (( tries >= 8 )) && break
            sleep 0.25
        done
    else
        pkill -x oxide-sloc 2>/dev/null || true
        command -v fuser &>/dev/null && fuser -k "${SLOC_PORT}/tcp" 2>/dev/null || true
        sleep 0.3
    fi
}

# Abort if the port is still in use after free_port — e.g. owned by another user.
assert_port_free() {
    [[ "$PLATFORM" == windows ]] && return
    command -v ss &>/dev/null || return
    if ss -tln 2>/dev/null | grep -qE ":${SLOC_PORT}\b"; then
        printf '\nERROR: could not free port %s; another oxide-sloc may be running as a different user.\n' "$SLOC_PORT" >&2
        printf '  Check: ss -tlnp | grep %s\n\n' "$SLOC_PORT" >&2
        exit 1
    fi
}

# ── Print startup banner ───────────────────────────────────────────────────────
print_banner() {
    local classified primary_ip lan_recs vpn_recs virt_recs first_ip
    classified="$(classify_ips)"
    primary_ip="$(get_primary_ip)"
    # Full 5-field records per category (category<TAB>alias<TAB>ip<TAB>plen<TAB>gw)
    lan_recs="$(awk -F '\t' '$1=="lan"'     <<< "$classified")"
    vpn_recs="$(awk -F '\t' '$1=="vpn"'     <<< "$classified")"
    virt_recs="$(awk -F '\t' '$1=="virtual"' <<< "$classified")"

    # Best single IP for the browser/curl example. An explicit SLOC_ADVERTISE_HOST
    # (hostname or IP) always wins — operators behind NAT/DNS or with a preferred
    # management address can pin exactly what clients should use. Otherwise:
    # recommended LAN IP > first LAN IP > first VPN IP (last resort).
    local advertise="${SLOC_ADVERTISE_HOST:-}"
    if [[ -n "$advertise" ]]; then
        first_ip="$advertise"
    elif [[ -n "$primary_ip" ]]; then
        first_ip="$primary_ip"
    elif [[ -n "$lan_recs" ]]; then
        first_ip="$(awk -F '\t' 'NR==1{print $3}' <<< "$lan_recs")"
    elif [[ -n "$vpn_recs" ]]; then
        first_ip="$(awk -F '\t' 'NR==1{print $3}' <<< "$vpn_recs")"
    else
        first_ip=""
    fi

    printf '\n'
    printf '  ╔══════════════════════════════════════════════════╗\n'
    printf '  ║         OxideSLOC — LAN Server Mode              ║\n'
    printf '  ╚══════════════════════════════════════════════════╝\n'
    printf '\n'

    if [[ "$_no_auth_mode" == true ]]; then
        printf '  *** WARNING: running WITHOUT authentication — all endpoints are open ***\n'
        printf '  Anyone on this network can access and scan directories.\n'
        printf '  To require authentication, restart with --with-auth or set SLOC_API_KEY.\n'
    elif [[ "$_key_generated" == true ]]; then
        printf '  API key (generated for this session):\n'
        printf '    %s\n' "$SLOC_API_KEY"
        printf '\n'
        printf '  To set a persistent key:\n'
        printf '    export SLOC_API_KEY=%s\n' "$SLOC_API_KEY"
        printf '    bash scripts/serve-server.sh\n'
    else
        printf '  API key: set (from environment)\n'
    fi
    printf '\n'

    # Bound on 0.0.0.0 → the server answers on EVERY address below. Which one a
    # given client can reach is a routing question, so we spell out the subnet and
    # gateway of each and recommend the single most-likely-routable one.
    printf '  Listening on 0.0.0.0:%s (all interfaces).\n\n' "$SLOC_PORT"

    if [[ -n "$first_ip" ]]; then
        printf '  ➜ RECOMMENDED URL for other machines:\n'
        printf '      %s://%s:%s/' "$SCHEME" "$first_ip" "$SLOC_PORT"
        [[ -n "$advertise" ]] && printf '   (pinned via SLOC_ADVERTISE_HOST)'
        printf '\n\n'
    fi

    printf '  Local (this machine only) → %s://127.0.0.1:%s\n' "$SCHEME" "$SLOC_PORT"

    # Physical-LAN addresses, each annotated with subnet + gateway so you can match
    # one to the network your peer is actually on (different VLANs = different subnets).
    if [[ -n "$lan_recs" ]]; then
        printf '  LAN / routed interfaces (give these to other hosts):\n'
        while IFS=$'\t' read -r cat alias ip plen gw; do
            [[ -z "$ip" ]] && continue
            local tag="" sub=""
            [[ "$ip" == "$primary_ip" ]] && tag="  ← recommended"
            [[ "$plen" != "-" ]] && sub="/$plen"
            if [[ "$gw" != "-" ]]; then
                printf '    %s://%s:%s   [%s%s via %s → gw %s]%s\n' \
                    "$SCHEME" "$ip" "$SLOC_PORT" "$alias" "$sub" "$ip" "$gw" "$tag"
            else
                printf '    %s://%s:%s   [%s%s — no gateway: same-subnet peers only]%s\n' \
                    "$SCHEME" "$ip" "$SLOC_PORT" "$alias" "$sub" "$tag"
            fi
        done <<< "$lan_recs"
    else
        printf '  LAN → (no routed LAN address detected — run "ipconfig" / "ip -4 addr")\n'
    fi

    # VPN tunnels: reachable ONLY by other clients on the same VPN.
    if [[ -n "$vpn_recs" ]]; then
        printf '  VPN tunnels (reachable ONLY by other clients on the SAME VPN, not the office LAN):\n'
        while IFS=$'\t' read -r cat alias ip plen gw; do
            [[ -z "$ip" ]] && continue
            printf '    %s://%s:%s   [%s]\n' "$SCHEME" "$ip" "$SLOC_PORT" "$alias"
        done <<< "$vpn_recs"
    fi
    # Hyper-V / Docker / WSL switches: never reachable from another machine.
    if [[ -n "$virt_recs" ]]; then
        printf '  Virtual switches (Hyper-V/Docker/WSL — NOT reachable from any other machine, ignore):\n'
        while IFS=$'\t' read -r cat alias ip plen gw; do
            [[ -z "$ip" ]] && continue
            printf '    %s://%s:%s   [%s]\n' "$SCHEME" "$ip" "$SLOC_PORT" "$alias"
        done <<< "$virt_recs"
    fi
    printf '\n'

    if [[ -n "$first_ip" ]]; then
        printf '  Open in a browser from another machine:\n'
        printf '    %s://%s:%s/\n' "$SCHEME" "$first_ip" "$SLOC_PORT"
        printf '\n'
        if [[ -n "${SLOC_API_KEY:-}" ]]; then
            printf '  API key is set — open the login page to sign in:\n'
            printf '    %s://%s:%s/auth/login\n' "$SCHEME" "$first_ip" "$SLOC_PORT"
            printf '\n'
            printf '  Auth lockout: %d failed attempts from the same IP triggers a\n' "${SLOC_AUTH_LOCKOUT_FAILS:-10}"
            printf '  temporary lockout (SLOC_AUTH_LOCKOUT_SECS=%s). Restart the server\n' "${SLOC_AUTH_LOCKOUT_SECS:-3600}"
            printf '  to clear it immediately.\n'
            printf '\n'
        fi
        printf '  CLI test:\n'
        local _curl_ca=""
        [[ "$SCHEME" == https && -n "$TLS_CA_PATH" ]] && _curl_ca="--cacert $TLS_CA_PATH "
        if [[ -n "${SLOC_API_KEY:-}" ]]; then
            printf '    curl %s-H "Authorization: Bearer %s" %s://%s:%s/healthz\n' \
                   "$_curl_ca" "$SLOC_API_KEY" "$SCHEME" "$first_ip" "$SLOC_PORT"
        else
            printf '    curl %s%s://%s:%s/healthz\n' "$_curl_ca" "$SCHEME" "$first_ip" "$SLOC_PORT"
        fi
    fi
    printf '\n'

    # TLS posture: when HTTPS is active, state the cert source and how clients trust it.
    if [[ "$SCHEME" == https ]]; then
        printf '  TLS: HTTPS enabled (rustls, TLS 1.2/1.3).\n'
        if [[ -n "${SLOC_TLS_CLIENT_CA:-}" ]]; then
            printf '    Mutual TLS: clients must present a cert signed by %s\n' "$SLOC_TLS_CLIENT_CA"
        fi
        if [[ -n "$TLS_CA_PATH" ]]; then
            printf '    Self-signed cert. Clients must trust the CA to avoid warnings:\n'
            printf '      %s\n' "$TLS_CA_PATH"
            printf '    (import it, or pass "curl --cacert %s ..."). Browsers will warn\n' "$TLS_CA_PATH"
            printf '    until the CA is imported into their trust store.\n'
        else
            printf '    Using the provided SLOC_TLS_CERT / SLOC_TLS_KEY.\n'
        fi
        printf '\n'
    fi

    if [[ "$PLATFORM" == linux ]]; then
        printf '  Firewall: %s\n' "$FIREWALL_STATUS"
        if [[ -n "$FIREWALL_FIX" ]]; then
            printf '    Other LAN hosts cannot reach this server until you run:\n'
            printf '      %s\n' "$FIREWALL_FIX"
            printf '    (or re-run this script with --open-firewall)\n'
        fi
        printf '\n'
    elif [[ "$PLATFORM" == windows ]]; then
        printf '  Firewall: %s\n' "$FIREWALL_STATUS"
        if [[ -n "$FIREWALL_FIX" ]]; then
            if [[ "$FIREWALL_STATUS" == PARTIAL* ]]; then
                printf '    The rule exists but is subnet-scoped, so hosts on OTHER VLANs/subnets\n'
                printf '    are still blocked. Widen it to all remote hosts:\n'
            else
                printf '    LAN hosts are BLOCKED until the port is opened. The one-time Windows\n'
                printf '    "Allow access" dialog only opens Private/Domain networks; a corporate\n'
                printf '    NIC is usually on the Public profile, so you must add an explicit rule:\n'
            fi
            printf '      \xe2\x80\xa2 re-run this script with --open-firewall   (prompts for admin via UAC), or\n'
            printf '      \xe2\x80\xa2 run this once in an elevated PowerShell / CMD:\n'
            printf '          %s\n' "$FIREWALL_FIX"
        fi
        printf '\n'
    fi

    # Corporate-network / VPN reality check — the things that still break LAN
    # reachability after the server is bound to 0.0.0.0 and the port is opened.
    if [[ -n "$vpn_recs" || -n "$FIREWALL_FIX" || "$FIREWALL_STATUS" == BLOCKED* || "$FIREWALL_STATUS" == PARTIAL* ]]; then
        printf '  Corporate network / VPN notes:\n'
        printf '    - Hand colleagues the RECOMMENDED / gateway-bearing LAN address above,\n'
        printf '      NOT a VPN-tunnel or virtual-switch address.\n'
        printf '    - Different VLAN/subnet than your peer? Traffic must cross a router; the\n'
        printf '      host firewall rule allows ALL remote hosts (not just LocalSubnet), which\n'
        printf '      the auto-setup already enforces. Inter-VLAN ACLs are still up to IT.\n'
        printf '    - Client-isolation / "AP isolation" on corporate Wi-Fi and guest VLANs\n'
        printf '      blocks host-to-host traffic even with the firewall open. Test from a\n'
        printf '      peer on the same wired subnet first.\n'
        printf '    - Full-tunnel VPN clients often block inbound on the physical NIC. Reach\n'
        printf '      the server via its VPN address, or split-tunnel the LAN subnet.\n'
        printf '    - Quick reachability test from a peer: curl -v %s://%s:%s/healthz\n' \
            "$SCHEME" "${first_ip:-<lan-ip>}" "$SLOC_PORT"
        printf '\n'
    fi

    printf '  Press Ctrl+C to stop.\n\n'
}

LOG_DIR="$REPO_ROOT/logs"
mkdir -p "$LOG_DIR"

print_log_link() {
    local f="$1"
    local url
    if [[ "$PLATFORM" == windows ]]; then
        local wf
        wf="$(cygpath -w "$f" 2>/dev/null)" || wf="$f"
        wf="${wf//\\/\/}"
        url="file:///$wf"
    else
        url="file://$f"
    fi
    printf '  Log dir:  %s\n' "$LOG_DIR"
    printf '  Log file: %s\n' "$(basename "$f")"
    printf '  \033]8;;%s\033\\Click to open log\033]8;;\033\\\n' "$url"
}

# ── Launch ─────────────────────────────────────────────────────────────────────
do_launch_binary() {
    local bin="$1"
    free_port
    assert_port_free
    [[ "$PLATFORM" == linux ]] && chmod +x "$bin"
    prepare_firewall
    [[ "$QUIET" != true ]] && print_banner
    cd "$REPO_ROOT"
    export OXIDE_SLOC_ROOT="$REPO_ROOT"
    # Force an all-interfaces bind. --bind wins over SLOC_BIND and the config file,
    # so a stale bind_address pinned to one subnet's IP can't quietly limit the
    # server to a single VLAN/interface and lock out every other network.
    "$bin" serve --server --bind "0.0.0.0:${SLOC_PORT}"
}

do_launch_cargo() {
    free_port
    assert_port_free
    prepare_firewall

    local bin="$REPO_ROOT/target/debug/oxide-sloc"
    [[ "$PLATFORM" == windows ]] && bin="${bin}.exe"

    local tmpout
    tmpout="$(mktemp)"
    local LOG_FILE
    LOG_FILE="$LOG_DIR/serve-server-$(date +%Y-%m-%d-%H-%M-%S).log"

    local total_pkgs=0
    if [[ -f "$REPO_ROOT/Cargo.lock" ]]; then
        total_pkgs="$(grep -c '^\[\[package\]\]' "$REPO_ROOT/Cargo.lock" 2>/dev/null)" || total_pkgs=0
    fi

    printf '\n  oxide-sloc \xe2\x80\x94 building from source\n\n'
    cd "$REPO_ROOT"
    export OXIDE_SLOC_ROOT="$REPO_ROOT"

    cargo build -p oxide-sloc 2>"$tmpout" &
    local cargo_pid=$!

    trap 'kill "$cargo_pid" 2>/dev/null; rm -f "$tmpout"; [[ "$is_tty" -eq 1 ]] && printf "\033[?25h\n"; exit 130' INT TERM

    local spin=('|' '/' '-' '\')
    local si=0 compiled=0 last_crate="" bar_w=34
    local start_time="$SECONDS"
    local is_linking=0 first_draw=1
    local NLINES=9
    local resolution_msgs=(
        "Reading Cargo.lock..."
        "Verifying workspace sources..."
        "Resolving dependency graph..."
        "Checking workspace manifests..."
        "Scanning crate metadata..."
        "Preparing build graph..."
    )
    local msg_idx=0 frame_count=0 msg_interval=17

    # Suppress cursor-escape TUI when stdout is not a terminal (CI capture, tee, harness).
    local is_tty=0; [[ -t 1 ]] && is_tty=1
    local last_plain_ts="$SECONDS"

    [[ "$is_tty" -eq 1 ]] && printf '\033[?25l'

    while kill -0 "$cargo_pid" 2>/dev/null; do
        compiled="$(grep -c "^   Compiling" "$tmpout" 2>/dev/null)" || compiled=0
        if [[ "$compiled" -gt 0 ]]; then
            last_crate="$(grep "^   Compiling" "$tmpout" 2>/dev/null | tail -1 | \
                sed 's/^   Compiling //' | cut -c1-46)" || last_crate=""
        fi
        grep -q "Linking " "$tmpout" 2>/dev/null && is_linking=1 || true

        local elapsed=$(( SECONDS - start_time ))
        local elapsed_str
        printf -v elapsed_str '%d:%02d' "$(( elapsed / 60 ))" "$(( elapsed % 60 ))"

        local frame="${spin[$si]}"
        si=$(( (si + 1) % 4 ))
        frame_count=$(( frame_count + 1 ))

        local phase step_n
        if   [[ "$is_linking" -eq 1 ]]; then phase="Linking & launching";    step_n=3
        elif [[ "$compiled"   -gt 0 ]]; then phase="Compiling workspace";    step_n=2
        else                                 phase="Resolving dependencies";  step_n=1
        fi

        local bar="" fill=0 i
        if [[ "$total_pkgs" -gt 0 && "$compiled" -gt 0 ]]; then
            fill=$(( compiled * bar_w / total_pkgs ))
            [[ $fill -gt $bar_w ]] && fill=$bar_w
        fi
        for (( i = 0; i < bar_w; i++ )); do
            if   [[ $i -lt $fill ]]; then
                bar+='='
            elif [[ $i -eq $fill && $compiled -gt 0 && $fill -lt $bar_w ]]; then
                bar+='>'
            else
                bar+=' '
            fi
        done

        local count_str pct_str=""
        if [[ "$total_pkgs" -gt 0 ]]; then
            count_str="${compiled} / ~${total_pkgs}"
            [[ "$compiled" -gt 0 ]] && pct_str="($(( compiled * 100 / total_pkgs ))%)"
        else
            count_str="${compiled} crates"
        fi

        local ic_res='\xe2\x97\x8b' ic_cmp='\xe2\x97\x8b' ic_lnk='\xe2\x97\x8b'
        if   [[ "$is_linking" -eq 1 ]]; then
            ic_res='\xe2\x9c\x93'; ic_cmp='\xe2\x9c\x93'; ic_lnk="$frame"
        elif [[ "$compiled"   -gt 0 ]]; then
            ic_res='\xe2\x9c\x93'; ic_cmp="$frame"
        else
            ic_res="$frame"
        fi

        if [[ "$is_tty" -eq 1 ]]; then
            [[ "$first_draw" -eq 0 ]] && printf "\033[%dA" "$NLINES"
            first_draw=0

            printf "  %s  Step [%d/3]  %-46s\033[K\n"  "$frame"  "$step_n"  "$phase"
            printf "     [%s] %-16s%s\033[K\n"          "$bar"    "$count_str"  "$pct_str"
            local current_display
            if [[ "$compiled" -eq 0 ]]; then
                (( frame_count % msg_interval == 0 )) && msg_idx=$(( (msg_idx + 1) % ${#resolution_msgs[@]} ))
                current_display="${resolution_msgs[$msg_idx]}"
            else
                current_display="$last_crate"
            fi
            printf "     Current:  %-50s\033[K\n"       "$current_display"
            printf "     Elapsed:  %-50s\033[K\n"       "$elapsed_str"
            printf "     \033[K\n"
            printf "     Milestones:\033[K\n"
            printf "     %b  Resolve dependencies\033[K\n"  "$ic_res"
            printf "     %b  Compile workspace\033[K\n"     "$ic_cmp"
            printf "     %b  Launch server\033[K\n"         "$ic_lnk"

            sleep 0.12
        else
            # Non-TTY: print one plain progress line every ~10 s to keep the log alive.
            if (( SECONDS - last_plain_ts >= 10 )); then
                printf '  [build] Step [%d/3] %s — %s crates — %s\n' \
                    "$step_n" "$phase" "$compiled" "$elapsed_str"
                last_plain_ts="$SECONDS"
            fi
            sleep 1
        fi
    done

    wait "$cargo_pid"
    local exit_code=$?
    [[ "$is_tty" -eq 1 ]] && printf '\033[?25h'
    trap - INT TERM

    compiled="$(grep -c "^   Compiling" "$tmpout" 2>/dev/null)" || compiled=0

    {
        printf '# oxide-sloc serve-server log\n# Date:     %s\n# Platform: %s\n# Exit:     %d\n#\n' \
            "$(date)" "$PLATFORM" "$exit_code"
        cat "$tmpout"
    } > "$LOG_FILE"

    [[ "$is_tty" -eq 1 && "$first_draw" -eq 0 ]] && printf "\033[%dA" "$NLINES"

    if [[ $exit_code -ne 0 ]]; then
        if [[ "$is_tty" -eq 1 ]]; then
            printf "  \xe2\x9c\x97  BUILD FAILED%-67s\n" ""
            for (( i = 1; i < NLINES; i++ )); do printf "\033[K\n"; done
            printf '\n'
        fi
        cat "$tmpout" >&2
        rm -f "$tmpout"
        printf '\n  Build failed. Full output saved:\n'
        print_log_link "$LOG_FILE"
        printf '\n'
        exit 1
    fi

    rm -f "$tmpout"

    local final_elapsed=$(( SECONDS - start_time ))
    local final_elapsed_str
    printf -v final_elapsed_str '%d:%02d' "$(( final_elapsed / 60 ))" "$(( final_elapsed % 60 ))"

    local final_bar=""
    for (( i = 0; i < bar_w; i++ )); do final_bar+='='; done

    local final_count_str final_pct_str=""
    if [[ "$compiled" -gt 0 ]]; then
        final_count_str="${compiled} crates"
        final_pct_str="(100%)"
    fi

    if [[ "$is_tty" -eq 1 ]]; then
        if [[ "$compiled" -gt 0 ]]; then
            printf "  \xe2\x9c\x93  Step [3/3]  Build complete \xe2\x80\x94 %d crates compiled.%-13s\033[K\n" "$compiled" ""
            printf "     [%s] %-16s%s\033[K\n"    "$final_bar" "$final_count_str" "$final_pct_str"
            printf "     Current:  %-50s\033[K\n" "Done."
        else
            printf "  \xe2\x9c\x93  Step [3/3]  Already up to date.%-31s\033[K\n" ""
            printf "     [%s]\033[K\n"             "$final_bar"
            printf "     Current:  %-50s\033[K\n" "No changes detected."
        fi
        printf "     Elapsed:  %-50s\033[K\n" "$final_elapsed_str"
        printf "     \033[K\n"
        printf "     Milestones:\033[K\n"
        printf "     \xe2\x9c\x93  Resolve dependencies\033[K\n"
        printf "     \xe2\x9c\x93  Compile workspace\033[K\n"
        printf "     \xe2\x9c\x93  Launch server\033[K\n"
    else
        if [[ "$compiled" -gt 0 ]]; then
            printf '  [build] Done — %d crates compiled in %s\n' "$compiled" "$final_elapsed_str"
        else
            printf '  [build] Already up to date (%s)\n' "$final_elapsed_str"
        fi
    fi
    printf '\n  Log \xe2\x86\x92 %s\n' "$LOG_FILE"

    [[ "$QUIET" != true ]] && print_banner
    # Force an all-interfaces bind (see do_launch_binary for the rationale).
    "$bin" serve --server --bind "0.0.0.0:${SLOC_PORT}"
}

# Resolve TLS (export SLOC_TLS_CERT/KEY, or fail fast on a cert/enforcement error)
# before touching the port, firewall, or binary — so a misconfigured HTTPS request
# fails cleanly and with no side effects regardless of which launch path runs.
setup_tls

if [[ -f "$EXE" ]]; then
    do_launch_binary "$EXE"
    exit 0
fi

if command -v cargo &>/dev/null && [[ -f "$REPO_ROOT/Cargo.toml" ]]; then
    do_launch_cargo
    exit 0
fi

printf '\noxide-sloc: no binary found.\n\n' >&2
printf '  Run the installer first: bash scripts/run.sh\n' >&2
printf '  See docs/airgap.md for all deployment paths (no internet required).\n\n' >&2
exit 1
