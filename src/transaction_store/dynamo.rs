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

    async fn settles_since(
        &self,
        since_ms: u64,
    ) -> Result<Option<Vec<TransactionRecord>>, TransactionStoreError> {
        let mut out = Vec::new();
        let first = since_ms as i64 / 86_400_000;
        let today = (crate::events::now_ms() / 86_400_000) as i64;
        // One Query per day partition, oldest first, each starting at the
        // cursor's sort key on the cursor's own day; never more days than the
        // usage window, whatever the cursor says.
        let first = first.max(today - crate::discovery_usage::USAGE_WINDOW_DAYS as i64);
        for day in first..=today {
            let (y, m, d) = super::civil_from_days(day);
            let pk = format!("day#{y:04}-{m:02}-{d:02}");
            let from = format!("{:013}", since_ms.max(day as u64 * 86_400_000));
            let mut start_key = None;
            loop {
                let page = self
                    .client
                    .query()
                    .table_name(&self.table_name)
                    .key_condition_expression("pk = :pk AND #sk >= :from")
                    .filter_expression("#kind = :settle AND #ok = :yes")
                    .projection_expression(
                        "#ts, #kind, #ok, #net, #payer, #payto, #res, #tx, #amount, #asset, #scheme",
                    )
                    .expression_attribute_names("#sk", "sk")
                    .expression_attribute_names("#ts", "ts")
                    .expression_attribute_names("#kind", "kind")
                    .expression_attribute_names("#ok", "ok")
                    .expression_attribute_names("#net", "network")
                    .expression_attribute_names("#payer", "payer")
                    .expression_attribute_names("#payto", "pay_to")
                    .expression_attribute_names("#res", "resource")
                    .expression_attribute_names("#tx", "tx")
                    .expression_attribute_names("#amount", "amount")
                    .expression_attribute_names("#asset", "asset")
                    .expression_attribute_names("#scheme", "scheme")
                    .expression_attribute_values(":pk", Self::s(&pk))
                    .expression_attribute_values(":from", Self::s(&from))
                    .expression_attribute_values(":settle", Self::s("settle"))
                    .expression_attribute_values(":yes", AttributeValue::Bool(true))
                    .set_exclusive_start_key(start_key)
                    .send()
                    .await
                    .map_err(|e| TransactionStoreError::Dynamo(format!("{e:?}")))?;
                out.extend(page.items().iter().filter_map(Self::from_item));
                if out.len() >= crate::discovery_usage::MAX_WINDOW_SETTLEMENTS {
                    return Ok(Some(out));
                }
                match page.last_evaluated_key() {
                    Some(key) if !key.is_empty() => start_key = Some(key.clone()),
                    _ => break,
                }
            }
        }
        Ok(Some(out))
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
    use std::sync::Arc;

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

    /// Every attribute the Bazaar usage counters read off a settle record, as
    /// this store names it: `ingest` (`src/discovery_usage.rs`) keeps only a
    /// successful settle of the window (`kind`, `ok`, `ts`), matches it on
    /// `network`, `resource`, `payer`, `pay_to`, `asset`, `scheme` and
    /// `amount`, and holds it by its sort key (`ts`, `kind`, `tx`).
    const USAGE_READS: [&str; 11] = [
        "ts", "kind", "ok", "network", "resource", "payer", "pay_to", "asset", "scheme", "amount",
        "tx",
    ];

    const SELLER: &str = "0x1234567890123456789012345678901234567890";
    const USDC_BASE: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
    const URL: &str = "https://api.example.com/quote";

    /// A DynamoDB endpoint on localhost that keeps the rows `PutItem` writes
    /// and answers a `Query` the way DynamoDB does: the rows of the partition
    /// `:pk` names, each with only the attributes `ProjectionExpression` names
    /// (`#placeholders` resolved through `ExpressionAttributeNames`), and a 400
    /// for a declared name or value no expression uses. `UpdateItem` (the
    /// aggregate) answers `{}`. Records every `Query` body.
    async fn projecting_stub(queries: Arc<std::sync::Mutex<Vec<serde_json::Value>>>) -> String {
        use axum::http::{HeaderMap, StatusCode};

        let rows: Arc<std::sync::Mutex<Vec<serde_json::Value>>> = Default::default();
        let app = axum::Router::new().route(
            "/",
            axum::routing::post(move |headers: HeaderMap, body: axum::body::Bytes| {
                let (rows, queries) = (rows.clone(), queries.clone());
                async move {
                    let request: serde_json::Value =
                        serde_json::from_slice(&body).unwrap_or_default();
                    let target = headers
                        .get("x-amz-target")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default();
                    let answer = match target {
                        "DynamoDB_20120810.PutItem" => {
                            rows.lock().unwrap().push(request["Item"].clone());
                            Ok(serde_json::json!({}))
                        }
                        "DynamoDB_20120810.Query" => {
                            queries.lock().unwrap().push(request.clone());
                            query(&request, &rows.lock().unwrap())
                        }
                        _ => Ok(serde_json::json!({})),
                    };
                    let (status, body) = match answer {
                        Ok(body) => (StatusCode::OK, body),
                        Err(message) => (
                            StatusCode::BAD_REQUEST,
                            serde_json::json!({
                                "__type": "com.amazon.coral.validate#ValidationException",
                                "message": message,
                            }),
                        ),
                    };
                    (
                        status,
                        [(
                            axum::http::header::CONTENT_TYPE,
                            "application/x-amz-json-1.0",
                        )],
                        body.to_string(),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    /// The stub's `Query`: see [`projecting_stub`].
    fn query(
        request: &serde_json::Value,
        rows: &[serde_json::Value],
    ) -> Result<serde_json::Value, String> {
        let empty = serde_json::Map::new();
        let names = request["ExpressionAttributeNames"]
            .as_object()
            .unwrap_or(&empty);
        let values = request["ExpressionAttributeValues"]
            .as_object()
            .unwrap_or(&empty);
        let expressions = [
            "KeyConditionExpression",
            "FilterExpression",
            "ProjectionExpression",
        ]
        .iter()
        .filter_map(|k| request[*k].as_str())
        .collect::<Vec<_>>()
        .join(" ");
        let used: std::collections::HashSet<&str> = expressions
            .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '#' | ':')))
            .collect();
        if let Some(unused) = names
            .keys()
            .chain(values.keys())
            .find(|k| !used.contains(k.as_str()))
        {
            return Err(format!("unused in expressions: {unused}"));
        }
        let projection: Option<Vec<&str>> = request["ProjectionExpression"].as_str().map(|p| {
            p.split(',')
                .map(|a| {
                    let a = a.trim();
                    names.get(a).and_then(|n| n.as_str()).unwrap_or(a)
                })
                .collect()
        });
        let items: Vec<serde_json::Value> = rows
            .iter()
            .filter(|row| Some(&row["pk"]) == values.get(":pk"))
            .map(|row| match &projection {
                None => row.clone(),
                Some(attributes) => attributes
                    .iter()
                    .filter_map(|a| row.get(*a).map(|v| (a.to_string(), v.clone())))
                    .collect::<serde_json::Map<_, _>>()
                    .into(),
            })
            .collect();
        Ok(serde_json::json!({
            "Items": items,
            "Count": items.len(),
            "ScannedCount": items.len(),
        }))
    }

    fn stub_store(endpoint: String) -> DynamoTransactionStore {
        use aws_sdk_dynamodb::config::{BehaviorVersion, Builder, Credentials, Region};

        let config = Builder::new()
            .behavior_version(BehaviorVersion::latest())
            .endpoint_url(endpoint)
            .region(Region::new("us-east-2"))
            .credentials_provider(Credentials::new("test", "test", None, None, "test"))
            .build();
        DynamoTransactionStore::new(
            aws_sdk_dynamodb::Client::from_conf(config),
            "facilitator-transactions".to_string(),
            DEFAULT_TTL_DAYS,
        )
    }

    /// `settles_since` reads through a projection, and DynamoDB answers an
    /// attribute the projection leaves out by leaving it out, with no error.
    /// Leave out one the usage counters read and `ingest` skips every
    /// settlement: `usage` reads zero in production while every store the rest
    /// of the suite reads through is a stub that returns whole records. So:
    /// the projection names each of [`USAGE_READS`], a settle written by
    /// `record` comes back from `settles_since` with each of them, and the
    /// usage counters count it.
    #[tokio::test]
    async fn the_settles_since_projection_names_every_attribute_usage_reads() {
        use crate::caip2::Caip2NetworkId;
        use crate::discovery_price::{CatalogPaymentOption, CatalogScheme};
        use crate::types::{MixedAddress, Scheme, TokenAmount};

        let queries = Arc::new(std::sync::Mutex::new(Vec::new()));
        let store = stub_store(projecting_stub(queries.clone()).await);
        let now = crate::events::now_ms();
        let written = TransactionRecord {
            ts: now - 60_000,
            kind: "settle".into(),
            network: "base".into(),
            ok: true,
            payer: Some("0x00000000000000000000000000000000000000a1".into()),
            tx: Some("0xfeed".into()),
            amount: Some("10000".into()),
            asset: Some(USDC_BASE.into()),
            resource: Some(URL.into()),
            pay_to: Some(SELLER.into()),
            description: Some("not read by the usage counters".into()),
            scheme: Some("exact".into()),
        };
        store
            .record(written.clone())
            .await
            .expect("the stub takes the write");

        let read = store
            .settles_since(now - 3_600_000)
            .await
            .expect("the stub answers the Query")
            .expect("this store keeps history");

        let request = queries.lock().unwrap().last().cloned().expect("a Query");
        let projected: Vec<&str> = request["ProjectionExpression"]
            .as_str()
            .expect("settles_since reads through a projection")
            .split(',')
            .map(|a| {
                let a = a.trim();
                request["ExpressionAttributeNames"][a].as_str().unwrap_or(a)
            })
            .collect();
        for attribute in USAGE_READS {
            assert!(
                projected.contains(&attribute),
                "the projection leaves out `{attribute}`: {projected:?}"
            );
        }

        assert_eq!(read.len(), 1, "{read:?}");
        let expected = serde_json::to_value(TransactionRecord {
            description: None,
            ..written.clone()
        })
        .unwrap();
        let got = serde_json::to_value(&read[0]).unwrap();
        for attribute in USAGE_READS {
            let field = match attribute {
                "pay_to" => "payTo",
                other => other,
            };
            assert_eq!(
                got[field], expected[field],
                "`{attribute}` did not come back"
            );
        }

        let listing = crate::types_v2::DiscoveryResource::new(
            url::Url::parse(URL).unwrap(),
            "http".to_string(),
            String::new(),
            vec![CatalogPaymentOption::new(
                CatalogScheme::Known(Scheme::Exact),
                Caip2NetworkId::eip155(8453),
                MixedAddress::Evm(USDC_BASE.parse().unwrap()),
                TokenAmount::from(10_000u64),
                MixedAddress::Evm(SELLER.parse().unwrap()),
                300,
            )],
        );
        let tracker = crate::discovery_usage::UsageTracker::new();
        tracker
            .refresh(&store, now)
            .await
            .expect("the stub answers every day of the window");
        let usage = tracker
            .snapshot()
            .expect("read once")
            .of(&listing)
            .expect("not a template");
        assert_eq!(usage.calls_30d, 1, "{usage:?}");
        assert_eq!(usage.unique_payers_30d, 1, "{usage:?}");
    }
}
