//! CAIP-2 Network Identifier support for x402 Protocol v2.
//!
//! Implements the Chain Agnostic Improvement Proposal 2 (CAIP-2) standard
//! for blockchain network identification.
//!
//! Format: `{namespace}:{reference}`
//!
//! Examples:
//! - `eip155:8453` (Base mainnet)
//! - `solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp` (Solana mainnet)
//! - `near:mainnet` (NEAR mainnet)
//! - `stellar:pubnet` (Stellar mainnet)
//!
//! Reference: <https://github.com/ChainAgnostic/CAIPs/blob/main/CAIPs/caip-2.md>

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

/// CAIP-2 namespace identifiers for different blockchain ecosystems.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Namespace {
    /// EIP-155 compatible chains (Ethereum, Base, Polygon, etc.)
    /// Reference is the chain ID as a decimal string.
    Eip155,
    /// Solana ecosystem chains.
    /// Reference is the genesis hash (base58 encoded).
    Solana,
    /// NEAR Protocol.
    /// Reference is the network name ("mainnet" or "testnet").
    Near,
    /// Stellar network.
    /// Reference is the network name ("pubnet" or "testnet").
    Stellar,
    /// Fogo blockchain (SVM-based, custom namespace).
    /// Reference is the network name ("mainnet" or "testnet").
    Fogo,
}

impl Display for Namespace {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Namespace::Eip155 => write!(f, "eip155"),
            Namespace::Solana => write!(f, "solana"),
            Namespace::Near => write!(f, "near"),
            Namespace::Stellar => write!(f, "stellar"),
            Namespace::Fogo => write!(f, "fogo"),
        }
    }
}

impl FromStr for Namespace {
    type Err = Caip2ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "eip155" => Ok(Namespace::Eip155),
            "solana" => Ok(Namespace::Solana),
            "near" => Ok(Namespace::Near),
            "stellar" => Ok(Namespace::Stellar),
            "fogo" => Ok(Namespace::Fogo),
            _ => Err(Caip2ParseError::UnknownNamespace(s.to_string())),
        }
    }
}

/// CAIP-2 compliant network identifier.
///
/// Represents a blockchain network using the format `{namespace}:{reference}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Caip2NetworkId {
    namespace: Namespace,
    reference: String,
}

impl Caip2NetworkId {
    /// Create a new CAIP-2 network ID with validation.
    pub fn new(namespace: Namespace, reference: impl Into<String>) -> Result<Self, Caip2ParseError> {
        let reference = reference.into();

        // Validate reference format based on namespace
        match namespace {
            Namespace::Eip155 => {
                // Must be a valid chain ID (positive integer)
                reference
                    .parse::<u64>()
                    .map_err(|_| Caip2ParseError::InvalidChainId(reference.clone()))?;
            }
            Namespace::Solana => {
                // Genesis hash: base58 encoded, typically 32-44 characters
                if reference.is_empty() || reference.len() > 50 {
                    return Err(Caip2ParseError::InvalidGenesisHash(reference));
                }
                // Basic base58 character validation
                if !reference.chars().all(|c| {
                    c.is_ascii_alphanumeric() && c != '0' && c != 'O' && c != 'I' && c != 'l'
                }) {
                    return Err(Caip2ParseError::InvalidGenesisHash(reference));
                }
            }
            Namespace::Near => {
                if reference != "mainnet" && reference != "testnet" {
                    return Err(Caip2ParseError::InvalidNetworkName {
                        namespace: "near".to_string(),
                        reference,
                    });
                }
            }
            Namespace::Stellar => {
                if reference != "pubnet" && reference != "testnet" {
                    return Err(Caip2ParseError::InvalidNetworkName {
                        namespace: "stellar".to_string(),
                        reference,
                    });
                }
            }
            Namespace::Fogo => {
                if reference != "mainnet" && reference != "testnet" {
                    return Err(Caip2ParseError::InvalidNetworkName {
                        namespace: "fogo".to_string(),
                        reference,
                    });
                }
            }
        }

        Ok(Self {
            namespace,
            reference,
        })
    }

    /// Create a CAIP-2 ID for an EIP-155 chain by chain ID.
    pub fn eip155(chain_id: u64) -> Self {
        Self {
            namespace: Namespace::Eip155,
            reference: chain_id.to_string(),
        }
    }

    /// Create a CAIP-2 ID for Solana by genesis hash.
    pub fn solana(genesis_hash: impl Into<String>) -> Result<Self, Caip2ParseError> {
        Self::new(Namespace::Solana, genesis_hash)
    }

    /// Create a CAIP-2 ID for NEAR mainnet.
    pub fn near_mainnet() -> Self {
        Self {
            namespace: Namespace::Near,
            reference: "mainnet".to_string(),
        }
    }

    /// Create a CAIP-2 ID for NEAR testnet.
    pub fn near_testnet() -> Self {
        Self {
            namespace: Namespace::Near,
            reference: "testnet".to_string(),
        }
    }

    /// Create a CAIP-2 ID for Stellar pubnet (mainnet).
    pub fn stellar_pubnet() -> Self {
        Self {
            namespace: Namespace::Stellar,
            reference: "pubnet".to_string(),
        }
    }

    /// Create a CAIP-2 ID for Stellar testnet.
    pub fn stellar_testnet() -> Self {
        Self {
            namespace: Namespace::Stellar,
            reference: "testnet".to_string(),
        }
    }

    /// Create a CAIP-2 ID for Fogo mainnet.
    pub fn fogo_mainnet() -> Self {
        Self {
            namespace: Namespace::Fogo,
            reference: "mainnet".to_string(),
        }
    }

    /// Create a CAIP-2 ID for Fogo testnet.
    pub fn fogo_testnet() -> Self {
        Self {
            namespace: Namespace::Fogo,
            reference: "testnet".to_string(),
        }
    }

    /// Get the namespace.
    pub fn namespace(&self) -> Namespace {
        self.namespace
    }

    /// Get the reference string.
    pub fn reference(&self) -> &str {
        &self.reference
    }

    /// For EIP-155 chains, get the chain ID as u64.
    pub fn chain_id(&self) -> Option<u64> {
        if self.namespace == Namespace::Eip155 {
            self.reference.parse().ok()
        } else {
            None
        }
    }
}

impl Display for Caip2NetworkId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.namespace, self.reference)
    }
}

impl FromStr for Caip2NetworkId {
    type Err = Caip2ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (namespace_str, reference) = s
            .split_once(':')
            .ok_or_else(|| Caip2ParseError::InvalidFormat(s.to_string()))?;

        let namespace = Namespace::from_str(namespace_str)?;
        Self::new(namespace, reference)
    }
}

impl Serialize for Caip2NetworkId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Caip2NetworkId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

/// Errors that can occur when parsing CAIP-2 identifiers.
#[derive(Debug, Clone, thiserror::Error)]
pub enum Caip2ParseError {
    /// Invalid CAIP-2 format (missing colon separator).
    #[error("invalid CAIP-2 format (expected 'namespace:reference'): {0}")]
    InvalidFormat(String),

    /// Unknown namespace.
    #[error("unknown CAIP-2 namespace: {0}")]
    UnknownNamespace(String),

    /// Invalid EVM chain ID.
    #[error("invalid EVM chain ID (must be positive integer): {0}")]
    InvalidChainId(String),

    /// Invalid Solana genesis hash.
    #[error("invalid Solana genesis hash (must be base58): {0}")]
    InvalidGenesisHash(String),

    /// Invalid network name for namespace.
    #[error("invalid {namespace} network name: {reference}")]
    InvalidNetworkName { namespace: String, reference: String },
}

// ============================================================================
// Well-known CAIP-2 identifiers as constants
// ============================================================================

/// Solana mainnet genesis hash.
pub const SOLANA_MAINNET_GENESIS: &str = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp";

/// Solana devnet genesis hash.
pub const SOLANA_DEVNET_GENESIS: &str = "EtWTRABZaYq6iMfeYKouRu166VU2xqa1";

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_namespace_display() {
        assert_eq!(Namespace::Eip155.to_string(), "eip155");
        assert_eq!(Namespace::Solana.to_string(), "solana");
        assert_eq!(Namespace::Near.to_string(), "near");
        assert_eq!(Namespace::Stellar.to_string(), "stellar");
        assert_eq!(Namespace::Fogo.to_string(), "fogo");
    }

    #[test]
    fn test_namespace_from_str() {
        assert_eq!(Namespace::from_str("eip155").unwrap(), Namespace::Eip155);
        assert_eq!(Namespace::from_str("solana").unwrap(), Namespace::Solana);
        assert_eq!(Namespace::from_str("near").unwrap(), Namespace::Near);
        assert_eq!(Namespace::from_str("stellar").unwrap(), Namespace::Stellar);
        assert_eq!(Namespace::from_str("fogo").unwrap(), Namespace::Fogo);
        assert!(Namespace::from_str("unknown").is_err());
    }

    #[test]
    fn test_caip2_eip155() {
        let id = Caip2NetworkId::eip155(8453);
        assert_eq!(id.to_string(), "eip155:8453");
        assert_eq!(id.namespace(), Namespace::Eip155);
        assert_eq!(id.reference(), "8453");
        assert_eq!(id.chain_id(), Some(8453));
    }

    #[test]
    fn test_caip2_solana() {
        let id = Caip2NetworkId::solana(SOLANA_MAINNET_GENESIS).unwrap();
        assert_eq!(id.to_string(), "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp");
        assert_eq!(id.namespace(), Namespace::Solana);
        assert_eq!(id.chain_id(), None);
    }

    #[test]
    fn test_caip2_near() {
        let mainnet = Caip2NetworkId::near_mainnet();
        assert_eq!(mainnet.to_string(), "near:mainnet");

        let testnet = Caip2NetworkId::near_testnet();
        assert_eq!(testnet.to_string(), "near:testnet");
    }

    #[test]
    fn test_caip2_stellar() {
        let pubnet = Caip2NetworkId::stellar_pubnet();
        assert_eq!(pubnet.to_string(), "stellar:pubnet");

        let testnet = Caip2NetworkId::stellar_testnet();
        assert_eq!(testnet.to_string(), "stellar:testnet");
    }

    #[test]
    fn test_caip2_fogo() {
        let mainnet = Caip2NetworkId::fogo_mainnet();
        assert_eq!(mainnet.to_string(), "fogo:mainnet");

        let testnet = Caip2NetworkId::fogo_testnet();
        assert_eq!(testnet.to_string(), "fogo:testnet");
    }

    #[test]
    fn test_caip2_parse() {
        let id: Caip2NetworkId = "eip155:8453".parse().unwrap();
        assert_eq!(id.namespace(), Namespace::Eip155);
        assert_eq!(id.reference(), "8453");

        let id: Caip2NetworkId = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".parse().unwrap();
        assert_eq!(id.namespace(), Namespace::Solana);

        let id: Caip2NetworkId = "near:mainnet".parse().unwrap();
        assert_eq!(id.namespace(), Namespace::Near);
    }

    #[test]
    fn test_caip2_parse_errors() {
        // Missing colon
        assert!(matches!(
            "eip155".parse::<Caip2NetworkId>(),
            Err(Caip2ParseError::InvalidFormat(_))
        ));

        // Unknown namespace
        assert!(matches!(
            "bitcoin:mainnet".parse::<Caip2NetworkId>(),
            Err(Caip2ParseError::UnknownNamespace(_))
        ));

        // Invalid chain ID
        assert!(matches!(
            "eip155:not-a-number".parse::<Caip2NetworkId>(),
            Err(Caip2ParseError::InvalidChainId(_))
        ));

        // Invalid NEAR network name
        assert!(matches!(
            "near:devnet".parse::<Caip2NetworkId>(),
            Err(Caip2ParseError::InvalidNetworkName { .. })
        ));
    }

    #[test]
    fn test_caip2_serde() {
        let id = Caip2NetworkId::eip155(8453);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"eip155:8453\"");

        let parsed: Caip2NetworkId = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn test_all_evm_chain_ids() {
        // Verify all EVM chain IDs parse correctly
        let chain_ids = [
            (8453, "Base"),
            (84532, "Base Sepolia"),
            (1, "Ethereum"),
            (11155111, "Ethereum Sepolia"),
            (42161, "Arbitrum"),
            (421614, "Arbitrum Sepolia"),
            (10, "Optimism"),
            (11155420, "Optimism Sepolia"),
            (137, "Polygon"),
            (80002, "Polygon Amoy"),
            (43114, "Avalanche"),
            (43113, "Avalanche Fuji"),
            (42220, "Celo"),
            (44787, "Celo Sepolia"),
            (999, "HyperEVM"),
            (333, "HyperEVM Testnet"),
            (1329, "Sei"),
            (1328, "Sei Testnet"),
            (130, "Unichain"),
            (1301, "Unichain Sepolia"),
            (143, "Monad"),
            (50, "XDC"),
            (1440000, "XRPL EVM"),
        ];

        for (chain_id, name) in chain_ids {
            let caip2 = format!("eip155:{}", chain_id);
            let parsed: Caip2NetworkId = caip2.parse().expect(&format!("Failed to parse {}", name));
            assert_eq!(parsed.chain_id(), Some(chain_id), "Chain ID mismatch for {}", name);
        }
    }
}
