//! Closed vocabulary for Bazaar listings: what KIND of thing a listing sells,
//! and which categories of ONE closed list it belongs to.
//!
//! # Why this exists
//!
//! On the 2026-10-01 catalog snapshot (2 000 listings) categories were
//! declared in 26 different spellings -- `data` and `Data`, `developer` and
//! `developer-tools`, `Inference` and `ai` -- in three different places:
//! `metadata.category`, the `bazaar` extension, and a `bazaar` object some
//! sellers put inside a payment option's `extra`. A router cannot filter on
//! that. And nothing separated a call to a tool from a paid essay, so the
//! essays under one publisher's prefix sat in a curated tier above every API.
//!
//! # How it resolves
//!
//! The vocabulary is `config/bazaar_taxonomy.json`, compiled in and defined
//! there once. Per listing, at read time and never stored, into three
//! response-only fields (`kind`, `categories`, `categorySource`) next to the
//! seller's own `metadata.category`, which is never touched -- other systems
//! decide on what the seller declared, and must keep reading exactly that.
//!
//! 1. An operator override whose prefix matches the URL decides first. Matching
//!    is [`match_manifest_prefix`], the same host-exact, path-boundary matcher
//!    the curation manifest uses -- never a string prefix. A category it
//!    assigns is `inferred`: nobody declared it, the operator curated it.
//! 2. Otherwise the seller's own declarations, in the order above, normalized
//!    (lowercase, `_` and spaces to `-`). Each that is a category, or a listed
//!    alias of one, contributes it once, in that order. `declared` when every
//!    one was the id itself, `normalized` when any needed another spelling. One
//!    that maps to nothing is left out: no category is ever guessed.
//! 3. `kind` is `content` when an override says so or when any declared value
//!    is a content spelling (`article`, `essay`, ...); otherwise `api`.
//!
//! Tags are not read. A tag is a seller's search hint, and the seller that
//! tags itself `market-data` turns out to sell data about the x402 market,
//! not about markets -- which is also why that spelling maps to nothing.
//!
//! Fail-open like the curation manifest: if the embedded file could not be
//! read every listing is an `api` with no category, which is what the catalog
//! said before this module existed. The shipped file is checked strictly by
//! the tests below, so that branch is for a broken build, not a bad edit.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::discovery_security::match_manifest_prefix;
use crate::types_v2::DiscoveryResource;

/// The vocabulary, as shipped.
const SHIPPED: &str = include_str!("../config/bazaar_taxonomy.json");

/// What a listing sells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A paid call to a tool.
    #[default]
    Api,
    /// A paid piece of content, the same for every buyer.
    Content,
}

impl Kind {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Api => "api",
            Kind::Content => "content",
        }
    }
}

/// How a listing's categories were obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CategorySource {
    /// Every one is a value the seller declared, spelled exactly as the id.
    Declared,
    /// From what the seller declared, through another spelling of the same id
    /// (`Data`, `data_processing`, `twitter`).
    Normalized,
    /// The seller declared nothing usable; the operator's curation assigned
    /// it, with its evidence in the vocabulary file.
    Inferred,
}

/// One listing's resolved place in the vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification<'a> {
    pub kind: Kind,
    /// Keys of the closed list, in the order they were found, without repeats.
    /// Empty when nothing maps.
    pub categories: Vec<&'a str>,
    /// How `categories` were obtained; `None` exactly when it is empty.
    pub source: Option<CategorySource>,
}

#[derive(Debug, Clone, Deserialize)]
struct OverrideEntry {
    host: String,
    path: String,
    #[serde(default)]
    kind: Option<Kind>,
    #[serde(default)]
    category: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaxonomyFile {
    kinds: BTreeMap<String, String>,
    categories: BTreeMap<String, String>,
    #[serde(default)]
    aliases: BTreeMap<String, String>,
    #[serde(default)]
    content_categories: Vec<String>,
    #[serde(default)]
    overrides: Vec<OverrideEntry>,
}

/// The loaded vocabulary.
pub struct Taxonomy {
    /// Canonical id -> what it covers.
    categories: BTreeMap<String, String>,
    /// Normalized spelling -> canonical id. Always points into `categories`.
    aliases: HashMap<String, String>,
    /// Normalized spellings that mean the listing sells content.
    content: HashSet<String>,
    overrides: Vec<OverrideEntry>,
}

/// Lowercase, trim, and turn `_`, `-` and whitespace runs into one `-`, with
/// none around a `/`. `" Social / X "` and `"social/x"` are one spelling;
/// `"data_processing"` and `"Data Processing"` are `"data-processing"`.
pub fn normalize_label(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut pending_dash = false;
    for ch in raw.trim().chars().flat_map(char::to_lowercase) {
        if ch == '_' || ch == '-' || ch.is_whitespace() {
            pending_dash = !out.is_empty();
            continue;
        }
        if ch == '/' {
            pending_dash = false;
            out.push('/');
            continue;
        }
        if pending_dash && !out.ends_with('/') {
            out.push('-');
        }
        pending_dash = false;
        out.push(ch);
    }
    out
}

impl Taxonomy {
    /// Parse a vocabulary file, refusing any inconsistency: an alias or an
    /// override naming a category that is not in the list, an alias that
    /// shadows a category, a kind the file does not describe, an override that
    /// declares nothing. The tests hold the shipped file to this.
    fn parse_strict(raw: &str) -> Result<Self, String> {
        let file: TaxonomyFile = serde_json::from_str(raw).map_err(|e| e.to_string())?;

        for kind in [Kind::Api, Kind::Content] {
            if !file.kinds.contains_key(kind.as_str()) {
                return Err(format!("kind {:?} is not described", kind.as_str()));
            }
        }
        if file.kinds.len() != 2 {
            return Err("kinds must be exactly api and content".to_string());
        }

        let mut categories = BTreeMap::new();
        for (id, about) in file.categories {
            if normalize_label(&id) != id {
                return Err(format!("category {id:?} is not in normalized form"));
            }
            if about.trim().is_empty() {
                return Err(format!("category {id:?} is not described"));
            }
            categories.insert(id, about);
        }

        let mut aliases = HashMap::new();
        for (spelling, target) in file.aliases {
            let key = normalize_label(&spelling);
            if !categories.contains_key(&target) {
                return Err(format!(
                    "alias {spelling:?} points at {target:?}, which is not a category"
                ));
            }
            if categories.contains_key(&key) {
                return Err(format!("alias {spelling:?} shadows a category"));
            }
            if aliases.insert(key, target).is_some() {
                return Err(format!("alias {spelling:?} is declared twice"));
            }
        }

        let mut content = HashSet::new();
        for spelling in file.content_categories {
            let key = normalize_label(&spelling);
            if categories.contains_key(&key) || aliases.contains_key(&key) {
                return Err(format!(
                    "content spelling {spelling:?} is also a category or an alias"
                ));
            }
            content.insert(key);
        }

        for o in &file.overrides {
            if o.kind.is_none() && o.category.is_none() {
                return Err(format!("override {}{} declares nothing", o.host, o.path));
            }
            if let Some(c) = &o.category {
                if !categories.contains_key(c) {
                    return Err(format!(
                        "override {}{} names {c:?}, which is not a category",
                        o.host, o.path
                    ));
                }
            }
        }

        Ok(Self {
            categories,
            aliases,
            content,
            overrides: file.overrides,
        })
    }

    fn empty() -> Self {
        Self {
            categories: BTreeMap::new(),
            aliases: HashMap::new(),
            content: HashSet::new(),
            overrides: Vec::new(),
        }
    }

    /// The closed list: `(id, what it covers)`, sorted by id.
    // Read by the tests that tie the docs to the list; the binary has no caller.
    #[allow(dead_code)]
    pub fn categories(&self) -> impl Iterator<Item = (&str, &str)> {
        self.categories
            .iter()
            .map(|(id, about)| (id.as_str(), about.as_str()))
    }

    /// The category a spelling names, if it names one.
    pub fn canonical(&self, raw: &str) -> Option<&str> {
        let key = normalize_label(raw);
        if let Some((id, _)) = self.categories.get_key_value(&key) {
            return Some(id.as_str());
        }
        self.aliases.get(&key).map(String::as_str)
    }

    /// Whether a spelling says the listing sells content.
    pub fn is_content(&self, raw: &str) -> bool {
        self.content.contains(&normalize_label(raw))
    }

    /// Resolve one listing.
    pub fn classify(&self, r: &DiscoveryResource) -> Classification<'_> {
        let mut kind = None;
        let mut assigned = None;
        for o in &self.overrides {
            if match_manifest_prefix(&r.url, &o.host, &o.path) {
                kind = o.kind;
                assigned = o
                    .category
                    .as_deref()
                    .and_then(|c| self.categories.get_key_value(c))
                    .map(|(id, _)| id.as_str());
                break;
            }
        }

        let declared = declared_categories(r);
        let kind = kind.unwrap_or_else(|| {
            if declared.iter().any(|raw| self.is_content(raw)) {
                Kind::Content
            } else {
                Kind::Api
            }
        });

        if let Some(id) = assigned {
            return Classification {
                kind,
                categories: vec![id],
                source: Some(CategorySource::Inferred),
            };
        }

        let mut categories = Vec::new();
        let mut verbatim = true;
        for raw in declared {
            if let Some(id) = self.canonical(raw) {
                verbatim &= raw == id;
                if !categories.contains(&id) {
                    categories.push(id);
                }
            }
        }
        let source = (!categories.is_empty()).then_some(if verbatim {
            CategorySource::Declared
        } else {
            CategorySource::Normalized
        });
        Classification {
            kind,
            categories,
            source,
        }
    }
}

/// What the seller declared as a category, in the order it is trusted: the
/// listing's metadata, then the `bazaar` extension, then a `bazaar` object
/// inside the first payment option's `extra` that carries one.
fn declared_categories(r: &DiscoveryResource) -> Vec<&str> {
    let mut out = Vec::new();
    if let Some(c) = r.metadata.as_ref().and_then(|m| m.category.as_deref()) {
        out.push(c);
    }
    if let Some(c) = r
        .extensions
        .as_ref()
        .and_then(|e| e.get("bazaar"))
        .and_then(|b| b.get("category"))
        .and_then(serde_json::Value::as_str)
    {
        out.push(c);
    }
    if let Some(c) = r.accepts.iter().find_map(|a| {
        a.extra
            .as_ref()
            .and_then(|e| e.get("bazaar"))
            .and_then(|b| b.get("category"))
            .and_then(serde_json::Value::as_str)
    }) {
        out.push(c);
    }
    out
}

/// The shipped vocabulary.
pub fn taxonomy() -> &'static Taxonomy {
    static LOADED: OnceLock<Taxonomy> = OnceLock::new();
    LOADED.get_or_init(|| match Taxonomy::parse_strict(SHIPPED) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(
                error = %e,
                "config/bazaar_taxonomy.json is inconsistent; every listing is an api with no category"
            );
            Taxonomy::empty()
        }
    })
}

/// Resolve one listing against the shipped vocabulary.
pub fn classify(r: &DiscoveryResource) -> Classification<'static> {
    taxonomy().classify(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caip2::Caip2NetworkId;
    use crate::types::{MixedAddress, Scheme, TokenAmount};
    use crate::types_v2::{DiscoveryMetadata, PaymentRequirementsV2};

    fn listing(url: &str) -> DiscoveryResource {
        let accepts = vec![PaymentRequirementsV2 {
            scheme: Scheme::Exact,
            network: Caip2NetworkId::eip155(8453),
            asset: MixedAddress::Evm(
                "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"
                    .parse()
                    .unwrap(),
            ),
            amount: TokenAmount::from(10_000u64),
            pay_to: MixedAddress::Evm(
                "0x1234567890123456789012345678901234567890"
                    .parse()
                    .unwrap(),
            ),
            max_timeout_seconds: 300,
            extra: None,
        }
        .into()];
        DiscoveryResource::new(
            url::Url::parse(url).unwrap(),
            "http".to_string(),
            "a listing".to_string(),
            accepts,
        )
    }

    fn with_category(url: &str, category: &str) -> DiscoveryResource {
        let mut r = listing(url);
        r.metadata = Some(DiscoveryMetadata {
            category: Some(category.to_string()),
            provider: None,
            tags: Vec::new(),
        });
        r
    }

    #[test]
    fn the_shipped_vocabulary_is_consistent() {
        let t = Taxonomy::parse_strict(SHIPPED).expect("config/bazaar_taxonomy.json");
        // The ten the router feedback asked for are all in it.
        for id in [
            "people",
            "company",
            "web-search",
            "page-read",
            "social/x",
            "social/reddit",
            "finance",
            "crypto",
            "weather",
            "image",
        ] {
            assert!(
                t.categories().any(|(c, _)| c == id),
                "{id} is missing from the closed list"
            );
        }
        // And what the house already sells: work done by people.
        assert!(t.categories().any(|(c, _)| c == "human-work"));
        // A category that returns personal data says so.
        let (_, people) = t.categories().find(|(c, _)| *c == "people").unwrap();
        assert!(people.contains("personal data"), "{people}");
    }

    /// The list is typed once, in the JSON; the API docs and the plan repeat it
    /// for readers and must not drift from it.
    #[test]
    fn the_docs_name_exactly_the_closed_list() {
        const OPENAPI: &str = include_str!("openapi.rs");
        const PLAN: &str = include_str!("../docs/plans/bazaar/10-listing-data-and-taxonomy.md");
        let ids: Vec<&str> = taxonomy().categories().map(|(id, _)| id).collect();

        let start = OPENAPI
            .find("lists ids from one closed list:")
            .expect("the /discovery/resources docs state the closed list");
        let sentence = &OPENAPI[start..];
        let sentence = &sentence[..sentence.find(". They come from").expect("sentence end")];
        let mut documented: Vec<&str> = sentence.split('`').skip(1).step_by(2).collect();
        documented.sort_unstable();
        // `ids` is already sorted (a BTreeMap); a duplicate would show as a
        // length mismatch.
        assert_eq!(
            documented, ids,
            "the /discovery/resources docs list a different closed list than config/bazaar_taxonomy.json"
        );

        for id in ids {
            assert!(
                PLAN.contains(&format!("| `{id}` |")),
                "docs/plans/bazaar/10 does not describe {id}"
            );
        }
    }

    #[test]
    fn spellings_normalize_to_one_form() {
        for (raw, want) in [
            ("Data", "data"),
            ("  data  ", "data"),
            ("data_processing", "data-processing"),
            ("Data Processing", "data-processing"),
            ("web--search", "web-search"),
            (" Social / X ", "social/x"),
            ("social -/- reddit", "social/reddit"),
            ("-x-", "x"),
            ("", ""),
            ("INFERENCE", "inference"),
        ] {
            assert_eq!(normalize_label(raw), want, "{raw:?}");
        }
    }

    #[test]
    fn every_spelling_measured_in_the_catalog_lands_or_stays_unmapped_on_purpose() {
        // The 26 spellings declared on the 2026-10-01 snapshot, in any of the
        // three places, plus the three new ones of 2026-10-02.
        let t = taxonomy();
        for (raw, want) in [
            ("data", Some("data")),
            ("Data", Some("data")),
            ("data_processing", Some("data")),
            ("data-enrichment", Some("data")),
            ("finance", Some("finance")),
            ("crypto", Some("crypto")),
            // Seen on the 2026-10-02 snapshot.
            ("chain-data", Some("crypto")),
            ("security", Some("security")),
            ("research", Some("research")),
            ("reputation", Some("reputation")),
            ("communication", Some("communication")),
            ("payroll-compliance", Some("compliance")),
            ("developer-tools", Some("developer-tools")),
            ("developer", Some("developer-tools")),
            ("advertising", Some("advertising")),
            ("infrastructure", Some("infrastructure")),
            ("payment-facilitator", Some("infrastructure")),
            ("ai", Some("ai")),
            ("Inference", Some("ai")),
            ("search", Some("web-search")),
            // Too vague to place without guessing what the seller meant.
            // `market-data`: its main seller sells data about the x402
            // market, the other one a prediction market. Neither is `finance`.
            ("market-data", None),
            ("marketing", None),
            ("utility", None),
            ("web", None),
            ("verification", None),
            ("mcp", None),
            ("test", None),
            ("sustainability", None),
            ("execution", None),
        ] {
            assert_eq!(t.canonical(raw), want, "{raw:?}");
        }
    }

    #[test]
    fn an_essay_publisher_is_content_by_override_and_an_impostor_is_not() {
        let essay = listing("https://tenjin.blog/api/read/eth-rd/some-essay");
        let c = classify(&essay);
        assert_eq!(c.kind, Kind::Content);
        assert!(
            c.categories.is_empty() && c.source.is_none(),
            "the override sets the kind, not a category"
        );

        for other in [
            // Same host, outside the prefix.
            "https://tenjin.blog/api/readers",
            // Lookalike hosts.
            "https://tenjin.blog.evil.example/api/read/x",
            "https://api.tenjin.blog/api/read/x",
            // Plain http is not the matcher's https.
            "http://tenjin.blog/api/read/x",
        ] {
            assert_eq!(classify(&listing(other)).kind, Kind::Api, "{other}");
        }
    }

    #[test]
    fn a_seller_that_declares_content_is_content() {
        for raw in ["article", "Essays", "blog_post", "Newsletter"] {
            let r = with_category("https://writer.example.com/p/1", raw);
            assert_eq!(classify(&r).kind, Kind::Content, "{raw}");
            assert!(classify(&r).categories.is_empty(), "{raw}");
        }
        let tool = with_category("https://tool.example.com/x", "finance");
        assert_eq!(
            classify(&tool),
            Classification {
                kind: Kind::Api,
                categories: vec!["finance"],
                source: Some(CategorySource::Declared),
            }
        );
    }

    #[test]
    fn every_declaration_that_maps_contributes_once_in_order() {
        let of = |r: &DiscoveryResource| {
            let c = classify(r);
            (c.categories, c.source)
        };

        // metadata unmapped, extension mapped: the extension supplies it, and
        // through another spelling.
        let mut r = with_category("https://a.example.com/x", "test");
        r.extensions = Some(serde_json::json!({ "bazaar": { "category": "Inference" } }));
        assert_eq!(of(&r), (vec!["ai"], Some(CategorySource::Normalized)));

        // Both map: both, in the order they are trusted.
        let mut r = with_category("https://a.example.com/x", "Data");
        r.extensions = Some(serde_json::json!({ "bazaar": { "category": "search" } }));
        assert_eq!(
            of(&r),
            (vec!["data", "web-search"], Some(CategorySource::Normalized))
        );

        // Declared twice in the exact spelling: once, and `declared`.
        let mut r = with_category("https://a.example.com/x", "data");
        r.extensions = Some(serde_json::json!({ "bazaar": { "category": "data" } }));
        assert_eq!(of(&r), (vec!["data"], Some(CategorySource::Declared)));

        // One exact and one respelled: the list is `normalized`.
        let mut r = with_category("https://a.example.com/x", "finance");
        r.extensions = Some(serde_json::json!({ "bazaar": { "category": "Stocks" } }));
        assert_eq!(of(&r), (vec!["finance"], Some(CategorySource::Normalized)));

        // Only the option's extra declares one.
        let mut r = listing("https://a.example.com/x");
        r.accepts[0].extra = Some(serde_json::json!({
            "name": "USD Coin",
            "version": "2",
            "bazaar": { "category": "chain-data", "tags": ["x402"] }
        }));
        assert_eq!(of(&r), (vec!["crypto"], Some(CategorySource::Normalized)));

        // Nothing declared: api, no category. Never guessed from the URL.
        let r = listing("https://weather.example.com/forecast");
        assert_eq!(
            classify(&r),
            Classification {
                kind: Kind::Api,
                categories: Vec::new(),
                source: None,
            }
        );
    }

    #[test]
    fn work_done_by_people_is_assigned_by_the_operator_and_says_so() {
        // The listing declares `execution`, the product's name, which maps to
        // nothing; the operator's override places it, and it reads `inferred`.
        for url in [
            "https://mcp.execution.market/mcp",
            "https://api.execution.market/api/v1/tasks/123",
        ] {
            let r = with_category(url, "execution");
            assert_eq!(
                classify(&r),
                Classification {
                    kind: Kind::Api,
                    categories: vec!["human-work"],
                    source: Some(CategorySource::Inferred),
                },
                "{url}"
            );
        }
        for impostor in [
            "https://mcp.execution.market.evil.example/mcp",
            "https://mcp.execution.market/mcp-evil",
            "https://api.execution.market/api/v1/taskforce",
        ] {
            assert!(
                classify(&listing(impostor)).categories.is_empty(),
                "{impostor}"
            );
        }
    }

    #[test]
    fn tags_are_not_read() {
        let mut r = listing("https://a.example.com/x");
        r.metadata = Some(DiscoveryMetadata {
            category: None,
            provider: None,
            tags: vec!["essay".to_string(), "weather".to_string()],
        });
        assert_eq!(
            classify(&r),
            Classification {
                kind: Kind::Api,
                categories: Vec::new(),
                source: None,
            }
        );
    }

    #[test]
    fn an_inconsistent_file_is_refused() {
        let base = |aliases: &str, overrides: &str| {
            format!(
                r#"{{"kinds":{{"api":"a","content":"c"}},
                    "categories":{{"finance":"money","social/x":"x"}},
                    "aliases":{aliases},
                    "contentCategories":["essay"],
                    "overrides":{overrides}}}"#
            )
        };
        assert!(Taxonomy::parse_strict(&base("{}", "[]")).is_ok());
        // An alias to something that is not a category.
        assert!(Taxonomy::parse_strict(&base(r#"{"stocks":"stocks"}"#, "[]")).is_err());
        // An alias that shadows a category.
        assert!(Taxonomy::parse_strict(&base(r#"{"Finance":"finance"}"#, "[]")).is_err());
        // An override naming an unknown category, and one declaring nothing.
        assert!(Taxonomy::parse_strict(&base(
            "{}",
            r#"[{"host":"a.example","path":"/","category":"essays"}]"#
        ))
        .is_err());
        assert!(
            Taxonomy::parse_strict(&base("{}", r#"[{"host":"a.example","path":"/"}]"#)).is_err()
        );
        // A category id that is not in normalized form.
        let raw = r#"{"kinds":{"api":"a","content":"c"},"categories":{"Finance":"money"}}"#;
        assert!(Taxonomy::parse_strict(raw).is_err());
    }
}
