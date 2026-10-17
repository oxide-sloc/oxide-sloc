# Reproducible builds

oxide-sloc release binaries are **reproducible**: rebuilding the same tagged commit with the
pinned toolchain produces a byte-for-byte identical binary. This is the verification layer under
the signing and provenance already in place (cosign keyless signatures, SLSA build-provenance
attestation, GPG, VirusTotal — see [supply-chain.md](./supply-chain.md)). Signatures prove *who*
built a binary; reproducibility lets anyone independently prove *what* the source compiles to, so
a published, attested binary can be confirmed free of hidden changes.

## What makes the build deterministic

| Source of nondeterminism | How it is pinned |
|---|---|
| Compiler / std version | `rust-toolchain.toml` pins the exact toolchain; the air-gap bundle ships that same version |
| Dependency versions | `Cargo.lock` is committed; builds use `--locked` |
| Embedded build timestamps | `SOURCE_DATE_EPOCH` is set to a fixed value |
| Incremental-cache fingerprints | `CARGO_INCREMENTAL=0` |
| Absolute build path leaking into debug info | `RUSTFLAGS=--remap-path-prefix=<checkout>=/oxide-sloc` |

## CI verification

[`.github/workflows/reproducible-build.yml`](../.github/workflows/reproducible-build.yml) checks
out the repository **twice, into two different directories**, builds the release binary in each
with the settings above, and fails if the two SHA-256 hashes differ. It runs weekly and on
manual dispatch (report-only — never on the release critical path).

## Verifying locally

Reproduce the two-path build on any Linux host with the pinned toolchain:

```bash
export SOURCE_DATE_EPOCH=1700000000
export CARGO_INCREMENTAL=0

git clone https://github.com/oxide-sloc/oxide-sloc a && \
  RUSTFLAGS="--remap-path-prefix=$PWD/a=/oxide-sloc" \
  cargo build --release --locked -p oxide-sloc --manifest-path a/Cargo.toml

git clone https://github.com/oxide-sloc/oxide-sloc b && \
  RUSTFLAGS="--remap-path-prefix=$PWD/b=/oxide-sloc" \
  cargo build --release --locked -p oxide-sloc --manifest-path b/Cargo.toml

sha256sum a/target/release/oxide-sloc b/target/release/oxide-sloc
# The two hashes must match.
```

To check a published release binary against your own rebuild, check out the release tag, rebuild
with the same flags, and compare your hash to the one in the release's `SHA256SUMS.txt`.

> Reproducibility is verified on `x86_64-unknown-linux-gnu`. Cross-compiled targets (Windows GNU,
> arm64) are signed and provenance-attested but not asserted bit-identical here, because the
> linkers and system objects differ per host.
