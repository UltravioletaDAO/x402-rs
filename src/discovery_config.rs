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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_published_view_lists_every_group() {
        let v = effective();
        for group in ["catalog", "healthProber", "revalidation", "observedTerms"] {
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
}
