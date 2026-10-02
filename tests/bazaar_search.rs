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
//! Every listing is registered first-hand, so no capacity rule of any build
//! touches it: run against an older build, the same file measures the old
//! search on the same 1 999 listings. Run with `--nocapture` for the table; the
//! assertions are the floor this change must keep, not the score.

use std::collections::HashMap;
use std::time::Instant;

use url::Url;
use x402_rs::caip2::Caip2NetworkId;
use x402_rs::discovery::{DiscoveryRegistry, ImportPolicy};
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
    CatalogPaymentOption::new(
        CatalogScheme::Known(Scheme::Exact),
        Caip2NetworkId::eip155(8453),
        MixedAddress::Evm(USDC_BASE.parse().unwrap()),
        TokenAmount::from(amount),
        MixedAddress::Evm(PAY_TO.parse().unwrap()),
        300,
    )
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
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(CATALOG).expect("catalog fixture parses");
    rows.iter()
        .enumerate()
        .map(|(n, row)| {
            let text = |key: &str| row.get(key).and_then(|v| v.as_str()).map(str::to_string);
            let mut r = DiscoveryResource::new(
                Url::parse(&text("url").expect("every row has a url")).expect("fixture url"),
                text("type").unwrap_or_else(|| "http".to_string()),
                text("description").unwrap_or_default(),
                vec![usdc(1_000 + (n as u64 % 50) * 1_000)],
            );
            r.last_updated = NEWEST - n as u64;
            let (category, provider, tags) =
                (text("category"), text("provider"), strings(&row["tags"]));
            if category.is_some() || provider.is_some() || !tags.is_empty() {
                r.metadata = Some(DiscoveryMetadata {
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
            r
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

/// The catalog as a registry, every listing registered first-hand.
async fn registry() -> DiscoveryRegistry {
    let registry = DiscoveryRegistry::new();
    for r in catalog() {
        registry
            .register(r)
            .await
            .expect("fixture listing registers");
    }
    registry
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
    // The fixture's 379 paid essays are VIP through the shipped manifest, and
    // twelve of them are titled with an intent's own words. Two more views
    // separate that from the ranking: the essays without the VIP tier (content
    // is not to take it), and a router that leaves the content host out.
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
async fn a_request_ranks_the_service_above_a_vip_essay_that_shares_one_word() {
    // The VIP essay says "weather" and nothing else of the request; the
    // service says what was asked. Tier is a boost, not the order.
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
    // ...and the old order, still available by name, puts the VIP first.
    let page = registry
        .list(5, 0, Some(filters("weather", Some("tier"))))
        .await;
    assert_eq!(page.items[0].url.as_str(), essay);
}

#[tokio::test]
async fn one_seller_does_not_fill_the_top_of_a_request() {
    // 86 templated endpoints of one seller say "stock"; a request for a stock
    // quote must still see other sellers in its first page.
    let registry = registry().await;
    let page = registry
        .list(10, 0, Some(filters("stock quote for a ticker", None)))
        .await;
    let losbeto = page
        .items
        .iter()
        .filter(|r| r.url.host_str() == Some("api.losbeto.xyz"))
        .count();
    assert!(losbeto <= 2, "{losbeto} of the first 10 are one host");
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
/// Health is a separate overlay that a snapshot does not carry, so every
/// listing reads as unknown and none is hidden as quarantined.
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
        println!(
            "SNAPSHOT share {pct}%: held {held_before} -> {}, waiting listings admitted {admitted}/600, topHosts {}",
            registry.count().await,
            registry.stats().await["topHosts"]
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
        let stats = registry.stats().await;
        let hosts: Vec<(String, u64)> = stats["topHosts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| {
                (
                    h["host"].as_str().unwrap().to_string(),
                    h["count"].as_u64().unwrap(),
                )
            })
            .collect();
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
