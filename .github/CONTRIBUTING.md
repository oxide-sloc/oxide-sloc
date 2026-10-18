# Contributing to oxide-sloc

Thank you for your interest in improving oxide-sloc. This guide covers everything you need to get started.

## Quick start

```bash
git clone https://github.com/oxide-sloc/oxide-sloc.git
cd oxide-sloc

# Build the full workspace (cargo downloads dependencies from crates.io)
cargo build --workspace

# Run the CI gates locally before pushing
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
cargo test --workspace
```

## CI gates

All four must pass before a PR can merge:

| Check | Command |
|---|---|
| Format | `cargo fmt --all -- --check` |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| Build | `cargo build --workspace` |
| Tests | `cargo test --workspace` |

Run `cargo fmt --all` to auto-fix formatting issues.

## Adding a new language

Language support touches two crates and must be kept in sync:

1. **`crates/sloc-languages/src/lib.rs`**
   - Add a variant to `Language`
   - Add cases in `display_name()`, `as_slug()`, `from_name()`
   - Add extension / filename detection in `detect_language()`
   - Add a `ScanConfig` entry in `analyze_text()`
   - Add to `supported_languages()`

2. **`crates/sloc-config/src/lib.rs`** — verify that the new language name is accepted via `enabled_languages` filtering (it uses `Language::from_name` so no change is usually needed).

3. Add a small test file for the new language under `testing/fixtures/basic/` so the smoke test covers it.

## Adding a new CLI flag

All flags live in `crates/sloc-cli/src/main.rs`. Follow the existing pattern: add the field to the relevant `*Args` struct, wire it in the `run_*` handler, and add it to the relevant `resolve_*_config` helper if it maps to a config field.

## Adding a new output format

Output writers live in `crates/sloc-report/src/lib.rs`. Export the public function from that crate, then call it from the CLI handler after the analysis run completes.

## Counting standard

oxide-sloc implements **physical SLOC** per **IEEE Std 1045-1992**. The configurable parameters live in `sloc_config::AnalysisConfig`; the state machine that applies them is in `sloc_languages::analyze_generic`. When touching line-counting logic, keep the following invariants:

- `total_physical_lines` must equal the number of `\n`-terminated lines in the file, regardless of any policy setting.
- `compiler_directive_lines` must always be a strict subset of `code_only_lines` (never incremented for mixed or comment-only lines).
- `effective_counts.code_lines` must never go negative — use `saturating_sub` when subtracting directive lines.
- All new counting options must be mirrored in three places: `AnalysisConfig` (config + TOML), `AnalysisOptions` / `IeeeFlags` in `sloc-languages` (state machine), and the CLI `AnalyzeArgs` struct (flag).

## Vendor archive (for air-gapped builds and releases)

The split gzip parts `vendor.tar.gz.aa` / `.ab` / `.ac` and `vendor.checksums.sha256` are **committed to the repository** so that a plain `git clone` is sufficient for a fully offline (air-gapped) build. All parts and the checksums file must always be updated together. Gzip is used (not xz) because stock Git Bash on Windows has no `xz`; gzip extracts everywhere. The parts are split into ≤45 MB chunks to stay under GitHub's 100 MB per-file limit and total ~115 MB.

For normal development, cargo downloads from crates.io and no vendor setup is needed. When you add or upgrade a dependency, regenerate the archive and commit all parts atomically:

```bash
# Regenerate the vendor snapshot, repack the split parts, and rewrite the checksums file
bash scripts/internal/update-vendor.sh
git add vendor.tar.gz.* vendor.checksums.sha256
git commit -m "chore: update vendor archive"
```

Verify + reassemble + extract with `sha256sum -c vendor.checksums.sha256 && cat vendor.tar.gz.* | tar -xzf -`. Never commit the `vendor.tar.gz.*` parts without updating `vendor.checksums.sha256` — both the Docker build and Jenkins CI verify the checksums before extracting.

For air-gapped builds, see [`docs/airgap.md`](./docs/airgap.md).

## Release secrets

The release workflow (`release.yml`) uses several optional GitHub repository secrets. Configure them under **Settings → Secrets and variables → Actions**.

| Secret | Purpose | Required? |
|--------|---------|-----------|
| `GPG_PRIVATE_KEY` | Armored GPG key for detached `.sig` signatures | Optional |
| `GPG_PASSPHRASE` | Passphrase protecting the GPG key | Optional |
| `WINDOWS_CERTIFICATE` | Base64-encoded PFX for Windows Authenticode signing | Optional |
| `WINDOWS_CERTIFICATE_PASSWORD` | Password for the Windows PFX | Optional |
| `APPLE_DEVELOPER_ID_CERT` | Base64-encoded PFX for macOS Developer ID signing | Optional |
| `APPLE_DEVELOPER_ID_CERT_PASSWORD` | Password for the macOS PFX | Optional |
| `APPLE_NOTARIZE_APPLE_ID` | Apple ID email for notarization | Optional |
| `APPLE_NOTARIZE_PASSWORD` | App-specific password for notarization | Optional |
| `APPLE_NOTARIZE_TEAM_ID` | Apple Developer Team ID | Optional |
| `VT_API_KEY` | VirusTotal API v3 key — submit binaries for malware scanning on every release | Optional |

All secrets are optional — the corresponding workflow steps skip gracefully when the secret is absent.

### VirusTotal scanning

When `VT_API_KEY` is set, the `virustotal` job in `release.yml` automatically runs on every tagged release. It uploads each release binary to VirusTotal, polls for scan completion, and writes a results table to the GitHub Release body showing malicious / suspicious / undetected / harmless counts and a permalink to each full report.

To obtain a free API key: create an account at virustotal.com → API key → copy the v3 key. Free accounts allow 4 requests/minute and 500 requests/day, which is sufficient for a release with 5 binaries. Add the key as a repository secret named `VT_API_KEY`.

False positives on freshly compiled Rust binaries are common. Zero malicious detections is the expected result; any flags on a clean build can be disputed directly from the VirusTotal report page.

## Commit messages

Follow the format used in `git log`:

```
type: short imperative summary (≤72 chars)
```

Common types: `feat`, `fix`, `refactor`, `docs`, `chore`, `ci`, `test`.

The first line is the summary shown in `git log` and in the GitHub file listing —
keep the `type:` at the very start with no stray leading characters, and put any
longer explanation in the body after a blank line.

## Pull request checklist

- [ ] All four CI gates pass locally
- [ ] New language support updates both `sloc-languages` and has a fixture file in `testing/fixtures/basic/`
- [ ] New dependencies: run `bash scripts/internal/update-vendor.sh` so the next release bundles them
- [ ] `CHANGELOG.md` updated under `[Unreleased]`
- [ ] No `#[allow(...)]` without a comment explaining why
- [ ] No `.unwrap()` or `.expect()` in library code

## License

By contributing you agree that your contributions will be licensed under the
project's [AGPL-3.0-or-later](LICENSE) license.
