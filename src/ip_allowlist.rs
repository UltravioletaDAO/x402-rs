//! Client addresses the rate policy treats like a stack identity.
//!
//! Between the stack's own services, and for the operator's own tests, there
//! is no rate limiting of policy (decision 144, 2026-10-02). Services prove who
//! they are with `X-UVD-Stack-Key` (`crate::rate_policy`). A person at a
//! terminal, or a tool that carries no key, cannot; but they call from a known
//! public address, and this list names it.
//!
//! The list is not configuration baked into the task. Such an address changes
//! when its provider says so, and it must never sit in a repository, a task
//! definition, an application log or a page. So it lives in one Secrets
//! Manager secret, named by `UVD_IP_ALLOWLIST_SECRET`, and every task re-reads
//! it every `UVD_IP_ALLOWLIST_REFRESH_SECS` ([`DEFAULT_REFRESH_SECS`]): a new
//! address takes effect within one refresh, with no deploy.
//!
//! An address on the list skips what a recognized key skips -- the per-IP
//! budgets and the per-address in-flight ceiling -- and nothing else. The body
//! deadline, the machine's ceiling and the ERC-8004 daily write cap hold for it
//! as for anybody: the exemption is from a quota, never from a protection.
//!
//! # Matching
//!
//! A request's address is the one every budget keys on
//! (`crate::client_ip::ClientIpKeyExtractor`): the entry the load balancer
//! appended last to `X-Forwarded-For`. Entries a client wrote in front of it
//! are never read, so naming a listed address in that header buys nothing.
//!
//! # The secret
//!
//! One secret for the whole stack (`uvd/allowlist/home` in production), which
//! the operator creates and loads by hand, outside Terraform; each service
//! reads it in its own region by name. Its form is a JSON array of strings;
//! entries separated by commas, semicolons or white space are read too. An entry
//! is an address or a CIDR prefix (an IPv6 address may rotate inside its
//! prefix). Refused, and logged by position and reason but never by value:
//!
//! - anything that is not an address or a prefix; and a document that starts
//!   like JSON and is not a JSON array -- an object, or JSON that does not
//!   parse -- which then counts as an empty list;
//! - a prefix broader than [`MIN_PREFIX_V4`] or [`MIN_PREFIX_V6`] bits: a slip
//!   there exempts strangers;
//! - an address that is not public: unspecified, loopback, private, shared
//!   (100.64.0.0/10), link-local, multicast, reserved, unique-local. Behind the
//!   load balancer a client address is always public, so a private entry could
//!   only ever match one of our own hops;
//! - every entry past [`MAX_ENTRIES`].
//!
//! # Failing closed
//!
//! Without `UVD_IP_ALLOWLIST_SECRET` the list is disabled and nothing calls
//! AWS. A secret that does not exist, or holds no value yet, is an empty list,
//! and so is a document that is not a list -- even over a list read before. A
//! read that fails (permissions, throttling, the network, no answer within the
//! read timeout) keeps the last list until [`EMPTIED_AFTER_FAILED_READS`] reads
//! in a row have failed, and then empties it: an address from that long ago may
//! belong to somebody else now. And whatever happens to the refresher, a list
//! that has not been read for one read longer than that counts as empty.

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

/// The name of the Secrets Manager secret that holds the list. Unset or empty
/// disables the list.
pub const ENV_SECRET: &str = "UVD_IP_ALLOWLIST_SECRET";

/// Seconds between two reads of the secret.
pub const ENV_REFRESH_SECS: &str = "UVD_IP_ALLOWLIST_REFRESH_SECS";

/// Five minutes: how long a new address waits to be honoured, for 288 reads a
/// day per task.
pub const DEFAULT_REFRESH_SECS: u64 = 300;
const MIN_REFRESH_SECS: u64 = 30;
const MAX_REFRESH_SECS: u64 = 3_600;

/// Failed reads in a row after which the list is emptied.
pub const EMPTIED_AFTER_FAILED_READS: u32 = 3;

/// A failing read is logged the first time, then once every this many, so a
/// missing permission is one line an hour and not one every refresh.
const FAILED_READ_LOG_EVERY: u32 = 12;

/// How long one read may take before it counts as failed.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// The broadest IPv4 prefix accepted.
pub const MIN_PREFIX_V4: u8 = 24;

/// The broadest IPv6 prefix accepted, the stack's rule for the shared secret.
pub const MIN_PREFIX_V6: u8 = 48;

/// The most entries kept. The list names a few addresses, not a network.
pub const MAX_ENTRIES: usize = 64;

/// What `x-ratelimit-exempt` says on a response the list exempted.
pub const EXEMPT_AS: &str = "ip-allowlist";

// ============================================================================
// Entries
// ============================================================================

/// One entry: a network and how many of its leading bits count.
///
/// Deliberately not `Debug`: nothing can print an address from the list by
/// formatting it.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Prefix {
    network: IpAddr,
    len: u8,
}

impl Prefix {
    /// Whether `ip`, already canonical, falls inside this prefix.
    fn contains(&self, ip: IpAddr) -> bool {
        self.network.is_ipv4() == ip.is_ipv4() && masked(ip, self.len) == self.network
    }
}

/// `ip` with every bit past the first `len` cleared.
fn masked(ip: IpAddr, len: u8) -> IpAddr {
    match ip {
        IpAddr::V4(a) => {
            let mask = u32::MAX.checked_shl(32 - u32::from(len)).unwrap_or(0);
            IpAddr::V4(Ipv4Addr::from(u32::from(a) & mask))
        }
        IpAddr::V6(a) => {
            let mask = u128::MAX.checked_shl(128 - u32::from(len)).unwrap_or(0);
            IpAddr::V6(Ipv6Addr::from(u128::from(a) & mask))
        }
    }
}

/// Why an entry was refused. Logged by name, with the entry's position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    NotAnAddress,
    TooBroad,
    NotPublic,
    TooMany,
}

impl Refusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Refusal::NotAnAddress => "not an address or a CIDR prefix",
            Refusal::TooBroad => "prefix broader than /24 (IPv4) or /48 (IPv6)",
            Refusal::NotPublic => "not a public address",
            Refusal::TooMany => "past the most entries kept",
        }
    }
}

/// An address that can be a client's as the load balancer sees it.
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            let [first, second, ..] = a.octets();
            !(first == 0
                || a.is_loopback()
                || a.is_private()
                || a.is_link_local()
                || (first == 100 && (second & 0xc0) == 64)
                || a.is_multicast()
                || first >= 240)
        }
        IpAddr::V6(a) => {
            let head = a.segments()[0];
            !(a.is_unspecified()
                || a.is_loopback()
                || a.is_multicast()
                || (head & 0xfe00) == 0xfc00
                || (head & 0xffc0) == 0xfe80)
        }
    }
}

/// One entry of the list: `address` or `address/len`, an IPv6 address
/// optionally in square brackets. An IPv4-mapped IPv6 entry is its IPv4
/// address, as a client's is.
fn parse_entry(raw: &str) -> Result<Prefix, Refusal> {
    let (address, len) = match raw.split_once('/') {
        Some((address, len)) => (address, Some(len)),
        None => (raw, None),
    };
    let address = address
        .strip_prefix('[')
        .and_then(|a| a.strip_suffix(']'))
        .unwrap_or(address);
    let written: IpAddr = address.parse().map_err(|_| Refusal::NotAnAddress)?;
    let width: u8 = if written.is_ipv4() { 32 } else { 128 };
    let len = match len {
        None => width,
        Some(len) if !len.is_empty() && len.bytes().all(|b| b.is_ascii_digit()) => {
            match len.parse::<u8>() {
                Ok(len) if len <= width => len,
                _ => return Err(Refusal::NotAnAddress),
            }
        }
        Some(_) => return Err(Refusal::NotAnAddress),
    };
    let ip = written.to_canonical();
    let len = if written.is_ipv6() && ip.is_ipv4() {
        len.checked_sub(96).ok_or(Refusal::TooBroad)?
    } else {
        len
    };
    let floor = if ip.is_ipv4() {
        MIN_PREFIX_V4
    } else {
        MIN_PREFIX_V6
    };
    if len < floor {
        return Err(Refusal::TooBroad);
    }
    let network = masked(ip, len);
    if !is_public(network) {
        return Err(Refusal::NotPublic);
    }
    Ok(Prefix { network, len })
}

/// What one reading of the secret yielded.
struct Parsed {
    entries: Vec<Prefix>,
    /// Position of each refused entry, and why.
    refused: Vec<(usize, Refusal)>,
    /// The document started like JSON and is not a JSON array: nothing in it
    /// counts.
    unreadable: bool,
}

fn parse(raw: &str) -> Parsed {
    let mut parsed = Parsed {
        entries: Vec::new(),
        refused: Vec::new(),
        unreadable: false,
    };
    let Some(items) = items(raw) else {
        parsed.unreadable = true;
        return parsed;
    };
    for (position, item) in items.into_iter().enumerate() {
        let entry = item
            .as_deref()
            .map_or(Err(Refusal::NotAnAddress), parse_entry);
        match entry {
            Ok(prefix) if parsed.entries.contains(&prefix) => {}
            Ok(_) if parsed.entries.len() >= MAX_ENTRIES => {
                parsed.refused.push((position, Refusal::TooMany));
            }
            Ok(prefix) => parsed.entries.push(prefix),
            Err(refusal) => parsed.refused.push((position, refusal)),
        }
    }
    parsed
}

/// The candidate entries of `raw`, `None` for an item that is not text. `None`
/// overall for a document that starts like JSON and is not a JSON array: an
/// object is not the stack's form, and JSON that does not parse is no list.
fn items(raw: &str) -> Option<Vec<Option<String>>> {
    let raw = raw.trim();
    let mut items = Vec::new();
    if !(raw.starts_with('[') || raw.starts_with('{')) {
        push_words(raw, &mut items);
        return Some(items);
    }
    let Value::Array(list) = serde_json::from_str::<Value>(raw).ok()? else {
        return None;
    };
    list.iter().for_each(|v| push_item(v, &mut items));
    Some(items)
}

fn push_item(value: &Value, items: &mut Vec<Option<String>>) {
    match value {
        Value::String(text) => push_words(text, items),
        _ => items.push(None),
    }
}

fn push_words(text: &str, items: &mut Vec<Option<String>>) {
    items.extend(
        text.split(|c: char| c == ',' || c == ';' || c.is_whitespace())
            .filter(|word| !word.is_empty())
            .map(|word| Some(word.to_string())),
    );
}

// ============================================================================
// The list
// ============================================================================

/// How the last read of the secret went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LastRead {
    Never,
    Ok,
    /// Read, and not a list (it starts like JSON and is not an array): empty.
    Unreadable,
    /// The secret does not exist or holds no value: an empty list.
    Missing,
    Failing,
}

impl LastRead {
    pub fn as_str(self) -> &'static str {
        match self {
            LastRead::Never => "never",
            LastRead::Ok => "ok",
            LastRead::Unreadable => "unreadable",
            LastRead::Missing => "missing",
            LastRead::Failing => "failing",
        }
    }
}

struct State {
    entries: Arc<[Prefix]>,
    last_read: LastRead,
    failed_reads: u32,
    /// SHA-256 of the document the entries came from, so an unchanged secret
    /// is not parsed, nor its refusals logged, on every refresh.
    fingerprint: Option<[u8; 32]>,
    /// That document was not a list.
    unreadable: bool,
    /// The last read that reached the secret, with or without a value.
    read_at: Option<Instant>,
}

/// The addresses exempt from the per-IP policy, re-read from their secret.
///
/// `Debug` prints the secret's name and a count, never an address.
pub struct IpAllowlist {
    source: Option<String>,
    refresh: Duration,
    /// How long one read may take before it counts as failed.
    read_timeout: Duration,
    state: RwLock<State>,
}

impl std::fmt::Debug for IpAllowlist {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IpAllowlist")
            .field("source", &self.source)
            .field("entries", &self.len())
            .finish()
    }
}

impl IpAllowlist {
    fn new(source: Option<String>, refresh: Duration) -> Self {
        Self {
            source,
            refresh,
            read_timeout: READ_TIMEOUT,
            state: RwLock::new(State {
                entries: Arc::from(Vec::new()),
                last_read: LastRead::Never,
                failed_reads: 0,
                fingerprint: None,
                unreadable: false,
                read_at: None,
            }),
        }
    }

    /// No list: nobody is exempt by address, and nothing is read.
    pub fn disabled() -> Self {
        Self::new(None, Duration::from_secs(DEFAULT_REFRESH_SECS))
    }

    pub fn from_env() -> Self {
        Self::from_lookup(|var| std::env::var(var).ok())
    }

    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let source = lookup(ENV_SECRET)
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty());
        let refresh_secs = match lookup(ENV_REFRESH_SECS) {
            None => DEFAULT_REFRESH_SECS,
            Some(raw) => match raw.trim().parse::<u64>() {
                Ok(secs) if (MIN_REFRESH_SECS..=MAX_REFRESH_SECS).contains(&secs) => secs,
                _ => {
                    warn!(
                        variable = ENV_REFRESH_SECS,
                        default = DEFAULT_REFRESH_SECS,
                        "not between 30 and 3600 seconds; the default applies"
                    );
                    DEFAULT_REFRESH_SECS
                }
            },
        };
        match &source {
            Some(name) => info!(
                secret = %name,
                refresh_secs,
                "IP allowlist: read from Secrets Manager (addresses are never logged)"
            ),
            None => info!(
                variable = ENV_SECRET,
                "IP allowlist disabled: no secret configured"
            ),
        }
        Self::new(source, Duration::from_secs(refresh_secs))
    }

    /// A list read once from `raw`, for tests. Production's refresh, so it
    /// does not go stale while a test runs.
    #[cfg(test)]
    pub fn for_tests(raw: &str) -> Self {
        let list = Self::new(
            Some("test".to_string()),
            Duration::from_secs(DEFAULT_REFRESH_SECS),
        );
        list.apply(Ok(Some(raw.to_string())));
        list
    }

    /// An enabled, empty list with its own refresh and read timeout.
    #[cfg(test)]
    fn with_timing(refresh: Duration, read_timeout: Duration) -> Self {
        Self {
            read_timeout,
            ..Self::new(Some("test".to_string()), refresh)
        }
    }

    /// The secret's name, when the list is enabled.
    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    /// How many entries are in force: none once the list is stale.
    pub fn len(&self) -> usize {
        self.in_force().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn state(&self) -> std::sync::RwLockReadGuard<'_, State> {
        self.state.read().unwrap_or_else(|e| e.into_inner())
    }

    /// How long a list may go without a read that reached the secret before it
    /// counts as empty: one read past the point where the failed reads alone
    /// empty it. The backstop for a refresher that stopped running at all.
    fn stale_after(&self) -> Duration {
        (self.refresh + self.read_timeout) * (EMPTIED_AFTER_FAILED_READS + 1)
    }

    fn is_stale(&self, state: &State) -> bool {
        state
            .read_at
            .is_none_or(|at| at.elapsed() > self.stale_after())
    }

    /// The entries in force: the last ones read, or none once they are stale.
    fn in_force(&self) -> Arc<[Prefix]> {
        let state = self.state();
        if self.is_stale(&state) {
            Arc::from(Vec::new())
        } else {
            Arc::clone(&state.entries)
        }
    }

    /// Whether `ip` is on the list. An IPv4-mapped IPv6 address is its IPv4
    /// address.
    pub fn contains(&self, ip: IpAddr) -> bool {
        let entries = self.in_force();
        let ip = ip.to_canonical();
        entries.iter().any(|prefix| prefix.contains(ip))
    }

    /// Take in one read of the secret: its document, `None` when it does not
    /// exist or holds no value, or the error that kept it from being read.
    pub fn apply(&self, read: Result<Option<String>, String>) {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        match read {
            Ok(Some(raw)) => {
                let fingerprint: [u8; 32] = Sha256::digest(raw.as_bytes()).into();
                if state.fingerprint != Some(fingerprint) {
                    let parsed = parse(&raw);
                    if parsed.unreadable {
                        warn!(
                            "IP allowlist: the secret starts like JSON and is not a JSON \
                             array; no address is exempt"
                        );
                    }
                    for (position, refusal) in &parsed.refused {
                        warn!(
                            position,
                            reason = refusal.as_str(),
                            "IP allowlist: entry refused"
                        );
                    }
                    info!(
                        entries = parsed.entries.len(),
                        refused = parsed.refused.len(),
                        "IP allowlist loaded"
                    );
                    state.entries = Arc::from(parsed.entries);
                    state.unreadable = parsed.unreadable;
                    state.fingerprint = Some(fingerprint);
                }
                state.last_read = if state.unreadable {
                    LastRead::Unreadable
                } else {
                    LastRead::Ok
                };
                state.read_at = Some(Instant::now());
                state.failed_reads = 0;
            }
            Ok(None) => {
                if state.last_read != LastRead::Missing {
                    info!("IP allowlist: the secret does not exist or holds no value; no address is exempt");
                }
                state.entries = Arc::from(Vec::new());
                state.fingerprint = None;
                state.unreadable = false;
                state.last_read = LastRead::Missing;
                state.read_at = Some(Instant::now());
                state.failed_reads = 0;
            }
            Err(error) => {
                state.failed_reads = state.failed_reads.saturating_add(1);
                let failed_reads = state.failed_reads;
                if failed_reads == 1 || failed_reads.is_multiple_of(FAILED_READ_LOG_EVERY) {
                    warn!(
                        failed_reads,
                        error = %error,
                        "IP allowlist: the secret could not be read"
                    );
                }
                if failed_reads >= EMPTIED_AFTER_FAILED_READS && !state.entries.is_empty() {
                    warn!(
                        failed_reads,
                        "IP allowlist emptied: not re-read for too long, and an old address \
                         may belong to somebody else"
                    );
                    state.entries = Arc::from(Vec::new());
                    state.fingerprint = None;
                }
                state.last_read = LastRead::Failing;
            }
        }
    }

    /// Read the list now and then every refresh, for as long as the process
    /// runs. `None`, and nothing spawned, when the list is disabled.
    pub fn spawn_refresher<R, F>(self: &Arc<Self>, read: R) -> Option<tokio::task::JoinHandle<()>>
    where
        R: Fn() -> F + Send + 'static,
        F: Future<Output = Result<Option<String>, String>> + Send + 'static,
    {
        self.source.as_ref()?;
        let list = Arc::clone(self);
        Some(tokio::spawn(async move {
            loop {
                let outcome = match tokio::time::timeout(list.read_timeout, read()).await {
                    Ok(outcome) => outcome,
                    Err(_) => Err(format!(
                        "no answer within {} ms",
                        list.read_timeout.as_millis()
                    )),
                };
                list.apply(outcome);
                tokio::time::sleep(list.refresh).await;
            }
        }))
    }

    /// What `GET /config` says about the list: whether it is on, how many
    /// entries are in force and how the last read went. Never an address, nor
    /// the secret's name.
    pub fn summary(&self) -> Value {
        let state = self.state();
        let stale = state.read_at.is_some() && self.is_stale(&state);
        json!({
            "enabled": self.source.is_some(),
            "entries": if stale { 0 } else { state.entries.len() },
            "lastRead": if stale { "stale" } else { state.last_read.as_str() },
            "refreshSecs": self.refresh.as_secs(),
            "keyedOn": "client IP: the last X-Forwarded-For entry, else the TCP peer",
            "exemptHeader": format!("x-ratelimit-exempt: {EXEMPT_AS}"),
            "configuration": format!(
                "{ENV_SECRET} (the name of the Secrets Manager secret holding the list; unset \
                 disables it) and {ENV_REFRESH_SECS} ({MIN_REFRESH_SECS}-{MAX_REFRESH_SECS}, \
                 default {DEFAULT_REFRESH_SECS})"
            ),
        })
    }
}

/// One read of the secret named `name`. `Ok(None)` when it does not exist or
/// holds no value yet: an empty list, not a failure.
pub async fn read_secret(
    client: &aws_sdk_secretsmanager::Client,
    name: &str,
) -> Result<Option<String>, String> {
    match client.get_secret_value().secret_id(name).send().await {
        Ok(output) => Ok(Some(output.secret_string().unwrap_or_default().to_string())),
        Err(error) => {
            if error
                .as_service_error()
                .is_some_and(|e| e.is_resource_not_found_exception())
            {
                Ok(None)
            } else {
                Err(aws_sdk_secretsmanager::error::DisplayErrorContext(&error).to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |var| vars.get(var).cloned()
    }

    /// The stack's form (a JSON array of strings) and plain text yield the same
    /// two entries.
    #[test]
    fn entries_in_every_shape_the_secret_may_take() {
        for raw in [
            r#"["198.51.100.7", "2001:db8:1:2::/64"]"#,
            r#" [ "198.51.100.7", "[2001:db8:1:2::]/64" ] "#,
            r#"["198.51.100.7, 2001:db8:1:2::/64"]"#,
            "198.51.100.7, 2001:db8:1:2::/64",
            " 198.51.100.7 ; 2001:db8:1:2::/64 ",
            "198.51.100.7\n2001:db8:1:2::/64\n",
            "198.51.100.7, 198.51.100.7, 2001:db8:1:2::/64",
        ] {
            let list = IpAllowlist::for_tests(raw);
            assert_eq!(list.len(), 2, "{raw}");
            assert!(list.contains(ip("198.51.100.7")), "{raw}");
            assert!(list.contains(ip("2001:db8:1:2:a:b:c:d")), "{raw}");
            assert!(!list.contains(ip("198.51.100.8")), "{raw}");
            assert!(!list.contains(ip("2001:db8:1:3::1")), "{raw}");
        }
    }

    /// What is refused, and why. Each would exempt somebody it should not, or
    /// is a slip nobody meant.
    #[test]
    fn what_is_refused_and_why() {
        let cases = [
            ("not-an-ip", Refusal::NotAnAddress),
            ("198.51.100", Refusal::NotAnAddress),
            ("198.51.100.7/", Refusal::NotAnAddress),
            ("198.51.100.7/33", Refusal::NotAnAddress),
            ("198.51.100.7/+24", Refusal::NotAnAddress),
            ("198.51.100.7/24/8", Refusal::NotAnAddress),
            ("2001:db8::/129", Refusal::NotAnAddress),
            ("0.0.0.0/0", Refusal::TooBroad),
            ("198.51.100.0/16", Refusal::TooBroad),
            ("198.51.100.0/23", Refusal::TooBroad),
            ("2001:db8::/47", Refusal::TooBroad),
            ("::/0", Refusal::TooBroad),
            ("::ffff:198.51.100.0/112", Refusal::TooBroad),
            ("::ffff:0.0.0.0/64", Refusal::TooBroad),
            ("0.0.0.0", Refusal::NotPublic),
            ("10.1.2.3", Refusal::NotPublic),
            ("172.16.5.4", Refusal::NotPublic),
            ("192.168.1.10", Refusal::NotPublic),
            ("127.0.0.1", Refusal::NotPublic),
            ("169.254.169.254", Refusal::NotPublic),
            ("100.64.1.2", Refusal::NotPublic),
            ("100.127.255.255", Refusal::NotPublic),
            ("224.0.0.1", Refusal::NotPublic),
            ("255.255.255.255", Refusal::NotPublic),
            ("240.0.0.1", Refusal::NotPublic),
            ("::", Refusal::NotPublic),
            ("::1", Refusal::NotPublic),
            ("fe80::1", Refusal::NotPublic),
            ("febf::1", Refusal::NotPublic),
            ("fc00::1", Refusal::NotPublic),
            ("fd12:3456::1", Refusal::NotPublic),
            ("ff02::1", Refusal::NotPublic),
            ("::ffff:10.0.0.1", Refusal::NotPublic),
        ];
        for (raw, why) in cases {
            assert_eq!(parse_entry(raw).err(), Some(why), "{raw}");
            let list = IpAllowlist::for_tests(raw);
            assert!(list.is_empty(), "{raw} made it onto the list");
        }
        // The narrowest accepted prefixes, and a mapped address, are taken.
        // So are the neighbours of the IPv6 blocks refused above. (Only
        // documentation addresses are written here, never a real one.)
        for raw in [
            "198.51.100.0/24",
            "2001:db8:1::/48",
            "::ffff:198.51.100.9",
            "fec0::1",
            "fbff::1",
        ] {
            assert!(parse_entry(raw).is_ok(), "{raw}");
        }
    }

    /// A document that starts like JSON is read only as a JSON array. An object
    /// is not the stack's form, and JSON that does not parse is no list.
    #[test]
    fn a_document_that_starts_like_json_and_is_no_array_exempts_nobody() {
        for raw in [
            r#"["198.51.100.7""#,
            r#"[198.51.100.7]"#,
            r#"{"ips": ["198.51.100.7"]}"#,
            r#"{"list": "198.51.100.7"}"#,
            r#"{"ips": ["198.51.100.7"]"#,
        ] {
            let parsed = parse(raw);
            assert!(parsed.unreadable, "{raw}");
            assert!(IpAllowlist::for_tests(raw).is_empty(), "{raw}");
            // Over a list read before, too: the old entries do not survive a
            // document that is not a list, and /config says why.
            let list = IpAllowlist::for_tests("198.51.100.7");
            list.apply(Ok(Some(raw.to_string())));
            assert!(list.is_empty(), "{raw} left the old list in force");
            assert_eq!(list.summary()["lastRead"], "unreadable", "{raw}");
            list.apply(Ok(Some(r#"["198.51.100.7"]"#.to_string())));
            assert!(list.contains(ip("198.51.100.7")), "{raw}");
        }
        // A non-text item is refused by position; the rest of the list stands.
        let parsed = parse(r#"["198.51.100.7", 42, null, "203.0.113.9"]"#);
        assert_eq!(parsed.entries.len(), 2);
        assert_eq!(
            parsed.refused,
            [(1, Refusal::NotAnAddress), (2, Refusal::NotAnAddress)]
        );
    }

    #[test]
    fn the_list_keeps_at_most_max_entries() {
        let raw: Vec<String> = (0..=MAX_ENTRIES)
            .map(|n| format!("2001:db8:{n:x}::1"))
            .collect();
        let parsed = parse(&raw.join(","));
        assert_eq!(parsed.entries.len(), MAX_ENTRIES);
        assert_eq!(parsed.refused, [(MAX_ENTRIES, Refusal::TooMany)]);
    }

    /// A prefix holds its network and nothing beside it; an address holds
    /// itself; an IPv4 client seen as IPv4-mapped IPv6 is still itself.
    #[test]
    fn a_prefix_matches_its_network_and_nothing_next_to_it() {
        // A /25 inside a documentation /24, so both edges of the mask are
        // documentation addresses too.
        let list = IpAllowlist::for_tests("198.51.100.128/25, 203.0.113.9, 2001:db8:a:b::/64");
        for inside in [
            "198.51.100.128",
            "198.51.100.255",
            "203.0.113.9",
            "::ffff:203.0.113.9",
            "::ffff:198.51.100.200",
            "2001:db8:a:b::",
            "2001:db8:a:b:ffff:ffff:ffff:ffff",
        ] {
            assert!(list.contains(ip(inside)), "{inside}");
        }
        for outside in [
            "198.51.100.127",
            "198.51.100.0",
            "203.0.113.8",
            "203.0.113.10",
            "2001:db8:a:c::",
            "2001:db8:a:a:ffff:ffff:ffff:ffff",
            "192.0.2.1",
            "::",
        ] {
            assert!(!list.contains(ip(outside)), "{outside}");
        }
        assert!(!IpAllowlist::disabled().contains(ip("198.51.100.1")));
    }

    /// The provider hands out a new address: the next read moves the
    /// exemption to it and takes it from the old one.
    #[test]
    fn a_new_read_replaces_the_list_and_a_missing_secret_empties_it() {
        let list = IpAllowlist::for_tests("198.51.100.7");
        assert!(list.contains(ip("198.51.100.7")));

        list.apply(Ok(Some("198.51.100.8".to_string())));
        assert!(
            !list.contains(ip("198.51.100.7")),
            "the old address kept its exemption"
        );
        assert!(list.contains(ip("198.51.100.8")));

        list.apply(Ok(None));
        assert!(list.is_empty());
        assert_eq!(list.state().last_read, LastRead::Missing);

        // The same document read again after being emptied is taken again.
        list.apply(Ok(Some("198.51.100.8".to_string())));
        assert!(list.contains(ip("198.51.100.8")));
        list.apply(Ok(Some(String::new())));
        assert!(list.is_empty(), "an empty secret is an empty list");
    }

    /// A read that fails keeps the last list for a while, never for ever.
    #[test]
    fn a_failing_read_keeps_the_list_briefly_then_empties_it() {
        assert_eq!(
            EMPTIED_AFTER_FAILED_READS, 3,
            "the CHANGELOG promises three"
        );
        let list = IpAllowlist::for_tests("198.51.100.7");
        for n in 1..EMPTIED_AFTER_FAILED_READS {
            list.apply(Err("AccessDeniedException".to_string()));
            assert!(
                list.contains(ip("198.51.100.7")),
                "emptied after {n} failures"
            );
            assert_eq!(list.state().last_read, LastRead::Failing);
        }
        list.apply(Err("AccessDeniedException".to_string()));
        assert!(list.is_empty(), "a stale list outlived its reads");

        // The secret is readable again: the same document comes back.
        list.apply(Ok(Some("198.51.100.7".to_string())));
        assert!(list.contains(ip("198.51.100.7")));
        assert_eq!(list.state().failed_reads, 0);
    }

    #[test]
    fn the_secret_and_its_refresh_are_read_defensively() {
        let disabled = IpAllowlist::from_lookup(lookup(&[]));
        assert_eq!(disabled.source(), None);
        let blank = IpAllowlist::from_lookup(lookup(&[(ENV_SECRET, "  ")]));
        assert_eq!(blank.source(), None);

        let on = IpAllowlist::from_lookup(lookup(&[(ENV_SECRET, " uvd/allowlist/home ")]));
        assert_eq!(on.source(), Some("uvd/allowlist/home"));
        assert_eq!(on.refresh, Duration::from_secs(DEFAULT_REFRESH_SECS));
        assert!(on.is_empty(), "nothing is exempt before the first read");
        assert_eq!(on.state().last_read, LastRead::Never);

        for (raw, secs) in [
            ("60", 60),
            (" 3600 ", 3600),
            ("29", DEFAULT_REFRESH_SECS),
            ("3601", DEFAULT_REFRESH_SECS),
            ("0", DEFAULT_REFRESH_SECS),
            ("5m", DEFAULT_REFRESH_SECS),
        ] {
            let list = IpAllowlist::from_lookup(lookup(&[
                (ENV_SECRET, "uvd/allowlist/home"),
                (ENV_REFRESH_SECS, raw),
            ]));
            assert_eq!(list.refresh, Duration::from_secs(secs), "{raw}");
        }
    }

    /// Neither `Debug` nor the summary carries an address.
    #[test]
    fn nothing_prints_an_address() {
        let list = IpAllowlist::for_tests("198.51.100.7, 2001:db8:1:2::/64");
        let debug = format!("{list:?}");
        let summary = list.summary().to_string();
        for shown in [&debug, &summary] {
            assert!(!shown.contains("198.51.100"), "{shown}");
            assert!(!shown.contains("2001:db8"), "{shown}");
        }
        assert_eq!(list.summary()["entries"], 2);
        assert_eq!(list.summary()["lastRead"], "ok");
        assert!(
            list.summary().get("secret").is_none(),
            "the summary names the secret"
        );
    }

    /// The refresher reads the secret again on its own, so a new address takes
    /// effect without a restart, and a disabled list spawns nothing.
    #[tokio::test]
    async fn the_refresher_picks_up_a_new_address_without_a_restart() {
        assert!(Arc::new(IpAllowlist::disabled())
            .spawn_refresher(|| async { Ok(Some("198.51.100.7".to_string())) })
            .is_none());

        let list = Arc::new(IpAllowlist::new(
            Some("test".to_string()),
            Duration::from_millis(10),
        ));
        let current = Arc::new(std::sync::Mutex::new("198.51.100.7".to_string()));
        let source = Arc::clone(&current);
        let task = list
            .spawn_refresher(move || {
                let raw = source.lock().unwrap().clone();
                async move { Ok(Some(raw)) }
            })
            .expect("an enabled list spawns its refresher");

        let wait_for = |target: &'static str| {
            let list = Arc::clone(&list);
            async move {
                for _ in 0..500 {
                    if list.contains(ip(target)) {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                panic!("{target} never made it onto the list");
            }
        };
        wait_for("198.51.100.7").await;
        *current.lock().unwrap() = "198.51.100.8".to_string();
        wait_for("198.51.100.8").await;
        assert!(!list.contains(ip("198.51.100.7")));
        task.abort();
    }

    /// A read that never answers is a failed read: it times out, counts, and
    /// past the limit the list is emptied -- it does not hold the last list in
    /// force while the refresher waits for ever.
    #[tokio::test]
    async fn a_read_that_never_answers_is_a_failed_read() {
        let list = Arc::new(IpAllowlist::with_timing(
            Duration::from_millis(10),
            Duration::from_millis(20),
        ));
        list.apply(Ok(Some("198.51.100.7".to_string())));
        assert!(list.contains(ip("198.51.100.7")));
        let task = list
            .spawn_refresher(std::future::pending::<Result<Option<String>, String>>)
            .expect("an enabled list spawns its refresher");
        for _ in 0..400 {
            if list.state().failed_reads >= EMPTIED_AFTER_FAILED_READS {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        task.abort();
        let state = list.state();
        assert!(
            state.failed_reads >= EMPTIED_AFTER_FAILED_READS,
            "a read with no answer never counted as failed"
        );
        assert_eq!(state.last_read, LastRead::Failing);
        assert!(
            state.entries.is_empty(),
            "the list outlived its failed reads"
        );
    }

    /// Whatever happens to the refresher, a list nobody re-reads goes stale
    /// and exempts nobody; the next read brings it back.
    #[test]
    fn a_list_nobody_re_reads_goes_stale_and_exempts_nobody() {
        let list = IpAllowlist::with_timing(Duration::from_millis(10), Duration::from_millis(10));
        assert!(list.is_empty(), "never read is not in force");
        list.apply(Ok(Some("198.51.100.7".to_string())));
        assert!(list.contains(ip("198.51.100.7")));
        std::thread::sleep(list.stale_after() + Duration::from_millis(50));
        assert!(!list.contains(ip("198.51.100.7")), "a stale list exempted");
        assert!(list.is_empty());
        assert_eq!(list.summary()["entries"], 0);
        assert_eq!(list.summary()["lastRead"], "stale");
        list.apply(Ok(Some("198.51.100.7".to_string())));
        assert!(list.contains(ip("198.51.100.7")));
        // Production's numbers: about twenty minutes without a read that
        // reached the secret, against fifteen for three failed reads.
        let production = IpAllowlist::for_tests("198.51.100.7");
        assert_eq!(production.stale_after(), Duration::from_secs(4 * 310));
    }
}
