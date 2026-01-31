//! Types for PaymentOperator x402r integration
//!
//! These types mirror the Solidity structs in AuthCaptureEscrow and PaymentOperator contracts.

use alloy::primitives::{Address, U256};
use serde::{Deserialize, Serialize};

/// PaymentInfo struct matching AuthCaptureEscrow.PaymentInfo in Solidity
///
/// This contains all information required to authorize and capture a unique payment.
/// The hash of this struct is used to identify payments throughout their lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentInfo {
    /// Entity responsible for driving payment flow (PaymentOperator contract)
    pub operator: Address,

    /// The payer's address authorizing the payment
    pub payer: Address,

    /// Address that receives the payment (minus fees)
    pub receiver: Address,

    /// The token contract address (e.g., USDC)
    pub token: Address,

    /// Maximum amount of tokens that can be authorized (uint120 in Solidity)
    #[serde(with = "string_u128")]
    pub max_amount: u128,

    /// Timestamp when the payer's pre-approval can no longer authorize payment (uint48)
    #[serde(with = "string_u64")]
    pub pre_approval_expiry: u64,

    /// Timestamp when an authorization can no longer be captured (uint48)
    /// After this, payer can reclaim from escrow
    #[serde(with = "string_u64")]
    pub authorization_expiry: u64,

    /// Timestamp when a successful payment can no longer be refunded (uint48)
    #[serde(with = "string_u64")]
    pub refund_expiry: u64,

    /// Minimum fee percentage in basis points (uint16)
    pub min_fee_bps: u16,

    /// Maximum fee percentage in basis points (uint16)
    pub max_fee_bps: u16,

    /// Address that receives the fee portion of payments
    /// If address(0), operator can set at capture time
    pub fee_receiver: Address,

    /// Source of entropy to ensure unique hashes across different payments
    #[serde(with = "u256_string")]
    pub salt: U256,
}

impl PaymentInfo {
    /// Convert to Alloy struct for contract calls
    pub fn to_contract_type(&self) -> super::abi::PaymentInfo {
        use alloy::primitives::Uint;

        super::abi::PaymentInfo {
            operator: self.operator,
            payer: self.payer,
            receiver: self.receiver,
            token: self.token,
            maxAmount: Uint::from(self.max_amount),
            preApprovalExpiry: Uint::from(self.pre_approval_expiry),
            authorizationExpiry: Uint::from(self.authorization_expiry),
            refundExpiry: Uint::from(self.refund_expiry),
            minFeeBps: self.min_fee_bps,
            maxFeeBps: self.max_fee_bps,
            feeReceiver: self.fee_receiver,
            salt: self.salt,
        }
    }
}

/// Operator action to execute
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OperatorAction {
    /// Authorize payment - places funds in escrow
    Authorize,
    /// Charge payment - immediate collection and transfer to receiver
    Charge,
    /// Release funds - captures authorized funds and transfers to receiver
    Release,
    /// Refund while in escrow - returns funds to payer before capture
    RefundInEscrow,
    /// Refund after escrow - returns captured funds to payer
    RefundPostEscrow,
}

impl OperatorAction {
    /// Parse action from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "authorize" => Some(Self::Authorize),
            "charge" => Some(Self::Charge),
            "release" => Some(Self::Release),
            "refundinescrow" | "refund_in_escrow" => Some(Self::RefundInEscrow),
            "refundpostescrow" | "refund_post_escrow" => Some(Self::RefundPostEscrow),
            _ => None,
        }
    }
}

/// Operator extension data in payment payload
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorExtension {
    /// The action to execute
    pub action: OperatorAction,

    /// Payment information struct
    pub payment_info: PaymentInfo,

    /// Amount for this operation (in token decimals)
    #[serde(with = "string_u128")]
    pub amount: u128,

    /// Token collector address (for authorize/charge/refundPostEscrow)
    #[serde(default)]
    pub token_collector: Option<Address>,

    /// Data to pass to the token collector
    #[serde(default)]
    pub collector_data: Option<String>,
}

/// Payment state from escrow contract
#[derive(Debug, Clone, Default)]
pub struct EscrowState {
    /// True if payment has been authorized or charged
    pub has_collected_payment: bool,
    /// Amount currently on hold in escrow that can be captured
    pub capturable_amount: u128,
    /// Amount previously captured that can be refunded
    pub refundable_amount: u128,
}

/// Fee calculation result
#[derive(Debug, Clone)]
pub struct FeeCalculation {
    /// Total fee in basis points (protocol + operator)
    pub total_fee_bps: u16,
    /// Protocol fee portion in basis points
    pub protocol_fee_bps: u16,
    /// Operator fee portion in basis points
    pub operator_fee_bps: u16,
}

// ============================================================================
// Serde helpers for string <-> integer conversion
// ============================================================================

mod string_u128 {
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &u128, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u128, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse::<u128>().map_err(serde::de::Error::custom)
    }
}

mod string_u64 {
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &u64, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u64, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse::<u64>().map_err(serde::de::Error::custom)
    }
}

mod u256_string {
    use alloy::primitives::U256;
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &U256, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<U256, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        U256::from_str_radix(&s, 10).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    #[test]
    fn test_payment_info_serialization() {
        let info = PaymentInfo {
            operator: address!("Fa8C4Cb156053b867Ae7489220A29b5939E3Df70"),
            payer: address!("1111111111111111111111111111111111111111"),
            receiver: address!("2222222222222222222222222222222222222222"),
            token: address!("036CbD53842c5426634e7929541eC2318f3dCF7e"),
            max_amount: 1_000_000,
            pre_approval_expiry: 1738400000,
            authorization_expiry: 1738500000,
            refund_expiry: 1738600000,
            min_fee_bps: 0,
            max_fee_bps: 100,
            fee_receiver: address!("Fa8C4Cb156053b867Ae7489220A29b5939E3Df70"),
            salt: U256::from(12345),
        };

        let json = serde_json::to_string(&info).unwrap();
        let parsed: PaymentInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(info.operator, parsed.operator);
        assert_eq!(info.max_amount, parsed.max_amount);
        assert_eq!(info.salt, parsed.salt);
    }

    #[test]
    fn test_operator_action_parsing() {
        assert_eq!(OperatorAction::from_str("authorize"), Some(OperatorAction::Authorize));
        assert_eq!(OperatorAction::from_str("CHARGE"), Some(OperatorAction::Charge));
        assert_eq!(OperatorAction::from_str("Release"), Some(OperatorAction::Release));
        assert_eq!(OperatorAction::from_str("refundInEscrow"), Some(OperatorAction::RefundInEscrow));
        assert_eq!(OperatorAction::from_str("refund_post_escrow"), Some(OperatorAction::RefundPostEscrow));
        assert_eq!(OperatorAction::from_str("invalid"), None);
    }
}
