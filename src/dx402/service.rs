//! Wiring: configuration, and the service the HTTP handlers call.
//!
//! The facilitator's role in DX402 is narrow on purpose. It does not encrypt, it
//! does not upload, and in `direct` mode it holds nothing that could decrypt
//! anything. It notarises what a resource server reports, indexes it so a buyer
//! can find it later, and serves those two things back.
//!
//! Everything here follows one rule from `super`: **DX402 must never make a
//! payment fail.** Callers on the settle path treat every error as a
//! [`SkipReason`] and carry on.

use std::sync::Arc;

use alloy::signers::local::PrivateKeySigner;
use tracing::{info, warn};

use super::receipt;
use super::registry::{
    DynamoEvidenceRegistry, EvidenceRecord, EvidenceRegistry, MemoryEvidenceRegistry,
    RegistryError, DEFAULT_REGISTRY_TABLE_NAME,
};
use super::store::{EvidenceStore, MemoryEvidenceStore, S3EvidenceStore, StoreError};
use super::types::{
    AnchorRequest, AnchoredEvidence, DurableEvidence, Dx402ErrorCode, EvidenceMode,
    EvidenceReceipt, Retention, SkipReason, StorageBackend, DX402_VERSION,
};

/// Resolved configuration for this deployment.
#[derive(Debug, Clone)]
pub struct Dx402Config {
    pub enabled: bool,
    pub backend: StorageBackend,
    pub bucket: Option<String>,
    pub public_base: Option<String>,
    pub registry_table: Option<String>,
    pub default_retention: Retention,
}

impl Default for Dx402Config {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: StorageBackend::S3,
            bucket: None,
            public_base: None,
            registry_table: None,
            default_retention: Retention::Days90,
        }
    }
}

impl Dx402Config {
    /// Read configuration from the environment.
    ///
    /// Default is **off**. A payment path that already works must not change
    /// behaviour because a new module was linked in.
    pub fn from_env() -> Self {
        use super::env::*;

        let enabled = std::env::var(ENABLE_DX402)
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        let backend = match std::env::var(DX402_STORE_BACKEND).as_deref() {
            Ok("ipfs") => StorageBackend::Ipfs,
            Ok("arweave") => StorageBackend::Arweave,
            _ => StorageBackend::S3,
        };

        let default_retention = match std::env::var(DX402_RETENTION).as_deref() {
            Ok("1y") => Retention::Year1,
            Ok("permanent") => Retention::Permanent,
            _ => Retention::Days90,
        };

        Self {
            enabled,
            backend,
            bucket: std::env::var(DX402_STORE_BUCKET)
                .ok()
                .filter(|v| !v.is_empty()),
            public_base: std::env::var(DX402_STORE_PUBLIC_BASE)
                .ok()
                .filter(|v| !v.is_empty()),
            registry_table: std::env::var(DX402_REGISTRY_TABLE_NAME)
                .ok()
                .filter(|v| !v.is_empty()),
            default_retention,
        }
    }
}

/// The facilitator-side DX402 service.
#[derive(Clone)]
pub struct Dx402Service {
    config: Dx402Config,
    registry: Arc<dyn EvidenceRegistry>,
    store: Arc<dyn EvidenceStore>,
    signer: Arc<PrivateKeySigner>,
}

impl std::fmt::Debug for Dx402Service {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dx402Service")
            .field("config", &self.config)
            .field("registry", &self.registry)
            .field("store", &self.store)
            .field("signer", &self.signer.address())
            .finish()
    }
}

impl Dx402Service {
    pub fn new(
        config: Dx402Config,
        registry: Arc<dyn EvidenceRegistry>,
        store: Arc<dyn EvidenceStore>,
        signer: Arc<PrivateKeySigner>,
    ) -> Self {
        Self {
            config,
            registry,
            store,
            signer,
        }
    }

    /// Build from the environment, or return `None` when DX402 is off.
    ///
    /// Missing or unusable configuration disables the feature and logs why. It
    /// never falls back to something that merely looks like it works -- an
    /// in-memory "durable" store in production would report evidence for data
    /// that dies with the process.
    pub async fn from_env() -> Option<Self> {
        let config = Dx402Config::from_env();
        if !config.enabled {
            return None;
        }

        let signing_key = std::env::var(super::env::DX402_SIGNING_KEY)
            .ok()
            .filter(|v| !v.is_empty());
        let Some(signing_key) = signing_key else {
            warn!(
                "{} is set but {} is empty -- DX402 stays off (receipts could not be signed)",
                super::env::ENABLE_DX402,
                super::env::DX402_SIGNING_KEY
            );
            return None;
        };
        let signer: PrivateKeySigner = match signing_key.parse() {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "DX402 signing key is unparseable -- DX402 stays off");
                return None;
            }
        };

        if config.backend != StorageBackend::S3 {
            warn!(
                backend = %config.backend,
                "only the s3 backend is implemented in v0.1 -- DX402 stays off"
            );
            return None;
        }
        let (Some(bucket), Some(public_base)) = (config.bucket.clone(), config.public_base.clone())
        else {
            warn!(
                "{} and {} are both required for the s3 backend -- DX402 stays off",
                super::env::DX402_STORE_BUCKET,
                super::env::DX402_STORE_PUBLIC_BASE
            );
            return None;
        };

        let store = Arc::new(S3EvidenceStore::from_env(bucket, public_base).await);

        let registry: Arc<dyn EvidenceRegistry> = match &config.registry_table {
            Some(table) => Arc::new(DynamoEvidenceRegistry::from_env(table.clone()).await),
            None => {
                warn!(
                    "{} is unset -- falling back to the in-memory index (table defaults to {}). \
                     Sealed evidence is still durable; only lookup by paymentId is lost on restart.",
                    super::env::DX402_REGISTRY_TABLE_NAME,
                    DEFAULT_REGISTRY_TABLE_NAME
                );
                Arc::new(MemoryEvidenceRegistry::new())
            }
        };

        info!(
            backend = %config.backend,
            retention = %config.default_retention,
            receipt_signer = %signer.address(),
            "DX402 durable-evidence enabled"
        );

        Some(Self::new(config, registry, store, Arc::new(signer)))
    }

    /// A service backed entirely by memory. Tests only.
    pub fn in_memory(signer: PrivateKeySigner) -> Self {
        Self::new(
            Dx402Config {
                enabled: true,
                ..Dx402Config::default()
            },
            Arc::new(MemoryEvidenceRegistry::new()),
            Arc::new(MemoryEvidenceStore::new()),
            Arc::new(signer),
        )
    }

    pub fn config(&self) -> &Dx402Config {
        &self.config
    }

    /// The address a third party checks receipt signatures against.
    pub fn receipt_signer(&self) -> alloy::primitives::Address {
        self.signer.address()
    }

    pub fn store(&self) -> &Arc<dyn EvidenceStore> {
        &self.store
    }

    /// Record an anchor reported by a resource server, and notarise it.
    ///
    /// The `chain_id` binds the receipt to the settlement chain, so evidence for
    /// a testnet payment cannot be presented as mainnet evidence.
    pub async fn anchor(
        &self,
        req: AnchorRequest,
        chain_id: u64,
        now: u64,
    ) -> Result<AnchoredEvidence, Dx402ErrorCode> {
        let retention_until = req.retention.until(now);

        let receipt_body = EvidenceReceipt {
            payment_id: req.payment_id.clone(),
            content_hash: req.content_hash.clone(),
            pointer: req.pointer.clone(),
            payer: req.payer.clone(),
            payee: req.payee.clone(),
            tx_hash: req.tx_hash.clone(),
            network: req.network,
            mode: req.mode,
            anchored_at: now,
            retention_until,
        };

        let signature = receipt::sign(&receipt_body, &self.signer, chain_id).map_err(|e| {
            warn!(error = %e, payment_id = %req.payment_id, "DX402 receipt signing failed");
            Dx402ErrorCode::Dx402StoreUnavailable
        })?;

        let record = EvidenceRecord {
            payment_id: req.payment_id.clone(),
            pointer: req.pointer.clone(),
            backend: req.backend,
            content_hash: req.content_hash.clone(),
            key_alg: req.key_alg,
            mode: req.mode,
            retention: req.retention,
            anchored_at: now,
            retention_until,
            receipt: receipt_body,
            signature: signature.clone(),
            // Only carried in `escrowed` mode. In `direct` mode this stays None,
            // which is what makes a leak of the index harmless.
            wrapped_cek: match req.mode {
                EvidenceMode::Escrowed => req.wrapped_cek.clone(),
                EvidenceMode::Direct => None,
            },
        };

        self.registry.put(&record).await.map_err(|e| {
            warn!(error = %e, payment_id = %req.payment_id, "DX402 registry write failed");
            Dx402ErrorCode::Dx402StoreUnavailable
        })?;

        Ok(AnchoredEvidence {
            v: DX402_VERSION,
            payment_id: req.payment_id,
            pointer: req.pointer,
            backend: req.backend,
            content_hash: req.content_hash,
            cipher: "AES-256-GCM".to_string(),
            key_alg: req.key_alg,
            mode: req.mode,
            retention: req.retention,
            receipt: Some(signature),
        })
    }

    /// Look up recorded evidence.
    pub async fn lookup(
        &self,
        payment_id: &str,
        now: u64,
    ) -> Result<EvidenceRecord, Dx402ErrorCode> {
        let record = self.registry.get(payment_id).await.map_err(|e| match e {
            RegistryError::NotFound => Dx402ErrorCode::Dx402UnknownPayment,
            RegistryError::Unavailable(_) => Dx402ErrorCode::Dx402StoreUnavailable,
        })?;

        // "It expired" and "it never existed" are different answers to a
        // dispute, so they get different codes.
        if record.is_expired(now) {
            return Err(Dx402ErrorCode::Dx402EvidenceExpired);
        }
        Ok(record)
    }

    /// How many anchors this facilitator has recorded.
    pub async fn count(&self) -> u64 {
        self.registry.count().await.unwrap_or(0)
    }

    /// Fetch the sealed blob itself.
    ///
    /// Returns ciphertext. Serving it to anyone who asks is safe by design: in
    /// `direct` mode it is unreadable without the payer's private key, so the
    /// access control lives in the cryptography rather than in an ACL that could
    /// be misconfigured.
    pub async fn fetch_sealed(
        &self,
        payment_id: &str,
        now: u64,
    ) -> Result<Vec<u8>, Dx402ErrorCode> {
        let record = self.lookup(payment_id, now).await?;
        self.store.get(&record.pointer).await.map_err(|e| match e {
            StoreError::NotFound => Dx402ErrorCode::Dx402UnknownPayment,
            StoreError::Expired => Dx402ErrorCode::Dx402EvidenceExpired,
            _ => Dx402ErrorCode::Dx402StoreUnavailable,
        })
    }
}

/// The EIP-712 `chainId` a receipt for `network` is signed under.
///
/// EVM chains use their real chain id. Non-EVM families have no `eip155`
/// reference, so they get `0` -- the receipt's binding to a specific settlement
/// comes from `paymentId` and `txHash`, not from this field. What matters is
/// only that it is *stable*, so a receipt signed today still verifies tomorrow.
pub fn chain_id_of(network: crate::network::Network) -> u64 {
    crate::caip2::Caip2NetworkId::parse(&network.to_caip2())
        .ok()
        .and_then(|id| id.chain_id())
        .unwrap_or(0)
}

/// Build the `durable-evidence` extension value for a settle response.
///
/// Never returns an error: a settle response that could not carry evidence still
/// carries the settlement, and the skip reason explains itself.
pub fn extension_value(evidence: Option<AnchoredEvidence>, disabled: bool) -> DurableEvidence {
    match (evidence, disabled) {
        (Some(a), _) => DurableEvidence::Anchored(Box::new(a)),
        (None, true) => DurableEvidence::skipped(SkipReason::Disabled),
        (None, false) => DurableEvidence::skipped(SkipReason::AnchorFailed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dx402::types::{DurablePointer, KeyAlg};
    use crate::network::Network;

    fn addr(s: &str) -> crate::types::MixedAddress {
        serde_json::from_value(serde_json::Value::String(s.to_string())).unwrap()
    }

    fn anchor_request(mode: EvidenceMode) -> AnchorRequest {
        AnchorRequest {
            payment_id: format!("0x{}", "11".repeat(32)),
            network: Network::Base,
            tx_hash: format!("0x{}", "33".repeat(32)),
            payer: addr("0x103040545AC5031A11E8C03dd11324C7333a13C7"),
            payee: addr("0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8"),
            pointer: DurablePointer("mem://x".into()),
            backend: StorageBackend::S3,
            content_hash: format!("0x{}", "22".repeat(32)),
            key_alg: KeyAlg::Secp256k1,
            mode,
            retention: Retention::Days90,
            wrapped_cek: Some("0xdeadbeef".into()),
        }
    }

    #[tokio::test]
    async fn anchoring_produces_a_verifiable_receipt() {
        let signer = PrivateKeySigner::random();
        let expected = signer.address();
        let svc = Dx402Service::in_memory(signer);

        let out = svc
            .anchor(anchor_request(EvidenceMode::Direct), 8453, 1_000)
            .await
            .unwrap();

        let record = svc.lookup(&out.payment_id, 1_000).await.unwrap();
        assert!(receipt::verify(
            &record.receipt,
            &record.signature,
            expected,
            8453
        ));
    }

    #[tokio::test]
    async fn direct_mode_never_stores_key_material() {
        // The seller may send a wrapped CEK by mistake. In `direct` mode it must
        // be dropped on the floor -- keeping it would quietly turn an
        // end-to-end payment into an escrowed one while the receipt still said
        // `direct`.
        let svc = Dx402Service::in_memory(PrivateKeySigner::random());
        let out = svc
            .anchor(anchor_request(EvidenceMode::Direct), 8453, 1_000)
            .await
            .unwrap();
        let record = svc.lookup(&out.payment_id, 1_000).await.unwrap();
        assert!(record.wrapped_cek.is_none());
    }

    #[tokio::test]
    async fn escrowed_mode_retains_the_wrapped_key() {
        let svc = Dx402Service::in_memory(PrivateKeySigner::random());
        let out = svc
            .anchor(anchor_request(EvidenceMode::Escrowed), 8453, 1_000)
            .await
            .unwrap();
        let record = svc.lookup(&out.payment_id, 1_000).await.unwrap();
        assert_eq!(record.wrapped_cek.as_deref(), Some("0xdeadbeef"));
        assert_eq!(record.receipt.mode, EvidenceMode::Escrowed);
    }

    #[tokio::test]
    async fn expired_evidence_is_distinguished_from_missing() {
        let svc = Dx402Service::in_memory(PrivateKeySigner::random());
        let out = svc
            .anchor(anchor_request(EvidenceMode::Direct), 8453, 1_000)
            .await
            .unwrap();

        assert_eq!(
            svc.lookup(&out.payment_id, 1_000 + 90 * 86_400 + 1)
                .await
                .unwrap_err(),
            Dx402ErrorCode::Dx402EvidenceExpired
        );
        assert_eq!(
            svc.lookup("0xnever-existed", 1_000).await.unwrap_err(),
            Dx402ErrorCode::Dx402UnknownPayment
        );
    }

    #[tokio::test]
    async fn the_counter_reflects_anchors() {
        let svc = Dx402Service::in_memory(PrivateKeySigner::random());
        assert_eq!(svc.count().await, 0);
        svc.anchor(anchor_request(EvidenceMode::Direct), 8453, 1_000)
            .await
            .unwrap();
        assert_eq!(svc.count().await, 1);
    }

    #[test]
    fn dx402_is_off_unless_explicitly_enabled() {
        // Guards the promise that linking this module in changes nothing.
        temp_env_absent(super::super::env::ENABLE_DX402, || {
            assert!(!Dx402Config::from_env().enabled);
        });
    }

    #[test]
    fn a_skipped_extension_value_still_serializes() {
        let v = extension_value(None, true);
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["skipped"], "disabled");
    }

    /// Run `f` with `key` removed from the environment, then restore it.
    fn temp_env_absent<F: FnOnce()>(key: &str, f: F) {
        let prior = std::env::var(key).ok();
        std::env::remove_var(key);
        f();
        if let Some(v) = prior {
            std::env::set_var(key, v);
        }
    }
}
