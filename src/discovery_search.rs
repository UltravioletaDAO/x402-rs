//! Relevance search and router filters for `GET /discovery/resources`.
//!
//! # Why this module exists
//!
//! Through 2.46.1 `q` was a case-insensitive substring test over the url,
//! description, provider, category and tags, capped at 128 characters, and the
//! matches came back ordered by curated tier, not by how well they matched. A
//! router sends the agent's request as written -- "find a person's work email"
//! -- and a sentence is never a substring of a listing, so a router partner
//! that tested twelve everyday intents on 2026-10-01 got no result for any of
//! them, and a one-word query came back ordered by who we curate.
//!
//! So `q` is ranked: BM25 over the fields a listing carries, plus a small fixed
//! lexicon that joins the catalog's own vocabulary across English and Spanish
//! ("weather" finds a listing that only says "forecast", "precio" one that says
//! "price"). That lexicon is the hybrid half: deterministic data in this file,
//! no model, no embedding, no call out. The curated tier multiplies relevance;
//! it is never the primary order.
//!
//! # What stays exactly as it was
//!
//! A `q` of up to 128 characters still keeps every listing the substring test
//! kept. Those listings stay in the result even when no term scores, ranked
//! after every scored one and among themselves in the old order. Nothing a
//! caller found before disappears from the result set.
//!
//! # The index
//!
//! Built from the in-memory catalog and stamped with the catalog's generation,
//! which every write moves. The first search after a write rebuilds it; every
//! other search reuses it. See `DiscoveryRegistry::search_index`.
//!
//! # The router filters
//!
//! `maxPriceUsd`, `method`, `hasInputSchema`, `kind` and `excludeHost` are
//! parsed here once per request ([`Constraints`]), and the handler parses them
//! with the same functions to answer 400 on a value that cannot mean anything.

use std::collections::{HashMap, HashSet};

use alloy::primitives::U256;
use once_cell::sync::Lazy;

use crate::network::Network;
use crate::types_v2::{DiscoveryFilters, DiscoveryResource, Tier};

// ============================================================================
// Limits
// ============================================================================

/// Longest `q` accepted, in characters: a whole agent request, not a keyword.
pub const MAX_QUERY_CHARS: usize = 400;

/// Longest `q` the substring match accepted before 2.47.0, and so the longest
/// `q` it still applies to. A longer one was a 400, so no caller depended on it.
pub const LEGACY_SUBSTRING_MAX_CHARS: usize = 128;

/// Distinct terms of one query that are scored. 400 characters of prose stay
/// well under it; it bounds the work a single request can ask for.
const MAX_QUERY_TERMS: usize = 32;

/// Hosts one `excludeHost` may name.
pub const MAX_EXCLUDED_HOSTS: usize = 20;

/// Longest `maxPriceUsd` accepted, in characters.
const MAX_PRICE_CHARS: usize = 32;

/// Methods `method=` understands: the ones a declaration can resolve to
/// ([`crate::discovery_health::declared_request`] reads `HEAD` and `DELETE` as
/// `GET`), so no accepted value is a filter that can never match.
pub const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH"];

/// Values `kind=` understands.
pub const KINDS: &[&str] = &["api", "content"];

// ============================================================================
// Ranking constants
// ============================================================================

/// BM25 term-frequency saturation.
const K1: f32 = 1.2;
/// BM25 length normalization.
const B: f32 = 0.75;
/// Weight of a lexicon alternative against the word the caller typed.
const ALTERNATIVE_WEIGHT: f32 = 0.5;
/// Relevance of a listing kept only by the substring rule: above zero, below
/// anything a term scored.
const SUBSTRING_ONLY_SCORE: f32 = 1e-4;

/// Field weights. The path outweighs the description because an aggregated
/// listing very often has no description at all and `/twitter/search` is
/// then the only thing it says about itself. Tags and category are curated
/// words, so they count double; schema field names are many and incidental.
const W_HOST: f32 = 1.0;
const W_PATH: f32 = 1.5;
const W_DESCRIPTION: f32 = 1.0;
const W_PROVIDER: f32 = 1.0;
const W_CATEGORY: f32 = 2.0;
const W_TAG: f32 = 2.0;
const W_SCHEMA: f32 = 0.5;

/// The curated tier as a multiplier on relevance.
///
/// Small on purpose. A VIP listing that matches one word of a request must not
/// outrank an unlisted service that matches the request; between two listings
/// that match about as well, the curated one goes first.
pub fn tier_boost(tier: Tier) -> f32 {
    match tier {
        Tier::FirstParty => 1.3,
        Tier::Vip => 1.2,
        Tier::Verified => 1.1,
        Tier::Listed => 1.0,
    }
}

// ============================================================================
// Text normalization
// ============================================================================
//
// The tokenizer and the stopword list are COPIED from Paarce, the intent
// matcher of Emporium: `rust/crates/paarce/src/baseline.rs` at emporium
// origin/main 748cf72, lines 21-23, 29-244, 246-281 and 288-292, between the
// markers below, byte for byte. A copy and not a dependency because that
// repository is private and this one is public; it is annotated so the two can
// become one shared crate, and until then a change to either is a change to
// both. Do not edit inside the markers.
//
// What this module adds, only around it: a camelCase split before `tokens`
// (a schema's field names are `companyDomain`), a length cap after it (a
// 42-character address is not a word), and light stemming -- BM25 counts terms,
// so `companies` has to count as `company`, where Paarce compares whole tokens.

/// Longest term indexed, in characters.
const MAX_TERM_CHARS: usize = 32;

// BEGIN PAARCE COPY
/// El largo minimo, en caracteres, de un token significativo: uno de un caracter («a», «y», «o»,
/// «2») no elige.
pub const LARGO_MINIMO_SIGNIFICATIVO: usize = 2;

/// Las palabras que no eligen una opcion, en espanol y en ingles, escritas como los tokens de
/// [`tokens`] (minusculas y sin acentos). Son del motor y no se importan de quien lo hospeda. Unen
/// la frase o dicen quien, cuando o cuanto, pero no que se pide:
///
/// 1. articulos, preposiciones y conjunciones;
/// 2. pronombres, posesivos, demostrativos e indefinidos;
/// 3. las formas comunes de ser, estar, haber y tener, y de querer, necesitar y poder;
/// 4. los numeros del uno al diez, en letras;
/// 5. hoy, manana, ayer, ahora y los dias de la semana.
///
/// Cerrada: no entra un sustantivo ni un verbo que nombre un trabajo. No trae palabras de un
/// caracter, que ya descarta [`LARGO_MINIMO_SIGNIFICATIVO`], ni repetidas.
pub const PALABRAS_VACIAS: [&str; 197] = [
    // 1. articulos, preposiciones y conjunciones
    "de",
    "del",
    "la",
    "el",
    "los",
    "las",
    "un",
    "una",
    "unos",
    "unas",
    "al",
    "en",
    "con",
    "por",
    "para",
    "que",
    "desde",
    "hasta",
    "entre",
    "pero",
    "como",
    "si",
    "ni",
    "the",
    "an",
    "of",
    "for",
    "and",
    "or",
    "to",
    "in",
    "on",
    "with",
    "by",
    "from",
    "at",
    "into",
    "about",
    "but",
    "as",
    "if",
    "so",
    "than",
    // 2. pronombres, posesivos, demostrativos e indefinidos
    "yo",
    "tu",
    "vos",
    "usted",
    "ustedes",
    "nosotros",
    "nosotras",
    "ella",
    "ellos",
    "ellas",
    "me",
    "te",
    "se",
    "nos",
    "le",
    "les",
    "lo",
    "mi",
    "mis",
    "tus",
    "su",
    "sus",
    "nuestro",
    "nuestra",
    "este",
    "esta",
    "esto",
    "ese",
    "esa",
    "eso",
    "otro",
    "otra",
    "otros",
    "otras",
    "alguien",
    "algo",
    "my",
    "mine",
    "we",
    "us",
    "our",
    "you",
    "your",
    "he",
    "him",
    "his",
    "she",
    "her",
    "it",
    "its",
    "they",
    "them",
    "their",
    "this",
    "that",
    "these",
    "those",
    "other",
    "another",
    "someone",
    "something",
    // 3. ser, estar, haber, tener, querer, necesitar y poder (be, have, do, want, need, can)
    "es",
    "son",
    "soy",
    "somos",
    "ser",
    "estan",
    "estoy",
    "estamos",
    "estar",
    "hay",
    "ha",
    "han",
    "haber",
    "tengo",
    "tiene",
    "tienen",
    "tenemos",
    "tener",
    "quiero",
    "quiere",
    "queremos",
    "querer",
    "necesito",
    "necesita",
    "necesitamos",
    "necesitar",
    "puedo",
    "puede",
    "podemos",
    "poder",
    "is",
    "are",
    "am",
    "was",
    "were",
    "be",
    "been",
    "have",
    "has",
    "had",
    "do",
    "does",
    "did",
    "want",
    "wants",
    "need",
    "needs",
    "can",
    "could",
    "would",
    "will",
    // 4. los numeros del uno al diez
    "uno",
    "dos",
    "tres",
    "cuatro",
    "cinco",
    "seis",
    "siete",
    "ocho",
    "nueve",
    "diez",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    // 5. cuando
    "hoy",
    "manana",
    "ayer",
    "ahora",
    "lunes",
    "martes",
    "miercoles",
    "jueves",
    "viernes",
    "sabado",
    "domingo",
    "today",
    "tomorrow",
    "yesterday",
    "now",
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
];

/// La tabla de acentos, explicita y cerrada: minusculas del espanol y del portugues. Es todo lo
/// que se quita; cualquier otro caracter fuera de `[a-z0-9]` no se traduce, separa.
const ACENTOS: [(char, char); 14] = [
    ('á', 'a'),
    ('é', 'e'),
    ('í', 'i'),
    ('ó', 'o'),
    ('ú', 'u'),
    ('ü', 'u'),
    ('ñ', 'n'),
    ('à', 'a'),
    ('â', 'a'),
    ('ã', 'a'),
    ('ê', 'e'),
    ('ô', 'o'),
    ('õ', 'o'),
    ('ç', 'c'),
];

fn sin_acento(c: char) -> char {
    match ACENTOS.iter().find(|(con, _)| *con == c) {
        Some((_, sin)) => *sin,
        None => c,
    }
}

/// Los tokens de un texto: minusculas, sin acentos de la tabla, cortados por cualquier caracter que
/// no sea `[a-z0-9]`, en el orden en que aparecen y con repeticiones.
pub fn tokens(texto: &str) -> Vec<String> {
    let plano: String = texto.to_lowercase().chars().map(sin_acento).collect();
    plano
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Si `token` cuenta para elegir: de [`LARGO_MINIMO_SIGNIFICATIVO`] caracteres o mas y fuera de
/// [`PALABRAS_VACIAS`].
fn es_significativo(token: &str) -> bool {
    token.chars().count() >= LARGO_MINIMO_SIGNIFICATIVO && !PALABRAS_VACIAS.contains(&token)
}
// END PAARCE COPY

/// Groups of words that mean the same thing to a buyer of this catalog.
///
/// The vocabulary is the catalog's own (people, company, web search, page
/// read, X, Reddit, crypto, finance, weather, image) in both languages. A word
/// typed by the caller scores at full weight; the rest of its group scores at
/// [`ALTERNATIVE_WEIGHT`], and a listing is credited once per typed word, with
/// the best of the group, so stuffing synonyms earns nothing.
///
/// Entries are normalized with the same pipeline as everything else, so they
/// are written as a person would write them, accents included.
const LEXICON: &[&[&str]] = &[
    &[
        "person",
        "people",
        "individual",
        "contact",
        "persona",
        "personas",
        "gente",
        "contacto",
    ],
    &[
        "work",
        "worker",
        "employee",
        "employment",
        "staff",
        "workforce",
        "professional",
        "corporate",
        "trabajo",
        "trabaja",
        "trabajan",
        "trabajador",
        "empleado",
        "laboral",
    ],
    &["email", "mail", "mailbox", "inbox", "correo"],
    &[
        "find",
        "finder",
        "lookup",
        "locate",
        "encontrar",
        "encuentra",
    ],
    &[
        "company",
        "business",
        "organization",
        "organisation",
        "firm",
        "corporation",
        "enterprise",
        "empresa",
        "compañía",
        "negocio",
        "organización",
    ],
    &[
        "record",
        "profile",
        "enrich",
        "enrichment",
        "firmographic",
        "firmographics",
        "registro",
        "perfil",
        "ficha",
    ],
    &["domain", "website", "dominio"],
    &["search", "serp", "buscar", "búsqueda", "buscador", "busca"],
    &["web", "internet", "online", "google", "bing"],
    &[
        "read",
        "reader",
        "scrape",
        "scraper",
        "scraping",
        "extract",
        "extraction",
        "extractor",
        "crawl",
        "crawler",
        "fetch",
        "leer",
        "lectura",
        "extraer",
    ],
    &["page", "webpage", "url", "html", "página", "sitio"],
    &[
        "crypto",
        "cryptocurrency",
        "cryptocurrencies",
        "coin",
        "token",
        "bitcoin",
        "btc",
        "ethereum",
        "eth",
        "altcoin",
        "cripto",
        "criptomoneda",
    ],
    &[
        "price",
        "pricing",
        "quote",
        "rate",
        "ticker",
        "spot",
        "valuation",
        "precio",
        "cotización",
        "valor",
    ],
    &[
        "weather",
        "forecast",
        "temperature",
        "rain",
        "precipitation",
        "humidity",
        "wind",
        "meteo",
        "meteorological",
        "clima",
        "pronóstico",
        "temperatura",
        "lluvia",
    ],
    &["twitter", "tweet"],
    &["reddit", "subreddit", "redditor"],
    &[
        "phone",
        "telephone",
        "mobile",
        "cellphone",
        "caller",
        "callerid",
        "msisdn",
        "teléfono",
        "celular",
        "móvil",
    ],
    &[
        "stock", "equity", "equities", "nasdaq", "nyse", "ticker", "acción", "bolsa",
    ],
    &[
        "image",
        "picture",
        "photo",
        "illustration",
        "artwork",
        "imagen",
        "imágenes",
        "foto",
        "ilustración",
        "dibujo",
    ],
    &[
        "generate",
        "generation",
        "generator",
        "generative",
        "create",
        "render",
        "draw",
        "txt2img",
        "genera",
        "generar",
        "generación",
        "crear",
        "dibujar",
    ],
];

/// Normalized word -> every other normalized word that shares a group with it.
static ALTERNATIVES: Lazy<HashMap<String, Vec<String>>> = Lazy::new(|| {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for group in LEXICON {
        let words: Vec<String> = group.iter().filter_map(|w| normalize_word(w)).collect();
        for w in &words {
            let entry = map.entry(w.clone()).or_default();
            for other in &words {
                if other != w && !entry.contains(other) {
                    entry.push(other.clone());
                }
            }
        }
    }
    map
});

/// The one term a single word normalizes to, if it normalizes to exactly one.
fn normalize_word(word: &str) -> Option<String> {
    let mut out = Vec::new();
    tokenize_into(word, &mut out);
    match out.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    }
}

/// Split `text` into search terms and append them to `out`: Paarce's
/// [`tokens`] and [`es_significativo`], after a camelCase split and before a
/// length cap and [`stem`].
pub(crate) fn tokenize_into(text: &str, out: &mut Vec<String>) {
    for token in tokens(&split_camel_case(text)) {
        if es_significativo(&token) && token.len() <= MAX_TERM_CHARS {
            out.push(stem(&token));
        }
    }
}

/// `personName` -> `person Name`, `x402Version` -> `x402 Version`.
fn split_camel_case(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut prev: Option<char> = None;
    for c in text.chars() {
        if c.is_uppercase() && prev.is_some_and(|p| p.is_lowercase() || p.is_ascii_digit()) {
            out.push(' ');
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

/// Light stemming, English and Spanish, deliberately conservative: plurals and
/// `-ation`. Anything subtler (`pricing` / `price`, `imagen` / `imágenes`) is
/// joined by [`LEXICON`] instead, where a wrong join is visible and reviewable.
fn stem(word: &str) -> String {
    if let Some(s) = word.strip_suffix("ciones") {
        return format!("{s}cion");
    }
    if let Some(s) = word.strip_suffix("siones") {
        return format!("{s}sion");
    }
    if word.len() > 7 {
        if let Some(s) = word.strip_suffix("ations") {
            return format!("{s}ate");
        }
        if let Some(s) = word.strip_suffix("ation") {
            return format!("{s}ate");
        }
    }
    if word.len() > 4 {
        if let Some(s) = word.strip_suffix("ies") {
            return format!("{s}y");
        }
    }
    if let Some(s) = word.strip_suffix("sses") {
        return format!("{s}ss");
    }
    for suffix in ["ches", "shes", "xes", "zes"] {
        if word.len() > suffix.len() + 1 {
            if let Some(s) = word.strip_suffix(suffix) {
                return format!("{s}{}", &suffix[..suffix.len() - 2]);
            }
        }
    }
    if word.len() > 3
        && word.ends_with('s')
        && !word.ends_with("ss")
        && !word.ends_with("us")
        && !word.ends_with("is")
    {
        return word[..word.len() - 1].to_string();
    }
    word.to_string()
}

/// A URL path with each `%XX` escape read as a separator, so a template
/// `/item/%7Bid%7D` contributes `item` and `id`, not `7bid`.
fn path_text(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = String::with_capacity(path.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
        {
            out.push(' ');
            i += 3;
            continue;
        }
        let Some(c) = path[i..].chars().next() else {
            break;
        };
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// Keys of a declared request or response that carry no meaning of their own:
/// the structure of the `bazaar` extension and of JSON Schema.
const STRUCTURAL_KEYS: &[&str] = &[
    "bazaar",
    "info",
    "input",
    "output",
    "schema",
    "type",
    "method",
    "body",
    "bodytype",
    "queryparams",
    "pathparams",
    "headers",
    "headerfields",
    "bodyfields",
    "properties",
    "items",
    "required",
    "additionalproperties",
    "example",
    "examples",
    "enum",
    "format",
    "default",
    "minimum",
    "maximum",
    "minlength",
    "maxlength",
    "minitems",
    "maxitems",
    "pattern",
    "const",
    "nullable",
    "anyof",
    "oneof",
    "allof",
    "$schema",
    "$ref",
    "$defs",
    "definitions",
    "discoverable",
];
const MAX_SCHEMA_DEPTH: usize = 8;
const MAX_SCHEMA_KEYS: usize = 128;
const MAX_SCHEMA_TEXT: usize = 2_048;

/// The searchable text of a listing's `extensions.bazaar`: the names of the
/// fields it takes and returns, and any `description` / `title` its schema
/// wrote for them. Example values are left out -- `"0xd8da..."` describes
/// nothing -- and inside an example a key named `title` is a field like any
/// other, not a schema title.
fn schema_text(extensions: &serde_json::Value) -> String {
    let mut out = String::new();
    if let Some(bazaar) = extensions.get("bazaar") {
        let mut keys_left = MAX_SCHEMA_KEYS;
        walk_schema(bazaar, 0, false, &mut out, &mut keys_left);
    }
    out
}

fn walk_schema(
    v: &serde_json::Value,
    depth: usize,
    in_example: bool,
    out: &mut String,
    keys_left: &mut usize,
) {
    if depth > MAX_SCHEMA_DEPTH || out.len() >= MAX_SCHEMA_TEXT {
        return;
    }
    match v {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                if *keys_left == 0 || out.len() >= MAX_SCHEMA_TEXT {
                    return;
                }
                let lower = key.to_ascii_lowercase();
                if !in_example && (lower == "description" || lower == "title") {
                    if let Some(text) = child.as_str() {
                        out.push(' ');
                        out.extend(text.chars().take(MAX_SCHEMA_TEXT / 4));
                        continue;
                    }
                }
                if in_example || !STRUCTURAL_KEYS.contains(&lower.as_str()) {
                    out.push(' ');
                    out.push_str(key);
                    *keys_left -= 1;
                }
                let entering_example = matches!(lower.as_str(), "example" | "examples");
                walk_schema(
                    child,
                    depth + 1,
                    in_example || entering_example,
                    out,
                    keys_left,
                );
            }
        }
        serde_json::Value::Array(items) => {
            for item in items.iter().take(8) {
                walk_schema(item, depth + 1, in_example, out, keys_left);
            }
        }
        _ => {}
    }
}

/// Call `f` with each searchable field of `r` and its weight.
fn for_each_field(r: &DiscoveryResource, mut f: impl FnMut(&str, f32)) {
    if let Some(host) = r.url.host_str() {
        f(host, W_HOST);
    }
    f(&path_text(r.url.path()), W_PATH);
    f(&r.description, W_DESCRIPTION);
    if let Some(m) = &r.metadata {
        if let Some(p) = &m.provider {
            f(p, W_PROVIDER);
        }
        if let Some(c) = &m.category {
            f(c, W_CATEGORY);
        }
        for t in &m.tags {
            f(t, W_TAG);
        }
    }
    if let Some(ext) = &r.extensions {
        f(&schema_text(ext), W_SCHEMA);
    }
}

// ============================================================================
// Query
// ============================================================================

/// How a listing with `q` is ordered.
///
/// `tier` is 2.46.1's search: the substring filter, ordered by curated tier,
/// then liveness, then recency (and now, last, the URL, so ties are the same
/// on every replica). It stays the default for a one-word `q`,
/// because consumers built on that order -- a word looked up, the page
/// re-sorted locally -- must keep getting the same page. A request in plain
/// words is relevance by default, since the substring test has nothing to give
/// it. Either can be asked for by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    Relevance,
    Tier,
}

/// Values `sort=` understands.
pub const SORTS: &[&str] = &["relevance", "tier"];

/// Parse `sort`: one of [`SORTS`], any case.
pub fn parse_sort(raw: &str) -> Result<SortOrder, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "relevance" => Ok(SortOrder::Relevance),
        "tier" => Ok(SortOrder::Tier),
        _ => Err(format!(
            "sort must be one of {} (got {:?})",
            SORTS.join(", "),
            truncate(raw)
        )),
    }
}

/// A parsed `q`.
#[derive(Debug, Clone)]
pub struct SearchQuery {
    /// One entry per distinct term the caller typed: that term at weight 1,
    /// then its lexicon alternatives.
    concepts: Vec<Vec<(String, f32)>>,
    /// The needle of the substring test, normalized exactly as 2.46.1 did.
    needle: String,
    /// Whether `q` is short enough for the substring test to keep its matches
    /// under relevance ranking (it always applies under `sort=tier`).
    substring_rule: bool,
}

impl SearchQuery {
    /// `None` for a blank `q`, which filters nothing, as before.
    pub fn parse(q: &str) -> Option<Self> {
        let trimmed = q.trim();
        if trimmed.is_empty() {
            return None;
        }
        // Exactly the old normalization, so the old match set is reproduced
        // byte for byte.
        let needle = trimmed.to_ascii_lowercase();
        let substring_rule = trimmed.chars().count() <= LEGACY_SUBSTRING_MAX_CHARS;

        let mut terms = Vec::new();
        tokenize_into(trimmed, &mut terms);
        let mut seen = HashSet::new();
        let mut concepts = Vec::new();
        for term in terms {
            if concepts.len() >= MAX_QUERY_TERMS {
                break;
            }
            if !seen.insert(term.clone()) {
                continue;
            }
            let mut alternatives = vec![(term.clone(), 1.0)];
            if let Some(more) = ALTERNATIVES.get(&term) {
                alternatives.extend(more.iter().map(|a| (a.clone(), ALTERNATIVE_WEIGHT)));
            }
            concepts.push(alternatives);
        }
        Some(Self {
            concepts,
            needle,
            substring_rule,
        })
    }

    /// The order a request gets when it names none: relevance for a request
    /// in words -- two or more terms, or longer than the substring test ever
    /// took -- and 2.46.1's order for a single word.
    pub fn default_sort(&self) -> SortOrder {
        if self.concepts.len() >= 2 || !self.substring_rule {
            SortOrder::Relevance
        } else {
            SortOrder::Tier
        }
    }

    /// Whether `r` matches the substring test `q` was through 2.46.1:
    /// url, description, provider, category or a tag contains it, ignoring
    /// ASCII case.
    pub fn legacy_hit(&self, r: &DiscoveryResource) -> bool {
        let needle = self.needle.as_str();
        if r.url.as_str().to_ascii_lowercase().contains(needle)
            || r.description.to_ascii_lowercase().contains(needle)
        {
            return true;
        }
        r.metadata.as_ref().is_some_and(|m| {
            m.provider
                .as_ref()
                .is_some_and(|p| p.to_ascii_lowercase().contains(needle))
                || m.category
                    .as_ref()
                    .is_some_and(|c| c.to_ascii_lowercase().contains(needle))
                || m.tags
                    .iter()
                    .any(|t| t.to_ascii_lowercase().contains(needle))
        })
    }
}

// ============================================================================
// Index
// ============================================================================

/// An inverted index over the catalog, for one catalog generation.
#[derive(Debug)]
pub struct SearchIndex {
    generation: u64,
    /// Document id by catalog key (the resource URL as the cache keys it).
    ids: HashMap<String, u32>,
    /// Weighted length of each document.
    lengths: Vec<f32>,
    avg_length: f32,
    /// Term -> `(document, weighted term frequency)`.
    postings: HashMap<String, Vec<(u32, f32)>>,
}

impl SearchIndex {
    /// Index every `(key, resource)` of a catalog at `generation`.
    pub fn build<'a, I>(generation: u64, catalog: I) -> Self
    where
        I: IntoIterator<Item = (&'a String, &'a DiscoveryResource)>,
    {
        let mut ids = HashMap::new();
        let mut lengths = Vec::new();
        let mut postings: HashMap<String, Vec<(u32, f32)>> = HashMap::new();
        let mut tokens = Vec::new();
        let mut tf: HashMap<String, f32> = HashMap::new();

        for (key, r) in catalog {
            let id = lengths.len() as u32;
            let mut length = 0.0f32;
            for_each_field(r, |text, weight| {
                tokens.clear();
                tokenize_into(text, &mut tokens);
                for t in tokens.drain(..) {
                    *tf.entry(t).or_insert(0.0) += weight;
                    length += weight;
                }
            });
            for (term, weight) in tf.drain() {
                postings.entry(term).or_default().push((id, weight));
            }
            ids.insert(key.clone(), id);
            lengths.push(length);
        }

        let avg_length = if lengths.is_empty() {
            1.0
        } else {
            (lengths.iter().sum::<f32>() / lengths.len() as f32).max(f32::EPSILON)
        };
        Self {
            generation,
            ids,
            lengths,
            avg_length,
            postings,
        }
    }

    /// The catalog generation this index was built from.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// How many listings this index covers.
    pub fn listing_count(&self) -> usize {
        self.lengths.len()
    }

    /// Relevance of every indexed listing to `query`, by document id.
    ///
    /// BM25 per typed term, taking the best of the term and its lexicon
    /// alternatives, summed over terms.
    pub fn scores(&self, query: &SearchQuery) -> Vec<f32> {
        let n = self.lengths.len();
        let mut total = vec![0f32; n];
        let mut best = vec![0f32; n];
        let mut touched: Vec<u32> = Vec::new();
        for concept in &query.concepts {
            touched.clear();
            for (term, weight) in concept {
                let Some(list) = self.postings.get(term) else {
                    continue;
                };
                let idf = self.idf(list.len());
                for &(doc, tf) in list {
                    let d = doc as usize;
                    let norm = K1 * (1.0 - B + B * self.lengths[d] / self.avg_length);
                    let s = weight * idf * tf * (K1 + 1.0) / (tf + norm);
                    if best[d] == 0.0 {
                        touched.push(doc);
                    }
                    if s > best[d] {
                        best[d] = s;
                    }
                }
            }
            for &doc in &touched {
                total[doc as usize] += best[doc as usize];
                best[doc as usize] = 0.0;
            }
        }
        total
    }

    fn idf(&self, df: usize) -> f32 {
        let n = self.lengths.len() as f32;
        let df = df as f32;
        ((n - df + 0.5) / (df + 0.5) + 1.0).ln()
    }

    /// Relevance of the listing stored under `key`, given `scores` from
    /// [`Self::scores`] and the query they came from. `None` means it does not
    /// match at all and leaves the result.
    pub fn relevance(
        &self,
        scores: &[f32],
        query: &SearchQuery,
        key: &str,
        r: &DiscoveryResource,
    ) -> Option<f32> {
        let scored = self
            .ids
            .get(key)
            .and_then(|id| scores.get(*id as usize))
            .copied()
            .unwrap_or(0.0);
        if scored > 0.0 {
            Some(scored)
        } else if query.substring_rule && query.legacy_hit(r) {
            Some(SUBSTRING_ONLY_SCORE)
        } else {
            None
        }
    }
}

// ============================================================================
// Diversity
// ============================================================================

/// Results of one host that keep their rank in a relevance-ordered result.
pub const MAX_RESULTS_PER_HOST: usize = 2;

/// Keep each host's first [`MAX_RESULTS_PER_HOST`] results where relevance put
/// them, and move the rest of that host's results, still in rank order, after
/// every other host's.
///
/// A templated family -- 86 `/stock-history/{ticker}` of one seller -- would
/// otherwise fill a whole page for any request that names one of its words,
/// and a router reading the top three would see one seller three times.
/// Nothing is dropped: `total` and pagination are unchanged, only the order.
/// Applied to relevance results only; see `DiscoveryRegistry::list`.
pub fn diversify_by_host<T, F>(ranked: Vec<T>, host: F) -> Vec<T>
where
    F: Fn(&T) -> &str,
{
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut head = Vec::with_capacity(ranked.len());
    let mut tail = Vec::new();
    for item in ranked {
        let count = seen.entry(host(&item).to_ascii_lowercase()).or_insert(0);
        *count += 1;
        if *count <= MAX_RESULTS_PER_HOST {
            head.push(item);
        } else {
            tail.push(item);
        }
    }
    head.extend(tail);
    head
}

// ============================================================================
// Router filters
// ============================================================================

/// The router filters of one request, parsed once.
///
/// Built from [`DiscoveryFilters`], whose values the handler has already
/// validated with the same parsers. A value that still fails to parse here --
/// a caller that reached `list()` without the handler -- matches nothing
/// rather than being ignored: an ignored filter reads as one that matched
/// everything.
#[derive(Debug, Default)]
pub struct Constraints {
    max_price: Option<UsdLimit>,
    method: Option<String>,
    has_input_schema: Option<bool>,
    kind: Option<String>,
    exclude_hosts: Vec<String>,
    unsatisfiable: bool,
}

impl Constraints {
    pub fn from_filters(filters: Option<&DiscoveryFilters>) -> Self {
        let Some(f) = filters else {
            return Self::default();
        };
        let mut c = Self::default();
        if let Some(raw) = f.max_price_usd.as_deref() {
            match parse_max_price_usd(raw) {
                Ok(limit) => c.max_price = Some(limit),
                Err(_) => c.unsatisfiable = true,
            }
        }
        if let Some(raw) = f.method.as_deref() {
            match parse_method(raw) {
                Ok(m) => c.method = Some(m),
                Err(_) => c.unsatisfiable = true,
            }
        }
        c.has_input_schema = f.has_input_schema;
        if let Some(raw) = f.kind.as_deref() {
            match parse_kind(raw) {
                Ok(k) => c.kind = Some(k),
                Err(_) => c.unsatisfiable = true,
            }
        }
        if let Some(hosts) = f.exclude_host.as_ref() {
            for raw in hosts {
                match parse_exclude_hosts(raw) {
                    Ok(parsed) => c.exclude_hosts.extend(parsed),
                    Err(_) => c.unsatisfiable = true,
                }
            }
        }
        c
    }

    /// Whether `r` passes every router filter of this request.
    pub fn admits(&self, r: &DiscoveryResource) -> bool {
        if self.unsatisfiable {
            return false;
        }
        if let Some(limit) = &self.max_price {
            if !within_price(r, limit) {
                return false;
            }
        }
        if let Some(method) = &self.method {
            if effective_method(r).as_deref() != Some(method.as_str()) {
                return false;
            }
        }
        if let Some(want) = self.has_input_schema {
            if has_input_schema(r) != want {
                return false;
            }
        }
        if let Some(kind) = &self.kind {
            if listing_kind(r) != kind.as_str() {
                return false;
            }
        }
        if !self.exclude_hosts.is_empty() && host_excluded(r, &self.exclude_hosts) {
            return false;
        }
        true
    }
}

/// A dollar amount, kept as the decimal digits it was written with so it can be
/// scaled to any token's decimals without a float.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsdLimit {
    whole: U256,
    fraction: String,
}

impl UsdLimit {
    /// The largest amount, in atomic units of a token with `decimals`, that
    /// does not exceed this limit. Digits past `decimals` are dropped, which
    /// rounds the limit DOWN: a listing is never let in above the price asked.
    fn atomic(&self, decimals: u8) -> U256 {
        let d = decimals as usize;
        let mut digits: String = self.fraction.chars().take(d).collect();
        while digits.len() < d {
            digits.push('0');
        }
        let scale = U256::from(10u64).pow(U256::from(d));
        let fraction = if digits.is_empty() {
            U256::ZERO
        } else {
            U256::from_str_radix(&digits, 10).unwrap_or(U256::ZERO)
        };
        self.whole.saturating_mul(scale).saturating_add(fraction)
    }
}

/// Parse `maxPriceUsd`: a non-negative decimal number of dollars, like `0.01`
/// or `5`. No sign, no exponent, no thousands separator.
pub fn parse_max_price_usd(raw: &str) -> Result<UsdLimit, String> {
    let s = raw.trim();
    let invalid = || {
        format!(
            "maxPriceUsd must be a non-negative decimal amount of US dollars, like 0.01 (got {:?})",
            truncate(raw)
        )
    };
    if s.is_empty() || s.chars().count() > MAX_PRICE_CHARS {
        return Err(invalid());
    }
    let (whole, fraction) = s.split_once('.').unwrap_or((s, ""));
    if whole.is_empty() && fraction.is_empty() {
        return Err(invalid());
    }
    if !whole.chars().all(|c| c.is_ascii_digit()) || !fraction.chars().all(|c| c.is_ascii_digit()) {
        return Err(invalid());
    }
    let whole = if whole.is_empty() {
        U256::ZERO
    } else {
        U256::from_str_radix(whole, 10).map_err(|_| invalid())?
    };
    Ok(UsdLimit {
        whole,
        fraction: fraction.to_string(),
    })
}

/// Parse `method`: one of [`METHODS`], any case.
pub fn parse_method(raw: &str) -> Result<String, String> {
    let m = raw.trim().to_ascii_uppercase();
    if METHODS.contains(&m.as_str()) {
        Ok(m)
    } else {
        Err(format!(
            "method must be one of {} (got {:?})",
            METHODS.join(", "),
            truncate(raw)
        ))
    }
}

/// Parse `hasInputSchema`: `true` or `false`, any case.
pub fn parse_has_input_schema(raw: &str) -> Result<bool, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!(
            "hasInputSchema must be true or false (got {:?})",
            truncate(raw)
        )),
    }
}

/// Parse `kind`: one of [`KINDS`], any case.
pub fn parse_kind(raw: &str) -> Result<String, String> {
    let k = raw.trim().to_ascii_lowercase();
    if KINDS.contains(&k.as_str()) {
        Ok(k)
    } else {
        Err(format!(
            "kind must be one of {} (got {:?})",
            KINDS.join(", "),
            truncate(raw)
        ))
    }
}

/// Parse `excludeHost`: a comma-separated list of host names, each matched
/// with its subdomains. A host is a host: a scheme, path, port or credentials
/// is a 400, not something to strip -- a filter that quietly reads a different
/// value than the one sent is the failure this endpoint already refuses for
/// unknown parameters.
pub fn parse_exclude_hosts(raw: &str) -> Result<Vec<String>, String> {
    let mut hosts: Vec<String> = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if hosts.len() >= MAX_EXCLUDED_HOSTS {
            return Err(format!(
                "excludeHost names at most {MAX_EXCLUDED_HOSTS} hosts"
            ));
        }
        let invalid = || {
            format!(
                "excludeHost must be host names like api.example.com, comma-separated (got {:?})",
                truncate(part)
            )
        };
        if part.contains(['/', '@', '?', '#', ':']) && !part.starts_with('[') {
            return Err(invalid());
        }
        let parsed = url::Url::parse(&format!("http://{part}/")).map_err(|_| invalid())?;
        if parsed.path() != "/"
            || parsed.port().is_some()
            || !parsed.username().is_empty()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(invalid());
        }
        let host = parsed
            .host_str()
            .ok_or_else(invalid)?
            .trim_end_matches('.')
            .to_ascii_lowercase();
        if host.is_empty() {
            return Err(invalid());
        }
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    if hosts.is_empty() {
        return Err("excludeHost names no host".to_string());
    }
    Ok(hosts)
}

/// At most 64 characters of a caller's value, for an error message.
fn truncate(raw: &str) -> String {
    raw.chars().take(64).collect()
}

/// Whether `r` is priced at or below `limit` in a dollar stablecoin.
///
/// Any one payment option is enough: a router can pay whichever it likes. An
/// option in a token we cannot price in dollars -- an unknown asset, EURC --
/// does not count, so a listing priced only in those does not match.
pub fn within_price(r: &DiscoveryResource, limit: &UsdLimit) -> bool {
    r.accepts.iter().any(|option| {
        let Some(network) = Network::from_caip2(&option.network.to_string()) else {
            return false;
        };
        let Some(decimals) = crate::discovery_price::usd_pegged_decimals(network, &option.asset)
        else {
            return false;
        };
        option.amount.0 <= limit.atomic(decimals)
    })
}

/// The method the listing declares, read exactly as the health prober reads it
/// ([`crate::discovery_health::declared_request`]): `info.input.method` of the
/// `bazaar` extension, else the method its JSON Schema input names, else POST
/// when it declares a body. One reading for the filter and the prober, so
/// `method=POST` finds the listings the prober sends a POST.
pub fn declared_method(r: &DiscoveryResource) -> Option<String> {
    use crate::discovery_health::{declared_request, ProbeRequest};
    match declared_request(r.extensions.as_ref()) {
        ProbeRequest::Declared { method, .. } => Some(method.as_str().to_string()),
        ProbeRequest::Undeclared => None,
    }
}

/// The method a caller uses on `r`: the declared one, or `GET` for an HTTP
/// listing that declares none -- the HTTP default, and what the health
/// prober sends it. MCP, A2A and facilitator listings have none: they speak a
/// protocol, not a method.
pub fn effective_method(r: &DiscoveryResource) -> Option<String> {
    if r.resource_type != "http" {
        return None;
    }
    Some(declared_method(r).unwrap_or_else(|| "GET".to_string()))
}

/// Whether the listing says what to send: a non-empty
/// `extensions.bazaar.info.input`, or a non-empty `input` property in
/// `extensions.bazaar.schema`. The listing's own rule
/// ([`DiscoveryResource::has_input_schema`]), the one its `hasInputSchema`
/// field reports, so the filter and the field never disagree.
pub fn has_input_schema(r: &DiscoveryResource) -> bool {
    r.has_input_schema()
}

/// `api` or `content`: the listing's `kind`, from the closed vocabulary
/// ([`crate::discovery_taxonomy::classify`]), the same value
/// `GET /discovery/resources` serves beside it.
pub fn listing_kind(r: &DiscoveryResource) -> &'static str {
    crate::discovery_taxonomy::classify(r).kind.as_str()
}

/// Whether `r`'s host is one of `hosts` or a subdomain of one.
pub fn host_excluded(r: &DiscoveryResource, hosts: &[String]) -> bool {
    let Some(host) = r.url.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    hosts.iter().any(|h| {
        host == *h
            || (host.len() > h.len()
                && host.ends_with(h.as_str())
                && host.as_bytes()[host.len() - h.len() - 1] == b'.')
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caip2::Caip2NetworkId;
    use crate::discovery_price::{CatalogPaymentOption, CatalogScheme};
    use crate::types::{MixedAddress, Scheme, TokenAmount};
    use crate::types_v2::DiscoveryMetadata;

    const USDC_BASE: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
    const EURC_BASE: &str = "0x60a3E35Cc302bFA44Cb288Bc5a4F316Fdb1adb42";

    fn terms(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        tokenize_into(text, &mut out);
        out
    }

    fn option(asset: &str, amount: u64) -> CatalogPaymentOption {
        CatalogPaymentOption::new(
            CatalogScheme::Known(Scheme::Exact),
            Caip2NetworkId::eip155(8453),
            MixedAddress::Evm(asset.parse().unwrap()),
            TokenAmount::from(amount),
            MixedAddress::Evm(
                "0x1234567890123456789012345678901234567890"
                    .parse()
                    .unwrap(),
            ),
            300,
        )
    }

    fn listing(url: &str, description: &str) -> DiscoveryResource {
        DiscoveryResource::new(
            url::Url::parse(url).unwrap(),
            "http".to_string(),
            description.to_string(),
            vec![option(USDC_BASE, 10_000)],
        )
    }

    fn index_of(
        listings: &[DiscoveryResource],
    ) -> (SearchIndex, HashMap<String, DiscoveryResource>) {
        let catalog: HashMap<String, DiscoveryResource> = listings
            .iter()
            .map(|r| (r.url.to_string(), r.clone()))
            .collect();
        (SearchIndex::build(1, catalog.iter()), catalog)
    }

    /// Ranked URLs for `q`, best first.
    fn ranked(listings: &[DiscoveryResource], q: &str) -> Vec<String> {
        let (index, catalog) = index_of(listings);
        let query = SearchQuery::parse(q).unwrap();
        let scores = index.scores(&query);
        let mut hits: Vec<(String, f32)> = catalog
            .iter()
            .filter_map(|(key, r)| {
                index
                    .relevance(&scores, &query, key, r)
                    .map(|s| (key.clone(), s))
            })
            .collect();
        hits.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        hits.into_iter().map(|(k, _)| k).collect()
    }

    #[test]
    fn tokenizing_folds_case_accents_and_splits_words() {
        assert_eq!(terms("Búsqueda WEB"), ["busqueda", "web"]);
        assert_eq!(terms("Pronóstico del CLIMA"), ["pronostico", "clima"]);
        assert_eq!(terms("personName"), ["person", "name"]);
        assert_eq!(terms("stock-quote/v2"), ["stock", "quote", "v2"]);
        // A decomposed accent (e + U+0301) folds the same as the composed one.
        assert_eq!(terms("cafe\u{0301}"), terms("café"));
    }

    #[test]
    fn function_words_and_single_characters_are_dropped() {
        assert_eq!(
            terms("Find a person's work email"),
            ["find", "person", "work", "email"]
        );
        assert_eq!(terms("el precio de una acción"), ["precio", "accion"]);
        // Paarce's rule: a number of two digits or more is a word; one is not.
        assert_eq!(terms("the 2026 of a 7"), ["2026"]);
        // Paarce's list, all 197 of them.
        assert_eq!(PALABRAS_VACIAS.len(), 197);
        assert!(terms(&PALABRAS_VACIAS.join(" ")).is_empty());
    }

    #[test]
    fn a_term_longer_than_the_cap_is_not_indexed() {
        let address = "0xd8da6bf26964af9d7eed9e03e53415d37aa96045";
        assert!(terms(address).is_empty());
        assert_eq!(
            terms(&"a".repeat(MAX_TERM_CHARS)),
            ["a".repeat(MAX_TERM_CHARS)]
        );
    }

    #[test]
    fn plurals_and_ation_meet_their_singular() {
        assert_eq!(terms("companies"), terms("company"));
        assert_eq!(terms("searches"), terms("search"));
        assert_eq!(terms("works"), terms("work"));
        assert_eq!(terms("acciones"), terms("acción"));
        assert_eq!(terms("generation"), terms("generate"));
        // Endings that only look like a plural stay whole.
        assert_eq!(terms("address"), ["address"]);
        assert_eq!(terms("status"), ["status"]);
    }

    #[test]
    fn an_escaped_template_contributes_its_words() {
        assert_eq!(path_text("/item/%7Bid%7D"), "/item/ id ");
        assert_eq!(terms(&path_text("/item/%7Bid%7D")), ["item", "id"]);
        // A stray percent sign is just a separator, never a panic.
        assert_eq!(path_text("/ab%2/cd%"), "/ab%2/cd%");
        assert_eq!(terms(&path_text("/ab%2/cd%")), ["ab", "cd"]);
    }

    #[test]
    fn the_lexicon_joins_words_across_languages() {
        let alts = ALTERNATIVES
            .get("weather")
            .expect("weather is in the lexicon");
        assert!(alts.contains(&"forecast".to_string()));
        assert!(alts.contains(&"clima".to_string()));
        // Normalized like everything else: written with accents, stored without.
        assert!(ALTERNATIVES.contains_key("pronostico"));
        assert!(ALTERNATIVES
            .get("precio")
            .unwrap()
            .contains(&"price".to_string()));
    }

    #[test]
    fn every_lexicon_entry_normalizes_to_one_term() {
        // An entry that tokenizes to nothing (a stopword) or to two words is a
        // silent no-op in the lexicon; make it a visible failure instead.
        for group in LEXICON {
            for word in *group {
                assert!(
                    normalize_word(word).is_some(),
                    "lexicon entry {word:?} does not normalize to a single term"
                );
            }
        }
    }

    #[test]
    fn a_whole_request_finds_the_service_that_does_it() {
        let listings = vec![
            listing(
                "https://api.mailfinder.example/v1/find",
                "Find the verified work email address of a person from their name and company domain.",
            ),
            listing("https://weather.example/current", "Current weather conditions for a city."),
            listing("https://send.example/email", "Send a transactional email."),
        ];
        let hits = ranked(&listings, "Find a person's work email");
        assert_eq!(hits[0], "https://api.mailfinder.example/v1/find");
        assert!(!hits.contains(&"https://weather.example/current".to_string()));
    }

    #[test]
    fn a_synonym_finds_a_listing_that_never_says_the_word() {
        let listings = vec![
            listing(
                "https://meteo.example/now",
                "Seven-day forecast with hourly temperature.",
            ),
            listing("https://news.example/today", "Top headlines of the day."),
        ];
        assert_eq!(ranked(&listings, "weather"), ["https://meteo.example/now"]);
        assert_eq!(
            ranked(&listings, "el clima de hoy"),
            ["https://meteo.example/now"]
        );
    }

    #[test]
    fn the_word_typed_outranks_its_synonym() {
        let listings = vec![
            listing("https://a.example/x", "Hourly forecast."),
            listing("https://b.example/y", "Hourly weather."),
        ];
        assert_eq!(ranked(&listings, "weather")[0], "https://b.example/y");
    }

    #[test]
    fn synonyms_are_credited_once_per_typed_word() {
        // One listing says the typed word; the other stuffs four synonyms of
        // it. The stuffing must not win.
        let listings = vec![
            listing("https://plain.example/w", "Weather now."),
            listing(
                "https://stuffed.example/w",
                "Forecast temperature humidity wind rain now.",
            ),
        ];
        assert_eq!(ranked(&listings, "weather")[0], "https://plain.example/w");
    }

    #[test]
    fn a_short_q_keeps_every_substring_match_it_had() {
        // `trad` is no term of "trading", but it is a substring of it; 2.46.1
        // returned this listing for `q=trad`, and so must this.
        let listings = vec![listing(
            "https://x.example/perps",
            "Perp trading suggestions.",
        )];
        assert_eq!(ranked(&listings, "trad"), ["https://x.example/perps"]);
        assert_eq!(
            ranked(&listings, "PERP TRADING"),
            ["https://x.example/perps"]
        );
    }

    #[test]
    fn a_substring_only_match_ranks_after_every_scored_one() {
        let listings = vec![
            listing("https://a.example/trading", "Signals."),
            listing("https://b.example/x", "Retrading desk."),
        ];
        let query = SearchQuery::parse("trading").unwrap();
        let (index, catalog) = index_of(&listings);
        let scores = index.scores(&query);
        let a = index
            .relevance(
                &scores,
                &query,
                "https://a.example/trading",
                &catalog["https://a.example/trading"],
            )
            .unwrap();
        let b = index
            .relevance(
                &scores,
                &query,
                "https://b.example/x",
                &catalog["https://b.example/x"],
            )
            .unwrap();
        assert!(a > b, "scored {a} must outrank substring-only {b}");
        assert_eq!(b, SUBSTRING_ONLY_SCORE);
    }

    #[test]
    fn a_long_q_has_no_substring_rule_under_relevance() {
        let long = "xy".repeat(LEGACY_SUBSTRING_MAX_CHARS / 2 + 1);
        let listings = vec![listing(&format!("https://a.example/{long}"), "")];
        // Under relevance the long needle keeps nothing by substring...
        let q = SearchQuery::parse(&long).unwrap();
        assert!(!q.substring_rule);
        let (index, catalog) = index_of(&listings);
        let scores = index.scores(&q);
        let (key, r) = catalog.iter().next().unwrap();
        assert_eq!(index.relevance(&scores, &q, key, r), None);
        // ...while the substring test itself, which `sort=tier` uses, still runs.
        assert!(q.legacy_hit(r));
        let short = "x".repeat(LEGACY_SUBSTRING_MAX_CHARS);
        assert!(SearchQuery::parse(&short).unwrap().substring_rule);
        assert!(SearchQuery::parse("   ").is_none());
    }

    #[test]
    fn one_word_keeps_the_old_order_and_a_request_in_words_gets_relevance() {
        let one = |q: &str| SearchQuery::parse(q).unwrap().default_sort();
        assert_eq!(one("weather"), SortOrder::Tier);
        assert_eq!(one("  Tenjin "), SortOrder::Tier);
        // Function words do not make a request: "the weather" is one term.
        assert_eq!(one("the weather"), SortOrder::Tier);
        assert_eq!(one("web search"), SortOrder::Relevance);
        assert_eq!(one("Find a person's work email"), SortOrder::Relevance);
        // Longer than the substring test ever took: relevance whatever it says.
        assert_eq!(
            one(&"a".repeat(LEGACY_SUBSTRING_MAX_CHARS + 1)),
            SortOrder::Relevance
        );
        assert_eq!(parse_sort(" Tier ").unwrap(), SortOrder::Tier);
        assert_eq!(parse_sort("RELEVANCE").unwrap(), SortOrder::Relevance);
        assert!(parse_sort("newest").is_err());
    }

    #[test]
    fn a_query_scores_a_bounded_number_of_terms() {
        let words: Vec<String> = (0..100).map(|i| format!("word{i}")).collect();
        let q = SearchQuery::parse(&words.join(" ")).unwrap();
        assert_eq!(q.concepts.len(), MAX_QUERY_TERMS);
    }

    #[test]
    fn schema_field_names_are_searchable_and_example_values_are_not() {
        let mut r = listing("https://e.example/x", "");
        r.extensions = Some(serde_json::json!({
            "bazaar": {"info": {
                "input": {"type": "http", "method": "POST", "bodyType": "json",
                          "body": {"companyDomain": "stripe.com"}},
                "output": {"type": "json", "example": {"employees": [{"title": "VP"}]}}
            }}
        }));
        let text = terms(&schema_text(r.extensions.as_ref().unwrap()));
        assert!(text.contains(&"company".to_string()));
        assert!(text.contains(&"domain".to_string()));
        assert!(text.contains(&"employee".to_string()));
        assert!(!text.contains(&"stripe".to_string()), "{text:?}");
        assert!(!text.contains(&"vp".to_string()), "{text:?}");
    }

    #[test]
    fn a_declared_json_schema_contributes_its_property_names_and_descriptions() {
        // The shape a seller that publishes `extensions.bazaar.schema` sends:
        // a JSON Schema of the request body, with descriptions.
        let ext = serde_json::json!({"bazaar": {"schema": {"properties": {
            "input": {"properties": {"body": {
                "type": "object",
                "required": ["prompt", "nick"],
                "properties": {
                    "prompt": {"type": "string", "description": "Question to deliberate on"},
                    "nick": {"type": "string"}
                }
            }}},
            "output": {"properties": {"example": {"id": "q_1", "title": "Weekly digest"}}}
        }}}});
        let text = terms(&schema_text(&ext));
        for want in ["prompt", "question", "deliberate", "nick", "title"] {
            assert!(
                text.contains(&want.to_string()),
                "{want} missing from {text:?}"
            );
        }
        // An example's values are not text, even under a key named `title`.
        assert!(!text.contains(&"weekly".to_string()), "{text:?}");
        assert!(!text.contains(&"type".to_string()), "{text:?}");
        let mut r = listing("https://j.example/x", "");
        r.extensions = Some(ext);
        assert!(has_input_schema(&r));
    }

    #[test]
    fn max_price_compares_atomic_units_at_each_token_s_decimals() {
        let mut r = listing("https://p.example/x", "");
        r.accepts = vec![option(USDC_BASE, 10_000)]; // 0.01 USDC
        assert!(within_price(&r, &parse_max_price_usd("0.01").unwrap()));
        assert!(within_price(&r, &parse_max_price_usd("1").unwrap()));
        assert!(!within_price(&r, &parse_max_price_usd("0.009999").unwrap()));
        // Digits past the token's decimals round the limit DOWN.
        assert!(!within_price(
            &r,
            &parse_max_price_usd("0.0099999").unwrap()
        ));
        assert!(within_price(&r, &parse_max_price_usd(".01").unwrap()));
        assert!(within_price(
            &r,
            &parse_max_price_usd(" 0.010000000 ").unwrap()
        ));
    }

    #[test]
    fn a_price_in_a_token_we_cannot_value_in_dollars_never_matches() {
        let mut r = listing("https://p.example/x", "");
        r.accepts = vec![option(EURC_BASE, 1)];
        assert!(!within_price(&r, &parse_max_price_usd("1000").unwrap()));
        r.accepts = vec![option("0x0000000000000000000000000000000000000001", 1)];
        assert!(!within_price(&r, &parse_max_price_usd("1000").unwrap()));
        // One dollar option among others is enough.
        r.accepts.push(option(USDC_BASE, 5));
        assert!(within_price(&r, &parse_max_price_usd("0.000005").unwrap()));
    }

    #[test]
    fn max_price_refuses_anything_but_a_plain_decimal() {
        for bad in [
            "", " ", ".", "-1", "1e-3", "0x10", "1,5", "NaN", "inf", "0.1.2", "$1", "1 000",
        ] {
            assert!(parse_max_price_usd(bad).is_err(), "{bad:?} must be refused");
        }
        assert!(parse_max_price_usd(&"9".repeat(MAX_PRICE_CHARS + 1)).is_err());
        assert!(parse_max_price_usd("0").is_ok());
    }

    #[test]
    fn method_defaults_to_get_only_for_http_listings() {
        let mut r = listing("https://m.example/x", "");
        assert_eq!(effective_method(&r).as_deref(), Some("GET"));
        r.extensions =
            Some(serde_json::json!({"bazaar": {"info": {"input": {"method": " post "}}}}));
        assert_eq!(effective_method(&r).as_deref(), Some("POST"));
        r.extensions = Some(serde_json::json!({"bazaar": {"info": {"input": {"method": "BREW"}}}}));
        assert_eq!(declared_method(&r), None);
        assert_eq!(effective_method(&r).as_deref(), Some("GET"));
        // The prober's reading, not a second one: a JSON Schema body with no
        // method (MeshRelay's shape) is a POST to the filter too.
        r.extensions = Some(serde_json::json!({"bazaar": {"schema": {"properties": {
            "input": {"properties": {"body": {"type": "object"}}}
        }}}}));
        assert_eq!(declared_method(&r).as_deref(), Some("POST"));
        r.resource_type = "mcp".to_string();
        assert_eq!(effective_method(&r), None);
        assert_eq!(parse_method("Post").unwrap(), "POST");
        assert!(parse_method("BREW").is_err());
        assert!(
            parse_method("DELETE").is_err(),
            "no declaration resolves to DELETE"
        );
    }

    #[test]
    fn an_input_schema_is_a_declared_request_or_a_schema() {
        let mut r = listing("https://s.example/x", "");
        assert!(!has_input_schema(&r));
        r.extensions = Some(serde_json::json!({"bazaar": {"info": {"output": {"type": "json"}}}}));
        assert!(!has_input_schema(&r));
        r.extensions = Some(
            serde_json::json!({"bazaar": {"info": {"input": {"type": "http", "method": "GET"}}}}),
        );
        assert!(has_input_schema(&r));
        // An empty declaration declares nothing.
        r.extensions = Some(serde_json::json!({"bazaar": {"info": {"input": {}}}}));
        assert!(!has_input_schema(&r));
        r.extensions = Some(serde_json::json!({"bazaar": {"schema": {}}}));
        assert!(!has_input_schema(&r));
        // A schema has to describe the input, not only exist.
        r.extensions = Some(serde_json::json!({"bazaar": {"schema": {"type": "object"}}}));
        assert!(!has_input_schema(&r));
        r.extensions = Some(
            serde_json::json!({"bazaar": {"schema": {"properties": {"input": {"type": "object"}}}}}),
        );
        assert!(has_input_schema(&r));
        assert!(parse_has_input_schema("TRUE").unwrap());
        assert!(parse_has_input_schema("yes").is_err());
    }

    #[test]
    fn kind_is_the_listings_own_kind() {
        // The value the listing serves as `kind`, from the closed vocabulary:
        // the shipped taxonomy names the pay-per-read essays content.
        let r = listing("https://k.example/x", "");
        assert_eq!(listing_kind(&r), "api");
        let mut mcp = listing("https://k.example/mcp", "");
        mcp.resource_type = "mcp".to_string();
        assert_eq!(listing_kind(&mcp), "api");
        let essay = listing("https://tenjin.blog/api/read/x/an-essay", "");
        assert_eq!(listing_kind(&essay), "content");
        assert_eq!(parse_kind(" API ").unwrap(), "api");
        assert!(parse_kind("tool").is_err());
    }

    #[test]
    fn exclude_host_covers_subdomains_and_nothing_that_merely_ends_alike() {
        let hosts = parse_exclude_hosts("Example.COM, other.test.").unwrap();
        assert_eq!(hosts, ["example.com", "other.test"]);
        assert!(host_excluded(&listing("https://example.com/a", ""), &hosts));
        assert!(host_excluded(
            &listing("https://api.example.com/a", ""),
            &hosts
        ));
        assert!(!host_excluded(
            &listing("https://notexample.com/a", ""),
            &hosts
        ));
        assert!(!host_excluded(
            &listing("https://example.com.evil.test/a", ""),
            &hosts
        ));
    }

    #[test]
    fn exclude_host_refuses_what_is_not_a_host() {
        for bad in [
            "",
            ",",
            "https://example.com",
            "example.com/path",
            "example.com:8080",
            "user@example.com",
            "example.com?x=1",
            "exa mple.com",
        ] {
            assert!(parse_exclude_hosts(bad).is_err(), "{bad:?} must be refused");
        }
        let many: Vec<String> = (0..=MAX_EXCLUDED_HOSTS)
            .map(|i| format!("h{i}.example"))
            .collect();
        assert!(parse_exclude_hosts(&many.join(",")).is_err());
        // Unicode host names compare in their ASCII form.
        assert_eq!(
            parse_exclude_hosts("bücher.example").unwrap(),
            ["xn--bcher-kva.example"]
        );
    }

    #[test]
    fn one_host_keeps_two_places_at_the_top_and_nothing_is_dropped() {
        let ranked = vec![
            ("a.example", 1),
            ("a.example", 2),
            ("A.example", 3),
            ("b.example", 4),
            ("a.example", 5),
            ("c.example", 6),
        ];
        let out = diversify_by_host(ranked.clone(), |(host, _)| *host);
        let order: Vec<i32> = out.iter().map(|(_, n)| *n).collect();
        assert_eq!(order, [1, 2, 4, 6, 3, 5]);
        assert_eq!(out.len(), ranked.len());
    }

    #[test]
    fn an_invalid_filter_that_reaches_list_matches_nothing() {
        let f = DiscoveryFilters {
            method: Some("BREW".to_string()),
            ..Default::default()
        };
        let c = Constraints::from_filters(Some(&f));
        assert!(!c.admits(&listing("https://a.example/x", "")));
        assert!(Constraints::from_filters(None).admits(&listing("https://a.example/x", "")));
    }

    #[test]
    fn metadata_fields_are_indexed() {
        let mut r = listing("https://t.example/x", "");
        r.metadata = Some(DiscoveryMetadata {
            category: Some("social/reddit".to_string()),
            provider: Some("Acme".to_string()),
            tags: vec!["subreddits".to_string()],
        });
        let other = listing("https://u.example/y", "Nothing relevant.");
        assert_eq!(
            ranked(&[r.clone(), other.clone()], "reddit")[0],
            "https://t.example/x"
        );
        assert_eq!(ranked(&[r, other], "acme")[0], "https://t.example/x");
    }
}
