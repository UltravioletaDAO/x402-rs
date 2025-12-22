# Track Specification: Implement support for a new blockchain, including necessary compliance and integration updates.

## 1. Introduction

This document outlines the specifications for integrating a new blockchain into the x402-rs payment facilitation service. The integration will cover blockchain-specific RPC communication, transaction signing, and compliance checks, adhering to the project's non-custodial and gasless payment philosophy.

<h2>2. Goals</h2>

*   Enable x402-rs to process payment authorizations and settle transactions on the new blockchain.
*   Maintain the gasless payment experience for users on the new chain.
*   Ensure compliance checks are performed as required for the new chain's ecosystem.
*   Integrate seamlessly with the existing x402-rs architecture, particularly the web server, compliance module, and multi-chain network handlers.

<h2>3. Scope</h2>

This track covers:
*   **Blockchain Communication:** Establishing robust and efficient communication with the new blockchain's RPC endpoints.
*   **Transaction Handling:** Implementing mechanisms for building, signing (off-chain by user, on-chain by facilitator), and submitting transactions specific to the new blockchain. This includes support for native tokens and relevant stablecoins (e.g., USDC, if applicable).
*   **Compliance Integration:** Adapting or extending the `x402-compliance` module to perform necessary checks relevant to the new blockchain, such as OFAC address screening.
*   **API Exposure:** Extending the x402-rs API to support the new blockchain for operations like `/supported`, `/verify`, and `/settle`.
*   **Configuration:** Updating environment variables and configuration files (`.env.example`, `config/`) to include settings for the new blockchain (RPC URLs, private keys, etc.).
*   **Testing:** Developing unit and integration tests specific to the new blockchain's integration.
*   **Documentation:** Updating relevant internal and external documentation (e.g., `README.md`, `docs/`) to reflect support for the new blockchain.

<h2>4. Out of Scope</h2>

This track does not cover:
*   Frontend UI development for the new blockchain.
*   Deep protocol-level changes to the x402-rs core functionality beyond what is required for integrating a new chain.
*   Support for arbitrary tokens beyond specified stablecoins or native tokens on the new chain, unless explicitly defined as part of the integration.

<h2>5. Technical Details</h2>

<h3>5.1. New Blockchain Characteristics (Placeholder for actual details)</h3>

*   **Type:** [EVM / Non-EVM, e.g., UTXO-based, Account-based]
*   **Consensus Mechanism:** [e.g., PoS, PoW, DPoS]
*   **Native Token:** [Symbol]
*   **Stablecoin(s) Supported:** [e.g., USDC, USDT, native stablecoin]
*   **Transaction Model:** [e.g., Account-based, UTXO]
*   **Signature Format:** [e.g., EIP-3009, custom]

<h3>5.2. Integration Points</h3>

*   **`src/network.rs`:** Extend for new blockchain-specific network client initialization and management.
*   **`src/handlers.rs`:** Modify to route and process requests for the new blockchain.
*   **`crates/x402-compliance`:** Implement new or extend existing compliance checks.
*   **`src/types.rs` / `src/types_v2.rs`:** Define new data structures or extend existing ones for the new blockchain's transaction types and parameters.
*   **Configuration:** Add new entries to `.env.example` and potentially `config/blacklist.json.example`.

<h2>6. Testing Strategy</h2>

*   **Unit Tests:** Will be developed for all new functions and modules specific to the new blockchain's integration.
*   **Integration Tests:** Will verify end-to-end payment flows on the new blockchain, including `/verify` and `/settle` endpoints.
*   **Manual Testing:** To be performed on a testnet environment for the new blockchain.

<h2>7. Deliverables</h2>

*   Working integration of the new blockchain.
*   Updated API endpoints supporting the new blockchain.
*   Comprehensive unit and integration tests.
*   Updated configuration examples.
*   Updated documentation reflecting the new blockchain support.
