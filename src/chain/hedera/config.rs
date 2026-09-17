use super::{codec::Result, id::EntityId};
use crate::network::Network;
use hiero_sdk::{Client, Hbar, PrivateKey};
use std::{collections::BTreeMap, time::Duration};

#[derive(Clone)]
pub struct Config {
    pub network: Network,
    pub account: EntityId,
    pub key: PrivateKey,
    pub mirror: url::Url,
    pub assets: BTreeMap<EntityId, u8>,
    pub max_fee: u64,
    pub daily_budget: u64,
    pub settle_timeout: Duration,
    pub table: String,
    pub admissions: bool,
}
impl Config {
    pub fn from_env(network: Network) -> Result<Option<Self>> {
        if !network.is_hedera() {
            return Err("not a Hedera network".into());
        }
        let suffix = if network.is_testnet() {
            "TESTNET"
        } else {
            "MAINNET"
        };
        if std::env::var(format!("HEDERA_ENABLED_{suffix}"))
            .ok()
            .as_deref()
            != Some("true")
        {
            return Ok(None);
        }
        let required = |name: &str| {
            std::env::var(name)
                .ok()
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("missing {name}"))
        };
        let account: EntityId = required(&format!("HEDERA_ACCOUNT_ID_{suffix}"))?.parse()?;
        account.account()?;
        let key: PrivateKey = required(&format!("HEDERA_PRIVATE_KEY_{suffix}"))?
            .parse()
            .map_err(|_| "invalid Hedera private key (value redacted)")?;
        let other = if network.is_testnet() {
            "MAINNET"
        } else {
            "TESTNET"
        };
        if let Ok(value) = std::env::var(format!("HEDERA_PRIVATE_KEY_{other}")) {
            if let Ok(other) = value.parse::<PrivateKey>() {
                if other.public_key() == key.public_key() {
                    return Err("Hedera mainnet and testnet require distinct signing keys".into());
                }
            }
        }
        let mirror = std::env::var(format!("HEDERA_MIRROR_URL_{suffix}"))
            .unwrap_or_else(|_| {
                if network.is_testnet() {
                    "https://testnet.mirrornode.hedera.com/"
                } else {
                    "https://mainnet-public.mirrornode.hedera.com/"
                }
                .into()
            })
            .parse::<url::Url>()
            .map_err(|_| "invalid Hedera Mirror URL")?;
        if mirror.scheme() != "https"
            || mirror.host_str().is_none()
            || !mirror.username().is_empty()
            || mirror.password().is_some()
            || mirror.query().is_some()
            || mirror.fragment().is_some()
            || mirror.path() != "/"
        {
            return Err("Hedera Mirror URL must be an HTTPS origin without credentials".into());
        }
        let usdc = if network.is_testnet() {
            "0.0.429274"
        } else {
            "0.0.456858"
        };
        let mut assets = BTreeMap::from([("0.0.0".parse()?, 8), (usdc.parse()?, 6)]);
        // Additional HTS FTs require an explicit decimals assertion, validated
        // against fresh token metadata on every payment: 0.0.1234:4,...
        if let Ok(extra) = std::env::var(format!("HEDERA_ADDITIONAL_TOKENS_{suffix}")) {
            for item in extra.split(',').filter(|v| !v.is_empty()) {
                let (id, decimals) = item
                    .split_once(':')
                    .ok_or("HTS config must be token-id:decimals")?;
                let id = id.parse::<EntityId>()?;
                let decimals = decimals.parse::<u8>().map_err(|_| "invalid HTS decimals")?;
                if assets.contains_key(&id) || decimals > 18 {
                    return Err("duplicate token or unsupported decimals".into());
                }
                assets.insert(id, decimals);
            }
        }
        let number = |name: &str, fallback: u64| -> Result<u64> {
            std::env::var(name).map_or(Ok(fallback), |v| {
                v.parse().map_err(|_| format!("invalid {name}"))
            })
        };
        let max_fee = number("HEDERA_MAX_TRANSACTION_FEE_TINYBARS", 100_000_000)?;
        let daily_budget = required(&format!("HEDERA_DAILY_BUDGET_TINYBARS_{suffix}"))?
            .parse::<u64>()
            .map_err(|_| "invalid Hedera daily budget")?;
        let timeout = number("HEDERA_SETTLEMENT_TIMEOUT_SECS", 45)?;
        if max_fee == 0
            || max_fee > i64::MAX as u64
            || daily_budget < max_fee
            || daily_budget > i64::MAX as u64
            || !(5..=60).contains(&timeout)
        {
            return Err("invalid Hedera fee, daily budget or settlement timeout".into());
        }
        Ok(Some(Self {
            network,
            account,
            key,
            mirror,
            assets,
            max_fee,
            daily_budget,
            settle_timeout: Duration::from_secs(timeout),
            table: required("HEDERA_SETTLEMENT_TABLE_NAME")?,
            admissions: std::env::var(format!("HEDERA_ADMISSIONS_ENABLED_{suffix}"))
                .ok()
                .as_deref()
                != Some("false"),
        }))
    }
    pub fn client(&self) -> Client {
        let client = if self.network.is_testnet() {
            Client::for_testnet()
        } else {
            Client::for_mainnet()
        };
        // Never let a timeout/expiry create a NEW transaction ID. We also do
        // not install an SDK operator: only the explicitly inspected, persisted
        // bytes below may acquire the sponsor's signature.
        client.set_default_regenerate_transaction_id(false);
        client.set_default_max_transaction_fee(Hbar::from_tinybars(self.max_fee as i64));
        client.set_default_max_query_payment(Hbar::from_tinybars(0));
        client.set_request_timeout(Some(self.settle_timeout));
        client
    }
}
