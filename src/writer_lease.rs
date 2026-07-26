//! Single-writer lease for EVM transaction submission.
//!
//! # Why
//!
//! The in-process nonce allocator ([`crate::chain::evm::PendingNonceManager`])
//! is only sound while ONE process signs for a given EOA. ECS breaks that on
//! every rolling deploy: with `minimumHealthyPercent=100` /
//! `maximumPercent=200` the new task is started and made healthy *before* the
//! old one is stopped, so two tasks serve traffic simultaneously for roughly a
//! minute, each with its own private nonce cache. Autoscaling can do the same
//! (`max_capacity=3`), though in practice it has never fired — the observed
//! exposure is entirely deploy-driven.
//!
//! # How
//!
//! A conditional `PutItem` against the existing `facilitator-nonces` table
//! elects one writer. The holder renews every [`RENEW_INTERVAL`]; the lease
//! self-expires after [`LEASE_TTL`] so a task that dies without releasing does
//! not wedge the lane. Non-holders keep serving reads (`/verify`, `/supported`,
//! the landing page) and refuse only the writes that share the nonce lane.
//!
//! # Failure posture
//!
//! Fail-OPEN. If DynamoDB cannot be reached we assume the writer role and log
//! loudly, which degrades to exactly the pre-lease behaviour — concurrent
//! nonce allocation, now survivable thanks to the resync and retry logic in
//! `PendingNonceManager` — rather than refusing payments outright. A control
//! plane that is down must not stop settlement.
//!
//! Set `ENABLE_WRITER_LEASE=false` to disable the mechanism entirely.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use aws_sdk_dynamodb::types::AttributeValue;
use tracing::{error, info, warn};

/// Partition key of the lease record.
const LEASE_KEY: &str = "writer-lease#evm";

/// How long a lease survives without renewal.
const LEASE_TTL: Duration = Duration::from_secs(15);

/// How often the holder renews. Comfortably inside [`LEASE_TTL`] so a single
/// slow round-trip does not drop the lease.
const RENEW_INTERVAL: Duration = Duration::from_secs(5);

/// Whether this process currently holds the write lease.
///
/// Starts `true` so that a process which never manages to run the lease loop
/// (feature disabled, AWS unreachable at boot) behaves exactly as it did
/// before the lease existed.
static IS_WRITER: AtomicBool = AtomicBool::new(true);

/// Whether the lease mechanism is switched on. Kill-switch, default ON.
pub fn is_enabled() -> bool {
    !matches!(
        std::env::var("ENABLE_WRITER_LEASE")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "false" | "0" | "no"
    )
}

/// Whether this process may currently submit EVM transactions.
pub fn is_writer() -> bool {
    IS_WRITER.load(Ordering::Relaxed)
}

/// Lease holder identity and DynamoDB plumbing.
pub struct WriterLease {
    client: aws_sdk_dynamodb::Client,
    table_name: String,
    owner: String,
}

impl WriterLease {
    /// Build from the ambient AWS config.
    ///
    /// Reuses `NONCE_STORE_TABLE_NAME` because the lease lives in the same
    /// table as the replay-protection records: same key schema, same TTL
    /// attribute, same IAM statement (`dynamodb:PutItem` already covers a
    /// conditional put), so this needs no terraform change at all.
    pub async fn from_env() -> Self {
        let table_name = std::env::var("NONCE_STORE_TABLE_NAME")
            .unwrap_or_else(|_| "facilitator-nonces".to_string());
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = aws_sdk_dynamodb::Client::new(&config);
        let owner = uuid::Uuid::new_v4().to_string();
        Self {
            client,
            table_name,
            owner,
        }
    }

    fn now_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// Attempt to take or renew the lease.
    ///
    /// Succeeds when the record is absent, already ours, or expired. Returns
    /// `Err` only for transport failures — a lost election is `Ok(false)`.
    async fn try_acquire(&self) -> Result<bool, String> {
        let now = Self::now_secs();
        let expires_at = now + LEASE_TTL.as_secs();

        let result = self
            .client
            .put_item()
            .table_name(&self.table_name)
            .item("pk", AttributeValue::S(LEASE_KEY.to_string()))
            .item("owner", AttributeValue::S(self.owner.clone()))
            .item("expires_at", AttributeValue::N(expires_at.to_string()))
            .condition_expression("attribute_not_exists(pk) OR #owner = :me OR #expires_at < :now")
            .expression_attribute_names("#owner", "owner")
            .expression_attribute_names("#expires_at", "expires_at")
            .expression_attribute_values(":me", AttributeValue::S(self.owner.clone()))
            .expression_attribute_values(":now", AttributeValue::N(now.to_string()))
            .send()
            .await;

        match result {
            Ok(_) => Ok(true),
            Err(e) => {
                // A failed condition means somebody else holds a live lease.
                // That is a normal outcome, not an error.
                let service_err = e.into_service_error();
                if service_err.is_conditional_check_failed_exception() {
                    Ok(false)
                } else {
                    Err(format!("{service_err:?}"))
                }
            }
        }
    }

    /// Give the lease up so a successor can take it immediately instead of
    /// waiting out the TTL. Best-effort.
    pub async fn release(&self) {
        let result = self
            .client
            .delete_item()
            .table_name(&self.table_name)
            .key("pk", AttributeValue::S(LEASE_KEY.to_string()))
            .condition_expression("#owner = :me")
            .expression_attribute_names("#owner", "owner")
            .expression_attribute_values(":me", AttributeValue::S(self.owner.clone()))
            .send()
            .await;

        match result {
            Ok(_) => info!(owner = %self.owner, "Released EVM writer lease"),
            Err(e) => warn!(owner = %self.owner, error = ?e, "Could not release writer lease"),
        }
        IS_WRITER.store(false, Ordering::Relaxed);
    }
}

/// Start the background renewal loop.
///
/// Returns the lease handle so the shutdown path can release it. When the
/// feature is disabled the process simply stays a writer, as before.
pub async fn spawn() -> Option<Arc<WriterLease>> {
    if !is_enabled() {
        info!("EVM writer lease disabled; this process always writes");
        return None;
    }

    let lease = Arc::new(WriterLease::from_env().await);
    let loop_lease = Arc::clone(&lease);

    tokio::spawn(async move {
        let mut held = false;
        loop {
            match loop_lease.try_acquire().await {
                Ok(true) => {
                    if !held {
                        info!(owner = %loop_lease.owner, "Acquired EVM writer lease");
                        held = true;
                    }
                    IS_WRITER.store(true, Ordering::Relaxed);
                }
                Ok(false) => {
                    if held {
                        warn!(owner = %loop_lease.owner, "Lost EVM writer lease");
                        held = false;
                    }
                    IS_WRITER.store(false, Ordering::Relaxed);
                }
                Err(e) => {
                    // Fail open: a control-plane outage must not stop payments.
                    error!(
                        owner = %loop_lease.owner,
                        error = %e,
                        "Writer lease check failed; assuming writer role"
                    );
                    IS_WRITER.store(true, Ordering::Relaxed);
                }
            }
            tokio::time::sleep(RENEW_INTERVAL).await;
        }
    });

    Some(lease)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_renews_well_inside_its_ttl() {
        // A single slow round-trip must not be able to drop the lease.
        assert!(RENEW_INTERVAL.as_secs() * 2 < LEASE_TTL.as_secs());
    }

    #[test]
    fn kill_switch_defaults_to_enabled() {
        std::env::remove_var("ENABLE_WRITER_LEASE");
        assert!(is_enabled());
        std::env::set_var("ENABLE_WRITER_LEASE", "false");
        assert!(!is_enabled());
        std::env::set_var("ENABLE_WRITER_LEASE", "true");
        assert!(is_enabled());
        std::env::remove_var("ENABLE_WRITER_LEASE");
    }

    #[test]
    fn processes_start_as_writers() {
        // Fail-open posture: never refuse writes just because the lease loop
        // has not run yet.
        assert!(is_writer());
    }
}
