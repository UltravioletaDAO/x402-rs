//! Security primitives shared across the Bazaar discovery subsystem.
//!
//! Consolidates the mitigations from `docs/plans/bazaar/08-security-hardening.md`
//! that can live as pure, reusable functions:
//!
//! - [`canonical_url`] — the single URL normalizer used as a key / match target
//!   everywhere (F1/F9/F13), so divergent normalization can't become a bypass.
//! - [`match_manifest_prefix`] — host-exact + path-boundary tier matching on a
//!   *parsed* URL, never `String::starts_with` on a raw one (F1). Prevents
//!   `https://api.meshrelay.xyz.evil.com/` from being awarded a curated tier.
//! - [`check_url_target`] / [`safe_get`] — the outbound SSRF connector
//!   (F2/F3/F16): reject userinfo, non-`{80,443,8080,8443}` ports and non-http
//!   schemes; resolve DNS and reject if *any* resolved address is
//!   non-routable/private/metadata (a mixed answer is an attack); pin the
//!   connection to the checked address (no re-resolve at connect); follow
//!   redirects manually, re-running every check on each hop.
//!
//! The DNS-resolving guard here is what makes the "resolve → check → pin"
//! design real: a hostname whose `A` record points at `169.254.169.254`
//! (EC2/Fargate metadata) is refused before any bytes are exchanged, and the
//! socket is pinned so a rebinding second resolution cannot swap in a private
//! address.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use reqwest::header::LOCATION;
use url::Url;

use crate::discovery::{host_as_encoded_ipv4, is_disallowed_target_ip};

/// Ports the outbound connector is allowed to reach. Everything else (e.g. an
/// internal `:6379` Redis or `:22` SSH) is refused so the prober cannot be
/// turned into an internal port scanner even if the IP checks ever regress.
const ALLOWED_PORTS: [u16; 4] = [80, 443, 8080, 8443];

/// Maximum redirect hops followed by [`safe_get`].
const MAX_REDIRECTS: usize = 3;

/// Reasons a URL is rejected before/while fetching.
#[derive(Debug, thiserror::Error)]
pub enum SecurityReject {
    #[error("URL parse error: {0}")]
    Parse(String),
    #[error("scheme must be http or https, got {0}")]
    Scheme(String),
    #[error("URL must not contain userinfo (user[:pass]@host)")]
    Userinfo,
    #[error("URL has no host")]
    NoHost,
    #[error("port {0} is not in the allowed set")]
    Port(u16),
    #[error("host {0} resolves to a non-routable, private, or link-local address")]
    DisallowedAddress(String),
    #[error("DNS resolution failed for {0}")]
    ResolutionFailed(String),
    #[error("too many redirects (>{0})")]
    TooManyRedirects(usize),
    #[error("http error: {0}")]
    Http(String),
}

/// A canonicalized URL plus its stable string key.
// Consumed as registry / health / curation / suppression keys by WS-A/WS-B/WS-C;
// public API in the library crate, wired into the binary in those workstreams.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalUrl {
    pub url: Url,
    /// Stable lowercase key for use as a HashMap / S3 identity.
    pub key: String,
    /// Lowercased host with any single trailing FQDN dot removed.
    pub host: String,
}

/// Normalize a URL for use as a key or match target. `url` 2.5.x already
/// lowercases the host and drops default ports for special schemes; on top of
/// that this rejects userinfo and non-http(s) schemes, strips a single trailing
/// FQDN dot from the host, and produces one canonical string. Every URL used as
/// a registry key, health/curation overlay key, suppression entry, or manifest
/// match target must go through this so the same input always yields the same
/// key (F9/F13).
// Callers land in WS-A (merge/GC), WS-B (health overlay) and WS-C (curation).
#[allow(dead_code)]
pub fn canonical_url(raw: &str) -> Result<CanonicalUrl, SecurityReject> {
    let mut url = Url::parse(raw).map_err(|e| SecurityReject::Parse(e.to_string()))?;

    let scheme = url.scheme().to_string();
    if scheme != "http" && scheme != "https" {
        return Err(SecurityReject::Scheme(scheme));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(SecurityReject::Userinfo);
    }

    let host = url.host_str().ok_or(SecurityReject::NoHost)?.to_string();
    // url already lowercases hosts; strip a single trailing FQDN dot so
    // `example.com.` and `example.com` share a key.
    let host = host.strip_suffix('.').unwrap_or(&host).to_ascii_lowercase();
    if host.is_empty() {
        return Err(SecurityReject::NoHost);
    }
    // Re-set the (possibly dot-stripped) host so the key reflects it.
    let _ = url.set_host(Some(&host));

    // Collapse a bare empty path to "/".
    if url.path().is_empty() {
        url.set_path("/");
    }

    let key = url.as_str().to_string();
    Ok(CanonicalUrl { url, key, host })
}

/// Host-exact + path-boundary match for a curated-tier manifest entry.
///
/// `manifest_host` is compared for exact equality (after lowercasing and
/// trailing-dot stripping); `manifest_path` matches either exactly or, when it
/// ends in `/`, as a path prefix on a segment boundary. Scheme must be https.
/// This is deliberately NOT `raw_url.starts_with(prefix)`: that would award the
/// tier to `https://api.meshrelay.xyz.evil.com/`, `…xyz@evil.com/`,
/// `…xyzevil.com/`, or `/api-evil` (F1).
// Called by WS-C tier resolution against `config/bazaar_curation.json`.
#[allow(dead_code)]
pub fn match_manifest_prefix(url: &Url, manifest_host: &str, manifest_path: &str) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    let manifest_host = manifest_host
        .strip_suffix('.')
        .unwrap_or(manifest_host)
        .to_ascii_lowercase();
    if host != manifest_host {
        return false;
    }
    let path = url.path();
    if path == manifest_path {
        return true;
    }
    // Prefix match only on a `/`-terminated manifest path, so `/api/` never
    // matches `/api-evil` but does match `/api/read/x`.
    if manifest_path.ends_with('/') && path.starts_with(manifest_path) {
        return true;
    }
    // Also allow the case where the manifest path lacks a trailing slash but the
    // URL adds one directly after it (`/mcp` matches `/mcp/...`).
    if !manifest_path.ends_with('/') {
        let with_slash = format!("{manifest_path}/");
        if path.starts_with(&with_slash) {
            return true;
        }
    }
    false
}

/// Validate a URL's scheme/userinfo/port, then resolve its host and confirm no
/// resolved address is disallowed. Returns the pinned, vetted socket addresses
/// to connect to. IP-literal hosts (including the alternate encodings the
/// `url` crate may not normalize) are checked directly without DNS.
pub async fn check_url_target(url: &Url) -> Result<Vec<SocketAddr>, SecurityReject> {
    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(SecurityReject::Scheme(scheme.to_string()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(SecurityReject::Userinfo);
    }
    let host = url.host_str().ok_or(SecurityReject::NoHost)?;
    let port = url.port_or_known_default().ok_or(SecurityReject::Port(0))?;
    if !ALLOWED_PORTS.contains(&port) {
        return Err(SecurityReject::Port(port));
    }

    // IP-literal host (canonical or alternate-encoded): check directly.
    if let Some(ip) = host
        .parse::<IpAddr>()
        .ok()
        .or_else(|| host_as_encoded_ipv4(host).map(IpAddr::V4))
    {
        if is_disallowed_target_ip(&ip) {
            return Err(SecurityReject::DisallowedAddress(host.to_string()));
        }
        return Ok(vec![SocketAddr::new(ip, port)]);
    }

    // DNS name: resolve, then reject if ANY resolved address is disallowed
    // (a mixed public+private answer is treated as an attack, not filtered).
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| SecurityReject::ResolutionFailed(host.to_string()))?
        .collect();
    if addrs.is_empty() {
        return Err(SecurityReject::ResolutionFailed(host.to_string()));
    }
    for a in &addrs {
        if is_disallowed_target_ip(&a.ip()) {
            return Err(SecurityReject::DisallowedAddress(host.to_string()));
        }
    }
    Ok(addrs)
}

/// SSRF-hardened GET. Resolves and vets the target, pins the connection to the
/// checked address(es), disables automatic redirects, and follows up to
/// [`MAX_REDIRECTS`] redirects manually — re-running every check on each hop.
/// Used by the crawler and (once built) the health prober for fetching
/// untrusted, listing-supplied URLs.
pub async fn safe_get(
    user_agent: &str,
    timeout: Duration,
    url: &Url,
) -> Result<reqwest::Response, SecurityReject> {
    let mut current = url.clone();
    for _hop in 0..=MAX_REDIRECTS {
        let addrs = check_url_target(&current).await?;
        let host = current
            .host_str()
            .ok_or(SecurityReject::NoHost)?
            .to_string();

        let client = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(user_agent)
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(&host, &addrs)
            .build()
            .map_err(|e| SecurityReject::Http(e.to_string()))?;

        let resp = client
            .get(current.clone())
            .send()
            .await
            .map_err(|e| SecurityReject::Http(e.to_string()))?;

        if resp.status().is_redirection() {
            let location = resp
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| SecurityReject::Http("redirect without Location".to_string()))?;
            // Resolve relative redirects against the current URL.
            let next = current
                .join(location)
                .map_err(|e| SecurityReject::Parse(e.to_string()))?;
            current = next;
            continue;
        }
        return Ok(resp);
    }
    Err(SecurityReject::TooManyRedirects(MAX_REDIRECTS))
}

/// Redirect policy for a long-lived shared client (the aggregator, which fetches
/// trusted hardcoded facilitator URLs but must not follow a redirect into an
/// internal target). Cannot resolve DNS (the policy closure is sync), so it does
/// URL-level checks only: reject IP-literal disallowed targets, userinfo,
/// non-allowed ports, and cap the hop count. DNS-name redirect targets are
/// vetted by the connector at connect time only for [`safe_get`]; for the
/// aggregator's trusted sources this URL-level policy is the proportionate
/// control (F15).
pub fn aggregator_redirect_policy(max: usize) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= max {
            return attempt.stop();
        }
        let url = attempt.url();
        let scheme = url.scheme();
        if scheme != "http" && scheme != "https" {
            return attempt.stop();
        }
        if !url.username().is_empty() || url.password().is_some() {
            return attempt.stop();
        }
        match url.port_or_known_default() {
            Some(p) if ALLOWED_PORTS.contains(&p) => {}
            _ => return attempt.stop(),
        }
        if let Some(host) = url.host_str() {
            if let Some(ip) = host
                .parse::<IpAddr>()
                .ok()
                .or_else(|| host_as_encoded_ipv4(host).map(IpAddr::V4))
            {
                if is_disallowed_target_ip(&ip) {
                    return attempt.stop();
                }
            }
        }
        attempt.follow()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_url_normalizes_and_rejects() {
        let c = canonical_url("HTTPS://API.Example.COM:443/a/b").unwrap();
        assert_eq!(c.host, "api.example.com");
        assert!(c.key.starts_with("https://api.example.com/a/b"));

        // trailing FQDN dot collapses to the same host
        let c2 = canonical_url("https://api.example.com./a/b").unwrap();
        assert_eq!(c2.host, "api.example.com");

        assert!(matches!(
            canonical_url("ftp://example.com/x"),
            Err(SecurityReject::Scheme(_))
        ));
        assert!(matches!(
            canonical_url("https://trusted@evil.com/x"),
            Err(SecurityReject::Userinfo)
        ));
    }

    #[test]
    fn manifest_matcher_rejects_f1_payloads() {
        let host = "api.meshrelay.xyz";
        let path = "/payments/access/";

        // legitimate
        let ok = Url::parse("https://api.meshrelay.xyz/payments/access/alpha-test").unwrap();
        assert!(match_manifest_prefix(&ok, host, path));

        // F1 impersonation payloads -> must NOT match
        for bad in [
            "https://api.meshrelay.xyz.evil.com/payments/access/x",
            "https://api.meshrelay.xyzevil.com/payments/access/x",
            "https://evil.com/payments/access/x",
            "http://api.meshrelay.xyz/payments/access/x", // wrong scheme
        ] {
            let u = Url::parse(bad).unwrap();
            assert!(
                !match_manifest_prefix(&u, host, path),
                "must not match: {bad}"
            );
        }

        // userinfo trick (host is really evil.com)
        let u = Url::parse("https://api.meshrelay.xyz@evil.com/payments/access/x").unwrap();
        assert!(!match_manifest_prefix(&u, host, path));

        // path boundary: /api must not match /api-evil
        let host2 = "svc.example.com";
        let u2 = Url::parse("https://svc.example.com/api-evil/x").unwrap();
        assert!(!match_manifest_prefix(&u2, host2, "/api/"));
        let u3 = Url::parse("https://svc.example.com/api/x").unwrap();
        assert!(match_manifest_prefix(&u3, host2, "/api/"));

        // no-trailing-slash manifest path matches a slash-delimited child
        let mcp = Url::parse("https://mcp.execution.market/mcp/tools").unwrap();
        assert!(match_manifest_prefix(&mcp, "mcp.execution.market", "/mcp"));
        let mcpevil = Url::parse("https://mcp.execution.market/mcp-evil").unwrap();
        assert!(!match_manifest_prefix(
            &mcpevil,
            "mcp.execution.market",
            "/mcp"
        ));
    }

    #[tokio::test]
    async fn check_url_target_rejects_ssrf_and_bad_ports() {
        // loopback literal + encoded loopback
        for bad in [
            "http://127.0.0.1/x",
            "http://2130706433/x",                      // decimal 127.0.0.1
            "http://0x7f000001/x",                      // hex
            "http://169.254.169.254/latest/meta-data/", // AWS metadata
        ] {
            let u = Url::parse(bad).unwrap();
            let r = check_url_target(&u).await;
            assert!(
                matches!(r, Err(SecurityReject::DisallowedAddress(_))),
                "must reject {bad}, got {r:?}"
            );
        }

        // disallowed port on a public-literal host
        let u = Url::parse("http://93.184.216.34:6379/x").unwrap();
        assert!(matches!(
            check_url_target(&u).await,
            Err(SecurityReject::Port(6379))
        ));

        // userinfo
        let u = Url::parse("https://a@93.184.216.34/x").unwrap();
        assert!(matches!(
            check_url_target(&u).await,
            Err(SecurityReject::Userinfo)
        ));

        // allowed public literal on 443 passes
        let u = Url::parse("https://93.184.216.34/x").unwrap();
        assert!(check_url_target(&u).await.is_ok());
    }

    #[test]
    fn aggregator_policy_builds() {
        // Smoke test: the policy constructs without panicking.
        let _ = aggregator_redirect_policy(3);
    }
}
