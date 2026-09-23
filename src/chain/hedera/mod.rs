//! Native Hedera exact/v2. Client signatures authorize principal; the dedicated
//! account sponsors only consensus fees. No EVM relay or EIP-3009 is involved.
pub mod codec;
mod config;
#[cfg(test)]
pub(crate) use config::DEFAULT_MAX_TRANSACTION_FEE_TINYBARS;
pub mod id;
mod mirror;
mod store;
pub mod wire;

use crate::{
    chain::{FacilitatorLocalError, FromEnvByNetworkBuild, NetworkProviderOps},
    facilitator::Facilitator,
    network::Network,
    types::{
        ExactPaymentPayload, FacilitatorErrorReason, MixedAddress, Scheme, SettleRequest,
        SettleResponse, SupportedPaymentKind, SupportedPaymentKindExtra,
        SupportedPaymentKindsResponse, SupportedTokenInfo, TokenType, TransactionHash,
        VerifyRequest, VerifyResponse, X402Version,
    },
};
use base64::{engine::general_purpose::STANDARD, Engine};
use codec::{Decoded, Intent, Policy, Result};
use config::Config;
use hiero_sdk::{AnyTransaction, Client, Status, TransactionReceiptQuery, TransferTransaction};
use id::EntityId;
use mirror::Mirror;
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use store::{Record, State, Store};

// Metrics resolve configured HTS decimals from the same validated provider
// configuration that /supported advertises, never a guessed fiat default.
static ASSET_DECIMALS: once_cell::sync::Lazy<
    std::sync::RwLock<std::collections::HashMap<(Network, String), u8>>,
> = once_cell::sync::Lazy::new(|| std::sync::RwLock::new(std::collections::HashMap::new()));
pub fn asset_decimals(network: Network, asset: &str) -> Option<u8> {
    ASSET_DECIMALS
        .read()
        .ok()?
        .get(&(network, asset.to_owned()))
        .copied()
}

#[derive(Clone)]
pub struct HederaProvider {
    config: Config,
    client: Client,
    mirror: Mirror,
    store: Arc<dyn Store>,
}
impl std::fmt::Debug for HederaProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HederaProvider")
            .field("network", &self.config.network)
            .field("account", &self.config.account)
            .finish_non_exhaustive()
    }
}
fn error(reason: impl Into<String>) -> FacilitatorLocalError {
    FacilitatorLocalError::Other(format!("hedera: {}", reason.into()))
}
impl FromEnvByNetworkBuild for HederaProvider {
    async fn from_env(
        network: Network,
    ) -> std::result::Result<Option<Self>, Box<dyn std::error::Error>> {
        let Some(config) = Config::from_env(network)? else {
            return Ok(None);
        };
        let client = config.client();
        let mirror = Mirror::new(config.mirror.clone())?;
        let store = Arc::new(store::DynamoStore::new(config.table.clone()).await);
        let provider = Self {
            config,
            client,
            mirror,
            store,
        };
        // A configured ledger is served whatever its health check answers. The
        // check runs in the background and alerts; `/health/ready` reports the
        // ledger's state on every refresh. Through 2.39.1 a failed check was `?`,
        // which `ProviderCache::from_env` turned into `exit(1)` for every
        // network, and through 2.39.3 it left the ledger out of /supported
        // until the next deploy: a consensus probe that timed out once at
        // startup took Hedera mainnet off the landing on 2026-09-23.
        if let Ok(mut registry) = ASSET_DECIMALS.write() {
            for (asset, decimals) in &provider.config.assets {
                registry.insert((network, asset.to_string()), *decimals);
            }
        }
        provider.start_recovery();
        provider.watch_startup_health();
        Ok(Some(provider))
    }
}

/// Why a health check did not pass. `/health/ready` reports [`Self::reason`];
/// the detail stays in the server log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HealthFailure {
    /// The consensus nodes did not answer the cost query in time.
    ConsensusTimeout,
    /// The consensus nodes or the Mirror Node could not be read.
    Unreachable(String),
    /// The settlement table could not be read.
    StoreUnavailable(String),
    /// The ledger says the sponsor account is controlled by another key, or by
    /// one this facilitator cannot sign for alone.
    SponsorKeyMismatch,
}

impl HealthFailure {
    /// Bounded token for `/health/ready`.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::ConsensusTimeout => "rpc_timeout",
            Self::Unreachable(_) => "rpc_unreachable",
            Self::StoreUnavailable(_) => "store_unavailable",
            Self::SponsorKeyMismatch => "signer_key_mismatch",
        }
    }
}

impl std::fmt::Display for HealthFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConsensusTimeout => f.write_str("Hedera consensus probe timeout"),
            Self::Unreachable(detail) | Self::StoreUnavailable(detail) => f.write_str(detail),
            Self::SponsorKeyMismatch => f.write_str("sponsor account/key mismatch"),
        }
    }
}

/// The part of the health check the Mirror Node's account answer decides:
/// settles the sponsor's HBAR still pays for, at one `max_fee` each (0 below
/// one), or a key that is not ours.
fn sponsor_settles(
    account: &serde_json::Value,
    key: &hiero_sdk::PrivateKey,
    max_fee: u64,
) -> std::result::Result<u64, HealthFailure> {
    let ledger_key = mirror::account_key(account).map_err(HealthFailure::Unreachable)?;
    let actual = match ledger_key.key {
        Some(
            hiero_sdk_proto::services::key::Key::Ed25519(k)
            | hiero_sdk_proto::services::key::Key::EcdsaSecp256k1(k),
        ) => k,
        _ => return Err(HealthFailure::SponsorKeyMismatch),
    };
    if actual != key.public_key().to_bytes_raw() {
        return Err(HealthFailure::SponsorKeyMismatch);
    }
    let balance = mirror::hbar_balance(account).map_err(HealthFailure::Unreachable)?;
    Ok(balance / max_fee)
}
impl NetworkProviderOps for HederaProvider {
    fn signer_address(&self) -> MixedAddress {
        MixedAddress::Hedera(self.config.account.clone())
    }
    fn network(&self) -> Network {
        self.config.network
    }
}
impl HederaProvider {
    /// Read-only recovery for the portable receipt. Reuses the native durable
    /// intent and HTTPS mirror verification; never co-signs or submits.
    pub async fn receipt_evidence(&self, transaction_id: &str) -> Option<bool> {
        let key = format!("{}#{transaction_id}", self.config.network.to_caip2());
        let record = self.store.read(&key).await.ok()??;
        if record.state == State::Confirmed { return Some(true); }
        if record.state == State::Failed { return Some(false); }
        self.resolve(&record, Duration::from_secs(3)).await.map(|r| r.0)
    }

    fn inspect_admission(&self, request: &VerifyRequest) -> std::result::Result<(Decoded, Intent), FacilitatorLocalError> {
        let requirements = &request.payment_requirements;
        if requirements.network == self.config.network {
            if let Ok(asset) = requirements.asset.to_string().parse::<EntityId>() {
                if !self.config.assets.contains_key(&asset) {
                    return Err(FacilitatorLocalError::UnsupportedAsset(
                        None, self.config.network, asset.to_string(),
                    ));
                }
            }
        }
        self.inspect(request, true).map_err(error)
    }

    fn inspect(&self, request: &VerifyRequest, check_time: bool) -> Result<(Decoded, Intent)> {
        if request.x402_version != X402Version::V2
            || request.payment_payload.x402_version != X402Version::V2
        {
            return Err("only x402 v2 is supported".into());
        }
        let r = &request.payment_requirements;
        if r.network != self.config.network
            || request.payment_payload.network != self.config.network
            || r.scheme != Scheme::Exact
            || request.payment_payload.scheme != Scheme::Exact
        {
            return Err("network or scheme mismatch".into());
        }
        let ExactPaymentPayload::Hedera(payload) = &request.payment_payload.payload else {
            return Err("native Hedera envelope required".into());
        };
        let extra = r
            .extra
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .ok_or("missing fee payer")?;
        if extra.len() != 1
            || extra.get("feePayer").and_then(serde_json::Value::as_str)
                != Some(self.config.account.to_string().as_str())
        {
            return Err("fee payer mismatch or unsupported extension".into());
        }
        let asset: EntityId = r.asset.to_string().parse()?;
        let pay_to: EntityId = r.pay_to.to_string().parse()?;
        // With time checks off we only decode an intent to look up its durable
        // record. This preserves historical HBAR receipts. Every new admission
        // and every new co-signature goes through inspect(..., true).
        if check_time && !self.config.assets.contains_key(&asset) {
            return Err("unsupported asset: Hedera payments support native USDC only; HBAR is for network fees".into());
        }
        let amount = r
            .max_amount_required
            .to_string()
            .parse::<i64>()
            .map_err(|_| "amount exceeds i64")?;
        let nodes: BTreeSet<_> = self
            .client
            .network()
            .values()
            .map(|id| id.to_string().parse())
            .collect::<std::result::Result<_, _>>()?;
        let decoded = Decoded::from_base64(&payload.transaction)?;
        let intent = decoded.inspect(
            &Policy {
                network: &self.config.network.to_caip2(),
                fee_payer: &self.config.account,
                pay_to: &pay_to,
                asset: &asset,
                amount,
                max_fee: self.config.max_fee,
                max_duration: r.max_timeout_seconds,
                allowed_nodes: &nodes,
            },
            store::now(),
            check_time,
        )?;
        Ok((decoded, intent))
    }
    fn record_key(&self, intent: &Intent) -> String {
        format!(
            "{}#{}",
            self.config.network.to_caip2(),
            intent.transaction_id
        )
    }
    fn unconfirmed(&self, intent: &Intent) -> FacilitatorLocalError {
        FacilitatorLocalError::SettlementUnconfirmed(
            TransactionHash::Hedera(intent.transaction_id.clone()),
            self.config.network,
        )
    }
    fn response(&self, record: &Record) -> SettleResponse {
        SettleResponse {
            success: record.state == State::Confirmed,
            error_reason: (record.state != State::Confirmed).then(|| {
                FacilitatorErrorReason::FreeForm(format!(
                    "hedera_consensus_{}",
                    record.consensus_status.as_deref().unwrap_or("unknown")
                ))
            }),
            payer: MixedAddress::Hedera(record.intent.payer.clone()),
            transaction: Some(TransactionHash::Hedera(
                record.intent.transaction_id.clone(),
            )),
            network: self.config.network,
            proof_of_payment: None,
            extensions: None,
        }
    }
    async fn receipt(&self, intent: &Intent, timeout: Duration) -> Option<Status> {
        let id = intent.transaction_id.parse().ok()?;
        let mut query = TransactionReceiptQuery::new();
        query
            .transaction_id(id)
            .validate_status(false)
            .include_duplicates(false)
            .node_account_ids(self.client.network().values().copied());
        match tokio::time::timeout(timeout, query.execute(&self.client)).await {
            Ok(Ok(receipt))
                if !matches!(receipt.status, Status::Unknown | Status::ReceiptNotFound) =>
            {
                Some(receipt.status)
            }
            _ => None,
        }
    }
    async fn resolve(&self, record: &Record, timeout: Duration) -> Option<(bool, String)> {
        // Published Rust SDK consensus transport is plaintext gRPC. Do not
        // authorize delivery from that response alone: bind the result to our
        // persisted signed bytes through the authenticated HTTPS Mirror.
        let deadline = tokio::time::Instant::now() + timeout;
        let _ = self
            .receipt(&record.intent, timeout.min(Duration::from_secs(3)))
            .await;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }
            if let Ok(Ok(Some(status))) = tokio::time::timeout(
                remaining,
                self.mirror.settled(&record.intent, record.signed.as_ref()?),
            )
            .await
            {
                return Some(status);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }
    async fn confirmed(
        &self,
        key: &str,
        mut record: Record,
        status: (bool, String),
    ) -> std::result::Result<SettleResponse, FacilitatorLocalError> {
        record.state = if status.0 {
            State::Confirmed
        } else {
            State::Failed
        };
        record.consensus_status = Some(status.1);
        record.lease_until = store::now();
        // If the receipt is known but cannot be persisted, return uncertainty;
        // a retry reconciles the SAME transaction ID and makes the result durable.
        self.store
            .save(key, &record)
            .await
            .map_err(|_| self.unconfirmed(&record.intent))?;
        Ok(self.response(&record))
    }
    // Recovery runs even when admissions are disabled. Leases and conditional
    // writes coordinate replicas; only the original persisted bytes may be sent.
    fn start_recovery(&self) {
        let provider = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(30));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                let network = provider.config.network.to_caip2();
                let pending = match provider.store.pending(&network).await {
                    Ok(records) => records,
                    Err(_) => {
                        tracing::warn!(network, "Hedera recovery storage unavailable");
                        continue;
                    }
                };
                for (key, candidate) in pending {
                    let owner = uuid::Uuid::new_v4().to_string();
                    let record = match provider
                        .store
                        .reserve(
                            &key,
                            &network,
                            &candidate.intent,
                            &owner,
                            provider.config.daily_budget,
                        )
                        .await
                    {
                        Ok(record)
                            if record.owner == owner
                                && !record.terminal()
                                && record.signed.is_some() =>
                        {
                            record
                        }
                        _ => continue,
                    };
                    if let Some(status) = provider.resolve(&record, Duration::from_secs(3)).await {
                        let _ = provider.confirmed(&key, record, status).await;
                    } else {
                        let _ = provider.deliver_record(&key, record).await;
                    }
                }
            }
        });
    }
    async fn deliver_record(
        &self,
        key: &str,
        mut record: Record,
    ) -> std::result::Result<SettleResponse, FacilitatorLocalError> {
        if record.intent.expires_at > store::now().saturating_add(2) {
            let bytes = STANDARD
                .decode(
                    record
                        .signed
                        .as_ref()
                        .ok_or_else(|| error("missing persisted transaction"))?,
                )
                .map_err(|_| error("invalid persisted bytes"))?;
            let mut tx: TransferTransaction = AnyTransaction::from_bytes(&bytes)
                .map_err(|_| error("invalid persisted transaction"))?
                .downcast()
                .map_err(|_| error("persisted transaction is not a transfer"))?;
            record.state = State::Submitted;
            // The signed bytes are durable and recovery sends them once the
            // lease runs out, so a write that fails here is not a failure of
            // the payment: it may still go out, under this id.
            self.store
                .save(key, &record)
                .await
                .map_err(|_| self.unconfirmed(&record.intent))?;
            // Errors, including DUPLICATE_TRANSACTION, do not establish the
            // outcome. Resolve the original ID through a consensus receipt.
            let _ = tokio::time::timeout(Duration::from_secs(10), tx.execute(&self.client)).await;
        }
        if let Some(status) = self.resolve(&record, self.config.settle_timeout).await {
            return self.confirmed(key, record, status).await;
        }
        record.state = State::Uncertain;
        record.lease_until = store::now().saturating_add(60);
        let _ = self.store.save(key, &record).await;
        Err(self.unconfirmed(&record.intent))
    }
    /// Settles the sponsor's HBAR still pays for, once the settlement table,
    /// the Mirror Node and the consensus nodes have all answered. A balance
    /// below one `max_fee` is `Ok(0)`, which `/health/ready` grades
    /// `signer_gas_critical`, not a failure to reach the ledger.
    pub async fn health(&self) -> std::result::Result<u64, HealthFailure> {
        let consensus = async {
            // Consensus v0.77 removed cryptoGetBalance. Explicit node IDs also
            // avoid the old SDK's implicit balance-based ping. COST_ANSWER
            // probes connectivity without signing or paying for a query.
            let mut query = hiero_sdk::AccountInfoQuery::new();
            query
                .account_id(hiero_sdk::AccountId::new(0, 0, 2))
                .node_account_ids(self.client.network().values().copied());
            match tokio::time::timeout(Duration::from_secs(5), query.get_cost(&self.client)).await {
                Err(_) => Err(HealthFailure::ConsensusTimeout),
                Ok(Err(_)) => Err(HealthFailure::Unreachable(
                    "Hedera consensus unavailable".into(),
                )),
                Ok(Ok(_)) => Ok(()),
            }
        };
        let (_, account, _) = tokio::try_join!(
            async {
                self.store
                    .health()
                    .await
                    .map_err(HealthFailure::StoreUnavailable)
            },
            async {
                self.mirror
                    .account(&self.config.account)
                    .await
                    .map_err(HealthFailure::Unreachable)
            },
            consensus
        )?;
        sponsor_settles(&account, &self.config.key, self.config.max_fee)
    }

    /// Run the health check once, in the background, and again every 30-60 s
    /// while it fails; log the first failure (the alert token) and the
    /// recovery. It never decides whether the ledger is served.
    fn watch_startup_health(&self) {
        let provider = self.clone();
        tokio::spawn(async move {
            let network = provider.config.network;
            let Err(failure) = provider.health().await else {
                return;
            };
            tracing::error!(
                %network,
                reason = failure.reason(),
                detail = %crate::redact::scrub_urls(&failure.to_string()),
                "[FAIL] hedera_health_failed_at_startup: {network} stays served; \
                 /health/ready reports it and the check runs again until it passes"
            );
            let ((), attempts) = crate::chain::reprobe(
                || async {
                    match provider.health().await {
                        Ok(_) => Some(()),
                        Err(failure) => {
                            tracing::debug!(
                                %network,
                                reason = failure.reason(),
                                "Hedera health check still failing"
                            );
                            None
                        }
                    }
                },
                crate::chain::reprobe_delay,
            )
            .await;
            tracing::info!(
                %network,
                attempts,
                "[OK] hedera_health_recovered: the ledger answers again"
            );
        });
    }

    /// A provider on `mirror` and `nodes` with a settlement table that always
    /// answers, for tests that need a ledger whose health they control.
    #[cfg(test)]
    pub(crate) fn for_health_tests(
        network: Network,
        mirror: url::Url,
        nodes: std::collections::HashMap<String, hiero_sdk::AccountId>,
    ) -> Self {
        struct HealthyTable;
        #[async_trait::async_trait]
        impl Store for HealthyTable {
            async fn read(&self, _: &str) -> Result<Option<Record>> {
                Ok(None)
            }
            async fn reserve(
                &self,
                _: &str,
                _: &str,
                _: &Intent,
                _: &str,
                _: u64,
            ) -> Result<Record> {
                Err("health tests reserve nothing".into())
            }
            async fn save(&self, _: &str, _: &Record) -> Result<()> {
                Err("health tests save nothing".into())
            }
            async fn pending(&self, _: &str) -> Result<Vec<(String, Record)>> {
                Ok(vec![])
            }
            async fn health(&self) -> Result<()> {
                Ok(())
            }
        }
        let config = Config {
            network,
            account: "0.0.3003".parse().expect("entity id"),
            key: hiero_sdk::PrivateKey::from_bytes_ed25519(&[7; 32]).expect("key"),
            mirror: mirror.clone(),
            assets: Config::payment_assets(network, None).expect("assets"),
            max_fee: DEFAULT_MAX_TRANSACTION_FEE_TINYBARS,
            daily_budget: 10 * DEFAULT_MAX_TRANSACTION_FEE_TINYBARS,
            settle_timeout: Duration::from_secs(5),
            table: "unused".into(),
            admissions: true,
        };
        let client = hiero_sdk::Client::for_network(nodes).expect("client");
        Self {
            config,
            client,
            mirror: Mirror::new(mirror).expect("mirror"),
            store: Arc::new(HealthyTable),
        }
    }
}
impl Facilitator for HederaProvider {
    type Error = FacilitatorLocalError;
    async fn verify(
        &self,
        request: &VerifyRequest,
    ) -> std::result::Result<VerifyResponse, Self::Error> {
        if !self.config.admissions {
            return Err(error("new Hedera admissions are disabled"));
        }
        let (decoded, intent) = self.inspect_admission(request)?;
        if self
            .store
            .read(&self.record_key(&intent))
            .await
            .map_err(error)?
            .is_some()
        {
            return Ok(VerifyResponse::invalid(
                Some(MixedAddress::Hedera(intent.payer)),
                FacilitatorErrorReason::FreeForm("hedera_payment_already_reserved".into()),
            ));
        }
        self.mirror
            .preflight(&decoded, &intent, &self.config)
            .await
            .map_err(error)?;
        Ok(VerifyResponse::Valid {
            payer: MixedAddress::Hedera(intent.payer),
        })
    }
    async fn settle(
        &self,
        request: &SettleRequest,
    ) -> std::result::Result<SettleResponse, Self::Error> {
        let (decoded, intent) = self.inspect(request, false).map_err(error)?;
        // Link the portable receipt BEFORE native recovery can see a reserved
        // or co-signed transaction. A crash between the two stores remains
        // reconcilable by this immutable native transaction ID.
        crate::receipts::prepared_hedera(&intent.transaction_id).await.map_err(error)?;
        let key = self.record_key(&intent);
        if let Some(existing) = self.store.read(&key).await.map_err(error)? {
            if existing.intent.fingerprint != intent.fingerprint {
                return Err(error("transaction ID binds another intent"));
            }
            if existing.terminal() {
                return Ok(self.response(&existing));
            }
        } else {
            if !self.config.admissions {
                return Err(error("new Hedera admissions are disabled"));
            }
            self.inspect_admission(request)?;
            self.mirror
                .preflight(&decoded, &intent, &self.config)
                .await
                .map_err(error)?;
        }
        let owner = uuid::Uuid::new_v4().to_string();
        let mut record = self
            .store
            .reserve(
                &key,
                &self.config.network.to_caip2(),
                &intent,
                &owner,
                self.config.daily_budget,
            )
            .await
            .map_err(error)?;
        if record.terminal() {
            return Ok(self.response(&record));
        }
        if record.owner != owner {
            return Err(self.unconfirmed(&intent));
        }
        if record.signed.is_some() {
            if let Some(status) = self.resolve(&record, Duration::from_secs(3)).await {
                return self.confirmed(&key, record, status).await;
            }
        } else {
            self.inspect_admission(request)?;
            self.mirror
                .preflight(&decoded, &intent, &self.config)
                .await
                .map_err(error)?;
            let signed = decoded.cosign(&self.config.key).map_err(error)?;
            record.signed = Some(STANDARD.encode(signed));
            record.state = State::Prepared;
            // Mandatory durable boundary BEFORE any call that can submit. A
            // write that fails may still have stored the co-signed bytes, which
            // recovery would then send: reported under the id, never as a
            // failure the caller would answer with a new signature.
            self.store
                .save(&key, &record)
                .await
                .map_err(|_| self.unconfirmed(&intent))?;
        }
        self.deliver_record(&key, record).await
    }
    async fn supported(&self) -> std::result::Result<SupportedPaymentKindsResponse, Self::Error> {
        if !self.config.admissions {
            return Ok(SupportedPaymentKindsResponse { kinds: vec![] });
        }
        let usdc = if self.config.network.is_testnet() {
            "0.0.429274"
        } else {
            "0.0.456858"
        };
        let tokens = self
            .config
            .assets
            .iter()
            .map(|(id, decimals)| SupportedTokenInfo {
                token: if id.is_hbar() {
                    TokenType::Hbar
                } else if id.to_string() == usdc {
                    TokenType::Usdc
                } else {
                    TokenType::Hts
                },
                address: MixedAddress::Hedera(id.clone()),
                decimals: *decimals,
            })
            .collect();
        Ok(SupportedPaymentKindsResponse {
            kinds: vec![SupportedPaymentKind {
                x402_version: X402Version::V2,
                scheme: Scheme::Exact,
                network: self.config.network.to_caip2(),
                network_aliases: Some(vec![self.config.network.to_caip2()]),
                extra: Some(SupportedPaymentKindExtra {
                    fee_payer: Some(self.signer_address()),
                    tokens: Some(tokens),
                    escrow: None,
                }),
            }],
        })
    }
}

#[cfg(test)]
mod tests;
