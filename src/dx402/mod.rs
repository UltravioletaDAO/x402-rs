//! DX402 -- the `durable-evidence` extension for x402.
//!
//! x402 settles payment durably on-chain but delivers the purchased resource
//! exactly once, in the body of a `200 OK`, and retains nothing. Settlement is
//! permanent; delivery is not. A buyer who did not capture the response at that
//! instant cannot recover it, and neither party can later prove *what* was
//! delivered -- only *that* payment happened.
//!
//! DX402 closes that gap with three properties:
//!
//! 1. **Durable** -- the delivered body survives the session.
//! 2. **Private** -- encrypted to the payer; no third party, this facilitator
//!    included, can read it.
//! 3. **Coupled** -- no registration and no extra round trip, because the
//!    encryption key material is produced by the act of paying.
//!
//! Spec: `docs/plans/dx402/02-SPEC-v0.1.md`.
//! Research and prior art: `docs/plans/dx402/00-RESEARCH.md`.
//!
//! # Where the pieces run
//!
//! The facilitator is **not** in the response path -- it only sees `/verify` and
//! `/settle`, never the body. So the work is split:
//!
//! - **Resource server** (`x402-axum`): holds the plaintext. Encrypts, uploads,
//!   and reports the pointer. This is the post-hook.
//! - **Facilitator** (this module): notary and index. Signs receipts, records
//!   pointers, answers lookups. In `direct` mode it never holds plaintext or key
//!   material.
//! - **Buyer** (`x402-reqwest`): fetches and decrypts locally.
//!
//! # The one rule
//!
//! **DX402 must never make a payment fail.** Every failure path here degrades to
//! a [`types::SkipReason`] and lets the payment through. The chain is the
//! ledger; evidence is an addition to it, not a gate in front of it.

// This crate builds as both a library and a binary, and `main.rs` compiles its
// own copy of this module tree. Most of what follows is the library-facing API
// consumed by `x402-axum` and `x402-reqwest`, so from the binary's point of view
// it reads as unused. That is a property of the dual-target layout, not dead
// code, and the tests in each submodule exercise all of it.
#![allow(dead_code, unused_imports)]

pub mod envelope;
pub mod handlers;
pub mod payer;
pub mod pubkey;
pub mod receipt;
pub mod registry;
pub mod service;
pub mod store;
pub mod types;

pub use envelope::{open, seal, PayerPublicKey, PayerSecretKey, SealedEnvelope};
pub use service::{Dx402Config, Dx402Service};
pub use types::{
    AnchorRequest, AnchoredEvidence, DurableEvidence, DurableEvidenceConfig, DurablePointer,
    Dx402ErrorCode, EvidenceMode, EvidenceReceipt, KeyAlg, Retention, SkipReason, StorageBackend,
    DX402_VERSION, EVIDENCE_HEADER, EXTENSION_KEY,
};

/// keccak256 of a response body, `0x`-prefixed.
///
/// Over the **plaintext**, deliberately. Hashing the ciphertext would only prove
/// the blob was not corrupted in storage; hashing the plaintext proves the blob
/// decrypts to exactly the bytes the buyer was served, which is the check that
/// catches a seller anchoring something other than what it delivered.
pub fn content_hash(body: &[u8]) -> String {
    format!("0x{:x}", alloy::primitives::keccak256(body))
}

/// A stable identifier for a settled payment.
///
/// Uses the `payment-identifier` extension's value when the caller has one;
/// otherwise derives `keccak256(network ‖ txHash)`. It has to be deterministic
/// from data both sides already hold, because it is the AEAD associated data
/// binding a ciphertext to its payment -- if buyer and seller derived it
/// differently, decryption would fail with no obvious cause.
pub fn payment_id(network: crate::network::Network, tx_hash: &str) -> String {
    let mut preimage = network.to_caip2().into_bytes();
    preimage.extend_from_slice(tx_hash.trim_start_matches("0x").as_bytes());
    format!("0x{:x}", alloy::primitives::keccak256(&preimage))
}

/// Environment variables read by this module.
pub mod env {
    /// Master switch. Absent or anything but `true` leaves DX402 entirely off.
    pub const ENABLE_DX402: &str = "ENABLE_DX402";
    /// `s3` | `ipfs` | `arweave`.
    pub const DX402_STORE_BACKEND: &str = "DX402_STORE_BACKEND";
    /// S3 bucket holding sealed evidence.
    pub const DX402_STORE_BUCKET: &str = "DX402_STORE_BUCKET";
    /// Public base URL a pointer dereferences through.
    pub const DX402_STORE_PUBLIC_BASE: &str = "DX402_STORE_PUBLIC_BASE";
    /// DynamoDB table for the evidence index.
    pub const DX402_REGISTRY_TABLE_NAME: &str = "DX402_REGISTRY_TABLE_NAME";
    /// Key the facilitator signs receipts with. Secrets Manager in production.
    pub const DX402_SIGNING_KEY: &str = "DX402_SIGNING_KEY";
    /// Default retention: `90d` | `1y` | `permanent`.
    pub const DX402_RETENTION: &str = "DX402_RETENTION";
}
