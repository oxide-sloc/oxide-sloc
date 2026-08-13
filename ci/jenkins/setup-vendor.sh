#!/usr/bin/env bash
# Reassemble the split vendor.tar.gz parts and write .cargo/config.toml for offline builds.
# CARGO_HOME is set by the Jenkinsfile environment{} block.
set -euo pipefail

AGENT_CHECKSUMS="${CARGO_HOME}/../vendor.checksums.sha256"
# Per-workspace marker recording which vendor set the extracted tree came from. Kept at
# the workspace root (NOT inside vendor/, which cargo scans as vendored sources) so it
# survives independently of the vendor/ contents.
STAMP=".vendor-stamp"

# Identity of the committed vendor set = SHA-256 of its checksums manifest. When the
# extracted vendor/ already carries a stamp equal to this, the tree is byte-for-byte
# what the current checkout expects, so re-extraction is pure waste. Jenkins reuses the
# workspace and `git checkout -f` leaves the untracked vendor/ dir in place, so on a
# source build this skips thousands of small-file writes (each AV-scanned on a corporate
# Windows agent) every run — historically the bulk of the Setup stage. The stamp changes
# whenever a dependency bump rewrites vendor.checksums.sha256, which correctly forces a
# fresh re-extract so cargo never resolves against a stale Cargo.lock-misaligned vendor/.
want_stamp=""
if [ -f vendor.checksums.sha256 ]; then
    want_stamp="$(sha256sum vendor.checksums.sha256 | awk '{print $1}')"
fi

if [ -d vendor ] && [ -n "${want_stamp}" ] \
        && [ "$(cat "${STAMP}" 2>/dev/null || true)" = "${want_stamp}" ]; then
    echo "vendor/ already extracted and matches committed checksums — skipping re-extraction."
else
    # Stale (recycled workspace) or absent vendor/ — wipe and (re-)extract from the
    # workspace parts, or fall back to the agent cache.
    rm -rf vendor
    if ls vendor.tar.gz.* >/dev/null 2>&1; then
        echo "Verifying vendor.tar.gz.* integrity..."
        sha256sum -c vendor.checksums.sha256
        echo "Reassembling and decompressing vendor.tar.gz.*..."
        cat vendor.tar.gz.* | tar -xzf -
    elif ls "${CARGO_HOME}/.."/vendor.tar.gz.* >/dev/null 2>&1; then
        echo "vendor.tar.gz.* not in workspace — falling back to agent cache..."
        cp "${CARGO_HOME}/.."/vendor.tar.gz.* .
        if [ -f "${AGENT_CHECKSUMS}" ]; then
            cp "${AGENT_CHECKSUMS}" vendor.checksums.sha256
            echo "Verifying vendor.tar.gz.* integrity..."
            sha256sum -c vendor.checksums.sha256
        else
            echo "WARNING: No vendor.checksums.sha256 in agent cache — skipping checksum verification."
        fi
        echo "Reassembling and decompressing vendor.tar.gz.*..."
        cat vendor.tar.gz.* | tar -xzf -
    else
        echo "ERROR: vendor.tar.gz.* parts not found in workspace or agent cache." >&2
        echo "       Ensure the repository was cloned from the correct branch/tag." >&2
        exit 1
    fi
    # Record the identity we just extracted so the next build can skip re-extraction.
    # Recompute here: the agent-cache branch may have just written the manifest.
    if [ -f vendor.checksums.sha256 ]; then
        sha256sum vendor.checksums.sha256 | awk '{print $1}' > "${STAMP}"
    fi
fi

echo "Writing .cargo/config.toml for fully offline builds..."
mkdir -p .cargo
cat > .cargo/config.toml << 'CARGOEOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
CARGOEOF
