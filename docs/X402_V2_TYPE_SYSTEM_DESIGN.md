# x402 Protocol v2: Rust Type System Design

**Document Version:** 1.0
**Date:** 2025-12-11
**Author:** Aegis (Rust Systems Architect)
**Target Facilitator Version:** v2.0.0

---

## Executive Summary

This document defines the complete Rust type system for migrating x402-rs to protocol v2. The design focuses on:

1. **Zero-cost abstractions** for CAIP-2 network identifiers
2. **Type-safe dual v1/v2 support** during transition period
3. **Extensibility** for custom chains (NEAR, Stellar, Fogo)
4. **Backward compatibility** with existing v1 types
5. **Compile-time safety** for network/address validation

---

## 1. CAIP-2 Network Identifiers

### 1.1 Core CAIP-2 Type

```rust
/// CAIP-2 compliant network identifier: `{namespace}:{reference}`
///
/// Examples:
/// - `eip155:8453` (Base mainnet)
/// - `solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp` (Solana mainnet)
/// - `near:mainnet` (NEAR mainnet)
/// - `stellar:pubnet` (Stellar mainnet)
///
/// # Format
/// - `namespace`: Blockchain ecosystem identifier (e.g., "eip155", "solana")
/// - `reference`: Chain-specific identifier (e.g., chain ID, genesis hash)
///
/// # References
/// - CAIP-2: https://github.com/ChainAgnostic/CAIPs/blob/main/CAIPs/caip-2.md
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Caip2NetworkId {
    /// Namespace (e.g., "eip155", "solana", "near", "stellar")
    namespace: Namespace,
    /// Reference (e.g., "8453", "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp")
    reference: String,
}

/// Supported CAIP-2 namespaces
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Namespace {
    /// EIP-155 Ethereum-compatible chains (uses chain ID)
    Eip155,
    /// Solana ecosystem (uses genesis hash)
    Solana,
    /// NEAR Protocol (uses network name: "mainnet", "testnet")
    Near,
    /// Stellar network (uses network name: "pubnet", "testnet")
    Stellar,
    /// Fogo blockchain (custom namespace, not standardized)
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

impl Caip2NetworkId {
    /// Construct a CAIP-2 network ID from namespace and reference
    pub fn new(namespace: Namespace, reference: String) -> Result<Self, Caip2ParseError> {
        // Validate reference format based on namespace
        match namespace {
            Namespace::Eip155 => {
                // Must be a valid chain ID (positive integer)
                reference
                    .parse::<u64>()
                    .map_err(|_| Caip2ParseError::InvalidEvmChainId(reference.clone()))?;
            }
            Namespace::Solana => {
                // Must be a valid base58-encoded genesis hash (32 bytes)
                if reference.len() < 32 || reference.len() > 44 {
                    return Err(Caip2ParseError::InvalidSolanaGenesisHash(reference.clone()));
                }
            }
            Namespace::Near => {
                // Must be "mainnet" or "testnet"
                if reference != "mainnet" && reference != "testnet" {
                    return Err(Caip2ParseError::InvalidNearNetwork(reference.clone()));
                }
            }
            Namespace::Stellar => {
                // Must be "pubnet" or "testnet"
                if reference != "pubnet" && reference != "testnet" {
                    return Err(Caip2ParseError::InvalidStellarNetwork(reference.clone()));
                }
            }
            Namespace::Fogo => {
                // Custom validation: "mainnet" or "testnet"
                if reference != "mainnet" && reference != "testnet" {
                    return Err(Caip2ParseError::InvalidFogoNetwork(reference.clone()));
                }
            }
        }

        Ok(Self {
            namespace,
            reference,
        })
    }

    /// Parse from CAIP-2 string format: "namespace:reference"
    pub fn parse(s: &str) -> Result<Self, Caip2ParseError> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 {
            return Err(Caip2ParseError::InvalidFormat(s.to_string()));
        }

        let namespace = Namespace::from_str(parts[0])?;
        let reference = parts[1].to_string();

        Self::new(namespace, reference)
    }

    /// Convert to canonical CAIP-2 string: "namespace:reference"
    pub fn to_string(&self) -> String {
        format!("{}:{}", self.namespace, self.reference)
    }

    /// Get the namespace
    pub fn namespace(&self) -> Namespace {
        self.namespace
    }

    /// Get the reference (e.g., chain ID, genesis hash)
    pub fn reference(&self) -> &str {
        &self.reference
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
        Self::parse(s)
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
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Caip2ParseError {
    #[error("Invalid CAIP-2 format (expected 'namespace:reference'): {0}")]
    InvalidFormat(String),
    #[error("Unknown namespace: {0}")]
    UnknownNamespace(String),
    #[error("Invalid EVM chain ID: {0}")]
    InvalidEvmChainId(String),
    #[error("Invalid Solana genesis hash: {0}")]
    InvalidSolanaGenesisHash(String),
    #[error("Invalid NEAR network (expected 'mainnet' or 'testnet'): {0}")]
    InvalidNearNetwork(String),
    #[error("Invalid Stellar network (expected 'pubnet' or 'testnet'): {0}")]
    InvalidStellarNetwork(String),
    #[error("Invalid Fogo network (expected 'mainnet' or 'testnet'): {0}")]
    InvalidFogoNetwork(String),
}
```

### 1.2 Network Enum ↔ CAIP-2 Bidirectional Mapping

```rust
impl Network {
    /// Convert v1 Network enum to CAIP-2 format for v2 compatibility
    ///
    /// # Examples
    /// ```
    /// use x402_rs::network::Network;
    ///
    /// assert_eq!(Network::Base.to_caip2().to_string(), "eip155:8453");
    /// assert_eq!(Network::BaseSepolia.to_caip2().to_string(), "eip155:84532");
    /// assert_eq!(Network::Solana.to_caip2().to_string(), "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp");
    /// assert_eq!(Network::Near.to_caip2().to_string(), "near:mainnet");
    /// assert_eq!(Network::Stellar.to_caip2().to_string(), "stellar:pubnet");
    /// ```
    pub fn to_caip2(&self) -> Caip2NetworkId {
        match self {
            // EVM chains - eip155:{chainId}
            Network::BaseSepolia => Caip2NetworkId::new(Namespace::Eip155, "84532".to_string()).unwrap(),
            Network::Base => Caip2NetworkId::new(Namespace::Eip155, "8453".to_string()).unwrap(),
            Network::XdcMainnet => Caip2NetworkId::new(Namespace::Eip155, "50".to_string()).unwrap(),
            Network::AvalancheFuji => Caip2NetworkId::new(Namespace::Eip155, "43113".to_string()).unwrap(),
            Network::Avalanche => Caip2NetworkId::new(Namespace::Eip155, "43114".to_string()).unwrap(),
            Network::XrplEvm => Caip2NetworkId::new(Namespace::Eip155, "1440000".to_string()).unwrap(),
            Network::PolygonAmoy => Caip2NetworkId::new(Namespace::Eip155, "80002".to_string()).unwrap(),
            Network::Polygon => Caip2NetworkId::new(Namespace::Eip155, "137".to_string()).unwrap(),
            Network::Optimism => Caip2NetworkId::new(Namespace::Eip155, "10".to_string()).unwrap(),
            Network::OptimismSepolia => Caip2NetworkId::new(Namespace::Eip155, "11155420".to_string()).unwrap(),
            Network::Celo => Caip2NetworkId::new(Namespace::Eip155, "42220".to_string()).unwrap(),
            Network::CeloSepolia => Caip2NetworkId::new(Namespace::Eip155, "44787".to_string()).unwrap(),
            Network::HyperEvm => Caip2NetworkId::new(Namespace::Eip155, "999".to_string()).unwrap(),
            Network::HyperEvmTestnet => Caip2NetworkId::new(Namespace::Eip155, "333".to_string()).unwrap(),
            Network::Sei => Caip2NetworkId::new(Namespace::Eip155, "1329".to_string()).unwrap(),
            Network::SeiTestnet => Caip2NetworkId::new(Namespace::Eip155, "1328".to_string()).unwrap(),
            Network::Ethereum => Caip2NetworkId::new(Namespace::Eip155, "1".to_string()).unwrap(),
            Network::EthereumSepolia => Caip2NetworkId::new(Namespace::Eip155, "11155111".to_string()).unwrap(),
            Network::Arbitrum => Caip2NetworkId::new(Namespace::Eip155, "42161".to_string()).unwrap(),
            Network::ArbitrumSepolia => Caip2NetworkId::new(Namespace::Eip155, "421614".to_string()).unwrap(),
            Network::Unichain => Caip2NetworkId::new(Namespace::Eip155, "130".to_string()).unwrap(),
            Network::UnichainSepolia => Caip2NetworkId::new(Namespace::Eip155, "1301".to_string()).unwrap(),
            Network::Monad => Caip2NetworkId::new(Namespace::Eip155, "143".to_string()).unwrap(),

            // Solana - solana:{genesisHash}
            Network::Solana => Caip2NetworkId::new(
                Namespace::Solana,
                "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string()
            ).unwrap(),
            Network::SolanaDevnet => Caip2NetworkId::new(
                Namespace::Solana,
                "EtWTRABZaYq6iMfeYKouRu166VU2xqa1".to_string()
            ).unwrap(),

            // NEAR - near:{network}
            Network::Near => Caip2NetworkId::new(Namespace::Near, "mainnet".to_string()).unwrap(),
            Network::NearTestnet => Caip2NetworkId::new(Namespace::Near, "testnet".to_string()).unwrap(),

            // Stellar - stellar:{network}
            Network::Stellar => Caip2NetworkId::new(Namespace::Stellar, "pubnet".to_string()).unwrap(),
            Network::StellarTestnet => Caip2NetworkId::new(Namespace::Stellar, "testnet".to_string()).unwrap(),

            // Fogo (SVM) - fogo:{network} (custom namespace, not standardized)
            Network::Fogo => Caip2NetworkId::new(Namespace::Fogo, "mainnet".to_string()).unwrap(),
            Network::FogoTestnet => Caip2NetworkId::new(Namespace::Fogo, "testnet".to_string()).unwrap(),
        }
    }

    /// Parse from CAIP-2 format to Network enum
    ///
    /// # Examples
    /// ```
    /// use x402_rs::network::Network;
    ///
    /// assert_eq!(Network::from_caip2("eip155:8453").unwrap(), Network::Base);
    /// assert_eq!(Network::from_caip2("near:mainnet").unwrap(), Network::Near);
    /// ```
    pub fn from_caip2(caip2: &str) -> Result<Self, NetworkParseError> {
        let id = Caip2NetworkId::parse(caip2)
            .map_err(|e| NetworkParseError::InvalidCaip2(e.to_string()))?;

        match id.namespace() {
            Namespace::Eip155 => {
                let chain_id = id.reference().parse::<u64>()
                    .map_err(|_| NetworkParseError::InvalidChainId(id.reference().to_string()))?;

                match chain_id {
                    84532 => Ok(Network::BaseSepolia),
                    8453 => Ok(Network::Base),
                    50 => Ok(Network::XdcMainnet),
                    43113 => Ok(Network::AvalancheFuji),
                    43114 => Ok(Network::Avalanche),
                    1440000 => Ok(Network::XrplEvm),
                    80002 => Ok(Network::PolygonAmoy),
                    137 => Ok(Network::Polygon),
                    10 => Ok(Network::Optimism),
                    11155420 => Ok(Network::OptimismSepolia),
                    42220 => Ok(Network::Celo),
                    44787 => Ok(Network::CeloSepolia),
                    999 => Ok(Network::HyperEvm),
                    333 => Ok(Network::HyperEvmTestnet),
                    1329 => Ok(Network::Sei),
                    1328 => Ok(Network::SeiTestnet),
                    1 => Ok(Network::Ethereum),
                    11155111 => Ok(Network::EthereumSepolia),
                    42161 => Ok(Network::Arbitrum),
                    421614 => Ok(Network::ArbitrumSepolia),
                    130 => Ok(Network::Unichain),
                    1301 => Ok(Network::UnichainSepolia),
                    143 => Ok(Network::Monad),
                    _ => Err(NetworkParseError::UnknownChainId(chain_id)),
                }
            }
            Namespace::Solana => {
                match id.reference() {
                    "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp" => Ok(Network::Solana),
                    "EtWTRABZaYq6iMfeYKouRu166VU2xqa1" => Ok(Network::SolanaDevnet),
                    _ => Err(NetworkParseError::UnknownSolanaGenesisHash(id.reference().to_string())),
                }
            }
            Namespace::Near => {
                match id.reference() {
                    "mainnet" => Ok(Network::Near),
                    "testnet" => Ok(Network::NearTestnet),
                    _ => Err(NetworkParseError::UnknownNearNetwork(id.reference().to_string())),
                }
            }
            Namespace::Stellar => {
                match id.reference() {
                    "pubnet" => Ok(Network::Stellar),
                    "testnet" => Ok(Network::StellarTestnet),
                    _ => Err(NetworkParseError::UnknownStellarNetwork(id.reference().to_string())),
                }
            }
            Namespace::Fogo => {
                match id.reference() {
                    "mainnet" => Ok(Network::Fogo),
                    "testnet" => Ok(Network::FogoTestnet),
                    _ => Err(NetworkParseError::UnknownFogoNetwork(id.reference().to_string())),
                }
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NetworkParseError {
    #[error("Invalid CAIP-2 format: {0}")]
    InvalidCaip2(String),
    #[error("Invalid chain ID: {0}")]
    InvalidChainId(String),
    #[error("Unknown EVM chain ID: {0}")]
    UnknownChainId(u64),
    #[error("Unknown Solana genesis hash: {0}")]
    UnknownSolanaGenesisHash(String),
    #[error("Unknown NEAR network: {0}")]
    UnknownNearNetwork(String),
    #[error("Unknown Stellar network: {0}")]
    UnknownStellarNetwork(String),
    #[error("Unknown Fogo network: {0}")]
    UnknownFogoNetwork(String),
}
```

---

## 2. Version 2 Core Types

### 2.1 ResourceInfo (New in v2)

```rust
/// Information about the resource requiring payment.
///
/// Introduced in x402 v2 to separate resource metadata from payment requirements.
/// Previously these fields were embedded in PaymentRequirements.
///
/// # References
/// - x402 v2 spec: https://github.com/coinbase/x402/blob/main/specs/x402-specification-v2.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceInfo {
    /// The URL of the protected resource
    pub url: Url,

    /// Human-readable description of the resource
    pub description: String,

    /// MIME type of the resource (e.g., "application/json", "text/html")
    pub mime_type: String,
}

impl ResourceInfo {
    /// Create a new ResourceInfo
    pub fn new(url: Url, description: String, mime_type: String) -> Self {
        Self {
            url,
            description,
            mime_type,
        }
    }
}
```

### 2.2 PaymentRequirementsV2 (Simplified from v1)

```rust
/// Payment requirements for x402 v2.
///
/// Simplified from v1 - resource metadata moved to ResourceInfo at top level.
///
/// # Breaking Changes from v1
/// - `resource`, `description`, `mime_type`, `output_schema` removed (moved to ResourceInfo)
/// - `max_amount_required` renamed to `amount`
/// - `network` now uses CAIP-2 format
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentRequirementsV2 {
    /// Payment scheme (currently only "exact")
    pub scheme: Scheme,

    /// Network in CAIP-2 format (e.g., "eip155:8453", "solana:5eykt...")
    pub network: Caip2NetworkId,

    /// Token contract address or account
    pub asset: MixedAddress,

    /// Exact amount required (renamed from maxAmountRequired)
    pub amount: TokenAmount,

    /// Recipient address for payment
    pub pay_to: MixedAddress,

    /// Maximum seconds before payment expires
    pub max_timeout_seconds: u64,

    /// Optional chain-specific or application-specific data
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
}

impl PaymentRequirementsV2 {
    /// Convert to v1 PaymentRequirements for backward compatibility
    pub fn to_v1(&self, resource_info: &ResourceInfo) -> Result<PaymentRequirements, NetworkParseError> {
        let network = Network::from_caip2(&self.network.to_string())?;

        Ok(PaymentRequirements {
            scheme: self.scheme,
            network,
            max_amount_required: self.amount,
            resource: resource_info.url.clone(),
            description: resource_info.description.clone(),
            mime_type: resource_info.mime_type.clone(),
            output_schema: None, // v2 doesn't have this
            pay_to: self.pay_to.clone(),
            max_timeout_seconds: self.max_timeout_seconds,
            asset: self.asset.clone(),
            extra: self.extra.clone(),
        })
    }
}

impl PaymentRequirements {
    /// Convert v1 PaymentRequirements to v2 format
    pub fn to_v2(&self) -> (ResourceInfo, PaymentRequirementsV2) {
        let resource_info = ResourceInfo {
            url: self.resource.clone(),
            description: self.description.clone(),
            mime_type: self.mime_type.clone(),
        };

        let requirements_v2 = PaymentRequirementsV2 {
            scheme: self.scheme,
            network: self.network.to_caip2(),
            asset: self.asset.clone(),
            amount: self.max_amount_required,
            pay_to: self.pay_to.clone(),
            max_timeout_seconds: self.max_timeout_seconds,
            extra: self.extra.clone(),
        };

        (resource_info, requirements_v2)
    }
}
```

### 2.3 PaymentPayloadV2

```rust
/// x402 v2 payment payload structure.
///
/// # Major Changes from v1
/// - New `resource` field (ResourceInfo) at top level
/// - `accepted` field containing payment requirements
/// - New `extensions` field for protocol extensions
/// - `x402_version` is now a plain u8 (value: 2)
///
/// # References
/// - x402 v2 spec: https://github.com/coinbase/x402/blob/main/specs/x402-specification-v2.md
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentPayloadV2 {
    /// Protocol version (always 2 for v2)
    pub x402_version: u8,

    /// Information about the protected resource
    pub resource: ResourceInfo,

    /// Accepted payment requirements
    pub accepted: PaymentRequirementsV2,

    /// Chain-specific payment authorization data
    pub payload: ExactPaymentPayload,

    /// Optional protocol extensions (e.g., "bazaar", "sign_in_with_x")
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub extensions: HashMap<String, serde_json::Value>,
}

impl PaymentPayloadV2 {
    /// Create a new v2 payment payload
    pub fn new(
        resource: ResourceInfo,
        accepted: PaymentRequirementsV2,
        payload: ExactPaymentPayload,
    ) -> Self {
        Self {
            x402_version: 2,
            resource,
            accepted,
            payload,
            extensions: HashMap::new(),
        }
    }

    /// Add an extension to the payload
    pub fn with_extension(mut self, name: String, value: serde_json::Value) -> Self {
        self.extensions.insert(name, value);
        self
    }

    /// Convert to v1 PaymentPayload for backward compatibility
    pub fn to_v1(&self) -> Result<PaymentPayload, NetworkParseError> {
        let network = Network::from_caip2(&self.accepted.network.to_string())?;

        Ok(PaymentPayload {
            x402_version: X402Version::V1,
            scheme: self.accepted.scheme,
            network,
            payload: self.payload.clone(),
        })
    }
}

impl PaymentPayload {
    /// Convert v1 PaymentPayload to v2 format
    ///
    /// # Arguments
    /// - `resource_info`: Metadata about the protected resource (not present in v1)
    pub fn to_v2(&self, resource_info: ResourceInfo, amount: TokenAmount, asset: MixedAddress, pay_to: MixedAddress) -> PaymentPayloadV2 {
        let accepted = PaymentRequirementsV2 {
            scheme: self.scheme,
            network: self.network.to_caip2(),
            asset,
            amount,
            pay_to,
            max_timeout_seconds: 300, // Default 5 minutes
            extra: None,
        };

        PaymentPayloadV2 {
            x402_version: 2,
            resource: resource_info,
            accepted,
            payload: self.payload.clone(),
            extensions: HashMap::new(),
        }
    }
}
```

---

## 3. Version Negotiation and Dual Support

### 3.1 Unified Version Enum

```rust
/// x402 protocol version.
///
/// Extended from v1-only to support both v1 and v2.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum X402Version {
    /// Protocol version 1 (legacy)
    V1,
    /// Protocol version 2 (current)
    V2,
}

impl X402Version {
    pub fn as_u8(&self) -> u8 {
        match self {
            X402Version::V1 => 1,
            X402Version::V2 => 2,
        }
    }
}

impl Display for X402Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_u8())
    }
}

impl Serialize for X402Version {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.as_u8())
    }
}

impl<'de> Deserialize<'de> for X402Version {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let num = u8::deserialize(deserializer)?;
        X402Version::try_from(num).map_err(serde::de::Error::custom)
    }
}

impl TryFrom<u8> for X402Version {
    type Error = X402VersionError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(X402Version::V1),
            2 => Ok(X402Version::V2),
            _ => Err(X402VersionError(value)),
        }
    }
}
```

### 3.2 Envelope Type for Dual Payload Support

```rust
/// Envelope type for handling both v1 and v2 payment payloads.
///
/// This type enables the facilitator to accept and route both protocol versions
/// during the migration period. Deserialization automatically detects the version
/// from the `x402_version` field.
///
/// # Lifecycle
/// - **Phase 1** (0-6 months): Dual support for v1 and v2
/// - **Phase 2** (6-12 months): v2 preferred, v1 deprecated warnings
/// - **Phase 3** (12+ months): v1 removed, v2 only
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PaymentPayloadEnvelope {
    V1(PaymentPayload),
    V2(PaymentPayloadV2),
}

impl PaymentPayloadEnvelope {
    /// Extract the protocol version
    pub fn version(&self) -> X402Version {
        match self {
            PaymentPayloadEnvelope::V1(_) => X402Version::V1,
            PaymentPayloadEnvelope::V2(_) => X402Version::V2,
        }
    }

    /// Extract the network (v1 enum or v2 CAIP-2)
    pub fn network_v1(&self) -> Result<Network, NetworkParseError> {
        match self {
            PaymentPayloadEnvelope::V1(payload) => Ok(payload.network),
            PaymentPayloadEnvelope::V2(payload) => {
                Network::from_caip2(&payload.accepted.network.to_string())
            }
        }
    }

    /// Extract the network as CAIP-2 (for v2 compatibility)
    pub fn network_v2(&self) -> Caip2NetworkId {
        match self {
            PaymentPayloadEnvelope::V1(payload) => payload.network.to_caip2(),
            PaymentPayloadEnvelope::V2(payload) => payload.accepted.network.clone(),
        }
    }

    /// Extract the payment payload (chain-specific authorization)
    pub fn payload(&self) -> &ExactPaymentPayload {
        match self {
            PaymentPayloadEnvelope::V1(p) => &p.payload,
            PaymentPayloadEnvelope::V2(p) => &p.payload,
        }
    }
}
```

### 3.3 Request/Response Versioning

```rust
/// Unified verify request supporting both v1 and v2
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum VerifyRequestEnvelope {
    V1(VerifyRequest),
    V2(VerifyRequestV2),
}

/// x402 v2 verify request
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyRequestV2 {
    pub x402_version: u8, // Always 2
    pub payment_payload: PaymentPayloadV2,
    pub resource: ResourceInfo,
    pub accepted: PaymentRequirementsV2,
}

impl VerifyRequestV2 {
    pub fn network(&self) -> &Caip2NetworkId {
        &self.accepted.network
    }
}

/// Unified settle request (same structure as verify)
pub type SettleRequestEnvelope = VerifyRequestEnvelope;
pub type SettleRequestV2 = VerifyRequestV2;
```

---

## 4. Extended SupportedPaymentKindsResponse

### 4.1 V2 Extensions

```rust
/// x402 v2 extended response for /supported endpoint.
///
/// # New in v2
/// - `extensions`: List of supported protocol extensions
/// - `signers`: Map of network patterns to facilitator signer addresses
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportedPaymentKindsResponseV2 {
    /// List of supported payment methods
    pub kinds: Vec<SupportedPaymentKindV2>,

    /// List of supported extensions (e.g., ["bazaar", "sign_in_with_x"])
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<String>,

    /// Facilitator signer addresses per network pattern
    /// Key format: "namespace:*" (e.g., "eip155:*", "solana:*")
    /// Value: List of signer addresses for that namespace
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub signers: HashMap<String, Vec<String>>,
}

/// Single supported payment kind in v2 format
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportedPaymentKindV2 {
    pub x402_version: u8, // Can be 1 or 2
    pub scheme: Scheme,

    /// Network in CAIP-2 format for v2, v1 string for v1
    pub network: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<SupportedPaymentKindExtra>,
}

impl SupportedPaymentKindsResponse {
    /// Convert v1 response to v2 format
    pub fn to_v2(
        &self,
        extensions: Vec<String>,
        signers: HashMap<String, Vec<String>>,
    ) -> SupportedPaymentKindsResponseV2 {
        let kinds = self.kinds.iter().map(|kind| {
            SupportedPaymentKindV2 {
                x402_version: kind.x402_version.as_u8(),
                scheme: kind.scheme,
                network: kind.network.clone(),
                extra: kind.extra.clone(),
            }
        }).collect();

        SupportedPaymentKindsResponseV2 {
            kinds,
            extensions,
            signers,
        }
    }
}

impl SupportedPaymentKindsResponseV2 {
    /// Create a new v2 response with both v1 and v2 network formats
    pub fn new(networks: &[Network], facilitator_addresses: &HashMap<Namespace, Vec<MixedAddress>>) -> Self {
        let mut kinds = Vec::new();
        let mut signers = HashMap::new();

        // Generate v1 entries (for backward compatibility)
        for network in networks {
            kinds.push(SupportedPaymentKindV2 {
                x402_version: 1,
                scheme: Scheme::Exact,
                network: network.to_string(),
                extra: None,
            });
        }

        // Generate v2 entries (CAIP-2 format)
        for network in networks {
            kinds.push(SupportedPaymentKindV2 {
                x402_version: 2,
                scheme: Scheme::Exact,
                network: network.to_caip2().to_string(),
                extra: None,
            });
        }

        // Populate signers map
        for (namespace, addresses) in facilitator_addresses {
            let key = format!("{}:*", namespace);
            let address_strings = addresses.iter().map(|a| a.to_string()).collect();
            signers.insert(key, address_strings);
        }

        Self {
            kinds,
            extensions: vec![], // No extensions initially
            signers,
        }
    }
}
```

---

## 5. Error Types for V2

```rust
/// Error reasons specific to x402 v2
#[derive(Debug, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum FacilitatorErrorReasonV2 {
    /// Payer doesn't have sufficient funds
    #[error("insufficient_funds")]
    InsufficientFunds,

    /// Invalid payment scheme
    #[error("invalid_scheme")]
    InvalidScheme,

    /// Network not supported or invalid CAIP-2 format
    #[error("invalid_network")]
    InvalidNetwork,

    /// Unexpected settlement error
    #[error("unexpected_settle_error")]
    UnexpectedSettleError,

    /// Invalid CAIP-2 network identifier
    #[error("invalid_caip2_network")]
    InvalidCaip2Network,

    /// Unsupported protocol extension
    #[error("unsupported_extension")]
    UnsupportedExtension,

    /// Resource metadata missing or invalid
    #[error("invalid_resource_info")]
    InvalidResourceInfo,

    /// Free-form error message
    #[error("{0}")]
    FreeForm(String),
}

impl From<FacilitatorErrorReason> for FacilitatorErrorReasonV2 {
    fn from(v1: FacilitatorErrorReason) -> Self {
        match v1 {
            FacilitatorErrorReason::InsufficientFunds => FacilitatorErrorReasonV2::InsufficientFunds,
            FacilitatorErrorReason::InvalidScheme => FacilitatorErrorReasonV2::InvalidScheme,
            FacilitatorErrorReason::InvalidNetwork => FacilitatorErrorReasonV2::InvalidNetwork,
            FacilitatorErrorReason::UnexpectedSettleError => FacilitatorErrorReasonV2::UnexpectedSettleError,
            FacilitatorErrorReason::FreeForm(msg) => FacilitatorErrorReasonV2::FreeForm(msg),
        }
    }
}
```

---

## 6. Facilitator Trait Extension

### 6.1 Versioned Trait Methods

```rust
/// Extended Facilitator trait supporting both v1 and v2
pub trait Facilitator {
    type Error: Debug + Display;

    // --- V1 methods (maintained for backward compatibility) ---

    fn verify(
        &self,
        request: &VerifyRequest,
    ) -> impl Future<Output = Result<VerifyResponse, Self::Error>> + Send;

    fn settle(
        &self,
        request: &SettleRequest,
    ) -> impl Future<Output = Result<SettleResponse, Self::Error>> + Send;

    fn supported(
        &self,
    ) -> impl Future<Output = Result<SupportedPaymentKindsResponse, Self::Error>> + Send;

    // --- V2 methods (new) ---

    /// Verify a v2 payment payload
    fn verify_v2(
        &self,
        request: &VerifyRequestV2,
    ) -> impl Future<Output = Result<VerifyResponse, Self::Error>> + Send {
        async {
            // Default implementation: convert to v1 and delegate
            let v1_request = request.to_v1()
                .map_err(|e| /* convert error */)?;
            self.verify(&v1_request).await
        }
    }

    /// Settle a v2 payment payload
    fn settle_v2(
        &self,
        request: &SettleRequestV2,
    ) -> impl Future<Output = Result<SettleResponse, Self::Error>> + Send {
        async {
            // Default implementation: convert to v1 and delegate
            let v1_request = request.to_v1()
                .map_err(|e| /* convert error */)?;
            self.settle(&v1_request).await
        }
    }

    /// Get v2 supported payment kinds with extensions
    fn supported_v2(
        &self,
    ) -> impl Future<Output = Result<SupportedPaymentKindsResponseV2, Self::Error>> + Send {
        async {
            let v1_response = self.supported().await?;
            // Convert to v2 with empty extensions/signers
            Ok(v1_response.to_v2(vec![], HashMap::new()))
        }
    }

    /// Unified verify handling both v1 and v2
    fn verify_any(
        &self,
        envelope: &VerifyRequestEnvelope,
    ) -> impl Future<Output = Result<VerifyResponse, Self::Error>> + Send {
        async {
            match envelope {
                VerifyRequestEnvelope::V1(req) => self.verify(req).await,
                VerifyRequestEnvelope::V2(req) => self.verify_v2(req).await,
            }
        }
    }

    /// Unified settle handling both v1 and v2
    fn settle_any(
        &self,
        envelope: &SettleRequestEnvelope,
    ) -> impl Future<Output = Result<SettleResponse, Self::Error>> + Send {
        async {
            match envelope {
                SettleRequestEnvelope::V1(req) => self.settle(req).await,
                SettleRequestEnvelope::V2(req) => self.settle_v2(req).await,
            }
        }
    }

    fn blacklist_info(
        &self,
    ) -> impl Future<Output = Result<serde_json::Value, Self::Error>> + Send {
        async {
            Ok(serde_json::json!({
                "total_blocked": 0,
                "evm_count": 0,
                "solana_count": 0,
                "near_count": 0,
                "stellar_count": 0,
                "entries": [],
                "source": "none",
                "loaded_at_startup": false
            }))
        }
    }
}
```

---

## 7. Migration Strategy

### 7.1 Phased Rollout

**Phase 1: Foundation (Weeks 1-2)**
- Add `Caip2NetworkId` type and `Network::to_caip2()` / `Network::from_caip2()` methods
- Add v2 types: `ResourceInfo`, `PaymentRequirementsV2`, `PaymentPayloadV2`
- Add envelope types: `PaymentPayloadEnvelope`, `VerifyRequestEnvelope`
- **No breaking changes** - v1 types remain unchanged

**Phase 2: Handler Updates (Week 3)**
- Update `/verify` and `/settle` to accept `VerifyRequestEnvelope` / `SettleRequestEnvelope`
- Auto-detect version and route to appropriate handler
- Update `/supported` to return v2 format with both v1 and v2 network strings

**Phase 3: Testing (Week 4)**
- Integration tests for v1 → v2 conversion
- Test dual v1/v2 clients against same facilitator
- Validate CAIP-2 parsing for all custom chains (NEAR, Stellar, Fogo)

**Phase 4: Deprecation (6+ months)**
- Add deprecation warnings to v1 types
- Update docs to recommend v2
- Monitor usage metrics (log v1 vs v2 requests)

**Phase 5: Removal (12+ months)**
- Remove v1 types and envelope wrappers
- v2 becomes canonical

### 7.2 Backward Compatibility Guarantees

```rust
impl PaymentPayloadV2 {
    /// Convert v2 payload to v1 format for legacy clients
    ///
    /// # Limitations
    /// - Extensions are dropped (v1 doesn't support)
    /// - ResourceInfo metadata is embedded into PaymentRequirements
    pub fn to_v1(&self) -> Result<PaymentPayload, NetworkParseError> {
        let network = Network::from_caip2(&self.accepted.network.to_string())?;

        Ok(PaymentPayload {
            x402_version: X402Version::V1,
            scheme: self.accepted.scheme,
            network,
            payload: self.payload.clone(),
        })
    }
}

impl PaymentPayload {
    /// Convert v1 payload to v2 format
    ///
    /// # Arguments
    /// - `resource_info`: Must be provided externally (not present in v1)
    /// - `amount`: Payment amount (extracted from requirements)
    /// - `asset`: Token address
    /// - `pay_to`: Recipient address
    pub fn to_v2(
        &self,
        resource_info: ResourceInfo,
        amount: TokenAmount,
        asset: MixedAddress,
        pay_to: MixedAddress,
    ) -> PaymentPayloadV2 {
        let accepted = PaymentRequirementsV2 {
            scheme: self.scheme,
            network: self.network.to_caip2(),
            asset,
            amount,
            pay_to,
            max_timeout_seconds: 300,
            extra: None,
        };

        PaymentPayloadV2 {
            x402_version: 2,
            resource: resource_info,
            accepted,
            payload: self.payload.clone(),
            extensions: HashMap::new(),
        }
    }
}
```

---

## 8. Testing Strategy

### 8.1 Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_caip2_evm_roundtrip() {
        let network = Network::Base;
        let caip2 = network.to_caip2();
        assert_eq!(caip2.to_string(), "eip155:8453");

        let parsed = Network::from_caip2("eip155:8453").unwrap();
        assert_eq!(parsed, Network::Base);
    }

    #[test]
    fn test_caip2_solana_roundtrip() {
        let network = Network::Solana;
        let caip2 = network.to_caip2();
        assert_eq!(caip2.to_string(), "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp");

        let parsed = Network::from_caip2("solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp").unwrap();
        assert_eq!(parsed, Network::Solana);
    }

    #[test]
    fn test_caip2_near_custom() {
        let network = Network::Near;
        let caip2 = network.to_caip2();
        assert_eq!(caip2.to_string(), "near:mainnet");

        let parsed = Network::from_caip2("near:mainnet").unwrap();
        assert_eq!(parsed, Network::Near);
    }

    #[test]
    fn test_caip2_stellar_custom() {
        let network = Network::Stellar;
        let caip2 = network.to_caip2();
        assert_eq!(caip2.to_string(), "stellar:pubnet");

        let parsed = Network::from_caip2("stellar:pubnet").unwrap();
        assert_eq!(parsed, Network::Stellar);
    }

    #[test]
    fn test_caip2_fogo_custom() {
        let network = Network::Fogo;
        let caip2 = network.to_caip2();
        assert_eq!(caip2.to_string(), "fogo:mainnet");

        let parsed = Network::from_caip2("fogo:mainnet").unwrap();
        assert_eq!(parsed, Network::Fogo);
    }

    #[test]
    fn test_v1_to_v2_payload_conversion() {
        let v1_payload = PaymentPayload {
            x402_version: X402Version::V1,
            scheme: Scheme::Exact,
            network: Network::Base,
            payload: ExactPaymentPayload::Evm(/* ... */),
        };

        let resource_info = ResourceInfo {
            url: Url::parse("https://api.example.com/data").unwrap(),
            description: "Premium data".to_string(),
            mime_type: "application/json".to_string(),
        };

        let v2_payload = v1_payload.to_v2(
            resource_info.clone(),
            TokenAmount::from(1000000u64), // 1 USDC
            MixedAddress::Evm(address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913").into()),
            MixedAddress::Evm(address!("0x1234567890123456789012345678901234567890").into()),
        );

        assert_eq!(v2_payload.x402_version, 2);
        assert_eq!(v2_payload.accepted.network.to_string(), "eip155:8453");
        assert_eq!(v2_payload.resource.url, resource_info.url);
    }

    #[test]
    fn test_v2_to_v1_payload_conversion() {
        let resource_info = ResourceInfo {
            url: Url::parse("https://api.example.com/data").unwrap(),
            description: "Premium data".to_string(),
            mime_type: "application/json".to_string(),
        };

        let v2_payload = PaymentPayloadV2 {
            x402_version: 2,
            resource: resource_info,
            accepted: PaymentRequirementsV2 {
                scheme: Scheme::Exact,
                network: Caip2NetworkId::parse("eip155:8453").unwrap(),
                asset: MixedAddress::Evm(address!("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913").into()),
                amount: TokenAmount::from(1000000u64),
                pay_to: MixedAddress::Evm(address!("0x1234567890123456789012345678901234567890").into()),
                max_timeout_seconds: 300,
                extra: None,
            },
            payload: ExactPaymentPayload::Evm(/* ... */),
            extensions: HashMap::new(),
        };

        let v1_payload = v2_payload.to_v1().unwrap();
        assert_eq!(v1_payload.network, Network::Base);
        assert!(matches!(v1_payload.x402_version, X402Version::V1));
    }

    #[test]
    fn test_caip2_invalid_namespace() {
        let result = Caip2NetworkId::parse("unknown:123");
        assert!(result.is_err());
    }

    #[test]
    fn test_caip2_invalid_evm_chain_id() {
        let result = Caip2NetworkId::parse("eip155:not_a_number");
        assert!(result.is_err());
    }

    #[test]
    fn test_caip2_invalid_near_network() {
        let result = Caip2NetworkId::parse("near:devnet");
        assert!(result.is_err());
    }
}
```

### 8.2 Integration Tests

```python
# tests/integration/test_v2_protocol.py

import requests
import json

FACILITATOR_URL = "http://localhost:8080"

def test_supported_v2_format():
    """Test /supported endpoint returns v2 format with CAIP-2"""
    resp = requests.get(f"{FACILITATOR_URL}/supported")
    assert resp.status_code == 200

    data = resp.json()
    assert "kinds" in data
    assert "extensions" in data
    assert "signers" in data

    # Check for v2 CAIP-2 formats
    networks = [k["network"] for k in data["kinds"] if k["x402_version"] == 2]
    assert "eip155:8453" in networks  # Base mainnet
    assert "near:mainnet" in networks
    assert "stellar:pubnet" in networks

def test_verify_v2_payload():
    """Test /verify accepts v2 payload with CAIP-2 network"""
    payload = {
        "x402Version": 2,
        "paymentPayload": {
            "x402_version": 2,
            "resource": {
                "url": "https://api.example.com/data",
                "description": "Premium data",
                "mimeType": "application/json"
            },
            "accepted": {
                "scheme": "exact",
                "network": "eip155:84532",  # Base Sepolia CAIP-2
                "amount": "1000000",
                "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
                "payTo": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb",
                "maxTimeoutSeconds": 300
            },
            "payload": {
                "signature": "0x...",
                "authorization": {
                    "from": "0x...",
                    "to": "0x...",
                    "value": "1000000",
                    "validAfter": 0,
                    "validBefore": 2000000000,
                    "nonce": "0x..."
                }
            }
        },
        "resource": {
            "url": "https://api.example.com/data",
            "description": "Premium data",
            "mimeType": "application/json"
        },
        "accepted": {
            "scheme": "exact",
            "network": "eip155:84532",
            "amount": "1000000",
            "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
            "payTo": "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb",
            "maxTimeoutSeconds": 300
        }
    }

    resp = requests.post(f"{FACILITATOR_URL}/verify", json=payload)
    assert resp.status_code == 200

def test_backward_compat_v1_still_works():
    """Test v1 payloads still work during transition"""
    payload = {
        "x402Version": 1,
        "paymentPayload": {
            "x402_version": 1,
            "scheme": "exact",
            "network": "base-sepolia",  # v1 format
            "payload": { /* ... */ }
        },
        "paymentRequirements": { /* ... */ }
    }

    resp = requests.post(f"{FACILITATOR_URL}/verify", json=payload)
    assert resp.status_code == 200
```

---

## 9. Performance Considerations

### 9.1 Zero-Cost Abstractions

```rust
// CAIP-2 parsing is done once at deserialization
// Internal representation uses efficient enum for namespace

#[inline(always)]
pub fn network_family(caip2: &Caip2NetworkId) -> NetworkFamily {
    // No string parsing - constant time lookup
    match caip2.namespace() {
        Namespace::Eip155 => NetworkFamily::Evm,
        Namespace::Solana | Namespace::Fogo => NetworkFamily::Solana,
        Namespace::Near => NetworkFamily::Near,
        Namespace::Stellar => NetworkFamily::Stellar,
    }
}

// Network enum to CAIP-2 conversion is zero-cost at runtime
// All mappings are compile-time constants
impl Network {
    #[inline(always)]
    pub const fn chain_id(&self) -> Option<u64> {
        match self {
            Network::Base => Some(8453),
            Network::BaseSepolia => Some(84532),
            // ... all mappings known at compile time
            _ => None,
        }
    }
}
```

### 9.2 Memory Layout

```rust
// Caip2NetworkId is stack-allocated, no heap allocation for namespace
assert_eq!(std::mem::size_of::<Namespace>(), 1); // Single byte enum

// String reference is efficient for small chain IDs
// Only Solana genesis hashes are large (44 chars)
```

---

## 10. Summary and Next Steps

### 10.1 Type System Highlights

1. **CAIP-2 Support**: Full bidirectional conversion between `Network` enum and CAIP-2 format
2. **Custom Chains**: First-class support for NEAR, Stellar, Fogo via custom namespaces
3. **Dual v1/v2 Support**: Envelope types enable transparent protocol version negotiation
4. **Zero-Cost**: Compile-time constants and efficient runtime parsing
5. **Type Safety**: Compile-time validation of network identifiers via enum

### 10.2 Implementation Checklist

- [ ] Add `caip2.rs` module with `Caip2NetworkId`, `Namespace` types
- [ ] Extend `network.rs` with `to_caip2()` and `from_caip2()` methods
- [ ] Add v2 types in `types.rs`: `ResourceInfo`, `PaymentRequirementsV2`, `PaymentPayloadV2`
- [ ] Add envelope types: `PaymentPayloadEnvelope`, `VerifyRequestEnvelope`
- [ ] Extend `Facilitator` trait with `verify_v2()`, `settle_v2()`, `supported_v2()`
- [ ] Update handlers to accept envelope types
- [ ] Add unit tests for CAIP-2 parsing and conversions
- [ ] Add integration tests for v1/v2 dual support
- [ ] Update `/supported` to return v2 format
- [ ] Document migration guide for users

### 10.3 Open Questions

1. **NEAR CAIP-2 Standard**: Is `near:mainnet` / `near:testnet` the canonical format, or should we use network IDs?
2. **Stellar CAIP-2 Standard**: Same question for `stellar:pubnet` / `stellar:testnet`
3. **Fogo Namespace**: Should we propose `fogo` as a CAIP namespace, or use `solana` namespace with Fogo genesis hashes?
4. **Extension Support**: Which v2 extensions should we prioritize (Bazaar, Sign-in-with-X, etc.)?

---

## References

- **x402 v2 Spec**: https://github.com/coinbase/x402/blob/main/specs/x402-specification-v2.md
- **CAIP-2 Standard**: https://github.com/ChainAgnostic/CAIPs/blob/main/CAIPs/caip-2.md
- **x402 v1 Spec**: https://github.com/coinbase/x402/blob/main/specs/x402-specification-v1.md
- **TypeScript SDK**: https://github.com/coinbase/x402/tree/main/typescript

---

**Document End**
