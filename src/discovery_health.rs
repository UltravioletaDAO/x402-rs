//! Bazaar health prober (WS-B) — the "pre-ping" that keeps the curated catalog
//! alive-only.
//!
//! An x402 resource is UP iff it answers HTTP 402 (a live payment challenge).
//! A background task probes registered URLs with the SSRF-hardened
//! [`crate::discovery_security::safe_get`] connector (never attaching payment),
//! classifies the response, and drives a small hysteresis state machine so that
//! dead endpoints are quarantined (hidden from the default listing) and
//! recoveries resurface automatically. Liveness lives in a **separate overlay**
//! (`bazaar/health.json`), never inline on the resource, so imports and the
//! retention GC can never clobber it.
//!
//! Probe classification:
//! - `402` -> alive (a live x402 resource).
//! - `401/403/405/415` -> auth-gated (healthy for its design; e.g. Execution
//!   Market authenticates before 402).
//! - `200/201/429` -> degraded (responds, no payment challenge).
//! - `404/410` / dead / 5xx / DNS-fail -> fail (counts toward quarantine).
//! - SSRF-refused / template / non-http -> unprobeable.
//!
//! The request is the one the listing declares (see [`declared_request`]): a
//! listing whose `bazaar` extension says `POST` is probed with a POST carrying
//! its own example body, because a GET to a POST-only endpoint answers 405 or
//! 404 and says nothing about whether the service is up. A listing that
//! declares nothing is probed with GET, and gets ONE POST `{}` retry in the same
//! cycle when that GET answers 405 or 400.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, error, info, warn};

use crate::discovery::DiscoveryRegistry;
use crate::discovery_price::{
    normalize_declared_option, CatalogPaymentOption, DeclaredPaymentOption,
};
use crate::discovery_revalidation::RefreshReason;
use crate::discovery_security::{safe_get, safe_send_json, safe_send_mcp, SecurityReject};
use crate::discovery_terms::{
    ObservationContext, ObservationPhase, ObservedTerms, TermsProvenance, TermsTransport,
    TransportReading,
};
use crate::types_v2::{HealthState, HealthStatus, QuarantineReason, VerifiedBy};

/// Consecutive fail-class probes before a resource is quarantined.
const QUARANTINE_AFTER_FAILS: u32 = 3;
/// Consecutive alive probes to recover a quarantined resource.
const RECOVER_AFTER_OK: u32 = 2;
/// Re-probe cadence for a healthy resource (seconds).
const HEALTHY_REPROBE_SECS: u64 = 7 * 24 * 3600;
/// Backoff schedule (seconds) for quarantined resources, indexed by fail streak.
const BACKOFF_SECS: [u64; 4] = [3600, 6 * 3600, 24 * 3600, 72 * 3600];
/// Max probes issued to a single host in one tick (politeness for mega-hosts).
const MAX_PER_HOST_PER_TICK: usize = 3;
/// Probe request timeout.
const PROBE_TIMEOUT: Duration = Duration::from_secs(12);
/// User-Agent for probes.
const PROBE_UA: &str = "uvd-bazaar-health/1.0 (+https://facilitator.ultravioletadao.xyz)";
/// Longest example body the prober will send, serialized.
///
/// The body is a third party's, so it is bounded like everything else a
/// listing hands us. Past this the probe still goes out with the declared
/// method, carrying [`EMPTY_JSON_BODY`] -- the same body a listing that
/// declares no example gets. Never truncated: half a JSON document is not a
/// smaller one.
const MAX_PROBE_BODY_BYTES: usize = 8 * 1024;
/// The body of a body-method probe with no usable example.
const EMPTY_JSON_BODY: &str = "{}";
/// Most of a 402 response body the prober reads; see [`read_capped`].
const MAX_PROBE_RESPONSE_BYTES: usize = 256 * 1024;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// An HTTP method the prober sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeMethod {
    Get,
    Post,
    Put,
    Patch,
}

impl ProbeMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            ProbeMethod::Get => "GET",
            ProbeMethod::Post => "POST",
            ProbeMethod::Put => "PUT",
            ProbeMethod::Patch => "PATCH",
        }
    }

    /// Read a method as a listing declares it (or as a health record stored
    /// it). Case and surrounding whitespace are not significant.
    ///
    /// `HEAD` and `DELETE` probe as `GET`: a HEAD carries no challenge body to
    /// read, and the prober never sends a DELETE to somebody else's endpoint.
    /// Anything else is not a declaration we can act on.
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_uppercase().as_str() {
            "GET" | "HEAD" | "DELETE" => Some(ProbeMethod::Get),
            "POST" => Some(ProbeMethod::Post),
            "PUT" => Some(ProbeMethod::Put),
            "PATCH" => Some(ProbeMethod::Patch),
            _ => None,
        }
    }

    fn to_reqwest(self) -> reqwest::Method {
        match self {
            ProbeMethod::Get => reqwest::Method::GET,
            ProbeMethod::Post => reqwest::Method::POST,
            ProbeMethod::Put => reqwest::Method::PUT,
            ProbeMethod::Patch => reqwest::Method::PATCH,
        }
    }
}

/// The request a listing declares, as far as the prober honours it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeRequest {
    /// The listing declares a method: the probe uses exactly that one. A body
    /// method is sent `{}` first; `example` -- the listing's own example body,
    /// when it declared a usable one -- only when `{}` is refused with a 400 or
    /// a 422. Always `None` for GET.
    Declared {
        method: ProbeMethod,
        example: Option<String>,
    },
    /// No usable declaration. GET -- or the method this origin's own challenge
    /// or the fallback last named -- with ONE retry by the other method. See
    /// [`probe_listing`].
    Undeclared,
}

impl ProbeRequest {
    /// Request slots one probe of this request may spend on its host: 2 when
    /// it can send an extra request in the same cycle, else 1. The scheduler
    /// reserves them up front, so the extra request counts against
    /// [`MAX_PER_HOST_PER_TICK`] like any other and is never starved of it.
    fn slots(&self) -> usize {
        match self {
            ProbeRequest::Undeclared
            | ProbeRequest::Declared {
                example: Some(_), ..
            } => 2,
            ProbeRequest::Declared { .. } => 1,
        }
    }
}

/// One catalog resource, as the prober sees it.
#[derive(Debug, Clone)]
pub struct ProbeTarget {
    pub url: url::Url,
    pub resource_type: String,
    /// The offers the listing declares -- scheme, network, asset, recipient:
    /// the payTo drift baseline ([`declared_offers`]).
    pub pay_to: Vec<DeclaredOffer>,
    /// The request the listing declares.
    pub request: ProbeRequest,
}

/// One offer: what it is paid in, where, and to whom.
///
/// Two readings of the network live side by side, on purpose. `network` is the
/// conservative key the drift check uses to tell networks apart
/// ([`drift_network`], a family for non-EVM chains): coarser can only find
/// MORE drift there. `chain` is the exact CAIP-2 identifier, the identity a
/// client matches an offer by: Solana mainnet and devnet are one family and
/// two offers, and only the exact one shows that the declared offer is still
/// on sale ([`DeclaredOffer::is_offered_by`]).
///
/// Addresses are kept as written ([`canonical_address`]): only an EVM address
/// is case-folded, because only there is case a checksum and not the address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredOffer {
    /// The scheme, exactly as written: a client takes `exact`, not `EXACT`.
    pub scheme: String,
    /// [`drift_network`] of the network.
    pub network: Option<String>,
    /// The exact CAIP-2 identifier of the network.
    pub chain: String,
    pub asset: String,
    pub pay_to: String,
}

impl DeclaredOffer {
    pub fn of(option: &CatalogPaymentOption) -> Self {
        let chain = option.network.to_string();
        Self {
            scheme: option.scheme.to_string(),
            network: drift_network(&chain),
            asset: canonical_address(&chain, &option.asset.to_string()),
            pay_to: canonical_address(&chain, &option.pay_to.to_string()),
            chain,
        }
    }

    /// The offer a live option makes, read the way a client reads it: a
    /// scheme the protocol's `Scheme` takes as written (`EXACT`, ` exact `
    /// are refused), the asset and recipient as written ([`canonical_address`]:
    /// untrimmed, folded only on EVM, where normalization already refused a
    /// `0X` prefix), on the network the catalog resolved. `None` when a client could not
    /// take the option, so it never stands for a declared offer.
    fn live(raw: &serde_json::Value, option: &CatalogPaymentOption) -> Option<Self> {
        let field = |k: &str| raw.get(k).and_then(serde_json::Value::as_str);
        let scheme = field("scheme")?;
        serde_json::from_value::<crate::types::Scheme>(serde_json::Value::from(scheme)).ok()?;
        let chain = option.network.to_string();
        Some(Self {
            scheme: scheme.to_string(),
            network: drift_network(&chain),
            asset: canonical_address(&chain, field("asset")?),
            pay_to: canonical_address(&chain, field("payTo")?),
            chain,
        })
    }

    /// Whether `live` is this very offer: same scheme, same CAIP-2 network,
    /// same asset and same recipient, with no folding beyond an EVM address's.
    fn is_offered_by(&self, live: &DeclaredOffer) -> bool {
        self.scheme == live.scheme
            && self.chain == live.chain
            && self.asset == live.asset
            && self.pay_to == live.pay_to
    }

    /// Whether `pay_to`, read on a live option, is this offer's recipient --
    /// on any network: the drift check's "declared recipient". An EVM
    /// recipient matches in any case; any other only as written, or as the
    /// same parsed address.
    fn declares_recipient(&self, pay_to: &str) -> bool {
        same_address(&self.chain, &self.pay_to, pay_to)
    }

    #[cfg(test)]
    fn new(scheme: &str, network: &str, asset: &str, pay_to: &str) -> Self {
        let chain = exact_chain(network).unwrap_or_else(|| network.to_string());
        Self {
            scheme: scheme.to_string(),
            network: drift_network(network),
            asset: canonical_address(&chain, asset),
            pay_to: canonical_address(&chain, pay_to),
            chain,
        }
    }
}

/// An address as offers are compared: lowercased on an EVM chain, where case
/// is only a checksum, and exactly as written anywhere else -- a Solana mint
/// and the same string in another case are two different accounts.
fn canonical_address(chain: &str, raw: &str) -> String {
    if chain.starts_with("eip155:") {
        raw.to_ascii_lowercase()
    } else {
        raw.to_string()
    }
}

/// Whether two recipients are the same account: equal as written, equal in
/// any case on an EVM chain, or parsed to the same address.
fn same_address(chain: &str, declared: &str, live: &str) -> bool {
    if declared == live || (chain.starts_with("eip155:") && declared.eq_ignore_ascii_case(live)) {
        return true;
    }
    matches!(
        (
            crate::discovery_price::parse_catalog_address(declared),
            crate::discovery_price::parse_catalog_address(live),
        ),
        (Some(a), Some(b)) if a == b
    )
}

/// The exact CAIP-2 identifier a network spelling resolves to.
fn exact_chain(raw: &str) -> Option<String> {
    let s = raw.trim();
    crate::discovery_price::resolve_catalog_network(s)
        .map(|id| id.to_string())
        .or_else(|| crate::network::resolve_network(s).map(|n| n.to_caip2()))
}

/// The drift baseline of a catalog record: every offer its options declare.
pub fn declared_offers(accepts: &[CatalogPaymentOption]) -> Vec<DeclaredOffer> {
    accepts.iter().map(DeclaredOffer::of).collect()
}

/// One recipient a listing declares, and the network it declares it on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredRecipient {
    /// [`drift_network`] of the option's network; `None` when it names no
    /// chain family the check can tell apart.
    pub network: Option<String>,
    /// `payTo`, trimmed; lowercased only on an EVM chain
    /// ([`canonical_address`]).
    pub pay_to: String,
}

impl DeclaredRecipient {
    pub fn new(network: &str, pay_to: &str) -> Self {
        let network = drift_network(network);
        let chain = network.as_deref().unwrap_or("");
        Self {
            pay_to: canonical_address(chain, pay_to.trim()),
            network,
        }
    }
}

/// The drift baseline of a catalog record: every recipient its options declare,
/// with the network each is declared on.
pub fn declared_recipients(accepts: &[CatalogPaymentOption]) -> Vec<DeclaredRecipient> {
    accepts
        .iter()
        .map(|a| DeclaredRecipient::new(&a.network.to_string(), &a.pay_to.to_string()))
        .collect()
}

/// Chain families the drift check tells apart by their CAIP-2 namespace alone.
///
/// Within one of them every reference is the same network for this check:
/// `solana:5eykt…` and `solana:mainnet` spell one chain, and reading them as two
/// would let an option on an alias of a declared network pass as "another
/// network". Coarser can only make the drift check stricter -- and is why it
/// is never the identity of an offer ([`DeclaredOffer::chain`] is). EVM is the
/// exception, because its reference is a chain id we can read exactly.
///
/// A namespace outside this list is no chain we can name -- `aws:base`, the
/// network of Coinbase's `agent-pay` option, reads like an alias of Base -- and
/// a recipient offered on it fails closed. The list is the families this
/// facilitator settles, plus a registered CAIP-2 namespace once a live seller
/// is measured offering it: `stacks` (183 listings of one host on 2026-10-04,
/// Base, Arbitrum and Polygon to the declared address and `stacks:1` beside).
const DRIFT_FAMILIES: [&str; 9] = [
    "solana", "near", "stellar", "hedera", "fogo", "sui", "xrpl", "algorand", "stacks",
];

/// The network a recipient is offered on, as the drift check compares it.
///
/// An EVM chain is its chain id: `base`, `eip155:8453` and `eip155:08453` are
/// one network. Any other chain we can name is its family ([`DRIFT_FAMILIES`]).
/// `None` for anything else: a network we cannot name is never "another
/// network", and a recipient the listing does not declare on it is a drift.
fn drift_network(raw: &str) -> Option<String> {
    let caip2 = exact_chain(raw).unwrap_or_else(|| raw.trim().to_string());
    let (namespace, reference) = caip2.split_once(':')?;
    let namespace = namespace.trim().to_ascii_lowercase();
    if namespace == "eip155" {
        let chain_id: u64 = reference.trim().parse().ok()?;
        return Some(format!("eip155:{chain_id}"));
    }
    DRIFT_FAMILIES
        .contains(&namespace.as_str())
        .then_some(namespace)
}

impl ProbeTarget {
    /// Request slots one probe of this target may spend on its host. The MCP
    /// handshake is [`MCP_HANDSHAKE_REQUESTS`].
    fn slots(&self) -> usize {
        if self.resource_type == "mcp" {
            MCP_HANDSHAKE_REQUESTS
        } else {
            self.request.slots()
        }
    }
}

/// Choose this tick's probes: the demanded ones first, then the periodic
/// sweep. Every one is admitted against its host's request slots ([`admit`])
/// and against the tick's budget, and both are counted in REQUESTS -- a probe
/// that may send an extra one spends two -- so neither the per-host cap nor
/// `DISCOVERY_HEALTH_MAX_RPS` is exceeded by the fallback.
///
/// Returns the probes to run, and the demanded ones that did not fit, for the
/// revalidation queue to hand back next tick rather than lose.
fn plan_tick(
    tracker: &HealthTracker,
    targets: Vec<ProbeTarget>,
    demanded: Vec<(ProbeTarget, RefreshReason)>,
    max_per_tick: usize,
    now: u64,
) -> (Vec<ProbeTarget>, Vec<(String, RefreshReason)>) {
    let mut per_host: HashMap<String, usize> = HashMap::new();
    let mut spent = 0usize;
    let mut due = Vec::new();
    let mut overflow = Vec::new();
    let mut taken = std::collections::HashSet::new();
    for (target, reason) in demanded {
        taken.insert(target.url.to_string());
        if spent + target.slots() <= max_per_tick && admit(&mut per_host, &target) {
            spent += target.slots();
            due.push(target);
        } else {
            overflow.push((target.url.to_string(), reason));
        }
    }
    for target in targets {
        if spent >= max_per_tick {
            break;
        }
        if taken.contains(target.url.as_str()) || !tracker_due(tracker, &target, now) {
            continue;
        }
        if spent + target.slots() > max_per_tick || !admit(&mut per_host, &target) {
            continue;
        }
        spent += target.slots();
        due.push(target);
    }
    (due, overflow)
}

/// Whether a listing is **verified alive** -- the only thing the Bazaar exposes.
///
/// All of it, or nothing:
/// * its last probe answered `alive` (not quarantined for any reason, not
///   auth-gated, degraded, unprobeable or never probed);
/// * that probe read a valid x402 challenge in a 402 and passed the drift
///   check (`verifiedAt`), within the observed-terms freshness window;
/// * and it was the RIGHT request: the method the listing declares, or -- for a
///   listing that declares none -- whichever the probe sent (GET, or what the
///   fallback found). A POST-only listing that answered a GET does not count.
///
/// An MCP endpoint (`mcp`) is verified by its own handshake instead -- the
/// owner's rule of 2026-10-02: its last probe completed `initialize` and
/// `tools/list` and listed at least one tool, within the same window, and it is
/// not quarantined. Neither kind of evidence stands in for the other: a 402 to
/// an MCP endpoint, or a handshake recorded for an HTTP listing, verifies
/// nothing.
///
/// `observed_at` is when the observed-terms overlay last read this listing's
/// challenge; it only matters for a record from before `verifiedAt` existed
/// ([`legacy_verified_at`]).
pub fn is_verified_alive(
    state: &HealthState,
    request: &ProbeRequest,
    mcp: bool,
    observed_at: Option<u64>,
    now: u64,
    window: u64,
) -> bool {
    if mcp {
        return state.verified_by == Some(VerifiedBy::McpHandshake)
            && verified_recently(state.status, state.verified_at, now, window);
    }
    if state.verified_by == Some(VerifiedBy::McpHandshake) {
        return false;
    }
    let verified_at = state
        .verified_at
        .or_else(|| legacy_verified_at(state, observed_at));
    if !verified_recently(state.status, verified_at, now, window) {
        return false;
    }
    match request {
        ProbeRequest::Declared { method, .. } => {
            state
                .probe_method
                .as_deref()
                .and_then(ProbeMethod::parse)
                .unwrap_or(ProbeMethod::Get)
                == *method
        }
        ProbeRequest::Undeclared => true,
    }
}

/// Alive, and verified by a challenge no older than `window`.
fn verified_recently(
    status: HealthStatus,
    verified_at: Option<u64>,
    now: u64,
    window: u64,
) -> bool {
    status == HealthStatus::Alive && verified_at.is_some_and(|t| now.saturating_sub(t) <= window)
}

/// Most seconds between a probe and the observation it recorded. Both are
/// stamped inside one `probe_and_record`, a few awaits apart.
const LEGACY_OBSERVATION_SLACK_SECS: u64 = 60;

/// The verification a record written before `verifiedAt` existed can still
/// show, so a deploy does not empty the catalog until each record is probed
/// again.
///
/// Such a record (no `verifiedAt`, no method: every probe then was a GET)
/// said `alive` for any 402, readable or not. The observed-terms overlay tells
/// the two apart: a challenge is recorded there only when it could be read,
/// and by the same probe, so its `observedAt` sits beside `lastChecked`. One
/// recorded long before or after belongs to another probe and proves nothing
/// about this one. A drift is not a risk here: it quarantines, and only
/// `alive` is ever exposed.
///
/// Read-only, so every replica agrees the moment it loads both overlays. It
/// lasts one probe: [`tracker_due`] probes these records at once, and that
/// probe records a method and its own `verifiedAt`, which ends this.
pub fn legacy_verified_at(state: &HealthState, observed_at: Option<u64>) -> Option<u64> {
    if state.verified_at.is_some()
        || state.probe_method.is_some()
        || state.status != HealthStatus::Alive
        || state.http_status != Some(402)
    {
        return None;
    }
    let (checked, seen) = (state.last_checked?, observed_at?);
    (checked.abs_diff(seen) <= LEGACY_OBSERVATION_SLACK_SECS).then_some(seen)
}

/// When an alive record is probed again: the healthy cadence, but never later
/// than its verification would expire. Exposure needs a verification no older
/// than the observed-terms window, so the re-probe lands inside it, with an
/// eighth of the window to spare for a full tick or a host at its cap.
fn alive_reprobe_secs() -> u64 {
    let window = crate::discovery_terms::freshness_window_secs();
    HEALTHY_REPROBE_SECS
        .min(window.saturating_sub(window / 8))
        .max(1)
}

/// Take `target`'s request slots on its host for this tick, if they fit under
/// [`MAX_PER_HOST_PER_TICK`].
///
/// A target that may send an extra request reserves two up front, so the
/// fallback counts against the per-host cap like any other request -- and,
/// reserved before the probe rather than asked for during it, is never left
/// without room by the host's other listings.
fn admit(per_host: &mut HashMap<String, usize>, target: &ProbeTarget) -> bool {
    let host = target.url.host_str().unwrap_or_default().to_string();
    let used = per_host.entry(host).or_insert(0);
    if *used + target.slots() > MAX_PER_HOST_PER_TICK {
        return false;
    }
    *used += target.slots();
    true
}

/// Read the request a listing declares from its `bazaar` extension.
///
/// The method, from the first of these that names one:
///
/// 1. `extensions.bazaar.info.input.method` -- the x402 Bazaar spelling, and
///    what the Coinbase feed carries;
/// 2. the extension's JSON Schema: `schema.properties.input.method` (the HTTP
///    shape `uvd-x402-sdk` writes) or the `const` / first `enum` value of
///    `schema.properties.input.properties.method`;
/// 3. POST, when either half declares a body (`body` or `bodyType`) without a
///    method -- the body shape `uvd-x402-sdk` writes, and MeshRelay's.
///
/// The `resource.method` an origin puts in its own 402 is never in the catalog
/// record; the prober learns it from the live challenge instead (see
/// [`probe_and_record`]).
///
/// Only the method and the example body are used. The input also declares
/// `queryParams` and `headers`, and they are ignored on purpose: the listing is
/// a third party, and nothing it says goes into the URL or the headers we send.
/// The example must be JSON and at most [`MAX_PROBE_BODY_BYTES`] serialized,
/// or there is none.
pub fn declared_request(extensions: Option<&serde_json::Value>) -> ProbeRequest {
    let bazaar = extensions.and_then(|e| e.get("bazaar"));
    let info_input = bazaar
        .and_then(|b| b.get("info"))
        .and_then(|i| i.get("input"));
    let schema_input = bazaar
        .and_then(|b| b.get("schema"))
        .and_then(|s| s.get("properties"))
        .and_then(|p| p.get("input"));
    let method = info_input
        .and_then(|i| i.get("method"))
        .and_then(|m| m.as_str())
        .and_then(ProbeMethod::parse)
        .or_else(|| schema_input.and_then(schema_method))
        .or_else(|| declares_body(info_input, schema_input).then_some(ProbeMethod::Post));
    match method {
        None => ProbeRequest::Undeclared,
        Some(ProbeMethod::Get) => ProbeRequest::Declared {
            method: ProbeMethod::Get,
            example: None,
        },
        Some(method) => ProbeRequest::Declared {
            method,
            example: info_input.and_then(example_body),
        },
    }
}

/// The method a JSON Schema input declares: the plain `method` of the SDK's
/// HTTP shape, or the `const` / first `enum` value of `properties.method`.
fn schema_method(schema_input: &serde_json::Value) -> Option<ProbeMethod> {
    if let Some(m) = schema_input.get("method").and_then(|m| m.as_str()) {
        return ProbeMethod::parse(m);
    }
    let method = schema_input.get("properties")?.get("method")?;
    method
        .get("const")
        .and_then(|c| c.as_str())
        .or_else(|| {
            method
                .get("enum")
                .and_then(|e| e.as_array())
                .and_then(|e| e.first())
                .and_then(|m| m.as_str())
        })
        .and_then(ProbeMethod::parse)
}

/// Whether either half of the extension declares a request body.
fn declares_body(
    info_input: Option<&serde_json::Value>,
    schema_input: Option<&serde_json::Value>,
) -> bool {
    let present = |v: Option<&serde_json::Value>| v.is_some_and(|v| !v.is_null());
    let info = info_input.is_some_and(|i| present(i.get("body")) || present(i.get("bodyType")));
    let schema = schema_input
        .and_then(|s| s.get("properties"))
        .is_some_and(|p| present(p.get("body")) || present(p.get("bodyType")));
    info || schema
}

/// The request the prober sends to `url`: the one the listing declares --
/// except where only a GET may go, which gets exactly that: no body method and
/// no fallback.
///
/// * `get_only`: the owner asked for it (`probeGetOnly` in the curation
///   manifest, `config/bazaar_curation.json`).
/// * This facilitator's own origin. A listing is anybody's to write
///   (`POST /discovery/register` is open), and a body method aimed at our own
///   API -- `/register`, `/feedback`, `/settle` -- would be this service
///   calling its own write endpoints with a stranger's JSON, on a schedule.
pub fn probe_request(
    url: &url::Url,
    extensions: Option<&serde_json::Value>,
    get_only: bool,
) -> ProbeRequest {
    if get_only || is_own_origin(url) {
        return ProbeRequest::Declared {
            method: ProbeMethod::Get,
            example: None,
        };
    }
    declared_request(extensions)
}

/// Whether `url` is on this facilitator's public origin
/// ([`crate::interop::PUBLIC_URL`]), any port, any case, with or without the
/// trailing FQDN dot.
fn is_own_origin(url: &url::Url) -> bool {
    let own = url::Url::parse(crate::interop::PUBLIC_URL).ok();
    match (url.host_str(), own.as_ref().and_then(|u| u.host_str())) {
        (Some(host), Some(own)) => host.trim_end_matches('.').eq_ignore_ascii_case(own),
        _ => false,
    }
}

/// The listing's example body, serialized, when it is JSON and within bounds.
fn example_body(input: &serde_json::Value) -> Option<String> {
    // Only JSON. A form or a text body is not something this prober builds,
    // and sending one we assembled from somebody else's description would be
    // inventing a request.
    if let Some(body_type) = input.get("bodyType").filter(|t| !t.is_null()) {
        let is_json = body_type
            .as_str()
            .is_some_and(|t| t.trim().eq_ignore_ascii_case("json"));
        if !is_json {
            return None;
        }
    }
    let body = input.get("body").filter(|b| !b.is_null())?;
    let serialized = serde_json::to_string(body).ok()?;
    // `{}` is what the first probe sends anyway: not an example worth a retry.
    (serialized.len() <= MAX_PROBE_BODY_BYTES && serialized != EMPTY_JSON_BODY)
        .then_some(serialized)
}

/// Read the persisted overlay record by record. A record this build cannot
/// read -- one written by a newer build with a value it does not know -- is
/// left out and counted, rather than costing every other record; only a body
/// that is not a JSON object of records at all is an error.
fn parse_overlay(bytes: &[u8]) -> Result<HashMap<String, HealthRecord>, serde_json::Error> {
    let raw: HashMap<String, serde_json::Value> = serde_json::from_slice(bytes)?;
    let total = raw.len();
    let records: HashMap<String, HealthRecord> = raw
        .into_iter()
        .filter_map(|(url, v)| serde_json::from_value(v).ok().map(|r| (url, r)))
        .collect();
    if records.len() < total {
        warn!(
            skipped = total - records.len(),
            kept = records.len(),
            "health overlay records this build cannot read were left out"
        );
    }
    Ok(records)
}

/// Persisted per-resource liveness record (overlay `bazaar/health.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthRecord {
    pub status: HealthStatus,
    #[serde(default)]
    pub last_checked: Option<u64>,
    #[serde(default)]
    pub http_status: Option<u16>,
    #[serde(default)]
    pub latency_ms: Option<u64>,
    #[serde(default)]
    pub consecutive_ok: u32,
    #[serde(default)]
    pub consecutive_fail: u32,
    #[serde(default)]
    pub next_probe_at: u64,
    #[serde(default)]
    pub quarantined_at: Option<u64>,
    /// Cumulative probe totals (for WS-E uptime attestation).
    #[serde(default)]
    pub total_probes: u64,
    #[serde(default)]
    pub total_ok: u64,
    /// HTTP method of the last probe of an HTTP listing. Absent on a record
    /// written before the prober honoured a listing's declared method -- every
    /// one of those was a GET -- and on MCP endpoints, which are probed by
    /// their handshake whatever they declare.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_method: Option<String>,
    /// Why the record is quarantined, while it is. Absent on a record written
    /// before the reason was kept; [`Self::reason`] reads those.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quarantine_reason: Option<QuarantineReason>,
    /// The method this origin's own challenge names (`resource.method` of its
    /// 402), for a listing whose catalog record declares none. Where the next
    /// probe of such a listing starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learned_method: Option<String>,
    /// When the LAST probe verified the resource: a valid x402 challenge in a
    /// 402 that passed the drift check, or -- for an MCP endpoint -- a
    /// handshake that listed at least one tool; cleared by any probe that did
    /// not. What "verified alive" is measured from ([`is_verified_alive`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<u64>,
    /// How `verified_at` was earned. Absent with it, and on a record verified
    /// before this field existed, which only a challenge could verify.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_by: Option<VerifiedBy>,
    /// Tools the last MCP handshake listed (0 when it listed none or did not
    /// get that far). Written by every handshake of a build that asks
    /// `tools/list`, so its absence on an MCP record means no such probe has
    /// run yet ([`unverified_legacy_alive`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_tools: Option<u32>,
    /// The drift rule ([`DRIFT_RULE`]) under which the last probe of a payTo
    /// drift hold judged it. Absent (or older) on a hold the current rule has
    /// not looked at yet, which is probed once more straight away
    /// ([`probed_with_another_request`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift_rule: Option<u8>,
}

/// The drift check's revision, recorded on the holds it judges. 2 compares
/// recipients per network ([`compare_recipients`]): a build before it held a
/// listing whose challenge added a network, and those holds are looked at
/// again once.
const DRIFT_RULE: u8 = 2;

impl HealthRecord {
    /// Why the record is quarantined, `None` when it is not.
    ///
    /// A record from before the reason was kept is read by its signature: a
    /// fail-class probe never carries a 402 (402 is alive, or drift), and a
    /// drift zeroes the alive streak, so a quarantined record whose last probe
    /// answered 402 with no alive streak was put there by a payTo drift. One
    /// alive probe into a recovery (a streak of 1) reads as the fail streak it
    /// is recovering from: it has already seen one clean challenge.
    fn reason(&self) -> Option<QuarantineReason> {
        if self.status != HealthStatus::Quarantined {
            return None;
        }
        let drift_signature = self.http_status == Some(402) && self.consecutive_ok == 0;
        Some(self.quarantine_reason.unwrap_or(if drift_signature {
            QuarantineReason::PayToDrift
        } else {
            QuarantineReason::FailStreak
        }))
    }

    fn to_state(&self) -> HealthState {
        // The same number the uptime attestation publishes
        // (`HealthTracker::uptime`), so the listing and the on-chain feedback
        // can never disagree about it.
        let uptime_bps = (self.total_probes > 0)
            .then(|| ((self.total_ok as u128 * 10_000) / self.total_probes as u128) as u16);
        HealthState {
            status: self.status,
            last_checked: self.last_checked,
            http_status: self.http_status,
            latency_ms: self.latency_ms,
            uptime_bps,
            probe_count: (self.total_probes > 0).then_some(self.total_probes),
            probe_method: self.probe_method.clone(),
            quarantine_reason: self.reason(),
            verified_at: self.verified_at,
            verified_by: self
                .verified_at
                .map(|_| self.verified_by.unwrap_or(VerifiedBy::X402Challenge)),
        }
    }

    /// The method the last probe used; a record without one was a GET.
    fn last_method(&self) -> ProbeMethod {
        self.probe_method
            .as_deref()
            .and_then(ProbeMethod::parse)
            .unwrap_or(ProbeMethod::Get)
    }

    /// Held in quarantine by a payTo drift rather than by failures. No 402 lifts
    /// such a hold early, whatever the request: it keeps its two-probe recovery.
    fn held_for_drift(&self) -> bool {
        self.reason() == Some(QuarantineReason::PayToDrift)
    }
}

/// Outcome class of a single probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ProbeClass {
    #[default]
    Alive,
    AuthGated,
    Degraded,
    Fail,
    Unprobeable,
    /// The live 402 pays a recipient the listing never declared — a hijack
    /// signal. Quarantines immediately, bypassing the failure hysteresis.
    PayToDrift,
}

struct S3Overlay {
    client: aws_sdk_s3::Client,
    bucket: String,
    key: String,
}

/// In-memory health records + optional S3 overlay persistence.
pub struct HealthTracker {
    records: Arc<RwLock<HashMap<String, HealthRecord>>>,
    overlay: RwLock<Option<S3Overlay>>,
    dirty: AtomicBool,
    /// Unix seconds of the last successful upload. `0` means never.
    last_persist: AtomicU64,
    /// ETag of the overlay this process last read or wrote.
    ///
    /// Only a replica that does NOT own the discovery jobs uses it: with the
    /// job lease exactly one task probes, so this is how the others learn what
    /// it found -- a HEAD, and a read of the 5 MB object only when it moved.
    etag: RwLock<Option<String>>,
    /// Whether the records reflect the persisted overlay: true when there is
    /// none to read, once it was read, or once it was found not to exist.
    /// False from [`Self::expect_overlay`] until then.
    ///
    /// What is exposed is decided from these records, and a full catalog makes
    /// room only from what is not exposed. An overlay that could not be read
    /// makes everything look unexposed. The listing is then empty until it is
    /// read -- nothing unverified is shown -- but while this is false an import
    /// does not evict verified copies as if they were pending, and no upload
    /// overwrites the good overlay ([`Self::is_loaded`], [`Self::persist`]).
    loaded: AtomicBool,
}

impl Default for HealthTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl HealthTracker {
    pub fn new() -> Self {
        Self {
            records: Arc::new(RwLock::new(HashMap::new())),
            overlay: RwLock::new(None),
            dirty: AtomicBool::new(false),
            last_persist: AtomicU64::new(0),
            etag: RwLock::new(None),
            loaded: AtomicBool::new(true),
        }
    }

    /// Declare that a persisted overlay will be attached ([`Self::configure_s3`]),
    /// so that until it is read nothing treats the empty records as the truth.
    /// Called at startup before anything that imports into the catalog runs.
    pub fn expect_overlay(&self) {
        self.loaded.store(false, Ordering::SeqCst);
    }

    /// Whether the records reflect the persisted overlay (see `loaded`).
    pub fn is_loaded(&self) -> bool {
        self.loaded.load(Ordering::SeqCst)
    }

    /// Attach an S3 overlay and load any existing records. A read that fails
    /// for any reason other than the object not existing leaves the tracker
    /// not loaded; [`Self::persist`] tries again before it would write.
    pub async fn configure_s3(&self, bucket: String, key: String) {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = aws_sdk_s3::Client::new(&config);
        let overlay = S3Overlay {
            client,
            bucket,
            key,
        };
        self.load_overlay(&overlay).await;
        *self.overlay.write().await = Some(overlay);
    }

    /// Read the persisted overlay into the records, keeping any record this
    /// process wrote since it started (those are newer). Sets `loaded` when the
    /// object was read or does not exist; anything else leaves it as it was.
    async fn load_overlay(&self, overlay: &S3Overlay) {
        let obj = match overlay
            .client
            .get_object()
            .bucket(&overlay.bucket)
            .key(&overlay.key)
            .send()
            .await
        {
            Ok(obj) => obj,
            Err(e) => {
                if e.as_service_error().is_some_and(|s| s.is_no_such_key()) {
                    info!("No existing health overlay (starting empty)");
                    self.loaded.store(true, Ordering::SeqCst);
                } else {
                    warn!(
                        error = %e,
                        "Could not read the health overlay; nothing is exposed or evicted on an empty view until it is read"
                    );
                }
                return;
            }
        };
        // Read the version BEFORE the body: `collect()` consumes the output,
        // and an ETag taken afterwards would have to come from somewhere else.
        let etag = obj.e_tag().map(str::to_string);
        let Ok(bytes) = obj.body.collect().await else {
            warn!("Could not read the health overlay body; will retry");
            return;
        };
        match parse_overlay(&bytes.into_bytes()) {
            Ok(mut loaded) => {
                let n = loaded.len();
                let mut records = self.records.write().await;
                loaded.extend(records.drain());
                *records = loaded;
                drop(records);
                *self.etag.write().await = etag;
                self.loaded.store(true, Ordering::SeqCst);
                info!(count = n, "Loaded health overlay from S3");
            }
            Err(e) => warn!(error = %e, "Health overlay parse failed; will retry"),
        }
    }

    /// Cumulative uptime for a URL as `(uptime_bps, total_probes, total_ok)`
    /// (WS-E attestation). `None` until the URL has at least one probe.
    pub async fn uptime(&self, url: &str) -> Option<(u16, u64, u64)> {
        let records = self.records.read().await;
        let r = records.get(url)?;
        if r.total_probes == 0 {
            return None;
        }
        let bps = ((r.total_ok as u128 * 10_000) / r.total_probes as u128) as u16;
        Some((bps, r.total_probes, r.total_ok))
    }

    /// Drop records for URLs the catalog no longer holds.
    ///
    /// The overlay is keyed by URL and written WHOLE on every prober tick. It
    /// only ever grew: nothing removed a record when its resource left the
    /// catalog, so on 2026-09-10 it reached 9.8 MB -- uploaded every 60 seconds,
    /// for resources that no longer exist. Pruning it against the catalog is not
    /// a loss: a record whose URL we no longer list is a record nothing can read.
    ///
    /// # An empty catalog prunes nothing
    ///
    /// A task whose S3 read fails at startup does not stop: `main` falls back to
    /// an empty in-memory registry and keeps serving. Its prober would then
    /// offer an empty keep-set, and pruning against it would delete the whole
    /// overlay -- every resource's liveness history and the cumulative counts
    /// the uptime attestation is built from -- because one GET failed. Same rule
    /// as the catalog store's: a read we could not complete tells us nothing,
    /// and must never become a delete.
    ///
    /// Returns how many were dropped.
    pub async fn retain_urls(&self, keep: &std::collections::HashSet<String>) -> usize {
        if keep.is_empty() {
            return 0;
        }
        let mut records = self.records.write().await;
        let before = records.len();
        records.retain(|url, _| keep.contains(url));
        let dropped = before - records.len();
        drop(records);
        if dropped > 0 {
            self.dirty.store(true, Ordering::SeqCst);
        }
        dropped
    }

    /// Response-facing snapshot: url -> HealthState, for annotating listings.
    pub async fn snapshot(&self) -> HashMap<String, HealthState> {
        self.records
            .read()
            .await
            .iter()
            .map(|(u, r)| (u.clone(), r.to_state()))
            .collect()
    }

    /// Test hook: leave a URL in `status`, as the prober would have. Built
    /// through serde so it keeps compiling whatever fields the record gains.
    #[cfg(test)]
    pub(crate) async fn set_status_for_test(&self, url: &str, status: HealthStatus) {
        let record: HealthRecord = serde_json::from_value(serde_json::json!({ "status": status }))
            .expect("a health record needs only its status");
        self.records.write().await.insert(url.to_string(), record);
    }

    /// Minimum seconds between two uploads of the overlay.
    ///
    /// It was "every tick", i.e. every 60 seconds, because `dirty` is set by any
    /// probe and the sweep probes on every tick. On 2026-09-10 that was **5,0 MB
    /// uploaded every minute** -- 7 GB a day -- to record that some liveness
    /// counters moved. Nothing reads this object except a task that is starting.
    fn persist_interval_secs() -> u64 {
        crate::discovery_config::health_persist_secs()
    }

    /// Persist the overlay to S3 if it changed AND the debounce has elapsed.
    ///
    /// Never before the persisted overlay was read: an upload then would replace
    /// it with whatever this process probed since it started. Until it is read,
    /// this reads it instead.
    async fn persist(&self) {
        if !self.is_loaded() {
            let guard = self.overlay.read().await;
            if let Some(overlay) = guard.as_ref() {
                self.load_overlay(overlay).await;
            }
            return;
        }
        if !self.dirty.load(Ordering::SeqCst) {
            return;
        }
        let now = now_secs();
        let last = self.last_persist.load(Ordering::SeqCst);
        if last != 0 && now.saturating_sub(last) < Self::persist_interval_secs() {
            return;
        }
        let guard = self.overlay.read().await;
        let Some(overlay) = guard.as_ref() else {
            return;
        };
        let records = self.records.read().await;
        let body = match serde_json::to_vec(&*records) {
            Ok(b) => b,
            Err(e) => {
                error!(error = %e, "Health overlay serialize failed");
                return;
            }
        };
        drop(records);
        // Cleared HERE and nowhere earlier. An earlier clear consumes the flag
        // on every call that returns without writing -- no overlay configured,
        // or configuration not finished yet -- and the first real upload then
        // finds nothing to write. A failed PUT sets it back below.
        self.dirty.store(false, Ordering::SeqCst);
        match overlay
            .client
            .put_object()
            .bucket(&overlay.bucket)
            .key(&overlay.key)
            .body(body.into())
            .send()
            .await
        {
            Ok(out) => {
                self.last_persist.store(now, Ordering::SeqCst);
                // Remember what we wrote, so that if this task later stops
                // owning the jobs its first refresh does not re-read its own
                // object.
                *self.etag.write().await = out.e_tag().map(str::to_string);
            }
            Err(e) => {
                self.dirty.store(true, Ordering::SeqCst);
                error!(error = %e, "Failed to persist health overlay");
            }
        }
    }

    /// Re-read the overlay if the object moved. Non-owners only.
    ///
    /// The counterpart of single ownership for liveness: one task probes, and
    /// this is how the other two learn what it found. Without it a non-owner
    /// would annotate every listing with the health it loaded at boot, and
    /// quarantine decisions made hours ago would be the newest it ever had.
    ///
    /// A HEAD first, and a GET only when the ETag moved. The overlay is ~5.8 MB
    /// and usually unchanged between refreshes, so the cheap question is the
    /// one worth asking every time. Best-effort throughout: a failed refresh
    /// leaves the copy this task already has, which is exactly what it had
    /// before this existed.
    pub async fn refresh_overlay(&self) {
        let guard = self.overlay.read().await;
        let Some(overlay) = guard.as_ref() else {
            return;
        };

        let head = match overlay
            .client
            .head_object()
            .bucket(&overlay.bucket)
            .key(&overlay.key)
            .send()
            .await
        {
            Ok(head) => head,
            Err(e) => {
                debug!(error = %e, "Could not check the health overlay for changes");
                return;
            }
        };

        let latest = head.e_tag().map(str::to_string);
        // An unreadable ETag means "reload": the alternative is to skip
        // forever on a store that stops reporting one.
        if latest.is_some() && latest == *self.etag.read().await {
            return;
        }

        let obj = match overlay
            .client
            .get_object()
            .bucket(&overlay.bucket)
            .key(&overlay.key)
            .send()
            .await
        {
            Ok(obj) => obj,
            Err(e) => {
                debug!(error = %e, "Could not re-read the health overlay");
                return;
            }
        };
        let etag = obj.e_tag().map(str::to_string);
        let Ok(bytes) = obj.body.collect().await else {
            return;
        };
        match parse_overlay(&bytes.into_bytes()) {
            Ok(loaded) => {
                let n = loaded.len();
                *self.records.write().await = loaded;
                *self.etag.write().await = etag;
                self.loaded.store(true, Ordering::SeqCst);
                info!(
                    count = n,
                    "Reloaded the health overlay published by the job owner"
                );
            }
            // A parse failure must NOT empty the records: what is in memory is
            // still the last good view, and replacing it with nothing would
            // un-quarantine every dead endpoint in the catalog.
            Err(e) => warn!(error = %e, "Health overlay parse failed; keeping the current records"),
        }
    }

    /// Where the next probe of a listing that declares nothing starts: the
    /// method its own challenge named (`resource.method`), else the one the
    /// last probe used -- which, after a fallback, is the one that answered.
    async fn recorded_method(&self, url: &str) -> Option<ProbeMethod> {
        self.records
            .read()
            .await
            .get(url)
            .and_then(|r| r.learned_method.as_deref().or(r.probe_method.as_deref()))
            .and_then(ProbeMethod::parse)
    }

    /// Apply a probe result to the record for `url`, driving the state machine.
    ///
    /// `method` is the request the probe sent, for an HTTP listing; `None` for
    /// a probe the method does not describe (the MCP handshake), which leaves
    /// the record's method alone.
    async fn record_probe(
        &self,
        url: &str,
        class: ProbeClass,
        http: Option<u16>,
        latency: u64,
        method: Option<ProbeMethod>,
    ) {
        let now = now_secs();
        let mut records = self.records.write().await;
        let rec = records.entry(url.to_string()).or_insert(HealthRecord {
            status: HealthStatus::Unknown,
            last_checked: None,
            http_status: None,
            latency_ms: None,
            consecutive_ok: 0,
            consecutive_fail: 0,
            next_probe_at: 0,
            quarantined_at: None,
            total_probes: 0,
            total_ok: 0,
            probe_method: None,
            quarantine_reason: None,
            learned_method: None,
            verified_at: None,
            verified_by: None,
            mcp_tools: None,
            drift_rule: None,
        });
        // A record from before the reason was kept gets it now, read off its
        // signature while the last status code is still the one that set it.
        rec.quarantine_reason = rec.reason();
        // Evidence about one request says nothing about another. A listing
        // that declares POST and was probed with GET collected 405s and 404s
        // from a request it never serves; when the request changes, the
        // streaks restart, and a quarantine the other request built is lifted
        // by the first live challenge this one gets. Not a drift hold: that is
        // a security hold, and it keeps its own recovery rule.
        let request_changed = method.is_some_and(|m| rec.last_method() != m);
        let drift_hold = rec.held_for_drift();
        // This probe is the current rule judging the hold (or putting one in
        // place), so the one early look an older hold gets is spent.
        if drift_hold || class == ProbeClass::PayToDrift {
            rec.drift_rule = Some(DRIFT_RULE);
        }
        let quarantined_by_other_request =
            request_changed && rec.status == HealthStatus::Quarantined && !drift_hold;
        if request_changed {
            rec.consecutive_ok = 0;
            rec.consecutive_fail = 0;
        }
        if let Some(m) = method {
            rec.probe_method = Some(m.as_str().to_string());
        }
        rec.last_checked = Some(now);
        rec.http_status = http;
        rec.latency_ms = Some(latency);
        if class != ProbeClass::Unprobeable {
            rec.total_probes = rec.total_probes.saturating_add(1);
            if matches!(
                class,
                ProbeClass::Alive | ProbeClass::AuthGated | ProbeClass::Degraded
            ) {
                rec.total_ok = rec.total_ok.saturating_add(1);
            }
        }

        match class {
            // A drift hold is a security hold. An answer that is not a payment
            // challenge -- a 401, a 405, a 400 to `{}`, a hop to a private
            // address -- says nothing about who gets paid, so it cannot lift
            // it: only two clean challenges in a row do.
            ProbeClass::AuthGated | ProbeClass::Degraded | ProbeClass::Unprobeable
                if drift_hold =>
            {
                rec.consecutive_ok = 0;
                rec.next_probe_at = now + BACKOFF_SECS[BACKOFF_SECS.len() - 1];
            }
            ProbeClass::Alive => {
                rec.consecutive_ok = rec.consecutive_ok.saturating_add(1);
                rec.consecutive_fail = 0;
                let recovering = rec.status == HealthStatus::Quarantined;
                if !recovering
                    || quarantined_by_other_request
                    || rec.consecutive_ok >= RECOVER_AFTER_OK
                {
                    rec.status = HealthStatus::Alive;
                    rec.quarantined_at = None;
                    rec.next_probe_at = now + alive_reprobe_secs();
                } else {
                    // Still quarantined but recovering — re-probe soon to confirm.
                    rec.next_probe_at = now + BACKOFF_SECS[0];
                }
            }
            ProbeClass::AuthGated => {
                rec.consecutive_fail = 0;
                rec.status = HealthStatus::AuthGated;
                rec.quarantined_at = None;
                rec.next_probe_at = now + HEALTHY_REPROBE_SECS;
            }
            ProbeClass::Degraded => {
                rec.consecutive_fail = 0;
                rec.status = HealthStatus::Degraded;
                rec.next_probe_at = now + HEALTHY_REPROBE_SECS;
            }
            ProbeClass::Fail => {
                rec.consecutive_ok = 0;
                rec.consecutive_fail = rec.consecutive_fail.saturating_add(1);
                if rec.consecutive_fail >= QUARANTINE_AFTER_FAILS {
                    if rec.status != HealthStatus::Quarantined {
                        rec.quarantined_at = Some(now);
                        rec.quarantine_reason = Some(QuarantineReason::FailStreak);
                    }
                    rec.status = HealthStatus::Quarantined;
                }
                let idx =
                    (rec.consecutive_fail.saturating_sub(1) as usize).min(BACKOFF_SECS.len() - 1);
                rec.next_probe_at = now + BACKOFF_SECS[idx];
            }
            ProbeClass::Unprobeable => {
                rec.status = HealthStatus::Unprobeable;
                rec.next_probe_at = now + HEALTHY_REPROBE_SECS;
            }
            ProbeClass::PayToDrift => {
                // Security event, not a liveness event: quarantine on the first
                // observation rather than after the usual failure streak.
                rec.consecutive_ok = 0;
                rec.consecutive_fail = QUARANTINE_AFTER_FAILS;
                if rec.status != HealthStatus::Quarantined {
                    rec.quarantined_at = Some(now);
                }
                rec.status = HealthStatus::Quarantined;
                rec.quarantine_reason = Some(QuarantineReason::PayToDrift);
                rec.next_probe_at = now + BACKOFF_SECS[BACKOFF_SECS.len() - 1];
            }
        }
        if rec.status != HealthStatus::Quarantined {
            rec.quarantine_reason = None;
        }
        self.dirty.store(true, Ordering::SeqCst);
    }

    /// Record whether the probe just recorded for `url` verified it, and how: a
    /// 402 whose challenge we could read and that passed the drift check, or an
    /// MCP handshake that listed a tool. `None` -- any other outcome -- clears
    /// it: "verified alive" is about the LAST probe, not the best one.
    async fn note_verified(&self, url: &str, by: Option<VerifiedBy>) {
        if let Some(rec) = self.records.write().await.get_mut(url) {
            let at = by.map(|_| now_secs());
            if rec.verified_at != at || rec.verified_by != by {
                rec.verified_at = at;
                rec.verified_by = by;
                self.dirty.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Record how many tools the MCP handshake just recorded for `url` listed.
    async fn note_mcp_tools(&self, url: &str, tools: u32) {
        if let Some(rec) = self.records.write().await.get_mut(url) {
            if rec.mcp_tools != Some(tools) {
                rec.mcp_tools = Some(tools);
                self.dirty.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Record a verified-alive probe of `url` with `method`, as the prober does
    /// when a 402 with a readable challenge answers the right request. For
    /// tests (this crate's and the integration suite) that need a listing to be
    /// exposed without a network round trip.
    #[doc(hidden)]
    #[allow(dead_code)]
    pub async fn mark_verified(&self, url: &str, method: ProbeMethod) {
        self.record_probe(url, ProbeClass::Alive, Some(402), 1, Some(method))
            .await;
        self.note_verified(url, Some(VerifiedBy::X402Challenge))
            .await;
    }

    /// Record an MCP handshake of `url` that listed `tools` tools, as the
    /// prober does; verified when it listed at least one. For tests, like
    /// [`Self::mark_verified`].
    #[doc(hidden)]
    #[allow(dead_code)]
    pub async fn mark_mcp_handshake(&self, url: &str, tools: u32) {
        self.record_probe(url, ProbeClass::Alive, Some(200), 1, None)
            .await;
        self.note_mcp_tools(url, tools).await;
        self.note_verified(url, (tools > 0).then_some(VerifiedBy::McpHandshake))
            .await;
    }

    /// Cumulative uptime aggregated over the URLs starting with `prefix` that
    /// are verified alive right now -- a curated product usually owns many
    /// resource URLs (all of MeshRelay's channels, every Tenjin article), so its
    /// attested uptime is the aggregate rather than one representative URL --
    /// and `None` when none is: an attestation, like every other public surface
    /// of the Bazaar, speaks only for what is exposed.
    pub async fn uptime_prefix_verified(
        &self,
        prefix: &str,
        now: u64,
        window: u64,
    ) -> Option<(u16, u64, u64)> {
        let records = self.records.read().await;
        let (mut probes, mut oks) = (0u64, 0u64);
        for (url, r) in records.iter() {
            if url.starts_with(prefix) && verified_recently(r.status, r.verified_at, now, window) {
                probes = probes.saturating_add(r.total_probes);
                oks = oks.saturating_add(r.total_ok);
            }
        }
        if probes == 0 {
            return None;
        }
        let bps = ((oks as u128 * 10_000) / probes as u128) as u16;
        Some((bps, probes, oks))
    }

    /// Remember the method this origin's own challenge names, for a listing
    /// whose catalog record declares none. Forget it with `None`.
    async fn learn_method(&self, url: &str, method: Option<ProbeMethod>) {
        let learned = method.map(|m| m.as_str().to_string());
        if let Some(rec) = self.records.write().await.get_mut(url) {
            if rec.learned_method != learned {
                rec.learned_method = learned;
                self.dirty.store(true, Ordering::SeqCst);
            }
        }
    }
}

/// JSON-RPC `initialize` handshake used to probe MCP endpoints, which answer
/// POST-only JSON-RPC rather than a bare GET 402.
const MCP_INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"uvd-bazaar-health","version":"1.0"}}}"#;

/// The notification a client sends once `initialize` succeeded; a server may
/// refuse other requests until it arrives.
const MCP_INITIALIZED: &str = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;

/// The read-only request whose answer verifies an MCP endpoint: the tools it
/// serves. Nothing is called; listing is all.
const MCP_TOOLS_LIST: &str = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;

/// Requests one MCP handshake sends, what its probe reserves on its host and
/// on the tick's budget ([`ProbeTarget::slots`]).
const MCP_HANDSHAKE_REQUESTS: usize = 3;

/// What an MCP endpoint's probe found.
#[derive(Debug, Default)]
struct McpProbe {
    class: ProbeClass,
    /// The status of the `initialize` answer: what the record shows.
    http: Option<u16>,
    latency_ms: u64,
    /// Tools `tools/list` returned, when the handshake got that far and the
    /// answer was a JSON-RPC result carrying a `tools` array; `None` otherwise.
    tools: Option<u32>,
}

/// Probe an MCP endpoint by its handshake: `initialize`, the `initialized`
/// notification, then `tools/list`, each a fixed JSON-RPC body of ours, with
/// the session id the server assigned (if any) sent back to it.
///
/// Liveness is the `initialize` answer, classified as it always was (a 2xx
/// JSON-RPC reply, or a 402, is live). VERIFIED is more: the owner's rule of
/// 2026-10-02 -- `initialize` returned a JSON-RPC result and `tools/list`
/// listed at least one tool. An answer is read up to
/// [`MAX_PROBE_RESPONSE_BYTES`], as JSON or as an event stream.
async fn probe_mcp<T: ProbeTransport + ?Sized>(transport: &T, url: &url::Url) -> McpProbe {
    let start = std::time::Instant::now();
    let result = transport.send_mcp(url, MCP_INITIALIZE, None).await;
    let latency_ms = start.elapsed().as_millis() as u64;
    let resp = match result {
        Ok(resp) => resp,
        Err(SecurityReject::DisallowedAddress(_))
        | Err(SecurityReject::Scheme(_))
        | Err(SecurityReject::Userinfo)
        | Err(SecurityReject::Port(_))
        | Err(SecurityReject::NoHost) => {
            return McpProbe {
                class: ProbeClass::Unprobeable,
                latency_ms,
                ..McpProbe::default()
            }
        }
        Err(_) => {
            return McpProbe {
                class: ProbeClass::Fail,
                latency_ms,
                ..McpProbe::default()
            }
        }
    };
    let code = resp.status().as_u16();
    let class = match code {
        402 => ProbeClass::Alive,
        // A JSON-RPC handshake that the server answers is a live MCP
        // service — that is this resource type's healthy signal.
        200 | 201 => ProbeClass::Alive,
        401 | 403 | 405 | 415 => ProbeClass::AuthGated,
        429 => ProbeClass::Degraded,
        404 | 410 => ProbeClass::Fail,
        c if (500..600).contains(&c) => ProbeClass::Fail,
        _ => ProbeClass::Degraded,
    };
    let tools = if matches!(code, 200 | 201) {
        mcp_list_tools(transport, url, resp).await
    } else {
        None
    };
    McpProbe {
        class,
        http: Some(code),
        latency_ms,
        tools,
    }
}

/// The rest of the handshake, after an `initialize` that answered 2xx: how
/// many tools `tools/list` returned, or `None` when any step did not give a
/// JSON-RPC result, or an answer read was not the endpoint's own
/// ([`own_response`]).
async fn mcp_list_tools<T: ProbeTransport + ?Sized>(
    transport: &T,
    url: &url::Url,
    initialized: reqwest::Response,
) -> Option<u32> {
    if !own_response(url, &initialized) {
        return None;
    }
    // Whatever answered `initialize` assigned it; it is sent back only on
    // requests to the listing's own host (`discovery_security::hop_headers`).
    let session = initialized
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let content_type = response_content_type(&initialized);
    let body = read_capped(initialized, MAX_PROBE_RESPONSE_BYTES).await?;
    jsonrpc_result(&body, content_type.as_deref(), 1)?;

    // A notification has no answer to read; whatever status it gets, the
    // listing below says whether the server is serving.
    let _ = transport
        .send_mcp(url, MCP_INITIALIZED, session.as_deref())
        .await;

    let listed = transport
        .send_mcp(url, MCP_TOOLS_LIST, session.as_deref())
        .await
        .ok()?;
    if !listed.status().is_success() || !own_response(url, &listed) {
        return None;
    }
    let content_type = response_content_type(&listed);
    let body = read_capped(listed, MAX_PROBE_RESPONSE_BYTES).await?;
    let result = jsonrpc_result(&body, content_type.as_deref(), 2)?;
    // A tool is an object with a name; anything else in the array is not one.
    let tools = result
        .get("tools")?
        .as_array()?
        .iter()
        .filter(|t| {
            t.get("name")
                .and_then(|n| n.as_str())
                .is_some_and(|n| !n.trim().is_empty())
        })
        .count();
    Some(u32::try_from(tools).unwrap_or(u32::MAX))
}

fn response_content_type(resp: &reqwest::Response) -> Option<String> {
    resp.headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_ascii_lowercase())
}

/// The `result` object of the JSON-RPC response with `id`, from a body that is
/// either one JSON message or an event stream (`text/event-stream`) whose
/// `data:` lines carry messages. An `error` response, another id, or anything
/// unreadable is `None`.
fn jsonrpc_result(body: &str, content_type: Option<&str>, id: u64) -> Option<serde_json::Value> {
    let answer = |message: serde_json::Value| -> Option<serde_json::Value> {
        if message.get("id").and_then(|v| v.as_u64()) != Some(id) || message.get("error").is_some()
        {
            return None;
        }
        message.get("result").filter(|r| r.is_object()).cloned()
    };
    let streamed = content_type.is_some_and(|c| c.starts_with("text/event-stream"));
    if !streamed {
        return answer(serde_json::from_str(body.trim()).ok()?);
    }
    body.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter_map(|data| serde_json::from_str::<serde_json::Value>(data.trim()).ok())
        .find_map(answer)
}

/// The payment terms a live 402 advertised, and -- separately -- whether we
/// managed to read them at all.
///
/// The distinction is the whole point. "The recipients match" and "we could not
/// find any recipients" are different answers, and collapsing them is what let
/// the hijack check pass silently on every resource we probe.
///
/// It used to carry the recipients and nothing else, which is why a resource
/// could be marked alive and go on advertising a price from months ago (F4):
/// the one component that actually reads a live challenge was throwing away
/// every field except `payTo`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LiveTerms {
    /// `payTo` recipients, lowercased. The union of BOTH transports: a hijack
    /// declared anywhere in the challenge is a hijack.
    pub pay_to: Vec<String>,
    /// The same recipients, list by list -- each transport's `accepts` and
    /// `paymentRequirements` apart -- each with the network its option names
    /// ([`drift_network`]). What the drift check judges, one list at a time
    /// ([`compare_recipients`]): a client pays from one list, so an offer in
    /// another cannot vouch for it.
    pub by_transport: Vec<TransportRecipients>,
    /// Whether a parseable x402 challenge was found in either transport.
    pub readable: bool,
    /// The full requirements from the transport that won. Never a blend of the
    /// two: fields taken from different transports compose an offer nobody made.
    pub accepts: Vec<CatalogPaymentOption>,
    /// Which transport `accepts` came from.
    pub transport: Option<TermsTransport>,
    /// Protocol version that transport declared, when it declared one.
    pub x402_version: Option<u64>,
    /// The losing transport's reading, kept whenever the two disagreed.
    pub conflict: Option<TransportReading>,
    /// Options in the challenge we could not read, counted by cause.
    pub rejected: BTreeMap<String, usize>,
    /// The method the winning transport's `resource` object names, when it
    /// names one (`resource.method` of an x402 v2 challenge).
    pub resource_method: Option<ProbeMethod>,
    /// The host of that `resource.url`, lowercased.
    pub resource_host: Option<String>,
}

impl LiveTerms {
    /// Whether this reading can verify a listing: a challenge we read that
    /// offers at least one payment option we could normalize. `readable` is
    /// enough for the drift check -- a recipient anywhere in it counts -- but a
    /// challenge with no option in it offers nothing to pay.
    pub fn offers_payment(&self) -> bool {
        self.readable && !self.accepts.is_empty()
    }
}

/// One list of options (`accepts` or `paymentRequirements` of one
/// transport), as the drift check reads it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TransportRecipients {
    /// Every recipient the document names -- in any option, readable or not,
    /// and in a v1 top-level `payTo` -- with the network its option names.
    pub named: Vec<DeclaredRecipient>,
    /// The options we could read as an offer ([`normalize_declared_option`])
    /// whose network is spelled the way a client reads it
    /// ([`spelled_for_clients`]). Only one of these, equal to an offer the
    /// listing declares, can show that a declared recipient is still being
    /// paid: a recipient in an option nobody can take -- unreadable, loose in
    /// the document, under another scheme or asset, or a chain spelled so no
    /// client recognizes it -- is a mention.
    pub payable: Vec<DeclaredOffer>,
}

/// One transport's reading of a challenge, before the two are reconciled.
#[derive(Debug, Default, PartialEq, Eq)]
struct ChallengeReading {
    pay_to: Vec<String>,
    /// One entry per list key the document carries, never merged.
    lists: Vec<TransportRecipients>,
    accepts: Vec<CatalogPaymentOption>,
    x402_version: Option<u64>,
    rejected: BTreeMap<String, usize>,
    resource_method: Option<ProbeMethod>,
    resource_host: Option<String>,
    /// Whether the document looked like an x402 challenge at all. A body that
    /// parses as JSON but carries no payment terms -- a free preview, an error
    /// object -- has not been read.
    found_shape: bool,
}

/// Read the payment terms a live 402 advertises, from whichever transport
/// carries them.
///
/// x402 allows the challenge in EITHER transport and sellers pick freely:
///
/// * base64 JSON in the `PAYMENT-REQUIRED` (or `X-PAYMENT-REQUIRED`) header
/// * JSON in the response body
///
/// This read the body only, and measured against production on 2026-08-20 that
/// was the wrong half: of 40 real Bazaar resources, **36 of 36 that answered
/// 402 carried the terms in the header and none in the body**. On Tenjin the
/// body is the free preview of the article -- perfectly valid JSON with no
/// payment terms in it at all -- so the parse succeeded and returned nothing.
///
/// Reported by an external prober measuring our catalog's walls.
///
/// # When the two transports disagree
///
/// They are not merged. A record whose network came from a header and whose
/// amount came from a body describes an offer neither document made, and it
/// would be indistinguishable from a real one afterwards. Instead:
///
/// 1. A transport offering an option we can read wins over one offering none
///    -- the other could only ever hide a payable offer.
/// 2. Then the higher declared `x402Version` wins -- a seller serving two
///    protocol versions is telling us which one is current by numbering it.
/// 3. On a tie, or with no version declared, the header wins, because that is
///    where sellers actually put the challenge.
/// 4. The loser is preserved whole, in `conflict`, as evidence.
///
/// `pay_to` stays the union of both, deliberately: the hijack check must fire
/// on a recipient declared anywhere in the response, whichever transport the
/// terms were finally taken from.
fn pay_to_from_402(body: Option<&str>, header: Option<&str>) -> LiveTerms {
    let from_header = header
        .and_then(decode_payment_required)
        .map(|v| read_challenge(&v))
        .filter(|r| r.found_shape);
    let from_body = body
        .and_then(|b| serde_json::from_str::<serde_json::Value>(b).ok())
        .map(|v| read_challenge(&v))
        .filter(|r| r.found_shape);

    let mut terms = LiveTerms::default();
    for reading in [from_header.as_ref(), from_body.as_ref()]
        .into_iter()
        .flatten()
    {
        terms.readable = true;
        for p in &reading.pay_to {
            if !terms.pay_to.contains(p) {
                terms.pay_to.push(p.clone());
            }
        }
        terms.by_transport.extend(reading.lists.iter().cloned());
    }
    if !terms.readable {
        return terms;
    }

    let (winner, winning_transport, loser, losing_transport) = match (from_header, from_body) {
        (Some(h), Some(b)) => {
            // An offer we can read beats one we cannot; then the version.
            let header_wins = match (h.accepts.is_empty(), b.accepts.is_empty()) {
                (false, true) => true,
                (true, false) => false,
                _ => !matches!((h.x402_version, b.x402_version), (Some(hv), Some(bv)) if bv > hv),
            };
            if header_wins {
                (h, TermsTransport::Header, Some(b), TermsTransport::Body)
            } else {
                (b, TermsTransport::Body, Some(h), TermsTransport::Header)
            }
        }
        (Some(h), None) => (h, TermsTransport::Header, None, TermsTransport::Body),
        (None, Some(b)) => (b, TermsTransport::Body, None, TermsTransport::Header),
        (None, None) => unreachable!("readable implies at least one reading"),
    };

    terms.transport = Some(winning_transport);
    terms.resource_method = winner.resource_method;
    terms.resource_host = winner.resource_host;
    terms.x402_version = winner.x402_version;
    terms.accepts = winner.accepts;
    terms.rejected = winner.rejected;
    if let Some(other) = loser {
        // Only a real disagreement is worth keeping. Two transports carrying the
        // same offer is the common case and is not evidence of anything.
        if other.accepts != terms.accepts || other.x402_version != terms.x402_version {
            terms.conflict = Some(TransportReading {
                transport: losing_transport,
                x402_version: other.x402_version,
                accepts: other.accepts,
            });
        }
    }
    terms
}

/// Decode a `PAYMENT-REQUIRED` header value into the challenge it carries.
///
/// Base64 in practice; a few sellers send bare JSON, so both are accepted.
///
/// The base64 is read as forgivingly as the clients that pay it read it:
/// Node's `Buffer.from(value, "base64")` and the browser's `atob`. Measured on
/// Node 22: either alphabet (mixed too), padding optional, trailing bits
/// ignored, whitespace and every other character outside the alphabets
/// skipped, and decoding stops at the first `=`. The bytes are text the way
/// `TextDecoder` and `Buffer#toString` make them, invalid UTF-8 replaced. A
/// header a client reads and this decoder did not would leave only the body
/// to judge, and an attacker who keeps the declared offer in the body would
/// pass the hijack check with the header paying someone else.
fn decode_payment_required(raw: &str) -> Option<serde_json::Value> {
    use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
    use base64::Engine as _;
    const FORGIVING: GeneralPurpose = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new()
            .with_decode_padding_mode(DecodePaddingMode::RequireNone)
            .with_decode_allow_trailing_bits(true),
    );
    let trimmed = raw.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Some(v);
    }
    let mut symbols: Vec<u8> = trimmed
        .bytes()
        .take_while(|b| *b != b'=')
        .filter_map(|b| match b {
            b'-' => Some(b'+'),
            b'_' => Some(b'/'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/' => Some(b),
            _ => None,
        })
        .collect();
    // A lone final symbol carries no whole byte; Buffer drops it.
    if symbols.len() % 4 == 1 {
        symbols.pop();
    }
    let decoded = FORGIVING.decode(&symbols).ok()?;
    serde_json::from_str(&String::from_utf8_lossy(&decoded)).ok()
}

/// Read one challenge document: its recipients, its full requirements and the
/// protocol version it declares.
///
/// `found_shape` is set only when the value actually looks like an x402
/// challenge. A body that parses as JSON but carries no payment terms -- a free
/// preview, an error object -- must NOT count as "read": that is exactly the
/// case that made the hijack check pass while seeing nothing.
///
/// Requirements go through [`normalize_declared_option`], the same single
/// normalization rule the aggregator, the crawler and `POST /discovery/register`
/// use. An observation parsed by its own private rules would be comparable with
/// nothing -- and would be free to invent the `exact` that P0 removed.
fn read_challenge(v: &serde_json::Value) -> ChallengeReading {
    let mut reading = ChallengeReading::default();
    reading.x402_version = v.get("x402Version").and_then(|x| x.as_u64());
    reading.resource_method = v
        .get("resource")
        .and_then(|r| r.get("method"))
        .and_then(|m| m.as_str())
        .and_then(ProbeMethod::parse);
    reading.resource_host = v
        .get("resource")
        .and_then(|r| r.get("url"))
        .and_then(|u| u.as_str())
        .and_then(|u| url::Url::parse(u).ok())
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase));

    // `paymentRequirements` is the v1 spelling of `accepts`. Missing it made a
    // seller using it look like "no terms here" -- which is exactly the state
    // that let the hijack check pass while seeing nothing.
    //
    // Each key is its own list for the drift check: a client pays from one of
    // them, so the declared offer in `paymentRequirements` cannot show that
    // the extra option in `accepts` is still the seller's (nor the reverse).
    for key in ["accepts", "paymentRequirements"] {
        if let Some(accepts) = v.get(key).and_then(|a| a.as_array()) {
            // A list with no option in it -- `[]`, `["junk"]` -- is not one.
            if accepts.iter().any(|a| a.is_object()) {
                reading.found_shape = true;
            }
            let mut list = TransportRecipients::default();
            for a in accepts {
                if let Some(p) = a.get("payTo").and_then(drift_recipient_value) {
                    list.named.push(live_recipient(a.get("network"), &p));
                    reading.pay_to.push(p.to_ascii_lowercase());
                }
                match serde_json::from_value::<DeclaredPaymentOption>(a.clone()) {
                    Ok(declared) => match normalize_declared_option(declared) {
                        Ok(option) => {
                            if spelled_for_clients(a.get("network"), &option.network) {
                                if let Some(offer) = DeclaredOffer::live(a, &option) {
                                    list.payable.push(offer);
                                }
                            }
                            reading.accepts.push(option);
                        }
                        Err(reject) => {
                            *reading
                                .rejected
                                .entry(reject.rule().to_string())
                                .or_insert(0) += 1;
                        }
                    },
                    Err(_) => {
                        *reading
                            .rejected
                            .entry("option-malformed".to_string())
                            .or_insert(0) += 1;
                    }
                }
            }
            reading.lists.push(list);
        }
    }
    // A v1 top-level `payTo` belongs to no list, so it is named in every one.
    if let Some(p) = v.get("payTo").filter(|p| p.is_string()) {
        reading.found_shape = true;
        if let Some(p) = drift_recipient_value(p) {
            let recipient = live_recipient(v.get("network"), &p);
            if reading.lists.is_empty() {
                reading.lists.push(TransportRecipients::default());
            }
            for list in &mut reading.lists {
                list.named.push(recipient.clone());
            }
            reading.pay_to.push(p.to_ascii_lowercase());
        }
    }
    reading
}

/// Whether an option names its network the way a client reads it: exactly the
/// CAIP-2 identifier the catalog resolves it to, or an x402 v1 name as the
/// derived serde of [`crate::network::Network`] reads it -- the wire name
/// clients send. A bare chain id (`8453`) or a `Network::from_str`
/// alias (`base-mainnet`, `bnb`) resolves for the catalog, but a client
/// matching network strings would not take that option, so it cannot stand for
/// the declared offer.
fn spelled_for_clients(
    raw: Option<&serde_json::Value>,
    resolved: &crate::caip2::Caip2NetworkId,
) -> bool {
    let Some(raw) = raw.and_then(serde_json::Value::as_str) else {
        return false;
    };
    raw == resolved.to_string()
        || serde_json::from_value::<crate::network::Network>(serde_json::Value::from(raw)).is_ok()
}

/// A live recipient with the network its option names. A `network` that is
/// absent or not a string names no network, and fails closed like any other
/// network we cannot name.
fn live_recipient(network: Option<&serde_json::Value>, pay_to: &str) -> DeclaredRecipient {
    let network = network
        .and_then(serde_json::Value::as_str)
        .and_then(drift_network);
    let chain = network.as_deref().unwrap_or("");
    DeclaredRecipient {
        pay_to: canonical_address(chain, pay_to),
        network,
    }
}

/// A live `payTo`, trimmed, for the drift check -- or `None` when it is a URN.
/// Not case-folded: whether case matters is the network's to say
/// ([`canonical_address`]).
///
/// A URN (RFC 8141) names something; it is not an account and cannot receive a
/// transfer, so it cannot redirect anybody's money. The `agent-pay` option
/// carries one (`urn:x402:agent-pay:see-quote`), the catalog drops that option
/// at import (`parse_catalog_address`), so it is never in the declared set --
/// and comparing it anyway reported its seller as hijacked: one offer like that
/// in a challenge quarantined every listing that challenge guards, at once.
///
/// That is the ONLY exclusion, on purpose. The check fails closed: a payTo we
/// cannot parse is still compared, because a client may pay a spelling we do
/// not read (`0X` + hex, a chain we do not support).
fn drift_recipient(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let urn = trimmed
        .get(..4)
        .is_some_and(|p| p.eq_ignore_ascii_case("urn:"));
    (!urn).then(|| trimmed.to_string())
}

/// [`drift_recipient`] for the JSON value an option carries. A `payTo` that is
/// not a string -- a number, an object -- is no recipient we can name, and is
/// compared as written, so it counts as undeclared: failing closed. Only an
/// absent or `null` one is nothing at all.
fn drift_recipient_value(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => drift_recipient(s),
        other => Some(other.to_string()),
    }
}

/// One probe's result.
///
/// A struct rather than a tuple because it grew a sixth member: the origin's own
/// `Retry-After`. A politeness instruction that arrives and is dropped on the
/// floor is worse than not asking for one, and a six-tuple is where that happens.
#[derive(Debug, Default)]
struct ProbeOutcome {
    class: ProbeClass,
    http: Option<u16>,
    latency_ms: u64,
    /// 402 response body, when there was one.
    body: Option<String>,
    /// `PAYMENT-REQUIRED` header, when there was one.
    challenge_header: Option<String>,
    /// What the origin asked us to wait, when it asked.
    retry_after: Option<Duration>,
    /// Where the redirects it followed ended, when it followed any.
    redirect: Option<crate::discovery_security::FollowedRedirect>,
}

impl ProbeOutcome {
    /// Whether this is the listing's own answer to the request as it was sent
    /// ([`own_answer`]).
    fn answered_as_sent(&self, url: &url::Url) -> bool {
        own_answer(url, self.redirect.as_ref())
    }
}

/// Whether a response is `url`'s own answer to the request as it was sent: no
/// redirect, or only redirects that stayed on the listing's host and kept the
/// method. An answer another host gave, or one a 301/302/303 reached by
/// turning the request into a GET, answers a request the listing does not
/// declare, and verifies nothing.
fn own_answer(
    url: &url::Url,
    redirect: Option<&crate::discovery_security::FollowedRedirect>,
) -> bool {
    redirect.is_none_or(|r| {
        r.method_kept
            && r.host
                .as_deref()
                .zip(url.host_str())
                .is_some_and(|(theirs, ours)| theirs.eq_ignore_ascii_case(ours))
    })
}

/// [`own_answer`] for a response as the transport handed it over.
fn own_response(url: &url::Url, resp: &reqwest::Response) -> bool {
    own_answer(
        url,
        resp.extensions()
            .get::<crate::discovery_security::FollowedRedirect>(),
    )
}

/// How a probe reaches the network.
///
/// Production is [`SafeTransport`], the SSRF-hardened connector. The seam
/// exists for the tests: that connector refuses loopback by design, so they
/// drive the same probe logic against a local server through a client that
/// builds its requests with the same [`wire_body`] and
/// [`crate::discovery_security::json_request`].
#[async_trait::async_trait]
trait ProbeTransport: Send + Sync {
    async fn send(
        &self,
        url: &url::Url,
        method: ProbeMethod,
        body: Option<&str>,
    ) -> Result<reqwest::Response, SecurityReject>;

    /// One message of the MCP handshake: a POST of `body` (always one of ours,
    /// [`MCP_INITIALIZE`] and its two followers) with
    /// [`crate::discovery_security::mcp_headers`].
    async fn send_mcp(
        &self,
        url: &url::Url,
        body: &str,
        session: Option<&str>,
    ) -> Result<reqwest::Response, SecurityReject>;
}

/// What goes on the wire for `method`: nothing for a GET, else the method and
/// its JSON body (`{}` when there is none). Never a payment header.
fn wire_body(method: ProbeMethod, body: Option<&str>) -> Option<(reqwest::Method, String)> {
    match method {
        ProbeMethod::Get => None,
        m => Some((m.to_reqwest(), body.unwrap_or(EMPTY_JSON_BODY).to_string())),
    }
}

/// The production transport: every request through `discovery_security`, so a
/// POST gets exactly the SSRF checks, redirect handling and timeout a GET gets.
struct SafeTransport;

#[async_trait::async_trait]
impl ProbeTransport for SafeTransport {
    async fn send(
        &self,
        url: &url::Url,
        method: ProbeMethod,
        body: Option<&str>,
    ) -> Result<reqwest::Response, SecurityReject> {
        match wire_body(method, body) {
            None => safe_get(PROBE_UA, PROBE_TIMEOUT, url).await,
            Some((m, b)) => safe_send_json(PROBE_UA, PROBE_TIMEOUT, url, m, b).await,
        }
    }

    async fn send_mcp(
        &self,
        url: &url::Url,
        body: &str,
        session: Option<&str>,
    ) -> Result<reqwest::Response, SecurityReject> {
        safe_send_mcp(PROBE_UA, PROBE_TIMEOUT, url, body.to_string(), session).await
    }
}

/// A listing's probe: what it found, and with which request.
#[derive(Debug)]
struct ListingProbe {
    outcome: ProbeOutcome,
    /// The request whose answer `outcome` is.
    method: ProbeMethod,
    /// Whether that answer came from the fallback's extra request.
    fell_back: bool,
}

/// Probe one listing with the request it declares. At most ONE extra request
/// per listing per cycle, and its answer replaces the first only when it
/// [`proves`] the request: a 402 carrying a challenge we can read. Anything
/// less leaves the first answer standing.
///
/// * **Declared GET**: one GET.
/// * **Declared POST/PUT/PATCH**: that method with `{}`. Only when `{}` is
///   refused as malformed -- 400 or 422 -- and the listing declared an example,
///   the same method once more, carrying the example. A seller that does work
///   before it challenges does it with an empty body, and a stranger's example
///   travels only when the endpoint needs it to get as far as the 402.
/// * **Undeclared**: GET, or the method this origin last named or answered
///   with (`recorded`). A GET answering 405, 400 or 404 -- "not like this", or
///   the 404 an Express app gives a method it has no route for -- gets ONE
///   POST `{}`; a remembered body method refused with a 405, 400, 404 or 422
///   gets ONE GET, so a remembered method cannot get stuck. A 429, a 5xx or a
///   timeout gets nothing more: that is the origin asking to be left alone.
///
/// A 405 to the method the listing declares, or to the one it answered with
/// last time, is [`refused_expected_method`]: degraded, not auth-gated.
async fn probe_listing<T: ProbeTransport + ?Sized>(
    transport: &T,
    url: &url::Url,
    request: &ProbeRequest,
    recorded: Option<ProbeMethod>,
) -> ListingProbe {
    let (first, example, expected) = match request {
        ProbeRequest::Declared { method, example } => (*method, example.as_deref(), true),
        ProbeRequest::Undeclared => match recorded {
            Some(m) if m != ProbeMethod::Get => (m, None, true),
            _ => (ProbeMethod::Get, None, false),
        },
    };
    let mut outcome = probe_once(transport, url, first, None).await;
    if expected {
        refused_expected_method(&mut outcome);
    }
    let retry = match request {
        ProbeRequest::Declared { .. } => match example {
            Some(example) if matches!(outcome.http, Some(400) | Some(422)) => {
                Some((first, Some(example)))
            }
            _ => None,
        },
        ProbeRequest::Undeclared if first == ProbeMethod::Get => {
            matches!(outcome.http, Some(405) | Some(400) | Some(404))
                .then_some((ProbeMethod::Post, None))
        }
        // Only a refusal of the request, never a refusal of us: a 429, a 5xx or
        // a timeout is the origin asking to be left alone, not a wrong method.
        ProbeRequest::Undeclared => {
            matches!(outcome.http, Some(405) | Some(400) | Some(404) | Some(422))
                .then_some((ProbeMethod::Get, None))
        }
    };
    let Some((method, body)) = retry else {
        return ListingProbe {
            outcome,
            method: first,
            fell_back: false,
        };
    };
    let second = probe_once(transport, url, method, body).await;
    if proves(url, &second) {
        ListingProbe {
            outcome: second,
            method,
            fell_back: true,
        }
    } else {
        ListingProbe {
            outcome,
            method: first,
            fell_back: false,
        }
    }
}

/// Whether a probe proved its request: the listing's own 402 to the request as
/// sent ([`ProbeOutcome::answered_as_sent`]), carrying a challenge that offers
/// a payment ([`LiveTerms::offers_payment`]).
fn proves(url: &url::Url, outcome: &ProbeOutcome) -> bool {
    outcome.class == ProbeClass::Alive
        && outcome.answered_as_sent(url)
        && pay_to_from_402(outcome.body.as_deref(), outcome.challenge_header.as_deref())
            .offers_payment()
}

/// Read at most `cap` bytes of a response body; `None` past that.
///
/// A stranger's 402 used to be read whole. A challenge is a few KB; the free
/// preview some sellers put beside it can be an article. Past the cap the body
/// is dropped rather than truncated -- the `PAYMENT-REQUIRED` header still
/// carries the challenge, which is where sellers put it.
async fn read_capped(mut resp: reqwest::Response, cap: usize) -> Option<String> {
    if resp.content_length().is_some_and(|n| n > cap as u64) {
        return None;
    }
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await.ok()? {
        if buf.len() + chunk.len() > cap {
            return None;
        }
        buf.extend_from_slice(&chunk);
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// A 405 to the method a listing is known to take -- the one it declares, or
/// the one that answered it last time -- is the endpoint refusing the very
/// request it is listed for. It responds, so it is not dead; but it is not
/// "healthy for its design" either, which is what auth-gated means and what a
/// consumer admits as live. Degraded.
///
/// A 405 to a GET we sent on a guess stays auth-gated, as it always was: that
/// is the case the fallback exists for.
fn refused_expected_method(outcome: &mut ProbeOutcome) {
    if outcome.http == Some(405) {
        outcome.class = ProbeClass::Degraded;
    }
}

/// Classify a single request to `url` (no payment attached).
///
/// On a 402 BOTH transports are captured -- the body and the `PAYMENT-REQUIRED`
/// header -- because the caller has to check for a payTo swap and sellers put
/// the challenge in either one. Reading only the body found nothing on 36 of 36
/// live resources measured 2026-08-20.
async fn probe_once<T: ProbeTransport + ?Sized>(
    transport: &T,
    url: &url::Url,
    method: ProbeMethod,
    body: Option<&str>,
) -> ProbeOutcome {
    let start = std::time::Instant::now();
    let result = transport.send(url, method, body).await;
    let latency = start.elapsed().as_millis() as u64;
    match result {
        Ok(resp) => {
            let code = resp.status().as_u16();
            let class = match code {
                402 => ProbeClass::Alive,
                401 | 403 | 405 | 415 => ProbeClass::AuthGated,
                200 | 201 | 429 => ProbeClass::Degraded,
                404 | 410 => ProbeClass::Fail,
                c if (500..600).contains(&c) => ProbeClass::Fail,
                _ => ProbeClass::Degraded,
            };
            // Read before the body is consumed: `text()` takes the response.
            let retry_after = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(crate::discovery_revalidation::parse_retry_after);
            let redirect = resp
                .extensions()
                .get::<crate::discovery_security::FollowedRedirect>()
                .cloned();
            // Only a 402 carries payment terms worth diffing.
            let (body, header) = if code == 402 {
                let header = resp
                    .headers()
                    .get("payment-required")
                    .or_else(|| resp.headers().get("x-payment-required"))
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                (read_capped(resp, MAX_PROBE_RESPONSE_BYTES).await, header)
            } else {
                (None, None)
            };
            ProbeOutcome {
                class,
                http: Some(code),
                latency_ms: latency,
                body,
                challenge_header: header,
                retry_after,
                redirect,
            }
        }
        // A URL the SSRF connector refuses (private/template/bad-port) is not a
        // dead endpoint — it is simply not probeable this way.
        Err(SecurityReject::DisallowedAddress(_))
        | Err(SecurityReject::Scheme(_))
        | Err(SecurityReject::Userinfo)
        | Err(SecurityReject::Port(_))
        | Err(SecurityReject::NoHost) => ProbeOutcome {
            class: ProbeClass::Unprobeable,
            latency_ms: latency,
            ..ProbeOutcome::default()
        },
        // Resolution failure / connection error / redirect loop -> dead.
        Err(_) => ProbeOutcome {
            class: ProbeClass::Fail,
            latency_ms: latency,
            ..ProbeOutcome::default()
        },
    }
}

/// Start the background health prober. Wakes every `tick_secs`, probes the due
/// URLs (bounded per tick so the initial full sweep spreads over hours), and
/// debounce-persists the overlay.
pub fn start_health_task(
    registry: DiscoveryRegistry,
    tracker: Arc<HealthTracker>,
    tick_secs: u64,
    concurrency: usize,
    max_rps: u64,
) -> tokio::task::JoinHandle<()> {
    info!(
        tick_secs = tick_secs,
        concurrency = concurrency,
        max_rps = max_rps,
        "Starting Bazaar health prober"
    );
    tokio::spawn(async move {
        let sem = Arc::new(Semaphore::new(concurrency.max(1)));
        // Bound work per tick so we respect max_rps on average and spread the
        // initial ~21k sweep over hours rather than hammering all at once.
        let max_per_tick = (max_rps.max(1) * tick_secs).max(1) as usize;
        let interval = Duration::from_secs(tick_secs.max(5));
        loop {
            tokio::time::sleep(interval).await;

            // Probing is periodic discovery work: one task does it, and the
            // others read the overlay it publishes with
            // [`HealthTracker::refresh_overlay`]. Every replica probing the
            // same catalog is that many times the outbound TLS handshakes --
            // the CPU that took production down on 2026-09-10 -- and that many
            // writers of one whole-object PUT, where the last one to finish
            // erases what the others found. The 2.21.2 debounce made each
            // writer cheaper; this makes there be one.
            //
            // Note this gate covers the two overlay prunes and both persists
            // below as well, which is the point: a task that probes nothing has
            // nothing to prune and nothing to publish.
            if !crate::discovery_owner::owns_jobs() {
                continue;
            }

            let now = now_secs();

            // Collect due URLs from the registry (a plain snapshot of URLs, so
            // no registry guard is held across the probes below). Cap probes
            // per host per tick so a mega-host (e.g. orbisapi.com with thousands
            // of listings) is spread across ticks rather than hammered.
            let targets = registry.probe_targets().await;
            // The catalog is bounded now, so the overlay has to be too: a health
            // record for a URL that left the catalog is written to S3 every tick
            // and read by nobody.
            let live: std::collections::HashSet<String> =
                targets.iter().map(|t| t.url.to_string()).collect();
            let pruned = tracker.retain_urls(&live).await;
            if pruned > 0 {
                info!(
                    pruned = pruned,
                    held = live.len(),
                    "dropped health records for resources no longer in the catalog"
                );
            }
            // The observed-terms overlay is a second object written whole, so it
            // needs the same hygiene against the same keep-set.
            let pruned_terms = registry.terms().retain_urls(&live).await;
            if pruned_terms > 0 {
                info!(
                    pruned = pruned_terms,
                    held = live.len(),
                    "dropped observed terms for resources no longer in the catalog"
                );
            }
            // ----------------------------------------------------------------
            // The budget, split.
            //
            // `max_per_tick` is the SAME allowance 2.21.2 settled on. Demand
            // spends from it first; the periodic sweep keeps a reserved floor
            // so a busy resource cannot starve the long tail. Nothing here adds
            // a probe: this decides which probes the tick spends.
            // ----------------------------------------------------------------
            let queue = registry.revalidation();
            let (demand_budget, _reserved) = crate::discovery_revalidation::split_budget(
                max_per_tick,
                crate::discovery_config::long_tail_share(),
            );

            let mut demanded: Vec<(ProbeTarget, RefreshReason)> = Vec::new();
            if crate::discovery_config::revalidation_enabled() {
                // Fold in what the other replicas asked for, then take the top
                // of the queue. Both are owner-only: this whole block is behind
                // the ownership gate above.
                let folded = queue.absorb_shared(now).await;
                queue.evict_expired(now).await;
                let batch = queue.take_batch(demand_budget, now).await;
                if !batch.is_empty() || folded > 0 {
                    // Resolved BEFORE the macro: an `.await` inside a tracing
                    // macro's argument list holds a non-Send `format_args!`
                    // temporary across the suspension point.
                    let depth = queue.depth().await;
                    debug!(
                        folded_from_replicas = folded,
                        taken = batch.len(),
                        depth = depth,
                        demand_budget = demand_budget,
                        "revalidation batch"
                    );
                }
                let by_url: HashMap<String, &ProbeTarget> =
                    targets.iter().map(|t| (t.url.to_string(), t)).collect();
                for (url, reason) in batch {
                    // A queued URL that has left the catalog is simply dropped:
                    // we do not probe what we no longer list.
                    if let Some(target) = by_url.get(&url) {
                        debug!(url = %url, reason = reason.as_str(), "revalidating on demand");
                        demanded.push(((*target).clone(), reason));
                    }
                }
            }

            let (due, overflow) = plan_tick(&tracker, targets, demanded, max_per_tick, now);
            // Demand that did not fit this tick's requests goes back to the
            // queue instead of being lost; it is taken again next tick.
            for (url, reason) in overflow {
                queue.requeue(&url, reason, now).await;
            }

            if due.is_empty() {
                continue;
            }
            debug!(due = due.len(), "Health prober cycle");

            let mut handles = Vec::with_capacity(due.len());
            for target in due {
                let sem = Arc::clone(&sem);
                let tracker = Arc::clone(&tracker);
                let registry = registry.clone();
                handles.push(tokio::spawn(async move {
                    let _permit = sem.acquire().await.ok();
                    probe_and_record(&SafeTransport, &registry, &tracker, target).await;
                }));
            }
            for h in handles {
                let _ = h.await;
            }
            tracker.persist().await;
            // Its own debounce, slower than the tick: a price observed twice in
            // five minutes is the same observation, and this object is larger.
            registry.terms().persist().await;
        }
    })
}

/// Probe one target and record everything the probe found: the liveness
/// verdict, the politeness feedback for its host, and the terms its challenge
/// advertised.
///
/// One function for the scheduler and for the tests, so what the tests drive
/// is the production path -- including the method handed to the record, which
/// is what turns the one-off re-probe off again.
async fn probe_and_record<T: ProbeTransport + ?Sized>(
    transport: &T,
    registry: &DiscoveryRegistry,
    tracker: &HealthTracker,
    target: ProbeTarget,
) {
    let queue = registry.revalidation();
    let ProbeTarget {
        url: u,
        resource_type,
        pay_to: expected_pay_to,
        request,
    } = target;
    // MCP endpoints answer a POST JSON-RPC handshake, not a GET
    // 402 — probing them with GET would mark our own first-party
    // MCP services dead.
    let mut mcp_tools = None;
    let (outcome, method, fell_back) = if resource_type == "mcp" {
        let probed = probe_mcp(transport, &u).await;
        mcp_tools = Some(probed.tools.unwrap_or(0));
        let outcome = ProbeOutcome {
            class: probed.class,
            http: probed.http,
            latency_ms: probed.latency_ms,
            ..ProbeOutcome::default()
        };
        (outcome, None, false)
    } else {
        let recorded = tracker.recorded_method(u.as_str()).await;
        let probed = probe_listing(transport, &u, &request, recorded).await;
        if probed.fell_back {
            debug!(
                url = %u,
                method = probed.method.as_str(),
                "the extra request answered 402"
            );
        }
        (probed.outcome, Some(probed.method), probed.fell_back)
    };
    let answered_as_sent = outcome.answered_as_sent(&u);
    let ProbeOutcome {
        mut class,
        http,
        latency_ms: latency,
        body,
        challenge_header: pr_header,
        retry_after,
        redirect: _,
    } = outcome;

    // The challenge is read ONCE, and read whole. Two callers
    // want it and they want different halves: the hijack check
    // wants the recipients, the terms overlay wants the price.
    // Probing twice for that would double every seller's load.
    let live = if class == ProbeClass::Alive && (body.is_some() || pr_header.is_some()) {
        Some(pay_to_from_402(body.as_deref(), pr_header.as_deref()))
    } else {
        None
    };

    // payTo drift (F4): a live 402 that now pays a recipient the
    // listing never declared is a hijack signal, not a health
    // signal. Quarantine immediately and alarm.
    //
    // A changed AMOUNT is deliberately not in this branch and
    // must never be: a seller repricing is ordinary commerce,
    // and quarantining for it would hide a live resource over a
    // change it is entitled to make. The price change is
    // recorded below, as an observation.
    if !expected_pay_to.is_empty() {
        if let Some(live) = live.as_ref() {
            let verdict = compare_recipients(&expected_pay_to, live);
            if verdict == Recipients::Drifted {
                let expected: Vec<&str> =
                    expected_pay_to.iter().map(|d| d.pay_to.as_str()).collect();
                warn!(
                    url = %u,
                    expected = ?expected,
                    observed = ?live.pay_to,
                    "paytoswap: live 402 pays an undeclared recipient; quarantining"
                );
                class = ProbeClass::PayToDrift;
            } else if let Recipients::ExtraNetworks(extra) = verdict {
                // Not held: a declared recipient is still offered on its own
                // network. A recipient nobody vouched for still takes money
                // through this option, so it is logged at WARN with its network
                // and address, which is the record of it even when the catalog
                // cannot read the option. It never becomes part of the listing.
                log_extra_networks(&u, &extra);
            } else if !live.readable {
                // A check that did NOT run must not look like one
                // that passed. This is the state that hid the bug:
                // the terms were in the header, the body parsed as
                // a free preview, and the swap check quietly saw
                // nothing on every resource it examined.
                warn!(
                    url = %u,
                    has_body = body.is_some(),
                    has_header = pr_header.is_some(),
                    "paytoswap: could not read payment terms from either transport -- \
                     the hijack check did not run for this resource"
                );
            }
        }
    }

    tracker
        .record_probe(u.as_str(), class, http, latency, method)
        .await;

    // A listing whose catalog record declares nothing may still say how it is
    // called in its own challenge (`resource.method`, where MeshRelay puts
    // it), and the next probe starts there. Only a POST, only from a clean
    // challenge that names this listing's own host -- a 402 reached through a
    // redirect, or one that just failed the drift check, names nothing for us
    // -- and forgotten as soon as the fallback has to answer instead.
    if request == ProbeRequest::Undeclared {
        let named = live
            .as_ref()
            .filter(|_| class == ProbeClass::Alive)
            .filter(|l| {
                l.resource_host
                    .as_deref()
                    .zip(u.host_str())
                    .is_some_and(|(theirs, ours)| theirs.eq_ignore_ascii_case(ours))
            })
            .and_then(|l| l.resource_method)
            .filter(|m| *m == ProbeMethod::Post);
        if fell_back {
            tracker.learn_method(u.as_str(), None).await;
        } else if named.is_some() {
            tracker.learn_method(u.as_str(), named).await;
        }
    }

    // Verified alive is earned by THIS probe or lost by it: the listing's own
    // 402 to the request as sent, whose challenge offers a payment and whose
    // recipients passed the drift check -- or, for an MCP endpoint, a
    // handshake that listed at least one tool. Any other answer clears it.
    let verified_by = match mcp_tools {
        Some(tools) => {
            tracker.note_mcp_tools(u.as_str(), tools).await;
            (class == ProbeClass::Alive && tools > 0).then_some(VerifiedBy::McpHandshake)
        }
        None => (class == ProbeClass::Alive
            && answered_as_sent
            && live.as_ref().is_some_and(LiveTerms::offers_payment))
        .then_some(VerifiedBy::X402Challenge),
    };
    tracker.note_verified(u.as_str(), verified_by).await;

    // Politeness feedback. A host that refuses us goes into
    // backoff -- its own `Retry-After` when it sent one, an
    // exponential schedule with jitter when it did not -- so a
    // failing origin is asked less often rather than by every
    // replica at the same instant.
    match http {
        Some(429) | Some(503) => {
            queue
                .note_refusal(u.as_str(), retry_after, now_secs())
                .await
        }
        Some(code) if (500..600).contains(&code) => {
            queue.note_refusal(u.as_str(), None, now_secs()).await
        }
        None => queue.note_refusal(u.as_str(), None, now_secs()).await,
        _ => queue.note_success(u.as_str()).await,
    }

    // Record what the origin actually said, with the context it
    // said it in. Written even when the probe quarantined the
    // resource: the reading happened, and hiding it would lose
    // the evidence of what it was hidden for.
    if let Some(live) = live {
        record_observation(
            registry,
            &registry.terms(),
            &u,
            &resource_type,
            // `None` is the MCP handshake, which is a POST.
            method.unwrap_or(ProbeMethod::Post),
            http,
            live,
        )
        .await;
    }
}

/// The WARN an extra payment option leaves: its network and its recipient,
/// the only record of a recipient nobody vouched for.
fn log_extra_networks(url: &impl std::fmt::Display, extra: &[DeclaredRecipient]) {
    let extra: Vec<String> = extra
        .iter()
        .map(|r| format!("{} {}", r.network.as_deref().unwrap_or("?"), r.pay_to))
        .collect();
    warn!(
        url = %url,
        extra = ?extra,
        "paytoswap: live 402 adds a payment option on a network the listing \
         does not declare; recorded, not quarantined"
    );
}

/// What a live challenge's recipients mean against the ones the listing declares.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Recipients {
    /// Every recipient the challenge pays is one the listing declares.
    Declared,
    /// It also names recipients the listing never declared, each on a network
    /// the listing does not declare at all, and it still offers a payable
    /// option to a declared recipient on that recipient's declared network: a
    /// way to pay that the listing does not mention, not a changed one. Not a
    /// drift. Those extra recipients, for the log.
    ExtraNetworks(Vec<DeclaredRecipient>),
    /// A hijack signal: quarantine.
    Drifted,
}

/// Whether a live challenge pays a recipient the listing never declared, and
/// where. Each transport is judged on its own and the worst verdict stands:
/// a client reads one of them, so a declared recipient in the body does not
/// vouch for a header that pays somebody else.
///
/// * A recipient the listing declares, on any network, is never a drift by
///   itself -- the rule this check had before networks entered it, kept as it
///   was.
/// * A recipient it does not declare, on a network it DOES declare, is a
///   drift: the money on that network now goes to somebody else. So is one on
///   a network we cannot name ([`drift_network`]), which fails closed.
/// * One on a network we can name and the listing does not declare is an
///   extra way to pay (a seller that lists Base and adds Solana), not a changed
///   one -- as long as the same transport still carries an offer the listing
///   declares, scheme, network, asset and recipient alike, spelled so a client
///   takes it. A mention does not count: the declared address in an option
///   nobody can take (unreadable, another scheme or asset, a bare chain id),
///   loose in the document, or on another chain than the one it is declared on
///   leaves the extra option as the only real offer, and that is a drift.
///
/// The extra option is never adopted: the declared recipients come from the
/// catalog's sources, never from the 402 being checked against them. It is
/// logged with its network and recipient, and kept in the observed terms when
/// the catalog can read it as an offer.
///
/// The AMOUNT is not an input here, and must never become one. A seller
/// repricing is ordinary commerce; a seller redirecting the money is a hijack.
/// Quarantine is the response to the second, and applying it to the first would
/// hide a live resource over a change it is entitled to make. A price change is
/// recorded as an observation instead, where a reader can see it and decide.
fn compare_recipients(declared: &[DeclaredOffer], live: &LiveTerms) -> Recipients {
    let mut extra: Vec<DeclaredRecipient> = Vec::new();
    for transport in &live.by_transport {
        match judge_transport(declared, transport) {
            Recipients::Drifted => return Recipients::Drifted,
            Recipients::ExtraNetworks(found) => {
                for r in found {
                    if !extra.contains(&r) {
                        extra.push(r);
                    }
                }
            }
            Recipients::Declared => {}
        }
    }
    if extra.is_empty() {
        Recipients::Declared
    } else {
        Recipients::ExtraNetworks(extra)
    }
}

/// [`compare_recipients`] for one transport.
fn judge_transport(declared: &[DeclaredOffer], live: &TransportRecipients) -> Recipients {
    let declared_networks: Vec<&str> = declared
        .iter()
        .filter_map(|d| d.network.as_deref())
        .collect();
    let mut extra: Vec<DeclaredRecipient> = Vec::new();
    for r in &live.named {
        if declared.iter().any(|d| d.declares_recipient(&r.pay_to)) {
            continue;
        }
        match r.network.as_deref() {
            Some(network) if !declared_networks.contains(&network) => {
                if !extra.contains(r) {
                    extra.push(r.clone());
                }
            }
            _ => return Recipients::Drifted,
        }
    }
    if extra.is_empty() {
        return Recipients::Declared;
    }
    let still_paid = live
        .payable
        .iter()
        .any(|p| p.network.is_some() && declared.iter().any(|d| d.is_offered_by(p)));
    if still_paid {
        Recipients::ExtraNetworks(extra)
    } else {
        Recipients::Drifted
    }
}

/// [`compare_recipients`] as the one bit quarantine needs.
#[cfg(test)]
fn pay_to_drifted(declared: &[DeclaredOffer], live: &LiveTerms) -> bool {
    compare_recipients(declared, live) == Recipients::Drifted
}

/// Store one reading of an origin's live terms in the observed-terms overlay.
///
/// The record's fingerprint is captured alongside it, so a later listing can
/// tell "observed against these exact terms" from "observed, and the listing has
/// been revised since" -- which is the difference between a fresh price and one
/// that is due for revalidation.
///
/// Nothing here can fail a probe. A response that was not a challenge at all
/// records nothing -- we learned nothing, and overwriting a real reading with an
/// empty one would erase evidence. A challenge we DID read but whose options we
/// could not parse is recorded, with the causes counted: that dates the look,
/// and the freshness assessment reports the price as unverified rather than
/// treating an empty reading as agreement.
async fn record_observation(
    registry: &DiscoveryRegistry,
    overlay: &Arc<crate::discovery_terms::TermsOverlay>,
    url: &url::Url,
    resource_type: &str,
    method: ProbeMethod,
    http_status: Option<u16>,
    live: LiveTerms,
) {
    if !live.readable {
        return;
    }
    let content_hash = registry
        .get(url.as_str())
        .await
        .map(|r| r.content_fingerprint());
    let observation = ObservedTerms {
        accepts: live.accepts,
        observed_at: now_secs(),
        // The request that drew this challenge. A price read from a POST with
        // the listing's example body is a reading of THAT request, and says so.
        context: ObservationContext::anonymous(method.as_str(), resource_type),
        // A challenge is the verification phase by construction: no payment has
        // been made, so an `upto` amount here is the ceiling, not a charge.
        phase: ObservationPhase::Verification,
        provenance: TermsProvenance::OriginResponse,
        transport: live.transport.unwrap_or(TermsTransport::Header),
        x402_version: live.x402_version,
        http_status,
        content_hash,
        conflict: live.conflict,
        rejected: live.rejected,
        truncated: false,
    };
    if observation.conflict.is_some() {
        warn!(
            url = %url,
            context = %observation.context.key(),
            transport = ?observation.transport,
            "the header and the body of this 402 declare different terms; keeping both"
        );
    } else {
        debug!(
            url = %url,
            context = %observation.context.key(),
            options = observation.accepts.len(),
            rejected = ?observation.rejected,
            "recorded the payment terms this origin advertises"
        );
    }
    overlay.record(url.as_str(), observation).await;
}

/// Whether `target` is due for a probe now (blocking helper is cheap: one read).
fn tracker_due(tracker: &HealthTracker, target: &ProbeTarget, now: u64) -> bool {
    // Best-effort non-async read via try_read; if contended, treat as due.
    match tracker.records.try_read() {
        Ok(records) => records
            .get(target.url.as_str())
            .map(|r| {
                r.next_probe_at <= now
                    || verification_due(r, now)
                    || unverified_legacy_alive(r, target)
                    || probed_with_another_request(r, target)
            })
            .unwrap_or(true),
        Err(_) => false,
    }
}

/// A verification about to leave the window it is exposed on, under the
/// CURRENT window: a record scheduled before the window was shortened would
/// otherwise wait out the old cadence and drop out of the catalog first.
fn verification_due(rec: &HealthRecord, now: u64) -> bool {
    rec.verified_at
        .is_some_and(|t| now >= t.saturating_add(alive_reprobe_secs()))
}

/// An `alive` record from before `verifiedAt` existed. It is probed now, so
/// what it is exposed on is this build's own verification rather than an
/// overlay reading ([`legacy_verified_at`]); afterwards it carries a method
/// and this stops matching -- one probe per record, once.
///
/// An MCP endpoint never records a method; for it the mark is `mcp_tools`,
/// which every handshake that asks `tools/list` writes: an `alive` MCP record
/// without it was probed by `initialize` alone and cannot be exposed until its
/// handshake runs, so it runs now, once.
fn unverified_legacy_alive(rec: &HealthRecord, target: &ProbeTarget) -> bool {
    if rec.status != HealthStatus::Alive || rec.verified_at.is_some() {
        return false;
    }
    if target.resource_type == "mcp" {
        return rec.mcp_tools.is_none();
    }
    rec.probe_method.is_none()
}

/// Whether a record's verdict came from a request this listing does not call
/// for, so it is re-probed now rather than at its scheduled time.
///
/// Without this, a listing that declares POST and was quarantined by GET 404s
/// would sit hidden until its backoff ran out (up to 72 h), and one labelled
/// auth-gated by a GET 405 would wait out the 7-day healthy cadence. Once
/// re-probed, the record carries the method it was probed with and this stops
/// matching, so it costs one probe per listing, once. The re-probes still go
/// through the per-host cap, so a host with many such listings is worked
/// through over several ticks.
fn probed_with_another_request(rec: &HealthRecord, target: &ProbeTarget) -> bool {
    // The MCP handshake is what an MCP endpoint is probed with, whatever it
    // declares; there is no other request to try.
    if target.resource_type == "mcp" {
        return false;
    }
    // A payTo-drift hold keeps its schedule: it is a security hold, not a
    // liveness verdict, and a different request is no reason to look sooner.
    // Except ONCE for a hold a build without the URN exclusion set (no method
    // recorded): that build quarantined every seller whose challenge carried an
    // agent-pay quote reference, for 72 hours. And once for a hold no build
    // with the per-network rule has judged (no `drift_rule`): before it, a
    // challenge that added a network held its listing for 72 hours at a time.
    // Looking again does not lift anything -- the hold still needs two clean
    // challenges in a row, and a real swap is seen again on this very probe.
    // Same for a hold any later revision of the rule has not judged yet.
    if rec.held_for_drift() {
        return rec.probe_method.is_none() || rec.drift_rule != Some(DRIFT_RULE);
    }
    match &target.request {
        ProbeRequest::Declared { method, .. } => rec.last_method() != *method,
        // Probed only by a build without the fallback, and its GET answered
        // exactly what the fallback exists for.
        ProbeRequest::Undeclared => {
            rec.probe_method.is_none()
                && matches!(rec.http_status, Some(405) | Some(400) | Some(404))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn status_of(t: &HealthTracker, url: &str) -> HealthStatus {
        t.snapshot().await.get(url).unwrap().status
    }

    #[tokio::test]
    async fn quarantine_after_three_fails_and_recovers_after_two() {
        let t = HealthTracker::new();
        let u = "https://x.example/a";
        t.record_probe(u, ProbeClass::Fail, Some(404), 10, None)
            .await;
        t.record_probe(u, ProbeClass::Fail, Some(404), 10, None)
            .await;
        assert_ne!(status_of(&t, u).await, HealthStatus::Quarantined);
        t.record_probe(u, ProbeClass::Fail, Some(404), 10, None)
            .await;
        assert_eq!(status_of(&t, u).await, HealthStatus::Quarantined);
        // recovery needs two consecutive alives
        t.record_probe(u, ProbeClass::Alive, Some(402), 10, None)
            .await;
        assert_eq!(status_of(&t, u).await, HealthStatus::Quarantined);
        t.record_probe(u, ProbeClass::Alive, Some(402), 10, None)
            .await;
        assert_eq!(status_of(&t, u).await, HealthStatus::Alive);
    }

    #[test]
    fn pay_to_extraction_handles_v2_and_v1_bodies() {
        // x402 v2: accepts[]
        let v2 = r#"{"x402Version":2,"accepts":[
            {"network":"eip155:8453","payTo":"0xAAAa0000000000000000000000000000000000aa"},
            {"network":"eip155:1","payTo":"0xBBBb0000000000000000000000000000000000bb"}]}"#;
        let got = pay_to_from_402(Some(v2), None);
        assert_eq!(got.pay_to.len(), 2);
        assert!(got
            .pay_to
            .contains(&"0xaaaa0000000000000000000000000000000000aa".to_string()));
        // v1-style top-level payTo
        let v1 = r#"{"payTo":"0xCCCc0000000000000000000000000000000000cc","amount":"1"}"#;
        assert_eq!(
            pay_to_from_402(Some(v1), None).pay_to,
            vec!["0xcccc0000000000000000000000000000000000cc".to_string()]
        );
        // Garbage yields nothing AND is not marked readable, so the caller can
        // tell "no drift" from "we never got to look".
        let junk = pay_to_from_402(Some("not json"), None);
        assert!(junk.pay_to.is_empty());
        assert!(!junk.readable);
    }

    #[tokio::test]
    async fn the_overlay_drops_records_for_resources_that_left_the_catalog() {
        // 9.8 MB on 2026-09-10, uploaded whole every 60 seconds, and nothing
        // ever removed an entry. A record whose URL is no longer listed is a
        // record nothing can read.
        let t = HealthTracker::new();
        for url in ["https://a.example/x", "https://gone.example/x"] {
            t.record_probe(url, ProbeClass::Alive, Some(402), 5, None)
                .await;
        }
        assert_eq!(t.snapshot().await.len(), 2);

        let live: std::collections::HashSet<String> =
            ["https://a.example/x".to_string()].into_iter().collect();
        assert_eq!(t.retain_urls(&live).await, 1);

        let held = t.snapshot().await;
        assert_eq!(held.len(), 1);
        assert!(held.contains_key("https://a.example/x"));
    }

    #[tokio::test]
    async fn the_overlay_is_not_uploaded_again_within_the_debounce() {
        // It was uploaded every tick, i.e. every 60 seconds: 5,0 MB a minute,
        // 7 GB a day, to record that some counters moved. Nothing reads this
        // object except a task that is starting.
        let t = HealthTracker::new();
        t.record_probe("https://a.example/x", ProbeClass::Alive, Some(402), 5, None)
            .await;
        assert!(t.dirty.load(Ordering::SeqCst), "a probe marks it changed");

        // No overlay is attached, so `persist` cannot upload; what it must do is
        // leave the dirty flag alone rather than consume it, or the first real
        // upload after configuration would find nothing to write.
        t.persist().await;
        assert!(
            t.dirty.load(Ordering::SeqCst),
            "an upload that did not happen must not clear the change flag"
        );

        // And a tracker that just uploaded holds off.
        t.last_persist.store(now_secs(), Ordering::SeqCst);
        assert!(
            now_secs().saturating_sub(t.last_persist.load(Ordering::SeqCst))
                < HealthTracker::persist_interval_secs(),
            "the debounce window is what suppresses the next upload"
        );
    }

    #[tokio::test]
    async fn an_empty_catalog_never_empties_the_overlay() {
        // `main` falls back to an EMPTY in-memory registry when the S3 read
        // fails at startup, and keeps serving. Without this guard one transient
        // GET failure would delete every liveness record and every cumulative
        // count the uptime attestation is built from.
        let t = HealthTracker::new();
        for url in ["https://a.example/x", "https://b.example/x"] {
            t.record_probe(url, ProbeClass::Alive, Some(402), 5, None)
                .await;
        }
        let nothing: std::collections::HashSet<String> = std::collections::HashSet::new();
        assert_eq!(t.retain_urls(&nothing).await, 0);
        assert_eq!(
            t.snapshot().await.len(),
            2,
            "a catalog we could not read is not a catalog with no resources"
        );
    }

    #[tokio::test]
    async fn pruning_nothing_does_not_dirty_the_overlay() {
        // A no-op prune must not schedule a 9.8 MB upload.
        let t = HealthTracker::new();
        t.record_probe("https://a.example/x", ProbeClass::Alive, Some(402), 5, None)
            .await;
        t.dirty.store(false, Ordering::SeqCst);
        let live: std::collections::HashSet<String> =
            ["https://a.example/x".to_string()].into_iter().collect();
        assert_eq!(t.retain_urls(&live).await, 0);
        assert!(!t.dirty.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn paytodrift_quarantines_immediately() {
        let t = HealthTracker::new();
        let u = "https://hijacked.example/pay";
        // A single drift observation quarantines — no failure streak required.
        t.record_probe(u, ProbeClass::PayToDrift, Some(402), 12, None)
            .await;
        assert_eq!(status_of(&t, u).await, HealthStatus::Quarantined);
    }

    #[tokio::test]
    async fn alive_and_authgated_are_immediate() {
        let t = HealthTracker::new();
        t.record_probe("https://a/x", ProbeClass::Alive, Some(402), 5, None)
            .await;
        assert_eq!(status_of(&t, "https://a/x").await, HealthStatus::Alive);
        t.record_probe("https://b/x", ProbeClass::AuthGated, Some(401), 5, None)
            .await;
        assert_eq!(status_of(&t, "https://b/x").await, HealthStatus::AuthGated);
    }
}

#[cfg(test)]
mod payment_required_transport_tests {
    use super::*;

    /// A real `PAYMENT-REQUIRED` header, base64 of the challenge Tenjin serves:
    /// x402 v2 with `accepts[].payTo` on Base. Shortened to the fields that
    /// matter, but the shape and the encoding are what production sends.
    const REAL_HEADER: &str = "eyJ4NDAyVmVyc2lvbiI6IDIsICJlcnJvciI6ICJQYXltZW50IHJlcXVpcmVkIiwgImFjY2VwdHMiOiBbeyJzY2hlbWUiOiAiZXhhY3QiLCAibmV0d29yayI6ICJlaXAxNTU6ODQ1MyIsICJhbW91bnQiOiAiMTAwMDAwIiwgImFzc2V0IjogIjB4ODMzNTg5ZkNENmVEYjZFMDhmNGM3QzMyRDRmNzFiNTRiZEEwMjkxMyIsICJwYXlUbyI6ICIweGIwNTllQUM5MzMwREM1ZjIzRjUzNDZhODEzNDhBZjFFOTlmMzc5YmQiLCAibWF4VGltZW91dFNlY29uZHMiOiAzMDB9XX0=";

    /// What Tenjin actually puts in the 402 BODY: the free preview of the
    /// article. Valid JSON, zero payment terms.
    const REAL_BODY: &str =
        r#"{"id":"01a01a4c","slug":"china-macro-weekly-3","title":"China Macro Weekly"}"#;

    #[test]
    fn the_terms_are_read_from_the_header() {
        // The bug: this returned nothing for 36 of 36 live resources, because
        // it looked only at the body -- where the terms are not.
        let terms = pay_to_from_402(Some(REAL_BODY), Some(REAL_HEADER));
        assert!(
            terms.readable,
            "a challenge in the header must count as read"
        );
        assert_eq!(
            terms.pay_to,
            vec!["0xb059eac9330dc5f23f5346a81348af1e99f379bd".to_string()]
        );
    }

    #[test]
    fn a_body_that_is_not_a_challenge_is_not_readable() {
        // THE failure that hid everything: the body parses fine and carries no
        // payment terms, so the old code returned an empty vec -- and the
        // caller's `if !live.is_empty()` guard read that as "nothing drifted".
        let terms = pay_to_from_402(Some(REAL_BODY), None);
        assert!(terms.pay_to.is_empty());
        assert!(
            !terms.readable,
            "valid JSON without payment terms is NOT a challenge we read"
        );
    }

    #[test]
    fn the_body_transport_still_works() {
        // Both transports are legal. Supporting the header must not drop the
        // sellers who use the body.
        let body = r#"{"accepts":[{"payTo":"0xAAAA"}],"x402Version":2}"#;
        let terms = pay_to_from_402(Some(body), None);
        assert!(terms.readable);
        assert_eq!(terms.pay_to, vec!["0xaaaa".to_string()]);
    }

    #[test]
    fn a_v1_top_level_pay_to_is_read_too() {
        let terms = pay_to_from_402(Some(r#"{"payTo":"0xBBBB"}"#), None);
        assert!(terms.readable);
        assert_eq!(terms.pay_to, vec!["0xbbbb".to_string()]);
    }

    // ========================================================================
    // The whole challenge, not just its recipients
    // ========================================================================

    /// A full v2 challenge in the body: scheme, network, asset, amount, payTo.
    const FULL_BODY: &str = r#"{"x402Version":2,"accepts":[{
        "scheme":"exact","network":"eip155:8453",
        "asset":"0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
        "amount":"30000",
        "payTo":"0xe4dc963c56979E0260fc146b87eE24F18220e545",
        "maxTimeoutSeconds":300}]}"#;

    #[test]
    fn the_whole_requirement_is_read_not_only_the_recipient() {
        // F4: this component is the only one that sees a live 402, and it kept
        // the recipients and threw the price away. A resource could be marked
        // alive and go on advertising an amount from months ago.
        let terms = pay_to_from_402(None, Some(REAL_HEADER));
        assert_eq!(terms.accepts.len(), 1);
        let o = &terms.accepts[0];
        assert_eq!(o.scheme.to_string(), "exact");
        assert_eq!(o.network.to_string(), "eip155:8453");
        assert_eq!(o.amount.to_string(), "100000");
        assert_eq!(o.max_timeout_seconds, 300);
        assert_eq!(terms.x402_version, Some(2));
        assert_eq!(terms.transport, Some(TermsTransport::Header));
    }

    #[test]
    fn the_v1_spelling_of_the_amount_is_read_by_the_same_rule_as_every_import() {
        // `maxAmountRequired` is the v1 name for the same number. The prober
        // goes through the one shared normalization rule, so a challenge and a
        // feed entry describing the same offer produce the same record -- and
        // the prober cannot invent the `exact` that P0 removed.
        let body = r#"{"x402Version":1,"paymentRequirements":[{
            "scheme":"upto","network":"base",
            "asset":"0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            "maxAmountRequired":"100000",
            "payTo":"0xe4dc963c56979E0260fc146b87eE24F18220e545"}]}"#;
        let terms = pay_to_from_402(Some(body), None);
        assert_eq!(terms.accepts.len(), 1);
        assert_eq!(terms.accepts[0].scheme.to_string(), "upto");
        assert_eq!(terms.accepts[0].amount.to_string(), "100000");
        assert_eq!(terms.x402_version, Some(1));
    }

    #[test]
    fn an_unreadable_option_is_counted_by_cause_and_never_becomes_a_price() {
        let body = r#"{"x402Version":2,"accepts":[{
            "scheme":"exact","network":"eip155:8453",
            "asset":"0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
            "amount":"0.002",
            "payTo":"0xe4dc963c56979E0260fc146b87eE24F18220e545"}]}"#;
        let terms = pay_to_from_402(Some(body), None);
        assert!(terms.readable, "we read the challenge");
        assert!(terms.accepts.is_empty(), "and refused the option");
        assert_eq!(terms.rejected.get("amount-not-an-integer"), Some(&1));
    }

    #[test]
    fn two_transports_carrying_the_same_offer_are_not_a_conflict() {
        let terms = pay_to_from_402(Some(FULL_BODY), Some(header_of(FULL_BODY).as_str()));
        assert!(terms.conflict.is_none());
        assert_eq!(terms.accepts.len(), 1);
    }

    #[test]
    fn a_header_and_a_body_that_disagree_are_kept_apart_not_blended() {
        // The header says 0.10, the body says 0.03. There is no single offer
        // here, and manufacturing one -- a network from the header, an amount
        // from the body -- would be indistinguishable afterwards from an offer
        // the seller really made.
        let cheaper = FULL_BODY.replace("30000", "100000");
        let terms = pay_to_from_402(Some(FULL_BODY), Some(header_of(&cheaper).as_str()));
        assert_eq!(
            terms.transport,
            Some(TermsTransport::Header),
            "same protocol version: the header is where sellers put the challenge"
        );
        assert_eq!(terms.accepts[0].amount.to_string(), "100000");
        let conflict = terms.conflict.expect("the other reading is kept");
        assert_eq!(conflict.transport, TermsTransport::Body);
        assert_eq!(conflict.accepts[0].amount.to_string(), "30000");
    }

    #[test]
    fn a_newer_protocol_version_decides_which_transport_is_current() {
        // A seller serving two protocol versions is telling us which is current
        // by numbering it. The tie-break is the version, not the transport.
        let v1_header = header_of(
            &FULL_BODY
                .replace(r#""x402Version":2"#, r#""x402Version":1"#)
                .replace("30000", "100000"),
        );
        let terms = pay_to_from_402(Some(FULL_BODY), Some(v1_header.as_str()));
        assert_eq!(terms.transport, Some(TermsTransport::Body));
        assert_eq!(terms.x402_version, Some(2));
        assert_eq!(terms.accepts[0].amount.to_string(), "30000");
        let conflict = terms.conflict.expect("the v1 reading is kept as evidence");
        assert_eq!(conflict.x402_version, Some(1));
    }

    #[test]
    fn a_hijacked_recipient_is_seen_in_either_transport_even_when_one_wins() {
        // The terms come from one transport; the drift check sees BOTH. A
        // recipient declared anywhere in the response is a recipient declared.
        let other_payee = FULL_BODY.replace(
            "0xe4dc963c56979E0260fc146b87eE24F18220e545",
            "0x000000000000000000000000000000000000dEaD",
        );
        let terms = pay_to_from_402(Some(&other_payee), Some(header_of(FULL_BODY).as_str()));
        assert_eq!(terms.pay_to.len(), 2);
        assert!(terms
            .pay_to
            .contains(&"0x000000000000000000000000000000000000dead".to_string()));
    }

    #[test]
    fn a_repriced_offer_is_not_a_hijack() {
        // Same recipient, a very different number. This must NOT quarantine:
        // the whole distinction between an identity failure and a commercial
        // one lives in this predicate.
        let expected = vec![base_offer("0xe4dc963c56979e0260fc146b87ee24f18220e545")];
        let repriced = FULL_BODY.replace(r#""amount":"30000""#, r#""amount":"500000""#);
        let terms = pay_to_from_402(Some(&repriced), None);
        assert!(terms.readable);
        assert!(
            !pay_to_drifted(&expected, &terms),
            "a price change is not a payTo swap"
        );
        assert_eq!(
            terms.accepts[0].amount.to_string(),
            "500000",
            "and the new price is what gets recorded"
        );
    }

    #[test]
    fn a_redirected_payment_still_is_a_hijack() {
        let expected = vec![base_offer("0xe4dc963c56979e0260fc146b87ee24f18220e545")];
        let hijacked = FULL_BODY.replace(
            "0xe4dc963c56979E0260fc146b87eE24F18220e545",
            "0x000000000000000000000000000000000000dEaD",
        );
        let terms = pay_to_from_402(Some(&hijacked), None);
        assert!(pay_to_drifted(&expected, &terms));
    }

    fn header_of(json: &str) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(json)
    }

    #[test]
    fn a_hijack_in_the_header_is_now_visible() {
        // The whole point: a live 402 paying an undeclared recipient. Before
        // this, a swap hidden in the header was invisible.
        let declared = ["0x1111111111111111111111111111111111111111".to_string()];
        let terms = pay_to_from_402(Some(REAL_BODY), Some(REAL_HEADER));
        let drifted: Vec<_> = terms
            .pay_to
            .iter()
            .filter(|p| !declared.contains(p))
            .collect();
        assert!(!drifted.is_empty(), "the swap must be detectable");
    }

    // ========================================================================
    // Recipients per network
    // ========================================================================

    /// losbeto's live 402, 2026-10-04 (observed-terms overlay): the challenge in
    /// the header as x402 v2 and in the body as v1, both paying the Base
    /// recipient its catalog copy declares and adding a Solana option.
    const LOSBETO_HEADER: &str = r#"{"x402Version":2,"accepts":[
        {"scheme":"exact","network":"eip155:8453",
         "asset":"0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913","amount":"350000",
         "payTo":"0xd5Ba9711a3D052846a3695C70e7fcb8b3168FE7d","maxTimeoutSeconds":300},
        {"scheme":"exact","network":"solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
         "asset":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v","amount":"350000",
         "payTo":"GEhr9HCFTRDjanMg435frSgCVwVZYpNoPrEkmNBnFHFE","maxTimeoutSeconds":300}]}"#;
    const LOSBETO_BODY: &str = r#"{"x402Version":1,"accepts":[
        {"scheme":"exact","network":"base",
         "asset":"0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913","maxAmountRequired":"350000",
         "payTo":"0xd5Ba9711a3D052846a3695C70e7fcb8b3168FE7d","maxTimeoutSeconds":300},
        {"scheme":"exact","network":"solana",
         "asset":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v","maxAmountRequired":"350000",
         "payTo":"GEhr9HCFTRDjanMg435frSgCVwVZYpNoPrEkmNBnFHFE","maxTimeoutSeconds":300}]}"#;

    const USDC_BASE: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";

    /// An `exact` USDC offer to `pay_to` on `network`, as the catalog declares
    /// it ([`challenge`] puts this same asset in every option).
    fn usdc_offer(network: &str, pay_to: &str) -> DeclaredOffer {
        DeclaredOffer::new("exact", network, USDC_BASE, pay_to)
    }

    fn base_offer(pay_to: &str) -> DeclaredOffer {
        usdc_offer("eip155:8453", pay_to)
    }

    /// What the catalog declared for it: PayAI's copy, Base only.
    fn losbeto_declared() -> Vec<DeclaredOffer> {
        vec![base_offer("0xd5Ba9711a3D052846a3695C70e7fcb8b3168FE7d")]
    }

    /// A challenge with one option per `(network, payTo)`.
    fn challenge(options: &[(&str, &str)]) -> LiveTerms {
        let accepts: Vec<serde_json::Value> = options
            .iter()
            .map(|(network, pay_to)| {
                serde_json::json!({
                    "scheme": "exact", "network": network, "payTo": pay_to,
                    "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                    "amount": "1000", "maxTimeoutSeconds": 60
                })
            })
            .collect();
        let doc = serde_json::json!({ "x402Version": 2, "accepts": accepts });
        pay_to_from_402(None, Some(header_of(&doc.to_string()).as_str()))
    }

    const BASE_A: &str = "0xd5Ba9711a3D052846a3695C70e7fcb8b3168FE7d";
    const OTHER: &str = "0x000000000000000000000000000000000000dEaD";
    const SOLANA_S: &str = "GEhr9HCFTRDjanMg435frSgCVwVZYpNoPrEkmNBnFHFE";

    #[test]
    fn losbeto_adds_a_network_and_is_not_quarantined() {
        // The case that held 10+ of its listings: the declared Base recipient is
        // still paid, and the challenge adds Solana. A network more, not a
        // recipient changed.
        let live = pay_to_from_402(Some(LOSBETO_BODY), Some(header_of(LOSBETO_HEADER).as_str()));
        assert!(live.readable);
        assert!(live.offers_payment());
        assert_eq!(
            compare_recipients(&losbeto_declared(), &live),
            Recipients::ExtraNetworks(vec![DeclaredRecipient::new("solana", SOLANA_S)])
        );
        assert!(!pay_to_drifted(&losbeto_declared(), &live));
        // Each transport alone says the same.
        for one in [
            pay_to_from_402(Some(LOSBETO_BODY), None),
            pay_to_from_402(None, Some(header_of(LOSBETO_HEADER).as_str())),
        ] {
            assert!(!pay_to_drifted(&losbeto_declared(), &one));
        }
    }

    #[test]
    fn tavily_pays_another_recipient_on_its_declared_network_and_stays_quarantined() {
        // x402.tavily.com/search, 2026-10-04: the catalog (Coinbase's copy,
        // lastUpdated 15:08:50Z) declares one Base recipient; the live 402 pays
        // another on Base, next to the `agent-pay` quote reference. That is the
        // swap the quarantine exists for, and no extra network changes it.
        let declared = vec![base_offer("0xC5c967576a19Ed250030f8C15B6158968DD64643")];
        let live = challenge(&[
            ("eip155:8453", "0x24B8FD1A73685B25a4c3bA03FAC4A2d733015C6F"),
            ("aws:base", "urn:x402:agent-pay:see-quote"),
        ]);
        assert_eq!(compare_recipients(&declared, &live), Recipients::Drifted);
        // Even with an extra network and the old recipient still paid on another
        // chain: a new recipient on Base is a new recipient on Base.
        let live = challenge(&[
            ("eip155:8453", "0x24B8FD1A73685B25a4c3bA03FAC4A2d733015C6F"),
            ("eip155:137", "0xC5c967576a19Ed250030f8C15B6158968DD64643"),
            ("solana", SOLANA_S),
        ]);
        assert_eq!(compare_recipients(&declared, &live), Recipients::Drifted);
    }

    #[test]
    fn an_extra_network_counts_only_while_a_declared_recipient_is_still_paid() {
        // Declared recipient gone, only the new network left: the money moved.
        assert_eq!(
            compare_recipients(&losbeto_declared(), &challenge(&[("solana", SOLANA_S)])),
            Recipients::Drifted
        );
        // Gone from the network it is declared on counts as gone, even when
        // the address shows up on another chain or on a testnet.
        for elsewhere in ["eip155:137", "eip155:84532"] {
            assert_eq!(
                compare_recipients(
                    &losbeto_declared(),
                    &challenge(&[(elsewhere, BASE_A), ("solana", SOLANA_S)])
                ),
                Recipients::Drifted,
                "{elsewhere}"
            );
        }
        // On another chain alone it is a declared recipient, as before.
        assert_eq!(
            compare_recipients(&losbeto_declared(), &challenge(&[("eip155:137", BASE_A)])),
            Recipients::Declared
        );
    }

    #[test]
    fn a_mention_of_the_declared_recipient_is_not_an_offer_to_it() {
        let attacker = serde_json::json!({
            "scheme": "exact", "network": "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
            "asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", "amount": "350000",
            "payTo": "AttackerSo1anaAddress1111111111111111111111", "maxTimeoutSeconds": 300
        });
        let declared_base = serde_json::json!({
            "scheme": "exact", "network": "eip155:8453",
            "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913", "amount": "350000",
            "payTo": BASE_A, "maxTimeoutSeconds": 300
        });
        let v2 = |accepts: serde_json::Value| {
            header_of(&serde_json::json!({"x402Version": 2, "accepts": accepts}).to_string())
        };
        // An option nobody can pay that names the declared address.
        let live = pay_to_from_402(
            None,
            Some(v2(serde_json::json!([{"payTo": BASE_A}, attacker])).as_str()),
        );
        assert!(
            pay_to_drifted(&losbeto_declared(), &live),
            "unpayable option"
        );
        // The declared address loose at the top of the document.
        let doc = serde_json::json!({"x402Version": 2, "payTo": BASE_A, "accepts": [attacker]});
        let live = pay_to_from_402(None, Some(header_of(&doc.to_string()).as_str()));
        assert!(
            pay_to_drifted(&losbeto_declared(), &live),
            "top-level payTo"
        );
        // The declared offer in the body, only the attacker in the header that
        // a v2 client reads: each transport is judged on its own.
        let body = serde_json::json!({"x402Version": 1, "accepts": [declared_base]});
        let live = pay_to_from_402(
            Some(&body.to_string()),
            Some(v2(serde_json::json!([attacker])).as_str()),
        );
        assert!(
            pay_to_drifted(&losbeto_declared(), &live),
            "split transports"
        );
        // A decoy: the declared recipient on the declared network, in an option
        // no client takes -- a scheme nobody implements, an asset nobody
        // declared, or the chain written as a bare id.
        for (field, value) in [
            ("scheme", serde_json::json!("x")),
            (
                "asset",
                serde_json::json!("0x0000000000000000000000000000000000000001"),
            ),
            ("network", serde_json::json!("8453")),
        ] {
            let mut decoy = declared_base.clone();
            decoy[field] = value;
            let live = pay_to_from_402(
                None,
                Some(v2(serde_json::json!([decoy, attacker])).as_str()),
            );
            assert!(pay_to_drifted(&losbeto_declared(), &live), "decoy {field}");
        }
        // The real thing still passes: the declared offer beside the extra one,
        // with the network as v1 names it too.
        let live = pay_to_from_402(
            None,
            Some(v2(serde_json::json!([declared_base, attacker])).as_str()),
        );
        assert!(!pay_to_drifted(&losbeto_declared(), &live));
        let mut v1_named = declared_base.clone();
        v1_named["network"] = serde_json::json!("base");
        let live = pay_to_from_402(
            None,
            Some(v2(serde_json::json!([v1_named, attacker])).as_str()),
        );
        assert!(!pay_to_drifted(&losbeto_declared(), &live));
    }

    #[test]
    fn a_new_recipient_on_a_declared_network_is_a_drift_in_any_spelling() {
        // Every spelling of Base is Base: none of them is "another network".
        for spelling in [
            "eip155:8453",
            "base",
            "Base",
            "base-mainnet",
            "8453",
            "eip155:08453",
            "EIP155:8453",
            " eip155:8453 ",
        ] {
            let live = challenge(&[("eip155:8453", BASE_A), (spelling, OTHER)]);
            assert_eq!(
                compare_recipients(&losbeto_declared(), &live),
                Recipients::Drifted,
                "{spelling:?}"
            );
        }
    }

    #[test]
    fn a_network_we_cannot_name_fails_closed() {
        // `aws:base` is the network of Coinbase's agent-pay option and reads like
        // an alias of Base; the rest are no chain at all. A recipient the
        // listing does not declare on any of them is a drift.
        for network in [
            "aws:base",
            "solana-mainnet",
            "evm:8453",
            "eip155:base",
            "eip155:",
            ":8453",
            "",
            "not a network",
        ] {
            let live = challenge(&[("eip155:8453", BASE_A), (network, OTHER)]);
            assert_eq!(
                compare_recipients(&losbeto_declared(), &live),
                Recipients::Drifted,
                "{network:?}"
            );
        }
        // So is an option whose network is missing or not a string.
        for option in [
            serde_json::json!({"payTo": OTHER}),
            serde_json::json!({"payTo": OTHER, "network": 8453}),
            serde_json::json!({"payTo": OTHER, "network": null}),
        ] {
            let doc = serde_json::json!({"x402Version": 2, "accepts": [
                {"scheme": "exact", "network": "eip155:8453", "payTo": BASE_A,
                 "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913", "amount": "1"},
                option.clone()
            ]});
            let live = pay_to_from_402(Some(&doc.to_string()), None);
            assert!(pay_to_drifted(&losbeto_declared(), &live), "{option}");
        }
        // And a v1 top-level payTo: it names no network unless the document does.
        let live = pay_to_from_402(Some(&format!(r#"{{"payTo":"{OTHER}"}}"#)), None);
        assert!(pay_to_drifted(&losbeto_declared(), &live));
    }

    #[test]
    fn a_stacks_option_beside_the_declared_evm_recipient_is_an_extra_network() {
        // The shape 183 listings of one host served on 2026-10-04: the declared
        // address on Base, Arbitrum and Polygon, and an option on `stacks:1`
        // paying a Stacks account. Held for 72 hours at a time before this rule.
        let declared: Vec<DeclaredOffer> = ["eip155:8453", "eip155:42161", "eip155:137"]
            .iter()
            .map(|n| usdc_offer(n, BASE_A))
            .collect();
        let stacks = "SP000000000000000000002Q6VF78";
        let live = challenge(&[
            ("eip155:8453", BASE_A),
            ("eip155:42161", BASE_A),
            ("eip155:137", BASE_A),
            ("stacks:1", stacks),
        ]);
        assert_eq!(
            compare_recipients(&declared, &live),
            Recipients::ExtraNetworks(vec![DeclaredRecipient::new("stacks:1", stacks)])
        );
        // The same option with the EVM recipient swapped is still a swap.
        let live = challenge(&[("eip155:8453", OTHER), ("stacks:1", stacks)]);
        assert_eq!(compare_recipients(&declared, &live), Recipients::Drifted);
    }

    #[test]
    fn an_alias_of_a_declared_family_is_that_family() {
        // A listing that declares Solana: an option on `solana:mainnet`, or on
        // the devnet, is not "another network" -- every Solana spelling is one
        // family for this check, so a new recipient there is a drift.
        let declared = vec![usdc_offer(
            "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
            SOLANA_S,
        )];
        for alias in [
            "solana",
            "solana:mainnet",
            "SOLANA:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
            "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1",
        ] {
            let live = challenge(&[
                ("solana", SOLANA_S),
                (alias, "AttackerSo1anaAddress1111111111111111111111"),
            ]);
            assert_eq!(
                compare_recipients(&declared, &live),
                Recipients::Drifted,
                "{alias:?}"
            );
        }
    }

    #[test]
    fn the_drift_network_keys() {
        assert_eq!(drift_network("base").as_deref(), Some("eip155:8453"));
        assert_eq!(
            drift_network("eip155:08453").as_deref(),
            Some("eip155:8453")
        );
        assert_eq!(drift_network("EIP155:8453").as_deref(), Some("eip155:8453"));
        assert_eq!(drift_network("polygon").as_deref(), Some("eip155:137"));
        assert_eq!(
            drift_network("solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp").as_deref(),
            Some("solana")
        );
        // Algorand as the x402 spec spells it (CDP lists losbeto with it): a
        // family we can name, though the catalog cannot hold the option.
        assert_eq!(
            drift_network("algorand:wGHE2Pwdvd7S12BL5FaOP20EGYesN73ktiC1qzkkit8=").as_deref(),
            Some("algorand")
        );
        assert_eq!(drift_network("stacks:1").as_deref(), Some("stacks"));
        for unnamed in ["aws:base", "evm:8453", "eip155:base", "", "solana-mainnet"] {
            assert_eq!(drift_network(unnamed), None, "{unnamed:?}");
        }
    }

    #[test]
    fn a_garbage_header_does_not_masquerade_as_a_reading() {
        for junk in ["not base64!!", "", "e30=", "bnVsbA=="] {
            let terms = pay_to_from_402(None, Some(junk));
            assert!(
                !terms.readable,
                "{junk:?} must not count as a challenge we read"
            );
        }
    }
}

#[cfg(test)]
mod declared_method_tests {
    //! The prober against a local seller: what it sends, and what it concludes.
    //!
    //! The SSRF connector refuses loopback by design, so these drive the same
    //! probe logic through [`Loopback`], which builds every request with the
    //! production [`wire_body`] and `json_request`. No test reaches a real host.
    use super::*;
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::{HeaderMap, Method, StatusCode, Uri};
    use axum::response::{IntoResponse, Response};
    use serde_json::{json, Value};
    use std::sync::Mutex;

    const CDP_PAGE: &str = include_str!("../tests/fixtures/bazaar/cdp-pricing-page.json");
    const REPORT_CASES: &str =
        include_str!("../tests/fixtures/bazaar/declared-method-listings.json");

    /// What the seller saw of one request.
    #[derive(Debug, Clone)]
    struct Seen {
        method: String,
        path: String,
        content_type: Option<String>,
        payment_header: bool,
        body: String,
    }

    /// How the local seller answers one path.
    #[derive(Clone)]
    struct Route {
        /// Status for a GET.
        get: u16,
        /// Status for any body method.
        other: u16,
        /// When set, a body method gets its 402 only for exactly this JSON;
        /// any other body gets `refuse`, as a validating seller would answer.
        expect_body: Option<Value>,
        refuse: u16,
        /// The PAYMENT-REQUIRED header a 402 carries; `None` is a 402 with no
        /// challenge anywhere.
        challenge: Option<String>,
        /// Bytes of body beside a challenged 402 (a free preview); 0 is `{}`.
        pad: usize,
    }

    #[derive(Clone, Default)]
    struct Seller {
        routes: Arc<HashMap<String, Route>>,
        seen: Arc<Mutex<Vec<Seen>>>,
    }

    impl Seller {
        fn seen(&self, path: &str) -> Vec<Seen> {
            self.seen
                .lock()
                .unwrap()
                .iter()
                .filter(|s| s.path == path)
                .cloned()
                .collect()
        }

        fn methods(&self, path: &str) -> Vec<String> {
            self.seen(path).into_iter().map(|s| s.method).collect()
        }
    }

    async fn answer(
        State(seller): State<Seller>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let body = String::from_utf8_lossy(&body).into_owned();
        seller.seen.lock().unwrap().push(Seen {
            method: method.to_string(),
            path: uri.path().to_string(),
            content_type: headers
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string),
            payment_header: headers
                .keys()
                .any(|k| k.as_str() == "x-payment" || k.as_str() == "payment-signature"),
            body: body.clone(),
        });
        let Some(route) = seller.routes.get(uri.path()) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let mut code = if method == Method::GET {
            route.get
        } else {
            route.other
        };
        if code == 402 && method != Method::GET {
            if let Some(expected) = &route.expect_body {
                if serde_json::from_str::<Value>(&body).ok().as_ref() != Some(expected) {
                    code = route.refuse;
                }
            }
        }
        let status = StatusCode::from_u16(code).unwrap();
        let preview = if route.pad > 0 {
            "x".repeat(route.pad)
        } else {
            "{}".to_string()
        };
        match (code, &route.challenge) {
            (402, Some(c)) => (status, [("payment-required", c.clone())], preview).into_response(),
            (402, None) => (status, "payment required").into_response(),
            _ => status.into_response(),
        }
    }

    async fn serve(routes: HashMap<String, Route>) -> (String, Seller) {
        let seller = Seller {
            routes: Arc::new(routes),
            seen: Arc::default(),
        };
        let app = axum::Router::new()
            .fallback(answer)
            .with_state(seller.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), seller)
    }

    /// The prober's transport, pointed at loopback.
    struct Loopback {
        client: reqwest::Client,
        /// When set, every request goes to this local seller, whatever host
        /// the listing names -- so a listing keeps its real URL end to end.
        serving: Option<url::Url>,
    }

    impl Loopback {
        fn new() -> Self {
            Self {
                client: reqwest::Client::builder()
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .unwrap(),
                serving: None,
            }
        }

        fn serving(base: &str) -> Self {
            Self {
                serving: Some(url::Url::parse(base).unwrap()),
                ..Self::new()
            }
        }
    }

    #[async_trait::async_trait]
    impl ProbeTransport for Loopback {
        async fn send(
            &self,
            url: &url::Url,
            method: ProbeMethod,
            body: Option<&str>,
        ) -> Result<reqwest::Response, SecurityReject> {
            let mut to = url.clone();
            if let Some(base) = &self.serving {
                let _ = to.set_scheme("http");
                let _ = to.set_host(base.host_str());
                let _ = to.set_port(base.port());
            }
            crate::discovery_security::json_request(
                &self.client,
                &to,
                wire_body(method, body).as_ref(),
            )
            .send()
            .await
            .map_err(|e| SecurityReject::Http(e.to_string()))
        }

        async fn send_mcp(
            &self,
            url: &url::Url,
            body: &str,
            session: Option<&str>,
        ) -> Result<reqwest::Response, SecurityReject> {
            let mut to = url.clone();
            if let Some(base) = &self.serving {
                let _ = to.set_scheme("http");
                let _ = to.set_host(base.host_str());
                let _ = to.set_port(base.port());
            }
            crate::discovery_security::json_request_with(
                &self.client,
                &to,
                Some(&(reqwest::Method::POST, body.to_string())),
                &crate::discovery_security::mcp_headers(session),
            )
            .send()
            .await
            .map_err(|e| SecurityReject::Http(e.to_string()))
        }
    }

    fn header_of(challenge: &Value) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(challenge.to_string())
    }

    fn challenge_for(accepts: &Value) -> String {
        header_of(&json!({ "x402Version": 2, "accepts": accepts }))
    }

    fn route(get: u16, other: u16, accepts: &Value) -> Route {
        Route {
            get,
            other,
            expect_body: None,
            refuse: 400,
            challenge: Some(challenge_for(accepts)),
            pad: 0,
        }
    }

    fn at(base: &str, path: &str) -> url::Url {
        url::Url::parse(&format!("{base}{path}")).unwrap()
    }

    fn captured(resource: &str) -> Value {
        let page: Value = serde_json::from_str(CDP_PAGE).unwrap();
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["resource"] == resource)
            .unwrap_or_else(|| panic!("{resource} is not in cdp-pricing-page.json"))
            .clone()
    }

    /// Tavily's listing as the Coinbase feed carries it: POST with an example
    /// body, and two payment options -- `exact` to an address, and `agent-pay`
    /// whose payTo is a quote reference.
    fn tavily() -> Value {
        captured("https://x402.tavily.com/search")
    }

    fn accepts() -> Value {
        tavily()["accepts"].clone()
    }

    fn post(example: Option<&str>) -> ProbeRequest {
        ProbeRequest::Declared {
            method: ProbeMethod::Post,
            example: example.map(str::to_string),
        }
    }

    fn get() -> ProbeRequest {
        ProbeRequest::Declared {
            method: ProbeMethod::Get,
            example: None,
        }
    }

    fn target(url: &str, request: ProbeRequest) -> ProbeTarget {
        ProbeTarget {
            url: url::Url::parse(url).unwrap(),
            resource_type: "http".to_string(),
            pay_to: vec![],
            request,
        }
    }

    async fn status_of(t: &HealthTracker, url: &str) -> HealthStatus {
        t.snapshot().await.get(url).unwrap().status
    }

    async fn reason_of(t: &HealthTracker, url: &str) -> Option<QuarantineReason> {
        t.snapshot().await.get(url).unwrap().quarantine_reason
    }

    // ------------------------------------------------------------------------
    // Where the method comes from
    // ------------------------------------------------------------------------

    #[test]
    fn declared_request_reads_every_declaration_in_order() {
        // The feed's own shapes: GET with query params, POST with a body, and
        // an entry that declares nothing.
        let onesource = captured("https://api.onesource.io/api/chain/erc20-balance");
        assert_eq!(
            declared_request(onesource.get("extensions")),
            get(),
            "query params are declared, and never sent: nothing a listing says goes into the URL"
        );
        let enrich = captured("https://stableenrich.dev/api/fullenrich/people-search");
        let ProbeRequest::Declared { method, example } = declared_request(enrich.get("extensions"))
        else {
            panic!("StableEnrich declares POST");
        };
        assert_eq!(method, ProbeMethod::Post);
        assert_eq!(
            serde_json::from_str::<Value>(&example.unwrap()).unwrap(),
            enrich["extensions"]["bazaar"]["info"]["input"]["body"]
        );
        let nodate = captured("https://api.example-nodate.test/quote");
        assert_eq!(
            declared_request(nodate.get("extensions")),
            ProbeRequest::Undeclared
        );
        assert_eq!(declared_request(None), ProbeRequest::Undeclared);
        assert_eq!(
            declared_request(Some(&json!({ "other": {} }))),
            ProbeRequest::Undeclared
        );

        // 1. info.input.method, case and spaces aside.
        let info = |m: Value| json!({ "bazaar": { "info": { "input": { "method": m } } } });
        for (raw, want) in [
            (json!(" post "), Some(ProbeMethod::Post)),
            (json!("put"), Some(ProbeMethod::Put)),
            (json!("PATCH"), Some(ProbeMethod::Patch)),
            (json!("get"), Some(ProbeMethod::Get)),
            (json!("HEAD"), Some(ProbeMethod::Get)),
            (json!("DELETE"), Some(ProbeMethod::Get)),
            (json!("FETCH"), None),
            (json!(""), None),
            (json!(42), None),
            (Value::Null, None),
        ] {
            let got = declared_request(Some(&info(raw.clone())));
            match want {
                None => assert_eq!(got, ProbeRequest::Undeclared, "{raw}"),
                Some(ProbeMethod::Get) => assert_eq!(got, get(), "{raw}"),
                Some(m) => assert_eq!(
                    got,
                    ProbeRequest::Declared {
                        method: m,
                        example: None
                    },
                    "{raw}: a body method with no example"
                ),
            }
        }

        // 2. The JSON Schema half: the SDK's HTTP shape, a const, an enum.
        let schema =
            |input: Value| json!({ "bazaar": { "schema": { "properties": { "input": input } } } });
        assert_eq!(
            declared_request(Some(&schema(json!({ "type": "http", "method": "GET" })))),
            get()
        );
        assert_eq!(
            declared_request(Some(&schema(
                json!({ "properties": { "method": { "const": "PUT" } } })
            ))),
            ProbeRequest::Declared {
                method: ProbeMethod::Put,
                example: None
            }
        );
        assert_eq!(
            declared_request(Some(&schema(
                json!({ "properties": { "method": { "enum": ["POST", "GET"] } } })
            ))),
            post(None)
        );

        // 3. A body declared without a method is a POST: the SDK's body shape
        // (MeshRelay's), a bare bodyType, a bare example body.
        assert_eq!(
            declared_request(Some(&schema(
                json!({ "properties": { "body": { "type": "object" } } })
            ))),
            post(None)
        );
        assert_eq!(
            declared_request(Some(
                &json!({ "bazaar": { "info": { "input": { "bodyType": "json" } } } })
            )),
            post(None)
        );
        assert_eq!(
            declared_request(Some(
                &json!({ "bazaar": { "info": { "input": { "body": { "q": 1 } } } } })
            )),
            post(Some(r#"{"q":1}"#))
        );

        // The order: info.input.method wins over the schema.
        assert_eq!(
            declared_request(Some(&json!({ "bazaar": {
                "info": { "input": { "method": "GET" } },
                "schema": { "properties": { "input": { "properties": { "body": {} } } } }
            } }))),
            get()
        );
    }

    #[test]
    fn our_own_origin_and_opted_out_prefixes_only_ever_get_a_get() {
        let ext = json!({ "bazaar": { "info": { "input": {
            "method": "POST", "body": { "agentUri": "https://x.example", "recipient": "0x1" }
        } } } });
        for own in [
            "https://facilitator.ultravioletadao.xyz/register",
            "https://FACILITATOR.ultravioletadao.xyz./feedback",
            "https://facilitator.ultravioletadao.xyz:8443/settle",
        ] {
            let url = url::Url::parse(own).unwrap();
            assert_eq!(probe_request(&url, Some(&ext), false), get(), "{own}");
        }
        let other = url::Url::parse("https://facilitator.ultravioletadao.xyz.evil.com/x").unwrap();
        assert!(matches!(
            probe_request(&other, Some(&ext), false),
            ProbeRequest::Declared {
                method: ProbeMethod::Post,
                ..
            }
        ));
        // The owner's opt-out, read from the curation manifest.
        assert_eq!(probe_request(&other, Some(&ext), true), get());
        assert_eq!(
            probe_request(&other, None, true),
            get(),
            "and no fallback either"
        );
    }

    // ------------------------------------------------------------------------
    // What goes on the wire
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn a_post_only_listing_is_probed_with_post_and_found_alive() {
        let listing = tavily();
        let request = declared_request(listing.get("extensions"));
        let (base, seller) = serve(HashMap::from([(
            "/search".to_string(),
            route(405, 402, &listing["accepts"]),
        )]))
        .await;
        let url = at(&base, "/search");

        // What the prober did until now: a GET, which a POST-only endpoint
        // refuses -- and that refusal was filed as auth-gated.
        let get = probe_once(&Loopback::new(), &url, ProbeMethod::Get, None).await;
        assert_eq!(get.http, Some(405));
        assert_eq!(get.class, ProbeClass::AuthGated);

        let probed = probe_listing(&Loopback::new(), &url, &request, None).await;
        assert_eq!(probed.outcome.class, ProbeClass::Alive);
        assert_eq!(probed.outcome.http, Some(402));
        assert_eq!(probed.method, ProbeMethod::Post);
        assert!(!probed.fell_back);

        let posts: Vec<Seen> = seller
            .seen("/search")
            .into_iter()
            .filter(|s| s.method == "POST")
            .collect();
        assert_eq!(posts.len(), 1, "one probe: `{{}}` was enough");
        assert_eq!(posts[0].body, EMPTY_JSON_BODY, "the example stays home");
        assert_eq!(posts[0].content_type.as_deref(), Some("application/json"));
        assert!(!posts[0].payment_header, "unpaid: no payment header");
    }

    #[tokio::test]
    async fn the_example_body_travels_only_when_empty_is_refused() {
        let listing = tavily();
        let example = listing["extensions"]["bazaar"]["info"]["input"]["body"].clone();
        let request = declared_request(listing.get("extensions"));
        let mut validating = route(405, 402, &accepts());
        validating.expect_body = Some(example.clone());
        let mut strict = validating.clone();
        strict.refuse = 422;
        let (base, seller) = serve(HashMap::from([
            ("/validating".to_string(), validating),
            ("/strict".to_string(), strict),
        ]))
        .await;

        for path in ["/validating", "/strict"] {
            let probed = probe_listing(&Loopback::new(), &at(&base, path), &request, None).await;
            assert_eq!(probed.outcome.class, ProbeClass::Alive, "{path}");
            assert!(probed.fell_back, "{path}");
            let seen = seller.seen(path);
            assert_eq!(seen.len(), 2, "{path}: one extra request, no more");
            assert_eq!(seen[0].body, EMPTY_JSON_BODY, "{path}");
            assert_eq!(
                serde_json::from_str::<Value>(&seen[1].body).unwrap(),
                example,
                "{path}: the listing's own example, second"
            );
        }

        // No example declared: the 400 stands, and nothing else is sent.
        let probed = probe_listing(
            &Loopback::new(),
            &at(&base, "/validating"),
            &post(None),
            None,
        )
        .await;
        assert_eq!(probed.outcome.http, Some(400));
        assert_eq!(probed.outcome.class, ProbeClass::Degraded);
        assert_eq!(seller.seen("/validating").len(), 3);
    }

    #[tokio::test]
    async fn a_declared_put_or_patch_is_sent_as_declared() {
        let (base, seller) = serve(HashMap::from([(
            "/item".to_string(),
            route(405, 402, &accepts()),
        )]))
        .await;
        let url = at(&base, "/item");
        for method in [ProbeMethod::Put, ProbeMethod::Patch] {
            let request = ProbeRequest::Declared {
                method,
                example: Some(r#"{"id":1}"#.to_string()),
            };
            let probed = probe_listing(&Loopback::new(), &url, &request, None).await;
            assert_eq!(probed.outcome.class, ProbeClass::Alive);
            let last = seller.seen("/item").pop().unwrap();
            assert_eq!(last.method, method.as_str());
            assert_eq!(last.body, EMPTY_JSON_BODY);
        }
    }

    #[tokio::test]
    async fn a_405_to_the_declared_method_is_degraded_not_auth_gated() {
        let (base, seller) = serve(HashMap::from([(
            "/moved".to_string(),
            route(405, 405, &accepts()),
        )]))
        .await;
        let probed = probe_listing(
            &Loopback::new(),
            &at(&base, "/moved"),
            &post(Some(r#"{"q":1}"#)),
            None,
        )
        .await;
        assert_eq!(probed.outcome.http, Some(405));
        assert_eq!(probed.outcome.class, ProbeClass::Degraded);
        assert_eq!(
            seller.methods("/moved"),
            ["POST"],
            "a refusal of the method is not a refusal of the body: the example stays home"
        );
    }

    #[tokio::test]
    async fn a_declared_post_that_answers_404_fails_as_always() {
        let (base, _seller) = serve(HashMap::from([(
            "/gone".to_string(),
            route(404, 404, &accepts()),
        )]))
        .await;
        let url = at(&base, "/gone");
        let probed = probe_listing(&Loopback::new(), &url, &post(None), None).await;
        assert_eq!(probed.outcome.class, ProbeClass::Fail);
        assert_eq!(probed.outcome.http, Some(404));

        let t = HealthTracker::new();
        for n in 1..=3 {
            t.record_probe(
                url.as_str(),
                ProbeClass::Fail,
                Some(404),
                1,
                Some(probed.method),
            )
            .await;
            let quarantined = status_of(&t, url.as_str()).await == HealthStatus::Quarantined;
            assert_eq!(quarantined, n == 3, "fail #{n}");
        }
        assert_eq!(
            reason_of(&t, url.as_str()).await,
            Some(QuarantineReason::FailStreak)
        );
    }

    #[tokio::test]
    async fn an_oversized_or_non_json_example_is_never_sent() {
        let ext = |body: Value, body_type: Value| {
            json!({ "bazaar": { "info": { "input": {
                "method": "POST", "bodyType": body_type, "body": body
            } } } })
        };
        let example_of = |e: &Value| match declared_request(Some(e)) {
            ProbeRequest::Declared { example, .. } => example,
            other => panic!("POST is declared: {other:?}"),
        };
        // `{"q":"…"}` serializes to 8 bytes plus the string.
        let at_cap = json!({ "q": "x".repeat(MAX_PROBE_BODY_BYTES - 8) });
        assert_eq!(
            serde_json::to_string(&at_cap).unwrap().len(),
            MAX_PROBE_BODY_BYTES
        );
        assert_eq!(
            example_of(&ext(at_cap.clone(), json!("json"))),
            Some(serde_json::to_string(&at_cap).unwrap()),
            "exactly at the cap is kept"
        );
        let giant = json!({ "q": "x".repeat(MAX_PROBE_BODY_BYTES - 7) });
        assert_eq!(example_of(&ext(giant.clone(), json!("json"))), None);
        assert_eq!(
            example_of(&ext(json!({ "a": 1 }), json!("form-data"))),
            None
        );
        assert_eq!(example_of(&ext(json!("a=1"), json!("text"))), None);
        assert_eq!(example_of(&ext(Value::Null, json!("json"))), None);
        assert_eq!(
            example_of(&ext(json!({}), json!("json"))),
            None,
            "`{{}}` is no example"
        );
        assert_eq!(
            example_of(&ext(json!({ "a": 1 }), json!(" JSON "))),
            Some(r#"{"a":1}"#.to_string()),
            "bodyType is not case-sensitive"
        );
        assert_eq!(
            example_of(&ext(json!({ "a": 1 }), Value::Null)),
            Some(r#"{"a":1}"#.to_string()),
            "no bodyType: the example is JSON already"
        );

        // On the wire: a seller that refuses `{}` never sees the giant either.
        let mut validating = route(405, 402, &accepts());
        validating.expect_body = Some(giant.clone());
        let (base, seller) = serve(HashMap::from([("/echo".to_string(), validating)])).await;
        let request = declared_request(Some(&ext(giant, json!("json"))));
        let probed = probe_listing(&Loopback::new(), &at(&base, "/echo"), &request, None).await;
        assert_eq!(probed.outcome.http, Some(400));
        let seen = seller.seen("/echo");
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].body, EMPTY_JSON_BODY);
    }

    #[tokio::test]
    async fn a_body_probe_goes_through_the_same_connector_checks_as_a_get() {
        use crate::discovery_security::{json_request, safe_send_json};
        let loopback = url::Url::parse("http://127.0.0.1:8080/x").unwrap();
        assert!(matches!(
            safe_send_json(
                PROBE_UA,
                PROBE_TIMEOUT,
                &loopback,
                reqwest::Method::POST,
                "{}".into()
            )
            .await,
            Err(SecurityReject::DisallowedAddress(_))
        ));
        let bad_port = url::Url::parse("http://93.184.216.34:6379/x").unwrap();
        assert!(matches!(
            safe_send_json(
                PROBE_UA,
                PROBE_TIMEOUT,
                &bad_port,
                reqwest::Method::PUT,
                "{}".into()
            )
            .await,
            Err(SecurityReject::Port(6379))
        ));

        // The shape each hop sends, built without sending it.
        let client = reqwest::Client::new();
        let url = url::Url::parse("https://seller.example/x").unwrap();
        let body = wire_body(ProbeMethod::Patch, Some(r#"{"a":1}"#));
        let req = json_request(&client, &url, body.as_ref()).build().unwrap();
        assert_eq!(req.method(), reqwest::Method::PATCH);
        assert_eq!(
            req.headers().get(reqwest::header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
        assert_eq!(req.body().unwrap().as_bytes(), Some(&br#"{"a":1}"#[..]));
        assert!(!req.headers().keys().any(|k| k.as_str().contains("payment")));
        let req = json_request(&client, &url, wire_body(ProbeMethod::Get, None).as_ref())
            .build()
            .unwrap();
        assert_eq!(req.method(), reqwest::Method::GET);
        assert!(req.body().is_none());
        assert!(req.headers().get(reqwest::header::CONTENT_TYPE).is_none());
    }

    #[tokio::test]
    async fn an_oversized_402_body_is_dropped_and_the_header_still_read() {
        let mut big = route(402, 402, &accepts());
        big.pad = MAX_PROBE_RESPONSE_BYTES + 1;
        let mut small = route(402, 402, &accepts());
        small.pad = 1024;
        let (base, _seller) = serve(HashMap::from([
            ("/big".to_string(), big),
            ("/small".to_string(), small),
        ]))
        .await;
        let out = probe_once(&Loopback::new(), &at(&base, "/big"), ProbeMethod::Get, None).await;
        assert_eq!(out.class, ProbeClass::Alive);
        assert!(out.body.is_none(), "never read whole");
        assert!(
            pay_to_from_402(out.body.as_deref(), out.challenge_header.as_deref()).readable,
            "the challenge is in the header"
        );
        let out = probe_once(
            &Loopback::new(),
            &at(&base, "/small"),
            ProbeMethod::Get,
            None,
        )
        .await;
        assert_eq!(out.body.map(|b| b.len()), Some(1024));
    }

    // ------------------------------------------------------------------------
    // The fallback
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn a_listing_with_no_method_is_found_by_one_post_after_a_405_400_or_404() {
        let mut routes = HashMap::new();
        for code in [405u16, 400, 404] {
            routes.insert(format!("/r{code}"), route(code, 402, &accepts()));
        }
        let (base, seller) = serve(routes).await;
        for code in [405u16, 400, 404] {
            let path = format!("/r{code}");
            let url = at(&base, &path);
            let probed =
                probe_listing(&Loopback::new(), &url, &ProbeRequest::Undeclared, None).await;
            assert_eq!(probed.outcome.class, ProbeClass::Alive, "{code}");
            assert_eq!(probed.method, ProbeMethod::Post, "{code}");
            assert!(probed.fell_back, "{code}");
            let seen = seller.seen(&path);
            assert_eq!(seller.methods(&path), ["GET", "POST"], "{code}");
            assert_eq!(seen[1].body, EMPTY_JSON_BODY);
            assert_eq!(seen[1].content_type.as_deref(), Some("application/json"));
            assert!(!seen[1].payment_header);
        }

        // The method that worked is remembered, and the next cycle starts there.
        let url = at(&base, "/r404");
        let t = HealthTracker::new();
        t.record_probe(
            url.as_str(),
            ProbeClass::Alive,
            Some(402),
            1,
            Some(ProbeMethod::Post),
        )
        .await;
        let recorded = t.recorded_method(url.as_str()).await;
        assert_eq!(recorded, Some(ProbeMethod::Post));
        assert_eq!(
            t.snapshot().await[url.as_str()].probe_method.as_deref(),
            Some("POST"),
            "and the listing says which request answered"
        );
        let again =
            probe_listing(&Loopback::new(), &url, &ProbeRequest::Undeclared, recorded).await;
        assert_eq!(again.outcome.class, ProbeClass::Alive);
        assert!(!again.fell_back);
        assert_eq!(seller.methods("/r404"), ["GET", "POST", "POST"]);
    }

    #[tokio::test]
    async fn the_fallback_is_one_request_and_has_to_be_proven_by_a_readable_challenge() {
        let (base, seller) = serve(HashMap::from([
            ("/both-405".to_string(), route(405, 405, &accepts())),
            (
                "/unreadable".to_string(),
                Route {
                    challenge: None,
                    ..route(405, 402, &accepts())
                },
            ),
            ("/teapot".to_string(), route(418, 402, &accepts())),
        ]))
        .await;

        for path in ["/both-405", "/unreadable"] {
            let probed = probe_listing(
                &Loopback::new(),
                &at(&base, path),
                &ProbeRequest::Undeclared,
                None,
            )
            .await;
            assert_eq!(
                seller.seen(path).len(),
                2,
                "{path}: never more than one extra probe"
            );
            assert_eq!(
                probed.outcome.class,
                ProbeClass::AuthGated,
                "{path}: the GET stands"
            );
            assert_eq!(probed.outcome.http, Some(405), "{path}");
            assert_eq!(probed.method, ProbeMethod::Get, "{path}");
            assert!(!probed.fell_back, "{path}");
        }
        // Any other refusal is not what the fallback is for.
        let probed = probe_listing(
            &Loopback::new(),
            &at(&base, "/teapot"),
            &ProbeRequest::Undeclared,
            None,
        )
        .await;
        assert_eq!(seller.methods("/teapot"), ["GET"]);
        assert_eq!(probed.outcome.http, Some(418));
    }

    #[tokio::test]
    async fn a_remembered_post_that_stops_answering_is_retried_with_get() {
        let (base, seller) = serve(HashMap::from([
            ("/now-get".to_string(), route(402, 404, &accepts())),
            ("/gone".to_string(), route(404, 405, &accepts())),
            ("/busy".to_string(), route(402, 503, &accepts())),
        ]))
        .await;
        // An origin asking to be left alone is not a wrong method: no retry.
        let probed = probe_listing(
            &Loopback::new(),
            &at(&base, "/busy"),
            &ProbeRequest::Undeclared,
            Some(ProbeMethod::Post),
        )
        .await;
        assert_eq!(seller.methods("/busy"), ["POST"]);
        assert_eq!(probed.outcome.http, Some(503));
        let probed = probe_listing(
            &Loopback::new(),
            &at(&base, "/now-get"),
            &ProbeRequest::Undeclared,
            Some(ProbeMethod::Post),
        )
        .await;
        assert_eq!(seller.methods("/now-get"), ["POST", "GET"]);
        assert_eq!(probed.outcome.class, ProbeClass::Alive);
        assert_eq!(probed.method, ProbeMethod::Get);

        // Both refuse: the remembered method's 405 is the answer, and it is
        // not "healthy for its design".
        let probed = probe_listing(
            &Loopback::new(),
            &at(&base, "/gone"),
            &ProbeRequest::Undeclared,
            Some(ProbeMethod::Post),
        )
        .await;
        assert_eq!(seller.methods("/gone"), ["POST", "GET"]);
        assert_eq!(probed.method, ProbeMethod::Post);
        assert_eq!(probed.outcome.http, Some(405));
        assert_eq!(probed.outcome.class, ProbeClass::Degraded);
    }

    #[test]
    fn a_tick_is_planned_in_requests_demand_included() {
        let t = HealthTracker::new();
        let undeclared = |u: &str| target(u, ProbeRequest::Undeclared);
        // The sweep: a host with three listings that may each send two
        // requests, and one that sends one.
        let targets = vec![
            undeclared("https://busy.example/a"),
            undeclared("https://busy.example/b"),
            target("https://busy.example/c", get()),
            target("https://quiet.example/x", get()),
        ];
        let (due, overflow) = plan_tick(&t, targets.clone(), Vec::new(), 120, now_secs());
        let urls: Vec<&str> = due.iter().map(|t| t.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://busy.example/a",
                "https://busy.example/c",
                "https://quiet.example/x"
            ]
        );
        assert!(overflow.is_empty());

        // The global budget is in requests too: 3 requests, not 3 listings --
        // a second two-slot probe does not fit even on a fresh host.
        let spread = vec![
            undeclared("https://busy.example/a"),
            undeclared("https://other.example/y"),
            target("https://other.example/z", get()),
        ];
        let (due, _) = plan_tick(&t, spread, Vec::new(), 3, now_secs());
        let urls: Vec<&str> = due.iter().map(|t| t.url.as_str()).collect();
        assert_eq!(urls, ["https://busy.example/a", "https://other.example/z"]);

        // Demand goes first and is held to the same cap; what does not fit is
        // handed back, and is not probed by the sweep in the same tick.
        let demanded = vec![
            (
                undeclared("https://busy.example/a"),
                RefreshReason::PurchaseIntent,
            ),
            (
                undeclared("https://busy.example/b"),
                RefreshReason::ListingStale,
            ),
        ];
        let (due, overflow) = plan_tick(&t, targets, demanded, 120, now_secs());
        let urls: Vec<&str> = due.iter().map(|t| t.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://busy.example/a",
                "https://busy.example/c",
                "https://quiet.example/x"
            ]
        );
        assert_eq!(
            overflow,
            [(
                "https://busy.example/b".to_string(),
                RefreshReason::ListingStale
            )]
        );
    }

    #[tokio::test]
    async fn demand_that_did_not_fit_goes_back_to_the_queue() {
        let q = crate::discovery_revalidation::RevalidationQueue::new();
        let url = "https://busy.example/b";
        let now = now_secs();
        assert!(q.request(url, RefreshReason::ListingStale, now).await);
        assert_eq!(q.take_batch(10, now).await.len(), 1);
        assert!(
            !q.request(url, RefreshReason::ListingStale, now).await,
            "inside its window a request is refused"
        );
        q.requeue(url, RefreshReason::ListingStale, now).await;
        let again = q.take_batch(10, now).await;
        assert_eq!(again, [(url.to_string(), RefreshReason::ListingStale)]);
    }

    #[test]
    fn the_extra_request_counts_against_the_per_host_cap() {
        let mut per_host = HashMap::new();
        let a = target("https://busy.example/a", ProbeRequest::Undeclared);
        let b = target("https://busy.example/b", ProbeRequest::Undeclared);
        let c = target("https://busy.example/c", get());
        let d = target("https://busy.example/d", post(Some(r#"{"q":1}"#)));
        assert!(
            admit(&mut per_host, &a),
            "two slots: the GET and its fallback"
        );
        assert!(!admit(&mut per_host, &b), "2 + 2 is over the cap of 3");
        assert!(
            !admit(&mut per_host, &d),
            "an example retry reserves two as well"
        );
        assert!(
            admit(&mut per_host, &c),
            "a GET-only probe fits the last slot"
        );
        assert_eq!(per_host["busy.example"], MAX_PER_HOST_PER_TICK);
        assert!(admit(
            &mut per_host,
            &target("https://quiet.example/x", post(None))
        ));
        let mut mcp = target("https://mcp.example/mcp", ProbeRequest::Undeclared);
        mcp.resource_type = "mcp".to_string();
        assert_eq!(
            mcp.slots(),
            MCP_HANDSHAKE_REQUESTS,
            "the MCP handshake is initialize, initialized and tools/list"
        );
        let mut crowded_mcp = target("https://quiet.example/mcp", ProbeRequest::Undeclared);
        crowded_mcp.resource_type = "mcp".to_string();
        assert!(
            !admit(&mut per_host, &crowded_mcp),
            "it does not fit a host that already spent a slot"
        );
        assert!(admit(&mut per_host, &mcp), "and it fits a fresh host whole");
    }

    /// The two listings of the 2026-10-04 report, through the production probe
    /// path: losbeto adds Solana to the Base recipient its catalog copy declares
    /// and is verified; Tavily pays another Base recipient and stays held.
    #[tokio::test]
    async fn an_extra_network_is_verified_and_a_swapped_recipient_stays_held() {
        let option = |network: &str, pay_to: &str| {
            json!({
                "scheme": "exact", "network": network, "payTo": pay_to,
                "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
                "amount": "350000", "maxTimeoutSeconds": 300
            })
        };
        let losbeto = json!([
            option("eip155:8453", "0xd5Ba9711a3D052846a3695C70e7fcb8b3168FE7d"),
            option(
                "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
                "GEhr9HCFTRDjanMg435frSgCVwVZYpNoPrEkmNBnFHFE"
            ),
        ]);
        let tavily_live = json!([
            option("eip155:8453", "0x24B8FD1A73685B25a4c3bA03FAC4A2d733015C6F"),
            { "scheme": "agent-pay", "network": "aws:base",
              "payTo": "urn:x402:agent-pay:see-quote", "amount": "0" },
        ]);
        let (base, _seller) = serve(HashMap::from([
            ("/council-deep".to_string(), route(402, 405, &losbeto)),
            ("/held".to_string(), route(402, 405, &losbeto)),
            ("/search".to_string(), route(405, 402, &tavily_live)),
        ]))
        .await;
        let lo = Loopback::serving(&base);
        let registry = DiscoveryRegistry::new();
        let t = HealthTracker::new();
        let usdc = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
        let losbeto_target = |p: &str| ProbeTarget {
            pay_to: vec![DeclaredOffer::new(
                "exact",
                "eip155:8453",
                usdc,
                "0xd5Ba9711a3D052846a3695C70e7fcb8b3168FE7d",
            )],
            ..target(&format!("https://api.losbeto.xyz{p}"), get())
        };
        let tavily = ProbeTarget {
            pay_to: vec![DeclaredOffer::new(
                "exact",
                "eip155:8453",
                usdc,
                "0xC5c967576a19Ed250030f8C15B6158968DD64643",
            )],
            ..target("https://x402.tavily.com/search", post(None))
        };

        probe_and_record(&lo, &registry, &t, losbeto_target("/council-deep")).await;
        let state = t
            .snapshot()
            .await
            .remove("https://api.losbeto.xyz/council-deep")
            .unwrap();
        assert_eq!(state.status, HealthStatus::Alive);
        assert_eq!(state.quarantine_reason, None);
        assert_eq!(state.verified_by, Some(VerifiedBy::X402Challenge));
        // The extra option is in the observation, never in the listing.
        let observed = registry
            .terms()
            .get("https://api.losbeto.xyz/council-deep")
            .await
            .expect("the challenge is recorded");
        assert_eq!(observed.accepts.len(), 2);

        // A listing a build without this rule held for drift recovers like any
        // drift hold: two clean challenges in a row, never one.
        let held = "https://api.losbeto.xyz/held";
        t.record_probe(
            held,
            ProbeClass::PayToDrift,
            Some(402),
            1,
            Some(ProbeMethod::Get),
        )
        .await;
        probe_and_record(&lo, &registry, &t, losbeto_target("/held")).await;
        assert_eq!(status_of(&t, held).await, HealthStatus::Quarantined);
        probe_and_record(&lo, &registry, &t, losbeto_target("/held")).await;
        assert_eq!(status_of(&t, held).await, HealthStatus::Alive);

        probe_and_record(&lo, &registry, &t, tavily).await;
        let state = t
            .snapshot()
            .await
            .remove("https://x402.tavily.com/search")
            .unwrap();
        assert_eq!(state.status, HealthStatus::Quarantined);
        assert_eq!(state.quarantine_reason, Some(QuarantineReason::PayToDrift));
        assert_eq!(state.verified_by, None);
    }

    #[tokio::test]
    async fn an_origin_that_names_its_method_in_its_challenge_is_asked_that_way_next() {
        let accepts = accepts();
        let naming = |url: &str, method: &str| {
            header_of(&json!({
                "x402Version": 2,
                "resource": { "url": url, "method": method },
                "accepts": accepts
            }))
        };
        let path = |p: &str, get: u16, post: u16, challenge: String| {
            (
                p.to_string(),
                Route {
                    challenge: Some(challenge),
                    ..route(get, post, &accepts)
                },
            )
        };
        let mut hijacked = accepts.clone();
        hijacked[0]["payTo"] = json!("0x000000000000000000000000000000000000dEaD");
        let drifted = header_of(&json!({
            "x402Version": 2,
            "resource": { "url": "https://seller.example/drifted", "method": "POST" },
            "accepts": hijacked
        }));
        let (base, seller) = serve(HashMap::from([
            path(
                "/query",
                402,
                402,
                naming("https://seller.example/query", "POST"),
            ),
            path(
                "/elsewhere",
                402,
                402,
                naming("https://other.example/q", "POST"),
            ),
            path(
                "/put",
                402,
                402,
                naming("https://seller.example/put", "PUT"),
            ),
            path(
                "/forget",
                402,
                404,
                naming("https://seller.example/forget", "POST"),
            ),
            path("/drifted", 402, 402, drifted),
        ]))
        .await;
        let registry = DiscoveryRegistry::new();
        let t = HealthTracker::new();
        let lo = Loopback::serving(&base);
        // What the catalog declares for this listing, by the import rule itself.
        let (mut imported, _rejected) = crate::discovery_aggregator::convert_resources(
            vec![serde_json::from_value(tavily()).unwrap()],
            "coinbase",
        );
        let declared = declared_offers(&imported.remove(0).accepts);
        let undeclared = |p: &str| ProbeTarget {
            pay_to: declared.clone(),
            ..target(
                &format!("https://seller.example{p}"),
                ProbeRequest::Undeclared,
            )
        };
        let learned = |t: &HealthTracker, p: &str| {
            let url = format!("https://seller.example{p}");
            let records = t.records.try_read().unwrap();
            records[url.as_str()].learned_method.clone()
        };

        for p in ["/query", "/elsewhere", "/put", "/forget", "/drifted"] {
            probe_and_record(&lo, &registry, &t, undeclared(p)).await;
            assert_eq!(seller.methods(p), ["GET"], "{p}");
        }
        assert_eq!(learned(&t, "/query").as_deref(), Some("POST"));
        assert_eq!(
            learned(&t, "/elsewhere"),
            None,
            "another host names nothing for us"
        );
        assert_eq!(learned(&t, "/put"), None, "only a POST is learned");
        assert_eq!(
            learned(&t, "/drifted"),
            None,
            "a challenge that failed the drift check names nothing"
        );

        probe_and_record(&lo, &registry, &t, undeclared("/query")).await;
        assert_eq!(
            seller.methods("/query"),
            ["GET", "POST"],
            "its own challenge said POST"
        );
        probe_and_record(&lo, &registry, &t, undeclared("/elsewhere")).await;
        assert_eq!(seller.methods("/elsewhere"), ["GET", "GET"]);

        // The remembered POST stops answering and the GET still does: the
        // fallback answers, and what was learned is forgotten.
        probe_and_record(&lo, &registry, &t, undeclared("/forget")).await;
        assert_eq!(seller.methods("/forget"), ["GET", "POST", "GET"]);
        assert_eq!(learned(&t, "/forget"), None);
    }

    // ------------------------------------------------------------------------
    // The state machine
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn a_quarantine_built_by_get_is_reprobed_at_once_and_lifted_by_the_first_402() {
        let url = "https://seller.example/search";
        let post_target = target(url, post(None));
        let t = HealthTracker::new();
        // What the GET-only build left: three GET 404s, quarantined, and the
        // next probe an hour or more away.
        for _ in 0..3 {
            t.record_probe(url, ProbeClass::Fail, Some(404), 1, None)
                .await;
        }
        assert_eq!(status_of(&t, url).await, HealthStatus::Quarantined);
        let now = now_secs();
        assert!(
            tracker_due(&t, &post_target, now),
            "re-probed this cycle, not after the backoff"
        );
        // The same record under a listing that declares GET keeps its schedule:
        // that one was probed with the right request.
        assert!(!tracker_due(&t, &target(url, get()), now));

        t.record_probe(
            url,
            ProbeClass::Alive,
            Some(402),
            1,
            Some(ProbeMethod::Post),
        )
        .await;
        assert_eq!(
            status_of(&t, url).await,
            HealthStatus::Alive,
            "the first live challenge to the right request lifts what the wrong one built"
        );
        assert!(
            !tracker_due(&t, &post_target, now_secs()),
            "once probed right, it is back on its schedule"
        );
    }

    #[tokio::test]
    async fn a_get_405_or_404_without_a_declared_method_is_reprobed_once_for_the_fallback() {
        for (class, code) in [(ProbeClass::AuthGated, 405u16), (ProbeClass::Fail, 404)] {
            let url = "https://seller.example/lookup";
            let undeclared = target(url, ProbeRequest::Undeclared);
            let t = HealthTracker::new();
            t.record_probe(url, class, Some(code), 1, None).await;
            assert!(
                tracker_due(&t, &undeclared, now_secs()),
                "{code}: not after its schedule"
            );
            t.record_probe(url, class, Some(code), 1, Some(ProbeMethod::Get))
                .await;
            assert!(
                !tracker_due(&t, &undeclared, now_secs()),
                "{code}: a build with the fallback already tried it"
            );
        }
    }

    #[tokio::test]
    async fn mcp_endpoints_keep_their_handshake_and_their_schedule() {
        let url = "https://mcp.seller.example/mcp";
        let mut mcp = target(url, post(None));
        mcp.resource_type = "mcp".to_string();
        let t = HealthTracker::new();
        t.record_probe(url, ProbeClass::AuthGated, Some(405), 1, None)
            .await;
        assert!(!tracker_due(&t, &mcp, now_secs()));
        t.record_probe(url, ProbeClass::PayToDrift, Some(402), 1, None)
            .await;
        assert!(!tracker_due(&t, &mcp, now_secs()));
    }

    #[tokio::test]
    async fn evidence_about_another_request_does_not_count_toward_quarantine() {
        let url = "https://seller.example/search";
        let t = HealthTracker::new();
        t.record_probe(url, ProbeClass::Fail, Some(404), 1, None)
            .await;
        t.record_probe(url, ProbeClass::Fail, Some(404), 1, None)
            .await;
        // One POST failure is one failure of THIS request, not the third.
        t.record_probe(url, ProbeClass::Fail, Some(404), 1, Some(ProbeMethod::Post))
            .await;
        assert_ne!(status_of(&t, url).await, HealthStatus::Quarantined);
    }

    #[tokio::test]
    async fn a_drift_hold_is_never_lifted_by_a_single_402() {
        let url = "https://hijacked.example/pay";
        let post_target = target(url, post(None));

        // A hold this build set: its schedule stands, and a new request does
        // not shortcut its recovery.
        let t = HealthTracker::new();
        t.record_probe(
            url,
            ProbeClass::PayToDrift,
            Some(402),
            1,
            Some(ProbeMethod::Get),
        )
        .await;
        assert_eq!(reason_of(&t, url).await, Some(QuarantineReason::PayToDrift));
        assert!(!tracker_due(&t, &post_target, now_secs()));
        t.record_probe(
            url,
            ProbeClass::Alive,
            Some(402),
            1,
            Some(ProbeMethod::Post),
        )
        .await;
        assert_eq!(
            status_of(&t, url).await,
            HealthStatus::Quarantined,
            "one clean challenge to another request does not lift it"
        );
        assert_eq!(reason_of(&t, url).await, Some(QuarantineReason::PayToDrift));
        t.record_probe(
            url,
            ProbeClass::Alive,
            Some(402),
            1,
            Some(ProbeMethod::Post),
        )
        .await;
        assert_eq!(
            status_of(&t, url).await,
            HealthStatus::Alive,
            "two do, as always"
        );
        assert_eq!(reason_of(&t, url).await, None);

        // A hold an older build set (no method, no reason): read by its
        // signature, looked at once now -- and still not lifted by one 402.
        let legacy = HealthTracker::new();
        legacy
            .record_probe(url, ProbeClass::PayToDrift, Some(402), 1, None)
            .await;
        legacy
            .records
            .write()
            .await
            .get_mut(url)
            .unwrap()
            .quarantine_reason = None;
        assert_eq!(
            reason_of(&legacy, url).await,
            Some(QuarantineReason::PayToDrift)
        );
        assert!(
            tracker_due(&legacy, &post_target, now_secs()),
            "looked at once"
        );
        legacy
            .record_probe(
                url,
                ProbeClass::Alive,
                Some(402),
                1,
                Some(ProbeMethod::Post),
            )
            .await;
        assert_eq!(status_of(&legacy, url).await, HealthStatus::Quarantined);
        assert!(
            !tracker_due(&legacy, &post_target, now_secs()),
            "and only once"
        );
    }

    #[tokio::test]
    async fn a_drift_hold_the_per_network_rule_never_judged_is_looked_at_once() {
        // A listing a build before the per-network rule held because its
        // challenge added a network: probed again straight away instead of in
        // 72 hours, still lifted only by two clean challenges.
        let url = "https://api.losbeto.xyz/stock-quote";
        let get_target = target(url, get());
        let t = HealthTracker::new();
        t.record_probe(
            url,
            ProbeClass::PayToDrift,
            Some(402),
            1,
            Some(ProbeMethod::Get),
        )
        .await;
        assert!(
            !tracker_due(&t, &get_target, now_secs()),
            "a hold this rule set keeps its schedule"
        );
        // What the previous build left in the overlay: the same hold, no rule.
        t.records.write().await.get_mut(url).unwrap().drift_rule = None;
        assert!(tracker_due(&t, &get_target, now_secs()), "looked at once");
        t.record_probe(url, ProbeClass::Alive, Some(402), 1, Some(ProbeMethod::Get))
            .await;
        assert_eq!(
            status_of(&t, url).await,
            HealthStatus::Quarantined,
            "one clean challenge lifts nothing"
        );
        assert!(
            !tracker_due(&t, &get_target, now_secs()),
            "and only once: the second challenge waits its hour"
        );

        // An overlay written before the field existed reads as that hold.
        let old = r#"{"https://x.example/a":{"status":"quarantined","http_status":402,
            "consecutive_ok":0,"consecutive_fail":3,"next_probe_at":0,
            "quarantine_reason":"pay_to_drift","probe_method":"GET"}}"#;
        let parsed = parse_overlay(old.as_bytes()).unwrap();
        assert_eq!(parsed["https://x.example/a"].drift_rule, None);
        assert!(parsed["https://x.example/a"].held_for_drift());
    }

    #[tokio::test]
    async fn a_drift_hold_is_not_lifted_by_an_answer_that_is_not_a_challenge() {
        for (class, code) in [
            (ProbeClass::AuthGated, Some(401)),
            (ProbeClass::Degraded, Some(400)),
            (ProbeClass::Unprobeable, None),
        ] {
            let url = "https://hijacked.example/pay";
            let t = HealthTracker::new();
            t.record_probe(
                url,
                ProbeClass::PayToDrift,
                Some(402),
                1,
                Some(ProbeMethod::Post),
            )
            .await;
            t.record_probe(
                url,
                ProbeClass::Alive,
                Some(402),
                1,
                Some(ProbeMethod::Post),
            )
            .await;
            t.record_probe(url, class, code, 1, Some(ProbeMethod::Post))
                .await;
            assert_eq!(
                status_of(&t, url).await,
                HealthStatus::Quarantined,
                "{class:?}"
            );
            assert_eq!(
                reason_of(&t, url).await,
                Some(QuarantineReason::PayToDrift),
                "{class:?}"
            );
            // And it reset the clean streak: one more challenge is not two.
            t.record_probe(
                url,
                ProbeClass::Alive,
                Some(402),
                1,
                Some(ProbeMethod::Post),
            )
            .await;
            assert_eq!(
                status_of(&t, url).await,
                HealthStatus::Quarantined,
                "{class:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_legacy_drift_hold_stays_a_drift_hold_through_a_failure() {
        // Read off its signature on the first probe by this build, and KEPT:
        // once its last status code is a 404, the signature would say "fail
        // streak", and a change of request would lift it with one 402.
        let url = "https://hijacked.example/pay";
        let old: HashMap<String, HealthRecord> = serde_json::from_value(json!({
            "https://hijacked.example/pay": { "status": "quarantined", "http_status": 402 }
        }))
        .unwrap();
        let t = HealthTracker::new();
        *t.records.write().await = old;
        t.record_probe(url, ProbeClass::Fail, Some(404), 1, Some(ProbeMethod::Get))
            .await;
        assert_eq!(reason_of(&t, url).await, Some(QuarantineReason::PayToDrift));
        t.record_probe(
            url,
            ProbeClass::Alive,
            Some(402),
            1,
            Some(ProbeMethod::Post),
        )
        .await;
        assert_eq!(
            status_of(&t, url).await,
            HealthStatus::Quarantined,
            "a new request does not lift a drift hold"
        );
    }

    #[tokio::test]
    async fn the_quarantine_reason_is_kept_and_published() {
        let url = "https://seller.example/x";
        let t = HealthTracker::new();
        for _ in 0..3 {
            t.record_probe(url, ProbeClass::Fail, Some(404), 1, Some(ProbeMethod::Get))
                .await;
        }
        let json = serde_json::to_value(&t.snapshot().await[url]).unwrap();
        assert_eq!(json["quarantineReason"], "fail_streak");
        // One clean challenge into the recovery: still a fail streak, not a
        // drift -- the reason is kept, not guessed from the last status code.
        t.record_probe(url, ProbeClass::Alive, Some(402), 1, Some(ProbeMethod::Get))
            .await;
        assert_eq!(reason_of(&t, url).await, Some(QuarantineReason::FailStreak));
        t.record_probe(
            url,
            ProbeClass::PayToDrift,
            Some(402),
            1,
            Some(ProbeMethod::Get),
        )
        .await;
        assert_eq!(reason_of(&t, url).await, Some(QuarantineReason::PayToDrift));
        for _ in 0..2 {
            t.record_probe(url, ProbeClass::Alive, Some(402), 1, Some(ProbeMethod::Get))
                .await;
        }
        let json = serde_json::to_value(&t.snapshot().await[url]).unwrap();
        assert_eq!(json["status"], "alive");
        assert!(
            json.get("quarantineReason").is_none(),
            "only while quarantined"
        );

        // A record from before the reason existed, as the overlay holds it.
        let old: HashMap<String, HealthRecord> = serde_json::from_value(json!({
            "https://a.example/x": { "status": "quarantined", "http_status": 402 },
            "https://b.example/x": { "status": "quarantined", "http_status": 404 },
            "https://c.example/x": {
                "status": "quarantined", "http_status": 402, "consecutive_ok": 1
            }
        }))
        .unwrap();
        let t = HealthTracker::new();
        *t.records.write().await = old;
        assert_eq!(
            reason_of(&t, "https://a.example/x").await,
            Some(QuarantineReason::PayToDrift)
        );
        assert_eq!(
            reason_of(&t, "https://b.example/x").await,
            Some(QuarantineReason::FailStreak)
        );
        assert_eq!(
            reason_of(&t, "https://c.example/x").await,
            Some(QuarantineReason::FailStreak),
            "one clean challenge into a recovery is not a drift"
        );
        // Its first probe by this build keeps the reason the signature gave it,
        // even when that probe's 402 would read as a drift on its own.
        t.record_probe(
            "https://b.example/x",
            ProbeClass::Alive,
            Some(402),
            1,
            Some(ProbeMethod::Get),
        )
        .await;
        assert_eq!(
            reason_of(&t, "https://b.example/x").await,
            Some(QuarantineReason::FailStreak)
        );
    }

    #[test]
    fn the_drift_check_ignores_a_quote_reference_and_nothing_else() {
        let listing = tavily();
        // What the catalog keeps from this listing, by the import rule itself:
        // the agent-pay option is dropped, so its quote reference is never a
        // declared recipient.
        let (mut imported, _rejected) = crate::discovery_aggregator::convert_resources(
            vec![serde_json::from_value(listing.clone()).unwrap()],
            "coinbase",
        );
        let declared = declared_offers(&imported.remove(0).accepts);
        assert_eq!(declared.len(), 1);
        assert_eq!(declared[0].network.as_deref(), Some("eip155:8453"));
        assert_eq!(
            declared[0].pay_to,
            "0xfe2d09ca270818e9736207ee27f0fa464a67ac66"
        );

        let live = pay_to_from_402(None, Some(&challenge_for(&listing["accepts"])));
        assert!(live.readable);
        assert_eq!(
            live.pay_to,
            ["0xfe2d09ca270818e9736207ee27f0fa464a67ac66"],
            "urn:x402:agent-pay:see-quote is not a recipient"
        );
        assert!(
            !pay_to_drifted(&declared, &live),
            "the seller's own challenge"
        );

        let with = |slot: usize, pay_to: &str| {
            let mut a = listing["accepts"].clone();
            a[slot]["payTo"] = json!(pay_to);
            pay_to_from_402(None, Some(&challenge_for(&a)))
        };
        // Another URN, in any case, is still not a recipient.
        assert!(!pay_to_drifted(
            &declared,
            &with(1, "URN:x402:agent-pay:other")
        ));
        // The declared address in another spelling is the declared address.
        assert!(!pay_to_drifted(
            &declared,
            &with(0, " 0XFE2D09CA270818E9736207EE27F0FA464A67AC66 ")
        ));
        // Fails closed: anything else counts, in either slot -- including
        // spellings and chains this build cannot parse.
        for slot in [0, 1] {
            for swapped in [
                "0x000000000000000000000000000000000000dEaD",
                "0X000000000000000000000000000000000000DEAD",
                "attacker-account",
                "cosmos1qypqxpq9qcrsszg2pvxq6rs0zqg3yyc5lzv7xu",
                "eip155:8453:0x000000000000000000000000000000000000dEaD",
            ] {
                assert!(
                    pay_to_drifted(&declared, &with(slot, swapped)),
                    "slot {slot}: {swapped}"
                );
            }
        }
        // A payTo that is not a string is no recipient we can name: it counts.
        let mut odd = listing["accepts"].clone();
        odd[1]["payTo"] = json!({ "to": "0x000000000000000000000000000000000000dEaD" });
        let live = pay_to_from_402(None, Some(&challenge_for(&odd)));
        assert!(pay_to_drifted(&declared, &live), "fails closed");
        odd[1]["payTo"] = Value::Null;
        let live = pay_to_from_402(None, Some(&challenge_for(&odd)));
        assert!(
            !pay_to_drifted(&declared, &live),
            "a null one is nothing at all"
        );
    }

    #[tokio::test]
    async fn the_observation_records_the_request_that_drew_it() {
        let registry = DiscoveryRegistry::new();
        let overlay = registry.terms();
        let url = url::Url::parse("https://x402.tavily.com/search").unwrap();
        let live = pay_to_from_402(None, Some(&challenge_for(&accepts())));
        record_observation(
            &registry,
            &overlay,
            &url,
            "http",
            ProbeMethod::Post,
            Some(402),
            live,
        )
        .await;
        let observed = overlay.get(url.as_str()).await.unwrap();
        assert_eq!(observed.context.method, "POST");
        assert_eq!(
            observed.accepts[0].amount.to_string(),
            "10000",
            "and the price it read"
        );
    }

    #[tokio::test]
    async fn the_uptime_attested_is_the_uptime_listed() {
        let t = HealthTracker::new();
        let url = "https://seller.example/x";
        assert!(t.snapshot().await.is_empty());
        t.record_probe(url, ProbeClass::Alive, Some(402), 1, Some(ProbeMethod::Get))
            .await;
        t.record_probe(url, ProbeClass::Fail, Some(500), 1, Some(ProbeMethod::Get))
            .await;
        t.record_probe(
            url,
            ProbeClass::AuthGated,
            Some(401),
            1,
            Some(ProbeMethod::Get),
        )
        .await;
        let state = t.snapshot().await.remove(url).unwrap();
        assert_eq!(state.probe_count, Some(3));
        assert_eq!(state.uptime_bps, Some(6666));
        assert_eq!(
            t.uptime(url).await,
            Some((6666, 3, 2)),
            "the attestation's number"
        );
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["uptimeBps"], 6666);
        assert_eq!(json["probeCount"], 3);
        assert_eq!(json["probeMethod"], "GET");
        // A record that predates all of this serializes exactly as before.
        let legacy = HealthTracker::new();
        legacy
            .record_probe(url, ProbeClass::Alive, Some(402), 1, None)
            .await;
        let json = serde_json::to_value(legacy.snapshot().await.remove(url).unwrap()).unwrap();
        assert!(json.get("probeMethod").is_none());
        assert!(json.get("quarantineReason").is_none());
    }

    // ------------------------------------------------------------------------
    // End to end
    // ------------------------------------------------------------------------

    /// The listings the router report found live but hidden or mislabelled,
    /// plus the first-party family measured the same day, end to end: the
    /// catalog import, `probe_targets`, the record the GET-only build left,
    /// one `probe_and_record`, and what the listing shows after it.
    #[tokio::test]
    async fn the_reported_listings_are_found_with_the_request_they_declare() {
        let file: Value = serde_json::from_str(REPORT_CASES).unwrap();
        let cases = file["cases"].as_array().unwrap();
        let registry = DiscoveryRegistry::new();
        let mut routes = HashMap::new();
        let mut resources = Vec::new();
        for case in cases {
            let listing = match case["capturedIn"].as_str() {
                Some(_) => captured(case["resource"].as_str().unwrap()),
                None => case["listing"].clone(),
            };
            let (mut imported, _) = crate::discovery_aggregator::convert_resources(
                vec![serde_json::from_value(listing.clone()).unwrap()],
                "coinbase",
            );
            let resource = imported.remove(0);
            routes.insert(
                resource.url.path().to_string(),
                route(
                    case["seller"]["GET"].as_u64().unwrap() as u16,
                    case["seller"]["POST"].as_u64().unwrap() as u16,
                    &listing["accepts"],
                ),
            );
            resources.push(resource.url.to_string());
            registry.register(resource).await.unwrap();
        }
        let (base, seller) = serve(routes).await;
        let lo = Loopback::serving(&base);
        let targets = registry.probe_targets().await;
        assert_eq!(targets.len(), cases.len());

        for (case, resource) in cases.iter().zip(&resources) {
            let name = case["name"].as_str().unwrap();
            let target = targets
                .iter()
                .find(|t| t.url.as_str() == resource)
                .unwrap()
                .clone();
            let url = target.url.to_string();

            // What the GET-only build left behind.
            let t = HealthTracker::new();
            match case["reported"].as_str().unwrap() {
                "quarantined" => {
                    for _ in 0..3 {
                        t.record_probe(&url, ProbeClass::Fail, Some(404), 1, None)
                            .await;
                    }
                }
                "auth_gated" => {
                    t.record_probe(&url, ProbeClass::AuthGated, Some(405), 1, None)
                        .await;
                }
                other => panic!("{name}: unexpected label {other}"),
            }

            let due_now = tracker_due(&t, &target, now_secs());
            probe_and_record(&lo, &registry, &t, target.clone()).await;
            let state = t.snapshot().await.remove(&url).unwrap();
            let expect = &case["expect"];
            assert_eq!(
                state.probe_method.as_deref(),
                expect["method"].as_str(),
                "{name}: answered by"
            );
            assert_eq!(state.http_status, Some(402), "{name}");
            let visible = state.status != HealthStatus::Quarantined;
            let expect_visible = expect["visibleAfterOneProbe"].as_bool().unwrap();
            assert_eq!(due_now, expect_visible, "{name}: re-probed this cycle");
            assert_eq!(visible, expect_visible, "{name}: visible after one probe");
            assert_eq!(
                is_verified_alive(
                    &state,
                    &target.request,
                    false,
                    None,
                    now_secs(),
                    crate::discovery_terms::freshness_window_secs()
                ),
                expect_visible,
                "{name}: and exposed, by the rule every public surface applies"
            );
            if visible {
                assert_eq!(state.status, HealthStatus::Alive, "{name}: same vocabulary");
            } else {
                assert_eq!(
                    state.quarantine_reason,
                    Some(QuarantineReason::FailStreak),
                    "{name}: and it says why"
                );
            }
            assert!(
                !tracker_due(&t, &target, now_secs()),
                "{name}: back on its schedule after one probe"
            );
            let observed = registry.terms().get(&url).await.unwrap();
            assert_eq!(
                Some(observed.context.method.as_str()),
                expect["method"].as_str(),
                "{name}: the price says which request it answers"
            );
        }
        // Tavily got `{}` and answered: its example never travelled.
        let tavily = seller.seen("/search");
        assert!(tavily
            .iter()
            .all(|s| s.body.is_empty() || s.body == EMPTY_JSON_BODY));
        // The one that declares nothing: GET, then the one fallback POST.
        assert_eq!(seller.methods("/api/phone-lookup"), ["GET", "POST"]);
        assert_eq!(
            seller.methods("/payments/access/alpha-test"),
            ["GET", "POST"]
        );
    }

    // ------------------------------------------------------------------------
    // Exposure: only what is verified alive leaves the registry
    // ------------------------------------------------------------------------

    /// A listing of `path` on `seller.example` paying Tavily's options,
    /// declaring `method` when given, imported the way the aggregator does.
    fn listing_of(path: &str, method: Option<&str>) -> crate::types_v2::DiscoveryResource {
        let mut listing = json!({
            "resource": format!("https://seller.example{path}"),
            "type": "http",
            "x402Version": 2,
            "description": "exposure fixture",
            "accepts": accepts(),
        });
        if let Some(m) = method {
            listing["extensions"] =
                json!({ "bazaar": { "info": { "input": { "type": "http", "method": m } } } });
        }
        let (mut imported, _) = crate::discovery_aggregator::convert_resources(
            vec![serde_json::from_value(listing).unwrap()],
            "coinbase",
        );
        imported.remove(0)
    }

    /// Every exposed URL, by walking the default listing page by page, as a
    /// consumer does (Emporium's `bazar.rs` walks it by offset).
    async fn walk(registry: &DiscoveryRegistry) -> Vec<String> {
        let mut urls = Vec::new();
        loop {
            let page = registry.list(2, urls.len() as u32, None).await;
            if page.items.is_empty() {
                break;
            }
            urls.extend(page.items.into_iter().map(|r| r.url.to_string()));
        }
        urls.sort();
        urls
    }

    /// The owner's rule, end to end through real probes of a local seller: a
    /// listing is exposed only once a probe with the request it declares has
    /// read a valid challenge in a 402. Auth-gated, degraded, quarantined, an
    /// unreadable 402, a GET answer to a POST listing, and never probed: none
    /// of those reach the listing, the stats or a full walk, and the stats
    /// count exactly what the walk returns.
    #[tokio::test]
    async fn only_a_verified_challenge_exposes_a_listing_anywhere() {
        let a = accepts();
        let (base, _seller) = serve(HashMap::from([
            ("/get-alive".to_string(), route(402, 405, &a)),
            ("/post-alive".to_string(), route(405, 402, &a)),
            ("/auth".to_string(), route(401, 401, &a)),
            ("/degraded".to_string(), route(200, 200, &a)),
            ("/dead".to_string(), route(404, 404, &a)),
            (
                "/garbage".to_string(),
                Route {
                    challenge: None,
                    ..route(402, 402, &a)
                },
            ),
            ("/get-only-alive".to_string(), route(402, 404, &a)),
        ]))
        .await;
        let registry = DiscoveryRegistry::new();
        for (path, method) in [
            ("/get-alive", None),
            ("/post-alive", Some("POST")),
            ("/auth", Some("GET")),
            ("/degraded", Some("GET")),
            ("/dead", Some("GET")),
            ("/garbage", Some("GET")),
            ("/get-only-alive", Some("POST")),
            ("/never-probed", Some("GET")),
        ] {
            registry.register(listing_of(path, method)).await.unwrap();
        }
        assert!(walk(&registry).await.is_empty(), "nothing before a probe");

        let lo = Loopback::serving(&base);
        let health = registry.health();
        for t in registry.probe_targets().await {
            let rounds = if t.url.path() == "/dead" { 3 } else { 1 };
            if t.url.path() == "/never-probed" {
                continue;
            }
            for _ in 0..rounds {
                probe_and_record(&lo, &registry, &health, t.clone()).await;
            }
        }
        // A POST listing a GET found alive -- what the GET-only build left --
        // is not verified by that answer, however clean the challenge.
        let get_only = "https://seller.example/get-only-alive";
        health.mark_verified(get_only, ProbeMethod::Get).await;

        let states = health.snapshot().await;
        let status = |p: &str| states[&format!("https://seller.example{p}")].status;
        assert_eq!(status("/auth"), HealthStatus::AuthGated);
        assert_eq!(status("/degraded"), HealthStatus::Degraded);
        assert_eq!(status("/dead"), HealthStatus::Quarantined);
        assert_eq!(
            status("/garbage"),
            HealthStatus::Alive,
            "alive by the old rule"
        );
        assert_eq!(status("/get-only-alive"), HealthStatus::Alive);

        let exposed = vec![
            "https://seller.example/get-alive".to_string(),
            "https://seller.example/post-alive".to_string(),
        ];
        assert_eq!(walk(&registry).await, exposed);
        let stats = registry.stats().await;
        assert_eq!(stats["verifiedAlive"], 2);
        assert_eq!(stats["visible"], 2);
        assert_eq!(stats["total"], 2, "no public count of the rest");
        assert_eq!(stats["byHealth"], json!({ "alive": 2 }));
        // No parameter widens it.
        for health_filter in ["any", "quarantined", "auth_gated", "degraded", "unknown"] {
            let filters = crate::types_v2::DiscoveryFilters {
                health: Some(health_filter.to_string()),
                ..Default::default()
            };
            let page = registry.list(100, 0, Some(filters)).await;
            assert!(
                page.items
                    .iter()
                    .all(|r| exposed.contains(&r.url.to_string())),
                "health={health_filter}"
            );
        }
        // The rest is held, probed, and readable by the admin route only.
        let pending = registry.list_pending(100, 0).await;
        assert_eq!(pending.pagination.total, 6);
        assert!(pending
            .items
            .iter()
            .all(|r| !exposed.contains(&r.url.to_string())));
        let mut exposed_set: Vec<String> = registry.exposed_urls().await.into_iter().collect();
        exposed_set.sort();
        assert_eq!(exposed_set, exposed);
    }

    /// A listing appears on the first probe that verifies it, and leaves on the
    /// first that does not: verified alive is about the LAST probe.
    #[tokio::test]
    async fn a_listing_is_exposed_by_its_last_probe_not_its_best() {
        let a = accepts();
        let (base, _seller) =
            serve(HashMap::from([("/flaky".to_string(), route(405, 402, &a))])).await;
        let registry = DiscoveryRegistry::new();
        registry
            .register(listing_of("/flaky", Some("POST")))
            .await
            .unwrap();
        let target = registry.probe_targets().await.remove(0);
        let health = registry.health();
        assert_eq!(registry.list(10, 0, None).await.pagination.total, 0);

        probe_and_record(
            &Loopback::serving(&base),
            &registry,
            &health,
            target.clone(),
        )
        .await;
        assert_eq!(registry.list(10, 0, None).await.pagination.total, 1);
        let state = health.snapshot().await.remove(target.url.as_str()).unwrap();
        assert!(state.verified_at.is_some());
        assert_eq!(
            serde_json::to_value(&state).unwrap()["verifiedAt"],
            json!(state.verified_at.unwrap())
        );

        // Same seller, now answering a 402 with no challenge in it.
        let (garbage, _s) = serve(HashMap::from([(
            "/flaky".to_string(),
            Route {
                challenge: None,
                ..route(405, 402, &a)
            },
        )]))
        .await;
        probe_and_record(
            &Loopback::serving(&garbage),
            &registry,
            &health,
            target.clone(),
        )
        .await;
        let state = health.snapshot().await.remove(target.url.as_str()).unwrap();
        assert_eq!(state.status, HealthStatus::Alive);
        assert_eq!(
            state.verified_at, None,
            "cleared by a probe that read nothing"
        );
        assert_eq!(registry.list(10, 0, None).await.pagination.total, 0);
    }

    /// A transport that answers every request with the same 402, carrying
    /// `challenge` in its header, as the connector hands a response over after
    /// following `redirect` (when set).
    struct Answers {
        challenge: String,
        redirect: Option<crate::discovery_security::FollowedRedirect>,
    }

    #[async_trait::async_trait]
    impl ProbeTransport for Answers {
        async fn send(
            &self,
            _url: &url::Url,
            _method: ProbeMethod,
            _body: Option<&str>,
        ) -> Result<reqwest::Response, SecurityReject> {
            let mut response = axum::http::Response::builder()
                .status(402)
                .header("payment-required", self.challenge.as_str())
                .body("{}")
                .unwrap();
            if let Some(redirect) = &self.redirect {
                response.extensions_mut().insert(redirect.clone());
            }
            Ok(reqwest::Response::from(response))
        }

        async fn send_mcp(
            &self,
            _url: &url::Url,
            _body: &str,
            _session: Option<&str>,
        ) -> Result<reqwest::Response, SecurityReject> {
            Err(SecurityReject::Http("no MCP here".to_string()))
        }
    }

    /// A 402 verifies a listing only as the listing's own answer to the
    /// request it declares, and only when it offers a payment: not one another
    /// host gave after a redirect, not one a 301/302/303 reached by turning the
    /// POST into a GET, and not a challenge with no option we can read --
    /// whatever its status says.
    #[tokio::test]
    async fn a_402_verifies_only_the_listings_own_answer_offering_a_payment() {
        use crate::discovery_security::FollowedRedirect;
        let followed = |host: &str, method_kept: bool| {
            Some(FollowedRedirect {
                host: Some(host.to_string()),
                method_kept,
            })
        };
        let a = accepts();
        // The listing's own recipient, so the drift check passes: only the
        // options are missing.
        let unreadable_options = json!([{ "payTo": a[0]["payTo"] }]);
        let cases = [
            ("as sent", challenge_for(&a), None, true),
            (
                "same host, same method",
                challenge_for(&a),
                followed("seller.example", true),
                true,
            ),
            (
                "another host",
                challenge_for(&a),
                followed("elsewhere.example", true),
                false,
            ),
            (
                "the POST became a GET",
                challenge_for(&a),
                followed("seller.example", false),
                false,
            ),
            ("no option", challenge_for(&json!([])), None, false),
            ("only junk", challenge_for(&json!(["junk"])), None, false),
            (
                "no option we can read",
                challenge_for(&unreadable_options),
                None,
                false,
            ),
        ];
        for (name, challenge, redirect, verified) in cases {
            let registry = DiscoveryRegistry::new();
            registry
                .register(listing_of("/r", Some("POST")))
                .await
                .unwrap();
            let target = registry.probe_targets().await.remove(0);
            let health = registry.health();
            let transport = Answers {
                challenge,
                redirect,
            };
            probe_and_record(&transport, &registry, &health, target.clone()).await;
            let state = health.snapshot().await.remove(target.url.as_str()).unwrap();
            assert_eq!(state.status, HealthStatus::Alive, "{name}");
            assert_eq!(state.verified_by.is_some(), verified, "{name}");
            assert_eq!(
                registry.list(10, 0, None).await.pagination.total,
                u32::from(verified),
                "{name}"
            );
        }
    }

    /// What a challenge has to hold to be read at all, and to verify: a list
    /// with no option in it is not a challenge, and one whose options none
    /// normalize is read -- its recipients still face the drift check -- but
    /// offers nothing to pay.
    #[test]
    fn a_challenge_offers_a_payment_only_with_an_option_we_read() {
        for body in [
            r#"{"x402Version":2,"accepts":[]}"#,
            r#"{"accepts":["junk"]}"#,
            r#"{"paymentRequirements":[1,null,"x"]}"#,
        ] {
            let live = pay_to_from_402(Some(body), None);
            assert!(!live.readable, "{body}");
            assert!(!live.offers_payment(), "{body}");
        }
        let live = pay_to_from_402(Some(r#"{"accepts":[{"payTo":"0xAAAA"}]}"#), None);
        assert!(
            live.readable,
            "a recipient is still read for the drift check"
        );
        assert_eq!(live.pay_to, vec!["0xaaaa".to_string()]);
        assert!(!live.offers_payment());
        let live = pay_to_from_402(Some(r#"{"payTo":"0xBBBB"}"#), None);
        assert!(live.readable);
        assert!(!live.offers_payment());
        assert!(pay_to_from_402(None, Some(&challenge_for(&accepts()))).offers_payment());
    }

    /// When both transports carry a challenge, the one offering an option we
    /// can read wins, whatever the version or the default: a header we cannot
    /// read never hides the payable offer in the body.
    #[test]
    fn a_readable_offer_wins_over_one_that_offers_nothing() {
        let a = accepts();
        let unreadable =
            header_of(&json!({ "x402Version": 3, "accepts": [{ "payTo": a[0]["payTo"] }] }));
        let body = json!({ "x402Version": 2, "accepts": a }).to_string();
        let live = pay_to_from_402(Some(&body), Some(&unreadable));
        assert!(live.offers_payment());
        assert_eq!(live.transport, Some(TermsTransport::Body));
        let live = pay_to_from_402(
            Some(r#"{"accepts":[{"payTo":"0xAAAA"}]}"#),
            Some(&challenge_for(&a)),
        );
        assert!(live.offers_payment());
        assert_eq!(live.transport, Some(TermsTransport::Header));
    }

    /// The fallback's extra request replaces the first answer only with the
    /// listing's own 402: a POST that a redirect took to another host leaves
    /// the GET's 405 standing, while the same POST answered on the listing's
    /// host replaces it.
    #[tokio::test]
    async fn the_fallback_answer_counts_only_as_the_listings_own() {
        struct Fallback {
            challenge: String,
            redirect: Option<crate::discovery_security::FollowedRedirect>,
        }

        #[async_trait::async_trait]
        impl ProbeTransport for Fallback {
            async fn send(
                &self,
                _url: &url::Url,
                method: ProbeMethod,
                _body: Option<&str>,
            ) -> Result<reqwest::Response, SecurityReject> {
                let mut response = if method == ProbeMethod::Get {
                    axum::http::Response::builder()
                        .status(405)
                        .body(String::new())
                        .unwrap()
                } else {
                    axum::http::Response::builder()
                        .status(402)
                        .header("payment-required", self.challenge.as_str())
                        .body("{}".to_string())
                        .unwrap()
                };
                if method != ProbeMethod::Get {
                    if let Some(redirect) = &self.redirect {
                        response.extensions_mut().insert(redirect.clone());
                    }
                }
                Ok(reqwest::Response::from(response))
            }

            async fn send_mcp(
                &self,
                _url: &url::Url,
                _body: &str,
                _session: Option<&str>,
            ) -> Result<reqwest::Response, SecurityReject> {
                Err(SecurityReject::Http("no MCP here".to_string()))
            }
        }

        let elsewhere = crate::discovery_security::FollowedRedirect {
            host: Some("elsewhere.example".to_string()),
            method_kept: true,
        };
        let a = accepts();
        let payable = challenge_for(&a);
        // Read, recipient and all, but with no option to pay.
        let unpayable = challenge_for(&json!([{ "payTo": a[0]["payTo"] }]));
        for (name, challenge, redirect, status, verified) in [
            (
                "another host",
                payable.clone(),
                Some(elsewhere),
                HealthStatus::AuthGated,
                false,
            ),
            (
                "nothing to pay",
                unpayable,
                None,
                HealthStatus::AuthGated,
                false,
            ),
            ("its own 402", payable, None, HealthStatus::Alive, true),
        ] {
            let registry = DiscoveryRegistry::new();
            registry.register(listing_of("/r", None)).await.unwrap();
            let target = registry.probe_targets().await.remove(0);
            let health = registry.health();
            let transport = Fallback {
                challenge,
                redirect,
            };
            probe_and_record(&transport, &registry, &health, target.clone()).await;
            let state = health.snapshot().await.remove(target.url.as_str()).unwrap();
            assert_eq!(state.status, status, "{name}");
            assert_eq!(state.verified_by.is_some(), verified, "{name}");
        }
    }

    /// A record from before `verifiedAt` existed is exposed on its probe's
    /// reading of the terms only when that reading offered something to pay.
    #[tokio::test]
    async fn a_legacy_record_needs_a_reading_with_something_to_pay() {
        for (name, payable, shown) in [("nothing to pay", false, 0), ("an offer", true, 1)] {
            let registry = DiscoveryRegistry::new();
            let r = listing_of("/legacy", Some("GET"));
            let url = r.url.to_string();
            let offer = r.accepts.clone();
            registry.register(r).await.unwrap();
            let health = registry.health();
            health
                .record_probe(&url, ProbeClass::Alive, Some(402), 1, None)
                .await;
            let checked = health.snapshot().await[&url].last_checked.unwrap();
            let reading = ObservedTerms {
                accepts: if payable { offer } else { Vec::new() },
                observed_at: checked,
                context: ObservationContext::anonymous_get("http"),
                phase: ObservationPhase::Verification,
                provenance: TermsProvenance::OriginResponse,
                transport: TermsTransport::Header,
                x402_version: Some(2),
                http_status: Some(402),
                content_hash: None,
                conflict: None,
                rejected: BTreeMap::new(),
                truncated: false,
            };
            registry.terms().record(&url, reading).await;
            assert_eq!(
                registry.list(10, 0, None).await.pagination.total,
                shown,
                "{name}"
            );
        }
    }

    /// The MCP handshake verifies an endpoint only on its own answers: an
    /// `initialize` or a `tools/list` answered by another host, or reached by
    /// turning the POST into a GET, verifies nothing.
    #[tokio::test]
    async fn an_mcp_handshake_verifies_only_the_endpoints_own_answers() {
        use crate::discovery_security::FollowedRedirect;

        struct Handshake {
            initialize: Option<FollowedRedirect>,
            tools_list: Option<FollowedRedirect>,
        }

        #[async_trait::async_trait]
        impl ProbeTransport for Handshake {
            async fn send(
                &self,
                _url: &url::Url,
                _method: ProbeMethod,
                _body: Option<&str>,
            ) -> Result<reqwest::Response, SecurityReject> {
                Err(SecurityReject::Http("no HTTP probe here".to_string()))
            }

            async fn send_mcp(
                &self,
                _url: &url::Url,
                body: &str,
                _session: Option<&str>,
            ) -> Result<reqwest::Response, SecurityReject> {
                let (text, redirect) = if body == MCP_TOOLS_LIST {
                    (
                        r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"search"}]}}"#,
                        self.tools_list.clone(),
                    )
                } else if body == MCP_INITIALIZE {
                    (
                        r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}"#,
                        self.initialize.clone(),
                    )
                } else {
                    ("", None)
                };
                let mut response = axum::http::Response::builder()
                    .status(200)
                    .header("content-type", "application/json")
                    .body(text)
                    .unwrap();
                if let Some(redirect) = redirect {
                    response.extensions_mut().insert(redirect);
                }
                Ok(reqwest::Response::from(response))
            }
        }

        let url = "https://mcp.seller.example/mcp";
        let followed = |host: &str, method_kept: bool| {
            Some(FollowedRedirect {
                host: Some(host.to_string()),
                method_kept,
            })
        };
        let cases = [
            ("as sent", None, None, true),
            (
                "same host, same method",
                followed("mcp.seller.example", true),
                None,
                true,
            ),
            (
                "initialize from another host",
                followed("elsewhere.example", true),
                None,
                false,
            ),
            (
                "tools/list from another host",
                None,
                followed("elsewhere.example", true),
                false,
            ),
            (
                "initialize as a GET",
                followed("mcp.seller.example", false),
                None,
                false,
            ),
        ];
        for (name, initialize, tools_list, verified) in cases {
            let registry = DiscoveryRegistry::new();
            let health = registry.health();
            let target = ProbeTarget {
                url: url::Url::parse(url).unwrap(),
                resource_type: "mcp".to_string(),
                pay_to: vec![],
                request: ProbeRequest::Undeclared,
            };
            let transport = Handshake {
                initialize,
                tools_list,
            };
            probe_and_record(&transport, &registry, &health, target).await;
            let state = health.snapshot().await.remove(url).unwrap();
            assert_eq!(
                state.verified_by == Some(VerifiedBy::McpHandshake),
                verified,
                "{name}"
            );
        }
    }

    #[test]
    fn verified_alive_is_the_declared_request_inside_the_window() {
        let now = 10_000_000;
        let window = 600;
        let state = |status, verified_at: Option<u64>, method: Option<&str>| HealthState {
            status,
            last_checked: Some(now),
            http_status: Some(402),
            latency_ms: None,
            uptime_bps: None,
            probe_count: None,
            probe_method: method.map(str::to_string),
            quarantine_reason: None,
            verified_at,
            verified_by: verified_at.map(|_| VerifiedBy::X402Challenge),
        };
        let alive = HealthStatus::Alive;
        let ok =
            |s: &HealthState, r: &ProbeRequest| is_verified_alive(s, r, false, None, now, window);

        assert!(ok(&state(alive, Some(now - window), Some("GET")), &get()));
        assert!(
            !ok(&state(alive, Some(now - window - 1), Some("GET")), &get()),
            "past the window"
        );
        assert!(
            !ok(&state(alive, None, Some("GET")), &get()),
            "never verified"
        );
        for status in [
            HealthStatus::AuthGated,
            HealthStatus::Degraded,
            HealthStatus::Quarantined,
            HealthStatus::Unprobeable,
            HealthStatus::Unknown,
        ] {
            assert!(
                !ok(&state(status, Some(now), Some("GET")), &get()),
                "{status:?}"
            );
        }
        assert!(
            !ok(&state(alive, Some(now), Some("GET")), &post(None)),
            "a GET answer to a POST listing"
        );
        assert!(ok(&state(alive, Some(now), Some("POST")), &post(None)));
        assert!(
            ok(
                &state(alive, Some(now), Some("POST")),
                &ProbeRequest::Undeclared
            ),
            "a listing that declares nothing counts what the fallback found"
        );
    }

    /// A record from before `verifiedAt` existed is exposed on the overlay
    /// reading of the SAME probe, and on nothing else -- and is probed again
    /// at once, so that lasts one probe.
    #[tokio::test]
    async fn a_legacy_alive_record_keeps_its_listing_only_on_the_same_probes_reading() {
        let url = "https://seller.example/legacy";
        let t = HealthTracker::new();
        t.record_probe(url, ProbeClass::Alive, Some(402), 1, None)
            .await;
        let state = t.snapshot().await.remove(url).unwrap();
        let checked = state.last_checked.unwrap();
        let window = 600;
        let now = checked + 10;

        assert_eq!(
            legacy_verified_at(&state, Some(checked + 3)),
            Some(checked + 3)
        );
        assert_eq!(
            legacy_verified_at(&state, Some(checked - 3)),
            Some(checked - 3)
        );
        assert_eq!(
            legacy_verified_at(&state, Some(checked + LEGACY_OBSERVATION_SLACK_SECS + 1)),
            None,
            "another probe's reading"
        );
        assert_eq!(
            legacy_verified_at(&state, None),
            None,
            "no readable challenge"
        );
        assert!(is_verified_alive(
            &state,
            &get(),
            false,
            Some(checked),
            now,
            window
        ));
        assert!(!is_verified_alive(&state, &get(), false, None, now, window));
        assert!(
            !is_verified_alive(&state, &post(None), false, Some(checked), now, window),
            "and a GET reading still does not verify a POST listing"
        );
        let other = |f: &dyn Fn(&mut HealthState)| {
            let mut s = state.clone();
            f(&mut s);
            legacy_verified_at(&s, Some(checked))
        };
        assert_eq!(
            other(&|s| s.probe_method = Some("GET".into())),
            None,
            "this build's"
        );
        assert_eq!(other(&|s| s.status = HealthStatus::AuthGated), None);
        assert_eq!(other(&|s| s.http_status = Some(200)), None);

        // Due now. An MCP endpoint that only an `initialize`-only build probed
        // is too: it carries no tool count, and its handshake decides now.
        let http = target(url, get());
        let mut mcp = target(url, get());
        mcp.resource_type = "mcp".to_string();
        assert!(tracker_due(&t, &http, checked + 1));
        assert!(tracker_due(&t, &mcp, checked + 1));
        t.mark_verified(url, ProbeMethod::Get).await;
        assert!(
            !tracker_due(&t, &http, now_secs()),
            "and once probed, it is not"
        );
    }

    // ------------------------------------------------------------------------
    // MCP endpoints: verified by their handshake (owner's rule, 2026-10-02)
    // ------------------------------------------------------------------------

    /// A local MCP server (Streamable HTTP). `initialize` answers a JSON-RPC
    /// result -- or `init`, when set -- and, with `session`, assigns one that
    /// `tools/list` then requires; `tools/list` lists `tools` tools. Answers are
    /// JSON, or an event stream with `sse`.
    #[derive(Clone, Default)]
    struct McpServer {
        tools: usize,
        /// Tools listed as objects with no `name`.
        nameless: bool,
        sse: bool,
        session: bool,
        init: Option<(u16, &'static str)>,
        /// Every request the server saw.
        seen: Arc<Mutex<Vec<McpSeen>>>,
    }

    /// (JSON-RPC method, session header, accept header) of one request.
    type McpSeen = (String, Option<String>, Option<String>);

    async fn mcp_answer(State(s): State<McpServer>, headers: HeaderMap, body: Bytes) -> Response {
        let msg: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let method = msg["method"].as_str().unwrap_or_default().to_string();
        let header = |k: &str| {
            headers
                .get(k)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let session = header("mcp-session-id");
        s.seen
            .lock()
            .unwrap()
            .push((method.clone(), session.clone(), header("accept")));
        let reply = |v: Value| -> Response {
            if s.sse {
                (
                    [("content-type", "text/event-stream")],
                    format!("event: message\ndata: {v}\n\n"),
                )
                    .into_response()
            } else {
                axum::Json(v).into_response()
            }
        };
        match method.as_str() {
            "initialize" => {
                if let Some((code, raw)) = s.init {
                    return (StatusCode::from_u16(code).unwrap(), raw).into_response();
                }
                let mut r = reply(json!({
                    "jsonrpc": "2.0", "id": msg["id"],
                    "result": {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}},
                               "serverInfo": {"name": "local", "version": "1"}}
                }));
                if s.session {
                    r.headers_mut().insert(
                        "mcp-session-id",
                        axum::http::HeaderValue::from_static("s-123"),
                    );
                }
                r
            }
            "notifications/initialized" => StatusCode::ACCEPTED.into_response(),
            "tools/list" if !s.session || session.as_deref() == Some("s-123") => {
                let tools: Vec<Value> = (0..s.tools)
                    .map(|i| {
                        if s.nameless {
                            json!({"inputSchema": {"type": "object"}})
                        } else {
                            json!({"name": format!("tool{i}"), "inputSchema": {"type": "object"}})
                        }
                    })
                    .collect();
                reply(json!({"jsonrpc": "2.0", "id": msg["id"], "result": {"tools": tools}}))
            }
            _ => (StatusCode::BAD_REQUEST, "no session").into_response(),
        }
    }

    async fn serve_mcp(server: McpServer) -> String {
        let app = axum::Router::new().fallback(mcp_answer).with_state(server);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    const MCP_URL: &str = "https://mcp.seller.example/mcp";

    /// A registry holding one MCP listing, probed once against `server`.
    async fn probed_mcp(server: &McpServer) -> DiscoveryRegistry {
        let base = serve_mcp(server.clone()).await;
        let registry = DiscoveryRegistry::new();
        let mut listing = listing_of("/mcp", None);
        listing.url = url::Url::parse(MCP_URL).unwrap();
        listing.resource_type = "mcp".to_string();
        registry.register(listing).await.unwrap();
        let health = registry.health();
        for t in registry.probe_targets().await {
            probe_and_record(&Loopback::serving(&base), &registry, &health, t).await;
        }
        registry
    }

    /// The owner's rule, end to end: an MCP endpoint whose handshake lists at
    /// least one tool is verified alive -- exposed, counted, and marked as
    /// verified by its handshake, not by a 402 -- in either answer format, with
    /// the session it assigned sent back and `accept` naming both.
    #[tokio::test]
    async fn an_mcp_server_that_lists_a_tool_is_verified_alive_by_its_handshake() {
        for sse in [false, true] {
            let server = McpServer {
                tools: 2,
                sse,
                session: true,
                ..McpServer::default()
            };
            let registry = probed_mcp(&server).await;
            assert_eq!(walk(&registry).await, [MCP_URL], "sse={sse}");
            let stats = registry.stats().await;
            assert_eq!(stats["verifiedAlive"], 1, "sse={sse}");
            let item = serde_json::to_value(&registry.list(10, 0, None).await.items[0]).unwrap();
            assert_eq!(item["health"]["verifiedBy"], "mcp_handshake", "sse={sse}");
            assert!(item["health"]["verifiedAt"].is_u64());

            let seen = server.seen.lock().unwrap().clone();
            let methods: Vec<&str> = seen.iter().map(|(m, _, _)| m.as_str()).collect();
            assert_eq!(
                methods,
                ["initialize", "notifications/initialized", "tools/list"]
            );
            assert_eq!(seen[0].1, None, "no session before the server assigns one");
            assert_eq!(seen[2].1.as_deref(), Some("s-123"), "its own session, back");
            for (_, _, accept) in &seen {
                let accept = accept.as_deref().unwrap_or_default();
                assert!(
                    accept.contains("application/json") && accept.contains("text/event-stream"),
                    "{accept}"
                );
            }
        }
    }

    /// The other path: a handshake that lists no tool, or does not complete,
    /// verifies nothing. The endpoint may still be alive -- the vocabulary does
    /// not change -- but it stays in the pending queue, out of every public
    /// surface; and a server that stops listing tools loses its exposure on
    /// that very probe.
    #[tokio::test]
    async fn an_mcp_handshake_that_lists_no_tool_or_breaks_stays_pending() {
        let cases = [
            ("no tool", McpServer::default(), HealthStatus::Alive),
            (
                "tools without a name",
                McpServer {
                    tools: 3,
                    nameless: true,
                    ..McpServer::default()
                },
                HealthStatus::Alive,
            ),
            (
                "not JSON-RPC",
                McpServer {
                    tools: 3,
                    init: Some((200, "hello")),
                    ..McpServer::default()
                },
                HealthStatus::Alive,
            ),
            (
                "a JSON-RPC error",
                McpServer {
                    tools: 3,
                    init: Some((200, r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600}}"#)),
                    ..McpServer::default()
                },
                HealthStatus::Alive,
            ),
            (
                "auth at the door",
                McpServer {
                    tools: 3,
                    init: Some((401, "")),
                    ..McpServer::default()
                },
                HealthStatus::AuthGated,
            ),
        ];
        for (name, server, status) in cases {
            let registry = probed_mcp(&server).await;
            let state = registry.health().snapshot().await[MCP_URL].clone();
            assert_eq!(state.status, status, "{name}");
            assert_eq!(state.verified_at, None, "{name}");
            assert_eq!(state.verified_by, None, "{name}");
            assert!(walk(&registry).await.is_empty(), "{name}");
            assert_eq!(registry.stats().await["verifiedAlive"], 0, "{name}");
            assert_eq!(
                registry.list_pending(10, 0).await.pagination.total,
                1,
                "{name}: in the queue"
            );
            let target = &registry.probe_targets().await[0];
            let rec = registry.health().records.read().await[MCP_URL].clone();
            assert!(
                !unverified_legacy_alive(&rec, target),
                "{name}: this build's handshake ran, so it is not probed again at once"
            );
        }

        // Verified, then the server stops listing tools: the next probe hides it.
        let registry = probed_mcp(&McpServer {
            tools: 1,
            ..McpServer::default()
        })
        .await;
        assert_eq!(walk(&registry).await, [MCP_URL]);
        let base = serve_mcp(McpServer::default()).await;
        let target = registry.probe_targets().await.remove(0);
        probe_and_record(
            &Loopback::serving(&base),
            &registry,
            &registry.health(),
            target,
        )
        .await;
        assert!(walk(&registry).await.is_empty());
    }

    /// The overlay is read record by record: one this build cannot read is left
    /// out, never the whole overlay with it.
    #[test]
    fn an_unreadable_overlay_record_costs_only_itself() {
        let body = json!({
            "https://a.example/x": {"status": "alive", "http_status": 402},
            "https://b.example/x": {"status": "some_future_status"},
        });
        let records = parse_overlay(body.to_string().as_bytes()).unwrap();
        assert_eq!(records.len(), 1);
        assert!(records.contains_key("https://a.example/x"));
        assert!(parse_overlay(b"[1, 2]").is_err(), "not records at all");
    }

    /// A JSON-RPC answer is the `result` object of the message with the id
    /// asked, from one JSON message or from an event stream; an error, another
    /// id or a non-object result is no answer.
    #[test]
    fn a_jsonrpc_answer_is_read_by_its_id_from_json_or_an_event_stream() {
        let ok = r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[]}}"#;
        assert!(jsonrpc_result(ok, Some("application/json"), 2).is_some());
        assert!(jsonrpc_result(ok, None, 1).is_none(), "another id");
        assert!(
            jsonrpc_result(
                r#"{"jsonrpc":"2.0","id":2,"error":{"code":-1},"result":{}}"#,
                None,
                2
            )
            .is_none(),
            "an error is not a result"
        );
        assert!(
            jsonrpc_result(r#"{"jsonrpc":"2.0","id":2,"result":[]}"#, None, 2).is_none(),
            "a result is an object"
        );
        let stream = "event: message\r\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\r\n\r\ndata:{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\r\n\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[{}]}}\r\n\r\n";
        let answer = jsonrpc_result(stream, Some("text/event-stream; charset=utf-8"), 2).unwrap();
        assert_eq!(answer["tools"].as_array().unwrap().len(), 1);
        assert!(
            jsonrpc_result(stream, Some("application/json"), 2).is_none(),
            "a stream is read as one only when it says it is one"
        );
    }

    /// Neither kind of evidence stands in for the other.
    #[test]
    fn a_challenge_does_not_verify_an_mcp_endpoint_nor_a_handshake_an_http_listing() {
        let now = 10_000_000;
        let window = 600;
        let state = |by: VerifiedBy| HealthState {
            status: HealthStatus::Alive,
            last_checked: Some(now),
            http_status: Some(200),
            latency_ms: None,
            uptime_bps: None,
            probe_count: None,
            probe_method: None,
            quarantine_reason: None,
            verified_at: Some(now),
            verified_by: Some(by),
        };
        let undeclared = ProbeRequest::Undeclared;
        let ok =
            |s: &HealthState, mcp: bool| is_verified_alive(s, &undeclared, mcp, None, now, window);
        assert!(ok(&state(VerifiedBy::McpHandshake), true));
        assert!(!ok(&state(VerifiedBy::X402Challenge), true));
        assert!(ok(&state(VerifiedBy::X402Challenge), false));
        assert!(!ok(&state(VerifiedBy::McpHandshake), false));
        let mut stale = state(VerifiedBy::McpHandshake);
        stale.verified_at = Some(now - window - 1);
        assert!(!ok(&stale, true), "the same window");
        let mut held = state(VerifiedBy::McpHandshake);
        held.status = HealthStatus::Quarantined;
        assert!(!ok(&held, true), "and never in quarantine");
    }

    /// The re-probe of a verified listing lands inside the window it is
    /// exposed on, under the window in force NOW.
    #[tokio::test]
    async fn a_verified_listing_is_reprobed_before_its_verification_expires() {
        let window = crate::discovery_terms::freshness_window_secs();
        assert!(alive_reprobe_secs() < window);
        assert!(alive_reprobe_secs() <= HEALTHY_REPROBE_SECS);

        let url = "https://seller.example/verified";
        let t = HealthTracker::new();
        t.mark_verified(url, ProbeMethod::Get).await;
        let verified_at = t.snapshot().await[url].verified_at.unwrap();
        let rec = t.records.read().await[url].clone();
        assert!(
            rec.next_probe_at + window / 8 <= verified_at + window,
            "with an eighth of the window to spare"
        );
        let target = target(url, get());
        assert!(!tracker_due(&t, &target, verified_at + 1));
        // Scheduled under a longer window, then the window shortens: due by
        // the verification's age, not by the old schedule.
        t.records.write().await.get_mut(url).unwrap().next_probe_at = u64::MAX;
        assert!(tracker_due(&t, &target, verified_at + alive_reprobe_secs()));
    }

    /// The attestation speaks only for what is exposed.
    #[tokio::test]
    async fn the_attested_uptime_counts_only_verified_listings() {
        let t = HealthTracker::new();
        let now = now_secs();
        let window = 600;
        assert_eq!(
            t.uptime_prefix_verified("https://p.example/", now, window)
                .await,
            None
        );
        t.mark_verified("https://p.example/a", ProbeMethod::Get)
            .await;
        // Alive by the old rule (an unreadable 402): not verified.
        t.record_probe(
            "https://p.example/b",
            ProbeClass::Alive,
            Some(402),
            1,
            Some(ProbeMethod::Get),
        )
        .await;
        t.record_probe(
            "https://p.example/b",
            ProbeClass::Alive,
            Some(402),
            1,
            Some(ProbeMethod::Get),
        )
        .await;
        assert_eq!(
            t.uptime_prefix_verified("https://p.example/", now_secs(), window)
                .await,
            Some((10_000, 1, 1))
        );
        assert_eq!(
            t.uptime_prefix_verified("https://other.example/", now_secs(), window)
                .await,
            None
        );
    }
}

/// The declared offer that lets an extra network pass is compared as a client
/// matches it: the exact CAIP-2 network, the asset and recipient as written
/// outside EVM, a scheme the protocol takes literally. Anything looser lets a
/// decoy vouch for the attacker's option (REF-X4114, P1-1..3).
#[cfg(test)]
mod strict_offer_identity_tests {
    use super::*;

    const BASE: &str = "eip155:8453";
    const MAINNET: &str = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp";
    const DEVNET: &str = "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1";
    const A: &str = "0x1111111111111111111111111111111111111111";
    const B: &str = "0x2222222222222222222222222222222222222222";
    const USDC: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
    const MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    const SOL: &str = "GEhr9HCFTRDjanMg435frSgCVwVZYpNoPrEkmNBnFHFE";

    fn option(network: &str, asset: &str, recipient: &str) -> serde_json::Value {
        serde_json::json!({"scheme": "exact", "network": network, "asset": asset,
            "payTo": recipient, "amount": "1", "maxTimeoutSeconds": 60})
    }

    fn baseline(options: &[serde_json::Value]) -> Vec<DeclaredOffer> {
        options
            .iter()
            .map(|value| {
                let wire = serde_json::from_value::<DeclaredPaymentOption>(value.clone()).unwrap();
                DeclaredOffer::of(&normalize_declared_option(wire).unwrap())
            })
            .collect()
    }

    fn challenge(options: &[serde_json::Value]) -> LiveTerms {
        let doc = serde_json::json!({"x402Version": 2, "accepts": options});
        pay_to_from_402(Some(&doc.to_string()), None)
    }

    #[test]
    fn another_solana_network_is_not_the_declared_offer() {
        // P1-1: mainnet and devnet are one family for the drift key and two
        // offers for a client.
        let declared = baseline(&[option(MAINNET, MINT, SOL)]);
        let live = challenge(&[option(DEVNET, MINT, SOL), option(BASE, USDC, B)]);
        assert_eq!(
            compare_recipients(&declared, &live),
            Recipients::Drifted,
            "mainnet was removed; a devnet decoy must not vouch for the attacker on Base"
        );
    }

    #[test]
    fn a_different_case_sensitive_mint_is_not_the_declared_asset() {
        // P1-2: two valid, distinct base58 mints that only differ in case.
        let other_mint = MINT.replacen('j', "J", 1);
        let before: crate::types::MixedAddress =
            serde_json::from_value(serde_json::json!(MINT)).unwrap();
        let after: crate::types::MixedAddress =
            serde_json::from_value(serde_json::json!(other_mint)).unwrap();
        assert_ne!(before, after, "distinct valid base58 addresses");
        let declared = baseline(&[option(MAINNET, MINT, SOL)]);
        let live = challenge(&[option(MAINNET, &other_mint, SOL), option(BASE, USDC, B)]);
        assert_eq!(
            compare_recipients(&declared, &live),
            Recipients::Drifted,
            "USDC was removed; another mint must not vouch for the attacker on Base"
        );
    }

    #[test]
    fn a_solana_recipient_in_another_case_is_not_the_declared_one() {
        let other = SOL.replacen('G', "g", 1);
        let declared = baseline(&[option(MAINNET, MINT, SOL)]);
        let live = challenge(&[option(MAINNET, MINT, &other), option(BASE, USDC, B)]);
        assert_eq!(compare_recipients(&declared, &live), Recipients::Drifted);
        // Beside the declared Base offer, the other-case recipient is a new
        // recipient on a declared network, not the declared one.
        let declared = baseline(&[option(BASE, USDC, A), option(MAINNET, MINT, SOL)]);
        let live = challenge(&[option(BASE, USDC, A), option(MAINNET, MINT, &other)]);
        assert_eq!(compare_recipients(&declared, &live), Recipients::Drifted);
    }

    #[test]
    fn a_declared_recipient_in_another_offer_does_not_complete_this_one() {
        // Two declared Solana offers whose recipients differ only in case: the
        // live option pays the second recipient in the first offer's mint,
        // which is neither declared offer.
        let other = SOL.replacen("hr", "hR", 1);
        let before: crate::types::MixedAddress =
            serde_json::from_value(serde_json::json!(SOL)).unwrap();
        let after: crate::types::MixedAddress =
            serde_json::from_value(serde_json::json!(other)).unwrap();
        assert_ne!(before, after, "distinct valid base58 addresses");
        let usdt = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";
        let declared = baseline(&[option(MAINNET, MINT, SOL), option(MAINNET, usdt, &other)]);
        let live = challenge(&[option(MAINNET, MINT, &other), option(BASE, USDC, B)]);
        assert_eq!(compare_recipients(&declared, &live), Recipients::Drifted);
    }

    #[test]
    fn a_declared_scheme_no_client_takes_vouches_for_nothing() {
        // Equal to the declaration is not enough: the live option must also be
        // one the protocol's `Scheme` takes as written.
        let declared = vec![DeclaredOffer::new("Exact", BASE, USDC, A)];
        let mut decoy = option(BASE, USDC, A);
        decoy["scheme"] = serde_json::json!("Exact");
        let live = challenge(&[decoy, option(MAINNET, MINT, SOL)]);
        assert!(pay_to_drifted(&declared, &live));
    }

    #[test]
    fn literal_scheme_rejected_by_clients_is_not_a_payable_offer() {
        // P1-3: the catalog normalizes `EXACT`; a client does not take it.
        let declared = baseline(&[option(BASE, USDC, A)]);
        let mut accepted = Vec::new();
        for scheme in ["EXACT", " exact ", "Exact", "exact "] {
            assert!(
                serde_json::from_value::<crate::types::Scheme>(serde_json::json!(scheme)).is_err()
            );
            let mut decoy = option(BASE, USDC, A);
            decoy["scheme"] = serde_json::json!(scheme);
            let live = challenge(&[decoy, option(MAINNET, MINT, SOL)]);
            if !pay_to_drifted(&declared, &live) {
                accepted.push(scheme);
            }
        }
        assert!(
            accepted.is_empty(),
            "client-rejected schemes wrongly vouch for attacker: {accepted:?}"
        );
    }

    #[test]
    fn an_evm_address_a_client_cannot_read_is_not_the_declared_offer() {
        let declared = baseline(&[option(BASE, USDC, A)]);
        let upper_prefix = A.replacen("0x", "0X", 1);
        for (asset, pay_to) in [
            (USDC.replacen("0x", "0X", 1), A.to_string()),
            (USDC.to_string(), upper_prefix),
            (format!(" {USDC}"), A.to_string()),
        ] {
            let live = challenge(&[option(BASE, &asset, &pay_to), option(MAINNET, MINT, SOL)]);
            assert!(
                pay_to_drifted(&declared, &live),
                "{asset} / {pay_to} must not vouch for the Solana option"
            );
        }
    }

    #[test]
    fn an_evm_address_in_another_case_is_still_the_declared_offer() {
        // EVM case is a checksum, not the address: the legitimate equivalence stays.
        let declared = baseline(&[option(BASE, &USDC.to_ascii_lowercase(), A)]);
        let checksummed_recipient = "0xd5Ba9711a3D052846a3695C70e7fcb8b3168FE7d";
        let declared_mixed = baseline(&[option(BASE, USDC, checksummed_recipient)]);
        let live = challenge(&[option(BASE, USDC, A), option(MAINNET, MINT, SOL)]);
        assert_eq!(
            compare_recipients(&declared, &live),
            Recipients::ExtraNetworks(vec![DeclaredRecipient::new(MAINNET, SOL)])
        );
        let live = challenge(&[
            option(
                BASE,
                &USDC.to_ascii_uppercase().replacen("0X", "0x", 1),
                &checksummed_recipient.to_ascii_lowercase(),
            ),
            option(MAINNET, MINT, SOL),
        ]);
        assert!(!pay_to_drifted(&declared_mixed, &live));
    }

    #[test]
    fn the_actual_offer_beside_an_extra_network_is_accepted() {
        let declared = baseline(&[option(BASE, USDC, A)]);
        let live = challenge(&[option(BASE, USDC, A), option(MAINNET, MINT, SOL)]);
        assert!(!pay_to_drifted(&declared, &live));
        let declared = baseline(&[option(MAINNET, MINT, SOL)]);
        let live = challenge(&[option(MAINNET, MINT, SOL), option(BASE, USDC, B)]);
        assert_eq!(
            compare_recipients(&declared, &live),
            Recipients::ExtraNetworks(vec![DeclaredRecipient::new(BASE, B)])
        );
    }

    #[test]
    fn payment_requirements_decoy_does_not_vouch_for_accepts() {
        // P1-A: `accepts` and `paymentRequirements` are two lists a client
        // reads one at a time; the declared offer in one does not pay the
        // Solana option in the other.
        let declared = baseline(&[option(BASE, USDC, A)]);
        for version in [1, 2] {
            for (accepts, requirements) in [
                (option(MAINNET, MINT, SOL), option(BASE, USDC, A)),
                (option(BASE, USDC, A), option(MAINNET, MINT, SOL)),
            ] {
                let doc = serde_json::json!({"x402Version": version,
                    "accepts": [accepts], "paymentRequirements": [requirements]});
                for live in [
                    pay_to_from_402(Some(&doc.to_string()), None),
                    pay_to_from_402(None, Some(&doc.to_string())),
                ] {
                    assert_eq!(
                        compare_recipients(&declared, &live),
                        Recipients::Drifted,
                        "v{version}: {doc}"
                    );
                }
            }
        }
    }

    #[test]
    fn each_list_key_still_vouches_for_its_own_extra_option() {
        let declared = baseline(&[option(BASE, USDC, A)]);
        for key in ["accepts", "paymentRequirements"] {
            let doc = serde_json::json!({"x402Version": 1,
                key: [option(BASE, USDC, A), option(MAINNET, MINT, SOL)]});
            let live = pay_to_from_402(Some(&doc.to_string()), None);
            assert_eq!(
                compare_recipients(&declared, &live),
                Recipients::ExtraNetworks(vec![DeclaredRecipient::new(MAINNET, SOL)]),
                "{key}"
            );
        }
    }

    #[test]
    fn a_top_level_pay_to_is_judged_in_every_list() {
        // A v1 top-level `payTo` belongs to no list: a list that does not
        // carry the declared offer cannot vouch for it.
        let declared = baseline(&[option(BASE, USDC, A)]);
        for (accepts, requirements) in [
            (vec![option(BASE, USDC, A)], vec![]),
            (vec![], vec![option(BASE, USDC, A)]),
        ] {
            let doc = serde_json::json!({"x402Version": 1, "accepts": accepts,
                "paymentRequirements": requirements, "payTo": SOL, "network": MAINNET});
            let live = pay_to_from_402(Some(&doc.to_string()), None);
            assert_eq!(
                compare_recipients(&declared, &live),
                Recipients::Drifted,
                "{doc}"
            );
        }
        let doc = serde_json::json!({"x402Version": 1, "accepts": [option(BASE, USDC, A)],
            "payTo": SOL, "network": MAINNET});
        let live = pay_to_from_402(Some(&doc.to_string()), None);
        assert_eq!(
            compare_recipients(&declared, &live),
            Recipients::ExtraNetworks(vec![DeclaredRecipient::new(MAINNET, SOL)])
        );
    }

    /// Every `Network::from_str` spelling that the derived serde of `Network`
    /// refuses, with an asset and a recipient valid on that network.
    fn from_str_only_aliases() -> Vec<(&'static str, &'static str, &'static str)> {
        let evm = |alias| (alias, USDC, A);
        #[allow(unused_mut)]
        let mut aliases = vec![
            evm("base-mainnet"),
            evm("bnb"),
            evm("binance"),
            evm("skale"),
            evm("skale-testnet"),
            evm("scroll-mainnet"),
            evm("robinhood-mainnet"),
            evm("robinhood-chain"),
        ];
        #[cfg(feature = "hedera")]
        aliases.push(("hedera-mainnet", "0.0.456858", "0.0.12345"));
        #[cfg(feature = "xrpl")]
        aliases.push((
            "xrpl-mainnet",
            "rMxCKbEDwqr76QuheSUMdEGf4B9xJ8m5De",
            "rN7n7otQDd6FczFgLdSqtcsAUxDkw6fzRH",
        ));
        #[cfg(feature = "sui")]
        aliases.push((
            "sui-mainnet",
            "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC",
            "0x1111111111111111111111111111111111111111111111111111111111111111",
        ));
        aliases
    }

    #[test]
    fn a_from_str_alias_is_not_a_v1_wire_name() {
        // P1-B: a v1 name counts only as the derived serde of `Network`
        // reads it, which is what clients send and accept.
        let mut vouched = Vec::new();
        for (alias, asset, recipient) in from_str_only_aliases() {
            let network = alias.parse::<crate::network::Network>().unwrap();
            assert!(
                serde_json::from_value::<crate::network::Network>(serde_json::json!(alias))
                    .is_err(),
                "{alias} is a serde name"
            );
            let wire = serde_json::to_value(network).unwrap();
            let wire = wire.as_str().unwrap();
            let chain = network.to_caip2();
            let declared = baseline(&[option(&chain, asset, recipient)]);
            for (spelling, expect_drift) in [(alias, true), (wire, false), (chain.as_str(), false)]
            {
                let live = challenge(&[
                    option(spelling, asset, recipient),
                    option(MAINNET, MINT, SOL),
                ]);
                if pay_to_drifted(&declared, &live) != expect_drift {
                    vouched.push(format!("{spelling} (declared {chain})"));
                }
            }
        }
        assert!(vouched.is_empty(), "wrong verdict for: {vouched:?}");
    }

    #[cfg(feature = "algorand")]
    #[test]
    fn algorand_mainnet_cannot_be_declared_so_its_alias_cannot_vouch() {
        // The catalog has no CAIP-2 namespace for Algorand: no listing can
        // declare an offer there, so `algorand-mainnet` never reaches
        // `spelled_for_clients`.
        for spelling in ["algorand-mainnet", "algorand", "algorand:mainnet"] {
            assert!(crate::discovery_price::resolve_catalog_network(spelling).is_none());
        }
    }

    #[test]
    fn the_v1_wire_name_still_vouches_for_the_extra_option() {
        let declared = baseline(&[option(BASE, USDC, A)]);
        let live = challenge(&[option("base", USDC, A), option(MAINNET, MINT, SOL)]);
        assert_eq!(
            compare_recipients(&declared, &live),
            Recipients::ExtraNetworks(vec![DeclaredRecipient::new(MAINNET, SOL)])
        );
    }

    #[derive(Clone, Default)]
    struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn the_extra_option_is_logged_at_warn_with_its_network_and_recipient() {
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        // See rate_policy's log test: a second dispatcher keeps another
        // thread's cached `never` interest from silencing this capture.
        let _second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
        tracing::callsite::rebuild_interest_cache();

        let declared = baseline(&[option(BASE, USDC, A)]);
        let live = challenge(&[option(BASE, USDC, A), option(MAINNET, MINT, SOL)]);
        let Recipients::ExtraNetworks(extra) = compare_recipients(&declared, &live) else {
            panic!("expected an extra network");
        };
        log_extra_networks(&"https://seller.example/r", &extra);

        let out = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        let line = out
            .lines()
            .find(|l| l.contains("adds a payment option"))
            .unwrap_or_else(|| panic!("no extra-option event in {out:?}"));
        assert!(line.contains(" WARN "), "not at WARN: {line}");
        assert!(
            line.contains(&format!("solana {SOL}")),
            "network and recipient missing: {line}"
        );
        assert!(
            line.contains("https://seller.example/r"),
            "url missing: {line}"
        );
    }

    /// One challenge document as the base64 spellings a paying client reads:
    /// `Buffer.from(value, "base64")` on Node, `atob` in the browser. Each one
    /// was decoded by Node 22 to the same document (PR #114, X4114R3).
    fn client_readable_headers(options: &[serde_json::Value]) -> Vec<(&'static str, String)> {
        use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE};
        use base64::Engine as _;
        // `?>` gives the encoding a `/` and a `+`, so the URL-safe alphabet
        // differs from the standard one.
        let doc = serde_json::json!({"x402Version": 2, "accepts": options,
            "error": "??>>??>>"})
        .to_string();
        // Trailing whitespace is still the same JSON; it sets the length mod 3.
        let sized = |rem: usize| {
            let mut d = doc.clone();
            while d.len() % 3 != rem {
                d.push(' ');
            }
            d
        };
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let with_trailing_bits = |rem: usize, mask: usize| {
            let mut s = STANDARD_NO_PAD.encode(sized(rem));
            let last = s.pop().unwrap();
            let at = alphabet.iter().position(|c| *c as char == last).unwrap();
            s.push(alphabet[at | mask] as char);
            s
        };
        let standard = STANDARD.encode(sized(1));
        assert!(standard.ends_with("=="), "a padded spelling to cut");
        let url_safe = URL_SAFE.encode(sized(1));
        assert!(
            url_safe.contains('-') && url_safe.contains('_'),
            "the URL-safe spelling must differ"
        );
        let mixed = STANDARD.encode(sized(1)).replacen('+', "-", 1);
        assert!(mixed.contains('/'), "both alphabets in one value");
        let spaced: String = STANDARD
            .encode(sized(1))
            .chars()
            .enumerate()
            .flat_map(|(i, c)| {
                let gap = if i % 8 == 7 {
                    Some(if i % 16 == 7 { ' ' } else { '\t' })
                } else {
                    None
                };
                std::iter::once(c).chain(gap)
            })
            .collect();
        let junk: String = STANDARD
            .encode(sized(1))
            .chars()
            .enumerate()
            .flat_map(|(i, c)| std::iter::once(c).chain((i % 10 == 9).then_some('.')))
            .collect();
        let mut invalid_utf8 = sized(1).into_bytes();
        let at = invalid_utf8.windows(2).position(|w| w == b"??").unwrap();
        invalid_utf8[at] = 0xFF;
        vec![
            ("standard_no_pad", STANDARD_NO_PAD.encode(sized(1))),
            ("url_safe_padded", url_safe),
            ("mixed_alphabets", mixed),
            ("spaces_and_tabs", spaced),
            ("junk_skipped", junk),
            ("extra_padding", format!("{standard}==")),
            ("after_padding", format!("{standard}QUFB")),
            ("trailing_bits_rem1", with_trailing_bits(1, 0x0F)),
            ("trailing_bits_rem2", with_trailing_bits(2, 0x03)),
            ("dangling_symbol", format!("{}Q", STANDARD.encode(sized(0)))),
            ("invalid_utf8", STANDARD.encode(invalid_utf8)),
        ]
    }

    #[test]
    fn a_header_only_attacker_in_any_client_readable_base64_is_a_drift() {
        // P2-1 (VER2-X4114): the body keeps the declared offer, the header a
        // client actually pays pays the attacker, spelled in base64 the old
        // decoder (STANDARD or URL_SAFE_NO_PAD only) could not read.
        let declared = baseline(&[option(BASE, USDC, A)]);
        let body =
            serde_json::json!({"x402Version": 2, "accepts": [option(BASE, USDC, A)]}).to_string();
        for (spelling, header) in client_readable_headers(&[option(BASE, USDC, B)]) {
            let live = pay_to_from_402(Some(&body), Some(&header));
            assert_eq!(
                compare_recipients(&declared, &live),
                Recipients::Drifted,
                "{spelling}: {header:?}"
            );
        }
    }

    #[test]
    fn the_same_spellings_carrying_the_declared_offer_are_not_a_drift() {
        let declared = baseline(&[option(BASE, USDC, A)]);
        let body =
            serde_json::json!({"x402Version": 2, "accepts": [option(BASE, USDC, A)]}).to_string();
        for (spelling, header) in client_readable_headers(&[option(BASE, USDC, A)]) {
            let alone = pay_to_from_402(None, Some(&header));
            assert!(alone.readable, "{spelling}: the header must be read");
            assert_eq!(alone.pay_to, vec![A.to_string()], "{spelling}");
            let live = pay_to_from_402(Some(&body), Some(&header));
            assert_eq!(
                compare_recipients(&declared, &live),
                Recipients::Declared,
                "{spelling}: {header:?}"
            );
        }
    }
}
