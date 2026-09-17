# Native Hedera payments

**Current payment policy (2026-09-17, facilitator 2.35.0): native USDC only on both Hedera ledgers. HBAR is retained exclusively for sponsor network fees. New HBAR and custom HTS payment offers are rejected. Historical HBAR receipts and transaction records remain valid evidence of earlier releases.**

Hedera uses native `CryptoTransfer`, `exact`, and x402 v2. The network identifiers are `hedera:testnet` and `hedera:mainnet`. EVM chain IDs 295/296 and EIP-3009 are not this payment rail.

| Asset | Testnet | Mainnet | Decimals |
| --- | --- | --- | --- |
| Native USDC | `0.0.429274` | `0.0.456858` | 6 |

Only the network's native USDC token is admitted; additional-token configuration is disabled. NFTs, allowances, hooks, scheduled/batch transactions, custom token fees, escrow, upto, DX402 and ERC-8004 extensions are rejected before sponsorship.

## Merchant and client

Use the live `/supported` response to discover enabled networks and their **network-specific** `extra.feePayer`. The payer and merchant are native numeric account IDs; the fee payer is a separate facilitator account. The buyer signs the principal transfer, and the facilitator co-signs only after verification. The buyer and recipient must already have any HTS token associated, with applicable KYC granted and freeze/pause clear.

```json
{
  "scheme": "exact",
  "network": "hedera:testnet",
  "asset": "0.0.429274",
  "amount": "1000",
  "payTo": "0.0.YOUR_MERCHANT",
  "maxTimeoutSeconds": 180,
  "extra": { "feePayer": "0.0.FROM_SUPPORTED" }
}
```

Replace the placeholders with real account IDs. `amount` is an integer string: `1000` is **0.001 USDC**. HBAR is only the sponsor fee currency, not a payment option.

The official `@x402/hedera@2.26.0` client produces `{transaction: base64}`. Submit the v2 payload with its `accepted` requirements and identical outer `paymentRequirements`. Its default transaction duration is 120 seconds; merchant `maxTimeoutSeconds` must accommodate that (180 in the example). Every frozen node variant is inspected. Unknown protobuf operations and fields are rejected; valid explicit protobuf defaults emitted by the JavaScript SDK are accepted.

On success, `payer` is the buyer, `network` uses the native CAIP-2 identifier, and `transaction` is the original native transaction ID (`0.0.account@seconds.nanoseconds`). Native IDs are not EVM transaction hashes.

Retries return the original settlement. Merchants must make fulfillment idempotent using that transaction ID so a repeated HTTP request does not deliver the same purchase twice.

Use native USDC with the official client. HBAR and custom HTS offers are not admitted; retain spend controls. See [live-canary.mjs](../../tests/hedera-e2e/live-canary.mjs) for the official `x402Client` + `x402HTTPClient` handshake.

## Configuration

Build with feature `hedera`. The production Docker image and CI include it. Keep mainnet and testnet signing keys distinct.

```dotenv
HEDERA_ENABLED_TESTNET=true
HEDERA_ACCOUNT_ID_TESTNET=0.0.YOUR_FEE_PAYER
HEDERA_PRIVATE_KEY_TESTNET=<secret injection, never commit>
HEDERA_DAILY_BUDGET_TINYBARS_TESTNET=1000000000
HEDERA_SETTLEMENT_TABLE_NAME=facilitator-hedera-settlements
HEDERA_MAX_TRANSACTION_FEE_TINYBARS=100000000
HEDERA_SETTLEMENT_TIMEOUT_SECS=45
# Optional, per network:
# Additional token overrides are disabled; only native USDC is admitted.
# HEDERA_MIRROR_URL_TESTNET=https://testnet.mirrornode.hedera.com/
```

Use the corresponding `_MAINNET` settings to enable mainnet. Both Terraform enable flags default to false; the mainnet daily budget defaults to zero and must be explicitly set. Private keys and IDs are injected from dedicated AWS Secrets Manager JSON fields only for enabled networks. No native operator is installed on the Rust SDK client: all sponsor signatures are applied explicitly to inspected payment bytes.

The quota is shared by all replicas and charges the **maximum signed transaction fee**, once per admitted transaction, against a UTC-day budget. It is deliberately conservative and is not refunded when actual fees are lower. With the example 10-HBAR budget and a 1-HBAR signed fee, at most ten new payments can be admitted per UTC day. Choose an operational budget deliberately; this example is a canary budget, not a throughput promise.

## Recovery and operation

The dedicated DynamoDB table is required. Verification fails closed when admission state cannot be read. Settlement atomically reserves the ID, intent fingerprint and daily budget; it persists the exact co-signed bytes before transmission. A conditional 120-second lease coordinates replicas. Terminal results are immutable. Signed nonterminal records are indexed for recovery and retained seven days.

After an uncertain response, reconcile/retry **the same payload and transaction ID**. Do not create a new payment to replace an unknown outcome. The background worker claims expired leases and checks the original consensus receipt; Mirror evidence must match both the original ID and the hash of a persisted signed node variant. This authenticated HTTPS check is required before declaring an outcome because the pinned Rust SDK uses plaintext native gRPC. Mirror also recovers outcomes after consensus receipts expire. Resubmission uses the same bytes and never regenerates a transaction ID. An expired transaction without authoritative outcome stays uncertain; lack of a record is not proof of failure.

Set `HEDERA_ADMISSIONS_ENABLED_TESTNET=false` (or `_MAINNET=false`) to stop new admissions and hide that network from discovery while recovery remains active. Do not disable the provider entirely while unresolved transactions need recovery.

`/health/ready` includes native Hedera: storage, Mirror account/key/balance, and a bounded native consensus connectivity check. Consensus v0.77 removed `cryptoGetBalance`; the implementation uses free `AccountInfoQuery.get_cost` with explicit node IDs. Receipt queries also use explicit nodes to avoid the published Rust SDK's obsolete balance-based automatic ping. No paid balance query is used. ECS egress permits the native SDK consensus port TCP 50211 only when a Hedera network is enabled. The balance Lambda uses native numeric account IDs and converts tinybars with eight decimals.

Initial deployment prerequisites were applied from reviewed Terraform 1.9.8 plans with operator credentials: settlement storage, scoped task-role access, execution-role access to the two dedicated secrets, and outbound TCP 50211. CI cannot elevate its own IAM permissions or edit security-group rules. Future changes to those protected resources require the same operator procedure before deploying; ordinary Hedera mainnet activation reuses the two secret grants and consensus egress already provisioned.

## Validation and rollout status

[Testnet evidence](../reports/2026-09-16-hedera-testnet-canaries.json) records three confirmed native payments through the complete Rust server running locally: HBAR, USDC, and a four-decimal HTS fixture. Each completed HTTP 402 → signed official-client payment → verify/settle → HTTP 200, returned the same transaction on retry, and rejected replay verification. Mirror movements reconcile the exact principal and the sponsor-only fees.

Hedera testnet is now live at `https://facilitator.ultravioletadao.xyz` (initial deployment 2.32.0). [Public acceptance evidence](../reports/2026-09-16-hedera-public-canaries.json) records these additional payments through that HTTPS endpoint:

| Asset | Amount | Native transaction |
| --- | --- | --- |
| HBAR | 0.0001 HBAR | [0.0.10576385@1789609544.527025786](https://hashscan.io/testnet/transaction/0.0.10576385-1789609544-527025786) |
| USDC | 0.001 USDC | [0.0.10576385@1789609553.483480778](https://hashscan.io/testnet/transaction/0.0.10576385-1789609553-483480778) |

Both completed the official client flow, preserved the transaction on retry, rejected replay verification, and have immutable confirmed storage records. Independent receipt reconciliation verified the exact payer/payee principal, sponsor-only consensus fees, and the SHA-384 hash of the persisted signed bytes. The final-code crash-recovery test also passed for `0.0.10576385@1789608851.534483569` after an injected terminal-write outage.

Hedera mainnet is publicly deployed and verified since **2.33.0**. The dedicated fee payer is **`0.0.10868300`**, buyer **`0.0.10868301`**, and merchant **`0.0.10868302`**. The bootstrap **`0.0.10868282`** is a setup account, not the facilitator signer. Testnet uses fee payer **`0.0.10576385`**, buyer **`0.0.10576386`**, merchant **`0.0.10576387`**.

[Mainnet acceptance evidence](../reports/2026-09-16-hedera-mainnet-public-canaries.json) records both public payments:

| Asset | Amount | Native transaction |
| --- | --- | --- |
| HBAR | 0.0001 HBAR | [0.0.10868300@1789613986.642051223](https://hashscan.io/mainnet/transaction/0.0.10868300-1789613986-642051223) |
| USDC | 0.001 USDC | [0.0.10868300@1789614004.016143440](https://hashscan.io/mainnet/transaction/0.0.10868300-1789614004-016143440) |

Both completed HTTP 402 → official client signature → public verify/settle → HTTP 200. Retries returned the original ID, replay verification was rejected, DynamoDB records are confirmed, and independent SHA-384 reconciliation matched persisted signed bytes to successful Mirror receipts. The sponsor paid only consensus fees; the buyer paid exactly the advertised principal.

The official JavaScript client's bundled mainnet address book includes retired nodes. The canary resolves current node `0.0.3` from the trusted network's HTTPS Mirror address book before freezing; the facilitator still checks every signed node variant and rejects unknown/duplicate nodes. A failure during `/verify` created no payment or quota charge. This is documented in the harness rather than weakening verification.

See the [complete transaction log](../reports/2026-09-16-hedera-transaction-ledger.md) for funding, account creation, token associations, the bounded HBAR/USDC swap and every observed payment, including recovery tests. Receipts contain public evidence only; keys and co-signed transaction bytes are excluded.

Both networks use a conservative **10-HBAR daily reserved-fee budget**. At a 1-HBAR maximum signed fee this permits ten new settlements per network per UTC day, even though actual fees are smaller. Increase that budget deliberately before a higher-volume launch. Historical HBAR and USDC acceptance was completed in 2.33.1; current payments accept USDC only. Python **0.85.0** and TypeScript **2.93.0** are published and independently tested from clean PyPI/npm installs. [Eight SDK payment receipts](../reports/2026-09-16-hedera-sdk-release-acceptance.json) cover both assets on both networks. Arc remains supported. [Public web acceptance](../reports/2026-09-16-arc-hedera-public-web-acceptance.json) verifies landing account IDs, networks, OpenAPI and all ten OG surfaces.

See [the original integration plan](../plans/hedera-native-x402-integration-plan.md) and [test harness instructions](../../tests/hedera-e2e/README.md).


## Project SDKs

- [Python native Hedera guide](https://github.com/UltravioletaDAO/uvd-x402-sdk-python/blob/main/docs/networks/hedera.md): `pip install 'uvd-x402-sdk[hedera]==0.87.0'`, Python 3.10+ for signing; registry/builders remain available on 3.9.
- [TypeScript native Hedera guide](https://github.com/UltravioletaDAO/uvd-x402-sdk-typescript/blob/main/docs/networks/hedera.md): `npm install uvd-x402-sdk@2.95.0 @hiero-ledger/sdk@2.85.0`; server-side `HederaProvider`, not a HashPack browser connector.

Both implement ledger-bound offline DER signing, atomic USDC requirements,
the full accepted echo and HTTP 402 buyer retries. Merchant helpers compare the
buyer's offer with the server's own requirements. USD merchant pricing accepts
native USDC only; HBAR payment requests are rejected before signing.

Historical 0.85.0/2.93.0 release validation: 1,218 Python tests, 758 TypeScript tests, 430 cross-language
checks, eight production `/verify` preflights and eight actual payments from
published packages. The preflights were read-only; they are not listed as
on-chain payments. Each actual payment is bound to independently checked Mirror
consensus, exact principal and the persisted signed-transaction hash.
