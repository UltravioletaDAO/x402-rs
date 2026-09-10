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
//! Until 2026-09-10 this was fail-OPEN: a DynamoDB error made **every** task
//! set `IS_WRITER = true`, so one control-plane failure handed the same signer
//! to three processes at once, each with its own private nonce cache. That is
//! the exact condition the lease exists to prevent, reached by the lease's own
//! error path.
//!
//! It is now a **grant with a deadline**, and the deadline is what decides.
//!
//! * A successful acquire earns a grant that expires
//!   [`HANDOVER_MARGIN`] BEFORE the record it wrote can be taken by anyone
//!   else, measured from the instant the request was SENT rather than the
//!   instant the answer came back. Every source of error — a slow round trip,
//!   clock skew between tasks — shortens our own grant instead of extending it.
//! * A control-plane error changes nothing. The grant keeps running down and
//!   the loop retries sooner. A blip is absorbed (renewal every
//!   [`RENEW_INTERVAL`] against a [`grant_len`] grant, so several consecutive
//!   failures cost nothing); a sustained outage ends the grant, on its own,
//!   without anybody having to decide.
//! * When the grant ends this process stops signing. It does NOT stop serving:
//!   non-holders forward to the holder, which is what keeps a deploy from
//!   costing availability. Only if there is no reachable holder either does a
//!   caller get 503.
//!
//! The consequence to state plainly: if DynamoDB is unreachable from every
//! task for longer than a grant, EVM writes become temporarily unavailable
//! instead of being served by three racing signers. That is the trade the
//! audit asked for. `ENABLE_WRITER_LEASE=false` is the break-glass that
//! restores the old fail-open behaviour, and it is the documented remedy for a
//! control-plane outage.
//!
//! # Generation, and why a token alone is not enough
//!
//! Each takeover raises a `generation` on the record, and a renewal is
//! conditional on the generation still being ours. That makes a handover
//! observable rather than inferred, and it fences an A → B → A sequence that
//! timing alone would miss.
//!
//! But a fencing token in DynamoDB does not fence a signature on chain: no
//! contract checks it. So the signing path takes a [`SigningPermit`], which is
//! issued only while the grant has at least [`SIGNING_HEADROOM`] left — enough
//! for the broadcast to finish inside [`HANDOVER_MARGIN`] even if the RPC takes
//! its whole timeout — and [`WriterLease::release`] waits for outstanding
//! permits to drain before handing the record over. That is what "reconcile
//! in-flight broadcasts before the handover" means in code.
//!
//! Note that the permit covers the nonce allocation and the broadcast, NOT the
//! receipt wait. The receipt wait is up to 900s on Ethereum and allocates
//! nothing; holding a permit across it would make every grant unsatisfiable.
//!
//! Forwarding degrades as it always did. If this task cannot discover its own
//! address, or does not know the holder's, or the forward itself fails, the
//! caller gets 503 + `Retry-After`.
//!
//! Set `ENABLE_WRITER_LEASE=false` to disable the mechanism entirely, or
//! `ENABLE_WRITER_FORWARD=false` to keep the lease but go back to refusing.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

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

/// Attribute on the lease record holding the tenancy counter.
const GENERATION_ATTR: &str = "generation";

/// How long a lease survives without renewal, as written on the record.
///
/// Raised from 15s on 2026-09-10. The number is not a comfort setting: it has
/// to be large enough that [`HANDOVER_MARGIN`] can cover a whole RPC timeout
/// AND leave a grant long enough to absorb several failed renewals. See
/// [`the_timings_leave_room_for_a_broadcast_to_finish`].
///
/// The cost is stated plainly: after a task is killed WITHOUT running
/// [`WriterLease::release`], the lane waits this long before a successor can
/// take it. A normal deploy does not pay that — ECS sends SIGTERM, the shutdown
/// path releases, and the record is gone within a second.
const LEASE_TTL: Duration = Duration::from_secs(30);

/// How often the holder renews.
///
/// Shortened from 5s so that a grant of [`grant_len`] survives several
/// consecutive control-plane failures rather than one. Costs ~20 conditional
/// writes per minute per task on a PAY_PER_REQUEST table.
const RENEW_INTERVAL: Duration = Duration::from_secs(3);

/// How much earlier than the record's own expiry this process stops believing
/// it is the writer.
///
/// This is the whole safety margin, and it pays for three things at once:
/// clock skew between tasks (the successor's takeover condition compares its
/// wall clock against the `expires_at` we wrote with ours), the round trip we
/// do not measure, and the tail of a broadcast that began with only
/// [`SIGNING_HEADROOM`] left.
const HANDOVER_MARGIN: Duration = Duration::from_secs(10);

/// How much grant a signature needs in front of it before it may start.
///
/// A broadcast that starts with this much left finishes, at worst, one RPC
/// timeout later — which [`HANDOVER_MARGIN`] is sized to cover. Below this the
/// permit is refused and the caller gets "temporarily unavailable" instead of a
/// signature nobody can prove we were entitled to make.
const SIGNING_HEADROOM: Duration = Duration::from_secs(3);

/// How soon to retry after a control-plane error.
///
/// Faster than [`RENEW_INTERVAL`], because every failed attempt spends grant we
/// cannot get back. Bounded and fixed: there is nothing to back off from — the
/// table is not overloaded, it is unreachable.
const ERROR_RETRY_INTERVAL: Duration = Duration::from_millis(750);

/// How long [`WriterLease::release`] waits for in-flight signatures to finish
/// before handing the record to a successor.
///
/// Generous relative to the critical section it drains (a nonce reservation and
/// one `eth_sendRawTransaction`), and well inside the ECS stop timeout.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(15);

/// How long this process may sign for, once an acquire succeeds.
///
/// A function rather than a `const` so the relationship to [`LEASE_TTL`] and
/// [`HANDOVER_MARGIN`] is stated once and cannot drift.
const fn grant_len() -> Duration {
    Duration::from_secs(LEASE_TTL.as_secs() - HANDOVER_MARGIN.as_secs())
}

/// Whether this process runs with no coordinator at all.
///
/// True when the lease is switched off, and true before [`spawn`] has decided
/// anything — so a process that never manages to run the lease loop behaves
/// exactly as it did before the lease existed. [`spawn`] clears it the moment
/// this process decides to stand in the election, and from then on the grant
/// decides.
static STANDALONE: AtomicBool = AtomicBool::new(true);

/// Monotonic deadline of the current grant, in microseconds since
/// [`process_start`]. `0` means no grant.
///
/// Monotonic, so a wall-clock correction cannot extend it.
static GRANT_UNTIL_MICROS: AtomicU64 = AtomicU64::new(0);

/// Wall-clock deadline of the same grant, in Unix seconds. `0` means no grant.
///
/// Both are checked. `CLOCK_MONOTONIC` does not advance while a host is
/// suspended, so a monotonic deadline alone would survive a suspend that a
/// peer's wall clock ran straight through — which is precisely the "long
/// process pause" the audit asked about.
static GRANT_UNTIL_UNIX: AtomicU64 = AtomicU64::new(0);

/// Tenancy counter of the grant this process currently holds.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Signatures currently inside the exclusive section.
static IN_FLIGHT_SIGNINGS: AtomicUsize = AtomicUsize::new(0);

/// This process's own start, for the monotonic clock.
fn process_start() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// Monotonic microseconds since [`process_start`].
fn now_micros() -> u64 {
    process_start().elapsed().as_micros() as u64
}

/// Wall-clock Unix seconds, or `0` if the clock is unreadable.
///
/// `0` reads as "before every deadline", so an unreadable clock revokes the
/// grant rather than extending it.
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Record a grant earned by an acquire that was SENT at `sent_at`.
///
/// Deliberately measured from the send, not from the reply: a slow round trip
/// then shortens our own grant instead of pushing it past the point where a
/// successor may legitimately take over.
fn grant_from(sent_at_micros: u64, sent_at_unix: u64, generation: u64) {
    GRANT_UNTIL_MICROS.store(
        sent_at_micros + grant_len().as_micros() as u64,
        Ordering::Release,
    );
    GRANT_UNTIL_UNIX.store(sent_at_unix + grant_len().as_secs(), Ordering::Release);
    GENERATION.store(generation, Ordering::Release);
}

/// Drop the grant. Called when the election is lost, on release, and by tests.
fn revoke_grant() {
    GRANT_UNTIL_MICROS.store(0, Ordering::Release);
    GRANT_UNTIL_UNIX.store(0, Ordering::Release);
}

/// Remaining grant, or `None` when there is none.
///
/// Both clocks have to agree. Returns the SMALLER of the two remainders, so
/// whichever clock is less favourable to us wins.
fn grant_remaining() -> Option<Duration> {
    let until_mono = GRANT_UNTIL_MICROS.load(Ordering::Acquire);
    let until_wall = GRANT_UNTIL_UNIX.load(Ordering::Acquire);
    if until_mono == 0 || until_wall == 0 {
        return None;
    }
    let now_mono = now_micros();
    let now_wall = now_unix();
    if now_mono >= until_mono || now_wall >= until_wall {
        return None;
    }
    let mono_left = Duration::from_micros(until_mono - now_mono);
    let wall_left = Duration::from_secs(until_wall - now_wall);
    Some(mono_left.min(wall_left))
}

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
///
/// True while a grant is demonstrably still running, or while there is no
/// coordinator at all (lease switched off, or this process abstained from the
/// election). Never true merely because the control plane failed to answer:
/// that is the defect this replaced.
pub fn is_writer() -> bool {
    if STANDALONE.load(Ordering::Acquire) {
        return true;
    }
    grant_remaining().is_some()
}

/// The tenancy this process's grant belongs to. `0` before any acquire.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

/// Signatures currently inside the exclusive section.
pub fn in_flight_signings() -> usize {
    IN_FLIGHT_SIGNINGS.load(Ordering::Acquire)
}

/// Permission to allocate a nonce and broadcast, held for the duration of that
/// critical section and no longer.
///
/// Existing because `is_writer()` at the HTTP boundary is not the same question
/// as "may I sign, now, and finish before anybody else may". Seconds of
/// validation, an `eth_call` and a gas estimate sit between the two, and a
/// grant can end inside that gap.
///
/// Dropping the permit is what tells [`WriterLease::release`] the handover may
/// proceed, so it must not be held across the receipt wait — that is up to 900s
/// on Ethereum and allocates nothing.
#[derive(Debug)]
pub struct SigningPermit {
    generation: u64,
    standalone: bool,
}

impl SigningPermit {
    /// Whether the grant this permit was issued under is still the current one.
    ///
    /// Checks the tenancy as well as the clock: an A → B → A handover restores
    /// a valid grant, but not the one this permit was issued under, and a
    /// broadcast begun in the first tenancy must not be treated as authorised
    /// by the third.
    pub fn still_valid(&self) -> bool {
        if self.standalone {
            return STANDALONE.load(Ordering::Acquire);
        }
        generation() == self.generation && grant_remaining().is_some()
    }
}

impl Drop for SigningPermit {
    fn drop(&mut self) {
        IN_FLIGHT_SIGNINGS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Ask for permission to sign, or `None` when this process cannot prove it is
/// entitled to.
///
/// Refuses when the grant has less than [`SIGNING_HEADROOM`] left, rather than
/// at the moment it expires: a broadcast that starts with headroom finishes
/// inside [`HANDOVER_MARGIN`] even if the RPC takes its whole timeout, which is
/// what keeps the signature inside the tenancy that authorised it.
pub fn signing_permit() -> Option<SigningPermit> {
    if STANDALONE.load(Ordering::Acquire) {
        IN_FLIGHT_SIGNINGS.fetch_add(1, Ordering::AcqRel);
        return Some(SigningPermit {
            generation: generation(),
            standalone: true,
        });
    }
    let remaining = grant_remaining()?;
    if remaining < SIGNING_HEADROOM {
        return None;
    }
    // Registered BEFORE the generation is read, so a permit can never be
    // invisible to a concurrent drain.
    IN_FLIGHT_SIGNINGS.fetch_add(1, Ordering::AcqRel);
    let permit = SigningPermit {
        generation: generation(),
        standalone: false,
    };
    // Re-check after registering: if the grant went away in between, the permit
    // is dropped here and its registration undone with it.
    grant_remaining()?;
    Some(permit)
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

/// Force the writer decision. Tests only.
///
/// `true` puts the process in standalone mode (no coordinator, always the
/// writer), which is what a process with the lease switched off does. `false`
/// puts it under a coordinator with no grant, which is what a task that lost
/// the election looks like.
///
/// This is process-global, so a test that flips it must flip it back. CI runs
/// with `--test-threads=1`, which makes that safe; without it, a parallel test
/// reading `is_writer()` could observe the flip.
#[cfg(test)]
pub fn set_writer_for_test(value: bool) {
    if value {
        STANDALONE.store(true, Ordering::Release);
    } else {
        STANDALONE.store(false, Ordering::Release);
        revoke_grant();
    }
}

/// Install a grant that lasts `remaining`, at tenancy `generation`. Tests only.
#[cfg(test)]
pub fn grant_for_test(remaining: Duration, generation: u64) {
    STANDALONE.store(false, Ordering::Release);
    GRANT_UNTIL_MICROS.store(
        now_micros() + remaining.as_micros() as u64,
        Ordering::Release,
    );
    // Rounded UP, so the wall-clock half never becomes the reason a test-set
    // grant is shorter than it asked for.
    GRANT_UNTIL_UNIX.store(now_unix() + remaining.as_secs() + 1, Ordering::Release);
    GENERATION.store(generation, Ordering::Release);
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
    /// Highest tenancy counter this process has SEEN on the record, whether it
    /// won or lost. A fresh claim raises it by one; a claim built on a stale
    /// observation loses the conditional check and learns the real value from
    /// the rejection, so it converges in one extra round rather than needing a
    /// read of its own.
    observed_generation: AtomicU64,
}

/// What one attempt at the lease produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AcquireOutcome {
    /// The record is ours until `sent_*` plus [`LEASE_TTL`], so the grant runs
    /// to `sent_*` plus [`grant_len`].
    Held {
        generation: u64,
        sent_mono: u64,
        sent_unix: u64,
    },
    /// Somebody else holds a live lease.
    Lost,
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
            observed_generation: AtomicU64::new(0),
        }
    }

    /// Attempt to take or renew the lease.
    ///
    /// `renewing` carries the tenancy this process believes it holds. `Some(g)`
    /// renews conditionally on the record still being ours AT that tenancy;
    /// `None` claims afresh and raises the tenancy past the highest value this
    /// process has seen.
    ///
    /// Returns `Err` only for transport failures — a lost election is
    /// `Ok(AcquireOutcome::Lost)`, and the caller must treat the two
    /// differently: a lost election proves somebody else is entitled, a
    /// transport failure proves nothing at all.
    ///
    /// A lost election also refreshes [`HOLDER_ENDPOINT`] and the observed
    /// tenancy. Both come back in the SAME response thanks to
    /// `ReturnValuesOnConditionCheckFailure::AllOld`, so learning where to
    /// forward costs no extra request and cannot itself fail separately.
    ///
    /// The timestamps returned are taken BEFORE the request goes out. The grant
    /// they earn is therefore never longer than the record actually guarantees,
    /// whatever the round trip costs.
    async fn try_acquire(&self, renewing: Option<u64>) -> Result<AcquireOutcome, String> {
        let sent_unix = now_unix();
        let sent_mono = now_micros();
        let expires_at = sent_unix + LEASE_TTL.as_secs();

        let generation = match renewing {
            Some(current) => current,
            None => self.observed_generation.load(Ordering::Acquire) + 1,
        };

        let mut request = self
            .client
            .put_item()
            .table_name(&self.table_name)
            .item("pk", AttributeValue::S(LEASE_KEY.to_string()))
            .item("owner", AttributeValue::S(self.owner.clone()))
            .item("expires_at", AttributeValue::N(expires_at.to_string()))
            .item(GENERATION_ATTR, AttributeValue::N(generation.to_string()))
            .expression_attribute_names("#owner", "owner")
            .expression_attribute_names("#generation", GENERATION_ATTR)
            .expression_attribute_values(":me", AttributeValue::S(self.owner.clone()))
            .expression_attribute_values(":gen", AttributeValue::N(generation.to_string()))
            .return_values_on_condition_check_failure(ReturnValuesOnConditionCheckFailure::AllOld);

        // DynamoDB rejects an expression attribute name or value that no
        // condition uses, so each branch declares exactly what it references.
        request = match renewing {
            // Renewal. Conditional on the tenancy as well as the owner, so an
            // A -> B -> A handover cannot be mistaken for an unbroken tenancy.
            Some(_) => request.condition_expression("#owner = :me AND #generation = :gen"),
            // Fresh claim. Allowed when nobody holds it, when the holder's
            // record has expired, or when it was already ours -- and only ever
            // at a HIGHER tenancy, which is what makes the counter a fence
            // rather than a label.
            None => request
                .expression_attribute_names("#expires_at", "expires_at")
                .expression_attribute_values(":now", AttributeValue::N(sent_unix.to_string()))
                .condition_expression(
                    "(attribute_not_exists(pk) OR #expires_at < :now OR #owner = :me) \
                     AND (attribute_not_exists(#generation) OR #generation < :gen)",
                ),
        };

        // Only advertise an address we actually resolved. Writing an empty or
        // guessed one would send peers into a black hole.
        if let Some(endpoint) = &self.endpoint {
            request = request.item(ENDPOINT_ATTR, AttributeValue::S(endpoint.clone()));
        }

        match request.send().await {
            Ok(_) => {
                self.observe_generation(generation);
                Ok(AcquireOutcome::Held {
                    generation,
                    sent_mono,
                    sent_unix,
                })
            }
            Err(e) => {
                // A failed condition means somebody else holds a live lease.
                // That is a normal outcome, not an error.
                let service_err = e.into_service_error();
                if let aws_sdk_dynamodb::operation::put_item::PutItemError::
                    ConditionalCheckFailedException(failed) = &service_err
                {
                    // The winner's record rides along on the rejection.
                    let item = failed.item();
                    let holder = item
                        .and_then(|item| item.get(ENDPOINT_ATTR))
                        .and_then(|v| v.as_s().ok())
                        .filter(|e| !e.is_empty())
                        .map(|e| Arc::from(e.as_str()));
                    set_holder_endpoint(holder);
                    if let Some(seen) = item
                        .and_then(|item| item.get(GENERATION_ATTR))
                        .and_then(|v| v.as_n().ok())
                        .and_then(|n| n.parse::<u64>().ok())
                    {
                        self.observe_generation(seen);
                    }
                    return Ok(AcquireOutcome::Lost);
                }
                Err(format!("{service_err:?}"))
            }
        }
    }

    /// Raise the highest tenancy this process has seen. Never lowers it: a
    /// stale rejection must not walk the fence backwards.
    fn observe_generation(&self, seen: u64) {
        self.observed_generation.fetch_max(seen, Ordering::AcqRel);
    }

    /// Give the lease up so a successor can take it immediately instead of
    /// waiting out the TTL. Best-effort.
    ///
    /// Order is the point. New signatures stop FIRST, outstanding ones are
    /// waited for SECOND, and only then is the record deleted — so the
    /// successor's first nonce allocation happens after our last broadcast has
    /// left, not alongside it. Deleting first and draining afterwards would
    /// make the handover exactly as racy as the TTL expiry this exists to
    /// avoid.
    pub async fn release(&self) {
        revoke_grant();

        let outstanding = in_flight_signings();
        if outstanding > 0 {
            info!(
                outstanding,
                "Draining in-flight EVM signatures before handing the writer lease over"
            );
        }
        if !drain_signings(DRAIN_TIMEOUT).await {
            // Hand over anyway: holding the record past the drain timeout only
            // moves the problem to the TTL, where the successor takes it with
            // no drain at all. Say so, loudly, because a broadcast may still be
            // in flight from this process.
            error!(
                outstanding = in_flight_signings(),
                timeout_secs = DRAIN_TIMEOUT.as_secs(),
                "EVM signatures did not drain before the writer lease handover; \
                 releasing anyway. A broadcast from this task may still be in flight"
            );
        }

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
        set_holder_endpoint(None);
    }
}

/// Wait until no signature is inside the exclusive section, or the timeout
/// runs out. `true` if it drained.
async fn drain_signings(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if in_flight_signings() == 0 {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Elect, then keep the grant alive in the background.
///
/// Returns the lease handle so the shutdown path can release it. When the
/// feature is disabled, or this process has no address peers could forward to,
/// the process stays in standalone mode and writes as it did before the lease
/// existed.
///
/// The FIRST attempt is awaited here, before the HTTP server binds. Otherwise
/// the ALB can route a settle to a task that has not yet decided anything, and
/// "has not decided" is exactly the state that must never be read as "may
/// sign".
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

    // From here on this process is under a coordinator, and only a grant makes
    // it a writer. Nothing before this point may be read as one.
    STANDALONE.store(false, Ordering::Release);
    revoke_grant();

    let first = apply_outcome(&lease.owner, lease.try_acquire(None).await, None);
    if first.is_none() && !is_writer() {
        warn!(
            "This task starts WITHOUT the EVM writer lease. EVM writes will be forwarded to the \
             holder, or answered 503 while no holder is known. If DynamoDB is unreachable from \
             every task, set ENABLE_WRITER_LEASE=false to restore the pre-lease behaviour."
        );
    }

    let loop_lease = Arc::clone(&lease);
    tokio::spawn(async move {
        let mut held = first;
        loop {
            let outcome = loop_lease.try_acquire(held).await;
            let failed = outcome.is_err();
            held = apply_outcome(&loop_lease.owner, outcome, held);
            // A failed attempt spends grant this process cannot get back, so it
            // comes back sooner. Bounded and fixed: the table is not
            // overloaded, it is unreachable, and there is nothing to back off
            // from.
            let wait = if failed {
                ERROR_RETRY_INTERVAL
            } else {
                RENEW_INTERVAL
            };
            tokio::time::sleep(wait).await;
        }
    });

    Some(lease)
}

/// Fold one attempt into the process's grant, and report the tenancy now held.
///
/// The whole A2 fix is the `Err` arm: it neither extends the grant nor revokes
/// it. Before 2026-09-10 it set `IS_WRITER = true`, which meant one
/// control-plane failure authorised every task at once for the same signer.
/// `owner` is only ever logged, which is what lets the tests drive this
/// function -- the one that actually decides -- instead of a stand-in.
fn apply_outcome(
    owner: &str,
    outcome: Result<AcquireOutcome, String>,
    previous: Option<u64>,
) -> Option<u64> {
    match outcome {
        Ok(AcquireOutcome::Held {
            generation,
            sent_mono,
            sent_unix,
        }) => {
            if previous.is_none() {
                info!(
                    owner,
                    generation,
                    grant_secs = grant_len().as_secs(),
                    "Acquired EVM writer lease"
                );
            }
            grant_from(sent_mono, sent_unix, generation);
            // We are the destination now; a stale peer address must not survive
            // to send our own traffic somewhere else.
            set_holder_endpoint(None);
            Some(generation)
        }
        Ok(AcquireOutcome::Lost) => {
            if previous.is_some() {
                warn!(owner, "Lost EVM writer lease");
            }
            // Somebody else is demonstrably entitled. Stop now, whatever our
            // own clock says.
            revoke_grant();
            None
        }
        Err(e) => {
            // A control-plane failure proves NOTHING about who holds the lease,
            // so it changes nothing. The grant keeps running down on its own; a
            // blip is absorbed, a sustained outage ends it without anyone
            // having to decide. The last known holder endpoint is deliberately
            // kept: while we cannot renew, forwarding to the task that probably
            // still holds it is the only thing that keeps writes flowing.
            match grant_remaining() {
                Some(left) => warn!(
                    owner,
                    error = %e,
                    grant_left_ms = left.as_millis() as u64,
                    "Writer lease check failed; the existing grant is unchanged and still running"
                ),
                None => error!(
                    owner,
                    error = %e,
                    "Writer lease check failed and this task holds no grant, so it will NOT sign. \
                     EVM writes are forwarded to the holder if one is known, otherwise answered \
                     503. Set ENABLE_WRITER_LEASE=false to restore the pre-lease behaviour if \
                     the control plane stays unreachable"
                ),
            }
            previous
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The timings are a system, not five independent knobs. Every relation
    /// below is load-bearing, so they are asserted rather than described.
    #[test]
    fn the_timings_leave_room_for_a_broadcast_to_finish() {
        // A grant is shorter than the record it was written from, by exactly
        // the handover margin. This is what makes "we stopped" happen strictly
        // before "somebody else may start".
        assert_eq!(grant_len() + HANDOVER_MARGIN, LEASE_TTL);

        // A blip must not cost the lease. Several consecutive failed renewals
        // have to fit inside a grant, or a single slow minute on the control
        // plane becomes a handover.
        let renewals_per_grant = grant_len().as_secs() / RENEW_INTERVAL.as_secs();
        assert!(
            renewals_per_grant >= 5,
            "only {renewals_per_grant} renewal attempts fit in a grant"
        );

        // A broadcast that starts with the headroom left finishes, at worst,
        // one RPC timeout later. That tail has to fit inside the handover
        // margin, or a signature can outlive the tenancy that authorised it.
        let rpc_timeout = Duration::from_secs(crate::chain::RPC_REQUEST_TIMEOUT_SECS);
        assert!(
            rpc_timeout <= SIGNING_HEADROOM + HANDOVER_MARGIN,
            "a broadcast started with {SIGNING_HEADROOM:?} left can outrun {HANDOVER_MARGIN:?}"
        );

        // ...and the headroom has to be small enough that a healthy holder can
        // actually sign. A headroom near the grant means permits are refused
        // most of the time.
        assert!(SIGNING_HEADROOM * 3 < grant_len());
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
        // A process that has never run the lease loop -- feature disabled, or a
        // binary that never calls `spawn` -- behaves exactly as it did before
        // the lease existed. `spawn` is what ends standalone mode, and from
        // that point on only a grant makes this process a writer.
        set_writer_for_test(true);
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

    // ===================================================================
    // A2: exclusivity under failure
    // ===================================================================
    //
    // These tests exist because of one line. Before 2026-09-10 the renewal
    // loop's `Err` arm ran `IS_WRITER.store(true)`, so a DynamoDB failure --
    // which every task sees at the same moment -- authorised every task at once
    // for the same signer. The lease's own error path produced the condition
    // the lease exists to prevent.

    /// The regression itself: a control-plane error must not create a writer.
    ///
    /// Modelled on the real loop rather than described: `apply_outcome`'s `Err`
    /// arm is exactly what the background task runs.
    #[test]
    fn a_control_plane_error_never_grants_the_writer_role() {
        set_writer_for_test(false);
        assert!(!is_writer(), "no grant to begin with");

        // Ten consecutive failures, as a regional outage would produce.
        for _ in 0..10 {
            let held = apply_outcome("task", Err("dynamodb unreachable".to_string()), None);
            assert!(held.is_none());
            assert!(
                !is_writer(),
                "a task with no grant must not become a writer because DynamoDB failed"
            );
        }
        set_writer_for_test(true);
    }

    /// ...and the other half: an error must not REVOKE a grant either. A blip
    /// that dropped the lease would hand the lane over for no reason, which is
    /// how a fail-closed change turns into an availability regression.
    #[test]
    fn a_control_plane_blip_does_not_cost_a_live_grant() {
        grant_for_test(grant_len(), 7);
        assert!(is_writer());

        // Five failures in a row is longer than any credible blip, and still
        // well inside the grant.
        for _ in 0..5 {
            let held = apply_outcome("task", Err("dynamodb unreachable".to_string()), Some(7));
            assert_eq!(held, Some(7), "an error does not end a tenancy");
            assert!(
                is_writer(),
                "the grant was still running; nothing proved otherwise"
            );
        }
        assert_eq!(generation(), 7, "a failure is not a handover");
        set_writer_for_test(true);
    }

    /// A sustained outage ends the grant on its own, with nobody deciding.
    #[test]
    fn a_sustained_outage_ends_the_grant_without_a_decision() {
        // A grant with almost nothing left, as one would be after an outage
        // longer than `grant_len()`.
        grant_for_test(Duration::from_millis(40), 3);
        assert!(is_writer());
        std::thread::sleep(Duration::from_millis(60));
        assert!(
            !is_writer(),
            "an expired grant is not a grant, however the control plane is doing"
        );
        set_writer_for_test(true);
    }

    /// A lost election is proof somebody else is entitled. Stop immediately,
    /// whatever our own clock says about the grant we were holding.
    #[test]
    fn losing_the_election_revokes_the_grant_at_once() {
        grant_for_test(grant_len(), 4);
        assert!(is_writer());
        let held = apply_outcome("task", Ok(AcquireOutcome::Lost), Some(4));
        assert_eq!(held, None);
        assert!(
            !is_writer(),
            "somebody else is demonstrably entitled; our own clock does not get a vote"
        );
        set_writer_for_test(true);
    }

    /// A successful acquire dates the grant from when the request was SENT, so
    /// a slow round trip shortens our own grant rather than pushing it past the
    /// point where a successor may take over.
    #[test]
    fn a_slow_round_trip_shortens_our_own_grant() {
        set_writer_for_test(false);
        // Two seconds of monotonic time already gone when the answer arrives.
        let sent_mono = now_micros().saturating_sub(2_000_000);
        let sent_unix = now_unix().saturating_sub(2);
        let held = apply_outcome(
            "task",
            Ok(AcquireOutcome::Held {
                generation: 5,
                sent_mono,
                sent_unix,
            }),
            None,
        );
        assert_eq!(held, Some(5));
        assert_eq!(generation(), 5);
        let left = grant_remaining().expect("a grant was earned");
        assert!(
            left <= grant_len() - Duration::from_secs(1),
            "{left:?} is not shorter than a full grant; the round trip was not paid for"
        );
        set_writer_for_test(true);
    }

    /// The signing gate is not the same question as the HTTP gate. A grant with
    /// less than the headroom left still says "I am the writer" -- and must
    /// still refuse to start a broadcast that could outlive it.
    #[test]
    fn a_grant_too_short_to_finish_a_broadcast_refuses_to_start_one() {
        grant_for_test(SIGNING_HEADROOM / 2, 9);
        assert!(
            is_writer(),
            "the grant has not expired, so forwarding here would be wrong"
        );
        assert!(
            signing_permit().is_none(),
            "but it cannot cover a broadcast, so no signature may start"
        );
        assert_eq!(
            in_flight_signings(),
            0,
            "a refused permit registers nothing"
        );

        grant_for_test(grant_len(), 9);
        let permit = signing_permit().expect("a full grant issues a permit");
        assert_eq!(in_flight_signings(), 1);
        assert!(permit.still_valid());
        drop(permit);
        assert_eq!(in_flight_signings(), 0);
        set_writer_for_test(true);
    }

    /// An A -> B -> A handover restores a valid grant, but not the tenancy a
    /// permit was issued under. Timing alone would wave that through; the
    /// generation is what catches it.
    #[test]
    fn a_permit_does_not_survive_its_tenancy() {
        grant_for_test(grant_len(), 11);
        let permit = signing_permit().expect("permit");
        assert!(permit.still_valid());

        // The lease went elsewhere and came back.
        revoke_grant();
        grant_for_test(grant_len(), 13);
        assert!(is_writer(), "the new tenancy is perfectly valid");
        assert!(
            !permit.still_valid(),
            "...but not for a broadcast authorised by the previous one"
        );
        drop(permit);
        set_writer_for_test(true);
    }

    /// A long process pause -- a frozen container, a throttled cgroup, a host
    /// suspend -- must not come back believing it is still the writer. The
    /// monotonic clock stops across a suspend, so the wall clock is checked too
    /// and the less favourable of the two wins.
    #[test]
    fn a_paused_process_wakes_up_without_a_grant() {
        // A grant whose wall-clock half is already in the past, which is what a
        // suspend that the monotonic clock slept through looks like.
        STANDALONE.store(false, Ordering::Release);
        GRANT_UNTIL_MICROS.store(now_micros() + 3_600_000_000, Ordering::Release);
        GRANT_UNTIL_UNIX.store(now_unix().saturating_sub(1), Ordering::Release);
        assert!(
            !is_writer(),
            "an hour of monotonic grant does not survive a wall clock that ran past it"
        );

        // ...and the mirror case: a wall clock dragged forward by an NTP
        // correction must not extend a grant either.
        GRANT_UNTIL_MICROS.store(now_micros().saturating_sub(1), Ordering::Release);
        GRANT_UNTIL_UNIX.store(now_unix() + 3600, Ordering::Release);
        assert!(!is_writer());
        set_writer_for_test(true);
    }

    /// Standalone mode is the pre-lease behaviour, and it has to keep working:
    /// a laptop, a single-task deployment, and `ENABLE_WRITER_LEASE=false` all
    /// land here, and none of them has a coordinator to ask.
    #[test]
    fn standalone_mode_signs_without_a_grant() {
        set_writer_for_test(true);
        assert!(is_writer());
        let permit = signing_permit().expect("standalone always signs");
        assert!(permit.still_valid());
        drop(permit);
    }

    /// A drain returns as soon as the exclusive section is empty, and reports
    /// honestly when it is not.
    #[tokio::test]
    async fn the_handover_waits_for_the_broadcasts_it_can_see() {
        set_writer_for_test(true);
        assert!(drain_signings(Duration::from_millis(50)).await);

        let permit = signing_permit().expect("permit");
        assert!(
            !drain_signings(Duration::from_millis(60)).await,
            "a handover must not report a clean drain while a broadcast is out"
        );
        drop(permit);
        assert!(drain_signings(Duration::from_millis(200)).await);
    }

    // -------------------------------------------------------------------
    // Two processes, one signer
    // -------------------------------------------------------------------

    /// The lease record, with DynamoDB's conditional-write semantics and
    /// nothing else. Enough to run the real decision function against a second
    /// process without an AWS account.
    #[derive(Debug, Default, Clone)]
    struct FakeRecord {
        owner: Option<String>,
        expires_at: u64,
        generation: u64,
    }

    /// One task's view: the state the real loop keeps, plus the arithmetic the
    /// real code uses for the grant.
    #[derive(Debug)]
    struct SimTask {
        owner: String,
        held: Option<u64>,
        observed_generation: u64,
        grant_until: Option<u64>,
    }

    impl SimTask {
        fn new(owner: &str) -> Self {
            Self {
                owner: owner.to_string(),
                held: None,
                observed_generation: 0,
                grant_until: None,
            }
        }

        /// Does this task believe it may sign at `now`?
        fn is_writer(&self, now: u64) -> bool {
            self.grant_until.is_some_and(|until| now < until)
        }

        /// One pass of the loop. `reachable` injects the control-plane fault.
        fn tick(&mut self, record: &mut FakeRecord, now: u64, reachable: bool) {
            if !reachable {
                // The `Err` arm: nothing changes. This is the whole fix.
                return;
            }

            let renewing = self.held;
            let generation = match renewing {
                Some(current) => current,
                None => self.observed_generation + 1,
            };

            let condition_holds = match renewing {
                // Renewal: still ours, at still the same tenancy.
                Some(_) => {
                    record.owner.as_deref() == Some(self.owner.as_str())
                        && record.generation == generation
                }
                // Fresh claim: free, expired or already ours, and only ever at
                // a higher tenancy.
                None => {
                    (record.owner.is_none()
                        || record.expires_at < now
                        || record.owner.as_deref() == Some(self.owner.as_str()))
                        && record.generation < generation
                }
            };

            if condition_holds {
                record.owner = Some(self.owner.clone());
                record.expires_at = now + LEASE_TTL.as_secs();
                record.generation = generation;
                self.held = Some(generation);
                self.observed_generation = self.observed_generation.max(generation);
                self.grant_until = Some(now + grant_len().as_secs());
            } else {
                self.held = None;
                self.observed_generation = self.observed_generation.max(record.generation);
                self.grant_until = None;
            }
        }
    }

    /// The invariant, over a whole simulated hour of the worst weather this
    /// system sees: a rolling deploy, a partition that hides the table from one
    /// task, a full outage, and a process frozen for a minute.
    ///
    /// At no second may two tasks both believe they may sign for the same
    /// signer.
    #[test]
    fn two_tasks_are_never_both_writers() {
        let mut record = FakeRecord::default();
        let mut a = SimTask::new("task-a");
        let mut b = SimTask::new("task-b");

        let mut violations = 0usize;
        let mut a_wrote = 0usize;
        let mut b_wrote = 0usize;
        let mut nobody = 0usize;

        for now in 0..3600u64 {
            // 0-600     : only A exists
            // 600+      : both tasks run, as production does with min_capacity 2
            // 900-1200  : the table is unreachable from B only (a partition)
            // 1200-1500 : the table is unreachable from BOTH (a regional outage)
            //             -- this is the window the old fail-open arm turned
            //             into two authorised writers
            // 1500-1560 : B is frozen; its wall clock runs, its loop does not
            // 2000-2100 : A is killed without releasing; B takes over on TTL
            let a_alive = !(2000..2100).contains(&now);
            let b_alive = now >= 600;
            let a_reachable = !(1200..1500).contains(&now);
            let b_reachable = !(900..1500).contains(&now);
            let b_running = !(1500..1560).contains(&now);

            if a_alive && now % RENEW_INTERVAL.as_secs() == 0 {
                a.tick(&mut record, now, a_reachable);
            }
            if b_alive && b_running && now % RENEW_INTERVAL.as_secs() == 1 {
                b.tick(&mut record, now, b_reachable);
            }

            let a_writes = a_alive && a.is_writer(now);
            let b_writes = b_alive && b.is_writer(now);
            if a_writes && b_writes {
                violations += 1;
            }
            if a_writes {
                a_wrote += 1;
            }
            if b_writes {
                b_wrote += 1;
            }
            if !a_writes && !b_writes {
                nobody += 1;
            }
        }

        assert_eq!(
            violations, 0,
            "two tasks authorised for the same signer at the same second"
        );
        // Availability, stated as a number rather than hoped for: the lane has
        // a writer for the overwhelming majority of the hour, and the gaps are
        // the two deliberate outages plus the TTL wait after A dies without
        // releasing.
        // Availability, stated as a number rather than hoped for.
        assert!(a_wrote > 1000, "A wrote for only {a_wrote}s of 3600");
        assert!(b_wrote > 100, "B never really took over: {b_wrote}s");
        assert!(
            a_wrote + b_wrote > 3000,
            "the lane had a writer for only {}s of 3600; fail-closed cost more \
             availability than the injected faults account for",
            a_wrote + b_wrote
        );
        // The gaps are the 300s regional outage plus the TTL wait after A is
        // killed without releasing, and nothing else.
        assert!(
            nobody < 500,
            "{nobody}s with no writer at all is more than the injected outages account for"
        );
    }

    /// A normal deploy must not cost more than a few seconds of writer, which
    /// is the condition the audit set on going fail-closed at all.
    ///
    /// The incoming task loses the election while the outgoing one still holds
    /// it, then the outgoing one RELEASES on SIGTERM and the incoming one takes
    /// over on its next tick.
    #[test]
    fn a_normal_deploy_leaves_the_lane_without_a_writer_for_seconds_not_minutes() {
        let mut record = FakeRecord::default();
        let mut old = SimTask::new("old");
        let mut new = SimTask::new("new");

        // Steady state.
        for now in (0..30).step_by(RENEW_INTERVAL.as_secs() as usize) {
            old.tick(&mut record, now, true);
        }
        assert!(old.is_writer(30));

        // The new task starts and loses, as it must.
        new.tick(&mut record, 30, true);
        assert!(!new.is_writer(30));
        assert!(old.is_writer(30), "the old task keeps writing meanwhile");

        // SIGTERM: `release` drops the grant and deletes the record.
        old.grant_until = None;
        record = FakeRecord::default();

        let mut gap = 0u64;
        let mut now = 31;
        while !new.is_writer(now) {
            new.tick(&mut record, now, true);
            if !new.is_writer(now) {
                gap += 1;
                now += 1;
            }
            assert!(gap < 60, "the handover took longer than a minute");
        }
        assert!(
            gap <= RENEW_INTERVAL.as_secs(),
            "a graceful handover cost {gap}s; it must cost at most one renewal interval"
        );
    }

    /// Two tasks racing for a free lease: exactly one wins, and the tenancy
    /// moves forward by exactly one.
    #[test]
    fn a_contested_free_lease_produces_one_winner_and_one_tenancy() {
        let mut record = FakeRecord::default();
        let mut a = SimTask::new("a");
        let mut b = SimTask::new("b");

        a.tick(&mut record, 0, true);
        b.tick(&mut record, 0, true);

        assert_ne!(a.is_writer(0), b.is_writer(0), "exactly one writer");
        assert_eq!(record.generation, 1);

        // The loser learns the real tenancy from the rejection, so its next
        // claim is built on the truth rather than on its own guess.
        let loser = if a.is_writer(0) { &b } else { &a };
        assert_eq!(loser.observed_generation, 1);
    }

    /// A stale claimant -- one that slept through a handover and still thinks
    /// the tenancy is what it was -- cannot take the lease back by guessing.
    #[test]
    fn a_stale_claim_cannot_walk_the_tenancy_backwards() {
        let mut record = FakeRecord {
            owner: Some("current".to_string()),
            expires_at: 0, // already expired: the lease IS available
            generation: 9,
        };
        let mut stale = SimTask::new("stale");
        stale.observed_generation = 2; // its last look was long ago

        stale.tick(&mut record, 100, true);
        assert!(
            !stale.is_writer(100),
            "a claim at tenancy 3 must not overwrite tenancy 9"
        );
        assert_eq!(record.generation, 9, "the fence did not move backwards");

        // ...and it converges on the next try, without an extra read.
        stale.tick(&mut record, 103, true);
        assert!(stale.is_writer(103));
        assert_eq!(record.generation, 10);
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
