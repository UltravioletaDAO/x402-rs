//! Durable receipt reservations. These rows deliberately have NO DynamoDB TTL:
//! expiry of a cache must never authorize a second payment.
use super::{Record, Result};
use async_trait::async_trait;
use aws_sdk_dynamodb::{
    operation::transact_write_items::builders::TransactWriteItemsFluentBuilder,
    types::{AttributeValue as A, Put, TransactWriteItem},
};
use std::{collections::HashMap, sync::Arc, time::Duration};

#[async_trait]
pub trait Store: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<Record>>;
    /// All aliases and the record are inserted atomically, or none are.
    ///
    /// `token` makes the write idempotent, here and in `readmit`: the SDK
    /// resending a write whose success was lost gets that success back instead
    /// of failing its own condition, which would leave an admission that
    /// nobody runs. One fresh token per logical write, never derived from the
    /// record: two resends of one payment build identical records, and a shared
    /// token would tell both of them that they won.
    async fn reserve(&self, record: &Record, aliases: &[String], token: &str) -> Result<bool>;
    async fn save(&self, record: &Record, previous_revision: u64) -> Result<bool>;
    /// The record moves on from `previous_revision` and the new aliases are
    /// inserted, atomically, or nothing is written. Only puts: aliases are
    /// never deleted, so an admission keeps every binding it ever had.
    async fn readmit(
        &self,
        record: &Record,
        previous_revision: u64,
        aliases: &[String],
        token: &str,
    ) -> Result<bool>;
    /// Every receipt record. A full table scan, for the operator command only.
    async fn records(&self) -> Result<Vec<Record>>;
}

pub struct DynamoStore {
    client: aws_sdk_dynamodb::Client,
    table: String,
}

#[cfg(test)]
mod integration {
    use super::*;
    use aws_sdk_dynamodb::{
        config::{Credentials, Region},
        types::{AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType},
    };

    /// Readmission is one conditional transaction of puts: exactly one of many
    /// concurrent resends wins the revision, only its new alias is written, a
    /// stale revision or a taken alias writes nothing, and the operator scan
    /// finds the receipt record and none of its aliases.
    #[tokio::test]
    #[ignore = "requires an explicitly configured local DynamoDB emulator"]
    async fn local_dynamodb_readmission_has_one_winner_and_the_scan_finds_receipts() {
        let (client, table) = local_table().await;
        let first = Arc::new(DynamoStore {
            client: client.clone(),
            table: table.clone(),
        });
        let second = Arc::new(DynamoStore {
            client: client.clone(),
            table: table.clone(),
        });
        let mut record = crate::receipts::tests::fixture_record();
        let auth = "receipt:auth:v1:local-readmission".to_owned();
        assert!(first
            .reserve(&record, std::slice::from_ref(&auth), &token())
            .await
            .unwrap());
        record.receipt.revision = 2;
        record.receipt.status = "rejected".into();
        assert!(first.save(&record, 1).await.unwrap());
        let mut tasks = vec![];
        for n in 0..20 {
            let store = if n % 2 == 0 {
                first.clone()
            } else {
                second.clone()
            };
            let mut next = record.clone();
            next.receipt.revision = 3;
            next.receipt.status = "unknown".into();
            let alias = format!("receipt:idem:v1:local-{n}");
            tasks.push(tokio::spawn(async move {
                let won = store
                    .readmit(&next, 2, std::slice::from_ref(&alias), &token())
                    .await
                    .unwrap();
                (won, alias)
            }));
        }
        let mut winners = vec![];
        for task in tasks {
            let (won, alias) = task.await.unwrap();
            if won {
                winners.push(alias);
            }
        }
        assert_eq!(winners.len(), 1, "{winners:?}");
        for n in 0..20 {
            let alias = format!("receipt:idem:v1:local-{n}");
            let found = second.get(&alias).await.unwrap();
            assert_eq!(found.is_some(), alias == winners[0], "{alias}");
        }
        let stored = second.get(&auth).await.unwrap().unwrap();
        assert_eq!(stored.receipt.revision, 3);
        assert_eq!(stored.receipt.status, "unknown");
        let mut stale = record.clone();
        stale.receipt.revision = 3;
        assert!(!first.readmit(&stale, 2, &[], &token()).await.unwrap());
        let mut next = stored.clone();
        next.receipt.revision = 4;
        assert!(!first
            .readmit(&next, 3, std::slice::from_ref(&winners[0]), &token())
            .await
            .unwrap());
        assert_eq!(
            second.get(&auth).await.unwrap().unwrap().receipt.revision,
            3
        );
        let records = second.records().await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].receipt.receipt_id, record.receipt.receipt_id);
        assert_eq!(records[0].receipt.revision, 3);
        let rows = client.scan().table_name(&table).send().await.unwrap();
        assert_eq!(rows.items().len(), 3, "record, authorization, one key");
        for row in rows.items() {
            assert!(!row.contains_key("ttl") && !row.contains_key("expires_at"));
        }
        client
            .delete_table()
            .table_name(&table)
            .send()
            .await
            .unwrap();
    }

    /// The AWS SDK resends a write whose answer was lost as the same request,
    /// token included. DynamoDB then reports the original success instead of
    /// failing the write's own condition; a new token is a new write, refused.
    #[tokio::test]
    #[ignore = "requires an explicitly configured local DynamoDB emulator"]
    async fn local_dynamodb_a_resent_write_keeps_its_success() {
        let (client, table) = local_table().await;
        let store = DynamoStore {
            client: client.clone(),
            table: table.clone(),
        };
        let mut record = crate::receipts::tests::fixture_record();
        let auth = "receipt:auth:v1:local-token".to_owned();
        let aliases = [auth.clone()];
        let reservation = token();
        assert!(store
            .reserve(&record, &aliases, &reservation)
            .await
            .unwrap());
        assert!(
            store
                .reserve(&record, &aliases, &reservation)
                .await
                .unwrap(),
            "a resent reservation lost its success"
        );
        assert!(!store.reserve(&record, &aliases, &token()).await.unwrap());
        record.receipt.revision = 2;
        record.receipt.status = "rejected".into();
        assert!(store.save(&record, 1).await.unwrap());
        let mut next = record.clone();
        next.receipt.revision = 3;
        next.receipt.status = "unknown".into();
        let key = ["receipt:idem:v1:local-token".to_owned()];
        let readmission = token();
        assert!(store.readmit(&next, 2, &key, &readmission).await.unwrap());
        assert!(
            store.readmit(&next, 2, &key, &readmission).await.unwrap(),
            "a resent readmission lost its success"
        );
        assert!(!store.readmit(&next, 2, &key, &token()).await.unwrap());
        assert_eq!(store.get(&auth).await.unwrap().unwrap().receipt.revision, 3);
        client
            .delete_table()
            .table_name(&table)
            .send()
            .await
            .unwrap();
    }

    fn token() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    async fn local_table() -> (aws_sdk_dynamodb::Client, String) {
        let endpoint = std::env::var("DYNAMODB_LOCAL_URL").expect("local emulator URL");
        assert!(
            endpoint.starts_with("http://127.0.0.1:")
                || endpoint.starts_with("http://host.docker.internal:")
        );
        let config = aws_sdk_dynamodb::config::Builder::new()
            .behavior_version_latest()
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new("local", "local", None, None, "local-test"))
            .endpoint_url(endpoint)
            .build();
        let client = aws_sdk_dynamodb::Client::from_conf(config);
        let table = format!("receipt-test-{}", uuid::Uuid::new_v4());
        client
            .create_table()
            .table_name(&table)
            .billing_mode(BillingMode::PayPerRequest)
            .attribute_definitions(
                AttributeDefinition::builder()
                    .attribute_name("idempotency_key")
                    .attribute_type(ScalarAttributeType::S)
                    .build()
                    .unwrap(),
            )
            .key_schema(
                KeySchemaElement::builder()
                    .attribute_name("idempotency_key")
                    .key_type(KeyType::Hash)
                    .build()
                    .unwrap(),
            )
            .send()
            .await
            .unwrap();
        (client, table)
    }

    #[tokio::test]
    #[ignore = "requires an explicitly configured local DynamoDB emulator"]
    async fn local_dynamodb_atomic_admission_cas_and_no_ttl() {
        let (client, table) = local_table().await;
        let first = Arc::new(DynamoStore {
            client: client.clone(),
            table: table.clone(),
        });
        let second = Arc::new(DynamoStore {
            client: client.clone(),
            table: table.clone(),
        });
        let records = [
            ("arc-testnet", crate::receipts::tests::fixture_record()),
            ("base", crate::receipts::tests::base_fixture_record()),
        ];
        for (network, mut record) in records {
            let aliases = vec![
                format!("receipt:auth:v1:local-{network}"),
                format!("receipt:purchase:v1:local-{network}"),
            ];
            let mut tasks = vec![];
            for n in 0..20 {
                let store = if n % 2 == 0 {
                    first.clone()
                } else {
                    second.clone()
                };
                let record = record.clone();
                let aliases = aliases.clone();
                tasks.push(tokio::spawn(async move {
                    store.reserve(&record, &aliases, &token()).await.unwrap()
                }));
            }
            let mut admitted = 0;
            for task in tasks {
                admitted += usize::from(task.await.unwrap());
            }
            assert_eq!(admitted, 1, "{network}");
            record.receipt.revision += 1;
            record.receipt.status = "pending".into();
            assert!(first.save(&record, 1).await.unwrap(), "{network}");
            assert!(!second.save(&record, 1).await.unwrap(), "{network}");
            let stored = second.get(&aliases[1]).await.unwrap().unwrap();
            assert_eq!(stored.receipt.status, "pending", "{network}");
            assert_eq!(stored.receipt.network, record.receipt.network);
        }
        let rows = client.scan().table_name(&table).send().await.unwrap();
        assert_eq!(rows.items().len(), 6);
        for row in rows.items() {
            assert!(!row.contains_key("ttl") && !row.contains_key("expires_at"));
        }
        client
            .delete_table()
            .table_name(&table)
            .send()
            .await
            .unwrap();
    }
}

/// What is sent, without sending it.
#[cfg(test)]
mod requests {
    use super::*;
    use aws_sdk_dynamodb::config::{Credentials, Region};

    #[test]
    fn both_admission_writes_carry_the_callers_token() {
        let config = aws_sdk_dynamodb::config::Builder::new()
            .behavior_version_latest()
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new("offline", "offline", None, None, "test"))
            .build();
        let store = DynamoStore {
            client: aws_sdk_dynamodb::Client::from_conf(config),
            table: "offline".into(),
        };
        let record = crate::receipts::tests::fixture_record();
        let aliases = ["receipt:auth:v1:offline".to_owned()];
        let reserve = store.reserve_request(&record, &aliases, "first").unwrap();
        assert_eq!(reserve.get_client_request_token().as_deref(), Some("first"));
        assert_eq!(reserve.get_transact_items().as_ref().map(Vec::len), Some(2));
        let readmit = store
            .readmit_request(&record, 1, &aliases, "second")
            .unwrap();
        assert_eq!(
            readmit.get_client_request_token().as_deref(),
            Some("second")
        );
        assert_eq!(readmit.get_transact_items().as_ref().map(Vec::len), Some(2));
    }
}

impl DynamoStore {
    pub async fn from_env() -> Option<Arc<dyn Store>> {
        let table = std::env::var("IDEMPOTENCY_TABLE_NAME")
            .ok()
            .filter(|s| !s.is_empty())?;
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        Some(Arc::new(Self {
            client: aws_sdk_dynamodb::Client::new(&config),
            table,
        }))
    }

    fn item(record: &Record) -> Result<HashMap<String, A>> {
        let data = serde_json::to_string(record).map_err(|_| "receipt_encoding_failed")?;
        if data.len() > 300_000 {
            return Err("receipt_record_too_large".into());
        }
        Ok(HashMap::from([
            (
                "idempotency_key".into(),
                A::S(format!("receipt:v1:{}", record.receipt.receipt_id)),
            ),
            ("revision".into(), A::N(record.receipt.revision.to_string())),
            ("data".into(), A::S(data)),
        ]))
    }

    async fn read_item(&self, key: &str) -> Result<Option<HashMap<String, A>>> {
        tokio::time::timeout(
            Duration::from_secs(5),
            self.client
                .get_item()
                .table_name(&self.table)
                .key("idempotency_key", A::S(key.into()))
                .consistent_read(true)
                .send(),
        )
        .await
        .map_err(|_| "receipt_store_timeout")?
        .map(|r| r.item)
        .map_err(|_| "receipt_store_unavailable".into())
    }

    fn alias_put(&self, key: &str, record: &Record) -> Result<TransactWriteItem> {
        let put = Put::builder()
            .table_name(&self.table)
            .set_item(Some(HashMap::from([
                ("idempotency_key".into(), A::S(key.to_owned())),
                (
                    "receipt_ref".into(),
                    A::S(record.receipt.receipt_id.clone()),
                ),
            ])))
            .condition_expression("attribute_not_exists(idempotency_key)")
            .build()
            .map_err(|_| "receipt_store_invalid_write")?;
        Ok(TransactWriteItem::builder().put(put).build())
    }

    fn reserve_request(
        &self,
        record: &Record,
        aliases: &[String],
        token: &str,
    ) -> Result<TransactWriteItemsFluentBuilder> {
        let row = Put::builder()
            .table_name(&self.table)
            .set_item(Some(Self::item(record)?))
            .condition_expression("attribute_not_exists(idempotency_key)")
            .build()
            .map_err(|_| "receipt_store_invalid_write")?;
        let mut tx = self
            .client
            .transact_write_items()
            .client_request_token(token)
            .transact_items(TransactWriteItem::builder().put(row).build());
        for key in aliases {
            tx = tx.transact_items(self.alias_put(key, record)?);
        }
        Ok(tx)
    }

    fn readmit_request(
        &self,
        record: &Record,
        previous_revision: u64,
        aliases: &[String],
        token: &str,
    ) -> Result<TransactWriteItemsFluentBuilder> {
        let row = Put::builder()
            .table_name(&self.table)
            .set_item(Some(Self::item(record)?))
            .condition_expression("revision = :previous")
            .expression_attribute_values(":previous", A::N(previous_revision.to_string()))
            .build()
            .map_err(|_| "receipt_store_invalid_write")?;
        let mut tx = self
            .client
            .transact_write_items()
            .client_request_token(token)
            .transact_items(TransactWriteItem::builder().put(row).build());
        for key in aliases {
            tx = tx.transact_items(self.alias_put(key, record)?);
        }
        Ok(tx)
    }
}

#[async_trait]
impl Store for DynamoStore {
    async fn get(&self, key: &str) -> Result<Option<Record>> {
        let Some(mut item) = self.read_item(key).await? else {
            return Ok(None);
        };
        if let Some(id) = item.get("receipt_ref").and_then(|v| v.as_s().ok()) {
            item = self
                .read_item(&format!("receipt:v1:{id}"))
                .await?
                .ok_or("receipt_store_corrupt")?;
        }
        let data = item
            .get("data")
            .and_then(|v| v.as_s().ok())
            .ok_or("receipt_store_corrupt")?;
        serde_json::from_str(data)
            .map(Some)
            .map_err(|_| "receipt_store_corrupt".into())
    }

    async fn reserve(&self, record: &Record, aliases: &[String], token: &str) -> Result<bool> {
        let tx = self.reserve_request(record, aliases, token)?;
        match tokio::time::timeout(Duration::from_secs(5), tx.send()).await {
            Ok(Ok(_)) => Ok(true),
            // A timeout may have committed. Never claim ownership after an
            // ambiguous transaction; a later request can read the reservation.
            _ => {
                for key in aliases {
                    if self.get(key).await?.is_some() {
                        return Ok(false);
                    }
                }
                Err("receipt_store_unavailable".into())
            }
        }
    }

    async fn save(&self, record: &Record, previous_revision: u64) -> Result<bool> {
        let write = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(Self::item(record)?))
            .condition_expression("revision = :previous")
            .expression_attribute_values(":previous", A::N(previous_revision.to_string()));
        match tokio::time::timeout(Duration::from_secs(5), write.send()).await {
            Ok(Ok(_)) => Ok(true),
            Ok(Err(e))
                if e.as_service_error()
                    .is_some_and(|e| e.is_conditional_check_failed_exception()) =>
            {
                Ok(false)
            }
            _ => Err("receipt_store_unavailable".into()),
        }
    }

    async fn readmit(
        &self,
        record: &Record,
        previous_revision: u64,
        aliases: &[String],
        token: &str,
    ) -> Result<bool> {
        let tx = self.readmit_request(record, previous_revision, aliases, token)?;
        match tokio::time::timeout(Duration::from_secs(5), tx.send()).await {
            Ok(Ok(_)) => Ok(true),
            // A cancelled transaction wrote nothing: the revision moved on, an
            // alias exists, or another transaction held one of the items.
            Ok(Err(e))
                if e.as_service_error()
                    .is_some_and(|e| e.is_transaction_canceled_exception()) =>
            {
                Ok(false)
            }
            // Anything else may still have committed; the caller never runs
            // under it and closes it again (`release_unrun`).
            _ => Err("receipt_store_unavailable".into()),
        }
    }

    async fn records(&self) -> Result<Vec<Record>> {
        let mut records = Vec::new();
        let mut start = None;
        loop {
            let page = tokio::time::timeout(
                Duration::from_secs(30),
                self.client
                    .scan()
                    .table_name(&self.table)
                    .filter_expression("begins_with(idempotency_key, :prefix)")
                    .expression_attribute_values(":prefix", A::S("receipt:v1:".into()))
                    .consistent_read(true)
                    .set_exclusive_start_key(start)
                    .send(),
            )
            .await
            .map_err(|_| "receipt_store_timeout")?
            .map_err(|_| "receipt_store_unavailable")?;
            for item in page.items() {
                let data = item
                    .get("data")
                    .and_then(|v| v.as_s().ok())
                    .ok_or("receipt_store_corrupt")?;
                records.push(serde_json::from_str(data).map_err(|_| "receipt_store_corrupt")?);
            }
            start = page.last_evaluated_key().cloned();
            if start.is_none() {
                return Ok(records);
            }
        }
    }
}

/// Rows, and the tokens of the writes that committed: a write sent again
/// with its token reports its original success, as DynamoDB does.
#[cfg(test)]
#[derive(Default)]
pub struct MemoryStore(
    pub tokio::sync::Mutex<HashMap<String, Record>>,
    tokio::sync::Mutex<std::collections::HashSet<String>>,
);

#[cfg(test)]
#[async_trait]
impl Store for MemoryStore {
    async fn get(&self, key: &str) -> Result<Option<Record>> {
        Ok(self.0.lock().await.get(key).cloned())
    }
    async fn reserve(&self, record: &Record, aliases: &[String], token: &str) -> Result<bool> {
        let mut rows = self.0.lock().await;
        let mut committed = self.1.lock().await;
        if committed.contains(token) {
            return Ok(true);
        }
        if aliases.iter().any(|a| rows.contains_key(a)) {
            return Ok(false);
        }
        committed.insert(token.to_owned());
        rows.insert(
            format!("receipt:v1:{}", record.receipt.receipt_id),
            record.clone(),
        );
        for alias in aliases {
            rows.insert(alias.clone(), record.clone());
        }
        Ok(true)
    }
    async fn save(&self, record: &Record, previous_revision: u64) -> Result<bool> {
        let mut rows = self.0.lock().await;
        let key = format!("receipt:v1:{}", record.receipt.receipt_id);
        if rows
            .get(&key)
            .is_none_or(|r| r.receipt.revision != previous_revision)
        {
            return Ok(false);
        }
        for row in rows.values_mut() {
            if row.receipt.receipt_id == record.receipt.receipt_id {
                *row = record.clone();
            }
        }
        Ok(true)
    }
    async fn readmit(
        &self,
        record: &Record,
        previous_revision: u64,
        aliases: &[String],
        token: &str,
    ) -> Result<bool> {
        let mut rows = self.0.lock().await;
        let mut committed = self.1.lock().await;
        if committed.contains(token) {
            return Ok(true);
        }
        let key = format!("receipt:v1:{}", record.receipt.receipt_id);
        if rows
            .get(&key)
            .is_none_or(|r| r.receipt.revision != previous_revision)
            || aliases.iter().any(|a| rows.contains_key(a))
        {
            return Ok(false);
        }
        committed.insert(token.to_owned());
        for row in rows.values_mut() {
            if row.receipt.receipt_id == record.receipt.receipt_id {
                *row = record.clone();
            }
        }
        for alias in aliases {
            rows.insert(alias.clone(), record.clone());
        }
        Ok(true)
    }
    async fn records(&self) -> Result<Vec<Record>> {
        let rows = self.0.lock().await;
        Ok(rows
            .iter()
            .filter(|(key, _)| key.starts_with("receipt:v1:"))
            .map(|(_, record)| record.clone())
            .collect())
    }
}
