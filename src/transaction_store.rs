//! Historical record of every operation the facilitator handled.
//!
//! `GET /events` is a live hint and lossy by construction — an event nobody was
//! connected for does not exist anywhere. This module is the other half: a
//! durable, queryable index of what was processed, so "how much have we settled
//! on Polygon this month" stops being a question you answer by scraping logs.
//!
//! # This is an INDEX, not a ledger
//!
//! Stated here because the distinction is the one that costs money when it is
//! forgotten. The write is fire-and-forget *after* the settlement resolved: if
//! DynamoDB is unreachable, the payment still happened and the record simply
//! does not exist. **The chain is the source of truth.** A number from this
//! store is "what we recorded", never "what occurred" — and the `/stats` page
//! labels it that way.
//!
//! That is a deliberate trade. The alternative — making the record a
//! precondition of settling — would let a DynamoDB outage stop payments, which
//! is a far worse failure than an incomplete index.
//!
//! # Why a trait
//!
//! DynamoDB is cheap at this volume (measured 2026-07-30: ~1,600 operations a
//! day ≈ 48k/month ≈ **$0.06/month** in writes), but "cheap now" is not
//! "cheap forever", and the read side is where cost actually bites: scanning
//! the table on every `/stats` load is what turns cents into hundreds of
//! dollars. Two consequences, both baked in here:
//!
//! 1. Aggregates are maintained on write, in one small partition, so the stats
//!    page issues a single bounded Query and never scans.
//! 2. Everything goes through [`TransactionStore`], so moving to Postgres, S3 +
//!    Athena or anything else is one new implementation and touches no handler.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Table used when `TRANSACTIONS_TABLE_NAME` is not set.
pub const DEFAULT_TRANSACTIONS_TABLE_NAME: &str = "facilitator_transactions";

/// Partition holding the pre-aggregated counters.
///
/// Every aggregate lives under one partition key so the stats page can read all
/// of them with a single Query. A scan over the transaction records would work
/// too, and would get more expensive every day the facilitator stays up.
const AGGREGATE_PK: &str = "AGG";

/// Default retention. Storage is not the reason for a TTL — 48k records a month
/// at ~700 bytes is about $0.03 — but an unbounded table is a decision nobody
/// made, and a TTL is far easier to lengthen than a mistake is to undo.
const DEFAULT_TTL_DAYS: u64 = 90;

#[derive(Debug, thiserror::Error)]
pub enum TransactionStoreError {
    #[error("DynamoDB error: {0}")]
    Dynamo(String),
    #[error("serialization error: {0}")]
    Serde(String),
}

/// One operation, as recorded.
///
/// Mirrors the live event plus the fields only the store needs. Everything is
/// optional except the identity of the operation itself, because the store must
/// accept whatever the handler had — a record with gaps is worth more than no
/// record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionRecord {
    /// Epoch milliseconds UTC.
    pub ts: u64,
    /// `"verify"` or `"settle"`.
    pub kind: String,
    /// Canonical network slug, the same one `/supported` uses.
    pub network: String,
    /// Did the operation resolve successfully?
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payer: Option<String>,
    /// Present on `settle`, absent on `verify` — nothing has settled yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx: Option<String>,
    /// Atomic token units, as a string. Kept as a string on purpose: these are
    /// u256-shaped values and JSON numbers lose precision above 2^53.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
    /// The endpoint that was bought.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    /// The seller that was paid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pay_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
}

impl TransactionRecord {
    /// `YYYY-MM-DD` in UTC, the partition this record belongs to.
    ///
    /// Days are the partition because every query anyone actually asks is time
    /// ordered ("last N", "this month"), and at ~1,600 records a day a single
    /// day partition stays far below the point where it becomes hot.
    pub fn day(&self) -> String {
        let secs = (self.ts / 1000) as i64;
        let days = secs.div_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        format!("{y:04}-{m:02}-{d:02}")
    }

    /// Sort key: time first so a Query returns chronological order, then a
    /// discriminator so two operations in the same millisecond cannot collide
    /// and silently overwrite each other.
    pub fn sort_key(&self) -> String {
        let discriminator = self
            .tx
            .as_deref()
            .unwrap_or_else(|| self.payer.as_deref().unwrap_or("unknown"));
        format!("{:013}#{}#{}", self.ts, self.kind, discriminator)
    }
}

/// Days since the Unix epoch → `(year, month, day)`, proleptic Gregorian.
///
/// Hand-rolled rather than pulling in a date crate for one function. Howard
/// Hinnant's `civil_from_days`, which is exact for the whole representable
/// range — no leap-year special cases to get subtly wrong.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Counters for one `(network, asset)` pair, maintained on write.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Aggregate {
    pub network: String,
    pub asset: String,
    /// Settles that resolved successfully.
    pub settles_ok: u64,
    /// Settles that resolved unsuccessfully. Note this does NOT include settles
    /// that errored — those are not published or recorded at all today.
    pub settles_failed: u64,
    pub verifies: u64,
    /// Sum of `amount`, in atomic units. A string because the total outgrows
    /// f64 precision long before it outgrows u128.
    pub volume_atomic: u128,
    /// Newest record folded into this aggregate, so a stale page is detectable.
    pub last_ts: u64,
}

#[async_trait]
pub trait TransactionStore: Send + Sync + std::fmt::Debug {
    /// Persist one operation and fold it into the aggregates.
    ///
    /// Called after the operation resolved. Errors are for the caller to log,
    /// never to propagate to the payer.
    async fn record(&self, record: TransactionRecord) -> Result<(), TransactionStoreError>;

    /// Most recent records, newest first, walking back day by day.
    async fn recent(
        &self,
        limit: usize,
        network: Option<&str>,
    ) -> Result<Vec<TransactionRecord>, TransactionStoreError>;

    /// Every aggregate, in one bounded read. Never a scan.
    async fn aggregates(&self) -> Result<Vec<Aggregate>, TransactionStoreError>;

    fn store_type(&self) -> &'static str;
}

// ============================================================================
// No-op store
// ============================================================================

/// Records nothing. Used when the table is not configured — local dev, CI, and
/// any deployment that has not opted in. Payments are entirely unaffected,
/// which is the point: the store is an index, never a precondition.
#[derive(Debug, Default)]
pub struct NoopTransactionStore;

#[async_trait]
impl TransactionStore for NoopTransactionStore {
    async fn record(&self, _record: TransactionRecord) -> Result<(), TransactionStoreError> {
        Ok(())
    }

    async fn recent(
        &self,
        _limit: usize,
        _network: Option<&str>,
    ) -> Result<Vec<TransactionRecord>, TransactionStoreError> {
        Ok(Vec::new())
    }

    async fn aggregates(&self) -> Result<Vec<Aggregate>, TransactionStoreError> {
        Ok(Vec::new())
    }

    fn store_type(&self) -> &'static str {
        "noop"
    }
}

/// Build the configured store, falling back to the no-op one.
///
/// Failure to reach DynamoDB at boot is NOT fatal: the facilitator settles
/// payments, and an index it cannot write to must not stop it from doing that.
pub async fn create_transaction_store() -> Arc<dyn TransactionStore> {
    match std::env::var("TRANSACTIONS_TABLE_NAME") {
        Ok(table) if !table.is_empty() => match dynamo::DynamoTransactionStore::from_env().await {
            Ok(store) => {
                info!(table = %table, "Using DynamoDB transaction store");
                Arc::new(store)
            }
            Err(e) => {
                warn!(error = %e, "DynamoDB transaction store unavailable; recording nothing");
                Arc::new(NoopTransactionStore)
            }
        },
        _ => {
            info!("TRANSACTIONS_TABLE_NAME unset — history is not being recorded");
            Arc::new(NoopTransactionStore)
        }
    }
}

pub mod dynamo;

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(ts: u64, kind: &str, tx: Option<&str>) -> TransactionRecord {
        TransactionRecord {
            ts,
            kind: kind.into(),
            network: "base".into(),
            ok: true,
            payer: Some("0xpayer".into()),
            tx: tx.map(String::from),
            amount: Some("1000000".into()),
            asset: Some("0x8335".into()),
            resource: Some("https://api.example.com/thing".into()),
            pay_to: Some("0xseller".into()),
            description: Some("A thing".into()),
            scheme: Some("exact".into()),
        }
    }

    #[test]
    fn day_partitions_by_utc_date() {
        // 2026-07-30T00:00:00Z and one millisecond before it.
        assert_eq!(rec(1_785_369_600_000, "settle", None).day(), "2026-07-30");
        assert_eq!(rec(1_785_369_599_999, "settle", None).day(), "2026-07-29");
    }

    #[test]
    fn day_handles_leap_days() {
        // 2024-02-29 was a real day; an off-by-one in the leap rule shifts every
        // record after February into the wrong partition.
        assert_eq!(rec(1_709_164_800_000, "settle", None).day(), "2024-02-29");
    }

    #[test]
    fn sort_key_is_chronological_as_a_string() {
        // Zero-padded because DynamoDB sorts sort keys lexicographically: "9"
        // sorts after "10" without the padding, and the whole ordering inverts.
        let early = rec(999, "settle", Some("0xa")).sort_key();
        let late = rec(1_785_369_600_000, "settle", Some("0xb")).sort_key();
        assert!(early < late, "{early} should sort before {late}");
    }

    #[test]
    fn two_operations_in_the_same_millisecond_do_not_collide() {
        // Same ts, different transaction: distinct keys, or one silently
        // overwrites the other and the record is simply lost.
        let a = rec(1_785_369_600_000, "settle", Some("0xaaa")).sort_key();
        let b = rec(1_785_369_600_000, "settle", Some("0xbbb")).sort_key();
        assert_ne!(a, b);
    }

    #[test]
    fn a_verify_and_a_settle_at_the_same_instant_stay_separate() {
        let v = rec(1_785_369_600_000, "verify", None).sort_key();
        let s = rec(1_785_369_600_000, "settle", None).sort_key();
        assert_ne!(v, s);
    }

    #[tokio::test]
    async fn noop_store_accepts_everything_and_returns_nothing() {
        // The contract that keeps payments independent of the index.
        let store = NoopTransactionStore;
        assert!(store.record(rec(1, "settle", Some("0x1"))).await.is_ok());
        assert!(store.recent(10, None).await.unwrap().is_empty());
        assert!(store.aggregates().await.unwrap().is_empty());
    }
}
