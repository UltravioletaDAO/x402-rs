//! Network definitions and known token deployments.
//!
//! This module defines supported networks and their chain IDs,
//! and provides statically known USDC deployments per network.

use crate::types::{MixedAddress, TokenAsset, TokenDeployment, TokenDeploymentEip712, TokenType};
use alloy::primitives::address;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use solana_sdk::pubkey::Pubkey;
use std::borrow::Borrow;
use std::fmt::{Display, Formatter};
use std::ops::Deref;
use std::str::FromStr;

/// Supported Ethereum-compatible networks.
///
/// Used to differentiate between testnet and mainnet environments for the x402 protocol.
#[derive(Debug, Hash, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Network {
    /// Base Sepolia testnet (chain ID 84532).
    #[serde(rename = "base-sepolia")]
    BaseSepolia,
    /// Base mainnet (chain ID 8453).
    #[serde(rename = "base")]
    Base,
    /// XDC mainnet (chain ID 50).
    #[serde(rename = "xdc")]
    XdcMainnet,
    /// Avalanche Fuji testnet (chain ID 43113)
    #[serde(rename = "avalanche-fuji")]
    AvalancheFuji,
    /// Avalanche Mainnet (chain ID 43114)
    #[serde(rename = "avalanche")]
    Avalanche,
    /// XRPL EVM mainnet (chain ID 1440000) - NEW from upstream v0.10.0
    #[serde(rename = "xrpl-evm")]
    XrplEvm,
    /// Solana Mainnet - Live production environment for deployed applications
    #[serde(rename = "solana")]
    Solana,
    /// Solana Devnet - Testing with public accessibility for developers experimenting with their applications
    #[serde(rename = "solana-devnet")]
    SolanaDevnet,
    /// Polygon Amoy testnet (chain ID 80002).
    #[serde(rename = "polygon-amoy")]
    PolygonAmoy,
    /// Polygon mainnet (chain ID 137).
    #[serde(rename = "polygon")]
    Polygon,
    /// Optimism mainnet (chain ID 10).
    #[serde(rename = "optimism")]
    Optimism,
    /// Optimism Sepolia testnet (chain ID 11155420).
    #[serde(rename = "optimism-sepolia")]
    OptimismSepolia,
    /// Celo mainnet (chain ID 42220).
    #[serde(rename = "celo")]
    Celo,
    /// Celo Sepolia testnet (chain ID 44787).
    #[serde(rename = "celo-sepolia")]
    CeloSepolia,
    /// HyperEVM mainnet (chain ID 999).
    #[serde(rename = "hyperevm")]
    HyperEvm,
    /// HyperEVM testnet (chain ID 333).
    #[serde(rename = "hyperevm-testnet")]
    HyperEvmTestnet,
    /// Sei mainnet (chain ID 1329).
    #[serde(rename = "sei")]
    Sei,
    /// Sei testnet (chain ID 1328).
    #[serde(rename = "sei-testnet")]
    SeiTestnet,
    /// Ethereum mainnet (chain ID 1).
    #[serde(rename = "ethereum")]
    Ethereum,
    /// Ethereum Sepolia testnet (chain ID 11155111).
    #[serde(rename = "ethereum-sepolia")]
    EthereumSepolia,
    /// Arbitrum One mainnet (chain ID 42161).
    #[serde(rename = "arbitrum")]
    Arbitrum,
    /// Arbitrum Sepolia testnet (chain ID 421614).
    #[serde(rename = "arbitrum-sepolia")]
    ArbitrumSepolia,
    /// Unichain mainnet (chain ID 130).
    #[serde(rename = "unichain")]
    Unichain,
    /// Unichain Sepolia testnet (chain ID 1301).
    #[serde(rename = "unichain-sepolia")]
    UnichainSepolia,
    /// Monad mainnet (chain ID 143).
    #[serde(rename = "monad")]
    Monad,
    /// NEAR Protocol mainnet.
    #[serde(rename = "near")]
    Near,
    /// NEAR Protocol testnet.
    #[serde(rename = "near-testnet")]
    NearTestnet,
    /// Stellar mainnet.
    #[serde(rename = "stellar")]
    Stellar,
    /// Stellar testnet.
    #[serde(rename = "stellar-testnet")]
    StellarTestnet,
    /// Fogo mainnet (Solana Virtual Machine).
    #[serde(rename = "fogo")]
    Fogo,
    /// Fogo testnet (Solana Virtual Machine).
    #[serde(rename = "fogo-testnet")]
    FogoTestnet,
}

impl Display for Network {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Network::BaseSepolia => write!(f, "base-sepolia"),
            Network::Base => write!(f, "base"),
            Network::XdcMainnet => write!(f, "xdc"),
            Network::AvalancheFuji => write!(f, "avalanche-fuji"),
            Network::Avalanche => write!(f, "avalanche"),
            Network::XrplEvm => write!(f, "xrpl-evm"),
            Network::Solana => write!(f, "solana"),
            Network::SolanaDevnet => write!(f, "solana-devnet"),
            Network::PolygonAmoy => write!(f, "polygon-amoy"),
            Network::Polygon => write!(f, "polygon"),
            Network::Optimism => write!(f, "optimism"),
            Network::OptimismSepolia => write!(f, "optimism-sepolia"),
            Network::Celo => write!(f, "celo"),
            Network::CeloSepolia => write!(f, "celo-sepolia"),
            Network::HyperEvm => write!(f, "hyperevm"),
            Network::HyperEvmTestnet => write!(f, "hyperevm-testnet"),
            Network::Sei => write!(f, "sei"),
            Network::SeiTestnet => write!(f, "sei-testnet"),
            Network::Ethereum => write!(f, "ethereum"),
            Network::EthereumSepolia => write!(f, "ethereum-sepolia"),
            Network::Arbitrum => write!(f, "arbitrum"),
            Network::ArbitrumSepolia => write!(f, "arbitrum-sepolia"),
            Network::Unichain => write!(f, "unichain"),
            Network::UnichainSepolia => write!(f, "unichain-sepolia"),
            Network::Monad => write!(f, "monad"),
            Network::Near => write!(f, "near"),
            Network::NearTestnet => write!(f, "near-testnet"),
            Network::Stellar => write!(f, "stellar"),
            Network::StellarTestnet => write!(f, "stellar-testnet"),
            Network::Fogo => write!(f, "fogo"),
            Network::FogoTestnet => write!(f, "fogo-testnet"),
        }
    }
}

/// Error type for parsing Network from string.
#[derive(Debug, Clone, thiserror::Error)]
#[error("Unknown network: {0}")]
pub struct NetworkParseError(pub String);

impl FromStr for Network {
    type Err = NetworkParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "base-sepolia" => Ok(Network::BaseSepolia),
            "base" => Ok(Network::Base),
            "xdc" => Ok(Network::XdcMainnet),
            "avalanche-fuji" => Ok(Network::AvalancheFuji),
            "avalanche" => Ok(Network::Avalanche),
            "xrpl-evm" => Ok(Network::XrplEvm),
            "solana" => Ok(Network::Solana),
            "solana-devnet" => Ok(Network::SolanaDevnet),
            "polygon-amoy" => Ok(Network::PolygonAmoy),
            "polygon" => Ok(Network::Polygon),
            "optimism" => Ok(Network::Optimism),
            "optimism-sepolia" => Ok(Network::OptimismSepolia),
            "celo" => Ok(Network::Celo),
            "celo-sepolia" => Ok(Network::CeloSepolia),
            "hyperevm" => Ok(Network::HyperEvm),
            "hyperevm-testnet" => Ok(Network::HyperEvmTestnet),
            "sei" => Ok(Network::Sei),
            "sei-testnet" => Ok(Network::SeiTestnet),
            "ethereum" => Ok(Network::Ethereum),
            "ethereum-sepolia" => Ok(Network::EthereumSepolia),
            "arbitrum" => Ok(Network::Arbitrum),
            "arbitrum-sepolia" => Ok(Network::ArbitrumSepolia),
            "unichain" => Ok(Network::Unichain),
            "unichain-sepolia" => Ok(Network::UnichainSepolia),
            "monad" => Ok(Network::Monad),
            "near" => Ok(Network::Near),
            "near-testnet" => Ok(Network::NearTestnet),
            "stellar" => Ok(Network::Stellar),
            "stellar-testnet" => Ok(Network::StellarTestnet),
            "fogo" => Ok(Network::Fogo),
            "fogo-testnet" => Ok(Network::FogoTestnet),
            _ => Err(NetworkParseError(s.to_string())),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum NetworkFamily {
    Evm,
    Solana,
    Near,
    Stellar,
}

impl From<Network> for NetworkFamily {
    fn from(value: Network) -> Self {
        match value {
            Network::BaseSepolia => NetworkFamily::Evm,
            Network::Base => NetworkFamily::Evm,
            Network::XdcMainnet => NetworkFamily::Evm,
            Network::AvalancheFuji => NetworkFamily::Evm,
            Network::Avalanche => NetworkFamily::Evm,
            Network::XrplEvm => NetworkFamily::Evm,
            Network::Solana => NetworkFamily::Solana,
            Network::SolanaDevnet => NetworkFamily::Solana,
            Network::PolygonAmoy => NetworkFamily::Evm,
            Network::Polygon => NetworkFamily::Evm,
            Network::Optimism => NetworkFamily::Evm,
            Network::OptimismSepolia => NetworkFamily::Evm,
            Network::Celo => NetworkFamily::Evm,
            Network::CeloSepolia => NetworkFamily::Evm,
            Network::HyperEvm => NetworkFamily::Evm,
            Network::HyperEvmTestnet => NetworkFamily::Evm,
            Network::Sei => NetworkFamily::Evm,
            Network::SeiTestnet => NetworkFamily::Evm,
            Network::Ethereum => NetworkFamily::Evm,
            Network::EthereumSepolia => NetworkFamily::Evm,
            Network::Arbitrum => NetworkFamily::Evm,
            Network::ArbitrumSepolia => NetworkFamily::Evm,
            Network::Unichain => NetworkFamily::Evm,
            Network::UnichainSepolia => NetworkFamily::Evm,
            Network::Monad => NetworkFamily::Evm,
            Network::Near => NetworkFamily::Near,
            Network::NearTestnet => NetworkFamily::Near,
            Network::Stellar => NetworkFamily::Stellar,
            Network::StellarTestnet => NetworkFamily::Stellar,
            Network::Fogo => NetworkFamily::Solana,
            Network::FogoTestnet => NetworkFamily::Solana,
        }
    }
}

impl Network {
    /// Return all known [`Network`] variants.
    pub fn variants() -> &'static [Network] {
        &[
            Network::BaseSepolia,
            Network::Base,
            Network::XdcMainnet,
            Network::AvalancheFuji,
            Network::Avalanche,
            Network::XrplEvm,
            Network::Solana,
            Network::SolanaDevnet,
            Network::PolygonAmoy,
            Network::Polygon,
            Network::Optimism,
            Network::OptimismSepolia,
            Network::Celo,
            Network::CeloSepolia,
            Network::HyperEvm,
            Network::HyperEvmTestnet,
            Network::Sei,
            Network::SeiTestnet,
            Network::Ethereum,
            Network::EthereumSepolia,
            Network::Arbitrum,
            Network::ArbitrumSepolia,
            Network::Unichain,
            Network::UnichainSepolia,            
            Network::Monad,
            Network::Near,
            Network::NearTestnet,
            Network::Stellar,
            Network::StellarTestnet,
            Network::Fogo,
            Network::FogoTestnet,
        ]
    }

    /// Returns true if this network is a testnet environment.
    pub fn is_testnet(&self) -> bool {
        matches!(
            self,
            Network::BaseSepolia
                | Network::AvalancheFuji
                | Network::SolanaDevnet
                | Network::PolygonAmoy
                | Network::OptimismSepolia
                | Network::CeloSepolia
                | Network::HyperEvmTestnet
                | Network::SeiTestnet
                | Network::EthereumSepolia
                | Network::ArbitrumSepolia
                | Network::UnichainSepolia               
                | Network::NearTestnet
                | Network::StellarTestnet
                | Network::FogoTestnet
        )
    }

    /// Returns true if this network is a mainnet environment.
    pub fn is_mainnet(&self) -> bool {
        !self.is_testnet()
    }

    /// Convert this network to a CAIP-2 identifier string.
    ///
    /// Format: `{namespace}:{reference}`
    /// - EVM chains: `eip155:{chain_id}`
    /// - Solana: `solana:{genesis_hash}`
    /// - NEAR: `near:{network_name}`
    /// - Stellar: `stellar:{network_name}`
    /// - Fogo: `fogo:{network_name}`
    pub fn to_caip2(&self) -> String {
        match self {
            // EVM chains - eip155:{chain_id}
            Network::Ethereum => "eip155:1".to_string(),
            Network::EthereumSepolia => "eip155:11155111".to_string(),
            Network::Base => "eip155:8453".to_string(),
            Network::BaseSepolia => "eip155:84532".to_string(),
            Network::Arbitrum => "eip155:42161".to_string(),
            Network::ArbitrumSepolia => "eip155:421614".to_string(),
            Network::Optimism => "eip155:10".to_string(),
            Network::OptimismSepolia => "eip155:11155420".to_string(),
            Network::Polygon => "eip155:137".to_string(),
            Network::PolygonAmoy => "eip155:80002".to_string(),
            Network::Avalanche => "eip155:43114".to_string(),
            Network::AvalancheFuji => "eip155:43113".to_string(),
            Network::Celo => "eip155:42220".to_string(),
            Network::CeloSepolia => "eip155:44787".to_string(),
            Network::HyperEvm => "eip155:999".to_string(),
            Network::HyperEvmTestnet => "eip155:333".to_string(),
            Network::Sei => "eip155:1329".to_string(),
            Network::SeiTestnet => "eip155:1328".to_string(),
            Network::Unichain => "eip155:130".to_string(),
            Network::UnichainSepolia => "eip155:1301".to_string(),
            Network::Monad => "eip155:143".to_string(),
            Network::XdcMainnet => "eip155:50".to_string(),
            Network::XrplEvm => "eip155:1440000".to_string(),
            // Solana - solana:{genesis_hash}
            Network::Solana => "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
            Network::SolanaDevnet => "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1".to_string(),
            // NEAR - near:{network_name}
            Network::Near => "near:mainnet".to_string(),
            Network::NearTestnet => "near:testnet".to_string(),
            // Stellar - stellar:{network_name}
            Network::Stellar => "stellar:pubnet".to_string(),
            Network::StellarTestnet => "stellar:testnet".to_string(),
            // Fogo - fogo:{network_name}
            Network::Fogo => "fogo:mainnet".to_string(),
            Network::FogoTestnet => "fogo:testnet".to_string(),
        }
    }

    /// Parse a CAIP-2 identifier string into a Network.
    ///
    /// Returns `None` if the CAIP-2 identifier is not recognized.
    pub fn from_caip2(caip2: &str) -> Option<Self> {
        match caip2 {
            // EVM chains
            "eip155:1" => Some(Network::Ethereum),
            "eip155:11155111" => Some(Network::EthereumSepolia),
            "eip155:8453" => Some(Network::Base),
            "eip155:84532" => Some(Network::BaseSepolia),
            "eip155:42161" => Some(Network::Arbitrum),
            "eip155:421614" => Some(Network::ArbitrumSepolia),
            "eip155:10" => Some(Network::Optimism),
            "eip155:11155420" => Some(Network::OptimismSepolia),
            "eip155:137" => Some(Network::Polygon),
            "eip155:80002" => Some(Network::PolygonAmoy),
            "eip155:43114" => Some(Network::Avalanche),
            "eip155:43113" => Some(Network::AvalancheFuji),
            "eip155:42220" => Some(Network::Celo),
            "eip155:44787" => Some(Network::CeloSepolia),
            "eip155:999" => Some(Network::HyperEvm),
            "eip155:333" => Some(Network::HyperEvmTestnet),
            "eip155:1329" => Some(Network::Sei),
            "eip155:1328" => Some(Network::SeiTestnet),
            "eip155:130" => Some(Network::Unichain),
            "eip155:1301" => Some(Network::UnichainSepolia),
            "eip155:143" => Some(Network::Monad),
            "eip155:50" => Some(Network::XdcMainnet),
            "eip155:1440000" => Some(Network::XrplEvm),
            // Solana
            "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp" => Some(Network::Solana),
            "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1" => Some(Network::SolanaDevnet),
            // NEAR
            "near:mainnet" => Some(Network::Near),
            "near:testnet" => Some(Network::NearTestnet),
            // Stellar
            "stellar:pubnet" => Some(Network::Stellar),
            "stellar:testnet" => Some(Network::StellarTestnet),
            // Fogo
            "fogo:mainnet" => Some(Network::Fogo),
            "fogo:testnet" => Some(Network::FogoTestnet),
            _ => None,
        }
    }
}

/// Lazily initialized known USDC deployment on Base Sepolia as [`USDCDeployment`].
static USDC_BASE_SEPOLIA: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x036CbD53842c5426634e7929541eC2318f3dCF7e").into(),
            network: Network::BaseSepolia,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USDC".into(),
            version: "2".into(),
        }),
    })
});

/// Lazily initialized known USDC deployment on Base mainnet as [`USDCDeployment`].
static USDC_BASE: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913").into(),
            network: Network::Base,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USD Coin".into(),
            version: "2".into(),
        }),
    })
});

/// Lazily initialized known USDC deployment on XDC mainnet as [`USDCDeployment`].
static USDC_XDC: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x2A8E898b6242355c290E1f4Fc966b8788729A4D4").into(),
            network: Network::XdcMainnet,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "Bridged USDC(XDC)".into(),
            version: "2".into(),
        }),
    })
});

/// Lazily initialized known USDC deployment on Avalanche Fuji testnet as [`USDCDeployment`].
static USDC_AVALANCHE_FUJI: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x5425890298aed601595a70AB815c96711a31Bc65").into(),
            network: Network::AvalancheFuji,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USD Coin".into(),
            version: "2".into(),
        }),
    })
});

/// Lazily initialized known USDC deployment on Avalanche mainnet as [`USDCDeployment`].
static USDC_AVALANCHE: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0xB97EF9Ef8734C71904D8002F8b6Bc66Dd9c48a6E").into(),
            network: Network::Avalanche,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USD Coin".into(),
            version: "2".into(),
        }),
    })
});

/// Lazily initialized known USDC deployment on XRPL EVM mainnet as [`USDCDeployment`].
static USDC_XRPL_EVM: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0xDaF4556169c4F3f2231d8ab7BC8772Ddb7D4c84C").into(),
            network: Network::XrplEvm,
        },
        decimals: 6,
        // EIP-712 domain fields (name/version) are resolved dynamically if not provided.
        eip712: None,
    })
});

/// Lazily initialized known USDC deployment on Solana mainnet as [`USDCDeployment`].
static USDC_SOLANA: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: MixedAddress::Solana(
                Pubkey::from_str("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap(),
            ),
            network: Network::Solana,
        },
        decimals: 6,
        eip712: None,
    })
});

/// Lazily initialized known USDC deployment on Solana devnet as [`USDCDeployment`].
static USDC_SOLANA_DEVNET: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: MixedAddress::Solana(
                Pubkey::from_str("4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU").unwrap(),
            ),
            network: Network::SolanaDevnet,
        },
        decimals: 6,
        eip712: None,
    })
});

/// Lazily initialized known USDC deployment on Polygon Amoy testnet as [`USDCDeployment`].
static USDC_POLYGON_AMOY: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x41E94Eb019C0762f9Bfcf9Fb1E58725BfB0e7582").into(),
            network: Network::PolygonAmoy,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USDC".into(),
            version: "2".into(),
        }),
    })
});

/// Lazily initialized known USDC deployment on Polygon mainnet as [`USDCDeployment`].
static USDC_POLYGON: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359").into(),
            network: Network::Polygon,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USDC".into(),
            version: "2".into(),
        }),
    })
});

/// Lazily initialized known USDC deployment on Optimism mainnet as [`USDCDeployment`].
static USDC_OPTIMISM: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x0b2C639c533813f4Aa9D7837CAf62653d097Ff85").into(),
            network: Network::Optimism,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USD Coin".into(),
            version: "2".into(),
        }),
    })
});

/// Lazily initialized known USDC deployment on Optimism Sepolia testnet as [`USDCDeployment`].
static USDC_OPTIMISM_SEPOLIA: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x5fd84259d66Cd46123540766Be93DFE6D43130D7").into(),
            network: Network::OptimismSepolia,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USDC".into(),
            version: "2".into(),
        }),
    })
});

/// Lazily initialized known USDC deployment on Celo mainnet as [`USDCDeployment`].
static USDC_CELO: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0xcebA9300f2b948710d2653dD7B07f33A8B32118C").into(),
            network: Network::Celo,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USD Coin".into(),
            version: "2".into(),
        }),
    })
});

/// Lazily initialized known USDC deployment on Celo Sepolia testnet as [`USDCDeployment`].
static USDC_CELO_SEPOLIA: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x01C5C0122039549AD1493B8220cABEdD739BC44E").into(),
            network: Network::CeloSepolia,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USD Coin".into(),
            version: "2".into(),
        }),
    })
});

/// Lazily initialized known USDC deployment on HyperEVM mainnet as [`USDCDeployment`].
static USDC_HYPEREVM: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0xb88339cb7199b77e23db6e890353e22632ba630f").into(),
            network: Network::HyperEvm,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USDC".into(),
            version: "2".into(),
        }),
    })
});

/// Lazily initialized known USDC deployment on HyperEVM testnet as [`USDCDeployment`].
static USDC_HYPEREVM_TESTNET: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x2B3370eE501B4a559b57D449569354196457D8Ab").into(),
            network: Network::HyperEvmTestnet,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USD Coin".into(),
            version: "2".into(),
        }),
    })
});

static USDC_SEI: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0xe15fC38F6D8c56aF07bbCBe3BAf5708A2Bf42392").into(),
            network: Network::Sei,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USDC".into(),
            version: "2".into(),
        }),
    })
});

static USDC_SEI_TESTNET: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x4fCF1784B31630811181f670Aea7A7bEF803eaED").into(),
            network: Network::SeiTestnet, // Fixed: was Network::Sei in our version
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USDC".into(),
            version: "2".into(),
        }),
    })
});

/// Lazily initialized known USDC deployment on Ethereum mainnet as [`USDCDeployment`].
static USDC_ETHEREUM: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48").into(),
            network: Network::Ethereum,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USD Coin".into(),
            version: "2".into(),
        }),
    })
});

/// Lazily initialized known USDC deployment on Ethereum Sepolia testnet as [`USDCDeployment`].
static USDC_ETHEREUM_SEPOLIA: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x1c7D4B196Cb0C7B01d743Fbc6116a902379C7238").into(),
            network: Network::EthereumSepolia,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USDC".into(),
            version: "2".into(),
        }),
    })
});

/// Lazily initialized known USDC deployment on Arbitrum One mainnet as [`USDCDeployment`].
static USDC_ARBITRUM: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0xaf88d065e77c8cC2239327C5EDb3A432268e5831").into(),
            network: Network::Arbitrum,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USD Coin".into(),
            version: "2".into(),
        }),
    })
});

/// Lazily initialized known USDC deployment on Arbitrum Sepolia testnet as [`USDCDeployment`].
static USDC_ARBITRUM_SEPOLIA: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x75faf114eafb1BDbe2F0316DF893fd58CE46AA4d").into(),
            network: Network::ArbitrumSepolia,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USDC".into(),
            version: "2".into(),
        }),
    })
});

/// Lazily initialized known USDC deployment on Unichain mainnet as [`USDCDeployment`].
static USDC_UNICHAIN: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x078D782b760474a361dDA0AF3839290b0EF57AD6").into(),
            network: Network::Unichain,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USDC".into(),
            version: "2".into(),
        }),
    })
});

/// Lazily initialized known USDC deployment on Unichain Sepolia testnet as [`USDCDeployment`].
static USDC_UNICHAIN_SEPOLIA: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x31d0220469e10c4E71834a79b1f276d740d3768F").into(),
            network: Network::UnichainSepolia,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USDC".into(),
            version: "2".into(),
        }),
    })
});

/// Lazily initialized known USDC deployment on Monad mainnet as [`USDCDeployment`].
static USDC_MONAD: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x754704bc059f8c67012fed69bc8a327a5aafb603").into(),
            network: Network::Monad,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USDC".into(),
            version: "2".into(),
        }),
    })
});

/// Lazily initialized known USDC deployment on NEAR Protocol mainnet as [`USDCDeployment`].
/// NEAR uses native Circle USDC (not bridged).
static USDC_NEAR: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: MixedAddress::Near(
                "17208628f84f5d6ad33f0da3bbbeb27ffcb398eac501a31bd6ad2011e36133a1".to_string(),
            ),
            network: Network::Near,
        },
        decimals: 6,
        eip712: None, // NEAR uses borsh serialization, not EIP-712
    })
});

/// Lazily initialized known USDC deployment on NEAR Protocol testnet as [`USDCDeployment`].
static USDC_NEAR_TESTNET: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: MixedAddress::Near(
                "3e2210e1184b45b64c8a434c0a7e7b23cc04ea7eb7a6c3c32520d03d4afcb8af".to_string(),
            ),
            network: Network::NearTestnet,
        },
        decimals: 6,
        eip712: None,
    })
});

/// Lazily initialized known USDC deployment on Stellar mainnet as [`USDCDeployment`].
/// Note: Stellar USDC has 7 decimals (not 6 like other chains).
static USDC_STELLAR: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: MixedAddress::Stellar(
                "CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75".to_string(),
            ),
            network: Network::Stellar,
        },
        decimals: 7, // Stellar USDC uses 7 decimals
        eip712: None, // Stellar uses XDR, not EIP-712
    })
});

/// Lazily initialized known USDC deployment on Stellar testnet as [`USDCDeployment`].
static USDC_STELLAR_TESTNET: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: MixedAddress::Stellar(
                "CBIELTK6YBZJU5UP2WWQEUCYKLPU6AUNZ2BQ4WWFEIE3USCIHMXQDAMA".to_string(),
            ),
            network: Network::StellarTestnet,
        },
        decimals: 7, // Stellar USDC uses 7 decimals
        eip712: None,
    })
});

/// Lazily initialized known USDC deployment on Fogo mainnet as [`USDCDeployment`].
static USDC_FOGO: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: MixedAddress::Solana(
                Pubkey::from_str("uSd2czE61Evaf76RNbq4KPpXnkiL3irdzgLFUMe3NoG").unwrap(),
            ),
            network: Network::Fogo,
        },
        decimals: 6,
        eip712: None,
    })
});

/// Lazily initialized known USDC deployment on Fogo testnet as [`USDCDeployment`].
static USDC_FOGO_TESTNET: Lazy<USDCDeployment> = Lazy::new(|| {
    USDCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: MixedAddress::Solana(
                Pubkey::from_str("ELNbJ1RtERV2fjtuZjbTscDekWhVzkQ1LjmiPsxp5uND").unwrap(),
            ),
            network: Network::FogoTestnet,
        },
        decimals: 6,
        eip712: None,
    })
});

/// A known USDC deployment as a wrapper around [`TokenDeployment`].
#[derive(Clone, Debug)]
pub struct USDCDeployment(pub TokenDeployment);

impl Deref for USDCDeployment {
    type Target = TokenDeployment;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<&USDCDeployment> for TokenDeployment {
    fn from(deployment: &USDCDeployment) -> Self {
        deployment.0.clone()
    }
}

impl From<USDCDeployment> for Vec<TokenAsset> {
    fn from(deployment: USDCDeployment) -> Self {
        vec![deployment.asset.clone()]
    }
}

impl From<&USDCDeployment> for Vec<TokenAsset> {
    fn from(deployment: &USDCDeployment) -> Self {
        vec![deployment.asset.clone()]
    }
}

impl USDCDeployment {
    /// Return the known USDC deployment for the given network.
    ///
    /// Panic if the network is unsupported (not expected in practice).
    pub fn by_network<N: Borrow<Network>>(network: N) -> &'static USDCDeployment {
        match network.borrow() {
            Network::BaseSepolia => &USDC_BASE_SEPOLIA,
            Network::Base => &USDC_BASE,
            Network::XdcMainnet => &USDC_XDC,
            Network::AvalancheFuji => &USDC_AVALANCHE_FUJI,
            Network::Avalanche => &USDC_AVALANCHE,
            Network::XrplEvm => &USDC_XRPL_EVM,
            Network::Solana => &USDC_SOLANA,
            Network::SolanaDevnet => &USDC_SOLANA_DEVNET,
            Network::PolygonAmoy => &USDC_POLYGON_AMOY,
            Network::Polygon => &USDC_POLYGON,
            Network::Optimism => &USDC_OPTIMISM,
            Network::OptimismSepolia => &USDC_OPTIMISM_SEPOLIA,
            Network::Celo => &USDC_CELO,
            Network::CeloSepolia => &USDC_CELO_SEPOLIA,
            Network::HyperEvm => &USDC_HYPEREVM,
            Network::HyperEvmTestnet => &USDC_HYPEREVM_TESTNET,
            Network::Sei => &USDC_SEI,
            Network::SeiTestnet => &USDC_SEI_TESTNET,
            Network::Ethereum => &USDC_ETHEREUM,
            Network::EthereumSepolia => &USDC_ETHEREUM_SEPOLIA,
            Network::Arbitrum => &USDC_ARBITRUM,
            Network::ArbitrumSepolia => &USDC_ARBITRUM_SEPOLIA,
            Network::Unichain => &USDC_UNICHAIN,
            Network::UnichainSepolia => &USDC_UNICHAIN_SEPOLIA,
            Network::Monad => &USDC_MONAD,
            Network::Near => &USDC_NEAR,
            Network::NearTestnet => &USDC_NEAR_TESTNET,
            Network::Stellar => &USDC_STELLAR,
            Network::StellarTestnet => &USDC_STELLAR_TESTNET,
            Network::Fogo => &USDC_FOGO,
            Network::FogoTestnet => &USDC_FOGO_TESTNET,
        }
    }
}

// ============================================================================
// EURC (Euro Coin) Deployments - Circle
// ============================================================================

/// EURC deployment on Ethereum mainnet.
static EURC_ETHEREUM: Lazy<EURCDeployment> = Lazy::new(|| {
    EURCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x1aBaEA1f7C830bD89Acc67eC4af516284b1bC33c").into(),
            network: Network::Ethereum,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "Euro Coin".into(),
            version: "2".into(),
        }),
    })
});

/// EURC deployment on Base mainnet.
/// NOTE: Base EURC uses "EURC" as EIP-712 domain name (NOT "Euro Coin" like on Ethereum/Avalanche)
static EURC_BASE: Lazy<EURCDeployment> = Lazy::new(|| {
    EURCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x60a3E35Cc302bFA44Cb288Bc5a4F316Fdb1adb42").into(),
            network: Network::Base,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "EURC".into(),
            version: "2".into(),
        }),
    })
});

/// EURC deployment on Avalanche mainnet.
static EURC_AVALANCHE: Lazy<EURCDeployment> = Lazy::new(|| {
    EURCDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0xC891EB4cbdEFf6e073e859e987815Ed1505c2ACD").into(),
            network: Network::Avalanche,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "Euro Coin".into(),
            version: "2".into(),
        }),
    })
});

/// A known EURC (Euro Coin) deployment as a wrapper around [`TokenDeployment`].
#[derive(Clone, Debug)]
pub struct EURCDeployment(pub TokenDeployment);

impl Deref for EURCDeployment {
    type Target = TokenDeployment;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<&EURCDeployment> for TokenDeployment {
    fn from(deployment: &EURCDeployment) -> Self {
        deployment.0.clone()
    }
}

impl EURCDeployment {
    /// Return the known EURC deployment for the given network.
    ///
    /// Returns `None` if EURC is not deployed on the specified network.
    pub fn by_network<N: Borrow<Network>>(network: N) -> Option<&'static EURCDeployment> {
        match network.borrow() {
            Network::Ethereum => Some(&EURC_ETHEREUM),
            Network::Base => Some(&EURC_BASE),
            Network::Avalanche => Some(&EURC_AVALANCHE),
            _ => None,
        }
    }

    /// Return all networks where EURC is deployed.
    pub fn supported_networks() -> &'static [Network] {
        &[Network::Ethereum, Network::Base, Network::Avalanche]
    }
}

// ============================================================================
// AUSD (Agora USD) Deployments - Agora Finance
// Uses deterministic CREATE2 address: 0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a
// ============================================================================

/// AUSD deployment on Ethereum mainnet.
static AUSD_ETHEREUM: Lazy<AUSDDeployment> = Lazy::new(|| {
    AUSDDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a").into(),
            network: Network::Ethereum,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "AUSD".into(),
            version: "1".into(),
        }),
    })
});

/// AUSD deployment on Polygon mainnet.
static AUSD_POLYGON: Lazy<AUSDDeployment> = Lazy::new(|| {
    AUSDDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a").into(),
            network: Network::Polygon,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "AUSD".into(),
            version: "1".into(),
        }),
    })
});

/// AUSD deployment on Arbitrum mainnet.
static AUSD_ARBITRUM: Lazy<AUSDDeployment> = Lazy::new(|| {
    AUSDDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a").into(),
            network: Network::Arbitrum,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "AUSD".into(),
            version: "1".into(),
        }),
    })
});

/// AUSD deployment on Avalanche mainnet.
static AUSD_AVALANCHE: Lazy<AUSDDeployment> = Lazy::new(|| {
    AUSDDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a").into(),
            network: Network::Avalanche,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "AUSD".into(),
            version: "1".into(),
        }),
    })
});

/// AUSD deployment on Monad mainnet.
static AUSD_MONAD: Lazy<AUSDDeployment> = Lazy::new(|| {
    AUSDDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x00000000eFE302BEAA2b3e6e1b18d08D69a9012a").into(),
            network: Network::Monad,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "AUSD".into(),
            version: "1".into(),
        }),
    })
});

/// A known AUSD (Agora USD) deployment as a wrapper around [`TokenDeployment`].
#[derive(Clone, Debug)]
pub struct AUSDDeployment(pub TokenDeployment);

impl Deref for AUSDDeployment {
    type Target = TokenDeployment;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<&AUSDDeployment> for TokenDeployment {
    fn from(deployment: &AUSDDeployment) -> Self {
        deployment.0.clone()
    }
}

impl AUSDDeployment {
    /// Return the known AUSD deployment for the given network.
    ///
    /// Returns `None` if AUSD is not deployed on the specified network.
    /// Note: AUSD uses CREATE2, so address is same on all supported chains.
    pub fn by_network<N: Borrow<Network>>(network: N) -> Option<&'static AUSDDeployment> {
        match network.borrow() {
            Network::Ethereum => Some(&AUSD_ETHEREUM),
            Network::Polygon => Some(&AUSD_POLYGON),
            Network::Arbitrum => Some(&AUSD_ARBITRUM),
            Network::Avalanche => Some(&AUSD_AVALANCHE),
            Network::Monad => Some(&AUSD_MONAD),
            _ => None,
        }
    }

    /// Return all networks where AUSD is deployed with EIP-3009 support.
    pub fn supported_networks() -> &'static [Network] {
        &[
            Network::Ethereum,
            Network::Polygon,
            Network::Arbitrum,
            Network::Avalanche,
            Network::Monad,
        ]
    }
}

// ============================================================================
// PYUSD (PayPal USD) Deployments - PayPal/Paxos
// ============================================================================

/// PYUSD deployment on Ethereum mainnet.
static PYUSD_ETHEREUM: Lazy<PYUSDDeployment> = Lazy::new(|| {
    PYUSDDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x6c3ea9036406852006290770BEdFcAbA0e23A0e8").into(),
            network: Network::Ethereum,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "PayPal USD".into(),
            version: "1".into(),
        }),
    })
});

/// A known PYUSD (PayPal USD) deployment as a wrapper around [`TokenDeployment`].
#[derive(Clone, Debug)]
pub struct PYUSDDeployment(pub TokenDeployment);

impl Deref for PYUSDDeployment {
    type Target = TokenDeployment;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<&PYUSDDeployment> for TokenDeployment {
    fn from(deployment: &PYUSDDeployment) -> Self {
        deployment.0.clone()
    }
}

impl PYUSDDeployment {
    /// Return the known PYUSD deployment for the given network.
    ///
    /// Returns `None` if PYUSD is not deployed on the specified network.
    /// Note: PYUSD is currently only available on Ethereum mainnet.
    pub fn by_network<N: Borrow<Network>>(network: N) -> Option<&'static PYUSDDeployment> {
        match network.borrow() {
            Network::Ethereum => Some(&PYUSD_ETHEREUM),
            _ => None,
        }
    }

    /// Return all networks where PYUSD is deployed.
    pub fn supported_networks() -> &'static [Network] {
        &[Network::Ethereum]
    }
}

// ============================================================================
// USDT (Tether USD / USDT0) Deployments - Tether via LayerZero OFT
// ============================================================================

/// USDT0 deployment on Arbitrum mainnet.
/// Contract: 0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9
/// EIP-712 name: "USD₮0" (with TUGRIK SIGN Unicode character)
static USDT_ARBITRUM: Lazy<USDTDeployment> = Lazy::new(|| {
    USDTDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9").into(),
            network: Network::Arbitrum,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USD\u{20AE}0".into(), // USD₮0 (TUGRIK SIGN)
            version: "1".into(),
        }),
    })
});

/// USDT deployment on Celo mainnet.
/// Contract: 0x48065fbBE25f71C9282ddf5e1cD6D6A887483D5e
/// EIP-712 name: "Tether USD" (standard Tether name on Celo)
static USDT_CELO: Lazy<USDTDeployment> = Lazy::new(|| {
    USDTDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x48065fbBE25f71C9282ddf5e1cD6D6A887483D5e").into(),
            network: Network::Celo,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "Tether USD".into(),
            version: "1".into(),
        }),
    })
});

/// USDT0 deployment on Optimism mainnet.
/// Contract: 0x01bff41798a0bcf287b996046ca68b395dbc1071
/// EIP-712 name: "USD₮0" (with TUGRIK SIGN Unicode character)
static USDT_OPTIMISM: Lazy<USDTDeployment> = Lazy::new(|| {
    USDTDeployment(TokenDeployment {
        asset: TokenAsset {
            address: address!("0x01bff41798a0bcf287b996046ca68b395dbc1071").into(),
            network: Network::Optimism,
        },
        decimals: 6,
        eip712: Some(TokenDeploymentEip712 {
            name: "USD\u{20AE}0".into(), // USD₮0 (TUGRIK SIGN)
            version: "1".into(),
        }),
    })
});

/// A known USDT (Tether USD / USDT0) deployment as a wrapper around [`TokenDeployment`].
///
/// USDT0 is Tether's omnichain stablecoin launched in January 2025 using LayerZero OFT.
/// It supports EIP-3009 `transferWithAuthorization` for gasless transfers.
#[derive(Clone, Debug)]
pub struct USDTDeployment(pub TokenDeployment);

impl Deref for USDTDeployment {
    type Target = TokenDeployment;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<&USDTDeployment> for TokenDeployment {
    fn from(deployment: &USDTDeployment) -> Self {
        deployment.0.clone()
    }
}

impl USDTDeployment {
    /// Return the known USDT deployment for the given network.
    ///
    /// Returns `None` if USDT is not deployed on the specified network.
    /// Note: USDT0 (EIP-3009 compatible) is only on Arbitrum, Celo, and Optimism.
    /// Legacy USDT on Ethereum does NOT support EIP-3009.
    pub fn by_network<N: Borrow<Network>>(network: N) -> Option<&'static USDTDeployment> {
        match network.borrow() {
            Network::Arbitrum => Some(&USDT_ARBITRUM),
            Network::Celo => Some(&USDT_CELO),
            Network::Optimism => Some(&USDT_OPTIMISM),
            _ => None,
        }
    }

    /// Return all networks where USDT0 is deployed with EIP-3009 support.
    pub fn supported_networks() -> &'static [Network] {
        &[Network::Arbitrum, Network::Celo, Network::Optimism]
    }
}

// ============================================================================
// Generic Token Deployment Lookup
// ============================================================================

/// Get a token deployment for any supported token type on a given network.
///
/// Returns `None` if the token is not deployed on the specified network.
/// For USDC, this always returns `Some` as USDC is deployed on all networks.
///
/// # Example
/// ```ignore
/// use x402_rs::network::{get_token_deployment, Network};
/// use x402_rs::types::TokenType;
///
/// let deployment = get_token_deployment(Network::Ethereum, TokenType::Eurc);
/// assert!(deployment.is_some());
/// ```
pub fn get_token_deployment(network: Network, token_type: TokenType) -> Option<TokenDeployment> {
    match token_type {
        TokenType::Usdc => Some(USDCDeployment::by_network(network).0.clone()),
        TokenType::Eurc => EURCDeployment::by_network(network).map(|d| d.0.clone()),
        TokenType::Ausd => AUSDDeployment::by_network(network).map(|d| d.0.clone()),
        TokenType::Pyusd => PYUSDDeployment::by_network(network).map(|d| d.0.clone()),
        TokenType::Usdt => USDTDeployment::by_network(network).map(|d| d.0.clone()),
    }
}

/// Check if a token type is supported on a given network.
///
/// # Example
/// ```ignore
/// use x402_rs::network::{is_token_supported, Network};
/// use x402_rs::types::TokenType;
///
/// assert!(is_token_supported(Network::Ethereum, TokenType::Usdc));
/// assert!(is_token_supported(Network::Ethereum, TokenType::Eurc));
/// assert!(!is_token_supported(Network::Solana, TokenType::Eurc)); // EURC not on Solana
/// ```
pub fn is_token_supported(network: Network, token_type: TokenType) -> bool {
    get_token_deployment(network, token_type).is_some()
}

/// Get all supported token types for a given network.
///
/// # Example
/// ```ignore
/// use x402_rs::network::{supported_tokens_for_network, Network};
///
/// let tokens = supported_tokens_for_network(Network::Ethereum);
/// // Returns [USDC, EURC, AUSD, PYUSD] for Ethereum
/// ```
pub fn supported_tokens_for_network(network: Network) -> Vec<TokenType> {
    TokenType::all()
        .iter()
        .filter(|&&token| is_token_supported(network, token))
        .copied()
        .collect()
}

/// Get all supported networks for a given token type.
///
/// # Example
/// ```ignore
/// use x402_rs::network::{supported_networks_for_token, Network};
/// use x402_rs::types::TokenType;
///
/// let networks = supported_networks_for_token(TokenType::Eurc);
/// // Returns [Ethereum, Base, Avalanche]
/// ```
pub fn supported_networks_for_token(token_type: TokenType) -> Vec<Network> {
    match token_type {
        TokenType::Usdc => Network::variants().to_vec(),
        TokenType::Eurc => EURCDeployment::supported_networks().to_vec(),
        TokenType::Ausd => AUSDDeployment::supported_networks().to_vec(),
        TokenType::Pyusd => PYUSDDeployment::supported_networks().to_vec(),
        TokenType::Usdt => USDTDeployment::supported_networks().to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::EvmAddress;
    use alloy::primitives::address;

    // ============================================================
    // USDC Deployment Tests
    // ============================================================

    #[test]
    fn test_usdc_available_on_all_networks() {
        // USDC should be available on all networks
        for network in Network::variants() {
            assert!(
                is_token_supported(*network, TokenType::Usdc),
                "USDC should be supported on {:?}",
                network
            );
            assert!(
                get_token_deployment(*network, TokenType::Usdc).is_some(),
                "USDC deployment should exist for {:?}",
                network
            );
        }
    }

    #[test]
    fn test_usdc_decimals() {
        for network in Network::variants() {
            let deployment = get_token_deployment(*network, TokenType::Usdc).unwrap();
            // Most networks use 6 decimals, but Stellar uses 7
            let expected_decimals = match network {
                Network::Stellar | Network::StellarTestnet => 7,
                _ => 6,
            };
            assert_eq!(
                deployment.decimals, expected_decimals,
                "USDC should have {} decimals on {:?}",
                expected_decimals, network
            );
        }
    }

    #[test]
    fn test_usdc_base_address() {
        let deployment = get_token_deployment(Network::Base, TokenType::Usdc).unwrap();
        assert_eq!(
            deployment.asset.address,
            MixedAddress::Evm(address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913").into())
        );
    }

    // ============================================================
    // EURC Deployment Tests
    // ============================================================

    #[test]
    fn test_eurc_supported_networks() {
        let networks = EURCDeployment::supported_networks();
        assert!(networks.contains(&Network::Ethereum));
        assert!(networks.contains(&Network::Base));
        assert!(networks.contains(&Network::Avalanche));
        assert_eq!(networks.len(), 3);
    }

    #[test]
    fn test_eurc_not_on_unsupported_networks() {
        assert!(!is_token_supported(Network::Polygon, TokenType::Eurc));
        assert!(!is_token_supported(Network::Optimism, TokenType::Eurc));
        assert!(get_token_deployment(Network::Polygon, TokenType::Eurc).is_none());
    }

    #[test]
    fn test_eurc_ethereum_address() {
        let deployment = get_token_deployment(Network::Ethereum, TokenType::Eurc).unwrap();
        assert_eq!(
            deployment.asset.address,
            MixedAddress::Evm(address!("1aBaEA1f7C830bD89Acc67eC4af516284b1bC33c").into())
        );
        assert_eq!(deployment.decimals, 6);
    }

    // ============================================================
    // AUSD Deployment Tests (CREATE2 - same address all chains)
    // ============================================================

    #[test]
    fn test_ausd_same_address_all_networks() {
        let expected_address: EvmAddress = address!("00000000eFE302BEAA2b3e6e1b18d08D69a9012a").into();
        let networks = AUSDDeployment::supported_networks();

        for network in networks {
            let deployment = get_token_deployment(*network, TokenType::Ausd).unwrap();
            assert_eq!(
                deployment.asset.address,
                MixedAddress::Evm(expected_address.clone()),
                "AUSD should have same CREATE2 address on {:?}",
                network
            );
        }
    }

    #[test]
    fn test_ausd_supported_networks() {
        let networks = AUSDDeployment::supported_networks();
        assert!(networks.contains(&Network::Ethereum));
        assert!(networks.contains(&Network::Polygon));
        assert!(networks.contains(&Network::Arbitrum));
        assert!(networks.contains(&Network::Avalanche));
    }

    // ============================================================
    // PYUSD Deployment Tests (Ethereum only)
    // ============================================================

    #[test]
    fn test_pyusd_ethereum_only() {
        let networks = PYUSDDeployment::supported_networks();
        assert_eq!(networks.len(), 1);
        assert_eq!(networks[0], Network::Ethereum);
    }

    #[test]
    fn test_pyusd_not_on_other_networks() {
        assert!(!is_token_supported(Network::Base, TokenType::Pyusd));
        assert!(!is_token_supported(Network::Polygon, TokenType::Pyusd));
        assert!(get_token_deployment(Network::Base, TokenType::Pyusd).is_none());
    }

    #[test]
    fn test_pyusd_ethereum_address() {
        let deployment = get_token_deployment(Network::Ethereum, TokenType::Pyusd).unwrap();
        assert_eq!(
            deployment.asset.address,
            MixedAddress::Evm(address!("6c3ea9036406852006290770BEdFcAbA0e23A0e8").into())
        );
        assert_eq!(deployment.decimals, 6);
    }

    // ============================================================
    // USDT (Tether USD / USDT0) Deployment Tests
    // ============================================================

    #[test]
    fn test_usdt_supported_networks() {
        let networks = USDTDeployment::supported_networks();
        assert_eq!(networks.len(), 3);
        assert!(networks.contains(&Network::Arbitrum));
        assert!(networks.contains(&Network::Celo));
        assert!(networks.contains(&Network::Optimism));
    }

    #[test]
    fn test_usdt_not_on_ethereum() {
        // Legacy USDT on Ethereum does NOT support EIP-3009
        assert!(!is_token_supported(Network::Ethereum, TokenType::Usdt));
        assert!(get_token_deployment(Network::Ethereum, TokenType::Usdt).is_none());
    }

    #[test]
    fn test_usdt_arbitrum_address() {
        let deployment = get_token_deployment(Network::Arbitrum, TokenType::Usdt).unwrap();
        assert_eq!(
            deployment.asset.address,
            MixedAddress::Evm(address!("Fd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9").into())
        );
        assert_eq!(deployment.decimals, 6);
        // Check EIP-712 name (USD₮0 with Unicode TUGRIK SIGN)
        assert_eq!(deployment.eip712.as_ref().unwrap().name, "USD\u{20AE}0");
    }

    #[test]
    fn test_usdt_celo_address() {
        let deployment = get_token_deployment(Network::Celo, TokenType::Usdt).unwrap();
        assert_eq!(
            deployment.asset.address,
            MixedAddress::Evm(address!("48065fbBE25f71C9282ddf5e1cD6D6A887483D5e").into())
        );
        assert_eq!(deployment.decimals, 6);
        // Celo uses "Tether USD" as EIP-712 name
        assert_eq!(deployment.eip712.as_ref().unwrap().name, "Tether USD");
    }

    #[test]
    fn test_usdt_optimism_address() {
        let deployment = get_token_deployment(Network::Optimism, TokenType::Usdt).unwrap();
        assert_eq!(
            deployment.asset.address,
            MixedAddress::Evm(address!("01bff41798a0bcf287b996046ca68b395dbc1071").into())
        );
        assert_eq!(deployment.decimals, 6);
        // Check EIP-712 name (USD₮0 with Unicode TUGRIK SIGN)
        assert_eq!(deployment.eip712.as_ref().unwrap().name, "USD\u{20AE}0");
    }

    // ============================================================
    // Helper Function Tests
    // ============================================================

    #[test]
    fn test_supported_tokens_for_ethereum() {
        // Ethereum supports all tokens
        let tokens = supported_tokens_for_network(Network::Ethereum);
        assert_eq!(tokens.len(), 4);
        assert!(tokens.contains(&TokenType::Usdc));
        assert!(tokens.contains(&TokenType::Eurc));
        assert!(tokens.contains(&TokenType::Ausd));
        assert!(tokens.contains(&TokenType::Pyusd));
    }

    #[test]
    fn test_supported_tokens_for_base() {
        // Base supports USDC, EURC
        let tokens = supported_tokens_for_network(Network::Base);
        assert!(tokens.contains(&TokenType::Usdc));
        assert!(tokens.contains(&TokenType::Eurc));
        assert!(!tokens.contains(&TokenType::Pyusd));
        assert!(!tokens.contains(&TokenType::Ausd));
        assert!(!tokens.contains(&TokenType::Usdt));
    }

    #[test]
    fn test_supported_tokens_for_polygon() {
        // Polygon supports USDC, AUSD
        let tokens = supported_tokens_for_network(Network::Polygon);
        assert!(tokens.contains(&TokenType::Usdc));
        assert!(tokens.contains(&TokenType::Ausd));
        assert!(!tokens.contains(&TokenType::Eurc));
        assert!(!tokens.contains(&TokenType::Usdt));
    }

    #[test]
    fn test_supported_tokens_for_arbitrum() {
        // Arbitrum supports USDC, AUSD, USDT
        let tokens = supported_tokens_for_network(Network::Arbitrum);
        assert!(tokens.contains(&TokenType::Usdc));
        assert!(tokens.contains(&TokenType::Ausd));
        assert!(tokens.contains(&TokenType::Usdt));
        assert!(!tokens.contains(&TokenType::Eurc));
        assert!(!tokens.contains(&TokenType::Pyusd));
    }

    #[test]
    fn test_supported_tokens_for_optimism() {
        // Optimism supports USDC, USDT
        let tokens = supported_tokens_for_network(Network::Optimism);
        assert!(tokens.contains(&TokenType::Usdc));
        assert!(tokens.contains(&TokenType::Usdt));
        assert!(!tokens.contains(&TokenType::Eurc));
        assert!(!tokens.contains(&TokenType::Pyusd));
    }

    #[test]
    fn test_supported_tokens_for_celo() {
        // Celo supports USDC, USDT
        let tokens = supported_tokens_for_network(Network::Celo);
        assert!(tokens.contains(&TokenType::Usdc));
        assert!(tokens.contains(&TokenType::Usdt));
        assert!(!tokens.contains(&TokenType::Eurc));
        assert!(!tokens.contains(&TokenType::Pyusd));
    }

    #[test]
    fn test_supported_tokens_for_solana() {
        // Solana only supports USDC (non-EVM)
        let tokens = supported_tokens_for_network(Network::Solana);
        assert_eq!(tokens.len(), 1);
        assert!(tokens.contains(&TokenType::Usdc));
    }

    #[test]
    fn test_supported_networks_for_usdc() {
        let networks = supported_networks_for_token(TokenType::Usdc);
        // USDC is on all networks
        assert_eq!(networks.len(), Network::variants().len());
    }

    #[test]
    fn test_supported_networks_for_eurc() {
        let networks = supported_networks_for_token(TokenType::Eurc);
        assert_eq!(networks.len(), 3);
        assert!(networks.contains(&Network::Ethereum));
        assert!(networks.contains(&Network::Base));
        assert!(networks.contains(&Network::Avalanche));
    }

    #[test]
    fn test_supported_networks_for_usdt() {
        let networks = supported_networks_for_token(TokenType::Usdt);
        assert_eq!(networks.len(), 3);
        assert!(networks.contains(&Network::Arbitrum));
        assert!(networks.contains(&Network::Celo));
        assert!(networks.contains(&Network::Optimism));
        // USDT not on Ethereum (legacy contract doesn't support EIP-3009)
        assert!(!networks.contains(&Network::Ethereum));
    }

    #[test]
    fn test_get_token_deployment_returns_correct_type() {
        // Verify the deployment contains correct token info
        let usdc = get_token_deployment(Network::Base, TokenType::Usdc).unwrap();
        assert_eq!(usdc.decimals, 6);

        let eurc = get_token_deployment(Network::Ethereum, TokenType::Eurc).unwrap();
        assert_eq!(eurc.decimals, 6);
    }

    #[test]
    fn test_token_deployment_address_method() {
        let deployment = get_token_deployment(Network::Base, TokenType::Usdc).unwrap();
        let address = deployment.address();
        assert!(matches!(address, MixedAddress::Evm(_)));
    }
}
