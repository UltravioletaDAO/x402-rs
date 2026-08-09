FROM --platform=$BUILDPLATFORM rust:bullseye AS builder

# FACILITATOR_VERSION is deliberately NOT declared in this stage. It changes on
# every release, and an ARG/ENV carrying it here would key every layer below it —
# including the dependency build — on the release version, invalidating the whole
# cache exactly as having the version in Cargo.toml used to. Nothing here reads
# it: the binary resolves the version at runtime (src/version.rs), so it only
# needs to exist in the final stage.
ENV PORT=8080

WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
 && rm -rf /var/lib/apt/lists/*

# ---------------------------------------------------------------------------
# Dependency layer
#
# The whole dependency tree used to recompile on every build, because `COPY . ./`
# put the sources in the same layer as `cargo build`: any edit invalidated it.
# Worse, so did the version bump every release carries, so CI never once reused
# the cache for the step that dominates the build (~20 min).
#
# Compiling dependencies against stub sources isolates them in a layer keyed only
# on the manifests. Keep this block above the source COPY.
# ---------------------------------------------------------------------------
# rust-toolchain.toml must land BEFORE the dependency build. Arriving later with
# the sources, it made rustup swap the toolchain between the two cargo
# invocations, and a different rustc means different fingerprints: every
# dependency compiled here was thrown away and rebuilt in the final step.
COPY rust-toolchain.toml ./
COPY Cargo.toml Cargo.lock ./
COPY crates/x402-axum/Cargo.toml crates/x402-axum/
COPY crates/x402-compliance/Cargo.toml crates/x402-compliance/
COPY crates/x402-reqwest/Cargo.toml crates/x402-reqwest/
COPY examples/x402-axum-example/Cargo.toml examples/x402-axum-example/
COPY examples/x402-reqwest-example/Cargo.toml examples/x402-reqwest-example/

RUN set -eux; \
    # No version rewriting here: Cargo.toml's version is a frozen placeholder and
    # the release version travels as FACILITATOR_VERSION (see src/version.rs).
    # An earlier attempt pinned it with sed at this point and did nothing, because
    # the COPY above had already keyed the layer on the file's checksum -- a sed
    # inside the image cannot undo an invalidation that happened outside it.
    mkdir -p src \
             crates/x402-axum/src crates/x402-compliance/src crates/x402-reqwest/src \
             examples/x402-axum-example/src examples/x402-reqwest-example/src; \
    echo 'fn main() {}' > src/main.rs; \
    : > src/lib.rs; \
    : > crates/x402-axum/src/lib.rs; \
    : > crates/x402-compliance/src/lib.rs; \
    : > crates/x402-reqwest/src/lib.rs; \
    echo 'fn main() {}' > examples/x402-axum-example/src/main.rs; \
    echo 'fn main() {}' > examples/x402-reqwest-example/src/main.rs

RUN cargo build --release --features solana,near,stellar,algorand,sui,xrpl

# ---------------------------------------------------------------------------
# Real build
# ---------------------------------------------------------------------------
COPY . ./
# config/blacklist.json is gitignored, so git-based builds (e.g. GitHub Actions `COPY .`
# of the checkout) do not include it -- but the facilitator hard-requires it at startup
# (src/main.rs with_blacklist) and exits(1) if missing. Default to an empty list when
# absent so the image always starts. (scripts/fast-build.sh rsyncs the real local file;
# commit a managed config/blacklist.json to git if you want CI builds to ship real entries.)
RUN [ -f config/blacklist.json ] || printf '[]\n' > config/blacklist.json

# COPY preserves the source mtimes from the build context, which can be OLDER
# than the stub artifacts just built. Cargo's fingerprints are mtime-based, so
# without this the real sources look already-compiled and the image ships the
# stubs -- a silent, passing build of an empty binary. Touching them forces the
# local crates to rebuild; dependencies are untouched and stay cached.
RUN find src crates examples -name '*.rs' -exec touch {} +

RUN cargo build --release --features solana,near,stellar,algorand,sui,xrpl

# Fail here rather than ship a stub: the landing page is only inside the binary
# if static/ was compiled in, which the stub build cannot do.
RUN set -eux; \
    grep -aq 'Ultravioleta' target/release/x402-rs

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
