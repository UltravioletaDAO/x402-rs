//! In-memory job store for asynchronous ERC-8004 `POST /register` (P1) plus the
//! in-flight de-duplication lock that stops concurrent double-mints (P3).
//!
//! # Why this exists
//!
//! `POST /register` mints an ERC-8004 identity NFT and (optionally) transfers it
//! to a recipient. The mint's on-chain confirmation has a p95 of ~28s, which
//! sits right against downstream 30s HTTP timeouts. When a caller times out and
//! retries, the first mint has usually already landed on-chain, so the retry
//! reverts on the registry's uniqueness check (`execution reverted`), burning
//! gas and confusing the caller.
//!
//! Two mitigations live here:
//!
//! 1. **Async pollable registration (P1).** With `Prefer: respond-async`, the
//!    handler returns a `jobId` in <2s and drives the mint+transfer on a
//!    background task; the caller polls `GET /register/status/{jobId}` until the
//!    `agentId` appears. The facilitator's on-chain latency leaves the caller's
//!    critical path entirely, so the 504-then-retry storm can't start.
//!
//! 2. **In-flight lock (P3).** Both the sync and async paths register an
//!    in-flight key (`network|agentUri|recipient`) before minting. A second
//!    request for the same key while the first is still confirming is *not*
//!    minted again: the async path returns the existing job, the sync path
//!    returns `409 Conflict`. This defends clients that retry without adopting
//!    the async flow.
//!
//! The store is process-local (single ECS task) and intentionally simple: an
//! in-memory map swept lazily on access. Terminal jobs (`Done`/`Failed`) live
//! for [`REGISTER_JOB_TTL_SECONDS`] so a slow poller can still read the result.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use once_cell::sync::Lazy;
use serde::Serialize;

use crate::erc8004::RegisterAgentResponse;
use crate::network::Network;
use crate::types::{MixedAddress, TransactionHash};

/// How long a terminal (`Done`/`Failed`) job is retained for polling before it
/// is swept from the store. One hour comfortably covers any downstream retry.
pub const REGISTER_JOB_TTL_SECONDS: u64 = 60 * 60;

/// Lifecycle of an async registration job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegisterJobStatus {
    /// Accepted; mint transaction not yet confirmed.
    Pending,
    /// Mint confirmed on-chain; `agentId` known. Transfer (if any) may be pending.
    MintConfirmed,
    /// Fully complete (minted, and transferred to recipient if requested).
    Done,
    /// Terminal failure. Partial fields (e.g. `agentId`) may still be populated
    /// for the "registered but transfer failed" case.
    Failed,
}

/// A single async registration job, serialized back to pollers verbatim.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterJob {
    pub job_id: String,
    pub status: RegisterJobStatus,
    pub network: Network,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction: Option<TransactionHash>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transfer_transaction: Option<TransactionHash>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<MixedAddress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// In-flight de-dup key; `None` when the request was not eligible for
    /// de-dup (no `agentUri` and no `recipient`). Not serialized.
    #[serde(skip)]
    pub key: Option<String>,
    /// Unix seconds of the last mutation; drives TTL sweeping. Not serialized.
    #[serde(skip)]
    pub updated_at: u64,
}

/// Result of attempting to start a registration.
pub enum BeginOutcome {
    /// A fresh job was created; drive the mint and finalize it.
    Started(String),
    /// A job for the same in-flight key is already running; here it is.
    AlreadyInflight(RegisterJob),
}

struct Inner {
    /// job_id -> job
    jobs: HashMap<String, RegisterJob>,
    /// in-flight key -> job_id (only present while a job is non-terminal)
    inflight: HashMap<String, String>,
}

static STORE: Lazy<Mutex<Inner>> = Lazy::new(|| {
    Mutex::new(Inner {
        jobs: HashMap::new(),
        inflight: HashMap::new(),
    })
});

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Sweep terminal jobs older than the TTL. In-flight jobs are never swept
/// (their in-flight key is only released via [`finalize_from_response`]).
fn sweep(inner: &mut Inner) {
    let t = now();
    inner.jobs.retain(|_, j| match j.status {
        RegisterJobStatus::Done | RegisterJobStatus::Failed => {
            t.saturating_sub(j.updated_at) < REGISTER_JOB_TTL_SECONDS
        }
        _ => true,
    });
}

/// Compute the in-flight de-dup key. Returns `None` when there is nothing
/// meaningful to de-dup on (anonymous mint with neither `agentUri` nor
/// `recipient`) — such requests each get their own job.
pub fn inflight_key(
    network: &Network,
    agent_uri: &str,
    recipient: &Option<MixedAddress>,
) -> Option<String> {
    if agent_uri.is_empty() && recipient.is_none() {
        return None;
    }
    let recip = match recipient {
        Some(addr) => format!("{addr}"),
        None => String::new(),
    };
    Some(format!("{network}|{agent_uri}|{recip}"))
}

/// Begin a registration. If `key` is already in flight, returns the existing
/// job; otherwise creates a fresh `Pending` job and (if keyed) marks it
/// in-flight. Atomic under the store lock.
pub fn begin(network: Network, key: Option<String>) -> BeginOutcome {
    let mut g = STORE.lock().unwrap();
    sweep(&mut g);

    if let Some(k) = &key {
        if let Some(existing_id) = g.inflight.get(k) {
            if let Some(job) = g.jobs.get(existing_id) {
                return BeginOutcome::AlreadyInflight(job.clone());
            }
        }
    }

    let id = format!("reg_{}", COUNTER.fetch_add(1, Ordering::Relaxed));
    let job = RegisterJob {
        job_id: id.clone(),
        status: RegisterJobStatus::Pending,
        network,
        agent_id: None,
        transaction: None,
        transfer_transaction: None,
        owner: None,
        error: None,
        key: key.clone(),
        updated_at: now(),
    };
    g.jobs.insert(id.clone(), job);
    if let Some(k) = key {
        g.inflight.insert(k, id.clone());
    }
    BeginOutcome::Started(id)
}

/// Record that the mint confirmed on-chain and the `agentId` is known. The
/// job stays non-terminal (transfer, if any, may still be pending).
pub fn set_mint_confirmed(
    job_id: &str,
    transaction: TransactionHash,
    agent_id: String,
    owner: MixedAddress,
) {
    let mut g = STORE.lock().unwrap();
    if let Some(j) = g.jobs.get_mut(job_id) {
        j.status = RegisterJobStatus::MintConfirmed;
        j.transaction = Some(transaction);
        j.agent_id = Some(agent_id);
        j.owner = Some(owner);
        j.updated_at = now();
    }
}

/// Finalize a job from the handler's terminal [`RegisterAgentResponse`] and
/// release its in-flight key so future registrations may proceed.
pub fn finalize_from_response(job_id: &str, resp: &RegisterAgentResponse) {
    let terminal = if resp.success && resp.error.is_none() {
        RegisterJobStatus::Done
    } else {
        RegisterJobStatus::Failed
    };

    let mut g = STORE.lock().unwrap();
    let released_key = {
        if let Some(j) = g.jobs.get_mut(job_id) {
            j.status = terminal;
            if resp.agent_id.is_some() {
                j.agent_id = resp.agent_id.clone();
            }
            if resp.transaction.is_some() {
                j.transaction = resp.transaction.clone();
            }
            if resp.transfer_transaction.is_some() {
                j.transfer_transaction = resp.transfer_transaction.clone();
            }
            if resp.owner.is_some() {
                j.owner = resp.owner.clone();
            }
            j.error = resp.error.clone();
            j.updated_at = now();
            j.key.clone()
        } else {
            None
        }
    };
    if let Some(k) = released_key {
        g.inflight.remove(&k);
    }
}

/// Fetch a job by id for the status endpoint (sweeps expired jobs first).
pub fn get(job_id: &str) -> Option<RegisterJob> {
    let mut g = STORE.lock().unwrap();
    sweep(&mut g);
    g.jobs.get(job_id).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp(success: bool, error: Option<&str>) -> RegisterAgentResponse {
        RegisterAgentResponse {
            success,
            agent_id: Some("42".to_string()),
            transaction: None,
            transfer_transaction: None,
            owner: None,
            error: error.map(|e| e.to_string()),
            network: Network::Base,
        }
    }

    #[test]
    fn keyless_requests_are_not_deduped() {
        assert!(inflight_key(&Network::Base, "", &None).is_none());
    }

    #[test]
    fn concurrent_same_key_returns_existing_job() {
        let key = inflight_key(&Network::Base, "ipfs://agent-a", &None);
        let first = match begin(Network::Base, key.clone()) {
            BeginOutcome::Started(id) => id,
            BeginOutcome::AlreadyInflight(_) => panic!("first begin should start"),
        };
        match begin(Network::Base, key.clone()) {
            BeginOutcome::AlreadyInflight(job) => assert_eq!(job.job_id, first),
            BeginOutcome::Started(_) => panic!("second begin should dedup"),
        }
        // After finalize the key is released and a new job may start.
        finalize_from_response(&first, &resp(true, None));
        assert!(matches!(
            begin(Network::Base, key),
            BeginOutcome::Started(_)
        ));
    }

    #[test]
    fn finalize_marks_done_and_failed() {
        let a = match begin(Network::Base, inflight_key(&Network::Base, "ipfs://d", &None)) {
            BeginOutcome::Started(id) => id,
            _ => unreachable!(),
        };
        finalize_from_response(&a, &resp(true, None));
        assert_eq!(get(&a).unwrap().status, RegisterJobStatus::Done);

        let b = match begin(Network::Base, inflight_key(&Network::Base, "ipfs://f", &None)) {
            BeginOutcome::Started(id) => id,
            _ => unreachable!(),
        };
        finalize_from_response(&b, &resp(false, Some("boom")));
        let job = get(&b).unwrap();
        assert_eq!(job.status, RegisterJobStatus::Failed);
        assert_eq!(job.error.as_deref(), Some("boom"));
    }
}
