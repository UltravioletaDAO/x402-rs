//! Durable admission, sponsor quota and crash recovery. The quota is charged
//! conservatively at the maximum signed fee and never refunded automatically.
//! A network/transaction ID can acquire only one intent, across all replicas.
use super::codec::{Intent, Result};
use async_trait::async_trait;
use aws_sdk_dynamodb::{
    types::{AttributeValue as A, Put, TransactWriteItem, Update},
    Client,
};
use serde::{Deserialize, Serialize};
use std::{
    future::Future,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const LEASE_SECONDS: u64 = 120;
const RETENTION_SECONDS: u64 = 7 * 24 * 60 * 60;
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Reserved,
    Prepared,
    Submitted,
    Uncertain,
    Confirmed,
    Failed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub intent: Intent,
    pub owner: String,
    pub lease_until: u64,
    pub state: State,
    pub signed: Option<String>,
    pub consensus_status: Option<String>,
}
impl Record {
    pub fn terminal(&self) -> bool {
        matches!(self.state, State::Confirmed | State::Failed)
    }
    pub fn state_name(&self) -> &'static str {
        match self.state {
            State::Reserved => "reserved",
            State::Prepared => "prepared",
            State::Submitted => "submitted",
            State::Uncertain => "uncertain",
            State::Confirmed => "confirmed",
            State::Failed => "failed",
        }
    }
}
#[async_trait]
pub trait Store: Send + Sync {
    async fn read(&self, key: &str) -> Result<Option<Record>>;
    /// Return a record owned by this request, an existing terminal result, or
    /// a record owned by another request (which the caller must not submit).
    async fn reserve(
        &self,
        key: &str,
        network: &str,
        intent: &Intent,
        owner: &str,
        budget: u64,
    ) -> Result<Record>;
    async fn save(&self, key: &str, record: &Record) -> Result<()>;
    async fn pending(&self, network: &str) -> Result<Vec<(String, Record)>>;
    async fn health(&self) -> Result<()>;
}
pub struct DynamoStore {
    client: Client,
    table: String,
}
async fn bounded<T, E: std::fmt::Display>(
    f: impl Future<Output = std::result::Result<T, E>>,
) -> Result<T> {
    tokio::time::timeout(Duration::from_secs(5), f)
        .await
        .map_err(|_| "Hedera settlement storage timed out")?
        .map_err(|e| format!("Hedera settlement storage: {e}"))
}
impl DynamoStore {
    pub async fn new(table: String) -> Self {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        Self {
            client: Client::new(&config),
            table,
        }
    }
    fn item(key: &str, record: &Record) -> Result<std::collections::HashMap<String, A>> {
        let mut item = std::collections::HashMap::from([
            ("id".into(), A::S(key.into())),
            (
                "fingerprint".into(),
                A::S(record.intent.fingerprint.clone()),
            ),
            ("owner".into(), A::S(record.owner.clone())),
            ("lease".into(), A::N(record.lease_until.to_string())),
            ("status".into(), A::S(record.state_name().into())),
            (
                "data".into(),
                A::S(serde_json::to_string(record).map_err(|_| "record serialization failed")?),
            ),
            (
                "expires_at".into(),
                A::N(
                    record
                        .intent
                        .expires_at
                        .saturating_add(RETENTION_SECONDS)
                        .to_string(),
                ),
            ),
        ]);
        if record.signed.is_some() && !record.terminal() {
            let (network, _) = key.split_once('#').ok_or("invalid settlement key")?;
            item.insert("recovery_network".into(), A::S(network.into()));
            item.insert(
                "recovery_lease".into(),
                A::N(record.lease_until.to_string()),
            );
        }
        Ok(item)
    }
    async fn claim(&self, key: &str, mut record: Record, owner: &str, at: u64) -> Result<Record> {
        if record.terminal() || record.lease_until >= at {
            return Ok(record);
        }
        record.owner = owner.into();
        record.lease_until = at + LEASE_SECONDS;
        let outcome = bounded(
            self.client
                .put_item()
                .table_name(&self.table)
                .set_item(Some(Self::item(key, &record)?))
                .condition_expression(
                    "#lease < :now AND fingerprint = :fp AND #status <> :ok AND #status <> :failed",
                )
                .expression_attribute_names("#lease", "lease")
                .expression_attribute_names("#status", "status")
                .expression_attribute_values(":now", A::N(at.to_string()))
                .expression_attribute_values(":fp", A::S(record.intent.fingerprint.clone()))
                .expression_attribute_values(":ok", A::S("confirmed".into()))
                .expression_attribute_values(":failed", A::S("failed".into()))
                .send(),
        )
        .await;
        match outcome {
            Ok(_) => Ok(record),
            Err(error) => self.read(key).await?.ok_or(error),
        }
    }
}
#[async_trait]
impl Store for DynamoStore {
    async fn read(&self, key: &str) -> Result<Option<Record>> {
        let response = bounded(
            self.client
                .get_item()
                .table_name(&self.table)
                .key("id", A::S(key.into()))
                .consistent_read(true)
                .send(),
        )
        .await?;
        response
            .item
            .map(|item| {
                let data = item
                    .get("data")
                    .and_then(|v| v.as_s().ok())
                    .ok_or("corrupt Hedera settlement record")?;
                serde_json::from_str(data).map_err(|_| "corrupt Hedera settlement record".into())
            })
            .transpose()
    }
    async fn reserve(
        &self,
        key: &str,
        network: &str,
        intent: &Intent,
        owner: &str,
        budget: u64,
    ) -> Result<Record> {
        let at = now();
        if let Some(existing) = self.read(key).await? {
            if existing.intent.fingerprint != intent.fingerprint {
                return Err("transaction ID already binds another intent".into());
            }
            return self.claim(key, existing, owner, at).await;
        }
        if at.saturating_add(5) > intent.expires_at || intent.fee > budget {
            return Err("expired payment or sponsor quota exceeded".into());
        }
        let record = Record {
            intent: intent.clone(),
            owner: owner.into(),
            lease_until: at + LEASE_SECONDS,
            state: State::Reserved,
            signed: None,
            consensus_status: None,
        };
        let put = Put::builder()
            .table_name(&self.table)
            .set_item(Some(Self::item(key, &record)?))
            .condition_expression("attribute_not_exists(id)")
            .build()
            .map_err(|_| "invalid settlement reservation")?;
        let update = Update::builder()
            .table_name(&self.table)
            .key("id", A::S(format!("budget#{network}#{}", at / 86_400)))
            .update_expression("SET expires_at = :expiry ADD spent :fee")
            .condition_expression("attribute_not_exists(spent) OR spent <= :remaining")
            .expression_attribute_values(":expiry", A::N((at + RETENTION_SECONDS).to_string()))
            .expression_attribute_values(":fee", A::N(intent.fee.to_string()))
            .expression_attribute_values(":remaining", A::N((budget - intent.fee).to_string()))
            .build()
            .map_err(|_| "invalid sponsor quota reservation")?;
        let outcome = bounded(
            self.client
                .transact_write_items()
                .transact_items(TransactWriteItem::builder().put(put).build())
                .transact_items(TransactWriteItem::builder().update(update).build())
                .send(),
        )
        .await;
        match outcome {
            Ok(_) => Ok(record),
            Err(error) => match self.read(key).await? {
                Some(existing) if existing.intent.fingerprint == intent.fingerprint => Ok(existing),
                Some(_) => Err("transaction ID already binds another intent".into()),
                None => Err(format!("sponsor quota reservation failed: {error}")),
            },
        }
    }
    async fn save(&self, key: &str, record: &Record) -> Result<()> {
        bounded(self.client.put_item().table_name(&self.table).set_item(Some(Self::item(key, record)?))
            .condition_expression("#owner = :owner AND fingerprint = :fp AND #status <> :ok AND #status <> :failed")
            .expression_attribute_names("#owner", "owner").expression_attribute_names("#status", "status")
            .expression_attribute_values(":owner", A::S(record.owner.clone()))
            .expression_attribute_values(":fp", A::S(record.intent.fingerprint.clone()))
            .expression_attribute_values(":ok", A::S("confirmed".into())).expression_attribute_values(":failed", A::S("failed".into())).send()).await?;
        Ok(())
    }
    async fn health(&self) -> Result<()> {
        let table = bounded(self.client.describe_table().table_name(&self.table).send()).await?;
        if table
            .table
            .as_ref()
            .and_then(|t| t.table_status.as_ref())
            .map(|s| s.as_str())
            != Some("ACTIVE")
        {
            return Err("Hedera settlement table is not active".into());
        }
        Ok(())
    }
    async fn pending(&self, network: &str) -> Result<Vec<(String, Record)>> {
        let response = bounded(
            self.client
                .query()
                .table_name(&self.table)
                .index_name("recovery")
                .key_condition_expression("recovery_network = :network AND recovery_lease < :now")
                .expression_attribute_values(":network", A::S(network.into()))
                .expression_attribute_values(":now", A::N(now().to_string()))
                .limit(8)
                .send(),
        )
        .await?;
        response
            .items()
            .iter()
            .map(|item| {
                let key = item
                    .get("id")
                    .and_then(|v| v.as_s().ok())
                    .ok_or("missing settlement key")?;
                let data = item
                    .get("data")
                    .and_then(|v| v.as_s().ok())
                    .ok_or("missing settlement record")?;
                let record: Record =
                    serde_json::from_str(data).map_err(|_| "invalid recovery record")?;
                Ok((key.clone(), record))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Real DynamoDB conditional writes, across independent clients. Never
    /// accesses a chain or signer, and uses a unique non-production partition.
    #[tokio::test]
    #[ignore = "requires HEDERA_TEST_TABLE and explicit AWS credentials"]
    async fn dynamodb_atomic_quota_leases_and_restart_recovery() {
        let table = std::env::var("HEDERA_TEST_TABLE").expect("explicit test table required");
        let store = Arc::new(DynamoStore::new(table.clone()).await);
        store.health().await.unwrap();
        let network = format!("storetest:{}", uuid::Uuid::new_v4());
        let intent = Intent {
            transaction_id: "0.0.3003@1789600000.000000001".into(),
            fingerprint: "test-intent".into(),
            payer: "0.0.1001".parse().unwrap(),
            pay_to: "0.0.2002".parse().unwrap(),
            asset: "0.0.0".parse().unwrap(),
            amount: 1,
            fee: 10,
            expires_at: now() + 120,
            expected_decimals: Some(8),
        };
        let key = format!("{network}#{}", intent.transaction_id);
        let mut tasks = tokio::task::JoinSet::new();
        for i in 0..12 {
            let store = store.clone();
            let key = key.clone();
            let network = network.clone();
            let intent = intent.clone();
            tasks.spawn(async move {
                let owner = format!("worker-{i}");
                let record = store
                    .reserve(&key, &network, &intent, &owner, 10)
                    .await
                    .unwrap();
                (record.owner == owner, record)
            });
        }
        let mut owners = 0;
        let mut winner = None;
        while let Some(result) = tasks.join_next().await {
            let (owns, record) = result.unwrap();
            if owns {
                owners += 1;
                winner = Some(record);
            }
        }
        assert_eq!(owners, 1, "one cross-replica admission");
        let mut record = winner.unwrap();
        let quota_key = format!("budget#{network}#{}", now() / 86_400);
        let quota = store
            .client
            .get_item()
            .table_name(&table)
            .key("id", A::S(quota_key.clone()))
            .consistent_read(true)
            .send()
            .await
            .unwrap();
        assert_eq!(
            quota.item.unwrap()["spent"].as_n().unwrap(),
            "10",
            "retry never charges budget again"
        );
        let mut second = intent.clone();
        second.transaction_id.push('2');
        second.fingerprint = "second".into();
        assert!(store
            .reserve(
                &format!("{network}#second"),
                &network,
                &second,
                "second",
                10
            )
            .await
            .is_err());
        second.transaction_id = intent.transaction_id.clone();
        assert!(store
            .reserve(&key, &network, &second, "conflict", 10)
            .await
            .is_err());
        record.signed = Some("immutable-persisted-signed-bytes".into());
        record.state = State::Prepared;
        record.lease_until = now() - 1;
        store.save(&key, &record).await.unwrap();
        // A fresh client represents a new process after the old one died
        // between persistence and submission.
        let restarted = DynamoStore::new(table.clone()).await;
        let mut resumed = restarted
            .reserve(&key, &network, &intent, "restarted", 10)
            .await
            .unwrap();
        assert_eq!(resumed.owner, "restarted");
        assert_eq!(resumed.signed, record.signed);
        assert_eq!(resumed.intent.transaction_id, record.intent.transaction_id);
        assert!(
            store.save(&key, &record).await.is_err(),
            "old lease cannot overwrite new owner"
        );
        resumed.state = State::Confirmed;
        resumed.consensus_status = Some("Success".into());
        restarted.save(&key, &resumed).await.unwrap();
        let replay = store
            .reserve(&key, &network, &intent, "late", 10)
            .await
            .unwrap();
        assert!(replay.terminal());
        assert_eq!(replay.owner, "restarted");
        assert!(
            store.save(&key, &record).await.is_err(),
            "terminal record cannot be replaced"
        );
        for id in [key, quota_key] {
            store
                .client
                .delete_item()
                .table_name(&table)
                .key("id", A::S(id))
                .send()
                .await
                .unwrap();
        }
    }
}
