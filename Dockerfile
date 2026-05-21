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
RUN cargo build --release --features solana,near,stellar,algorand,sui

# --- Stage 2 ---
FROM --platform=$BUILDPLATFORM debian:bullseye-slim

ARG FACILITATOR_VERSION=dev
ENV FACILITATOR_VERSION=${FACILITATOR_VERSION}
ENV PORT=8080

# much smaller than full ubuntu (~22MB compressed)

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

# B10: run as dedicated non-root user (defense-in-depth; ECS task-role still scopes IAM,
# but a compromised process cannot install packages, write outside owned paths, or
# escalate via setuid binaries).
RUN groupadd --system --gid 10001 facilitator \
 && useradd --system --uid 10001 --gid facilitator \
      --home-dir /app --shell /usr/sbin/nologin facilitator

WORKDIR /app

COPY --from=builder --chown=facilitator:facilitator /app/target/release/x402-rs /usr/local/bin/x402-rs

# Copy configuration files (blacklist.json must be present at runtime)
COPY --from=builder --chown=facilitator:facilitator /app/config /app/config

# Copy static assets (landing page, logos)
COPY --from=builder --chown=facilitator:facilitator /app/static /app/static

USER facilitator:facilitator

EXPOSE $PORT
ENV RUST_LOG=info \
    HOME=/app

LABEL org.opencontainers.image.title="x402-rs facilitator" \
      org.opencontainers.image.source="https://github.com/UltravioletaDAO/x402-rs" \
      org.opencontainers.image.vendor="Ultravioleta DAO" \
      org.opencontainers.image.licenses="Apache-2.0"

ENTRYPOINT ["x402-rs"]
