#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Edge-triggered health probe for oxide-sloc.
#
# Polls the server's /healthz endpoint and emails ONLY on a state transition
# (up -> down or down -> up), so a sustained outage produces one "DOWN" mail and
# one "RECOVERED" mail — never a flood every tick. Run periodically from
# oxide-sloc-health.timer.
#
# This catches "process alive but not serving" (hang, deadlock, wedged port),
# which systemd's Restart=on-failure alone cannot see.
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
HEALTH_URL="${HEALTH_URL:-http://127.0.0.1:4317/healthz}"
STATE_FILE="${HEALTH_STATE_FILE:-/var/lib/oxide-sloc/health.state}"
TIMEOUT="${HEALTH_TIMEOUT:-10}"
HOST="$(hostname 2>/dev/null || echo unknown-host)"

mkdir -p "$(dirname "$STATE_FILE")" 2>/dev/null || true
prev="up"
[ -f "$STATE_FILE" ] && prev="$(cat "$STATE_FILE" 2>/dev/null || echo up)"

code="$(curl --silent --output /dev/null --max-time "$TIMEOUT" \
    --write-out '%{http_code}' "$HEALTH_URL" 2>/dev/null || echo 000)"

if [ "$code" = "200" ]; then
    cur="up"
else
    cur="down"
fi

if [ "$cur" != "$prev" ]; then
    if [ "$cur" = "down" ]; then
        if "$HERE/send-alert.sh" \
            "[oxide-sloc] DOWN on ${HOST}" \
            "Health check to ${HEALTH_URL} FAILED (HTTP ${code}) at $(date -R).

The systemd service may be crash-looping or the process is alive but not
serving. Check:  systemctl status oxide-sloc  and  journalctl -u oxide-sloc -n 100"; then
            # Only persist the new state if the alert was sent successfully, so a mail
            # outage doesn't silently swallow the transition — we retry next tick.
            echo "$cur" > "$STATE_FILE"
        else
            echo "Failed to send DOWN alert; not updating health state." >&2
        fi
    else
        if "$HERE/send-alert.sh" \
            "[oxide-sloc] RECOVERED on ${HOST}" \
            "Health check to ${HEALTH_URL} is OK again (HTTP 200) at $(date -R)."; then
            # Only persist the new state if the alert was sent successfully, so a mail
            # outage doesn't silently swallow the transition — we retry next tick.
            echo "$cur" > "$STATE_FILE"
        else
            echo "Failed to send RECOVERED alert; not updating health state." >&2
        fi
    fi
fi
