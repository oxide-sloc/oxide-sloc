#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Send an email alert about the oxide-sloc service.
#
# Transport: SMTP over TLS via curl (present on virtually every Linux host).
# Configuration comes from the environment — see deploy/alert.env.example and the
# EnvironmentFile= line in the systemd units. Nothing here is oxide-sloc-specific,
# so you can reuse an existing relay / the same SLOC_SMTP_* values you already have.
#
# Usage: send-alert.sh "<subject>" "<body>"
set -euo pipefail

SUBJECT="${1:?usage: send-alert.sh <subject> <body>}"
BODY="${2:-}"

: "${ALERT_SMTP_URL:?set ALERT_SMTP_URL, e.g. smtps://smtp.example.com:465}"
: "${ALERT_FROM:?set ALERT_FROM, e.g. oxide-sloc@example.com}"
: "${ALERT_TO:?set ALERT_TO, e.g. ops@example.com}"
: "${ALERT_SMTP_USER:?set ALERT_SMTP_USER}"
: "${ALERT_SMTP_PASS:?set ALERT_SMTP_PASS}"

# RFC 5322 message. CRLF line endings are required by SMTP.
message="$(printf 'From: %s\r\nTo: %s\r\nSubject: %s\r\nDate: %s\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n%s\r\n' \
    "$ALERT_FROM" "$ALERT_TO" "$SUBJECT" "$(date -R)" "$BODY")"

# Avoid exposing SMTP credentials in the process list via --user.
curl_cfg="$(mktemp)"
trap 'rm -f "$curl_cfg"' EXIT
chmod 600 "$curl_cfg"
printf 'user = "%s:%s"\n' "$ALERT_SMTP_USER" "$ALERT_SMTP_PASS" >"$curl_cfg"

printf '%s' "$message" | curl --silent --show-error --ssl-reqd \
    --url "$ALERT_SMTP_URL" \
    --mail-from "$ALERT_FROM" \
    --mail-rcpt "$ALERT_TO" \
    --config "$curl_cfg" \
    --upload-file -
