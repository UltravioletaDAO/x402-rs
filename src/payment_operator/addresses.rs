//! Contract addresses for PaymentOperator system by network
//!
//! These addresses are for the deployed x402r PaymentOperator contracts.
//! Currently only Base Sepolia is deployed.

use alloy::primitives::{address, Address};

use crate::network::Network;

/// Contract addresses for Base Sepolia testnet
pub mod base_sepolia {
    use super::*;

    /// AuthCaptureEscrow - Core escrow contract
    pub const ESCROW: Address = address!("b9488351E48b23D798f24e8174514F28B741Eb4f");

    /// PaymentOperatorFactory - Deploys PaymentOperator instances
    pub const FACTORY: Address = address!("Fa8C4Cb156053b867Ae7489220A29b5939E3Df70");

    /// ProtocolFeeConfig - Shared protocol fee configuration
    pub const PROTOCOL_FEE_CONFIG: Address = address!("1e52a74cE6b69F04a506eF815743E1052A1BD28F");

    /// RefundRequest - Contract for managing refund requests
    pub const REFUND_REQUEST: Address = address!("6926c05193c714ED4bA3867Ee93d6816Fdc14128");

    /// PayerCondition - Allows only payer to call
    pub const PAYER_CONDITION: Address = address!("BAF68176FF94CAdD403EF7FbB776bbca548AC09D");

    /// ReceiverCondition - Allows only receiver to call
    pub const RECEIVER_CONDITION: Address = address!("12EDefd4549c53497689067f165c0f101796Eb6D");

    /// AlwaysTrueCondition - Always allows (for unrestricted actions)
    pub const ALWAYS_TRUE_CONDITION: Address = address!("785cC83DEa3d46D5509f3bf7496EAb26D42EE610");

    /// USDC token on Base Sepolia
    pub const USDC: Address = address!("036CbD53842c5426634e7929541eC2318f3dCF7e");

    /// ERC3009PaymentCollector - For collecting EIP-3009 authorized payments
    pub const ERC3009_COLLECTOR: Address = address!("0E3dF9510de65469C4518D7843919c0b8C7A7757");
}

// Placeholder for future mainnet deployment
// pub mod base_mainnet {
//     use super::*;
//     // Addresses TBD after mainnet deployment
// }

/// Get escrow address for a given network
pub fn escrow_for_network(network: Network) -> Option<Address> {
    match network {
        Network::BaseSepolia => Some(base_sepolia::ESCROW),
        // Network::Base => Some(base_mainnet::ESCROW),
        _ => None,
    }
}

/// Get factory address for a given network
pub fn factory_for_network(network: Network) -> Option<Address> {
    match network {
        Network::BaseSepolia => Some(base_sepolia::FACTORY),
        // Network::Base => Some(base_mainnet::FACTORY),
        _ => None,
    }
}

/// Get protocol fee config address for a given network
pub fn protocol_fee_config_for_network(network: Network) -> Option<Address> {
    match network {
        Network::BaseSepolia => Some(base_sepolia::PROTOCOL_FEE_CONFIG),
        // Network::Base => Some(base_mainnet::PROTOCOL_FEE_CONFIG),
        _ => None,
    }
}

/// Get refund request address for a given network
pub fn refund_request_for_network(network: Network) -> Option<Address> {
    match network {
        Network::BaseSepolia => Some(base_sepolia::REFUND_REQUEST),
        // Network::Base => Some(base_mainnet::REFUND_REQUEST),
        _ => None,
    }
}

/// Get ERC-3009 payment collector address for a given network
pub fn erc3009_collector_for_network(network: Network) -> Option<Address> {
    match network {
        Network::BaseSepolia => Some(base_sepolia::ERC3009_COLLECTOR),
        // Network::Base => Some(base_mainnet::ERC3009_COLLECTOR),
        _ => None,
    }
}

/// Check if a network supports PaymentOperator
pub fn is_supported(network: Network) -> bool {
    escrow_for_network(network).is_some()
}

/// Get all PaymentOperator contract addresses for a network
#[derive(Debug, Clone)]
pub struct OperatorAddresses {
    pub escrow: Address,
    pub factory: Address,
    pub protocol_fee_config: Address,
    pub refund_request: Address,
    pub erc3009_collector: Address,
}

impl OperatorAddresses {
    /// Get addresses for a network
    pub fn for_network(network: Network) -> Option<Self> {
        match network {
            Network::BaseSepolia => Some(Self {
                escrow: base_sepolia::ESCROW,
                factory: base_sepolia::FACTORY,
                protocol_fee_config: base_sepolia::PROTOCOL_FEE_CONFIG,
                refund_request: base_sepolia::REFUND_REQUEST,
                erc3009_collector: base_sepolia::ERC3009_COLLECTOR,
            }),
            // Network::Base => Some(Self { ... }),
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
        assert_eq!(escrow_for_network(Network::Avalanche), None);
    }

    #[test]
    fn test_is_supported() {
        assert!(is_supported(Network::BaseSepolia));
        assert!(!is_supported(Network::Base));
        assert!(!is_supported(Network::Avalanche));
    }

    #[test]
    fn test_operator_addresses() {
        let addrs = OperatorAddresses::for_network(Network::BaseSepolia).unwrap();
        assert_eq!(addrs.escrow, base_sepolia::ESCROW);
        assert_eq!(addrs.factory, base_sepolia::FACTORY);
    }
}
