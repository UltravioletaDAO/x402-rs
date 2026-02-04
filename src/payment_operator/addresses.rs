//! Contract addresses for x402r Escrow Scheme by network
//!
//! These addresses are from the x402r-sdk:
//! https://github.com/BackTrackCo/x402r-sdk/blob/main/packages/core/src/config/index.ts

use alloy::primitives::{address, Address};

use crate::network::Network;

/// Contract addresses for Base Sepolia testnet (eip155:84532)
pub mod base_sepolia {
    use super::*;

    /// AuthCaptureEscrow - Core escrow contract
    pub const ESCROW: Address = address!("b9488351E48b23D798f24e8174514F28B741Eb4f");

    /// PaymentOperatorFactory - Deploys PaymentOperator instances
    pub const FACTORY: Address = address!("Fa8C4Cb156053b867Ae7489220A29b5939E3Df70");

    /// TokenCollector - Receives tokens via ERC-3009 transferWithAuthorization
    pub const TOKEN_COLLECTOR: Address = address!("C80cd08d609673061597DE7fe54Af3978f10A825");

    /// ProtocolFeeConfig - Shared protocol fee configuration
    pub const PROTOCOL_FEE_CONFIG: Address = address!("1e52a74cE6b69F04a506eF815743E1052A1BD28F");

    /// RefundRequest - Contract for managing refund requests
    pub const REFUND_REQUEST: Address = address!("6926c05193c714ED4bA3867Ee93d6816Fdc14128");

    /// UsdcTvlLimit - TVL limit contract for USDC
    pub const USDC_TVL_LIMIT: Address = address!("cb9F7C34C6DecFB010385e1454ae1BF3182D78E7");

    /// USDC token on Base Sepolia
    pub const USDC: Address = address!("036CbD53842c5426634e7929541eC2318f3dCF7e");

    // Condition Singletons
    /// PayerCondition - Allows only payer to call
    pub const PAYER_CONDITION: Address = address!("BAF68176FF94CAdD403EF7FbB776bbca548AC09D");

    /// ReceiverCondition - Allows only receiver to call
    pub const RECEIVER_CONDITION: Address = address!("12EDefd4549c53497689067f165c0f101796Eb6D");

    /// AlwaysTrueCondition - Always allows (for unrestricted actions)
    pub const ALWAYS_TRUE_CONDITION: Address = address!("785cC83DEa3d46D5509f3bf7496EAb26D42EE610");
}

/// Contract addresses for Base mainnet (eip155:8453)
pub mod base_mainnet {
    use super::*;

    /// AuthCaptureEscrow - Core escrow contract
    pub const ESCROW: Address = address!("320a3c35F131E5D2Fb36af56345726B298936037");

    /// PaymentOperatorFactory - Deploys PaymentOperator instances
    pub const FACTORY: Address = address!("D979dBfBdA5f4b16AAF60Eaab32A44f352076838");

    /// Our deployed PaymentOperator (permissionless, anyone can call authorize)
    /// Deployed via factory at TX: 0x65a022e67576682f94dad9d9ec82d8c58cccc16fd22c405b8545a7247c5efa60
    /// Config: feeRecipient=0xD3868E1eD738CED6945A574a7c769433BeD5d474, all conditions=ZERO
    pub const PAYMENT_OPERATOR: Address = address!("a06958D93135BEd7e43893897C0d9fA931EF051C");

    /// TokenCollector - Receives tokens via ERC-3009 transferWithAuthorization
    pub const TOKEN_COLLECTOR: Address = address!("32d6AC59BCe8DFB3026F10BcaDB8D00AB218f5b6");

    /// ProtocolFeeConfig - Shared protocol fee configuration
    pub const PROTOCOL_FEE_CONFIG: Address = address!("230fd3A171750FA45db2976121376b7F47Cba308");

    /// RefundRequest - Contract for managing refund requests
    pub const REFUND_REQUEST: Address = address!("c1256Bb30bd0cdDa07D8C8Cf67a59105f2EA1b98");

    /// UsdcTvlLimit - TVL limit contract for USDC
    pub const USDC_TVL_LIMIT: Address = address!("E78648e7af7B1BaDE717FF6E410B922F92adE80f");

    /// USDC token on Base mainnet
    pub const USDC: Address = address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");

    // Condition Singletons
    /// PayerCondition - Allows only payer to call
    pub const PAYER_CONDITION: Address = address!("b33D6502EdBbC47201cd1E53C49d703EC0a660b8");

    /// ReceiverCondition - Allows only receiver to call
    pub const RECEIVER_CONDITION: Address = address!("ed02d3E5167BCc9582D851885A89b050AB816a56");

    /// AlwaysTrueCondition - Always allows (for unrestricted actions)
    pub const ALWAYS_TRUE_CONDITION: Address = address!("c9BbA6A2CF9838e7Dd8c19BC8B3BAC620B9D8178");
}

/// Get escrow address for a given network
pub fn escrow_for_network(network: Network) -> Option<Address> {
    match network {
        Network::BaseSepolia => Some(base_sepolia::ESCROW),
        Network::Base => Some(base_mainnet::ESCROW),
        _ => None,
    }
}

/// Get factory address for a given network
pub fn factory_for_network(network: Network) -> Option<Address> {
    match network {
        Network::BaseSepolia => Some(base_sepolia::FACTORY),
        Network::Base => Some(base_mainnet::FACTORY),
        _ => None,
    }
}

/// Get token collector address for a given network
pub fn token_collector_for_network(network: Network) -> Option<Address> {
    match network {
        Network::BaseSepolia => Some(base_sepolia::TOKEN_COLLECTOR),
        Network::Base => Some(base_mainnet::TOKEN_COLLECTOR),
        _ => None,
    }
}

/// Get protocol fee config address for a given network
pub fn protocol_fee_config_for_network(network: Network) -> Option<Address> {
    match network {
        Network::BaseSepolia => Some(base_sepolia::PROTOCOL_FEE_CONFIG),
        Network::Base => Some(base_mainnet::PROTOCOL_FEE_CONFIG),
        _ => None,
    }
}

/// Get refund request address for a given network
pub fn refund_request_for_network(network: Network) -> Option<Address> {
    match network {
        Network::BaseSepolia => Some(base_sepolia::REFUND_REQUEST),
        Network::Base => Some(base_mainnet::REFUND_REQUEST),
        _ => None,
    }
}

/// Check if a network supports the escrow scheme
pub fn is_supported(network: Network) -> bool {
    escrow_for_network(network).is_some()
}

/// All escrow contract addresses for a network
#[derive(Debug, Clone)]
pub struct OperatorAddresses {
    pub escrow: Address,
    pub factory: Address,
    pub payment_operator: Option<Address>,  // Our deployed permissionless operator
    pub token_collector: Address,
    pub protocol_fee_config: Address,
    pub refund_request: Address,
}

impl OperatorAddresses {
    /// Get addresses for a network
    pub fn for_network(network: Network) -> Option<Self> {
        match network {
            Network::BaseSepolia => Some(Self {
                escrow: base_sepolia::ESCROW,
                factory: base_sepolia::FACTORY,
                payment_operator: None,  // No deployed operator yet for Sepolia
                token_collector: base_sepolia::TOKEN_COLLECTOR,
                protocol_fee_config: base_sepolia::PROTOCOL_FEE_CONFIG,
                refund_request: base_sepolia::REFUND_REQUEST,
            }),
            Network::Base => Some(Self {
                escrow: base_mainnet::ESCROW,
                factory: base_mainnet::FACTORY,
                payment_operator: Some(base_mainnet::PAYMENT_OPERATOR),
                token_collector: base_mainnet::TOKEN_COLLECTOR,
                protocol_fee_config: base_mainnet::PROTOCOL_FEE_CONFIG,
                refund_request: base_mainnet::REFUND_REQUEST,
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escrow_for_network() {
        assert_eq!(
            escrow_for_network(Network::BaseSepolia),
            Some(base_sepolia::ESCROW)
        );
        assert_eq!(
            escrow_for_network(Network::Base),
            Some(base_mainnet::ESCROW)
        );
        assert_eq!(escrow_for_network(Network::Avalanche), None);
    }

    #[test]
    fn test_is_supported() {
        assert!(is_supported(Network::BaseSepolia));
        assert!(is_supported(Network::Base));
        assert!(!is_supported(Network::Avalanche));
    }

    #[test]
    fn test_operator_addresses() {
        // Base Sepolia
        let addrs = OperatorAddresses::for_network(Network::BaseSepolia).unwrap();
        assert_eq!(addrs.escrow, base_sepolia::ESCROW);
        assert_eq!(addrs.factory, base_sepolia::FACTORY);
        assert_eq!(addrs.token_collector, base_sepolia::TOKEN_COLLECTOR);

        // Base Mainnet
        let addrs = OperatorAddresses::for_network(Network::Base).unwrap();
        assert_eq!(addrs.escrow, base_mainnet::ESCROW);
        assert_eq!(addrs.factory, base_mainnet::FACTORY);
        assert_eq!(addrs.token_collector, base_mainnet::TOKEN_COLLECTOR);
    }
}
