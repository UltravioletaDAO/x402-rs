# Plan de Implementacion: x402r Sessions en el Facilitator

**Documento interno para Ultravioleta DAO**

**Fecha:** 25 de Diciembre, 2024
**Version:** 1.0
**Dependencia:** Requiere que Ali implemente `SessionEscrow.sol` primero

---

## Resumen Ejecutivo

Este documento detalla los cambios necesarios en el facilitator (x402-rs) para soportar sesiones de pago con reembolso parcial. La implementacion se divide en 4 fases con un total estimado de ~1500 lineas de codigo nuevo.

---

## Arquitectura General

```
                      FACILITATOR (x402-rs)
┌─────────────────────────────────────────────────────────────┐
│                                                             │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────┐  │
│  │ handlers.rs │  │ sessions.rs │  │ session_types.rs    │  │
│  │             │  │  (NUEVO)    │  │  (NUEVO)            │  │
│  └──────┬──────┘  └──────┬──────┘  └─────────────────────┘  │
│         │                │                                   │
│         │         ┌──────┴──────┐                           │
│         │         │             │                           │
│         ▼         ▼             ▼                           │
│  ┌─────────────────────────────────────┐                    │
│  │         session_manager.rs          │                    │
│  │              (NUEVO)                │                    │
│  │  - Estado de sesiones en memoria    │                    │
│  │  - Cache de firmas pendientes       │                    │
│  │  - Sincronizacion con blockchain    │                    │
│  └──────────────────┬──────────────────┘                    │
│                     │                                        │
│                     ▼                                        │
│  ┌─────────────────────────────────────┐                    │
│  │         chain/evm.rs                │                    │
│  │    (agregar funciones de session)   │                    │
│  └──────────────────┬──────────────────┘                    │
│                     │                                        │
└─────────────────────┼────────────────────────────────────────┘
                      │
                      ▼
              ┌───────────────┐
              │ SessionEscrow │
              │  (on-chain)   │
              └───────────────┘
```

---

## Fase 1: Tipos y Estructuras Base

### Archivo: `src/session_types.rs` (NUEVO)

```rust
//! Session types for x402r partial refund extension
//!
//! Defines all types needed for session-based payments with partial refunds.

use alloy::primitives::{Address, FixedBytes, U256};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// Session Extension (from payment payload)
// ============================================================================

/// Session extension data from payment payload
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionExtension {
    /// Type of session (prepaid, postpaid)
    #[serde(rename = "type")]
    pub session_type: SessionType,

    /// Price per unit of service (in smallest token unit)
    pub price_per_unit: String,

    /// Maximum number of units purchasable
    pub max_units: u32,

    /// Session duration in seconds
    pub duration: u64,

    /// Seller/service provider address
    pub seller: Address,

    /// Human-readable description
    #[serde(default)]
    pub description: Option<String>,
}

/// Session type enum
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SessionType {
    /// Pay upfront, consume later, refund unused
    Prepaid,
    /// Pay as you go (future)
    Postpaid,
}

// ============================================================================
// Session State (internal tracking)
// ============================================================================

/// Internal session state tracked by facilitator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    /// Unique session identifier (from contract)
    pub session_id: FixedBytes<32>,

    /// Buyer address
    pub buyer: Address,

    /// Seller address
    pub seller: Address,

    /// Token address (e.g., USDC)
    pub token: Address,

    /// Network identifier
    pub network: String,

    /// Total deposited amount
    pub total_deposit: U256,

    /// Amount consumed so far
    pub consumed_amount: U256,

    /// Price per unit
    pub price_per_unit: U256,

    /// Maximum units
    pub max_units: u32,

    /// Units used
    pub units_used: u32,

    /// Session creation timestamp
    pub created_at: u64,

    /// Session expiration timestamp
    pub expires_at: u64,

    /// Current status
    pub status: SessionStatus,

    /// On-chain transaction hash for creation
    pub creation_tx: Option<String>,
}

/// Session status enum
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    /// Session is active and accepting consumption
    Active,
    /// Session completed normally
    Completed,
    /// Session expired
    Expired,
    /// Session in dispute
    Disputed,
    /// Pending on-chain confirmation
    Pending,
}

// ============================================================================
// API Request/Response Types
// ============================================================================

/// Request to create a new session
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    /// x402 version
    pub x402_version: u8,

    /// Payment payload with session extension
    pub payment_payload: SessionPaymentPayload,

    /// Payment requirements
    pub payment_requirements: SessionPaymentRequirements,
}

/// Payment payload for session creation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPaymentPayload {
    /// Network in CAIP-2 format
    pub network: String,

    /// EIP-3009 authorization
    pub authorization: SessionAuthorization,

    /// Signature
    pub signature: String,

    /// Session extension
    pub extensions: SessionExtensions,
}

/// Session extensions container
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionExtensions {
    /// Session configuration
    pub session: SessionExtension,
}

/// Authorization for session deposit
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionAuthorization {
    pub from: Address,
    pub to: Address,  // SessionEscrow address
    pub value: String,
    pub valid_after: String,
    pub valid_before: String,
    pub nonce: String,
}

/// Payment requirements for session
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPaymentRequirements {
    pub asset: Address,
    pub amount: String,
    pub network: String,
}

/// Response after creating a session
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionResponse {
    /// Whether creation was successful
    pub success: bool,

    /// Session ID (if successful)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,

    /// Transaction hash
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction: Option<String>,

    /// Session details
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionInfo>,

    /// Error message (if failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Session info returned in responses
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: String,
    pub buyer: String,
    pub seller: String,
    pub total_deposit: String,
    pub consumed_amount: String,
    pub remaining_amount: String,
    pub units_used: u32,
    pub remaining_units: u32,
    pub expires_at: u64,
    pub status: String,
}

// ============================================================================
// Consume Units Types
// ============================================================================

/// Request to consume units from a session
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsumeUnitsRequest {
    /// Session identifier
    pub session_id: String,

    /// Number of units to consume
    pub units: u32,

    /// Buyer's signature authorizing consumption
    pub buyer_signature: String,

    /// Network
    pub network: String,
}

/// Response after consuming units
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsumeUnitsResponse {
    pub success: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub units_consumed: Option<u32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_consumed: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ============================================================================
// Close Session Types
// ============================================================================

/// Request to close a session
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseSessionRequest {
    /// Session identifier
    pub session_id: String,

    /// Network
    pub network: String,

    /// Caller signature (optional, for verification)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// Response after closing a session
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseSessionResponse {
    pub success: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction: Option<String>,

    /// Amount sent to seller
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seller_amount: Option<String>,

    /// Amount refunded to buyer
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buyer_refund: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ============================================================================
// Query Types
// ============================================================================

/// Request to get session status
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetSessionRequest {
    pub session_id: String,
    pub network: String,
}

/// Response with session details
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetSessionResponse {
    pub success: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionInfo>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
```

### Lineas estimadas: ~350

---

## Fase 2: Manejador de Sesiones

### Archivo: `src/session_manager.rs` (NUEVO)

```rust
//! Session manager for tracking and caching session state
//!
//! Maintains an in-memory cache of active sessions and handles
//! synchronization with on-chain state.

use crate::network::Network;
use crate::session_types::{SessionState, SessionStatus};
use alloy::primitives::{Address, FixedBytes};
use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Session manager for caching and tracking session state
pub struct SessionManager {
    /// Active sessions indexed by session_id
    sessions: DashMap<FixedBytes<32>, SessionState>,

    /// Sessions indexed by buyer address
    buyer_sessions: DashMap<Address, Vec<FixedBytes<32>>>,

    /// Sessions indexed by seller address
    seller_sessions: DashMap<Address, Vec<FixedBytes<32>>>,

    /// Expiration check interval
    expiration_check_interval: Duration,

    /// Whether the manager is running
    running: Arc<RwLock<bool>>,
}

impl SessionManager {
    /// Create a new session manager
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
            buyer_sessions: DashMap::new(),
            seller_sessions: DashMap::new(),
            expiration_check_interval: Duration::from_secs(60),
            running: Arc::new(RwLock::new(false)),
        }
    }

    /// Start the session manager background tasks
    pub async fn start(&self) {
        let mut running = self.running.write().await;
        if *running {
            return;
        }
        *running = true;
        drop(running);

        info!("Session manager started");

        // Start expiration checker
        self.start_expiration_checker().await;
    }

    /// Register a new session
    pub fn register_session(&self, session: SessionState) {
        let session_id = session.session_id;
        let buyer = session.buyer;
        let seller = session.seller;

        // Add to main sessions map
        self.sessions.insert(session_id, session);

        // Index by buyer
        self.buyer_sessions
            .entry(buyer)
            .or_insert_with(Vec::new)
            .push(session_id);

        // Index by seller
        self.seller_sessions
            .entry(seller)
            .or_insert_with(Vec::new)
            .push(session_id);

        debug!(
            session_id = %hex::encode(session_id),
            "Registered new session"
        );
    }

    /// Get a session by ID
    pub fn get_session(&self, session_id: &FixedBytes<32>) -> Option<SessionState> {
        self.sessions.get(session_id).map(|s| s.clone())
    }

    /// Update session state after consuming units
    pub fn update_consumption(
        &self,
        session_id: &FixedBytes<32>,
        units: u32,
        amount: alloy::primitives::U256,
    ) -> Result<(), SessionManagerError> {
        let mut session = self
            .sessions
            .get_mut(session_id)
            .ok_or(SessionManagerError::SessionNotFound)?;

        if session.status != SessionStatus::Active {
            return Err(SessionManagerError::SessionNotActive);
        }

        session.units_used += units;
        session.consumed_amount += amount;

        debug!(
            session_id = %hex::encode(session_id),
            units_used = session.units_used,
            consumed = %session.consumed_amount,
            "Updated session consumption"
        );

        Ok(())
    }

    /// Mark session as completed
    pub fn complete_session(
        &self,
        session_id: &FixedBytes<32>,
    ) -> Result<SessionState, SessionManagerError> {
        let mut session = self
            .sessions
            .get_mut(session_id)
            .ok_or(SessionManagerError::SessionNotFound)?;

        session.status = SessionStatus::Completed;

        Ok(session.clone())
    }

    /// Get all sessions for a buyer
    pub fn get_buyer_sessions(&self, buyer: &Address) -> Vec<SessionState> {
        self.buyer_sessions
            .get(buyer)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.sessions.get(id).map(|s| s.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get all sessions for a seller
    pub fn get_seller_sessions(&self, seller: &Address) -> Vec<SessionState> {
        self.seller_sessions
            .get(seller)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.sessions.get(id).map(|s| s.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get all active sessions
    pub fn get_active_sessions(&self) -> Vec<SessionState> {
        self.sessions
            .iter()
            .filter(|s| s.status == SessionStatus::Active)
            .map(|s| s.clone())
            .collect()
    }

    /// Check for expired sessions
    async fn start_expiration_checker(&self) {
        let sessions = self.sessions.clone();
        let interval = self.expiration_check_interval;

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;

                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();

                let mut expired_count = 0;

                for mut session in sessions.iter_mut() {
                    if session.status == SessionStatus::Active && session.expires_at < now {
                        session.status = SessionStatus::Expired;
                        expired_count += 1;
                    }
                }

                if expired_count > 0 {
                    info!(count = expired_count, "Marked expired sessions");
                }
            }
        });
    }

    /// Get session statistics
    pub fn get_stats(&self) -> SessionStats {
        let total = self.sessions.len();
        let active = self
            .sessions
            .iter()
            .filter(|s| s.status == SessionStatus::Active)
            .count();
        let completed = self
            .sessions
            .iter()
            .filter(|s| s.status == SessionStatus::Completed)
            .count();
        let expired = self
            .sessions
            .iter()
            .filter(|s| s.status == SessionStatus::Expired)
            .count();

        SessionStats {
            total,
            active,
            completed,
            expired,
        }
    }
}

/// Session statistics
#[derive(Debug, Clone)]
pub struct SessionStats {
    pub total: usize,
    pub active: usize,
    pub completed: usize,
    pub expired: usize,
}

/// Session manager errors
#[derive(Debug, thiserror::Error)]
pub enum SessionManagerError {
    #[error("Session not found")]
    SessionNotFound,

    #[error("Session is not active")]
    SessionNotActive,

    #[error("Session has expired")]
    SessionExpired,

    #[error("Insufficient units remaining")]
    InsufficientUnits,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}
```

### Lineas estimadas: ~250

---

## Fase 3: Logica de Sesiones (Blockchain)

### Archivo: `src/sessions.rs` (NUEVO)

```rust
//! Session settlement logic for x402r partial refunds
//!
//! Handles creating sessions, consuming units, and closing sessions
//! on the SessionEscrow contract.

use crate::chain::evm::{EvmProvider, MetaTransaction};
use crate::chain::{FacilitatorLocalError, NetworkProvider};
use crate::network::Network;
use crate::provider_cache::{HasProviderMap, ProviderMap};
use crate::session_manager::{SessionManager, SessionManagerError};
use crate::session_types::*;

use alloy::primitives::{Address, Bytes, FixedBytes, U256};
use alloy::sol;
use alloy::sol_types::SolCall;
use serde_json::json;
use std::env;
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, error, info, instrument, warn};

// ============================================================================
// Contract Bindings
// ============================================================================

sol!(
    #[allow(missing_docs)]
    #[derive(Debug)]
    #[sol(rpc)]
    SessionEscrow,
    "abi/SessionEscrow.json"
);

// ============================================================================
// Contract Addresses
// ============================================================================

/// SessionEscrow addresses per network
pub mod session_escrow_addresses {
    use super::*;

    /// Base Mainnet SessionEscrow (TBD - will be deployed by Ali)
    pub const BASE_MAINNET: Address = Address::ZERO; // TODO: Set after deployment

    /// Base Sepolia SessionEscrow (TBD - will be deployed by Ali)
    pub const BASE_SEPOLIA: Address = Address::ZERO; // TODO: Set after deployment
}

/// Get SessionEscrow address for a network
pub fn session_escrow_for_network(network: Network) -> Option<Address> {
    match network {
        Network::Base => {
            let addr = session_escrow_addresses::BASE_MAINNET;
            if addr == Address::ZERO {
                None
            } else {
                Some(addr)
            }
        }
        Network::BaseSepolia => {
            let addr = session_escrow_addresses::BASE_SEPOLIA;
            if addr == Address::ZERO {
                None
            } else {
                Some(addr)
            }
        }
        _ => None,
    }
}

// ============================================================================
// Feature Flag
// ============================================================================

/// Check if sessions feature is enabled
pub fn is_sessions_enabled() -> bool {
    env::var("ENABLE_SESSIONS")
        .map(|v| v.to_lowercase() == "true" || v == "1")
        .unwrap_or(false)
}

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("Sessions feature is disabled")]
    FeatureDisabled,

    #[error("Network {0} does not support sessions")]
    UnsupportedNetwork(String),

    #[error("Session not found: {0}")]
    SessionNotFound(String),

    #[error("Session is not active")]
    SessionNotActive,

    #[error("Session has expired")]
    SessionExpired,

    #[error("Insufficient units remaining")]
    InsufficientUnits,

    #[error("Invalid signature")]
    InvalidSignature,

    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("Transaction failed: {0}")]
    Transaction(String),

    #[error("Manager error: {0}")]
    Manager(#[from] SessionManagerError),
}

// ============================================================================
// Session Operations
// ============================================================================

/// Create a new payment session
#[instrument(skip(facilitator, manager))]
pub async fn create_session<T: HasProviderMap>(
    request: CreateSessionRequest,
    facilitator: &T,
    manager: &SessionManager,
) -> Result<CreateSessionResponse, SessionError> {
    if !is_sessions_enabled() {
        return Err(SessionError::FeatureDisabled);
    }

    // Parse network
    let network = parse_network(&request.payment_payload.network)?;

    // Get SessionEscrow address
    let escrow_address = session_escrow_for_network(network)
        .ok_or_else(|| SessionError::UnsupportedNetwork(network.to_string()))?;

    info!(
        network = %network,
        escrow = %escrow_address,
        buyer = %request.payment_payload.authorization.from,
        seller = %request.payment_payload.extensions.session.seller,
        amount = %request.payment_requirements.amount,
        "Creating new session"
    );

    // Get provider
    let provider = facilitator
        .provider_map()
        .get(&network)
        .ok_or_else(|| SessionError::Provider(format!("No provider for {}", network)))?;

    // Build createSession transaction
    let session_ext = &request.payment_payload.extensions.session;
    let total_deposit = U256::from_str_radix(
        &request.payment_requirements.amount.trim_start_matches("0x"),
        if request.payment_requirements.amount.starts_with("0x") { 16 } else { 10 }
    ).map_err(|e| SessionError::Transaction(format!("Invalid amount: {}", e)))?;

    let price_per_unit = U256::from_str_radix(
        &session_ext.price_per_unit.trim_start_matches("0x"),
        if session_ext.price_per_unit.starts_with("0x") { 16 } else { 10 }
    ).map_err(|e| SessionError::Transaction(format!("Invalid price: {}", e)))?;

    // First, execute the EIP-3009 transfer to escrow
    // (This transfers tokens from buyer to SessionEscrow)
    let auth = &request.payment_payload.authorization;
    let signature = hex::decode(
        request.payment_payload.signature.trim_start_matches("0x")
    ).map_err(|e| SessionError::Transaction(format!("Invalid signature: {}", e)))?;

    // Build the createSession call
    let create_call = SessionEscrow::createSessionCall {
        seller: session_ext.seller,
        token: request.payment_requirements.asset,
        totalDeposit: total_deposit,
        pricePerUnit: price_per_unit,
        duration: U256::from(session_ext.duration),
    };

    let calldata = create_call.abi_encode();

    // Execute transaction
    let tx_hash = provider
        .send_transaction(escrow_address, Bytes::from(calldata))
        .await
        .map_err(|e| SessionError::Transaction(format!("Transaction failed: {}", e)))?;

    info!(tx = %tx_hash, "Session creation transaction sent");

    // TODO: Wait for confirmation and extract session ID from logs
    // For now, generate a placeholder session ID
    let session_id = FixedBytes::from_slice(&alloy::primitives::keccak256(
        format!("{}-{}-{}", auth.from, session_ext.seller, tx_hash).as_bytes()
    ).0);

    // Create session state
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let session_state = SessionState {
        session_id,
        buyer: auth.from,
        seller: session_ext.seller,
        token: request.payment_requirements.asset,
        network: request.payment_payload.network.clone(),
        total_deposit,
        consumed_amount: U256::ZERO,
        price_per_unit,
        max_units: session_ext.max_units,
        units_used: 0,
        created_at: now,
        expires_at: now + session_ext.duration,
        status: SessionStatus::Active,
        creation_tx: Some(tx_hash.clone()),
    };

    // Register in manager
    manager.register_session(session_state.clone());

    Ok(CreateSessionResponse {
        success: true,
        session_id: Some(format!("0x{}", hex::encode(session_id))),
        transaction: Some(tx_hash),
        session: Some(session_state_to_info(&session_state)),
        error: None,
    })
}

/// Consume units from an active session
#[instrument(skip(facilitator, manager))]
pub async fn consume_units<T: HasProviderMap>(
    request: ConsumeUnitsRequest,
    facilitator: &T,
    manager: &SessionManager,
) -> Result<ConsumeUnitsResponse, SessionError> {
    if !is_sessions_enabled() {
        return Err(SessionError::FeatureDisabled);
    }

    // Parse session ID
    let session_id_bytes = hex::decode(request.session_id.trim_start_matches("0x"))
        .map_err(|_| SessionError::SessionNotFound(request.session_id.clone()))?;
    let session_id = FixedBytes::from_slice(&session_id_bytes);

    // Get session from manager
    let session = manager
        .get_session(&session_id)
        .ok_or_else(|| SessionError::SessionNotFound(request.session_id.clone()))?;

    if session.status != SessionStatus::Active {
        return Err(SessionError::SessionNotActive);
    }

    // Check units available
    let remaining_units = session.max_units - session.units_used;
    if request.units > remaining_units {
        return Err(SessionError::InsufficientUnits);
    }

    // Parse network
    let network = parse_network(&request.network)?;

    // Get SessionEscrow address
    let escrow_address = session_escrow_for_network(network)
        .ok_or_else(|| SessionError::UnsupportedNetwork(network.to_string()))?;

    // Get provider
    let provider = facilitator
        .provider_map()
        .get(&network)
        .ok_or_else(|| SessionError::Provider(format!("No provider for {}", network)))?;

    // Decode buyer signature
    let signature = hex::decode(request.buyer_signature.trim_start_matches("0x"))
        .map_err(|_| SessionError::InvalidSignature)?;

    // Build consumeUnits call
    // Note: Using simplified signature for now, real impl needs EIP-712
    let consume_call = SessionEscrow::consumeUnitsCall {
        sessionId: session_id,
        units: U256::from(request.units),
        buyerSignature: Bytes::from(signature),
        nonce: FixedBytes::ZERO, // TODO: Implement nonce tracking
    };

    let calldata = consume_call.abi_encode();

    // Execute transaction
    let tx_hash = provider
        .send_transaction(escrow_address, Bytes::from(calldata))
        .await
        .map_err(|e| SessionError::Transaction(format!("Transaction failed: {}", e)))?;

    // Update manager
    let amount = session.price_per_unit * U256::from(request.units);
    manager.update_consumption(&session_id, request.units, amount)?;

    // Get updated session
    let updated_session = manager.get_session(&session_id).unwrap();

    info!(
        session_id = %request.session_id,
        units = request.units,
        tx = %tx_hash,
        "Units consumed"
    );

    Ok(ConsumeUnitsResponse {
        success: true,
        transaction: Some(tx_hash),
        units_consumed: Some(request.units),
        total_consumed: Some(updated_session.consumed_amount.to_string()),
        remaining: Some((updated_session.total_deposit - updated_session.consumed_amount).to_string()),
        error: None,
    })
}

/// Close a session and distribute funds
#[instrument(skip(facilitator, manager))]
pub async fn close_session<T: HasProviderMap>(
    request: CloseSessionRequest,
    facilitator: &T,
    manager: &SessionManager,
) -> Result<CloseSessionResponse, SessionError> {
    if !is_sessions_enabled() {
        return Err(SessionError::FeatureDisabled);
    }

    // Parse session ID
    let session_id_bytes = hex::decode(request.session_id.trim_start_matches("0x"))
        .map_err(|_| SessionError::SessionNotFound(request.session_id.clone()))?;
    let session_id = FixedBytes::from_slice(&session_id_bytes);

    // Get session from manager
    let session = manager
        .get_session(&session_id)
        .ok_or_else(|| SessionError::SessionNotFound(request.session_id.clone()))?;

    if session.status != SessionStatus::Active && session.status != SessionStatus::Expired {
        return Err(SessionError::SessionNotActive);
    }

    // Parse network
    let network = parse_network(&request.network)?;

    // Get SessionEscrow address
    let escrow_address = session_escrow_for_network(network)
        .ok_or_else(|| SessionError::UnsupportedNetwork(network.to_string()))?;

    // Get provider
    let provider = facilitator
        .provider_map()
        .get(&network)
        .ok_or_else(|| SessionError::Provider(format!("No provider for {}", network)))?;

    // Build closeSession call
    let close_call = SessionEscrow::closeSessionCall {
        sessionId: session_id,
    };

    let calldata = close_call.abi_encode();

    // Execute transaction
    let tx_hash = provider
        .send_transaction(escrow_address, Bytes::from(calldata))
        .await
        .map_err(|e| SessionError::Transaction(format!("Transaction failed: {}", e)))?;

    // Calculate distribution
    let seller_amount = session.consumed_amount;
    let buyer_refund = session.total_deposit - session.consumed_amount;

    // Update manager
    manager.complete_session(&session_id)?;

    info!(
        session_id = %request.session_id,
        seller_amount = %seller_amount,
        buyer_refund = %buyer_refund,
        tx = %tx_hash,
        "Session closed"
    );

    Ok(CloseSessionResponse {
        success: true,
        transaction: Some(tx_hash),
        seller_amount: Some(seller_amount.to_string()),
        buyer_refund: Some(buyer_refund.to_string()),
        error: None,
    })
}

/// Get session details
pub async fn get_session_info(
    request: GetSessionRequest,
    manager: &SessionManager,
) -> Result<GetSessionResponse, SessionError> {
    // Parse session ID
    let session_id_bytes = hex::decode(request.session_id.trim_start_matches("0x"))
        .map_err(|_| SessionError::SessionNotFound(request.session_id.clone()))?;
    let session_id = FixedBytes::from_slice(&session_id_bytes);

    // Get from manager
    let session = manager
        .get_session(&session_id)
        .ok_or_else(|| SessionError::SessionNotFound(request.session_id.clone()))?;

    Ok(GetSessionResponse {
        success: true,
        session: Some(session_state_to_info(&session)),
        error: None,
    })
}

// ============================================================================
// Helpers
// ============================================================================

fn parse_network(network_str: &str) -> Result<Network, SessionError> {
    // Handle CAIP-2 format
    if network_str.starts_with("eip155:") {
        let chain_id = network_str
            .strip_prefix("eip155:")
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| SessionError::UnsupportedNetwork(network_str.to_string()))?;

        Network::from_chain_id(chain_id)
            .ok_or_else(|| SessionError::UnsupportedNetwork(network_str.to_string()))
    } else {
        // Try parsing as network name
        network_str
            .parse()
            .map_err(|_| SessionError::UnsupportedNetwork(network_str.to_string()))
    }
}

fn session_state_to_info(state: &SessionState) -> SessionInfo {
    let remaining = state.total_deposit - state.consumed_amount;
    let remaining_units = state.max_units - state.units_used;

    SessionInfo {
        session_id: format!("0x{}", hex::encode(state.session_id)),
        buyer: format!("{}", state.buyer),
        seller: format!("{}", state.seller),
        total_deposit: state.total_deposit.to_string(),
        consumed_amount: state.consumed_amount.to_string(),
        remaining_amount: remaining.to_string(),
        units_used: state.units_used,
        remaining_units,
        expires_at: state.expires_at,
        status: format!("{:?}", state.status).to_lowercase(),
    }
}
```

### Lineas estimadas: ~450

---

## Fase 4: Endpoints HTTP

### Modificar: `src/handlers.rs`

Agregar nuevos handlers para sesiones:

```rust
// Agregar imports
use crate::session_manager::SessionManager;
use crate::session_types::*;
use crate::sessions;

// Agregar al estado de la aplicacion
pub struct AppState {
    pub facilitator: Arc<FacilitatorLocal>,
    pub session_manager: Arc<SessionManager>,  // NUEVO
}

// ============================================================================
// Nuevos Handlers para Sesiones
// ============================================================================

/// POST /session/create - Create a new payment session
pub async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    match sessions::create_session(request, &*state.facilitator, &*state.session_manager).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(e) => {
            error!(error = %e, "Failed to create session");
            (
                StatusCode::BAD_REQUEST,
                Json(CreateSessionResponse {
                    success: false,
                    session_id: None,
                    transaction: None,
                    session: None,
                    error: Some(e.to_string()),
                }),
            ).into_response()
        }
    }
}

/// POST /session/consume - Consume units from a session
pub async fn consume_units(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ConsumeUnitsRequest>,
) -> impl IntoResponse {
    match sessions::consume_units(request, &*state.facilitator, &*state.session_manager).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(e) => {
            error!(error = %e, "Failed to consume units");
            (
                StatusCode::BAD_REQUEST,
                Json(ConsumeUnitsResponse {
                    success: false,
                    transaction: None,
                    units_consumed: None,
                    total_consumed: None,
                    remaining: None,
                    error: Some(e.to_string()),
                }),
            ).into_response()
        }
    }
}

/// POST /session/close - Close a session
pub async fn close_session(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CloseSessionRequest>,
) -> impl IntoResponse {
    match sessions::close_session(request, &*state.facilitator, &*state.session_manager).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(e) => {
            error!(error = %e, "Failed to close session");
            (
                StatusCode::BAD_REQUEST,
                Json(CloseSessionResponse {
                    success: false,
                    transaction: None,
                    seller_amount: None,
                    buyer_refund: None,
                    error: Some(e.to_string()),
                }),
            ).into_response()
        }
    }
}

/// GET /session/:id - Get session details
pub async fn get_session(
    State(state): State<Arc<AppState>>,
    Path((network, session_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let request = GetSessionRequest { session_id, network };

    match sessions::get_session_info(request, &*state.session_manager).await {
        Ok(response) => (StatusCode::OK, Json(response)).into_response(),
        Err(e) => {
            (
                StatusCode::NOT_FOUND,
                Json(GetSessionResponse {
                    success: false,
                    session: None,
                    error: Some(e.to_string()),
                }),
            ).into_response()
        }
    }
}

/// GET /sessions/stats - Get session statistics
pub async fn get_session_stats(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let stats = state.session_manager.get_stats();
    Json(json!({
        "total": stats.total,
        "active": stats.active,
        "completed": stats.completed,
        "expired": stats.expired
    }))
}
```

### Modificar: `src/main.rs`

Agregar rutas para sesiones:

```rust
// Agregar imports
mod session_manager;
mod session_types;
mod sessions;

use session_manager::SessionManager;

// En la funcion main, crear el manager
let session_manager = Arc::new(SessionManager::new());
session_manager.start().await;

// Crear estado compartido
let app_state = Arc::new(AppState {
    facilitator: Arc::new(facilitator),
    session_manager,
});

// Agregar rutas
let app = Router::new()
    // ... rutas existentes ...

    // Session routes (NEW)
    .route("/session/create", post(handlers::create_session))
    .route("/session/consume", post(handlers::consume_units))
    .route("/session/close", post(handlers::close_session))
    .route("/session/:network/:id", get(handlers::get_session))
    .route("/sessions/stats", get(handlers::get_session_stats))

    .with_state(app_state);
```

### Lineas estimadas para handlers.rs: ~150
### Lineas estimadas para main.rs: ~30

---

## Fase 5: Actualizaciones Adicionales

### Archivo: `abi/SessionEscrow.json` (NUEVO)

Este archivo sera proporcionado por Ali despues de desplegar el contrato. Formato esperado:

```json
{
  "abi": [
    {
      "name": "createSession",
      "type": "function",
      "inputs": [
        {"name": "seller", "type": "address"},
        {"name": "token", "type": "address"},
        {"name": "totalDeposit", "type": "uint256"},
        {"name": "pricePerUnit", "type": "uint256"},
        {"name": "duration", "type": "uint256"}
      ],
      "outputs": [{"name": "sessionId", "type": "bytes32"}]
    },
    {
      "name": "consumeUnits",
      "type": "function",
      "inputs": [
        {"name": "sessionId", "type": "bytes32"},
        {"name": "units", "type": "uint256"},
        {"name": "buyerSignature", "type": "bytes"},
        {"name": "nonce", "type": "bytes32"}
      ],
      "outputs": []
    },
    {
      "name": "closeSession",
      "type": "function",
      "inputs": [{"name": "sessionId", "type": "bytes32"}],
      "outputs": []
    },
    {
      "name": "getSession",
      "type": "function",
      "inputs": [{"name": "sessionId", "type": "bytes32"}],
      "outputs": [
        {"name": "buyer", "type": "address"},
        {"name": "seller", "type": "address"},
        {"name": "totalDeposit", "type": "uint256"},
        {"name": "consumedAmount", "type": "uint256"},
        {"name": "status", "type": "uint8"}
      ]
    }
  ]
}
```

### Modificar: `.env.example`

```bash
# Session Extension (partial refunds)
ENABLE_SESSIONS=false  # Set to true to enable session-based payments
```

### Modificar: `static/index.html`

Agregar seccion de Sessions en la landing page (similar a como hicimos con escrow).

---

## Resumen de Archivos

| Archivo | Accion | Lineas Estimadas |
|---------|--------|------------------|
| `src/session_types.rs` | NUEVO | ~350 |
| `src/session_manager.rs` | NUEVO | ~250 |
| `src/sessions.rs` | NUEVO | ~450 |
| `src/handlers.rs` | MODIFICAR | +150 |
| `src/main.rs` | MODIFICAR | +30 |
| `abi/SessionEscrow.json` | NUEVO | ~50 |
| `static/index.html` | MODIFICAR | +50 |
| `.env.example` | MODIFICAR | +2 |
| **TOTAL** | | **~1330** |

---

## Cronograma Propuesto

| Fase | Descripcion | Dependencia |
|------|-------------|-------------|
| **1** | Tipos y estructuras | Ninguna |
| **2** | Session manager | Fase 1 |
| **3** | Logica blockchain | Fase 2 + ABI de Ali |
| **4** | Endpoints HTTP | Fase 3 |
| **5** | Testing e integracion | Fase 4 + Contrato desplegado |

---

## Notas de Implementacion

1. **Esperar ABI de Ali** - No podemos completar Fase 3 sin el ABI del contrato `SessionEscrow`

2. **EIP-712 Signatures** - La firma para `consumeUnits` deberia usar EIP-712 para seguridad

3. **Persistencia** - El `SessionManager` actual es en memoria. Para produccion, considerar Redis o DB

4. **Rate Limiting** - Agregar rate limiting para endpoints de sessions

5. **Metricas** - Agregar metricas de OpenTelemetry para sessions

---

## Pruebas Requeridas

```bash
# Unit tests
cargo test session

# Integration tests (requiere contrato desplegado)
cd tests/integration
python test_sessions.py --network base-sepolia
```

---

**Documento creado:** 25 de Diciembre, 2024
**Autor:** Ultravioleta DAO
