# Supply chain security

oxide-sloc is built for **air-gapped deployment**: a plain `git clone` carries the binaries,
vendored crates, and Rust toolchain needed to build and run with zero internet. That
convenience only earns trust if you can verify what you are deploying. Every release is
therefore described by a Software Bill of Materials (SBOM) and scanned for known
vulnerabilities with the tooling security reviewers and procurement teams recognize.

This page documents the scanning layer. It complements — it does not replace — the signing and
provenance already in place (cosign, SLSA, GitHub build-provenance attestation, VirusTotal).

## Tools

| Tool | Role | Output | Published to |
|---|---|---|---|
| [Syft](https://github.com/anchore/syft) | SBOM generation | SPDX-JSON + CycloneDX-JSON | Release assets |
| [Grype](https://github.com/anchore/grype) | Vulnerability scan of the SBOM | Table report + SARIF | Release assets + code scanning |
| [Trivy](https://github.com/aquasecurity/trivy) | Filesystem + container scan (vulns, secrets, misconfig) | SARIF | GitHub code scanning |
| `cargo-cyclonedx` | Rust-native SBOM | CycloneDX-JSON | Release assets |
| `cargo-audit` + `cargo-deny` | Advisory + license/ban gate | CI pass/fail | **Hard CI gate** |

Only `cargo-audit` / `cargo-deny` are a hard gate. Syft, Grype, and Trivy run **report-only** so
a newly disclosed advisory in a transitive crate surfaces as information without blocking a
release or an image publish.

## Where the reports live

- **GitHub release assets** (per `vX.Y.Z` tag):
  - `oxide-sloc.spdx.json` — Syft SBOM, SPDX 2.3
  - `oxide-sloc.cdx.json` — Syft SBOM, CycloneDX
  - `sbom.cdx.json` — cargo-cyclonedx SBOM (Rust dependency graph)
  - `grype-report.txt` — Grype vulnerability scan of the CycloneDX SBOM
  - All are listed in `SHA256SUMS.txt` with the rest of the release assets.
- **GitHub Security → Code scanning** (categories `trivy-fs`, `trivy-image`, `grype`):
  - `trivy-fs` — filesystem scan on every push to `main` (ci.yml)
  - `trivy-image` — container image scan on each GHCR push (docker.yml)
  - `grype` — SBOM vulnerability scan on each release (release.yml)
- **Air-gap kit** (`scripts/internal/make-airgap-kit.sh`): a `security/` folder containing the
  Syft SBOMs, a Grype report, a Trivy report, and a `README.txt`. These are best-effort — if a
  tool is not installed on the networked machine building the kit, that file is skipped and the
  kit still builds.

## Which CI pipeline runs what

| Pipeline | File | Scan |
|---|---|---|
| CI (push / PR) | `.github/workflows/ci.yml` | Trivy `fs` → SARIF (`trivy-fs`) |
| Docker | `.github/workflows/docker.yml` | Trivy `image` by digest → SARIF (`trivy-image`) |
| Release (`v*` tag) | `.github/workflows/release.yml` | Syft SBOMs + Grype → release assets + SARIF (`grype`) |

## Verifying offline

Everything below runs with no network once you have the tools and the release assets on the
air-gapped host.

Inspect the SBOM contents (no extra tooling — it is JSON):

```bash
# Package count in the CycloneDX SBOM
jq '.components | length' oxide-sloc.cdx.json

# List every component name + version
jq -r '.components[] | "\(.name) \(.version)"' oxide-sloc.cdx.json
```

Re-run the vulnerability scan against the shipped SBOM (needs `grype`, which uses its last
downloaded database):

```bash
grype sbom:oxide-sloc.cdx.json
```

Scan the extracted source tree directly (needs `trivy`):

```bash
trivy fs --scanners vuln,secret,misconfig .
```

The air-gap kit's `security/README.txt` repeats these commands next to the shipped reports.

## Notes

- The vulnerability databases reflect the **scan date** (CI run / kit-build), not your install
  date. Bundling the databases for offline re-scan is intentionally out of scope — they are
  hundreds of megabytes and go stale quickly; the shipped reports are a timestamped snapshot.
- Report-only scanning is deliberate. Blocking a release on a transitive-dependency CVE would
  couple shipping to the upstream advisory feed; `cargo-audit` / `cargo-deny` already gate the
  dependency graph during development, and dependency bumps land continuously.
- See also: cosign / SLSA / GPG verification steps in the release notes, and `SECURITY.md` for
  reporting.
