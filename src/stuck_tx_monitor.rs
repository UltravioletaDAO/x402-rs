//! Detects the facilitator's OWN transactions wedged in a node's mempool.
//!
//! # Why this exists
//!
//! On 2026-09-03 at 21:28:00Z the facilitator submitted an escrow `release` on
//! Polygon while the chain's base fee was in a trough of 1.072 gwei. Alloy's
//! default estimator caps a transaction at `2 * baseFee + priority`, so it went
//! out with a 32.247 gwei ceiling. Forty minutes later Polygon's base fee was
//! back at its usual 248 gwei and that transaction could never be mined again.
//!
//! Nonces are strictly ordered, so the account froze behind it. 399 correctly
//! priced transactions piled up, their pooled `gasLimit * maxFeePerGas` reached
//! 82.80 of the signer's 82.86 POL, and from then on the node refused every new
//! Polygon settle with `insufficient funds for gas * price + value`.
//!
//! **It took six days to notice.** Nothing was down: `/health` was green, the
//! other nineteen chains kept settling, the balance never moved so no
//! balance alarm fired, and the node's rejection reads like an underfunded
//! wallet rather than a wedged queue. The one signal that was unambiguous the
//! whole time is the one this module watches: the account's next nonce did not
//! advance while its pending count kept climbing.
//!
//! # What counts as stuck
//!
//! Not "there is a backlog" -- a busy signer has one all the time. Stuck is
//! **the head is not moving**: `eth_getTransactionCount(latest)` stays put while
//! `pending` runs ahead of it. A queue that is draining slowly is a capacity
//! problem and deliberately does not fire here; conflating the two would put a
//! page on every traffic burst and teach everyone to ignore it.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::Address;
use alloy::providers::Provider;

use crate::chain::evm::MetaEvmProvider;
use crate::chain::NetworkProvider;
use crate::network::Network;
use crate::provider_cache::{ProviderCache, ProviderMap};

/// Pending transactions beyond the confirmed nonce before the head is even
/// considered. A settle burst routinely puts a handful in flight at once.
pub const DEFAULT_BACKLOG_THRESHOLD: u64 = 5;

/// How long the head must sit still, with a backlog behind it, before this is
/// reported. Ten minutes is far longer than any legitimate inclusion delay on
/// the chains served here and far shorter than the six days it took last time.
pub const DEFAULT_STUCK_AFTER: Duration = Duration::from_secs(600);

/// Gap between probes. Two `eth_getTransactionCount` calls per EVM network per
/// interval, on every task -- cheap, but not free, hence minutes and not seconds.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(120);

/// The message the CloudWatch metric filter matches on. Kept as a bare token
/// with no spaces so the log group's ANSI colouring cannot split it: the colour
/// codes wrap `key=value` separators, which is why filters here match message
/// text rather than positional patterns.
pub const STUCK_EVENT: &str = "evm_signer_transactions_stuck";

/// Thresholds, separated from the loop so tests do not wait ten real minutes.
#[derive(Clone, Copy, Debug)]
pub struct MonitorConfig {
    pub backlog_threshold: u64,
    pub stuck_after_secs: u64,
    pub poll_interval: Duration,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            backlog_threshold: DEFAULT_BACKLOG_THRESHOLD,
            stuck_after_secs: DEFAULT_STUCK_AFTER.as_secs(),
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }
}

/// What one observation of a signer's nonces means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// No meaningful backlog, or the head is advancing.
    Healthy,
    /// The head has not moved and a backlog is present, but not for long enough
    /// yet to distinguish from a slow block.
    Watching { backlog: u64, for_secs: u64 },
    /// The head has not moved for longer than the threshold.
    Stuck {
        backlog: u64,
        /// The account's next nonce: the transaction that has to clear before
        /// anything behind it can. This is the number an operator needs.
        first_unmined: u64,
        for_secs: u64,
    },
}

/// Per-signer bookkeeping. Time enters as a caller-supplied monotonic second
/// count so the state machine is testable without sleeping or mocking a clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BacklogTracker {
    /// Confirmed nonce at the last observation.
    last_latest: Option<u64>,
    /// When the head was last seen to move, or when watching began.
    head_still_since: Option<u64>,
}

impl BacklogTracker {
    /// Fold one `(latest, pending)` reading into the tracker.
    pub fn observe(
        &mut self,
        latest: u64,
        pending: u64,
        now_secs: u64,
        config: &MonitorConfig,
    ) -> Verdict {
        let backlog = pending.saturating_sub(latest);
        let previous = self.last_latest.replace(latest);

        // First sighting of this signer: record the baseline and say nothing.
        // Whether the head is moving takes two readings to know, and a verdict
        // from one would fire on every process start -- where `pending`
        // legitimately runs ahead of `latest` for transactions the PREVIOUS
        // task submitted and this one has no business judging.
        let Some(previous) = previous else {
            self.head_still_since = Some(now_secs);
            return Verdict::Healthy;
        };

        // The head moved: whatever was in front has been mined or dropped, and
        // any clock we were running is about the previous head, not this one.
        if latest > previous {
            self.head_still_since = Some(now_secs);
            return Verdict::Healthy;
        }

        if backlog <= config.backlog_threshold {
            // Nothing meaningful is waiting, so a motionless head is just an
            // idle signer. Clear the clock so the next real backlog is timed
            // from when IT started rather than from an hour of idleness.
            self.head_still_since = Some(now_secs);
            return Verdict::Healthy;
        }

        let since = *self.head_still_since.get_or_insert(now_secs);
        let for_secs = now_secs.saturating_sub(since);
        if for_secs >= config.stuck_after_secs {
            Verdict::Stuck {
                backlog,
                first_unmined: latest,
                for_secs,
            }
        } else {
            Verdict::Watching { backlog, for_secs }
        }
    }
}

/// Start the monitor. Returns immediately; the work runs in a background task.
///
/// Read-only and failure-tolerant by construction: it sends two
/// `eth_getTransactionCount` calls per EVM network per interval and can neither
/// slow down nor fail a payment. An RPC error is skipped rather than reported,
/// because "we could not read the nonce" is not "the queue is stuck", and
/// `chain_rpc_unreachable` already covers the former.
pub fn spawn(providers: Arc<ProviderCache>) {
    spawn_with_config(providers, MonitorConfig::default());
}

/// [`spawn`] with explicit thresholds.
pub fn spawn_with_config(providers: Arc<ProviderCache>, config: MonitorConfig) {
    let signers: Vec<(Network, Address)> = providers
        .values()
        .filter_map(|provider| match provider {
            NetworkProvider::Evm(evm) => Some((evm.chain().network(), evm.pinned_signer())),
            _ => None,
        })
        .collect();

    if signers.is_empty() {
        tracing::debug!("stuck-transaction monitor: no EVM providers configured");
        return;
    }

    tracing::info!(
        networks = signers.len(),
        backlog_threshold = config.backlog_threshold,
        stuck_after_secs = config.stuck_after_secs,
        poll_interval_secs = config.poll_interval.as_secs(),
        "stuck-transaction monitor started"
    );

    tokio::spawn(async move {
        let started = std::time::Instant::now();
        let mut trackers: HashMap<(Network, Address), BacklogTracker> = HashMap::new();
        let mut ticker = tokio::time::interval(config.poll_interval);
        // The first tick fires immediately, and at startup `pending` legitimately
        // runs ahead of `latest` for transactions the previous task submitted.
        // Skipping nothing here would time that from zero and is harmless (the
        // clock only starts on the SECOND observation of a motionless head), but
        // waiting one interval keeps the first reading from being about a queue
        // this process never touched.
        ticker.tick().await;

        loop {
            ticker.tick().await;
            let now_secs = started.elapsed().as_secs();

            for (network, signer) in &signers {
                let Some(NetworkProvider::Evm(evm)) = providers.by_network(network) else {
                    continue;
                };
                let latest = match evm.inner().get_transaction_count(*signer).await {
                    Ok(value) => value,
                    Err(error) => {
                        tracing::debug!(%network, ?error, "stuck-transaction monitor: latest nonce unreadable");
                        continue;
                    }
                };
                let pending = match evm.inner().get_transaction_count(*signer).pending().await {
                    Ok(value) => value,
                    Err(error) => {
                        tracing::debug!(%network, ?error, "stuck-transaction monitor: pending nonce unreadable");
                        continue;
                    }
                };

                let tracker = trackers.entry((*network, *signer)).or_default();
                match tracker.observe(latest, pending, now_secs, &config) {
                    Verdict::Healthy => {}
                    Verdict::Watching { backlog, for_secs } => {
                        tracing::debug!(
                            %network, %signer, backlog, for_secs,
                            "signer head not advancing"
                        );
                    }
                    Verdict::Stuck {
                        backlog,
                        first_unmined,
                        for_secs,
                    } => {
                        // Re-emitted on every tick while it lasts, on purpose:
                        // the alarm behind this sums occurrences over a window,
                        // and one line six days ago is not a signal.
                        tracing::warn!(
                            %network,
                            %signer,
                            backlog,
                            first_unmined_nonce = first_unmined,
                            stuck_for_secs = for_secs,
                            "{STUCK_EVENT}: the account's next nonce has not advanced \
                             while transactions pile up behind it. Nonce {first_unmined} \
                             is almost certainly unmineable (priced below the current \
                             base fee). Run scripts/polygon_destrabar_cola.py --dry-run."
                        );
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> MonitorConfig {
        MonitorConfig {
            backlog_threshold: 5,
            stuck_after_secs: 600,
            poll_interval: Duration::from_secs(120),
        }
    }

    #[test]
    fn an_idle_signer_is_healthy_forever() {
        let mut tracker = BacklogTracker::default();
        for tick in 0..100 {
            let verdict = tracker.observe(1157, 1157, tick * 120, &config());
            assert_eq!(verdict, Verdict::Healthy);
        }
    }

    #[test]
    fn a_draining_queue_never_fires() {
        // Busy but healthy: the head advances every tick and the backlog is
        // large the whole time. This is the false positive that would make the
        // alarm worthless, so it is pinned.
        let mut tracker = BacklogTracker::default();
        for tick in 0..100u64 {
            let latest = 1000 + tick;
            let verdict = tracker.observe(latest, latest + 40, tick * 120, &config());
            assert_eq!(
                verdict,
                Verdict::Healthy,
                "fired on a draining queue at tick {tick}"
            );
        }
    }

    #[test]
    fn the_polygon_incident_is_detected_in_about_ten_minutes() {
        // The real shape: latest pinned at 1157, pending climbing to 1557.
        let mut tracker = BacklogTracker::default();
        let cfg = config();

        // The first reading is a baseline -- two are needed to know the head is
        // still -- but it does stamp WHEN the head was last seen at 1157, so
        // once the second reading confirms it has not moved, the elapsed time
        // is counted from the baseline rather than from the confirmation.
        assert_eq!(tracker.observe(1157, 1200, 0, &cfg), Verdict::Healthy);
        assert_eq!(
            tracker.observe(1157, 1250, 120, &cfg),
            Verdict::Watching {
                backlog: 93,
                for_secs: 120
            }
        );
        assert!(matches!(
            tracker.observe(1157, 1400, 480, &cfg),
            Verdict::Watching { .. }
        ));

        // Ten minutes after the head was first seen where it still is, against
        // the six days it took to notice by hand.
        match tracker.observe(1157, 1557, 600, &cfg) {
            Verdict::Stuck {
                backlog,
                first_unmined,
                for_secs,
            } => {
                assert_eq!(backlog, 400);
                // The number an operator acts on: the nonce to replace.
                assert_eq!(first_unmined, 1157);
                assert_eq!(for_secs, 600);
            }
            other => panic!("expected Stuck, got {other:?}"),
        }
    }

    #[test]
    fn it_keeps_firing_while_the_queue_stays_stuck() {
        let mut tracker = BacklogTracker::default();
        let cfg = config();
        tracker.observe(1157, 1557, 0, &cfg); // baseline
        for tick in 5..60u64 {
            assert!(
                matches!(
                    tracker.observe(1157, 1557, tick * 120, &cfg),
                    Verdict::Stuck { .. }
                ),
                "stopped reporting at tick {tick} while the queue was still stuck"
            );
        }
    }

    #[test]
    fn clearing_the_head_resets_the_clock() {
        let mut tracker = BacklogTracker::default();
        let cfg = config();
        tracker.observe(1157, 1557, 0, &cfg); // baseline
        assert!(matches!(
            tracker.observe(1157, 1557, 600, &cfg),
            Verdict::Stuck { .. }
        ));
        // The replacement mines and the queue starts draining: the clock is
        // about THIS head, so it restarts rather than carrying the old elapsed.
        assert_eq!(tracker.observe(1158, 1557, 720, &cfg), Verdict::Healthy);
        assert_eq!(
            tracker.observe(1158, 1557, 840, &cfg),
            Verdict::Watching {
                backlog: 399,
                for_secs: 120
            }
        );
    }

    #[test]
    fn a_backlog_under_the_threshold_is_not_a_backlog() {
        let mut tracker = BacklogTracker::default();
        let cfg = config();
        for tick in 0..20u64 {
            assert_eq!(
                tracker.observe(1157, 1157 + cfg.backlog_threshold, tick * 120, &cfg),
                Verdict::Healthy
            );
        }
    }

    #[test]
    fn a_pending_count_below_latest_is_not_a_negative_backlog() {
        // Some load balancers answer `pending` from a node that is behind.
        let mut tracker = BacklogTracker::default();
        let cfg = config();
        for tick in 0..20u64 {
            assert_eq!(
                tracker.observe(1157, 1100, tick * 120, &cfg),
                Verdict::Healthy
            );
        }
    }
}
