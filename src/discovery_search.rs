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
/// anything a term scored ([`MIN_SCORED`], even after the tier boost).
const SUBSTRING_ONLY_SCORE: f32 = 1e-9;
/// The least relevance a listing a term scored is given: coverage can make a
/// score as small as it likes, and it must still rank above every listing kept
/// by the substring rule alone.
const MIN_SCORED: f32 = 1e-6;
/// Power of the share of the request a listing covers that multiplies its
/// relevance ([`SearchIndex::scores`]). Squared: a listing that matches half of
/// what the request says keeps a quarter of its score.
const COVERAGE_EXPONENT: i32 = 2;
/// The least share of a request that names a category a listing outside it
/// must cover to stay in the result ([`SearchIndex::relevance`]).
const COVERAGE_FLOOR: f32 = 0.5;
/// What a content listing keeps of its relevance to a request that names a
/// category ([`SearchIndex::relevance`]).
const CONTENT_FOR_A_TASK: f32 = 0.25;

/// What [`SearchIndex::scores`] computes for one query, by document id.
#[derive(Debug, Clone, Default)]
pub struct Scores {
    /// Relevance: BM25 scaled by coverage.
    score: Vec<f32>,
    /// The share of the request each listing covers, 0 to 1.
    coverage: Vec<f32>,
}

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
        // Not `ticker`: a ticker is a symbol, not a price. It sat here and in
        // the stock group, so "Daily stock history for a ticker" covered all
        // of "stock quote" and took a quote's place (2.49.0).
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

/// Blockchains by the names a listing or a request writes them, each group one
/// chain.
///
/// A request that names a chain never gets a listing that names only OTHER
/// chains: "solana rpc getLatestBlockhash" against a catalog without a Solana
/// RPC used to return the HyperEVM one, which shares "rpc" and "get" with it --
/// an answer a router would pay for and that cannot do the job. Empty is better
/// than wrong. A listing that names no chain at all (a price feed, a bridge
/// "between chains") is not excluded, and one that names the requested chain
/// beside others is kept: only a listing about other chains is.
///
/// Only names that are rarely anything else. Left out on purpose: `near`,
/// `stellar`, `scroll`, `ton`, `blast`, `mantle`, `sonic`, `abstract`, `arc`,
/// `cosmos` -- ordinary words in a description, where each would make an
/// unrelated listing look like it is about a chain. `base` stays although it is
/// a word too: it is the network most x402 payments settle on, and wrongly
/// reading "knowledge base" as Base only keeps that listing out of requests
/// that name another chain, which it does not serve anyway. Payment networks
/// (`accepts`) are never read: what a listing is paid on is not what it is
/// about.
const CHAINS: &[&[&str]] = &[
    &["solana", "sol"],
    &["ethereum", "eth"],
    &["bitcoin", "btc"],
    &["base"],
    &["arbitrum"],
    &["optimism"],
    &["polygon", "matic"],
    &["avalanche", "avax"],
    &["bnb", "bsc"],
    &["hyperevm", "hyperliquid"],
    &["celo"],
    &["monad"],
    &["sui"],
    &["aptos"],
    &["tron", "trx"],
    &["algorand"],
    &["xrpl", "xrp", "ripple"],
    &["hedera", "hbar"],
    &["fogo"],
    &["unichain"],
    &["starknet"],
    &["zksync"],
    &["linea"],
    &["cardano"],
    &["polkadot"],
    &["dogecoin", "doge"],
    &["litecoin", "ltc"],
    &["skale"],
    &["gnosis"],
    &["fantom"],
    &["berachain"],
];

/// Normalized chain name -> its bit (one per group of [`CHAINS`]).
static CHAIN_BITS: Lazy<HashMap<String, u64>> = Lazy::new(|| {
    let mut map = HashMap::new();
    for (i, group) in CHAINS.iter().enumerate() {
        for name in *group {
            if let Some(term) = normalize_word(name) {
                map.insert(term, 1u64 << i);
            }
        }
    }
    map
});

/// The chains `terms` name, one bit each.
fn chain_mask<'a>(terms: impl IntoIterator<Item = &'a String>) -> u64 {
    terms
        .into_iter()
        .filter_map(|t| CHAIN_BITS.get(t))
        .fold(0, |mask, bit| mask | bit)
}

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
    let words: Vec<String> = text_terms(word)
        .into_iter()
        .filter(|(_, kind)| *kind == TermKind::Word)
        .map(|(term, _)| term)
        .collect();
    match words.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    }
}

/// Split `text` into search terms and append them to `out`: Paarce's
/// [`tokens`] and [`es_significativo`], after a camelCase split and before a
/// length cap and [`stem`], each word followed by the extra terms
/// [`text_terms`] reads from it.
pub(crate) fn tokenize_into(text: &str, out: &mut Vec<String>) {
    out.extend(text_terms(text).into_iter().map(|(term, _)| term));
}

/// The copy's own test of a token stays the reference: [`significant`] must
/// answer exactly as it does (`the_stopword_set_answers_as_paarce_does`).
const _: fn(&str) -> bool = es_significativo;

/// [`PALABRAS_VACIAS`] as a set.
static STOPWORDS: Lazy<HashSet<&'static str>> = Lazy::new(|| PALABRAS_VACIAS.into_iter().collect());

/// [`es_significativo`], answered from a set: the copy scans its 197 words
/// for every token, and every listing's text is split into tokens for the
/// index and the categories. Same answer, pinned by a test.
fn significant(token: &str) -> bool {
    token.chars().count() >= LARGO_MINIMO_SIGNIFICATIVO && !STOPWORDS.contains(token)
}

/// What a term of a text is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TermKind {
    /// A word as the tokenizer cuts it.
    Word,
    /// A camelCase word written whole: `DeFi` -> `defi`, after `fi`;
    /// `HyperEVM` -> `hyperevm`, after `hyper` and `evm`.
    Whole,
    /// The letters a word starts with when digits follow ([`leading_word`]).
    Lead,
}

/// Each search term of `text`, in order: the words, each followed by the word
/// it starts with when digits follow it, and every camelCase word followed by
/// its whole spelling.
///
/// The camelCase split is what lets a schema's `companyDomain` say "company"
/// and "domain", and it cut proper names in pieces nobody types: `DeFi` was
/// `fi` (`de` is a stopword), `LinkedIn` was `linked`, `WhatsApp` was `whats
/// app`, `HyperEVM` was `hyper evm`. So the whole word is kept beside its
/// pieces.
fn text_terms(text: &str) -> Vec<(String, TermKind)> {
    let keep = |token: &str| significant(token) && token.len() <= MAX_TERM_CHARS;
    let mut out = Vec::new();
    for raw in text.split(|c: char| !c.is_alphanumeric()) {
        if raw.is_empty() {
            continue;
        }
        let parts = tokens(&split_camel_case(raw));
        let split = parts.len() > 1;
        for token in parts.iter().filter(|t| keep(t)) {
            out.push((stem(token), TermKind::Word));
            if let Some(word) = leading_word(token) {
                out.push((stem(word), TermKind::Lead));
            }
        }
        if split {
            if let [whole] = tokens(raw).as_slice() {
                if keep(whole) {
                    out.push((stem(whole), TermKind::Whole));
                }
            }
        }
    }
    out
}

/// The letters a token starts with when digits follow them, if they make a
/// word of their own: `keccak256` -> `keccak`, `gpt4o` -> `gpt`, `web3` ->
/// `web`, a host label `weather0` -> `weather`; `x402` and `v2` have none.
///
/// Paarce cuts on anything that is not `[a-z0-9]`, so a seller's `keccak256`
/// was one term no request for "keccak" could meet, while a CSS scraper that
/// says "selector" three times met the other word of "keccak selector" and won
/// it. The whole token stays, so a request for `keccak256` still prefers the
/// listing that says exactly that.
fn leading_word(token: &str) -> Option<&str> {
    let end = token.find(|c: char| !c.is_ascii_lowercase())?;
    let word = &token[..end];
    (token[end..].starts_with(|c: char| c.is_ascii_digit()) && significant(word)).then_some(word)
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
pub(crate) fn path_text(path: &str) -> String {
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
pub(crate) fn schema_text(extensions: &serde_json::Value) -> String {
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

/// The most terms one field of a listing gives the index.
pub const MAX_FIELD_TERMS: usize = 256;

/// The most terms one listing gives the index, all its fields together.
pub const MAX_DOCUMENT_TERMS: usize = 1_024;

/// The most `(term, listing)` entries one index holds. It is shared out: each
/// listing gives at most its even share of it ([`terms_per_listing`]), so a
/// listing with a lot to say never pushes another one out. Only past
/// [`MIN_TERMS_PER_LISTING`] a share is the last listings handed to
/// [`SearchIndex::build`] are left out, and a search matches them by the
/// substring test alone.
pub const MAX_INDEX_POSTINGS: usize = 600_000;

/// The smallest share of [`MAX_INDEX_POSTINGS`] a listing is cut to.
pub const MIN_TERMS_PER_LISTING: usize = 64;

/// The most text the documents of one build carry, in bytes, shared out the
/// same way down to [`MIN_TEXT_PER_LISTING`].
pub const MAX_INDEX_TEXT_BYTES: usize = 16 * 1024 * 1024;

/// The smallest share of [`MAX_INDEX_TEXT_BYTES`] a listing is cut to.
pub const MIN_TEXT_PER_LISTING: usize = 1_024;

/// Rebuilds of the index one process may start back to back, before
/// [`REBUILD_REFILL_SECS`] paces them ([`RebuildBudget`]).
pub const REBUILD_BURST: u32 = 8;

/// Seconds after which one more rebuild may start once the burst is spent.
pub const REBUILD_REFILL_SECS: u64 = 10;

/// The terms each of `listings` listings may give the index: its even share of
/// [`MAX_INDEX_POSTINGS`], no more than [`MAX_DOCUMENT_TERMS`] and no less than
/// [`MIN_TERMS_PER_LISTING`].
pub fn terms_per_listing(listings: usize) -> usize {
    (MAX_INDEX_POSTINGS / listings.max(1)).clamp(MIN_TERMS_PER_LISTING, MAX_DOCUMENT_TERMS)
}

/// When the index may be rebuilt: [`REBUILD_BURST`] rebuilds at once, then
/// one every [`REBUILD_REFILL_SECS`]. While none is allowed, a search is
/// ranked with the index it already has.
#[derive(Debug)]
pub struct RebuildBudget {
    available: u32,
    refilled_at: std::time::Instant,
}

impl Default for RebuildBudget {
    fn default() -> Self {
        Self {
            available: REBUILD_BURST,
            refilled_at: std::time::Instant::now(),
        }
    }
}

impl RebuildBudget {
    /// Spend one rebuild at `now`, if one is available.
    pub fn take(&mut self, now: std::time::Instant) -> bool {
        let refill = std::time::Duration::from_secs(REBUILD_REFILL_SECS);
        while self.available < REBUILD_BURST && now.duration_since(self.refilled_at) >= refill {
            self.available += 1;
            self.refilled_at += refill;
        }
        if self.available == REBUILD_BURST {
            self.refilled_at = now;
        }
        if self.available == 0 {
            return false;
        }
        self.available -= 1;
        true
    }
}

/// Call `emit` with each searchable field of `r` and its weight, each cut to
/// the length the import filter lets that field have, and no more tags than
/// it lets a listing have ([`crate::discovery_security::curation_check`]).
fn for_each_field(r: &DiscoveryResource, mut emit: impl FnMut(&str, f32)) {
    use crate::discovery_security::{
        MAX_DESCRIPTION_LEN, MAX_META_FIELD_LEN, MAX_TAGS, MAX_TAG_LEN, MAX_URL_LEN,
    };
    let mut f = |text: &str, max: usize, weight: f32| emit(clip(text, max), weight);
    if let Some(host) = r.url.host_str() {
        f(host, MAX_URL_LEN, W_HOST);
    }
    f(&path_text(r.url.path()), MAX_URL_LEN, W_PATH);
    f(&r.description, MAX_DESCRIPTION_LEN, W_DESCRIPTION);
    if let Some(m) = &r.metadata {
        if let Some(p) = &m.provider {
            f(p, MAX_META_FIELD_LEN, W_PROVIDER);
        }
        if let Some(c) = &m.category {
            f(c, MAX_META_FIELD_LEN, W_CATEGORY);
        }
        for t in m.tags.iter().take(MAX_TAGS) {
            f(t, MAX_TAG_LEN, W_TAG);
        }
    }
    if let Some(ext) = &r.extensions {
        f(&schema_text(ext), MAX_DESCRIPTION_LEN, W_SCHEMA);
    }
}

/// `text` cut to at most `max` bytes, on a character boundary.
fn clip(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// What the index reads of one listing: its catalog key and its searchable
/// fields with their weights, already cut ([`for_each_field`]) -- or no fields
/// at all for a listing left out of the index. Owned, so the documents are
/// taken under the catalog guard and the index is built after it is released,
/// off the request's thread.
#[derive(Debug)]
pub struct IndexDocument {
    key: String,
    fields: Option<Vec<(String, f32)>>,
    /// The listing's categories ([`crate::discovery_taxonomy::Taxonomy::category_bits`]).
    categories: u64,
    /// Whether the listing sells content rather than a call to a tool.
    content: bool,
}

impl IndexDocument {
    /// The document of the listing stored under `key`, carrying at most
    /// `text_bytes` of text, its fields taken in order.
    fn within(key: &str, r: &DiscoveryResource, text_bytes: usize) -> Self {
        let mut fields = Vec::new();
        let mut left = text_bytes;
        for_each_field(r, |text, weight| {
            let text = clip(text, left);
            left -= text.len();
            if !text.is_empty() {
                fields.push((text.to_string(), weight));
            }
        });
        let taxonomy = crate::discovery_taxonomy::taxonomy();
        let class = taxonomy.classify(r);
        Self {
            key: key.to_string(),
            fields: Some(fields),
            categories: taxonomy.category_bits(&class.categories),
            content: class.kind == crate::discovery_taxonomy::Kind::Content,
        }
    }

    /// A listing the index is built from but does not index.
    fn left_out(key: &str) -> Self {
        Self {
            key: key.to_string(),
            fields: None,
            categories: 0,
            content: false,
        }
    }

    fn text_bytes(&self) -> usize {
        self.fields
            .iter()
            .flatten()
            .map(|(text, _)| text.len())
            .sum()
    }
}

/// The documents of `listings`, in the order given: each carrying at most its
/// even share of [`MAX_INDEX_TEXT_BYTES`] (no less than
/// [`MIN_TEXT_PER_LISTING`]), until they carry the whole budget; every listing
/// after that is left out of the index.
pub fn documents(listings: &[(&String, &DiscoveryResource)]) -> Vec<IndexDocument> {
    let share = (MAX_INDEX_TEXT_BYTES / listings.len().max(1)).max(MIN_TEXT_PER_LISTING);
    let mut text_bytes = 0usize;
    listings
        .iter()
        .map(|(key, r)| {
            if text_bytes >= MAX_INDEX_TEXT_BYTES {
                return IndexDocument::left_out(key);
            }
            let document = IndexDocument::within(key, r, share);
            text_bytes += document.text_bytes();
            document
        })
        .collect()
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
    /// Whether `q` is one word: no whitespace in it, however many terms it
    /// splits into (`stock-quote`, `tenjin.blog`).
    one_word: bool,
    /// The chains `q` names ([`CHAINS`]), one bit each; 0 when it names none.
    chains: u64,
    /// The category `q` asks for, as a bit of
    /// [`crate::discovery_taxonomy::Taxonomy::category_bits`]; 0 when its words
    /// place it in none.
    category: u64,
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

        let terms = text_terms(trimmed);
        // The chains a request names are the words it wrote, whole, never the
        // start of another one (`base64` names no chain).
        let chains = chain_mask(
            terms
                .iter()
                .filter(|(_, kind)| *kind != TermKind::Lead)
                .map(|(term, _)| term),
        );
        let mut seen = HashSet::new();
        let mut concepts: Vec<Vec<(String, f32)>> = Vec::new();
        // Whether the last word typed opened a concept (and was not a repeat
        // or past the cap): its extra spellings join that concept.
        let mut open = false;
        for (term, kind) in terms {
            if kind != TermKind::Word {
                // `keccak256` also finds a listing that writes `Keccak-256`,
                // and `getLatestBlockhash` one that writes it in one word.
                if let Some(concept) = concepts.last_mut().filter(|_| open) {
                    if concept.iter().all(|(t, _)| *t != term) {
                        concept.push((term, ALTERNATIVE_WEIGHT));
                    }
                }
                continue;
            }
            open = concepts.len() < MAX_QUERY_TERMS && seen.insert(term.clone());
            if !open {
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
            one_word: !trimmed.contains(char::is_whitespace),
            chains,
            category: {
                let taxonomy = crate::discovery_taxonomy::taxonomy();
                taxonomy
                    .request_category(trimmed)
                    .map_or(0, |id| taxonomy.category_bits(&[id]))
            },
        })
    }

    /// The order a request gets when it names none: relevance for a request
    /// in words -- two or more words that leave two or more terms, or longer
    /// than the substring test ever took -- and 2.46.1's order for a single
    /// word, even one that splits into several terms.
    pub fn default_sort(&self) -> SortOrder {
        if (self.concepts.len() >= 2 && !self.one_word) || !self.substring_rule {
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

/// An inverted index over the listings a search can return, for one catalog
/// generation.
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
    /// The keys of the listings it was built from and does not index: past
    /// [`MAX_INDEX_TEXT_BYTES`] or [`MAX_INDEX_POSTINGS`].
    left_out: HashSet<String>,
    /// The chains each document names ([`CHAINS`]), by document id.
    chains: Vec<u64>,
    /// The categories each document is in, by document id.
    categories: Vec<u64>,
    /// Whether each document is content, by document id.
    content: Vec<bool>,
}

impl SearchIndex {
    /// Index `documents` of the catalog at `generation`, in the order given: at
    /// most [`MAX_FIELD_TERMS`] terms from each field and
    /// [`terms_per_listing`] from each listing, until the index holds
    /// [`MAX_INDEX_POSTINGS`]. The listings after that are left out.
    pub fn build<I>(generation: u64, documents: I) -> Self
    where
        I: IntoIterator<Item = IndexDocument>,
    {
        let documents: Vec<IndexDocument> = documents.into_iter().collect();
        let per_listing =
            terms_per_listing(documents.iter().filter(|d| d.fields.is_some()).count());
        let mut ids = HashMap::new();
        let mut lengths = Vec::new();
        let mut postings: HashMap<String, Vec<(u32, f32)>> = HashMap::new();
        let mut left_out = HashSet::new();
        let mut held = 0usize;
        let mut tokens = Vec::new();
        let mut tf: HashMap<String, f32> = HashMap::new();
        let mut chain_masks = Vec::new();
        let mut category_masks = Vec::new();
        let mut content_flags = Vec::new();

        for document in documents {
            let Some(fields) = document.fields.filter(|_| held < MAX_INDEX_POSTINGS) else {
                left_out.insert(document.key);
                continue;
            };
            let mut length = 0.0f32;
            let mut terms = 0usize;
            let mut chains = 0u64;
            for (text, weight) in &fields {
                tokens.clear();
                for (term, kind) in text_terms(text) {
                    // The chain a listing names is a word it wrote, whole,
                    // never the start of another one (`base64` names no chain).
                    if kind != TermKind::Lead {
                        chains |= CHAIN_BITS.get(&term).copied().unwrap_or(0);
                    }
                    tokens.push(term);
                }
                tokens.truncate(MAX_FIELD_TERMS.min(per_listing - terms));
                terms += tokens.len();
                for t in tokens.drain(..) {
                    *tf.entry(t).or_insert(0.0) += *weight;
                    length += *weight;
                }
            }
            if held + tf.len() > MAX_INDEX_POSTINGS {
                // The budget is spent: this listing and every one after it.
                held = MAX_INDEX_POSTINGS;
                tf.clear();
                left_out.insert(document.key);
                continue;
            }
            held += tf.len();
            let id = lengths.len() as u32;
            for (term, weight) in tf.drain() {
                postings.entry(term).or_default().push((id, weight));
            }
            ids.insert(document.key, id);
            lengths.push(length);
            chain_masks.push(chains);
            category_masks.push(document.categories);
            content_flags.push(document.content);
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
            left_out,
            chains: chain_masks,
            categories: category_masks,
            content: content_flags,
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

    /// How many listings it was built from and left out.
    pub fn left_out_count(&self) -> usize {
        self.left_out.len()
    }

    /// How many `(term, listing)` entries it holds: its size.
    pub fn posting_count(&self) -> usize {
        self.postings.values().map(Vec::len).sum()
    }

    /// Whether it was built from exactly the listings stored under `keys`,
    /// indexed or left out.
    pub fn covers(&self, keys: &HashSet<String>) -> bool {
        self.ids.len() + self.left_out.len() == keys.len()
            && keys
                .iter()
                .all(|k| self.ids.contains_key(k) || self.left_out.contains(k))
    }

    /// Relevance of every indexed listing to `query`, by document id.
    ///
    /// BM25 per typed term, taking the best of the term and its lexicon
    /// alternatives, summed over terms -- then scaled by how much of the
    /// request the listing covers ([`COVERAGE_EXPONENT`]).
    ///
    /// # Why coverage
    ///
    /// BM25 sums what each word earns, and a word saturates rather than
    /// stops: a listing that repeats ONE word of the request in its path, its
    /// description and its schema earns nearly twice what a listing earns for
    /// saying it once. So a CSS scraper that says "selector" everywhere beat a
    /// keccak selector tool for "keccak selector", and a meme generator beat
    /// the trending-coins feeds for "trending meme coins": one shared word won
    /// over the task. Coverage is the share of the request's information (each
    /// typed word weighted by its idf, so "get" counts for little and
    /// "keccak" for a lot) that the listing matches, by the word or its
    /// lexicon alternatives. A word no listing carries is left out of it: it
    /// cannot tell listings apart, and counting it would sink every one.
    pub fn scores(&self, query: &SearchQuery) -> Scores {
        let n = self.lengths.len();
        let mut total = vec![0f32; n];
        let mut covered = vec![0f32; n];
        let mut best = vec![0f32; n];
        let mut information = 0f32;
        let mut touched: Vec<u32> = Vec::new();
        for concept in &query.concepts {
            touched.clear();
            let mut importance = 0f32;
            for (term, weight) in concept {
                let Some(list) = self.postings.get(term) else {
                    continue;
                };
                let idf = self.idf(list.len());
                importance = importance.max(weight * idf);
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
            information += importance;
            for &doc in &touched {
                total[doc as usize] += best[doc as usize];
                covered[doc as usize] += importance;
                best[doc as usize] = 0.0;
            }
        }
        let coverage: Vec<f32> = covered
            .iter()
            .map(|c| {
                if information > 0.0 {
                    (c / information).min(1.0)
                } else {
                    0.0
                }
            })
            .collect();
        for (score, share) in total.iter_mut().zip(&coverage) {
            *score *= share.powi(COVERAGE_EXPONENT);
        }
        Scores {
            score: total,
            coverage,
        }
    }

    /// Whether the listing stored under `key` answers some other request than
    /// `query`, and so does not answer it however many words they share:
    ///
    /// - it names one or more chains ([`CHAINS`]), the query names one or
    ///   more, and none is the same -- "solana rpc" is not answered by a
    ///   HyperEVM RPC;
    /// - the query's words place it in a category (the inference rules of
    ///   `config/bazaar_taxonomy.json`, applied to the request), the listing is
    ///   in one or more, and not in that one -- "keccak selector" asks for a
    ///   developer tool and a CSS scraper is a page reader; "stock quote" asks
    ///   for finance and a swap quote is crypto.
    ///
    /// A listing that names no chain, or is in no category (or only in the
    /// fallback `data`), is never excluded by either: nothing says it is about
    /// something else. Empty is a better answer than a wrong one, which a
    /// router would pay for.
    fn answers_another_request(&self, query: &SearchQuery, id: usize) -> bool {
        let disjoint = |asked: u64, listing: Option<&u64>| {
            asked != 0 && listing.is_some_and(|l| *l != 0 && l & asked == 0)
        };
        disjoint(query.chains, self.chains.get(id))
            || disjoint(query.category, self.categories.get(id))
    }

    /// Whether the listing at `id` says too little of a request that names
    /// a category to stand for it: it is not in that category (it is in no
    /// category, or only in the fallback one -- one in another category does
    /// not get here, see [`Self::answers_another_request`]), and it covers
    /// less than [`COVERAGE_FLOOR`] of what the request says. A listing in
    /// the request's category is kept whatever it covers.
    fn says_too_little(&self, query: &SearchQuery, id: usize, scores: &Scores) -> bool {
        query.category != 0
            && self
                .categories
                .get(id)
                .is_some_and(|c| c & query.category == 0)
            && scores.coverage.get(id).copied().unwrap_or(0.0) < COVERAGE_FLOOR
    }

    fn idf(&self, df: usize) -> f32 {
        let n = self.lengths.len() as f32;
        let df = df as f32;
        ((n - df + 0.5) / (df + 0.5) + 1.0).ln()
    }

    /// Relevance of the listing stored under `key`, given `scores` from
    /// [`Self::scores`] and the query they came from. `None` means it does not
    /// match at all and leaves the result.
    ///
    /// A listing that answers another request, or says too little of one in
    /// a category, leaves the result -- unless it carries the request word for
    /// word: everything the substring test of 2.46.1 kept is still kept.
    pub fn relevance(
        &self,
        scores: &Scores,
        query: &SearchQuery,
        key: &str,
        r: &DiscoveryResource,
    ) -> Option<f32> {
        let id = self.ids.get(key).map(|id| *id as usize);
        let literal = || query.substring_rule && query.legacy_hit(r);
        let excluded = id.is_some_and(|id| {
            self.answers_another_request(query, id) || self.says_too_little(query, id, scores)
        });
        if excluded && !literal() {
            return None;
        }
        let mut scored = id
            .and_then(|id| scores.score.get(id))
            .copied()
            .unwrap_or(0.0);
        // A request that names a category asks for a tool: an essay that
        // shares its words stays in the result, behind the tools.
        if query.category != 0 && id.is_some_and(|id| self.content.get(id) == Some(&true)) {
            scored *= CONTENT_FOR_A_TASK;
        }
        if scored > 0.0 {
            Some(scored.max(MIN_SCORED))
        } else if literal() {
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

/// Results paying one recipient that keep their rank in a relevance-ordered
/// result, whatever hosts they are on.
pub const MAX_RESULTS_PER_PAY_TO: usize = 2;

/// Results of one templated family (`/stock-history/{ticker}`) that keep their
/// rank in a relevance-ordered result.
pub const MAX_RESULTS_PER_FAMILY: usize = 1;

/// What a result is grouped by when the top of a relevance-ordered result is
/// shared out ([`diversify`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    /// The listing's host, lowercase.
    Host(String),
    /// A recipient one of its options pays ([`pay_to_key`]).
    PayTo(String),
    /// Its templated family (host and path with the variable segments
    /// written `*`), when it belongs to one.
    Family(String),
}

impl Group {
    fn limit(&self) -> usize {
        match self {
            Group::Host(_) => MAX_RESULTS_PER_HOST,
            Group::PayTo(_) => MAX_RESULTS_PER_PAY_TO,
            Group::Family(_) => MAX_RESULTS_PER_FAMILY,
        }
    }
}

/// A recipient as a grouping key: an EVM address in lowercase (the same
/// account in any case), anything else exactly as written.
pub fn pay_to_key(pay_to: &str) -> String {
    let pay_to = pay_to.trim();
    if pay_to.starts_with("0x") || pay_to.starts_with("0X") {
        pay_to.to_ascii_lowercase()
    } else {
        pay_to.to_string()
    }
}

/// Keep a result where relevance put it while each of its groups has kept
/// fewer than its limit ([`Group::limit`]) above it; move every other one,
/// still in rank order, after them.
///
/// Three kinds of crowding, one rule. A templated family -- 86
/// `/stock-history/{ticker}` of one seller -- would fill a page for any request
/// that names one of its words, and a router reading the top three would see
/// one seller three times; so would one seller listed on many hosts, or a
/// reseller whose every host pays the same address. A host keeps two places, a
/// recipient two, a family one. Nothing is dropped: `total` and pagination are
/// unchanged, only the order -- this is ranking, never admission. Applied to
/// relevance results only; see `DiscoveryRegistry::list`.
pub fn diversify<T, F>(ranked: Vec<T>, groups: F) -> Vec<T>
where
    F: Fn(&T) -> Vec<Group>,
{
    let mut kept: HashMap<Group, usize> = HashMap::new();
    let mut head = Vec::with_capacity(ranked.len());
    let mut tail = Vec::new();
    for item in ranked {
        let mut of_item = groups(&item);
        of_item.sort_unstable();
        of_item.dedup();
        if of_item
            .iter()
            .all(|g| kept.get(g).copied().unwrap_or(0) < g.limit())
        {
            for g in of_item {
                *kept.entry(g).or_insert(0) += 1;
            }
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
    crate::discovery_taxonomy::kind(r).as_str()
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
        let documents = catalog.iter().map(|(k, r)| document_of(k, r));
        (SearchIndex::build(1, documents), catalog)
    }

    /// The whole document of `r`, with no text budget.
    fn document_of(key: &str, r: &DiscoveryResource) -> IndexDocument {
        IndexDocument::within(key, r, usize::MAX)
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
        assert_eq!(terms("personName"), ["person", "name", "personname"]);
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
    fn the_stopword_set_answers_as_paarce_does() {
        for word in PALABRAS_VACIAS
            .iter()
            .copied()
            .chain(["a", "7", "", "weather", "keccak", "de", "la", "ab", "zz"])
        {
            assert_eq!(significant(word), es_significativo(word), "{word:?}");
        }
    }

    #[test]
    fn a_word_followed_by_digits_is_also_read_without_them() {
        assert_eq!(terms("keccak256"), ["keccak256", "keccak"]);
        assert_eq!(terms("gpt4o web3"), ["gpt4o", "gpt", "web3", "web"]);
        assert_eq!(
            terms("weather0.x402.example"),
            ["weather0", "weather", "x402", "example"]
        );
        // One letter, a stopword or digits first: nothing more.
        assert_eq!(terms("x402 v2 to2 2fa"), ["x402", "v2", "to2", "2fa"]);
        // The lexicon reads a word as it is written: one term.
        assert_eq!(normalize_word("txt2img"), Some("txt2img".to_string()));
        // A camelCase name is also kept whole.
        assert_eq!(terms("DeFi yields"), ["fi", "defi", "yield"]);
        assert_eq!(terms("HyperEVM"), ["hyper", "evm", "hyperevm"]);
        assert_eq!(terms("LinkedIn"), ["linked", "linkedin"]);
        // A request for `keccak256` also finds `Keccak-256`, at half weight.
        let q = SearchQuery::parse("keccak256").unwrap();
        assert_eq!(
            q.concepts,
            [vec![
                ("keccak256".to_string(), 1.0),
                ("keccak".to_string(), ALTERNATIVE_WEIGHT)
            ]]
        );
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
        // One word is one word, however many terms it splits into.
        assert_eq!(one("stock-quote"), SortOrder::Tier);
        assert_eq!(one("tenjin.blog"), SortOrder::Tier);
        assert_eq!(one("x402Version"), SortOrder::Tier);
        assert_eq!(one("stock-quote now"), SortOrder::Relevance);
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
        let out = diversify(ranked.clone(), |(host, _)| {
            vec![Group::Host(host.to_ascii_lowercase())]
        });
        let order: Vec<i32> = out.iter().map(|(_, n)| *n).collect();
        assert_eq!(order, [1, 2, 4, 6, 3, 5]);
        assert_eq!(out.len(), ranked.len());
    }

    #[test]
    fn a_recipient_keeps_two_places_and_a_family_one_across_hosts() {
        // (rank, host, payTo, family)
        let ranked = vec![
            (1, "a", "0xseller", Some("a/quote/*")),
            (2, "b", "0xSELLER", None),
            (3, "c", "0xseller", None),
            (4, "a", "0xother", Some("a/quote/*")),
            (5, "a", "0xother", None),
            (6, "d", "0xthird", None),
        ];
        let out = diversify(ranked.clone(), |(_, host, pay_to, family)| {
            let mut groups = vec![
                Group::Host(host.to_string()),
                Group::PayTo(pay_to_key(pay_to)),
            ];
            groups.extend(family.map(|f| Group::Family(f.to_string())));
            groups
        });
        let order: Vec<i32> = out.iter().map(|(n, ..)| *n).collect();
        // 3 is the recipient's third (one EVM address in two cases); 4 is the
        // family's second. Neither takes a place from host `a`, so 5 keeps
        // its rank: a result moved down holds no slot of any of its groups.
        assert_eq!(order, [1, 2, 5, 6, 3, 4]);
        assert_eq!(out.len(), ranked.len());
        // A non-EVM recipient is compared exactly as written.
        assert_ne!(pay_to_key("SoLaNaAddr"), pay_to_key("solanaaddr"));
        assert_eq!(pay_to_key(" 0xAbC "), "0xabc");
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
            upstream: None,
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

    /// `n` distinct letters-only words, from the `from`th on.
    fn words(from: usize, n: usize) -> String {
        (from..from + n)
            .map(|mut i| {
                let mut w = String::from("q");
                loop {
                    w.push((b'a' + (i % 26) as u8) as char);
                    i /= 26;
                    if i == 0 {
                        break w;
                    }
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// One listing gives the index a bounded amount of text whatever it
    /// carries: each field cut to the length an import lets that field have
    /// and to MAX_FIELD_TERMS terms, and no more tags than an import keeps.
    #[test]
    fn a_listing_gives_the_index_a_bounded_amount_of_text() {
        use crate::discovery_security::{
            MAX_DESCRIPTION_LEN, MAX_META_FIELD_LEN, MAX_TAGS, MAX_TAG_LEN,
        };
        let plain = listing("https://big.example/x", "");
        let baseline = index_of(std::slice::from_ref(&plain)).0.posting_count();

        let mut big = plain.clone();
        big.description = words(0, 12_000);
        assert!(big.description.len() > 50_000);
        big.metadata = Some(DiscoveryMetadata {
            upstream: None,
            category: None,
            provider: Some(words(20_000, 2_000)),
            tags: (0..100).map(|i| words(30_000 + i * 500, 500)).collect(),
        });
        let document = document_of("https://big.example/x", &big);
        let fields = document.fields.as_ref().unwrap();
        // host, path, description, provider, then the tags.
        assert_eq!(fields.len(), 4 + MAX_TAGS);
        assert!(fields[2].0.len() <= MAX_DESCRIPTION_LEN);
        assert!(fields[3].0.len() <= MAX_META_FIELD_LEN);
        assert!(
            fields[4..]
                .iter()
                .all(|(text, _)| text.len() <= MAX_TAG_LEN),
            "a tag reached the index whole"
        );

        let added = index_of(std::slice::from_ref(&big)).0.posting_count() - baseline;
        let most = MAX_FIELD_TERMS + (MAX_META_FIELD_LEN + MAX_TAGS * MAX_TAG_LEN) / 3;
        assert!(added <= most, "{added} postings from one listing");
        assert!(
            added >= MAX_FIELD_TERMS,
            "{added}: the description is still indexed"
        );

        // The cut is a byte budget that never splits a character.
        assert_eq!(clip("añb", 2), "a");
        assert_eq!(clip("añb", 3), "añ");
        assert_eq!(clip("ab", 8), "ab");
        assert_eq!(clip("", 0), "");
    }

    /// However many fields a listing has, it gives the index at most
    /// MAX_DOCUMENT_TERMS terms.
    #[test]
    fn a_listing_gives_the_index_at_most_its_term_budget() {
        let document = IndexDocument {
            categories: 0,
            content: false,
            key: "https://wide.example/x".to_string(),
            fields: Some(
                (0..10)
                    .map(|f| (words(f * MAX_FIELD_TERMS, MAX_FIELD_TERMS), 1.0))
                    .collect(),
            ),
        };
        let index = SearchIndex::build(1, [document]);
        assert_eq!(index.listing_count(), 1);
        assert_eq!(index.posting_count(), MAX_DOCUMENT_TERMS);

        // And at most MAX_FIELD_TERMS of them from one field.
        let one_field = IndexDocument {
            categories: 0,
            content: false,
            key: "https://long.example/x".to_string(),
            fields: Some(vec![(words(0, 3 * MAX_FIELD_TERMS), 1.0)]),
        };
        let index = SearchIndex::build(1, [one_field]);
        assert_eq!(index.posting_count(), MAX_FIELD_TERMS);
    }

    /// `count` documents of `terms` distinct terms each (the same terms in
    /// every one: an entry is a term in a listing), under keys `l0`, `l1`...
    fn heavy(count: usize, terms: usize) -> (Vec<String>, Vec<IndexDocument>) {
        let fields: Vec<(String, f32)> = (0..terms.div_ceil(MAX_FIELD_TERMS))
            .map(|f| {
                let from = f * MAX_FIELD_TERMS;
                (words(from, MAX_FIELD_TERMS.min(terms - from)), 1.0)
            })
            .collect();
        let keys: Vec<String> = (0..count)
            .map(|i| format!("https://l{i}.example/x"))
            .collect();
        let documents = keys
            .iter()
            .map(|key| IndexDocument {
                key: key.clone(),
                fields: Some(fields.clone()),
                categories: 0,
                content: false,
            })
            .collect();
        (keys, documents)
    }

    /// The postings budget is shared out: listings with as much to say as
    /// they are allowed, handed over first, never push out the one handed over
    /// last -- each is cut to its share instead.
    #[test]
    fn the_postings_budget_is_shared_so_nobody_is_pushed_out() {
        let count = MAX_INDEX_POSTINGS / MAX_DOCUMENT_TERMS + 10;
        let (mut keys, mut documents) = heavy(count, MAX_DOCUMENT_TERMS);
        let late = listing("https://late.example/x", "Seven-day forecast.");
        keys.push("https://late.example/x".to_string());
        documents.push(document_of("https://late.example/x", &late));

        let index = SearchIndex::build(1, documents);
        assert!(index.posting_count() <= MAX_INDEX_POSTINGS);
        assert_eq!(index.left_out_count(), 0);
        assert_eq!(index.listing_count(), count + 1);
        let query = SearchQuery::parse("weather forecast").unwrap();
        let scores = index.scores(&query);
        assert!(
            index.relevance(&scores, &query, "https://late.example/x", &late) > Some(0.0),
            "the listing handed over last is not ranked"
        );
        assert!(index.posting_count() <= (count + 1) * terms_per_listing(count + 1));
    }

    /// Past the smallest share, the budget is a cut: the listings handed over
    /// first are indexed, the rest left out -- and remembered, so the index
    /// still counts as built from all of them, and from no other set.
    #[test]
    fn past_the_smallest_share_the_index_leaves_the_last_ones_out() {
        let count = MAX_INDEX_POSTINGS / MIN_TERMS_PER_LISTING + 10;
        let (keys, mut documents) = heavy(count, MIN_TERMS_PER_LISTING + 6);
        // A short first listing, so the budget runs out in the middle of one:
        // that one is left out whole, never indexed past the budget.
        documents[0].fields = Some(vec![(words(0, 10), 1.0)]);
        let index = SearchIndex::build(1, documents);
        assert!(index.posting_count() <= MAX_INDEX_POSTINGS);
        assert_eq!(
            index.listing_count(),
            MAX_INDEX_POSTINGS / MIN_TERMS_PER_LISTING
        );
        assert_eq!(index.left_out_count(), 10);
        assert!(
            index.ids.contains_key(&keys[0]),
            "the first one handed over"
        );
        assert!(!index.ids.contains_key(&keys[count - 1]));
        let mut all: HashSet<String> = keys.iter().cloned().collect();
        assert!(index.covers(&all));
        all.remove(&keys[0]);
        assert!(!index.covers(&all), "one fewer");
        all.insert("https://other.example/x".to_string());
        assert!(!index.covers(&all), "as many, but one swapped");
    }

    /// The text budget is shared out the same way: each listing carries at
    /// most its share, and only past the smallest share are the last ones
    /// left out, before anything of theirs is copied.
    #[test]
    fn the_text_budget_is_shared_and_then_cut() {
        let description = words(0, 600);
        assert!(description.len() > 2_000);
        let catalog = |count: usize| -> Vec<(String, DiscoveryResource)> {
            (0..count)
                .map(|i| {
                    let url = format!("https://d{i}.example/x");
                    (url.clone(), listing(&url, &description))
                })
                .collect()
        };

        // Shared: nobody left out, nobody over its share.
        let shared = catalog(MAX_INDEX_TEXT_BYTES / 2_000 + 50);
        let pairs: Vec<(&String, &DiscoveryResource)> =
            shared.iter().map(|(k, r)| (k, r)).collect();
        let docs = documents(&pairs);
        let share = MAX_INDEX_TEXT_BYTES / pairs.len();
        assert!(docs.iter().all(|d| d.fields.is_some()));
        assert!(docs.iter().all(|d| d.text_bytes() <= share));
        assert!(docs.iter().map(IndexDocument::text_bytes).sum::<usize>() <= MAX_INDEX_TEXT_BYTES);

        // Past the smallest share: a cut.
        let cut = catalog(MAX_INDEX_TEXT_BYTES / MIN_TEXT_PER_LISTING + 16);
        let pairs: Vec<(&String, &DiscoveryResource)> = cut.iter().map(|(k, r)| (k, r)).collect();
        let docs = documents(&pairs);
        let kept = docs.iter().filter(|d| d.fields.is_some()).count();
        assert_eq!(kept, MAX_INDEX_TEXT_BYTES / MIN_TEXT_PER_LISTING);
        assert!(docs[..kept].iter().all(|d| d.fields.is_some()));
        assert!(docs[kept..].iter().all(|d| d.fields.is_none()));
    }

    /// Rebuilds come in a burst, then one per refill.
    #[test]
    fn rebuilds_are_paced_after_a_burst() {
        let start = std::time::Instant::now();
        let mut budget = RebuildBudget::default();
        for n in 0..REBUILD_BURST {
            assert!(budget.take(start), "rebuild {n} of the burst");
        }
        assert!(!budget.take(start));
        let refill = std::time::Duration::from_secs(REBUILD_REFILL_SECS);
        assert!(!budget.take(start + refill / 2));
        assert!(budget.take(start + refill));
        assert!(!budget.take(start + refill));
        // Long idle refills the burst, and no more.
        let later = start + refill * 1_000;
        for _ in 0..REBUILD_BURST {
            assert!(budget.take(later));
        }
        assert!(!budget.take(later));
    }
}
