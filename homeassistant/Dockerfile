ARG BUILD_FROM
FROM rust:1-bookworm AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Install dx CLI for building the fullstack app
RUN cargo install dioxus-cli

WORKDIR /build

# Copy manifests first for dependency caching
COPY Cargo.toml Cargo.lock* build.rs ./
COPY Dioxus.toml ./

# Create a dummy main.rs so cargo can fetch deps
RUN mkdir -p src && echo "fn main() {}" > src/main.rs
RUN cargo fetch

# Copy the full source and assets
COPY src/ src/
COPY assets/ assets/

# Create a dummy .env so build.rs doesn't warn
RUN touch .env

# Build the fullstack web app (server binary + WASM client bundle)
RUN dx build --release --platform web

# ---------------------------------------------------------------------------
# Runtime stage
# ---------------------------------------------------------------------------
FROM $BUILD_FROM

# Install runtime deps (OpenSSL)
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the built server binary and static assets from the builder
COPY --from=builder /build/target/dx/order_tracker/release/web/server /app/order_tracker
COPY --from=builder /build/target/dx/order_tracker/release/web/public /app/public

# Copy the run script
COPY homeassistant/run.sh /run.sh
RUN chmod a+x /run.sh

CMD [ "/run.sh" ]
