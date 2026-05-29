# Handoff: Add XRP/XRPL support to the MoonPay headless CLI

**Date:** 2026-05-29
**Author:** Claude (Opus 4.8, 1M ctx) session — x402-rs XRPL integration
**For:** A fresh session (or 0xultravioleta's Day-1-at-MoonPay project, starting 2026-06-01)
**Status:** RESEARCH COMPLETE — verdict below

---

## TL;DR / Verdict

**External open-source contribution is NOT possible.** The `@moonpay/cli` (the headless / AI-agent framework, `mp`/`moonpay`, homepage agents.moonpay.com) is **proprietary, closed-source** — only a compiled npm tarball ships; there is no public source repo. The supported-token allowlist that excludes XRP lives **inside that compiled code**, so it cannot be edited via fork+PR.

**Realistic path = INTERNAL at MoonPay.** This is a clean "Day 1" project for 0xultravioleta once inside MoonPay. The on-ramp backend already sells XRP (moonpay.com/buy/xrp), so the smallest slice (on-ramp checkout) is genuinely small; full wallet support is larger (see §4).

> Why this matters to us: XRP/XRPL is becoming the **20th mainnet** of our x402 facilitator. The MoonPay CLI being able to `mp buy --token xrp` (and native XRPL `rlusd`) would let an agent self-fund the facilitator's XRP reserves + RLUSD/USDC trust lines end-to-end — exactly the automation we wanted but couldn't get today.

---

## 1. Open-source status (the load-bearing finding)

| Artifact | Status | Usable for XRP contribution? |
|---|---|---|
| `@moonpay/cli` (npm) | **License "Proprietary"**; no `repository`/`license` field; ships **minified** `dist/*.js` (e.g. `chunk-W4FTEJ7M.js`, 269 KB one-liner). No public source repo. | **No** — closed source |
| `moonpay/skills` (GitHub) | **MIT**, has CONTRIBUTING.md / CODEOWNERS / PR template. Open to external PRs. | **No** — it is a **NO-CODE** repo (instructional `SKILL.md` markdown only; CONTRIBUTING forbids SDK/TS/Python code). Documenting `mp buy --token xrp` before the CLI supports it = hallucinated command (auto-reject). |
| `moonpay/moonpay-sign` | Next.js sample signature endpoint, "not production-ready" | No — irrelevant to chains |
| `moonpay/moonpay-demo-integrations` | Widget demos | No — irrelevant |

---

## 2. Where the gap actually is

The `mp buy --token` allowlist is a **Zod enum in the compiled CLI** (`dist/chunk-W4FTEJ7M.js`):

```
btc, eth, eth_arbitrum, eth_base, eth_optimism, eth_polygon, pol_polygon,
pyusd, pyusd_sol, rlusd, sol, trx, usdc, usdc_arbitrum, usdc_base,
usdc_cchain, usdc_optimism, usdc_polygon, usdc_sol, usdt, usdt_arbitrum,
usdt_bsc, usdt_optimism, usdt_polygon, usdt_sol, usdt_ton, usdt_trx,
usd1, usd1_bsc, xo_sol
```

- **`xrp` is ABSENT** (0 whole-word hits across the entire dist). No `xrpl`/`ripple` lib in `package.json` deps (deps are `viem`, `@solana/web3.js`, `bitcoinjs-lib`, `@ton/*` — no XRP library).
- **`rlusd` IS present but mapped to Ethereum** (help: "ethereum: eth, pyusd, rlusd…") — i.e. the ETH-issued RLUSD, **not** the XRPL-native RLUSD we care about.
- Wallet chain map is `{Solana, Ethereum, Polygon, Base, Arbitrum}` — **XRPL is not a modeled wallet chain**.
- **The on-ramp backend DOES support XRP** (moonpay.com/buy/xrp sells native XRP; moonpay.com/buy/rlusd sells XRPL-native RLUSD). So the omission is a **CLI-side gap in closed code**, not a backend limitation.

---

## 3. Why XRP is probably omitted on purpose (the real engineering work)

It is NOT just a one-line enum addition. XRPL needs primitives the CLI's wallet layer doesn't model:
- **Destination tags** (routing for custodial destinations; optional for self-custody r-addresses but the model must allow them).
- **Trust lines** (`TrustSet`) — required to hold ANY issued token, including XRPL-native RLUSD and Circle USDC. Each consumes account reserve.
- **An XRPL signer** — ed25519/secp256k1 + XRPL binary codec. Only ETH/Solana/BTC/TON signing kits are bundled today.
- **Account activation / base reserve** semantics.
- **X-addresses** vs classic r-addresses.

This is exactly why the CLI even routes `rlusd` through Ethereum — to avoid XRPL wallet mechanics.

---

## 4. Internal implementation plan (for inside MoonPay)

Smallest-slice-first:

1. **On-ramp only (smallest, highest value for us).** Add `"xrp"` (and an XRPL-native `rlusd`/`usdc` variant) to the `mp buy --token` Zod enum + help text + the currencyCode mapping passed to the on-ramp API. Because the on-ramp backend already supports XRP, `mp buy --token xrp --wallet r... --amount …` returning a checkout URL is the minimal, shippable change. **No XRPL signer needed for buy-to-self-custody** (funds are delivered by MoonPay; the CLI just builds the signed checkout URL).
2. **Wallet receive/balance.** Add `XRPL` to the blockchain→address map so the CLI can hold/show an XRP address.
3. **Full wallet/sign/swap.** Bundle an XRPL signing module; implement `TrustSet` (trust lines), `Payment`, destination-tag handling, and DEX `OfferCreate` (for XRP↔RLUSD/USDC swaps). This unlocks the full "agent self-funds reserves + sets trust lines + swaps to USDC" story.

Validate each slice against MoonPay's internal CLI test suite. Phase 1 alone gives us the tweet-worthy "self-fund XRP via MoonPay CLI" capability.

---

## 5. Contribution mechanics

- **Internal:** the change is in MoonPay's private CLI source (the real source behind `dist/chunk-W4FTEJ7M.js`). Needs MoonPay employee repo access (→ 0xultravioleta, June 1).
- **Downstream open contribution (later):** once `mp buy --token xrp` works in a released CLI version, a `moonpay/skills` PR adding e.g. `skills/ultravioleta-x402-xrpl-funding/SKILL.md` (documenting the agent funding flow) becomes legitimate and non-hallucinated. That IS an external MIT PR we could make afterward. PR flow: copy `template/`, register in `.claude-plugin/marketplace.json`, open via PR template; reviewed against an A+-only rubric; security via HackerOne; no CLA file found.

---

## 6. Our-side context a fresh session needs

- Our XRPL facilitator integration plan: `docs/plans/xrpl-native-x402-integration-plan.md`
- Provisioning state (logo, wallets, MoonPay reality): memory `project-xrpl-provisioning.md`
- Facilitator XRPL addresses (PUBLIC; seeds in AWS Secrets us-east-2):
  - mainnet `rfADKkVXBNqK3z72tVSS3LVzAR3psYkonp` (`facilitator-xrpl-keypair-mainnet`)
  - testnet `rGhTioKAFHe75KgVnQtacRiKFuPv28Wbwk` (`facilitator-xrpl-keypair-testnet`, faucet-funded)
- What we need from MoonPay CLI: `mp buy --token xrp --wallet <r-addr>` (reserves) and XRPL-native `rlusd`/`usdc`, plus (phase 3) trust-line + DEX-swap so an agent can fully self-provision.

---

## 7. Starting prompt for the new session

> "Read `docs/handoffs/2026-05-29-moonpay-cli-xrp-contribution.md`. I now have access to MoonPay's internal CLI source. Implement Phase 1 (add `xrp` + XRPL-native `rlusd`/`usdc` to the `mp buy` token enum + currencyCode mapping + help text), since the on-ramp backend already supports XRP. Then scope Phase 3 (XRPL signer + trust lines + DEX swap). Validate against the internal CLI test suite."

**Bottom line:** Not an open-source PR — an internal MoonPay project. The handoff above is the design brief to walk in with on Day 1.
