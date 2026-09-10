//! One replica does the periodic discovery work; the rest serve the snapshot.
//!
//! # The finding
//!
//! A4 of the 2026-09-09/10 architecture audit. Aggregation, the health prober
//! and the retention GC were started by `main` on every process, with no
//! coordination of any kind. Three ECS tasks therefore fetched the same
//! external feeds, probed the same endpoints and rewrote the same 15 MB S3
//! object, each on its own hour. Measured over the 24 h to 2026-09-10 17:00
//! UTC: 76 aggregation cycles, 196 source fetches, 92 catalog GETs, 106 catalog
//! PUTs — and **8 conditional writes refused**, each one an entire cycle's work
//! discarded because two tasks published within a moment of each other. A3 made
//! that safe; it did not make it cheap.
//!
//! # The shape of the fix
//!
//! One owner per cluster, elected with the SAME lease contract as the EVM
//! writer — [`crate::lease`], generalised out of [`crate::writer_lease`] for
//! this rather than copied. The owner runs the periodic jobs. Everybody else
//! keeps serving `/discovery/*` and `/bazaar` from a snapshot it refreshes from
//! S3, so reads stay on every task and stay fresh.
//!
//! ## Why the reads still need a loop
//!
//! Before this, every replica's in-memory catalog was kept current by its own
//! aggregation. Take that away and two of three replicas would answer from
//! whatever they loaded at startup — a freshness regression dressed up as an
//! efficiency win. So a non-owner asks S3 for the object's ETag every
//! [`DEFAULT_REFRESH_SECS`] and reloads only when it changed. A HEAD, not a
//! conditional GET: the 304 path of `GetObject` is an unmodelled error in the
//! SDK, and guessing at how it surfaces is exactly the kind of thing that
//! silently stops refreshing. This is also strictly fresher than before, where
//! a replica's view was as old as its own last cycle — up to an hour.
//!
//! ## Failure posture: nobody, never two
//!
//! The lease decides, and it decides the same way it does for the writer: a
//! control-plane error neither grants ownership nor revokes it. If DynamoDB is
//! unreachable from every task for longer than a grant, **no task aggregates**
//! and a line naming [`NO_OWNER_TOKEN`] says so on each of them. That is the
//! deliberate direction: a catalog that goes stale for an hour is recoverable,
//! three tasks racing on a 15 MB read-modify-write is what A3 had to be written
//! for.
//!
//! `ENABLE_DISCOVERY_LEASE=false` restores the old behaviour — every task runs
//! every job — and is the break-glass for a prolonged control-plane outage.
//!
//! ## Who may stand
//!
//! Only a process ECS is running. Winning takes the periodic work away from
//! every other replica, so a process that is not one of them must never win —
//! and on 2026-09-02 a laptop with the production credentials did stand in the
//! writer election against the real table. The test here is cheaper than the
//! writer's, because ownership publishes no address that anybody has to reach:
//! the presence of `ECS_CONTAINER_METADATA_URI[_V4]`, which ECS injects and
//! nothing else does. A process without it abstains and keeps running its own
//! discovery loops exactly as it did before this module existed.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tracing::{debug, error, info, warn};

use crate::discovery::DiscoveryRegistry;
use crate::lease::{self, Grant, Outcome, Record, Timings, Transition};

/// Partition key of the ownership record, in the same table as the writer
/// lease and the nonce records.
const OWNER_KEY: &str = "discovery-jobs#owner";

/// The timings, and why they are not the writer's.
///
/// A gap in discovery ownership costs a delayed refresh; a gap in the writer
/// lease costs a refused payment. So this trades a slower, cheaper heartbeat
/// for a takeover that is still an order of magnitude faster than the work it
/// governs: 60 s worst case against a 3600 s aggregation interval and a 60 s
/// health tick.
///
/// * `ttl` 60 s — how long a task that dies WITHOUT releasing wedges the role.
/// * `renew` 10 s — 4 renewals fit inside a grant, so a blip costs nothing.
/// * `handover_margin` 20 s — the successor may not start until 20 s after we
///   stopped, which is what makes "never two" true across clock skew.
/// * `error_retry` 2 s — a failed attempt spends grant we cannot get back.
const TIMINGS: Timings = Timings {
    ttl: Duration::from_secs(60),
    renew: Duration::from_secs(10),
    handover_margin: Duration::from_secs(20),
    error_retry: Duration::from_secs(2),
};

/// This process's claim on the periodic work.
static OWNERSHIP: Grant = Grant::standalone();

/// Consecutive control-plane failures suffered while holding no grant.
static BLIND_ATTEMPTS: AtomicU64 = AtomicU64::new(0);

/// How many consecutive blind attempts between alarm lines. At
/// `TIMINGS.error_retry` that is about one line a minute, which is a rate an
/// alarm can be built on without drowning the log.
const ALARM_EVERY: u64 = 30;

/// The string a metric filter should match. Stable on purpose: it is the only
/// signal that discovery has stopped, and it must survive rewording of the
/// sentence around it.
pub const NO_OWNER_TOKEN: &str = "discovery_owner_unreachable";

/// How often a non-owner asks S3 whether the catalog moved.
pub const DEFAULT_REFRESH_SECS: u64 = 60;

/// Whether the mechanism is switched on. Kill-switch, default ON.
///
/// Off means every task runs every job, which is the behaviour this module
/// replaced. It is the documented remedy for a control plane that stays
/// unreachable.
pub fn is_enabled() -> bool {
    !matches!(
        std::env::var("ENABLE_DISCOVERY_LEASE")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "false" | "0" | "no"
    )
}

/// Whether this process may run the periodic discovery work right now.
///
/// Cheap enough to call on every tick of every job — two atomic loads and a
/// clock read — which is the point: a job asks at the top of each cycle rather
/// than being started or not started once at boot, so a handover takes effect
/// within one tick instead of within one restart.
pub fn owns_jobs() -> bool {
    OWNERSHIP.held()
}

/// Whether ECS is running this process.
///
/// The variable is injected by the agent and by nothing else, so its presence
/// is the question "am I one of the replicas that share this cluster's work",
/// asked without a network call that could fail and hand ownership to a
/// developer's laptop.
fn runs_in_ecs() -> bool {
    std::env::var("ECS_CONTAINER_METADATA_URI_V4").is_ok()
        || std::env::var("ECS_CONTAINER_METADATA_URI").is_ok()
}

/// The ownership record and the loop that keeps it.
pub struct DiscoveryOwner {
    record: Record,
}

impl DiscoveryOwner {
    /// Give the role up so a successor can take it on its next tick instead of
    /// waiting out the TTL.
    ///
    /// Best-effort, and unlike the writer's release there is nothing to drain:
    /// an aggregation cycle interrupted mid-flight publishes nothing, because
    /// the catalog write is conditional on the version it read.
    pub async fn release(&self) {
        OWNERSHIP.revoke();
        match self.record.delete_if_ours().await {
            Ok(()) => info!(owner = %self.record.owner(), "Released discovery job ownership"),
            Err(e) => warn!(
                owner = %self.record.owner(),
                error = %e,
                "Could not release discovery job ownership; a successor will take it on the TTL"
            ),
        }
    }
}

/// Elect an owner, then keep the grant alive in the background.
///
/// The first attempt is awaited here, before the discovery jobs start, so no
/// task ever runs a cycle in the window before anything has been decided.
pub async fn spawn() -> Option<Arc<DiscoveryOwner>> {
    if !is_enabled() {
        info!(
            "Discovery job lease disabled; this process runs aggregation, health and the \
             retention GC on its own, as every process did before the lease existed"
        );
        return None;
    }

    if !runs_in_ecs() {
        info!(
            "Not standing in the discovery job election: no ECS task metadata, so this process \
             is not one of the cluster's replicas. It keeps running its own discovery loops. \
             Winning would take the periodic work away from the tasks that are."
        );
        return None;
    }

    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let client = aws_sdk_dynamodb::Client::new(&config);
    let owner = Arc::new(DiscoveryOwner {
        record: Record::new(client, lease::table_name(), OWNER_KEY),
    });

    // From here on this process is under a coordinator, and only a grant lets
    // it do the periodic work. Nothing before this point may be read as one.
    OWNERSHIP.enter_coordination();

    let first = apply_outcome(
        owner.record.owner(),
        owner.record.try_acquire(None, TIMINGS.ttl, &[]).await,
        None,
    );
    if first.is_none() {
        info!(
            renewals_per_grant = TIMINGS.renewals_per_grant(),
            "This task starts WITHOUT discovery job ownership; another replica has it. It serves \
             the catalog from the shared snapshot and refreshes it as the owner publishes."
        );
    }

    let loop_owner = Arc::clone(&owner);
    tokio::spawn(async move {
        let mut held = first;
        loop {
            let outcome = loop_owner.record.try_acquire(held, TIMINGS.ttl, &[]).await;
            let failed = outcome.is_err();
            held = apply_outcome(loop_owner.record.owner(), outcome, held);
            let wait = if failed {
                TIMINGS.error_retry
            } else {
                TIMINGS.renew
            };
            tokio::time::sleep(wait).await;
        }
    });

    Some(owner)
}

/// Fold one attempt into this process's ownership, and report the tenancy held.
///
/// A free function taking the outcome so the decision — not a stand-in for it —
/// is what the fault-injection tests drive.
fn apply_outcome(
    owner: &str,
    outcome: Result<Outcome, String>,
    previous: Option<u64>,
) -> Option<u64> {
    let (held, transition) = lease::fold(&OWNERSHIP, &TIMINGS, outcome, previous);

    match transition {
        Transition::Acquired(generation) => {
            BLIND_ATTEMPTS.store(0, Ordering::Release);
            info!(
                owner,
                generation,
                grant_secs = TIMINGS.grant_len().as_secs(),
                "This task now owns the periodic discovery work: aggregation, health probing \
                 and the retention GC run here and nowhere else"
            );
        }
        Transition::Renewed => {
            BLIND_ATTEMPTS.store(0, Ordering::Release);
        }
        Transition::Lost => {
            BLIND_ATTEMPTS.store(0, Ordering::Release);
            if previous.is_some() {
                warn!(
                    owner,
                    "Lost discovery job ownership; this task stops aggregating and goes back to \
                     refreshing the catalog from the shared snapshot"
                );
            }
        }
        Transition::Unproven { error, remaining } => match remaining {
            Some(left) => warn!(
                owner,
                error = %error,
                grant_left_ms = left.as_millis() as u64,
                "Discovery ownership check failed; the existing grant is unchanged and still \
                 running"
            ),
            None => {
                let n = BLIND_ATTEMPTS.fetch_add(1, Ordering::AcqRel) + 1;
                if n == 1 || n % ALARM_EVERY == 0 {
                    error!(
                        owner,
                        error = %error,
                        consecutive_failures = n,
                        token = NO_OWNER_TOKEN,
                        "{NO_OWNER_TOKEN}: this task cannot reach the lease table and holds no \
                         grant, so it will NOT aggregate, probe or publish the catalog. If every \
                         task is in this state the Bazaar catalog stops being refreshed until the \
                         control plane returns. Set ENABLE_DISCOVERY_LEASE=false to restore the \
                         pre-lease behaviour if it stays unreachable"
                    );
                }
            }
        },
    }

    held
}

/// Keep a non-owner's catalog current with what the owner publishes.
///
/// Runs on every task, and does nothing on the one that owns the jobs: that
/// task's in-memory cache IS the newest copy, and re-reading its own 15 MB
/// object would be the waste this change exists to remove.
pub fn start_snapshot_refresh_task(
    registry: DiscoveryRegistry,
    interval_secs: u64,
) -> tokio::task::JoinHandle<()> {
    info!(
        interval_secs,
        "Starting the discovery snapshot refresher: non-owners follow the published catalog"
    );
    tokio::spawn(async move {
        let interval = Duration::from_secs(interval_secs.max(5));
        loop {
            tokio::time::sleep(interval).await;

            if owns_jobs() {
                continue;
            }

            match registry.refresh_from_store().await {
                Ok(Some(count)) => info!(
                    count,
                    "Reloaded the discovery catalog published by the job owner"
                ),
                Ok(None) => debug!("Discovery catalog unchanged since the last refresh"),
                Err(e) => warn!(
                    error = %e,
                    "Could not refresh the discovery catalog from the store; serving the copy \
                     this task already has"
                ),
            }

            // Both overlays, for the same reason as the catalog: the owner is
            // the only task that probes, so liveness AND observed terms are
            // things this replica can only learn by reading what it wrote.
            // Leaving either behind would have a non-owner annotate its
            // listings with a boot-time snapshot -- quarantine decisions from
            // hours ago, and every price reported unverified -- while
            // presenting them as current judgements.
            registry.health().refresh_overlay().await;
            registry.terms().refresh_overlay().await;
        }
    })
}

/// Reset the module's process-global state. Tests only.
#[cfg(test)]
fn reset_for_test() {
    OWNERSHIP.enter_coordination();
    BLIND_ATTEMPTS.store(0, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialises the tests that drive the process-global ownership grant.
    /// CI runs with `--test-threads=1`, but a plain `cargo test` does not.
    static OWNER_FLAG: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn held(generation: u64, sent_ago_secs: u64) -> Result<Outcome, String> {
        Ok(Outcome::Held {
            generation,
            sent_mono: lease::now_micros().saturating_sub(sent_ago_secs * 1_000_000),
            sent_unix: lease::now_unix().saturating_sub(sent_ago_secs),
        })
    }

    /// The timings are a system. Every relation below is load-bearing for the
    /// acceptance criterion "a handover costs less than one interval".
    #[test]
    fn the_timings_hand_over_well_inside_one_interval() {
        assert_eq!(TIMINGS.grant_len() + TIMINGS.handover_margin, TIMINGS.ttl);

        // A blip must not cost the role.
        assert!(
            TIMINGS.renewals_per_grant() >= 4,
            "only {} renewal attempts fit in a grant",
            TIMINGS.renewals_per_grant()
        );

        // The worst case is a task killed without releasing: the record has to
        // expire before anybody else may claim it. That has to be far inside
        // the aggregation interval, and no worse than about one health tick.
        const DEFAULT_AGGREGATION_INTERVAL: u64 = 3600;
        const DEFAULT_HEALTH_TICK: u64 = 60;
        let worst_case = TIMINGS.ttl.as_secs() + TIMINGS.renew.as_secs();
        assert!(
            worst_case < DEFAULT_AGGREGATION_INTERVAL,
            "a silent death costs {worst_case}s of ownership, which is not inside an interval"
        );
        assert!(worst_case <= 2 * DEFAULT_HEALTH_TICK);

        // Cheaper than the writer's heartbeat, deliberately: this one governs
        // work measured in minutes, not signatures.
        assert!(TIMINGS.renew > crate::writer_lease::renew_interval());
    }

    #[test]
    fn kill_switch_defaults_to_enabled() {
        std::env::remove_var("ENABLE_DISCOVERY_LEASE");
        assert!(is_enabled());
        for off in ["false", "0", "no", "FALSE"] {
            std::env::set_var("ENABLE_DISCOVERY_LEASE", off);
            assert!(!is_enabled(), "{off} must switch the lease off");
        }
        std::env::set_var("ENABLE_DISCOVERY_LEASE", "true");
        assert!(is_enabled());
        std::env::remove_var("ENABLE_DISCOVERY_LEASE");
    }

    /// A process with no coordinator does every job, which is what a laptop,
    /// a single-task deployment and `ENABLE_DISCOVERY_LEASE=false` all need.
    #[test]
    fn standalone_processes_run_every_job() {
        let _guard = OWNER_FLAG.lock().unwrap_or_else(|e| e.into_inner());
        OWNERSHIP.enter_standalone();
        assert!(owns_jobs());
    }

    /// Only a task ECS is running may stand. This is the 2026-09-02 case: a
    /// laptop with the production credentials, which would otherwise have won
    /// the election and stopped production from aggregating.
    #[test]
    fn a_box_without_ecs_metadata_abstains() {
        std::env::remove_var("ECS_CONTAINER_METADATA_URI_V4");
        std::env::remove_var("ECS_CONTAINER_METADATA_URI");
        assert!(!runs_in_ecs());

        std::env::set_var("ECS_CONTAINER_METADATA_URI_V4", "http://169.254.170.2/v4/x");
        assert!(runs_in_ecs());
        std::env::remove_var("ECS_CONTAINER_METADATA_URI_V4");
    }

    // ===================================================================
    // Fault injection
    // ===================================================================

    /// The fail-safe the audit asked for, stated as an assertion: when the
    /// control plane cannot be reached, a task that holds nothing does NOT
    /// start aggregating. Nobody, rather than everybody.
    #[test]
    fn a_control_plane_outage_never_creates_an_owner() {
        let _guard = OWNER_FLAG.lock().unwrap_or_else(|e| e.into_inner());
        reset_for_test();
        assert!(!owns_jobs(), "no grant to begin with");

        for _ in 0..40 {
            let held = apply_outcome("task", Err("dynamodb unreachable".to_string()), None);
            assert!(held.is_none());
            assert!(
                !owns_jobs(),
                "a task with no grant must not start aggregating because DynamoDB failed"
            );
        }
        assert!(
            BLIND_ATTEMPTS.load(Ordering::Acquire) >= ALARM_EVERY,
            "the alarm line must have been emitted at least once"
        );

        OWNERSHIP.enter_standalone();
    }

    /// ...and the other half: a blip must not hand the role over for nothing.
    #[test]
    fn a_blip_does_not_cost_a_live_grant() {
        let _guard = OWNER_FLAG.lock().unwrap_or_else(|e| e.into_inner());
        reset_for_test();
        apply_outcome("task", held(3, 0), None);
        assert!(owns_jobs());

        for _ in 0..(TIMINGS.renewals_per_grant() - 1) {
            let still = apply_outcome("task", Err("dynamodb unreachable".to_string()), Some(3));
            assert_eq!(still, Some(3), "an error is not a handover");
            assert!(owns_jobs());
        }

        OWNERSHIP.enter_standalone();
    }

    /// A lost election stops the work at once, whatever our own clock says.
    /// This is the "never two" half: the winner may already be aggregating.
    #[test]
    fn a_lost_election_stops_the_jobs_immediately() {
        let _guard = OWNER_FLAG.lock().unwrap_or_else(|e| e.into_inner());
        reset_for_test();
        apply_outcome("task", held(5, 0), None);
        assert!(owns_jobs());

        let held_now = apply_outcome("task", Ok(Outcome::Lost { previous: None }), Some(5));
        assert_eq!(held_now, None);
        assert!(!owns_jobs());

        OWNERSHIP.enter_standalone();
    }

    /// A grant expires on its own, so a frozen or partitioned owner stops
    /// aggregating without anybody telling it to — which is what lets the
    /// successor start safely after the handover margin.
    #[test]
    fn an_owner_that_stops_renewing_stops_owning() {
        let _guard = OWNER_FLAG.lock().unwrap_or_else(|e| e.into_inner());
        reset_for_test();
        OWNERSHIP.install_for_test(Duration::from_millis(40), 9);
        assert!(owns_jobs());
        std::thread::sleep(Duration::from_millis(60));
        assert!(!owns_jobs());

        OWNERSHIP.enter_standalone();
    }

    // -------------------------------------------------------------------
    // Three tasks, one aggregator
    // -------------------------------------------------------------------

    /// The record, with DynamoDB's conditional-write semantics and nothing
    /// else — enough to run three processes against each other without an AWS
    /// account.
    #[derive(Debug, Default, Clone)]
    struct FakeRecord {
        owner: Option<String>,
        expires_at: u64,
        generation: u64,
    }

    #[derive(Debug)]
    struct SimTask {
        owner: String,
        held: Option<u64>,
        observed_generation: u64,
        grant_until: Option<u64>,
        /// Seconds this task spent doing the periodic work.
        worked: u64,
    }

    impl SimTask {
        fn new(owner: &str) -> Self {
            Self {
                owner: owner.to_string(),
                held: None,
                observed_generation: 0,
                grant_until: None,
                worked: 0,
            }
        }

        fn owns(&self, now: u64) -> bool {
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
                Some(_) => {
                    record.owner.as_deref() == Some(self.owner.as_str())
                        && record.generation == generation
                }
                None => {
                    (record.owner.is_none()
                        || record.expires_at < now
                        || record.owner.as_deref() == Some(self.owner.as_str()))
                        && record.generation < generation
                }
            };

            if condition_holds {
                record.owner = Some(self.owner.clone());
                record.expires_at = now + TIMINGS.ttl.as_secs();
                record.generation = generation;
                self.held = Some(generation);
                self.observed_generation = self.observed_generation.max(generation);
                self.grant_until = Some(now + TIMINGS.grant_len().as_secs());
            } else {
                self.held = None;
                self.observed_generation = self.observed_generation.max(record.generation);
                self.grant_until = None;
            }
        }
    }

    /// The invariant, over a simulated hour of the weather this system sees
    /// with three replicas: a rolling deploy, a partition that hides the table
    /// from one task, a full outage, an owner killed without releasing, and one
    /// frozen process.
    ///
    /// At no second may two tasks both be aggregating — that is the duplicate
    /// fetch and the duplicate 15 MB PUT the finding is about — and the role
    /// must come back to somebody quickly enough that the catalog keeps up.
    #[test]
    fn three_tasks_never_aggregate_at_once_and_the_role_comes_back() {
        let mut record = FakeRecord::default();
        let mut tasks = vec![SimTask::new("a"), SimTask::new("b"), SimTask::new("c")];

        let mut violations = 0usize;
        let mut ownerless = 0usize;
        let mut longest_gap = 0u64;
        let mut gap = 0u64;
        let mut handovers = 0usize;
        let mut previous_owner: Option<String> = None;

        for now in 0..3600u64 {
            //    0- 600  only A exists
            //  600+      all three run, as production does
            //  900-1200  the table is unreachable from B only (a partition)
            // 1200-1500  unreachable from ALL of them (a regional outage)
            // 1500-1560  C is frozen: its wall clock runs, its loop does not
            // 2000-2200  A is killed WITHOUT releasing
            let alive = [!(2000..2200).contains(&now), now >= 600, now >= 600];
            let reachable = [
                !(1200..1500).contains(&now),
                !(900..1500).contains(&now),
                !(1200..1500).contains(&now),
            ];
            let running = [true, true, !(1500..1560).contains(&now)];

            for (i, task) in tasks.iter_mut().enumerate() {
                if alive[i] && running[i] && now % TIMINGS.renew.as_secs() == i as u64 {
                    task.tick(&mut record, now, reachable[i]);
                }
            }

            let working: Vec<&str> = tasks
                .iter()
                .enumerate()
                .filter(|(i, t)| alive[*i] && t.owns(now))
                .map(|(_, t)| t.owner.as_str())
                .collect();

            if working.len() > 1 {
                violations += 1;
            }
            if working.is_empty() {
                ownerless += 1;
                gap += 1;
                longest_gap = longest_gap.max(gap);
            } else {
                gap = 0;
                let owner = working[0].to_string();
                if previous_owner.as_deref() != Some(owner.as_str()) {
                    if previous_owner.is_some() {
                        handovers += 1;
                    }
                    previous_owner = Some(owner);
                }
            }
            for (i, task) in tasks.iter_mut().enumerate() {
                if alive[i] && task.owns(now) {
                    task.worked += 1;
                }
            }
        }

        assert_eq!(
            violations, 0,
            "two tasks aggregated at the same second: duplicate fetches and a duplicate 15 MB PUT"
        );
        assert!(
            handovers >= 1,
            "the role never moved; the takeover is untested"
        );

        // The gaps are the 300 s regional outage plus the TTL wait after the
        // owner is killed without releasing, and nothing else.
        assert!(
            ownerless < 700,
            "{ownerless}s with nobody doing the periodic work is more than the injected \
             outages account for"
        );
        // The acceptance criterion, as a number: no gap longer than one
        // aggregation interval.
        assert!(
            longest_gap <= 400,
            "the longest stretch with no owner was {longest_gap}s"
        );
        let total: u64 = tasks.iter().map(|t| t.worked).sum();
        assert!(
            total > 2800,
            "the periodic work had an owner for only {total}s of 3600"
        );
    }

    /// A graceful stop — the SIGTERM path, which calls `release` — must hand
    /// the role over in about one renewal interval rather than one TTL.
    #[test]
    fn a_released_role_is_taken_over_within_one_renewal() {
        let mut record = FakeRecord::default();
        let mut old = SimTask::new("old");
        let mut new = SimTask::new("new");

        for now in (0..60).step_by(TIMINGS.renew.as_secs() as usize) {
            old.tick(&mut record, now, true);
        }
        assert!(old.owns(60));

        new.tick(&mut record, 60, true);
        assert!(
            !new.owns(60),
            "the incoming task loses while the old one holds"
        );

        // SIGTERM: `release` revokes the grant and deletes the record.
        old.grant_until = None;
        record = FakeRecord::default();

        let mut gap = 0u64;
        let mut now = 61;
        while !new.owns(now) {
            new.tick(&mut record, now, true);
            if !new.owns(now) {
                gap += 1;
                now += 1;
            }
            assert!(gap < 120, "the handover took longer than two minutes");
        }
        assert!(
            gap <= TIMINGS.renew.as_secs(),
            "a graceful handover cost {gap}s; it must cost at most one renewal interval"
        );
    }

    /// A task killed without releasing wedges the role for the TTL and no
    /// longer. This is the number the acceptance criterion is about.
    #[test]
    fn a_silent_death_costs_one_ttl_and_no_more() {
        let mut record = FakeRecord::default();
        let mut dead = SimTask::new("dead");
        let mut successor = SimTask::new("successor");

        dead.tick(&mut record, 0, true);
        assert!(dead.owns(0));
        // ...and then it stops ticking, without releasing.

        let mut now = 1;
        while !successor.owns(now) {
            successor.tick(&mut record, now, true);
            if !successor.owns(now) {
                now += 1;
            }
            assert!(now < 300, "nobody took the role over at all");
        }
        assert!(
            now <= TIMINGS.ttl.as_secs() + TIMINGS.renew.as_secs(),
            "the successor waited {now}s, more than the TTL plus one renewal"
        );
    }
}
