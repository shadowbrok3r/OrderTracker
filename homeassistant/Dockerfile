# Supervisor's build context is only this app folder, not the monorepo root.
# Fetch the full tree so Cargo.toml, src/, assets/ are available (public clone).
FROM rust:1-bookworm AS builder

# Override at build time if you fork: docker build --build-arg GIT_URL=https://github.com/you/OrderTracker.git
ARG GIT_URL=https://github.com/shadowbrok3r/OrderTracker.git

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    git \
    curl \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Install the dx CLI, pinned to match the `dioxus = "0.7"` library dependency.
# The published prebuilt binaries are gnu-only and require glibc 2.39, but this
# builder is bookworm (glibc 2.36) and no musl build exists, so compile from
# source. --locked uses dioxus-cli's own lockfile, which pins git2 below 0.21
# (git2 0.21 dropped Cred::credential_helper, breaking auth-git2 0.5.8).
RUN cargo install dioxus-cli@0.7.9 --locked

WORKDIR /build

# Bust BuildKit cache when this folder changes on disk after "Update" / git pull in the store.
# (Rust sources are not in the build context — only GitHub — so Dockerfile/config bumps matter.)
COPY Dockerfile /tmp/addon.Dockerfile
COPY config.yaml /tmp/addon.config.yaml

# Supervisor passes these (see build logs). Changing `version:` in config.yaml invalidates the clone layer.
ARG BUILD_VERSION=0.0.0
ARG BUILD_ARCH=amd64

RUN echo "Order Tracker build ${BUILD_VERSION} (${BUILD_ARCH})" \
    && git clone --depth 1 "${GIT_URL}" .

RUN touch .env
# Avoid wasm-opt + DWARF SIGABRT on HA builders; same-path optimize can corrupt wasm on failure.
RUN dx build --release --platform web --debug-symbols false

# ---------------------------------------------------------------------------
# Runtime stage
FROM ghcr.io/home-assistant/base-debian:bookworm

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /build/target/dx/order_tracker/release/web/server /app/order_tracker
COPY --from=builder /build/target/dx/order_tracker/release/web/public /app/public

# s6-overlay supervises long-running apps under /etc/services.d/<name>/run so Stop/Restart
# from Supervisor sends SIGTERM to order_tracker. CMD+exec alone is not a tracked service.
COPY rootfs /
RUN chmod a+x /etc/services.d/order-tracker/run
