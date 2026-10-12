//! Durable receipt reservations. Every row carries the table's TTL attribute,
//! set so that it outlives the authorization it protects (`expires_at`): the
//! expiry of a row must never be what lets a second payment in.
use super::{Record, Result};
use async_trait::async_trait;
use aws_sdk_dynamodb::{
    operation::{
        put_item::builders::PutItemFluentBuilder,
        transact_write_items::builders::TransactWriteItemsFluentBuilder,
    },
    types::{AttributeValue as A, Put, TransactWriteItem},
};
use std::{collections::HashMap, sync::Arc, time::Duration};

/// The table's TTL attribute: `ttl.attribute_name` of
/// `aws_dynamodb_table.idempotency_store`, shared with the Idempotency-Key cache.
pub const TTL_ATTRIBUTE: &str = "expires_at";

/// How long a receipt row outlives the authorization it protects, counted from
/// the later of its `validBefore` and the write. Until `validBefore` the row is
/// what stops the same authorization from being admitted twice; after it the
/// authorization can no longer move funds, and the row still answers the
/// purchase binding (`X-UVD-Purchase`, `Idempotency-Key`) that a client resumes
/// or retries with, and the private lookup. Ninety days, the retention of the
/// transaction index (`TRANSACTIONS_TTL_DAYS`) and of DX402 evidence; a test
/// keeps it at thirty days or more.
pub const RECEIPT_RETENTION_SECS: u64 = 90 * 24 * 60 * 60;

/// The message the CloudWatch metric filter of the receipt store alarm matches
/// (`alerts-receipts-store.tf`): a bare token, as `stuck_tx_monitor::STUCK_EVENT`.
pub const STORE_FAILURE: &str = "receipt_store_failure";

/// When DynamoDB may delete a row written at `now` for `record`.
pub fn expires_at(record: &Record, now: u64) -> u64 {
    record
        .authorization_expires_at
        .max(now)
        .saturating_add(RECEIPT_RETENTION_SECS)
}

/// A store request that did not answer as the store: an error from DynamoDB
/// other than a lost condition, or no answer within the timeout. What the
/// facilitator then answers is the caller's business; this line is the alarm's.
fn failed(op: &'static str, kind: &'static str) {
    tracing::warn!(op, kind, "{STORE_FAILURE}: receipt store request failed");
}

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
    /// Writes `record`, closing an admission this process wrote and never ran,
    /// unless the store holds it at a revision outside `held`: absent, or at
    /// one of them, it is written. The write whose outcome was lost can land
    /// after this one; it then finds the record taken and writes nothing,
    /// aliases included, so no admission is left that nobody runs.
    async fn release(&self, record: &Record, held: &[u64]) -> Result<bool> {
        match held.first() {
            Some(previous) => self.save(record, *previous).await,
            None => Ok(false),
        }
    }
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
            assert_expires_past_the_floor(row);
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

    /// Every receipt row expires, and not before thirty days from now.
    fn assert_expires_past_the_floor(row: &HashMap<String, A>) {
        let at: u64 = row
            .get(TTL_ATTRIBUTE)
            .expect("a receipt row without its TTL attribute")
            .as_n()
            .unwrap()
            .parse()
            .unwrap();
        assert!(at >= crate::receipts::now() + RETENTION_FLOOR_SECS, "{at}");
    }

    /// DynamoDB sweeps expired rows in no particular order. An alias whose
    /// record is gone reads as an expired admission once the alias itself has
    /// expired, and as corruption while it has not.
    #[tokio::test]
    #[ignore = "requires an explicitly configured local DynamoDB emulator"]
    async fn local_dynamodb_an_expired_alias_without_its_record_is_no_admission() {
        let (client, table) = local_table().await;
        let store = DynamoStore {
            client: client.clone(),
            table: table.clone(),
        };
        let now = crate::receipts::now();
        for (alias, at, expired) in [
            ("receipt:auth:v1:local-expired", now - 60, true),
            ("receipt:auth:v1:local-live", now + 3_600, false),
        ] {
            client
                .put_item()
                .table_name(&table)
                .item("idempotency_key", A::S(alias.into()))
                .item("receipt_ref", A::S(uuid::Uuid::new_v4().to_string()))
                .item(TTL_ATTRIBUTE, A::N(at.to_string()))
                .send()
                .await
                .unwrap();
            let found = store.get(alias).await;
            // A new admission takes the expired alias over; the live one stays
            // taken, and with its record gone that is no verdict.
            let record = crate::receipts::tests::fixture_record();
            let taken = store.reserve(&record, &[alias.to_owned()], &token()).await;
            if expired {
                assert!(matches!(found, Ok(None)), "{alias}");
                assert_eq!(taken, Ok(true), "{alias}");
                let now_held = store.get(alias).await.unwrap().unwrap();
                assert_eq!(now_held.receipt.receipt_id, record.receipt.receipt_id);
            } else {
                assert_eq!(
                    found.err().as_deref(),
                    Some("receipt_store_corrupt"),
                    "{alias}"
                );
                assert!(taken.is_err(), "{alias}");
            }
        }
        client
            .delete_table()
            .table_name(&table)
            .send()
            .await
            .unwrap();
    }

    /// `release` against DynamoDB: it writes where the record is absent or at a
    /// held revision and refuses any other, and a reservation landing after it
    /// finds the record taken and writes nothing, its aliases included.
    #[tokio::test]
    #[ignore = "requires an explicitly configured local DynamoDB emulator"]
    async fn local_dynamodb_a_release_takes_the_record_before_a_late_reservation() {
        let (client, table) = local_table().await;
        let store = DynamoStore {
            client: client.clone(),
            table: table.clone(),
        };
        let record = crate::receipts::tests::fixture_record();
        let key = format!("receipt:v1:{}", record.receipt.receipt_id);
        let alias = "receipt:auth:v1:local-late".to_owned();
        let mut closed = record.clone();
        closed.receipt.revision = 2;
        closed.receipt.status = "rejected".into();
        assert_eq!(store.release(&closed, &[1]).await, Ok(true));
        let _ = store
            .reserve(&record, std::slice::from_ref(&alias), &token())
            .await;
        assert!(store.get(&alias).await.unwrap().is_none());
        assert_eq!(store.get(&key).await.unwrap().unwrap().receipt.revision, 2);
        let mut later = closed.clone();
        later.receipt.revision = 3;
        assert_eq!(store.release(&later, &[1]).await, Ok(false));
        assert_eq!(store.release(&later, &[1, 2]).await, Ok(true));
        assert_eq!(store.get(&key).await.unwrap().unwrap().receipt.revision, 3);
        client
            .delete_table()
            .table_name(&table)
            .send()
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "requires an explicitly configured local DynamoDB emulator"]
    async fn local_dynamodb_atomic_admission_cas_and_ttl() {
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
            assert_expires_past_the_floor(row);
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

    fn offline() -> DynamoStore {
        let config = aws_sdk_dynamodb::config::Builder::new()
            .behavior_version_latest()
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new("offline", "offline", None, None, "test"))
            .build();
        DynamoStore {
            client: aws_sdk_dynamodb::Client::from_conf(config),
            table: "offline".into(),
        }
    }

    /// A store that sends to `endpoint`, once, with fixed credentials.
    fn sending_to(endpoint: &str) -> DynamoStore {
        let config = aws_sdk_dynamodb::config::Builder::new()
            .behavior_version_latest()
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new("offline", "offline", None, None, "test"))
            .endpoint_url(endpoint)
            .retry_config(aws_sdk_dynamodb::config::retry::RetryConfig::disabled())
            .build();
        DynamoStore {
            client: aws_sdk_dynamodb::Client::from_conf(config),
            table: "receipts".into(),
        }
    }

    /// The revision fence of `save`, against a local stand-in that answers
    /// `PutItem` as DynamoDB does. A lost condition is `Ok(false)`: the caller
    /// does not hold the revision it wrote from, so nothing it prepared is
    /// sent (`prepared_evm`) and nothing it closed is released. A stored write
    /// is `Ok(true)`, and any other error is no verdict.
    #[tokio::test]
    async fn a_save_that_lost_its_revision_is_not_saved() {
        use axum::http::{header::CONTENT_TYPE, HeaderMap, StatusCode};
        let answer = Arc::new(std::sync::Mutex::new((StatusCode::OK, String::new())));
        let served = answer.clone();
        let app = axum::Router::new().route(
            "/",
            axum::routing::post(move |headers: HeaderMap| {
                let served = served.clone();
                async move {
                    let target = headers.get("x-amz-target").cloned();
                    let (status, body) = served.lock().unwrap().clone();
                    let status = if target
                        .as_ref()
                        .is_some_and(|t| t == "DynamoDB_20120810.PutItem")
                    {
                        status
                    } else {
                        StatusCode::NOT_IMPLEMENTED
                    };
                    (status, [(CONTENT_TYPE, "application/x-amz-json-1.0")], body)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let store = sending_to(&endpoint);
        let record = crate::receipts::tests::fixture_record();
        let answers = |status: StatusCode, body: &str| {
            *answer.lock().unwrap() = (status, body.to_owned());
        };

        answers(
            StatusCode::BAD_REQUEST,
            r##"{"__type":"com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException","message":"The conditional request failed"}"##,
        );
        assert_eq!(store.save(&record, 1).await, Ok(false));
        answers(StatusCode::OK, "{}");
        assert_eq!(store.save(&record, 1).await, Ok(true));
        answers(
            StatusCode::BAD_REQUEST,
            r##"{"__type":"com.amazonaws.dynamodb.v20120810#ValidationException","message":"rejected"}"##,
        );
        assert!(store.save(&record, 1).await.is_err());
    }

    /// Closing an admission never run takes its record where the record is
    /// absent as well as at a revision the closer held: the write whose outcome
    /// was lost can still land, and has to find the record taken. The behavior
    /// against DynamoDB is `local_dynamodb_a_release_takes_the_record_before_a_late_reservation`.
    #[test]
    fn closing_an_unrun_admission_takes_the_record_even_before_it_exists() {
        let store = offline();
        let record = crate::receipts::tests::fixture_record();
        let request = store.release_request(&record, &[3, 2]).unwrap();
        assert_eq!(
            request.get_condition_expression().as_deref(),
            Some("attribute_not_exists(idempotency_key) OR revision = :held0 OR revision = :held1")
        );
        let values = request.get_expression_attribute_values().clone().unwrap();
        assert_eq!(values[":held0"], A::N("3".into()));
        assert_eq!(values[":held1"], A::N("2".into()));
        let item = request.get_item().clone().unwrap();
        assert_eq!(item["revision"], A::N(record.receipt.revision.to_string()));
        assert!(item.contains_key(TTL_ATTRIBUTE));
    }

    /// Every alias write, on reservation and readmission, takes the alias when
    /// it is absent or expired by the write's own time, never only when absent:
    /// an expired alias that DynamoDB has not swept would otherwise refuse each
    /// new admission (`receipt_store_unavailable`) until it does. The behavior
    /// against DynamoDB is `local_dynamodb_an_expired_alias_without_its_record_is_no_admission`.
    #[test]
    fn an_expired_alias_never_blocks_a_new_admission() {
        let store = offline();
        let record = crate::receipts::tests::fixture_record();
        let aliases = ["receipt:auth:v1:offline".to_owned()];
        let now = super::super::now();
        let reserve = store.reserve_request(&record, &aliases, "first").unwrap();
        let readmit = store
            .readmit_request(&record, 1, &aliases, "second")
            .unwrap();
        for request in [reserve, readmit] {
            let items = request.get_transact_items().clone().unwrap();
            let alias = items[1].put().unwrap();
            assert_eq!(
                alias.condition_expression(),
                Some("attribute_not_exists(idempotency_key) OR #expiry <= :now")
            );
            assert_eq!(
                alias.expression_attribute_names().unwrap()["#expiry"],
                TTL_ATTRIBUTE
            );
            let at = alias.expression_attribute_values().unwrap()[":now"]
                .as_n()
                .unwrap()
                .parse::<u64>()
                .unwrap();
            assert!(at >= now && at <= now + 5, "{at}");
        }
    }

    /// Every row an admission writes -- the record and each alias, on
    /// reservation, readmission and every revision -- carries the table's TTL
    /// attribute, set the retention past the later of the authorization's
    /// `validBefore` and the write. The retention never goes under thirty
    /// days: the rows keep answering the purchase a client resumes or retries
    /// well after the authorization itself is dead.
    #[test]
    fn every_row_written_outlives_its_authorization() {
        const FLOOR: u64 = RETENTION_FLOOR_SECS;
        assert!(RECEIPT_RETENTION_SECS >= FLOOR, "retention under 30 days");
        let store = offline();
        let aliases = [
            "receipt:auth:v1:offline".to_owned(),
            "receipt:purchase:v1:offline".to_owned(),
        ];
        let mut evm = crate::receipts::tests::fixture_record();
        // An authorization still valid for an hour, whatever the date of the run.
        evm.authorization_expires_at = crate::receipts::now() + 3_600;
        // Hedera keeps no validBefore here: its transactions live 180 s at most.
        let mut native = evm.clone();
        native.authorization_expires_at = 0;
        for record in [evm, native] {
            let before = crate::receipts::now();
            let reserve = store.reserve_request(&record, &aliases, "first").unwrap();
            let readmit = store
                .readmit_request(&record, 1, &aliases, "second")
                .unwrap();
            let expiry = |row: &HashMap<String, A>| -> u64 {
                row.get(TTL_ATTRIBUTE)
                    .unwrap_or_else(|| panic!("a row without {TTL_ATTRIBUTE}: {row:?}"))
                    .as_n()
                    .unwrap()
                    .parse()
                    .unwrap()
            };
            let mut rows: Vec<HashMap<String, A>> = Vec::new();
            for tx in [reserve, readmit] {
                let written: Vec<HashMap<String, A>> = tx
                    .get_transact_items()
                    .clone()
                    .unwrap_or_default()
                    .iter()
                    .map(|item| item.put().expect("admissions only put").item().clone())
                    .collect();
                // One write, one expiry: its record never expires before its aliases.
                let first = expiry(&written[0]);
                assert!(
                    written.iter().all(|row| expiry(row) == first),
                    "{written:?}"
                );
                rows.extend(written);
            }
            rows.push(DynamoStore::item(&record, crate::receipts::now()).unwrap());
            let after = crate::receipts::now();
            assert_eq!(rows.len(), 2 * (1 + aliases.len()) + 1);
            for row in rows {
                let at = expiry(&row);
                let from = record.authorization_expires_at.max(before);
                assert!(at >= from + FLOOR, "{at} expires under the floor");
                assert!(
                    at >= from + RECEIPT_RETENTION_SECS
                        && at
                            <= record.authorization_expires_at.max(after) + RECEIPT_RETENTION_SECS,
                    "{at}"
                );
            }
        }
    }

    /// The names the infrastructure must agree on: the table's TTL attribute
    /// and the message the receipt store alarm counts.
    #[test]
    fn the_terraform_reads_the_attribute_and_the_message_written_here() {
        fn block(file: &str, header: &str) -> String {
            let start = file.find(header).unwrap_or_else(|| panic!("{header}"));
            let rest = &file[start + header.len()..];
            let end = rest.find("\nresource ").unwrap_or(rest.len());
            rest[..end].split_whitespace().collect()
        }
        let main = include_str!("../../terraform/environments/production/main.tf");
        let table = block(
            main,
            "resource \"aws_dynamodb_table\" \"idempotency_store\"",
        );
        assert!(
            table.contains(&format!(
                "ttl{{attribute_name=\"{TTL_ATTRIBUTE}\"enabled=true}}"
            )),
            "aws_dynamodb_table.idempotency_store must expire rows on {TTL_ATTRIBUTE}"
        );
        let alerts =
            include_str!("../../terraform/environments/production/alerts-receipts-store.tf");
        let filter = block(
            alerts,
            "resource \"aws_cloudwatch_log_metric_filter\" \"receipt_store_failure\"",
        );
        assert!(
            filter.contains(&format!("pattern=\"{STORE_FAILURE}\"")),
            "the receipt store alarm must count {STORE_FAILURE}"
        );
    }

    /// The guide states the retention this code keeps, in the words a reader
    /// of either finds: the days after `validBefore`, and the floor.
    #[test]
    fn the_guide_states_the_retention_the_code_keeps() {
        let guide = include_str!("../../docs/facilitator-receipts.md");
        let flat: String = guide.split_whitespace().collect::<Vec<_>>().join(" ");
        let days = RECEIPT_RETENTION_SECS / 86_400;
        let floor = RETENTION_FLOOR_SECS / 86_400;
        for said in [
            format!("{days} days after the later of the authorization's `validBefore`"),
            format!("a test keeps it at {floor} days or more"),
        ] {
            assert!(
                flat.contains(&said),
                "docs/facilitator-receipts.md must say: {said}"
            );
        }
        // And every period the guide counts in days is one of those two.
        let words: Vec<&str> = flat.split(' ').collect();
        for pair in words.windows(2) {
            if pair[1].trim_end_matches(|c: char| !c.is_alphanumeric()) == "days" {
                if let Ok(n) = pair[0].parse::<u64>() {
                    assert!(
                        n == days || n == floor,
                        "docs/facilitator-receipts.md says {n} days; the code keeps {days}, floor {floor}"
                    );
                }
            }
        }
    }

    /// Reading through an alias: its record when there is one; no admission
    /// when the record is gone and the alias expired; a corrupt store when the
    /// record is gone and the alias has not expired (or carries no expiry).
    #[test]
    fn an_alias_whose_record_is_gone_is_no_admission_only_once_it_expired() {
        let now = 1_900_000_000;
        let alias = |expiry: Option<u64>| {
            let mut row = HashMap::from([
                (
                    "idempotency_key".to_owned(),
                    A::S("receipt:auth:v1:x".into()),
                ),
                ("receipt_ref".to_owned(), A::S("id".into())),
            ]);
            if let Some(at) = expiry {
                row.insert(TTL_ATTRIBUTE.to_owned(), A::N(at.to_string()));
            }
            row
        };
        let record = HashMap::from([("data".to_owned(), A::S("{}".into()))]);
        for expiry in [Some(now - 1), Some(now + 1), None] {
            assert_eq!(
                through_alias(&alias(expiry), Some(record.clone()), now),
                Ok(Some(record.clone())),
                "{expiry:?}"
            );
        }
        assert_eq!(through_alias(&alias(Some(now - 1)), None, now), Ok(None));
        for expiry in [Some(now + 1), None] {
            assert_eq!(
                through_alias(&alias(expiry), None, now),
                Err("receipt_store_corrupt".to_owned()),
                "{expiry:?}"
            );
        }
    }

    /// An alias reads as expired only once its own expiry has passed: never
    /// without a readable expiry (a row written before rows expired), and
    /// never ahead of time.
    #[test]
    fn an_alias_is_expired_only_once_its_expiry_has_passed() {
        let alias = |expiry: Option<A>| {
            let mut row = HashMap::from([
                (
                    "idempotency_key".to_owned(),
                    A::S("receipt:auth:v1:x".into()),
                ),
                ("receipt_ref".to_owned(), A::S("id".into())),
            ]);
            if let Some(expiry) = expiry {
                row.insert(TTL_ATTRIBUTE.to_owned(), expiry);
            }
            row
        };
        let now = 1_900_000_000;
        assert!(expired_alias(
            &alias(Some(A::N((now - 1).to_string()))),
            now
        ));
        assert!(expired_alias(&alias(Some(A::N(now.to_string()))), now));
        assert!(!expired_alias(
            &alias(Some(A::N((now + 1).to_string()))),
            now
        ));
        assert!(!expired_alias(&alias(None), now));
        assert!(!expired_alias(&alias(Some(A::N("soon".into()))), now));
        assert!(!expired_alias(
            &alias(Some(A::S((now - 1).to_string()))),
            now
        ));
    }

    #[derive(Clone, Default)]
    struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A store that answers nothing: every request the rail makes fails, and
    /// each failure writes the line the receipt store alarm counts, naming its
    /// operation. A closed local port, no retries: nothing leaves the machine.
    #[tokio::test]
    async fn every_failed_store_request_writes_the_line_the_alarm_counts() {
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        // As in rate_policy's capture: a second dispatcher keeps another test
        // thread from caching `never` for these callsites.
        let _second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
        tracing::callsite::rebuild_interest_cache();

        let config = aws_sdk_dynamodb::config::Builder::new()
            .behavior_version_latest()
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new("offline", "offline", None, None, "test"))
            .endpoint_url("http://127.0.0.1:9")
            .retry_config(aws_sdk_dynamodb::config::retry::RetryConfig::disabled())
            .build();
        let store = DynamoStore {
            client: aws_sdk_dynamodb::Client::from_conf(config),
            table: "unreachable".into(),
        };
        let record = crate::receipts::tests::fixture_record();
        let aliases = ["receipt:auth:v1:unreachable".to_owned()];
        assert!(store.get(&aliases[0]).await.is_err());
        assert!(store.reserve(&record, &aliases, "first").await.is_err());
        assert!(store.save(&record, 1).await.is_err());
        assert!(store.release(&record, &[1]).await.is_err());
        assert!(store.readmit(&record, 1, &aliases, "second").await.is_err());

        let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = log.lines().filter(|l| l.contains(STORE_FAILURE)).collect();
        for op in ["get", "reserve", "save", "release", "readmit"] {
            assert!(
                lines.iter().any(|l| l.contains(&format!("op=\"{op}\""))),
                "no {STORE_FAILURE} line for {op}:\n{log}"
            );
        }
    }
}

/// The least retention the rows may keep: thirty days past `validBefore`.
#[cfg(test)]
const RETENTION_FLOOR_SECS: u64 = 30 * 24 * 60 * 60;

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

    /// The record row of a write at `now`. One `now` per write, shared with
    /// its aliases, so the record never expires before an alias it writes.
    fn item(record: &Record, now: u64) -> Result<HashMap<String, A>> {
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
            (
                TTL_ATTRIBUTE.into(),
                A::N(expires_at(record, now).to_string()),
            ),
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
        .map_err(|_| {
            failed("get", "timeout");
            "receipt_store_timeout"
        })?
        .map(|r| r.item)
        .map_err(|_| {
            failed("get", "error");
            "receipt_store_unavailable".into()
        })
    }

    /// An alias is taken unless it exists and has not expired: DynamoDB can
    /// keep an expired row for days, and one whose record is gone is no
    /// admission (`through_alias`), so it must not refuse the next one either.
    /// An alias without an expiry (written before rows expired) stays taken.
    fn alias_put(&self, key: &str, record: &Record, now: u64) -> Result<TransactWriteItem> {
        let put = Put::builder()
            .table_name(&self.table)
            .set_item(Some(HashMap::from([
                ("idempotency_key".into(), A::S(key.to_owned())),
                (
                    "receipt_ref".into(),
                    A::S(record.receipt.receipt_id.clone()),
                ),
                (
                    TTL_ATTRIBUTE.into(),
                    A::N(expires_at(record, now).to_string()),
                ),
            ])))
            .condition_expression("attribute_not_exists(idempotency_key) OR #expiry <= :now")
            .expression_attribute_names("#expiry", TTL_ATTRIBUTE)
            .expression_attribute_values(":now", A::N(now.to_string()))
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
        let now = super::now();
        let row = Put::builder()
            .table_name(&self.table)
            .set_item(Some(Self::item(record, now)?))
            .condition_expression("attribute_not_exists(idempotency_key)")
            .build()
            .map_err(|_| "receipt_store_invalid_write")?;
        let mut tx = self
            .client
            .transact_write_items()
            .client_request_token(token)
            .transact_items(TransactWriteItem::builder().put(row).build());
        for key in aliases {
            tx = tx.transact_items(self.alias_put(key, record, now)?);
        }
        Ok(tx)
    }

    /// The close of an admission never run (`Store::release`): the record row,
    /// written where it is absent or at one of `held`.
    fn release_request(&self, record: &Record, held: &[u64]) -> Result<PutItemFluentBuilder> {
        let mut condition = String::from("attribute_not_exists(idempotency_key)");
        let mut write = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(Self::item(record, super::now())?));
        for (n, revision) in held.iter().enumerate() {
            condition.push_str(&format!(" OR revision = :held{n}"));
            write =
                write.expression_attribute_values(format!(":held{n}"), A::N(revision.to_string()));
        }
        Ok(write.condition_expression(condition))
    }

    fn readmit_request(
        &self,
        record: &Record,
        previous_revision: u64,
        aliases: &[String],
        token: &str,
    ) -> Result<TransactWriteItemsFluentBuilder> {
        let now = super::now();
        let row = Put::builder()
            .table_name(&self.table)
            .set_item(Some(Self::item(record, now)?))
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
            tx = tx.transact_items(self.alias_put(key, record, now)?);
        }
        Ok(tx)
    }
}

/// Whether an alias row has expired by `now`. The record row is rewritten at
/// every revision with a later expiry than any alias it wrote, and DynamoDB
/// sweeps expired rows in no particular order: an expired alias whose record
/// is gone is an expired admission. An alias without a readable expiry (one
/// written before rows expired) is never taken for one.
fn expired_alias(item: &HashMap<String, A>, now: u64) -> bool {
    item.get(TTL_ATTRIBUTE)
        .and_then(|v| v.as_n().ok())
        .and_then(|n| n.parse::<u64>().ok())
        .is_some_and(|at| at <= now)
}

/// What reading an alias yields, given the record row it points at: that
/// record; no admission when the record is gone and the alias has expired;
/// a corrupt store when the record is gone and the alias has not.
fn through_alias(
    alias: &HashMap<String, A>,
    record: Option<HashMap<String, A>>,
    now: u64,
) -> Result<Option<HashMap<String, A>>> {
    match record {
        Some(record) => Ok(Some(record)),
        None if expired_alias(alias, now) => Ok(None),
        None => {
            failed("get", "corrupt");
            Err("receipt_store_corrupt".into())
        }
    }
}

#[async_trait]
impl Store for DynamoStore {
    async fn get(&self, key: &str) -> Result<Option<Record>> {
        let Some(mut item) = self.read_item(key).await? else {
            return Ok(None);
        };
        if let Some(id) = item.get("receipt_ref").and_then(|v| v.as_s().ok()) {
            let record = self.read_item(&format!("receipt:v1:{id}")).await?;
            match through_alias(&item, record, super::now())? {
                Some(record) => item = record,
                None => return Ok(None),
            }
        }
        let record = item
            .get("data")
            .and_then(|v| v.as_s().ok())
            .and_then(|data| serde_json::from_str::<Record>(data).ok());
        if record.is_none() {
            failed("get", "corrupt");
            return Err("receipt_store_corrupt".into());
        }
        Ok(record)
    }

    async fn reserve(&self, record: &Record, aliases: &[String], token: &str) -> Result<bool> {
        let tx = self.reserve_request(record, aliases, token)?;
        match tokio::time::timeout(Duration::from_secs(5), tx.send()).await {
            Ok(Ok(_)) => Ok(true),
            // A timeout may have committed. Never claim ownership after an
            // ambiguous transaction; a later request can read the reservation.
            outcome => {
                match outcome {
                    // An alias already taken: contention, the store answered.
                    Ok(Err(e))
                        if e.as_service_error()
                            .is_some_and(|e| e.is_transaction_canceled_exception()) => {}
                    Ok(Err(_)) => failed("reserve", "error"),
                    Err(_) => failed("reserve", "timeout"),
                    Ok(Ok(_)) => {}
                }
                for key in aliases {
                    if self.get(key).await?.is_some() {
                        return Ok(false);
                    }
                }
                // Nothing reserved and nothing to read: no verdict.
                failed("reserve", "unresolved");
                Err("receipt_store_unavailable".into())
            }
        }
    }

    async fn save(&self, record: &Record, previous_revision: u64) -> Result<bool> {
        let write = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(Self::item(record, super::now())?))
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
            Ok(Err(_)) => {
                failed("save", "error");
                Err("receipt_store_unavailable".into())
            }
            Err(_) => {
                failed("save", "timeout");
                Err("receipt_store_unavailable".into())
            }
        }
    }

    async fn release(&self, record: &Record, held: &[u64]) -> Result<bool> {
        let write = self.release_request(record, held)?;
        match tokio::time::timeout(Duration::from_secs(5), write.send()).await {
            Ok(Ok(_)) => Ok(true),
            Ok(Err(e))
                if e.as_service_error()
                    .is_some_and(|e| e.is_conditional_check_failed_exception()) =>
            {
                Ok(false)
            }
            Ok(Err(_)) => {
                failed("release", "error");
                Err("receipt_store_unavailable".into())
            }
            Err(_) => {
                failed("release", "timeout");
                Err("receipt_store_unavailable".into())
            }
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
            Ok(Err(_)) => {
                failed("readmit", "error");
                Err("receipt_store_unavailable".into())
            }
            Err(_) => {
                failed("readmit", "timeout");
                Err("receipt_store_unavailable".into())
            }
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
            .map_err(|_| {
                failed("scan", "timeout");
                "receipt_store_timeout"
            })?
            .map_err(|_| {
                failed("scan", "error");
                "receipt_store_unavailable"
            })?;
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
        // Every row of a reservation is conditioned on its absence, the
        // record's included, as in `DynamoStore::reserve_request`.
        let key = format!("receipt:v1:{}", record.receipt.receipt_id);
        if rows.contains_key(&key) || aliases.iter().any(|a| rows.contains_key(a)) {
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
    async fn release(&self, record: &Record, held: &[u64]) -> Result<bool> {
        let mut rows = self.0.lock().await;
        let key = format!("receipt:v1:{}", record.receipt.receipt_id);
        if rows
            .get(&key)
            .is_some_and(|r| !held.contains(&r.receipt.revision))
        {
            return Ok(false);
        }
        rows.insert(key, record.clone());
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
