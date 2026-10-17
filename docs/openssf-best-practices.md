# OpenSSF Best Practices — Silver / Gold tracking

oxide-sloc holds the OpenSSF Best Practices **passing** badge
([project 12976](https://www.bestpractices.dev/en/projects/12976)). This page tracks progress
toward the **Silver** and **Gold** tiers. Most higher-tier criteria are already satisfied by work
in the repository; this is largely a matter of asserting each one in the badge app.

## Already satisfied (evidence in-repo)

| Criterion (tier) | Evidence |
|---|---|
| Version-controlled with interim history (Silver) | Public git history on GitHub |
| Unique version numbering + release notes (Silver) | SemVer in `Cargo.toml`; `CHANGELOG.md`; templated GitHub Release notes |
| Code of conduct (Silver) | [`CODE_OF_CONDUCT.md`](../CODE_OF_CONDUCT.md) |
| Contribution guide + DCO/roles (Silver) | [`CONTRIBUTING.md`](../CONTRIBUTING.md), `.github/ISSUE_TEMPLATE/` |
| Static analysis on every change (Silver) | CodeQL, clippy `-D warnings`, SonarQube |
| Dynamic analysis (Silver/Gold) | DAST workflow (`dast.yml`) |
| Dependency vulnerability scanning (Silver) | `cargo-audit` + `cargo-deny` hard gate; osv-scanner; Grype; Trivy |
| SBOM published (Gold) | Syft SPDX + CycloneDX and cargo-cyclonedx SBOMs attached to each release |
| Signed releases (Silver/Gold) | cosign keyless + GPG + SLSA build-provenance attestation |
| Reproducible build (Gold) | [`reproducible-builds.md`](./reproducible-builds.md) + `reproducible-build.yml` |
| Automated test suite + CI (Silver) | `cargo test` workspace suite in `ci.yml`; nextest config |
| Secured delivery against MITM (Silver) | HTTPS everywhere; per-part SHA-256 on the offline vendor/toolchain archives |
| Hardening headers / least privilege (Gold) | Web server security headers; top-level least-privilege GH token perms (Scorecard) |
| Warning flags enabled and clean (Silver) | `-D warnings`, `rustfmt --check`, fmt gate |
| Two-person review available (Gold) | Branch protection + OpenSSF Scorecard Code-Review |
| Citation metadata | [`CITATION.cff`](../CITATION.cff) |

## Remaining to assert / close

- [ ] **Silver — crypto:** document that oxide-sloc itself stores no secrets and that all
      transport uses TLS (link `docs/server-deployment.md`); confirm no custom crypto.
- [ ] **Silver — test coverage statement:** state the enforced coverage posture (SonarQube gate;
      new source lines must be covered) and link the codecov badge.
- [ ] **Silver — vulnerability report process ≤ 14 days:** reaffirm the SLA in `SECURITY.md`.
- [ ] **Gold — reproducible build claim:** check the box and link
      [`reproducible-builds.md`](./reproducible-builds.md).
- [ ] **Gold — SBOM claim:** check the box and link the release SBOM assets.
- [ ] **Gold — two independent reviewers / bus-factor:** document the review policy; note current
      maintainer bus-factor and the mirror strategy.

## How to update the badge

1. Sign in at <https://www.bestpractices.dev/en/projects/12976>.
2. Switch the tier selector to **Silver**, then **Gold**.
3. For each criterion above, paste the in-repo evidence link into the justification field.
4. Save; the badge level in `README.md` updates automatically from the live endpoint.
