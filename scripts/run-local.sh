#!/bin/bash
# run-local.sh - Run the facilitator on this machine without it touching production.
#
# WHY THIS SCRIPT EXISTS
#
# On 2026-09-02 the binary was run locally to measure the agentic surfaces. The
# recipe in docs/handoffs/2026-09-02-superficies-agenticas-listo.md switched
# nothing off, so the process:
#
#   - read NONCE_STORE_TABLE_NAME, whose writer-lease default was the REAL
#     table `facilitator-nonces`, and
#   - built an AWS client from aws_config::load_defaults(), which picks up
#     whatever credentials are in the environment without asking,
#
# and stood in the production EVM writer-lease election. It lost -- the
# ConditionalCheckFailed says the lease was not its -- but it could have won,
# and the winner's address is where every other task forwards its EVM settles.
# A settle routed to 127.0.0.1 on somebody's laptop is a payment that never
# lands on chain.
#
# src/writer_lease.rs now refuses the election outright when the address it
# would advertise is loopback or unknown, so the hole is closed by
# construction. This script is the second layer: it makes the whole local run
# incapable of reaching production AWS in the first place, instead of relying
# on anyone remembering a flag.
#
# Usage:
#   ./scripts/run-local.sh                 # build (debug) + run on 127.0.0.1:8402
#   PORT=9000 ./scripts/run-local.sh       # pick the port
#   ./scripts/run-local.sh --no-build      # skip cargo build
#
# Run it from the repo root, in WSL. Ctrl-C stops it.

set -euo pipefail

FEATURES="solana,near,stellar,algorand,sui,xrpl"
HOST="${HOST:-127.0.0.1}"
PORT="${PORT:-8402}"

# --- 1. Nothing this process does may reach a real AWS account ---------------
# Fake, syntactically valid credentials. The SDK's default chain reads the
# environment before any profile or IMDS, so these shadow whatever real keys
# the shell already had. AWS_EC2_METADATA_DISABLED stops the fallback to
# 169.254.169.254 on an EC2/ECS host, and clearing AWS_PROFILE stops a named
# profile from being consulted at all. Deliberately NOT in the `AKIA...`
# shape: a placeholder that looks like a real key id is a placeholder that
# trips every secret scanner that ever reads this repo.
export AWS_ACCESS_KEY_ID="local-run-not-a-real-access-key"
export AWS_SECRET_ACCESS_KEY="local-run-not-a-real-secret-key-000000000"
export AWS_REGION="${AWS_REGION:-us-east-2}"
export AWS_EC2_METADATA_DISABLED=true
unset AWS_PROFILE AWS_SESSION_TOKEN AWS_CONTAINER_CREDENTIALS_RELATIVE_URI || true

# --- 2. No production table names ------------------------------------------
# Every one of these is "unset means disabled" EXCEPT the writer lease, which
# used to default to the production table name on its own. Unsetting them is
# what keeps the in-memory stores in play (src/nonce_store.rs:407).
unset NONCE_STORE_TABLE_NAME TRANSACTIONS_TABLE_NAME IDEMPOTENCY_TABLE_NAME || true
unset DX402_REGISTRY_TABLE_NAME DX402_STORE_BUCKET || true

# --- 3. The lease kill-switch, belt to the code's braces ---------------------
# src/writer_lease.rs already abstains on a loopback address. This is the
# explicit "off" so the run is safe even against a build that predates it.
export ENABLE_WRITER_LEASE=false

# --- 4. A signing key that is ephemeral, unfunded and never written to disk --
# ProviderCache::from_env() aborts without one. It exists only so the process
# boots; it can neither sign anything anybody accepts nor pay for gas.
export SIGNER_TYPE="${SIGNER_TYPE:-private-key}"
export EVM_PRIVATE_KEY_TESTNET="0x$(python3 -c 'import secrets;print(secrets.token_hex(32))')"
export RPC_URL_BASE_SEPOLIA="${RPC_URL_BASE_SEPOLIA:-https://sepolia.base.org}"

export HOST PORT
export RUST_LOG="${RUST_LOG:-warn}"

if [ "${1:-}" != "--no-build" ]; then
  cargo build --features "$FEATURES"
fi

[ -f config/blacklist.json ] || cp config/blacklist.json.example config/blacklist.json

echo "Local facilitator on http://${HOST}:${PORT} -- writer lease off, AWS credentials fake."
exec ./target/debug/x402-rs
