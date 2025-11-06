# Multi-stage Dockerfile optimized for layer caching (no cargo-chef needed)
FROM --platform=$BUILDPLATFORM rust:bullseye AS builder

ENV PORT=8080

WORKDIR /app

# Install system dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
 && rm -rf /var/lib/apt/lists/*

# Copy only Cargo files first (this layer caches unless dependencies change)
COPY Cargo.toml Cargo.lock ./
COPY crates/x402-axum/Cargo.toml crates/x402-axum/Cargo.toml
COPY crates/x402-reqwest/Cargo.toml crates/x402-reqwest/Cargo.toml
COPY examples/x402-axum-example/Cargo.toml examples/x402-axum-example/Cargo.toml
COPY examples/x402-reqwest-example/Cargo.toml examples/x402-reqwest-example/Cargo.toml

# Create dummy source files to build dependencies
RUN mkdir -p src && \
    echo "fn main() {}" > src/main.rs && \
    echo "pub fn dummy() {}" > src/lib.rs && \
    mkdir -p crates/x402-axum/src && \
    echo "pub fn dummy() {}" > crates/x402-axum/src/lib.rs && \
    mkdir -p crates/x402-reqwest/src && \
    echo "pub fn dummy() {}" > crates/x402-reqwest/src/lib.rs && \
    mkdir -p examples/x402-axum-example/src && \
    echo "fn main() {}" > examples/x402-axum-example/src/main.rs && \
    mkdir -p examples/x402-reqwest-example/src && \
    echo "fn main() {}" > examples/x402-reqwest-example/src/main.rs

# Build dependencies only (this step caches until Cargo.toml/lock changes)
# Expected time: 3-5 minutes, but CACHED on subsequent builds
RUN cargo build --release --bin x402-rs && rm -rf src target/release/x402-rs* target/release/deps/libx402_rs*

# Now copy actual source code
COPY src ./src
COPY abi ./abi
COPY static ./static
COPY config ./config
COPY crates ./crates
COPY examples ./examples

# Build application with cached dependencies
# Expected time: 30-90 seconds for code-only changes
RUN cargo build --release --bin x402-rs

# --- Stage 2: Runtime ---
FROM --platform=$BUILDPLATFORM debian:bullseye-slim

ENV PORT=8080

RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the compiled binary
COPY --from=builder /app/target/release/x402-rs /usr/local/bin/x402-rs

# Copy configuration files (blacklist.json must be present at runtime)
COPY --from=builder /app/config /app/config

EXPOSE $PORT
ENV RUST_LOG=info

ENTRYPOINT ["x402-rs"]
