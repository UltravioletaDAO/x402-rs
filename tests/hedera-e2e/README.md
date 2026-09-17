# Native Hedera interoperability and canaries

Dependencies are pinned in `package-lock.json`: `@x402/hedera` and `@x402/core` 2.26.0, Hiero JavaScript SDK 2.85.0 and protobuf 2.31.0. Install with `npm ci --ignore-scripts`.

`vectors/` contains synthetic, offline accounts and publicly reproducible fixture keys. They are never funding destinations. `npm run generate` regenerates the official-client and adversarial vectors; it does not submit a transaction. Rust production-code tests consume these fixtures:

```sh
cargo test --locked --features hedera --lib chain::hedera
```

The ignored DynamoDB test explicitly requires `HEDERA_TEST_TABLE` and AWS credentials. It uses a unique `storetest:` partition, verifies conditional writes/quota/lease takeover across clients, and deletes only its own test items. It never signs or contacts a chain.

`live-canary.mjs` reads JSON from stdin with `confirm`, `network`, `facilitator`, `payer`, `privateKey`, `feePayer`, `payTo`, `asset` and `amount`. It starts a loopback merchant and uses the official HTTP client for 402/PAYMENT-REQUIRED/PAYMENT-SIGNATURE/200/PAYMENT-RESPONSE. It sends one bounded payment, asserts the returned sender/network/ID, retries the same settlement, then requires replay verification to fail. Keys never go in argv or output. Its JSON output contains public evidence only. Feed credentials from a secret store in memory; do not create plaintext configuration files.

The ignored `live_receipt_persistence_failure_recovers_original_payment` Rust test uses a fresh official-client envelope from `HEDERA_TEST_ENVELOPE`, an explicitly funded testnet signer and `HEDERA_LIVE_TEST=testnet`. It injects a terminal-record write failure after consensus, waits for the actual lease to expire, reconstructs the provider, and verifies background recovery without re-signing or a new transaction ID. Run only deliberately: this test sends one tiny HBAR testnet payment.

`provision-testnet.mjs`, `associate-testnet.mjs` and `provision-test-ft.mjs` are bounded, explicit testnet provisioning tools. They are not x402 payment evidence and must not be rerun when a transaction outcome is unknown. Reconcile their printed original IDs first. Test account/token creation was completed on 2026-09-16; existing public account IDs and payment receipts are documented in [the native integration guide](../../docs/guides/hedera-native.md).
