//! `config/network_capabilities.json`: how many networks this facilitator
//! serves, per capability, and which. Written from the code, never typed.
//!
//! Every count and list of networks the facilitator publishes (the landing,
//! `/docs`, the agent documents, the agent cards, the README) is held to that
//! file by `scripts/network_copy.py --check`, the first step of CI's test job.
//! This module keeps the file itself honest: it computes the same manifest
//! from the functions the payment, escrow and ERC-8004 paths call, and
//! [`tests::the_committed_manifest_is_the_one_the_code_computes`] fails when
//! the committed file and the code disagree. After a change of networks one
//! command rewrites the file and every published count and list:
//!
//! ```text
//! python scripts/network_copy.py --generate
//! ```
//!
//! # Production's profile
//!
//! The rows are `Network::variants()` under the features CI and the image
//! build with (`solana,near,stellar,algorand,sui,xrpl,hedera`), which is why
//! this module compiles only under all four feature-gated families. Which of
//! them take payments is `exactServed` in `config/supported_tokens.json`, the
//! registry `/networks.json` and the receipt rail already read; the check ties
//! that flag to the providers `terraform/environments/production` configures.
//! A capability behind a switch is counted as production sets it:
//! `ENABLE_UPTO`, `ENABLE_PAYMENT_OPERATOR` and `ENABLE_NEAR_ESCROW` on,
//! `ENABLE_ZAMA` off. The check reads those in the Terraform as well.
//!
//! Nothing here runs in the server: this is a test module, so the manifest can
//! never change what the facilitator serves.

use serde_json::{json, Value};

use crate::erc8004;
use crate::from_env::rpc_env_name_from_network;
use crate::network::{Network, NetworkFamily};
use crate::networks_json::{family_slug, v1_id, CATALOG};
use crate::payment_operator::addresses::{OperatorAddresses, ESCROW_NETWORKS};

/// Where the manifest lives, relative to the crate root.
pub const MANIFEST_PATH: &str = "config/network_capabilities.json";

/// Set (to anything) to rewrite [`MANIFEST_PATH`] instead of comparing it.
pub const WRITE_ENV: &str = "X402_WRITE_NETWORK_MANIFEST";

/// The features this manifest is computed under: CI's `CARGO_FEATURES`.
const FEATURES: [&str; 7] = [
    "solana", "near", "stellar", "algorand", "sui", "xrpl", "hedera",
];

/// The switches production sets, as the capabilities below assume them.
/// `scripts/network_copy.py` fails when the Terraform says otherwise.
const PROFILE_ENV: [(&str, &str); 4] = [
    ("ENABLE_UPTO", "true"),
    ("ENABLE_PAYMENT_OPERATOR", "true"),
    ("ENABLE_NEAR_ESCROW", "true"),
    ("ENABLE_ZAMA", "false"),
];

/// One capability: what it means, which function decides it, and the networks
/// it covers, in the order that function lists them.
struct Capability {
    key: &'static str,
    description: &'static str,
    decided_by: &'static str,
    networks: Vec<Network>,
}

/// The served networks: one row per network `/supported` can name.
fn served() -> Vec<Network> {
    Network::variants().to_vec()
}

/// `exactServed` for `network`, from the registry. A served network without an
/// entry is a red `networks_json` test already; here it is a panic, not a guess.
fn exact_served(network: Network) -> bool {
    let catalog = CATALOG
        .as_ref()
        .expect("config/supported_tokens.json parses");
    catalog
        .network(network)
        .unwrap_or_else(|| panic!("config/supported_tokens.json has no `{}`", v1_id(network)))
        .exact_served
}

/// The ERC-8004 registries every canonical EVM mainnet shares (the erc-8004
/// reference deployment), Ethereum's being the reference.
fn on_the_canonical_registries(network: Network) -> bool {
    let reference = erc8004::ETHEREUM_MAINNET_CONTRACTS;
    network.is_mainnet()
        && erc8004::get_contracts(&network).is_some_and(|c| {
            c.identity_registry == reference.identity_registry
                && c.reputation_registry == reference.reputation_registry
                && c.validation_registry == reference.validation_registry
        })
}

fn capabilities() -> Vec<Capability> {
    let served = served();
    let payments: Vec<Network> = served
        .iter()
        .copied()
        .filter(|n| exact_served(*n))
        .collect();
    // `FacilitatorLocal::supported`: `upto` on every network carrying `exact`
    // where the Permit2 proxy has code.
    let upto: Vec<Network> = payments
        .iter()
        .copied()
        .filter(|n| crate::upto::types::is_proxy_deployed_on(*n))
        .collect();
    // `escrow_supported_kinds_from`: one `commerce` entry per network of
    // ESCROW_NETWORKS with an address table. The `escrow` entries (one per
    // declared operator) are not a capability of their own here: every such
    // network declares one, and which canonical ones pass the on-chain
    // self-check is decided at runtime, not in the code.
    let escrow_x402r: Vec<Network> = ESCROW_NETWORKS
        .iter()
        .copied()
        .filter(|n| OperatorAddresses::for_network(*n).is_some())
        .collect();
    let escrow_near: Vec<Network> = served
        .iter()
        .copied()
        .filter(|n| matches!(NetworkFamily::from(*n), NetworkFamily::Near))
        .collect();
    let erc8004_all = erc8004::supported_networks();
    let erc8004_canonical: Vec<Network> = erc8004_all
        .iter()
        .copied()
        .filter(|n| on_the_canonical_registries(*n))
        .collect();
    let erc8004_relay: Vec<Network> = served
        .iter()
        .copied()
        .filter(|n| erc8004::relay::feedback_delegate(n).is_some())
        .collect();

    vec![
        Capability {
            key: "served",
            description: "Named by /supported under any scheme in production.",
            decided_by: "Network::variants() under the production features",
            networks: served,
        },
        Capability {
            key: "payments",
            description: "Takes x402 payments: production publishes its `exact` scheme.",
            decided_by: "Network::variants() and exactServed in config/supported_tokens.json",
            networks: payments,
        },
        Capability {
            key: "upto",
            description: "Carries `exact` and the Permit2 upto proxy has code there.",
            decided_by: "crate::upto::types::is_proxy_deployed_on (UPTO_DEPLOYED_NETWORKS)",
            networks: upto,
        },
        Capability {
            key: "escrow_x402r",
            description: "x402r escrow through a PaymentOperator: /supported publishes `commerce` there.",
            decided_by: "payment_operator::addresses::ESCROW_NETWORKS with OperatorAddresses::for_network",
            networks: escrow_x402r,
        },
        Capability {
            key: "escrow_near",
            description: "The `escrow` scheme on NEAR, into the Arras contracts.",
            decided_by: "the NEAR family: chain::near::escrow serves both NEAR networks, each once its NEAR_ESCROW_CONTRACTS_MAINNET / _TESTNET is set (scripts/network_copy.py holds both to the production Terraform)",
            networks: escrow_near,
        },
        Capability {
            key: "erc8004",
            description: "ERC-8004 identity and reputation (register, feedback, reads).",
            decided_by: "erc8004::supported_networks",
            networks: erc8004_all,
        },
        Capability {
            key: "erc8004_canonical",
            description: "ERC-8004 mainnets on the erc-8004 reference deployment: the same identity, reputation and validation addresses.",
            decided_by: "erc8004::get_contracts equal to ETHEREUM_MAINNET_CONTRACTS",
            networks: erc8004_canonical,
        },
        Capability {
            key: "erc8004_relay",
            description: "Rater-signed ERC-8004 feedback relayed through EIP-7702 (a FeedbackDelegate is deployed).",
            decided_by: "erc8004::relay::feedback_delegate",
            networks: erc8004_relay,
        },
    ]
}

fn display_name(network: Network) -> String {
    CATALOG
        .as_ref()
        .ok()
        .and_then(|catalog| catalog.network(network))
        .and_then(|meta| meta.display_name.clone())
        .unwrap_or_else(|| v1_id(network))
}

fn ids(networks: &[Network]) -> Vec<String> {
    networks.iter().map(|n| v1_id(*n)).collect()
}

/// Families in the order their first network appears.
fn families(networks: &[Network]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for network in networks {
        let slug = family_slug(NetworkFamily::from(*network));
        if !out.contains(&slug) {
            out.push(slug);
        }
    }
    out
}

/// The manifest, as the code computes it.
pub fn manifest() -> Value {
    let capabilities = capabilities();
    let mut caps = serde_json::Map::new();
    for capability in &capabilities {
        let mainnets: Vec<Network> = capability
            .networks
            .iter()
            .copied()
            .filter(|n| n.is_mainnet())
            .collect();
        let testnets: Vec<Network> = capability
            .networks
            .iter()
            .copied()
            .filter(|n| n.is_testnet())
            .collect();
        caps.insert(
            capability.key.to_string(),
            json!({
                "description": capability.description,
                "decided_by": capability.decided_by,
                "count": capability.networks.len(),
                "mainnets": mainnets.len(),
                "testnets": testnets.len(),
                "families": families(&capability.networks),
                "networks": ids(&capability.networks),
                "names": capability.networks.iter().map(|n| display_name(*n)).collect::<Vec<_>>(),
                "mainnet_networks": ids(&mainnets),
                "testnet_networks": ids(&testnets),
            }),
        );
    }

    let payments = capabilities
        .iter()
        .find(|c| c.key == "payments")
        .expect("payments is a capability");
    let payment_families = families(&payments.networks);

    let rows: Vec<Value> = served()
        .into_iter()
        .map(|network| {
            let has: Vec<&str> = capabilities
                .iter()
                .filter(|c| c.networks.contains(&network))
                .map(|c| c.key)
                .collect();
            json!({
                "id": v1_id(network),
                "caip2": network.to_caip2(),
                "family": family_slug(NetworkFamily::from(network)),
                "testnet": network.is_testnet(),
                "displayName": display_name(network),
                "rpcEnv": rpc_env_name_from_network(network),
                "capabilities": has,
            })
        })
        .collect();

    json!({
        "_generated": "AUTOGENERATED - DO NOT EDIT. Source: src/network_manifest.rs, from the functions the payment, escrow and ERC-8004 paths call. Regenerate: python scripts/network_copy.py --generate",
        "profile": {
            "description": "Production: the features CI and the image build with, the switches the production Terraform sets, and exactServed in config/supported_tokens.json for which networks take payments.",
            "features": FEATURES,
            "env": PROFILE_ENV.iter().map(|(k, v)| (k.to_string(), Value::from(*v))).collect::<serde_json::Map<_, _>>(),
        },
        "families": {
            "description": "The chain families of the payment networks.",
            "count": payment_families.len(),
            "slugs": payment_families,
        },
        "capabilities": Value::Object(caps),
        "networks": rows,
    })
}

/// The manifest as written to disk: pretty JSON, one trailing newline.
pub fn render(manifest: &Value) -> String {
    let mut text = serde_json::to_string_pretty(manifest).expect("a JSON value renders");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;

    fn committed() -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(MANIFEST_PATH);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "{MANIFEST_PATH} is missing ({e}): run python scripts/network_copy.py --generate"
            )
        });
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{MANIFEST_PATH} is not JSON: {e}"))
    }

    /// The committed manifest is what the code computes today. With
    /// `X402_WRITE_NETWORK_MANIFEST` set, the test writes it instead, which is
    /// how `scripts/network_copy.py --generate` regenerates it.
    #[test]
    fn the_committed_manifest_is_the_one_the_code_computes() {
        let computed = manifest();
        if asked_to_write() {
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(MANIFEST_PATH);
            std::fs::write(&path, render(&computed)).expect("the manifest is writable");
            return;
        }
        let differ = differences(&committed(), &computed);
        assert!(
            differ.is_empty(),
            "{MANIFEST_PATH} is not what the code computes (differs in: {}). A network or a \
             capability changed: run `python scripts/network_copy.py --generate` and commit \
             the manifest and the texts it rewrites",
            differ.join(", ")
        );
    }

    /// Only `--generate` sets the variable, and it runs the drift test alone.
    fn asked_to_write() -> bool {
        std::env::var_os(WRITE_ENV).is_some()
    }

    /// Everywhere else the drift test compares: a run of the whole suite never
    /// rewrites the manifest it is meant to check.
    #[test]
    fn the_suite_compares_the_manifest_and_never_writes_it() {
        if std::env::var_os(WRITE_ENV).is_none() {
            assert!(!asked_to_write());
        }
    }

    /// Where `committed` and `computed` differ, named down to the capability;
    /// empty when they are the same document.
    fn differences(committed: &Value, computed: &Value) -> Vec<String> {
        let mut differ: Vec<String> = Vec::new();
        if committed == computed {
            return differ;
        }
        for key in [
            "_generated",
            "profile",
            "families",
            "capabilities",
            "networks",
        ] {
            if committed[key] != computed[key] {
                differ.push(key.to_string());
            }
        }
        if let Some(capabilities) = computed["capabilities"].as_object() {
            for (key, value) in capabilities {
                if committed["capabilities"][key.as_str()] != *value {
                    differ.push(format!("capabilities.{key}"));
                }
            }
        }
        if differ.is_empty() {
            differ.push("a key outside the known sections".to_string());
        }
        differ
    }

    /// The drift test cannot pass a manifest that lost a network: removing one
    /// from a capability is named down to that capability.
    #[test]
    fn a_tampered_manifest_is_named_by_what_differs() {
        let computed = manifest();
        assert!(differences(&computed, &computed).is_empty());
        let mut tampered = computed.clone();
        tampered["capabilities"]["payments"]["networks"]
            .as_array_mut()
            .expect("payments lists its networks")
            .pop();
        let differ = differences(&tampered, &computed);
        assert!(
            differ.contains(&"capabilities.payments".to_string()),
            "{differ:?}"
        );
        let mut extra = computed.clone();
        extra["stray"] = json!(1);
        assert!(!differences(&extra, &computed).is_empty());
    }

    fn capability(manifest: &Value, key: &str) -> BTreeSet<String> {
        manifest["capabilities"][key]["networks"]
            .as_array()
            .unwrap_or_else(|| panic!("capability `{key}`"))
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    }

    /// Every capability names served networks only, and the counts are the
    /// lists' own lengths, so a reader of the counts reads the lists.
    #[test]
    fn every_capability_is_a_subset_of_the_served_networks_and_counts_itself() {
        let manifest = manifest();
        let served = capability(&manifest, "served");
        for (key, cap) in manifest["capabilities"].as_object().unwrap() {
            let networks = capability(&manifest, key);
            assert!(
                networks.is_subset(&served),
                "`{key}` names a network /supported never does"
            );
            let count = cap["count"].as_u64().unwrap() as usize;
            let mainnets = cap["mainnets"].as_u64().unwrap() as usize;
            let testnets = cap["testnets"].as_u64().unwrap() as usize;
            assert_eq!(count, networks.len(), "`{key}` count");
            assert_eq!(mainnets + testnets, count, "`{key}` mainnets + testnets");
            assert_eq!(cap["mainnet_networks"].as_array().unwrap().len(), mainnets);
            assert_eq!(cap["names"].as_array().unwrap().len(), count);
        }
    }

    /// The registry's `exactServed: false` is the only thing that keeps a
    /// served network out of payments, and such a network is served for some
    /// other scheme (Ethereum Sepolia today: escrow and commerce, with no
    /// `RPC_URL_ETHEREUM_SEPOLIA` in production). A row for a network nothing
    /// is served on would be a row `/supported` never shows.
    #[test]
    fn a_served_network_without_payments_is_served_for_another_scheme() {
        let manifest = manifest();
        let served = capability(&manifest, "served");
        let payments = capability(&manifest, "payments");
        let schemes: BTreeSet<String> = ["upto", "escrow_x402r", "escrow_near"]
            .iter()
            .flat_map(|key| capability(&manifest, key))
            .collect();
        for id in served.difference(&payments) {
            let network = crate::network::resolve_network(id).expect("a served id resolves");
            assert!(
                !exact_served(network),
                "{id} is out of payments only by exactServed"
            );
            assert!(schemes.contains(id), "{id} is served for no scheme at all");
        }
    }

    /// The manifest renders the same bytes twice: regenerating it with nothing
    /// changed leaves `git diff` empty.
    #[test]
    fn the_manifest_renders_deterministically() {
        let first = render(&manifest());
        let second = render(&manifest());
        assert_eq!(first, second);
        assert!(first.ends_with("}\n"));
        assert!(!first.contains('\r'));
    }
}
