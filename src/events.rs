//! Real-time traffic event stream (`GET /events`, Server-Sent Events).
//!
//! Publishes one event per facilitator operation (`verify` / `settle`) so external
//! observers can render live traffic without scraping CloudWatch. Built for
//! KarmaCadabra's 3D observatory, but deliberately generic: this module knows nothing
//! about any particular client — filtering is an ADDRESS ALLOWLIST fed by env.
//!
//! # Hard invariant: the money path never blocks on the stream
//!
//! The bus is a `tokio::sync::broadcast` channel, which is *lossy by construction*:
//! when there are no subscribers, or a subscriber lags past the buffer, the event is
//! dropped silently. [`EventBus::publish`] returns `()` and never propagates an error,
//! so a settle handler cannot fail, stall or slow down because someone is watching.
//! Publishing is always the LAST thing a handler does, after the payment already
//! resolved.
//!
//! # Privacy dial
//!
//! `X402_EVENTS_DETAIL=full` streams `payer` / `tx` / `amount`; `minimal` streams only
//! `{ts, kind, network, ok}`. The facilitator serves many clients, so the operator can
//! dial exposure down without a code change (see `docs/plans/traffic-events-stream.md`).

use std::env;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::sync::broadcast;

const ENV_ENABLED: &str = "X402_EVENTS_ENABLED";
const ENV_SCOPE: &str = "X402_EVENTS_SCOPE";
const ENV_ALLOWLIST: &str = "X402_EVENTS_ALLOWLIST";
const ENV_DETAIL: &str = "X402_EVENTS_DETAIL";
const ENV_BUFFER: &str = "X402_EVENTS_BUFFER";
const ENV_MAX_SUBSCRIBERS: &str = "X402_EVENTS_MAX_SUBSCRIBERS";
const ENV_PUBLISH_FAILURES: &str = "X402_EVENTS_PUBLISH_FAILURES";

const DEFAULT_BUFFER: usize = 256;
/// Concurrent SSE subscribers allowed. `/events` is public and unauthenticated, and
/// every subscriber is a long-lived connection on the SAME task that settles payments,
/// so an uncapped stream is a way to starve the money path without ever touching it.
const DEFAULT_MAX_SUBSCRIBERS: usize = 64;

/// Epoch millis UTC — no date crate needed, and the dashboard already normalises it.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// How much of each event reaches subscribers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// `payer` / `tx` / `amount` / `asset` included.
    Full,
    /// Only `{ts, kind, network, ok}` — no counterparty, no hash, no amount.
    Minimal,
}

/// Which operations are streamed at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Every operation the facilitator handles.
    All,
    /// Only operations whose payer is in the allowlist (lowercased addresses).
    Allowlist(Arc<Vec<String>>),
}

/// One facilitator operation, as seen by an observer.
///
/// camelCase on the wire, like the rest of the x402 protocol. Every field except
/// `pay_to` is a single word, so this only affects that one — but getting it
/// wrong would ship `pay_to` next to `payTo` everywhere else, and consumers
/// would have to special-case us.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficEvent {
    /// Epoch milliseconds UTC.
    pub ts: u64,
    /// `"verify"` or `"settle"`.
    pub kind: &'static str,
    /// Network slug as the facilitator names it (`base`, `celo`, `skale`…).
    pub network: String,
    /// Did the operation succeed?
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
    /// The protected endpoint the payer is buying — `PaymentRequirements.resource`.
    ///
    /// Answers "what was bought", which amount alone never does. Note this is a
    /// real exposure step: it turns "someone paid 1 USDC on Base" into "this
    /// wallet bought THIS from THAT seller". Deliberate, and reversible without
    /// a deploy through `X402_EVENTS_DETAIL=minimal`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    /// The seller being paid — `PaymentRequirements.pay_to`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pay_to: Option<String>,
    /// Human-readable description the seller advertised.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Payment scheme: `exact`, `escrow`, `commerce`, `upto`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
    /// Why the operation failed, as a BOUNDED CATEGORY — never the error text.
    ///
    /// Present only on operations that errored, and only when the operator has
    /// enabled failure publishing. The category is a closed set precisely so
    /// this field cannot leak: raw error strings carry addresses and sometimes
    /// RPC URLs with the API key inside them, which is why `src/redact.rs`
    /// exists at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<&'static str>,
}

impl TrafficEvent {
    /// Strip everything that isn't `{ts, kind, network, ok}`.
    fn redacted(mut self) -> Self {
        self.payer = None;
        self.tx = None;
        self.amount = None;
        self.asset = None;
        // `resource` and `pay_to` identify WHAT was bought and FROM WHOM, which
        // is more revealing than the amount ever was. Minimal mode has to drop
        // them or the privacy dial stops meaning what it says.
        self.resource = None;
        self.pay_to = None;
        self.description = None;
        self.scheme = None;
        // `error` deliberately SURVIVES minimal mode. It is a closed-set category
        // with no counterparty data in it, and stripping it would leave minimal
        // mode unable to answer "is anything failing?" — the one question it is
        // still useful for.
        self
    }
}

/// Lossy fan-out of [`TrafficEvent`]s to SSE subscribers.
#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<TrafficEvent>,
    enabled: bool,
    scope: Scope,
    detail: Detail,
    max_subscribers: usize,
    publish_failures: bool,
}

impl EventBus {
    /// Build from env. Never fails: an unparseable value falls back to the safe default.
    pub fn from_env() -> Self {
        let enabled = !matches!(
            env::var(ENV_ENABLED)
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "0" | "false" | "no" | "off"
        );
        let detail = match env::var(ENV_DETAIL)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "minimal" | "min" => Detail::Minimal,
            _ => Detail::Full,
        };
        let scope = match env::var(ENV_SCOPE)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "allowlist" | "allow" => {
                let list: Vec<String> = env::var(ENV_ALLOWLIST)
                    .unwrap_or_default()
                    .split(',')
                    .map(|s| s.trim().to_ascii_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect();
                Scope::Allowlist(Arc::new(list))
            }
            _ => Scope::All,
        };
        let buffer = env::var(ENV_BUFFER)
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_BUFFER);
        let max_subscribers = env::var(ENV_MAX_SUBSCRIBERS)
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_MAX_SUBSCRIBERS);

        // Default OFF: turning this on widens what a public, unauthenticated
        // stream broadcasts, so it has to be a decision someone makes rather
        // than one they inherit from an upgrade.
        let publish_failures = matches!(
            env::var(ENV_PUBLISH_FAILURES)
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "1" | "true" | "yes" | "on"
        );

        let (tx, _rx) = broadcast::channel(buffer);
        Self {
            tx,
            enabled,
            scope,
            detail,
            max_subscribers,
            publish_failures,
        }
    }

    /// Are operations that ERRORED published?
    ///
    /// When false, `ok:false` can only ever mean "resolved and came back
    /// negative", never "blew up" — so a 100% success rate means "no failures
    /// were recorded", which is a weaker claim than it looks.
    pub fn publish_failures(&self) -> bool {
        self.publish_failures
    }

    /// Is the stream serving at all? `false` → `/events` 404s and nothing is published.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// A new SSE subscriber, or `None` when the stream is already at capacity.
    ///
    /// Lagging subscribers lose messages, never block producers. The cap is the other
    /// half of that promise: `publish()` cannot be slowed by an observer, but an
    /// unbounded number of observers could still exhaust the task that settles
    /// payments, so admission is bounded here (`X402_EVENTS_MAX_SUBSCRIBERS`).
    ///
    /// Deliberately a SOFT cap: two connections racing the check can both pass, which
    /// overshoots by a handful — never by an order of magnitude, which is what matters.
    pub fn try_subscribe(&self) -> Option<broadcast::Receiver<TrafficEvent>> {
        if self.tx.receiver_count() >= self.max_subscribers {
            return None;
        }
        Some(self.tx.subscribe())
    }

    /// Uncapped subscribe. Tests only — production admission goes through
    /// [`EventBus::try_subscribe`] so the cap cannot be bypassed by accident.
    #[cfg(test)]
    fn subscribe(&self) -> broadcast::Receiver<TrafficEvent> {
        self.tx.subscribe()
    }

    /// Concurrent subscribers allowed before `/events` starts shedding connections.
    pub fn max_subscribers(&self) -> usize {
        self.max_subscribers
    }

    /// Current subscriber count (for metrics / diagnostics).
    pub fn subscribers(&self) -> usize {
        self.tx.receiver_count()
    }

    fn passes_scope(&self, ev: &TrafficEvent) -> bool {
        match &self.scope {
            Scope::All => true,
            Scope::Allowlist(list) => match &ev.payer {
                // No payer to match against → cannot prove it belongs to the allowlist.
                // Fail CLOSED: an allowlist that leaks everything is not an allowlist.
                None => false,
                Some(p) => {
                    let p = p.to_ascii_lowercase();
                    list.iter().any(|a| *a == p)
                }
            },
        }
    }

    /// Publish one event. Best-effort and infallible BY DESIGN — see module docs.
    /// Never call this before the operation it describes has fully resolved.
    pub fn publish(&self, ev: TrafficEvent) {
        if !self.enabled || !self.passes_scope(&ev) {
            return;
        }
        let ev = match self.detail {
            Detail::Full => ev,
            Detail::Minimal => ev.redacted(),
        };
        // `send` errors only when there are zero receivers — that is the normal case
        // (nobody watching) and explicitly not a problem. Drop it.
        let _ = self.tx.send(ev);
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::from_env()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(payer: Option<&str>) -> TrafficEvent {
        TrafficEvent {
            ts: 1_769_000_000_000,
            kind: "settle",
            network: "base".into(),
            ok: true,
            payer: payer.map(|s| s.to_string()),
            tx: Some("0xdeadbeef".into()),
            amount: Some("0.02".into()),
            asset: Some("usdc".into()),
            error: None,
            resource: Some("https://api.example.com/thing".into()),
            pay_to: Some("0xseller".into()),
            description: Some("A thing".into()),
            scheme: Some("exact".into()),
        }
    }

    fn bus(enabled: bool, scope: Scope, detail: Detail) -> EventBus {
        bus_capped(enabled, scope, detail, DEFAULT_MAX_SUBSCRIBERS)
    }

    fn bus_capped(enabled: bool, scope: Scope, detail: Detail, max_subscribers: usize) -> EventBus {
        let (tx, _rx) = broadcast::channel(16);
        EventBus {
            tx,
            enabled,
            scope,
            detail,
            max_subscribers,
            publish_failures: false,
        }
    }

    /// The money path must survive publishing into the void.
    #[test]
    fn publish_without_subscribers_is_a_noop_not_an_error() {
        let b = bus(true, Scope::All, Detail::Full);
        b.publish(ev(Some("0xabc"))); // must not panic
        assert_eq!(b.subscribers(), 0);
    }

    #[test]
    fn disabled_bus_publishes_nothing() {
        let b = bus(false, Scope::All, Detail::Full);
        let mut rx = b.subscribe();
        b.publish(ev(Some("0xabc")));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn allowlist_matches_case_insensitively_and_excludes_others() {
        let list = Arc::new(vec!["0xaaa".to_string()]);
        let b = bus(true, Scope::Allowlist(list), Detail::Full);
        let mut rx = b.subscribe();

        b.publish(ev(Some("0xAAA"))); // same address, different case
        assert_eq!(
            rx.try_recv().unwrap().payer.unwrap().to_ascii_lowercase(),
            "0xaaa"
        );

        b.publish(ev(Some("0xbbb"))); // not in the list
        assert!(rx.try_recv().is_err());
    }

    /// An allowlist that lets through what it cannot identify is not an allowlist.
    #[test]
    fn allowlist_fails_closed_when_there_is_no_payer() {
        let b = bus(
            true,
            Scope::Allowlist(Arc::new(vec!["0xaaa".into()])),
            Detail::Full,
        );
        let mut rx = b.subscribe();
        b.publish(ev(None));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn minimal_detail_strips_payer_tx_and_amount() {
        let b = bus(true, Scope::All, Detail::Minimal);
        let mut rx = b.subscribe();
        b.publish(ev(Some("0xabc")));
        let got = rx.try_recv().unwrap();
        assert!(
            got.payer.is_none() && got.tx.is_none() && got.amount.is_none() && got.asset.is_none()
        );
        // the shape that makes the wave still survives
        assert_eq!(
            (got.kind, got.network.as_str(), got.ok),
            ("settle", "base", true)
        );
    }

    /// The wire contract, asserted on the serialised form rather than the struct:
    /// consumers read JSON, not Rust field names.
    #[test]
    fn serialises_camelcase_and_omits_absent_fields() {
        let json = serde_json::to_string(&ev(Some("0xabc"))).expect("serialises");
        assert!(
            json.contains("\"payTo\""),
            "pay_to must go out as payTo: {json}"
        );
        assert!(
            !json.contains("pay_to"),
            "snake_case leaked to the wire: {json}"
        );

        let mut bare = ev(None);
        bare.resource = None;
        bare.pay_to = None;
        let json = serde_json::to_string(&bare).expect("serialises");
        // Absent means ABSENT, never null — a consumer checking `"payTo" in ev`
        // must not see a key whose value is null.
        assert!(
            !json.contains("payTo"),
            "absent field emitted as null: {json}"
        );
    }

    /// The category, not the message. Raw error text carries addresses and
    /// sometimes RPC URLs with keys inside; redact.rs exists because that
    /// already leaked once.
    #[test]
    fn error_category_is_a_closed_set_never_free_text() {
        for variant in [
            "rpc_error",
            "invalid_signature",
            "insufficient_funds",
            "contract_revert",
            "invalid_timing",
            "blocked_address",
            "unsupported_network",
            "other",
        ] {
            assert!(
                !variant.contains("0x"),
                "a category must never embed an address"
            );
            assert!(
                !variant.contains("http"),
                "a category must never embed a URL"
            );
        }
    }

    /// Minimal mode strips identity, not health. Dropping `error` too would
    /// leave minimal unable to answer the one question it is still good for.
    #[test]
    fn minimal_keeps_the_error_category() {
        let b = bus(true, Scope::All, Detail::Minimal);
        let mut rx = b.subscribe();
        let mut e = ev(Some("0xabc"));
        e.ok = false;
        e.error = Some("rpc_error");
        b.publish(e);
        let got = rx.try_recv().unwrap();
        assert_eq!(got.error, Some("rpc_error"));
        assert!(got.payer.is_none(), "identity must still be stripped");
    }

    #[test]
    fn publishing_failures_is_off_unless_asked() {
        // Enabling it widens what a public stream broadcasts, so it must never
        // arrive as a side effect of an upgrade.
        assert!(!EventBus::from_env().publish_failures());
    }

    #[test]
    fn from_env_defaults_to_enabled_all_full() {
        // No env set in this process → the documented defaults.
        let b = EventBus::from_env();
        assert!(b.enabled());
        assert_eq!(b.scope, Scope::All);
        assert_eq!(b.detail, Detail::Full);
        assert_eq!(b.max_subscribers(), DEFAULT_MAX_SUBSCRIBERS);
    }

    /// An unauthenticated public endpoint must not accept unbounded connections on the
    /// same task that settles payments.
    #[test]
    fn try_subscribe_stops_admitting_at_the_cap() {
        let b = bus_capped(true, Scope::All, Detail::Full, 2);
        let _a = b.try_subscribe().expect("first subscriber admitted");
        let _c = b.try_subscribe().expect("second subscriber admitted");
        assert!(
            b.try_subscribe().is_none(),
            "third must be shed, not queued"
        );
        assert_eq!(b.subscribers(), 2);
    }

    /// Shedding is not a permanent close: a freed slot must be reusable, or one burst
    /// of connections would take the stream down until the next deploy.
    #[test]
    fn a_dropped_subscriber_frees_its_slot() {
        let b = bus_capped(true, Scope::All, Detail::Full, 1);
        let first = b.try_subscribe().expect("first subscriber admitted");
        assert!(b.try_subscribe().is_none());
        drop(first);
        assert!(b.try_subscribe().is_some(), "the slot must come back");
    }

    /// The cap bounds observers, never producers: publishing at capacity is still a
    /// no-op for the money path.
    #[test]
    fn publishing_while_at_capacity_still_cannot_fail() {
        let b = bus_capped(true, Scope::All, Detail::Full, 1);
        let mut rx = b.try_subscribe().unwrap();
        assert!(b.try_subscribe().is_none());
        b.publish(ev(Some("0xabc"))); // must not panic
        assert_eq!(rx.try_recv().unwrap().kind, "settle");
    }
}
