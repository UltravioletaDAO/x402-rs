//! Native v2 envelope. Parse with network context before the untagged Solana
//! payload can capture the identically shaped Hedera transaction.
use crate::{
    network::Network,
    types::{
        ExactPaymentPayload, MixedAddress, PaymentPayload, Scheme, VerifyRequest, X402Version,
    },
    types_v2::{PaymentRequirementsV2, ResourceInfo},
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExactHederaPayload {
    pub transaction: String,
}

#[derive(Clone, Debug)]
pub struct HederaRequest {
    pub request: VerifyRequest,
    raw: Value,
}

impl<'de> Deserialize<'de> for HederaRequest {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = Value::deserialize(d)?;
        Self::parse(raw).map_err(serde::de::Error::custom)
    }
}
impl Serialize for HederaRequest {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.raw.serialize(s)
    }
}
impl HederaRequest {
    fn parse(raw: Value) -> Result<Self, String> {
        let payload = raw.get("paymentPayload").ok_or("missing paymentPayload")?;
        if payload.get("x402Version").and_then(Value::as_u64) != Some(2)
            || raw
                .get("x402Version")
                .is_some_and(|v| v.as_u64() != Some(2))
        {
            return Err("Hedera requires x402 v2".into());
        }
        let accepted = payload
            .get("accepted")
            .ok_or("missing accepted requirements")?;
        let outer = raw
            .get("paymentRequirements")
            .or_else(|| raw.get("accepted"))
            .ok_or("missing paymentRequirements")?;
        for requirements in [accepted, outer] {
            let object = requirements
                .as_object()
                .ok_or("invalid payment requirements")?;
            if object.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "scheme"
                        | "network"
                        | "asset"
                        | "amount"
                        | "payTo"
                        | "maxTimeoutSeconds"
                        | "extra"
                )
            }) {
                return Err("unsupported Hedera requirements field".into());
            }
        }
        if raw.get("paymentRequirements").is_some()
            && raw.get("accepted").is_some_and(|v| v != outer)
        {
            return Err("conflicting outer payment requirements".into());
        }
        let a: PaymentRequirementsV2 =
            serde_json::from_value(accepted.clone()).map_err(|e| e.to_string())?;
        let b: PaymentRequirementsV2 =
            serde_json::from_value(outer.clone()).map_err(|e| e.to_string())?;
        let network = Network::from_caip2(&a.network.to_string())
            .filter(Network::is_hedera)
            .ok_or("not a native Hedera network")?;
        if a != b || a.scheme != Scheme::Exact {
            return Err("accepted_payment_requirements_mismatch".into());
        }
        // Reject unsupported extensions before any co-signature. Do not silently
        // drop an extension when converting to the shared internal request.
        for ext in [raw.get("extensions"), payload.get("extensions")] {
            if ext.is_some_and(|v| !v.as_object().is_some_and(|m| m.is_empty())) {
                return Err("unsupported Hedera extension".into());
            }
        }
        let extra = a
            .extra
            .as_ref()
            .and_then(Value::as_object)
            .ok_or("missing feePayer")?;
        if extra.keys().any(|k| k != "feePayer") {
            return Err("unsupported Hedera extra field".into());
        }
        extra
            .get("feePayer")
            .and_then(Value::as_str)
            .ok_or("missing feePayer")?
            .parse::<super::id::EntityId>()?
            .account()?;
        let resource = payload.get("resource").or_else(|| raw.get("resource"));
        let info = ResourceInfo {
            url: resource
                .and_then(|v| v.get("url"))
                .and_then(Value::as_str)
                .unwrap_or("https://hedera.invalid/payment")
                .parse()
                .map_err(|_| "invalid resource URL")?,
            description: resource
                .and_then(|v| v.get("description"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
            mime_type: resource
                .and_then(|v| v.get("mimeType"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
        };
        let mut requirements = a.to_v1(&info).map_err(|e| e.to_string())?;
        requirements.asset = MixedAddress::Hedera(a.asset.to_string().parse()?);
        let pay_to: super::id::EntityId = a.pay_to.to_string().parse()?;
        pay_to.account()?;
        requirements.pay_to = MixedAddress::Hedera(pay_to);
        let native: ExactHederaPayload =
            serde_json::from_value(payload.get("payload").ok_or("missing payload")?.clone())
                .map_err(|e| e.to_string())?;
        let request = VerifyRequest {
            x402_version: X402Version::V2,
            payment_payload: PaymentPayload {
                x402_version: X402Version::V2,
                scheme: Scheme::Exact,
                network,
                payload: ExactPaymentPayload::Hedera(native),
            },
            payment_requirements: requirements,
        };
        Ok(Self { request, raw })
    }
}
