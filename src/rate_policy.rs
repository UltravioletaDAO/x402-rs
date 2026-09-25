//! The service's rate policy, in one place.
//!
//! Four limits live on this host and they protect different things, so they
//! answer differently and exempt differently:
//!
//! | Limit | Protects | Answer | Applies to |
//! |---|---|---|---|
//! | the per-IP budgets ([`BUDGETS`]) | a rule we chose: how much one third party may ask | `429` | third parties only |
//! | the per-address in-flight ceiling ([`Admission`]) | a rule we chose: one address cannot fill the machine's ceiling | `429` | third parties only |
//! | the global in-flight ceiling ([`Admission`]) | the machine: requests this task is handling at once | `503` | every caller |
//! | the ERC-8004 daily write cap (`erc8004::daily_cap`) | the gas the facilitator wallet pays | `429` | every caller |
//!
//! A request takes its global slot only once its body has arrived, and a body
//! has [`DEFAULT_BODY_DEADLINE_MS`] in total to arrive (`408` past it): an
//! upload that never finishes holds no slot, so it cannot turn the machine's
//! ceiling into a refusal for everybody while the CPU sits idle.
//!
//! The RPC provider throttle (`RPC_MAX_CU_PER_SECOND`, `chain::evm`) is a
//! fourth, client-side one: it paces our calls to the provider and answers
//! nobody. Nothing here touches it.
//!
//! # Stack identities
//!
//! Our own services -- Execution Market, KarmaKadabra, describe.net, meshrelay
//! -- are not third parties, and a budget sized against one anonymous client
//! must not throttle them: Execution Market relays every agent's rating from a
//! handful of egress addresses, so a per-IP bucket put the whole ecosystem in
//! one bucket (73 of 141 ratings refused on 2026-09-25). An address cannot say
//! who is calling -- KarmaKadabra's tasks leave from an ephemeral public IP --
//! so the exemption is by credential: a caller that presents a recognized
//! `X-UVD-Stack-Key` skips every per-IP budget, and nothing else.
//!
//! The same [`PolicyLayer`] wraps every governor on the service; no route has
//! an exemption of its own. The types hold it: [`config`] returns a [`Bucket`]
//! that only [`RatePolicy::layer`] can mount, and outside this module a
//! [`RatePolicy`] only comes from the environment ([`RatePolicy::from_env`]),
//! so a second policy that exempts nobody does not compile.
//! `every_governor_goes_through_the_policy` covers what a type cannot: a
//! budget built in `main.rs` and never mounted.
//!
//! # The credential
//!
//! A key is `uvdsk_` followed by at least 43 base64url characters (32 random
//! bytes). The facilitator never holds one: it is configured with the SHA-256
//! of each key, per service, and compares the digest of what a caller presents
//! against all of them in constant time. So there is no key in its
//! environment, its memory or `GET /config` to leak, and a digest that does
//! leak does not authenticate anybody. One variable per service,
//! `UVD_STACK_KEY_SHA256_<SERVICE>`, holding one digest or several separated by
//! commas -- two during a rotation. Emptying one service's variable revokes that
//! service alone. The service list defaults to [`DEFAULT_STACK_SERVICES`] and
//! `UVD_STACK_SERVICES` replaces it.
//!
//! A key that is absent, malformed, unknown or revoked makes the caller a third
//! party: charged to its address like anybody else, never a `500`. A rejected
//! key is logged with the caller's address and path, never its value.

use std::collections::HashMap;
use std::convert::Infallible;
use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Response, StatusCode};
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use governor::middleware::StateInformationMiddleware;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;
use tower::{Layer, Service, ServiceExt};
use tower_governor::governor::{Governor, GovernorConfig, GovernorConfigBuilder};
use tower_governor::key_extractor::KeyExtractor;
use tower_governor::GovernorLayer;
use tracing::{info, warn};

use crate::client_ip::ClientIpKeyExtractor;

// ============================================================================
// Budgets
// ============================================================================

/// A per-IP budget as `tower_governor` applies it: ONE token every `period`,
/// at most `burst` banked. `period` is a replenish period, not a rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit {
    pub period: Duration,
    pub burst: u32,
}

impl Limit {
    pub const fn every_ms(period_ms: u64, burst: u32) -> Self {
        Self {
            period: Duration::from_millis(period_ms),
            burst,
        }
    }
}

/// One per-IP budget: its default, and the two variables that override it.
#[derive(Debug)]
pub struct Budget {
    /// The name `GET /config` publishes it under.
    pub name: &'static str,
    /// What draws on it, in words, for `GET /config`.
    pub routes: &'static str,
    default_period_ms: u64,
    default_burst: u32,
    env_period_ms: &'static str,
    env_burst: &'static str,
}

impl Budget {
    pub const fn default_limit(&self) -> Limit {
        Limit::every_ms(self.default_period_ms, self.default_burst)
    }

    /// The effective limit: each override that parses to a positive integer
    /// replaces its half of the default, anything else is ignored.
    pub fn limit(&self) -> Limit {
        self.limit_from(|var| std::env::var(var).ok())
    }

    fn limit_from(&self, lookup: impl Fn(&str) -> Option<String>) -> Limit {
        let period_ms = lookup(self.env_period_ms)
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(self.default_period_ms);
        let burst = lookup(self.env_burst)
            .and_then(|v| v.trim().parse::<u32>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(self.default_burst);
        Limit::every_ms(period_ms, burst)
    }
}

/// `/verify`, `/settle` and `POST /mcp`: 1 token every 2s, burst 30 (~30/min).
///
/// Each call burns RPC quota against the configured chain providers, which is
/// why this one stays tight. `/mcp` shares the SAME bucket (one config, cloned
/// `Arc`), because an `x402_settle` tool call costs the chain what
/// `POST /settle` does.
pub const VERIFY_SETTLE: Budget = Budget {
    name: "verify_settle",
    routes: "/verify, /settle, /receipts/*, /.well-known/receipt-keys.json, POST /mcp",
    default_period_ms: 2_000,
    default_burst: 30,
    env_period_ms: "VERIFY_SETTLE_RATE_PER_MS",
    env_burst: "VERIFY_SETTLE_RATE_BURST",
};

/// `POST /discovery/register` and the bazaar admin routes: 1 token every 12s,
/// burst 250.
///
/// The burst was 5, sized against a threat this endpoint does not pose:
/// `DiscoveryRegistry::register` parses the URL, runs the syntactic SSRF checks
/// and writes to the store -- `validate_resource` contains no `.await` at all,
/// and the only outbound fetching lives in the aggregator's own background
/// interval. The cost of the wrong number was measured: three days running, a
/// batch of ~200 registrations arrived at 06:00 UTC and 97% of it was rejected.
///
/// Raising the burst does NOT loosen the abuse ceiling: the sustained rate is
/// unchanged at one token every 12s, so a spammer could already push ~7200/day
/// with burst 5. The burst only decides whether a legitimate batch survives its
/// first minute. If outbound validation is ever added to the register path,
/// this number must come back down.
pub const DISCOVERY_REGISTER: Budget = Budget {
    name: "discovery_register",
    routes: "POST /discovery/register, /discovery/admin/*, POST /discovery/refresh",
    default_period_ms: 12_000,
    default_burst: 250,
    env_period_ms: "DISCOVERY_REGISTER_RATE_PER_MS",
    env_burst: "DISCOVERY_REGISTER_RATE_BURST",
};

/// Bazaar reads (`/discovery/resources`, `/discovery/stats`), `/transactions`,
/// `/api/stats` and the DX402 routes: 1 token every 200ms (~300/min), burst 120.
///
/// Cheap in-memory reads, and the point of a bazaar is that consumers page
/// through it: a 21k-item catalog is ~212 requests at the 100/page cap. An
/// earlier 30/min throttled exactly that (every 429 observed in production was
/// a paginating client), so the budget covers a full-catalog walk while still
/// cutting off a hammering loop.
pub const DISCOVERY_READ: Budget = Budget {
    name: "discovery_read",
    routes: "/discovery/resources, /discovery/stats, /discovery/attestation/*, /discovery/config, /transactions, /api/stats/*, /dx402/*",
    default_period_ms: 200,
    default_burst: 120,
    env_period_ms: "DISCOVERY_READ_RATE_PER_MS",
    env_burst: "DISCOVERY_READ_RATE_BURST",
};

/// `GET /events`: 1 token every 2s, burst 10.
///
/// A public, long-lived connection on the task that settles payments. The
/// subscriber cap (`X402_EVENTS_MAX_SUBSCRIBERS`) bounds how many are held at
/// once; this bounds how fast they can be opened, so a reconnect loop cannot
/// churn through admission slots. A browser `EventSource` reconnects rarely.
pub const EVENTS: Budget = Budget {
    name: "events",
    routes: "GET /events",
    default_period_ms: 2_000,
    default_burst: 10,
    env_period_ms: "EVENTS_RATE_PER_MS",
    env_burst: "EVENTS_RATE_BURST",
};

/// ERC-8004 identity reads: 1 token every 500ms (~120/min), burst 60.
///
/// Every cold `/identity/{network}/owner/{address}` costs a `balanceOf`, a
/// `totalSupply` and a Multicall3 scan against the shared RPC budget; one
/// client sweeping nine networks in parallel dragged the whole service's p99
/// from 0.4s to 11.6s (2026-08-29). Deliberately GENEROUS all the same: that
/// sweep ran ~21 req/min aggregated, so 120/min does not touch legitimate
/// traffic and only cuts off a runaway loop.
pub const IDENTITY_READ: Budget = Budget {
    name: "identity_read",
    routes: "/identity/*",
    default_period_ms: 500,
    default_burst: 60,
    env_period_ms: "IDENTITY_READ_RATE_PER_MS",
    env_burst: "IDENTITY_READ_RATE_BURST",
};

/// `/reputation`, `/blacklist`, `POST /escrow/state`, `/health/ready`,
/// `GET /config` and the 404 fallback: 1 token every 300ms (~200/min), burst
/// 100.
///
/// Each of the first three costs at least one RPC or contract read; none was
/// implicated in the 2026-08-29 incident, so this is preventive and argues for
/// staying generous. Deliberately its OWN bucket, not a share of
/// [`DISCOVERY_READ`]: that one is sized against bazaar pagination, and a
/// paginating bazaar client and a reputation caller from one IP should not
/// draw down the same budget.
pub const SECONDARY_READ: Budget = Budget {
    name: "secondary_read",
    routes:
        "/reputation/*, /blacklist, POST /escrow/state, /health/ready, /config, unknown paths (404)",
    default_period_ms: 300,
    default_burst: 100,
    env_period_ms: "SECONDARY_READS_RATE_PER_MS",
    env_burst: "SECONDARY_READS_RATE_BURST",
};

/// The HTML pages a person reads: 1 token every 500ms (~120 pages/min), burst
/// 60, one bucket for all of them.
///
/// Metered because `/` alone is ~245 KB served from the task that settles
/// payments. Generous because a reader fetches one document per navigation --
/// logos, sheet and fonts are separate, unmetered routes -- and what does get
/// near 60 is an office or a carrier NAT putting many readers behind one
/// address. The ceiling is sized against a loop, not against a reader.
pub const HUMAN_PAGES: Budget = Budget {
    name: "human_pages",
    routes: "/, /bazaar, /networks, /x402, /dx402, /erc8004, /integrar, /events/live, /stats",
    default_period_ms: 500,
    default_burst: 60,
    env_period_ms: "HUMAN_PAGES_RATE_PER_MS",
    env_burst: "HUMAN_PAGES_RATE_BURST",
};

/// The ERC-8004 writes (`/register`, `/feedback`, `/feedback/*`): 1 token every
/// 12s, burst 30.
///
/// Sized against thirty days of production write traffic counted as if every
/// write came from one address: at this burst the batches that succeeded are
/// served in full save a single request, which its 429's `retry-after` covers.
/// The writes that send a transaction are ALSO under the per-network daily cap
/// (`erc8004::daily_cap`), which protects gas and which no identity skips.
pub const ERC8004_WRITES: Budget = Budget {
    name: "erc8004_writes",
    routes: "POST /register, POST /feedback, POST /feedback/*",
    default_period_ms: 12_000,
    default_burst: 30,
    env_period_ms: "ERC8004_WRITES_RATE_PER_MS",
    env_burst: "ERC8004_WRITES_RATE_BURST",
};

/// Every per-IP budget on the service. `GET /config` publishes these, and a
/// governor in `main.rs` sized by anything else fails
/// `every_governor_goes_through_the_policy`.
pub const BUDGETS: [&Budget; 8] = [
    &VERIFY_SETTLE,
    &DISCOVERY_REGISTER,
    &DISCOVERY_READ,
    &EVENTS,
    &IDENTITY_READ,
    &SECONDARY_READ,
    &HUMAN_PAGES,
    &ERC8004_WRITES,
];

/// The one governor configuration type on the service. Private: nothing
/// outside this module can name it, so nothing outside can mount one.
type PolicyConfig = GovernorConfig<ClientIpKeyExtractor, StateInformationMiddleware>;

/// A per-address bucket, sized by a budget, that only [`RatePolicy::layer`] can
/// mount. Opaque on purpose: it is not a `GovernorConfig`, so a
/// `GovernorLayer` built from it -- a governor the stack does not skip -- does
/// not compile. Clone it to share the bucket between routers.
#[derive(Clone)]
pub struct Bucket(Arc<PolicyConfig>);

/// A bucket per client address for `limit`. The only `GovernorConfigBuilder`
/// on the service.
///
/// Keyed on [`ClientIpKeyExtractor`], so a third party is the address the load
/// balancer appended. `use_headers()` makes the budget legible: a `200` carries
/// `x-ratelimit-limit` and `x-ratelimit-remaining`, not just the `429`.
pub fn config(limit: Limit) -> Bucket {
    Bucket(Arc::new(
        GovernorConfigBuilder::default()
            .period(limit.period)
            .burst_size(limit.burst)
            .key_extractor(ClientIpKeyExtractor)
            .use_headers()
            .finish()
            .expect("a policy budget has a non-zero period and burst"),
    ))
}

// ============================================================================
// Stack identities
// ============================================================================

/// The header a stack service presents its key in.
pub const STACK_KEY_HEADER: &str = "x-uvd-stack-key";

/// Set on a response the policy served without charging a budget, naming the
/// service it recognized. What the read-only probe looks for.
pub const EXEMPT_HEADER: &str = "x-ratelimit-exempt";

/// Every key starts with this, so a leaked one is recognizable to a secret
/// scanner and a stray value is not mistaken for one.
pub const STACK_KEY_PREFIX: &str = "uvdsk_";

/// 32 random bytes in base64url without padding.
const STACK_KEY_MIN_SECRET_CHARS: usize = 43;
const STACK_KEY_MAX_SECRET_CHARS: usize = 128;

pub const ENV_STACK_SERVICES: &str = "UVD_STACK_SERVICES";
pub const ENV_STACK_KEY_SHA256_PREFIX: &str = "UVD_STACK_KEY_SHA256_";

/// The services a key may be configured for unless `UVD_STACK_SERVICES` says
/// otherwise. A name with no digest is listed as inactive and exempts nobody.
pub const DEFAULT_STACK_SERVICES: [&str; 4] = [
    "execution-market",
    "karmakadabra",
    "describe-net",
    "meshrelay",
];

/// A rejected key is logged on the first rejection and then once every this
/// many, never with the value: a caller looping on a bad key must not become a
/// log line per request.
const REJECTED_LOG_EVERY: u64 = 100;

/// `UVD_STACK_KEY_SHA256_<SERVICE>`: the name upper-cased, `-` as `_`.
pub fn env_var_for(service: &str) -> String {
    format!(
        "{ENV_STACK_KEY_SHA256_PREFIX}{}",
        service.to_ascii_uppercase().replace('-', "_")
    )
}

struct StackService {
    name: Arc<str>,
    digests: Vec<[u8; 32]>,
}

/// The stack services and the digests of the keys each may present.
///
/// `Debug` prints names and counts, never a digest.
pub struct StackIdentities {
    services: Vec<StackService>,
    rejected: AtomicU64,
}

impl std::fmt::Debug for StackIdentities {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut list = f.debug_map();
        for service in &self.services {
            list.entry(&&*service.name, &service.digests.len());
        }
        list.finish()
    }
}

impl StackIdentities {
    /// Nobody is exempt.
    #[cfg(test)]
    pub fn none() -> Self {
        Self {
            services: Vec::new(),
            rejected: AtomicU64::new(0),
        }
    }

    pub fn from_env() -> Self {
        Self::from_lookup(|var| std::env::var(var).ok())
    }

    /// Read the service list and each service's digests through `lookup`.
    ///
    /// Private: production reads the environment ([`Self::from_env`]), and a
    /// second set of identities built elsewhere is how a governor ends up
    /// exempting nobody. Tests in this module call it directly.
    ///
    /// Every problem is logged by service and variable name, never by value: a
    /// digest is not a key, but a raw key pasted into the wrong variable is,
    /// and it must not reach a log line on its way to being rejected.
    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let names: Vec<String> = match lookup(ENV_STACK_SERVICES) {
            Some(raw) => raw
                .split(',')
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty())
                .collect(),
            None => DEFAULT_STACK_SERVICES
                .iter()
                .map(|n| n.to_string())
                .collect(),
        };
        let mut services: Vec<StackService> = Vec::new();
        for name in names {
            if !valid_service_name(&name) {
                warn!(
                    variable = ENV_STACK_SERVICES,
                    "stack service name skipped: use 1-40 characters of a-z, 0-9 and '-'"
                );
                continue;
            }
            if services.iter().any(|s| *s.name == *name) {
                continue;
            }
            let var = env_var_for(&name);
            let mut digests: Vec<[u8; 32]> = Vec::new();
            for (position, entry) in lookup(&var)
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|e| !e.is_empty())
                .enumerate()
            {
                let Some(digest) = parse_digest(entry) else {
                    warn!(
                        service = %name,
                        variable = %var,
                        position,
                        "stack key digest skipped: not 64 hex characters (a SHA-256)"
                    );
                    continue;
                };
                let claimed = services
                    .iter()
                    .any(|s| s.digests.iter().any(|d| bool::from(d.ct_eq(&digest))));
                if claimed || digests.contains(&digest) {
                    warn!(
                        service = %name,
                        variable = %var,
                        position,
                        "stack key digest skipped: already configured"
                    );
                    continue;
                }
                digests.push(digest);
            }
            services.push(StackService {
                name: Arc::from(name.as_str()),
                digests,
            });
        }
        let identities = Self {
            services,
            rejected: AtomicU64::new(0),
        };
        info!(
            active = identities.active(),
            services = ?identities,
            "stack identities configured (digest count per service)"
        );
        identities
    }

    /// How many services have at least one key configured.
    pub fn active(&self) -> usize {
        self.services
            .iter()
            .filter(|s| !s.digests.is_empty())
            .count()
    }

    /// The service a request's `X-UVD-Stack-Key` belongs to, or `None`.
    ///
    /// `None` -- a third party -- unless there is exactly one header line, it
    /// is a well-formed key, and the SHA-256 of its exact bytes equals a
    /// configured digest. Every configured digest is compared, in constant
    /// time, whatever matches first. Logs nothing: [`admit`] logs rejections,
    /// once per request, with the caller's address.
    pub fn authenticate(&self, headers: &HeaderMap) -> Option<Arc<str>> {
        match self.presented(headers) {
            Presented::Recognized(service) => Some(service),
            Presented::Absent | Presented::Rejected => None,
        }
    }

    fn presented(&self, headers: &HeaderMap) -> Presented {
        let mut lines = headers.get_all(STACK_KEY_HEADER).iter();
        let Some(presented) = lines.next() else {
            return Presented::Absent;
        };
        if self.active() == 0 {
            return Presented::Absent;
        }
        let one_line = lines.next().is_none();
        let key = presented.as_bytes();
        let found = if one_line && well_formed(key) {
            let digest: [u8; 32] = Sha256::digest(key).into();
            let mut found: Option<&StackService> = None;
            for service in &self.services {
                for configured in &service.digests {
                    if bool::from(configured.ct_eq(&digest)) {
                        found = Some(service);
                    }
                }
            }
            found
        } else {
            None
        };
        match found {
            Some(service) => Presented::Recognized(Arc::clone(&service.name)),
            None => Presented::Rejected,
        }
    }

    /// Log a rejected key by who sent it and where -- the first one, then one in
    /// every [`REJECTED_LOG_EVERY`] -- never by its value.
    fn note_rejected(&self, client: Option<IpAddr>, path: &str) {
        let rejected = self.rejected.fetch_add(1, Ordering::Relaxed) + 1;
        if rejected == 1 || rejected.is_multiple_of(REJECTED_LOG_EVERY) {
            let client = client.map_or_else(|| "unknown".to_string(), |ip| ip.to_string());
            warn!(
                rejected,
                client_ip = %client,
                path = %path,
                "X-UVD-Stack-Key presented but not recognized: the caller is \
                 charged as a third party"
            );
        }
    }

    /// What `GET /config` says about the identities: names, whether each is
    /// active and how many digests it holds. Never a digest.
    pub fn summary(&self) -> Value {
        json!({
            "header": "X-UVD-Stack-Key",
            "active": self.active(),
            "services": self
                .services
                .iter()
                .map(|s| json!({
                    "name": &*s.name,
                    "active": !s.digests.is_empty(),
                    "credentials": s.digests.len(),
                }))
                .collect::<Vec<_>>(),
            "exemptFrom": ["every budget under rateLimits", "overload.perClient"],
            "notExemptFrom": [
                "overload (the global ceiling)",
                "overload.bodyDeadlineMs",
                "erc8004DailyWriteCap",
                "the RPC provider throttle",
            ],
            "configuration": format!(
                "{ENV_STACK_SERVICES} (service names, default {}) and \
                 {ENV_STACK_KEY_SHA256_PREFIX}<SERVICE> (comma-separated SHA-256 hex digests \
                 of the keys; empty revokes the service)",
                DEFAULT_STACK_SERVICES.join(",")
            ),
        })
    }
}

/// What a request's `X-UVD-Stack-Key` turned out to be.
enum Presented {
    /// No header, or no identity configured: an ordinary caller.
    Absent,
    Recognized(Arc<str>),
    /// A header that authenticates nobody: charged as a third party, and logged.
    Rejected,
}

fn valid_service_name(name: &str) -> bool {
    (1..=40).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn parse_digest(entry: &str) -> Option<[u8; 32]> {
    if entry.len() != 64 {
        return None;
    }
    let mut digest = [0u8; 32];
    hex::decode_to_slice(entry, &mut digest).ok()?;
    Some(digest)
}

/// `uvdsk_` and 43 to 128 base64url characters. Checked before hashing so a
/// short or improvised value can never authenticate, whatever its digest.
fn well_formed(key: &[u8]) -> bool {
    let Some(secret) = key.strip_prefix(STACK_KEY_PREFIX.as_bytes()) else {
        return false;
    };
    (STACK_KEY_MIN_SECRET_CHARS..=STACK_KEY_MAX_SECRET_CHARS).contains(&secret.len())
        && secret
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_')
}

// ============================================================================
// The layer every governor is mounted through
// ============================================================================

/// The policy: which callers skip the per-IP budgets.
#[derive(Clone)]
pub struct RatePolicy {
    stack: Arc<StackIdentities>,
}

impl RatePolicy {
    /// Private for the same reason as [`StackIdentities::from_lookup`]: outside
    /// this module the only policy is the one read from the environment, so a
    /// governor mounted with a second, empty one does not compile.
    fn new(stack: StackIdentities) -> Self {
        Self {
            stack: Arc::new(stack),
        }
    }

    /// Nobody is exempt: every caller is charged to its address.
    #[cfg(test)]
    pub fn none() -> Self {
        Self::new(StackIdentities::none())
    }

    /// A policy whose `(service, key)` pairs are recognized, for tests outside
    /// this module that need the stack on a production router.
    #[cfg(test)]
    pub fn for_tests(keys: &[(&str, &str)]) -> Self {
        let vars: HashMap<String, String> = keys
            .iter()
            .map(|(service, key)| {
                let digest = hex::encode(Sha256::digest(key.as_bytes()));
                (env_var_for(service), digest)
            })
            .collect();
        Self::new(StackIdentities::from_lookup(|var| vars.get(var).cloned()))
    }

    pub fn from_env() -> Self {
        Self::new(StackIdentities::from_env())
    }

    pub fn stack(&self) -> &StackIdentities {
        &self.stack
    }

    /// The governor for `bucket`, which a recognized stack identity skips.
    /// Built with the service's JSON `429` (`handlers::rate_limit_error`).
    pub fn layer(&self, bucket: &Bucket) -> PolicyLayer {
        PolicyLayer {
            governor: GovernorLayer::new(Arc::clone(&bucket.0))
                .error_handler(crate::handlers::rate_limit_error),
            stack: Arc::clone(&self.stack),
        }
    }

    /// `routes` under a bucket of their own sized `limit`.
    pub fn govern<S>(&self, routes: Router<S>, limit: Limit) -> Router<S>
    where
        S: Clone + Send + Sync + 'static,
    {
        routes.layer(self.layer(&config(limit)))
    }
}

/// A `GovernorLayer` that a recognized stack identity goes around.
///
/// Built around the governor rather than inside it: a key extractor can only
/// choose a bucket, and a bucket of its own would still refuse the stack past
/// its burst. This keeps both services -- the governed one and the bare one it
/// wraps -- and picks per request.
#[derive(Clone)]
pub struct PolicyLayer {
    governor: GovernorLayer<ClientIpKeyExtractor, StateInformationMiddleware, Body>,
    stack: Arc<StackIdentities>,
}

impl<S: Clone> Layer<S> for PolicyLayer {
    type Service = PolicyService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        PolicyService {
            governed: self.governor.layer(inner.clone()),
            bare: inner,
            stack: Arc::clone(&self.stack),
        }
    }
}

#[derive(Clone)]
pub struct PolicyService<S> {
    governed: Governor<ClientIpKeyExtractor, StateInformationMiddleware, S, Body>,
    bare: S,
    stack: Arc<StackIdentities>,
}

impl<S> Service<Request> for PolicyService<S>
where
    S: Service<Request, Response = Response<Body>, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Response<Body>, Infallible>> + Send>>;

    /// Always ready: `oneshot` polls whichever clone serves the call.
    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request) -> Self::Future {
        match self.stack.authenticate(request.headers()) {
            Some(service) => {
                let bare = self.bare.clone();
                Box::pin(async move {
                    let mut response = bare.oneshot(request).await?;
                    if let Ok(value) = HeaderValue::from_str(&service) {
                        response
                            .headers_mut()
                            .insert(HeaderName::from_static(EXEMPT_HEADER), value);
                    }
                    Ok(response)
                })
            }
            None => Box::pin(self.governed.clone().oneshot(request)),
        }
    }
}

// ============================================================================
// Admission: the per-address ceiling, the body deadline, the machine's ceiling
// ============================================================================

pub const ENV_MAX_INFLIGHT_REQUESTS: &str = "MAX_INFLIGHT_REQUESTS";

/// Requests one task handles at once before it sheds with `503`.
///
/// Sized against the task, not against traffic: 1 vCPU and 2 GB (tfvars), a
/// body capped at 64 KiB, and a measured load two orders of magnitude below
/// this (216 settles in four hours, 2026-08-20). At 512 the bodies alone fit
/// in 32 MiB, so the ceiling is reached by a flood, never by a busy day.
pub const DEFAULT_MAX_INFLIGHT_REQUESTS: usize = 512;

pub const ENV_MAX_INFLIGHT_PER_CLIENT: &str = "MAX_INFLIGHT_PER_CLIENT";

/// Requests one client address may have in flight at once before it is
/// answered `429`.
///
/// So that one address cannot fill the machine's ceiling for everybody: at 32,
/// sixteen addresses are needed to reach 512. A person, an integrator or a NAT
/// with this many requests open at once is already past every budget's burst
/// save the bazaar's. A recognized stack identity skips it: Execution Market
/// relays the whole stack from a handful of addresses, and a per-address limit
/// is a rule we chose, not the machine's capacity.
pub const DEFAULT_MAX_INFLIGHT_PER_CLIENT: usize = 32;

pub const ENV_REQUEST_BODY_DEADLINE_MS: &str = "REQUEST_BODY_DEADLINE_MS";

/// The time a request body has, in total, to arrive once its headers have.
///
/// A TOTAL deadline, not an idle timeout between frames: one reset on every
/// frame is held open indefinitely by a body sent one byte at a time. Every
/// body this service accepts is a JSON document under 64 KiB that a client
/// writes in one go, so five seconds is generous; past it the request is
/// answered `408` and its connection closed.
pub const DEFAULT_BODY_DEADLINE_MS: u64 = 5_000;

/// What a shed or throttled request is told to wait. One second: slots clear
/// as fast as the requests in flight finish.
pub const OVERLOAD_RETRY_AFTER_SECS: u64 = 1;

/// Paths admission never refuses. `/health` is the load balancer's liveness
/// probe: failing it on a busy task would have ECS replace a task that is
/// working, and take its capacity with it.
const ADMISSION_EXEMPT_PATHS: [&str; 1] = ["/health"];

/// Admission to the task, before any route runs, in this order:
///
/// 1. **Per address** ([`DEFAULT_MAX_INFLIGHT_PER_CLIENT`], `429
///    too_many_concurrent_requests`): third parties only; a recognized stack
///    identity skips it.
/// 2. **The body**, read whole within [`DEFAULT_BODY_DEADLINE_MS`] (`408
///    request_timeout`), for every caller. Nothing global is held meanwhile.
/// 3. **The machine** ([`DEFAULT_MAX_INFLIGHT_REQUESTS`], `503 overloaded`): one
///    counter for every caller, stack included. The slot is taken only now, so
///    an upload that never finishes holds none, and it is held until the
///    response is produced -- not while a streamed body is written, so an open
///    `/events` stream holds none either (its own cap is
///    `X402_EVENTS_MAX_SUBSCRIBERS`).
#[derive(Clone)]
pub struct Admission {
    permits: Arc<Semaphore>,
    max: usize,
    per_client: Arc<Mutex<HashMap<IpAddr, usize>>>,
    max_per_client: usize,
    body_deadline: Duration,
    stack: Arc<StackIdentities>,
}

impl Admission {
    /// Every limit is at least 1 and the deadline at least 1 ms.
    pub fn new(
        policy: &RatePolicy,
        max: usize,
        max_per_client: usize,
        body_deadline: Duration,
    ) -> Self {
        let max = max.max(1);
        Self {
            permits: Arc::new(Semaphore::new(max)),
            max,
            per_client: Arc::new(Mutex::new(HashMap::new())),
            max_per_client: max_per_client.max(1),
            body_deadline: body_deadline.max(Duration::from_millis(1)),
            stack: Arc::clone(&policy.stack),
        }
    }

    pub fn from_env(policy: &RatePolicy) -> Self {
        let max = positive_env(ENV_MAX_INFLIGHT_REQUESTS, DEFAULT_MAX_INFLIGHT_REQUESTS);
        let max_per_client =
            positive_env(ENV_MAX_INFLIGHT_PER_CLIENT, DEFAULT_MAX_INFLIGHT_PER_CLIENT);
        let body_deadline_ms = positive_env(ENV_REQUEST_BODY_DEADLINE_MS, DEFAULT_BODY_DEADLINE_MS);
        info!(
            max_inflight_requests = max,
            max_inflight_per_client = max_per_client,
            body_deadline_ms,
            "admission configured: per-address ceiling, body deadline, global ceiling"
        );
        Self::new(
            policy,
            max,
            max_per_client,
            Duration::from_millis(body_deadline_ms),
        )
    }

    pub fn max(&self) -> usize {
        self.max
    }

    pub fn max_per_client(&self) -> usize {
        self.max_per_client
    }

    pub fn body_deadline(&self) -> Duration {
        self.body_deadline
    }

    /// A slot for `client`, or `None` when it already has its maximum in flight.
    fn enter(&self, client: IpAddr) -> Option<ClientSlot> {
        let mut counts = self.per_client.lock().unwrap_or_else(|e| e.into_inner());
        let count = counts.entry(client).or_insert(0);
        if *count >= self.max_per_client {
            return None;
        }
        *count += 1;
        Some(ClientSlot {
            counts: Arc::clone(&self.per_client),
            client,
        })
    }
}

/// One request in flight for one address; given back when dropped, however
/// the request ends (answered, refused, or its connection gone).
struct ClientSlot {
    counts: Arc<Mutex<HashMap<IpAddr, usize>>>,
    client: IpAddr,
}

impl Drop for ClientSlot {
    fn drop(&mut self) {
        let mut counts = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(count) = counts.get_mut(&self.client) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                counts.remove(&self.client);
            }
        }
    }
}

/// A positive integer from `var`, or `default` (with a warning if it was set).
fn positive_env<T>(var: &str, default: T) -> T
where
    T: std::str::FromStr + PartialOrd + Default + std::fmt::Display + Copy,
{
    match std::env::var(var) {
        Ok(raw) => match raw.trim().parse::<T>() {
            Ok(n) if n > T::default() => n,
            _ => {
                warn!(variable = var, default = %default, "not a positive integer; the default applies");
                default
            }
        },
        Err(_) => default,
    }
}

/// The outermost policy middleware; see [`Admission`] for what it enforces and
/// in which order. Mounted inside the tracing layer so a refusal is logged with
/// its status.
///
/// It also marks `X-UVD-Stack-Key` sensitive before anything else sees the
/// request, so a `Debug` of the headers anywhere below prints `Sensitive`
/// instead of the key, and it is where a rejected key is logged -- once per
/// request, with the caller's address and path, never the value.
pub async fn admit(
    State(admission): State<Admission>,
    mut request: Request,
    next: Next,
) -> axum::response::Response {
    mark_stack_key_sensitive(request.headers_mut());
    if ADMISSION_EXEMPT_PATHS.contains(&request.uri().path()) {
        return next.run(request).await;
    }

    let client = ClientIpKeyExtractor.extract(&request).ok();
    let presented = admission.stack.presented(request.headers());
    if matches!(presented, Presented::Rejected) {
        admission.stack.note_rejected(client, request.uri().path());
    }
    let _client_slot = match (&presented, client) {
        (Presented::Recognized(_), _) | (_, None) => None,
        (_, Some(ip)) => match admission.enter(ip) {
            Some(slot) => Some(slot),
            None => return too_many_concurrent(),
        },
    };

    let (parts, body) = request.into_parts();
    let whole = axum::body::to_bytes(body, usize::MAX);
    let bytes = match tokio::time::timeout(admission.body_deadline, whole).await {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(error)) => return unreadable_body(&error),
        Err(_) => return body_timeout(admission.body_deadline),
    };
    let request = Request::from_parts(parts, Body::from(bytes));

    let Ok(_slot) = Arc::clone(&admission.permits).try_acquire_owned() else {
        return overloaded();
    };
    next.run(request).await
}

fn mark_stack_key_sensitive(headers: &mut HeaderMap) {
    if let header::Entry::Occupied(mut entry) = headers.entry(STACK_KEY_HEADER) {
        for value in entry.iter_mut() {
            value.set_sensitive(true);
        }
    }
}

fn refusal(status: StatusCode, body: Value) -> axum::response::Response {
    let mut response = (status, Json(body)).into_response();
    response.headers_mut().insert(
        header::RETRY_AFTER,
        HeaderValue::from(OVERLOAD_RETRY_AFTER_SECS),
    );
    response
}

fn overloaded() -> axum::response::Response {
    refusal(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({
            "error": "The facilitator is at its ceiling of concurrent requests",
            "code": "overloaded",
            "hint": "Retry after the number of seconds in the `retry-after` header. This \
                     ceiling is the machine's capacity, shared by every caller; no \
                     credential exempts it.",
        }),
    )
}

fn too_many_concurrent() -> axum::response::Response {
    refusal(
        StatusCode::TOO_MANY_REQUESTS,
        json!({
            "error": "Too many requests in flight from this address",
            "code": "too_many_concurrent_requests",
            "hint": "Wait for a response before sending more, or retry after the number of \
                     seconds in the `retry-after` header. The limit is per client IP; \
                     GET /config publishes it.",
        }),
    )
}

fn body_timeout(deadline: Duration) -> axum::response::Response {
    let body = json!({
        "error": format!(
            "The request body did not arrive within {} ms",
            deadline.as_millis()
        ),
        "code": "request_timeout",
        "hint": "Send the whole body with the headers and resend the request; nothing was done.",
    });
    let mut response = (StatusCode::REQUEST_TIMEOUT, Json(body)).into_response();
    response
        .headers_mut()
        .insert(header::CONNECTION, HeaderValue::from_static("close"));
    response
}

fn unreadable_body(error: &axum::Error) -> axum::response::Response {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = source {
        if current.is::<http_body_util::LengthLimitError>() {
            let body = json!({
                "error": "The request body is larger than this service accepts",
                "code": "payload_too_large",
                "hint": "Request bodies are JSON documents of at most 64 KiB.",
            });
            return (StatusCode::PAYLOAD_TOO_LARGE, Json(body)).into_response();
        }
        source = current.source();
    }
    let body = json!({
        "error": "The request body could not be read",
        "code": "invalid_request_body",
        "hint": "Resend the request with the whole body.",
    });
    (StatusCode::BAD_REQUEST, Json(body)).into_response()
}

// ============================================================================
// GET /config
// ============================================================================

/// The effective policy, as `GET /config` publishes it: every budget with its
/// default, its override variables and the value in force; the stack identities
/// by name; the in-flight ceiling; and the ERC-8004 daily write cap when the
/// writes are mounted. Never a key or a digest.
pub fn document(
    policy: &RatePolicy,
    admission: &Admission,
    daily_cap: Option<&crate::erc8004::daily_cap::DailyWriteCap>,
) -> Value {
    let budgets: Vec<Value> = BUDGETS
        .iter()
        .map(|budget| {
            let effective = budget.limit();
            let default = budget.default_limit();
            json!({
                "name": budget.name,
                "routes": budget.routes,
                "periodMs": effective.period.as_millis() as u64,
                "burst": effective.burst,
                "default": {
                    "periodMs": default.period.as_millis() as u64,
                    "burst": default.burst,
                },
                "env": [budget.env_period_ms, budget.env_burst],
            })
        })
        .collect();
    let daily = match daily_cap {
        Some(cap) => {
            let (default, per_network) = cap.configured();
            json!({
                "mounted": true,
                "defaultPerNetwork": default,
                "perNetwork": per_network
                    .into_iter()
                    .map(|(network, limit)| (network.to_string(), json!(limit)))
                    .collect::<serde_json::Map<String, Value>>(),
                "counts": "transactions sent per network per UTC day",
                "refusal": { "status": 429, "code": "erc8004_daily_write_limit" },
                "appliesTo": "every caller, stack identities included: it protects the gas the facilitator pays",
            })
        }
        None => json!({ "mounted": false }),
    };
    json!({
        "rateLimits": {
            "keyedOn": "client IP: the last X-Forwarded-For entry, else the TCP peer",
            "refusal": { "status": 429, "code": "rate_limited" },
            "appliesTo": "third parties; a recognized stack identity skips every budget",
            "budgets": budgets,
        },
        "stackIdentities": policy.stack().summary(),
        "overload": {
            "maxInflightRequests": admission.max(),
            "env": ENV_MAX_INFLIGHT_REQUESTS,
            "refusal": { "status": 503, "code": "overloaded", "retryAfterSecs": OVERLOAD_RETRY_AFTER_SECS },
            "appliesTo": "every caller, stack identities included: it is the machine's capacity",
            "slotTakenAfterTheBody": true,
            "neverShed": ADMISSION_EXEMPT_PATHS,
            "perClient": {
                "maxInflightRequests": admission.max_per_client(),
                "env": ENV_MAX_INFLIGHT_PER_CLIENT,
                "keyedOn": "client IP: the last X-Forwarded-For entry, else the TCP peer",
                "refusal": { "status": 429, "code": "too_many_concurrent_requests", "retryAfterSecs": OVERLOAD_RETRY_AFTER_SECS },
                "appliesTo": "third parties; a recognized stack identity skips it (a per-address limit is policy, not hardware)",
            },
            "bodyDeadlineMs": admission.body_deadline().as_millis() as u64,
            "bodyDeadlineEnv": ENV_REQUEST_BODY_DEADLINE_MS,
            "bodyRefusal": { "status": 408, "code": "request_timeout" },
            "bodyDeadlineAppliesTo": "every caller, stack identities included: the whole body, not the gap between two bytes",
        },
        "erc8004DailyWriteCap": daily,
    })
}

/// `GET /config`, serving `document` as computed at startup.
pub fn config_routes(document: Value) -> Router {
    let document = Arc::new(document);
    Router::new().route(
        "/config",
        get(move || {
            let document = Arc::clone(&document);
            async move { Json((*document).clone()) }
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request as HttpRequest;
    use axum::routing::any;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A well-formed key: `uvdsk_` and `seed` repeated to 43 characters.
    fn key(seed: &str) -> String {
        let body: String = seed.chars().cycle().take(43).collect();
        format!("{STACK_KEY_PREFIX}{body}")
    }

    fn digest_hex(key: &str) -> String {
        hex::encode(Sha256::digest(key.as_bytes()))
    }

    fn em_key() -> String {
        key("ExecutionMarket")
    }

    fn kk_key() -> String {
        key("KarmaKadabra")
    }

    /// Identities read from `vars` as if they were the environment.
    fn identities(vars: &[(&str, String)]) -> StackIdentities {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        StackIdentities::from_lookup(|var| vars.get(var).cloned())
    }

    /// Execution Market and KarmaKadabra configured, the other two listed but
    /// inactive -- the shape production will have after the keys go in.
    fn stack_policy() -> RatePolicy {
        RatePolicy::new(identities(&[
            (
                "UVD_STACK_KEY_SHA256_EXECUTION_MARKET",
                digest_hex(&em_key()),
            ),
            ("UVD_STACK_KEY_SHA256_KARMAKADABRA", digest_hex(&kk_key())),
        ]))
    }

    /// One route under `policy`, mounted exactly as `main.rs` mounts a
    /// governor: `config(limit)` and `policy.layer(..)`.
    fn governed(policy: &RatePolicy, limit: Limit) -> Router {
        Router::new()
            .route("/probe", any(|| async { "ok" }))
            .layer(policy.layer(&config(limit)))
    }

    /// `router` behind admission, as `main.rs` mounts it.
    fn admitted(router: Router, admission: &Admission) -> Router {
        router.layer(axum::middleware::from_fn_with_state(
            admission.clone(),
            admit,
        ))
    }

    /// Production's burst, a period long enough that no token comes back
    /// while a test runs: the refusal lands on request `burst + 1`, exactly.
    fn frozen(budget: &Budget) -> Limit {
        Limit {
            period: Duration::from_secs(3600),
            burst: budget.limit().burst,
        }
    }

    async fn hit(router: &Router, ip: &str, stack_keys: &[&[u8]]) -> Response<Body> {
        let mut request = HttpRequest::builder()
            .uri("/probe")
            .header("x-forwarded-for", ip)
            .body(Body::empty())
            .unwrap();
        for key in stack_keys {
            request.headers_mut().append(
                STACK_KEY_HEADER,
                HeaderValue::from_bytes(key).expect("a header value"),
            );
        }
        router.clone().oneshot(request).await.unwrap()
    }

    async fn text(response: Response<Body>) -> (StatusCode, String, String) {
        let status = response.status();
        let headers = format!("{:?}", response.headers());
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, headers, String::from_utf8_lossy(&body).into_owned())
    }

    #[test]
    fn the_fixture_keys_are_well_formed() {
        for k in [em_key(), kk_key()] {
            assert!(well_formed(k.as_bytes()), "{k}");
        }
    }

    /// The digest the facilitator compares is the one the documented operator
    /// command prints: `printf %s "$KEY" | shasum -a 256`. Pinned from shasum,
    /// not from this crate, so the vector is not compared only to itself.
    #[test]
    fn the_digest_is_what_shasum_prints() {
        assert_eq!(
            em_key(),
            "uvdsk_ExecutionMarketExecutionMarketExecutionMark"
        );
        assert_eq!(
            digest_hex(&em_key()),
            "a88278cb7097f3f26c42010284b55784001ae91bfe595e187ec782e28af42b14"
        );
        let from_shasum = identities(&[(
            "UVD_STACK_KEY_SHA256_EXECUTION_MARKET",
            "A88278CB7097F3F26C42010284B55784001AE91BFE595E187EC782E28AF42B14".to_string(),
        )]);
        let mut headers = HeaderMap::new();
        headers.insert(STACK_KEY_HEADER, em_key().parse().unwrap());
        assert_eq!(
            from_shasum.authenticate(&headers).as_deref(),
            Some("execution-market")
        );
    }

    /// The owner's rule: a stack caller that goes past the burst of EVERY
    /// budget is never refused -- and the same cadence from a third party is.
    #[tokio::test]
    async fn a_stack_identity_is_never_refused_by_any_budget() {
        let policy = stack_policy();
        let kk = kk_key();
        for budget in BUDGETS {
            let limit = frozen(budget);
            let router = governed(&policy, limit);
            for n in 1..=limit.burst + 1 {
                let response = hit(&router, "203.0.113.10", &[kk.as_bytes()]).await;
                assert_eq!(
                    response.status(),
                    StatusCode::OK,
                    "{}: stack request {n} of {}",
                    budget.name,
                    limit.burst + 1
                );
                assert_eq!(
                    response.headers()[EXEMPT_HEADER],
                    "karmakadabra",
                    "{}",
                    budget.name
                );
            }
            // The stack spent nothing from its address's bucket: a third party
            // behind the same address still has the whole burst.
            for n in 1..=limit.burst {
                let response = hit(&router, "203.0.113.10", &[]).await;
                assert_eq!(response.status(), StatusCode::OK, "{} n={n}", budget.name);
                assert!(!response.headers().contains_key(EXEMPT_HEADER));
            }
            let refused = hit(&router, "203.0.113.10", &[]).await;
            assert_eq!(
                refused.status(),
                StatusCode::TOO_MANY_REQUESTS,
                "{}: a third party was served past the burst",
                budget.name
            );
            assert!(refused.headers().contains_key("retry-after"));
        }
    }

    /// The routers `handlers` builds for itself take the same policy.
    #[tokio::test]
    async fn the_human_pages_exempt_the_stack_too() {
        let router = crate::handlers::human_page_routes_governed(
            &stack_policy(),
            Limit::every_ms(3_600_000, 2),
        );
        let fetch = |ip: &'static str, key: Option<String>| {
            let router = router.clone();
            async move {
                let mut builder = HttpRequest::builder()
                    .uri("/stats")
                    .header("x-forwarded-for", ip);
                if let Some(key) = key {
                    builder = builder.header(STACK_KEY_HEADER, key);
                }
                router
                    .oneshot(builder.body(Body::empty()).unwrap())
                    .await
                    .unwrap()
                    .status()
            }
        };
        for _ in 0..5 {
            assert_eq!(fetch("203.0.113.11", Some(em_key())).await, StatusCode::OK);
        }
        assert_eq!(fetch("203.0.113.12", None).await, StatusCode::OK);
        assert_eq!(fetch("203.0.113.12", None).await, StatusCode::OK);
        assert_eq!(
            fetch("203.0.113.12", None).await,
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    /// Emptying one service's variable revokes that service, and only it.
    #[tokio::test]
    async fn a_revoked_key_is_a_third_party_again() {
        let revoked = RatePolicy::new(identities(&[
            (
                "UVD_STACK_KEY_SHA256_EXECUTION_MARKET",
                digest_hex(&em_key()),
            ),
            ("UVD_STACK_KEY_SHA256_KARMAKADABRA", String::new()),
        ]));
        let limit = Limit::every_ms(3_600_000, 3);
        let router = governed(&revoked, limit);
        let kk = kk_key();
        for n in 1..=3 {
            let response = hit(&router, "203.0.113.20", &[kk.as_bytes()]).await;
            assert_eq!(response.status(), StatusCode::OK, "request {n}");
            assert!(!response.headers().contains_key(EXEMPT_HEADER));
        }
        let refused = hit(&router, "203.0.113.20", &[kk.as_bytes()]).await;
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);

        // Execution Market was not touched by KarmaKadabra's revocation.
        let em = em_key();
        for _ in 0..10 {
            let response = hit(&router, "203.0.113.21", &[em.as_bytes()]).await;
            assert_eq!(response.status(), StatusCode::OK);
        }
    }

    /// Two digests during a rotation, then the old one dropped.
    #[test]
    fn a_rotation_accepts_both_keys_then_only_the_new_one() {
        let old = kk_key();
        let new = key("KarmaKadabraRotated");
        let headers = |k: &str| {
            let mut h = HeaderMap::new();
            h.insert(STACK_KEY_HEADER, k.parse().unwrap());
            h
        };
        let during = identities(&[(
            "UVD_STACK_KEY_SHA256_KARMAKADABRA",
            format!("{} , {}", digest_hex(&new), digest_hex(&old)),
        )]);
        assert_eq!(
            during.authenticate(&headers(&old)).as_deref(),
            Some("karmakadabra")
        );
        assert_eq!(
            during.authenticate(&headers(&new)).as_deref(),
            Some("karmakadabra")
        );
        let after = identities(&[("UVD_STACK_KEY_SHA256_KARMAKADABRA", digest_hex(&new))]);
        assert_eq!(after.authenticate(&headers(&old)), None);
        assert_eq!(
            after.authenticate(&headers(&new)).as_deref(),
            Some("karmakadabra")
        );
    }

    /// Anything but the exact key is a third party: served until its
    /// address's burst, then 429 -- never a 500, never exempt.
    #[tokio::test]
    async fn a_malformed_or_false_key_is_a_third_party_not_a_500() {
        let kk = kk_key();
        let mut last_changed = kk.clone().into_bytes();
        *last_changed.last_mut().unwrap() = b'Z';
        let mut first_changed = kk.clone().into_bytes();
        first_changed[STACK_KEY_PREFIX.len()] = b'k';
        let variants: Vec<(&str, Vec<Vec<u8>>)> = vec![
            ("empty", vec![Vec::new()]),
            ("not a key", vec![b"not-a-key".to_vec()]),
            (
                "no prefix",
                vec![kk.as_bytes()[STACK_KEY_PREFIX.len()..].to_vec()],
            ),
            ("too short", vec![kk.as_bytes()[..kk.len() - 1].to_vec()]),
            ("a suffix", vec![format!("{kk}A").into_bytes()]),
            (
                "a prefix of it plus padding",
                vec![format!("{}AAAA", &kk[..kk.len() - 4]).into_bytes()],
            ),
            ("last character changed", vec![last_changed]),
            ("first character changed", vec![first_changed]),
            ("upper-cased", vec![kk.to_ascii_uppercase().into_bytes()]),
            ("trailing space", vec![format!("{kk} ").into_bytes()]),
            ("leading space", vec![format!(" {kk}").into_bytes()]),
            ("bearer-prefixed", vec![format!("Bearer {kk}").into_bytes()]),
            ("its digest instead", vec![digest_hex(&kk).into_bytes()]),
            (
                "non-UTF-8",
                vec![[b"uvdsk_".as_slice(), &[0xff; 43]].concat()],
            ),
            (
                "very long",
                vec![format!("{kk}{}", "A".repeat(10_000)).into_bytes()],
            ),
            (
                "the key twice on two lines",
                vec![kk.clone().into_bytes(), kk.clone().into_bytes()],
            ),
            (
                "a false key, then the real one",
                vec![key("Nobody").into_bytes(), kk.clone().into_bytes()],
            ),
            ("a false well-formed key", vec![key("Nobody").into_bytes()]),
        ];
        let policy = stack_policy();
        for (i, (what, lines)) in variants.iter().enumerate() {
            let router = governed(&policy, Limit::every_ms(3_600_000, 3));
            let ip = format!("198.51.100.{i}");
            let lines: Vec<&[u8]> = lines.iter().map(Vec::as_slice).collect();
            for n in 1..=3 {
                let response = hit(&router, &ip, &lines).await;
                assert_eq!(response.status(), StatusCode::OK, "{what}: request {n}");
                assert!(
                    !response.headers().contains_key(EXEMPT_HEADER),
                    "{what} was recognized as a stack identity"
                );
            }
            let refused = hit(&router, &ip, &lines).await;
            assert_eq!(
                refused.status(),
                StatusCode::TOO_MANY_REQUESTS,
                "{what} was not charged as a third party"
            );
        }
    }

    /// With nothing configured -- the first deploy -- a key changes nothing.
    #[tokio::test]
    async fn with_no_identity_configured_everybody_is_a_third_party() {
        let router = governed(
            &RatePolicy::new(identities(&[])),
            Limit::every_ms(3_600_000, 2),
        );
        let kk = kk_key();
        assert_eq!(
            hit(&router, "203.0.113.30", &[kk.as_bytes()])
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            hit(&router, "203.0.113.30", &[kk.as_bytes()])
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            hit(&router, "203.0.113.30", &[kk.as_bytes()])
                .await
                .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    #[test]
    fn the_service_list_and_its_digests_are_read_defensively() {
        let defaults = identities(&[]);
        let names: Vec<&str> = defaults.services.iter().map(|s| &*s.name).collect();
        assert_eq!(names, DEFAULT_STACK_SERVICES);
        assert_eq!(defaults.active(), 0);

        let custom = identities(&[
            (
                "UVD_STACK_SERVICES",
                " karmakadabra, Bad_Name, photo2melee ,karmakadabra,".to_string(),
            ),
            (
                "UVD_STACK_KEY_SHA256_KARMAKADABRA",
                format!("nothex,{}", digest_hex(&kk_key())),
            ),
            // The same digest claimed by a second service is refused there.
            ("UVD_STACK_KEY_SHA256_PHOTO2MELEE", digest_hex(&kk_key())),
            // A service not in the list is not read at all.
            (
                "UVD_STACK_KEY_SHA256_EXECUTION_MARKET",
                digest_hex(&em_key()),
            ),
        ]);
        let listed: Vec<(&str, usize)> = custom
            .services
            .iter()
            .map(|s| (&*s.name, s.digests.len()))
            .collect();
        assert_eq!(listed, [("karmakadabra", 1), ("photo2melee", 0)]);
        assert_eq!(custom.active(), 1);
        assert_eq!(
            env_var_for("describe-net"),
            "UVD_STACK_KEY_SHA256_DESCRIBE_NET"
        );
    }

    #[test]
    fn a_budget_override_replaces_only_what_parses() {
        let vars: HashMap<&str, &str> = HashMap::from([
            ("VERIFY_SETTLE_RATE_PER_MS", " 500 "),
            ("VERIFY_SETTLE_RATE_BURST", "0"),
            ("EVENTS_RATE_PER_MS", "fast"),
            ("EVENTS_RATE_BURST", "40"),
        ]);
        let lookup = |var: &str| vars.get(var).map(|v| v.to_string());
        assert_eq!(VERIFY_SETTLE.limit_from(lookup), Limit::every_ms(500, 30));
        assert_eq!(EVENTS.limit_from(lookup), Limit::every_ms(2_000, 40));
        assert_eq!(
            IDENTITY_READ.limit_from(lookup),
            IDENTITY_READ.default_limit()
        );
    }

    /// Every budget has its own override variables, and none is shared.
    #[test]
    fn every_budget_has_its_own_name_and_variables() {
        let mut seen = std::collections::HashSet::new();
        for budget in BUDGETS {
            assert!(seen.insert(budget.name), "{} twice", budget.name);
            assert!(
                seen.insert(budget.env_period_ms),
                "{} twice",
                budget.env_period_ms
            );
            assert!(seen.insert(budget.env_burst), "{} twice", budget.env_burst);
            let limit = budget.default_limit();
            assert!(
                limit.burst > 0 && !limit.period.is_zero(),
                "{}",
                budget.name
            );
        }
    }

    /// A slow request holding the only slot sheds the next one with a 503 and
    /// `Retry-After` -- a stack identity included -- while `/health` answers.
    #[tokio::test]
    async fn the_ceiling_sheds_the_stack_too() {
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let (s, r) = (Arc::clone(&started), Arc::clone(&release));
        let policy = stack_policy();
        let router = Router::new()
            .route(
                "/slow",
                get(move || {
                    let (s, r) = (Arc::clone(&s), Arc::clone(&r));
                    async move {
                        s.notify_one();
                        r.notified().await;
                        "slow"
                    }
                }),
            )
            .route("/probe", get(|| async { "ok" }))
            .layer(policy.layer(&config(Limit::every_ms(3_600_000, 100))))
            .route("/health", get(|| async { "healthy" }))
            .layer(axum::middleware::from_fn_with_state(
                Admission::new(&policy, 1, 100, Duration::from_secs(5)),
                admit,
            ));
        let send = |path: &'static str| {
            let router = router.clone();
            let key = em_key();
            async move {
                let request = HttpRequest::builder()
                    .uri(path)
                    .header("x-forwarded-for", "203.0.113.40")
                    .header(STACK_KEY_HEADER, key)
                    .body(Body::empty())
                    .unwrap();
                router.oneshot(request).await.unwrap()
            }
        };

        let slow = tokio::spawn(send("/slow"));
        started.notified().await;

        let shed = send("/probe").await;
        assert_eq!(shed.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(shed.headers()[header::RETRY_AFTER], "1");
        assert!(!shed.headers().contains_key(EXEMPT_HEADER));
        let (_, _, body) = text(shed).await;
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["code"], "overloaded");

        let health = send("/health").await;
        assert_eq!(
            health.status(),
            StatusCode::OK,
            "/health must never be shed"
        );

        release.notify_one();
        assert_eq!(slow.await.unwrap().status(), StatusCode::OK);
        let after = send("/probe").await;
        assert_eq!(
            after.status(),
            StatusCode::OK,
            "the slot was not given back"
        );
        assert_eq!(after.headers()[EXEMPT_HEADER], "execution-market");
    }

    #[test]
    fn the_ceiling_is_never_zero() {
        let none = RatePolicy::none();
        let floor = Admission::new(&none, 0, 0, Duration::ZERO);
        assert_eq!(
            (floor.max(), floor.max_per_client(), floor.body_deadline()),
            (1, 1, Duration::from_millis(1))
        );
        let set = Admission::new(&none, 7, 3, Duration::from_secs(2));
        assert_eq!(
            (set.max(), set.max_per_client(), set.body_deadline()),
            (7, 3, Duration::from_secs(2))
        );
    }

    fn config_document(policy: &RatePolicy) -> Value {
        let cap = crate::erc8004::daily_cap::DailyWriteCap::new(
            1000,
            HashMap::from([(crate::network::Network::Ethereum, 100)]),
            Box::new(|| 0),
        );
        let admission = Admission::new(policy, 512, 32, Duration::from_millis(5_000));
        document(policy, &admission, Some(&cap))
    }

    /// `GET /config` names the identities and every budget, and says what is
    /// NOT exempt.
    #[tokio::test]
    async fn the_config_document_publishes_the_policy_in_force() {
        let router = config_routes(config_document(&stack_policy()));
        let response = router
            .oneshot(
                HttpRequest::builder()
                    .uri("/config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, _, body) = text(response).await;
        assert_eq!(status, StatusCode::OK);
        let doc: Value = serde_json::from_str(&body).unwrap();

        let stack = &doc["stackIdentities"];
        assert_eq!(stack["active"], 2);
        assert_eq!(stack["header"], "X-UVD-Stack-Key");
        let services: Vec<(String, bool)> = stack["services"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    s["name"].as_str().unwrap().to_string(),
                    s["active"].as_bool().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            services,
            [
                ("execution-market".to_string(), true),
                ("karmakadabra".to_string(), true),
                ("describe-net".to_string(), false),
                ("meshrelay".to_string(), false),
            ]
        );

        let budgets: Vec<&str> = doc["rateLimits"]["budgets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["name"].as_str().unwrap())
            .collect();
        let expected: Vec<&str> = BUDGETS.iter().map(|b| b.name).collect();
        assert_eq!(budgets, expected);
        let verify = &doc["rateLimits"]["budgets"][0];
        assert_eq!(verify["periodMs"], 2000);
        assert_eq!(verify["burst"], 30);

        assert_eq!(doc["overload"]["maxInflightRequests"], 512);
        assert_eq!(doc["overload"]["refusal"]["status"], 503);
        assert_eq!(doc["erc8004DailyWriteCap"]["defaultPerNetwork"], 1000);
        assert_eq!(doc["erc8004DailyWriteCap"]["perNetwork"]["ethereum"], 100);
        let not_exempt = stack["notExemptFrom"].to_string();
        assert!(not_exempt.contains("overload") && not_exempt.contains("erc8004DailyWriteCap"));
    }

    /// A buffer the log capture writes into.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Neither the key nor its digest reaches a log line, a response header or
    /// a response body -- on the exempt path, the refused path, the rejected
    /// path, `GET /config` asked WITH the key, or an operator who pasted a raw
    /// key where a digest belongs.
    #[tokio::test]
    async fn the_key_never_reaches_a_log_or_a_response() {
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        // With ONE dispatcher registered, tracing-core decides a callsite's
        // interest from the default of whichever thread registers it first --
        // another test thread, with no subscriber, caching `never`. A second
        // live dispatcher makes every registration ask all of them, this
        // capture included.
        let _second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
        tracing::callsite::rebuild_interest_cache();

        let kk = kk_key();
        let em = em_key();
        let misplaced = key("PastedRawKey");
        let policy = RatePolicy::new(identities(&[
            ("UVD_STACK_KEY_SHA256_EXECUTION_MARKET", digest_hex(&em)),
            ("UVD_STACK_KEY_SHA256_KARMAKADABRA", digest_hex(&kk)),
            ("UVD_STACK_KEY_SHA256_MESHRELAY", misplaced.clone()),
        ]));
        let secrets = [
            kk.clone(),
            em.clone(),
            misplaced.clone(),
            digest_hex(&kk),
            digest_hex(&em),
            digest_hex(&misplaced),
        ];

        let mut seen = Vec::new();
        let admission = Admission::new(&policy, 64, 64, Duration::from_secs(5));
        let router = admitted(governed(&policy, Limit::every_ms(3_600_000, 1)), &admission);
        seen.push(text(hit(&router, "203.0.113.50", &[kk.as_bytes()]).await).await);
        // A rejected key: logged as rejected, never by value.
        let false_key = key("RejectedKey");
        seen.push(text(hit(&router, "203.0.113.51", &[false_key.as_bytes()]).await).await);
        seen.push(text(hit(&router, "203.0.113.51", &[false_key.as_bytes()]).await).await);
        assert_eq!(seen[2].0, StatusCode::TOO_MANY_REQUESTS);

        let config_router = admitted(
            config_routes(config_document(&policy))
                .layer(policy.layer(&config(Limit::every_ms(3_600_000, 5)))),
            &admission,
        );
        let request = HttpRequest::builder()
            .uri("/config")
            .header("x-forwarded-for", "203.0.113.52")
            .header(STACK_KEY_HEADER, &kk)
            .body(Body::empty())
            .unwrap();
        seen.push(text(config_router.oneshot(request).await.unwrap()).await);
        assert_eq!(seen[3].0, StatusCode::OK);
        let debug = format!("{:?}", policy.stack());

        let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        assert!(
            logs.contains("not recognized") && logs.contains("digest skipped"),
            "the capture saw nothing, so it proves nothing: {logs}"
        );
        // A rejection says who and where, so a misconfigured client is
        // diagnosable from the log alone.
        assert!(
            logs.contains("client_ip=203.0.113.51") && logs.contains("path=/probe"),
            "the rejection does not name its caller: {logs}"
        );
        for secret in &secrets {
            assert!(
                !logs.contains(secret.as_str()),
                "a log line carries {secret}"
            );
            assert!(!debug.contains(secret.as_str()), "Debug prints {secret}");
            for (status, headers, body) in &seen {
                assert!(
                    !headers.contains(secret.as_str()),
                    "{status} header: {secret}"
                );
                assert!(!body.contains(secret.as_str()), "{status} body: {secret}");
            }
        }
        assert!(!false_key.is_empty() && !logs.contains(false_key.as_str()));
    }

    /// `admit` marks the header sensitive, so a `Debug` of the request's
    /// headers anywhere below it prints `Sensitive`, not the key.
    #[tokio::test]
    async fn below_admission_the_key_debugs_as_sensitive() {
        let router = Router::new()
            .route(
                "/echo",
                get(|request: Request| async move { format!("{:?}", request.headers()) }),
            )
            .layer(axum::middleware::from_fn_with_state(
                Admission::new(&RatePolicy::none(), 4, 4, Duration::from_secs(5)),
                admit,
            ));
        let kk = kk_key();
        let request = HttpRequest::builder()
            .uri("/echo")
            .header(STACK_KEY_HEADER, &kk)
            .body(Body::empty())
            .unwrap();
        let (status, _, body) = text(router.oneshot(request).await.unwrap()).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Sensitive"), "{body}");
        assert!(!body.contains(&kk), "{body}");
    }

    /// P1-1 of REF-X4-STACK-429, over a real socket: uploads that send their
    /// headers and never their body -- twice the machine's ceiling of them --
    /// hold no slot, so the stack is served while they hang; each is answered
    /// `408` at its deadline and closed; and the ceiling is whole afterwards.
    #[tokio::test]
    async fn over_real_tcp_an_upload_that_never_arrives_holds_no_slot() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let policy = stack_policy();
        let deadline = Duration::from_millis(400);
        let router = admitted(
            Router::new()
                .route(
                    "/upload",
                    axum::routing::post(
                        |body: axum::body::Bytes| async move { body.len().to_string() },
                    ),
                )
                .route("/probe", get(|| async { "ok" }))
                .layer(policy.layer(&config(Limit::every_ms(3_600_000, 100)))),
            &Admission::new(&policy, 2, 100, deadline),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
        });

        let mut stalled = Vec::new();
        for n in 0..4 {
            let mut socket = tokio::net::TcpStream::connect(addr).await.unwrap();
            let head = format!(
                "POST /upload HTTP/1.1\r\nHost: {addr}\r\nX-Forwarded-For: 198.51.100.{n}\r\n\
                 Content-Type: application/octet-stream\r\nContent-Length: 100\r\n\r\n"
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            stalled.push(socket);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;

        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let probe = |ip: &'static str| {
            client
                .get(format!("http://{addr}/probe"))
                .header(STACK_KEY_HEADER, em_key())
                .header("x-forwarded-for", ip)
                .send()
        };
        let during = probe("203.0.113.60").await.unwrap();
        assert_eq!(
            during.status().as_u16(),
            200,
            "uploads with no body hold the ceiling: the stack was shed"
        );
        assert_eq!(during.headers()[EXEMPT_HEADER], "execution-market");

        for (n, mut socket) in stalled.into_iter().enumerate() {
            let mut answer = Vec::new();
            let read =
                tokio::time::timeout(Duration::from_secs(5), socket.read_to_end(&mut answer)).await;
            assert!(read.is_ok(), "upload {n} was never answered nor closed");
            let answer = String::from_utf8_lossy(&answer);
            assert!(
                answer.starts_with("HTTP/1.1 408"),
                "upload {n} was answered {answer}"
            );
            assert!(answer.contains("request_timeout"), "{answer}");
        }

        let after = probe("203.0.113.61").await.unwrap();
        assert_eq!(after.status().as_u16(), 200);
        let upload = client
            .post(format!("http://{addr}/upload"))
            .header("x-forwarded-for", "198.51.100.9")
            .body(vec![0u8; 100])
            .send()
            .await
            .unwrap();
        assert_eq!(upload.status().as_u16(), 200);
        assert_eq!(upload.text().await.unwrap(), "100");
        server.abort();
    }

    /// An upload whose body never comes, held in flight from `ip`.
    fn stall(router: &Router, ip: &'static str) -> tokio::task::JoinHandle<()> {
        let router = router.clone();
        tokio::spawn(async move {
            let body = Body::from_stream(tokio_stream::pending::<
                Result<axum::body::Bytes, std::io::Error>,
            >());
            let request = HttpRequest::builder()
                .method("POST")
                .uri("/upload")
                .header("x-forwarded-for", ip)
                .body(body)
                .unwrap();
            let _ = router.oneshot(request).await;
        })
    }

    async fn in_flight(admission: &Admission, ip: &str, expected: usize) {
        let ip: IpAddr = ip.parse().unwrap();
        for _ in 0..100 {
            let now = admission
                .per_client
                .lock()
                .unwrap()
                .get(&ip)
                .copied()
                .unwrap_or(0);
            if now == expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{ip} never reached {expected} in flight");
    }

    /// One address cannot fill the machine's ceiling: past its own ceiling a
    /// third party is answered `429`, another address is not, and a stack
    /// identity behind the same address skips it (c0der's decision: the
    /// per-address ceiling is policy). The slot comes back when a request ends.
    #[tokio::test]
    async fn one_address_cannot_fill_the_ceiling_and_the_stack_skips_its_limit() {
        let policy = stack_policy();
        let admission = Admission::new(&policy, 100, 2, Duration::from_secs(30));
        let router = admitted(
            Router::new()
                .route(
                    "/upload",
                    axum::routing::post(
                        |body: axum::body::Bytes| async move { body.len().to_string() },
                    ),
                )
                .route("/probe", get(|| async { "ok" })),
            &admission,
        );
        let probe = |ip: &'static str, key: Option<String>| {
            let router = router.clone();
            async move {
                let mut builder = HttpRequest::builder()
                    .uri("/probe")
                    .header("x-forwarded-for", ip);
                if let Some(key) = key {
                    builder = builder.header(STACK_KEY_HEADER, key);
                }
                router
                    .oneshot(builder.body(Body::empty()).unwrap())
                    .await
                    .unwrap()
            }
        };

        let first = stall(&router, "203.0.113.70");
        let _second = stall(&router, "203.0.113.70");
        in_flight(&admission, "203.0.113.70", 2).await;

        let refused = probe("203.0.113.70", None).await;
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(refused.headers()[header::RETRY_AFTER], "1");
        let (_, _, body) = text(refused).await;
        assert!(body.contains("too_many_concurrent_requests"), "{body}");

        assert_eq!(probe("203.0.113.71", None).await.status(), StatusCode::OK);
        assert_eq!(
            probe("203.0.113.70", Some(em_key())).await.status(),
            StatusCode::OK,
            "the stack is held to a per-address limit"
        );

        first.abort();
        in_flight(&admission, "203.0.113.70", 1).await;
        assert_eq!(probe("203.0.113.70", None).await.status(), StatusCode::OK);
    }

    /// A body cut short by the service's body limit is a JSON `413`, not a
    /// generic `400`, now that admission is what reads it.
    #[tokio::test]
    async fn a_body_past_the_limit_is_a_json_413() {
        let router = admitted(
            Router::new().route(
                "/upload",
                axum::routing::post(
                    |body: axum::body::Bytes| async move { body.len().to_string() },
                ),
            ),
            &Admission::new(&RatePolicy::none(), 4, 4, Duration::from_secs(5)),
        )
        .layer(tower_http::limit::RequestBodyLimitLayer::new(16));
        let chunked = Body::from_stream(tokio_stream::iter(vec![Ok::<_, std::io::Error>(
            axum::body::Bytes::from(vec![b'a'; 32]),
        )]));
        let request = HttpRequest::builder()
            .method("POST")
            .uri("/upload")
            .header("x-forwarded-for", "203.0.113.72")
            .body(chunked)
            .unwrap();
        let (status, _, body) = text(router.oneshot(request).await.unwrap()).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
        assert!(body.contains("payload_too_large"), "{body}");
    }

    /// A short key never authenticates, even with its exact digest configured:
    /// the format is what makes a weak key useless, not the operator's care.
    #[test]
    fn a_short_key_never_authenticates_whatever_its_digest() {
        let short = format!("{STACK_KEY_PREFIX}abcdefghij");
        let identities = identities(&[("UVD_STACK_KEY_SHA256_KARMAKADABRA", digest_hex(&short))]);
        assert_eq!(identities.active(), 1);
        let mut headers = HeaderMap::new();
        headers.insert(STACK_KEY_HEADER, short.parse().unwrap());
        assert_eq!(identities.authenticate(&headers), None);
    }

    /// The gas cap cannot learn of the stack: `daily_cap` names neither the
    /// header nor this module, so nothing in it can let an identity past it.
    /// The behavior half is `handlers::erc8004_write_rate_tests::
    /// a_stack_identity_still_spends_the_daily_gas_cap`.
    #[test]
    fn the_gas_cap_knows_nothing_of_the_stack() {
        let src = include_str!("erc8004/daily_cap.rs").to_ascii_lowercase();
        for needle in [
            "x-uvd-stack-key",
            "stack_key_header",
            "rate_policy",
            "stackidentit",
        ] {
            assert!(
                !src.contains(needle),
                "src/erc8004/daily_cap.rs mentions `{needle}`: the gas cap must not know the stack"
            );
        }
    }

    /// What the types do not hold, read from source because the buckets in
    /// `main()` are locals:
    ///
    /// * no code outside this module and `client_ip.rs` touches `tower_governor`
    ///   beyond `GovernorError` -- no `GovernorLayer`, no `GovernorConfig`, no
    ///   `use tower_governor` under another name. (A `GovernorLayer` over a
    ///   [`Bucket`] does not compile; this catches one over a config built some
    ///   other way.)
    /// * every budget is drawn on, and every bucket `main.rs` builds is sized by
    ///   one of [`BUDGETS`] and MOUNTED -- `policy.layer(&<bucket>)` at least
    ///   once. A bucket built and never mounted is a route with no limit.
    /// * admission sits inside the tracing layer, and `/config` is mounted.
    #[test]
    fn every_governor_goes_through_the_policy() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let forbidden = [
            concat!("Governor", "Layer"),
            concat!("Governor", "Config"),
            concat!("tower_governor", "::governor"),
            concat!("tower_governor", "::key_extractor"),
            concat!("extern crate ", "tower_governor"),
            concat!("tower_governor", " as "),
        ];
        let allowed_use = concat!("use tower_governor", "::GovernorError;");
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if path.extension().and_then(|e| e.to_str()) != Some("rs")
                    || name == "rate_policy.rs"
                    || name == "client_ip.rs"
                {
                    continue;
                }
                let src = std::fs::read_to_string(&path).unwrap();
                for (n, line) in src.lines().enumerate() {
                    let code = line.split("//").next().unwrap_or_default().trim();
                    let shown = format!("{}:{}", path.display(), n + 1);
                    for needle in forbidden {
                        assert!(
                            !code.contains(needle),
                            "{shown} builds or names a governor outside the rate policy: {code}"
                        );
                    }
                    assert!(
                        !code.starts_with("use tower_governor") || code == allowed_use,
                        "{shown} imports tower_governor outside the rate policy: {code}"
                    );
                }
            }
        }

        let main = include_str!("main.rs");
        let handlers = include_str!("handlers.rs");
        for budget in [
            "VERIFY_SETTLE",
            "DISCOVERY_REGISTER",
            "DISCOVERY_READ",
            "EVENTS",
            "IDENTITY_READ",
            "SECONDARY_READ",
            "HUMAN_PAGES",
            "ERC8004_WRITES",
        ] {
            let used = format!("rate_policy::{budget}.limit()");
            assert!(
                main.contains(&used) || handlers.contains(&used),
                "no governor draws on {budget}"
            );
        }
        assert_eq!(BUDGETS.len(), 8);
        // A config sized by a literal would be a budget `GET /config` does
        // not publish.
        let built = concat!("rate_policy::", "config(");
        for call in main.split(built).skip(1) {
            assert!(
                call.trim_start().starts_with("rate_policy::")
                    && call
                        .split(')')
                        .next()
                        .unwrap_or_default()
                        .ends_with(".limit("),
                "main.rs sizes a governor outside rate_policy::BUDGETS: {}",
                call.lines().next().unwrap_or_default()
            );
        }
        assert!(main.matches(built).count() >= 6);
        // Every bucket built is mounted through the policy at least once.
        let lets: Vec<&str> = main
            .split("let ")
            .skip(1)
            .filter_map(|rest| {
                let (name, value) = rest.split_once(" = ")?;
                value.trim_start().starts_with(built).then_some(name.trim())
            })
            .collect();
        assert_eq!(lets.len(), main.matches(built).count(), "{lets:?}");
        for bucket in &lets {
            assert!(
                main.contains(&format!("policy.layer(&{bucket})")),
                "main.rs builds `{bucket}` and never mounts it: its routes have no limit"
            );
        }

        let admit_at = main
            .find("rate_policy::admit")
            .expect("main.rs does not mount the in-flight ceiling");
        let tracing_at = main
            .find(".layer(telemetry.http_tracing())")
            .expect("main.rs has no tracing layer");
        assert!(
            admit_at < tracing_at,
            "the ceiling must sit inside the tracing layer so a shed request is logged"
        );
        assert!(main.contains("rate_policy::config_routes("));
    }
}
