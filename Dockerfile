FROM --platform=$BUILDPLATFORM rust:bullseye AS builder

ARG FACILITATOR_VERSION=dev
ENV FACILITATOR_VERSION=${FACILITATOR_VERSION}
ENV PORT=8080

WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
 && rm -rf /var/lib/apt/lists/*

COPY . ./
RUN cargo build --release --features solana,near,stellar

# --- Stage 2 ---
FROM --platform=$BUILDPLATFORM debian:bullseye-slim

ENV PORT=8080

# much smaller than full ubuntu (~22MB compressed)

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/x402-rs /usr/local/bin/x402-rs

# Copy configuration files (blacklist.json must be present at runtime)
COPY --from=builder /app/config /app/config

# Copy static assets (landing page, logos)
COPY --from=builder /app/static /app/static

EXPOSE $PORT
ENV RUST_LOG=info

ENTRYPOINT ["x402-rs"]