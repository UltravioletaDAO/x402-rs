//! The intents benchmark for `GET /discovery/resources?q=`.
//!
//! # What this measures
//!
//! A router sends the agent's request as written -- "find a person's work
//! email" -- and reads the top three. For each of the twelve everyday intents a
//! router partner tested against the live catalog on 2026-10-01, this counts
//! how many of the top three are services that actually do the job. The live
//! result was 0 of 36 sent as written, and 11 of 36 with the best of two to four
//! hand-picked keywords per intent.
//!
//! # The fixture
//!
//! `tests/fixtures/bazaar/search-catalog.json` (1 999 listings) and
//! `search-intents.json` (the intents, the URLs that do each job, keywords and
//! held-out requests), written by `scripts/bazaar_search_fixture.py`. They are
//! JSON so that any other consumer of the Bazaar can run the same set; the
//! script's docstring says what the catalog is shaped after and what "does the
//! job" means.
//!
//! The benchmark runs over the EXPOSED rows -- the curated bazaar shows only
//! verified-alive listings, and the rows marked `pending` are the ones it would
//! not show (the script says how that is modelled) -- each registered
//! first-hand, so no capacity rule of any build touches them: run against an
//! older build, the same file measures the old search on the same listings. Run
//! with `--nocapture` for the table; the assertions are the floor this change
//! must keep, not the score.

use std::collections::HashMap;
use std::time::Instant;

use url::Url;
use x402_rs::caip2::Caip2NetworkId;
use x402_rs::discovery::{DiscoveryRegistry, ImportPolicy};
use x402_rs::discovery_health::{ProbeMethod, ProbeRequest};
use x402_rs::discovery_price::{CatalogPaymentOption, CatalogScheme};
use x402_rs::types::{MixedAddress, Scheme, TokenAmount};
use x402_rs::types_v2::{DiscoveryFilters, DiscoveryMetadata, DiscoveryResource, DiscoverySource};

const CATALOG: &str = include_str!("fixtures/bazaar/search-catalog.json");
const INTENTS: &str = include_str!("fixtures/bazaar/search-intents.json");

const USDC_BASE: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
const PAY_TO: &str = "0x1234567890123456789012345678901234567890";
/// The newest listing's `lastUpdated`; each row is one second older.
const NEWEST: u64 = 1_790_000_000;

fn usdc(amount: u64) -> CatalogPaymentOption {
    usdc_to(amount, PAY_TO)
}

fn usdc_to(amount: u64, pay_to: &str) -> CatalogPaymentOption {
    CatalogPaymentOption::new(
        CatalogScheme::Known(Scheme::Exact),
        Caip2NetworkId::eip155(8453),
        MixedAddress::Evm(USDC_BASE.parse().unwrap()),
        TokenAmount::from(amount),
        MixedAddress::Evm(pay_to.parse().unwrap()),
        300,
    )
}

/// One seller per host: a recipient derived from the host, so two hosts never
/// share one by accident (the search groups results by recipient).
fn pay_to_of(url: &Url) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    url.host_str().unwrap_or_default().hash(&mut hasher);
    format!("0x{:040x}", hasher.finish())
}

fn strings(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The catalog rows as resources, dated by position.
fn catalog() -> Vec<DiscoveryResource> {
    catalog_rows().into_iter().map(|(r, _)| r).collect()
}

/// The rows the public surface shows: every one not marked `pending`.
fn exposed_catalog() -> Vec<DiscoveryResource> {
    catalog_rows()
        .into_iter()
        .filter_map(|(r, pending)| (!pending).then_some(r))
        .collect()
}

/// Every row, with whether it is `pending` (not verified alive).
fn catalog_rows() -> Vec<(DiscoveryResource, bool)> {
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(CATALOG).expect("catalog fixture parses");
    rows.iter()
        .enumerate()
        .map(|(n, row)| {
            let pending = row
                .get("pending")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let text = |key: &str| row.get(key).and_then(|v| v.as_str()).map(str::to_string);
            let url = Url::parse(&text("url").expect("every row has a url")).expect("fixture url");
            let pay_to = pay_to_of(&url);
            let mut r = DiscoveryResource::new(
                url,
                text("type").unwrap_or_else(|| "http".to_string()),
                text("description").unwrap_or_default(),
                vec![usdc_to(1_000 + (n as u64 % 50) * 1_000, &pay_to)],
            );
            r.last_updated = NEWEST - n as u64;
            let (category, provider, tags) =
                (text("category"), text("provider"), strings(&row["tags"]));
            if category.is_some() || provider.is_some() || !tags.is_empty() {
                r.metadata = Some(DiscoveryMetadata {
                    upstream: None,
                    category,
                    provider,
                    tags,
                });
            }
            if let Some(method) = text("method") {
                let fields: serde_json::Map<String, serde_json::Value> =
                    strings(&row["inputFields"])
                        .into_iter()
                        .map(|f| (f, serde_json::json!("example")))
                        .collect();
                let slot = if method == "GET" {
                    "queryParams"
                } else {
                    "body"
                };
                r.extensions = Some(serde_json::json!({
                    "bazaar": {"info": {"input": {"type": "http", "method": method, slot: fields}}}
                }));
            }
            (r, pending)
        })
        .collect()
}

struct Intent {
    id: String,
    text: String,
    keywords: Vec<String>,
    expected: Vec<String>,
}

fn intents() -> (Vec<Intent>, Vec<(String, String)>) {
    let doc: serde_json::Value = serde_json::from_str(INTENTS).expect("intents fixture parses");
    let intents = doc["intents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| Intent {
            id: i["id"].as_str().unwrap().to_string(),
            text: i["text"].as_str().unwrap().to_string(),
            keywords: strings(&i["keywords"]),
            expected: strings(&i["expected"]),
        })
        .collect();
    let held_out = doc["heldOut"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| {
            (
                h["text"].as_str().unwrap().to_string(),
                h["intent"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    (intents, held_out)
}

/// The public surface of the catalog as a registry: the EXPOSED rows, every
/// one registered first-hand (so no capacity rule touches them) and marked
/// verified alive. A router querying the curated bazaar sees these and only
/// these; the benchmark measures over them.
async fn registry() -> DiscoveryRegistry {
    let registry = DiscoveryRegistry::new();
    for r in exposed_catalog() {
        registry
            .register(r)
            .await
            .expect("fixture listing registers");
    }
    expose_all(&registry).await;
    registry
}

/// Mark every held listing verified alive -- a readable 402 to the request its
/// listing declares, as the prober records one -- so the public surface shows
/// it. The one place this file records that 402; every listing test of this
/// file goes through it. (An MCP listing stays unexposed whatever is recorded:
/// its probe reads no challenge.)
async fn expose_all(registry: &DiscoveryRegistry) {
    for t in registry.probe_targets().await {
        let method = match t.request {
            ProbeRequest::Declared { method, .. } => method,
            ProbeRequest::Undeclared => ProbeMethod::Get,
        };
        registry
            .health()
            .mark_verified(t.url.as_str(), method)
            .await;
    }
}

/// Listings HELD per host, largest first: how the catalog itself spreads over
/// hosts, exposed or not (`stats.topHosts` counts only what is exposed).
async fn held_by_host(registry: &DiscoveryRegistry) -> Vec<(String, u64)> {
    let mut by_host: HashMap<String, u64> = HashMap::new();
    for url in registry.all_urls().await {
        *by_host
            .entry(url.host_str().unwrap_or_default().to_ascii_lowercase())
            .or_default() += 1;
    }
    let mut hosts: Vec<(String, u64)> = by_host.into_iter().collect();
    hosts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    hosts
}

/// The same registry under the shipped curation manifest minus one entry:
/// what the catalog ranks like once paid content no longer takes the VIP tier.
async fn registry_without_curated(entry: &str) -> DiscoveryRegistry {
    let mut manifest: serde_json::Value =
        serde_json::from_str(include_str!("../config/bazaar_curation.json")).unwrap();
    manifest["entries"]
        .as_array_mut()
        .unwrap()
        .retain(|e| e["name"] != entry);
    let path = std::env::temp_dir().join(format!("x4bs-curation-{}.json", std::process::id()));
    std::fs::write(&path, manifest.to_string()).unwrap();
    std::env::set_var("BAZAAR_CURATION_PATH", &path);
    let registry = registry().await;
    std::env::remove_var("BAZAAR_CURATION_PATH");
    let _ = std::fs::remove_file(&path);
    registry
}

fn filters(q: &str, sort: Option<&str>) -> DiscoveryFilters {
    DiscoveryFilters {
        q: Some(q.to_string()),
        sort: sort.map(str::to_string),
        ..Default::default()
    }
}

/// How many of the top three for `q` are in `expected`.
async fn top3(
    registry: &DiscoveryRegistry,
    expected: &[String],
    q: &str,
    sort: Option<&str>,
) -> (usize, Vec<String>) {
    top3_with(registry, expected, filters(q, sort)).await
}

async fn top3_with(
    registry: &DiscoveryRegistry,
    expected: &[String],
    filters: DiscoveryFilters,
) -> (usize, Vec<String>) {
    let page = registry.list(3, 0, Some(filters)).await;
    let urls: Vec<String> = page.items.iter().map(|r| r.url.to_string()).collect();
    let hits = urls.iter().filter(|u| expected.contains(u)).count();
    (hits, urls)
}

#[test]
fn the_fixture_has_the_shape_that_was_measured() {
    let listings = catalog();
    assert_eq!(listings.len(), 1_999);
    let mut by_host: HashMap<String, usize> = HashMap::new();
    for r in &listings {
        *by_host
            .entry(r.url.host_str().unwrap().to_string())
            .or_default() += 1;
    }
    let mut counts: Vec<usize> = by_host.values().copied().collect();
    counts.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(
        counts[..4].iter().sum::<usize>(),
        1_043,
        "{:?}",
        &counts[..6]
    );
    let urls: std::collections::HashSet<String> =
        listings.iter().map(|r| r.url.to_string()).collect();
    assert_eq!(urls.len(), listings.len(), "fixture URLs are unique");
    let (intents, held_out) = intents();
    assert_eq!(intents.len(), 12);
    for i in &intents {
        assert!(
            i.expected.len() >= 3,
            "{} has {} services",
            i.id,
            i.expected.len()
        );
        for u in &i.expected {
            assert!(
                urls.contains(u),
                "{} expects {u}, which is not in the catalog",
                i.id
            );
        }
    }
    for (_, id) in &held_out {
        assert!(intents.iter().any(|i| &i.id == id), "held-out names {id}");
    }
}

#[tokio::test]
async fn twelve_intents_sent_as_written_find_services_that_do_the_job() {
    let registry = registry().await;
    // The fixture's 379 paid essays are under a VIP prefix of the shipped
    // manifest, and twelve of them are titled with an intent's own words.
    // Since 2.47.0 paid content never shows the VIP tier, so the view without
    // that manifest entry ranks the same; it stays as a check that it does.
    // The other view is a router that leaves the content host out.
    let no_vip_content = registry_without_curated("Tenjin").await;
    let (intents, _) = intents();

    let (mut as_written, mut relevance, mut tier_written, mut tier_best) = (0, 0, 0, 0);
    let (mut content_not_vip, mut content_excluded) = (0, 0);
    println!(
        "{:<28} {:>9} {:>10} {:>9} {:>9} {:>12} {:>13}",
        "intent", "default", "relevance", "no-vip", "no-host", "tier/written", "tier/keyword"
    );
    for intent in &intents {
        let (d, _) = top3(&registry, &intent.expected, &intent.text, None).await;
        let (r, r_urls) = top3(&registry, &intent.expected, &intent.text, Some("relevance")).await;
        let (nv, _) = top3(
            &no_vip_content,
            &intent.expected,
            &intent.text,
            Some("relevance"),
        )
        .await;
        let (nh, _) = top3_with(
            &registry,
            &intent.expected,
            DiscoveryFilters {
                exclude_host: Some(vec!["tenjin.blog".to_string()]),
                ..filters(&intent.text, Some("relevance"))
            },
        )
        .await;
        let (t, _) = top3(&registry, &intent.expected, &intent.text, Some("tier")).await;
        let mut best = 0;
        for k in &intent.keywords {
            best = best.max(top3(&registry, &intent.expected, k, Some("tier")).await.0);
        }
        println!(
            "{:<28} {d:>7}/3 {r:>8}/3 {nv:>7}/3 {nh:>7}/3 {t:>10}/3 {best:>11}/3   {r_urls:?}",
            intent.text
        );
        as_written += d;
        relevance += r;
        content_not_vip += nv;
        content_excluded += nh;
        tier_written += t;
        tier_best += best;
    }
    println!(
        "TOTAL default {as_written}/36, relevance {relevance}/36, relevance with content not VIP \
         {content_not_vip}/36, relevance without the content host {content_excluded}/36, \
         2.46.1 search as written {tier_written}/36, 2.46.1 search best keyword {tier_best}/36"
    );

    // Floors at what was measured when this was written (2.47.0); a drop is
    // a regression in the ranking, a rise should move them.
    assert!(relevance >= 26, "relevance top-3 fell to {relevance}/36");
    assert!(as_written >= 25, "default top-3 fell to {as_written}/36");
    assert!(
        content_not_vip >= relevance,
        "{content_not_vip} < {relevance}"
    );
    assert!(
        content_excluded >= content_not_vip,
        "{content_excluded} < {content_not_vip}"
    );
    // `sort=tier` is the substring match: a sentence finds next to nothing.
    assert!(
        tier_written <= 6,
        "sort=tier found {tier_written}/36 as written"
    );
    assert!(
        relevance > tier_best,
        "relevance {relevance} <= best keyword {tier_best}"
    );
}

#[tokio::test]
async fn requests_the_lexicon_was_not_written_against_are_found_too() {
    // Not the twelve: paraphrases in English and Spanish, to show the ranking
    // is not fitted to the table.
    let registry = registry().await;
    let (intents, held_out) = intents();
    let mut total = 0;
    for (q, id) in &held_out {
        let expected = &intents.iter().find(|i| &i.id == id).unwrap().expected;
        let (hits, urls) = top3(&registry, expected, q, None).await;
        println!("{q:<48} {hits}/3 {urls:?}");
        total += hits;
    }
    println!("HELD-OUT TOTAL {total}/30");
    assert!(total >= 20, "held-out requests fell to {total}/30");
}

#[tokio::test]
async fn a_request_ranks_the_service_above_an_essay_that_shares_one_word() {
    // The essay says "weather" and nothing else of the request; the service
    // says what was asked. Tier is a boost, not the order.
    let registry = registry().await;
    let page = registry
        .list(
            5,
            0,
            Some(filters("current weather conditions in a city", None)),
        )
        .await;
    let urls: Vec<String> = page.items.iter().map(|r| r.url.to_string()).collect();
    assert_eq!(urls[0], "https://weather.x402.example/current", "{urls:?}");
    let essay = "https://tenjin.blog/api/read/weather-apps-lie-about-rain";
    if let Some(pos) = urls.iter().position(|u| u == essay) {
        assert!(pos > 0, "{urls:?}");
    }
    // ...and the old order, still available by name, is the catalog order
    // over the substring matches. The essay used to lead it from the VIP
    // tier; paid content no longer holds that tier, so it no longer jumps
    // the queue.
    let page = registry
        .list(5, 0, Some(filters("weather", Some("tier"))))
        .await;
    let by_tier: Vec<String> = page.items.iter().map(|r| r.url.to_string()).collect();
    let mut catalog_order = Vec::new();
    let mut offset = 0;
    loop {
        let walked = registry.list(100, offset, None).await;
        if walked.items.is_empty() {
            break;
        }
        offset += walked.items.len() as u32;
        catalog_order.extend(walked.items.into_iter().filter(|r| {
            let meta = r.metadata.as_ref();
            let hay = format!(
                "{} {} {} {} {}",
                r.url,
                r.description,
                meta.and_then(|m| m.provider.clone()).unwrap_or_default(),
                meta.and_then(|m| m.category.clone()).unwrap_or_default(),
                meta.map(|m| m.tags.join(" ")).unwrap_or_default()
            );
            hay.to_ascii_lowercase().contains("weather")
        }));
    }
    let expected: Vec<String> = catalog_order
        .iter()
        .take(5)
        .map(|r| r.url.to_string())
        .collect();
    assert_eq!(by_tier, expected);
    assert_ne!(by_tier[0], essay, "content no longer leads the old order");
}

#[tokio::test]
async fn one_seller_does_not_fill_the_top_of_a_request() {
    // A data-pack host has 388 templated listings whose slugs say "company",
    // "market", "ticker", "crypto"; the essays host has 379 that say almost
    // anything. A request must still see other sellers in its first page.
    let registry = registry().await;
    for q in [
        "stock quote for a ticker",
        "company revenue",
        "crypto market volume",
        "agents payments",
    ] {
        // No host's third result comes before any other host's first two:
        // once a third appears, everything after it is a third or later too.
        let page = registry.list(100, 0, Some(filters(q, None))).await;
        let mut by_host: HashMap<String, usize> = HashMap::new();
        let mut overflow_started = None;
        for (pos, r) in page.items.iter().enumerate() {
            let seen = by_host
                .entry(r.url.host_str().unwrap().to_string())
                .or_default();
            *seen += 1;
            if *seen > 2 {
                overflow_started.get_or_insert(pos);
            } else if let Some(start) = overflow_started {
                panic!(
                    "{q:?}: {} at {pos} is a host's first or second result, after a third at {start}",
                    r.url
                );
            }
        }
        assert!(page.items.len() > 3, "{q:?} found too little to test");
    }
}

#[tokio::test]
async fn search_latency_over_the_fixture_catalog() {
    let registry = registry().await;
    let (intents, _) = intents();

    // The first search after a write builds the index.
    let started = Instant::now();
    registry
        .list(10, 0, Some(filters("web search", None)))
        .await;
    let first = started.elapsed();

    let mut samples: Vec<u128> = Vec::new();
    for _ in 0..5 {
        for intent in &intents {
            let started = Instant::now();
            registry
                .list(10, 0, Some(filters(&intent.text, None)))
                .await;
            samples.push(started.elapsed().as_micros());
        }
    }
    samples.sort_unstable();
    let p50 = samples[samples.len() / 2];
    let p95 = samples[samples.len() * 95 / 100];

    let mut plain: Vec<u128> = Vec::new();
    for _ in 0..20 {
        let started = Instant::now();
        registry.list(10, 0, None).await;
        plain.push(started.elapsed().as_micros());
    }
    plain.sort_unstable();
    println!(
        "LATENCY (this build's profile) first search incl. index build {} us; \
         q p50 {p50} us p95 {p95} us; no q p50 {} us",
        first.as_micros(),
        plain[plain.len() / 2]
    );
}

/// The same questions asked of a copy of the REAL catalog, for whoever has one.
///
/// Not run by default and not a pass/fail test: the catalog is third-party data
/// and never enters the repository, and on real listings nobody has written
/// down which ones do the job -- the top three are printed for a person to
/// judge, next to what the 2.46.1 search returns. A copy of
/// `s3://<bucket>/bazaar/resources.json` (any version):
///
/// ```text
/// BAZAAR_SNAPSHOT=/path/to/resources.json \
///   cargo test --test bazaar_search -- --ignored --nocapture snapshot_report
/// ```
///
/// Health is a separate overlay that a snapshot does not carry, so nothing in
/// it is verified alive and the public surface would show none of it. The
/// report marks every listing verified first: it shows how the search ranks the
/// whole snapshot, not what the curated bazaar would expose.
#[tokio::test]
#[ignore]
async fn snapshot_report() {
    use x402_rs::discovery_store::{DiscoveryStore, MemoryStore};
    let Ok(path) = std::env::var("BAZAAR_SNAPSHOT") else {
        println!("BAZAAR_SNAPSHOT is not set; nothing to report");
        return;
    };
    let records: Vec<DiscoveryResource> =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("snapshot readable"))
            .expect("snapshot is a catalog");
    let load = || async {
        let store = MemoryStore::new();
        store.save_all(&records).await.unwrap();
        DiscoveryRegistry::with_store(store).await.unwrap()
    };
    let registry = load().await;
    expose_all(&registry).await;
    println!("SNAPSHOT {path}: {} records", records.len());
    println!("SNAPSHOT topHosts {}", registry.stats().await["topHosts"]);

    let (intents, _) = intents();
    for intent in &intents {
        for sort in ["relevance", "tier"] {
            let page = registry
                .list(3, 0, Some(filters(&intent.text, Some(sort))))
                .await;
            let shown: Vec<String> = page
                .items
                .iter()
                .map(|r| format!("{} | {:.60}", r.url, r.description))
                .collect();
            println!("SNAPSHOT {:<28} {sort:<9} {:?}", intent.text, shown);
        }
    }

    // What a full catalog gives up for 600 new services older than everything
    // in it, with the share off and at the default.
    for pct in ["0", "5"] {
        std::env::set_var("DISCOVERY_MAX_HOST_SHARE_PCT", pct);
        let registry = load().await;
        let held_before = registry.count().await;
        let mut feed = waiting_feed(600);
        for (i, r) in feed.iter_mut().enumerate() {
            r.last_updated = 1_000 + i as u64;
            r.source_updated_at = Some(r.last_updated);
        }
        let (admitted, _, _) = registry
            .bulk_import(feed, ImportPolicy::Filtered)
            .await
            .unwrap();
        let held = held_by_host(&registry).await;
        println!(
            "SNAPSHOT share {pct}%: held {held_before} -> {}, waiting listings admitted {admitted}/600, held by host {:?}",
            registry.count().await,
            &held[..held.len().min(10)]
        );
    }
    std::env::remove_var("DISCOVERY_MAX_HOST_SHARE_PCT");
}

/// The fixture as aggregated feed copies, the way the aggregator imports them.
fn aggregated(listings: Vec<DiscoveryResource>) -> Vec<DiscoveryResource> {
    listings
        .into_iter()
        .map(|mut r| {
            r.source = DiscoverySource::Aggregated;
            r.source_facilitator = Some("fixture-feed".to_string());
            r.source_updated_at = Some(r.last_updated);
            r
        })
        .collect()
}

/// Listings from hosts the catalog does not hold, OLDER than everything in it:
/// the coverage a full catalog turns away on date alone.
fn waiting_feed(n: usize) -> Vec<DiscoveryResource> {
    (0..n)
        .map(|i| {
            let mut r = DiscoveryResource::new(
                Url::parse(&format!(
                    "https://newhost{}.example/api/{}",
                    i / 4,
                    ["a", "b", "c", "d"][i % 4]
                ))
                .unwrap(),
                "http".to_string(),
                "A service the catalog has not listed yet.".to_string(),
                vec![usdc(5_000)],
            );
            r.source = DiscoverySource::Aggregated;
            r.source_facilitator = Some("fixture-feed".to_string());
            r.last_updated = 1_780_000_000 + i as u64;
            r.source_updated_at = Some(r.last_updated);
            r
        })
        .collect()
}

#[tokio::test]
async fn a_full_catalog_makes_room_from_families_and_crowded_hosts_first() {
    // The 1 999 fixture listings arrive as feed copies into the default 2 000
    // slots, then 1 200 listings from 300 hosts the catalog has never held,
    // older than all of it. One scenario per share.
    let mut rows = Vec::new();
    for pct in ["0", "2", "5", "10", "20"] {
        std::env::set_var("DISCOVERY_MAX_HOST_SHARE_PCT", pct);
        let registry = DiscoveryRegistry::new();
        registry
            .bulk_import(aggregated(catalog()), ImportPolicy::Filtered)
            .await
            .unwrap();
        // One recent feed listing fills the last slot (recent, so the oldest
        // copy held is still a fixture listing).
        let mut last = waiting_feed(1);
        last[0].last_updated = NEWEST;
        last[0].source_updated_at = Some(NEWEST);
        registry
            .bulk_import(last, ImportPolicy::Filtered)
            .await
            .unwrap();
        assert_eq!(registry.count().await, 2_000);
        let (admitted, _, _) = registry
            .bulk_import(waiting_feed(1_201).split_off(1), ImportPolicy::Filtered)
            .await
            .unwrap();
        let hosts = held_by_host(&registry).await;
        let top4: u64 = hosts.iter().take(4).map(|(_, c)| c).sum();
        let held = registry.count().await;
        println!(
            "share {pct:>2}%: held {held}, top-4 hosts hold {top4} ({:.0} %), waiting listings admitted {admitted}/1200, top {:?}",
            100.0 * top4 as f64 / held as f64,
            &hosts[..4]
        );
        rows.push((pct, top4, admitted, held));
    }
    std::env::remove_var("DISCOVERY_MAX_HOST_SHARE_PCT");

    let off = rows.iter().find(|r| r.0 == "0").unwrap();
    let five = rows.iter().find(|r| r.0 == "5").unwrap();
    // The catalog never grows past its cap, whatever the share.
    assert!(rows.iter().all(|r| r.3 == 2_000), "{rows:?}");
    // The templated families alone make room: by date alone (2.46.1) every one
    // of these older listings would have been refused.
    assert!(off.2 > 0, "{rows:?}");
    // The share only ever adds room, and the four largest hosts end up
    // holding less than half of what four hosts held.
    assert!(five.2 >= off.2, "{rows:?}");
    assert!(five.1 <= off.1, "{rows:?}");
    assert!(five.1 < 1_043 / 2, "{rows:?}");
}

// ============================================================================
// The router's five (2026-10-06)
// ============================================================================
//
// A router partner re-tested the Bazaar on 2.48.0 and reproduced five
// requests that came back wrong. Each is a test here, over the fixture
// catalog (so every word carries the weight it has in a catalog shaped like
// the real one) plus the listings that made it wrong: one that does the job
// and the ones that only share its words.

/// A first-hand listing with one seller per host.
fn tool(url: &str, description: &str) -> DiscoveryResource {
    let url = Url::parse(url).unwrap();
    let pay_to = pay_to_of(&url);
    DiscoveryResource::new(
        url,
        "http".to_string(),
        description.to_string(),
        vec![usdc_to(5_000, &pay_to)],
    )
}

/// `r` declaring the request it takes: `method`, with `fields`.
fn taking(mut r: DiscoveryResource, method: &str, fields: &[&str]) -> DiscoveryResource {
    let fields: serde_json::Map<String, serde_json::Value> = fields
        .iter()
        .map(|f| (f.to_string(), serde_json::json!("example")))
        .collect();
    let slot = if method == "GET" {
        "queryParams"
    } else {
        "body"
    };
    r.extensions = Some(serde_json::json!({
        "bazaar": {"info": {"input": {"type": "http", "method": method, slot: fields}}}
    }));
    r
}

/// The exposed fixture catalog plus `extra`, every listing verified alive.
async fn registry_with(extra: Vec<DiscoveryResource>) -> DiscoveryRegistry {
    let registry = DiscoveryRegistry::new();
    for r in exposed_catalog().into_iter().chain(extra) {
        registry
            .register(r)
            .await
            .expect("fixture listing registers");
    }
    expose_all(&registry).await;
    registry
}

/// The first ten URLs for `q`, in the default order, and how many matched.
async fn search(registry: &DiscoveryRegistry, q: &str) -> (Vec<String>, u32) {
    let page = registry.list(10, 0, Some(filters(q, None))).await;
    let urls = page.items.iter().map(|r| r.url.to_string()).collect();
    (urls, page.pagination.total)
}

#[tokio::test]
async fn keccak_selector_finds_the_keccak_tool_and_not_the_css_scraper() {
    let keccak = "https://evm.tools.example/keccak256";
    let css = "https://scrape.example/css-selector";
    let registry = registry_with(vec![
        tool(
            keccak,
            "Keccak256 of a function signature: the 4-byte function selector and the event topic.",
        ),
        taking(
            tool(
                css,
                "Scrape a page and return the text of every element a CSS selector matches.",
            ),
            "POST",
            &["url", "selector"],
        ),
    ])
    .await;
    let (urls, _) = search(&registry, "keccak selector").await;
    assert_eq!(urls.first().map(String::as_str), Some(keccak), "{urls:?}");
    assert!(
        !urls.contains(&css.to_string()),
        "a page reader does not answer a request for a developer tool: {urls:?}"
    );
}

#[tokio::test]
async fn a_request_for_one_chain_never_gets_another_chains_service() {
    let hyperevm = "https://rpc.hyperevm.example/";
    let distractors = vec![
        tool(
            hyperevm,
            "HyperEVM JSON-RPC endpoint: eth_blockNumber, eth_getBalance, eth_call and the latest block.",
        ),
        tool(
            "https://data.example/solana/volume",
            "Daily Solana DEX volume.",
        ),
        tool(
            "https://news.example/crypto/latest",
            "Get the latest crypto news.",
        ),
    ];
    // No Solana RPC in the catalog: nothing. Empty is better than wrong.
    let registry = registry_with(distractors.clone()).await;
    let (urls, total) = search(&registry, "solana rpc getLatestBlockhash").await;
    assert_eq!(total, 0, "{urls:?}");

    // With one, it comes first, and the HyperEVM one is not offered.
    let solana = "https://solana-rpc.example/";
    let mut listings = distractors;
    listings.push(taking(
        tool(
            solana,
            "Solana JSON-RPC: getLatestBlockhash, getBalance, sendTransaction.",
        ),
        "POST",
        &["method", "params"],
    ));
    let registry = registry_with(listings).await;
    let (urls, _) = search(&registry, "solana rpc getLatestBlockhash").await;
    assert_eq!(urls.first().map(String::as_str), Some(solana), "{urls:?}");
    assert!(!urls.contains(&hyperevm.to_string()), "{urls:?}");

    // And a request for HyperEVM finds that one, not Solana's.
    let (urls, _) = search(&registry, "hyperevm rpc").await;
    assert_eq!(urls.first().map(String::as_str), Some(hyperevm), "{urls:?}");
    assert!(!urls.contains(&solana.to_string()), "{urls:?}");
}

#[tokio::test]
async fn phone_number_lookup_finds_a_lookup_before_any_essay_or_sender() {
    // The publisher's own lookup tool sits on the host of its 379 essays, one
    // of them titled with the request's two first words.
    let tool_url = "https://tenjin.blog/api/phone-lookup";
    let sms = "https://sms.example/send";
    let random = "https://random.example/number";
    let registry = registry_with(vec![
        taking(
            tool(
                tool_url,
                "Phone number lookup: carrier, line type and owner name for a number.",
            ),
            "POST",
            &["phone"],
        ),
        tool(sms, "Send an SMS message to a phone number."),
        tool(random, "Random number generator."),
    ])
    .await;
    let lookups = [
        tool_url,
        "https://tenjin.sh/api/phone-lookup",
        "https://numverify.x402.example/validate",
        "https://callerid.x402.example/lookup",
    ];
    let (urls, _) = search(&registry, "phone number lookup").await;
    assert!(
        lookups.contains(&urls[0].as_str()),
        "a lookup comes first: {urls:?}"
    );
    assert!(
        urls[..3].contains(&tool_url.to_string()),
        "the publisher's tool is not crowded out by its own essays: {urls:?}"
    );
    let last_lookup = urls
        .iter()
        .rposition(|u| lookups.contains(&u.as_str()))
        .unwrap();
    assert!(
        urls[..last_lookup]
            .iter()
            .all(|u| lookups.contains(&u.as_str())),
        "no essay or other listing ahead of a lookup: {urls:?}"
    );
    assert!(!urls.contains(&sms.to_string()), "{urls:?}");
    assert!(!urls.contains(&random.to_string()), "{urls:?}");
}

#[tokio::test]
async fn stock_quote_puts_quotes_first() {
    let swap = "https://dex.example/swap/quote";
    let saying = "https://quotes.example/daily";
    let history = "https://history.example/stock-history/AAPL";
    let registry = registry_with(vec![
        tool(
            swap,
            "Swap quote from a DEX aggregator: best route for a token pair.",
        ),
        tool(saying, "Inspirational quote of the day."),
        tool(history, "Daily stock history for a ticker."),
    ])
    .await;
    let quotes = [
        "https://markets.x402.example/v1/quote",
        "https://finance.x402.example/stocks/price",
        "https://api.losbeto.xyz/stock-quote",
    ];
    let (urls, _) = search(&registry, "stock quote").await;
    let mut top3: Vec<&str> = urls[..3].iter().map(String::as_str).collect();
    top3.sort_unstable();
    let mut want = quotes.to_vec();
    want.sort_unstable();
    assert_eq!(top3, want, "{urls:?}");
    // A stock listing that is not a quote follows the quotes.
    if let Some(at) = urls.iter().position(|u| u == history) {
        assert!(at >= 3, "{urls:?}");
    }
    // A swap quote is crypto and a saying is not a price: neither answers.
    assert!(!urls.contains(&swap.to_string()), "{urls:?}");
    assert!(!urls.contains(&saying.to_string()), "{urls:?}");
}

#[tokio::test]
async fn trending_meme_coins_is_not_a_meme_generator() {
    let generator = "https://memes.example/generator";
    let trending = "https://dex.example/trending";
    let hot = "https://coins.example/memecoins/hot";
    let registry = registry_with(vec![
        tool(
            generator,
            "Meme generator: put your caption on a trending meme template.",
        ),
        tool(
            trending,
            "Trending meme coins on Solana and Base by 24h volume.",
        ),
        tool(hot, "The memecoins traders are buying right now."),
    ])
    .await;
    let (urls, _) = search(&registry, "trending meme coins").await;
    assert_eq!(urls.first().map(String::as_str), Some(trending), "{urls:?}");
    assert!(urls.contains(&hot.to_string()), "{urls:?}");
    assert!(
        !urls.contains(&generator.to_string()),
        "an image tool does not answer a request for crypto data: {urls:?}"
    );
}

#[tokio::test]
async fn one_seller_on_many_hosts_keeps_two_places_and_a_family_one() {
    // Six hosts, one recipient: a reseller. Three of one templated family.
    let reseller = "0x00000000000000000000000000000000000000aa";
    let mut extra: Vec<DiscoveryResource> = (0..6)
        .map(|i| {
            let mut r = tool(
                &format!("https://mirror{i}.resold.example/weather/now"),
                "Current weather conditions for a city: temperature, wind, humidity.",
            );
            r.accepts = vec![usdc_to(5_000, reseller)];
            r
        })
        .collect();
    for ticker in ["AAPL", "MSFT", "NVDA"] {
        extra.push(tool(
            &format!("https://quotes.family.example/quote/{ticker}"),
            "Current stock quote for a ticker symbol.",
        ));
    }
    let registry = registry_with(extra).await;

    let (urls, _) = search(&registry, "current weather conditions in a city").await;
    let resold_at: Vec<usize> = urls
        .iter()
        .enumerate()
        .filter(|(_, u)| u.contains("resold.example"))
        .map(|(i, _)| i)
        .collect();
    // Two keep their rank; the rest wait until every other result was shown.
    let others_after = urls
        .iter()
        .skip(resold_at.get(2).copied().unwrap_or(urls.len()))
        .filter(|u| !u.contains("resold.example"))
        .count();
    assert_eq!(others_after, 0, "{urls:?}");
    assert!(
        urls[..resold_at.get(2).copied().unwrap_or(urls.len())]
            .iter()
            .any(|u| u.starts_with("https://weather.x402.example")),
        "another seller is shown before the reseller's third: {urls:?}"
    );

    let (urls, total) = search(&registry, "stock quote for a ticker").await;
    let first_family = urls
        .iter()
        .position(|u| u.contains("quotes.family.example"))
        .expect("the family answers");
    let second_family = urls
        .iter()
        .skip(first_family + 1)
        .position(|u| u.contains("quotes.family.example"))
        .map(|p| p + first_family + 1);
    if let Some(second) = second_family {
        assert!(
            urls[first_family + 1..second]
                .iter()
                .any(|u| !u.contains("quotes.family.example")),
            "a family keeps one place: {urls:?}"
        );
    }
    // Ranking only: nothing is dropped.
    assert!(total as usize >= 3, "{total}");
}

#[test]
fn most_of_the_fixture_catalog_resolves_to_a_category() {
    // The fixture is shaped after the catalog of 2026-10-01; on 2.48.0 the
    // real one placed 200 of 1 765 listings (11 %) in a category, all from
    // what the seller declared. The paid essays are content and stay out.
    use x402_rs::discovery_taxonomy::{classify, CategorySource, Kind};
    let rows = catalog_rows();
    let (mut placed, mut inferred, mut content) = (0usize, 0usize, 0usize);
    let mut by_category: std::collections::BTreeMap<&str, usize> = Default::default();
    for (r, _) in &rows {
        let c = classify(r);
        if c.kind == Kind::Content {
            content += 1;
        }
        if !c.categories.is_empty() {
            placed += 1;
        }
        if c.source == Some(CategorySource::Inferred) {
            inferred += 1;
        }
        for id in c.categories {
            *by_category.entry(id).or_default() += 1;
        }
    }
    let n = rows.len();
    println!(
        "CATEGORIES {placed}/{n} placed ({:.0} %), {inferred} inferred, {content} content; tools placed {placed}/{} ({:.0} %); {by_category:?}",
        100.0 * placed as f64 / n as f64,
        n - content,
        100.0 * placed as f64 / (n - content) as f64,
    );
    assert!(placed * 2 > n, "{placed}/{n}");
}
