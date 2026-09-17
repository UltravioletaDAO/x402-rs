//! Bounded REST preflight. Never follow a URL supplied by a request or by a
//! pagination response; token relationships are queried by exact numeric ID.
use super::{
    codec::{Decoded, Intent, Result},
    config::Config,
    id::EntityId,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use hiero_sdk_proto::services as pb;
use prost::Message;
use serde_json::Value;
use std::time::Duration;

#[derive(Clone)]
pub struct Mirror {
    http: reqwest::Client,
    origin: url::Url,
}
impl Mirror {
    /// Consensus receipts have a short retention window. Recover older results
    /// from Mirror only when BOTH the original ID and a hash of our persisted
    /// co-signed bytes match. Duplicate-transaction records are not a result.
    pub async fn settled(&self, intent: &Intent, encoded: &str) -> Result<Option<(bool, String)>> {
        let decoded = Decoded::from_base64(encoded)?;
        // SHA-384 of each exact SignedTransaction byte string, as defined by
        // the native SDK. Do not thaw/re-serialize: protobuf field order/defaults
        // are significant to this hash even when semantically equivalent.
        use sha2::{Digest, Sha384};
        let list = hiero_sdk_proto::sdk::TransactionList::decode(decoded.bytes.as_slice())
            .map_err(|_| "invalid persisted transaction list")?;
        let hashes: std::collections::BTreeSet<_> = list
            .transaction_list
            .iter()
            .map(|entry| STANDARD.encode(Sha384::digest(&entry.signed_transaction_bytes)))
            .collect();
        let (id, timestamp) = intent
            .transaction_id
            .split_once('@')
            .ok_or("invalid transaction ID")?;
        let (seconds, nanos) = timestamp.split_once('.').ok_or("invalid timestamp")?;
        let mirror_id = format!("{id}-{seconds}-{nanos}");
        let response = self
            .get(&format!(
                "/api/v1/transactions/{mirror_id}?nonce=0&scheduled=false"
            ))
            .await?;
        let rows = response
            .get("transactions")
            .and_then(Value::as_array)
            .ok_or("missing transaction records")?;
        for row in rows {
            if row.get("transaction_id").and_then(Value::as_str) != Some(mirror_id.as_str())
                || row.get("nonce").and_then(Value::as_i64) != Some(0)
                || !row
                    .get("transaction_hash")
                    .and_then(Value::as_str)
                    .is_some_and(|h| hashes.contains(h))
            {
                continue;
            }
            let result = row
                .get("result")
                .and_then(Value::as_str)
                .ok_or("missing consensus result")?;
            if matches!(
                result,
                "DUPLICATE_TRANSACTION" | "UNKNOWN" | "RECEIPT_NOT_FOUND"
            ) {
                continue;
            }
            return Ok(Some((result == "SUCCESS", result.to_owned())));
        }
        Ok(None)
    }
    pub fn new(origin: url::Url) -> Result<Self> {
        Ok(Self {
            origin,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(8))
                .connect_timeout(Duration::from_secs(3))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| "cannot initialize Hedera Mirror client")?,
        })
    }
    async fn get(&self, path: &str) -> Result<Value> {
        let url = self.origin.join(path).map_err(|_| "invalid Mirror path")?;
        if url.origin() != self.origin.origin() {
            return Err("Mirror origin mismatch".into());
        }
        let mut response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|_| "Hedera Mirror unavailable")?;
        if !response.status().is_success() {
            return Err(format!("Hedera Mirror HTTP {}", response.status()));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Hedera Mirror read failed")?
        {
            if bytes.len() + chunk.len() > 262_144 {
                return Err("Hedera Mirror response too large".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| "invalid Hedera Mirror JSON".into())
    }
    pub async fn account(&self, id: &EntityId) -> Result<Value> {
        let value = self
            .get(&format!("/api/v1/accounts/{id}?transactions=false"))
            .await?;
        if value.get("account").and_then(Value::as_str) != Some(id.to_string().as_str())
            || value.get("deleted").and_then(Value::as_bool) != Some(false)
        {
            return Err("Hedera account missing, deleted or mismatched".into());
        }
        Ok(value)
    }
    async fn association(&self, id: &EntityId, asset: &EntityId, decimals: u8) -> Result<u64> {
        let value = self
            .get(&format!(
                "/api/v1/accounts/{id}/tokens?token.id={asset}&limit=2"
            ))
            .await?;
        if !value.pointer("/links/next").is_some_and(Value::is_null) {
            return Err("ambiguous token relationship page".into());
        }
        let relationships = value
            .get("tokens")
            .and_then(Value::as_array)
            .ok_or("missing token associations")?;
        if relationships.len() != 1 {
            return Err("token must be associated before payment".into());
        }
        let t = &relationships[0];
        if t.get("token_id").and_then(Value::as_str) != Some(asset.to_string().as_str())
            || t.get("decimals").and_then(Value::as_u64) != Some(decimals as u64)
            || !matches!(
                t.get("freeze_status").and_then(Value::as_str),
                Some("UNFROZEN" | "NOT_APPLICABLE")
            )
            || !matches!(
                t.get("kyc_status").and_then(Value::as_str),
                Some("GRANTED" | "NOT_APPLICABLE")
            )
        {
            return Err("token relationship is frozen, lacks KYC or mismatches metadata".into());
        }
        t.get("balance")
            .and_then(Value::as_u64)
            .ok_or("missing token balance".into())
    }
    pub async fn preflight(
        &self,
        decoded: &Decoded,
        intent: &Intent,
        config: &Config,
    ) -> Result<()> {
        let (payer, payee, sponsor) = tokio::try_join!(
            self.account(&intent.payer),
            self.account(&intent.pay_to),
            self.account(&config.account)
        )?;
        decoded.verify_key(&account_key(&payer)?)?;
        let sponsor_key = account_key(&sponsor)?;
        let raw = config.key.public_key().to_bytes_raw();
        let expected = if raw.len() == 32 {
            pb::key::Key::Ed25519(raw)
        } else {
            pb::key::Key::EcdsaSecp256k1(raw)
        };
        if sponsor_key.key.as_ref() != Some(&expected) {
            return Err("configured sponsor key does not control Hedera account".into());
        }
        if payee
            .get("receiver_sig_required")
            .and_then(Value::as_bool)
            .ok_or("missing receiver signature policy")?
            && intent.pay_to != config.account
        {
            decoded.verify_key(&account_key(&payee)?)?;
        }
        if hbar_balance(&sponsor)? < intent.fee {
            return Err("insufficient sponsor HBAR".into());
        }
        let decimals = *config
            .assets
            .get(&intent.asset)
            .ok_or("asset not allowed")?;
        if intent
            .expected_decimals
            .is_some_and(|d| d != decimals as u32)
        {
            return Err("transaction token decimals mismatch".into());
        }
        if intent.asset.is_hbar() {
            if hbar_balance(&payer)? < intent.amount as u64 {
                return Err("insufficient payer HBAR".into());
            }
        } else {
            let token = self
                .get(&format!("/api/v1/tokens/{}", intent.asset))
                .await?;
            if token.get("type").and_then(Value::as_str) != Some("FUNGIBLE_COMMON")
                || token.get("deleted").and_then(Value::as_bool) != Some(false)
                || !matches!(
                    token.get("pause_status").and_then(Value::as_str),
                    Some("UNPAUSED" | "NOT_APPLICABLE")
                )
                || token
                    .get("decimals")
                    .and_then(|d| d.as_u64().or_else(|| d.as_str()?.parse().ok()))
                    != Some(decimals as u64)
            {
                return Err("HTS token type, status or decimals unsupported".into());
            }
            let fees = token
                .get("custom_fees")
                .and_then(Value::as_object)
                .ok_or("missing custom fee metadata")?;
            for (kind, value) in fees {
                if kind == "created_timestamp" {
                    continue;
                }
                if !value.as_array().is_some_and(|fees| fees.is_empty()) {
                    return Err("HTS custom fees unsupported".into());
                }
            }
            let (balance, _) = tokio::try_join!(
                self.association(&intent.payer, &intent.asset, decimals),
                self.association(&intent.pay_to, &intent.asset, decimals)
            )?;
            if balance < intent.amount as u64 {
                return Err("insufficient payer HTS balance".into());
            }
        }
        Ok(())
    }
}
pub fn hbar_balance(account: &Value) -> Result<u64> {
    account
        .pointer("/balance/balance")
        .and_then(Value::as_u64)
        .ok_or("missing native HBAR balance".into())
}
pub fn account_key(account: &Value) -> Result<pb::Key> {
    let key = account.get("key").ok_or("missing account key")?;
    let text = key
        .get("key")
        .and_then(Value::as_str)
        .ok_or("hollow or missing account key")?;
    if text.len() > 16_384 {
        return Err("account key too large".into());
    }
    let bytes = hex::decode(text).map_err(|_| "invalid account key encoding")?;
    let key = match key.get("_type").and_then(Value::as_str) {
        Some("ED25519") if bytes.len() == 32 => pb::key::Key::Ed25519(bytes),
        Some("ECDSA_SECP256K1") if bytes.len() == 33 => pb::key::Key::EcdsaSecp256k1(bytes),
        Some("ProtobufEncoded") => {
            return pb::Key::decode(bytes.as_slice())
                .map_err(|_| "invalid protobuf account key".into())
        }
        _ => return Err("unsupported account key type".into()),
    };
    Ok(pb::Key { key: Some(key) })
}
