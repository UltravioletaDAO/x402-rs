use crate::network::Network;
use alloy::network::EthereumWallet;
use alloy::signers::local::PrivateKeySigner;
use serde::Deserialize;
use serde::Serialize;
use solana_sdk::signature::Keypair;
use std::env;
use std::str::FromStr;

pub const ENV_SIGNER_TYPE: &str = "SIGNER_TYPE";
pub const ENV_EVM_PRIVATE_KEY: &str = "EVM_PRIVATE_KEY";
pub const ENV_EVM_PRIVATE_KEY_MAINNET: &str = "EVM_PRIVATE_KEY_MAINNET";
pub const ENV_EVM_PRIVATE_KEY_TESTNET: &str = "EVM_PRIVATE_KEY_TESTNET";
pub const ENV_SOLANA_PRIVATE_KEY: &str = "SOLANA_PRIVATE_KEY";
pub const ENV_SOLANA_PRIVATE_KEY_MAINNET: &str = "SOLANA_PRIVATE_KEY_MAINNET";
pub const ENV_SOLANA_PRIVATE_KEY_TESTNET: &str = "SOLANA_PRIVATE_KEY_TESTNET";
pub const ENV_NEAR_PRIVATE_KEY: &str = "NEAR_PRIVATE_KEY";
pub const ENV_NEAR_PRIVATE_KEY_MAINNET: &str = "NEAR_PRIVATE_KEY_MAINNET";
pub const ENV_NEAR_PRIVATE_KEY_TESTNET: &str = "NEAR_PRIVATE_KEY_TESTNET";
pub const ENV_NEAR_ACCOUNT_ID: &str = "NEAR_ACCOUNT_ID";
pub const ENV_NEAR_ACCOUNT_ID_MAINNET: &str = "NEAR_ACCOUNT_ID_MAINNET";
pub const ENV_NEAR_ACCOUNT_ID_TESTNET: &str = "NEAR_ACCOUNT_ID_TESTNET";

// Stellar environment variables
pub const ENV_STELLAR_PRIVATE_KEY: &str = "STELLAR_PRIVATE_KEY";
pub const ENV_STELLAR_PRIVATE_KEY_MAINNET: &str = "STELLAR_PRIVATE_KEY_MAINNET";
pub const ENV_STELLAR_PRIVATE_KEY_TESTNET: &str = "STELLAR_PRIVATE_KEY_TESTNET";

// Algorand environment variables (25-word mnemonic)
pub const ENV_ALGORAND_MNEMONIC: &str = "ALGORAND_MNEMONIC";
pub const ENV_ALGORAND_MNEMONIC_MAINNET: &str = "ALGORAND_MNEMONIC_MAINNET";
pub const ENV_ALGORAND_MNEMONIC_TESTNET: &str = "ALGORAND_MNEMONIC_TESTNET";

pub const ENV_RPC_BASE: &str = "RPC_URL_BASE";
pub const ENV_RPC_BASE_SEPOLIA: &str = "RPC_URL_BASE_SEPOLIA";
pub const ENV_RPC_XDC: &str = "RPC_URL_XDC";
pub const ENV_RPC_AVALANCHE_FUJI: &str = "RPC_URL_AVALANCHE_FUJI";
pub const ENV_RPC_AVALANCHE: &str = "RPC_URL_AVALANCHE";
pub const ENV_RPC_XRPL_EVM: &str = "RPC_URL_XRPL_EVM";
pub const ENV_RPC_SOLANA: &str = "RPC_URL_SOLANA";
pub const ENV_RPC_SOLANA_DEVNET: &str = "RPC_URL_SOLANA_DEVNET";
pub const ENV_RPC_POLYGON_AMOY: &str = "RPC_URL_POLYGON_AMOY";
pub const ENV_RPC_POLYGON: &str = "RPC_URL_POLYGON";
pub const ENV_RPC_SEI: &str = "RPC_URL_SEI";
pub const ENV_RPC_SEI_TESTNET: &str = "RPC_URL_SEI_TESTNET";
pub const ENV_RPC_CELO: &str = "RPC_URL_CELO";
pub const ENV_RPC_CELO_SEPOLIA: &str = "RPC_URL_CELO_SEPOLIA";
pub const ENV_RPC_HYPEREVM: &str = "RPC_URL_HYPEREVM";
pub const ENV_RPC_HYPEREVM_TESTNET: &str = "RPC_URL_HYPEREVM_TESTNET";
pub const ENV_RPC_OPTIMISM: &str = "RPC_URL_OPTIMISM";
pub const ENV_RPC_OPTIMISM_SEPOLIA: &str = "RPC_URL_OPTIMISM_SEPOLIA";
pub const ENV_RPC_ETHEREUM: &str = "RPC_URL_ETHEREUM";
pub const ENV_RPC_ETHEREUM_SEPOLIA: &str = "RPC_URL_ETHEREUM_SEPOLIA";
pub const ENV_RPC_ARBITRUM: &str = "RPC_URL_ARBITRUM";
pub const ENV_RPC_ARBITRUM_SEPOLIA: &str = "RPC_URL_ARBITRUM_SEPOLIA";
pub const ENV_RPC_UNICHAIN: &str = "RPC_URL_UNICHAIN";
pub const ENV_RPC_UNICHAIN_SEPOLIA: &str = "RPC_URL_UNICHAIN_SEPOLIA";
pub const ENV_RPC_MONAD: &str = "RPC_URL_MONAD";
pub const ENV_RPC_BSC: &str = "RPC_URL_BSC";
pub const ENV_RPC_NEAR: &str = "RPC_URL_NEAR";
pub const ENV_RPC_NEAR_TESTNET: &str = "RPC_URL_NEAR_TESTNET";

// Stellar RPC (Horizon/Soroban) URLs
pub const ENV_RPC_STELLAR: &str = "RPC_URL_STELLAR";
pub const ENV_RPC_STELLAR_TESTNET: &str = "RPC_URL_STELLAR_TESTNET";
pub const ENV_RPC_FOGO: &str = "RPC_URL_FOGO";
pub const ENV_RPC_FOGO_TESTNET: &str = "RPC_URL_FOGO_TESTNET";

// Algorand RPC (Algod) URLs
pub const ENV_RPC_ALGORAND: &str = "RPC_URL_ALGORAND";
pub const ENV_RPC_ALGORAND_TESTNET: &str = "RPC_URL_ALGORAND_TESTNET";

// Sui RPC URLs
#[cfg(feature = "sui")]
pub const ENV_RPC_SUI: &str = "RPC_URL_SUI";
#[cfg(feature = "sui")]
pub const ENV_RPC_SUI_TESTNET: &str = "RPC_URL_SUI_TESTNET";

// SKALE RPC URLs (L3 on Base with gasless transactions)
pub const ENV_RPC_SKALE_BASE: &str = "RPC_URL_SKALE_BASE";
pub const ENV_RPC_SKALE_BASE_SEPOLIA: &str = "RPC_URL_SKALE_BASE_SEPOLIA";

// Scroll RPC URL (zkEVM L2 on Ethereum)
pub const ENV_RPC_SCROLL: &str = "RPC_URL_SCROLL";

// Sui wallet private key environment variables
#[cfg(feature = "sui")]
pub const ENV_SUI_PRIVATE_KEY: &str = "SUI_PRIVATE_KEY";
#[cfg(feature = "sui")]
pub const ENV_SUI_PRIVATE_KEY_MAINNET: &str = "SUI_PRIVATE_KEY_MAINNET";
#[cfg(feature = "sui")]
pub const ENV_SUI_PRIVATE_KEY_TESTNET: &str = "SUI_PRIVATE_KEY_TESTNET";

pub fn rpc_env_name_from_network(network: Network) -> &'static str {
    match network {
        Network::BaseSepolia => ENV_RPC_BASE_SEPOLIA,
        Network::Base => ENV_RPC_BASE,
        Network::XdcMainnet => ENV_RPC_XDC,
        Network::AvalancheFuji => ENV_RPC_AVALANCHE_FUJI,
        Network::Avalanche => ENV_RPC_AVALANCHE,
        Network::XrplEvm => ENV_RPC_XRPL_EVM,
        Network::Solana => ENV_RPC_SOLANA,
        Network::SolanaDevnet => ENV_RPC_SOLANA_DEVNET,
        Network::PolygonAmoy => ENV_RPC_POLYGON_AMOY,
        Network::Polygon => ENV_RPC_POLYGON,
        Network::Sei => ENV_RPC_SEI,
        Network::SeiTestnet => ENV_RPC_SEI_TESTNET,
        Network::Celo => ENV_RPC_CELO,
        Network::CeloSepolia => ENV_RPC_CELO_SEPOLIA,
        Network::HyperEvm => ENV_RPC_HYPEREVM,
        Network::HyperEvmTestnet => ENV_RPC_HYPEREVM_TESTNET,
        Network::Optimism => ENV_RPC_OPTIMISM,
        Network::OptimismSepolia => ENV_RPC_OPTIMISM_SEPOLIA,
        Network::Ethereum => ENV_RPC_ETHEREUM,
        Network::EthereumSepolia => ENV_RPC_ETHEREUM_SEPOLIA,
        Network::Arbitrum => ENV_RPC_ARBITRUM,
        Network::ArbitrumSepolia => ENV_RPC_ARBITRUM_SEPOLIA,
        Network::Unichain => ENV_RPC_UNICHAIN,
        Network::UnichainSepolia => ENV_RPC_UNICHAIN_SEPOLIA,
        Network::Monad => ENV_RPC_MONAD,
        Network::Bsc => ENV_RPC_BSC,
        Network::Near => ENV_RPC_NEAR,
        Network::NearTestnet => ENV_RPC_NEAR_TESTNET,
        Network::Stellar => ENV_RPC_STELLAR,
        Network::StellarTestnet => ENV_RPC_STELLAR_TESTNET,
        Network::Fogo => ENV_RPC_FOGO,
        Network::FogoTestnet => ENV_RPC_FOGO_TESTNET,
        #[cfg(feature = "algorand")]
        Network::Algorand => ENV_RPC_ALGORAND,
        #[cfg(feature = "algorand")]
        Network::AlgorandTestnet => ENV_RPC_ALGORAND_TESTNET,
        #[cfg(feature = "sui")]
        Network::Sui => ENV_RPC_SUI,
        #[cfg(feature = "sui")]
        Network::SuiTestnet => ENV_RPC_SUI_TESTNET,
        Network::SkaleBase => ENV_RPC_SKALE_BASE,
        Network::SkaleBaseSepolia => ENV_RPC_SKALE_BASE_SEPOLIA,
        Network::Scroll => ENV_RPC_SCROLL,
    }
}

/// Supported methods for constructing an Ethereum wallet from environment variables.
#[derive(Debug, Hash, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignerType {
    /// A local private key stored in the `EVM_PRIVATE_KEY` environment variable.
    #[serde(rename = "private-key")]
    PrivateKey,
}

impl SignerType {
    /// Parse the signer type from the `SIGNER_TYPE` environment variable.
    pub fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        let signer_type_string =
            env::var(ENV_SIGNER_TYPE).map_err(|_| format!("env {ENV_SIGNER_TYPE} not set"))?;
        match signer_type_string.as_str() {
            "private-key" => Ok(SignerType::PrivateKey),
            _ => Err(format!("Unknown signer type {signer_type_string}").into()),
        }
    }

    /// Constructs an [`EthereumWallet`] based on the [`SignerType`] selected from environment.
    ///
    /// Currently only supports [`SignerType::PrivateKey`] variant, based on the following environment variables:
    /// - `SIGNER_TYPE` — currently only `"private-key"` is supported
    /// - `EVM_PRIVATE_KEY_MAINNET` — comma-separated list of private keys for mainnet networks
    /// - `EVM_PRIVATE_KEY_TESTNET` — comma-separated list of private keys for testnet networks
    /// - `EVM_PRIVATE_KEY` — fallback for all networks if network-specific keys are not set
    pub fn make_evm_wallet(
        &self,
        network: Network,
    ) -> Result<EthereumWallet, Box<dyn std::error::Error>> {
        match self {
            SignerType::PrivateKey => {
                // Try network-specific key first, then fall back to generic EVM_PRIVATE_KEY
                let raw_keys = if network.is_testnet() {
                    env::var(ENV_EVM_PRIVATE_KEY_TESTNET)
                        .or_else(|_| env::var(ENV_EVM_PRIVATE_KEY))
                        .map_err(|_| {
                            format!(
                                "env {} or {} not set",
                                ENV_EVM_PRIVATE_KEY_TESTNET, ENV_EVM_PRIVATE_KEY
                            )
                        })?
                } else {
                    env::var(ENV_EVM_PRIVATE_KEY_MAINNET)
                        .or_else(|_| env::var(ENV_EVM_PRIVATE_KEY))
                        .map_err(|_| {
                            format!(
                                "env {} or {} not set",
                                ENV_EVM_PRIVATE_KEY_MAINNET, ENV_EVM_PRIVATE_KEY
                            )
                        })?
                };
                let signers = raw_keys
                    .split(',')
                    .map(str::trim)
                    .filter(|entry| !entry.is_empty())
                    .map(PrivateKeySigner::from_str)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })?;
                if signers.is_empty() {
                    return Err("env EVM_PRIVATE_KEY did not contain any private keys".into());
                }

                let mut iter = signers.into_iter();
                let first_signer = iter
                    .next()
                    .expect("iterator contains at least one element by construction");
                let mut wallet = EthereumWallet::from(first_signer);

                for signer in iter {
                    wallet.register_signer(signer);
                }

                Ok(wallet)
            }
        }
    }

    /// Constructs a Solana [`Keypair`] based on the [`SignerType`] selected from environment.
    ///
    /// Environment variables:
    /// - `SOLANA_PRIVATE_KEY_MAINNET` — base58 private key for mainnet networks
    /// - `SOLANA_PRIVATE_KEY_TESTNET` — base58 private key for testnet networks
    /// - `SOLANA_PRIVATE_KEY` — fallback for all networks if network-specific keys are not set
    pub fn make_solana_wallet(
        &self,
        network: Network,
    ) -> Result<Keypair, Box<dyn std::error::Error>> {
        match self {
            SignerType::PrivateKey => {
                let private_key = if network.is_testnet() {
                    env::var(ENV_SOLANA_PRIVATE_KEY_TESTNET)
                        .or_else(|_| env::var(ENV_SOLANA_PRIVATE_KEY))
                        .map_err(|_| {
                            format!(
                                "env {} or {} not set",
                                ENV_SOLANA_PRIVATE_KEY_TESTNET, ENV_SOLANA_PRIVATE_KEY
                            )
                        })?
                } else {
                    env::var(ENV_SOLANA_PRIVATE_KEY_MAINNET)
                        .or_else(|_| env::var(ENV_SOLANA_PRIVATE_KEY))
                        .map_err(|_| {
                            format!(
                                "env {} or {} not set",
                                ENV_SOLANA_PRIVATE_KEY_MAINNET, ENV_SOLANA_PRIVATE_KEY
                            )
                        })?
                };
                let keypair = Keypair::from_base58_string(private_key.as_str());
                Ok(keypair)
            }
        }
    }

    /// Constructs a NEAR signer based on the [`SignerType`] selected from environment.
    ///
    /// Environment variables:
    /// - `NEAR_PRIVATE_KEY_MAINNET` — ed25519 private key for mainnet (base58 or hex)
    /// - `NEAR_PRIVATE_KEY_TESTNET` — ed25519 private key for testnet (base58 or hex)
    /// - `NEAR_PRIVATE_KEY` — fallback for all networks if network-specific keys are not set
    /// - `NEAR_ACCOUNT_ID_MAINNET` — NEAR account ID for mainnet (implicit or named)
    /// - `NEAR_ACCOUNT_ID_TESTNET` — NEAR account ID for testnet (implicit or named)
    /// - `NEAR_ACCOUNT_ID` — fallback account ID for all networks
    pub fn make_near_signer(
        &self,
        network: Network,
    ) -> Result<(near_crypto::SecretKey, String), Box<dyn std::error::Error>> {
        match self {
            SignerType::PrivateKey => {
                // Get private key based on network type
                let private_key_str = if network.is_testnet() {
                    env::var(ENV_NEAR_PRIVATE_KEY_TESTNET)
                        .or_else(|_| env::var(ENV_NEAR_PRIVATE_KEY))
                        .map_err(|_| {
                            format!(
                                "env {} or {} not set",
                                ENV_NEAR_PRIVATE_KEY_TESTNET, ENV_NEAR_PRIVATE_KEY
                            )
                        })?
                } else {
                    env::var(ENV_NEAR_PRIVATE_KEY_MAINNET)
                        .or_else(|_| env::var(ENV_NEAR_PRIVATE_KEY))
                        .map_err(|_| {
                            format!(
                                "env {} or {} not set",
                                ENV_NEAR_PRIVATE_KEY_MAINNET, ENV_NEAR_PRIVATE_KEY
                            )
                        })?
                };

                // Get account ID based on network type
                let account_id = if network.is_testnet() {
                    env::var(ENV_NEAR_ACCOUNT_ID_TESTNET)
                        .or_else(|_| env::var(ENV_NEAR_ACCOUNT_ID))
                        .map_err(|_| {
                            format!(
                                "env {} or {} not set",
                                ENV_NEAR_ACCOUNT_ID_TESTNET, ENV_NEAR_ACCOUNT_ID
                            )
                        })?
                } else {
                    env::var(ENV_NEAR_ACCOUNT_ID_MAINNET)
                        .or_else(|_| env::var(ENV_NEAR_ACCOUNT_ID))
                        .map_err(|_| {
                            format!(
                                "env {} or {} not set",
                                ENV_NEAR_ACCOUNT_ID_MAINNET, ENV_NEAR_ACCOUNT_ID
                            )
                        })?
                };

                // Parse the private key - supports ed25519:base58 format
                let secret_key = near_crypto::SecretKey::from_str(&private_key_str)
                    .map_err(|e| format!("Failed to parse NEAR private key: {}", e))?;

                Ok((secret_key, account_id))
            }
        }
    }

    /// Retrieves Stellar secret key from environment variables.
    ///
    /// Environment variables:
    /// - `STELLAR_PRIVATE_KEY_MAINNET` — Stellar secret key for mainnet (S... format)
    /// - `STELLAR_PRIVATE_KEY_TESTNET` — Stellar secret key for testnet (S... format)
    /// - `STELLAR_PRIVATE_KEY` — fallback for all networks if network-specific keys are not set
    ///
    /// Returns the raw secret key string. Parsing into the Stellar SDK keypair
    /// is done in the StellarProvider to avoid adding stellar-sdk dependency here.
    pub fn get_stellar_secret_key(
        &self,
        network: Network,
    ) -> Result<String, Box<dyn std::error::Error>> {
        match self {
            SignerType::PrivateKey => {
                let secret_key = if network.is_testnet() {
                    env::var(ENV_STELLAR_PRIVATE_KEY_TESTNET)
                        .or_else(|_| env::var(ENV_STELLAR_PRIVATE_KEY))
                        .map_err(|_| {
                            format!(
                                "env {} or {} not set",
                                ENV_STELLAR_PRIVATE_KEY_TESTNET, ENV_STELLAR_PRIVATE_KEY
                            )
                        })?
                } else {
                    env::var(ENV_STELLAR_PRIVATE_KEY_MAINNET)
                        .or_else(|_| env::var(ENV_STELLAR_PRIVATE_KEY))
                        .map_err(|_| {
                            format!(
                                "env {} or {} not set",
                                ENV_STELLAR_PRIVATE_KEY_MAINNET, ENV_STELLAR_PRIVATE_KEY
                            )
                        })?
                };

                // Basic validation: Stellar secret keys start with 'S'
                if !secret_key.starts_with('S') {
                    return Err(format!(
                        "Invalid Stellar secret key format: must start with 'S'"
                    )
                    .into());
                }

                Ok(secret_key)
            }
        }
    }

    /// Retrieves Algorand mnemonic from environment variables.
    ///
    /// Environment variables:
    /// - `ALGORAND_MNEMONIC_MAINNET` — 25-word Algorand mnemonic for mainnet
    /// - `ALGORAND_MNEMONIC_TESTNET` — 25-word Algorand mnemonic for testnet
    /// - `ALGORAND_MNEMONIC` — fallback for all networks if network-specific keys are not set
    ///
    /// Returns the mnemonic string. Parsing into the Algorand account
    /// is done in the AlgorandProvider to avoid adding algonaut dependency here.
    pub fn get_algorand_mnemonic(
        &self,
        network: Network,
    ) -> Result<String, Box<dyn std::error::Error>> {
        match self {
            SignerType::PrivateKey => {
                let mnemonic = if network.is_testnet() {
                    env::var(ENV_ALGORAND_MNEMONIC_TESTNET)
                        .or_else(|_| env::var(ENV_ALGORAND_MNEMONIC))
                        .map_err(|_| {
                            format!(
                                "env {} or {} not set",
                                ENV_ALGORAND_MNEMONIC_TESTNET, ENV_ALGORAND_MNEMONIC
                            )
                        })?
                } else {
                    env::var(ENV_ALGORAND_MNEMONIC_MAINNET)
                        .or_else(|_| env::var(ENV_ALGORAND_MNEMONIC))
                        .map_err(|_| {
                            format!(
                                "env {} or {} not set",
                                ENV_ALGORAND_MNEMONIC_MAINNET, ENV_ALGORAND_MNEMONIC
                            )
                        })?
                };

                // Basic validation: Algorand mnemonics are 25 words
                let word_count = mnemonic.split_whitespace().count();
                if word_count != 25 {
                    return Err(format!(
                        "Invalid Algorand mnemonic: expected 25 words, got {}",
                        word_count
                    )
                    .into());
                }

                Ok(mnemonic)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::network::{Ethereum as AlloyEthereum, NetworkWallet};
    use alloy::signers::local::PrivateKeySigner;
    use std::str::FromStr;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvOverride {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvOverride {
        fn new(key: &'static str) -> Self {
            Self {
                key,
                original: env::var(key).ok(),
            }
        }

        fn set(&self, value: &str) {
            unsafe { env::set_var(self.key, value) };
        }
    }

    impl Drop for EnvOverride {
        fn drop(&mut self) {
            match &self.original {
                Some(value) => unsafe { env::set_var(self.key, value) },
                None => unsafe { env::remove_var(self.key) },
            }
        }
    }

    #[test]
    fn make_evm_wallet_supports_multiple_private_keys() {
        let _guard = ENV_LOCK.lock().expect("env lock poisoned");
        let signer_type_override = EnvOverride::new(ENV_SIGNER_TYPE);
        let evm_keys_override = EnvOverride::new(ENV_EVM_PRIVATE_KEY);

        const KEY_1: &str = "0xcafe000000000000000000000000000000000000000000000000000000000001";
        const KEY_2: &str = "0xcafe000000000000000000000000000000000000000000000000000000000002";

        signer_type_override.set("private-key");
        evm_keys_override.set(&format!("{KEY_1},{KEY_2}"));

        let signer_type = SignerType::from_env().expect("SIGNER_TYPE");
        let wallet = signer_type
            .make_evm_wallet(Network::Base) // Use any mainnet for testing
            .expect("wallet constructed from env");

        let expected_primary = PrivateKeySigner::from_str(KEY_1)
            .expect("key1 parses")
            .address();
        let expected_secondary = PrivateKeySigner::from_str(KEY_2)
            .expect("key2 parses")
            .address();

        assert_eq!(
            NetworkWallet::<AlloyEthereum>::default_signer_address(&wallet),
            expected_primary
        );

        let signers: Vec<_> = NetworkWallet::<AlloyEthereum>::signer_addresses(&wallet).collect();
        assert_eq!(signers.len(), 2);
        assert!(signers.contains(&expected_primary));
        assert!(signers.contains(&expected_secondary));
    }
}
