# Stage 1: build the release binary
# Pin builder to rust:1.97-slim-bookworm so the toolchain and runtime both use
# Debian bookworm (glibc 2.36). rust:slim can resolve to a trixie-based digest
# (glibc 2.39) while the runtime stage below is still bookworm-slim, causing
# "GLIBC_2.39 not found" at container startup. The minor series tracks
# rust-toolchain.toml (channel 1.97).
# Digest pinned to prevent silent base-image substitution.
# To update: docker pull rust:1.97-slim-bookworm && docker inspect --format '{{index .RepoDigests 0}}' rust:1.97-slim-bookworm
#
# Base images are build ARGs so a hardened build can substitute an approved
# registry image (e.g. Chainguard, distroless, or a private mirror) WITHOUT
# editing this file, e.g.:
#   docker build \
#     --build-arg BUILDER_IMAGE=registry.example.com/rust/rust:1.95 \
#     --build-arg RUNTIME_IMAGE=registry.example.com/ubi/ubi9-minimal:9.4 \
#     --build-arg INSTALL_CHROMIUM=0 .
# Defaults remain the digest-pinned Docker Hub images for the public build.
# NOTE: ARGs used in FROM must be declared before the first FROM (global scope).
ARG BUILDER_IMAGE=rust:1.97-slim-bookworm@sha256:b93beca49a16e4459e51e3886658afece337bb2e991325b2919e84f7ae4a42ef
ARG RUNTIME_IMAGE=debian:bookworm-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241
FROM ${BUILDER_IMAGE} AS builder

# Fail a piped RUN if any stage in the pipe fails (e.g. `cat vendor.tar.gz.* |
# tar`), not just the last. bookworm-slim ships bash, so this is safe here.
SHELL ["/bin/bash", "-o", "pipefail", "-c"]

# Upgrade base packages first to pull in any OS-level security fixes
# that have landed since the image was published. Package versions are left
# unpinned on purpose so each build tracks the current upstream patch level; the
# builder base image is digest-pinned above to keep the source reproducible.
# hadolint ignore=DL3005,DL3008
RUN apt-get update \
    && apt-get upgrade -y --no-install-recommends \
    && apt-get install -y --no-install-recommends \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
RUN mkdir -p .cargo
COPY ci/docker-cargo-config.toml .cargo/config.toml
COPY crates/ crates/
COPY docs/ docs/
COPY vendor.tar.gz.aa vendor.tar.gz.ab vendor.tar.gz.ac vendor.checksums.sha256 ./

# Verify the split vendor parts, reassemble and extract them.
# This must happen before `cargo build` because .cargo/config.toml points cargo
# at the vendor/ directory as the sole crate source (no network access).
# gzip is used (not xz) because it is universally available on all agents.
RUN sha256sum -c vendor.checksums.sha256 \
    && cat vendor.tar.gz.* | tar -xzf - \
    && rm -f vendor.tar.gz.*

# Pre-flight: confirm the workspace source tree is actually present in the build
# context. If crates/ is accidentally re-added to .dockerignore this produces a
# clear, actionable error instead of a cryptic Cargo manifest failure.
RUN test -d crates/sloc-config \
    || { \
         echo "ERROR: crates/sloc-config is missing from the Docker build context." >&2; \
         echo "Check .dockerignore — crates/ must not be excluded." >&2; \
         exit 1; \
       }

RUN cargo build --release -p oxide-sloc --no-default-features

# Stage 2: minimal runtime image
# Pin to a specific digest to prevent silent base-image substitution.
# To update: docker pull debian:bookworm-slim && docker inspect --format '{{index .RepoDigests 0}}' debian:bookworm-slim
#
# Hardened deployments: override RUNTIME_IMAGE (declared at the top) with an
# approved registry image (Chainguard, distroless, or a private mirror) and build
# with `--build-arg INSTALL_CHROMIUM=0` to drop the browser.
FROM ${RUNTIME_IMAGE}

# Chromium is a large, frequently-CVE'd attack surface. It is only needed for the
# headless-Chromium PDF path; oxide-sloc has a pure-Rust PDF fallback, so hardened
# images can omit it with `--build-arg INSTALL_CHROMIUM=0`.
# For a fully air-gapped Docker host, build this layer from a pre-populated apt mirror.
ARG INSTALL_CHROMIUM=1
# Versions left unpinned so the patch level tracks upstream; the runtime base
# image is digest-pinned above to keep the source reproducible.
# hadolint ignore=DL3005,DL3008
RUN apt-get update \
    && apt-get upgrade -y --no-install-recommends \
    && apt-get install -y --no-install-recommends ca-certificates \
    && if [ "$INSTALL_CHROMIUM" = "1" ]; then \
         apt-get install -y --no-install-recommends chromium; \
       fi \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/oxide-sloc /usr/local/bin/oxide-sloc

# Create a non-root service account and ensure the output directory is writable by it.
RUN groupadd -r sloc && useradd -r -g sloc -u 1001 sloc \
    && mkdir -p /app/out \
    && chown -R sloc:sloc /app/out

ENV OXIDE_SLOC_ROOT=/app

# Point oxide-sloc at the system Chromium
ENV SLOC_BROWSER=/usr/bin/chromium
# SLOC_BROWSER_NOSANDBOX is intentionally NOT set here.
# Pass -e SLOC_BROWSER_NOSANDBOX=1 at runtime when running in a container
# runtime that does not grant SYS_ADMIN (most runtimes, and required when
# cap_drop: ALL is set). With SYS_ADMIN and a permissive seccomp profile the
# sandbox can be enabled by leaving this unset.

EXPOSE 4317

USER 1001

# HEALTHCHECK verifies the /healthz endpoint is responsive.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD oxide-sloc healthz 2>/dev/null || exit 1

ENTRYPOINT ["oxide-sloc"]
# --server: binds to 0.0.0.0, suppresses browser auto-open, disables desktop-only routes.
CMD ["serve", "--server"]
