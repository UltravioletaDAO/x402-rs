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
//! 4. Only when the seller declared NO category at all, and the listing is not
//!    content: the inference rules of the vocabulary file, over what the
//!    listing says about itself -- host, path, description and the declared
//!    schema's field names and descriptions -- also `inferred`. See
//!    [`Inference`]. A declaration, even one that maps to nothing, is never
//!    second-guessed: the seller spoke, and what it said is normalized or left
//!    out, not reinterpreted.
//!
//! Tags are not read. A tag is a seller's search hint, and the seller that
//! tags itself `market-data` turns out to sell data about the x402 market,
//! not about markets -- which is also why that spelling maps to nothing.
//!
//! # Upstream
//!
//! The same file names, in a second closed list, the services listings resell
//! or wrap (Exa, Tavily, Firecrawl...), so a router can tell ten resellers of
//! one search API from ten search APIs. Resolved here too ([`Taxonomy::upstream`]):
//! what the seller declared, else what its host, path or description names.
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
    /// Nobody declared it: the seller declared nothing usable, and the
    /// operator's curation (an override, with its evidence) or, when the seller
    /// declared nothing at all, the inference rules of the vocabulary file
    /// placed it.
    Inferred,
}

/// How a listing's `upstream` was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamSource {
    /// The seller named it (`metadata.upstream` or `extensions.bazaar.upstream`).
    Declared,
    /// Its host, a path segment or its description names it.
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
struct RuleEntry {
    category: String,
    #[serde(default)]
    fallback: bool,
    #[serde(default)]
    strong: Vec<String>,
    #[serde(default)]
    weak: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InferenceFile {
    threshold: u32,
    max_categories: usize,
    rules: Vec<RuleEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct UpstreamEntry {
    id: String,
    name: String,
    #[serde(default)]
    hosts: Vec<String>,
    #[serde(default)]
    segments: Vec<String>,
    #[serde(default)]
    phrases: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct UpstreamsFile {
    entries: Vec<UpstreamEntry>,
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
    #[serde(default)]
    inference: Option<InferenceFile>,
    #[serde(default)]
    upstreams: Option<UpstreamsFile>,
}

/// A phrase of the inference rules or the upstream list, as search terms.
type Phrase = Vec<String>;

/// Points a distinct `strong` phrase of a rule scores.
const STRONG_POINTS: u32 = 2;
/// Points a distinct `weak` phrase of a rule scores.
const WEAK_POINTS: u32 = 1;

/// One inference rule, compiled.
struct Rule {
    /// Canonical id, always a key of `Taxonomy::categories`.
    category: String,
    /// Assigned only when no other rule reaches the threshold (`data`).
    fallback: bool,
    strong: Vec<Phrase>,
    weak: Vec<Phrase>,
}

/// The category inference rules: deterministic phrase rules over what a listing
/// that declares no category says about itself.
///
/// # Why rules and not a model
///
/// On 2026-10-06 (2.48.0, 1 765 listings) 200 listings resolved to a category,
/// all from the seller's own declaration: a router filtering by category saw a
/// ninth of the catalog. The rest says what it does in its URL and description
/// (`/twitter/search`, "Seven-day forecast with hourly temperature") and in the
/// field names of its schema. Phrases in a reviewed file, scored the same way
/// on every replica, every build, every request: a wrong placement is a line to
/// read and change, not a weight to retrain.
///
/// # How a listing is scored
///
/// Each field -- host, path, description, the declared schema's text -- is
/// split into search terms ([`crate::discovery_search::tokenize_into`]); a
/// phrase matches as consecutive terms of ONE field. A rule scores
/// [`STRONG_POINTS`] per distinct strong phrase it matched anywhere and
/// [`WEAK_POINTS`] per distinct weak one, so a word repeated in three fields
/// counts once. Every rule at or above the threshold is a candidate: the
/// non-fallback ones if any, else the fallback ones; best score first, ties in
/// file order; at most `max_categories`.
pub struct Inference {
    threshold: u32,
    max_categories: usize,
    rules: Vec<Rule>,
    /// First term of a phrase -> `(rule, strong, phrase)` it starts.
    by_first: HashMap<String, Vec<(usize, bool, usize)>>,
}

/// One upstream of the closed list, compiled.
struct Upstream {
    id: String,
    /// The id and the name, each normalized ([`normalize_label`]): what a
    /// declaration may spell.
    spellings: Vec<String>,
    /// The vendor's own domains, lowercase.
    hosts: Vec<String>,
    /// Exact path segments or host labels, lowercase.
    segments: Vec<String>,
    /// Phrases of the description.
    phrases: Vec<Phrase>,
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
    inference: Option<Inference>,
    upstreams: Vec<Upstream>,
}

/// `text` as search terms.
fn terms_of(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    crate::discovery_search::tokenize_into(text, &mut out);
    out
}

/// What a listing says about itself, as search terms, one list per field:
/// host, path, description, declared schema. Never tags or provider.
fn listing_fields(r: &DiscoveryResource) -> [Vec<String>; 4] {
    use crate::discovery_search::{path_text, schema_text};
    [
        terms_of(r.url.host_str().unwrap_or_default()),
        terms_of(&path_text(r.url.path())),
        terms_of(&r.description),
        r.extensions
            .as_ref()
            .map(|e| terms_of(&schema_text(e)))
            .unwrap_or_default(),
    ]
}

/// Whether `phrase` occurs as consecutive terms of `field` starting at `at`.
fn phrase_at(field: &[String], at: usize, phrase: &[String]) -> bool {
    field
        .get(at..at + phrase.len())
        .is_some_and(|window| window == phrase)
}

/// Whether `phrase` occurs as consecutive terms of `field`.
fn contains_phrase(field: &[String], phrase: &[String]) -> bool {
    !phrase.is_empty() && (0..field.len()).any(|at| phrase_at(field, at, phrase))
}

impl Inference {
    /// Whether `id` is a fallback category of these rules.
    fn is_fallback(&self, id: &str) -> bool {
        self.rules.iter().any(|r| r.fallback && r.category == id)
    }

    /// The categories `fields` place a listing in, best first.
    fn infer<'a>(&self, fields: &[Vec<String>], ids: &'a BTreeMap<String, String>) -> Vec<&'a str> {
        let mut matched: HashSet<(usize, bool, usize)> = HashSet::new();
        for field in fields {
            for (at, term) in field.iter().enumerate() {
                for &(rule, strong, phrase) in self.by_first.get(term).into_iter().flatten() {
                    let rule_ref = &self.rules[rule];
                    let words = if strong {
                        &rule_ref.strong[phrase]
                    } else {
                        &rule_ref.weak[phrase]
                    };
                    if phrase_at(field, at, words) {
                        matched.insert((rule, strong, phrase));
                    }
                }
            }
        }
        let mut score = vec![0u32; self.rules.len()];
        for (rule, strong, _) in matched {
            score[rule] += if strong { STRONG_POINTS } else { WEAK_POINTS };
        }
        let reached = |fallback: bool| -> Vec<usize> {
            (0..self.rules.len())
                .filter(|&i| score[i] >= self.threshold && self.rules[i].fallback == fallback)
                .collect()
        };
        let mut chosen = reached(false);
        if chosen.is_empty() {
            chosen = reached(true);
        }
        // Stable: equal scores keep file order.
        chosen.sort_by(|a, b| score[*b].cmp(&score[*a]));
        chosen
            .into_iter()
            .take(self.max_categories)
            .filter_map(|i| {
                ids.get_key_value(&self.rules[i].category)
                    .map(|(id, _)| id.as_str())
            })
            .collect()
    }
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
        // One bit each in a request's and a listing's category set.
        if categories.len() > 64 {
            return Err("the closed list holds more than 64 categories".to_string());
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

        let inference = file
            .inference
            .map(|i| Self::compile_inference(i, &categories))
            .transpose()?;
        let upstreams = file
            .upstreams
            .map(|u| Self::compile_upstreams(u.entries))
            .transpose()?
            .unwrap_or_default();

        Ok(Self {
            categories,
            aliases,
            content,
            overrides: file.overrides,
            inference,
            upstreams,
        })
    }

    /// A phrase of the file as search terms, refusing one that normalizes to
    /// nothing (only stopwords): it would be a rule that can never match.
    fn compile_phrase(raw: &str, owner: &str) -> Result<Phrase, String> {
        let terms = terms_of(raw);
        if terms.is_empty() {
            return Err(format!("{owner}: phrase {raw:?} has no searchable word"));
        }
        Ok(terms)
    }

    fn compile_inference(
        file: InferenceFile,
        categories: &BTreeMap<String, String>,
    ) -> Result<Inference, String> {
        if file.threshold == 0 {
            return Err("inference threshold must be at least 1".to_string());
        }
        if file.max_categories == 0 {
            return Err("inference maxCategories must be at least 1".to_string());
        }
        let mut rules = Vec::new();
        let mut by_first: HashMap<String, Vec<(usize, bool, usize)>> = HashMap::new();
        let mut seen_categories = HashSet::new();
        for entry in file.rules {
            let owner = format!("inference rule {:?}", entry.category);
            if !categories.contains_key(&entry.category) {
                return Err(format!("{owner} names a category that is not in the list"));
            }
            if !seen_categories.insert(entry.category.clone()) {
                return Err(format!("{owner} is declared twice"));
            }
            if entry.strong.is_empty() {
                return Err(format!("{owner} has no strong phrase"));
            }
            let index = rules.len();
            let mut compiled: [Vec<Phrase>; 2] = [Vec::new(), Vec::new()];
            let mut seen = HashSet::new();
            for (slot, strong, raw_list) in [(0, true, &entry.strong), (1, false, &entry.weak)] {
                for raw in raw_list {
                    let phrase = Self::compile_phrase(raw, &owner)?;
                    if !seen.insert(phrase.clone()) {
                        return Err(format!("{owner}: phrase {raw:?} is listed twice"));
                    }
                    by_first.entry(phrase[0].clone()).or_default().push((
                        index,
                        strong,
                        compiled[slot].len(),
                    ));
                    compiled[slot].push(phrase);
                }
            }
            let [strong, weak] = compiled;
            rules.push(Rule {
                category: entry.category,
                fallback: entry.fallback,
                strong,
                weak,
            });
        }
        Ok(Inference {
            threshold: file.threshold,
            max_categories: file.max_categories,
            rules,
            by_first,
        })
    }

    fn compile_upstreams(entries: Vec<UpstreamEntry>) -> Result<Vec<Upstream>, String> {
        let mut out: Vec<Upstream> = Vec::new();
        let mut spelled: HashSet<String> = HashSet::new();
        for e in entries {
            let owner = format!("upstream {:?}", e.id);
            if normalize_label(&e.id) != e.id || e.id.is_empty() || e.id.contains('/') {
                return Err(format!("{owner} is not a normalized id"));
            }
            if e.name.trim().is_empty() {
                return Err(format!("{owner} has no name"));
            }
            if e.hosts.is_empty() && e.segments.is_empty() && e.phrases.is_empty() {
                return Err(format!("{owner} can never be inferred"));
            }
            let mut spellings = vec![e.id.clone()];
            let name = normalize_label(&e.name);
            if name != e.id {
                spellings.push(name);
            }
            for s in &spellings {
                if !spelled.insert(s.clone()) {
                    return Err(format!("{owner}: {s:?} names another upstream too"));
                }
            }
            let mut hosts = Vec::new();
            for h in &e.hosts {
                let host = h.trim().trim_end_matches('.').to_ascii_lowercase();
                if host.is_empty() || !host.contains('.') || host.contains(['/', ':', '@', ' ']) {
                    return Err(format!("{owner}: {h:?} is not a host name"));
                }
                hosts.push(host);
            }
            let mut segments = Vec::new();
            for s in &e.segments {
                let segment = s.trim().to_ascii_lowercase();
                if segment.is_empty() || segment.contains(['/', '.', ' ']) {
                    return Err(format!("{owner}: {s:?} is not one path segment"));
                }
                segments.push(segment);
            }
            let phrases = e
                .phrases
                .iter()
                .map(|p| Self::compile_phrase(p, &owner))
                .collect::<Result<Vec<_>, _>>()?;
            out.push(Upstream {
                id: e.id,
                spellings,
                hosts,
                segments,
                phrases,
            });
        }
        Ok(out)
    }

    fn empty() -> Self {
        Self {
            categories: BTreeMap::new(),
            aliases: HashMap::new(),
            content: HashSet::new(),
            overrides: Vec::new(),
            inference: None,
            upstreams: Vec::new(),
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

    /// The first operator override whose prefix matches the listing's URL.
    fn override_for(&self, r: &DiscoveryResource) -> Option<&OverrideEntry> {
        self.overrides
            .iter()
            .find(|o| match_manifest_prefix(&r.url, &o.host, &o.path))
    }

    /// What a listing sells: `content` when an override says so or a value the
    /// seller declared is a content spelling, else `api`. Reads no text, so a
    /// filter can ask it of every listing; [`Self::classify`] serves the same
    /// value.
    pub fn kind(&self, r: &DiscoveryResource) -> Kind {
        if let Some(kind) = self.override_for(r).and_then(|o| o.kind) {
            return kind;
        }
        if declared_categories(r)
            .iter()
            .any(|raw| self.is_content(raw))
        {
            Kind::Content
        } else {
            Kind::Api
        }
    }

    /// Resolve one listing.
    pub fn classify(&self, r: &DiscoveryResource) -> Classification<'_> {
        let kind = self.kind(r);
        if let Some(id) = self
            .override_for(r)
            .and_then(|o| o.category.as_deref())
            .and_then(|c| self.categories.get_key_value(c))
            .map(|(id, _)| id.as_str())
        {
            return Classification {
                kind,
                categories: vec![id],
                source: Some(CategorySource::Inferred),
            };
        }

        let declared = declared_categories(r);
        let mut categories = Vec::new();
        let mut verbatim = true;
        for raw in &declared {
            if let Some(id) = self.canonical(raw) {
                verbatim &= *raw == id;
                if !categories.contains(&id) {
                    categories.push(id);
                }
            }
        }
        if !categories.is_empty() {
            return Classification {
                kind,
                categories,
                source: Some(if verbatim {
                    CategorySource::Declared
                } else {
                    CategorySource::Normalized
                }),
            };
        }

        // Silence only: a declaration that maps to nothing is the seller's
        // word, left out rather than overruled, and content is never placed in
        // a category of tools.
        if declared.is_empty() && kind == Kind::Api {
            if let Some(inference) = &self.inference {
                let inferred = inference.infer(&listing_fields(r), &self.categories);
                if !inferred.is_empty() {
                    return Classification {
                        kind,
                        categories: inferred,
                        source: Some(CategorySource::Inferred),
                    };
                }
            }
        }
        Classification {
            kind,
            categories: Vec::new(),
            source: None,
        }
    }

    /// The upstream a listing resells or wraps, and how that is known; `None`
    /// when nothing names one of the closed list.
    ///
    /// A declaration decides: an id or name of the list (any case) is
    /// `declared`, and any other value is left out and stops the inference,
    /// like a category. Otherwise, for a listing that is not content, the first
    /// of: its host is an upstream's own domain or a subdomain of one; a path
    /// segment or host label is one of its `segments`; its description carries
    /// one of its phrases. Each step in file order.
    pub fn upstream(&self, r: &DiscoveryResource) -> Option<(&str, UpstreamSource)> {
        if let Some(raw) = declared_upstream(r) {
            let key = normalize_label(raw);
            return self
                .upstreams
                .iter()
                .find(|u| u.spellings.contains(&key))
                .map(|u| (u.id.as_str(), UpstreamSource::Declared));
        }
        if self.kind(r) == Kind::Content {
            return None;
        }
        fn inferred(u: &Upstream) -> Option<(&str, UpstreamSource)> {
            Some((u.id.as_str(), UpstreamSource::Inferred))
        }
        let host = r
            .url
            .host_str()
            .unwrap_or_default()
            .trim_end_matches('.')
            .to_ascii_lowercase();
        for u in &self.upstreams {
            if u.hosts.iter().any(|h| {
                host == *h
                    || (host.len() > h.len()
                        && host.ends_with(h.as_str())
                        && host.as_bytes()[host.len() - h.len() - 1] == b'.')
            }) {
                return inferred(u);
            }
        }
        let labels: Vec<String> = host
            .split('.')
            .map(str::to_string)
            .chain(
                r.url
                    .path()
                    .split('/')
                    .filter(|s| !s.is_empty())
                    .map(str::to_ascii_lowercase),
            )
            .collect();
        for u in &self.upstreams {
            if u.segments.iter().any(|s| labels.contains(s)) {
                return inferred(u);
            }
        }
        let words = terms_of(&r.description);
        for u in &self.upstreams {
            if u.phrases.iter().any(|p| contains_phrase(&words, p)) {
                return inferred(u);
            }
        }
        None
    }

    /// The category a request in words asks for, if the inference rules place
    /// its text in one: the best one only, ties in file order. Never the
    /// fallback category -- "data" asks for nothing narrower.
    pub fn request_category(&self, text: &str) -> Option<&str> {
        let inference = self.inference.as_ref()?;
        inference
            .infer(&[terms_of(text)], &self.categories)
            .into_iter()
            .next()
            .filter(|id| !inference.is_fallback(id))
    }

    /// `ids` as a set of bits, one per category of the closed list, leaving
    /// the fallback category out: a listing only in `data` agrees with any
    /// request ([`crate::discovery_search`] uses it so).
    pub fn category_bits(&self, ids: &[&str]) -> u64 {
        let fallback = |id: &str| {
            self.inference
                .as_ref()
                .is_some_and(|inference| inference.is_fallback(id))
        };
        ids.iter()
            .filter(|id| !fallback(id))
            .filter_map(|id| self.categories.keys().position(|c| c == id))
            .filter(|i| *i < 64)
            .fold(0, |bits, i| bits | (1u64 << i))
    }

    /// The upstream ids, in file order.
    // Read by the tests that tie the docs to the list.
    #[allow(dead_code)]
    pub fn upstream_ids(&self) -> impl Iterator<Item = &str> {
        self.upstreams.iter().map(|u| u.id.as_str())
    }
}

/// The upstream the seller declared: `metadata.upstream`, then
/// `extensions.bazaar.upstream`. A blank value declares nothing.
fn declared_upstream(r: &DiscoveryResource) -> Option<&str> {
    r.metadata
        .as_ref()
        .and_then(|m| m.upstream.as_deref())
        .filter(|u| !u.trim().is_empty())
        .or_else(|| {
            r.extensions
                .as_ref()
                .and_then(|e| e.get("bazaar"))
                .and_then(|b| b.get("upstream"))
                .and_then(serde_json::Value::as_str)
                .filter(|u| !u.trim().is_empty())
        })
}

/// What the seller declared as a category, in the order it is trusted: the
/// listing's metadata, then the `bazaar` extension, then a `bazaar` object
/// inside the first payment option's `extra` that carries one. A blank value
/// declares nothing.
fn declared_categories(r: &DiscoveryResource) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
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
    out.retain(|c| !c.trim().is_empty());
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

/// A listing's kind under the shipped vocabulary ([`Taxonomy::kind`]).
pub fn kind(r: &DiscoveryResource) -> Kind {
    taxonomy().kind(r)
}

/// A listing's upstream under the shipped vocabulary ([`Taxonomy::upstream`]).
pub fn upstream(r: &DiscoveryResource) -> Option<(&'static str, UpstreamSource)> {
    taxonomy().upstream(r)
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
            upstream: None,
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
            // And what the router asked for on 2026-10-06.
            "rpc",
        ] {
            assert!(
                t.categories().any(|(c, _)| c == id),
                "{id} is missing from the closed list"
            );
        }
        assert!(t.inference.is_some(), "the inference rules are shipped");
        assert!(t.upstream_ids().count() > 0, "the upstream list is shipped");
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

        // Nothing declared, and nothing the listing says names a category:
        // api, no category.
        let r = listing("https://a.example.com/x");
        assert_eq!(
            classify(&r),
            Classification {
                kind: Kind::Api,
                categories: Vec::new(),
                source: None,
            }
        );
        // Nothing declared, and its host and path say what it is: inferred.
        let r = listing("https://weather.example.com/forecast");
        assert_eq!(
            classify(&r),
            Classification {
                kind: Kind::Api,
                categories: vec!["weather"],
                source: Some(CategorySource::Inferred),
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
            upstream: None,
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

    fn described(url: &str, description: &str) -> DiscoveryResource {
        let mut r = listing(url);
        r.description = description.to_string();
        r
    }

    /// Every rule, against a listing it must place first and one it must not
    /// place at all. The second column is the near miss: a listing that shares
    /// a word with the rule and is not what the rule is for.
    #[test]
    fn every_inference_rule_places_its_listing_and_not_its_near_miss() {
        /// A category, a listing it places first and a near miss it must not
        /// place, each as `(url, description)`.
        type Case<'a> = (&'a str, (&'a str, &'a str), (&'a str, &'a str));
        let cases: &[Case] = &[
            (
                "rpc",
                (
                    "https://solana-rpc.example/",
                    "Solana JSON-RPC endpoint: getLatestBlockhash, getBalance, sendTransaction.",
                ),
                (
                    "https://graph.example/v1/nodes",
                    "List the nodes of a graph.",
                ),
            ),
            (
                "social/x",
                (
                    "https://x.example/api/tweets",
                    "Search recent tweets by keyword.",
                ),
                (
                    "https://birds.example/chirp",
                    "Bird song identification from a recording.",
                ),
            ),
            (
                "social/reddit",
                ("https://api.example/reddit/search", ""),
                ("https://red.example/team", "Red team exercise planning."),
            ),
            (
                "weather",
                (
                    "https://api.example/v1/forecast",
                    "Seven-day forecast with hourly temperature and precipitation.",
                ),
                (
                    "https://llm.example/chat",
                    "Chat completion; set the temperature for more varied answers.",
                ),
            ),
            (
                "people",
                (
                    "https://api.example/v1/find",
                    "Find the work email of a person from their name and company domain.",
                ),
                ("https://mailer.example/send", "Send a transactional email."),
            ),
            (
                "company",
                (
                    "https://api.example/company/enrich",
                    "Company enrichment by domain: industry, headcount and funding.",
                ),
                (
                    "https://news.example/headlines",
                    "Latest news headlines that mention a company.",
                ),
            ),
            (
                "web-search",
                (
                    "https://serp.example/google",
                    "Google search results for a query.",
                ),
                (
                    "https://nft.example/collections/search",
                    "Search NFT collections by name.",
                ),
            ),
            (
                "page-read",
                (
                    "https://reader.example/read",
                    "Read a web page and extract the main article text.",
                ),
                (
                    "https://pdf.example/convert",
                    "Convert a PDF document to text.",
                ),
            ),
            (
                "image",
                (
                    "https://imagegen.example/v1/generate",
                    "Generate an image from a text prompt (text-to-image).",
                ),
                (
                    "https://memes.example/trending",
                    "Trending meme coins by volume.",
                ),
            ),
            (
                "finance",
                (
                    "https://markets.example/v1/quote",
                    "Delayed equity quote (NASDAQ, NYSE) for a ticker symbol.",
                ),
                (
                    "https://quotes.example/daily",
                    "An inspirational quote of the day.",
                ),
            ),
            (
                "crypto",
                (
                    "https://api.example/v1/price",
                    "Real-time cryptocurrency price for a symbol (BTC, ETH, SOL) in USD.",
                ),
                (
                    "https://auth.example/oauth",
                    "Exchange an OAuth code for an access token.",
                ),
            ),
            (
                "human-work",
                (
                    "https://tasks.example/post",
                    "Post a task for human workers and pay them on completion.",
                ),
                ("https://cron.example/jobs", "Schedule a background task."),
            ),
            (
                "ai",
                (
                    "https://llm.example/chat",
                    "Chat completion with an open model.",
                ),
                ("https://search.example/web", "Web search for AI agents."),
            ),
            (
                "communication",
                (
                    "https://sms.example/send",
                    "Send an SMS message to a phone number.",
                ),
                ("https://tv.example/guide", "TV channel guide for tonight."),
            ),
            (
                "security",
                (
                    "https://scan.example/token",
                    "Honeypot and rug pull check for a token contract.",
                ),
                (
                    "https://property.example/climate",
                    "Climate risk score for a property.",
                ),
            ),
            (
                "compliance",
                (
                    "https://kyc.example/check",
                    "KYC and AML screening against sanctions lists.",
                ),
                ("https://tax.example/rates", "Sales tax rates by city."),
            ),
            (
                "reputation",
                (
                    "https://rep.example/wallet",
                    "Reputation score of a wallet.",
                ),
                ("https://movies.example/info", "Movie rating by title."),
            ),
            (
                "developer-tools",
                (
                    "https://review.example/diff",
                    "Automated code review for a diff.",
                ),
                (
                    "https://css.example/scrape",
                    "Scrape the elements matching a CSS selector.",
                ),
            ),
            (
                "research",
                (
                    "https://patents.example/search",
                    "Patent search by keyword.",
                ),
                ("https://sales.example/daily", "Daily sales report."),
            ),
            (
                "advertising",
                (
                    "https://ads.example/buy",
                    "Buy an ad placement on the homepage.",
                ),
                ("https://news.example/feed", "Marketing news of the week."),
            ),
            (
                "infrastructure",
                (
                    "https://proxy.example/mobile",
                    "Rotating mobile proxy IPs by country.",
                ),
                ("https://whois.example/age", "Age of a domain in days."),
            ),
            (
                "data",
                (
                    "https://geo.example/geocode",
                    "Geocode an address to latitude and longitude.",
                ),
                // A narrower category fits, so the fallback stays out.
                (
                    "https://weather.example/by-geocode",
                    "Weather forecast for a geocode.",
                ),
            ),
        ];
        let t = taxonomy();
        let ruled: Vec<&str> = t
            .inference
            .as_ref()
            .unwrap()
            .rules
            .iter()
            .map(|r| r.category.as_str())
            .collect();
        assert_eq!(
            cases.iter().map(|c| c.0).collect::<Vec<_>>(),
            ruled,
            "one case per rule, in file order"
        );
        for (id, (yes_url, yes_text), (no_url, no_text)) in cases {
            let yes = classify(&described(yes_url, yes_text));
            assert_eq!(yes.categories.first(), Some(id), "{yes_url}: {yes:?}");
            assert_eq!(yes.source, Some(CategorySource::Inferred), "{yes_url}");
            let no = classify(&described(no_url, no_text));
            assert!(!no.categories.contains(id), "{no_url}: {no:?}");
        }
    }

    #[test]
    fn inference_fills_silence_only() {
        // The seller declared a category that maps to nothing: left out, not
        // overruled by what its description says.
        let r = with_category("https://weather.example.com/forecast", "utility");
        assert!(classify(&r).categories.is_empty());
        // A blank declaration declares nothing.
        let r = with_category("https://weather.example.com/forecast", "  ");
        assert_eq!(classify(&r).categories, vec!["weather"]);
        // A declaration that maps wins over everything the listing says.
        let r = with_category("https://weather.example.com/forecast", "finance");
        assert_eq!(
            (classify(&r).categories, classify(&r).source),
            (vec!["finance"], Some(CategorySource::Declared))
        );
        // Content is never placed in a category of tools.
        let essay = described(
            "https://tenjin.blog/api/read/weather-apps-lie-about-rain",
            "Paid essay on tenjin.blog: Weather apps lie about rain",
        );
        let c = classify(&essay);
        assert_eq!((c.kind, c.categories.len()), (Kind::Content, 0));
        // Tags and the provider are not read.
        let mut r = listing("https://a.example.com/x");
        r.metadata = Some(DiscoveryMetadata {
            category: None,
            provider: Some("Weather Co".to_string()),
            tags: vec!["weather".to_string(), "forecast".to_string()],
            upstream: None,
        });
        assert!(classify(&r).categories.is_empty());
    }

    #[test]
    fn a_phrase_matches_whole_consecutive_words_of_one_field() {
        // "web search" split across two fields is not the phrase.
        let r = described("https://web.example/a", "Search the catalog.");
        assert!(!classify(&r).categories.contains(&"web-search"));
        // A word inside another word is not the word: `weathered`, `rpcs`.
        let r = described("https://a.example/x", "Weathered wood finishes.");
        assert!(classify(&r).categories.is_empty(), "{:?}", classify(&r));
        // camelCase and letters-then-digits split: a schema's field names and
        // a method name say what they are.
        let r = described("https://a.example/x", "Call getLatestBlockhash.");
        assert_eq!(classify(&r).categories, vec!["rpc"]);
        let mut r = listing("https://a.example/x");
        r.extensions = Some(serde_json::json!({"bazaar": {"info": {"input": {
            "type": "http", "method": "GET",
            "queryParams": {"subreddit": "rust"}
        }}}}));
        assert_eq!(classify(&r).categories, vec!["social/reddit"]);
        let r = described("https://keccak256.example/selector", "");
        assert_eq!(classify(&r).categories.first(), Some(&"developer-tools"));
    }

    #[test]
    fn at_most_two_categories_best_first() {
        // A Solana RPC is rpc first and crypto second; nothing else.
        let r = described(
            "https://rpc.example/solana",
            "Solana JSON-RPC: getLatestBlockhash, getBalance and token balances.",
        );
        assert_eq!(classify(&r).categories, vec!["rpc", "crypto"]);
        // Many categories reach the threshold: two are kept.
        let r = described(
            "https://everything.example/x",
            "Weather forecast, stock quote, crypto price, web search and SMS.",
        );
        assert_eq!(classify(&r).categories.len(), 2);
    }

    #[test]
    fn an_inconsistent_inference_or_upstream_list_is_refused() {
        let with = |inference: &str, upstreams: &str| {
            format!(
                r#"{{"kinds":{{"api":"a","content":"c"}},
                    "categories":{{"finance":"money","data":"d"}},
                    "inference":{inference},
                    "upstreams":{upstreams}}}"#
            )
        };
        let rules = |rules: &str| format!(r#"{{"threshold":2,"maxCategories":2,"rules":{rules}}}"#);
        let ok_rules =
            rules(r#"[{"category":"finance","strong":["stock price"],"weak":["market"]}]"#);
        let ok_up = r#"{"entries":[{"id":"exa","name":"Exa","hosts":["exa.ai"]}]}"#;
        assert!(Taxonomy::parse_strict(&with(&ok_rules, ok_up)).is_ok());
        for bad_rules in [
            // A category that is not in the list.
            rules(r#"[{"category":"stocks","strong":["stock"]}]"#),
            // A phrase of stopwords only can never match.
            rules(r#"[{"category":"finance","strong":["of the"]}]"#),
            // The same phrase twice, the same rule twice, no strong phrase.
            rules(r#"[{"category":"finance","strong":["stock"],"weak":["Stocks"]}]"#),
            rules(
                r#"[{"category":"finance","strong":["a1"]},{"category":"finance","strong":["b1"]}]"#,
            ),
            rules(r#"[{"category":"finance","strong":[],"weak":["market"]}]"#),
            r#"{"threshold":0,"maxCategories":2,"rules":[]}"#.to_string(),
            r#"{"threshold":2,"maxCategories":0,"rules":[]}"#.to_string(),
        ] {
            assert!(
                Taxonomy::parse_strict(&with(&bad_rules, ok_up)).is_err(),
                "{bad_rules}"
            );
        }
        for bad_up in [
            r#"{"entries":[{"id":"Exa","name":"Exa","hosts":["exa.ai"]}]}"#,
            r#"{"entries":[{"id":"exa","name":" ","hosts":["exa.ai"]}]}"#,
            r#"{"entries":[{"id":"exa","name":"Exa"}]}"#,
            r#"{"entries":[{"id":"exa","name":"Exa","hosts":["https://exa.ai"]}]}"#,
            r#"{"entries":[{"id":"exa","name":"Exa","segments":["a/b"]}]}"#,
            r#"{"entries":[{"id":"exa","name":"Exa","hosts":["exa.ai"]},{"id":"exa2","name":"exa","hosts":["b.ai"]}]}"#,
        ] {
            assert!(
                Taxonomy::parse_strict(&with(&ok_rules, bad_up)).is_err(),
                "{bad_up}"
            );
        }
    }

    #[test]
    fn upstream_is_declared_or_named_by_host_path_or_description() {
        let of = |r: &DiscoveryResource| upstream(r);
        // The vendor's own host, and its subdomains; never a lookalike.
        assert_eq!(
            of(&listing("https://x402.tavily.com/search")),
            Some(("tavily", UpstreamSource::Inferred))
        );
        assert_eq!(
            of(&listing("https://exa.ai/search")).map(|u| u.0),
            Some("exa")
        );
        assert_eq!(of(&listing("https://notexa.ai/search")), None);
        assert_eq!(
            of(&listing("https://exa.ai.evil.example/search")).map(|u| u.0),
            Some("exa"),
            "a host label"
        );
        // A path segment or a host label of a reseller.
        assert_eq!(
            of(&listing("https://stableenrich.dev/api/hunter/email-finder")).map(|u| u.0),
            Some("hunter")
        );
        assert_eq!(
            of(&listing("https://api.firecrawl.x402.example/scrape")).map(|u| u.0),
            Some("firecrawl")
        );
        // Not part of a segment: `clawhunter` is not Hunter.
        assert_eq!(of(&listing("https://clawhunter.fun/api/v1/tools")), None);
        // The description, in a form that names the vendor.
        assert_eq!(
            of(&described(
                "https://search.example/web",
                "Neural web search powered by Exa."
            ))
            .map(|u| u.0),
            Some("exa")
        );
        assert_eq!(
            of(&described(
                "https://lm.example/eval",
                "Perplexity of a language model on a corpus."
            )),
            None,
            "a common word is not the vendor"
        );
        assert_eq!(
            of(&described(
                "https://lookup.example/x",
                "Email lookup backed by Hunter.io."
            ))
            .map(|u| u.0),
            Some("hunter")
        );
        // Declared: an id or a name, any case, in metadata or the extension.
        let mut r = listing("https://reseller.example/search");
        r.metadata = Some(DiscoveryMetadata {
            upstream: Some(" Brave Search ".to_string()),
            ..Default::default()
        });
        assert_eq!(of(&r), Some(("brave-search", UpstreamSource::Declared)));
        let mut r = listing("https://reseller.example/search");
        r.extensions = Some(serde_json::json!({"bazaar": {"upstream": "TAVILY"}}));
        assert_eq!(of(&r), Some(("tavily", UpstreamSource::Declared)));
        // A declaration that names nothing of the list is not published, and
        // the host does not overrule it.
        let mut r = listing("https://x402.tavily.com/search");
        r.metadata = Some(DiscoveryMetadata {
            upstream: Some("in-house".to_string()),
            ..Default::default()
        });
        assert_eq!(of(&r), None);
        // A blank one declares nothing.
        r.metadata.as_mut().unwrap().upstream = Some("  ".to_string());
        assert_eq!(of(&r).map(|u| u.1), Some(UpstreamSource::Inferred));
        // Content resells nothing.
        let essay = described(
            "https://tenjin.blog/api/read/exa-vs-tavily",
            "Paid essay on tenjin.blog: Exa vs Tavily",
        );
        assert_eq!(of(&essay), None);
    }

    #[test]
    fn the_kind_alone_reads_no_text() {
        // The cheap answer and the full one agree.
        for r in [
            listing("https://tenjin.blog/api/read/x/y"),
            with_category("https://w.example/p", "essay"),
            listing("https://a.example/x"),
            described("https://weather.example/forecast", "Weather."),
        ] {
            assert_eq!(kind(&r), classify(&r).kind, "{}", r.url);
        }
    }
}
