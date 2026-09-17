# Changelog

## [2.36.1] - 2026-09-17

- Return HTTP 200 for authorized receipt lookups even when the original payment returned an HTTP error; preserve its signed status and the original POST response.
- Raise the Hedera testnet daily reservation ceiling to 12 HBAR for the expanded acceptance matrix; retain the mainnet ceiling of 10 HBAR.
- Document the required execution-role secret grant before deploying a new receipt signing key.

## [2.36.0] - 2026-09-17

- Add portable signed facilitator receipts for Arc exact USDC/EURC and Hedera USDC, including both mainnet and testnet.
- Bind receipts to purchase and authorization; preserve payment state independently of the merchant HTTP result.
- Persist and resume the original authorization, expose private receipt lookup, and verify Ed25519 provenance with trusted issuer keys.
- Document recovery limits, merchant propagation and the pending live EURC acceptance.
- Shuffle mainnet/testnet blockchain cards once per page load; filters and language changes retain that visit's order.
