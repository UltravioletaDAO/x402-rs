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
struct ManifestEntry {
    name: String,
    tier: Tier,
    prefixes: Vec<Prefix>,
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
            Ok(raw) => match serde_json::from_str::<ManifestFile>(&raw) {
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
                    });
                }
            }
        }
        if alive {
            Some(CurationInfo {
                tier: Tier::Verified,
                label: None,
                first_party: false,
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> CurationManifest {
        CurationManifest {
            entries: vec![ManifestEntry {
                name: "MeshRelay".to_string(),
                tier: Tier::FirstParty,
                prefixes: vec![Prefix {
                    host: "api.meshrelay.xyz".to_string(),
                    path: "/payments/access/".to_string(),
                }],
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
}
