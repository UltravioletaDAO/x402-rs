# Zama Developer Program — Application (ready to submit)

**Target track:** Bounty Track (primary) · Builder Track (secondary)
**Prepared:** 2026-08-31
**Status:** DRAFT READY — needs the `[FILL]` fields and the confirmed form URL before sending
**Language:** English, because that is the language of the program.

> **Before submitting, do these three things:**
>
> 1. Get the real Bounty/Builder Track form URL. `zama.org/developer-program`
>    and `zama.org/programs` both 404 as of 2026-08-31. Ask **developer@zama.org**
>    or check the developer hub. Do not guess a URL.
> 2. Fill every `[FILL: ...]` marker below.
> 3. Re-verify the numbers in "Traction" — they came from a live read on
>    2026-08-31 and they move. Commands are in `00-RESEARCH.md` §5.
>
> **Do not add claims that are not in this document.** Everything here was
> verified against the running service. In particular: the FHE path has **no
> real traffic yet** and the application says so on purpose. Getting caught
> inflating it costs more than the grant is worth.

---

## Submission fields

**Project name:** Confidential x402 — FHE payment settlement for the HTTP 402 agent economy

**Team / organization:** Ultravioleta DAO

**Contact:** `[FILL: email]` · `[FILL: Telegram or Discord handle]`

**X / Twitter:** `[FILL: @handle]` (tag @zama and #ZamaDeveloperProgram when posting about it)

**Repository:** https://github.com/UltravioletaDAO/x402-rs (Apache 2.0)

**Live service:** https://facilitator.ultravioletadao.xyz

**FHE service:** https://zama-facilitator.ultravioletadao.xyz

**Reward address (Ethereum mainnet):** `[FILL: public receiving address — never a key]`

**Track:** Bounty Track — infrastructure integration

---

## One-paragraph summary

x402 is the HTTP 402 standard that AI agents are adopting to pay for APIs: the
server answers `402 Payment Required` with its price, the client signs a payment
authorization, and a facilitator verifies and settles it on-chain. Every one of
those payments is public — amount, payer, payee. We run a production x402
facilitator across 21 mainnets and 7 chain families, and we have already added a
`fhe-transfer` scheme on top of ERC7984 that routes confidential payments to the
Zama Protocol. It is live on Sepolia today. **We are applying for support to take
it to Ethereum mainnet and get the first real confidential x402 payment
settled.**

---

## The problem

An AI agent that consumes paid APIs makes hundreds of micropayments a day. Under
plain x402, each one writes to a public chain: *this agent paid this provider
this amount at this time*. Anyone can reconstruct the agent's operating budget,
its vendor list, its usage patterns and — by differencing what it charges its own
users — its margin.

This is not a hypothetical privacy nicety. It is the reason serious commercial
operators will not put their agent spend on a public rail. The x402 ecosystem is
building the payment layer for agents and shipping it with its books open.

Confidentiality here cannot be a separate, opt-in product that the buyer has to
discover. It has to be *another scheme on the same rail*, so a seller turns it on
by changing one field of the `402` response and every existing x402 client keeps
working.

---

## What we already built

A working `fhe-transfer` scheme inside a production x402 facilitator, wired to
the Zama Protocol.

### Architecture

```
  x402 client
      |
      |  POST /verify | /settle
      v
  facilitator.ultravioletadao.xyz          (Rust / Axum, AWS ECS Fargate)
      |
      +-- scheme "exact"          --> local settlement, 21 mainnets, 7 families
      |
      +-- scheme "fhe-transfer"   --> FHE proxy  (src/fhe_proxy.rs)
                                          |
                                          v
                             zama-facilitator.ultravioletadao.xyz
                             (AWS Lambda + API Gateway)
                                  - TFHE WASM runtime
                                  - Zama KMS / relayer integration
                                  - ERC7984 confidential token verification
                                          |
                                          v
                                  Zama Protocol on Ethereum Sepolia
```

### Design decisions worth defending

**The FHE scheme is a peer of `exact`, not a fork of the service.** A seller
advertises `"scheme": "fhe-transfer"` in its `402` response and nothing else in
the stack changes. Clients that only speak `exact` are unaffected — they never
see the new kind.

**Scheme detection happens before type deserialization.** FHE payloads carry
encrypted handles and proofs, which do not fit the shape of an EIP-3009
authorization. `src/handlers.rs:2440` reads the raw `scheme` field first and
routes on it, so the FHE payload never has to be squeezed into the `exact` type.
Commit `4122cd23` is the fix that established this.

**The FHE runtime is isolated in Lambda.** The TFHE WASM runtime and the KMS
integration have a different dependency tree, a different failure mode and a
different latency profile from the rest of the facilitator. Putting them behind
an HTTP boundary means an FHE cold start or a relayer stall cannot degrade
settlement on the other 21 chains. The proxy allows a 90s timeout precisely
because relayer-side decryption is slower than an ordinary RPC call.

**Confidentiality is advertised through the standard discovery path.**
`GET /supported` lists the FHE scheme in both x402 v1 (`ethereum-sepolia`) and
v2 CAIP-2 (`eip155:11155111`) form, so a client discovers it the same way it
discovers every other capability.

### Verify it yourself

```bash
# the FHE scheme is advertised in production
curl -s https://facilitator.ultravioletadao.xyz/supported \
  | jq '[.kinds[] | select(.scheme=="fhe-transfer")]'
# -> [{"x402Version":1,"scheme":"fhe-transfer","network":"ethereum-sepolia"},
#     {"x402Version":2,"scheme":"fhe-transfer","network":"eip155:11155111"}]

# the FHE service is up
curl -s https://zama-facilitator.ultravioletadao.xyz/health
# -> {"status":"ok","service":"x402-facilitator","version":"1.0.0",
#     "networks":["fhevm-local","sepolia"]}
```

Code: [`src/fhe_proxy.rs`](https://github.com/UltravioletaDAO/x402-rs/blob/main/src/fhe_proxy.rs) ·
routing at `src/handlers.rs:2440` (verify) and `:3111` (settle) ·
scheme at `src/types.rs:108` ·
docs at [`docs/ZAMA_FHE_INTEGRATION.md`](https://github.com/UltravioletaDAO/x402-rs/blob/main/docs/ZAMA_FHE_INTEGRATION.md).

---

## Traction, stated honestly

The facilitator itself is a real production service, not a demo:

- **21 payment mainnets** across **7 chain families** — EVM, Solana/SVM, NEAR, Stellar, Sui, Algorand and XRPL
- **6 stablecoins** — USDC, USDT, EURC, AUSD, PYUSD, USDG
- **2,306 successful settlement operations recorded** across 13 networks with activity (plus escrow on 9 mainnets and ERC-8004 agent identity on 12)
- Running v2.0.0 on AWS ECS Fargate with CI-gated deploys

*(Those counts come from `GET /api/stats`, which is a fire-and-forget index and
not a ledger — the chain is. Figures read on 2026-08-31.)*

**And the part we are not going to dress up: the FHE path has no real traffic
yet.** It is deployed, reachable and correct end-to-end on Sepolia, but no
integrator has pushed a live confidential payment through it. Closing that gap
is exactly what this application is for.

---

## What we will deliver

### Milestone 1 — Confidential x402 on Ethereum mainnet

Move `fhe-transfer` from Sepolia to Ethereum mainnet, where the Zama Protocol has
been live since December 2025.

- Add mainnet network config to the FHE service (gateway, KMS verifier, ACL contract, relayer)
- Register `fhe-transfer` for `ethereum` / `eip155:1` in `/supported`
- Support a real ERC7984 confidential token (cUSDT / cUSDC) as a payable asset
- Fund and operate a mainnet settlement wallet
- **Acceptance:** one real confidential payment verified and settled on Ethereum mainnet, with the transaction hash published

### Milestone 2 — Make it adoptable by someone who is not us

- A `fhe-transfer` client path in the x402 SDKs we maintain, so a buyer pays confidentially without hand-assembling encrypted payloads
- An end-to-end reference: a paywalled API where the price, the payer and the amount are encrypted, and the seller still gets a settlement it can verify
- An integration guide written for x402 developers who have never touched FHE — the audience Zama's Bounty Track exists to serve

### Milestone 3 — Push it upstream

Propose `fhe-transfer` as a scheme to the x402 Foundation. The Foundation
requires a reviewed PR and discards proposals with no production usage behind
them, which is why milestones 1 and 2 come first. A scheme accepted upstream
means every x402 facilitator can offer confidential settlement — that is the
outcome worth aiming at, not one more private integration.

**Timeline:** `[FILL: realistic weeks per milestone — do not promise what the calendar does not hold]`

**Requested grant:** `[FILL: amount, within the up-to-$5k monthly band]`

---

## Why fund this rather than another confidential dApp

A confidential dApp proves FHE works for one use case. This puts confidentiality
into the payment rail that AI agents are standardizing on, at the layer where
every one of them already passes through.

Zama has confidential tokens, confidential yield and confidential payroll live on
mainnet. What it does not yet have is a confidential way to *charge for an API
call* — the single most common transaction an autonomous agent makes. The x402
ecosystem is building that rail right now, in public, and today it has no privacy
story at all.

We are already on both sides of that gap: a production x402 facilitator on 21
chains, and a working Zama integration. We are asking for support to close it.

---

## Team

Ultravioleta DAO — `[FILL: 2–3 lines. What the DAO builds, how long the
facilitator has been in production, any relevant prior work. Keep it factual.]`

**Prior work in this repo:** multi-chain x402 settlement across 7 chain families,
ERC-8004 trustless-agent identity and reputation on 12 mainnets, an escrow scheme
with on-chain refunds, and a durable-evidence extension that seals paid responses
to the payer's own public key.

---

## Links

- Facilitator: https://facilitator.ultravioletadao.xyz
- FHE service health: https://zama-facilitator.ultravioletadao.xyz/health
- Repo: https://github.com/UltravioletaDAO/x402-rs
- FHE integration doc: `docs/ZAMA_FHE_INTEGRATION.md`
- x402 protocol: https://x402.org
- ERC-7984: https://eips.ethereum.org/EIPS/eip-7984

---

## Appendix — short version for a form with a character limit

> We run a production x402 payment facilitator (HTTP 402 for AI agents) across 21
> mainnets and 7 chain families, and we have built a `fhe-transfer` scheme on top
> of ERC7984 that routes confidential payments to the Zama Protocol. It is live on
> Sepolia today: `GET /supported` advertises it and the FHE service settles
> against Zama's KMS. Every x402 payment currently publishes amount, payer and
> payee in the clear, which exposes an agent's budget, vendors and margin. We want
> to take the FHE scheme to Ethereum mainnet with a real ERC7984 stablecoin, ship
> client SDK support and a developer guide, and then propose `fhe-transfer` as a
> standard scheme to the x402 Foundation — so confidential settlement becomes
> available to every facilitator, not just ours. The integration exists and is
> deployed; it has no live traffic yet, and that is the gap this grant closes.
> Repo: github.com/UltravioletaDAO/x402-rs (Apache 2.0).
