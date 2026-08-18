//! The evidence index: `paymentId` -> where the evidence went.
//!
//! This is a *lookup*, not a ledger. The authoritative artifacts are the sealed
//! blob in the store and the signed receipt the buyer already holds; both remain
//! verifiable if this table is lost entirely. Same discipline as
//! `transaction_store`: the chain is the ledger, and a record here never gates a
//! payment.
//!
//! What it buys is the case where a buyer comes back months later with nothing
//! but a transaction hash and asks "what did I buy?".

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::warn;

use super::types::{
    DurablePointer, EvidenceMode, EvidenceReceipt, KeyAlg, Retention, StorageBackend,
};

/// Table used when `DX402_REGISTRY_TABLE_NAME` is unset.
pub const DEFAULT_REGISTRY_TABLE_NAME: &str = "facilitator_dx402_evidence";

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("registry unavailable: {0}")]
    Unavailable(String),
    #[error("no evidence recorded for this payment")]
    NotFound,
    /// This payment already has evidence, and the existing record is at least as
    /// authoritative as the incoming one.
    #[error("this payment already has evidence anchored")]
    AlreadyAnchored,
}

impl RegistryError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, RegistryError::Unavailable(_))
    }
}

/// One recorded anchor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceRecord {
    pub payment_id: String,
    pub pointer: DurablePointer,
    pub backend: StorageBackend,
    pub content_hash: String,
    pub key_alg: KeyAlg,
    pub mode: EvidenceMode,
    pub retention: Retention,
    pub anchored_at: u64,
    pub retention_until: u64,
    /// The signed receipt, so `/dx402/receipt/{id}` can serve it without
    /// re-signing (and therefore without the signing key being reachable from a
    /// read path).
    pub receipt: EvidenceReceipt,
    pub signature: String,
    /// `escrowed` mode only. Absent in `direct` mode, which is what makes the
    /// facilitator unable to read `direct` payloads even if this table leaks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrapped_cek: Option<String>,
    /// Whether the payee proved, by signature, that this anchor is theirs.
    ///
    /// This is what separates a claim anyone can make from one only the seller
    /// can. An unverified record is **provisional**: it holds the slot so the
    /// same anchor is not written twice, but it can be superseded by a verified
    /// one for the same payment. A verified record is final.
    ///
    /// Without that asymmetry the anti-replay became a weapon: whoever anchored
    /// first owned the evidence of a payment forever, and the real seller was
    /// locked out with a 409. Reported by KarmaKadabra, 2026-08-18, reproduced
    /// against production.
    #[serde(default)]
    pub verified: bool,
}

impl EvidenceRecord {
    /// Whether the retention guarantee has lapsed. `0` means permanent.
    pub fn is_expired(&self, now: u64) -> bool {
        self.retention_until != 0 && now > self.retention_until
    }
}

#[async_trait]
pub trait EvidenceRegistry: Send + Sync + std::fmt::Debug {
    async fn put(&self, record: &EvidenceRecord) -> Result<(), RegistryError>;
    async fn get(&self, payment_id: &str) -> Result<EvidenceRecord, RegistryError>;
    /// Number of anchors recorded, for `/api/stats` and the landing counter.
    async fn count(&self) -> Result<u64, RegistryError>;
}

/// In-memory registry, for tests and for a deployment with DX402 on but no table
/// configured.
#[derive(Debug, Default)]
pub struct MemoryEvidenceRegistry {
    inner: std::sync::Mutex<std::collections::HashMap<String, EvidenceRecord>>,
}

impl MemoryEvidenceRegistry {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl EvidenceRegistry for MemoryEvidenceRegistry {
    async fn put(&self, record: &EvidenceRecord) -> Result<(), RegistryError> {
        let mut inner = self.inner.lock().expect("poisoned");
        // A provisional claim never locks out the seller who can prove the
        // anchor is theirs. See `EvidenceRecord::verified`.
        if let Some(existing) = inner.get(&record.payment_id) {
            let supersedes = record.verified && !existing.verified;
            if !supersedes {
                return Err(RegistryError::AlreadyAnchored);
            }
        }
        inner.insert(record.payment_id.clone(), record.clone());
        Ok(())
    }

    async fn get(&self, payment_id: &str) -> Result<EvidenceRecord, RegistryError> {
        self.inner
            .lock()
            .expect("poisoned")
            .get(payment_id)
            .cloned()
            .ok_or(RegistryError::NotFound)
    }

    async fn count(&self) -> Result<u64, RegistryError> {
        Ok(self.inner.lock().expect("poisoned").len() as u64)
    }
}

/// DynamoDB-backed registry.
#[derive(Debug, Clone)]
pub struct DynamoEvidenceRegistry {
    client: aws_sdk_dynamodb::Client,
    table: String,
}

impl DynamoEvidenceRegistry {
    pub fn new(client: aws_sdk_dynamodb::Client, table: String) -> Self {
        Self { client, table }
    }

    pub async fn from_env(table: String) -> Self {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        Self::new(aws_sdk_dynamodb::Client::new(&config), table)
    }
}

#[async_trait]
impl EvidenceRegistry for DynamoEvidenceRegistry {
    async fn put(&self, record: &EvidenceRecord) -> Result<(), RegistryError> {
        use aws_sdk_dynamodb::types::AttributeValue;

        let body = serde_json::to_string(record)
            .map_err(|e| RegistryError::Unavailable(format!("serialize record: {e}")))?;

        let mut req = self
            .client
            .put_item()
            .table_name(&self.table)
            .item("payment_id", AttributeValue::S(record.payment_id.clone()))
            .item("record", AttributeValue::S(body))
            .item(
                "anchored_at",
                AttributeValue::N(record.anchored_at.to_string()),
            );

        // Let DynamoDB expire the row in step with the retention promise, so the
        // index cannot outlive the bytes it points at and start answering
        // "evidence exists" for objects the bucket already deleted.
        if record.retention_until != 0 {
            req = req.item(
                "expires_at",
                AttributeValue::N(record.retention_until.to_string()),
            );
        }

        req = req.item("verified", AttributeValue::Bool(record.verified));

        // One payment anchors once -- but a claim nobody proved must never lock
        // out the seller who can prove it.
        //
        // A verified anchor may supersede an unverified one; anything else is
        // refused. Without that asymmetry the anti-replay became a weapon:
        // whoever anchored first owned the evidence forever, and the legitimate
        // seller got a permanent 409 while the attacker's artifact was the one
        // that existed in a dispute.
        req = if record.verified {
            req.condition_expression("attribute_not_exists(payment_id) OR verified = :f")
                .expression_attribute_values(":f", AttributeValue::Bool(false))
        } else {
            req.condition_expression("attribute_not_exists(payment_id)")
        };

        // Match the TYPED error, not its Display text. The string form of an AWS
        // SDK error does not reliably contain the exception name, and getting
        // this wrong is not cosmetic: it made a duplicate anchor answer
        // `store_unavailable` with `retryable: true`, telling the caller to
        // retry something that can never succeed.
        req.send().await.map_err(|e| {
            let service_error = e.into_service_error();
            if service_error.is_conditional_check_failed_exception() {
                return RegistryError::AlreadyAnchored;
            }
            warn!(error = %service_error, "DX402 registry put_item failed");
            RegistryError::Unavailable(format!("dynamodb put_item: {service_error}"))
        })?;
        Ok(())
    }

    async fn get(&self, payment_id: &str) -> Result<EvidenceRecord, RegistryError> {
        use aws_sdk_dynamodb::types::AttributeValue;

        let out = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("payment_id", AttributeValue::S(payment_id.to_string()))
            .send()
            .await
            .map_err(|e| RegistryError::Unavailable(format!("dynamodb get_item: {e}")))?;

        let item = out.item.ok_or(RegistryError::NotFound)?;
        let raw = item
            .get("record")
            .and_then(|v| v.as_s().ok())
            .ok_or(RegistryError::NotFound)?;

        serde_json::from_str(raw)
            .map_err(|e| RegistryError::Unavailable(format!("deserialize record: {e}")))
    }

    async fn count(&self) -> Result<u64, RegistryError> {
        // `Scan` with Select=COUNT. Fine at our volume and for a display counter;
        // if this table ever gets large this should move to an atomic counter
        // item rather than growing a slow full-table scan on a public route.
        let out = self
            .client
            .scan()
            .table_name(&self.table)
            .select(aws_sdk_dynamodb::types::Select::Count)
            .send()
            .await
            .map_err(|e| RegistryError::Unavailable(format!("dynamodb scan: {e}")))?;
        Ok(out.count() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::Network;

    fn addr(s: &str) -> crate::types::MixedAddress {
        serde_json::from_value(serde_json::Value::String(s.to_string())).unwrap()
    }

    fn record(payment_id: &str, retention_until: u64) -> EvidenceRecord {
        let receipt = EvidenceReceipt {
            payment_id: payment_id.to_string(),
            content_hash: format!("0x{}", "22".repeat(32)),
            pointer: DurablePointer("mem://x".into()),
            payer: addr("0x103040545AC5031A11E8C03dd11324C7333a13C7"),
            payee: addr("0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8"),
            tx_hash: format!("0x{}", "33".repeat(32)),
            network: Network::Base,
            mode: EvidenceMode::Direct,
            anchored_at: 1_000,
            retention_until,
        };
        EvidenceRecord {
            payment_id: payment_id.to_string(),
            pointer: DurablePointer("mem://x".into()),
            backend: StorageBackend::S3,
            content_hash: receipt.content_hash.clone(),
            key_alg: KeyAlg::Secp256k1,
            mode: EvidenceMode::Direct,
            retention: Retention::Days90,
            anchored_at: 1_000,
            retention_until,
            receipt,
            signature: "0xsig".into(),
            wrapped_cek: None,
            verified: false,
        }
    }

    #[tokio::test]
    async fn records_round_trip_and_count() {
        let reg = MemoryEvidenceRegistry::new();
        assert_eq!(reg.count().await.unwrap(), 0);

        let r = record("0xaaa", 2_000);
        reg.put(&r).await.unwrap();
        assert_eq!(reg.get("0xaaa").await.unwrap(), r);
        assert_eq!(reg.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn a_proven_record_supersedes_an_unproven_one() {
        // The rule that keeps the anti-replay from becoming a weapon.
        let reg = MemoryEvidenceRegistry::new();

        let unproven = record("0xaaa", 2_000);
        reg.put(&unproven).await.unwrap();

        let mut proven = record("0xaaa", 2_000);
        proven.verified = true;
        proven.content_hash = format!("0x{}", "77".repeat(32));
        reg.put(&proven)
            .await
            .expect("proven must supersede unproven");
        assert_eq!(
            reg.get("0xaaa").await.unwrap().content_hash,
            proven.content_hash
        );

        // And nothing supersedes a proven record.
        let mut another = record("0xaaa", 2_000);
        another.verified = true;
        assert!(matches!(
            reg.put(&another).await,
            Err(RegistryError::AlreadyAnchored)
        ));
    }

    #[tokio::test]
    async fn unknown_payments_are_not_found() {
        let reg = MemoryEvidenceRegistry::new();
        assert!(matches!(
            reg.get("0xmissing").await,
            Err(RegistryError::NotFound)
        ));
    }

    #[test]
    fn expiry_is_evaluated_against_retention_until() {
        let r = record("0xaaa", 2_000);
        assert!(!r.is_expired(1_999));
        assert!(!r.is_expired(2_000));
        assert!(r.is_expired(2_001));
    }

    #[test]
    fn permanent_records_never_expire() {
        let r = record("0xaaa", 0);
        assert!(!r.is_expired(u64::MAX));
    }

    #[test]
    fn direct_mode_records_carry_no_key_material() {
        // The whole guarantee of `direct` mode is that a leak of this table
        // reveals pointers and hashes, never anything that decrypts a payload.
        let r = record("0xaaa", 2_000);
        assert_eq!(r.mode, EvidenceMode::Direct);
        assert!(r.wrapped_cek.is_none());
        let json = serde_json::to_string(&r).unwrap();
        assert!(
            !json.contains("wrappedCek"),
            "key material leaked into the record"
        );
    }

    #[test]
    fn only_unavailable_is_retryable() {
        assert!(RegistryError::Unavailable("x".into()).is_retryable());
        assert!(!RegistryError::NotFound.is_retryable());
    }
}
