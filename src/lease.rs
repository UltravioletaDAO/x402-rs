//! One record, one owner: the lease contract shared by every elected role.
//!
//! # Why this is a module and not a second copy
//!
//! [`crate::writer_lease`] worked out, in September 2026, what a lease has to
//! do to be worth having: a grant that expires on its own, dated from when the
//! request was SENT; a generation that fences an A → B → A handover; an error
//! that neither extends nor revokes; two clocks, because a suspended host stops
//! the monotonic one and not the wall one. Every one of those exists because
//! its absence produced a real defect — the `Err` arm that set `IS_WRITER =
//! true` handed one signer to three processes at once.
//!
//! Discovery needs the same contract for a different reason (one aggregator,
//! not three), and a second implementation of "expiry and generation" would
//! drift from this one the first time somebody tuned a constant. So the
//! decision-making lives here, once, and each role supplies its own key, its
//! own timings and its own consequences.
//!
//! # What a role still owns
//!
//! Everything that is not the decision: what a grant entitles it to do, what to
//! log, what extra attributes ride on the record ([`crate::writer_lease`] puts
//! the holder's routable address there), and what to do while it does not hold
//! one. This module never decides that a lost election is bad news or that a
//! grant means "may write" — only whether a grant exists.
//!
//! # The posture, restated
//!
//! * A successful acquire earns a grant of [`Timings::grant_len`], which ends
//!   `handover_margin` BEFORE the record it wrote can be taken by anybody else.
//! * A lost election revokes at once: somebody else is demonstrably entitled.
//! * A control-plane error changes NOTHING. The grant keeps running down. A
//!   blip is absorbed; a sustained outage ends the grant with nobody deciding.
//!
//! That last line is the whole of A2, and it is what makes a role built on this
//! module fail closed rather than open.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use aws_sdk_dynamodb::types::{AttributeValue, ReturnValuesOnConditionCheckFailure};

/// Partition key attribute of a lease record.
pub const PK_ATTR: &str = "pk";
/// Attribute holding the current owner's opaque id.
pub const OWNER_ATTR: &str = "owner";
/// Attribute holding the record's own expiry, in Unix seconds.
pub const EXPIRES_AT_ATTR: &str = "expires_at";
/// Attribute holding the tenancy counter.
pub const GENERATION_ATTR: &str = "generation";

/// This process's own start, for the monotonic clock.
fn process_start() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// Monotonic microseconds since [`process_start`].
pub fn now_micros() -> u64 {
    process_start().elapsed().as_micros() as u64
}

/// Wall-clock Unix seconds, or `0` if the clock is unreadable.
///
/// `0` reads as "before every deadline", so an unreadable clock revokes a grant
/// rather than extending it.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The four durations that make a lease a system rather than four knobs.
///
/// Their relationships are what matter, and each role asserts its own in a
/// test: a grant shorter than the record by exactly the handover margin, and
/// enough renewals inside a grant that a blip cannot cost the lease.
#[derive(Debug, Clone, Copy)]
pub struct Timings {
    /// How long the record survives without renewal, as written on it.
    pub ttl: Duration,
    /// How often the holder renews.
    pub renew: Duration,
    /// How much earlier than the record's expiry the holder stops believing it
    /// owns anything. Pays for clock skew between tasks, the round trip nobody
    /// measures, and the tail of work that started just before the edge.
    pub handover_margin: Duration,
    /// How soon to retry after a control-plane error. Deliberately shorter than
    /// `renew`: every failed attempt spends grant that cannot be got back, and
    /// there is nothing to back off from — the table is not overloaded, it is
    /// unreachable.
    pub error_retry: Duration,
}

impl Timings {
    /// How long a process may act on a grant, once an acquire succeeds.
    ///
    /// A function rather than a constant so the relationship to `ttl` and
    /// `handover_margin` is stated once and cannot drift.
    pub const fn grant_len(&self) -> Duration {
        Duration::from_secs(self.ttl.as_secs() - self.handover_margin.as_secs())
    }

    /// How many renewal attempts fit inside one grant. Below about five, a
    /// single slow minute on the control plane becomes a handover.
    pub const fn renewals_per_grant(&self) -> u64 {
        self.grant_len().as_secs() / self.renew.as_secs()
    }
}

/// One role's live claim, or the absence of one.
///
/// Held in a `static` by each role, which is why every field is an atomic and
/// [`Grant::standalone`] is `const`.
#[derive(Debug)]
pub struct Grant {
    /// Monotonic deadline, in microseconds since [`process_start`]. `0` means
    /// no grant. Monotonic, so a wall-clock correction cannot extend it.
    until_micros: AtomicU64,
    /// Wall-clock deadline of the same grant, in Unix seconds. `0` means none.
    ///
    /// Both are checked. `CLOCK_MONOTONIC` does not advance while a host is
    /// suspended, so a monotonic deadline alone would survive a suspend that a
    /// peer's wall clock ran straight through.
    until_unix: AtomicU64,
    /// Tenancy counter of the grant currently held.
    generation: AtomicU64,
    /// Whether this process runs with no coordinator at all — the lease
    /// switched off, or a process that never stood in the election. True until
    /// a role calls [`Grant::enter_coordination`], so a binary that never runs
    /// the loop behaves exactly as it did before the lease existed.
    standalone: AtomicBool,
}

impl Grant {
    /// A grant in standalone mode: no coordinator, so [`Grant::held`] is true.
    pub const fn standalone() -> Self {
        Self {
            until_micros: AtomicU64::new(0),
            until_unix: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            standalone: AtomicBool::new(true),
        }
    }

    /// Leave standalone mode: from here on only a grant counts.
    pub fn enter_coordination(&self) {
        self.standalone.store(false, Ordering::Release);
        self.revoke();
    }

    /// Return to standalone mode.
    ///
    /// Test-only: in production a role that must not stand simply never leaves
    /// standalone, so nothing goes back. A test that drove a grant has to put
    /// the process-global state back the way it found it.
    #[cfg(test)]
    pub fn enter_standalone(&self) {
        self.standalone.store(true, Ordering::Release);
    }

    /// Whether this process has no coordinator to ask.
    pub fn is_standalone(&self) -> bool {
        self.standalone.load(Ordering::Acquire)
    }

    /// Whether this process currently holds the role.
    ///
    /// True while a grant is demonstrably still running, or while there is no
    /// coordinator at all. Never true merely because the control plane failed
    /// to answer.
    pub fn held(&self) -> bool {
        if self.is_standalone() {
            return true;
        }
        self.remaining().is_some()
    }

    /// The tenancy this process's grant belongs to. `0` before any acquire.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Record a grant earned by an acquire that was SENT at `sent_*`.
    ///
    /// Deliberately measured from the send, not from the reply: a slow round
    /// trip then shortens our own grant instead of pushing it past the point
    /// where a successor may legitimately take over.
    pub fn record(&self, sent_micros: u64, sent_unix: u64, generation: u64, len: Duration) {
        self.until_micros
            .store(sent_micros + len.as_micros() as u64, Ordering::Release);
        self.until_unix
            .store(sent_unix + len.as_secs(), Ordering::Release);
        self.generation.store(generation, Ordering::Release);
    }

    /// Drop the grant. Called when the election is lost, and on release.
    pub fn revoke(&self) {
        self.until_micros.store(0, Ordering::Release);
        self.until_unix.store(0, Ordering::Release);
    }

    /// Remaining grant, or `None` when there is none.
    ///
    /// Both clocks have to agree. Returns the SMALLER of the two remainders, so
    /// whichever clock is less favourable to us wins.
    pub fn remaining(&self) -> Option<Duration> {
        let until_mono = self.until_micros.load(Ordering::Acquire);
        let until_wall = self.until_unix.load(Ordering::Acquire);
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

    /// Install a grant that lasts `remaining`, at tenancy `generation`. Tests
    /// only.
    #[cfg(test)]
    pub fn install_for_test(&self, remaining: Duration, generation: u64) {
        self.standalone.store(false, Ordering::Release);
        self.until_micros.store(
            now_micros() + remaining.as_micros() as u64,
            Ordering::Release,
        );
        // Rounded UP, so the wall-clock half never becomes the reason a
        // test-set grant is shorter than it asked for.
        self.until_unix
            .store(now_unix() + remaining.as_secs() + 1, Ordering::Release);
        self.generation.store(generation, Ordering::Release);
    }

    /// Set the two deadlines independently, to model a clock that misbehaves.
    /// Tests only.
    #[cfg(test)]
    pub fn set_deadlines_for_test(&self, until_micros: u64, until_unix: u64) {
        self.standalone.store(false, Ordering::Release);
        self.until_micros.store(until_micros, Ordering::Release);
        self.until_unix.store(until_unix, Ordering::Release);
    }
}

/// What one attempt at a lease produced.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// The record is ours until `sent_*` plus the TTL, so the grant runs to
    /// `sent_*` plus [`Timings::grant_len`].
    Held {
        generation: u64,
        sent_mono: u64,
        sent_unix: u64,
    },
    /// Somebody else holds a live lease. `previous` is the winning record,
    /// which DynamoDB returns on the same rejection — so a role that publishes
    /// extra attributes (an address, say) learns them without a second read.
    Lost {
        previous: Option<HashMap<String, AttributeValue>>,
    },
}

/// What [`fold`] decided, for the caller to log and act on.
#[derive(Debug)]
pub enum Transition {
    /// This process did not hold the lease and now does.
    Acquired(u64),
    /// It held the lease and still does, at the same tenancy.
    Renewed,
    /// Somebody else is demonstrably entitled.
    Lost,
    /// The control plane did not answer, which proves nothing either way.
    /// `remaining` is what is left of the grant that keeps running down.
    Unproven {
        error: String,
        remaining: Option<Duration>,
    },
}

/// Fold one attempt into a role's grant, and report the tenancy now held.
///
/// The `Err` arm is the whole of A2: it neither extends the grant nor revokes
/// it. Before 2026-09-10 the writer's version of this set `IS_WRITER = true`,
/// which meant one control-plane failure authorised every task at once for the
/// same signer.
pub fn fold(
    grant: &Grant,
    timings: &Timings,
    outcome: Result<Outcome, String>,
    previous: Option<u64>,
) -> (Option<u64>, Transition) {
    match outcome {
        Ok(Outcome::Held {
            generation,
            sent_mono,
            sent_unix,
        }) => {
            grant.record(sent_mono, sent_unix, generation, timings.grant_len());
            let transition = if previous.is_none() {
                Transition::Acquired(generation)
            } else {
                Transition::Renewed
            };
            (Some(generation), transition)
        }
        Ok(Outcome::Lost { .. }) => {
            // Somebody else is demonstrably entitled. Stop now, whatever our
            // own clock says.
            grant.revoke();
            (None, Transition::Lost)
        }
        Err(error) => {
            // A control-plane failure proves NOTHING about who holds the lease,
            // so it changes nothing. The grant keeps running down on its own; a
            // blip is absorbed, a sustained outage ends it without anyone
            // having to decide.
            let remaining = grant.remaining();
            (previous, Transition::Unproven { error, remaining })
        }
    }
}

/// One lease record in DynamoDB, and the conditional writes that move it.
///
/// Reuses whichever table the caller names. In this service that is the nonce
/// table: same key schema, same TTL attribute, and the IAM statement already
/// grants `PutItem`/`DeleteItem`, so a new role costs no terraform. Note the
/// statement does NOT grant `UpdateItem`, which is why the generation is chosen
/// by the claimant and validated in the condition rather than incremented with
/// `if_not_exists(generation) + 1`.
#[derive(Debug)]
pub struct Record {
    client: aws_sdk_dynamodb::Client,
    table_name: String,
    key: &'static str,
    owner: String,
    /// Highest tenancy this process has SEEN on the record, whether it won or
    /// lost. A fresh claim raises it by one; a claim built on a stale
    /// observation loses the conditional check and learns the real value from
    /// the rejection, so it converges in one extra round rather than needing a
    /// read of its own.
    observed_generation: AtomicU64,
}

impl Record {
    /// Build a record handle. `owner` is opaque and only ever compared and
    /// logged.
    pub fn new(client: aws_sdk_dynamodb::Client, table_name: String, key: &'static str) -> Self {
        Self {
            client,
            table_name,
            key,
            owner: uuid::Uuid::new_v4().to_string(),
            observed_generation: AtomicU64::new(0),
        }
    }

    /// This process's opaque owner id.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Raise the highest tenancy this process has seen. Never lowers it: a
    /// stale rejection must not walk the fence backwards.
    pub fn observe_generation(&self, seen: u64) {
        self.observed_generation.fetch_max(seen, Ordering::AcqRel);
    }

    /// Attempt to take or renew the lease.
    ///
    /// `renewing` carries the tenancy this process believes it holds. `Some(g)`
    /// renews conditionally on the record still being ours AT that tenancy;
    /// `None` claims afresh and raises the tenancy past the highest value this
    /// process has seen.
    ///
    /// `extra` rides on the record on every successful write. A role that
    /// publishes nothing passes an empty slice.
    ///
    /// Returns `Err` only for transport failures — a lost election is
    /// `Ok(Outcome::Lost)`, and callers must treat the two differently: a lost
    /// election proves somebody else is entitled, a transport failure proves
    /// nothing at all.
    ///
    /// The timestamps returned are taken BEFORE the request goes out, so the
    /// grant they earn is never longer than the record actually guarantees.
    pub async fn try_acquire(
        &self,
        renewing: Option<u64>,
        ttl: Duration,
        extra: &[(&'static str, AttributeValue)],
    ) -> Result<Outcome, String> {
        let sent_unix = now_unix();
        let sent_mono = now_micros();
        let expires_at = sent_unix + ttl.as_secs();

        let generation = match renewing {
            Some(current) => current,
            None => self.observed_generation.load(Ordering::Acquire) + 1,
        };

        let mut request = self
            .client
            .put_item()
            .table_name(&self.table_name)
            .item(PK_ATTR, AttributeValue::S(self.key.to_string()))
            .item(OWNER_ATTR, AttributeValue::S(self.owner.clone()))
            .item(EXPIRES_AT_ATTR, AttributeValue::N(expires_at.to_string()))
            .item(GENERATION_ATTR, AttributeValue::N(generation.to_string()))
            .expression_attribute_names("#owner", OWNER_ATTR)
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
                .expression_attribute_names("#expires_at", EXPIRES_AT_ATTR)
                .expression_attribute_values(":now", AttributeValue::N(sent_unix.to_string()))
                .condition_expression(
                    "(attribute_not_exists(pk) OR #expires_at < :now OR #owner = :me) \
                     AND (attribute_not_exists(#generation) OR #generation < :gen)",
                ),
        };

        for (name, value) in extra {
            request = request.item(*name, value.clone());
        }

        match request.send().await {
            Ok(_) => {
                self.observe_generation(generation);
                Ok(Outcome::Held {
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
                    let item = failed.item().cloned();
                    if let Some(seen) = item
                        .as_ref()
                        .and_then(|item| item.get(GENERATION_ATTR))
                        .and_then(|v| v.as_n().ok())
                        .and_then(|n| n.parse::<u64>().ok())
                    {
                        self.observe_generation(seen);
                    }
                    return Ok(Outcome::Lost { previous: item });
                }
                Err(format!("{service_err:?}"))
            }
        }
    }

    /// Delete the record, but only while it is still ours, so a successor can
    /// take the role immediately instead of waiting out the TTL.
    ///
    /// Conditional on the owner: a process whose lease already moved on must
    /// not delete the successor's record on its way out.
    pub async fn delete_if_ours(&self) -> Result<(), String> {
        self.client
            .delete_item()
            .table_name(&self.table_name)
            .key(PK_ATTR, AttributeValue::S(self.key.to_string()))
            .condition_expression("#owner = :me")
            .expression_attribute_names("#owner", OWNER_ATTR)
            .expression_attribute_values(":me", AttributeValue::S(self.owner.clone()))
            .send()
            .await
            .map(|_| ())
            .map_err(|e| format!("{e:?}"))
    }
}

/// The table every lease in this service lives in.
///
/// The nonce table, deliberately: same key schema, same TTL attribute, and an
/// IAM statement that already permits the conditional writes a lease needs.
pub fn table_name() -> String {
    std::env::var("NONCE_STORE_TABLE_NAME").unwrap_or_else(|_| "facilitator-nonces".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Timings = Timings {
        ttl: Duration::from_secs(30),
        renew: Duration::from_secs(3),
        handover_margin: Duration::from_secs(10),
        error_retry: Duration::from_millis(750),
    };

    #[test]
    fn a_grant_is_shorter_than_the_record_by_the_handover_margin() {
        assert_eq!(T.grant_len() + T.handover_margin, T.ttl);
        assert_eq!(T.renewals_per_grant(), 6);
    }

    #[test]
    fn a_fresh_grant_is_standalone_and_therefore_held() {
        let g = Grant::standalone();
        assert!(g.is_standalone());
        assert!(g.held(), "a process with no coordinator holds every role");
        assert!(g.remaining().is_none(), "...but on no grant at all");
    }

    #[test]
    fn coordination_ends_standalone_and_only_a_grant_brings_it_back() {
        let g = Grant::standalone();
        g.enter_coordination();
        assert!(!g.held(), "under a coordinator, nothing is held by default");

        g.record(now_micros(), now_unix(), 4, T.grant_len());
        assert!(g.held());
        assert_eq!(g.generation(), 4);

        g.revoke();
        assert!(!g.held());
        assert_eq!(g.generation(), 4, "a revoke is not a handover");
    }

    /// The half of the contract that decides everything else: an error neither
    /// grants nor revokes.
    #[test]
    fn a_control_plane_error_neither_grants_nor_revokes() {
        let g = Grant::standalone();
        g.enter_coordination();

        // With no grant, ten failures in a row create nothing.
        for _ in 0..10 {
            let (held, transition) = fold(&g, &T, Err("unreachable".to_string()), None);
            assert!(held.is_none());
            assert!(matches!(transition, Transition::Unproven { .. }));
            assert!(!g.held(), "an error must never create an owner");
        }

        // With a grant, five failures in a row cost nothing.
        g.install_for_test(T.grant_len(), 7);
        for _ in 0..5 {
            let (held, _) = fold(&g, &T, Err("unreachable".to_string()), Some(7));
            assert_eq!(held, Some(7), "an error does not end a tenancy");
            assert!(g.held());
        }
        assert_eq!(g.generation(), 7);
    }

    #[test]
    fn a_lost_election_revokes_at_once() {
        let g = Grant::standalone();
        g.install_for_test(T.grant_len(), 4);
        assert!(g.held());

        let (held, transition) = fold(&g, &T, Ok(Outcome::Lost { previous: None }), Some(4));
        assert_eq!(held, None);
        assert!(matches!(transition, Transition::Lost));
        assert!(
            !g.held(),
            "somebody else is demonstrably entitled; our own clock does not get a vote"
        );
    }

    #[test]
    fn a_slow_round_trip_shortens_our_own_grant() {
        let g = Grant::standalone();
        g.enter_coordination();

        let (held, transition) = fold(
            &g,
            &T,
            Ok(Outcome::Held {
                generation: 5,
                sent_mono: now_micros().saturating_sub(2_000_000),
                sent_unix: now_unix().saturating_sub(2),
            }),
            None,
        );

        assert_eq!(held, Some(5));
        assert!(matches!(transition, Transition::Acquired(5)));
        let left = g.remaining().expect("a grant was earned");
        assert!(
            left <= T.grant_len() - Duration::from_secs(1),
            "{left:?} is not shorter than a full grant; the round trip was not paid for"
        );
    }

    /// A long process pause — a frozen container, a throttled cgroup, a host
    /// suspend — must not come back believing it still owns anything. The
    /// monotonic clock stops across a suspend, so the wall clock is checked too
    /// and the less favourable of the two wins.
    #[test]
    fn a_paused_process_wakes_up_without_a_grant() {
        let g = Grant::standalone();
        // An hour of monotonic grant, against a wall clock that already ran past.
        g.set_deadlines_for_test(now_micros() + 3_600_000_000, now_unix().saturating_sub(1));
        assert!(!g.held());

        // ...and the mirror case: an NTP correction dragging the wall clock
        // forward must not extend a grant either.
        g.set_deadlines_for_test(now_micros().saturating_sub(1), now_unix() + 3600);
        assert!(!g.held());
    }

    #[test]
    fn an_expired_grant_ends_on_its_own() {
        let g = Grant::standalone();
        g.install_for_test(Duration::from_millis(40), 3);
        assert!(g.held());
        std::thread::sleep(Duration::from_millis(60));
        assert!(
            !g.held(),
            "an expired grant is not a grant, however the control plane is doing"
        );
    }
}
