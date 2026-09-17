# Frontend capability follow-up — 2026-09-17

## Problem and changes

The landing used a manual token list that omitted XRPL entirely and advertised AUSD on Sui, despite the Sui provider only processing USDC. Several native providers omitted token metadata from `/supported`. The network table discarded CAIP-2-only rows except Hedera, and its shared icon map still used placeholders for Arc, Hedera and RLUSD.

- Landing badges now come from the running facilitator's `exact` capabilities. Explicit `networkAliases` merge v1/v2 entries without discarding native or v2-only networks. Native gas balances remain separate from stablecoin badges.
- XRPL mainnet and testnet show USDC and RLUSD. The missing testnet card and balance fallback are included. Native XRP remains an existing backend capability; this change does not remove it.
- Native providers publish their registered payment assets. Sui advertises only USDC; BSC advertises AUSD for `exact`, excluding the registered USDC deployment that lacks ERC-3009. Stellar USDC correctly reports seven decimals.
- Arc and Hedera use the supplied network images on both pages; RLUSD uses its existing image. Shared script URLs invalidate the previous cached icon map.
- Language changes also update token labels. Numeric zero is a valid balance, missing catalog data is explicit, and an API outage does not leave Hedera stuck loading. Tab clicks no longer rely on the browser's global `event`.
- Stablecoin buttons filter the card grid by the same live `exact` metadata. One selection persists across mainnet/testnet and language changes; All or a second click clears it. Empty results are explicit, keyboard interaction uses native buttons, and `aria-pressed` exposes selection. All visible token logos are 32px, with CSS compensating for the six assets' transparent margins to match RLUSD.
- The Markdown landing and generated agent context include EURC on both Arc networks. README payment tables distinguish deployed assets from usable payment capabilities.
- Hedera unsupported-asset admission now uses the typed `invalid_asset` error. In 2.35.0 the public response was HTTP 400 with `internal_error`; historical settlement recovery remains intact.

## Validation before Actions

Run `bash scripts/preflight.sh` in a Linux environment with the dependencies listed at its top. It runs the same landing, native balance, all-feature Rust build, facilitator tests and workspace/doctest checks as CI, plus the new frontend regressions. `python scripts/ci_paths_selftest.py` checks workflow path filters separately (PyYAML required).

For this release, the preflight uses a local Docker container and dedicated Cargo caches. No AWS credentials or production signing keys are mounted. Browser checks additionally cover desktop/mobile, EN/ES, XRPL badges on both ledgers, alias-only catalogs, missing metadata, numeric zero and failed APIs. Local browser fixtures are separate from subsequent production acceptance.

Run `python scripts/verify_frontend_browser.py` for those browser regressions. It requires Python Playwright and Chrome/Chromium (or Playwright's installed Chromium); `CHROME_EXECUTABLE` can select a browser. The committed fixture contains only public discovery metadata. All external browser requests are blocked, and screenshots/results go into ignored `.unused/`. This local check does not add browser installation time to GitHub Actions.

The earlier failed run `35241072014` caught two untranslated HBAR balance labels. They were corrected in `4966757cb9ae8680928c0a3e8f414b20b365b4c4`; both the following PR run and production deployment succeeded. Future changes should complete the local preflight before a push that triggers the build workflow. Local checks reduce avoidable failures; infrastructure or runner failures can still occur.

## Scope retained

Receipt issuance and durable purchase idempotency remain in the [master plan](../plans/facilitator-receipts-master-plan.md), not implemented by this frontend fix. Paid EURC acceptance on Arc is still pending by user instruction. This follow-up submits no payments, swaps or settlements.

## Production closure

Facilitator **2.35.1** is deployed and publicly verified. [Release evidence](../reports/2026-09-17-frontend-capabilities-release.json) records the passing PR/deployment runs, local validation and public acceptance. The live catalog includes XRPL USDC/RLUSD on both ledgers, native token metadata, Arc EURC, and the BSC/Sui corrections. Production browser checks cover filters, keyboard, EN/ES and mobile. HBAR `/verify` now returns `invalid_asset` on both Hedera ledgers. No new payments, swaps or settlements were submitted.
