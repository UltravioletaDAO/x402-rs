//! What a listing says it sells survives aggregation, from every source.
//!
//! # What these pin
//!
//! A router choosing a paid service on an agent's behalf needs three things
//! from a listing besides the price: a description, the declared input (what
//! to send) and the declared output (what comes back). On the 2026-10-01
//! catalog snapshot 873 of 2 000 listings had no description and 1 646 had no
//! input declaration, and every one of the 873 came from one source.
//!
//! Two losses produced that, and each has its tests here:
//!
//! 1. **The x402 v1 shape.** v1 put `description` and `outputSchema` on every
//!    payment option, not on the resource. The aggregator read the resource
//!    level only, so a v1 feed arrived with nothing. Both are now lifted, per
//!    the bazaar spec's own v1 mapping.
//! 2. **The merge.** Two sources publishing the same listing were reconciled as
//!    whole records, so a newer copy with an empty description replaced one
//!    that had text. Descriptive fields now only ever fill gaps.
//!
//! Plus the closed vocabulary: `kind` (`api` | `content`) and one `category`
//! per listing, and the rule that content never holds a curated tier above
//! the tools.
//!
//! `fixtures/bazaar/listing-data-pages.json` carries one page per source in the
//! shape that source publishes; hosts are synthetic.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use x402_rs::discovery::{DiscoveryRegistry, ImportPolicy};
use x402_rs::discovery_aggregator::{convert_resources, CoinbaseDiscoveryResponse};
use x402_rs::discovery_curation::CurationManifest;
use x402_rs::discovery_health::{ProbeMethod, ProbeRequest};
use x402_rs::discovery_taxonomy::{classify, taxonomy, CategorySource, Kind};
use x402_rs::types::TokenAmount;
use x402_rs::types_v2::{
    DiscoveryFilters, DiscoveryMetadata, DiscoveryResource, DiscoverySource, Tier,
};

const PAGES: &str = include_str!("fixtures/bazaar/listing-data-pages.json");

/// One source's page, converted exactly as the aggregator converts it.
fn page(source: &str) -> Vec<DiscoveryResource> {
    let all: serde_json::Value = serde_json::from_str(PAGES).expect("fixture is JSON");
    let parsed: CoinbaseDiscoveryResponse = serde_json::from_value(all[source].clone())
        .unwrap_or_else(|e| panic!("{source} page does not parse: {e}"));
    let (items, rejected) = convert_resources(parsed.items, source);
    assert!(
        rejected.is_empty(),
        "{source} dropped options: {rejected:?}"
    );
    items
}

fn by_url(items: &[DiscoveryResource], needle: &str) -> DiscoveryResource {
    items
        .iter()
        .find(|r| r.url.as_str().contains(needle))
        .unwrap_or_else(|| panic!("{needle} was not imported"))
        .clone()
}

fn bazaar(r: &DiscoveryResource) -> &serde_json::Value {
    r.extensions
        .as_ref()
        .and_then(|e| e.get("bazaar"))
        .unwrap_or_else(|| panic!("{} carries no bazaar extension", r.url))
}

// ============================================================================
// description
// ============================================================================

#[test]
fn thirdweb_v1_description_is_lifted_from_the_payment_option() {
    let items = page("thirdweb");
    assert_eq!(
        by_url(&items, "demand-company-acme-revenue").description,
        "Quarterly revenue for ACME Corp, as reported in its filings.",
        "the v1 description lives on the option; surrounding whitespace is the only edit"
    );
    // The first option says nothing; the second one does.
    assert_eq!(
        by_url(&items, "search.v1seller.test").description,
        "Web search over a fresh index; POST a query, get ranked results."
    );
}

#[test]
fn coinbase_v2_description_is_its_own_and_not_the_v1_leftover() {
    let r = by_url(&page("coinbase"), "api.v2seller.test/balance");
    assert_eq!(
        r.description,
        "ERC-20 balance of any wallet, read live from chain."
    );
}

#[test]
fn payai_description_survives() {
    let r = by_url(&page("payai"), "weather");
    assert_eq!(r.description, "Current conditions for a city.");
}

// ============================================================================
// input schema
// ============================================================================

#[test]
fn thirdweb_v1_input_declaration_becomes_bazaar_info_input_verbatim() {
    let items = page("thirdweb");

    let get = by_url(&items, "demand-company-acme-revenue");
    assert!(get.has_input_schema());
    let input = &bazaar(&get)["info"]["input"];
    assert_eq!(input["method"], "GET");
    assert_eq!(input["discoverable"], true);
    assert_eq!(
        input["queryParams"]["quarter"]["description"],
        "e.g. 2026Q2"
    );

    let post = by_url(&items, "search.v1seller.test");
    assert!(post.has_input_schema());
    let input = &bazaar(&post)["info"]["input"];
    assert_eq!(input["method"], "POST");
    assert_eq!(input["bodyFields"]["query"]["required"], true);
    assert!(
        input.get("body").is_none(),
        "a field list is not an example body; nothing may be invented from it"
    );
}

#[test]
fn coinbase_input_schema_survives_and_a_v1_leftover_does_not_replace_it() {
    let r = by_url(&page("coinbase"), "api.v2seller.test/balance");
    assert!(r.has_input_schema());
    let b = bazaar(&r);
    assert_eq!(
        b["info"]["input"]["method"], "GET",
        "the option's v1 leftover says POST; the resource's own declaration wins"
    );
    assert_eq!(b["schema"]["properties"]["input"]["type"], "object");
    assert_eq!(
        r.extensions.as_ref().unwrap()["builder-code"]["code"],
        "abc",
        "other extensions travel untouched"
    );
}

#[test]
fn payai_input_schema_survives() {
    let r = by_url(&page("payai"), "weather");
    assert!(r.has_input_schema());
    assert_eq!(
        bazaar(&r)["info"]["input"]["queryParams"]["city"],
        "Medellin"
    );
}

// ============================================================================
// output schema
// ============================================================================

#[test]
fn thirdweb_v1_output_declaration_becomes_bazaar_info_output() {
    let items = page("thirdweb");

    let split = by_url(&items, "demand-company-acme-revenue");
    assert_eq!(
        bazaar(&split)["info"]["output"]["properties"]["revenueUsd"]["type"],
        "number"
    );

    // A bare object is what the field's name says: the response schema.
    let bare = by_url(&items, "bare.v1seller.test");
    assert_eq!(
        bazaar(&bare)["info"]["output"]["properties"]["price"]["type"],
        "string"
    );
    assert!(bazaar(&bare)["info"].get("input").is_none());
    assert!(
        !bare.has_input_schema(),
        "an output schema says nothing about what to send"
    );
}

#[test]
fn coinbase_and_payai_output_schemas_survive() {
    let cb = by_url(&page("coinbase"), "api.v2seller.test/balance");
    assert_eq!(
        bazaar(&cb)["info"]["output"]["example"]["balance"],
        "1234567890"
    );
    let pa = by_url(&page("payai"), "weather");
    assert_eq!(bazaar(&pa)["info"]["output"]["example"]["tempC"], 24);
}

// ============================================================================
// tags and category
// ============================================================================

#[test]
fn payai_tags_category_and_provider_survive() {
    let r = by_url(&page("payai"), "weather");
    let meta = r.metadata.as_ref().expect("metadata survives");
    assert_eq!(
        meta.tags,
        vec!["weather".to_string(), "forecast".to_string()]
    );
    assert_eq!(
        meta.category.as_deref(),
        Some("Weather"),
        "the seller's spelling, untouched"
    );
    assert_eq!(meta.provider.as_deref(), Some("PayAI Seller"));
}

#[test]
fn coinbase_extension_tags_and_category_survive_verbatim() {
    let r = by_url(&page("coinbase"), "api.v2seller.test/balance");
    let b = bazaar(&r);
    assert_eq!(b["tags"], serde_json::json!(["balances", "erc20"]));
    assert_eq!(b["category"], "Inference");
}

// ============================================================================
// a feed that types its text wrong
// ============================================================================

#[test]
fn a_v1_option_that_types_its_text_wrong_does_not_cost_the_page() {
    let items = page("thirdweb");
    assert_eq!(items.len(), 4, "every listing on the page was imported");
    let r = by_url(&items, "untyped.v1seller.test");
    assert_eq!(r.description, "", "a number is not a description");
    assert!(r.extensions.is_none(), "a string is not a schema");
}

// ============================================================================
// two sources, one listing
// ============================================================================

/// The thirdweb copy as it is stored today: nothing but terms. Newer, and
/// priced differently, so the tests can tell which copy's terms stood.
fn thirdweb_copy_with_nothing_to_say() -> DiscoveryResource {
    let mut r = by_url(&page("thirdweb"), "demand-company-acme-revenue");
    r.description = String::new();
    r.extensions = None;
    r.accepts[0].amount = TokenAmount::from(12_000u64);
    r
}

fn coinbase_copy_with_text() -> DiscoveryResource {
    by_url(&page("coinbase"), "demand-company-acme-revenue")
}

const ACME: &str = "https://market.datapack.test/x402/demand-company-acme-revenue";
const COINBASE_TEXT: &str = "ACME Corp quarterly revenue, from its filings.";

#[tokio::test]
async fn a_newer_copy_with_nothing_to_say_does_not_erase_another_sources_text() {
    let registry = DiscoveryRegistry::new();
    registry
        .bulk_import(vec![coinbase_copy_with_text()], ImportPolicy::Filtered)
        .await
        .unwrap();

    let (_a, updated, _s) = registry
        .bulk_import(
            vec![thirdweb_copy_with_nothing_to_say()],
            ImportPolicy::Filtered,
        )
        .await
        .unwrap();
    assert_eq!(updated, 1, "the newer dated copy's terms land, as before");

    let held = registry.get(ACME).await.unwrap();
    assert_eq!(held.accepts[0].amount, TokenAmount::from(12_000u64));
    assert_eq!(held.source_facilitator.as_deref(), Some("thirdweb"));
    assert_eq!(held.description, COINBASE_TEXT, "but not its silence");
    assert!(held.has_input_schema());
}

#[tokio::test]
async fn an_older_copy_with_text_fills_the_held_empty_one_without_moving_its_terms() {
    let registry = DiscoveryRegistry::new();
    registry
        .bulk_import(
            vec![thirdweb_copy_with_nothing_to_say()],
            ImportPolicy::Filtered,
        )
        .await
        .unwrap();
    let before = registry.get(ACME).await.unwrap();

    let (added, updated, skipped) = registry
        .bulk_import(vec![coinbase_copy_with_text()], ImportPolicy::Filtered)
        .await
        .unwrap();
    assert_eq!(
        (added, updated, skipped),
        (0, 1, 0),
        "the older copy loses the terms and still brings the text"
    );

    let held = registry.get(ACME).await.unwrap();
    assert_eq!(held.description, COINBASE_TEXT);
    assert!(held.has_input_schema());
    assert_eq!(
        held.accepts, before.accepts,
        "the terms that stood are the ones that still stand"
    );
    assert_eq!(held.source_facilitator.as_deref(), Some("thirdweb"));
    assert_eq!(held.source_updated_at, before.source_updated_at);
    assert_eq!(held.last_updated, before.last_updated);
}

#[tokio::test]
async fn an_enriched_record_is_not_rewritten_every_cycle() {
    let registry = DiscoveryRegistry::new();
    for cycle in 0..3 {
        let (added, updated, skipped) = registry
            .bulk_import(
                vec![
                    thirdweb_copy_with_nothing_to_say(),
                    coinbase_copy_with_text(),
                ],
                ImportPolicy::Filtered,
            )
            .await
            .unwrap();
        match cycle {
            0 => assert_eq!((added, updated, skipped), (1, 1, 0), "cycle {cycle}"),
            _ => assert_eq!(
                (added, updated, skipped),
                (0, 0, 2),
                "cycle {cycle}: two feeds republishing the same page change nothing"
            ),
        }
    }
    assert_eq!(registry.get(ACME).await.unwrap().description, COINBASE_TEXT);
}

#[tokio::test]
async fn text_is_never_replaced_by_the_fill_only_never_lost() {
    // The thirdweb copy WITH its lifted v1 text, newer than Coinbase's: the
    // newer copy's own words land. Filling gaps is not a preference for one
    // source's prose.
    let registry = DiscoveryRegistry::new();
    let thirdweb = by_url(&page("thirdweb"), "demand-company-acme-revenue");
    registry
        .bulk_import(vec![coinbase_copy_with_text()], ImportPolicy::Filtered)
        .await
        .unwrap();
    registry
        .bulk_import(vec![thirdweb.clone()], ImportPolicy::Filtered)
        .await
        .unwrap();
    let held = registry.get(ACME).await.unwrap();
    assert_eq!(held.description, thirdweb.description);
    assert_eq!(held.extensions, thirdweb.extensions);
}

#[tokio::test]
async fn a_third_party_copy_never_completes_an_owners_own_listing() {
    // The owner registered the listing and declared no description and no
    // schema. A feed that publishes the same URL must not put words, or a
    // request schema, on a listing it does not own.
    let registry = DiscoveryRegistry::new();
    let mut own = listing(ACME, None);
    own.description = String::new();
    registry.register(own).await.unwrap();

    let (_a, updated, skipped) = registry
        .bulk_import(vec![coinbase_copy_with_text()], ImportPolicy::Filtered)
        .await
        .unwrap();
    assert_eq!((updated, skipped), (0, 1));
    let held = registry.get(ACME).await.unwrap();
    assert_eq!(held.description, "");
    assert!(!held.has_input_schema());
    assert_eq!(held.source, DiscoverySource::SelfRegistered);
}

#[tokio::test]
async fn the_origins_own_document_is_not_completed_by_a_copy_either() {
    // The origin's well-known document outranks the aggregated copy and
    // replaces it whole: what it says, and what it leaves unsaid.
    let registry = DiscoveryRegistry::new();
    registry
        .bulk_import(vec![coinbase_copy_with_text()], ImportPolicy::Filtered)
        .await
        .unwrap();
    let mut crawled = thirdweb_copy_with_nothing_to_say();
    crawled.source = DiscoverySource::Crawled;
    crawled.source_facilitator = Some("market.datapack.test".to_string());
    let (_a, updated, _s) = registry
        .bulk_import(vec![crawled], ImportPolicy::Filtered)
        .await
        .unwrap();
    assert_eq!(updated, 1, "the higher rung wins, as before");
    let held = registry.get(ACME).await.unwrap();
    assert_eq!(held.source, DiscoverySource::Crawled);
    assert_eq!(held.description, "");
    assert!(!held.has_input_schema());
}

// ============================================================================
// kind and category, as a router reads them
// ============================================================================

/// The listing exposes only what is verified alive, so every held record is
/// first given the probe that verifies it -- a readable 402 to the request its
/// listing declares. These tests are about what an exposed listing SAYS, not
/// about whether it is exposed.
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

fn listing(url: &str, category: Option<&str>) -> DiscoveryResource {
    let base = by_url(&page("payai"), "weather");
    let mut r = DiscoveryResource::new(
        url::Url::parse(url).unwrap(),
        "http".to_string(),
        "a listing".to_string(),
        base.accepts.clone(),
    );
    r.metadata = category.map(|c| DiscoveryMetadata {
        category: Some(c.to_string()),
        provider: None,
        tags: Vec::new(),
    });
    r
}

#[tokio::test]
async fn listings_carry_kind_and_category_from_the_closed_list() {
    let registry = DiscoveryRegistry::new();
    let mut all = page("payai");
    all.extend(page("coinbase"));
    registry
        .bulk_import(all, ImportPolicy::Filtered)
        .await
        .unwrap();
    expose_all(&registry).await;

    let listed = registry.list(100, 0, None).await.items;
    let get = |needle: &str| by_url(&listed, needle);

    let weather = get("weather");
    assert_eq!(weather.kind, Some(Kind::Api));
    assert_eq!(weather.categories, vec!["weather".to_string()]);
    assert_eq!(
        weather.category_source,
        Some(CategorySource::Normalized),
        "from `Weather`"
    );
    assert_eq!(weather.has_input_schema, Some(true));

    let balance = get("api.v2seller.test/balance");
    assert_eq!(
        balance.categories,
        vec!["ai".to_string()],
        "from the extension's `Inference`"
    );

    let acme = get("demand-company-acme-revenue");
    assert_eq!(acme.kind, Some(Kind::Api));
    assert!(
        acme.categories.is_empty() && acme.category_source.is_none(),
        "declared nothing, so no category is guessed"
    );
    assert_eq!(acme.has_input_schema, Some(true));

    // Response-only: the held record carries none of them.
    let held = registry.get(ACME).await.unwrap();
    assert_eq!(held.kind, None);
    assert!(held.categories.is_empty());
    assert_eq!((held.category_source, held.has_input_schema), (None, None));
}

#[tokio::test]
async fn the_sellers_declared_category_is_served_exactly_as_declared() {
    // Other systems admit or refuse a listing on `metadata.category`. The
    // normalized ids travel beside it, never in its place.
    let registry = DiscoveryRegistry::new();
    for (i, declared) in ["Weather", "data_processing", "execution", "whatever-it-is"]
        .iter()
        .enumerate()
    {
        registry
            .register(listing(
                &format!("https://seller{i}.example.com/x"),
                Some(declared),
            ))
            .await
            .unwrap();
    }
    expose_all(&registry).await;
    let listed = registry.list(100, 0, None).await.items;
    let mut served: Vec<(String, Vec<String>)> = listed
        .iter()
        .map(|r| {
            (
                r.metadata.as_ref().unwrap().category.clone().unwrap(),
                r.categories.clone(),
            )
        })
        .collect();
    served.sort();
    assert_eq!(
        served,
        vec![
            ("Weather".to_string(), vec!["weather".to_string()]),
            ("data_processing".to_string(), vec!["data".to_string()]),
            ("execution".to_string(), vec![]),
            ("whatever-it-is".to_string(), vec![]),
        ]
    );

    // And the merge never fills a declared category in from another copy.
    let mut empty = by_url(&page("thirdweb"), "demand-company-acme-revenue");
    empty.description = String::new();
    let mut other = coinbase_copy_with_text();
    other.metadata = Some(DiscoveryMetadata {
        category: Some("finance".to_string()),
        provider: Some("Someone".to_string()),
        tags: vec!["revenue".to_string()],
    });
    let registry = DiscoveryRegistry::new();
    registry
        .bulk_import(vec![empty, other], ImportPolicy::Filtered)
        .await
        .unwrap();
    let held = registry.get(ACME).await.unwrap();
    let meta = held.metadata.unwrap();
    assert_eq!(meta.category, None);
    assert_eq!(meta.provider, None);
    assert_eq!(meta.tags, vec!["revenue".to_string()], "tags are conserved");
}

#[tokio::test]
async fn content_never_holds_a_curated_tier_above_the_tools() {
    // The shipped curation manifest names the essay publisher `vip` and
    // MeshRelay `first_party`; the shipped taxonomy says the essays are content.
    let registry = DiscoveryRegistry::new();
    let essay = "https://tenjin.blog/api/read/eth-rd/an-essay";
    let tool = "https://api.meshrelay.xyz/payments/access/alpha";
    registry.register(listing(essay, None)).await.unwrap();
    registry.register(listing(tool, None)).await.unwrap();
    // Only what is verified alive is listed, so both are verified first.
    expose_all(&registry).await;

    let listed = registry.list(100, 0, None).await.items;
    let essay_row = by_url(&listed, "tenjin.blog");
    assert_eq!(essay_row.kind, Some(Kind::Content));
    let cur = essay_row
        .curation
        .as_ref()
        .expect("the publisher is still named");
    assert_eq!(cur.tier, Tier::Verified, "alive, so it earns `verified`");
    assert_eq!(cur.label.as_deref(), Some("Tenjin"));

    let tool_row = by_url(&listed, "meshrelay");
    assert_eq!(tool_row.curation.as_ref().unwrap().tier, Tier::FirstParty);
    assert_eq!(listed[0].url.as_str(), tool, "the tool sorts first");

    let vip = registry
        .list(
            100,
            0,
            Some(DiscoveryFilters {
                tier: Some("vip".to_string()),
                ..Default::default()
            }),
        )
        .await;
    assert_eq!(vip.pagination.total, 0, "no content answers `tier=vip`");

    let stats = registry.stats().await;
    assert_eq!(stats["byKind"]["content"], 1);
    assert_eq!(stats["byKind"]["api"], 1);
    assert!(stats["byTier"].get("vip").is_none());
    assert_eq!(stats["byCategory"]["none"], 2);
    assert_eq!(stats["noDescription"], 0);
    assert_eq!(
        stats["noInputSchema"], 2,
        "neither listing declares an input"
    );
}

#[test]
fn an_alive_essay_earns_verified_and_no_more() {
    let manifest = CurationManifest::load();
    let essay = listing("https://tenjin.blog/api/read/x/y", None);
    assert_eq!(
        manifest.resolve(&essay.url, true).unwrap().tier,
        Tier::Vip,
        "the manifest itself is unchanged"
    );
    assert_eq!(
        manifest.resolve_listing(&essay, true).unwrap().tier,
        Tier::Verified
    );
    // A tool under a curated prefix keeps its tier whatever its health.
    let tool = listing("https://api.meshrelay.xyz/payments/access/x", None);
    assert_eq!(
        manifest.resolve_listing(&tool, false).unwrap().tier,
        Tier::FirstParty
    );
}

#[tokio::test]
async fn the_category_filter_finds_every_spelling_and_still_the_raw_one() {
    let registry = DiscoveryRegistry::new();
    for (i, category) in ["Data", "data_processing", "Stocks", "web", "finance"]
        .iter()
        .enumerate()
    {
        registry
            .register(listing(
                &format!("https://seller{i}.example.com/x"),
                Some(category),
            ))
            .await
            .unwrap();
    }
    expose_all(&registry).await;
    let count = |c: &str| {
        let registry = &registry;
        let c = c.to_string();
        async move {
            registry
                .list(
                    100,
                    0,
                    Some(DiscoveryFilters {
                        category: Some(c),
                        ..Default::default()
                    }),
                )
                .await
                .pagination
                .total
        }
    };
    assert_eq!(count("data").await, 2, "`Data` and `data_processing`");
    assert_eq!(count("DATA").await, 2);
    assert_eq!(count("finance").await, 2, "`finance` and `Stocks`");
    assert_eq!(
        count("web").await,
        1,
        "an unmapped spelling still matches itself"
    );
    assert_eq!(count("social/x").await, 0);
    assert_eq!(count("twitter").await, 0);
}

// ============================================================================
// Measuring a real catalog
// ============================================================================

/// Before/after counts over a catalog snapshot, through the real code paths.
///
/// Not part of the suite: it reads a file that is third-party data and is
/// never committed. Run it against a copy of the stored catalog:
///
/// ```text
/// BAZAAR_SNAPSHOT=/path/to/resources.json \
///   cargo test --test bazaar_listing_data snapshot_report -- --ignored --nocapture
/// ```
///
/// The snapshot is the catalog as STORED, i.e. after aggregation already lost
/// what it lost: description and input schema cannot be recovered from it, and
/// are reported as they stand. The taxonomy and the content tier are resolved
/// at read time, so for those this is the real before and after.
#[test]
#[ignore = "needs BAZAAR_SNAPSHOT pointing at a local copy of the catalog"]
fn snapshot_report() {
    let path = std::env::var("BAZAAR_SNAPSHOT").expect("set BAZAAR_SNAPSHOT");
    let raw = std::fs::read_to_string(&path).expect("read the snapshot");
    let items: Vec<DiscoveryResource> = serde_json::from_str(&raw).expect("a stored catalog");
    let manifest = CurationManifest::load();

    let source_of = |r: &DiscoveryResource| -> String {
        match (&r.source, &r.source_facilitator) {
            (DiscoverySource::Aggregated, Some(f)) => f.clone(),
            (s, _) => s.to_string(),
        }
    };

    let mut total: BTreeMap<String, usize> = BTreeMap::new();
    let mut no_desc: BTreeMap<String, usize> = BTreeMap::new();
    let mut no_input: BTreeMap<String, usize> = BTreeMap::new();
    let mut declared_any = 0usize;
    let mut spellings: BTreeSet<String> = BTreeSet::new();
    let mut by_category: BTreeMap<&str, usize> = BTreeMap::new();
    let mut by_source_of_category: BTreeMap<String, usize> = BTreeMap::new();
    let mut no_category = 0usize;
    let mut by_kind: HashMap<&str, usize> = HashMap::new();
    let (mut vip_before, mut vip_after, mut vip_content_before, mut vip_content_after) =
        (0, 0, 0, 0);

    for r in &items {
        let s = source_of(r);
        *total.entry(s.clone()).or_default() += 1;
        if r.description.trim().is_empty() {
            *no_desc.entry(s.clone()).or_default() += 1;
        }
        if !r.has_input_schema() {
            *no_input.entry(s).or_default() += 1;
        }

        let mut declared = Vec::new();
        if let Some(c) = r.metadata.as_ref().and_then(|m| m.category.clone()) {
            declared.push(c);
        }
        if let Some(c) = r
            .extensions
            .as_ref()
            .and_then(|e| e.get("bazaar"))
            .and_then(|b| b.get("category"))
            .and_then(|c| c.as_str())
        {
            declared.push(c.to_string());
        }
        if let Some(c) = r.accepts.iter().find_map(|a| {
            a.extra
                .as_ref()
                .and_then(|e| e.get("bazaar"))
                .and_then(|b| b.get("category"))
                .and_then(|c| c.as_str())
                .map(str::to_string)
        }) {
            declared.push(c);
        }
        if !declared.is_empty() {
            declared_any += 1;
        }
        spellings.extend(declared);

        let class = classify(r);
        if class.categories.is_empty() {
            no_category += 1;
        }
        for id in &class.categories {
            *by_category.entry(*id).or_default() += 1;
        }
        *by_source_of_category
            .entry(format!("{:?}", class.source))
            .or_default() += 1;
        *by_kind.entry(class.kind.as_str()).or_default() += 1;

        // No health overlay in a stored snapshot: resolved as not alive.
        let before = manifest.resolve(&r.url, false).map(|c| c.tier);
        let after = manifest.resolve_listing(r, false).map(|c| c.tier);
        let content = class.kind == Kind::Content;
        if before == Some(Tier::Vip) {
            vip_before += 1;
            vip_content_before += usize::from(content);
        }
        if after == Some(Tier::Vip) {
            vip_after += 1;
            vip_content_after += usize::from(content);
        }
    }

    let n = items.len();
    let with_category = n - no_category;
    let canonical: BTreeSet<&str> = by_category.keys().copied().collect();
    println!("snapshot: {path}");
    println!("records: {n}  by source: {total:?}");
    println!(
        "empty description: {} {no_desc:?}",
        no_desc.values().sum::<usize>()
    );
    println!(
        "no input schema: {} {no_input:?}",
        no_input.values().sum::<usize>()
    );
    println!(
        "BEFORE category: {declared_any} listings declare one, in {} spellings: {spellings:?}",
        spellings.len()
    );
    println!(
        "AFTER  categories: {with_category} listings resolve to at least one of {} closed-list ids in use: {by_category:?}",
        canonical.len()
    );
    println!("AFTER  categorySource: {by_source_of_category:?}");
    println!("AFTER  kind: {by_kind:?}");
    println!(
        "vip tier: before {vip_before} (content {vip_content_before}) -> after {vip_after} (content {vip_content_after})"
    );
    println!(
        "closed list ({}): {:?}",
        taxonomy().categories().count(),
        taxonomy().categories().map(|(c, _)| c).collect::<Vec<_>>()
    );
}
