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

impl EvidenceRecord {
    /// How much this record proved, as a rank. Only a strictly higher rank may
    /// supersede: equal ranks are indistinguishable claims, and there the
    /// anti-replay holds -- first writer keeps the slot.
    ///
    /// 2 = the chain says this is the payee. 1 = the claimant committed to an
    /// identity. 0 = anyone could have written this.
    pub fn authority(&self) -> u8 {
        if self.verified {
            2
        } else if self.signed {
            1
        } else {
            0
        }
    }
}

impl RegistryError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, RegistryError::Unavailable(_))
    }
}

/// Proof that this process is the one that claimed a slot.
///
/// Minted per anchor attempt, written as its own top-level DynamoDB attribute,
/// and never a field of [`EvidenceRecord`] -- a condition expression cannot
/// read inside the serialized `record` blob, and keeping it out of that blob is
/// also what stops it reaching `/dx402/evidence`.
///
/// It fences the correction write that follows an upload. That condition is
/// strictly narrower than the authority ladder: it matches only the exact row
/// this call wrote, so a caller superseded mid-upload is refused and physically
/// cannot overwrite the winner.
///
/// Unlike `paymentId`, which is `keccak256(caip2 || txHash)` over entirely
/// public data, there is nothing here for an observer of a settlement to
/// derive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimToken(String);

impl ClaimToken {
    fn mint() -> Self {
        use rand::RngCore as _;
        let mut bytes = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        Self(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }

    pub fn as_str(&self) -> &str {
        &self.0
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
    /// Whether the claimant proved, by signature, that it controls the address
    /// it *declared* as payee.
    ///
    /// Strictly weaker than [`Self::verified`], and the distinction is the whole
    /// point: a signature over a caller-supplied `payee` proves only "I control
    /// the address I typed into my own request". That is not authorship -- but
    /// it is not nothing either, because it commits the claimant to an identity.
    /// So it ranks above an anonymous claim and below a chain-checked one.
    ///
    /// Collapsing these two into one flag is what let an observer of any
    /// settlement own a stranger's evidence slot permanently: `paymentId` is
    /// public, so it could front-run the seller, declare its own address, sign,
    /// and be recorded as FINAL. Found by an audit 2026-08-19.
    #[serde(default)]
    pub signed: bool,
    /// Backend handle for deletion, as the store that took the bytes reported
    /// it. Written by the correction that follows the upload, so it is absent
    /// on every anchor written before that correction existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

impl EvidenceRecord {
    /// Whether the retention guarantee has lapsed. `0` means permanent.
    pub fn is_expired(&self, now: u64) -> bool {
        self.retention_until != 0 && now > self.retention_until
    }
}

#[async_trait]
pub trait EvidenceRegistry: Send + Sync + std::fmt::Debug {
    /// Claim the slot for this payment. The returned token fences [`settle`].
    ///
    /// [`settle`]: EvidenceRegistry::settle
    async fn put(&self, record: &EvidenceRecord) -> Result<ClaimToken, RegistryError>;
    /// Correct the row this call claimed, and only that row.
    ///
    /// Exists because the pointer is a PREDICTION until the upload returns: a
    /// composed store names its primary and may write to its fallback. The
    /// token is what keeps this from being a second bite at the ladder -- a
    /// claim that was superseded mid-upload gets `AlreadyAnchored` here, which
    /// is the correct answer, because the row is genuinely not ours any more.
    async fn settle(
        &self,
        record: &EvidenceRecord,
        token: &ClaimToken,
    ) -> Result<(), RegistryError>;
    /// Rewrite a row whose pointer names nothing, in place.
    ///
    /// Deliberately NOT a rung of the authority ladder: it climbs nothing and
    /// takes nothing from anybody. It is a compare-and-swap on the exact row
    /// that was audited -- `anchored_at` must still match what the repair read
    /// -- so a record superseded between the read and the write is left alone,
    /// and the caller finds out rather than clobbering the winner.
    ///
    /// Reachable only behind the admin token. This module's recent history is
    /// ladder bypasses found one at a time, so a third write path earns its
    /// keep by being narrower than both the others, not wider.
    async fn repair(
        &self,
        record: &EvidenceRecord,
        expected_anchored_at: u64,
    ) -> Result<(), RegistryError>;
    async fn get(&self, payment_id: &str) -> Result<EvidenceRecord, RegistryError>;
    /// Number of anchors recorded, for `/api/stats` and the landing counter.
    async fn count(&self) -> Result<u64, RegistryError>;
}

/// In-memory registry, for tests and for a deployment with DX402 on but no table
/// configured.
#[derive(Debug, Default)]
pub struct MemoryEvidenceRegistry {
    inner: std::sync::Mutex<std::collections::HashMap<String, (ClaimToken, EvidenceRecord)>>,
}

impl MemoryEvidenceRegistry {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl EvidenceRegistry for MemoryEvidenceRegistry {
    async fn put(&self, record: &EvidenceRecord) -> Result<ClaimToken, RegistryError> {
        let mut inner = self.inner.lock().expect("poisoned");
        // A weaker claim never locks out one that proved more.
        // See `EvidenceRecord::authority`.
        if let Some((_, existing)) = inner.get(&record.payment_id) {
            if record.authority() <= existing.authority() {
                return Err(RegistryError::AlreadyAnchored);
            }
        }
        let token = ClaimToken::mint();
        inner.insert(record.payment_id.clone(), (token.clone(), record.clone()));
        Ok(token)
    }

    async fn settle(
        &self,
        record: &EvidenceRecord,
        token: &ClaimToken,
    ) -> Result<(), RegistryError> {
        let mut inner = self.inner.lock().expect("poisoned");
        match inner.get(&record.payment_id) {
            // Same fence DynamoDB applies: only the row this call wrote.
            Some((held, _)) if held == token => {
                inner.insert(record.payment_id.clone(), (token.clone(), record.clone()));
                Ok(())
            }
            _ => Err(RegistryError::AlreadyAnchored),
        }
    }

    async fn repair(
        &self,
        record: &EvidenceRecord,
        expected_anchored_at: u64,
    ) -> Result<(), RegistryError> {
        let mut inner = self.inner.lock().expect("poisoned");
        match inner.get(&record.payment_id) {
            Some((token, existing)) if existing.anchored_at == expected_anchored_at => {
                let token = token.clone();
                inner.insert(record.payment_id.clone(), (token, record.clone()));
                Ok(())
            }
            Some(_) => Err(RegistryError::AlreadyAnchored),
            None => Err(RegistryError::NotFound),
        }
    }

    async fn get(&self, payment_id: &str) -> Result<EvidenceRecord, RegistryError> {
        self.inner
            .lock()
            .expect("poisoned")
            .get(payment_id)
            .map(|(_, record)| record.clone())
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

/// The flags the ladder condition reads, paired with their values.
///
/// One list, used by both the writer and the condition, so the two cannot
/// drift apart again.
fn ladder_flags(record: &EvidenceRecord) -> [(&'static str, bool); 2] {
    [("verified", record.verified), ("signed", record.signed)]
}

/// Which existing rows a claim of this rank may take the slot from.
///
/// Split out of `put` so a test can evaluate it the way DynamoDB would. The
/// in-memory registry enforces the same ladder in Rust and got it right the
/// whole time -- which is precisely why nobody noticed the table was not
/// enforcing it at all.
fn ladder_condition(authority: u8) -> String {
    let unverified = "(attribute_not_exists(verified) OR verified = :f)";
    let unsigned = "(attribute_not_exists(signed) OR signed = :f)";
    match authority {
        // Chain-checked: may take the slot from anything not chain-checked.
        2 => format!("attribute_not_exists(payment_id) OR {unverified}"),
        // Identity-committed: may take it only from an anonymous claim.
        1 => format!("attribute_not_exists(payment_id) OR ({unverified} AND {unsigned})"),
        // Anonymous: may only take an empty slot.
        _ => "attribute_not_exists(payment_id)".to_string(),
    }
}

/// Top-level attribute holding the claim token.
///
/// Top-level and not inside `record` because a condition expression cannot see
/// inside the serialized blob -- the same reason the ladder flags are hoisted,
/// and the same bug class as the rung that stopped existing.
const CLAIM_ATTR: &str = "claim";

impl DynamoEvidenceRegistry {
    /// The row this record writes, minus the condition.
    ///
    /// Shared by the claim and the correction so the two can never write
    /// different shapes for the same record.
    fn row(
        &self,
        record: &EvidenceRecord,
        token: &ClaimToken,
    ) -> Result<aws_sdk_dynamodb::operation::put_item::builders::PutItemFluentBuilder, RegistryError>
    {
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
            )
            .item(CLAIM_ATTR, AttributeValue::S(token.as_str().to_string()));

        // Let DynamoDB expire the row in step with the retention promise, so the
        // index cannot outlive the bytes it points at and start answering
        // "evidence exists" for objects the bucket already deleted.
        if record.retention_until != 0 {
            req = req.item(
                "expires_at",
                AttributeValue::N(record.retention_until.to_string()),
            );
        }

        // Hoist every flag the ladder reads. A DynamoDB condition can only see
        // TOP-LEVEL attributes: a flag that lives only inside the serialized
        // `record` blob is invisible to it, and `attribute_not_exists` on an
        // attribute nobody writes is not a guard, it is the constant `true`.
        //
        // That is exactly how rung 1 stopped existing. `verified` was hoisted
        // and `signed` was not, so the `unsigned` half of rung 1's condition
        // was a tautology and any identity-committed claim could take the slot
        // from any other one.
        //
        // The two lists are one list on purpose. See
        // `every_flag_the_ladder_reads_is_a_flag_the_writer_hoists`.
        for (name, value) in ladder_flags(record) {
            req = req.item(name, AttributeValue::Bool(value));
        }

        Ok(req)
    }
}

#[async_trait]
impl EvidenceRegistry for DynamoEvidenceRegistry {
    async fn put(&self, record: &EvidenceRecord) -> Result<ClaimToken, RegistryError> {
        use aws_sdk_dynamodb::types::AttributeValue;

        let token = ClaimToken::mint();
        let mut req = self.row(record, &token)?;

        req = req.condition_expression(ladder_condition(record.authority()));
        if record.authority() > 0 {
            req = req.expression_attribute_values(":f", AttributeValue::Bool(false));
        }

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
        Ok(token)
    }

    /// Rewrite the row this call claimed -- and only that row.
    ///
    /// A full `PutItem`, not an update: the task role grants `PutItem`,
    /// `GetItem`, `DescribeTable` and `Scan` and nothing else, so a design
    /// built on `UpdateItem` would deploy green and answer `AccessDenied`
    /// forever, silently. See `terraform/environments/production/dx402.tf`.
    async fn settle(
        &self,
        record: &EvidenceRecord,
        token: &ClaimToken,
    ) -> Result<(), RegistryError> {
        use aws_sdk_dynamodb::types::AttributeValue;

        let req = self
            .row(record, token)?
            // Narrower than the ladder on purpose: this may only replace the
            // row this very call wrote. A claim superseded mid-upload fails
            // here, which is right -- the row is not ours any more.
            .condition_expression(format!("{CLAIM_ATTR} = :claim"))
            .expression_attribute_values(":claim", AttributeValue::S(token.as_str().to_string()));

        req.send().await.map_err(|e| {
            let service_error = e.into_service_error();
            if service_error.is_conditional_check_failed_exception() {
                return RegistryError::AlreadyAnchored;
            }
            warn!(error = %service_error, "DX402 registry settle failed");
            RegistryError::Unavailable(format!("dynamodb settle: {service_error}"))
        })?;
        Ok(())
    }

    async fn repair(
        &self,
        record: &EvidenceRecord,
        expected_anchored_at: u64,
    ) -> Result<(), RegistryError> {
        use aws_sdk_dynamodb::types::AttributeValue;

        // Keep whatever claim token the row already carries out of the way: a
        // repair is not a claim, so it must not hand a later `settle` a fence
        // it did not mint. A fresh token can never match an in-flight anchor's,
        // which is the conservative direction -- that anchor fails its settle
        // and says so, instead of silently writing over a repair.
        let req = self
            .row(record, &ClaimToken::mint())?
            .condition_expression("attribute_exists(payment_id) AND anchored_at = :t")
            .expression_attribute_values(":t", AttributeValue::N(expected_anchored_at.to_string()));

        req.send().await.map_err(|e| {
            let service_error = e.into_service_error();
            if service_error.is_conditional_check_failed_exception() {
                // Somebody changed the row between the audit and the write.
                return RegistryError::AlreadyAnchored;
            }
            warn!(error = %service_error, "DX402 registry repair failed");
            RegistryError::Unavailable(format!("dynamodb repair: {service_error}"))
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
    use std::collections::BTreeMap;

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
            signed: false,
            reference: None,
        }
    }

    #[tokio::test]
    async fn the_ladder_only_climbs() {
        // The whole point of ranking instead of a single flag: each rung may
        // take the slot from a lower one, never from an equal or higher one.
        let anon = |id: &str| record(id, 10_000);
        let signed = |id: &str| EvidenceRecord {
            signed: true,
            ..record(id, 10_000)
        };
        let chain = |id: &str| EvidenceRecord {
            verified: true,
            ..record(id, 10_000)
        };

        // 0 -> 1 -> 2 climbs.
        let reg = MemoryEvidenceRegistry::new();
        reg.put(&anon("0xa")).await.unwrap();
        reg.put(&signed("0xa"))
            .await
            .expect("identity beats anonymous");
        reg.put(&chain("0xa"))
            .await
            .expect("the chain beats a self-claim");

        // 2 is final: nothing takes it back.
        assert!(matches!(
            reg.put(&signed("0xa")).await,
            Err(RegistryError::AlreadyAnchored)
        ));
        assert!(matches!(
            reg.put(&anon("0xa")).await,
            Err(RegistryError::AlreadyAnchored)
        ));

        // Equal ranks tie to the first writer -- the anti-replay still holds.
        let reg2 = MemoryEvidenceRegistry::new();
        reg2.put(&signed("0xb")).await.unwrap();
        assert!(
            matches!(
                reg2.put(&signed("0xb")).await,
                Err(RegistryError::AlreadyAnchored)
            ),
            "two indistinguishable claims must not overwrite each other"
        );

        // And the hijack that started this: a self-signed squatter is NOT final.
        let reg3 = MemoryEvidenceRegistry::new();
        reg3.put(&signed("0xc")).await.unwrap();
        reg3.put(&chain("0xc"))
            .await
            .expect("the real seller, proving it on-chain, reclaims its slot");
    }

    /// Evaluate a DynamoDB condition expression the way DynamoDB would.
    ///
    /// Only the shapes `ladder_condition` produces: `OR`, `AND`, parentheses,
    /// `attribute_not_exists(name)` and `name = :f`. `item` is the row's
    /// TOP-LEVEL attributes -- absent means the attribute was never hoisted,
    /// which is the whole point of these tests.
    fn dynamo_would_allow(condition: &str, item: &BTreeMap<&str, bool>) -> bool {
        fn expr(t: &mut &str, item: &BTreeMap<&str, bool>) -> bool {
            let mut acc = term(t, item);
            while eat(t, "OR") {
                acc |= term(t, item);
            }
            acc
        }
        fn term(t: &mut &str, item: &BTreeMap<&str, bool>) -> bool {
            let mut acc = factor(t, item);
            while eat(t, "AND") {
                acc &= factor(t, item);
            }
            acc
        }
        fn factor(t: &mut &str, item: &BTreeMap<&str, bool>) -> bool {
            skip(t);
            if eat(t, "(") {
                let v = expr(t, item);
                assert!(eat(t, ")"), "unbalanced parens in condition");
                return v;
            }
            if eat(t, "attribute_not_exists(") {
                let name = take_while(t, |c| c != ')');
                assert!(eat(t, ")"), "unbalanced attribute_not_exists");
                // A comparison against a missing attribute is false in
                // DynamoDB; existence is the only thing you can ask about it.
                return !item.contains_key(name.trim());
            }
            let name = take_while(t, |c| c != ' ');
            skip(t);
            assert!(eat(t, "= :f"), "unsupported comparison in condition");
            // `= :f` on a missing attribute is FALSE, not true. That asymmetry
            // is why the existence check has to be OR'd alongside it.
            item.get(name.trim()).map(|v| !v).unwrap_or(false)
        }
        fn skip(t: &mut &str) {
            *t = t.trim_start();
        }
        fn eat(t: &mut &str, lit: &str) -> bool {
            skip(t);
            match t.strip_prefix(lit) {
                Some(rest) => {
                    *t = rest;
                    true
                }
                None => false,
            }
        }
        fn take_while<'a>(t: &mut &'a str, f: impl Fn(char) -> bool) -> &'a str {
            let end = t.find(|c| !f(c)).unwrap_or(t.len());
            let (head, tail) = t.split_at(end);
            *t = tail;
            head
        }
        let mut cursor = condition;
        let out = expr(&mut cursor, item);
        assert!(cursor.trim().is_empty(), "unconsumed condition: {cursor:?}");
        out
    }

    /// The row a record of this rank actually writes, as DynamoDB sees it.
    fn row(authority: u8) -> BTreeMap<&'static str, bool> {
        let rec = EvidenceRecord {
            verified: authority >= 2,
            signed: authority >= 1,
            ..record("0xa", 10_000)
        };
        let mut item = BTreeMap::new();
        item.insert("payment_id", true);
        for (name, value) in ladder_flags(&rec) {
            item.insert(name, value);
        }
        item
    }

    #[test]
    fn every_flag_the_ladder_reads_is_a_flag_the_writer_hoists() {
        // The invariant that was broken, stated so it cannot break again
        // quietly: a condition may only name attributes the writer puts at the
        // top level. `attribute_not_exists` on an attribute nobody writes is
        // not a guard, it is the constant `true`.
        let hoisted: Vec<&str> = ladder_flags(&record("0xa", 10_000))
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        for authority in 0..=2u8 {
            let condition = ladder_condition(authority);
            for flag in ["verified", "signed"] {
                if condition.contains(flag) {
                    assert!(
                        hoisted.contains(&flag),
                        "rung {authority} reads `{flag}` but the writer never hoists it, \
                         so the clause is always true and the rung does not exist"
                    );
                }
            }
        }
    }

    #[test]
    fn the_table_enforces_the_same_ladder_the_memory_registry_does() {
        // The regression this pins: `the_ladder_only_climbs` exercises
        // `MemoryEvidenceRegistry`, which enforces the rule in Rust and always
        // got it right. Production is DynamoDB, and for months its condition
        // let any identity-committed claim take the slot from any other one --
        // green tests, open door. So evaluate the CONDITION, not the Rust.
        for claimant in 0..=2u8 {
            for existing in 0..=2u8 {
                let allowed = dynamo_would_allow(&ladder_condition(claimant), &row(existing));
                assert_eq!(
                    allowed,
                    claimant > existing,
                    "a rung-{claimant} claim against a rung-{existing} row: \
                     allowed={allowed}, but a rung may only take the slot from a LOWER one"
                );
            }
        }
    }

    #[test]
    fn a_legacy_row_written_before_the_flags_existed_can_still_be_superseded() {
        // Load-bearing, not defensive: rows written before these attributes
        // existed carry neither, and in DynamoDB every comparison against a
        // missing attribute is false. Without the existence checks the rule
        // would refuse to supersede exactly the oldest records -- the ones with
        // the least authority behind them.
        let legacy = BTreeMap::from([("payment_id", true)]);
        for claimant in 1..=2u8 {
            assert!(
                dynamo_would_allow(&ladder_condition(claimant), &legacy),
                "rung {claimant} must be able to supersede a flagless legacy row"
            );
        }
        // And an anonymous claim still may not, legacy or not.
        assert!(!dynamo_would_allow(&ladder_condition(0), &legacy));
    }

    #[test]
    fn an_empty_slot_is_open_to_every_rank() {
        let empty = BTreeMap::new();
        for claimant in 0..=2u8 {
            assert!(
                dynamo_would_allow(&ladder_condition(claimant), &empty),
                "rung {claimant} must be able to take an unclaimed slot"
            );
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
