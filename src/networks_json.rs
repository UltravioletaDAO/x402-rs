//! `GET /networks.json`: how to present every network this facilitator serves.
//!
//! `/supported` says which networks, schemes and tokens exist. It does not say
//! what to call a network, which icon to draw or where its transactions can be
//! looked up, so every client that shows a network picker kept its own table of
//! those, by hand. This document is that table, published once.
//!
//! # Where each field comes from
//!
//! - **The rows are `/supported`.** [`document`] reads the `/supported` body
//!   itself and groups its entries by chain: a network has a row if and only if
//!   `/supported` names it, under the same two identifiers (`id`, the x402 v1
//!   name, and `caip2`; native Hedera has no v1 name, so its `id` is its CAIP-2
//!   id, as on `/supported`). `schemes` and each token's `address` and
//!   `decimals` are read off those same entries.
//! - `family`, `chainId` and `testnet` come from the [`Network`] enum, and a
//!   token's `eip712` from [`find_known_eip712_metadata`], the table `/verify`
//!   resolves EIP-712 domains with.
//! - Only presentation comes from `config/supported_tokens.json`, compiled in:
//!   `displayName`, the icon file, the explorer, and a token's `usdPegged` and
//!   icon.
//!
//! A served network the JSON does not describe keeps its row, with its `id` as
//! `displayName` and `explorer` / `icon` null. Dropping it would make this
//! endpoint disagree with `/supported`, which is the one thing it must never
//! do; a null is what the tests below turn red before it ships.
//!
//! Nothing here is added to `/supported`, which is protocol.

use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use crate::chain::evm::find_known_eip712_metadata;
use crate::network::{resolve_network, Network, NetworkFamily};
use crate::types::TokenType;

/// The presentation half of the document, compiled into the binary.
///
/// Also read by `scripts/arc_canary.py` and `scripts/scan/scan_evm.py`, which
/// is why its layout (one object per chain family, keyed by network id) stays.
pub const SUPPORTED_TOKENS_JSON: &str = include_str!("../config/supported_tokens.json");

/// Where the icons are served from when `FACILITATOR_URL` is unset: the icons
/// are compiled into the binary, so they are the same bytes everywhere.
const DEFAULT_PUBLIC_URL: &str = "https://facilitator.ultravioletadao.xyz";

/// The explorer of one network, as `config/supported_tokens.json` spells it.
#[derive(Debug, Clone, Deserialize)]
pub struct ExplorerPaths {
    /// Appended to `explorer`; carries a `{tx}` placeholder.
    pub tx: String,
    /// Appended to `explorer`; carries an `{address}` placeholder.
    pub address: String,
}

/// One network entry of `config/supported_tokens.json`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkMeta {
    pub display_name: Option<String>,
    /// File stem of a PNG served at `/<icon>.png`.
    pub icon: Option<String>,
    /// The explorer's base URL, no trailing slash.
    pub explorer: Option<String>,
    pub explorer_paths: Option<ExplorerPaths>,
    // The four fields below are not part of the document: they are what the
    // drift tests read to tie this file to `/supported` and to `Network`.
    /// Lowercase token ids, exactly as `/supported` names them.
    #[serde(default)]
    #[cfg_attr(not(test), allow(dead_code))]
    pub tokens: Vec<String>,
    /// `false` where production publishes no `exact` entry for the network;
    /// `tokens` is then empty, and the entry's `_note` says why.
    #[serde(default = "served")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub exact_served: bool,
    #[cfg_attr(not(test), allow(dead_code))]
    pub chain_id: Option<u64>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub caip2: Option<String>,
}

fn served() -> bool {
    true
}

/// One entry of `token_info`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenMeta {
    /// Ticker, as shown to a person.
    pub name: String,
    pub usd_pegged: bool,
    /// File stem of a PNG served at `/<icon>.png`, or null when none exists.
    pub icon: Option<String>,
}

/// `config/supported_tokens.json`, parsed.
#[derive(Debug, Clone)]
pub struct Catalog {
    /// Keyed by the identifier `/supported` uses as the v1 name (the CAIP-2 id
    /// for native Hedera).
    pub networks: BTreeMap<String, NetworkMeta>,
    /// Keyed by lowercase token id.
    pub tokens: BTreeMap<String, TokenMeta>,
}

impl Catalog {
    /// Parse the JSON. Network entries live in the top-level objects whose key
    /// ends in `_networks`, `_mainnets` or `_testnets`; keys starting with `_`
    /// are comments.
    pub fn parse(text: &str) -> Result<Self, String> {
        let root: Value = serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
        let root = root.as_object().ok_or("the root is not an object")?;
        let mut networks = BTreeMap::new();
        for (group, entries) in root {
            if !is_network_group(group) {
                continue;
            }
            let entries = entries
                .as_object()
                .ok_or_else(|| format!("`{group}` is not an object"))?;
            for (id, entry) in entries {
                if id.starts_with('_') {
                    continue;
                }
                let meta: NetworkMeta = serde_json::from_value(entry.clone())
                    .map_err(|e| format!("`{group}.{id}`: {e}"))?;
                if networks.insert(id.clone(), meta).is_some() {
                    return Err(format!("`{id}` is listed twice"));
                }
            }
        }
        let mut tokens = BTreeMap::new();
        if let Some(info) = root.get("token_info").and_then(Value::as_object) {
            for (id, entry) in info {
                let meta: TokenMeta = serde_json::from_value(entry.clone())
                    .map_err(|e| format!("`token_info.{id}`: {e}"))?;
                tokens.insert(id.clone(), meta);
            }
        }
        Ok(Self { networks, tokens })
    }

    /// The entry for a chain, found by the identifier `/supported` gives it.
    pub fn network(&self, network: Network) -> Option<&NetworkMeta> {
        self.networks.get(&v1_id(network))
    }
}

fn is_network_group(key: &str) -> bool {
    ["_networks", "_mainnets", "_testnets"]
        .iter()
        .any(|suffix| key.ends_with(suffix))
}

/// The compiled catalog. An `Err` is answered as a 500 by the handler; the
/// tests keep it from ever being one.
pub static CATALOG: Lazy<Result<Catalog, String>> =
    Lazy::new(|| Catalog::parse(SUPPORTED_TOKENS_JSON));

/// The identifier `/supported` publishes as a chain's v1 name: its x402 v1
/// name, or its CAIP-2 id where there is none (native Hedera). The same rule
/// `facilitator_local` uses for `networkAliases`.
pub fn v1_id(network: Network) -> String {
    if network.supports_v1() {
        network.to_string()
    } else {
        network.to_caip2()
    }
}

/// A chain family as a stable lowercase word, the spelling
/// `/.well-known/x402` already uses. Exhaustive, so a new family does not
/// compile until it has one.
pub fn family_slug(family: NetworkFamily) -> &'static str {
    match family {
        NetworkFamily::Evm => "evm",
        NetworkFamily::Solana => "svm",
        NetworkFamily::Near => "near",
        NetworkFamily::Stellar => "stellar",
        #[cfg(feature = "hedera")]
        NetworkFamily::Hedera => "hedera",
        #[cfg(feature = "xrpl")]
        NetworkFamily::Xrpl => "xrpl",
        #[cfg(feature = "algorand")]
        NetworkFamily::Algorand => "algorand",
        #[cfg(feature = "sui")]
        NetworkFamily::Sui => "sui",
    }
}

/// The URL the icons are published under: `FACILITATOR_URL` (the variable
/// `discovery_attestation` already reads), else production.
pub fn public_url() -> String {
    std::env::var("FACILITATOR_URL")
        .ok()
        .map(|url| url.trim().trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| DEFAULT_PUBLIC_URL.to_string())
}

/// Build the document from a `/supported` body.
///
/// `supported` is the JSON `/supported` answers with, not the internal struct,
/// so what is grouped here is exactly what a client of `/supported` reads.
pub fn document(supported: &Value, catalog: &Catalog, public_url: &str) -> Value {
    struct Row {
        schemes: BTreeSet<String>,
        tokens: Vec<Value>,
        seen_tokens: BTreeSet<String>,
    }

    let mut rows: BTreeMap<(bool, String), (Network, Row)> = BTreeMap::new();
    for kind in supported["kinds"].as_array().into_iter().flatten() {
        let Some(name) = kind["network"].as_str() else {
            continue;
        };
        let Some(network) = resolve_network(name) else {
            // Every entry is built from a `Network`, so this does not happen;
            // if it ever does, the router test comparing this document with
            // `/supported` is what says so.
            tracing::warn!(
                network = name,
                "/networks.json: /supported names a chain it cannot resolve"
            );
            continue;
        };
        let (_, row) = rows
            .entry((network.is_testnet(), v1_id(network)))
            .or_insert_with(|| {
                (
                    network,
                    Row {
                        schemes: BTreeSet::new(),
                        tokens: Vec::new(),
                        seen_tokens: BTreeSet::new(),
                    },
                )
            });
        if let Some(scheme) = kind["scheme"].as_str() {
            row.schemes.insert(scheme.to_string());
        }
        for token in kind["extra"]["tokens"].as_array().into_iter().flatten() {
            let Some(id) = token["token"].as_str() else {
                continue;
            };
            if row.seen_tokens.insert(id.to_string()) {
                row.tokens
                    .push(token_entry(network, id, token, catalog, public_url));
            }
        }
    }

    let networks: Vec<Value> = rows
        .into_values()
        .map(|(network, row)| {
            let meta = catalog.network(network);
            let id = v1_id(network);
            let caip2 = network.to_caip2();
            let chain_id = caip2
                .strip_prefix("eip155:")
                .and_then(|n| n.parse::<u64>().ok());
            let explorer = meta.and_then(|m| {
                let base = m.explorer.as_deref()?;
                let paths = m.explorer_paths.as_ref()?;
                Some(json!({
                    "base": base,
                    "tx": format!("{base}{}", paths.tx),
                    "address": format!("{base}{}", paths.address),
                }))
            });
            let icon = meta
                .and_then(|m| m.icon.as_deref())
                .map(|stem| format!("{public_url}/{stem}.png"));
            let display_name = meta
                .and_then(|m| m.display_name.clone())
                .unwrap_or_else(|| id.clone());
            let mut entry = Map::new();
            entry.insert("id".into(), json!(id));
            entry.insert("caip2".into(), json!(caip2));
            entry.insert("family".into(), json!(family_slug(network.into())));
            entry.insert("chainId".into(), json!(chain_id));
            entry.insert("testnet".into(), json!(network.is_testnet()));
            entry.insert("displayName".into(), json!(display_name));
            entry.insert("explorer".into(), explorer.unwrap_or(Value::Null));
            entry.insert("icon".into(), json!(icon));
            entry.insert("schemes".into(), json!(row.schemes));
            entry.insert("tokens".into(), Value::Array(row.tokens));
            Value::Object(entry)
        })
        .collect();

    json!({ "networks": networks })
}

/// One token of a row: what `/supported` published, plus its presentation.
fn token_entry(
    network: Network,
    id: &str,
    published: &Value,
    catalog: &Catalog,
    public_url: &str,
) -> Value {
    let meta = catalog.tokens.get(id);
    let token_type: Option<TokenType> = serde_json::from_value(json!(id)).ok();
    let symbol = meta
        .map(|m| m.name.clone())
        .or_else(|| token_type.map(|t| t.symbol().to_string()))
        .unwrap_or_else(|| id.to_ascii_uppercase());
    let address = published["address"].clone();
    // The domain `/verify` itself resolves; an EVM token outside the static
    // table has none to publish, and nothing else has an EIP-712 domain.
    let eip712 = match NetworkFamily::from(network) {
        NetworkFamily::Evm => address
            .as_str()
            .and_then(|a| alloy::primitives::Address::from_str(a).ok())
            .and_then(|a| find_known_eip712_metadata(network, &a))
            .map(|(name, version)| json!({ "name": name, "version": version })),
        _ => None,
    };
    json!({
        "symbol": symbol,
        "address": address,
        "decimals": published["decimals"].clone(),
        "eip712": eip712,
        "usdPegged": meta.map(|m| m.usd_pegged),
        "icon": meta
            .and_then(|m| m.icon.as_deref())
            .map(|stem| format!("{public_url}/{stem}.png")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> &'static Catalog {
        CATALOG
            .as_ref()
            .expect("config/supported_tokens.json parses")
    }

    /// `/supported` as production published it at 2026-09-24T19:39Z (2.40.0),
    /// with one edit: the eight Sui 32-byte values (two fee payers and two coin
    /// types, each under both network names) are cut to `0xabcd...wxyz`,
    /// because the repository's pre-commit hook refuses any `0x` + 64 hex.
    /// Nothing below reads them; `curl -s
    /// https://facilitator.ultravioletadao.xyz/supported` re-derives them.
    fn production() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/supported-2.40.0.json"))
            .expect("the fixture parses")
    }

    fn published_ids(supported: &Value) -> BTreeSet<String> {
        supported["kinds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| k["network"].as_str().unwrap().to_string())
            .collect()
    }

    fn row_ids(doc: &Value) -> BTreeSet<String> {
        doc["networks"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|r| [r["id"].clone(), r["caip2"].clone()])
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    }

    fn row<'a>(doc: &'a Value, id: &str) -> &'a Value {
        doc["networks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id)
            .unwrap_or_else(|| panic!("no row for {id}"))
    }

    fn symbols(row: &Value) -> Vec<String> {
        row["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["symbol"].as_str().unwrap().to_string())
            .collect()
    }

    /// The identifiers of the document are exactly the identifiers of
    /// `/supported`: every `network` it names is a row's `id` or `caip2`, and
    /// no row names anything it does not. Measured on production's own answer.
    #[test]
    fn the_rows_are_the_networks_supported_names() {
        let supported = production();
        let doc = document(&supported, catalog(), DEFAULT_PUBLIC_URL);
        assert_eq!(row_ids(&doc), published_ids(&supported));
        let rows = doc["networks"].as_array().unwrap();
        assert_eq!(
            rows.len(),
            43,
            "41 v1 names plus the two native Hedera ledgers"
        );
        assert_eq!(rows.iter().filter(|r| r["testnet"] == false).count(), 23);
    }

    /// Every row can be drawn and linked: a name, an icon, and an explorer
    /// with both templates.
    #[test]
    fn every_row_has_a_name_an_icon_and_an_explorer() {
        let doc = document(&production(), catalog(), DEFAULT_PUBLIC_URL);
        for row in doc["networks"].as_array().unwrap() {
            let id = row["id"].as_str().unwrap();
            assert!(
                row["displayName"].as_str().is_some_and(|n| n != id),
                "{id}: no displayName"
            );
            let icon = row["icon"]
                .as_str()
                .unwrap_or_else(|| panic!("{id}: no icon"));
            assert!(
                icon.starts_with("https://") && icon.ends_with(".png"),
                "{id}: {icon}"
            );
            let explorer = &row["explorer"];
            let base = explorer["base"]
                .as_str()
                .unwrap_or_else(|| panic!("{id}: no explorer"));
            assert!(
                base.starts_with("https://") && !base.ends_with('/'),
                "{id}: {base}"
            );
            for (kind, placeholder) in [("tx", "{tx}"), ("address", "{address}")] {
                let template = explorer[kind].as_str().unwrap();
                assert!(
                    template.starts_with(base),
                    "{id}: {kind} is not under {base}"
                );
                assert_eq!(template.matches(placeholder).count(), 1, "{id}: {template}");
            }
        }
    }

    /// What production serves on the three chains the JSON had wrong, and the
    /// fields a picker draws from.
    #[test]
    fn bsc_sui_and_hedera_read_as_served() {
        let doc = document(&production(), catalog(), DEFAULT_PUBLIC_URL);
        let bsc = row(&doc, "bsc");
        assert_eq!(
            symbols(bsc),
            ["AUSD"],
            "BSC USDC has no ERC-3009: not served"
        );
        assert_eq!(bsc["chainId"], 56);
        assert_eq!(bsc["family"], "evm");
        assert_eq!(bsc["tokens"][0]["eip712"]["name"], "Agora Dollar");
        assert_eq!(bsc["tokens"][0]["usdPegged"], true);
        assert_eq!(bsc["schemes"], json!(["exact", "upto"]));

        let sui = row(&doc, "sui");
        assert_eq!(symbols(sui), ["USDC"]);
        assert_eq!(sui["family"], "sui");
        assert!(sui["chainId"].is_null());
        assert!(sui["tokens"][0]["eip712"].is_null());

        let hedera = row(&doc, "hedera:mainnet");
        assert_eq!(hedera["caip2"], "hedera:mainnet");
        assert_eq!(hedera["family"], "hedera");
        assert_eq!(hedera["testnet"], false);
        assert_eq!(symbols(hedera), ["USDC"]);
        assert_eq!(hedera["tokens"][0]["address"], "0.0.456858");
        assert_eq!(hedera["icon"], format!("{DEFAULT_PUBLIC_URL}/hedera.png"));

        let base = row(&doc, "base");
        assert_eq!(base["caip2"], "eip155:8453");
        assert_eq!(
            base["tokens"][0]["eip712"],
            json!({"name": "USD Coin", "version": "2"})
        );
        assert_eq!(
            row(&doc, "base-sepolia")["tokens"][0]["eip712"]["name"],
            "USDC"
        );
        assert_eq!(row(&doc, "arc")["tokens"][1]["symbol"], "EURC");
        assert_eq!(row(&doc, "arc")["tokens"][1]["usdPegged"], false);
        let xrpl = row(&doc, "xrpl");
        let xrp = xrpl["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["symbol"] == "XRP")
            .unwrap();
        assert_eq!(xrp["usdPegged"], false);
        assert!(
            xrp["icon"].is_null(),
            "XRP has no image; a picker must not invent one"
        );
        assert_eq!(row(&doc, "ethereum-sepolia")["tokens"], json!([]));
    }

    /// A network the JSON does not describe keeps its row: the document never
    /// disagrees with `/supported`, it shows the gap instead.
    #[test]
    fn a_served_network_without_metadata_keeps_its_row() {
        let mut bare = catalog().clone();
        bare.networks.remove("scroll");
        let supported = production();
        let doc = document(&supported, &bare, DEFAULT_PUBLIC_URL);
        assert_eq!(row_ids(&doc), published_ids(&supported));
        let scroll = row(&doc, "scroll");
        assert!(scroll["explorer"].is_null() && scroll["icon"].is_null());
        assert_eq!(scroll["displayName"], "scroll");
    }

    /// A facilitator that serves every chain this build knows, each through
    /// the token list its provider publishes, plus one `escrow` and the FHE
    /// entry, named the way `FacilitatorLocal` names them.
    #[derive(Clone)]
    struct EveryChain;

    impl crate::facilitator::Facilitator for EveryChain {
        type Error = crate::chain::FacilitatorLocalError;
        async fn verify(
            &self,
            _: &crate::types::VerifyRequest,
        ) -> Result<crate::types::VerifyResponse, Self::Error> {
            Err(crate::chain::FacilitatorLocalError::Other(
                "not here".into(),
            ))
        }
        async fn settle(
            &self,
            _: &crate::types::SettleRequest,
        ) -> Result<crate::types::SettleResponse, Self::Error> {
            Err(crate::chain::FacilitatorLocalError::Other(
                "not here".into(),
            ))
        }
        async fn supported(
            &self,
        ) -> Result<crate::types::SupportedPaymentKindsResponse, Self::Error> {
            use crate::types::{
                Scheme, SupportedPaymentKind, SupportedPaymentKindExtra, X402Version,
            };
            let mut kinds: Vec<SupportedPaymentKind> = Network::variants()
                .iter()
                .filter(|n| **n != Network::EthereumSepolia)
                .map(|&network| SupportedPaymentKind {
                    x402_version: if network.supports_v1() {
                        X402Version::V1
                    } else {
                        X402Version::V2
                    },
                    scheme: Scheme::Exact,
                    network: v1_id(network),
                    network_aliases: None,
                    extra: Some(SupportedPaymentKindExtra {
                        fee_payer: None,
                        tokens: Some(served_token_infos(network)),
                        escrow: None,
                    }),
                })
                .collect();
            kinds.push(SupportedPaymentKind {
                x402_version: X402Version::V1,
                scheme: Scheme::FheTransfer,
                network: "ethereum-sepolia".into(),
                network_aliases: None,
                extra: None,
            });
            kinds.push(SupportedPaymentKind {
                x402_version: X402Version::V2,
                scheme: Scheme::Upto,
                network: Network::Base.to_caip2(),
                network_aliases: None,
                extra: None,
            });
            Ok(crate::types::SupportedPaymentKindsResponse {
                kinds: crate::facilitator_local::advertise_under_both_network_forms(kinds),
            })
        }
    }

    async fn get(
        router: axum::Router,
        path: &str,
    ) -> (axum::http::StatusCode, axum::http::HeaderMap, Value) {
        use tower::ServiceExt;
        let response = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(path)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let (parts, body) = response.into_parts();
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        (
            parts.status,
            parts.headers,
            serde_json::from_slice(&bytes).unwrap(),
        )
    }

    /// Through the router, `GET /networks.json` names exactly the networks
    /// `GET /supported` names, whatever the facilitator serves: this is the
    /// check that fails when a row is dropped or invented, and the one the
    /// closing command in the handoff runs against production.
    #[tokio::test]
    async fn networks_json_names_what_supported_names() {
        use axum::routing::get as route;
        let router = axum::Router::new()
            .route(
                "/supported",
                route(crate::handlers::get_supported::<EveryChain>),
            )
            .route(
                "/networks.json",
                route(crate::handlers::get_networks_json::<EveryChain>),
            )
            .with_state(EveryChain);
        let (status, _, supported) = get(router.clone(), "/supported").await;
        assert_eq!(status, 200);
        let (status, headers, doc) = get(router, "/networks.json").await;
        assert_eq!(status, 200);
        assert_eq!(headers["cache-control"], "public, max-age=300");
        assert_eq!(row_ids(&doc), published_ids(&supported));
        assert_eq!(
            doc["networks"].as_array().unwrap().len(),
            Network::variants().len()
        );
        let base = row(&doc, "base");
        assert_eq!(
            base["schemes"],
            json!(["exact", "upto"]),
            "a scheme published under one name counts for the chain"
        );
        assert_eq!(
            row(&doc, "ethereum-sepolia")["schemes"],
            json!(["fhe-transfer"])
        );
        for row in doc["networks"].as_array().unwrap() {
            assert!(
                row["explorer"].is_object() && row["icon"].is_string(),
                "{}",
                row["id"]
            );
        }
    }

    fn served_token_infos(network: Network) -> Vec<crate::types::SupportedTokenInfo> {
        match network {
            #[cfg(feature = "xrpl")]
            Network::Xrpl | Network::XrplTestnet => crate::chain::xrpl::payment_tokens(network),
            #[cfg(feature = "hedera")]
            Network::Hedera | Network::HederaTestnet => {
                crate::chain::hedera::payment_tokens(network)
            }
            _ => crate::network::exact_payment_tokens(network),
        }
    }

    /// The tokens the `exact` provider for `network` publishes in `/supported`,
    /// through the same function each provider calls.
    fn served_tokens(network: Network) -> BTreeSet<String> {
        served_token_infos(network)
            .into_iter()
            .map(|t| {
                serde_json::to_value(t.token)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    /// `config/supported_tokens.json` names, for every chain, exactly the
    /// tokens `/supported` publishes for it. It called itself the source of
    /// truth and was read by nothing that served: BSC listed USDC (never
    /// served, no ERC-3009), Sui listed AUSD (the Sui provider accepts USDC
    /// only) and Hedera was missing. Where production serves no `exact` entry
    /// at all (Ethereum Sepolia), the entry says so with `exactServed: false`.
    #[test]
    fn the_json_lists_the_tokens_supported_publishes() {
        let catalog = catalog();
        for (id, meta) in &catalog.networks {
            let Some(network) = resolve_network(id) else {
                continue; // a family not compiled into this build
            };
            let listed: BTreeSet<String> = meta.tokens.iter().cloned().collect();
            assert_eq!(
                listed.len(),
                meta.tokens.len(),
                "{id}: a token is listed twice"
            );
            let expected = if meta.exact_served {
                served_tokens(network)
            } else {
                BTreeSet::new()
            };
            assert_eq!(
                listed, expected,
                "{id}: config/supported_tokens.json and /supported disagree"
            );
        }
        let tokens = |id: &str| catalog.networks[id].tokens.clone();
        assert_eq!(tokens("bsc"), ["ausd"]);
        #[cfg(feature = "sui")]
        assert_eq!(tokens("sui"), ["usdc"]);
        #[cfg(feature = "hedera")]
        {
            assert_eq!(tokens("hedera:mainnet"), ["usdc"]);
            assert_eq!(tokens("hedera:testnet"), ["usdc"]);
        }
        assert!(!catalog.networks["ethereum-sepolia"].exact_served);
    }

    /// Every network the enum can serve has an entry, and every entry is one
    /// the enum can serve, under the identifier `/supported` uses. The chain
    /// ids and CAIP-2 ids the JSON repeats are the code's.
    #[test]
    fn the_json_describes_every_servable_network() {
        let catalog = catalog();
        for &network in Network::variants() {
            let meta = catalog.network(network).unwrap_or_else(|| {
                panic!("config/supported_tokens.json has no `{}`", v1_id(network))
            });
            let caip2 = network.to_caip2();
            if let Some(chain_id) = caip2.strip_prefix("eip155:") {
                assert_eq!(
                    meta.chain_id.map(|c| c.to_string()).as_deref(),
                    Some(chain_id),
                    "{network}"
                );
            }
            if let Some(listed) = &meta.caip2 {
                assert_eq!(listed, &caip2, "{network}");
            }
        }
        for id in catalog.networks.keys() {
            match resolve_network(id) {
                Some(network) => assert_eq!(
                    &v1_id(network),
                    id,
                    "`{id}` is not the name /supported uses"
                ),
                None => assert!(
                    ["algorand", "sui", "xrpl", "hedera"]
                        .iter()
                        .any(|f| id.starts_with(f)),
                    "`{id}` is not a network"
                ),
            }
        }
    }

    /// Every served token has presentation, and `usdPegged` agrees with the
    /// currency the code gives it.
    #[test]
    fn every_served_token_is_described() {
        let catalog = catalog();
        for &network in Network::variants() {
            for id in served_tokens(network) {
                let meta = catalog
                    .tokens
                    .get(&id)
                    .unwrap_or_else(|| panic!("token_info has no `{id}` ({network} serves it)"));
                let token: TokenType = serde_json::from_value(json!(id)).unwrap();
                assert_eq!(meta.usd_pegged, token.currency_symbol() == "$", "{id}");
                assert_eq!(meta.name, token.symbol(), "{id}");
            }
        }
    }

    /// Every icon the JSON names is a PNG in `static/` that the router serves
    /// at `/<icon>.png`.
    #[tokio::test]
    async fn every_icon_is_served() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let catalog = catalog();
        let icons: BTreeSet<&str> = catalog
            .networks
            .values()
            .filter_map(|m| m.icon.as_deref())
            .chain(catalog.tokens.values().filter_map(|m| m.icon.as_deref()))
            .collect();
        assert!(icons.len() >= 29, "{icons:?}");
        let router: axum::Router = crate::handlers::image_routes();
        for icon in icons {
            let path = format!("/{icon}.png");
            let response = router
                .clone()
                .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(response.headers()["content-type"], "image/png", "{path}");
        }
    }

    /// No explorer and no network or token icon is typed by hand in
    /// `static/`: the pages read them from `/networks.json`.
    ///
    /// Allowed: CSS that sizes an image by its file (`[src="/arc.png"]`, from
    /// the measured opaque share of that PNG), and DeBank, which is a
    /// portfolio view of a whole EVM family rather than one network's explorer.
    #[test]
    fn static_types_no_explorer_and_no_icon() {
        let catalog = catalog();
        let hosts: BTreeSet<String> = catalog
            .networks
            .values()
            .filter_map(|m| m.explorer.as_deref())
            .map(|url| {
                let host = url.trim_start_matches("https://");
                host.split('/').next().unwrap().to_string()
            })
            .collect();
        let icons: BTreeSet<&str> = catalog
            .networks
            .values()
            .filter_map(|m| m.icon.as_deref())
            .chain(catalog.tokens.values().filter_map(|m| m.icon.as_deref()))
            .collect();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("static");
        let mut scanned = 0;
        let mut found = Vec::new();
        let mut stack = vec![dir.clone()];
        while let Some(path) = stack.pop() {
            for entry in std::fs::read_dir(&path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue; // images and fonts
                };
                scanned += 1;
                let name = path.strip_prefix(&dir).unwrap().display().to_string();
                for host in &hosts {
                    if text.contains(host.as_str()) {
                        found.push(format!("{name}: explorer `{host}`"));
                    }
                }
                for icon in &icons {
                    for literal in [format!("\"/{icon}.png\""), format!("'/{icon}.png'")] {
                        for (at, _) in text.match_indices(&literal) {
                            // `.network-logo[src="/arc.png"]` sizes that file.
                            if !text[..at].ends_with("[src=") {
                                found.push(format!("{name}: icon `{literal}`"));
                            }
                        }
                    }
                }
                for map in ["ICONO_DE_RED", "ICONO_DE_TOKEN", "const EXPLORER"] {
                    if text.contains(map) {
                        found.push(format!("{name}: `{map}`"));
                    }
                }
            }
        }
        assert!(scanned >= 20, "scanned only {scanned} files");
        assert!(
            found.is_empty(),
            "typed by hand in static/:\n{}",
            found.join("\n")
        );
    }
}
