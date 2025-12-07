//! Network definitions and known token deployments.
//!
//! This module defines supported networks and their chain IDs,
//! and provides statically known USDC deployments per network.

use crate::types::{MixedAddress, TokenAsset, TokenDeployment, TokenDeploymentEip712};
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
            Network::Fogo => write!(f, "fogo"),
            Network::FogoTestnet => write!(f, "fogo-testnet"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum NetworkFamily {
    Evm,
    Solana,
    Near,
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
                | Network::FogoTestnet
        )
    }

    /// Returns true if this network is a mainnet environment.
    pub fn is_mainnet(&self) -> bool {
        !self.is_testnet()
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
            Network::Fogo => &USDC_FOGO,
            Network::FogoTestnet => &USDC_FOGO_TESTNET,
        }
    }
}
