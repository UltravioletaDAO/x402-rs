//! A per-network daily limit on the ERC-8004 writes that send a transaction.
//!
//! Each task counts, per network and per UTC day, the writes that reached the
//! point of broadcasting a transaction. Once a network's count reaches its
//! limit, further writes to that network are answered 429 with a
//! `Retry-After` that runs to 00:00 UTC, before any work is done and without
//! touching the chain. The count starts over at 00:00 UTC.
//!
//! # What counts
//!
//! A write takes a slot when it arrives ([`enforce`]) and keeps it only if a
//! transaction actually went out: the send primitives call [`mark_sent`] at
//! the moment they broadcast (`chain::evm::send_call_estimated`,
//! `EvmProvider::send_transaction_from`, and the ERC-8004 Solana senders). A
//! write refused before that -- a malformed body, a failed check, a gas
//! estimate that reverts, a Solana preflight that fails -- gives its slot back
//! when it finishes, so the limit counts transactions, not requests.
//!
//! The slot travels in a task-local, so the primitives need no new parameter,
//! and it outlives the response when the work does: an asynchronous
//! registration carries it into the task that mints ([`scope`]).
//!
//! # Configuration
//!
//! `ERC8004_DAILY_WRITE_CAP` sets the limit for every network without one of
//! its own (default [`DEFAULT_DAILY_WRITE_CAP`]). `ERC8004_DAILY_WRITE_CAP_<NETWORK>`
//! sets one network's, where `<NETWORK>` is its v1 name in upper case with `-`
//! as `_` (`ERC8004_DAILY_WRITE_CAP_ARC`, `ERC8004_DAILY_WRITE_CAP_BASE_SEPOLIA`).
//! A variable overrides the built-in value for that network. `0` refuses every
//! write on the network; a value that does not parse is ignored with a warning.
//! `ENABLE_ERC8004_WRITES=false` remains the switch that turns every write off.
//!
//! The counters live in memory, one set per task: a task that starts, or that
//! takes over the writer lease, starts from zero.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use tracing::{info, warn};

use crate::network::Network;

/// The limit for a network with none of its own.
pub const ENV_DAILY_WRITE_CAP: &str = "ERC8004_DAILY_WRITE_CAP";

/// Prefix of the per-network override, followed by the network's v1 name.
pub const ENV_DAILY_WRITE_CAP_PREFIX: &str = "ERC8004_DAILY_WRITE_CAP_";

/// Writes per network per UTC day for a network with no limit of its own.
pub const DEFAULT_DAILY_WRITE_CAP: u32 = 1000;

/// Built-in limits that differ from [`DEFAULT_DAILY_WRITE_CAP`]. Every value,
/// like the default, is several times the busiest day of write traffic
/// measured on that network; `arc` has none yet and starts low.
const BUILT_IN_DAILY_WRITE_CAPS: &[(Network, u32)] = &[
    (Network::Ethereum, 100),
    (Network::Solana, 150),
    (Network::Arc, 300),
];

const SECONDS_PER_DAY: u64 = 86_400;

/// Largest body read to find the network. The service's request body limit
/// is far below this; it only bounds what this layer buffers.
const MAX_BODY_BYTES: usize = 1024 * 1024;

tokio::task_local! {
    static SLOT: Arc<Slot>;
}

/// The process-wide limits, read from the environment on first use.
pub fn global() -> Arc<DailyWriteCap> {
    static GLOBAL: LazyLock<Arc<DailyWriteCap>> =
        LazyLock::new(|| Arc::new(DailyWriteCap::from_env()));
    Arc::clone(&GLOBAL)
}

/// Per-network daily counters and the limits they are held to.
pub struct DailyWriteCap {
    default_limit: u32,
    limits: HashMap<Network, u32>,
    counts: Mutex<HashMap<Network, DayCount>>,
    now: Box<dyn Fn() -> u64 + Send + Sync>,
}

#[derive(Clone, Copy)]
struct DayCount {
    day: u64,
    used: u32,
}

impl DailyWriteCap {
    /// Limits from the built-in values and the environment, on the wall clock.
    pub fn from_env() -> Self {
        let default_limit = match std::env::var(ENV_DAILY_WRITE_CAP) {
            Ok(raw) => parse_limit(ENV_DAILY_WRITE_CAP, &raw).unwrap_or(DEFAULT_DAILY_WRITE_CAP),
            Err(_) => DEFAULT_DAILY_WRITE_CAP,
        };
        let mut limits: HashMap<Network, u32> = BUILT_IN_DAILY_WRITE_CAPS.iter().copied().collect();
        for network in Network::variants() {
            let var = env_var_for(network);
            if let Ok(raw) = std::env::var(&var) {
                if let Some(limit) = parse_limit(&var, &raw) {
                    limits.insert(*network, limit);
                }
            }
        }
        let mut shown: Vec<String> = limits.iter().map(|(n, l)| format!("{n}={l}")).collect();
        shown.sort();
        info!(
            default = default_limit,
            per_network = %shown.join(","),
            "ERC-8004 daily write limit configured"
        );
        Self::new(default_limit, limits, Box::new(wall_clock_secs))
    }

    /// Explicit limits and clock, for tests.
    pub fn new(
        default_limit: u32,
        limits: HashMap<Network, u32>,
        now: Box<dyn Fn() -> u64 + Send + Sync>,
    ) -> Self {
        Self {
            default_limit,
            limits,
            counts: Mutex::new(HashMap::new()),
            now,
        }
    }

    /// The daily limit for `network`.
    pub fn limit(&self, network: &Network) -> u32 {
        self.limits
            .get(network)
            .copied()
            .unwrap_or(self.default_limit)
    }

    /// Writes counted for `network` today, reserved ones included.
    pub fn used_today(&self, network: &Network) -> u32 {
        let today = (self.now)() / SECONDS_PER_DAY;
        let counts = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        match counts.get(network) {
            Some(count) if count.day == today => count.used,
            _ => 0,
        }
    }

    /// Take a slot for one write to `network`, or the seconds until the limit
    /// resets.
    pub fn reserve(self: &Arc<Self>, network: Network) -> Result<Slot, u64> {
        let now = (self.now)();
        let today = now / SECONDS_PER_DAY;
        let limit = self.limit(&network);
        let mut counts = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        let count = counts.entry(network).or_insert(DayCount {
            day: today,
            used: 0,
        });
        if count.day != today {
            *count = DayCount {
                day: today,
                used: 0,
            };
        }
        if count.used >= limit {
            return Err(SECONDS_PER_DAY - now % SECONDS_PER_DAY);
        }
        count.used += 1;
        Ok(Slot {
            cap: Arc::clone(self),
            network,
            day: today,
            sent: AtomicBool::new(false),
        })
    }

    fn release(&self, network: Network, day: u64) {
        let mut counts = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(count) = counts.get_mut(&network) {
            // A slot taken yesterday is not today's to give back.
            if count.day == day && count.used > 0 {
                count.used -= 1;
            }
        }
    }
}

/// One write's place in its network's daily count. Given back when dropped
/// unless a transaction went out while it was held.
pub struct Slot {
    cap: Arc<DailyWriteCap>,
    network: Network,
    day: u64,
    sent: AtomicBool,
}

impl Drop for Slot {
    fn drop(&mut self) {
        if !self.sent.load(Ordering::Acquire) {
            self.cap.release(self.network, self.day);
        }
    }
}

/// Record that the write being served broadcast a transaction, so its slot
/// is kept. A no-op outside a capped write, which is every other caller of
/// the send primitives.
pub fn mark_sent() {
    let _ = SLOT.try_with(|slot| slot.sent.store(true, Ordering::Release));
}

/// The slot of the write being served, to hand to work that outlives it.
pub fn current() -> Option<Arc<Slot>> {
    SLOT.try_with(Arc::clone).ok()
}

/// Run `work` holding `slot`, so a send inside it keeps the slot and the slot
/// stays taken until `work` is done.
pub async fn scope<F: Future>(slot: Option<Arc<Slot>>, work: F) -> F::Output {
    match slot {
        Some(slot) => SLOT.scope(slot, work).await,
        None => work.await,
    }
}

/// The only field this layer reads. Every capped request carries it at the
/// top level, parsed with the same `Network` deserializer as the handler.
#[derive(Deserialize)]
struct NetworkField {
    network: Network,
}

/// Middleware for the write routes that send a transaction, on the process
/// limits. See [`enforce_with`].
pub async fn enforce(request: Request, next: Next) -> Response {
    enforce_with(State(global()), request, next).await
}

/// Take a slot for the request's network and serve it holding the slot, or
/// refuse it with 429 before the handler runs.
///
/// A body with no readable `network` is passed through untouched: the
/// handler parses the same field with the same type and refuses it.
pub async fn enforce_with(
    State(cap): State<Arc<DailyWriteCap>>,
    request: Request,
    next: Next,
) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let network = serde_json::from_slice::<NetworkField>(&bytes)
        .ok()
        .map(|field| field.network);
    let request = Request::from_parts(parts, Body::from(bytes));
    let Some(network) = network else {
        return next.run(request).await;
    };
    match cap.reserve(network) {
        Ok(slot) => SLOT.scope(Arc::new(slot), next.run(request)).await,
        Err(retry_after) => {
            warn!(
                network = %network,
                limit = cap.limit(&network),
                retry_after,
                "ERC-8004 daily write limit reached; write refused"
            );
            limit_reached(network, retry_after)
        }
    }
}

fn limit_reached(network: Network, retry_after: u64) -> Response {
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({
            "success": false,
            "error": "The daily ERC-8004 write limit for this network has been reached. It resets at 00:00 UTC.",
            "code": "erc8004_daily_write_limit",
            "network": network,
        })),
    )
        .into_response();
    response.headers_mut().insert(
        header::RETRY_AFTER,
        HeaderValue::from_str(&retry_after.to_string()).expect("digits are a valid header value"),
    );
    response
}

fn env_var_for(network: &Network) -> String {
    format!(
        "{ENV_DAILY_WRITE_CAP_PREFIX}{}",
        network.to_string().to_uppercase().replace('-', "_")
    )
}

fn parse_limit(var: &str, raw: &str) -> Option<u32> {
    match raw.trim().parse::<u32>() {
        Ok(limit) => Some(limit),
        Err(_) => {
            warn!(
                var,
                value = raw,
                "ignoring an ERC-8004 daily write limit that is not a number"
            );
            None
        }
    }
}

fn wall_clock_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::post;
    use axum::Router;
    use std::sync::atomic::{AtomicU64, AtomicUsize};
    use tower::ServiceExt;

    /// 2023-11-14T22:13:20Z: 6400 seconds before 00:00 UTC.
    const NOON_ISH: u64 = 1_700_000_000;

    fn cap_with(limits: &[(Network, u32)], clock: &Arc<AtomicU64>) -> Arc<DailyWriteCap> {
        let clock = Arc::clone(clock);
        Arc::new(DailyWriteCap::new(
            10,
            limits.iter().copied().collect(),
            Box::new(move || clock.load(Ordering::SeqCst)),
        ))
    }

    /// A write route whose handler counts every time it would touch the chain,
    /// and broadcasts (marks its slot sent) when `sends` is true.
    fn capped(cap: Arc<DailyWriteCap>, chain: Arc<AtomicUsize>, sends: bool) -> Router {
        Router::new()
            .route(
                "/feedback",
                post(move || {
                    let chain = Arc::clone(&chain);
                    async move {
                        chain.fetch_add(1, Ordering::SeqCst);
                        if sends {
                            mark_sent();
                        }
                        "written"
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(cap, enforce_with))
    }

    async fn write(router: &Router, body: &str) -> Response {
        let request = Request::builder()
            .method("POST")
            .uri("/feedback")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        router.clone().oneshot(request).await.unwrap()
    }

    const BASE: &str = r#"{"x402Version":1,"network":"base","feedback":{}}"#;
    const POLYGON: &str = r#"{"x402Version":1,"network":"polygon","feedback":{}}"#;

    /// The write past a network's limit is refused before its handler runs,
    /// with a `Retry-After` that reaches 00:00 UTC, and another network keeps
    /// writing.
    #[tokio::test]
    async fn the_write_past_the_limit_never_reaches_the_chain() {
        let clock = Arc::new(AtomicU64::new(NOON_ISH));
        let cap = cap_with(&[(Network::Base, 3)], &clock);
        let chain = Arc::new(AtomicUsize::new(0));
        let router = capped(Arc::clone(&cap), Arc::clone(&chain), true);

        for n in 1..=3 {
            assert_eq!(
                write(&router, BASE).await.status(),
                StatusCode::OK,
                "write {n}"
            );
        }
        assert_eq!(chain.load(Ordering::SeqCst), 3);

        let refused = write(&router, BASE).await;
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        let to_midnight = SECONDS_PER_DAY - NOON_ISH % SECONDS_PER_DAY;
        assert_eq!(
            refused.headers()[header::RETRY_AFTER],
            to_midnight.to_string().as_str()
        );
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(refused.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["code"], "erc8004_daily_write_limit");
        assert_eq!(body["network"], "base");
        assert_eq!(
            chain.load(Ordering::SeqCst),
            3,
            "the refused write reached the handler"
        );

        assert_eq!(write(&router, POLYGON).await.status(), StatusCode::OK);
        assert_eq!(chain.load(Ordering::SeqCst), 4);
        assert_eq!(cap.used_today(&Network::Base), 3);
        assert_eq!(cap.used_today(&Network::Polygon), 1);
    }

    /// A write that broadcasts nothing gives its slot back, so requests that
    /// never reach the chain cannot use up the day.
    #[tokio::test]
    async fn a_write_that_sends_nothing_gives_its_slot_back() {
        let clock = Arc::new(AtomicU64::new(NOON_ISH));
        let cap = cap_with(&[(Network::Base, 3)], &clock);
        let router = capped(Arc::clone(&cap), Arc::new(AtomicUsize::new(0)), false);
        for n in 1..=10 {
            assert_eq!(
                write(&router, BASE).await.status(),
                StatusCode::OK,
                "write {n}"
            );
        }
        assert_eq!(cap.used_today(&Network::Base), 0);
    }

    /// The count starts over at 00:00 UTC.
    #[tokio::test]
    async fn the_count_starts_over_at_midnight_utc() {
        let last_seconds = (NOON_ISH / SECONDS_PER_DAY + 1) * SECONDS_PER_DAY - 10;
        let clock = Arc::new(AtomicU64::new(last_seconds));
        let cap = cap_with(&[(Network::Base, 2)], &clock);
        let router = capped(Arc::clone(&cap), Arc::new(AtomicUsize::new(0)), true);

        write(&router, BASE).await;
        write(&router, BASE).await;
        let refused = write(&router, BASE).await;
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(refused.headers()[header::RETRY_AFTER], "10");

        clock.fetch_add(15, Ordering::SeqCst);
        assert_eq!(write(&router, BASE).await.status(), StatusCode::OK);
        assert_eq!(cap.used_today(&Network::Base), 1);
    }

    /// A slot taken before midnight and given back after it does not free a
    /// place in the new day.
    #[test]
    fn a_slot_from_yesterday_is_not_given_back_today() {
        let clock = Arc::new(AtomicU64::new(NOON_ISH));
        let cap = cap_with(&[(Network::Base, 2)], &clock);
        let yesterday = cap.reserve(Network::Base).unwrap();
        clock.fetch_add(SECONDS_PER_DAY, Ordering::SeqCst);
        let today = cap.reserve(Network::Base).unwrap();
        today.sent.store(true, Ordering::SeqCst);
        drop(yesterday);
        assert_eq!(cap.used_today(&Network::Base), 1);
    }

    /// Work that outlives the response -- an asynchronous registration --
    /// holds the slot until it finishes, and keeps it only if it sent.
    #[tokio::test]
    async fn work_after_the_response_holds_the_slot_until_it_is_done() {
        for sends in [true, false] {
            let clock = Arc::new(AtomicU64::new(NOON_ISH));
            let cap = cap_with(&[(Network::Base, 5)], &clock);
            let (go, wait) = tokio::sync::oneshot::channel::<()>();
            let (done, finished) = tokio::sync::oneshot::channel::<()>();
            let hand_off = Arc::new(Mutex::new(Some((wait, done))));
            let router = Router::new()
                .route(
                    "/feedback",
                    post(move || {
                        let (wait, done) = hand_off.lock().unwrap().take().unwrap();
                        async move {
                            let slot = current();
                            tokio::spawn(scope(slot, async move {
                                wait.await.unwrap();
                                if sends {
                                    mark_sent();
                                }
                                done.send(()).unwrap();
                            }));
                            StatusCode::ACCEPTED
                        }
                    }),
                )
                .layer(axum::middleware::from_fn_with_state(
                    Arc::clone(&cap),
                    enforce_with,
                ));

            assert_eq!(write(&router, BASE).await.status(), StatusCode::ACCEPTED);
            assert_eq!(
                cap.used_today(&Network::Base),
                1,
                "the slot left with the response"
            );
            go.send(()).unwrap();
            finished.await.unwrap();
            // The spawned task drops its slot as it ends.
            for _ in 0..100 {
                if cap.used_today(&Network::Base) == u32::from(sends) {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert_eq!(
                cap.used_today(&Network::Base),
                u32::from(sends),
                "sends: {sends}"
            );
        }
    }

    /// A body with no readable `network` reaches the handler, which refuses
    /// it, and counts against nothing.
    #[tokio::test]
    async fn a_body_without_a_network_is_left_to_the_handler() {
        let clock = Arc::new(AtomicU64::new(NOON_ISH));
        let cap = cap_with(&[], &clock);
        let chain = Arc::new(AtomicUsize::new(0));
        let router = capped(Arc::clone(&cap), Arc::clone(&chain), false);
        for body in ["{}", "not json", r#"{"network":"no-such-chain"}"#] {
            assert_eq!(
                write(&router, body).await.status(),
                StatusCode::OK,
                "{body}"
            );
        }
        assert_eq!(chain.load(Ordering::SeqCst), 3);
    }

    /// Serialises the tests that set the process-global limit variables.
    static ENV: Mutex<()> = Mutex::new(());

    const VARS: [&str; 4] = [
        ENV_DAILY_WRITE_CAP,
        "ERC8004_DAILY_WRITE_CAP_ARC",
        "ERC8004_DAILY_WRITE_CAP_ETHEREUM",
        "ERC8004_DAILY_WRITE_CAP_BASE_SEPOLIA",
    ];

    /// A variable overrides the built-in limit of its network, the global one
    /// sets every network without its own, `0` refuses every write, and a
    /// value that does not parse leaves the built-in value in place.
    #[test]
    fn variables_override_the_built_in_limits() {
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
        for var in VARS {
            std::env::remove_var(var);
        }
        let built_in = DailyWriteCap::from_env();
        assert_eq!(built_in.limit(&Network::Base), DEFAULT_DAILY_WRITE_CAP);
        assert_eq!(built_in.limit(&Network::Arc), 300);
        assert_eq!(built_in.limit(&Network::Ethereum), 100);
        assert_eq!(built_in.limit(&Network::Solana), 150);

        std::env::set_var(ENV_DAILY_WRITE_CAP, "7");
        std::env::set_var("ERC8004_DAILY_WRITE_CAP_ARC", "5");
        std::env::set_var("ERC8004_DAILY_WRITE_CAP_ETHEREUM", "lots");
        std::env::set_var("ERC8004_DAILY_WRITE_CAP_BASE_SEPOLIA", "0");
        let configured = Arc::new(DailyWriteCap::from_env());
        for var in VARS {
            std::env::remove_var(var);
        }
        assert_eq!(configured.limit(&Network::Base), 7);
        assert_eq!(configured.limit(&Network::Arc), 5);
        assert_eq!(configured.limit(&Network::Ethereum), 100);
        assert_eq!(configured.limit(&Network::Solana), 150);
        assert_eq!(configured.limit(&Network::BaseSepolia), 0);
        assert!(configured.reserve(Network::BaseSepolia).is_err());
    }

    /// Every primitive that broadcasts an ERC-8004 write keeps its slot, the
    /// routes that send are the ones under the limit, and an asynchronous
    /// registration carries its slot. Read from source because none of it can
    /// run here without a chain.
    #[test]
    fn every_send_is_counted_and_every_sending_route_is_capped() {
        let body = |src: &str, signature: &str, end: &str| -> String {
            src.split(signature)
                .nth(1)
                .unwrap_or_else(|| panic!("`{signature}` must exist"))
                .split(end)
                .next()
                .unwrap()
                .to_string()
        };
        let evm = include_str!("../chain/evm.rs");
        let estimated = body(evm, "pub async fn send_call_estimated<", "\n}\n");
        assert_eq!(estimated.matches("daily_cap::mark_sent()").count(), 2);
        let from = body(evm, "pub async fn send_transaction_from(", "\n    }\n");
        assert!(
            from.contains("daily_cap::mark_sent()"),
            "send_transaction_from"
        );

        let solana = include_str!("solana.rs");
        for signature in [
            "pub async fn cosign_and_send(",
            "pub async fn send_erc8004_transaction(",
            "pub async fn send_erc8004_transaction_with_signers(",
        ] {
            assert!(
                body(solana, signature, "\n}\n").contains("keep_slot_unless_preflight_failed("),
                "{signature}"
            );
        }

        let handlers = include_str!("../handlers.rs");
        let routes = body(
            handlers,
            "pub fn erc8004_write_routes<A>() -> Router<A>",
            "\n}\n",
        );
        let sends = body(&routes, "let sends = Router::new()", ";\n");
        let sends = sends.as_str();
        assert!(
            sends.contains("daily_cap::enforce"),
            "the sending routes are not capped"
        );
        for handler in [
            "post_register::<A>",
            "post_feedback::<A>",
            "post_submit_relay_feedback::<A>",
            "post_submit_solana_feedback::<A>",
            "post_append_response::<A>",
            "post_submit_relay_response::<A>",
        ] {
            assert!(
                sends.contains(handler),
                "{handler} is not under the daily limit"
            );
        }
        assert_eq!(sends.matches(".route(").count(), 6);
        assert!(
            body(handlers, "pub async fn post_register<A>(", "\n}\n").contains("daily_cap::scope("),
            "the asynchronous registration mints without its slot"
        );
    }
}
