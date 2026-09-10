//! Curated tier manifest (WS-C) — makes our products first-class citizens.
//!
//! Loads `config/bazaar_curation.json` (override `BAZAAR_CURATION_PATH`) and
//! resolves the tier of each resource at read time. Matching is host-exact +
//! path-boundary on the PARSED URL via
//! [`crate::discovery_security::match_manifest_prefix`] — never a raw string
//! prefix, so `https://api.meshrelay.xyz.evil.com/` can never impersonate a
//! curated product (F1). Fail-open: a missing/invalid file yields an empty
//! manifest (no tiers, no suppression) so a config mistake can never hide the
//! whole bazaar.

use serde::Deserialize;
use url::Url;

use crate::discovery_security::match_manifest_prefix;
use crate::types_v2::{CurationInfo, Tier};

#[derive(Debug, Clone, Deserialize)]
struct Prefix {
    host: String,
    path: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Erc8004Ref {
    network: String,
    #[serde(rename = "agentId")]
    agent_id: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct ManifestEntry {
    name: String,
    tier: Tier,
    prefixes: Vec<Prefix>,
    #[serde(default)]
    erc8004: Option<Erc8004Ref>,
}

#[derive(Debug, Clone, Deserialize)]
struct SuppressEntry {
    host: String,
    path: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ManifestFile {
    #[serde(default)]
    entries: Vec<ManifestEntry>,
    #[serde(default)]
    suppressed: Vec<SuppressEntry>,
}

/// Loaded curation manifest.
pub struct CurationManifest {
    entries: Vec<ManifestEntry>,
    suppressed: Vec<SuppressEntry>,
}

impl Default for CurationManifest {
    fn default() -> Self {
        Self::empty()
    }
}

impl CurationManifest {
    /// Load from `BAZAAR_CURATION_PATH` (default `config/bazaar_curation.json`).
    pub fn load() -> Self {
        let path = std::env::var("BAZAAR_CURATION_PATH")
            .unwrap_or_else(|_| "config/bazaar_curation.json".to_string());
        match std::fs::read_to_string(&path) {
            Ok(raw) => match Self::parse(&raw) {
                Ok(f) => {
                    tracing::info!(
                        path = %path,
                        entries = f.entries.len(),
                        suppressed = f.suppressed.len(),
                        "Loaded bazaar curation manifest"
                    );
                    Self {
                        entries: f.entries,
                        suppressed: f.suppressed,
                    }
                }
                Err(e) => {
                    tracing::warn!(path = %path, error = %e, "Malformed curation manifest; no tiers");
                    Self::empty()
                }
            },
            Err(e) => {
                tracing::info!(path = %path, error = %e, "No curation manifest; no tiers");
                Self::empty()
            }
        }
    }

    /// The manifest as serde sees it. Split out of [`Self::load`] so a test can
    /// hand it the file that actually ships and get the same verdict the
    /// running facilitator gets, instead of a hand-built copy that can drift
    /// from it silently.
    fn parse(raw: &str) -> Result<ManifestFile, serde_json::Error> {
        serde_json::from_str::<ManifestFile>(raw)
    }

    fn empty() -> Self {
        Self {
            entries: Vec::new(),
            suppressed: Vec::new(),
        }
    }

    /// Whether the URL is manifest-suppressed (a permanent delist).
    pub fn is_suppressed(&self, url: &Url) -> bool {
        self.suppressed
            .iter()
            .any(|s| match_manifest_prefix(url, &s.host, &s.path))
    }

    /// Resolve the curation tier. A manifest match wins; otherwise a
    /// health-alive resource is `verified`; everything else is `listed`
    /// (returns `None` so the response omits the curation field).
    pub fn resolve(&self, url: &Url, alive: bool) -> Option<CurationInfo> {
        for e in &self.entries {
            for p in &e.prefixes {
                if match_manifest_prefix(url, &p.host, &p.path) {
                    return Some(CurationInfo {
                        tier: e.tier,
                        label: Some(e.name.clone()),
                        first_party: e.tier == Tier::FirstParty,
                        verification: None,
                    });
                }
            }
        }
        if alive {
            Some(CurationInfo {
                tier: Tier::Verified,
                label: None,
                first_party: false,
                verification: None,
            })
        } else {
            None
        }
    }

    /// Manifest entries that carry an ERC-8004 identity, as
    /// `(label, url_prefix, network_string, agent_id)` for the attestation task
    /// (WS-E). `label` is the entry name — the join key for the verification
    /// cache, since a synthesized URL would not match registry URL variants
    /// (e.g. a trailing slash). `url_prefix` is `https://{host}{path}` of the
    /// first prefix: the on-chain feedback `endpoint` and the health-uptime
    /// aggregation prefix.
    pub fn attest_targets(&self) -> Vec<(String, String, String, u64)> {
        self.entries
            .iter()
            .filter_map(|e| {
                let r = e.erc8004.as_ref()?;
                let p = e.prefixes.first()?;
                Some((
                    e.name.clone(),
                    format!("https://{}{}", p.host, p.path),
                    r.network.clone(),
                    r.agent_id,
                ))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The manifest the binary actually loads, embedded at compile time so the
    /// test does not depend on the working directory `cargo test` was run from.
    const SHIPPED: &str = include_str!("../config/bazaar_curation.json");

    fn shipped() -> CurationManifest {
        let f = CurationManifest::parse(SHIPPED)
            .expect("config/bazaar_curation.json does not parse as a manifest");
        CurationManifest {
            entries: f.entries,
            suppressed: f.suppressed,
        }
    }

    fn tier_of(m: &CurationManifest, url: &str) -> Option<Tier> {
        // alive=false: only a manifest hit can produce a tier, so nothing here
        // can be explained by the health fallback.
        m.resolve(&Url::parse(url).unwrap(), false).map(|c| c.tier)
    }

    fn manifest() -> CurationManifest {
        CurationManifest {
            entries: vec![ManifestEntry {
                name: "MeshRelay".to_string(),
                tier: Tier::FirstParty,
                prefixes: vec![Prefix {
                    host: "api.meshrelay.xyz".to_string(),
                    path: "/payments/access/".to_string(),
                }],
                erc8004: None,
            }],
            suppressed: vec![SuppressEntry {
                host: "facilitator.ultravioletadao.xyz".to_string(),
                path: "/__bazaar_debug__".to_string(),
            }],
        }
    }

    #[test]
    fn resolves_first_party_and_rejects_impersonators() {
        let m = manifest();
        let ours = Url::parse("https://api.meshrelay.xyz/payments/access/alpha-test").unwrap();
        let info = m.resolve(&ours, false).unwrap();
        assert_eq!(info.tier, Tier::FirstParty);
        assert!(info.first_party);

        // impersonation must NOT get the tier (falls through to verified/listed)
        let evil = Url::parse("https://api.meshrelay.xyz.evil.com/payments/access/x").unwrap();
        assert_eq!(m.resolve(&evil, true).unwrap().tier, Tier::Verified);
        assert!(m.resolve(&evil, false).is_none());
    }

    #[test]
    fn verified_when_alive_else_listed() {
        let m = manifest();
        let u = Url::parse("https://random.example/x").unwrap();
        assert_eq!(m.resolve(&u, true).unwrap().tier, Tier::Verified);
        assert!(m.resolve(&u, false).is_none());
    }

    #[test]
    fn suppression_matches_debug_entry() {
        let m = manifest();
        let dbg = Url::parse("https://facilitator.ultravioletadao.xyz/__bazaar_debug__").unwrap();
        assert!(m.is_suppressed(&dbg));
        let other = Url::parse("https://facilitator.ultravioletadao.xyz/health").unwrap();
        assert!(!m.is_suppressed(&other));
    }
    /// describe.net is a first-class citizen on the paths that take money, and
    /// on no others.
    ///
    /// Each URL below stands for a route measured as 402 on 2026-09-10 (see the
    /// entry's `$evidence`). `/leaderboard` is the interesting one: it answered
    /// 200, it is free, and the paid route is one segment inside it. Writing the
    /// manifest path as `/leaderboard` instead of `/leaderboard/page` awards the
    /// tier to a page nobody pays for -- checked by mutation, it turns this test
    /// red with `left: Some(FirstParty)`. `/leaderboard/` does not, because
    /// [`match_manifest_prefix`] only prefix-matches past the trailing slash;
    /// the pinned path is still the narrower of the two.
    #[test]
    fn describe_net_is_first_party_on_its_paid_routes_only() {
        let m = shipped();

        for paid in [
            "https://api.describe.net/reputation/wallet/0x0000000000000000000000000000000000000001",
            "https://api.describe.net/reputation/wallet/0x0000000000000000000000000000000000000001/history",
            "https://api.describe.net/reputation/rater/0x0000000000000000000000000000000000000001",
            "https://api.describe.net/reputation/agent/base/1",
            "https://api.describe.net/leaderboard/page",
            "https://api.describe.net/mcp",
        ] {
            assert_eq!(
                tier_of(&m, paid),
                Some(Tier::FirstParty),
                "{paid} lost its first_party tier"
            );
        }

        for free in [
            "https://api.describe.net/leaderboard",
            "https://api.describe.net/health",
            "https://api.describe.net/pricing",
            "https://api.describe.net/search/0x0000000000000000000000000000000000000001",
        ] {
            assert_eq!(
                tier_of(&m, free),
                None,
                "{free} is a free route and must not carry a curated tier"
            );
        }
    }

    /// The host is the whole claim, so a neighbour of it is not describe.net.
    ///
    /// `describe.net` itself is listed on purpose: the paid API lives on
    /// `api.describe.net`, and the apex only serves the site.
    #[test]
    fn nothing_that_merely_looks_like_describe_net_inherits_the_tier() {
        let m = shipped();
        for impostor in [
            "https://api.describe.net.evil.com/reputation/wallet/0x1",
            "https://api-describe.net/reputation/wallet/0x1",
            "https://describe.net/reputation/wallet/0x1",
            "http://api.describe.net/reputation/wallet/0x1",
            "https://user@api.describe.net/reputation/wallet/0x1",
        ] {
            assert_eq!(
                tier_of(&m, impostor),
                None,
                "{impostor} was awarded a curated tier"
            );
        }
    }

    /// Every `payTo` in this file is a measurement, and describe.net shares its
    /// with MeshRelay.
    ///
    /// The two entries carrying the same address is the thing that looks like a
    /// copy-paste bug and is not: it is the owner's shared collection wallet,
    /// probed live on describe.net's five paid routes on 2026-09-10. Pinning it
    /// means a future edit that changes either one has to re-probe and say so,
    /// which is exactly what the file's `$comment` asks for.
    #[test]
    fn the_shared_collection_wallet_stays_a_measured_value() {
        const SHARED: &str = "0xe4dc963c56979E0260fc146b87eE24F18220e545";
        let raw: serde_json::Value =
            serde_json::from_str(SHIPPED).expect("manifest is not valid JSON");
        let entries = raw["entries"].as_array().expect("no entries array");

        let pay_to = |name: &str| -> Vec<String> {
            let e = entries
                .iter()
                .find(|e| e["name"] == name)
                .unwrap_or_else(|| panic!("{name} is no longer in the manifest"));
            assert_eq!(e["tier"], "first_party", "{name} is no longer first_party");
            assert!(
                e["$evidence"].as_str().is_some_and(|s| !s.is_empty()),
                "{name} carries no $evidence; the file's $comment forbids that"
            );
            e["expectedPayTo"]
                .as_array()
                .unwrap_or_else(|| panic!("{name} declares no expectedPayTo"))
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect()
        };

        assert_eq!(pay_to("describe.net"), vec![SHARED.to_string()]);
        assert_eq!(pay_to("MeshRelay"), vec![SHARED.to_string()]);
    }

    /// The storefront and the manifest name the same first-class citizens.
    ///
    /// `static/bazaar.html` hardcodes its own showcase, so a product can be
    /// curated by the API and invisible on the page -- or listed on the page
    /// with no curated tier behind it. Either way the reader is told something
    /// the API will not confirm.
    #[test]
    fn the_bazaar_page_names_every_first_party_entry() {
        const PAGE: &str = include_str!("../static/bazaar.html");
        let raw: serde_json::Value = serde_json::from_str(SHIPPED).unwrap();
        for e in raw["entries"].as_array().unwrap() {
            if e["tier"] != "first_party" {
                continue;
            }
            let name = e["name"].as_str().unwrap();
            let homepage = e["homepage"].as_str().unwrap_or_default();
            assert!(
                PAGE.contains(name),
                "static/bazaar.html does not name the first_party entry {name}"
            );
            assert!(
                PAGE.contains(homepage),
                "static/bazaar.html does not link {name} at {homepage}"
            );
        }
    }
}
