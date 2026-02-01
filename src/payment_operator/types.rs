//! Types for x402r Escrow Scheme integration
//!
//! These types match the reference implementation at:
//! https://github.com/BackTrackCo/x402r-scheme/tree/main/packages/evm/src
//!
//! The escrow scheme uses ERC-3009 TransferWithAuthorization to move funds
//! into escrow, where they can later be captured or refunded.

use alloy::primitives::{Address, Bytes, FixedBytes, U256};
use serde::{Deserialize, Serialize};

// ============================================================================
// EscrowPayload - The payload field in PaymentPayload when scheme="escrow"
// ============================================================================

/// ERC-3009 TransferWithAuthorization data
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EscrowAuthorization {
    /// The payer's address (signer of the authorization)
    pub from: Address,

    /// The token collector address (receives tokens to put in escrow)
    pub to: Address,

    /// Amount to transfer (in token decimals, as string)
    #[serde(with = "string_u128")]
    pub value: u128,

    /// Unix timestamp after which the authorization is valid
    #[serde(with = "string_u64")]
    pub valid_after: u64,

    /// Unix timestamp before which the authorization is valid
    #[serde(with = "string_u64")]
    pub valid_before: u64,

    /// Nonce for replay protection (computed from paymentInfo hash)
    pub nonce: FixedBytes<32>,
}

/// Payment information for the escrow contract
///
/// Note: The `payer` field is set from `authorization.from` during processing,
/// not from the payload directly (paymentInfo in payload doesn't include payer).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EscrowPaymentInfo {
    /// PaymentOperator contract address
    pub operator: Address,

    /// Address that receives the payment (minus fees)
    pub receiver: Address,

    /// The token contract address (e.g., USDC)
    pub token: Address,

    /// Maximum amount of tokens that can be authorized
    #[serde(with = "string_u128")]
    pub max_amount: u128,

    /// Timestamp when pre-approval expires (uint48)
    pub pre_approval_expiry: u64,

    /// Timestamp when authorization expires (uint48)
    pub authorization_expiry: u64,

    /// Timestamp when refund period expires (uint48)
    pub refund_expiry: u64,

    /// Minimum fee in basis points
    pub min_fee_bps: u16,

    /// Maximum fee in basis points
    pub max_fee_bps: u16,

    /// Address that receives fees
    pub fee_receiver: Address,

    /// Random salt for unique payment identification
    pub salt: FixedBytes<32>,
}

/// The complete escrow payload - sent in PaymentPayload.payload when scheme="escrow"
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EscrowPayload {
    /// ERC-3009 authorization data
    pub authorization: EscrowAuthorization,

    /// ERC-3009 signature (65 bytes, hex-encoded)
    pub signature: Bytes,

    /// Payment information for the escrow contract
    pub payment_info: EscrowPaymentInfo,
}

// ============================================================================
// EscrowExtra - Configuration in PaymentRequirements.extra
// ============================================================================

/// Extra configuration provided in PaymentRequirements for escrow scheme
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EscrowExtra {
    /// AuthCaptureEscrow contract address
    pub escrow_address: Address,

    /// PaymentOperator contract address
    pub operator_address: Address,

    /// ERC3009TokenCollector address (receives tokens via transferWithAuthorization)
    pub token_collector: Address,

    /// Optional override for authorize target (defaults to operator_address)
    #[serde(default)]
    pub authorize_address: Option<Address>,

    /// Minimum deposit amount (optional)
    #[serde(default)]
    pub min_deposit: Option<String>,

    /// Maximum deposit amount (optional)
    #[serde(default)]
    pub max_deposit: Option<String>,

    /// Pre-approval expiry in seconds (optional)
    #[serde(default)]
    pub pre_approval_expiry_seconds: Option<u64>,

    /// Authorization expiry in seconds (optional)
    #[serde(default)]
    pub authorization_expiry_seconds: Option<u64>,

    /// Refund expiry in seconds (optional)
    #[serde(default)]
    pub refund_expiry_seconds: Option<u64>,

    /// Minimum fee in basis points (optional)
    #[serde(default)]
    pub min_fee_bps: Option<u16>,

    /// Maximum fee in basis points (optional)
    #[serde(default)]
    pub max_fee_bps: Option<u16>,

    /// Fee receiver address (optional)
    #[serde(default)]
    pub fee_receiver: Option<Address>,

    /// EIP-712 domain name for the token (optional, defaults to "USD Coin")
    #[serde(default)]
    pub name: Option<String>,

    /// EIP-712 domain version for the token (optional, defaults to "2")
    #[serde(default)]
    pub version: Option<String>,
}

// ============================================================================
// Contract types - For building the authorize() call
// ============================================================================

/// PaymentInfo struct for contract calls (includes payer from authorization)
///
/// This matches AuthCaptureEscrow.PaymentInfo in Solidity.
#[derive(Debug, Clone)]
pub struct ContractPaymentInfo {
    pub operator: Address,
    pub payer: Address,
    pub receiver: Address,
    pub token: Address,
    pub max_amount: u128,
    pub pre_approval_expiry: u64,
    pub authorization_expiry: u64,
    pub refund_expiry: u64,
    pub min_fee_bps: u16,
    pub max_fee_bps: u16,
    pub fee_receiver: Address,
    pub salt: U256,
}

impl ContractPaymentInfo {
    /// Create from EscrowPayload (combines paymentInfo + authorization.from as payer)
    pub fn from_escrow_payload(payload: &EscrowPayload) -> Self {
        Self {
            operator: payload.payment_info.operator,
            payer: payload.authorization.from, // payer comes from authorization
            receiver: payload.payment_info.receiver,
            token: payload.payment_info.token,
            max_amount: payload.payment_info.max_amount,
            pre_approval_expiry: payload.payment_info.pre_approval_expiry,
            authorization_expiry: payload.payment_info.authorization_expiry,
            refund_expiry: payload.payment_info.refund_expiry,
            min_fee_bps: payload.payment_info.min_fee_bps,
            max_fee_bps: payload.payment_info.max_fee_bps,
            fee_receiver: payload.payment_info.fee_receiver,
            salt: U256::from_be_bytes(payload.payment_info.salt.0),
        }
    }

    /// Convert to Alloy ABI struct for contract calls
    pub fn to_abi_type(&self) -> super::abi::PaymentInfo {
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, fixed_bytes};

    #[test]
    fn test_escrow_payload_deserialization() {
        let json = r#"{
            "authorization": {
                "from": "0x1111111111111111111111111111111111111111",
                "to": "0x2222222222222222222222222222222222222222",
                "value": "1000000",
                "validAfter": "0",
                "validBefore": "1738500000",
                "nonce": "0x0000000000000000000000000000000000000000000000000000000000003039"
            },
            "signature": "0xabcdef",
            "paymentInfo": {
                "operator": "0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70",
                "receiver": "0x3333333333333333333333333333333333333333",
                "token": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
                "maxAmount": "1000000",
                "preApprovalExpiry": 281474976710655,
                "authorizationExpiry": 281474976710655,
                "refundExpiry": 281474976710655,
                "minFeeBps": 0,
                "maxFeeBps": 100,
                "feeReceiver": "0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70",
                "salt": "0x0000000000000000000000000000000000000000000000000000000000003039"
            }
        }"#;

        let payload: EscrowPayload = serde_json::from_str(json).unwrap();

        assert_eq!(
            payload.authorization.from,
            address!("1111111111111111111111111111111111111111")
        );
        assert_eq!(payload.authorization.value, 1_000_000);
        assert_eq!(payload.authorization.valid_before, 1738500000);
        assert_eq!(
            payload.payment_info.operator,
            address!("Fa8C4Cb156053b867Ae7489220A29b5939E3Df70")
        );
        assert_eq!(payload.payment_info.max_amount, 1_000_000);
    }

    #[test]
    fn test_contract_payment_info_from_payload() {
        let payload = EscrowPayload {
            authorization: EscrowAuthorization {
                from: address!("1111111111111111111111111111111111111111"),
                to: address!("2222222222222222222222222222222222222222"),
                value: 1_000_000,
                valid_after: 0,
                valid_before: 1738500000,
                nonce: fixed_bytes!("0000000000000000000000000000000000000000000000000000000000003039"),
            },
            signature: Bytes::from(vec![0xab, 0xcd, 0xef]),
            payment_info: EscrowPaymentInfo {
                operator: address!("Fa8C4Cb156053b867Ae7489220A29b5939E3Df70"),
                receiver: address!("3333333333333333333333333333333333333333"),
                token: address!("036CbD53842c5426634e7929541eC2318f3dCF7e"),
                max_amount: 1_000_000,
                pre_approval_expiry: 281474976710655,
                authorization_expiry: 281474976710655,
                refund_expiry: 281474976710655,
                min_fee_bps: 0,
                max_fee_bps: 100,
                fee_receiver: address!("Fa8C4Cb156053b867Ae7489220A29b5939E3Df70"),
                salt: fixed_bytes!("0000000000000000000000000000000000000000000000000000000000003039"),
            },
        };

        let contract_info = ContractPaymentInfo::from_escrow_payload(&payload);

        // Payer should come from authorization.from
        assert_eq!(
            contract_info.payer,
            address!("1111111111111111111111111111111111111111")
        );
        assert_eq!(contract_info.operator, payload.payment_info.operator);
        assert_eq!(contract_info.receiver, payload.payment_info.receiver);
    }

    #[test]
    fn test_escrow_extra_deserialization() {
        let json = r#"{
            "escrowAddress": "0xb9488351E48b23D798f24e8174514F28B741Eb4f",
            "operatorAddress": "0xFa8C4Cb156053b867Ae7489220A29b5939E3Df70",
            "tokenCollector": "0x0E3dF9510de65469C4518D7843919c0b8C7A7757",
            "name": "USD Coin",
            "version": "2"
        }"#;

        let extra: EscrowExtra = serde_json::from_str(json).unwrap();

        assert_eq!(
            extra.escrow_address,
            address!("b9488351E48b23D798f24e8174514F28B741Eb4f")
        );
        assert_eq!(extra.name, Some("USD Coin".to_string()));
        assert_eq!(extra.version, Some("2".to_string()));
    }
}
