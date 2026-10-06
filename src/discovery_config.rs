//! Every tunable the Bazaar has, defined once.
//!
//! # Why this module exists
//!
//! By 2.23.0 the discovery subsystem read seventeen environment variables from
//! eleven places. Each one was a `std::env::var` inline at its point of use,
//! with its default written next to it, which has three consequences that only
//! look small until an incident:
//!
//! * the same parameter can be read with two different defaults in two files,
//!   and nothing says so;
//! * there is no way to ask a *running* task what it actually resolved, so an
//!   incident is diagnosed against the values someone believes are set;
//! * a reader looking for "what governs background load" has to grep.
//!
//! So a parameter is declared here, once, with its default and a sentence about
//! what it costs. Call sites ask this module. `GET /discovery/config` publishes
//! the resolved values, which is what makes the second bullet false.
//!
//! # What may live here
//!
//! Numbers and switches. **No secrets, no endpoints carrying credentials**: this
//! module's whole purpose is that its contents can be served to anyone who asks,
//! so anything that could not be published must not be defined here.

use serde_json::json;

/// Read a `u64` from the environment, or fall back.
fn num(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

/// The same, refusing zero — for parameters where zero means "never" and that
/// would be a footgun rather than a setting.
fn positive(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(default)
}

fn flag(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(v) => !(v.eq_ignore_ascii_case("false") || v == "0"),
        Err(_) => default,
    }
}

// ============================================================================
// Catalog size. Set by the 2026-09-10 incident; see `discovery::enforce_capacity`.
// ============================================================================

/// Records the in-memory catalog will hold. ~22 KB of RSS each, measured.
pub fn max_resources() -> usize {
    num("DISCOVERY_MAX_RESOURCES", 2_000) as usize
}

/// Items pulled from ONE source in one aggregation cycle.
pub fn max_items_per_source() -> usize {
    positive("DISCOVERY_MAX_ITEMS_PER_SOURCE", 1_000) as usize
}

/// Sources the aggregator reads PAST `max_items_per_source`, keeping only the
/// copies of listings the catalog already holds (comma-separated ids; empty
/// disables). Nothing new enters this way: it is how a listing held from one
/// feed is completed and kept current from another feed that carries the same
/// URL further down its pages.
///
/// Coinbase alone by default, measured 2026-10-04: its feed publishes 32 701
/// resources, every one of the 144 listings held from thirdweb is in it --
/// none in the first 1 000 -- and its copies carry the description for 139 of
/// them and the input schema for all 144, which the thirdweb copies do not.
pub fn scan_sources() -> Vec<String> {
    let raw = std::env::var("DISCOVERY_SCAN_SOURCES").unwrap_or_else(|_| "coinbase".to_string());
    raw.split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Pages one scanned source is read past the cap in one cycle, resuming where
/// the last cycle stopped and wrapping at the end of the feed. `0` disables the
/// scan. Bounded at 16 -- half of Coinbase's feed, ~54 MB -- so a typo cannot
/// turn one cycle into a crawl of the whole feed.
///
/// 8 pages of 1 000 cover Coinbase's 31 700 resources past the cap in four
/// hourly cycles, for ~27 MB of transfer a cycle.
pub fn scan_pages_per_cycle() -> usize {
    num("DISCOVERY_SCAN_PAGES_PER_CYCLE", 8).min(16) as usize
}

/// Items asked for per scanned page. At most 1 000, the largest page measured
/// to be honoured (3.4 MB from Coinbase): one page is held in memory at a time,
/// and nothing of it but the copies of held listings outlives it.
pub fn scan_page_size() -> usize {
    positive("DISCOVERY_SCAN_PAGE_SIZE", 1_000).min(1_000) as usize
}

/// Share of the catalog a host may hold before its copies are the ones a FULL
/// catalog evicts first, in percent. `0` or `100` disables the rule.
///
/// An order of eviction, not a second cap: a catalog with room keeps whatever
/// it is given, and a first-hand record is never evicted for it
/// (`discovery::eviction_order`). Four hosts held 1 043 of the 2 000 slots on
/// 2026-10-01, most of them templated families and paid essays, while whole
/// kinds of service had no listing at all. 5 % is 100 listings per host at the
/// default cap: room for every API we know of to list all of its endpoints,
/// and twenty hosts before anyone is crowded out. On a copy of that catalog
/// every host above 100 had registered first-hand, so the share frees nothing
/// there yet -- collapsing templated families does -- and it stays as the rung
/// for an aggregated host that crowds without a template
/// (`docs/plans/bazaar/10-search-ranking-and-host-share.md`; fixture:
/// `a_full_catalog_makes_room_from_families_and_crowded_hosts_first`).
pub fn max_host_share_percent() -> u64 {
    num("DISCOVERY_MAX_HOST_SHARE_PCT", 5).min(100)
}

/// No host is held to fewer listings than this, whatever the share works out
/// to. A share of a small catalog rounds to a handful of records, and cutting
/// one API's endpoint set in half is not what the rule is for.
pub const MIN_PER_HOST: usize = 50;

/// Records one host may hold in a full catalog of `cap` before its copies go
/// first, or `None` when the rule is off (`cap` of 0, or a share of 0 or 100).
pub fn max_per_host(cap: usize) -> Option<usize> {
    let pct = max_host_share_percent();
    if cap == 0 || pct == 0 || pct >= 100 {
        return None;
    }
    let share = (cap as u64).saturating_mul(pct).div_ceil(100) as usize;
    Some(share.max(MIN_PER_HOST))
}

// ============================================================================
// Health prober. Every probe is a TLS handshake, so these are CPU.
// ============================================================================

/// Seconds between prober wake-ups.
pub fn health_tick_secs() -> u64 {
    positive("DISCOVERY_HEALTH_TICK", 60)
}

/// Probes issued per second, averaged over a tick. The per-tick budget is this
/// times the tick, and it is the number this phase must not raise.
pub fn health_max_rps() -> u64 {
    positive("DISCOVERY_HEALTH_MAX_RPS", 2)
}

/// Probes in flight at once.
pub fn health_concurrency() -> usize {
    positive("DISCOVERY_HEALTH_CONCURRENCY", 8) as usize
}

/// The whole per-tick probe allowance, demand and periodic together.
pub fn health_budget_per_tick() -> usize {
    (health_max_rps().saturating_mul(health_tick_secs())).max(1) as usize
}

/// Minimum seconds between uploads of the liveness overlay.
pub fn health_persist_secs() -> u64 {
    num("DISCOVERY_HEALTH_PERSIST_SECS", 300)
}

// ============================================================================
// Revalidation (P2)
// ============================================================================

/// Share of the per-tick budget reserved for the periodic sweep.
///
/// Percent. The demand queue may spend the rest. Without a floor a permanently
/// busy resource would hold the whole allowance and the long tail would never be
/// probed again -- which is the starvation the annex names, and it is worse than
/// a slow refresh because nothing reports it.
pub fn long_tail_share() -> u64 {
    num("DISCOVERY_REVALIDATION_LONG_TAIL_PCT", 40).min(100)
}

/// Coalescing window. Repeat requests for the same resource inside it are folded
/// into the one job.
pub fn revalidation_window_secs() -> u64 {
    positive("DISCOVERY_REVALIDATION_WINDOW", 300)
}

/// Most resources the demand queue will hold.
pub fn revalidation_queue_cap() -> usize {
    positive("DISCOVERY_REVALIDATION_QUEUE_CAP", 500) as usize
}

/// Most demand counts one entry can accumulate before it stops mattering.
pub fn revalidation_demand_cap() -> u32 {
    positive("DISCOVERY_REVALIDATION_DEMAND_CAP", 50) as u32
}

/// Probes one host may receive from the demand queue in one tick.
pub fn revalidation_per_host_per_tick() -> usize {
    positive("DISCOVERY_REVALIDATION_PER_HOST", 2) as usize
}

/// First backoff step after an origin refuses. Doubles per strike.
pub fn revalidation_base_backoff_secs() -> u64 {
    positive("DISCOVERY_REVALIDATION_BACKOFF_BASE", 60)
}

/// Ceiling for any backoff, including one an origin asked for.
pub fn revalidation_max_backoff_secs() -> u64 {
    positive("DISCOVERY_REVALIDATION_BACKOFF_MAX", 3_600)
}

/// Entries the owner claims from the shared queue per tick.
pub fn revalidation_claim_max() -> usize {
    positive("DISCOVERY_REVALIDATION_CLAIM_MAX", 100) as usize
}

/// Most URLs the shared DynamoDB set will hold.
pub fn revalidation_shared_cap() -> usize {
    positive("DISCOVERY_REVALIDATION_SHARED_CAP", 500) as usize
}

/// TTL on the shared queue item, so a queue nobody drains disappears.
pub fn revalidation_shared_ttl_secs() -> u64 {
    positive("DISCOVERY_REVALIDATION_SHARED_TTL", 3_600)
}

/// Whether demand-driven revalidation runs at all. Kill-switch, default ON.
///
/// Off means the prober does only its periodic sweep, which is 2.21.2's
/// behaviour exactly. It is the remedy if this phase ever misbehaves in
/// production, and it does not need a deploy.
pub fn revalidation_enabled() -> bool {
    flag("DISCOVERY_ENABLE_REVALIDATION", true)
}

// ============================================================================
// Observed terms overlay (P1)
// ============================================================================

pub fn terms_fresh_secs() -> u64 {
    positive("DISCOVERY_TERMS_FRESH_SECS", 7 * 24 * 3600)
}

pub fn terms_persist_secs() -> u64 {
    num("DISCOVERY_TERMS_PERSIST_SECS", 300)
}

pub fn terms_max_records() -> usize {
    positive("DISCOVERY_TERMS_MAX_RECORDS", 2_000) as usize
}

// ============================================================================
// The published view
// ============================================================================

/// Every resolved value, as JSON.
///
/// Grouped by what each group costs, because that is the question somebody has
/// when they open it during an incident.
pub fn effective() -> serde_json::Value {
    json!({
        "catalog": {
            "maxResources": max_resources(),
            "maxItemsPerSource": max_items_per_source(),
            "maxHostSharePercent": max_host_share_percent(),
            "maxPerHost": max_per_host(max_resources()),
            "scanPastCap": {
                "sources": scan_sources(),
                "pagesPerCycle": scan_pages_per_cycle(),
                "pageSize": scan_page_size(),
            },
        },
        "healthProber": {
            "tickSeconds": health_tick_secs(),
            "maxRps": health_max_rps(),
            "concurrency": health_concurrency(),
            "budgetPerTick": health_budget_per_tick(),
            "overlayPersistSeconds": health_persist_secs(),
        },
        "revalidation": {
            "enabled": revalidation_enabled(),
            "longTailSharePercent": long_tail_share(),
            "coalesceWindowSeconds": revalidation_window_secs(),
            "queueCap": revalidation_queue_cap(),
            "demandCap": revalidation_demand_cap(),
            "perHostPerTick": revalidation_per_host_per_tick(),
            "backoffBaseSeconds": revalidation_base_backoff_secs(),
            "backoffMaxSeconds": revalidation_max_backoff_secs(),
            "sharedClaimMax": revalidation_claim_max(),
            "sharedCap": revalidation_shared_cap(),
            "sharedTtlSeconds": revalidation_shared_ttl_secs(),
        },
        "observedTerms": {
            "freshnessWindowSeconds": terms_fresh_secs(),
            "overlayPersistSeconds": terms_persist_secs(),
            "maxRecords": terms_max_records(),
        },
        // The `/bazaar` page reads its `q` cap here instead of typing one.
        "search": {
            "maxQueryChars": crate::discovery_search::MAX_QUERY_CHARS,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_published_view_lists_every_group() {
        let v = effective();
        for group in [
            "catalog",
            "healthProber",
            "revalidation",
            "observedTerms",
            "search",
        ] {
            assert!(
                v.get(group).is_some(),
                "{group} missing from /discovery/config"
            );
        }
    }

    #[test]
    fn the_per_tick_budget_is_the_product_of_the_two_knobs() {
        // The invariant this phase rests on: demand refresh spends the SAME
        // allowance the periodic sweep had, never an extra one.
        assert_eq!(
            health_budget_per_tick(),
            (health_max_rps() * health_tick_secs()) as usize
        );
    }

    #[test]
    fn the_long_tail_keeps_a_reserved_share() {
        let share = long_tail_share();
        assert!(
            share > 0,
            "a reserved share of zero is starvation by default"
        );
        assert!(share <= 100);
    }

    #[test]
    fn defaults_match_what_the_incident_settled_on() {
        // 2.21.2's numbers are load-bearing: they are what took production from
        // 57 % memory and 5 s reads back to 18 % and 0,03 s. A change here is a
        // change to the thing that ended the incident.
        assert_eq!(max_resources(), 2_000);
        assert_eq!(max_items_per_source(), 1_000);
        assert_eq!(health_max_rps(), 2);
        assert_eq!(health_concurrency(), 8);
    }

    #[test]
    fn the_scan_past_the_cap_is_coinbase_eight_pages_of_a_thousand() {
        assert_eq!(scan_sources(), ["coinbase"]);
        assert_eq!(scan_pages_per_cycle(), 8);
        assert_eq!(scan_page_size(), 1_000);
        let published = effective();
        assert_eq!(
            published["catalog"]["scanPastCap"],
            json!({"sources": ["coinbase"], "pagesPerCycle": 8, "pageSize": 1000})
        );
    }

    #[test]
    fn the_host_share_is_a_share_of_the_cap_with_a_floor() {
        assert_eq!(max_host_share_percent(), 5);
        assert_eq!(max_per_host(2_000), Some(100));
        assert_eq!(max_per_host(10_000), Some(500));
        // A share of a small catalog never cuts a host below the floor.
        assert_eq!(max_per_host(10), Some(MIN_PER_HOST));
        // No cap, no share of it.
        assert_eq!(max_per_host(0), None);
        let published = effective();
        assert_eq!(published["catalog"]["maxPerHost"], 100);
        assert_eq!(published["catalog"]["maxHostSharePercent"], 5);
    }

    /// The `q` cap is defined once (`MAX_QUERY_CHARS`, which the handler
    /// enforces); the `/bazaar` page reads it from here and types none of its
    /// own, so the page and the server cannot disagree about it again.
    #[test]
    fn the_bazaar_page_reads_the_query_cap_it_does_not_type_one() {
        assert_eq!(
            effective()["search"]["maxQueryChars"],
            crate::discovery_search::MAX_QUERY_CHARS
        );
        let page = include_str!("../static/bazaar.html");
        assert!(
            page.contains("search.maxQueryChars"),
            "the page must read the cap from /discovery/config"
        );
        assert!(
            !page.contains("slice(0, 128)") && !page.contains("slice(0, 400)"),
            "the page must not cut `q` at a number of its own"
        );
    }
}
