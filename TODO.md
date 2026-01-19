# x402-rs Implementation TODO

> Auto-generated from control-plane brainstorming backlog on 2026-01-19.
> Sources: from_superfluid_x402_partnership, from_x402cloud, session_voice2earn_ecosystem, from_erc8004, session_meritstream

---

## Context

x402-rs is the core Rust implementation of the x402 payment protocol. Key integrations needed:
- Superfluid streaming payments
- ERC-8004 agent identity
- Voice2Earn real-time payments
- x402cloud serverless execution

---

## P0 (CRITICAL - This Week)

### 1. Integrate Superfluid as streaming backend
**Priority**: P0
**Status**: [ ] Not started
**Location**: New module `src/streaming/`

- Add Superfluid SDK dependency
- Dual mode: pay-per-request OR stream via Superfluid
- 8 networks overlap: Ethereum, Base, Arbitrum, Optimism, Polygon, Avalanche, Celo, BSC
- Script that reads `src/network.rs` as golden source for network/stablecoin matrix

**Why**: Enables continuous payment flows for Voice2Earn, MeritStream

---

### 2. Add ERC-8004 AGID payment identity
**Priority**: P0
**Status**: [ ] Not started
**Location**: `src/identity/`

- Payment processors/gateways register as AGID
- Reputation based on successful transactions + dispute resolution rate
- Validators can attest compliance
- Enables trustless A2A payments

**Why**: Foundation for agent economy payments

---

### 3. Implement x402cloud basic endpoint (S3 PUT)
**Priority**: P0
**Status**: [ ] Not started
**Location**: New crate `x402-cloud/`

- Proof-of-concept: S3 PUT → x402 payment required → AWS execution → response
- Validates full payment→execution→response flow
- Critical for "Serverless as a Service" dogfooding

**Why**: Proves x402 can gate cloud resources

---

## P1 (High Priority - This Month)

### 4. Voice2Earn real-time payment integration
**Priority**: P1
**Status**: [ ] Not started
**Location**: Integration with v2e-live

- Connect v2e-live scoring → x402 instant payments
- OR Superfluid streaming at flow_rate = SYNERGY_SCORE * BASE_RATE
- Per "moment of value" micropayments

**Why**: Core use case for Voice2Earn monetization

---

### 5. Quest IRC bounty bot prototype
**Priority**: P1
**Status**: [ ] Not started
**Location**: New example `examples/irc-bounty-bot/`

- IRC bot posts bounties on meshrelay channels
- Pays correct answerers via x402
- Flow: quest posted → claim → solve → verify → x402 payment

**Why**: Tests colmena-style distributed work + x402 settlement

---

### 6. ERC-8004 Agent Card integration
**Priority**: P1
**Status**: [ ] Not started
**Location**: `src/agent_card/`

- Auto-generate Agent Card for x402 services
- Declare OASF skills (payment-processing, escrow)
- Bidirectional registration linking to on-chain AGID NFT

**Why**: Agent discoverability in ecosystem

---

## P2 (Medium Priority - This Quarter)

### 7. Research Layer 1 vs Layer 2 priority
**Priority**: P2
**Status**: [ ] Research
**Location**: Documentation

- Superfluid has 8-network overlap with UVD
- UVD has unique advantage on 9 Superfluid-less networks (Solana, Sui, etc.)
- Consider adding Scroll (USDC via EIP-3009)

---

### 8. Implement payment mixing for Private Task Markets
**Priority**: P2
**Status**: [ ] Not started
**Location**: `src/privacy/`

- Escrow patterns for private tasks
- Output amounts different from input (mixer)
- Streaming payments during task execution

**Why**: Required for Private Task Markets idea

---

## Done

*Move completed items here with date.*
