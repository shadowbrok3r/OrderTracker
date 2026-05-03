# Supervisor's build context is only this app folder, not the monorepo root.
# Fetch the full tree so Cargo.toml, src/, assets/ are available (public clone).
FROM rust:1-bookworm AS builder

# Override at build time if you fork: docker build --build-arg GIT_URL=https://github.com/you/OrderTracker.git
ARG GIT_URL=https://github.com/shadowbrok3r/OrderTracker.git

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    git \
    && rm -rf /var/lib/apt/lists/*

RUN cargo install dioxus-cli

WORKDIR /build
# Shallow clone of default branch (Supervisor context is only homeassistant/, not repo root).
RUN git clone --depth 1 "${GIT_URL}" .

RUN touch .env
RUN dx build --release --platform web

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
COPY --from=builder /build/homeassistant/run.sh /run.sh
RUN chmod a+x /run.sh

CMD [ "/run.sh" ]
