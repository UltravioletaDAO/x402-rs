# x402-rs Facilitator Architecture Summary

**Version**: 1.7.7 (as of 2025-12-11)
**Purpose**: Reference document for task-decomposition-expert and other AI agents working on this codebase

---

## 1. Core Rust Architecture Patterns

### 1.1 Module Structure

```
x402-rs/
├── src/
│   ├── main.rs                  # HTTP server entrypoint (Axum)
│   ├── types.rs                 # Protocol types (PaymentPayload, VerifyRequest, etc.)
│   ├── network.rs               # Network enum + USDC deployments (35+ networks)
│   ├── facilitator.rs           # Core Facilitator trait (verify, settle, supported)
│   ├── facilitator_local.rs     # FacilitatorLocal implementation
│   ├── handlers.rs              # Axum HTTP handlers (/verify, /settle, /health, etc.)
│   ├── provider_cache.rs        # RPC provider cache (HashMap<Network, Provider>)
│   ├── timestamp.rs             # EIP-3009 timestamp utilities
│   ├── from_env.rs              # Environment variable loading
│   ├── telemetry.rs             # OpenTelemetry tracing setup
│   ├── sig_down.rs              # Graceful shutdown handling
│   ├── blocklist.rs             # (deprecated, migrated to x402-compliance crate)
│   └── chain/
│       ├── mod.rs               # NetworkProvider enum + FromEnvByNetworkBuild trait
│       ├── evm.rs               # EVM implementation (EIP-3009, EIP-712, EIP-6492)
│       ├── solana.rs            # Solana SPL token transfers
│       ├── near.rs              # NEAR NEP-366 meta-transactions
│       └── stellar.rs           # Stellar/Soroban authorization entries
│
├── crates/
│   ├── x402-axum/              # Axum middleware library for x402 protocol
│   ├── x402-reqwest/           # Reqwest client library for x402 payments
│   └── x402-compliance/        # Modular sanctions screening (OFAC, blacklist)
│
├── examples/
│   ├── x402-axum-example/      # Example server using x402-axum
│   └── x402-reqwest-example/   # Example client using x402-reqwest
│
└── static/
    ├── index.html              # Ultravioleta DAO branded landing page (57KB)
    └── images/                 # Network logos (avalanche.png, base.png, etc.)
```

### 1.2 Key Traits and Abstractions

**`Facilitator` trait** (`src/facilitator.rs`):
```rust
pub trait Facilitator {
    type Error: Debug + Display;

    async fn verify(&self, request: &VerifyRequest) -> Result<VerifyResponse, Self::Error>;
    async fn settle(&self, request: &SettleRequest) -> Result<SettleResponse, Self::Error>;
    async fn supported(&self) -> Result<SupportedPaymentKindsResponse, Self::Error>;
    async fn blacklist_info(&self) -> Result<serde_json::Value, Self::Error>;
}
```

- **Purpose**: Network-agnostic interface for payment verification and settlement
- **Implemented by**: `FacilitatorLocal`, `NetworkProvider`, `EvmProvider`, `SolanaProvider`, `NearProvider`, `StellarProvider`
- **Pattern**: Trait-based polymorphism with async methods

**`NetworkFamily` enum** (`src/network.rs`):
```rust
pub enum NetworkFamily {
    Evm,      // EIP-3009 transferWithAuthorization
    Solana,   // SPL token transfers + Fogo (SVM)
    Near,     // NEP-366 meta-transactions
    Stellar,  // Soroban authorization entries
}
```

**`NetworkProvider` enum** (`src/chain/mod.rs`):
```rust
pub enum NetworkProvider {
    Evm(EvmProvider),
    Solana(SolanaProvider),
    Near(NearProvider),
    Stellar(StellarProvider),
}
```

- **Pattern**: Enum-based dispatch to chain-specific implementations
- **Implements**: `Facilitator` trait via delegation to inner variants

### 1.3 Error Handling Patterns

**`FacilitatorLocalError` enum** (`src/chain/mod.rs`):
```rust
pub enum FacilitatorLocalError {
    UnsupportedNetwork(Option<MixedAddress>),
    NetworkMismatch(Option<MixedAddress>, Network, Network),
    SchemeMismatch(Option<MixedAddress>, Scheme, Scheme),
    InvalidAddress(String),
    ReceiverMismatch(MixedAddress, String, String),
    ClockError(#[source] SystemTimeError),
    InvalidTiming(MixedAddress, String),
    ContractCall(String),
    InvalidSignature(MixedAddress, String),
    InsufficientFunds(MixedAddress),
    InsufficientValue(MixedAddress),
    DecodingError(String),
    BlockedAddress(MixedAddress, String),  // Compliance screening
    Other(String),
}
```

- **Pattern**: `thiserror` for structured errors with context
- **Design**: Include payer address where available for audit trails
- **Compliance**: `BlockedAddress` variant for sanctions screening rejections

**Error propagation**:
- Use `?` operator extensively
- Convert between error types via `From`/`Into` traits
- Chain-specific errors (e.g., `StellarError`) convert to `FacilitatorLocalError`

### 1.4 Async Patterns

**Primary runtime**: Tokio 1.45+
```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ...
}
```

**Key async patterns**:
- **RPC calls**: All blockchain interactions are async (alloy, solana-client, near-jsonrpc-client)
- **HTTP server**: Axum 0.8 with async handlers
- **Concurrency**: `Arc<T>` for shared state, `DashMap` for concurrent nonce tracking (EVM)
- **Graceful shutdown**: `tokio_util::sync::CancellationToken` via `SigDown` struct

**No explicit executor spawning**: All async work happens within Axum request handlers or initialization routines

---

## 2. Multi-Chain Support Architecture

### 2.1 Chain Abstraction Pattern

**Flow**: `Network` → `NetworkFamily` → `NetworkProvider` → Chain-specific logic

```rust
// Step 1: Network enum variant
Network::Base => {
    // Step 2: Determine family
    NetworkFamily::Evm => {
        // Step 3: Resolve provider
        NetworkProvider::Evm(EvmProvider { ... }) => {
            // Step 4: Execute chain-specific logic
            evm_provider.verify(request).await
        }
    }
}
```

### 2.2 EVM Chain Support (23 networks)

**File**: `src/chain/evm.rs` (~1800 lines)

**Key types**:
```rust
pub struct EvmProvider {
    inner: InnerProvider,              // Alloy composed provider
    eip1559: bool,                     // Gas pricing strategy
    chain: EvmChain,                   // Network + chain ID
    signer_addresses: Arc<Vec<Address>>,  // Round-robin signers
    signer_cursor: Arc<AtomicUsize>,   // Rotation index
    nonce_manager: PendingNonceManager, // Nonce tracking
}

pub struct EvmChain {
    pub network: Network,
    pub chain_id: u64,  // e.g., 8453 for Base
}
```

**Critical features**:
- **EIP-3009**: `transferWithAuthorization` for gasless USDC transfers
- **EIP-712**: Typed data signing for authorization
- **EIP-1271**: Smart contract signature verification
- **EIP-6492**: Counterfactual wallet signatures (deploy + verify in one tx)
- **Multi-signer**: Round-robin across multiple facilitator wallets to parallelize nonces

**Verification flow** (off-chain simulation):
1. Decode `ExactEvmPayload` (signature + authorization)
2. Check timing (`validAfter` <= now < `validBefore`)
3. Check receiver matches requirements
4. Simulate transfer via `eth_call` to universal validator (address `0xdAcD...`)
5. Validator checks signature (EOA/EIP-1271/EIP-6492) and balance
6. Return `VerifyResponse::Valid { payer }` or `Invalid { reason }`

**Settlement flow** (on-chain transaction):
1. Re-verify (never trust prior verify call)
2. If EIP-6492: Deploy counterfactual wallet first
3. Call `USDC.transferWithAuthorization()` via multicall
4. Wait for transaction receipt
5. Return `SettleResponse { success, transaction, payer }`

**Nonce management**:
- `PendingNonceManager` tracks pending transactions per signer
- Increments nonce immediately when tx is sent
- Resets nonce if tx fails (to retry with same nonce)

### 2.3 Solana/Fogo Support (3 networks)

**File**: `src/chain/solana.rs`

**Key types**:
```rust
pub struct SolanaProvider {
    keypair: Arc<Keypair>,           // Facilitator's keypair (pays fees)
    client: Arc<RpcClient>,          // Solana RPC client
    network: Network,                // Solana, SolanaDevnet, Fogo, FogoTestnet
    compute_budget: ComputeBudgetConfig,  // Custom compute units/priority fee
}
```

**Payment flow**:
1. User signs SPL token transfer transaction off-chain
2. User sends serialized transaction (base64) to facilitator
3. Facilitator deserializes and validates transfer
4. Facilitator wraps in new transaction with payer = facilitator
5. Facilitator signs and submits to Solana network

**Fogo integration**:
- Fogo is Solana Virtual Machine (SVM) compatible
- Uses same `SolanaProvider` implementation
- Separate USDC token addresses (Fogo mainnet/testnet)
- Added in v1.7.6 (PR #2)

### 2.4 NEAR Protocol Support (2 networks)

**File**: `src/chain/near.rs`

**Key types**:
```rust
pub struct NearProvider {
    signer: Arc<Signer>,               // Facilitator's InMemorySigner
    client: Arc<JsonRpcClient>,        // NEAR JSON-RPC client
    network: Network,                  // Near, NearTestnet
    usdc_contract: AccountId,          // USDC token contract (implicit account)
}
```

**NEP-366 meta-transaction flow**:
1. User creates `DelegateAction` with `FunctionCall` actions (e.g., `ft_transfer`)
2. User signs → `SignedDelegateAction` (borsh-serialized, base64-encoded)
3. User sends to facilitator
4. Facilitator checks if recipient is registered on USDC contract
5. If not: Facilitator calls `storage_deposit` (~0.00125 NEAR) to register
6. Facilitator wraps `SignedDelegateAction` in `Action::Delegate`
7. Facilitator signs `Transaction` with own key (pays gas)
8. NEAR executes inner actions as if user submitted them

**Critical API changes** (near-primitives 0.34+):
```rust
// Type migrations
use near_token::NearToken;  // Replaces u128 for balances
use near_primitives::types::Gas;  // Now a wrapper struct

// Constants
const STORAGE_DEPOSIT: NearToken = NearToken::from_yoctonear(1_250_000_000_000_000_000_000);
const GAS_AMOUNT: Gas = Gas::from_gas(5_000_000_000_000);

// NonDelegateAction pattern matching requires conversion
for non_delegate_action in &signed_delegate_action.delegate_action.actions {
    let action: Action = non_delegate_action.clone().into();
    if let Action::FunctionCall(func_call) = action {
        // Now you can pattern match
    }
}

// Signer type change
let signer: Signer = InMemorySigner::from_secret_key(account_id, secret_key).into();
```

### 2.5 Stellar/Soroban Support (2 networks)

**File**: `src/chain/stellar.rs`

**Key types**:
```rust
pub struct StellarProvider {
    signing_key: SigningKey,              // Facilitator's ed25519 key
    chain: StellarChain,                  // Network + passphrase
    soroban_rpc_url: String,              // Soroban RPC endpoint
    nonce_cache: Arc<RwLock<HashMap<String, u64>>>,  // Replay protection
}

pub struct StellarChain {
    pub network: Network,
    pub network_passphrase: String,  // "Public Global Stellar Network ; September 2015"
}
```

**Soroban authorization flow**:
1. User creates `SorobanAuthorizationEntry` for USDC transfer
2. User signs authorization entry (ed25519 signature)
3. User sends XDR-encoded entry (base64) to facilitator
4. Facilitator verifies signature and expiry ledger
5. Facilitator constructs `InvokeHostFunctionOp` with authorization
6. Facilitator signs and submits `TransactionEnvelope`
7. Stellar network validates authorization and executes transfer

**Key differences from other chains**:
- **Decimals**: Stellar USDC uses 7 decimals (not 6 like EVM/Solana)
- **Addresses**: G... (accounts), C... (contracts) - 56 chars base32
- **Nonces**: Client-provided u64 (cached by facilitator for replay protection)
- **Expiry**: Ledger-based (not Unix timestamp)

---

## 3. Key Types and Data Flows

### 3.1 Core Protocol Types (`src/types.rs`)

**`PaymentPayload`**: Client's signed payment authorization
```rust
pub struct PaymentPayload {
    pub x402_version: X402Version,  // Currently V1 only
    pub scheme: Scheme,              // "exact" (only supported scheme)
    pub network: Network,            // Base, Avalanche, Solana, NEAR, Stellar, etc.
    pub payload: ExactPaymentPayload,
}

pub enum ExactPaymentPayload {
    Evm(ExactEvmPayload),         // EIP-3009 signature + authorization
    Solana(ExactSolanaPayload),   // Serialized SPL token transaction
    Near(ExactNearPayload),       // SignedDelegateAction (base64)
    Stellar(ExactStellarPayload), // SorobanAuthorizationEntry (XDR)
}
```

**`ExactEvmPayload`**: EIP-712 structured data
```rust
pub struct ExactEvmPayload {
    pub signature: EvmSignature,  // 65+ bytes (can include EIP-6492 wrapper)
    pub authorization: ExactEvmPayloadAuthorization,
}

pub struct ExactEvmPayloadAuthorization {
    pub from: EvmAddress,
    pub to: EvmAddress,
    pub value: TokenAmount,
    pub valid_after: UnixTimestamp,
    pub valid_before: UnixTimestamp,
    pub nonce: HexEncodedNonce,  // 0x[64 hex chars]
}
```

**`PaymentRequirements`**: Server-specified constraints
```rust
pub struct PaymentRequirements {
    pub scheme: Scheme,
    pub network: Network,
    pub max_amount_required: TokenAmount,
    pub resource: Url,
    pub description: String,
    pub mime_type: String,
    pub output_schema: Option<serde_json::Value>,
    pub pay_to: MixedAddress,
    pub max_timeout_seconds: u64,
    pub asset: MixedAddress,  // Token contract address
    pub extra: Option<serde_json::Value>,
}
```

**`MixedAddress`**: Multi-chain address abstraction
```rust
pub enum MixedAddress {
    Evm(EvmAddress),         // 0x[40 hex chars]
    Offchain(String),        // ^[A-Za-z0-9][A-Za-z0-9-]{0,34}[A-Za-z0-9]$
    Solana(Pubkey),          // base58, 32 bytes
    Near(String),            // alice.near or 64 hex chars (implicit)
    Stellar(String),         // G... (account) or C... (contract), 56 chars base32
}
```

### 3.2 Request/Response Types

**`VerifyRequest`**: Verify payment without executing
```rust
pub struct VerifyRequest {
    pub x402_version: X402Version,
    pub payment_payload: PaymentPayload,
    pub payment_requirements: PaymentRequirements,
}
```

**`VerifyResponse`**: Result of verification
```rust
pub enum VerifyResponse {
    Valid { payer: MixedAddress },
    Invalid {
        reason: FacilitatorErrorReason,
        payer: Option<MixedAddress>,
    },
}
```

**`SettleRequest`**: Same as `VerifyRequest` (type alias)
```rust
pub type SettleRequest = VerifyRequest;
```

**`SettleResponse`**: Result of settlement
```rust
pub struct SettleResponse {
    pub success: bool,
    pub error_reason: Option<FacilitatorErrorReason>,
    pub payer: MixedAddress,
    pub transaction: Option<TransactionHash>,
    pub network: Network,
}
```

### 3.3 Data Flow: Payment Verification

```
Client → POST /verify (VerifyRequest)
   ↓
handlers::post_verify()
   ↓
FacilitatorLocal::verify(&request)
   ↓
[Compliance Screening]
   ↓
ProviderMap::by_network(request.network)
   ↓
NetworkProvider::verify(&request)  [Enum dispatch]
   ↓
┌─────────────────────────────────────────────┐
│ EvmProvider::verify()                       │
│ 1. Parse ExactEvmPayload                    │
│ 2. Validate timing (validAfter/validBefore) │
│ 3. Check receiver matches requirements      │
│ 4. Simulate transfer via eth_call           │
│ 5. Verify signature via universal validator │
│ 6. Check balance >= value                   │
└─────────────────────────────────────────────┘
   ↓
VerifyResponse::Valid { payer } or Invalid { reason }
   ↓
JSON response to client
```

### 3.4 Data Flow: Payment Settlement

```
Client → POST /settle (SettleRequest)
   ↓
handlers::post_settle()
   ↓
FacilitatorLocal::settle(&request)
   ↓
[Compliance Screening - CRITICAL: Re-screen before settlement]
   ↓
ProviderMap::by_network(request.network)
   ↓
NetworkProvider::settle(&request)  [Enum dispatch]
   ↓
┌─────────────────────────────────────────────┐
│ EvmProvider::settle()                       │
│ 1. Re-verify payment (never trust prior verify) │
│ 2. If EIP-6492: Deploy wallet first        │
│ 3. Construct multicall transaction         │
│ 4. Call USDC.transferWithAuthorization()   │
│ 5. Sign with facilitator key               │
│ 6. Submit to blockchain                     │
│ 7. Wait for receipt                         │
└─────────────────────────────────────────────┘
   ↓
SettleResponse { success, transaction, payer }
   ↓
JSON response to client
```

---

## 4. Critical Files and Their Roles

### 4.1 Core Infrastructure

**`src/main.rs`** (137 lines):
- Initializes Axum HTTP server
- Loads `.env` variables via `dotenvy`
- Sets up OpenTelemetry tracing
- Creates `ProviderCache` (fail-fast if RPC unavailable)
- Initializes `ComplianceChecker` (OFAC + custom blacklist)
- Binds to `HOST:PORT` (default: `0.0.0.0:8080`)
- Enables graceful shutdown via `SigDown`

**`src/types.rs`** (1537 lines):
- Protocol types: `PaymentPayload`, `PaymentRequirements`, `VerifyRequest`, `SettleRequest`
- Response types: `VerifyResponse`, `SettleResponse`
- Address types: `EvmAddress`, `MixedAddress`
- Amount types: `TokenAmount` (wrapper around `U256`), `MoneyAmount` (human-readable)
- Encoding: `Base64Bytes`, EIP-712 domain types
- Comprehensive `Display`, `Serialize`, `Deserialize` implementations

**`src/network.rs`** (790 lines):
- `Network` enum: 35+ variants (Base, Avalanche, Solana, NEAR, Stellar, Fogo, etc.)
- `NetworkFamily` enum: `Evm`, `Solana`, `Near`, `Stellar`
- `USDCDeployment`: Static USDC contract addresses per network
- `TokenDeployment`, `TokenAsset`, `TokenDeploymentEip712`
- `Network::variants()`: Returns all supported networks
- `Network::is_testnet()`, `Network::is_mainnet()`

**`src/facilitator.rs`** (114 lines):
- `Facilitator` trait: Core interface for verify/settle/supported/blacklist_info
- Implemented by: `FacilitatorLocal`, `NetworkProvider`, chain-specific providers
- `Arc<T>` wrapper implementation for shared state

**`src/facilitator_local.rs`** (370 lines):
- `FacilitatorLocal<A>`: Main facilitator implementation
- Generic over `ProviderMap` (enables testing with mock providers)
- Delegates to chain-specific providers via `NetworkProvider` enum
- **Compliance screening**: Calls `ComplianceChecker` before verify and settle
- **Critical security**: Re-screens on settle (never trust prior verify call)

**`src/handlers.rs`** (404 lines):
- Axum HTTP handlers: `post_verify`, `post_settle`, `get_supported`, `get_health`, `get_version`
- Asset handlers: `get_logo`, `get_favicon`, network logo endpoints
- **Custom branding**: `get_root()` serves Ultravioleta DAO landing page via `include_str!()`
- Error handling: Maps `FacilitatorLocalError` to HTTP status codes

**`src/provider_cache.rs`** (93 lines):
- `ProviderCache`: `HashMap<Network, NetworkProvider>`
- `ProviderMap` trait: Generic interface for provider lookup
- `from_env()`: Initializes all providers from environment variables
- Fail-fast: Exits if required RPC URLs or private keys are missing

### 4.2 Chain-Specific Implementations

**`src/chain/mod.rs`** (159 lines):
- `NetworkProvider` enum: Dispatches to `EvmProvider`, `SolanaProvider`, `NearProvider`, `StellarProvider`
- `FromEnvByNetworkBuild` trait: Async initialization from environment
- `NetworkProviderOps` trait: `signer_address()`, `network()`
- `FacilitatorLocalError`: Comprehensive error enum

**`src/chain/evm.rs`** (~1800 lines):
- `EvmProvider`: Alloy-based EVM implementation
- `EvmChain`: Network + chain ID mapping (35+ EVM chains)
- EIP-3009 `transferWithAuthorization` logic
- EIP-712 typed data signing/verification
- EIP-1271 smart contract signature verification
- EIP-6492 counterfactual wallet deployment
- `PendingNonceManager`: Concurrent nonce tracking with `DashMap`
- Round-robin signer selection for parallelism

**`src/chain/solana.rs`** (~800 lines):
- `SolanaProvider`: SPL token transfer implementation
- Deserializes client-signed transactions
- Wraps in facilitator-signed transaction (facilitator pays fees)
- Configurable compute budget (via environment variables)
- Supports Fogo (SVM) networks

**`src/chain/near.rs`** (~700 lines):
- `NearProvider`: NEP-366 meta-transaction implementation
- Parses `SignedDelegateAction` (borsh-serialized, base64)
- Auto-registration: Calls `storage_deposit` if recipient not registered
- Wraps delegate action in `Transaction` (facilitator pays gas)
- **Critical**: Uses near-primitives 0.34+ API (NearToken, Gas types)

**`src/chain/stellar.rs`** (~900 lines):
- `StellarProvider`: Soroban authorization entry implementation
- Parses `SorobanAuthorizationEntry` (XDR-encoded)
- Verifies ed25519 signatures
- Constructs `InvokeHostFunctionOp` with authorization
- Nonce replay protection (in-memory cache)
- Horizon API for account/ledger queries
- Soroban RPC for transaction submission

### 4.3 Workspace Crates

**`crates/x402-compliance/`** (modular compliance crate):
- `ComplianceChecker` trait: `screen_payment()`, `list_metadata()`
- `OfacChecker`: OFAC SDN list (US Treasury)
- `BlacklistChecker`: Custom JSON blacklist
- `EvmExtractor`, `SolanaExtractor`: Extract addresses from payloads
- `ScreeningDecision`: `Block`, `Review`, `Clear`
- **Features**: `ofac`, `solana`, `un`, `uk`, `eu` (sanctions lists)

**`crates/x402-axum/`** (Axum middleware):
- `X402Layer`: Tower middleware for payment-gated endpoints
- Extracts `X-Payment` header, verifies via facilitator
- Returns HTTP 402 with `PaymentRequirements` if no/invalid payment

**`crates/x402-reqwest/`** (client library):
- `X402Client`: Reqwest-based client for x402 payments
- Automatically attaches `X-Payment` header
- Retries with payment on HTTP 402 response

---

## 5. Recent Additions (v1.5.0 - v1.7.7)

### 5.1 NEAR Protocol Integration (v1.6.x)

**Added**: Nov-Dec 2024
**Files**: `src/chain/near.rs`, NEAR variants in `Network` enum
**Features**:
- NEP-366 meta-transactions (delegate actions)
- Auto-registration for USDC recipients
- near-primitives 0.34+ API compatibility
- Frontend integration with MyNearWallet

**Key learnings**:
- Type migrations: `NearToken`, `Gas` wrapper types
- `NonDelegateAction` requires conversion to `Action` for pattern matching
- Signer type change: `InMemorySigner::from_secret_key().into()`

### 5.2 Stellar/Soroban Integration (v1.7.7)

**Added**: December 2024
**Files**: `src/chain/stellar.rs`, Stellar variants in `Network` enum
**Features**:
- Soroban authorization entries (pre-signed invocations)
- XDR encoding/decoding via `stellar-xdr` crate
- Ed25519 signature verification
- Nonce-based replay protection
- Horizon API + Soroban RPC integration

**Critical differences**:
- Stellar USDC has 7 decimals (not 6)
- Ledger-based expiry (not Unix timestamp)
- Address formats: G... (accounts), C... (contracts)

### 5.3 Fogo Chain Support (v1.7.6)

**Added**: December 2024
**Files**: Fogo variants in `Network` enum, reuses `SolanaProvider`
**Networks**: `Fogo` (mainnet), `FogoTestnet`
**Integration**: Solana Virtual Machine (SVM) compatible
**USDC**: Custom token addresses on Fogo mainnet/testnet

### 5.4 Compliance Integration (v1.3.11+)

**Added**: November 2024
**Files**: `crates/x402-compliance/`, compliance screening in `facilitator_local.rs`
**Features**:
- OFAC SDN list screening (auto-updated)
- Custom blacklist support (JSON format)
- EVM and Solana address extraction
- Fail-closed security model
- **Critical**: Re-screening on settlement (never trust prior verify)

**Future**: NEAR and Stellar compliance extractors (TODO)

### 5.5 x402 v2 Protocol Considerations

**Status**: Analyzed, not yet implemented
**Documentation**: `docs/X402_V2_PROTOCOL_ANALYSIS.md`
**Key changes**:
- Multiple asset support (not just USDC)
- Flexible pricing schemes (range, percentage, dynamic)
- Enhanced metadata fields
- Backward compatibility maintained

**Migration plan**: Requires protocol version negotiation, schema updates

---

## 6. Architectural Patterns Summary

### 6.1 Design Patterns

**Trait-based polymorphism**:
- `Facilitator` trait for network-agnostic interface
- Implemented by `FacilitatorLocal`, `NetworkProvider`, chain-specific providers

**Enum dispatch**:
- `NetworkProvider` enum delegates to chain-specific implementations
- Avoids dynamic dispatch overhead (zero-cost abstraction)

**Builder pattern**:
- `ProviderBuilder` (Alloy) for composing Ethereum providers
- `ComplianceCheckerBuilder` for modular compliance configuration

**Strategy pattern**:
- Gas pricing: EIP-1559 vs legacy
- Nonce management: pending vs confirmed
- Signature verification: EOA vs EIP-1271 vs EIP-6492

**Newtype pattern**:
- `TokenAmount(U256)`, `EvmAddress(Address)`, `HexEncodedNonce([u8; 32])`
- Type safety via wrapper types

**Extension traits**:
- `ProviderMap` for generic provider lookup
- `FromEnvByNetworkBuild` for async initialization

### 6.2 Concurrency Patterns

**Arc for shared state**:
- `Arc<ProviderCache>`, `Arc<ComplianceChecker>`, `Arc<Keypair>`
- Enables cheap cloning across async tasks

**DashMap for concurrent writes**:
- `PendingNonceManager` tracks nonces per signer
- Lock-free reads, fine-grained write locking

**RwLock for read-heavy caches**:
- Stellar nonce cache (`RwLock<HashMap<String, u64>>`)
- Many readers, occasional writer

**Atomic operations**:
- `AtomicUsize` for round-robin signer selection
- Lock-free counter increment

### 6.3 Error Handling Philosophy

**Fail-fast initialization**:
- Exit on missing RPC URLs, invalid private keys, compliance init failure
- Prevents serving with incomplete configuration

**Fail-closed security**:
- Compliance screening failure blocks payment
- Missing credentials = reject payment
- Invalid signature = reject payment

**Structured errors**:
- `thiserror` for error types with context
- Include payer address for audit trails
- Convert chain-specific errors to `FacilitatorLocalError`

**Graceful degradation**:
- Nonce reset on transaction failure (allows retry)
- Optional features (Solana compliance extraction fails open temporarily)

### 6.4 Performance Optimizations

**Lazy initialization**:
- `Lazy<USDCDeployment>` for static USDC addresses
- `once_cell` for compile-time constants

**Nonce parallelism**:
- Multiple facilitator wallets with round-robin selection
- Each wallet maintains independent nonce sequence
- Enables parallel transaction submission

**Provider caching**:
- `ProviderCache` initialized once at startup
- Reused across all requests (no reconnection overhead)

**Multicall batching** (EVM):
- Combine wallet deployment + transfer in single transaction
- Reduces gas costs and latency

**In-memory nonce tracking**:
- `DashMap` avoids RPC calls for nonce queries
- Resets on failure (guarantees consistency)

---

## 7. Configuration and Environment

### 7.1 Environment Variables

**Wallet separation** (mainnet vs testnet):
```bash
EVM_PRIVATE_KEY_MAINNET=0x...
EVM_PRIVATE_KEY_TESTNET=0x...
SOLANA_PRIVATE_KEY_MAINNET=...
SOLANA_PRIVATE_KEY_TESTNET=...
NEAR_PRIVATE_KEY_MAINNET=ed25519:...
NEAR_PRIVATE_KEY_TESTNET=ed25519:...
NEAR_ACCOUNT_ID_MAINNET=facilitator.near
NEAR_ACCOUNT_ID_TESTNET=facilitator.testnet
STELLAR_PRIVATE_KEY_MAINNET=S...
STELLAR_PRIVATE_KEY_TESTNET=S...
```

**RPC URLs** (per network):
```bash
RPC_URL_BASE_MAINNET=https://mainnet.base.org
RPC_URL_BASE_SEPOLIA=https://sepolia.base.org
RPC_URL_AVALANCHE_MAINNET=https://api.avax.network/ext/bc/C/rpc
RPC_URL_SOLANA_MAINNET=https://api.mainnet-beta.solana.com
RPC_URL_NEAR_MAINNET=https://rpc.mainnet.near.org
RPC_URL_STELLAR_MAINNET=https://soroban-rpc.mainnet.stellar.gateway.fm
# ... (35+ networks)
```

**Server configuration**:
```bash
HOST=0.0.0.0
PORT=8080
RUST_LOG=info  # or debug, trace
OTEL_EXPORTER_OTLP_ENDPOINT=https://api.honeycomb.io
```

**Solana compute budget** (optional):
```bash
SOLANA_COMPUTE_UNIT_LIMIT=200000
SOLANA_COMPUTE_UNIT_PRICE=1000
```

### 7.2 AWS Secrets Manager (Production)

**Secret names**:
- `facilitator-evm-private-key-mainnet`
- `facilitator-evm-private-key-testnet`
- `facilitator-solana-keypair-mainnet`
- `facilitator-solana-keypair-testnet`
- `facilitator-near-private-key-mainnet`
- `facilitator-near-private-key-testnet`
- `facilitator-stellar-private-key-mainnet`
- `facilitator-stellar-private-key-testnet`
- `facilitator-rpc-mainnet` (JSON: all mainnet RPC URLs)
- `facilitator-rpc-testnet` (JSON: all testnet RPC URLs)

**CRITICAL SECURITY**: Never put RPC URLs with API keys in ECS task definition environment variables (plaintext). Always use Secrets Manager references:
```json
{
  "name": "RPC_URL_ARBITRUM",
  "valueFrom": "arn:aws:secretsmanager:REGION:ACCOUNT:secret:facilitator-rpc-mainnet:arbitrum::"
}
```

### 7.3 Version Management

**Compile-time version from Cargo.toml**:
```rust
pub async fn get_version() -> impl IntoResponse {
    Json(json!({ "version": env!("CARGO_PKG_VERSION") }))
}
```

**WRONG** (runtime environment variable):
```rust
// DO NOT USE - returns "dev" if env var not set
pub async fn get_version() -> impl IntoResponse {
    Json(json!({ "version": option_env!("FACILITATOR_VERSION").unwrap_or("dev") }))
}
```

---

## 8. Testing Strategy

### 8.1 Integration Tests

**Location**: `tests/integration/`

**Key test files**:
- `test_facilitator.py`: Full facilitator test suite
- `test_usdc_payment.py`: USDC payment flow (EVM chains)
- `test_x402_integration.py`: x402 protocol compliance
- `test_endpoints.py`: HTTP endpoint validation

**Pattern**: Python tests against running facilitator (requires `cargo run --release`)

### 8.2 Load Testing

**Location**: `tests/load/`

**Tool**: k6 (JavaScript)
```bash
k6 run --vus 100 --duration 5m k6_load_test.js
```

**Metrics**: 100+ TPS sustained, nonce parallelism validation

### 8.3 Unit Tests

**Pattern**: Limited unit tests (mostly integration focus)

**Reason**: Heavy reliance on external RPC calls makes mocking complex

**Future**: Add more unit tests for pure logic (timestamp validation, address parsing, etc.)

---

## 9. Deployment Architecture

### 9.1 AWS ECS (Production)

**Infrastructure**: Terraform in `terraform/environments/production/`

**Components**:
- ECS Fargate cluster (`facilitator-production`)
- ECS service with 1-2 tasks (auto-scaling)
- Application Load Balancer (HTTPS)
- VPC with public/private subnets
- NAT instance (cost optimization)
- CloudWatch logs + alarms
- Secrets Manager for credentials

**Resource sizing**:
- CPU: 1 vCPU
- Memory: 2 GB
- Cost: ~$43-48/month

### 9.2 Docker Image

**Build script**: `scripts/build-and-push.sh`

**Process**:
1. Build Rust binary: `cargo build --release`
2. Create Docker image (multi-stage build)
3. Push to ECR: `518898403364.dkr.ecr.us-east-2.amazonaws.com/facilitator`
4. Tag with version (e.g., `v1.7.7`)

**Base image**: `debian:bookworm-slim` (glibc compatibility)

### 9.3 CI/CD

**Current**: Manual deployment via `build-and-push.sh` + `terraform apply`

**Future**: GitHub Actions for automated builds + deployments

---

## 10. Future Considerations

### 10.1 Protocol Migration (x402 v2)

**Changes needed**:
- Update `X402Version` enum to include `V2`
- Add multi-asset support to `PaymentRequirements`
- Implement flexible pricing schemes (range, percentage)
- Maintain backward compatibility with v1 clients

**Migration strategy**: Protocol version negotiation via HTTP headers

### 10.2 Additional Chains

**Planned**: Algorand, Sui, Aptos (research docs in `docs/`)

**Pattern**: Add new variant to `NetworkFamily`, implement chain-specific provider

**Checklist**: See `guides/ADDING_NEW_CHAINS.md` (~155 lines of code + logo)

### 10.3 Performance Improvements

**Database for nonce tracking**: Replace in-memory `DashMap` with Redis/PostgreSQL
- Enables multi-instance deployments
- Persistent nonce state across restarts

**Caching layer**: Cache balance checks, recipient registration status
- Reduce RPC calls
- Faster verification

**Parallel verification**: Batch verify multiple payments concurrently
- Use tokio `join!` macro
- Useful for high-throughput scenarios

---

## 11. Key Takeaways for Agents

### 11.1 When Adding New Features

1. **Identify the chain family**: EVM, Solana, NEAR, or Stellar?
2. **Add network variant**: Update `Network` enum in `src/network.rs`
3. **Add USDC deployment**: Static lazy initialization in `network.rs`
4. **Implement chain logic**: Extend existing provider or create new one
5. **Update environment loading**: Add RPC URL handling in `from_env.rs`
6. **Add frontend support**: Logo, network card in `static/index.html`
7. **Test integration**: Use `tests/integration/` scripts
8. **Update documentation**: Add to CHANGELOG, CLAUDE.md

### 11.2 When Debugging Payments

1. **Check logs**: `RUST_LOG=debug cargo run --release`
2. **Verify payload structure**: JSON schema matches `VerifyRequest`
3. **Check compliance screening**: Look for "blocked" or "review" logs
4. **Inspect RPC calls**: Enable Alloy/Solana/NEAR client debug logs
5. **Validate timestamps**: EIP-3009 uses Unix seconds (not milliseconds)
6. **Check nonce**: Must be unique 32-byte value (0x[64 hex chars])
7. **Verify signature**: Matches payer address and domain separator

### 11.3 When Modifying Chain Logic

1. **Preserve atomicity**: Settlement must be atomic (no partial transfers)
2. **Re-verify on settle**: Never trust prior verification call
3. **Handle reorgs**: EVM chains may reorg; wait for confirmations
4. **Nonce management**: Ensure no double-spending or skipped nonces
5. **Gas estimation**: Leave headroom for gas price fluctuations
6. **Error context**: Include payer address in all errors for audit trails
7. **Compliance screening**: Always check BEFORE settlement

### 11.4 Security Principles

1. **Fail-closed**: Reject on missing data, invalid signatures, compliance failures
2. **Never skip compliance**: Screen on both verify AND settle
3. **Separate wallets**: Mainnet and testnet keys must be different
4. **Secret management**: Use AWS Secrets Manager in production
5. **Audit logging**: Include payer, amount, network in all log messages
6. **Graceful shutdown**: Ensure pending transactions complete before exit

---

## 12. Glossary

**EIP-3009**: Ethereum Improvement Proposal for `transferWithAuthorization` (gasless transfers)
**EIP-712**: Typed data signing standard for structured messages
**EIP-1271**: Smart contract signature verification standard
**EIP-6492**: Counterfactual signature verification (sign before deployment)
**NEP-366**: NEAR Enhancement Proposal for meta-transactions (delegate actions)
**Soroban**: Stellar's smart contract platform
**SAC**: Stellar Asset Contract (wraps classic assets as Soroban tokens)
**SVM**: Solana Virtual Machine (used by Fogo)
**Facilitator**: Server that verifies and settles x402 payments on behalf of clients
**Payer**: User sending payment (signs authorization)
**Payee**: Recipient of payment (service provider)
**USDC**: USD Coin stablecoin (primary asset supported)
**Nonce**: Unique 32-byte value for replay protection
**Domain separator**: EIP-712 hash of contract name, version, chain ID, contract address

---

## 13. References

**Official x402 Specification**: https://x402.org
**Upstream Repository**: https://github.com/x402-rs/x402-rs
**Ultravioleta DAO Fork**: https://github.com/UltravioletaDAO/x402-rs
**Current Version**: v1.7.7 (as of 2025-12-11)
**Rust Edition**: 2021 (compatible with Rust 1.82+)
**Production URL**: https://facilitator.ultravioletadao.xyz

**Key Documentation Files**:
- `CLAUDE.md`: Project instructions for AI agents
- `docs/CUSTOMIZATIONS.md`: Fork-specific customizations
- `docs/CHANGELOG.md`: Version history
- `guides/ADDING_NEW_CHAINS.md`: Chain integration guide
- `docs/COMPLIANCE_INTEGRATION_COMPLETE.md`: Compliance system overview
- `docs/X402_V2_PROTOCOL_ANALYSIS.md`: Future protocol migration plan

---

**Last Updated**: 2025-12-11
**Document Version**: 1.0
**Prepared for**: task-decomposition-expert agent and future AI collaborators
