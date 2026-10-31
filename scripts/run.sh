#!/usr/bin/env bash
# oxide-sloc launcher
#
# Usage:
#   bash scripts/run.sh              # localhost only  (http://127.0.0.1:4317)
#   bash scripts/run.sh --host       # LAN server mode (http://0.0.0.0:4317)
#   bash scripts/run.sh --lan        # alias for --host
#   bash scripts/run.sh --build      # compile from bundled toolchain + vendor sources
#   bash scripts/run.sh --rebuild    # fast debug rebuild from source (dev GUI loop)
#   bash scripts/run.sh --rebuild --release  # production-identical release rebuild
#   SLOC_HOST=1 bash scripts/run.sh  # env-var form of --host
#
# Run a one-shot CLI command instead of the web UI by passing a subcommand:
#   bash scripts/run.sh analyze ./my-repo --per-file --html-out out/report.html
#   bash scripts/run.sh report result.json --pdf-out result.pdf
#   bash scripts/run.sh --help       # full CLI usage
# Any known subcommand (analyze, report, diff, git-scan, watch, prune, ...) is
# forwarded to the binary; the server is not started.
#
# For a dedicated LAN-server launcher with API-key setup and IP guidance see:
#   bash scripts/serve-server.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
SLOC_PORT=4317

HOST_MODE="${SLOC_HOST:-0}"
REBUILD_MODE=false
BUILD_MODE=false
RELEASE_REBUILD=false

# Detect Windows (Git Bash / MSYS2 / Cygwin)
if [[ -n "${WINDIR+x}" ]] || [[ "${OSTYPE:-}" == msys* ]] || [[ "${OSTYPE:-}" == cygwin* ]]; then
    PLATFORM=windows
    EXE="$REPO_ROOT/oxide-sloc.exe"
else
    PLATFORM=linux
    EXE="$REPO_ROOT/oxide-sloc"
fi

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

# Kill any process currently holding SLOC_PORT so re-launches never fail with
# "address already in use".  On Windows this covers both clean exits and
# hard-killed processes that leave a zombie socket.
free_port() {
    if [[ "$PLATFORM" == windows ]]; then
        powershell -NoProfile -Command "
            # Kill by process name first (fastest path)
            Get-Process -Name 'oxide-sloc' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
            # Also kill whatever process owns the TCP port
            \$conn = Get-NetTCPConnection -LocalPort $SLOC_PORT -ErrorAction SilentlyContinue
            if (\$conn) {
                \$conn | Select-Object -ExpandProperty OwningProcess | Sort-Object -Unique |
                    ForEach-Object { Stop-Process -Id \$_ -Force -ErrorAction SilentlyContinue }
            }
        " 2>/dev/null || true
        # Give Windows up to 2 s to release the socket
        local tries=0
        while netstat -ano 2>/dev/null | grep -qE ":${SLOC_PORT}[[:space:]].*LISTENING"; do
            (( tries++ )) && (( tries >= 8 )) && break
            sleep 0.25
        done
    else
        pkill -x oxide-sloc 2>/dev/null || true
        command -v fuser &>/dev/null && fuser -k "${SLOC_PORT}/tcp" 2>/dev/null || true
        # Brief wait for the kernel to release the socket
        sleep 0.3
    fi
}

# Print the local LAN IP if available (best-effort; no failure on error).
# On Linux, prefers the default-route interface over docker bridge addresses.
print_lan_ip() {
    local ip=""
    if [[ "$PLATFORM" == linux ]]; then
        # Try the address on the default route first (avoids picking a docker bridge)
        if command -v ip &>/dev/null; then
            ip="$(ip route get 1.1.1.1 2>/dev/null | grep -oP 'src \K\S+' | head -1)" || true
        fi
        if [[ -z "$ip" ]]; then
            ip="$(hostname -I 2>/dev/null | tr ' ' '\n' | \
                grep -vE '^(127\.|172\.(1[6-9]|2[0-9]|3[01])\.|10\.(88|244)\.|169\.254\.|$)' | \
                head -1)" || true
        fi
    else
        ip="$(powershell -NoProfile -Command \
            "(Get-NetIPAddress -AddressFamily IPv4 | Where-Object { \$_.IPAddress -notmatch '^127\.' -and \$_.IPAddress -notmatch '^169\.254\.' } | Select-Object -First 1).IPAddress" \
            2>/dev/null)" || true
    fi
    [[ -n "$ip" ]] && printf '  LAN address \xe2\x86\x92 http://%s:%s\n' "$ip" "$SLOC_PORT"
}

# ── Firewall preflight (Linux only) ───────────────────────────────────────────
FIREWALL_STATUS="unknown"
FIREWALL_FIX=""

check_firewall() {
    [[ "$PLATFORM" != linux ]] && return
    # Detect firewalld via reliable signals; avoid firewall-cmd --state which
    # exits 253 ("Authorization failed") for unprivileged users on RHEL/polkit.
    if command -v firewall-cmd &>/dev/null \
       && { systemctl is-active --quiet firewalld 2>/dev/null \
            || [ -S /run/firewalld/private ] \
            || pgrep -x firewalld >/dev/null 2>&1; }; then
        if firewall-cmd --query-port="${SLOC_PORT}/tcp" &>/dev/null 2>&1; then
            FIREWALL_STATUS="open (firewalld)"
        elif sudo -n firewall-cmd --query-port="${SLOC_PORT}/tcp" &>/dev/null 2>&1; then
            FIREWALL_STATUS="open (firewalld)"
        else
            FIREWALL_STATUS="firewalld active — port status unknown (needs root to query)"
            FIREWALL_FIX="sudo firewall-cmd --add-port=${SLOC_PORT}/tcp --permanent && sudo firewall-cmd --reload"
        fi
    elif command -v ufw &>/dev/null && ufw status 2>/dev/null | grep -q "Status: active"; then
        if ufw status | grep -qE "^${SLOC_PORT}(/tcp)?[[:space:]]+ALLOW"; then
            FIREWALL_STATUS="open (ufw)"
        else
            FIREWALL_STATUS="BLOCKED (ufw active, port not permitted)"
            FIREWALL_FIX="sudo ufw allow ${SLOC_PORT}/tcp"
        fi
    else
        FIREWALL_STATUS="no managed firewall detected"
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

launch() {
    free_port
    assert_port_free
    [[ "$PLATFORM" == linux ]] && chmod +x "$1"
    cd "$REPO_ROOT"
    export OXIDE_SLOC_ROOT="$REPO_ROOT"
    if [[ "$HOST_MODE" == "1" ]]; then
        check_firewall
        printf '\n  oxide-sloc starting in LAN server mode\n  Local   \xe2\x86\x92 http://127.0.0.1:%s\n' "$SLOC_PORT"
        print_lan_ip
        if [[ "$PLATFORM" == linux ]]; then
            printf '  Firewall: %s\n' "$FIREWALL_STATUS"
            if [[ -n "$FIREWALL_FIX" ]]; then
                printf '    Other LAN hosts cannot reach this server until you run:\n'
                printf '      %s\n' "$FIREWALL_FIX"
            fi
        elif [[ "$PLATFORM" == windows ]]; then
            printf '  Firewall: Windows Defender may show a network access dialog.\n'
            printf '    Click "Allow access" to open port %s \xe2\x80\x94 no admin rights required.\n' "$SLOC_PORT"
        fi
        printf '\n'
        printf '  Press Ctrl+C to stop.\n\n'
        "$1" serve --server
    else
        printf '\n  oxide-sloc starting \xe2\x86\x92 http://127.0.0.1:%s\n  Press Ctrl+C to stop.\n\n' "$SLOC_PORT"
        "$1"
    fi
}

launch_cargo() {
    free_port
    assert_port_free
    cd "$REPO_ROOT"
    export OXIDE_SLOC_ROOT="$REPO_ROOT"

    local bin_path="$REPO_ROOT/target/debug/oxide-sloc"
    [[ "$PLATFORM" == windows ]] && bin_path="${bin_path}.exe"

    local tmpout
    tmpout="$(mktemp)"
    local LOG_FILE
    LOG_FILE="$LOG_DIR/run-$(date +%Y-%m-%d-%H-%M-%S).log"

    # Approximate total from lockfile for the progress bar denominator
    local total_pkgs=0
    if [[ -f "$REPO_ROOT/Cargo.lock" ]]; then
        total_pkgs="$(grep -c '^\[\[package\]\]' "$REPO_ROOT/Cargo.lock" 2>/dev/null)" || total_pkgs=0
    fi

    printf '\n  oxide-sloc \xe2\x80\x94 building from source\n\n'

    cargo build -p oxide-sloc 2>"$tmpout" &
    local cargo_pid=$!

    trap 'kill "$cargo_pid" 2>/dev/null; rm -f "$tmpout"; printf "\033[?25h\n"; exit 130' INT TERM

    local spin=('|' '/' '-' '\')
    local si=0 compiled=0 last_crate="" bar_w=34
    local start_time="$SECONDS"
    local is_linking=0 first_draw=1
    local NLINES=9  # height of the dynamic status block
    local resolution_msgs=(
        "Reading Cargo.lock..."
        "Verifying vendor sources..."
        "Resolving dependency graph..."
        "Checking workspace manifests..."
        "Scanning crate metadata..."
        "Validating source checksums..."
    )
    local msg_idx=0 frame_count=0 msg_interval=17  # rotate every ~2 s (17 × 0.12 s)

    printf '\033[?25l'  # hide cursor

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

        # Progress bar
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

        # Milestone icons: ○ waiting  spinner in-progress  ✓ done
        local ic_res='\xe2\x97\x8b' ic_cmp='\xe2\x97\x8b' ic_lnk='\xe2\x97\x8b'
        if   [[ "$is_linking" -eq 1 ]]; then
            ic_res='\xe2\x9c\x93'; ic_cmp='\xe2\x9c\x93'; ic_lnk="$frame"
        elif [[ "$compiled"   -gt 0 ]]; then
            ic_res='\xe2\x9c\x93'; ic_cmp="$frame"
        else
            ic_res="$frame"
        fi

        # Move cursor back up to rewrite on every frame after the first
        [[ "$first_draw" -eq 0 ]] && printf "\033[%dA" "$NLINES"
        first_draw=0

        # 9-line status block — each line ends with \033[K to clear any trailing chars
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
    done

    wait "$cargo_pid"
    local exit_code=$?
    printf '\033[?25h'  # restore cursor
    trap - INT TERM

    compiled="$(grep -c "^   Compiling" "$tmpout" 2>/dev/null)" || compiled=0

    {
        printf '# oxide-sloc run log\n# Date:     %s\n# Platform: %s\n# Exit:     %d\n#\n' \
            "$(date)" "$PLATFORM" "$exit_code"
        cat "$tmpout"
    } > "$LOG_FILE"

    # Overwrite the status block with the final result
    [[ "$first_draw" -eq 0 ]] && printf "\033[%dA" "$NLINES"

    if [[ $exit_code -ne 0 ]]; then
        printf "  \xe2\x9c\x97  BUILD FAILED%-67s\n" ""
        for (( i = 1; i < NLINES; i++ )); do printf "\033[K\n"; done
        printf '\n'
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
    printf '\n  Log \xe2\x86\x92 %s\n' "$LOG_FILE"

    if [[ "$HOST_MODE" == "1" ]]; then
        check_firewall
        printf '\n  oxide-sloc starting in LAN server mode\n  Local   \xe2\x86\x92 http://127.0.0.1:%s\n' "$SLOC_PORT"
        print_lan_ip
        if [[ "$PLATFORM" == linux ]]; then
            printf '  Firewall: %s\n' "$FIREWALL_STATUS"
            if [[ -n "$FIREWALL_FIX" ]]; then
                printf '    Other LAN hosts cannot reach this server until you run:\n'
                printf '      %s\n' "$FIREWALL_FIX"
            fi
        elif [[ "$PLATFORM" == windows ]]; then
            printf '  Firewall: Windows Defender may show a network access dialog.\n'
            printf '    Click "Allow access" to open port %s \xe2\x80\x94 no admin rights required.\n' "$SLOC_PORT"
        fi
        printf '\n'
        printf '  Press Ctrl+C to stop.\n\n'
        "$bin_path" serve --server
    else
        printf '\n  oxide-sloc starting \xe2\x86\x92 http://127.0.0.1:%s\n  Press Ctrl+C to stop.\n\n' "$SLOC_PORT"
        "$bin_path"
    fi
}

# Mirror the server's fail-closed auth gate BEFORE we build/launch anything in
# --host mode. serve --server REFUSES to start without SLOC_API_KEY/SLOC_API_KEYS
# (unless the operator opts into an open server), so printing a "starting…" banner
# and only then dead-ending is misleading. Surface the real choice and stop early.
check_server_auth() {
    [[ "$HOST_MODE" == "1" ]] || return 0
    if [[ -n "${SLOC_API_KEY:-}" ]] || [[ -n "${SLOC_API_KEYS:-}" ]]; then
        return 0
    fi
    if [[ "${SLOC_ALLOW_UNAUTHENTICATED:-}" == "1" ]]; then
        printf '\n  WARNING: SLOC_ALLOW_UNAUTHENTICATED=1 \xe2\x80\x94 running an OPEN server; all\n'
        printf '           endpoints are unauthenticated. Only do this on a trusted, isolated network.\n'
        return 0
    fi
    printf '\n  ERROR: LAN server mode (--host) requires authentication, and none is set.\n' >&2
    printf '         serve --server will refuse to start. Choose one, then re-run:\n\n' >&2
    printf '           export SLOC_API_KEY="$(head -c 32 /dev/urandom | base64)"   # recommended\n' >&2
    printf '         or, ONLY on a trusted isolated network, opt into an open server:\n' >&2
    printf '           export SLOC_ALLOW_UNAUTHENTICATED=1\n\n' >&2
    exit 1
}

# ── CLI passthrough vs. web-UI launch ────────────────────────────────────────
# With no subcommand, run.sh launches the web UI (serve). The same launcher also
# runs one-shot CLI commands for automation, e.g.:
#   bash scripts/run.sh analyze ./my-repo --per-file --html-out out/report.html
# If the first non-flag argument is a known oxide-sloc subcommand (or a help /
# version query), the whole argument list is handed to the binary and the server
# is NOT started. Launcher-only flags (--host/--lan/--build/--rebuild) are
# consumed here and never forwarded; everything else passes through verbatim.
# `serve` is intentionally NOT a passthrough so `run.sh serve` keeps the managed
# port-freeing + fail-closed auth banner.
KNOWN_SUBCOMMANDS=" analyze batch report diff init validate send bundle git-scan git-compare watch prune pr-comment jira bitbucket-status verify-audit healthz completions help "
CLI_SUBCOMMAND=""
PASSTHROUGH_ARGS=()
for arg in "$@"; do
    case "$arg" in
        --rebuild|--force|-f) REBUILD_MODE=true; BUILD_MODE=true ;;
        --release) RELEASE_REBUILD=true ;;
        --host|--lan) HOST_MODE=1 ;;
        --build) BUILD_MODE=true ;;
        -h|--help|-V|--version)
            [[ -z "$CLI_SUBCOMMAND" ]] && CLI_SUBCOMMAND="help"
            PASSTHROUGH_ARGS+=("$arg") ;;
        -*) PASSTHROUGH_ARGS+=("$arg") ;;
        *)
            if [[ -z "$CLI_SUBCOMMAND" && "$KNOWN_SUBCOMMANDS" == *" $arg "* ]]; then
                CLI_SUBCOMMAND="$arg"
            fi
            PASSTHROUGH_ARGS+=("$arg") ;;
    esac
done

# The fail-closed server-auth gate only applies to the web/server launch path.
[[ -z "$CLI_SUBCOMMAND" ]] && check_server_auth

# ── Auto-install or rebuild if requested ─────────────────────────────────────
# `--rebuild` (without `--release`) is the fast developer loop: a plain debug
# `cargo build` that picks up source changes in seconds. The web GUI is compiled-in
# HTML/CSS/JS, so it is byte-identical to a release build — only runtime analysis
# speed differs, which does not matter for previewing UI changes. We point $EXE at
# the freshly built target/debug binary so the launch/CLI paths below run it (never a
# stale release $EXE at the repo root).
if [[ "$REBUILD_MODE" == true && "$RELEASE_REBUILD" != true ]]; then
    echo " [--rebuild] Fast debug build (pass --rebuild --release for the production build)..."
    echo ""
    cd "$REPO_ROOT"
    cargo build -p oxide-sloc \
        || { printf '\n [ERROR] Build failed. See above for details.\n' >&2; exit 1; }
    EXE="$REPO_ROOT/target/debug/oxide-sloc"
    [[ "$PLATFORM" == windows ]] && EXE="${EXE}.exe"
    echo ""
# Otherwise auto-install / release-rebuild via install.sh.
# On Windows this extracts the committed pre-built binary from dist/ (the .tar.gz
# or .zip, whichever an offline host can unpack) — no build, no network.
# On Linux this extracts dist/ if present, else builds from vendor + toolchain.
# `--rebuild --release` forces a from-source release build (install.sh --rebuild).
elif [[ ! -f "$EXE" ]] || [[ "$REBUILD_MODE" == true ]]; then
    if [[ "$REBUILD_MODE" == true ]]; then
        echo " [--rebuild --release] Forcing fresh release install..."
    else
        echo " Binary not found — running install first..."
    fi
    echo ""
    _INSTALL_FLAGS=""
    [[ "$REBUILD_MODE" == true ]] && _INSTALL_FLAGS="--rebuild"
    [[ "$BUILD_MODE" == true ]]   && _INSTALL_FLAGS="$_INSTALL_FLAGS --build"
    # shellcheck disable=SC2086
    bash "$SCRIPT_DIR/internal/install.sh" $_INSTALL_FLAGS \
        || { printf '\n [ERROR] Install failed. See above for details.\n' >&2; exit 1; }
    echo ""
fi

# CLI passthrough: run the requested subcommand directly and exit (no server).
if [[ -n "$CLI_SUBCOMMAND" ]]; then
    cd "$REPO_ROOT"
    export OXIDE_SLOC_ROOT="$REPO_ROOT"
    if [[ -f "$EXE" ]]; then
        exec "$EXE" "${PASSTHROUGH_ARGS[@]}"
    fi
    if command -v cargo &>/dev/null && [[ -f "$REPO_ROOT/Cargo.toml" ]]; then
        exec cargo run --quiet -p oxide-sloc -- "${PASSTHROUGH_ARGS[@]}"
    fi
    printf '\noxide-sloc: no binary found and cargo is unavailable.\n' >&2
    printf '  Run the installer first: bash scripts/internal/install.sh\n\n' >&2
    exit 1
fi

# Prefer the installed binary built by install.sh (release, no recompile needed).
# Fall back to cargo build only when no binary exists yet (e.g. install.sh failed
# to produce one but cargo is available for a quick debug build).
if [[ -f "$EXE" ]]; then
    launch "$EXE"
    exit 0
fi

if command -v cargo &>/dev/null && [[ -f "$REPO_ROOT/Cargo.toml" ]]; then
    launch_cargo
    exit 0
fi

printf '\noxide-sloc: no binary found.\n\n' >&2
printf '  Run the installer first: bash scripts/internal/install.sh\n' >&2
printf '  See docs/airgap.md for all deployment paths (no internet required).\n\n' >&2
exit 1
