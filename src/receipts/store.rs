//! Durable receipt reservations. These rows deliberately have NO DynamoDB TTL:
//! expiry of a cache must never authorize a second payment.
use super::{Record, Result};
use async_trait::async_trait;
use aws_sdk_dynamodb::types::{AttributeValue as A, Put, TransactWriteItem};
use std::{collections::HashMap, sync::Arc, time::Duration};

#[async_trait]
pub trait Store: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<Record>>;
    /// All aliases and the record are inserted atomically, or none are.
    async fn reserve(&self, record: &Record, aliases: &[String]) -> Result<bool>;
    async fn save(&self, record: &Record, previous_revision: u64) -> Result<bool>;
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

    #[tokio::test]
    #[ignore = "requires an explicitly configured local DynamoDB emulator"]
    async fn local_dynamodb_atomic_admission_cas_and_no_ttl() {
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
                    store.reserve(&record, &aliases).await.unwrap()
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

    async fn reserve(&self, record: &Record, aliases: &[String]) -> Result<bool> {
        let mut items = vec![Self::item(record)?];
        items.extend(aliases.iter().map(|key| {
            HashMap::from([
                ("idempotency_key".into(), A::S(key.clone())),
                (
                    "receipt_ref".into(),
                    A::S(record.receipt.receipt_id.clone()),
                ),
            ])
        }));
        let mut tx = self.client.transact_write_items();
        for item in items {
            let put = Put::builder()
                .table_name(&self.table)
                .set_item(Some(item))
                .condition_expression("attribute_not_exists(idempotency_key)")
                .build()
                .map_err(|_| "receipt_store_invalid_write")?;
            tx = tx.transact_items(TransactWriteItem::builder().put(put).build());
        }
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
}

#[cfg(test)]
#[derive(Default)]
pub struct MemoryStore(pub tokio::sync::Mutex<HashMap<String, Record>>);

#[cfg(test)]
#[async_trait]
impl Store for MemoryStore {
    async fn get(&self, key: &str) -> Result<Option<Record>> {
        Ok(self.0.lock().await.get(key).cloned())
    }
    async fn reserve(&self, record: &Record, aliases: &[String]) -> Result<bool> {
        let mut rows = self.0.lock().await;
        if aliases.iter().any(|a| rows.contains_key(a)) {
            return Ok(false);
        }
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
}
