//! DynamoDB implementation of [`TransactionStore`].
//!
//! # Key design
//!
//! Records: `pk = "day#YYYY-MM-DD"`, `sk = "<ts>#<kind>#<discriminator>"`.
//! Days partition the table because every question anyone actually asks is time
//! ordered, and at the measured ~1,600 operations a day one partition is far
//! from hot. Reading "the last N" walks back one day at a time and stops as
//! soon as it has enough, so an empty week costs a handful of tiny queries
//! rather than a scan.
//!
//! Aggregates: `pk = "AGG"`, `sk = "<network>#<asset>"`. All of them in ONE
//! partition so the stats page is a single bounded Query. This is the whole
//! reason the page stays cheap — scanning 500k records per page load is about
//! $0.011 a time, which is $330/month at a thousand views a day, while a Query
//! over a few dozen aggregate items is effectively free no matter how long the
//! facilitator has been running.
//!
//! Counters are updated with DynamoDB's atomic `ADD`, so two concurrent settles
//! cannot lose an increment the way a read-modify-write would.

use aws_sdk_dynamodb::types::AttributeValue;
use std::collections::HashMap;
use tracing::info;

use super::{
    Aggregate, BackfillRow, TransactionRecord, TransactionStore, TransactionStoreError,
    AGGREGATE_PK, BACKFILL_PK, DEFAULT_TRANSACTIONS_TABLE_NAME, DEFAULT_TTL_DAYS,
};

#[derive(Debug)]
pub struct DynamoTransactionStore {
    client: aws_sdk_dynamodb::Client,
    table_name: String,
    ttl_days: u64,
}

impl DynamoTransactionStore {
    pub fn new(client: aws_sdk_dynamodb::Client, table_name: String, ttl_days: u64) -> Self {
        info!(table = %table_name, ttl_days, "Initialized DynamoDB transaction store");
        Self {
            client,
            table_name,
            ttl_days,
        }
    }

    pub async fn from_env() -> Result<Self, TransactionStoreError> {
        let table_name = std::env::var("TRANSACTIONS_TABLE_NAME")
            .unwrap_or_else(|_| DEFAULT_TRANSACTIONS_TABLE_NAME.to_string());
        // 0 disables expiry entirely — an explicit choice, not an accident of
        // parsing. Anything unparseable falls back to the default rather than
        // silently meaning "keep forever".
        let ttl_days = std::env::var("TRANSACTIONS_TTL_DAYS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_TTL_DAYS);

        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        Ok(Self::new(
            aws_sdk_dynamodb::Client::new(&config),
            table_name,
            ttl_days,
        ))
    }

    fn s(v: impl Into<String>) -> AttributeValue {
        AttributeValue::S(v.into())
    }

    fn n(v: impl ToString) -> AttributeValue {
        AttributeValue::N(v.to_string())
    }

    /// Fold one record into the `(network, asset)` counters.
    ///
    /// Separate from the record write and allowed to fail on its own: an
    /// aggregate that drifts is a wrong number on a page, while a lost record is
    /// a missing row. Neither is worth failing a payment over, but they are not
    /// equally bad, so they do not share a failure.
    async fn bump_aggregate(&self, r: &TransactionRecord) -> Result<(), TransactionStoreError> {
        let asset = r.asset.clone().unwrap_or_else(|| "unknown".to_string());
        let volume: u128 = if r.kind == "settle" && r.ok {
            r.amount
                .as_deref()
                .and_then(|a| a.parse().ok())
                .unwrap_or(0)
        } else {
            0
        };

        let (settles_ok, settles_failed, verifies) = match (r.kind.as_str(), r.ok) {
            ("settle", true) => (1, 0, 0),
            ("settle", false) => (0, 1, 0),
            _ => (0, 0, 1),
        };

        self.client
            .update_item()
            .table_name(&self.table_name)
            .key("pk", Self::s(AGGREGATE_PK))
            .key("sk", Self::s(format!("{}#{}", r.network, asset)))
            .update_expression(
                "ADD settles_ok :so, settles_failed :sf, verifies :v, volume_atomic :vol \
                 SET network = :net, asset = :a, last_ts = :ts",
            )
            .expression_attribute_values(":so", Self::n(settles_ok))
            .expression_attribute_values(":sf", Self::n(settles_failed))
            .expression_attribute_values(":v", Self::n(verifies))
            .expression_attribute_values(":vol", Self::n(volume))
            .expression_attribute_values(":net", Self::s(&r.network))
            .expression_attribute_values(":a", Self::s(&asset))
            .expression_attribute_values(":ts", Self::n(r.ts))
            .send()
            .await
            .map_err(|e| TransactionStoreError::Dynamo(format!("{e:?}")))?;
        Ok(())
    }

    fn to_item(&self, r: &TransactionRecord) -> HashMap<String, AttributeValue> {
        let mut item = HashMap::new();
        item.insert("pk".into(), Self::s(format!("day#{}", r.day())));
        item.insert("sk".into(), Self::s(r.sort_key()));
        item.insert("ts".into(), Self::n(r.ts));
        item.insert("kind".into(), Self::s(&r.kind));
        item.insert("network".into(), Self::s(&r.network));
        item.insert("ok".into(), AttributeValue::Bool(r.ok));
        for (k, v) in [
            ("payer", &r.payer),
            ("tx", &r.tx),
            ("amount", &r.amount),
            ("asset", &r.asset),
            ("resource", &r.resource),
            ("pay_to", &r.pay_to),
            ("description", &r.description),
            ("scheme", &r.scheme),
        ] {
            if let Some(value) = v {
                item.insert(k.into(), Self::s(value));
            }
        }
        if self.ttl_days > 0 {
            let expires = r.ts / 1000 + self.ttl_days * 86_400;
            item.insert("expires_at".into(), Self::n(expires));
        }
        item
    }

    fn from_item(item: &HashMap<String, AttributeValue>) -> Option<TransactionRecord> {
        let get_s = |k: &str| item.get(k).and_then(|v| v.as_s().ok()).cloned();
        Some(TransactionRecord {
            ts: item.get("ts")?.as_n().ok()?.parse().ok()?,
            kind: get_s("kind")?,
            network: get_s("network")?,
            ok: item.get("ok").and_then(|v| v.as_bool().ok()).copied()?,
            payer: get_s("payer"),
            tx: get_s("tx"),
            amount: get_s("amount"),
            asset: get_s("asset"),
            resource: get_s("resource"),
            pay_to: get_s("pay_to"),
            description: get_s("description"),
            scheme: get_s("scheme"),
        })
    }
}

#[async_trait::async_trait]
impl TransactionStore for DynamoTransactionStore {
    async fn record(&self, record: TransactionRecord) -> Result<(), TransactionStoreError> {
        self.client
            .put_item()
            .table_name(&self.table_name)
            .set_item(Some(self.to_item(&record)))
            .send()
            .await
            .map_err(|e| TransactionStoreError::Dynamo(format!("{e:?}")))?;

        self.bump_aggregate(&record).await
    }

    async fn recent(
        &self,
        limit: usize,
        network: Option<&str>,
    ) -> Result<Vec<TransactionRecord>, TransactionStoreError> {
        let mut out = Vec::with_capacity(limit);
        let today = super::civil_from_days((crate::events::now_ms() / 1000) as i64 / 86_400);
        let mut day = today;

        // Walk back a day at a time. Bounded at 30 days so a facilitator that
        // has been idle for a month returns quickly with an honest empty list
        // rather than issuing hundreds of queries into the past.
        for _ in 0..30 {
            if out.len() >= limit {
                break;
            }
            let pk = format!("day#{:04}-{:02}-{:02}", day.0, day.1, day.2);
            let mut req = self
                .client
                .query()
                .table_name(&self.table_name)
                .key_condition_expression("pk = :pk")
                .expression_attribute_values(":pk", Self::s(&pk))
                // Newest first: the page shows recent activity, and reading
                // forward would make "last 50" mean "first 50 ever".
                .scan_index_forward(false)
                .limit((limit - out.len()) as i32);
            if let Some(net) = network {
                req = req
                    .filter_expression("network = :net")
                    .expression_attribute_values(":net", Self::s(net));
            }

            let page = req
                .send()
                .await
                .map_err(|e| TransactionStoreError::Dynamo(format!("{e:?}")))?;
            for item in page.items() {
                if let Some(r) = Self::from_item(item) {
                    out.push(r);
                }
            }
            day = super::civil_from_days(days_from_civil(day) - 1);
        }
        Ok(out)
    }

    async fn backfill(&self) -> Result<Vec<BackfillRow>, TransactionStoreError> {
        // One bounded Query against its own partition — same shape as
        // `aggregates`, never a scan, and it cannot pick up a live row because
        // the live rows are not in this partition.
        let page = self
            .client
            .query()
            .table_name(&self.table_name)
            .key_condition_expression("pk = :pk")
            .expression_attribute_values(":pk", Self::s(BACKFILL_PK))
            .send()
            .await
            .map_err(|e| TransactionStoreError::Dynamo(format!("{e:?}")))?;

        Ok(page
            .items()
            .iter()
            .filter_map(|item| {
                let num = |k: &str| -> u64 {
                    item.get(k)
                        .and_then(|v| v.as_n().ok())
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0)
                };
                let text = |k: &str| -> Option<String> {
                    item.get(k).and_then(|v| v.as_s().ok()).cloned()
                };
                Some(BackfillRow {
                    network: text("network")?,
                    asset: text("asset"),
                    scheme: text("scheme"),
                    op_kind: text("op_kind"),
                    // A settled row counts settles; an operation row counts
                    // operations. Both are "how many times did this happen".
                    count: if item.contains_key("op_count") {
                        num("op_count")
                    } else {
                        num("settles_ok")
                    },
                    volume_atomic: item
                        .get("volume_atomic")
                        .and_then(|v| v.as_n().ok())
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0),
                    first_ts: num("first_ts"),
                    last_ts: num("last_ts"),
                })
            })
            .collect())
    }

    async fn aggregates(&self) -> Result<Vec<Aggregate>, TransactionStoreError> {
        let page = self
            .client
            .query()
            .table_name(&self.table_name)
            .key_condition_expression("pk = :pk")
            .expression_attribute_values(":pk", Self::s(AGGREGATE_PK))
            .send()
            .await
            .map_err(|e| TransactionStoreError::Dynamo(format!("{e:?}")))?;

        Ok(page
            .items()
            .iter()
            .filter_map(|item| {
                let num = |k: &str| -> u64 {
                    item.get(k)
                        .and_then(|v| v.as_n().ok())
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0)
                };
                Some(Aggregate {
                    network: item.get("network")?.as_s().ok()?.clone(),
                    asset: item.get("asset")?.as_s().ok()?.clone(),
                    settles_ok: num("settles_ok"),
                    settles_failed: num("settles_failed"),
                    verifies: num("verifies"),
                    volume_atomic: item
                        .get("volume_atomic")
                        .and_then(|v| v.as_n().ok())
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0),
                    last_ts: num("last_ts"),
                })
            })
            .collect())
    }

    fn store_type(&self) -> &'static str {
        "dynamodb"
    }
}

/// `(year, month, day)` → days since the Unix epoch. Inverse of
/// `civil_from_days`, needed to step one day backwards across month and year
/// boundaries without special cases.
fn days_from_civil((y, m, d): (i64, u32, u32)) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if m > 2 { m - 3 } else { m + 9 } as u64;
    let doy = (153 * mp + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_conversions_round_trip() {
        for days in [0_i64, 19_000, 20_663, -1, 12_345] {
            let civil = super::super::civil_from_days(days);
            assert_eq!(days_from_civil(civil), days, "failed for {days}");
        }
    }

    #[test]
    fn stepping_back_crosses_month_and_year_boundaries() {
        // The reason this inverse exists: walking back from the 1st of a month
        // by subtracting from the day number would produce day 0.
        let march_first = (2026_i64, 3_u32, 1_u32);
        let prev = super::super::civil_from_days(days_from_civil(march_first) - 1);
        assert_eq!(prev, (2026, 2, 28));

        let jan_first = (2026_i64, 1_u32, 1_u32);
        let prev = super::super::civil_from_days(days_from_civil(jan_first) - 1);
        assert_eq!(prev, (2025, 12, 31));

        // And a leap year, where the naive answer is off by one.
        let march_first_leap = (2024_i64, 3_u32, 1_u32);
        let prev = super::super::civil_from_days(days_from_civil(march_first_leap) - 1);
        assert_eq!(prev, (2024, 2, 29));
    }
}
