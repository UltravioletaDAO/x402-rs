//! The per-IP rate limits, built in ONE place and published by the same code
//! that enforces them.
//!
//! # Why a governor is never built anywhere else
//!
//! A limit a client can only learn by hitting it is the failure this module
//! exists to remove. Every limit here is announced three ways, and all three
//! are derived from the one [`Policy`] that also configures the limiter:
//!
//! - `RateLimit-Policy` on every response the limit governs, `200` included;
//! - `RateLimit` next to it, with what is left in the caller's bucket;
//! - `rate_limits` in `/.well-known/uvd-stack.json` (`crate::interop`), which is
//!   built from the governors [`Bucket::govern`] actually mounted, not from a
//!   list someone keeps.
//!
//! A limiter built by hand next to a `.route(...)` would enforce a number none
//! of the three mention, so `client_ip`'s source test fails when a
//! `GovernorConfigBuilder` appears outside this file.
//!
//! # The header format
//!
//! `draft-ietf-httpapi-ratelimit-headers-11` (Structured Fields, RFC 9651),
//! the format the stack's interop specification names for R7.3:
//!
//! ```text
//! RateLimit-Policy: "verify-settle";q=30;w=60
//! RateLimit: "verify-settle";r=29;t=2
//! ```
//!
//! # How a token bucket becomes `q` and `w`
//!
//! tower_governor's GCRA refills ONE token every `period` and holds at most
//! `burst`. That is published as `q = burst` per `w = burst * period`, and the
//! translation is exact, not an approximation: a caller that never sends more
//! than `q` requests inside ANY window of `w` seconds is never refused, because
//! a GCRA bucket admits every sequence with at most `burst + t / period`
//! requests in any interval of `t` seconds. The same pair is the sustained rate
//! (`q / w = 1 / period`) and the burst (`q` at once).
//!
//! `t` on `RateLimit` is one `period`, rounded up: within that many seconds the
//! caller can spend no more than the `r` it was told, because at most one
//! token arrives in the meantime and it arrives at the end. It is deliberately
//! not "seconds until the bucket is full", which on the registration bucket
//! would tell a client to wait fifty minutes for a token that is twelve seconds
//! away.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use axum::Router;
use governor::middleware::StateInformationMiddleware;
use tower_governor::governor::{GovernorConfig, GovernorConfigBuilder};
use tower_governor::GovernorLayer;

use crate::client_ip::ClientIpKeyExtractor;

/// `RateLimit-Policy`: the quota a response is governed by.
pub const RATELIMIT_POLICY: HeaderName = HeaderName::from_static("ratelimit-policy");

/// `RateLimit`: what is left of that quota for this caller.
pub const RATELIMIT: HeaderName = HeaderName::from_static("ratelimit");

/// What tower_governor reports the caller's remaining tokens under, on the
/// `200` and on the `429` alike (`use_headers()`).
const GOVERNOR_REMAINING: &str = "x-ratelimit-remaining";

/// Which public door of the service a limit guards: the two doors the
/// interop manifest names for this service (`endpoints.api`, `endpoints.mcp`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Door {
    /// Every HTTP route of the facilitator.
    Api,
    /// `POST /mcp`.
    Mcp,
}

impl Door {
    /// The manifest's name for this door (`rate_limits[].applies_to`).
    pub fn as_str(self) -> &'static str {
        match self {
            Door::Api => "api",
            Door::Mcp => "mcp",
        }
    }
}

/// One per-IP limit: tower_governor's GCRA, one token every `period`, at most
/// `burst` held.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Policy {
    /// The policy's name in `RateLimit-Policy` and `RateLimit`. Two routes that
    /// share a bucket share the name, which is how a client learns that
    /// draining one drains the other (`/mcp` and `/settle`).
    pub name: &'static str,
    /// One token comes back every `period`. A period, NOT a rate.
    pub period: Duration,
    /// The most tokens the bucket holds.
    pub burst: u32,
}

impl Policy {
    /// A policy; `name` must be lowercase ASCII, digits and `-` (it is
    /// written unescaped into a Structured Field string).
    pub const fn new(name: &'static str, period: Duration, burst: u32) -> Self {
        Self {
            name,
            period,
            burst,
        }
    }

    /// `q`: the quota of one window, which is the burst.
    pub fn quota(&self) -> u64 {
        u64::from(self.burst)
    }

    /// `w`: the window over which `q` holds, `burst * period` rounded UP to a
    /// whole second (a longer window with the same quota is stricter, so the
    /// rounding never promises more than the limiter grants). At least 1.
    pub fn window_s(&self) -> u64 {
        ceil_secs(self.period.saturating_mul(self.burst))
    }

    /// `t`: one period, rounded up. See the module docs for why.
    pub fn reset_s(&self) -> u64 {
        ceil_secs(self.period)
    }

    /// The `RateLimit-Policy` value: `"<name>";q=<burst>;w=<window>`.
    pub fn policy_field(&self) -> HeaderValue {
        HeaderValue::from_str(&format!(
            "\"{}\";q={};w={}",
            self.name,
            self.quota(),
            self.window_s()
        ))
        .expect("policy names are plain ASCII")
    }

    /// The `RateLimit` value for a caller with `remaining` tokens:
    /// `"<name>";r=<remaining>;t=<period>`.
    pub fn limit_field(&self, remaining: u64) -> HeaderValue {
        HeaderValue::from_str(&format!(
            "\"{}\";r={};t={}",
            self.name,
            remaining,
            self.reset_s()
        ))
        .expect("policy names are plain ASCII")
    }
}

/// Seconds, rounded up, never zero: `w` and `t` are non-zero integers in the
/// draft, and a sub-second period still means "wait before the next one".
fn ceil_secs(duration: Duration) -> u64 {
    let nanos = duration.as_nanos();
    let secs = nanos.div_ceil(1_000_000_000);
    u64::try_from(secs).unwrap_or(u64::MAX).max(1)
}

/// A limiter mounted on one door. What the manifest publishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Mount {
    pub policy: Policy,
    pub door: Door,
}

/// Every governor [`Bucket::govern`] mounted in this process, in mount order.
///
/// Written while `main` assembles the router, before the first request is
/// served, and read by the manifest. A process-wide list rather than a value
/// threaded through `main`, because two of the limiters are mounted inside
/// `handlers` (`human_page_routes_governed`, `erc8004_write_routes_governed`)
/// and a list `main` kept would have to be told about them by hand -- which is
/// the drift this module exists to rule out.
static MOUNTED: Mutex<Vec<Mount>> = Mutex::new(Vec::new());

/// The limits mounted so far, each (policy, door) once, in mount order.
pub fn mounted() -> Vec<Mount> {
    MOUNTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

fn record(mount: Mount) {
    let mut all = MOUNTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !all.contains(&mount) {
        all.push(mount);
    }
}

/// The limiter tower_governor builds, keyed on the client IP, with its state
/// reported in `x-ratelimit-*` (`use_headers()`).
type Config = GovernorConfig<ClientIpKeyExtractor, StateInformationMiddleware>;

/// One token bucket per client IP, shared by every router it governs.
///
/// Cloning a `Bucket`, or governing a second router with it, shares the SAME
/// buckets: that is how `/mcp` draws on `/verify` and `/settle`'s budget.
#[derive(Clone)]
pub struct Bucket {
    policy: Policy,
    config: Arc<Config>,
}

impl Bucket {
    /// The only `GovernorConfigBuilder` in the service.
    pub fn new(policy: Policy) -> Self {
        let config = GovernorConfigBuilder::default()
            .period(policy.period)
            .burst_size(policy.burst)
            .key_extractor(ClientIpKeyExtractor)
            .use_headers()
            .finish()
            .unwrap_or_else(|| {
                panic!(
                    "rate limit {:?} is invalid: period and burst must be non-zero",
                    policy.name
                )
            });
        Self {
            policy,
            config: Arc::new(config),
        }
    }

    /// `router` under this bucket, with the refusal as typed JSON
    /// (`handlers::rate_limit_error`) and the `RateLimit-Policy` / `RateLimit`
    /// headers on every response, the `429` included. Records the mount for
    /// the manifest.
    ///
    /// The header layer sits OUTSIDE the limiter, so it sees the limiter's own
    /// refusal and the `x-ratelimit-remaining` it computed; inside, it would
    /// never run on a `429`.
    pub fn govern<S>(&self, router: Router<S>, door: Door) -> Router<S>
    where
        S: Clone + Send + Sync + 'static,
    {
        record(Mount {
            policy: self.policy,
            door,
        });
        router
            .layer(
                GovernorLayer::new(Arc::clone(&self.config))
                    .error_handler(crate::handlers::rate_limit_error),
            )
            .layer(axum::middleware::from_fn_with_state(
                self.policy,
                stamp_headers,
            ))
    }
}

/// Add `RateLimit-Policy`, and `RateLimit` when the limiter reported what is
/// left. A response the limiter could not key (no client address) carries only
/// the policy: there is no bucket to report on.
async fn stamp_headers(State(policy): State<Policy>, request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let remaining = response
        .headers()
        .get(GOVERNOR_REMAINING)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    let headers = response.headers_mut();
    headers.insert(RATELIMIT_POLICY, policy.policy_field());
    if let Some(remaining) = remaining {
        headers.insert(RATELIMIT, policy.limit_field(remaining));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::routing::get;
    use tower::ServiceExt;

    fn policy(name: &'static str, period_ms: u64, burst: u32) -> Policy {
        Policy::new(name, Duration::from_millis(period_ms), burst)
    }

    #[test]
    fn a_token_bucket_publishes_its_burst_over_the_time_it_takes_to_refill() {
        // /verify and /settle: one token every 2s, burst 30.
        let p = policy("verify-settle", 2_000, 30);
        assert_eq!((p.quota(), p.window_s(), p.reset_s()), (30, 60, 2));
        assert_eq!(p.policy_field(), "\"verify-settle\";q=30;w=60");
        assert_eq!(p.limit_field(7), "\"verify-settle\";r=7;t=2");
    }

    #[test]
    fn sub_second_periods_round_up_and_never_to_zero() {
        // The bazaar reads: one token every 200ms, burst 120 -> 120 per 24s.
        let p = policy("discovery-read", 200, 120);
        assert_eq!((p.window_s(), p.reset_s()), (24, 1));
        // A window that is not a whole number of seconds rounds UP: 7 * 300ms
        // is 2.1s, and promising 7 per 2s would be more than the bucket gives.
        assert_eq!(policy("x", 300, 7).window_s(), 3);
        assert_eq!(policy("x", 1, 1).window_s(), 1);
        assert_eq!(policy("x", 1, 1).reset_s(), 1);
    }

    /// The publication is only worth anything if it is a limit the bucket
    /// never breaks: a caller spending exactly `q` per `w` must never see a
    /// 429, and one request more inside the same window must.
    #[tokio::test]
    async fn a_caller_that_keeps_to_the_published_quota_is_never_refused() {
        let p = policy("probe", 1_000, 3);
        let router = Bucket::new(p).govern(
            Router::new().route("/probe", get(|| async { "ok" })),
            Door::Api,
        );
        let call = || async {
            router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/probe")
                        .header("x-forwarded-for", "203.0.113.9")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
        };
        for expected_remaining in (0..3).rev() {
            let response = call().await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers()[RATELIMIT_POLICY],
                "\"probe\";q=3;w=3",
                "the 200 must carry the policy"
            );
            assert_eq!(
                response.headers()[RATELIMIT],
                format!("\"probe\";r={expected_remaining};t=1").as_str()
            );
        }
        let refused = call().await;
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(refused.headers()[RATELIMIT_POLICY], "\"probe\";q=3;w=3");
        assert_eq!(refused.headers()[RATELIMIT], "\"probe\";r=0;t=1");
        assert!(
            refused.headers().contains_key("retry-after"),
            "a 429 carries Retry-After (R7.3)"
        );
        assert!(
            refused.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("application/json"),
            "the refusal is still the typed JSON one"
        );
    }

    /// Two routers governed by one `Bucket` share the caller's tokens -- the
    /// `/mcp` and `/settle` arrangement -- and say so with one policy name.
    #[tokio::test]
    async fn one_bucket_on_two_doors_is_one_budget_under_one_name() {
        let bucket = Bucket::new(policy("shared-probe", 60_000, 2));
        let a = bucket.govern(Router::new().route("/a", get(|| async { "a" })), Door::Api);
        let b = bucket.govern(Router::new().route("/b", get(|| async { "b" })), Door::Mcp);
        let hit = |router: Router, path: &'static str| async move {
            router
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header("x-forwarded-for", "198.51.100.4")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
        };
        assert_eq!(hit(a.clone(), "/a").await.status(), StatusCode::OK);
        assert_eq!(hit(b.clone(), "/b").await.status(), StatusCode::OK);
        let third = hit(a, "/a").await;
        assert_eq!(third.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            third.headers()[RATELIMIT_POLICY],
            "\"shared-probe\";q=2;w=120"
        );

        let doors: Vec<Door> = mounted()
            .into_iter()
            .filter(|m| m.policy.name == "shared-probe")
            .map(|m| m.door)
            .collect();
        assert_eq!(doors, vec![Door::Api, Door::Mcp]);
    }

    /// A route the limiter cannot key (no X-Forwarded-For, no peer address)
    /// still names its policy, and does not invent a remaining count.
    #[tokio::test]
    async fn an_unkeyed_request_names_the_policy_and_reports_no_remaining() {
        let router = Bucket::new(policy("unkeyed-probe", 1_000, 5))
            .govern(Router::new().route("/u", get(|| async { "u" })), Door::Api);
        let response = router
            .oneshot(Request::builder().uri("/u").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            response.headers()[RATELIMIT_POLICY],
            "\"unkeyed-probe\";q=5;w=5"
        );
        assert!(!response.headers().contains_key(RATELIMIT));
    }

    #[test]
    fn a_mount_is_recorded_once_however_often_it_is_governed() {
        let bucket = Bucket::new(policy("recorded-once", 1_000, 1));
        for _ in 0..3 {
            let _ = bucket.govern(Router::<()>::new(), Door::Api);
        }
        let count = mounted()
            .iter()
            .filter(|m| m.policy.name == "recorded-once")
            .count();
        assert_eq!(count, 1);
    }
}
