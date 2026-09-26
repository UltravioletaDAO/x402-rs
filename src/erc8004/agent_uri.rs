//! What an `agentUri` may be before the facilitator pays to write it on-chain.
//!
//! `POST /register` is public and gasless: whoever calls it gets an ERC-8004
//! identity minted by our wallet, with our gas, carrying the URI they chose.
//! Until 2.44.0 that URI was taken as-is, and in September 2026 an identity
//! minted this way, with an `http://` URI on a host that embedded a bare IP,
//! was the public face of a campaign that asked agents to run a remote
//! package. The identity cost its author nothing.
//!
//! So the URI has to name something a registration file can legitimately live
//! at: `https://` on a public DNS name, or `ipfs://`. Refused, each with its
//! own code so a caller can tell what to fix:
//!
//! - a host that IS an IP, in any of the forms a URL parser accepts
//!   (`198.51.100.7`, `3325256711`, `0xc6.0x33.0x64.0x07`, `[2001:db8::1]`);
//! - a host that EMBEDS one (`198-51-100-7.sslip.io`, `app.198-51-100-7.example.com`),
//!   plus the wildcard-DNS services that resolve such names;
//! - tunnels (`*.ngrok-free.app`, `*.trycloudflare.com`, ...), which put a
//!   laptop behind a name that looks like a service;
//! - names that do not resolve publicly (`localhost`, `*.local`, `*.onion`, one label);
//! - credentials in the URI, which is how `https://trusted@<ip>/` reads as trusted,
//!   and a `\` anywhere, which two URL parsers split into two different hosts.
//!
//! The domain lists live in `config/erc8004_agent_uri_rules.json`, shared with
//! `scripts/erc8004_custodied_identities.py`, and both are tested against the
//! same corpus (`tests/fixtures/erc8004_agent_uri_cases.json`).

use std::net::Ipv4Addr;

use once_cell::sync::Lazy;
use serde::Deserialize;

const RULES_JSON: &str = include_str!("../../config/erc8004_agent_uri_rules.json");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rules {
    schemes: Vec<String>,
    max_bytes: usize,
    non_public_suffixes: Vec<String>,
    wildcard_dns_domains: Vec<String>,
    tunnel_domains: Vec<String>,
}

static RULES: Lazy<Rules> = Lazy::new(|| {
    serde_json::from_str(RULES_JSON).expect("config/erc8004_agent_uri_rules.json must parse")
});

/// One rule an `agentUri` breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Violation {
    /// Empty or only whitespace.
    Missing,
    /// Longer than the rules allow.
    TooLong,
    /// Not a URI we can read unambiguously: whitespace or control characters
    /// inside, no host, or a host no URL parser accepts.
    Malformed,
    /// A scheme other than `https` or `ipfs`.
    Scheme,
    /// A user or password before the host.
    Credentials,
    /// The host is an IP address.
    IpLiteral,
    /// The host is not a public DNS name.
    NonPublicHost,
    /// The host embeds an IP, or belongs to a service that resolves such names.
    EmbeddedIp,
    /// The host belongs to a tunnelling service.
    Tunnel,
}

impl Violation {
    /// Stable machine-readable code, returned as `errorCode`.
    pub fn code(self) -> &'static str {
        match self {
            Violation::Missing => "agent_uri_missing",
            Violation::TooLong => "agent_uri_too_long",
            Violation::Malformed => "agent_uri_malformed",
            Violation::Scheme => "agent_uri_scheme",
            Violation::Credentials => "agent_uri_credentials",
            Violation::IpLiteral => "agent_uri_ip_literal",
            Violation::NonPublicHost => "agent_uri_non_public_host",
            Violation::EmbeddedIp => "agent_uri_embedded_ip",
            Violation::Tunnel => "agent_uri_tunnel",
        }
    }

    /// What to fix, for the `error` text.
    pub fn explain(self) -> String {
        match self {
            Violation::Missing => "agentUri is required".to_string(),
            Violation::TooLong => format!("agentUri is longer than {} bytes", RULES.max_bytes),
            Violation::Malformed => {
                "agentUri is not a well-formed URI (a host, and no spaces, control characters or \\)"
                    .to_string()
            }
            Violation::Scheme => "agentUri must start with https:// or ipfs://".to_string(),
            Violation::Credentials => "agentUri must not carry a user or password".to_string(),
            Violation::IpLiteral => "agentUri must name a host, not an IP address".to_string(),
            Violation::NonPublicHost => {
                "agentUri must name a public DNS host (not localhost, .local or a single label)"
                    .to_string()
            }
            Violation::EmbeddedIp => {
                "agentUri must not use a host that embeds an IP address or a wildcard-DNS service"
                    .to_string()
            }
            Violation::Tunnel => "agentUri must not point at a tunnelling service".to_string(),
        }
    }
}

/// The first rule `uri` breaks, if any. What `POST /register` enforces.
pub fn check(uri: &str) -> Result<(), Violation> {
    match violations(uri).first() {
        Some(v) => Err(*v),
        None => Ok(()),
    }
}

/// Every rule `uri` breaks, in a fixed order; empty when it is acceptable.
///
/// All of them rather than the first, for the audit of URIs already on-chain
/// and for the admin dry run: `http://` on an embedded IP is two findings.
pub fn violations(uri: &str) -> Vec<Violation> {
    if uri.trim().is_empty() {
        return vec![Violation::Missing];
    }
    let mut found = Vec::new();
    if uri.len() > RULES.max_bytes {
        found.push(Violation::TooLong);
    }
    // A URL parser strips tabs and newlines and trims spaces, so the string it
    // judged would not be the string written on-chain. Refuse the difference
    // instead of reasoning about it. Same for `\`: a WHATWG parser ends the
    // host at it (`https://good\@198.51.100.7/` is `good`), while other URL
    // parsers, Python's among them, read the host after the `@`.
    if uri
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
    {
        found.push(Violation::Malformed);
        return found;
    }
    let url = match url::Url::parse(uri) {
        Ok(url) => url,
        Err(_) => {
            found.push(Violation::Malformed);
            return found;
        }
    };
    let scheme = url.scheme();
    if !RULES.schemes.iter().any(|s| s == scheme) {
        found.push(Violation::Scheme);
    }
    if scheme == "ipfs" {
        // `ipfs://<cid>[/path]`: the authority is the content id, and nothing
        // else -- no user, no port.
        let cid_ok = url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && uri
                .get(.."ipfs://".len())
                .is_some_and(|p| p.eq_ignore_ascii_case("ipfs://"))
            && url
                .host_str()
                .is_some_and(|h| !h.is_empty() && h.chars().all(|c| c.is_ascii_alphanumeric()));
        if !cid_ok {
            found.push(Violation::Malformed);
        }
        return found;
    }
    let Some(host) = url.host() else {
        // `data:`, `javascript:` and friends: the scheme already says no.
        if !found.contains(&Violation::Scheme) {
            found.push(Violation::Malformed);
        }
        return found;
    };
    if !url.username().is_empty() || url.password().is_some() {
        found.push(Violation::Credentials);
    }
    match host {
        url::Host::Ipv4(_) | url::Host::Ipv6(_) => found.push(Violation::IpLiteral),
        url::Host::Domain(domain) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            // An opaque host (a scheme the parser does not know) is not
            // normalised, so a dotted quad arrives here as a "domain".
            if domain.parse::<Ipv4Addr>().is_ok() {
                found.push(Violation::IpLiteral);
                return found;
            }
            if !domain.contains('.') || matches_any(&domain, &RULES.non_public_suffixes) {
                found.push(Violation::NonPublicHost);
            }
            if embeds_ipv4(&domain) || matches_any(&domain, &RULES.wildcard_dns_domains) {
                found.push(Violation::EmbeddedIp);
            }
            if matches_any(&domain, &RULES.tunnel_domains) {
                found.push(Violation::Tunnel);
            }
        }
    }
    found
}

/// `host` is one of `suffixes`, or a subdomain of one.
fn matches_any(host: &str, suffixes: &[String]) -> bool {
    suffixes.iter().any(|s| {
        host == s
            || host
                .strip_suffix(s.as_str())
                .is_some_and(|rest| rest.ends_with('.'))
    })
}

/// Four consecutive labels or dash-separated parts that read as IPv4 octets:
/// `198-51-100-7.x`, `198.51.100.7.x`, `app.198-51-100-7.x`.
fn embeds_ipv4(host: &str) -> bool {
    let parts: Vec<&str> = host.split(['.', '-']).collect();
    parts.windows(4).any(|w| {
        w.iter().all(|p| {
            (1..=3).contains(&p.len())
                && p.chars().all(|c| c.is_ascii_digit())
                && p.parse::<u16>().is_ok_and(|n| n <= 255)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CASES: &str = include_str!("../../tests/fixtures/erc8004_agent_uri_cases.json");

    fn corpus() -> serde_json::Value {
        serde_json::from_str(CASES).expect("the corpus parses")
    }

    fn codes(uri: &str) -> Vec<&'static str> {
        violations(uri).into_iter().map(Violation::code).collect()
    }

    /// Every case in the shared corpus, exactly: the same file the audit
    /// script's tests read, so the two cannot disagree about a URI.
    #[test]
    fn every_corpus_case_gets_exactly_its_violations() {
        let corpus = corpus();
        let cases = corpus["cases"].as_array().expect("cases");
        assert!(cases.len() >= 40, "the corpus lost cases");
        let mut accepted = 0;
        for case in cases {
            let uri = case["uri"].as_str().unwrap();
            let want: Vec<&str> = case["violations"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            assert_eq!(codes(uri), want, "{uri:?}");
            if want.is_empty() {
                accepted += 1;
                assert_eq!(check(uri), Ok(()), "{uri:?}");
            } else {
                assert_eq!(check(uri).unwrap_err().code(), want[0], "{uri:?}");
            }
        }
        // Both directions are exercised, or a guard that refuses everything
        // (or nothing) would pass half the table.
        assert!(accepted >= 8, "the corpus needs accepted URIs too");
        assert!(
            cases.len() - accepted >= 30,
            "the corpus needs refused URIs too"
        );
    }

    #[test]
    fn the_length_limit_is_the_configured_one_and_inclusive() {
        let corpus = corpus();
        let spec = &corpus["tooLong"];
        let prefix = spec["prefix"].as_str().unwrap();
        let total = spec["totalBytes"].as_u64().unwrap() as usize;
        assert_eq!(
            total,
            RULES.max_bytes + 1,
            "the corpus and the rules disagree"
        );
        let too_long = format!("{prefix}{}", "a".repeat(total - prefix.len()));
        assert_eq!(codes(&too_long), vec!["agent_uri_too_long"]);
        let at_limit = format!("{prefix}{}", "a".repeat(RULES.max_bytes - prefix.len()));
        assert_eq!(check(&at_limit), Ok(()));
    }

    /// Every code is distinct: a caller branches on them.
    #[test]
    fn codes_are_distinct() {
        let all = [
            Violation::Missing,
            Violation::TooLong,
            Violation::Malformed,
            Violation::Scheme,
            Violation::Credentials,
            Violation::IpLiteral,
            Violation::NonPublicHost,
            Violation::EmbeddedIp,
            Violation::Tunnel,
        ];
        let mut seen: Vec<&str> = all.iter().map(|v| v.code()).collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), all.len());
    }

    /// The URIs every caller in the stack builds today must keep passing.
    #[test]
    fn the_stack_callers_uri_shapes_pass() {
        for uri in [
            "https://execution.market/workers/0x3333333333333333333333333333333333333333",
            "https://execution.market/publishers/0x3333333333333333333333333333333333333333",
            "https://execution.market/agents/0x3333333333333333333333333333333333333333",
            "https://execution.market/agents/6xNPewUdKRbEZDReQdpyfNUdgNg8QRc8Mt263T5GZSRv",
        ] {
            assert_eq!(check(uri), Ok(()), "{uri}");
        }
    }
}
