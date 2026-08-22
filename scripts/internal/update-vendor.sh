#!/usr/bin/env bash
# Regenerate the split vendor.tar.gz.* parts and vendor.checksums.sha256 from the
# current Cargo.lock.
#
# Run this any time you add, remove, or update a dependency in Cargo.toml/Cargo.lock,
# then stage and commit ALL generated files:
#
#   git add vendor.tar.gz.* vendor.checksums.sha256
#   git commit -m "chore: update vendor archive"
#
# All files are tracked by git so that a plain `git clone` is sufficient for a fully
# offline (air-gapped) build — no separate download step required. gzip (not xz) is
# used so extraction works everywhere (Git for Windows bundles no xz), and the
# archive is split into 45 MB parts so no single file exceeds GitHub's 100 MB limit.
#
# Usage: bash scripts/internal/update-vendor.sh
#
# Verifying the offline build — ALWAYS start from a clean extraction, never a pre-existing
# vendor/ directory. Both /vendor/ and /.cargo/config.toml are gitignored (regenerated per
# checkout), so they are NOT the source of truth and a fresh `git clone` contains neither.
# The only committed vendor source is vendor.tar.gz.* + vendor.checksums.sha256. Therefore:
#   * `git show HEAD:vendor/<crate>/Cargo.toml` prints NOTHING (vendor/ is untracked) — it is
#     not a valid acceptance check. Inspect the archive instead:
#         cat vendor.tar.gz.* | tar -xzO vendor/<crate>/Cargo.toml | grep '^version'
#   * A stale vendor/ left over from an earlier checkout (or a local overlay) can make a
#     `cargo … --offline` build resolve an OLD crate version and report a bogus "wrong version"
#     failure that does not reflect the committed archive. Wipe and re-extract before trusting
#     any offline-build check:
#         git clean -xfd vendor/ .cargo/config.toml   # or: rm -rf vendor/ .cargo/config.toml
#         sha256sum -c vendor.checksums.sha256
#         cat vendor.tar.gz.* | tar -xzf -
#         cargo check --workspace --all-features --offline
#     Correct acceptance: checksums verify, the extracted vendor/<crate>/Cargo.toml shows the
#     version locked in Cargo.lock, and the offline check exits 0.
set -euo pipefail

CHECKSUMS_FILE=vendor.checksums.sha256

echo "Removing old vendor directory and archives..."
rm -rf vendor/
rm -f vendor.tar.gz vendor.tar.gz.* vendor.tar.xz vendor.tar.xz.sha256 "${CHECKSUMS_FILE}"

echo "Running cargo vendor..."
# --sync ci/tools/Cargo.toml pulls CI tooling (cargo-llvm-cov, etc.) into the
# vendor archive so they can be installed offline on air-gapped agents.
cargo vendor --sync ci/tools/Cargo.toml vendor/

echo "Packing vendor.tar.gz (gzip -9, deterministic sort)..."
# Use LC_ALL=C + sorted find output for a reproducible archive on any OS.
LC_ALL=C tar \
  --sort=name \
  --mtime="@0" \
  --owner=0 --group=0 --numeric-owner \
  -cf - vendor/ | gzip -9 > vendor.tar.gz

echo "Splitting into 45 MB parts (vendor.tar.gz.aa, .ab, ...)..."
split -b 45m vendor.tar.gz vendor.tar.gz.
rm -f vendor.tar.gz

echo "Writing ${CHECKSUMS_FILE}..."
sha256sum vendor.tar.gz.* > "${CHECKSUMS_FILE}"

echo "Done."
echo "  Parts   : $(ls vendor.tar.gz.* | tr '\n' ' ')"
echo "  Checksum: ${CHECKSUMS_FILE}"
echo ""
echo "Stage and commit all files:"
echo "  git add vendor.tar.gz.* vendor.checksums.sha256"
echo "  git rm --ignore-unmatch vendor.tar.xz vendor.tar.xz.sha256"
echo "  git commit -m \"chore: update vendor archive\""
