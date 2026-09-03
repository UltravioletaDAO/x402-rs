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
//! not wedge the lane.
//!
//! ## Non-holders FORWARD; they do not refuse
//!
//! Refusing was correct only while "more than one task" meant "for about a
//! minute per deploy". It stopped being correct on 2026-08-29, when
//! `min_capacity` went 1 -> 2 and the ALB request-count alarm immediately took
//! the service to 3: from then on the ALB spread writes evenly over three
//! tasks of which exactly one could serve them, so **two out of every three
//! EVM writes were rejected, permanently**. Measured over six hours before the
//! fix: 582 rejections on the settle path and 132 on the ERC-8004 write
//! routes, with zero lease handovers — the lease never moved, the other two
//! tasks simply never wrote. Callers saw it as intermittent 502/503 and
//! "facilitator lease time-out", and retried into the same one-in-three odds.
//!
//! So the lease record now also carries the holder's **routable address**, and
//! a non-holder proxies the write to it instead of answering 503. The
//! invariant the lease exists to protect is untouched — exactly one process
//! still allocates nonces for the shared EOA — while every task serves 100% of
//! the traffic the ALB hands it. Adding tasks now adds capacity instead of
//! subtracting availability.
//!
//! The address is learned for free: a lost election returns
//! `ConditionalCheckFailedException` and, with
//! `ReturnValuesOnConditionCheckFailure::AllOld`, the winning item comes back
//! in that same response. No extra read, no second table, no service
//! discovery.
//!
//! Forwarding is capped at ONE hop. A proxied request carries
//! [`FORWARDED_HEADER`]; a task that receives one while not holding the lease
//! answers 503 rather than forwarding again, so a stale endpoint can never
//! build a loop between tasks.
//!
//! ## A process only stands in the election with an address peers can reach
//!
//! Because the winner's address is where every other task sends its EVM
//! writes, standing in the election with a loopback address is not a degraded
//! advertisement, it is a black hole. Measured on 2026-09-02: the binary run
//! on a developer laptop, with the ambient AWS credentials, stood in this
//! election against the real `facilitator-nonces` table. It lost on the
//! conditional check, but had it won, production settles would have been
//! forwarded to `127.0.0.1` on a machine nothing else can route to.
//!
//! So the guard is structural rather than a kill-switch somebody has to
//! remember to set: if this process cannot determine an address, or the only
//! one it has answers to itself alone, it never issues the conditional
//! `PutItem` at all. It keeps serving every route and keeps writing its own
//! transactions, exactly as it does with the lease disabled.
//!
//! # Failure posture
//!
//! Fail-OPEN. If DynamoDB cannot be reached we assume the writer role and log
//! loudly, which degrades to exactly the pre-lease behaviour — concurrent
//! nonce allocation, now survivable thanks to the resync and retry logic in
//! `PendingNonceManager` — rather than refusing payments outright. A control
//! plane that is down must not stop settlement.
//!
//! Forwarding degrades the same way. If this task cannot discover its own
//! address, or does not know the holder's, or the forward itself fails, the
//! caller gets the pre-fix 503 + `Retry-After`. The fix can therefore never be
//! worse than what it replaces.
//!
//! Set `ENABLE_WRITER_LEASE=false` to disable the mechanism entirely, or
//! `ENABLE_WRITER_FORWARD=false` to keep the lease but go back to refusing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use aws_sdk_dynamodb::types::{AttributeValue, ReturnValuesOnConditionCheckFailure};
use tracing::{error, info, warn};

/// Partition key of the lease record.
const LEASE_KEY: &str = "writer-lease#evm";

/// Marks a request that has already been proxied to the lease holder.
///
/// One hop, never two. A task that sees this header while not holding the
/// lease refuses instead of forwarding again, so a stale endpoint cannot make
/// two tasks bounce a request between them until it times out.
pub const FORWARDED_HEADER: &str = "x-facilitator-forwarded-for-writer";

/// Attribute on the lease record holding the writer's routable address.
const ENDPOINT_ATTR: &str = "endpoint";

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

/// Address of the task that currently holds the lease, as last observed.
///
/// `None` until an election is lost with a readable endpoint on the winning
/// record, which is also the state on a single-task service where nobody ever
/// loses one.
static HOLDER_ENDPOINT: RwLock<Option<Arc<str>>> = RwLock::new(None);

/// Whether this process may currently submit EVM transactions.
pub fn is_writer() -> bool {
    IS_WRITER.load(Ordering::Relaxed)
}

/// Whether a non-holder should proxy writes to the holder. Kill-switch,
/// default ON. Turning it off restores the pre-2026-08-31 behaviour of
/// answering 503, which is strictly worse but is a known quantity.
pub fn forwarding_enabled() -> bool {
    !matches!(
        std::env::var("ENABLE_WRITER_FORWARD")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "false" | "0" | "no"
    )
}

/// Where to proxy a write, when this process is not the writer.
///
/// A poisoned lock yields `None` rather than panicking: the caller then falls
/// back to 503, which is the behaviour this whole mechanism replaces, so the
/// degraded path is one we already know is survivable.
pub fn holder_endpoint() -> Option<Arc<str>> {
    HOLDER_ENDPOINT.read().ok().and_then(|g| g.clone())
}

/// Record the holder's address, logging only real transitions.
fn set_holder_endpoint(endpoint: Option<Arc<str>>) {
    let Ok(mut guard) = HOLDER_ENDPOINT.write() else {
        return;
    };
    let changed = match (guard.as_deref(), endpoint.as_deref()) {
        (Some(a), Some(b)) => a != b,
        (None, None) => false,
        _ => true,
    };
    if changed {
        match endpoint.as_deref() {
            Some(e) => info!(endpoint = %e, "EVM writer lease holder endpoint updated"),
            None => warn!("EVM writer lease holder endpoint is unknown; writes will 503"),
        }
    }
    *guard = endpoint;
}

/// Set the holder endpoint directly. Tests only.
#[cfg(test)]
pub fn set_holder_endpoint_for_test(endpoint: Option<&str>) {
    set_holder_endpoint(endpoint.map(Arc::from));
}

/// This task's own routable address, or `None` if it cannot be determined.
///
/// Order matters. `WRITER_LEASE_ENDPOINT` is an explicit operator override and
/// wins outright. Otherwise the address comes from the ECS task metadata
/// endpoint, which under `awsvpc` reports the ENI address other tasks in the
/// VPC can actually reach — unlike the container hostname, which resolves to
/// nothing from outside the task.
///
/// Publishing a WRONG address would be worse than publishing none: peers would
/// forward into a black hole instead of falling back to 503. So every step
/// fails to `None` rather than to a guess.
async fn discover_own_endpoint() -> Option<String> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());

    if let Ok(explicit) = std::env::var("WRITER_LEASE_ENDPOINT") {
        let explicit = explicit.trim().trim_end_matches('/').to_string();
        if !explicit.is_empty() {
            return Some(explicit);
        }
    }

    let base = std::env::var("ECS_CONTAINER_METADATA_URI_V4")
        .or_else(|_| std::env::var("ECS_CONTAINER_METADATA_URI"))
        .ok()?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .ok()?;
    let meta: serde_json::Value = client.get(&base).send().await.ok()?.json().await.ok()?;

    let ip = meta
        .get("Networks")?
        .as_array()?
        .iter()
        .find_map(|n| n.get("IPv4Addresses")?.as_array()?.first()?.as_str())?;

    Some(format!("http://{ip}:{port}"))
}

/// Host of an endpoint, with scheme, credentials, port and path stripped.
///
/// Deliberately not a URL parser: the value can also come straight from an
/// operator's `WRITER_LEASE_ENDPOINT`, which is not guaranteed to be a URL at
/// all, and a parse failure must not be read as "reachable".
fn endpoint_host(endpoint: &str) -> &str {
    let rest = endpoint.trim();
    let rest = rest.split_once("://").map_or(rest, |(_, r)| r);
    let rest = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);

    if let Some(after) = rest.strip_prefix('[') {
        // `[::1]:8080`
        return after.split_once(']').map_or(after, |(host, _)| host);
    }
    if rest.matches(':').count() > 1 {
        // A bracket-less IPv6 literal. It cannot carry a port -- that is what
        // the brackets are for -- so the whole string is the host, and
        // splitting on the last colon would turn `::1` into `:`.
        return rest;
    }
    match rest.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => host,
        _ => rest,
    }
}

/// Whether a peer running in another task could open a connection to this
/// address.
///
/// Loopback and the unspecified address answer only inside the machine that
/// published them. A hostname that is not an IP literal is taken at face
/// value: an operator who points `WRITER_LEASE_ENDPOINT` at an internal DNS
/// name means it, and this process is in no position to second-guess the
/// VPC's resolver.
fn is_peer_reachable(endpoint: &str) -> bool {
    let host = endpoint_host(endpoint);
    if host.is_empty() {
        return false;
    }
    let host = host.to_ascii_lowercase();
    // RFC 6761 reserves `localhost` and everything under it for the loopback.
    if host == "localhost" || host.ends_with(".localhost") {
        return false;
    }
    match host.parse::<std::net::IpAddr>() {
        // `::ffff:127.0.0.1` is loopback wearing an IPv6 coat, and
        // `Ipv6Addr::is_loopback` answers false for it.
        Ok(std::net::IpAddr::V6(v6)) => match v6.to_ipv4_mapped() {
            Some(v4) => !(v4.is_loopback() || v4.is_unspecified()),
            None => !(v6.is_loopback() || v6.is_unspecified()),
        },
        Ok(ip) => !(ip.is_loopback() || ip.is_unspecified()),
        Err(_) => true,
    }
}

/// Why this process must not stand in the writer election, or `None` if it
/// may.
///
/// Winning publishes an address that every other task then forwards its EVM
/// writes to, so an address only this machine can reach routes production
/// settlement into a hole. Deciding it here, from the address itself, is what
/// makes the safe configuration structural instead of something an operator
/// has to remember.
fn lease_refusal(own_endpoint: Option<&str>) -> Option<String> {
    match own_endpoint {
        None => Some("this task could not determine an address other tasks can reach".to_string()),
        Some(endpoint) if !is_peer_reachable(endpoint) => Some(format!(
            "the only address this task can advertise, {endpoint}, answers on this machine alone"
        )),
        Some(_) => None,
    }
}

/// Force the writer flag. Tests only.
///
/// This is process-global, so a test that flips it must flip it back. CI runs
/// with `--test-threads=1`, which makes that safe; without it, a parallel test
/// reading `is_writer()` could observe the flip.
#[cfg(test)]
pub fn set_writer_for_test(value: bool) {
    IS_WRITER.store(value, Ordering::Relaxed);
}

/// Lease holder identity and DynamoDB plumbing.
pub struct WriterLease {
    client: aws_sdk_dynamodb::Client,
    table_name: String,
    owner: String,
    /// This task's routable address, published on the lease record so peers
    /// can forward writes here. `None` when it could not be discovered, in
    /// which case peers keep answering 503 as they did before.
    endpoint: Option<String>,
}

impl WriterLease {
    /// Build from the ambient AWS config.
    ///
    /// Reuses `NONCE_STORE_TABLE_NAME` because the lease lives in the same
    /// table as the replay-protection records: same key schema, same TTL
    /// attribute, same IAM statement (`dynamodb:PutItem` already covers a
    /// conditional put), so this needs no terraform change at all.
    /// `own_endpoint` is the address [`spawn`] already resolved and cleared
    /// for the election. It is passed in rather than discovered here so that
    /// the decision to stand at all happens before an AWS client exists: a
    /// process that must not touch the lease table must not reach it for any
    /// reason, credential resolution included.
    pub async fn from_env(own_endpoint: Option<String>) -> Self {
        let table_name = std::env::var("NONCE_STORE_TABLE_NAME")
            .unwrap_or_else(|_| "facilitator-nonces".to_string());
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = aws_sdk_dynamodb::Client::new(&config);
        let owner = uuid::Uuid::new_v4().to_string();

        let endpoint = if forwarding_enabled() {
            if let Some(e) = own_endpoint.as_deref() {
                info!(endpoint = %e, "Writer lease will advertise this address");
            }
            own_endpoint
        } else {
            info!("EVM writer forwarding disabled; non-holders will refuse writes");
            None
        };

        Self {
            client,
            table_name,
            owner,
            endpoint,
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
    ///
    /// A lost election also refreshes [`HOLDER_ENDPOINT`]. That comes back in
    /// the SAME response thanks to
    /// `ReturnValuesOnConditionCheckFailure::AllOld`, so learning where to
    /// forward costs no extra request and cannot itself fail separately.
    async fn try_acquire(&self) -> Result<bool, String> {
        let now = Self::now_secs();
        let expires_at = now + LEASE_TTL.as_secs();

        let mut request = self
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
            .return_values_on_condition_check_failure(ReturnValuesOnConditionCheckFailure::AllOld);

        // Only advertise an address we actually resolved. Writing an empty or
        // guessed one would send peers into a black hole.
        if let Some(endpoint) = &self.endpoint {
            request = request.item(ENDPOINT_ATTR, AttributeValue::S(endpoint.clone()));
        }

        match request.send().await {
            Ok(_) => Ok(true),
            Err(e) => {
                // A failed condition means somebody else holds a live lease.
                // That is a normal outcome, not an error.
                let service_err = e.into_service_error();
                if let aws_sdk_dynamodb::operation::put_item::PutItemError::
                    ConditionalCheckFailedException(failed) = &service_err
                {
                    // The winner's record rides along on the rejection.
                    let holder = failed
                        .item()
                        .and_then(|item| item.get(ENDPOINT_ATTR))
                        .and_then(|v| v.as_s().ok())
                        .filter(|e| !e.is_empty())
                        .map(|e| Arc::from(e.as_str()));
                    set_holder_endpoint(holder);
                    return Ok(false);
                }
                Err(format!("{service_err:?}"))
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
        set_holder_endpoint(None);
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

    // Resolved BEFORE any AWS client exists. Winning the election publishes
    // this address as the place every other task must forward its EVM writes
    // to, so a process that has nothing routable to publish never issues the
    // conditional PutItem at all -- not even to lose it.
    let own_endpoint = discover_own_endpoint().await;
    if let Some(reason) = lease_refusal(own_endpoint.as_deref()) {
        warn!(
            reason = %reason,
            "Not standing in the EVM writer lease election. Winning it would route every other \
             task's EVM settles to an address they cannot reach, so this process abstains by \
             construction rather than by kill-switch. It keeps serving every route and keeps \
             writing its own EVM transactions. Set WRITER_LEASE_ENDPOINT to an address peers \
             can reach in order to take part."
        );
        return None;
    }

    let lease = Arc::new(WriterLease::from_env(own_endpoint).await);
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
                    // We are the destination now; a stale peer address must not
                    // survive to send our own traffic somewhere else.
                    set_holder_endpoint(None);
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
                    // Every task takes this branch at once, so they all write
                    // concurrently — survivable thanks to the resync/retry in
                    // `PendingNonceManager`, and strictly better than all of
                    // them forwarding to an address DynamoDB can no longer
                    // confirm.
                    error!(
                        owner = %loop_lease.owner,
                        error = %e,
                        "Writer lease check failed; assuming writer role"
                    );
                    IS_WRITER.store(true, Ordering::Relaxed);
                    set_holder_endpoint(None);
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

    #[test]
    fn forwarding_kill_switch_defaults_to_enabled() {
        std::env::remove_var("ENABLE_WRITER_FORWARD");
        assert!(forwarding_enabled());
        std::env::set_var("ENABLE_WRITER_FORWARD", "false");
        assert!(!forwarding_enabled());
        std::env::set_var("ENABLE_WRITER_FORWARD", "0");
        assert!(!forwarding_enabled());
        std::env::set_var("ENABLE_WRITER_FORWARD", "true");
        assert!(forwarding_enabled());
        std::env::remove_var("ENABLE_WRITER_FORWARD");
    }

    #[test]
    fn holder_endpoint_round_trips_and_clears() {
        set_holder_endpoint(Some(Arc::from("http://10.0.1.7:8080")));
        assert_eq!(holder_endpoint().as_deref(), Some("http://10.0.1.7:8080"));

        // Becoming the writer must drop the peer address: continuing to point
        // at a former holder would send our own traffic elsewhere.
        set_holder_endpoint(None);
        assert!(holder_endpoint().is_none());
    }

    /// An address is only useful if it came from the winner's record. An empty
    /// string is not an address, and forwarding to it would turn a 503 into a
    /// connection error, which is worse.
    #[test]
    fn empty_endpoint_is_not_an_address() {
        set_holder_endpoint(Some(Arc::from("http://10.0.1.7:8080")));
        let parsed = Some(String::new())
            .filter(|e: &String| !e.is_empty())
            .map(|e| Arc::from(e.as_str()));
        set_holder_endpoint(parsed);
        assert!(holder_endpoint().is_none());
    }

    /// The regression this whole change exists for.
    ///
    /// With N tasks behind the ALB and one lease, refusing means (N-1)/N of
    /// EVM writes fail. At the capacity production actually ran on 2026-08-29
    /// (min 2, autoscaled to 3) that is two failures in three, which is what
    /// callers reported as intermittent 502/503. Forwarding makes it zero, and
    /// this test states the arithmetic so nobody re-derives "it only lasts a
    /// minute per deploy" from the old comment.
    #[test]
    fn refusing_fails_a_share_of_writes_that_grows_with_task_count() {
        fn refused_share(tasks: u32) -> f64 {
            f64::from(tasks - 1) / f64::from(tasks)
        }

        assert_eq!(refused_share(1), 0.0, "single task: the old assumption");
        assert!((refused_share(2) - 0.5).abs() < f64::EPSILON);
        assert!((refused_share(3) - 2.0 / 3.0).abs() < f64::EPSILON);

        // Forwarding is what makes the count irrelevant.
        assert!(forwarding_enabled());
    }

    /// `WRITER_LEASE_ENDPOINT` must win over metadata discovery, so an operator
    /// can always pin the address by hand.
    #[tokio::test]
    async fn explicit_endpoint_override_wins() {
        std::env::set_var("WRITER_LEASE_ENDPOINT", "http://127.0.0.1:9999/");
        // A metadata URI that would fail if it were consulted at all.
        std::env::set_var("ECS_CONTAINER_METADATA_URI_V4", "http://127.0.0.1:1/bad");

        // The trailing slash is trimmed so joining a path cannot double it.
        assert_eq!(
            discover_own_endpoint().await.as_deref(),
            Some("http://127.0.0.1:9999")
        );

        std::env::remove_var("WRITER_LEASE_ENDPOINT");
        std::env::remove_var("ECS_CONTAINER_METADATA_URI_V4");
    }

    /// No metadata endpoint and no override means no address. It must NOT
    /// invent one: peers that forward into a black hole are worse off than
    /// peers that answer 503.
    #[tokio::test]
    async fn no_metadata_means_no_advertised_address() {
        std::env::remove_var("WRITER_LEASE_ENDPOINT");
        std::env::remove_var("ECS_CONTAINER_METADATA_URI_V4");
        std::env::remove_var("ECS_CONTAINER_METADATA_URI");
        assert!(discover_own_endpoint().await.is_none());
    }

    /// The host has to survive every shape an endpoint can arrive in, because
    /// a host this function gets wrong is a reachability verdict that is
    /// wrong. `::1` is the trap: splitting on the last colon turns it into
    /// `:`, which parses as no IP at all and would be waved through.
    #[test]
    fn endpoint_host_survives_every_shape() {
        assert_eq!(endpoint_host("http://10.0.1.7:8080"), "10.0.1.7");
        assert_eq!(endpoint_host("http://10.0.1.7"), "10.0.1.7");
        assert_eq!(
            endpoint_host("https://host.internal:8443/write"),
            "host.internal"
        );
        assert_eq!(endpoint_host("10.0.1.7:8080"), "10.0.1.7");
        assert_eq!(endpoint_host("http://[::1]:8080"), "::1");
        assert_eq!(endpoint_host("http://[2600:1f18::1]:8080"), "2600:1f18::1");
        assert_eq!(endpoint_host("::1"), "::1");
        assert_eq!(endpoint_host("http://user:pw@10.0.1.7:8080"), "10.0.1.7");
    }

    /// (a) A loopback address is not an address another task can use, so this
    /// process must not stand in the election at all -- winning it would send
    /// production settles to a socket only this machine has.
    #[test]
    fn loopback_addresses_refuse_the_election() {
        for endpoint in [
            "http://127.0.0.1:8080",
            "http://127.0.0.53:8080",
            "http://localhost:8080",
            "http://LocalHost:8080",
            "http://box.localhost:8080",
            "http://[::1]:8080",
            "http://[::ffff:127.0.0.1]:8080",
            "http://0.0.0.0:8080",
            "http://[::]:8080",
            "",
        ] {
            assert!(
                lease_refusal(Some(endpoint)).is_some(),
                "{endpoint:?} must not stand in the writer election"
            );
        }
    }

    /// (b) A routable address elects exactly as it did before the guard. This
    /// is the half that fails if the guard is ever widened into "abstain
    /// always", which would silently switch the lease off in production.
    #[test]
    fn routable_addresses_still_stand_in_the_election() {
        for endpoint in [
            "http://10.0.1.7:8080",
            "http://172.31.4.9:8080",
            "https://facilitator-writer.internal:8443",
            "http://[2600:1f18::1]:8080",
        ] {
            assert!(
                lease_refusal(Some(endpoint)).is_none(),
                "{endpoint} is reachable by peers and must still elect"
            );
        }
    }

    /// The case that actually happened on 2026-09-02: a laptop with no ECS
    /// metadata endpoint stood in the election against the production table.
    /// No address means no election, without anyone having to remember a flag.
    #[tokio::test]
    async fn a_box_without_ecs_metadata_abstains_without_a_kill_switch() {
        std::env::remove_var("WRITER_LEASE_ENDPOINT");
        std::env::remove_var("ECS_CONTAINER_METADATA_URI_V4");
        std::env::remove_var("ECS_CONTAINER_METADATA_URI");

        assert!(is_enabled(), "the kill-switch is still ON by default");
        assert!(lease_refusal(discover_own_endpoint().await.as_deref()).is_some());
    }

    /// (c) `WRITER_LEASE_ENDPOINT` is a way to declare an address, not a way
    /// around the check. A hand-set loopback is still loopback -- otherwise
    /// the obvious "make it work locally" fix would reopen the hole.
    #[tokio::test]
    async fn explicit_loopback_override_cannot_skip_the_check() {
        std::env::set_var("WRITER_LEASE_ENDPOINT", "http://127.0.0.1:9999/");

        let own = discover_own_endpoint().await;
        assert_eq!(own.as_deref(), Some("http://127.0.0.1:9999"));
        assert!(
            lease_refusal(own.as_deref()).is_some(),
            "WRITER_LEASE_ENDPOINT must not be a way around the reachability check"
        );

        std::env::remove_var("WRITER_LEASE_ENDPOINT");
    }

    /// An explicit `ENABLE_WRITER_LEASE=false` keeps meaning exactly what it
    /// meant. The guard is a second, independent gate: it decides whether a
    /// process may stand, never whether the mechanism exists.
    #[test]
    fn explicit_kill_switch_is_unchanged_by_the_reachability_guard() {
        std::env::set_var("ENABLE_WRITER_LEASE", "false");
        assert!(!is_enabled());
        // ...and an address that would have been perfectly electable does not
        // switch it back on.
        assert!(lease_refusal(Some("http://10.0.1.7:8080")).is_none());

        std::env::remove_var("ENABLE_WRITER_LEASE");
        assert!(is_enabled());
    }
}
