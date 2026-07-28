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

use serde::Serialize;
use tokio::sync::broadcast;

const ENV_ENABLED: &str = "X402_EVENTS_ENABLED";
const ENV_SCOPE: &str = "X402_EVENTS_SCOPE";
const ENV_ALLOWLIST: &str = "X402_EVENTS_ALLOWLIST";
const ENV_DETAIL: &str = "X402_EVENTS_DETAIL";
const ENV_BUFFER: &str = "X402_EVENTS_BUFFER";

const DEFAULT_BUFFER: usize = 256;

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
#[derive(Debug, Clone, Serialize)]
pub struct TrafficEvent {
    /// RFC3339 UTC.
    pub ts: String,
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
}

impl TrafficEvent {
    /// Strip everything that isn't `{ts, kind, network, ok}`.
    fn redacted(mut self) -> Self {
        self.payer = None;
        self.tx = None;
        self.amount = None;
        self.asset = None;
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
}

impl EventBus {
    /// Build from env. Never fails: an unparseable value falls back to the safe default.
    pub fn from_env() -> Self {
        let enabled = !matches!(
            env::var(ENV_ENABLED).unwrap_or_default().trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        );
        let detail = match env::var(ENV_DETAIL).unwrap_or_default().trim().to_ascii_lowercase().as_str() {
            "minimal" | "min" => Detail::Minimal,
            _ => Detail::Full,
        };
        let scope = match env::var(ENV_SCOPE).unwrap_or_default().trim().to_ascii_lowercase().as_str() {
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

        let (tx, _rx) = broadcast::channel(buffer);
        Self { tx, enabled, scope, detail }
    }

    /// Is the stream serving at all? `false` → `/events` 404s and nothing is published.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// A new SSE subscriber. Lagging subscribers lose messages, never block producers.
    pub fn subscribe(&self) -> broadcast::Receiver<TrafficEvent> {
        self.tx.subscribe()
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
            ts: "2026-07-28T00:00:00Z".into(),
            kind: "settle",
            network: "base".into(),
            ok: true,
            payer: payer.map(|s| s.to_string()),
            tx: Some("0xdeadbeef".into()),
            amount: Some("0.02".into()),
            asset: Some("usdc".into()),
        }
    }

    fn bus(enabled: bool, scope: Scope, detail: Detail) -> EventBus {
        let (tx, _rx) = broadcast::channel(16);
        EventBus { tx, enabled, scope, detail }
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
        assert_eq!(rx.try_recv().unwrap().payer.unwrap().to_ascii_lowercase(), "0xaaa");

        b.publish(ev(Some("0xbbb"))); // not in the list
        assert!(rx.try_recv().is_err());
    }

    /// An allowlist that lets through what it cannot identify is not an allowlist.
    #[test]
    fn allowlist_fails_closed_when_there_is_no_payer() {
        let b = bus(true, Scope::Allowlist(Arc::new(vec!["0xaaa".into()])), Detail::Full);
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
        assert!(got.payer.is_none() && got.tx.is_none() && got.amount.is_none() && got.asset.is_none());
        // the shape that makes the wave still survives
        assert_eq!((got.kind, got.network.as_str(), got.ok), ("settle", "base", true));
    }

    #[test]
    fn from_env_defaults_to_enabled_all_full() {
        // No env set in this process → the documented defaults.
        let b = EventBus::from_env();
        assert!(b.enabled());
        assert_eq!(b.scope, Scope::All);
        assert_eq!(b.detail, Detail::Full);
    }
}
