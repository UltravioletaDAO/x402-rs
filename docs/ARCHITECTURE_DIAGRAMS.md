# x402-rs Architecture Diagrams

**Version**: 1.7.7 | **Purpose**: Visual reference for system architecture

---

## 📐 System Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                        CLIENT (Browser/SDK)                         │
│  - x402-reqwest (Rust)                                              │
│  - x402-js (TypeScript)                                             │
│  - Custom HTTP client                                               │
└────────────────────────┬────────────────────────────────────────────┘
                         │ HTTP/HTTPS
                         │ POST /verify, POST /settle
                         ▼
┌─────────────────────────────────────────────────────────────────────┐
│              AWS APPLICATION LOAD BALANCER (HTTPS)                  │
│  - TLS termination                                                  │
│  - Health checks                                                    │
│  - Request routing                                                  │
└────────────────────────┬────────────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────────────────┐
│              AWS ECS FARGATE (facilitator-production)               │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │            x402-rs Facilitator (Docker Container)             │  │
│  │                                                               │  │
│  │  ┌─────────────────────────────────────────────────────────┐ │  │
│  │  │  Axum HTTP Server (main.rs)                             │ │  │
│  │  │  - Routes: /verify, /settle, /health, /supported        │ │  │
│  │  │  - OpenTelemetry tracing                                │ │  │
│  │  │  - CORS support                                         │ │  │
│  │  │  - Graceful shutdown (SigDown)                          │ │  │
│  │  └────────────────────┬────────────────────────────────────┘ │  │
│  │                       │                                       │  │
│  │                       ▼                                       │  │
│  │  ┌─────────────────────────────────────────────────────────┐ │  │
│  │  │  FacilitatorLocal (facilitator_local.rs)                │ │  │
│  │  │  - Compliance screening (OFAC + blacklist)              │ │  │
│  │  │  - Provider routing (by network)                        │ │  │
│  │  │  - Error handling and logging                           │ │  │
│  │  └────────────────────┬────────────────────────────────────┘ │  │
│  │                       │                                       │  │
│  │       ┌───────────────┼───────────────┐                       │  │
│  │       ▼               ▼               ▼                       │  │
│  │  ┌─────────┐   ┌──────────┐   ┌──────────┐   ┌──────────┐   │  │
│  │  │   EVM   │   │  Solana  │   │   NEAR   │   │ Stellar  │   │  │
│  │  │Provider │   │ Provider │   │ Provider │   │ Provider │   │  │
│  │  │ (23 nw) │   │ (3 nw)   │   │ (2 nw)   │   │ (2 nw)   │   │  │
│  │  └─────────┘   └──────────┘   └──────────┘   └──────────┘   │  │
│  │       │              │              │              │          │  │
│  └───────┼──────────────┼──────────────┼──────────────┼──────────┘  │
│          │              │              │              │             │
└──────────┼──────────────┼──────────────┼──────────────┼─────────────┘
           │              │              │              │
           ▼              ▼              ▼              ▼
  ┌────────────┐  ┌────────────┐  ┌────────────┐  ┌────────────┐
  │ EVM RPC    │  │ Solana RPC │  │ NEAR RPC   │  │Stellar RPC │
  │ Endpoints  │  │ Endpoints  │  │ Endpoints  │  │ Endpoints  │
  │ (Base,     │  │ (Solana,   │  │ (NEAR,     │  │ (Stellar,  │
  │ Avalanche, │  │ Fogo)      │  │ testnet)   │  │ testnet)   │
  │ Polygon,   │  │            │  │            │  │            │
  │ etc.)      │  │            │  │            │  │            │
  └────────────┘  └────────────┘  └────────────┘  └────────────┘
           │              │              │              │
           ▼              ▼              ▼              ▼
  ┌────────────┐  ┌────────────┐  ┌────────────┐  ┌────────────┐
  │ Blockchain │  │ Blockchain │  │ Blockchain │  │ Blockchain │
  │ Networks   │  │ Networks   │  │ Networks   │  │ Networks   │
  └────────────┘  └────────────┘  └────────────┘  └────────────┘
```

---

## 🔄 Payment Verification Flow

```
┌─────────┐                                                     ┌─────────┐
│ Client  │                                                     │Blockchain│
└────┬────┘                                                     └────┬────┘
     │                                                               │
     │ 1. POST /verify (PaymentPayload + Requirements)              │
     ├──────────────────────────────────────────────────────────┐   │
     │                                                           │   │
     │                        ┌─────────────────────────┐        │   │
     │                        │  Axum Handler           │        │   │
     │                        │  (handlers::post_verify)│        │   │
     │                        └───────────┬─────────────┘        │   │
     │                                    │                      │   │
     │                        ┌───────────▼─────────────┐        │   │
     │                        │  FacilitatorLocal       │        │   │
     │                        │  ::verify()             │        │   │
     │                        └───────────┬─────────────┘        │   │
     │                                    │                      │   │
     │                        ┌───────────▼─────────────┐        │   │
     │                        │  Compliance Screening   │        │   │
     │                        │  (OFAC + Blacklist)     │        │   │
     │                        └───────────┬─────────────┘        │   │
     │                                    │                      │   │
     │                        ┌───────────▼─────────────┐        │   │
     │                        │  ProviderMap            │        │   │
     │                        │  ::by_network()         │        │   │
     │                        └───────────┬─────────────┘        │   │
     │                                    │                      │   │
     │                   ┌────────────────┴────────────────┐     │   │
     │                   │                                 │     │   │
     │          ┌────────▼────────┐              ┌────────▼────────┐ │
     │          │ EvmProvider     │              │ SolanaProvider  │ │
     │          │ ::verify()      │              │ ::verify()      │ │
     │          └────────┬────────┘              └────────┬────────┘ │
     │                   │                                 │     │   │
     │     ┌─────────────┼─────────────────────────────────┘     │   │
     │     │             │                                       │   │
     │     │ 2. Parse payload (ExactEvmPayload)                 │   │
     │     │ 3. Validate timing (validAfter <= now < validBefore)│   │
     │     │ 4. Check receiver matches requirements             │   │
     │     │ 5. Simulate transfer (eth_call)                    │   │
     │     │             │                                       │   │
     │     │             ├───────────────────────────────────────────┤
     │     │             │ 6. RPC: eth_call (MULTICALL3 simulate)│   │
     │     │             │ ──────────────────────────────────────────▶
     │     │             │                                       │   │
     │     │             │ 7. Signature verification            │   │
     │     │             │    (Universal Validator 0xdAcD...)   │   │
     │     │             │ ◀──────────────────────────────────────────
     │     │             │                                       │   │
     │     │ 8. Return VerifyResponse::Valid{payer} or Invalid  │   │
     │     └─────────────┼─────────────────────────────────────┐ │   │
     │                   │                                     │ │   │
     │                   ▼                                     │ │   │
     │         ┌──────────────────┐                            │ │   │
     │         │ JSON Response    │                            │ │   │
     │         │ {                │                            │ │   │
     │         │   isValid: true, │                            │ │   │
     │         │   payer: "0x..." │                            │ │   │
     │         │ }                │                            │ │   │
     │         └──────────────────┘                            │ │   │
     │◀─────────────────────────────────────────────────────────┘ │   │
     │                                                           │   │
```

---

## ⚙️ Payment Settlement Flow

```
┌─────────┐                                                     ┌─────────┐
│ Client  │                                                     │Blockchain│
└────┬────┘                                                     └────┬────┘
     │                                                               │
     │ 1. POST /settle (PaymentPayload + Requirements)              │
     ├──────────────────────────────────────────────────────────┐   │
     │                                                           │   │
     │                        ┌─────────────────────────┐        │   │
     │                        │  Axum Handler           │        │   │
     │                        │  (handlers::post_settle)│        │   │
     │                        └───────────┬─────────────┘        │   │
     │                                    │                      │   │
     │                        ┌───────────▼─────────────┐        │   │
     │                        │  FacilitatorLocal       │        │   │
     │                        │  ::settle()             │        │   │
     │                        └───────────┬─────────────┘        │   │
     │                                    │                      │   │
     │                        ┌───────────▼─────────────┐        │   │
     │                        │  RE-SCREEN Compliance   │        │   │
     │                        │  ⚠️ CRITICAL SECURITY   │        │   │
     │                        │  (Never trust prior     │        │   │
     │                        │   verify call)          │        │   │
     │                        └───────────┬─────────────┘        │   │
     │                                    │                      │   │
     │                        ┌───────────▼─────────────┐        │   │
     │                        │  ProviderMap            │        │   │
     │                        │  ::by_network()         │        │   │
     │                        └───────────┬─────────────┘        │   │
     │                                    │                      │   │
     │                        ┌───────────▼─────────────┐        │   │
     │                        │  EvmProvider::settle()  │        │   │
     │                        └───────────┬─────────────┘        │   │
     │                                    │                      │   │
     │     ┌──────────────────────────────┘                      │   │
     │     │ 2. Re-verify payment (duplicate all checks)         │   │
     │     │ 3. Get next signer wallet (round-robin)             │   │
     │     │ 4. Get nonce (from PendingNonceManager)             │   │
     │     │ 5. Check if EIP-6492 (counterfactual wallet)        │   │
     │     │             │                                       │   │
     │     │             ├─── If EIP-6492 signature ────────┐    │   │
     │     │             │                                  │    │   │
     │     │             │ 6a. Deploy wallet first          │    │   │
     │     │             │ ────────────────────────────────────────▶  │
     │     │             │                                  │    │   │
     │     │             │ 6b. Wait for deployment          │    │   │
     │     │             │ ◀────────────────────────────────────────  │
     │     │             │                                  │    │   │
     │     │             └──────────────────────────────────┘    │   │
     │     │             │                                       │   │
     │     │ 7. Construct multicall transaction                 │   │
     │     │    (USDC.transferWithAuthorization)                │   │
     │     │             │                                       │   │
     │     │             │ 8. Sign with facilitator key          │   │
     │     │             │ ──────────────────────────────────────────▶
     │     │             │                                       │   │
     │     │             │ 9. Submit to mempool                  │   │
     │     │             │ ◀──────────────────────────────────────────
     │     │             │                                       │   │
     │     │             │ 10. Wait for receipt                  │   │
     │     │             │ ──────────────────────────────────────────▶
     │     │             │                                       │   │
     │     │             │ 11. Transaction mined                 │   │
     │     │             │ ◀──────────────────────────────────────────
     │     │             │                                       │   │
     │     │ 12. Return SettleResponse{success, tx_hash}        │   │
     │     └─────────────┼─────────────────────────────────────┐ │   │
     │                   │                                     │ │   │
     │                   ▼                                     │ │   │
     │         ┌──────────────────┐                            │ │   │
     │         │ JSON Response    │                            │ │   │
     │         │ {                │                            │ │   │
     │         │   success: true, │                            │ │   │
     │         │   transaction:   │                            │ │   │
     │         │     "0x...",     │                            │ │   │
     │         │   payer: "0x..." │                            │ │   │
     │         │ }                │                            │ │   │
     │         └──────────────────┘                            │ │   │
     │◀─────────────────────────────────────────────────────────┘ │   │
     │                                                           │   │
```

---

## 🏗️ Module Dependency Graph

```
                          ┌──────────┐
                          │ main.rs  │
                          └────┬─────┘
                               │
                ┌──────────────┼──────────────┐
                │              │              │
                ▼              ▼              ▼
         ┌──────────┐   ┌──────────┐   ┌──────────┐
         │handlers  │   │telemetry │   │sig_down  │
         │   .rs    │   │   .rs    │   │   .rs    │
         └────┬─────┘   └──────────┘   └──────────┘
              │
              ▼
    ┌──────────────────┐
    │facilitator_local │
    │      .rs         │
    └────┬───────┬─────┘
         │       │
         │       └─────────────────────┐
         │                             │
         ▼                             ▼
┌─────────────────┐          ┌──────────────────┐
│provider_cache   │          │x402-compliance   │
│      .rs        │          │    (crate)       │
└────┬────────────┘          └──────────────────┘
     │                                │
     │                                ├─ OfacChecker
     │                                ├─ BlacklistChecker
     │                                ├─ EvmExtractor
     │                                └─ SolanaExtractor
     │
     ▼
┌─────────────────┐
│  chain/mod.rs   │
└────┬────────────┘
     │
     ├───────────┬───────────┬───────────┐
     │           │           │           │
     ▼           ▼           ▼           ▼
┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐
│chain/   │ │chain/   │ │chain/   │ │chain/   │
│evm.rs   │ │solana.rs│ │near.rs  │ │stellar  │
│         │ │         │ │         │ │  .rs    │
└─────────┘ └─────────┘ └─────────┘ └─────────┘
     │           │           │           │
     └───────────┴───────────┴───────────┘
                     │
                     ▼
           ┌──────────────────┐
           │   types.rs       │
           │   network.rs     │
           │   from_env.rs    │
           │   timestamp.rs   │
           └──────────────────┘
```

---

## 🌐 Network Provider Enum Dispatch

```
          NetworkProvider (enum)
                  │
      ┌───────────┼───────────┬───────────┐
      │           │           │           │
      ▼           ▼           ▼           ▼
┌───────────┐ ┌───────────┐ ┌───────────┐ ┌───────────┐
│    Evm    │ │  Solana   │ │   Near    │ │  Stellar  │
│  Variant  │ │  Variant  │ │  Variant  │ │  Variant  │
└───────────┘ └───────────┘ └───────────┘ └───────────┘
      │           │           │           │
      ▼           ▼           ▼           ▼
┌───────────┐ ┌───────────┐ ┌───────────┐ ┌───────────┐
│EvmProvider│ │ SolanaP.. │ │ NearPr..  │ │StellarP.. │
├───────────┤ ├───────────┤ ├───────────┤ ├───────────┤
│ inner:    │ │ keypair:  │ │ signer:   │ │signing_key│
│ Provider  │ │ Keypair   │ │ Signer    │ │SigningKey │
├───────────┤ ├───────────┤ ├───────────┤ ├───────────┤
│ eip1559:  │ │ client:   │ │ client:   │ │ chain:    │
│ bool      │ │ RpcClient │ │ JsonRpc.. │ │StellarChn │
├───────────┤ ├───────────┤ ├───────────┤ ├───────────┤
│ chain:    │ │ network:  │ │ network:  │ │soroban_rpc│
│ EvmChain  │ │ Network   │ │ Network   │ │ _url      │
├───────────┤ ├───────────┤ ├───────────┤ ├───────────┤
│signers:   │ │compute_   │ │usdc_      │ │nonce_cache│
│Arc<Vec>   │ │budget     │ │contract   │ │RwLock<..> │
└───────────┘ └───────────┘ └───────────┘ └───────────┘
      │           │           │           │
      │           │           │           │
      └───────────┴───────────┴───────────┘
                     │
           Implements Facilitator trait
                     │
      ┌──────────────┼──────────────┐
      │              │              │
      ▼              ▼              ▼
 verify()       settle()      supported()
```

---

## 🔒 Compliance Screening Flow

```
                ┌─────────────────────┐
                │  Payment Received   │
                └──────────┬──────────┘
                           │
                           ▼
           ┌───────────────────────────────┐
           │  Extract Payer/Payee Addresses│
           │  (EvmExtractor/SolanaExtractor│
           │   /NearExtractor/etc.)        │
           └──────────┬────────────────────┘
                      │
                      ▼
        ┌─────────────────────────────┐
        │  Create TransactionContext  │
        │  - amount                   │
        │  - currency (USDC)          │
        │  - network                  │
        │  - transaction_id (optional)│
        └──────────┬──────────────────┘
                   │
                   ▼
    ┌──────────────────────────────────┐
    │  ComplianceChecker               │
    │  ::screen_payment()              │
    │                                  │
    │  ┌────────────────────────────┐  │
    │  │  1. OFAC SDN List Check    │  │
    │  │     (US Treasury)          │  │
    │  └────────────┬───────────────┘  │
    │               │                  │
    │               ▼                  │
    │  ┌────────────────────────────┐  │
    │  │  2. Custom Blacklist Check │  │
    │  │     (JSON file)            │  │
    │  └────────────┬───────────────┘  │
    │               │                  │
    │               ▼                  │
    │  ┌────────────────────────────┐  │
    │  │  3. Future: UN, UK, EU     │  │
    │  │     Sanctions Lists        │  │
    │  └────────────┬───────────────┘  │
    └───────────────┼──────────────────┘
                    │
                    ▼
         ┌──────────────────┐
         │ ScreeningDecision│
         └──────────┬───────┘
                    │
      ┌─────────────┼─────────────┐
      │             │             │
      ▼             ▼             ▼
┌──────────┐  ┌──────────┐  ┌──────────┐
│  Block   │  │  Review  │  │  Clear   │
│  {reason}│  │  {reason}│  │          │
└────┬─────┘  └────┬─────┘  └────┬─────┘
     │             │             │
     │             │             │
     ▼             ▼             ▼
┌──────────┐  ┌──────────┐  ┌──────────┐
│ Reject   │  │ Reject   │  │ Proceed  │
│ Payment  │  │ Payment  │  │ with     │
│          │  │ (manual  │  │ Payment  │
│          │  │  review) │  │          │
└──────────┘  └──────────┘  └──────────┘
```

---

## 🔄 EVM Nonce Management (Multi-Signer)

```
┌───────────────────────────────────────────────────┐
│          Facilitator Wallet Pool                  │
│                                                   │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐
│  │ Wallet 1     │  │ Wallet 2     │  │ Wallet 3     │
│  │ 0xAAA...     │  │ 0xBBB...     │  │ 0xCCC...     │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘
│         │ nonce=5         │ nonce=3         │ nonce=7
│         │                 │                 │
└─────────┼─────────────────┼─────────────────┼─────────┘
          │                 │                 │
          │  Round-Robin Selection (AtomicUsize)
          │                 │                 │
   ┌──────▼──────┐   ┌──────▼──────┐   ┌──────▼──────┐
   │  Request 1  │   │  Request 2  │   │  Request 3  │
   │  (Wallet 1) │   │  (Wallet 2) │   │  (Wallet 3) │
   └──────┬──────┘   └──────┬──────┘   └──────┬──────┘
          │                 │                 │
          │  Parallel Execution (Independent Nonce Sequences)
          │                 │                 │
          ▼                 ▼                 ▼
   ┌──────────────┐  ┌──────────────┐  ┌──────────────┐
   │ Send Tx      │  │ Send Tx      │  │ Send Tx      │
   │ nonce=5      │  │ nonce=3      │  │ nonce=7      │
   └──────┬───────┘  └──────┬───────┘  └──────┬───────┘
          │                 │                 │
          │ Success: nonce++ │ Success: nonce++ │ Success: nonce++
          │ Failure: reset   │ Failure: reset   │ Failure: reset
          │                 │                 │
          ▼                 ▼                 ▼
   ┌──────────────┐  ┌──────────────┐  ┌──────────────┐
   │ nonce=6      │  │ nonce=4      │  │ nonce=8      │
   └──────────────┘  └──────────────┘  └──────────────┘

┌───────────────────────────────────────────────────┐
│       PendingNonceManager (DashMap)               │
│                                                   │
│  Wallet 1: PendingNonce { current: 6, pending: [] }
│  Wallet 2: PendingNonce { current: 4, pending: [] }
│  Wallet 3: PendingNonce { current: 8, pending: [] }
│                                                   │
│  - Lock-free reads for nonce queries             │
│  - Fine-grained locking for nonce updates         │
│  - Reset on transaction failure (retry with same nonce)
└───────────────────────────────────────────────────┘
```

**Benefit**: 3 wallets = 3x throughput (parallel nonce sequences)

---

## 📦 Workspace Crate Relationships

```
┌─────────────────────────────────────────────────────────┐
│                  x402-rs Workspace                      │
│                                                         │
│  ┌───────────────────────────────────────────────────┐  │
│  │           Root Crate (x402-rs)                    │  │
│  │           - Binary: facilitator service          │  │
│  │           - Depends on all workspace crates      │  │
│  └────────────────────┬──────────────────────────────┘  │
│                       │                                 │
│       ┌───────────────┼───────────────┐                 │
│       │               │               │                 │
│       ▼               ▼               ▼                 │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐              │
│  │x402-axum │  │x402-     │  │x402-     │              │
│  │(library) │  │compliance│  │reqwest   │              │
│  │          │  │(library) │  │(library) │              │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘              │
│       │             │             │                     │
│       │             │             │                     │
│  ┌────▼─────┐  ┌────▼─────┐  ┌────▼─────┐              │
│  │x402-axum-│  │          │  │x402-     │              │
│  │example   │  │          │  │reqwest-  │              │
│  │          │  │          │  │example   │              │
│  └──────────┘  └──────────┘  └──────────┘              │
│                                                         │
└─────────────────────────────────────────────────────────┘

┌───────────────────────────────────────────────────────┐
│            x402-axum (Middleware)                     │
│  - X402Layer (Tower layer)                            │
│  - PaymentGate (extract X-Payment header)            │
│  - Integrates with Axum router                       │
│  - Returns HTTP 402 on missing/invalid payment       │
└───────────────────────────────────────────────────────┘

┌───────────────────────────────────────────────────────┐
│         x402-compliance (Modular Screening)           │
│  - ComplianceChecker trait                            │
│  - OfacChecker (US Treasury SDN list)                 │
│  - BlacklistChecker (custom JSON)                     │
│  - EvmExtractor, SolanaExtractor                      │
│  - ScreeningDecision (Block/Review/Clear)             │
│  - Features: ofac, solana, un, uk, eu                 │
└───────────────────────────────────────────────────────┘

┌───────────────────────────────────────────────────────┐
│         x402-reqwest (Client Library)                 │
│  - X402Client (HTTP client with auto-payment)         │
│  - Attaches X-Payment header                          │
│  - Retries with payment on HTTP 402                   │
│  - Integrates with reqwest                            │
└───────────────────────────────────────────────────────┘
```

---

## 🌍 Multi-Chain Type Hierarchy

```
                    MixedAddress (enum)
                          │
          ┌───────────────┼───────────────┬───────────┐
          │               │               │           │
          ▼               ▼               ▼           ▼
    ┌──────────┐    ┌──────────┐    ┌──────────┐ ┌──────────┐
    │   Evm    │    │  Solana  │    │   Near   │ │ Stellar  │
    │(0x...,   │    │(base58,  │    │(alice.   │ │(G.../C...,
    │ 20 bytes)│    │ 32 bytes)│    │ near, or │ │ 56 chars)│
    │          │    │          │    │ 64 hex)  │ │          │
    └────┬─────┘    └────┬─────┘    └────┬─────┘ └────┬─────┘
         │               │               │           │
         │               │               │           │
         ▼               ▼               ▼           ▼
  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐ ┌──────────────┐
  │EvmAddress    │ │Solana Pubkey │ │String        │ │String        │
  │(wrapper      │ │              │ │              │ │              │
  │ around       │ │              │ │              │ │              │
  │ alloy::      │ │              │ │              │ │              │
  │ Address)     │ │              │ │              │ │              │
  └──────────────┘ └──────────────┘ └──────────────┘ └──────────────┘

                 ExactPaymentPayload (enum)
                          │
          ┌───────────────┼───────────────┬───────────┐
          │               │               │           │
          ▼               ▼               ▼           ▼
    ┌──────────┐    ┌──────────┐    ┌──────────┐ ┌──────────┐
    │   Evm    │    │  Solana  │    │   Near   │ │ Stellar  │
    └────┬─────┘    └────┬─────┘    └────┬─────┘ └────┬─────┘
         │               │               │           │
         ▼               ▼               ▼           ▼
┌───────────────┐ ┌───────────────┐ ┌───────────────┐ ┌───────────────┐
│ExactEvmPayload│ │ExactSolana..  │ │ExactNear..    │ │ExactStellar.. │
├───────────────┤ ├───────────────┤ ├───────────────┤ ├───────────────┤
│signature:     │ │transaction:   │ │signed_delegate│ │from: String   │
│EvmSignature   │ │String (base64)│ │_action: String│ │to: String     │
├───────────────┤ └───────────────┘ │(base64 borsh) │ │amount: String │
│authorization: │                   └───────────────┘ │token_contract │
│ -from         │                                     │authorization_ │
│ -to           │                                     │entry_xdr      │
│ -value        │                                     │nonce: u64     │
│ -valid_after  │                                     │signature_exp. │
│ -valid_before │                                     │_ledger: u32   │
│ -nonce        │                                     └───────────────┘
└───────────────┘
```

---

## 🔐 Security Layers

```
┌─────────────────────────────────────────────────────────┐
│               Layer 1: TLS/HTTPS                        │
│  - AWS ALB terminates TLS                               │
│  - Certificate management via ACM                       │
└────────────────────────┬────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────┐
│          Layer 2: Compliance Screening                  │
│  - OFAC SDN list (a file, regenerated by hand)          │
│  - Custom blacklist (JSON)                              │
│  - Fail-closed on screening failure                     │
│  - Re-screen on both verify AND settle                  │
└────────────────────────┬────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────┐
│          Layer 3: Payment Validation                    │
│  - Signature verification (EIP-712/Ed25519)             │
│  - Timing validation (validAfter/validBefore)           │
│  - Receiver address matching                            │
│  - Balance sufficiency check                            │
│  - Nonce uniqueness (replay protection)                 │
└────────────────────────┬────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────┐
│       Layer 4: On-Chain Settlement Security             │
│  - Re-verify payment before execution                   │
│  - Atomic transactions (deploy + transfer)              │
│  - Gas limit safety margins                             │
│  - Transaction receipt confirmation                     │
└────────────────────────┬────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────┐
│         Layer 5: Secrets Management                     │
│  - AWS Secrets Manager for production keys              │
│  - Separate mainnet/testnet wallets                     │
│  - No plaintext secrets in env vars or logs             │
│  - Secrets rotation procedures documented               │
└─────────────────────────────────────────────────────────┘
```

---

## 📈 Performance Optimization Points

```
┌─────────────────────────────────────────────────────────┐
│            1. Lazy Initialization                       │
│  - Static USDC deployments (once_cell::Lazy)            │
│  - Compile-time constants (const fn)                    │
│  - Benefit: No runtime allocation overhead              │
└─────────────────────────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────┐
│            2. Provider Caching                          │
│  - Initialize all RPC providers at startup              │
│  - Reuse across requests (Arc<Provider>)                │
│  - Benefit: No reconnection overhead                    │
└─────────────────────────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────┐
│            3. Nonce Parallelism                         │
│  - Multiple facilitator wallets (round-robin)           │
│  - Independent nonce sequences                          │
│  - Benefit: N wallets = N× throughput                   │
└─────────────────────────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────┐
│            4. In-Memory Nonce Tracking                  │
│  - DashMap for lock-free reads                          │
│  - Avoids RPC calls for nonce queries                   │
│  - Benefit: Faster transaction submission               │
└─────────────────────────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────┐
│            5. Multicall Batching (EVM)                  │
│  - Combine deploy + transfer in single tx               │
│  - Use Multicall3 for atomic operations                 │
│  - Benefit: Lower gas costs, faster settlement          │
└─────────────────────────────────────────────────────────┘
```

---

**Last Updated**: 2025-12-11 | **Document Version**: 1.0
