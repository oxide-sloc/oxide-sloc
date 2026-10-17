#!/usr/bin/env bash
# Creates a fully self-contained offline build kit for oxide-sloc.
#
# Run this on a NETWORKED machine. Transfer the output archive to the
# air-gapped system (USB, internal file server, etc.) and follow the
# README.txt inside.
#
# What the kit includes:
#   - Rust standalone installer  (rustc, cargo, std — pinned to rust-toolchain.toml)
#   - musl Rust target stdlib    (for the static-binary target)
#   - musl C toolchain           (musl-gcc + headers from musl.cc — no system libc deps)
#   - Rust crate vendor sources  (vendor.tar.gz.* split parts — all ~328 crates, offline cargo build)
#   - oxide-sloc source archive
#   - Self-contained install.sh  (extracts toolchains, vendors, builds — no internet)
#
# Runtime result: a single fully static binary with zero system library dependencies.
#
# Usage:
#   bash scripts/internal/make-airgap-kit.sh                  # auto-detect host arch
#   bash scripts/internal/make-airgap-kit.sh linux-x86_64
#   bash scripts/internal/make-airgap-kit.sh linux-arm64
#
# Output: oxide-sloc-airgap-kit-{target}-v{version}.tar.gz
# On the air-gapped machine:
#   tar xzf oxide-sloc-airgap-kit-*.tar.gz
#   cd oxide-sloc-airgap-kit-*/
#   bash install.sh

set -euo pipefail

# ── Resolve versions ─────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

# Rust channel from rust-toolchain.toml ("1.95" → download version "1.95.0")
RUST_CHANNEL=$(grep '^channel' rust-toolchain.toml | sed 's/.*= *"\([^"]*\)".*/\1/')
if [[ "$RUST_CHANNEL" =~ ^[0-9]+\.[0-9]+$ ]]; then
    RUST_DL_VERSION="${RUST_CHANNEL}.0"
elif [[ "$RUST_CHANNEL" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    RUST_DL_VERSION="$RUST_CHANNEL"
else
    RUST_DL_VERSION="$RUST_CHANNEL"   # stable / nightly
fi

PROJ_VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*= *"\([^"]*\)".*/\1/')

# ── Target selection ─────────────────────────────────────────────────────────

case "${1:-auto}" in
    linux-x86_64|auto)
        RUST_HOST_TARGET="x86_64-unknown-linux-gnu"
        RUST_MUSL_TARGET="x86_64-unknown-linux-musl"
        MUSL_CC_ARCHIVE="x86_64-linux-musl-native.tgz"
        PLATFORM_LABEL="linux-x86_64"
        ;;
    linux-arm64)
        RUST_HOST_TARGET="aarch64-unknown-linux-gnu"
        RUST_MUSL_TARGET="aarch64-unknown-linux-musl"
        MUSL_CC_ARCHIVE="aarch64-linux-musl-native.tgz"
        PLATFORM_LABEL="linux-arm64"
        ;;
    *)
        echo "Usage: $0 [linux-x86_64|linux-arm64]" >&2
        exit 1
        ;;
esac

KIT_NAME="oxide-sloc-airgap-kit-${PLATFORM_LABEL}-v${PROJ_VERSION}"
KIT_DIR="$REPO_ROOT/$KIT_NAME"
KIT_ARCHIVE="${KIT_NAME}.tar.gz"

RUST_BASE="https://static.rust-lang.org/dist"
MUSL_CC_BASE="https://musl.cc"

echo ""
echo "  oxide-sloc airgap kit builder"
echo "  ════════════════════════════"
echo "  Platform  : $PLATFORM_LABEL"
echo "  Rust      : $RUST_DL_VERSION"
echo "  Project   : v$PROJ_VERSION"
echo "  Output    : $KIT_ARCHIVE"
echo ""

rm -rf "$KIT_DIR"
mkdir -p "$KIT_DIR"

# ── Helper: download + optional checksum ─────────────────────────────────────

fetch() {
    local url="$1" dest="$2"
    echo "  → $(basename "$dest")"
    curl -fL --retry 3 --retry-delay 2 --progress-bar "$url" -o "$dest"
    # Try to fetch a .sha256 side-car; if unavailable, skip silently.
    if curl -fsL "${url}.sha256" -o "${dest}.sha256" 2>/dev/null; then
        sha256sum -c "${dest}.sha256" --quiet \
            || { echo "CHECKSUM MISMATCH: $dest" >&2; exit 1; }
        echo "    checksum OK"
    fi
}

# ── Step 1: Vendor archive ───────────────────────────────────────────────────

echo "==> Generating vendor archive (cargo vendor)..."
bash scripts/internal/update-vendor.sh
cp vendor.tar.gz.*          "$KIT_DIR/"
cp vendor.checksums.sha256  "$KIT_DIR/vendor.checksums.sha256"

# ── Step 2: Source archive ───────────────────────────────────────────────────

echo "==> Creating source archive (git archive HEAD)..."
git archive HEAD --format=tar.gz -o "$KIT_DIR/oxide-sloc-src.tar.gz"

# ── Step 2b: Supply-chain trust reports (best-effort) ────────────────────────
# The trust reports travel *with* the kit so an air-gapped reviewer can inspect
# the SBOM and vulnerability scan without any network access. These need Syft /
# Grype / Trivy on the builder's PATH; if any is absent we print a note and skip
# it — the kit still builds. Generated now, so the reports are timestamped to the
# kit-build moment with the DBs current as of this run.

echo "==> Generating supply-chain trust reports (security/)..."
SEC_DIR="$KIT_DIR/security"
mkdir -p "$SEC_DIR"
SEC_TS="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

if command -v syft >/dev/null 2>&1; then
    syft dir:. -o spdx-json="$SEC_DIR/oxide-sloc.spdx.json" \
                -o cyclonedx-json="$SEC_DIR/oxide-sloc.cdx.json" \
        && echo "    ✓ Syft SBOM (SPDX + CycloneDX)" \
        || echo "    ! Syft run failed — SBOM skipped"
else
    echo "    - Syft not found on PATH — SBOM skipped (install: https://github.com/anchore/syft)"
fi

if command -v grype >/dev/null 2>&1 && [[ -f "$SEC_DIR/oxide-sloc.cdx.json" ]]; then
    grype "sbom:$SEC_DIR/oxide-sloc.cdx.json" -o table \
        > "$SEC_DIR/grype-report.txt" 2>/dev/null \
        && echo "    ✓ Grype vulnerability report" \
        || echo "    ! Grype run failed — report skipped"
elif ! command -v grype >/dev/null 2>&1; then
    echo "    - Grype not found on PATH — vuln report skipped (install: https://github.com/anchore/grype)"
fi

if command -v trivy >/dev/null 2>&1; then
    trivy fs --scanners vuln,secret,misconfig --severity HIGH,CRITICAL \
        --ignore-unfixed --no-progress -f table -o "$SEC_DIR/trivy-report.txt" . \
        && echo "    ✓ Trivy filesystem report" \
        || echo "    ! Trivy run failed — report skipped"
else
    echo "    - Trivy not found on PATH — fs report skipped (install: https://github.com/aquasecurity/trivy)"
fi

cat > "$SEC_DIR/README.txt" << SECREADME
oxide-sloc — supply-chain trust reports
=======================================

Generated at kit-build time: ${SEC_TS}
Project version: v${PROJ_VERSION}

These reports let you inspect what this build contains and whether any known
vulnerabilities apply — entirely offline, from inside the air-gap kit.

  oxide-sloc.spdx.json   Software Bill of Materials (SPDX 2.3, from Syft)
  oxide-sloc.cdx.json    Software Bill of Materials (CycloneDX, from Syft)
  grype-report.txt       Vulnerability scan of the CycloneDX SBOM (Grype)
  trivy-report.txt       Filesystem scan: vulns + secrets + misconfig (Trivy)

Any file missing above means the corresponding tool was not installed on the
machine that built this kit. The reports are a point-in-time snapshot; the
vulnerability databases reflect the kit-build date, not the install date.

To re-scan on a networked machine with the same tools:
  syft  dir:<source> -o cyclonedx-json=sbom.json
  grype sbom:sbom.json
  trivy fs <source>
SECREADME
echo "    ✓ security/README.txt"

# ── Step 3: Rust host toolchain ──────────────────────────────────────────────

echo "==> Downloading Rust $RUST_DL_VERSION host toolchain ($RUST_HOST_TARGET)..."
fetch \
    "${RUST_BASE}/rust-${RUST_DL_VERSION}-${RUST_HOST_TARGET}.tar.gz" \
    "${KIT_DIR}/rust-${RUST_DL_VERSION}-${RUST_HOST_TARGET}.tar.gz"

# ── Step 4: Rust musl target std ────────────────────────────────────────────

echo "==> Downloading Rust $RUST_DL_VERSION musl std ($RUST_MUSL_TARGET)..."
fetch \
    "${RUST_BASE}/rust-std-${RUST_DL_VERSION}-${RUST_MUSL_TARGET}.tar.gz" \
    "${KIT_DIR}/rust-std-${RUST_DL_VERSION}-${RUST_MUSL_TARGET}.tar.gz"

# ── Step 5: musl C toolchain (musl-gcc + headers + libc) ────────────────────

echo "==> Downloading musl C toolchain ($MUSL_CC_ARCHIVE from musl.cc)..."
fetch \
    "${MUSL_CC_BASE}/${MUSL_CC_ARCHIVE}" \
    "${KIT_DIR}/${MUSL_CC_ARCHIVE}"

# ── Step 6: Embed the self-contained install script ──────────────────────────

echo "==> Writing kit install.sh..."

cat > "${KIT_DIR}/install.sh" << 'INNER_INSTALL'
#!/usr/bin/env bash
# oxide-sloc offline build installer
# Run this on the air-gapped machine after extracting the kit archive.
#
# Requirements: bash, tar, gzip, sha256sum
# Internet access: NOT required.
# System compiler: NOT required (bundled musl-gcc is used).
#
# Usage:
#   bash install.sh          # static musl binary (no system lib deps at runtime)
#   bash install.sh --gnu    # dynamic GNU binary (requires system gcc to be present)
#
# After a successful build:
#   ./oxide-sloc serve       # web UI at http://127.0.0.1:4317
#   ./oxide-sloc --help

set -euo pipefail

KIT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TOOLS_DIR="$KIT_DIR/.tools"
RUST_PREFIX="$TOOLS_DIR/rust"
MUSL_PREFIX="$TOOLS_DIR/musl"
BUILD_DIR="$KIT_DIR/build"

USE_GNU=false
for arg in "$@"; do [[ "$arg" == "--gnu" ]] && USE_GNU=true; done

echo ""
echo "  oxide-sloc offline build installer"
echo "  ═══════════════════════════════════"

# ── 1. Rust toolchain ────────────────────────────────────────────────────────

RUST_ARCHIVE=$(ls "$KIT_DIR"/rust-[0-9]*.tar.gz 2>/dev/null \
    | grep -v "rust-std" | head -1 || true)

if [[ -z "$RUST_ARCHIVE" ]]; then
    echo "ERROR: Rust toolchain archive not found in kit." >&2; exit 1
fi

if ! command -v cargo &>/dev/null; then
    echo "==> Installing Rust toolchain (offline)..."
    [[ -f "${RUST_ARCHIVE}.sha256" ]] \
        && sha256sum -c "${RUST_ARCHIVE}.sha256" --quiet

    mkdir -p "$RUST_PREFIX"
    RUST_BASENAME=$(basename "$RUST_ARCHIVE" .tar.gz)
    tar xzf "$RUST_ARCHIVE" -C "$TOOLS_DIR"
    bash "$TOOLS_DIR/$RUST_BASENAME/install.sh" \
        --prefix="$RUST_PREFIX" \
        --disable-ldconfig \
        2>&1 | grep -v "^creating\|^installing\|^warning" || true
    rm -rf "$TOOLS_DIR/$RUST_BASENAME"
    echo "==> Rust installed: $RUST_PREFIX/bin/rustc"
fi

export PATH="$RUST_PREFIX/bin:$PATH"
export CARGO_HOME="$RUST_PREFIX"

# ── 2. Rust musl std library ─────────────────────────────────────────────────

MUSL_STD_ARCHIVE=$(ls "$KIT_DIR"/rust-std-*musl*.tar.gz 2>/dev/null | head -1 || true)

if [[ -n "$MUSL_STD_ARCHIVE" ]] && [[ "$USE_GNU" == false ]]; then
    MUSL_TARGET=$(basename "$MUSL_STD_ARCHIVE" .tar.gz \
        | sed 's/rust-std-[^-]*-//')
    if [[ ! -d "$RUST_PREFIX/lib/rustlib/$MUSL_TARGET" ]]; then
        echo "==> Installing Rust musl std ($MUSL_TARGET)..."
        [[ -f "${MUSL_STD_ARCHIVE}.sha256" ]] \
            && sha256sum -c "${MUSL_STD_ARCHIVE}.sha256" --quiet

        MUSL_STD_BASENAME=$(basename "$MUSL_STD_ARCHIVE" .tar.gz)
        tar xzf "$MUSL_STD_ARCHIVE" -C "$TOOLS_DIR"
        bash "$TOOLS_DIR/$MUSL_STD_BASENAME/install.sh" \
            --prefix="$RUST_PREFIX" \
            --disable-ldconfig \
            2>&1 | grep -v "^creating\|^installing\|^warning" || true
        rm -rf "$TOOLS_DIR/$MUSL_STD_BASENAME"
        echo "==> musl Rust std installed."
    fi
fi

# ── 3. musl C toolchain (musl-gcc + headers + libc) ──────────────────────────

MUSL_CC_ARCHIVE=$(ls "$KIT_DIR"/*-linux-musl-native.tgz 2>/dev/null | head -1 || true)

if [[ -n "$MUSL_CC_ARCHIVE" ]] && [[ "$USE_GNU" == false ]]; then
    if [[ ! -d "$MUSL_PREFIX/bin" ]]; then
        echo "==> Installing musl C toolchain..."
        mkdir -p "$MUSL_PREFIX"
        tar xzf "$MUSL_CC_ARCHIVE" -C "$MUSL_PREFIX" --strip-components=1
        echo "==> musl C toolchain installed: $MUSL_PREFIX/bin"
    fi
    export PATH="$MUSL_PREFIX/bin:$PATH"
fi

# ── 4. Determine build target and C compiler ──────────────────────────────────

ARCH=$(uname -m)
if [[ "$USE_GNU" == true ]]; then
    BUILD_TARGET="${ARCH}-unknown-linux-gnu"
    CC_ENV_KEY=""
    LINKER_VAR=""
else
    BUILD_TARGET="${ARCH}-unknown-linux-musl"
    MUSL_GCC="$MUSL_PREFIX/bin/${ARCH}-linux-musl-gcc"
    # Env var key that the 'cc' crate reads for cross-compilation
    CC_ENV_KEY="CC_$(echo "$BUILD_TARGET" | tr '-' '_')"
    LINKER_VAR="$MUSL_GCC"
    if [[ ! -f "$MUSL_GCC" ]]; then
        echo "ERROR: musl-gcc not found at $MUSL_GCC" >&2
        echo "       Re-run without --gnu to use the bundled musl toolchain," >&2
        echo "       or run with --gnu to use the system gcc." >&2
        exit 1
    fi
    export "$CC_ENV_KEY=$MUSL_GCC"
fi

echo "==> Build target: $BUILD_TARGET"

# ── 5. Extract source and vendor ─────────────────────────────────────────────

echo "==> Extracting source..."
rm -rf "$BUILD_DIR"
mkdir -p "$BUILD_DIR"
tar xzf "$KIT_DIR/oxide-sloc-src.tar.gz" -C "$BUILD_DIR"

echo "==> Verifying and extracting vendor sources..."
( cd "$KIT_DIR" && sha256sum -c vendor.checksums.sha256 --quiet )
cat "$KIT_DIR"/vendor.tar.gz.* | tar -xzf - -C "$BUILD_DIR"

# ── 6. Configure cargo for offline vendored build ─────────────────────────────

mkdir -p "$BUILD_DIR/.cargo"

CARGO_CONFIG="$BUILD_DIR/.cargo/config.toml"
cat > "$CARGO_CONFIG" << EOF
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF

# Point cargo at the musl linker for the musl target
if [[ -n "$LINKER_VAR" ]]; then
    cat >> "$CARGO_CONFIG" << EOF

[target.$BUILD_TARGET]
linker = "$LINKER_VAR"
EOF
fi

# ── 7. Build ──────────────────────────────────────────────────────────────────

LOG_DIR="$KIT_DIR/logs"
mkdir -p "$LOG_DIR"
LOG_FILE="$LOG_DIR/kit-install-$(date +%Y-%m-%d-%H-%M-%S).log"

# Tee all remaining output to the log file.
exec > >(tee -a "$LOG_FILE") 2>&1
printf '# oxide-sloc kit install log — %s\n# Target: %s\n\n' "$(date)" "$BUILD_TARGET"

_on_exit() {
    local ec=$?
    [[ $ec -eq 0 ]] && return
    printf '\n[FAILED] Build exited with code %d.\n' "$ec"
    printf '  Log dir:  %s\n' "$LOG_DIR"
    printf '  Log file: %s\n' "$(basename "$LOG_FILE")"
    printf '  \033]8;;file://%s\033\\Click to open log\033]8;;\033\\\n' "$LOG_FILE"
}
trap '_on_exit' EXIT

echo "==> Building oxide-sloc (offline release build — target: $BUILD_TARGET)..."
cd "$BUILD_DIR"
cargo build --release --offline --target "$BUILD_TARGET" -p oxide-sloc

BINARY="$BUILD_DIR/target/${BUILD_TARGET}/release/oxide-sloc"
if [[ ! -f "$BINARY" ]]; then
    echo "ERROR: Build succeeded but binary not found at $BINARY" >&2
    exit 1
fi

cp "$BINARY" "$KIT_DIR/oxide-sloc"
chmod +x "$KIT_DIR/oxide-sloc"

# Print a summary of what was linked (static vs dynamic)
echo ""
if command -v file &>/dev/null; then
    file "$KIT_DIR/oxide-sloc"
fi
if command -v ldd &>/dev/null; then
    ldd "$KIT_DIR/oxide-sloc" 2>/dev/null || echo "  (statically linked — no system libs required)"
fi

echo ""
echo "  ════════════════════════════════════════"
echo "  [OK] Binary ready: $KIT_DIR/oxide-sloc"
echo ""
echo "  Start web UI:   ./oxide-sloc serve"
echo "  Analyze a repo: ./oxide-sloc analyze /path/to/repo --plain"
echo "  Full CLI help:  ./oxide-sloc --help"
echo "  Deploy guide:   ./build/docs/airgap.md"
echo "  ════════════════════════════════════════"
INNER_INSTALL

chmod +x "${KIT_DIR}/install.sh"

# ── Step 7: README ───────────────────────────────────────────────────────────

cat > "${KIT_DIR}/README.txt" << README
oxide-sloc offline build kit — v${PROJ_VERSION} (${PLATFORM_LABEL})
═══════════════════════════════════════════════════════════════════

WHAT'S INCLUDED
  Rust toolchain   rust-${RUST_DL_VERSION}-${RUST_HOST_TARGET}
  musl Rust std    rust-std-${RUST_DL_VERSION}-${RUST_MUSL_TARGET}
  musl C toolchain ${MUSL_CC_ARCHIVE}  (musl-gcc, headers, libc)
  Crate sources    vendor.tar.gz.*  (split parts, ~328 crates, no crates.io access needed)
  Source code      oxide-sloc-src.tar.gz

QUICK START (air-gapped machine, no internet, no pre-installed tools)
  bash install.sh

  This will:
    1. Install the Rust toolchain into .tools/rust/  (no root, no PATH changes)
    2. Install the musl C toolchain into .tools/musl/ (no root)
    3. Extract and verify the vendor crate sources
    4. Build a fully static oxide-sloc binary in this directory

  After a successful build:
    ./oxide-sloc serve                    # web UI → http://127.0.0.1:4317
    ./oxide-sloc analyze /path/to/repo --plain

OPTIONS
  bash install.sh --gnu    Use system gcc instead of bundled musl toolchain.
                           Produces a dynamically linked binary (requires glibc).
                           Use this if the musl build fails or you prefer it.

RUNTIME REQUIREMENTS AFTER BUILDING
  Static musl binary (default):   NONE — copy the binary anywhere and run it.
  GNU binary (--gnu option):      glibc on the target system (almost always present).

SYSTEM REQUIREMENTS FOR BUILDING
  OS:        Linux ${PLATFORM_LABEL}
  Tools:     bash, tar, gzip, sha256sum
  Root:      NOT required (installs to .tools/ inside this directory)
  Internet:  NOT required

TROUBLESHOOTING
  "gzip: command not found":
    Install gzip (should be present on any Linux system;
    Debian/Ubuntu: apt-get install gzip / RHEL/Rocky: yum install gzip).

  "sha256sum: command not found":
    Install coreutils (should be present on any Linux system).

  Build fails after "Compiling ring":
    Make sure the musl toolchain extracted correctly (.tools/musl/bin/ should exist).
    Or try: bash install.sh --gnu  (uses system gcc instead).

  To restart a failed build cleanly:
    rm -rf build/ .tools/  && bash install.sh
README

# ── Step 8: Package the kit ──────────────────────────────────────────────────

echo "==> Packaging kit..."
cd "$REPO_ROOT"
tar -czf "$KIT_ARCHIVE" "$(basename "$KIT_DIR")/"
rm -rf "$KIT_DIR"

KIT_SIZE=$(du -sh "$KIT_ARCHIVE" | cut -f1)
echo ""
echo "  ══════════════════════════════════════════════════"
echo "  Kit created: $KIT_ARCHIVE  ($KIT_SIZE)"
echo ""
echo "  Transfer to the air-gapped machine, then:"
echo "    tar xzf $KIT_ARCHIVE"
echo "    cd $KIT_NAME"
echo "    bash install.sh"
echo "  ══════════════════════════════════════════════════"
