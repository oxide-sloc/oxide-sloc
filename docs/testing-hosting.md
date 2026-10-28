# Hosting verification matrix

A repeatable checklist for confirming the server hosts correctly across platforms
and network topologies. Automated coverage (`scripts/tests/serve-server.bats`,
`scripts/tests/serve-server.Tests.ps1`, `crates/sloc-web/tests/tls_config.rs`,
`crates/sloc-web/tests/server_mode_routes.rs`) runs in the **Launcher & hosting
tests** CI workflow; this doc covers the end-to-end cases that need a real host or
a real network and can't run in CI.

## How to read it

Run each row and record pass/fail + date. Linux rows are reproducible locally via
the project's containers (`Dockerfile.rhel-build` for RHEL, the published GHCR
image) when bare-metal isn't available.

| # | Platform | Scenario | Command | Expected |
|---|----------|----------|---------|----------|
| 1 | Win 10/11, RHEL 8/9/10, Ubuntu/Debian | HTTP LAN launch | `bash scripts/serve-server.sh --with-auth` | Banner shows `http://<lan-ip>:4317`; `curl -H "Authorization: Bearer <key>" http://<lan-ip>:4317/healthz` → `ok` |
| 2 | same | Fail-closed auth gate | `bash scripts/serve-server.sh` (no flags, clean env) | Refuses to start; prints the `--with-auth` / `--i-understand-no-auth` guidance |
| 3 | same | Self-signed HTTPS | `bash scripts/serve-server.sh --with-auth --tls --tls-generate` | Banner shows `https://…`; cert under `~/.oxide-sloc/tls/`; client trusting `ca.crt` reaches `/healthz` (see client-trust notes in server-deployment.md) |
| 4 | same | Bring-your-own cert | `SLOC_TLS_CERT=… SLOC_TLS_KEY=… bash scripts/serve-server.sh --with-auth --tls` | HTTPS served with the supplied cert; no `~/.oxide-sloc/tls/` generated |
| 5 | same | Mutual TLS | `bash scripts/serve-server.sh --with-auth --tls --tls-generate --mtls ca.crt` | Request **without** a client cert is rejected at the TLS layer; request **with** a cert signed by `ca.crt` succeeds |
| 6 | same | TLS-required gate | `SLOC_REQUIRE_TLS=1 bash scripts/serve-server.sh --with-auth` (no `--tls`) | Refuses up front with "TLS is not enabled" guidance |
| 7 | RHEL 8 specifically | OpenSSL 1.1.1 cert gen | `bash scripts/internal/gen-tls-cert.sh` | Succeeds (config-file idiom, no `-addext`); `openssl x509 -text` shows `serverAuth` + SANs |
| 8 | Linux (firewalld) | Cross-VLAN firewall | `bash scripts/serve-server.sh --with-auth --open-firewall` | Port opened in the zone owning the primary-route interface; a peer on another subnet reaches `/healthz` |
| 9 | Windows | First-run firewall | `bash scripts/serve-server.sh --with-auth` | One UAC prompt adds an all-profiles/`remoteip=any` rule; subsequent launches are silent |
| 10 | Linux (systemd) | Persistent service | `sudo bash scripts/internal/install-systemd.sh` then reboot | Service auto-starts; `systemctl status oxide-sloc` active; LAN URL answers |
| 11 | Windows | Persistent service | `install-service.ps1` then restart the task (or reboot) | Scheduled task runs as SYSTEM; LAN URL answers; `-Uninstall` removes task + secret |
| 12 | any | Reverse proxy | Bind `127.0.0.1:4317` + `SLOC_TRUST_PROXY=1` behind nginx/Caddy/Apache/IIS (server-deployment.md) | HTTPS via the proxy; audit log / rate limit show the real client IP |
| 13 | any | Server-mode attack surface | `curl` the desktop routes with a valid key in server mode | `/pick-directory`, `/pick-file` → 404 (or inert); `/open-path` → `server_mode_disabled` |
| 14 | air-gapped | Offline build + serve | plain `git clone` → `bash scripts/run.sh --host` on a host with no internet | Builds from vendored crates + bundled toolchain and serves; no outbound connection |

## Notes

- **Windows client trust for self-signed certs:** the bundled `curl` uses schannel
  and ignores `--cacert`. Import the CA into the current-user store with
  `certutil -user -addstore Root ~/.oxide-sloc/tls/ca.crt`, or use `-k` to confirm
  the server itself is serving HTTPS.
- **Full-tunnel VPN / AP-isolation / inter-VLAN ACLs** can block LAN reachability
  even after the firewall is open — the launcher banner calls these out. Test from
  a peer on the same wired subnet first.
- Record results against the current release version (`oxide-sloc --version`).
