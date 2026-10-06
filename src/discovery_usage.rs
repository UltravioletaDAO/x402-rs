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
//! bought, the network, the recipient and the payer. Read, never written, by
//! this module: the first read takes the whole window, every later one only
//! what was recorded since the previous read (with a margin for records that
//! land late), so a refresh costs a Query over the current day. Every replica
//! reads the same store and serves the same counts, a refresh apart.
//!
//! # What counts
//!
//! A successful settle on a **mainnet** that matches one of the listing's own
//! payment options -- the same network, recipient, asset and scheme, and for
//! `exact` at least the option's price -- for the listing's own URL (any query
//! string, for a listing whose URL has none), by a payer that is not that
//! recipient. The URL, recipient and amount of a settle are whatever its
//! caller sent, so a settle on a testnet, one paying somebody else, or one of a
//! single atomic unit would let anyone write counts onto any listing for the
//! price of faucet gas; none counts. The amount recorded is the one the payment
//! requirements named, which `exact` verification refuses to settle for less
//! than on every chain family; under another scheme (`upto`) it is the declared
//! maximum, so it is not compared. A payer paying a listing it controls, at its
//! price, from many fresh wallets is still counted: the numbers say how much
//! was paid, not by whom.
//!
//! # A floor, not a ledger
//!
//! The record is written after a settlement resolves, fire-and-forget: if the
//! store is unreachable the payment happened and the record does not exist. A
//! settlement through another facilitator is never seen. So every number here
//! is AT LEAST what this facilitator saw paid, and says so wherever it is
//! published. A listing whose URL is a template (`{id}`) carries no counts at
//! all: no paid URL equals it, and a zero would claim it was never paid.
//!
//! A deployment that records nothing (`TRANSACTIONS_TABLE_NAME` unset) publishes
//! no counts at all rather than zeros: zero is a claim about what happened,
//! and such a deployment has none to make.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use alloy::primitives::U256;

use crate::network::Network;
use crate::transaction_store::{TransactionRecord, TransactionStore};
use crate::types_v2::DiscoveryResource;

/// The window `calls30d` and `uniquePayers30d` count over, in days.
pub const USAGE_WINDOW_DAYS: u64 = 30;

/// Seconds between two reads of the transaction store.
pub const USAGE_REFRESH_SECS: u64 = 300;

/// The most settlements one process keeps in its window. Measured on
/// 2026-07-30: ~1 600 operations a day, verifies included, so 30 days is about
/// 48 000 at most; this is four times that, and past it the oldest go first --
/// the counts become a smaller floor, never a wrong ceiling.
pub const MAX_WINDOW_SETTLEMENTS: usize = 200_000;

/// How far before the previous read each refresh starts again. A record is
/// stamped before its fire-and-forget write lands, by one of several tasks,
/// and read from an eventually consistent Query: one stamped just before the
/// previous read can become visible just after it. Read twice, it counts once.
const READ_MARGIN_MS: u64 = 120_000;

/// The usage of one listing, as `GET /discovery/resources` serves it under
/// `usage`.
///
/// Every count is a floor ([module docs](self)): what this facilitator recorded,
/// never what happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListingUsage {
    /// The latest settlement counted for the listing in the window, Unix
    /// seconds; absent when there is none. Never the listing's own
    /// `lastSettledAt`, which a settle moves without these checks.
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

/// Whether a listing URL is a template (`/item/{id}`, as written or
/// percent-encoded): a declaration that the URL paid for is built from
/// parameters, so no paid URL is ever equal to it.
fn is_template(url: &url::Url) -> bool {
    let s = url.as_str();
    s.contains(['{', '}'])
        || ["%7b", "%7d"]
            .iter()
            .any(|e| s.to_ascii_lowercase().contains(e))
}

/// One recorded settlement, as the window keeps it.
#[derive(Debug, Clone)]
struct Settlement {
    ts_ms: u64,
    /// `usage_keys` of what was bought.
    exact: Arc<str>,
    path: Arc<str>,
    /// Where it settled, and who it paid ([`pay_to_key`]).
    network: Network,
    pay_to: Arc<str>,
    /// What it paid in ([`pay_to_key`]), under which scheme, and the amount
    /// the payment requirements named, in atomic units.
    asset: Arc<str>,
    scheme: Arc<str>,
    amount: U256,
    /// The payer as a grouping key (an EVM address in lowercase).
    payer: Arc<str>,
    /// The record's own key in the store, so a record read twice counts once.
    store_key: Arc<str>,
}

/// The settlements of the window at one moment, indexed by the URL bought.
#[derive(Debug, Default)]
pub struct UsageSnapshot {
    as_of: u64,
    settlements: Vec<Settlement>,
    /// By exact URL, query included: positions in `settlements`.
    exact: HashMap<Arc<str>, Vec<u32>>,
    /// By URL without its query.
    path: HashMap<Arc<str>, Vec<u32>>,
}

impl UsageSnapshot {
    /// The usage of `listing`, or `None` for a URL template, which no paid URL
    /// can equal, so a zero there would be a measurement nobody made.
    ///
    /// Counted: the window's settlements of the listing's URL (that exact URL
    /// for a listing whose URL has a query; any query its buyers sent for one
    /// whose URL has none) that match one of the listing's payment options
    /// ([`Price::paid_by`]). `lastSettledAt` is the latest of those.
    pub fn of(&self, listing: &DiscoveryResource) -> Option<ListingUsage> {
        if is_template(&listing.url) {
            return None;
        }
        let prices: Vec<Price> = listing
            .accepts
            .iter()
            .filter_map(|option| {
                Some(Price {
                    network: Network::from_caip2(&option.network.to_string())?,
                    pay_to: pay_to_key(&option.pay_to.to_string()),
                    asset: pay_to_key(&option.asset.to_string()),
                    scheme: option.scheme.to_string(),
                    amount: option.amount.0,
                })
            })
            .collect();
        let positions = usage_keys(listing.url.as_str()).and_then(|(exact, path)| {
            if listing.url.query().is_some() {
                self.exact.get(exact.as_str())
            } else {
                self.path.get(path.as_str())
            }
        });
        let mut calls = 0u64;
        let mut last_ms = 0u64;
        let mut payers: HashSet<&str> = HashSet::new();
        for s in positions
            .into_iter()
            .flatten()
            .filter_map(|i| self.settlements.get(*i as usize))
            .filter(|s| prices.iter().any(|price| price.paid_by(s)))
        {
            calls += 1;
            last_ms = last_ms.max(s.ts_ms);
            payers.insert(&s.payer);
        }
        Some(ListingUsage {
            last_settled_at: (last_ms > 0).then_some(last_ms / 1000),
            calls_30d: calls,
            unique_payers_30d: payers.len() as u64,
            as_of: self.as_of,
        })
    }
}

/// One payment option of a listing, as a settlement is matched against it.
struct Price {
    network: Network,
    pay_to: String,
    asset: String,
    scheme: String,
    amount: U256,
}

impl Price {
    /// Whether `s` paid this option: the same network, recipient, asset and
    /// scheme, and under `exact` at least its price -- a single atomic unit
    /// to the right address is not a call of the listing.
    fn paid_by(&self, s: &Settlement) -> bool {
        self.network == s.network
            && *self.pay_to == *s.pay_to
            && *self.asset == *s.asset
            && *self.scheme == *s.scheme
            && (self.scheme != "exact" || s.amount >= self.amount)
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
    seen: HashSet<Arc<str>>,
    /// When the previous read was made, once the whole window was read.
    read_at: Option<u64>,
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

/// An address as a grouping key: an EVM address in lowercase, anything else
/// exactly as written.
fn pay_to_key(address: &str) -> String {
    crate::discovery_search::pay_to_key(address)
}

/// The network a record names, when it is a mainnet this build knows: the
/// slug the settle path writes (`base`), or its CAIP-2 form.
fn mainnet_of(raw: &str) -> Option<Network> {
    let network = raw
        .parse::<Network>()
        .ok()
        .or_else(|| Network::from_caip2(raw))?;
    network.is_mainnet().then_some(network)
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
        // The whole window the first time; afterwards from the previous read,
        // less a margin for late records (read twice, they count once).
        let since = match window.read_at {
            Some(at) => at.saturating_sub(READ_MARGIN_MS).max(start),
            None => start,
        };
        let Some(records) = store.settles_since(since).await? else {
            return Ok(None);
        };
        // A read cut at the cap stopped at its newest record: the next one
        // goes on from there, not from now.
        let cut = (records.len() >= MAX_WINDOW_SETTLEMENTS)
            .then(|| records.iter().map(|r| r.ts).max())
            .flatten();
        let added = Self::ingest(&mut window, records, start);
        window.read_at = Some(cut.unwrap_or(now_ms));
        let snapshot = Self::count(&window, now_ms);
        drop(window);
        *self
            .snapshot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(snapshot));
        Ok(Some(added))
    }

    /// Fold `records` into the window, keeping only successful settles of the
    /// window, on a mainnet, of a URL no longer than a listing's may be, that
    /// say what they paid, paid by somebody other than their recipient, and
    /// not already held.
    fn ingest(window: &mut Window, records: Vec<TransactionRecord>, start: u64) -> usize {
        let mut added = 0;
        for r in records {
            if r.kind != "settle" || !r.ok || r.ts < start {
                continue;
            }
            let (Some(resource), Some(payer), Some(pay_to), Some(asset), Some(scheme)) = (
                r.resource.as_deref(),
                r.payer.as_deref(),
                r.pay_to.as_deref(),
                r.asset.as_deref(),
                r.scheme.as_deref(),
            ) else {
                continue;
            };
            let Some(amount) = r
                .amount
                .as_deref()
                .and_then(|a| U256::from_str_radix(a.trim(), 10).ok())
            else {
                continue;
            };
            // A URL is what the caller of /settle sent: bounded like a
            // listing's, or it could hold any amount of memory per record.
            if resource.len() > crate::discovery_security::MAX_URL_LEN {
                continue;
            }
            let Some(network) = mainnet_of(&r.network) else {
                continue;
            };
            let (payer, pay_to) = (pay_to_key(payer), pay_to_key(pay_to));
            if payer == pay_to {
                continue;
            }
            let Some((exact, path)) = usage_keys(resource) else {
                continue;
            };
            let store_key: Arc<str> = Arc::from(r.sort_key());
            if !window.seen.insert(Arc::clone(&store_key)) {
                continue;
            }
            let settlement = Settlement {
                ts_ms: r.ts,
                exact: window.intern(&exact),
                path: window.intern(&path),
                network,
                pay_to: window.intern(&pay_to),
                asset: window.intern(&pay_to_key(asset)),
                scheme: window.intern(scheme.trim()),
                amount,
                payer: window.intern(&payer),
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

    /// The settlements in `window` from `now_ms` minus the window on, indexed
    /// by the URL bought.
    fn count(window: &Window, now_ms: u64) -> UsageSnapshot {
        let start = now_ms.saturating_sub(USAGE_WINDOW_DAYS * 86_400_000);
        let mut snapshot = UsageSnapshot {
            as_of: now_ms / 1000,
            ..Default::default()
        };
        for s in window.settlements.iter().filter(|s| s.ts_ms >= start) {
            let at = snapshot.settlements.len() as u32;
            snapshot
                .exact
                .entry(Arc::clone(&s.exact))
                .or_default()
                .push(at);
            snapshot
                .path
                .entry(Arc::clone(&s.path))
                .or_default()
                .push(at);
            snapshot.settlements.push(s.clone());
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
    use crate::caip2::Caip2NetworkId;
    use crate::discovery_price::{CatalogPaymentOption, CatalogScheme};
    use crate::transaction_store::TransactionStoreError;
    use crate::types::{MixedAddress, Scheme, TokenAmount};

    const DAY_MS: u64 = 86_400_000;
    const NOW: u64 = 1_790_000_000_000;
    const SELLER: &str = "0x1234567890123456789012345678901234567890";

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
            pay_to: Some(SELLER.into()),
            description: None,
            scheme: Some("exact".into()),
        }
    }

    /// A listing at `url` that takes USDC on Base, paid to [`SELLER`].
    fn listing(url: &str) -> DiscoveryResource {
        DiscoveryResource::new(
            url::Url::parse(url).unwrap(),
            "http".to_string(),
            String::new(),
            vec![CatalogPaymentOption::new(
                CatalogScheme::Known(Scheme::Exact),
                Caip2NetworkId::eip155(8453),
                MixedAddress::Evm(
                    "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
                        .parse()
                        .unwrap(),
                ),
                TokenAmount::from(10_000u64),
                MixedAddress::Evm(SELLER.parse().unwrap()),
                300,
            )],
        )
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
        let u = snap.of(&listing("https://api.example.com/quote")).unwrap();
        assert_eq!((u.calls_30d, u.unique_payers_30d), (3, 2));
        assert_eq!(u.last_settled_at, Some((NOW - DAY_MS) / 1000));
        assert_eq!(u.as_of, NOW / 1000);
        // A listing whose URL has a query is counted by that URL.
        let u = snap
            .of(&listing("https://api.example.com/quote?t=MSFT"))
            .unwrap();
        assert_eq!((u.calls_30d, u.unique_payers_30d), (1, 1));
        // Nothing recorded: zero and no date -- not even the listing's own
        // `lastSettledAt`, which a settle moves without these checks.
        let mut quiet = listing("https://quiet.example.com/x");
        quiet.last_settled_at = Some(1_700_000_000);
        let u = snap.of(&quiet).unwrap();
        assert_eq!(
            (u.calls_30d, u.unique_payers_30d, u.last_settled_at),
            (0, 0, None)
        );
        // A template cannot be matched by a paid URL: no count, not zero.
        assert_eq!(
            snap.of(&listing("https://api.example.com/quote/{ticker}")),
            None
        );
    }

    /// The URL, the network, the recipient and the amount of a settle are
    /// what its caller sent. Only a mainnet payment matching an option the
    /// listing itself declares -- recipient, asset, scheme and at least its
    /// price -- by somebody else, counts: anything else would let anyone write
    /// counts onto any listing for the price of faucet gas.
    #[tokio::test]
    async fn only_mainnet_payments_matching_the_listings_own_option_count() {
        let url = "https://api.example.com/quote";
        let s = store(vec![
            settle(NOW - DAY_MS, url, "0xa1", "0x1"),
            // A testnet.
            TransactionRecord {
                network: "base-sepolia".into(),
                ..settle(NOW - DAY_MS, url, "0xa2", "0x2")
            },
            // Another recipient on the listing's network.
            TransactionRecord {
                pay_to: Some("0x00000000000000000000000000000000000000ff".into()),
                ..settle(NOW - DAY_MS, url, "0xa3", "0x3")
            },
            // The listing's recipient, on a network it does not declare.
            TransactionRecord {
                network: "polygon".into(),
                ..settle(NOW - DAY_MS, url, "0xa4", "0x4")
            },
            // The recipient paying itself.
            settle(
                NOW - DAY_MS,
                url,
                &SELLER.to_ascii_uppercase().replace("0X", "0x"),
                "0x5",
            ),
            // No recipient, an unknown network, an oversized URL.
            TransactionRecord {
                pay_to: None,
                ..settle(NOW - DAY_MS, url, "0xa6", "0x6")
            },
            TransactionRecord {
                network: "not-a-chain".into(),
                ..settle(NOW - DAY_MS, url, "0xa7", "0x7")
            },
            settle(
                NOW - DAY_MS,
                &format!(
                    "{url}?q={}",
                    "x".repeat(crate::discovery_security::MAX_URL_LEN)
                ),
                "0xa8",
                "0x8",
            ),
            // The CAIP-2 spelling of Base counts like `base`.
            TransactionRecord {
                network: "eip155:8453".into(),
                ..settle(NOW - DAY_MS, url, "0xa9", "0x9")
            },
            // More than the price counts; one atomic unit does not.
            TransactionRecord {
                amount: Some("20000".into()),
                ..settle(NOW - DAY_MS, url, "0xb1", "0xb1")
            },
            TransactionRecord {
                amount: Some("1".into()),
                ..settle(NOW - DAY_MS, url, "0xb2", "0xb2")
            },
            // Another asset, another scheme, no amount, an amount that is not
            // a number.
            TransactionRecord {
                asset: Some("0x00000000000000000000000000000000000000ee".into()),
                ..settle(NOW - DAY_MS, url, "0xb3", "0xb3")
            },
            TransactionRecord {
                scheme: Some("upto".into()),
                amount: Some("99999999".into()),
                ..settle(NOW - DAY_MS, url, "0xb4", "0xb4")
            },
            TransactionRecord {
                amount: None,
                ..settle(NOW - DAY_MS, url, "0xb5", "0xb5")
            },
            TransactionRecord {
                amount: Some("ten".into()),
                ..settle(NOW - DAY_MS, url, "0xb6", "0xb6")
            },
        ]);
        let tracker = UsageTracker::new();
        tracker.refresh(&s, NOW).await.unwrap();
        let snap = tracker.snapshot().unwrap();
        let u = snap.of(&listing(url)).unwrap();
        assert_eq!((u.calls_30d, u.unique_payers_30d), (3, 3), "{u:?}");
        // Under another scheme the amount recorded is a declared maximum, not
        // what moved: it is not compared.
        let mut upto = listing(url);
        upto.accepts[0].scheme = CatalogScheme::Unsupported("upto".to_string());
        let u = snap.of(&upto).unwrap();
        assert_eq!(u.calls_30d, 1, "{u:?}");
    }

    #[tokio::test]
    async fn a_later_read_starts_before_the_previous_one_and_counts_nothing_twice() {
        let s = store(vec![
            settle(NOW - DAY_MS, "https://a.example/x", "0x1", "0xa"),
            // Inside the margin: the next read reads it again.
            settle(NOW - 60_000, "https://a.example/x", "0x4", "0xd"),
        ]);
        let tracker = UsageTracker::new();
        assert_eq!(tracker.refresh(&s, NOW).await.unwrap(), Some(2));
        // Recorded by another task just before the first read, visible only
        // after it; and one recorded after it.
        s.records
            .lock()
            .unwrap()
            .push(settle(NOW - 30_000, "https://a.example/x", "0x2", "0xb"));
        s.records
            .lock()
            .unwrap()
            .push(settle(NOW + 1_000, "https://a.example/x", "0x3", "0xc"));
        assert_eq!(tracker.refresh(&s, NOW + 300_000).await.unwrap(), Some(2));
        let asked = s.asked.lock().unwrap().clone();
        assert_eq!(
            asked[0],
            NOW - USAGE_WINDOW_DAYS * DAY_MS,
            "the whole window first"
        );
        assert_eq!(
            asked[1],
            NOW - READ_MARGIN_MS,
            "then from the previous read, less a margin"
        );
        let u = tracker
            .snapshot()
            .unwrap()
            .of(&listing("https://a.example/x"))
            .unwrap();
        assert_eq!((u.calls_30d, u.unique_payers_30d), (4, 4));
        // A read that finds only what it already holds adds nothing.
        assert_eq!(tracker.refresh(&s, NOW + 310_000).await.unwrap(), Some(0));
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
            .of(&listing("https://a.example/x"))
            .unwrap();
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
        assert!(is_template(
            &url::Url::parse("https://a.example/item/%7Bid%7D").unwrap()
        ));
        assert!(!is_template(
            &url::Url::parse("https://a.example/item/7").unwrap()
        ));
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
