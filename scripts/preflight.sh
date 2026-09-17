#!/usr/bin/env bash
# The offline build/test gate used before opening a PR. Requires Rust stable,
# pkg-config, OpenSSL, protoc, Node >=18, Python 3 and boto3, matching ci.yaml.
# No credentials, payment submissions or infrastructure mutations.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
features=solana,near,stellar,algorand,sui,xrpl,hedera
python3 scripts/verify_landing_canonical.py --offline
node --test tests/frontend-capabilities.test.cjs
python3 -m unittest discover -s tests/scripts -p 'test_*balances.py'
cargo build --locked --features "$features"
cargo test --locked -p x402-rs --features "$features" -- --test-threads=1
cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance -- --test-threads=1
