//! Usage counters for Bazaar listings: when one was last paid for through this
//! facilitator, and how many paid calls and distinct payers it had in the last
//! 30 days.
//!
//! # Why
//!
//! A router choosing among ten listings that do the same job has their prices
//! and their liveness, and nothing about whether anybody actually pays them.
//! On 2026-10-06 (2.48.0, 1 765 listings) not one listing carried a usage
//! count: `settlementCount` and `lastSettledAt` move only when a settle names
//! `discoverable: true`, which no seller sends.
//!
//! # Where the numbers come from
//!
//! The settlements this facilitator already records in its transaction store
//! ([`crate::transaction_store`]), one record per settle with the URL that was
//! bought and the payer. Read, never written, by this module: the first read
//! takes the whole window, every later one only what was recorded after the
//! newest record it holds, so a refresh costs a Query over the current day.
//! Every replica reads the same store and serves the same counts.
//!
//! # A floor, not a ledger
//!
//! The record is written after a settlement resolves, fire-and-forget: if the
//! store is unreachable the payment happened and the record does not exist. A
//! settlement through another facilitator is never seen. And a payment counts
//! for a listing only when the URL the buyer paid for is the listing's own
//! (its query string aside, for a listing whose URL has none). So every number
//! here is AT LEAST what happened, and says so wherever it is published.
//!
//! A deployment that records nothing (`TRANSACTIONS_TABLE_NAME` unset) publishes
//! no counts at all rather than zeros: zero is a claim about what happened,
//! and such a deployment has none to make.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::transaction_store::{TransactionRecord, TransactionStore};

/// The window `calls30d` and `uniquePayers30d` count over, in days.
pub const USAGE_WINDOW_DAYS: u64 = 30;

/// Seconds between two reads of the transaction store.
pub const USAGE_REFRESH_SECS: u64 = 300;

/// The most settlements one process keeps in its window. Measured on
/// 2026-07-30: ~1 600 operations a day, verifies included, so 30 days is about
/// 48 000 at most; this is four times that, and past it the oldest go first --
/// the counts become a smaller floor, never a wrong ceiling.
pub const MAX_WINDOW_SETTLEMENTS: usize = 200_000;

/// The usage of one listing, as `GET /discovery/resources` serves it under
/// `usage`.
///
/// Every count is a floor ([module docs](self)): what this facilitator recorded,
/// never what happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListingUsage {
    /// The latest settlement recorded for the listing, Unix seconds: in the
    /// window, or the listing's own `lastSettledAt` when that is later.
    /// Absent when there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_settled_at: Option<u64>,
    /// Successful settlements recorded for the listing in the last
    /// [`USAGE_WINDOW_DAYS`] days.
    #[serde(rename = "calls30d")]
    pub calls_30d: u64,
    /// Distinct payers among them.
    #[serde(rename = "uniquePayers30d")]
    pub unique_payers_30d: u64,
    /// When the counts were read from the store, Unix seconds.
    pub as_of: u64,
}

/// The keys a paid-for URL is counted under: the URL itself, and the URL
/// without its query, each with scheme and host in lowercase (the URL parser
/// does that), no fragment, and no trailing `/` on a path longer than `/`.
/// `None` when it is not a URL.
pub(crate) fn usage_keys(raw: &str) -> Option<(String, String)> {
    let mut url = url::Url::parse(raw.trim()).ok()?;
    url.set_fragment(None);
    let path = url.path().to_string();
    if path.len() > 1 && path.ends_with('/') {
        url.set_path(path.trim_end_matches('/'));
    }
    let exact = url.to_string();
    url.set_query(None);
    Some((exact, url.to_string()))
}

/// One recorded settlement, as the window keeps it.
#[derive(Debug, Clone)]
struct Settlement {
    ts_ms: u64,
    /// `usage_keys` of what was bought.
    exact: Arc<str>,
    path: Arc<str>,
    /// The payer as a grouping key (an EVM address in lowercase).
    payer: Arc<str>,
    /// The record's own key in the store, so a record read twice counts once.
    store_key: String,
}

/// Counts of one key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Counts {
    last_ms: u64,
    calls: u64,
    payers: u64,
}

/// The counts at one moment, by key.
#[derive(Debug, Default)]
pub struct UsageSnapshot {
    as_of: u64,
    /// By exact URL, query included.
    exact: HashMap<Arc<str>, Counts>,
    /// By URL without its query.
    path: HashMap<Arc<str>, Counts>,
}

impl UsageSnapshot {
    /// The usage of the listing at `listing_url`, given the listing's own
    /// `lastSettledAt`. A listing whose URL has a query is counted by that
    /// exact URL; one without is counted over every query its buyers sent.
    pub fn of(&self, listing_url: &url::Url, own_last_settled_at: Option<u64>) -> ListingUsage {
        let counts = usage_keys(listing_url.as_str()).and_then(|(exact, path)| {
            if listing_url.query().is_some() {
                self.exact.get(exact.as_str())
            } else {
                self.path.get(path.as_str())
            }
        });
        let window_last = counts.map(|c| c.last_ms / 1000).filter(|t| *t > 0);
        ListingUsage {
            last_settled_at: window_last.max(own_last_settled_at),
            calls_30d: counts.map_or(0, |c| c.calls),
            unique_payers_30d: counts.map_or(0, |c| c.payers),
            as_of: self.as_of,
        }
    }
}

/// The settlements of the window, the cursor of the next read, and the
/// published counts.
#[derive(Debug, Default)]
pub struct UsageTracker {
    window: tokio::sync::Mutex<Window>,
    snapshot: RwLock<Option<Arc<UsageSnapshot>>>,
}

#[derive(Debug, Default)]
struct Window {
    /// Oldest first.
    settlements: std::collections::VecDeque<Settlement>,
    seen: HashSet<String>,
    /// Whether the whole window was read once.
    loaded: bool,
    /// Interned strings, so 48 000 settlements of a few hundred listings hold
    /// a few hundred URLs.
    interned: HashSet<Arc<str>>,
}

impl Window {
    fn intern(&mut self, s: &str) -> Arc<str> {
        if let Some(existing) = self.interned.get(s) {
            return Arc::clone(existing);
        }
        let new: Arc<str> = Arc::from(s);
        self.interned.insert(Arc::clone(&new));
        new
    }
}

/// The payer as a grouping key: an EVM address in lowercase, anything else
/// exactly as written.
fn payer_key(payer: &str) -> String {
    crate::discovery_search::pay_to_key(payer)
}

impl UsageTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// The counts published now; `None` until the window was read once, and
    /// forever on a deployment that records no settlements.
    pub fn snapshot(&self) -> Option<Arc<UsageSnapshot>> {
        self.snapshot
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Read what the store recorded since the last read and publish the
    /// counts as of `now_ms`. Returns how many new settlements were read, or
    /// `None` when the store keeps no history.
    pub async fn refresh(
        &self,
        store: &dyn TransactionStore,
        now_ms: u64,
    ) -> Result<Option<usize>, crate::transaction_store::TransactionStoreError> {
        let window_ms = USAGE_WINDOW_DAYS * 86_400_000;
        let start = now_ms.saturating_sub(window_ms);
        let mut window = self.window.lock().await;
        // From the newest record held (records of the same millisecond are
        // read again and dropped by key), or the whole window the first time.
        let since = match (window.loaded, window.settlements.back()) {
            (true, Some(newest)) => newest.ts_ms.max(start),
            _ => start,
        };
        let Some(records) = store.settles_since(since).await? else {
            return Ok(None);
        };
        let added = Self::ingest(&mut window, records, start);
        window.loaded = true;
        let snapshot = Self::count(&window, now_ms);
        drop(window);
        *self
            .snapshot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(snapshot));
        Ok(Some(added))
    }

    /// Fold `records` into the window, dropping what is older than `start`,
    /// what is not a successful settle, and what the window already holds.
    fn ingest(window: &mut Window, records: Vec<TransactionRecord>, start: u64) -> usize {
        let mut added = 0;
        for r in records {
            if r.kind != "settle" || !r.ok || r.ts < start {
                continue;
            }
            let (Some(resource), Some(payer)) = (r.resource.as_deref(), r.payer.as_deref()) else {
                continue;
            };
            let Some((exact, path)) = usage_keys(resource) else {
                continue;
            };
            let store_key = r.sort_key();
            if !window.seen.insert(store_key.clone()) {
                continue;
            }
            let settlement = Settlement {
                ts_ms: r.ts,
                exact: window.intern(&exact),
                path: window.intern(&path),
                payer: window.intern(&payer_key(payer)),
                store_key,
            };
            // Kept in time order whatever order the store answered in.
            let at = window
                .settlements
                .partition_point(|s| s.ts_ms <= settlement.ts_ms);
            window.settlements.insert(at, settlement);
            added += 1;
        }
        while window.settlements.front().is_some_and(|s| s.ts_ms < start)
            || window.settlements.len() > MAX_WINDOW_SETTLEMENTS
        {
            if let Some(old) = window.settlements.pop_front() {
                window.seen.remove(&old.store_key);
            }
        }
        // Strings nothing in the window points at any more.
        window.interned.retain(|s| Arc::strong_count(s) > 1);
        added
    }

    /// The counts of the settlements in `window` from `now_ms` minus the
    /// window on.
    fn count(window: &Window, now_ms: u64) -> UsageSnapshot {
        let start = now_ms.saturating_sub(USAGE_WINDOW_DAYS * 86_400_000);
        let mut payers: HashMap<(bool, Arc<str>), HashSet<Arc<str>>> = HashMap::new();
        let mut snapshot = UsageSnapshot {
            as_of: now_ms / 1000,
            ..Default::default()
        };
        for s in window.settlements.iter().filter(|s| s.ts_ms >= start) {
            for (exact, key) in [(true, &s.exact), (false, &s.path)] {
                let map = if exact {
                    &mut snapshot.exact
                } else {
                    &mut snapshot.path
                };
                let c = map.entry(Arc::clone(key)).or_default();
                c.calls += 1;
                c.last_ms = c.last_ms.max(s.ts_ms);
                payers
                    .entry((exact, Arc::clone(key)))
                    .or_default()
                    .insert(Arc::clone(&s.payer));
            }
        }
        for ((exact, key), who) in payers {
            let map = if exact {
                &mut snapshot.exact
            } else {
                &mut snapshot.path
            };
            if let Some(c) = map.get_mut(&key) {
                c.payers = who.len() as u64;
            }
        }
        snapshot
    }
}

/// Keep `tracker` current from `store`: a read now, then one every
/// [`USAGE_REFRESH_SECS`]. A store that keeps no history is asked once and
/// never again. A failed read keeps the counts already published.
pub fn spawn_refresher(tracker: Arc<UsageTracker>, store: Arc<dyn TransactionStore>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(USAGE_REFRESH_SECS));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            match tracker
                .refresh(store.as_ref(), crate::events::now_ms())
                .await
            {
                Ok(None) => {
                    info!(
                        store = store.store_type(),
                        "the transaction store keeps no history; Bazaar listings publish no usage"
                    );
                    return;
                }
                Ok(Some(added)) => debug!(added, "Bazaar usage counters refreshed"),
                Err(e) => {
                    warn!(error = %e, "Bazaar usage counters not refreshed; serving the last ones")
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction_store::TransactionStoreError;

    const DAY_MS: u64 = 86_400_000;
    const NOW: u64 = 1_790_000_000_000;

    fn settle(ts: u64, resource: &str, payer: &str, tx: &str) -> TransactionRecord {
        TransactionRecord {
            ts,
            kind: "settle".into(),
            network: "base".into(),
            ok: true,
            payer: Some(payer.into()),
            tx: Some(tx.into()),
            amount: Some("10000".into()),
            asset: Some("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".into()),
            resource: Some(resource.into()),
            pay_to: Some("0x1234567890123456789012345678901234567890".into()),
            description: None,
            scheme: Some("exact".into()),
        }
    }

    /// A store that answers `settles_since` from a list, and records the
    /// cursor it was asked for.
    #[derive(Debug, Default)]
    struct Recorded {
        records: std::sync::Mutex<Vec<TransactionRecord>>,
        asked: std::sync::Mutex<Vec<u64>>,
        history: bool,
    }

    #[async_trait::async_trait]
    impl TransactionStore for Recorded {
        async fn record(&self, record: TransactionRecord) -> Result<(), TransactionStoreError> {
            self.records.lock().unwrap().push(record);
            Ok(())
        }
        async fn recent(
            &self,
            _: usize,
            _: Option<&str>,
        ) -> Result<Vec<TransactionRecord>, TransactionStoreError> {
            Ok(Vec::new())
        }
        async fn aggregates(
            &self,
        ) -> Result<Vec<crate::transaction_store::Aggregate>, TransactionStoreError> {
            Ok(Vec::new())
        }
        async fn settles_since(
            &self,
            since_ms: u64,
        ) -> Result<Option<Vec<TransactionRecord>>, TransactionStoreError> {
            self.asked.lock().unwrap().push(since_ms);
            if !self.history {
                return Ok(None);
            }
            Ok(Some(
                self.records
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|r| r.ts >= since_ms)
                    .cloned()
                    .collect(),
            ))
        }
        fn store_type(&self) -> &'static str {
            "recorded"
        }
    }

    fn store(records: Vec<TransactionRecord>) -> Recorded {
        Recorded {
            records: std::sync::Mutex::new(records),
            history: true,
            ..Default::default()
        }
    }

    fn url(s: &str) -> url::Url {
        url::Url::parse(s).unwrap()
    }

    #[tokio::test]
    async fn counts_calls_and_distinct_payers_in_the_window() {
        let s = store(vec![
            settle(
                NOW - DAY_MS,
                "https://api.example.com/quote?t=AAPL",
                "0xAAAA",
                "0x1",
            ),
            settle(
                NOW - 2 * DAY_MS,
                "https://api.example.com/quote?t=MSFT",
                "0xaaaa",
                "0x2",
            ),
            settle(
                NOW - 3 * DAY_MS,
                "https://api.example.com/quote",
                "0xbbbb",
                "0x3",
            ),
            // Out of the window.
            settle(
                NOW - 31 * DAY_MS,
                "https://api.example.com/quote",
                "0xcccc",
                "0x4",
            ),
            // Not a successful settle.
            TransactionRecord {
                ok: false,
                ..settle(
                    NOW - DAY_MS,
                    "https://api.example.com/quote",
                    "0xdddd",
                    "0x5",
                )
            },
            TransactionRecord {
                kind: "verify".into(),
                ..settle(
                    NOW - DAY_MS,
                    "https://api.example.com/quote",
                    "0xeeee",
                    "0x6",
                )
            },
            settle(NOW - DAY_MS, "https://other.example.com/x", "0xaaaa", "0x7"),
        ]);
        let tracker = UsageTracker::new();
        assert!(
            tracker.snapshot().is_none(),
            "nothing read, nothing claimed"
        );
        assert_eq!(tracker.refresh(&s, NOW).await.unwrap(), Some(4));
        let snap = tracker.snapshot().unwrap();

        // A listing without a query counts every query its buyers sent; one
        // EVM payer in two spellings is one payer.
        let u = snap.of(&url("https://api.example.com/quote"), None);
        assert_eq!((u.calls_30d, u.unique_payers_30d), (3, 2));
        assert_eq!(u.last_settled_at, Some((NOW - DAY_MS) / 1000));
        assert_eq!(u.as_of, NOW / 1000);
        // A listing whose URL has a query is counted by that URL.
        let u = snap.of(&url("https://api.example.com/quote?t=MSFT"), None);
        assert_eq!((u.calls_30d, u.unique_payers_30d), (1, 1));
        // Nothing recorded: zero, and no date unless the listing has one.
        let u = snap.of(&url("https://quiet.example.com/x"), None);
        assert_eq!(
            (u.calls_30d, u.unique_payers_30d, u.last_settled_at),
            (0, 0, None)
        );
        let u = snap.of(&url("https://quiet.example.com/x"), Some(1_700_000_000));
        assert_eq!(u.last_settled_at, Some(1_700_000_000));
    }

    #[tokio::test]
    async fn a_later_read_asks_only_for_what_is_new_and_counts_nothing_twice() {
        let s = store(vec![settle(
            NOW - DAY_MS,
            "https://a.example/x",
            "0x1",
            "0xa",
        )]);
        let tracker = UsageTracker::new();
        tracker.refresh(&s, NOW).await.unwrap();
        s.records
            .lock()
            .unwrap()
            .push(settle(NOW + 1_000, "https://a.example/x", "0x2", "0xb"));
        assert_eq!(tracker.refresh(&s, NOW + 2_000).await.unwrap(), Some(1));
        let asked = s.asked.lock().unwrap().clone();
        assert_eq!(
            asked[0],
            NOW - USAGE_WINDOW_DAYS * DAY_MS,
            "the whole window first"
        );
        assert_eq!(asked[1], NOW - DAY_MS, "then from the newest record held");
        let u = tracker
            .snapshot()
            .unwrap()
            .of(&url("https://a.example/x"), None);
        assert_eq!((u.calls_30d, u.unique_payers_30d), (2, 2));
    }

    #[tokio::test]
    async fn settlements_leave_the_window_as_it_moves() {
        let s = store(vec![
            settle(NOW - 29 * DAY_MS, "https://a.example/x", "0x1", "0xa"),
            settle(NOW - DAY_MS, "https://a.example/x", "0x2", "0xb"),
        ]);
        let tracker = UsageTracker::new();
        tracker.refresh(&s, NOW).await.unwrap();
        tracker.refresh(&s, NOW + 2 * DAY_MS).await.unwrap();
        let u = tracker
            .snapshot()
            .unwrap()
            .of(&url("https://a.example/x"), None);
        assert_eq!(u.calls_30d, 1);
    }

    #[tokio::test]
    async fn a_store_without_history_publishes_nothing() {
        let s = Recorded::default();
        let tracker = UsageTracker::new();
        assert_eq!(tracker.refresh(&s, NOW).await.unwrap(), None);
        assert!(tracker.snapshot().is_none());
        // The no-op store is such a store.
        let noop = crate::transaction_store::NoopTransactionStore;
        assert_eq!(tracker.refresh(&noop, NOW).await.unwrap(), None);
    }

    #[test]
    fn a_paid_url_is_keyed_without_case_fragment_or_trailing_slash() {
        let (exact, path) = usage_keys("HTTPS://API.Example.com/Quote/?t=1#frag").unwrap();
        assert_eq!(exact, "https://api.example.com/Quote?t=1");
        assert_eq!(path, "https://api.example.com/Quote");
        assert_eq!(
            usage_keys("https://a.example/").unwrap().1,
            "https://a.example/"
        );
        assert!(usage_keys("not a url").is_none());
        assert!(usage_keys("").is_none());
    }

    #[test]
    fn the_counts_serialize_as_documented() {
        let u = ListingUsage {
            last_settled_at: None,
            calls_30d: 3,
            unique_payers_30d: 2,
            as_of: 7,
        };
        assert_eq!(
            serde_json::to_value(&u).unwrap(),
            serde_json::json!({"calls30d": 3, "uniquePayers30d": 2, "asOf": 7})
        );
    }
}
