# Native Hedera payments

Hedera uses native `CryptoTransfer`, `exact`, and x402 v2. The network identifiers are `hedera:testnet` and `hedera:mainnet`. EVM chain IDs 295/296 and EIP-3009 are not this payment rail.

| Asset | Testnet | Mainnet | Decimals |
| --- | --- | --- | --- |
| HBAR | `0.0.0` | `0.0.0` | 8 |
| Native USDC | `0.0.429274` | `0.0.456858` | 6 |

Additional fungible HTS tokens require an explicit `token-id:decimals` allowlist and matching Mirror metadata. NFTs, allowances, hooks, scheduled/batch transactions, custom token fees, escrow, upto, DX402 and ERC-8004 extensions are rejected before sponsorship.

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

Replace the placeholders with real account IDs. `amount` is an integer string: `1000` is **0.001 USDC**, while `10000` tinybars is **0.0001 HBAR**. HBAR volume is never USD volume.

The official `@x402/hedera@2.26.0` client produces `{transaction: base64}`. Submit the v2 payload with its `accepted` requirements and identical outer `paymentRequirements`. Its default transaction duration is 120 seconds; merchant `maxTimeoutSeconds` must accommodate that (180 in the example). Every frozen node variant is inspected. Unknown protobuf operations and fields are rejected; valid explicit protobuf defaults emitted by the JavaScript SDK are accepted.

On success, `payer` is the buyer, `network` uses the native CAIP-2 identifier, and `transaction` is the original native transaction ID (`0.0.account@seconds.nanoseconds`). Native IDs are not EVM transaction hashes.

Retries return the original settlement. Merchants must make fulfillment idempotent using that transaction ID so a repeated HTTP request does not deliver the same purchase twice.

HBAR is not a default USD asset in the official client. Add an explicit, bounded `spendControls.allowedAssets` entry for HBAR or a custom FT; do not disable spend controls globally. See [live-canary.mjs](../../tests/hedera-e2e/live-canary.mjs) for the official `x402Client` + `x402HTTPClient` handshake.

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
HEDERA_ADDITIONAL_TOKENS_TESTNET=0.0.YOUR_FT:4
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

Mainnet provisioning is complete: the dedicated fee payer `0.0.10868300` was funded with 30 HBAR, and test buyer `0.0.10868301` and recipient `0.0.10868302` are associated with native USDC `0.0.456858`. Production configuration enables mainnet with a conservative 10-HBAR daily reserved-fee budget. Confirm rollout via `/supported`; public mainnet HBAR/USDC acceptance payments are still required. The bootstrap account `0.0.10868282` is not the facilitator signer. The project's own SDK signing support is a separate follow-up after facilitator completion.

See [the original integration plan](../plans/hedera-native-x402-integration-plan.md) and [test harness instructions](../../tests/hedera-e2e/README.md).
