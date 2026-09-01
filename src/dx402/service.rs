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
    /// Pinata credential. Present -> the ipfs backends can be offered.
    pub pinata_jwt: Option<String>,
    /// The account's own gateway domain.
    pub pinata_gateway: Option<String>,
    /// Whether `ipfs-public` is offered at all. See `DX402_ALLOW_PUBLIC_IPFS`.
    pub allow_public_ipfs: bool,
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
            pinata_jwt: None,
            pinata_gateway: None,
            allow_public_ipfs: false,
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
            pinata_jwt: std::env::var(DX402_PINATA_JWT)
                .ok()
                .filter(|v| !v.is_empty()),
            pinata_gateway: std::env::var(DX402_PINATA_GATEWAY)
                .ok()
                .filter(|v| !v.is_empty()),
            allow_public_ipfs: std::env::var(DX402_ALLOW_PUBLIC_IPFS)
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
        }
    }

    /// What this deployment can actually offer, in the order a caller sees it.
    ///
    /// Derived from configuration rather than declared, so it cannot promise a
    /// backend whose credential is missing -- the failure mode this whole
    /// mechanism exists to prevent.
    pub fn offers(&self) -> Vec<super::types::BackendOffer> {
        use super::types::BackendOffer;
        let has_pinata = self.pinata_jwt.is_some() && self.pinata_gateway.is_some();
        let retention = self.default_retention.to_string();
        vec![
            BackendOffer {
                id: "s3".into(),
                retention: retention.clone(),
                revocable: true,
                public: false,
                enabled: self.bucket.is_some() && self.public_base.is_some(),
                disabled_reason: (self.bucket.is_none() || self.public_base.is_none())
                    .then(|| "no bucket configured".to_string()),
            },
            BackendOffer {
                id: "ipfs-private".into(),
                retention,
                revocable: true,
                public: false,
                enabled: has_pinata,
                disabled_reason: (!has_pinata).then(|| "no pinata credential".to_string()),
            },
            BackendOffer {
                id: "ipfs-public".into(),
                // Not the configured default: public IPFS cannot expire.
                retention: "permanent".into(),
                revocable: false,
                public: true,
                enabled: has_pinata && self.allow_public_ipfs,
                disabled_reason: if !has_pinata {
                    Some("no pinata credential".to_string())
                } else if !self.allow_public_ipfs {
                    // Named precisely: it is not missing config, it is a
                    // deliberate hold until the buyer -- whose ciphertext this
                    // makes permanent -- can consent through `accepts`.
                    Some("irreversible; awaiting buyer opt-in".to_string())
                } else {
                    None
                },
            },
        ]
    }

    /// Whether this configuration can actually produce a working service.
    ///
    /// `enabled` alone is not enough: the flag can be on while the bucket or the
    /// public base is missing, in which case `Dx402Service::from_env` returns
    /// `None`, the `/dx402/*` routes are never registered, and every one of them
    /// 404s. Advertising the extension off the flag alone therefore announced a
    /// capability that was not there -- exactly what the comment on
    /// `get_supported` says it exists to prevent.
    ///
    /// Both the construction path and the advertisement read THIS, so they
    /// cannot drift apart.
    /// Whether this deployment can actually write to `backend`.
    ///
    /// Narrower than [`is_serviceable`], which asks whether the extension can
    /// run at all. This asks whether one specific request can be honoured.
    ///
    /// [`is_serviceable`]: Dx402Config::is_serviceable
    pub fn serves_backend(&self, backend: StorageBackend) -> bool {
        match backend {
            // Always writable when the extension is on: S3 config is required
            // even on the ipfs backend, because it is the fallback.
            StorageBackend::S3 => true,
            StorageBackend::Ipfs => {
                self.backend == StorageBackend::Ipfs
                    && self.pinata_jwt.is_some()
                    && self.pinata_gateway.is_some()
            }
            // No implementation. Accepting it would record a store that has
            // never held a byte.
            StorageBackend::Arweave => false,
        }
    }

    pub fn is_serviceable(&self) -> bool {
        if !self.enabled || self.bucket.is_none() || self.public_base.is_none() {
            // S3 config is required even for the ipfs backend: it is the
            // fallback, so without it a Pinata outage would lose evidence.
            return false;
        }
        match self.backend {
            StorageBackend::S3 => true,
            StorageBackend::Ipfs => self.pinata_jwt.is_some() && self.pinata_gateway.is_some(),
            // No implementation, so it must not be advertised.
            StorageBackend::Arweave => false,
        }
    }
}

/// The largest sealed blob an anchor request can carry.
///
/// Not a storage limit -- it is what survives the facilitator's own 64 KiB
/// request-body cap once the ciphertext is base64'd (x4/3) and wrapped in the
/// anchor JSON. Both SDKs already measure the SERIALIZED REQUEST against 65536
/// rather than the plaintext, which is the right check; this is the server side
/// of the same rule, so a caller that skips it gets an answer naming the limit
/// instead of a bare 413 from a middleware that has never heard of DX402.
///
/// Deliberately below the arithmetic ceiling (~48.7 KB with minimal metadata,
/// ~48.1 KB once `sellerSignature` and `proofOfPayment` are present): the band
/// moves with the metadata, so the published number has to clear the widest
/// request, not the narrowest.
///
/// **It does not cover every oversize request, and cannot.** The body limit is
/// the OUTERMOST layer on the whole router (`main.rs`, and its comment says the
/// position is deliberate), so a request above 64 KiB is cut before any handler
/// runs and still gets the bare 413. What this covers is the band a seller
/// actually lands in when they are merely over the line -- roughly 48 KB to
/// 64 KiB -- which is where the confusing failures come from. Anything far
/// above is unambiguous on its own.
pub const MAX_SEALED_BLOB_BYTES: usize = 48_000;

/// What auditing one anchor found, and what was done about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairOutcome {
    /// The pointer resolves. Nothing to do.
    Healthy,
    /// The pointer named nothing, the bytes were found elsewhere, and the
    /// record now names where they actually are.
    Repaired,
    /// The pointer named nothing and the bytes were found -- but this was an
    /// audit, so nothing was written. Kept distinct from `Repaired` so a report
    /// cannot be mistaken for a repair that happened.
    Repairable,
    /// The pointer named nothing and no store holds the bytes. Reported, never
    /// papered over: a record that points at a real absence is telling the
    /// truth, and rewriting it would only hide that the evidence is gone.
    Lost,
}

/// The facilitator-side DX402 service.
#[derive(Clone)]
pub struct Dx402Service {
    config: Dx402Config,
    registry: Arc<dyn EvidenceRegistry>,
    store: Arc<dyn EvidenceStore>,
    signer: Arc<PrivateKeySigner>,
    /// RPC access, for verifying that an anchor describes a real payment.
    ///
    /// Optional so the service is still constructible in tests and on a
    /// deployment with no EVM providers. Absent, the gate reports
    /// `RpcUnavailable` -- which never blocks, because "we could not check" must
    /// not be recorded as "this anchor is fraudulent".
    providers: Option<Arc<crate::provider_cache::ProviderCache>>,
}

impl std::fmt::Debug for Dx402Service {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dx402Service")
            .field("config", &self.config)
            .field("registry", &self.registry)
            .field("store", &self.store)
            .field("signer", &self.signer.address())
            .field("providers", &self.providers.is_some())
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
            providers: None,
        }
    }

    /// Attach the provider cache so the anchor gate can verify payments.
    pub fn with_providers(mut self, providers: Arc<crate::provider_cache::ProviderCache>) -> Self {
        self.providers = Some(providers);
        self
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

        // Same predicate the /supported advertisement reads, so a deployment can
        // never announce an extension whose routes were not registered.
        if !config.is_serviceable() {
            warn!(
                backend = %config.backend,
                has_bucket = config.bucket.is_some(),
                has_public_base = config.public_base.is_some(),
                "DX402 is enabled but not serviceable -- it stays off, and \
                 /supported will not advertise it"
            );
            return None;
        }
        let (Some(bucket), Some(public_base)) = (config.bucket.clone(), config.public_base.clone())
        else {
            unreachable!("is_serviceable() guarantees both are present")
        };

        // S3 is always built: it is the default AND the fallback. Pinata sits in
        // front of it when configured, so a Pinata outage costs latency, never
        // the evidence -- and never upgrades a revocable promise into an
        // irrevocable one, because the fallback is the more conservative store.
        let s3: Arc<dyn EvidenceStore> =
            Arc::new(S3EvidenceStore::from_env(bucket, public_base.clone()).await);

        let store: Arc<dyn EvidenceStore> = match config.backend {
            StorageBackend::Ipfs => {
                match (config.pinata_jwt.clone(), config.pinata_gateway.clone()) {
                    (Some(jwt), Some(gateway)) => {
                        let public_base_for_sweeper = public_base.clone();
                        use super::store_pinata::{
                            FallbackEvidenceStore, PinataEvidenceStore, PinataNetwork,
                        };
                        // Private: the only network on which `retentionUntil`
                        // -- which we sign -- can actually be honoured.
                        let pinata: Arc<dyn EvidenceStore> = Arc::new(PinataEvidenceStore::new(
                            jwt,
                            gateway,
                            public_base,
                            PinataNetwork::Private,
                        ));
                        info!("DX402 storage: pinata (private) with an s3 fallback");
                        // Pinata expires nothing on its own. Without this the
                        // `retentionUntil` in every receipt we sign would never
                        // come true on this backend.
                        super::store_pinata::spawn_retention_sweeper(
                            Arc::new(PinataEvidenceStore::new(
                                config.pinata_jwt.clone().unwrap_or_default(),
                                config.pinata_gateway.clone().unwrap_or_default(),
                                public_base_for_sweeper,
                                PinataNetwork::Private,
                            )),
                            3600,
                        );
                        Arc::new(FallbackEvidenceStore::new(pinata, s3))
                    }
                    _ => {
                        // Refuse rather than quietly serve S3: a seller that
                        // asked for IPFS and silently got something else builds
                        // on a promise nobody made.
                        warn!(
                            "{} is `ipfs` but {} / {} are not set -- DX402 stays off",
                            super::env::DX402_STORE_BACKEND,
                            super::env::DX402_PINATA_JWT,
                            super::env::DX402_PINATA_GATEWAY,
                        );
                        return None;
                    }
                }
            }
            _ => s3,
        };

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

    /// Run the anchor gate: does this anchor describe a payment that happened?
    ///
    /// Returns the verdict rather than acting on it, so the caller decides
    /// whether phase 1 (report) or phase 2 (reject) applies. Never returns an
    /// error -- an unverifiable anchor is a verdict, not a crash.
    async fn evaluate_gate(&self, req: &AnchorRequest) -> Option<super::gate::AnchorRejection> {
        use super::gate::{verify_anchor, AnchorClaim, AnchorRejection};
        use crate::chain::evm::MetaEvmProvider;
        use crate::provider_cache::ProviderMap;

        let Some(providers) = self.providers.as_ref() else {
            return Some(AnchorRejection::RpcUnavailable);
        };
        // Only EVM chains carry a receipt this gate can read. The others get
        // `UnverifiableChain`, which is reported and never enforced -- rejecting
        // a check that cannot run would silently disable DX402 on Solana, NEAR,
        // Stellar and Algorand.
        let Some(crate::chain::NetworkProvider::Evm(provider)) = providers.by_network(req.network)
        else {
            return Some(AnchorRejection::UnverifiableChain);
        };

        let (Ok(payment_id), Ok(content_hash)) = (
            req.payment_id.parse::<alloy::primitives::B256>(),
            req.content_hash.parse::<alloy::primitives::B256>(),
        ) else {
            return Some(AnchorRejection::Payment("malformed identifiers".into()));
        };

        let claim = AnchorClaim {
            network: req.network,
            proof: req.proof_of_payment.as_ref(),
            sealed_to: &req.payer,
            payment_id,
            content_hash,
            pointer: req.pointer.as_ref().map(|p| p.as_str()).unwrap_or(""),
            seller_signature: req.seller_signature.as_deref(),
            chain_id: chain_id_of(req.network),
        };

        verify_anchor(provider.inner(), &claim).await.err()
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
        // The gate. Phase 1 (`DX402_REQUIRE_PROOF=false`, the default) verifies
        // and reports; phase 2 rejects. Two verdicts never block in either
        // phase -- see `AnchorRejection::is_enforceable`.
        let gate_verdict = self.evaluate_gate(&req).await;
        if let Some(rejection) = gate_verdict.as_ref() {
            let enforced = super::gate::require_proof() && rejection.is_enforceable();
            if enforced {
                warn!(
                    payment_id = %req.payment_id,
                    verdict = rejection.as_str(),
                    "DX402 anchor REJECTED by the proof gate"
                );
                return Err(Dx402ErrorCode::Dx402ProofRejected);
            }
            warn!(
                payment_id = %req.payment_id,
                verdict = rejection.as_str(),
                enforceable = rejection.is_enforceable(),
                "DX402 anchor would be rejected by the proof gate (phase 1: reporting only)"
            );
        }

        // Two different questions, deliberately kept apart. Collapsing them is
        // what produced the critical below.
        //
        // 1. DIAGNOSTIC: does the signature match the payee the CALLER declared?
        //    This tells a seller its digest form is right (the forms differ by
        //    payee curve, and signing the wrong one produces a signature that
        //    never verifies with no error). It proves NOTHING about authorship,
        //    because the caller also chose `req.payee`.
        let signed_pointer = req.pointer.as_ref().map(|p| p.as_str()).unwrap_or("");
        let signature_matches_declared_payee = match (
            req.seller_signature.as_deref(),
            req.payment_id.parse::<alloy::primitives::B256>(),
            req.content_hash.parse::<alloy::primitives::B256>(),
        ) {
            (Some(signature), Ok(payment_id), Ok(content_hash)) => {
                super::gate::verify_authorization_for(
                    &req.payee,
                    signature,
                    payment_id,
                    content_hash,
                    signed_pointer,
                    chain_id,
                )
            }
            _ => false,
        };

        // 2. FINALITY: `verified` makes a record unsupersedable, so it may only
        //    come from the gate, which checks the signature against the payee it
        //    read OFF THE CHAIN.
        //
        //    This used to be question 1's answer. That made finality
        //    self-asserted: proving "I control the address I typed into my own
        //    request" was enough. Any observer of a settlement can compute the
        //    paymentId (keccak256(caip2 || txHash), all public), declare its own
        //    address as payee, sign with its own key, and permanently own a
        //    stranger's evidence slot -- and, worse than the hijack v1.82.0 was
        //    written to fix, SUPERSEDE the real seller's record. Found by an
        //    audit of that very fix, 2026-08-19.
        let verified = gate_verdict.is_none();

        // Say WHY when we did not certify it. Silence here is how the wrong
        // digest form stayed invisible for two releases: a 201 that looks
        // perfect while the anchor is provisional forever.
        let not_verified_reason = gate_verdict.as_ref().map(|r| r.as_str().to_string());

        if !verified {
            warn!(
                payment_id = %req.payment_id,
                verdict = gate_verdict.as_ref().map(|r| r.as_str()).unwrap_or("none"),
                signature_matches_declared_payee,
                "DX402 anchor is PROVISIONAL: authorship was not proven against the chain. \
                 A gate-verified anchor for this payment will supersede it."
            );
        }

        // Believe the store, not the request. `backend` is free text the caller
        // supplies; accepting one we cannot write to meant persisting -- and
        // serving from `/dx402/evidence` -- a claim about where somebody's
        // evidence lives that was never true. Arweave is the sharp case: it has
        // no implementation at all, so a record could name a store that has
        // never held a single byte.
        if !self.config.serves_backend(req.backend) {
            warn!(
                payment_id = %req.payment_id,
                requested = %req.backend,
                serving = %self.config.backend,
                "DX402 anchor asked for a backend this deployment does not write to"
            );
            return Err(Dx402ErrorCode::Dx402BackendUnavailable);
        }

        let retention_until = req.retention.until(now);

        // Decode the blob but do NOT write it yet.
        //
        // The write order here is load-bearing. `put_object` is unconditional,
        // the bucket has versioning deliberately Disabled (a retention promise
        // that keeps noncurrent versions is not a retention promise), and the
        // key is derived from `paymentId` -- which is keccak256 over public
        // data. Uploading before the registry decides meant any anonymous
        // caller could POST an already-anchored paymentId with garbage,
        // irreversibly destroy the real ciphertext, and receive a tidy 409 as
        // if nothing had happened. The bytes were gone, the recorded
        // contentHash could never be reproduced again, and the retention tag
        // had been rewritten too. Found by an audit, 2026-08-19.
        //
        // So: decode (cheap, no side effect), reserve the slot, and only the
        // caller that actually won it gets to write bytes.
        let sealed_blob = match &req.sealed {
            Some(encoded) => {
                use base64::Engine as _;
                let blob = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .or_else(|_| {
                        base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded)
                    })
                    .map_err(|e| {
                        warn!(error = %e, payment_id = %req.payment_id, "DX402 sealed blob is not base64");
                        Dx402ErrorCode::Dx402StoreUnavailable
                    })?;
                if blob.len() > MAX_SEALED_BLOB_BYTES {
                    warn!(
                        payment_id = %req.payment_id,
                        sealed_bytes = blob.len(),
                        limit = MAX_SEALED_BLOB_BYTES,
                        "DX402 sealed blob is over what an anchor request can carry"
                    );
                    return Err(Dx402ErrorCode::Dx402SealedTooLarge);
                }
                Some(blob)
            }
            None => None,
        };

        // The pointer is known before the upload: for a blob we host, it is
        // derived from the paymentId, so the record can be written first.
        let pointer = match (&sealed_blob, &req.pointer) {
            (Some(blob), _) => self.store.pointer_for(&req.payment_id, blob),
            (None, Some(p)) => p.clone(),
            (None, None) => {
                warn!(
                    payment_id = %req.payment_id,
                    "DX402 anchor carried neither `sealed` nor `pointer`"
                );
                return Err(Dx402ErrorCode::Dx402StoreUnavailable);
            }
        };

        let receipt_body = EvidenceReceipt {
            payment_id: req.payment_id.clone(),
            content_hash: req.content_hash.clone(),
            pointer: pointer.clone(),
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
            pointer: pointer.clone(),
            backend: req.backend,
            content_hash: req.content_hash.clone(),
            key_alg: req.key_alg,
            mode: req.mode,
            retention: req.retention,
            anchored_at: now,
            retention_until,
            receipt: receipt_body,
            signature: signature.clone(),
            verified,
            signed: signature_matches_declared_payee,
            // Filled in by the correction after the upload: the store that
            // takes the bytes is the only one that knows its own handle.
            reference: None,
            // Only carried in `escrowed` mode. In `direct` mode this stays None,
            // which is what makes a leak of the index harmless.
            wrapped_cek: match req.mode {
                EvidenceMode::Escrowed => req.wrapped_cek.clone(),
                EvidenceMode::Direct => None,
            },
        };

        let signed_but_unverified =
            req.seller_signature.is_some() && !signature_matches_declared_payee;
        let claim = self.registry.put(&record).await.map_err(|e| match e {
            // Losing to an existing record has two causes that do not look
            // alike, and answering both with "already anchored" points the
            // caller at the wrong one. If they signed and the signature did not
            // verify, THAT is why they could not supersede -- say so.
            RegistryError::AlreadyAnchored if signed_but_unverified => {
                warn!(
                    payment_id = %req.payment_id,
                    "DX402 anchor refused: the sellerSignature did not verify against the payee, \
                     so it could not supersede the record already holding this payment"
                );
                Dx402ErrorCode::Dx402SignatureNotVerified
            }
            RegistryError::AlreadyAnchored => {
                warn!(
                    payment_id = %req.payment_id,
                    "DX402 anchor refused: this payment already has evidence"
                );
                Dx402ErrorCode::Dx402AlreadyAnchored
            }
            other => {
                warn!(error = %other, payment_id = %req.payment_id, "DX402 registry write failed");
                Dx402ErrorCode::Dx402StoreUnavailable
            }
        })?;

        // Slot won -- now, and only now, write the bytes. A caller that did not
        // earn the slot never reaches this line, so it cannot overwrite evidence
        // it does not own.
        //
        // If this upload fails the record exists with no blob behind it:
        // `/dx402/blob` will 404 for that payment. That is a visible, honest
        // degradation, and strictly better than the alternative it replaces --
        // silently destroying somebody else's ciphertext.
        let mut pointer = pointer;
        let mut backend = req.backend;
        let mut signature = signature;
        if let Some(blob) = sealed_blob {
            let stored = self
                .store
                .put(&req.payment_id, &blob, req.retention)
                .await
                .map_err(|e| {
                    warn!(error = %e, payment_id = %req.payment_id, "DX402 blob upload failed after the slot was reserved");
                    Dx402ErrorCode::Dx402StoreUnavailable
                })?;

            // The pointer was a PREDICTION until this returned. A composed
            // store names its primary and may write to its fallback, so one
            // Pinata blip used to leave a receipt WE SIGNED naming an object
            // that never existed -- unreadable forever, with no error anywhere.
            //
            // The correction goes strictly below the anti-replay: the slot was
            // already won above, and the fence lets this replace only the row
            // this call wrote.
            let corrected = EvidenceRecord {
                pointer: stored.pointer.clone(),
                // Measured, not declared. `backend` on the request is free text
                // the caller supplies and nothing validates.
                backend: stored.backend,
                reference: stored.reference.clone(),
                ..record.clone()
            };

            if corrected.pointer != record.pointer
                || corrected.backend != record.backend
                || corrected.reference != record.reference
            {
                // Re-sign: `pointer` is the third field of the EIP-712 struct,
                // so a corrected pointer with the old signature is a receipt
                // that does not verify. `backend` is NOT in the type hash, so a
                // backend-only correction keeps the original signature.
                let mut corrected = corrected;
                if corrected.pointer != record.pointer {
                    corrected.receipt.pointer = stored.pointer.clone();
                    signature = receipt::sign(&corrected.receipt, &self.signer, chain_id)
                        .map_err(|e| {
                            warn!(error = %e, payment_id = %req.payment_id, "DX402 receipt re-signing failed");
                            Dx402ErrorCode::Dx402StoreUnavailable
                        })?;
                    corrected.signature = signature.clone();
                    warn!(
                        payment_id = %req.payment_id,
                        predicted = %record.pointer,
                        actual = %stored.pointer,
                        "dx402_pointer_reconciled -- the write fell back to another store"
                    );
                }

                match self.registry.settle(&corrected, &claim).await {
                    Ok(()) => {}
                    // Somebody with more authority took the slot while we were
                    // uploading. The row is genuinely not ours; do not retry
                    // and do not force.
                    Err(RegistryError::AlreadyAnchored) => {
                        warn!(
                            payment_id = %req.payment_id,
                            "dx402_superseded_after_upload -- a stronger claim took the slot mid-upload"
                        );
                        return Err(Dx402ErrorCode::Dx402AlreadyAnchored);
                    }
                    // The bytes are stored and the receipt is correct; only the
                    // index write failed. Answering 500 would push the seller
                    // into a retry that can only earn a 409, and throw away the
                    // one artifact that is already right and verifies offline.
                    Err(other) => {
                        warn!(
                            error = %other,
                            payment_id = %req.payment_id,
                            "dx402_index_stale -- evidence is correct but the index was not corrected"
                        );
                    }
                }
            }

            pointer = stored.pointer;
            backend = stored.backend;
        }

        Ok(AnchoredEvidence {
            v: DX402_VERSION,
            payment_id: req.payment_id,
            pointer,
            backend,
            content_hash: req.content_hash,
            cipher: "AES-256-GCM".to_string(),
            key_alg: req.key_alg,
            mode: req.mode,
            retention: req.retention,
            receipt: Some(signature),
            verified,
            not_verified_reason,
            signed: signature_matches_declared_payee,
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
            // Unreachable on a read, but spelled out rather than lumped in: a
            // catch-all here would silently mistranslate a future variant.
            RegistryError::AlreadyAnchored | RegistryError::Unavailable(_) => {
                Dx402ErrorCode::Dx402StoreUnavailable
            }
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
    /// Audit one anchor and, if its pointer names nothing, correct it.
    ///
    /// Exists for the 464 anchors written while the pointer was a prediction
    /// nobody reconciled. A fallback write put the bytes in one store while the
    /// record -- and the receipt we SIGNED -- named another, and the read path
    /// treats the resulting `NotFound` as a verdict, so the evidence is
    /// unreachable with no error anywhere to say so.
    ///
    /// Admin-only, and narrow on purpose: it may only rewrite where the bytes
    /// are, never who owns the slot. `verified` and `signed` are carried
    /// structurally from the record read, so a repair cannot escalate authority
    /// even by mistake.
    /// `write` false audits without touching anything, so an operator can see
    /// the damage before deciding to rewrite signed attestations. A dry run
    /// that silently wrote would make the safe-looking invocation the dangerous
    /// one.
    pub async fn repair(
        &self,
        payment_id: &str,
        now: u64,
        write: bool,
    ) -> Result<RepairOutcome, Dx402ErrorCode> {
        let record = self.lookup(payment_id, now).await?;

        if self.store.get(&record.pointer).await.is_ok() {
            return Ok(RepairOutcome::Healthy);
        }

        let Some(found) = self.store.locate(payment_id).await else {
            warn!(
                payment_id,
                pointer = %record.pointer,
                "dx402_evidence_lost -- the pointer resolves to nothing and no store holds the bytes"
            );
            return Ok(RepairOutcome::Lost);
        };

        if !write {
            warn!(
                payment_id,
                recorded = %record.pointer,
                found = %found.pointer,
                "dx402_pointer_repairable -- audit only, nothing written"
            );
            return Ok(RepairOutcome::Repairable);
        }

        let mut corrected = EvidenceRecord {
            pointer: found.pointer.clone(),
            backend: found.backend,
            reference: found.reference.clone(),
            ..record.clone()
        };
        // `pointer` is the third field of the EIP-712 struct, so the old
        // signature does not cover the corrected one. Re-signing is what makes
        // the repaired receipt verifiable rather than merely present.
        corrected.receipt.pointer = found.pointer.clone();
        corrected.signature = receipt::sign(
            &corrected.receipt,
            &self.signer,
            chain_id_of(corrected.receipt.network),
        )
        .map_err(|e| {
            warn!(error = %e, payment_id, "DX402 repair could not re-sign the receipt");
            Dx402ErrorCode::Dx402StoreUnavailable
        })?;

        self.registry
            .repair(&corrected, record.anchored_at)
            .await
            .map_err(|e| match e {
                // The row moved between the audit and the write. Leave it.
                RegistryError::AlreadyAnchored => Dx402ErrorCode::Dx402AlreadyAnchored,
                other => {
                    warn!(error = %other, payment_id, "DX402 repair write failed");
                    Dx402ErrorCode::Dx402StoreUnavailable
                }
            })?;

        warn!(
            payment_id,
            was = %record.pointer,
            now = %found.pointer,
            "dx402_pointer_repaired -- the record named a store the bytes were never in"
        );
        Ok(RepairOutcome::Repaired)
    }

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
    use base64::Engine as _;

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
            pointer: Some(DurablePointer("mem://x".into())),
            sealed: None,
            backend: StorageBackend::S3,
            content_hash: format!("0x{}", "22".repeat(32)),
            key_alg: KeyAlg::Secp256k1,
            mode,
            retention: Retention::Days90,
            proof_of_payment: None,
            seller_signature: None,
            wrapped_cek: Some("0xdeadbeef".into()),
        }
    }

    /// A primary that always fails, so `FallbackEvidenceStore` writes to the
    /// fallback and returns a pointer the prediction never named.
    #[derive(Debug)]
    struct AlwaysFallsBack;

    #[async_trait::async_trait]
    impl crate::dx402::store::EvidenceStore for AlwaysFallsBack {
        fn backend(&self) -> StorageBackend {
            StorageBackend::Ipfs
        }
        fn pointer_for(&self, payment_id: &str, _blob: &[u8]) -> DurablePointer {
            DurablePointer(format!("ipfs+https://gw.test/{payment_id}#bafkreiwrong"))
        }
        async fn put(
            &self,
            _payment_id: &str,
            _blob: &[u8],
            _retention: Retention,
        ) -> Result<crate::dx402::store::StoredObject, crate::dx402::store::StoreError> {
            Err(crate::dx402::store::StoreError::Unavailable(
                "pinata down".into(),
            ))
        }
        async fn get(
            &self,
            _pointer: &DurablePointer,
        ) -> Result<Vec<u8>, crate::dx402::store::StoreError> {
            Err(crate::dx402::store::StoreError::Unavailable(
                "pinata down".into(),
            ))
        }
    }

    /// The shape production runs: Pinata primary, S3 fallback, primary broken.
    fn service_that_falls_back(signer: PrivateKeySigner) -> Dx402Service {
        let store = Arc::new(crate::dx402::store_pinata::FallbackEvidenceStore::new(
            Arc::new(AlwaysFallsBack),
            Arc::new(crate::dx402::store::MemoryEvidenceStore::new()),
        ));
        // The config has to say `ipfs` too, or the request is refused before it
        // reaches the store -- which is production's shape, not a detail: the
        // whole bug only exists on a deployment whose primary can fall back.
        let config = Dx402Config {
            backend: StorageBackend::Ipfs,
            pinata_jwt: Some("test".into()),
            pinata_gateway: Some("gw.test".into()),
            ..Dx402Config::default()
        };
        Dx402Service::new(
            config,
            Arc::new(crate::dx402::registry::MemoryEvidenceRegistry::new()),
            store,
            Arc::new(signer),
        )
    }

    fn sealed_request() -> AnchorRequest {
        AnchorRequest {
            sealed: Some(base64::engine::general_purpose::STANDARD.encode(b"the paid response")),
            pointer: None,
            backend: StorageBackend::Ipfs,
            ..anchor_request(EvidenceMode::Direct)
        }
    }

    #[tokio::test]
    async fn a_backend_the_deployment_cannot_serve_is_refused() {
        // `backend` was free text nothing checked, so a record -- and every
        // later read of `/dx402/evidence` -- could claim the bytes were on
        // Arweave, which has no implementation at all and has never held one.
        let svc = Dx402Service::in_memory(PrivateKeySigner::random());
        for unavailable in [StorageBackend::Arweave, StorageBackend::Ipfs] {
            let err = svc
                .anchor(
                    AnchorRequest {
                        backend: unavailable,
                        ..anchor_request(EvidenceMode::Direct)
                    },
                    8453,
                    1_000,
                )
                .await
                .expect_err("an unservable backend must be refused, not recorded");
            assert!(matches!(err, Dx402ErrorCode::Dx402BackendUnavailable));
        }
        // And the one it does serve still works.
        svc.anchor(anchor_request(EvidenceMode::Direct), 8453, 1_000)
            .await
            .expect("s3 is always writable when the extension is on");
    }

    #[tokio::test]
    async fn an_oversized_sealed_blob_is_named_not_just_cut() {
        // Above the limit the body-limit middleware answers a bare 413 that
        // names no field and never mentions DX402. Inside the band a seller
        // actually lands in, say so ourselves with the real number.
        let svc = Dx402Service::in_memory(PrivateKeySigner::random());
        let too_big = vec![0u8; MAX_SEALED_BLOB_BYTES + 1];
        let err = svc
            .anchor(
                AnchorRequest {
                    sealed: Some(base64::engine::general_purpose::STANDARD.encode(&too_big)),
                    pointer: None,
                    ..anchor_request(EvidenceMode::Direct)
                },
                8453,
                1_000,
            )
            .await
            .expect_err("an oversized sealed blob must be refused by us");
        assert!(matches!(err, Dx402ErrorCode::Dx402SealedTooLarge));

        // One byte under still anchors -- the limit is a limit, not a moat.
        svc.anchor(
            AnchorRequest {
                sealed: Some(
                    base64::engine::general_purpose::STANDARD
                        .encode(vec![0u8; MAX_SEALED_BLOB_BYTES]),
                ),
                pointer: None,
                ..anchor_request(EvidenceMode::Direct)
            },
            8453,
            1_000,
        )
        .await
        .expect("exactly at the limit must be accepted");
    }

    #[tokio::test]
    async fn a_record_naming_a_store_the_bytes_are_not_in_is_repaired() {
        // The 464 anchors this exists for: written while the pointer was a
        // prediction nobody reconciled, so a fallback write left the bytes in
        // one store while the record and the signed receipt named another.
        let signer = PrivateKeySigner::random();
        let expected = signer.address();
        let registry = Arc::new(crate::dx402::registry::MemoryEvidenceRegistry::new());
        let store = Arc::new(crate::dx402::store::MemoryEvidenceStore::new());
        let svc = Dx402Service::new(
            Dx402Config::default(),
            registry.clone(),
            store.clone(),
            Arc::new(signer),
        );

        // Anchor normally, then break the record the way the old code did:
        // point it somewhere the bytes never were.
        let out = svc
            .anchor(
                AnchorRequest {
                    sealed: Some(base64::engine::general_purpose::STANDARD.encode(b"paid bytes")),
                    pointer: None,
                    ..anchor_request(EvidenceMode::Direct)
                },
                8453,
                1_000,
            )
            .await
            .unwrap();
        let good = registry.get(&out.payment_id).await.unwrap();
        let broken = EvidenceRecord {
            pointer: DurablePointer("ipfs+https://gw.test/x#bafkreinever".into()),
            backend: StorageBackend::Ipfs,
            ..good.clone()
        };
        registry.repair(&broken, good.anchored_at).await.unwrap();
        assert!(
            svc.fetch_sealed(&out.payment_id, 1_000).await.is_err(),
            "the setup must actually be broken, or the test proves nothing"
        );

        assert_eq!(
            svc.repair(&out.payment_id, 1_000, true).await.unwrap(),
            RepairOutcome::Repaired
        );

        // The bytes are reachable again...
        let bytes = svc.fetch_sealed(&out.payment_id, 1_000).await.unwrap();
        assert_eq!(bytes, b"paid bytes");
        // ...and the receipt over the corrected pointer still verifies, which
        // is the half that a correction without re-signing would break.
        let fixed = registry.get(&out.payment_id).await.unwrap();
        assert_eq!(fixed.receipt.pointer, fixed.pointer);
        assert!(receipt::verify(
            &fixed.receipt,
            &fixed.signature,
            expected,
            8453
        ));
        // Authority is carried structurally, never recomputed.
        assert_eq!(fixed.verified, good.verified);
        assert_eq!(fixed.signed, good.signed);
    }

    #[tokio::test]
    async fn an_audit_reports_without_rewriting_anything() {
        // Auditing is safe; rewriting a facilitator-signed attestation is not.
        // If the read-only-looking invocation wrote, the safe call would be the
        // dangerous one -- so the dangerous half has to be asked for by name.
        let registry = Arc::new(crate::dx402::registry::MemoryEvidenceRegistry::new());
        let store = Arc::new(crate::dx402::store::MemoryEvidenceStore::new());
        let svc = Dx402Service::new(
            Dx402Config::default(),
            registry.clone(),
            store.clone(),
            Arc::new(PrivateKeySigner::random()),
        );
        let out = svc
            .anchor(
                AnchorRequest {
                    sealed: Some(base64::engine::general_purpose::STANDARD.encode(b"paid")),
                    pointer: None,
                    ..anchor_request(EvidenceMode::Direct)
                },
                8453,
                1_000,
            )
            .await
            .unwrap();
        let good = registry.get(&out.payment_id).await.unwrap();
        let broken = EvidenceRecord {
            pointer: DurablePointer("ipfs+https://gw.test/x#bafkreinever".into()),
            ..good.clone()
        };
        registry.repair(&broken, good.anchored_at).await.unwrap();

        assert_eq!(
            svc.repair(&out.payment_id, 1_000, false).await.unwrap(),
            RepairOutcome::Repairable,
            "an audit must say it COULD fix this, not that it did"
        );
        assert_eq!(
            registry.get(&out.payment_id).await.unwrap().pointer,
            broken.pointer,
            "an audit must leave the record exactly as it found it"
        );
    }

    #[tokio::test]
    async fn a_healthy_anchor_is_left_alone_and_a_lost_one_is_not_invented() {
        let svc = Dx402Service::in_memory(PrivateKeySigner::random());
        let out = svc
            .anchor(
                AnchorRequest {
                    sealed: Some(base64::engine::general_purpose::STANDARD.encode(b"ok")),
                    pointer: None,
                    ..anchor_request(EvidenceMode::Direct)
                },
                8453,
                1_000,
            )
            .await
            .unwrap();
        assert_eq!(
            svc.repair(&out.payment_id, 1_000, true).await.unwrap(),
            RepairOutcome::Healthy,
            "a resolvable pointer must not be rewritten"
        );

        // A record whose bytes are genuinely gone is reported as lost, not
        // repointed at something plausible. A record that names a real absence
        // is telling the truth.
        let svc2 = Dx402Service::in_memory(PrivateKeySigner::random());
        svc2.anchor(anchor_request(EvidenceMode::Direct), 8453, 1_000)
            .await
            .unwrap();
        assert_eq!(
            svc2.repair(&format!("0x{}", "11".repeat(32)), 1_000, true)
                .await
                .unwrap(),
            RepairOutcome::Lost
        );
    }

    #[tokio::test]
    async fn a_pointer_that_fell_back_is_the_one_recorded() {
        // The bug this closes: the pointer was PREDICTED from the primary,
        // signed, and recorded, while the pointer `put` actually returned was
        // discarded. One Pinata blip left a signed pointer naming an object
        // that never existed -- and `get` answers `NotFound` for it, a verdict
        // the fallback store deliberately does not second-guess. Unreadable
        // forever, with our signature on it, and no error anywhere.
        let svc = service_that_falls_back(PrivateKeySigner::random());
        let out = svc.anchor(sealed_request(), 8453, 1_000).await.unwrap();

        assert!(
            out.pointer.as_str().starts_with("mem://"),
            "the response must name where the bytes landed, not the prediction: {}",
            out.pointer
        );
        let record = svc.lookup(&out.payment_id, 1_000).await.unwrap();
        assert_eq!(record.pointer, out.pointer, "the record must agree");
        assert_eq!(
            record.backend,
            StorageBackend::S3,
            "the backend must be the one that took the bytes, not the one the caller declared"
        );
    }

    #[tokio::test]
    async fn the_signed_receipt_names_a_resolvable_pointer() {
        // `pointer` is the third field of the EIP-712 struct, so a corrected
        // pointer carrying the original signature is a receipt that does not
        // verify. Correcting without re-signing would trade one broken artifact
        // for another.
        let signer = PrivateKeySigner::random();
        let expected = signer.address();
        let svc = service_that_falls_back(signer);

        let out = svc.anchor(sealed_request(), 8453, 1_000).await.unwrap();
        let record = svc.lookup(&out.payment_id, 1_000).await.unwrap();

        assert_eq!(record.receipt.pointer, record.pointer);
        assert!(
            receipt::verify(&record.receipt, &record.signature, expected, 8453),
            "the re-signed receipt must still verify against the facilitator key"
        );
        // And the bytes are actually there under that pointer.
        let bytes = svc.fetch_sealed(&out.payment_id, 1_000).await.unwrap();
        assert_eq!(bytes, b"the paid response");
    }

    #[tokio::test]
    async fn the_deletion_reference_is_persisted() {
        // Without this, retention on a backend with no lifecycle rule is a
        // promise with no mechanism: a private IPFS pointer names the payment,
        // not the object, so there is nothing to hand `delete`.
        let svc = service_that_falls_back(PrivateKeySigner::random());
        let out = svc.anchor(sealed_request(), 8453, 1_000).await.unwrap();
        let record = svc.lookup(&out.payment_id, 1_000).await.unwrap();
        assert_eq!(record.reference, None, "memory store needs no handle");
        assert!(record.pointer.as_str().starts_with("mem://"));
    }

    #[tokio::test]
    async fn a_slot_race_loser_still_cannot_overwrite() {
        // The correction goes strictly BELOW the anti-replay, and its fence is
        // narrower than the ladder: it matches only the row this very call
        // wrote. A weaker claim that never won the slot must not reach the
        // bytes at all -- that is the v1.82.0 rule, and the reconcile must not
        // become a second door into it.
        let svc = service_that_falls_back(PrivateKeySigner::random());
        let first = svc.anchor(sealed_request(), 8453, 1_000).await.unwrap();

        let err = svc
            .anchor(sealed_request(), 8453, 2_000)
            .await
            .expect_err("a second equal-authority claim must be refused");
        assert!(matches!(err, Dx402ErrorCode::Dx402AlreadyAnchored));

        let record = svc.lookup(&first.payment_id, 2_000).await.unwrap();
        assert_eq!(record.anchored_at, 1_000, "the winner's row must be intact");
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
    async fn a_seller_with_no_storage_can_anchor_by_sending_the_blob() {
        // The path that matters for adoption: a resource server with no bucket,
        // no credentials and no public object store sends the ciphertext and
        // gets back a pointer it can hand to the buyer.
        use base64::Engine as _;

        let svc = Dx402Service::in_memory(PrivateKeySigner::random());
        let blob = b"DX402\x01\x01pretend-this-is-a-sealed-envelope";

        let req = AnchorRequest {
            pointer: None,
            sealed: Some(base64::engine::general_purpose::STANDARD.encode(blob)),
            ..anchor_request(EvidenceMode::Direct)
        };
        let out = svc.anchor(req, 8453, 1_000).await.unwrap();

        // The facilitator issued the pointer, and the bytes come back verbatim.
        let record = svc.lookup(&out.payment_id, 1_000).await.unwrap();
        assert_eq!(record.pointer, out.pointer);
        assert_eq!(
            svc.fetch_sealed(&out.payment_id, 1_000).await.unwrap(),
            blob
        );
    }

    #[tokio::test]
    async fn an_anchor_with_neither_blob_nor_pointer_is_refused() {
        // Recording an anchor that points at nothing would produce a signed
        // receipt attesting to evidence that does not exist.
        let svc = Dx402Service::in_memory(PrivateKeySigner::random());
        let req = AnchorRequest {
            pointer: None,
            sealed: None,
            ..anchor_request(EvidenceMode::Direct)
        };
        assert!(svc.anchor(req, 8453, 1_000).await.is_err());
    }

    #[tokio::test]
    async fn a_malformed_blob_is_refused_rather_than_stored() {
        let svc = Dx402Service::in_memory(PrivateKeySigner::random());
        let req = AnchorRequest {
            pointer: None,
            sealed: Some("!!! not base64 !!!".into()),
            ..anchor_request(EvidenceMode::Direct)
        };
        assert!(svc.anchor(req, 8453, 1_000).await.is_err());
    }

    /// Sign an anchor the way a real EVM seller would.
    fn sign_as(signer: &PrivateKeySigner, req: &AnchorRequest, chain_id: u64) -> String {
        super::super::gate::sign_authorization(
            signer,
            req.payment_id.parse().unwrap(),
            req.content_hash.parse().unwrap(),
            req.pointer.as_ref().map(|p| p.as_str()).unwrap_or(""),
            chain_id,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn an_unsigned_anchor_cannot_lock_out_the_real_seller() {
        // THE hijack, reported by KarmaKadabra 2026-08-18 and reproduced against
        // production. An observer of a settlement anchors garbage first; the
        // anti-replay then gives the legitimate seller a permanent 409, and in a
        // dispute the artifact that exists is the attacker's.
        //
        // A claim nobody proved is provisional and must yield to one that is.
        let svc = Dx402Service::in_memory(PrivateKeySigner::random());
        let seller = PrivateKeySigner::random();

        // The attacker gets there first, with no proof of anything.
        let mut hijack = anchor_request(EvidenceMode::Direct);
        hijack.content_hash = format!("0x{}", "ee".repeat(32));
        hijack.seller_signature = None;
        svc.anchor(hijack, 8453, 1_000)
            .await
            .expect("provisional claim lands");

        // The real seller shows up and proves it.
        let mut real = anchor_request(EvidenceMode::Direct);
        real.payee = addr(&seller.address().to_string());
        real.seller_signature = Some(sign_as(&seller, &real, 8453));
        let out = svc
            .anchor(real.clone(), 8453, 1_100)
            .await
            .expect("a signed anchor must supersede an unproven one");

        // The seller's content is what survives, not the attacker's.
        let record = svc.lookup(&out.payment_id, 1_100).await.unwrap();
        assert_eq!(
            record.authority(),
            1,
            "a signature outranks an anonymous claim -- without certifying authorship"
        );
        assert_eq!(record.content_hash, real.content_hash);
    }

    #[tokio::test]
    async fn an_attacker_cannot_self_sign_a_final_anchor_for_someone_elses_payment() {
        // `paymentId` is keccak256 over public data, so any observer of a
        // settlement can compute it and race the seller. What must NOT happen is
        // that racing + signing produces a FINAL record: a signature over a
        // caller-supplied `payee` proves only "I control the address I typed
        // into my own request".
        //
        // Before the fix this recorded `verified = true`, which nothing may
        // supersede -- so an observer owned a stranger's evidence permanently,
        // and the real seller got a 409 forever.
        let svc = Dx402Service::in_memory(PrivateKeySigner::random());
        let mallory = PrivateKeySigner::random();

        let mut hijack = anchor_request(EvidenceMode::Direct);
        hijack.payee = addr(&mallory.address().to_string());
        hijack.content_hash = format!("0x{}", "ee".repeat(32));
        hijack.seller_signature = Some(sign_as(&mallory, &hijack, 8453));
        let out = svc.anchor(hijack, 8453, 1_000).await.expect("it lands");

        // It holds the slot -- but only at the rank it actually earned.
        assert!(
            !out.verified,
            "self-signing one's own declared payee must never certify authorship"
        );
        let record = svc.lookup(&out.payment_id, 1_000).await.unwrap();
        assert_eq!(
            record.authority(),
            1,
            "identity-committed, not chain-checked"
        );
        assert!(
            record.authority() < 2,
            "a self-signed claim must stay supersedable by a chain-verified one"
        );
    }

    #[tokio::test]
    async fn a_verified_anchor_is_final() {
        let svc = Dx402Service::in_memory(PrivateKeySigner::random());
        let seller = PrivateKeySigner::random();

        let mut real = anchor_request(EvidenceMode::Direct);
        real.payee = addr(&seller.address().to_string());
        real.seller_signature = Some(sign_as(&seller, &real, 8453));
        svc.anchor(real, 8453, 1_000).await.unwrap();

        // Neither an unsigned claim nor another signed one may replace it.
        let mut later = anchor_request(EvidenceMode::Direct);
        later.seller_signature = None;
        assert_eq!(
            svc.anchor(later, 8453, 1_100).await.unwrap_err(),
            Dx402ErrorCode::Dx402AlreadyAnchored
        );

        let impostor = PrivateKeySigner::random();
        let mut forged = anchor_request(EvidenceMode::Direct);
        forged.payee = addr(&impostor.address().to_string());
        forged.seller_signature = Some(sign_as(&impostor, &forged, 8453));
        assert_eq!(
            svc.anchor(forged, 8453, 1_200).await.unwrap_err(),
            Dx402ErrorCode::Dx402AlreadyAnchored
        );
    }

    #[tokio::test]
    async fn a_rejected_signature_says_so_instead_of_blaming_a_duplicate() {
        // KarmaKadabra, 2026-08-19. A seller whose signature did not verify was
        // told `dx402_already_anchored`, which is TRUE and points at the wrong
        // thing: they go audit their retries and their idempotency, where
        // plausible suspects always exist, and never look at the digest.
        let svc = Dx402Service::in_memory(PrivateKeySigner::random());
        let seller = PrivateKeySigner::random();
        let impostor = PrivateKeySigner::random();

        // Something already holds the slot.
        svc.anchor(anchor_request(EvidenceMode::Direct), 8453, 1_000)
            .await
            .unwrap();

        // The seller signs, but the signature does not check out against payee.
        let mut bad = anchor_request(EvidenceMode::Direct);
        bad.payee = addr(&seller.address().to_string());
        bad.seller_signature = Some(sign_as(&impostor, &bad, 8453));

        assert_eq!(
            svc.anchor(bad, 8453, 1_100).await.unwrap_err(),
            Dx402ErrorCode::Dx402SignatureNotVerified,
            "the reason it could not supersede is the signature, not a duplicate"
        );
    }

    #[tokio::test]
    async fn a_rejected_signature_is_visible_on_the_very_first_anchor() {
        // The quieter half of the same bug: with no prior record the anchor
        // SUCCEEDS, so a seller signing the wrong digest form got a 201 that
        // looked perfect while its anchor stayed provisional forever. It should
        // not take a collision -- which may never come -- to find that out.
        let svc = Dx402Service::in_memory(PrivateKeySigner::random());
        let seller = PrivateKeySigner::random();
        let impostor = PrivateKeySigner::random();

        let mut bad = anchor_request(EvidenceMode::Direct);
        bad.payee = addr(&seller.address().to_string());
        bad.seller_signature = Some(sign_as(&impostor, &bad, 8453));

        let out = svc.anchor(bad, 8453, 1_000).await.expect("still anchors");
        assert!(
            !out.verified,
            "a 201 must not hide that the signature was rejected"
        );

        // And durability is NOT sacrificed to make the point: the evidence is
        // recorded, so a seller-side signing bug never costs the buyer its copy.
        assert!(svc.lookup(&out.payment_id, 1_000).await.is_ok());
    }

    #[tokio::test]
    async fn a_good_signature_is_reported_as_signed_but_not_chain_verified() {
        let svc = Dx402Service::in_memory(PrivateKeySigner::random());
        let seller = PrivateKeySigner::random();

        let mut real = anchor_request(EvidenceMode::Direct);
        real.payee = addr(&seller.address().to_string());
        real.seller_signature = Some(sign_as(&seller, &real, 8453));

        // `verified` means the CHAIN says so. A signature over a self-declared
        // payee cannot reach that rung, and reporting that it did is what made
        // the hijack final.
        let out = svc.anchor(real, 8453, 1_000).await.unwrap();
        assert!(
            !out.verified,
            "no proofOfPayment -> the gate reached no verdict"
        );
        assert_eq!(
            svc.lookup(&out.payment_id, 1_000)
                .await
                .unwrap()
                .authority(),
            1
        );
    }

    #[tokio::test]
    async fn a_refused_duplicate_cannot_destroy_the_evidence_it_lost_to() {
        // The blob used to be uploaded BEFORE the anti-replay check, to a key
        // derived from the (public) paymentId, in a bucket with versioning
        // deliberately off. So a duplicate got its 409 -- after irreversibly
        // overwriting the real ciphertext with whatever it sent.
        use base64::Engine as _;
        let svc = Dx402Service::in_memory(PrivateKeySigner::random());
        let real_blob = b"DX402\x01\x01the-sellers-actual-sealed-envelope";

        let real = AnchorRequest {
            pointer: None,
            sealed: Some(base64::engine::general_purpose::STANDARD.encode(real_blob)),
            ..anchor_request(EvidenceMode::Direct)
        };
        let out = svc.anchor(real, 8453, 1_000).await.unwrap();
        assert_eq!(
            svc.fetch_sealed(&out.payment_id, 1_000).await.unwrap(),
            real_blob,
            "precondition: the seller's bytes are stored"
        );

        // Mallory sends junk for the same, publicly derivable, paymentId.
        let junk = AnchorRequest {
            pointer: None,
            sealed: Some(base64::engine::general_purpose::STANDARD.encode(b"destroyed")),
            ..anchor_request(EvidenceMode::Direct)
        };
        assert!(
            svc.anchor(junk, 8453, 1_100).await.is_err(),
            "the duplicate must be refused"
        );

        assert_eq!(
            svc.fetch_sealed(&out.payment_id, 1_100).await.unwrap(),
            real_blob,
            "a refused anchor must not have touched the stored bytes"
        );
    }

    #[tokio::test]
    async fn one_unproven_claim_still_blocks_another() {
        // The anti-replay has to keep doing its job against plain duplicates;
        // only a PROVEN anchor earns the right to supersede.
        let svc = Dx402Service::in_memory(PrivateKeySigner::random());
        svc.anchor(anchor_request(EvidenceMode::Direct), 8453, 1_000)
            .await
            .unwrap();
        assert_eq!(
            svc.anchor(anchor_request(EvidenceMode::Direct), 8453, 1_100)
                .await
                .unwrap_err(),
            Dx402ErrorCode::Dx402AlreadyAnchored
        );
    }

    #[tokio::test]
    async fn a_signature_from_the_wrong_payee_does_not_verify() {
        let svc = Dx402Service::in_memory(PrivateKeySigner::random());
        let impostor = PrivateKeySigner::random();

        let mut req = anchor_request(EvidenceMode::Direct);
        req.seller_signature = Some(sign_as(&impostor, &req, 8453)); // payee is somebody else
        let out = svc.anchor(req, 8453, 1_000).await.unwrap();

        // It still anchors -- signatures gate authority, not admission -- but it
        // is provisional, so the real seller can still claim it.
        assert!(!svc.lookup(&out.payment_id, 1_000).await.unwrap().verified);
    }

    #[tokio::test]
    async fn a_payment_can_only_be_anchored_once() {
        // Without this, an observer of a settlement could overwrite real
        // evidence with their own after the fact -- and the receipt would still
        // verify, because we would have signed the replacement.
        let svc = Dx402Service::in_memory(PrivateKeySigner::random());

        svc.anchor(anchor_request(EvidenceMode::Direct), 8453, 1_000)
            .await
            .expect("first anchor should succeed");

        assert_eq!(
            svc.anchor(anchor_request(EvidenceMode::Direct), 8453, 1_000)
                .await
                .unwrap_err(),
            Dx402ErrorCode::Dx402AlreadyAnchored
        );
    }

    #[tokio::test]
    async fn the_first_anchor_survives_a_second_attempt() {
        // The winner must be the ORIGINAL record, not the last writer.
        let svc = Dx402Service::in_memory(PrivateKeySigner::random());
        let first = svc
            .anchor(anchor_request(EvidenceMode::Direct), 8453, 1_000)
            .await
            .unwrap();

        let mut overwrite = anchor_request(EvidenceMode::Direct);
        overwrite.content_hash = format!("0x{}", "99".repeat(32));
        let _ = svc.anchor(overwrite, 8453, 2_000).await;

        let record = svc.lookup(&first.payment_id, 1_000).await.unwrap();
        assert_eq!(record.content_hash, first.content_hash);
        assert_eq!(record.anchored_at, 1_000);
    }

    #[tokio::test]
    async fn the_gate_reports_but_does_not_block_in_phase_one() {
        // No providers attached, so the gate cannot reach a verdict. Phase 1
        // must let the anchor through anyway -- and even in phase 2,
        // RpcUnavailable is not enforceable.
        let svc = Dx402Service::in_memory(PrivateKeySigner::random());
        assert!(svc
            .anchor(anchor_request(EvidenceMode::Direct), 8453, 1_000)
            .await
            .is_ok());
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

#[cfg(test)]
mod advertisement_tests {
    use super::*;

    fn cfg(enabled: bool, bucket: Option<&str>, base: Option<&str>) -> Dx402Config {
        Dx402Config {
            enabled,
            bucket: bucket.map(str::to_string),
            public_base: base.map(str::to_string),
            ..Dx402Config::default()
        }
    }

    #[test]
    fn the_flag_alone_does_not_make_it_serviceable() {
        // The gap this closes: ENABLE_DX402=true with no bucket meant the
        // service was never built and every /dx402 route 404'd -- while
        // /supported still advertised `durable-evidence`. Harmless until a
        // frontend keys off that signal, which is exactly what it now does.
        assert!(!cfg(true, None, Some("https://f.test")).is_serviceable());
        assert!(!cfg(true, Some("b"), None).is_serviceable());
        assert!(!cfg(false, Some("b"), Some("https://f.test")).is_serviceable());
        assert!(cfg(true, Some("b"), Some("https://f.test")).is_serviceable());
    }

    #[test]
    fn a_backend_we_cannot_serve_is_not_advertised() {
        let mut c = cfg(true, Some("b"), Some("https://f.test"));
        c.backend = StorageBackend::Arweave;
        assert!(
            !c.is_serviceable(),
            "asking for a backend with no implementation must not advertise the extension"
        );
    }
}

#[cfg(test)]
mod offer_tests {
    use super::*;

    fn base() -> Dx402Config {
        Dx402Config {
            enabled: true,
            bucket: Some("b".into()),
            public_base: Some("https://f.test".into()),
            ..Dx402Config::default()
        }
    }

    #[test]
    fn a_backend_without_its_credential_is_listed_but_not_enabled() {
        // Listed rather than hidden: a caller has to be able to tell "not on
        // this deployment" from "not a thing", and a silent omission reads as
        // the second.
        let offers = base().offers();
        let ipfs = offers.iter().find(|o| o.id == "ipfs-private").unwrap();
        assert!(!ipfs.enabled);
        assert_eq!(
            ipfs.disabled_reason.as_deref(),
            Some("no pinata credential")
        );
        assert!(offers.iter().find(|o| o.id == "s3").unwrap().enabled);
    }

    #[test]
    fn public_ipfs_stays_off_even_with_a_working_credential() {
        // The credential is not the gate. It is irreversible, and it is the
        // BUYER's ciphertext that becomes permanent -- so it waits for consent
        // the buyer can actually give.
        let mut c = base();
        c.pinata_jwt = Some("jwt".into());
        c.pinata_gateway = Some("gw.mypinata.cloud".into());
        let offers = c.offers();
        assert!(
            offers
                .iter()
                .find(|o| o.id == "ipfs-private")
                .unwrap()
                .enabled
        );

        let public = offers.iter().find(|o| o.id == "ipfs-public").unwrap();
        assert!(!public.enabled);
        assert_eq!(
            public.disabled_reason.as_deref(),
            Some("irreversible; awaiting buyer opt-in")
        );
    }

    #[test]
    fn only_public_ipfs_is_irrevocable_and_it_says_so() {
        // `revocable` is what makes the signed `retentionUntil` true or not, so
        // it must never be inferred from the name.
        let mut c = base();
        c.pinata_jwt = Some("jwt".into());
        c.pinata_gateway = Some("gw".into());
        c.allow_public_ipfs = true;
        for o in c.offers() {
            match o.id.as_str() {
                "ipfs-public" => {
                    assert!(!o.revocable);
                    assert!(o.public);
                    assert_eq!(
                        o.retention, "permanent",
                        "it cannot expire, so it must not claim to"
                    );
                }
                _ => {
                    assert!(o.revocable, "{} must be deletable", o.id);
                    assert!(!o.public);
                }
            }
        }
    }
}
