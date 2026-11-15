//! Sui fullnode gRPC (`sui.rpc.v2`) behind verify and settle, over a list of
//! endpoints tried in order.
//!
//! Sui JSON-RPC is gone from the public fullnodes: `fullnode.*.sui.io` answer
//! `-32601` to every method and `sui-rpc.publicnode.com` answers `-32000`
//! (measured 2026-11-14), so the facilitator could neither verify nor settle a
//! Sui payment. Reads and executions now go through Mysten's gRPC client
//! (`sui-rpc`). The transaction itself is still decoded, validated and signed
//! with `sui-types` and travels as its BCS bytes.
//!
//! # Endpoints
//!
//! `RPC_URL_SUI` / `RPC_URL_SUI_TESTNET` take one URL or several separated by
//! commas, tried in that order (starting from the last one that answered).
//! Unset, blank, or without one usable URL, the public defaults below apply:
//! two operators per network, Mysten's fullnodes and Allnodes' publicnode.
//! Public endpoints are rate limited. Only scheme, host and port reach the
//! server (tonic drops a URL's path) and no header is added, so a provider that
//! wants its key in the path or a header cannot be used here.
//!
//! Errors returned to callers name endpoints by position (`endpoint 2`), never
//! by host; the log lines carry the redacted URL.
//!
//! # What moves to the next endpoint
//!
//! - A read (objects, balance, a transaction lookup) that fails on one endpoint
//!   for any reason is asked of the next. Reads change nothing.
//! - An execution is sent to an endpoint only after that endpoint answered a
//!   lookup of the transaction's digest. Found before this call sent anything,
//!   with or without a verdict: an earlier call executed it, nothing is sent
//!   anywhere and the answer is [`Execution::AlreadyExecuted`], never this
//!   call's own execution. Found after a send: that send's effects. Not found:
//!   the endpoint is reachable and does not know it. An answer about another
//!   transaction, or about none, before any send: as an endpoint that did not
//!   answer. Any other answer: nothing was sent there, so the next endpoint is
//!   tried.
//! - A refusal from the chain (`InvalidArgument`, `FailedPrecondition`: a bad
//!   signature, unusable inputs, objects locked by another transaction) is the
//!   verdict. The transaction is never sent anywhere else.
//! - An endpoint that refuses the call at the door (`Unimplemented`,
//!   `PermissionDenied`, `Unauthenticated`) did not take it: next endpoint. If
//!   no endpoint takes it, every endpoint is still asked about the digest once
//!   more before the answer is "nothing was sent", in case a proxy said no
//!   after forwarding it.
//! - No new send starts once [`EXECUTE_TIMEOUT`] has passed since the first:
//!   the endpoints are asked about the digest instead, which bounds a settle.
//! - Every other failure leaves the outcome unknown: the transaction may be
//!   final. The next endpoint's digest lookup comes before any resend, and once
//!   an outcome was unknown the answer can only be the effects an endpoint
//!   reports or [`Execution::Unconfirmed`] -- never a refusal, which a resend
//!   can earn precisely because the first send landed.
//! - A resend is the same signed bytes. Sui executes a digest at most once, so a
//!   resend can only return the effects of that one execution.

use std::future::Future;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use sui_rpc::field::{FieldMask, FieldMaskUtil};
use sui_rpc::proto::sui::rpc::v2::{
    get_object_result, owner::OwnerKind, BatchGetObjectsRequest, Bcs, ExecuteTransactionRequest,
    ExecutedTransaction, GetBalanceRequest, GetObjectRequest, GetTransactionRequest, Object,
    Transaction, UserSignature,
};
use sui_types::base_types::{ObjectDigest, ObjectID, SequenceNumber, SuiAddress};
use tonic::Code;
use tracing::{info, warn};

use crate::network::Network;

/// Public gRPC endpoints for Sui mainnet, in the order they are tried.
///
/// Both answered `GetServiceInfo` with chain id `4btiuiMP...` and `GetBalance`
/// for the facilitator's wallet on 2026-11-14 (`scripts/sui_endpoints_smoke.py`
/// and the ignored test `sui_endpoints_live_smoke` re-measure them).
pub const DEFAULT_ENDPOINTS_MAINNET: &[&str] = &[
    "https://fullnode.mainnet.sui.io:443",
    "https://sui-grpc.publicnode.com:443",
];

/// Public gRPC endpoints for Sui testnet (chain id `69WiPg3D...`), in order.
pub const DEFAULT_ENDPOINTS_TESTNET: &[&str] = &[
    "https://fullnode.testnet.sui.io:443",
    "https://sui-testnet-grpc.publicnode.com:443",
];

/// How long one execution may wait for its effects on one endpoint: the
/// request timeout the JSON-RPC SDK used. Past it the outcome is unknown,
/// never a failure.
pub const EXECUTE_TIMEOUT: Duration = Duration::from_secs(60);

/// The default endpoints of a Sui network.
pub fn default_endpoints(network: Network) -> &'static [&'static str] {
    match network {
        Network::Sui => DEFAULT_ENDPOINTS_MAINNET,
        _ => DEFAULT_ENDPOINTS_TESTNET,
    }
}

/// The endpoints `raw` names (comma-separated), or the network's defaults.
///
/// Entries are trimmed and de-duplicated in order (case and a trailing `/`
/// aside). An entry that is not an absolute `http`/`https` URL with a host is
/// skipped with a warning. A list with no usable entry -- unset, blank, only
/// commas, or nothing but garbage -- falls back to the defaults, so a typo in
/// the variable degrades to the public endpoints instead of disabling Sui.
pub fn endpoint_list(raw: Option<&str>, network: Network) -> Vec<String> {
    let mut endpoints: Vec<String> = Vec::new();
    let mut skipped = 0usize;
    for entry in raw.unwrap_or_default().split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        if !is_endpoint_url(entry) {
            skipped += 1;
            warn!(
                network = %network,
                endpoint = %crate::redact::rpc_url(entry),
                "Ignoring a Sui RPC endpoint that is not an http(s) URL"
            );
            continue;
        }
        let key = entry.trim_end_matches('/');
        if !endpoints
            .iter()
            .any(|known| known.trim_end_matches('/').eq_ignore_ascii_case(key))
        {
            endpoints.push(entry.to_string());
        }
    }
    if endpoints.is_empty() {
        if skipped > 0 {
            warn!(
                network = %network,
                "No usable Sui RPC endpoint configured; using the public defaults"
            );
        }
        return default_endpoints(network)
            .iter()
            .map(|url| url.to_string())
            .collect();
    }
    endpoints
}

fn is_endpoint_url(entry: &str) -> bool {
    match url::Url::parse(entry) {
        Ok(url) => {
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some_and(|host| !host.is_empty())
        }
        Err(_) => false,
    }
}

/// One object as the chain holds it now, or its absence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectRead {
    Found {
        version: SequenceNumber,
        digest: ObjectDigest,
    },
    /// The endpoint holds no such object (at that version, when one was asked).
    NotFound,
}

/// What became of a sponsored transaction handed to [`SuiRpc::execute`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Execution {
    /// The chain executed what this call sent. `failure` is the on-chain error
    /// when the transaction executed but its commands did not take effect (gas
    /// is still charged and the digest exists).
    Executed { failure: Option<String> },
    /// An endpoint held the transaction, executed, before this call sent
    /// anything (with or without its verdict): an earlier call executed it,
    /// so this one is not a new execution. Nothing was sent.
    AlreadyExecuted,
    /// The chain refused it before execution. Final: nothing executed.
    Refused(String),
    /// It may have executed: an endpoint took it and no endpoint answered with
    /// its effects.
    Unconfirmed(String),
    /// No endpoint took it: every one failed before it was sent.
    Unreachable(String),
}

/// How an execution error leaves the transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sent {
    /// The chain looked at it and said no.
    Refused,
    /// The endpoint turned the call away before forwarding it.
    NotSent,
    /// It may have reached the validators and may be final.
    Unknown,
}

/// Classify a failed `ExecuteTransaction` by its gRPC status.
///
/// A Sui node answers `InvalidArgument` for an invalid signature or inputs and
/// `FailedPrecondition` for objects locked by another transaction
/// (`sui-rpc-api` `From<TransactionSubmissionError>`). It answers `Unavailable`
/// when the transaction timed out before finality or the system is overloaded,
/// `Aborted` when it is already finalized with other signatures, and
/// `Internal`/`ResourceExhausted` for its own trouble: all of those may be
/// final. `Unimplemented`, `PermissionDenied` and `Unauthenticated` come from
/// an endpoint that does not serve execution (or a proxy that wants a key)
/// and forwarded nothing. Anything else is unknown.
pub fn classify_execution_error(status: &tonic::Status) -> Sent {
    match status.code() {
        Code::InvalidArgument | Code::FailedPrecondition => Sent::Refused,
        Code::Unimplemented | Code::PermissionDenied | Code::Unauthenticated => Sent::NotSent,
        _ => Sent::Unknown,
    }
}

/// The on-chain verdict an executed transaction carries: `Ok(None)` executed
/// and took effect, `Ok(Some(error))` executed and failed, `Err` when the
/// answer has no verdict (no status, or another transaction's digest).
pub fn effects_verdict(
    transaction: &ExecutedTransaction,
    digest: &str,
) -> Result<Option<String>, String> {
    if let Some(answered) = transaction.digest.as_deref() {
        if answered != digest {
            return Err(format!("answered for transaction {answered}, not {digest}"));
        }
    }
    let status = transaction
        .effects
        .as_ref()
        .and_then(|effects| effects.status.as_ref())
        .ok_or_else(|| "answer carries no execution status".to_string())?;
    match status.success {
        Some(true) => Ok(None),
        Some(false) => {
            let error = status.error.as_ref();
            let description = error
                .and_then(|e| e.description.clone())
                .filter(|d| !d.trim().is_empty());
            Ok(Some(description.unwrap_or_else(
                || match error.and_then(|e| e.kind) {
                    Some(kind) => format!("execution failed (error kind {kind})"),
                    None => "execution failed".to_string(),
                },
            )))
        }
        None => Err("answer carries no execution status".to_string()),
    }
}

/// `Coin<coin_type>` owned by `owner` as an address-owned object, read from an
/// object with `owner` and `object_type`.
pub fn is_coin_owned_by(object: &Object, coin_type: &str, owner: &SuiAddress) -> bool {
    let owned_by = object.owner.as_ref().is_some_and(|o| {
        o.kind == Some(OwnerKind::Address as i32)
            && o.address
                .as_deref()
                .and_then(|address| SuiAddress::from_str(address).ok())
                .is_some_and(|address| address == *owner)
    });
    let expected = sui_types::parse_sui_struct_tag(&format!("0x2::coin::Coin<{coin_type}>"));
    let actual = object
        .object_type
        .as_deref()
        .map(sui_types::parse_sui_struct_tag);
    let is_coin = match (expected, actual) {
        (Ok(expected), Some(Ok(actual))) => expected == actual,
        _ => false,
    };
    owned_by && is_coin
}

/// A spendable coin balance from a `GetBalance` answer: the total minus what
/// sits in the address balance, which a `SplitCoins` of a coin object cannot
/// reach. A node without address balances reports neither, and its total is
/// the coins.
pub fn coin_balance_of(balance: &sui_rpc::proto::sui::rpc::v2::Balance) -> Option<u64> {
    let total = balance.balance?;
    Some(total.saturating_sub(balance.address_balance.unwrap_or(0)))
}

fn describe(status: &tonic::Status) -> String {
    crate::redact::scrub_urls(&format!("{:?}: {}", status.code(), status.message()))
}

struct Endpoint {
    /// Its position in the configured list, for errors a caller sees.
    name: String,
    /// The redacted URL, for log lines only.
    label: String,
    client: sui_rpc::Client,
}

enum Lookup {
    /// The endpoint knows the transaction; the verdict, or why there is none.
    Found(Result<Option<String>, String>),
    /// The endpoint answered about another transaction, or about none: not
    /// evidence that it knows this one.
    Malformed(String),
    NotFound,
    Unreadable(String),
}

/// The Sui gRPC endpoints of one network.
pub struct SuiRpc {
    network: Network,
    urls: Vec<String>,
    /// Built on first use: a lazy tonic channel spawns its worker on the
    /// current runtime, and a provider can be built outside one (tests).
    endpoints: OnceLock<Vec<Endpoint>>,
    /// The endpoint that answered last; every call starts there.
    preferred: AtomicUsize,
    read_timeout: Duration,
    execute_timeout: Duration,
}

impl SuiRpc {
    pub fn new(
        network: Network,
        urls: Vec<String>,
        read_timeout: Duration,
        execute_timeout: Duration,
    ) -> Self {
        Self {
            network,
            urls,
            endpoints: OnceLock::new(),
            preferred: AtomicUsize::new(0),
            read_timeout,
            execute_timeout,
        }
    }

    /// The endpoints named by the network's env var, or the defaults.
    pub fn from_env(network: Network, env_var: &str) -> Self {
        let raw = std::env::var(env_var).ok();
        let urls = endpoint_list(raw.as_deref(), network);
        Self::new(
            network,
            urls,
            crate::chain::rpc_http_timeout(),
            EXECUTE_TIMEOUT,
        )
    }

    /// The endpoints in the order they are tried, redacted, for a log line.
    pub fn labels(&self) -> Vec<String> {
        self.urls
            .iter()
            .map(|url| crate::redact::rpc_url(url))
            .collect()
    }

    /// Build the clients now (inside a runtime) and say how many are usable.
    pub fn connect(&self) -> usize {
        self.endpoints().len()
    }

    fn endpoints(&self) -> &[Endpoint] {
        self.endpoints.get_or_init(|| {
            let built = self.build(&self.urls);
            if !built.is_empty() || self.urls.is_empty() {
                return built;
            }
            // Every configured URL passed `endpoint_list` and tonic still took
            // none (a host it cannot parse): the public defaults, as for a
            // variable with no usable URL, rather than no Sui at all.
            warn!(
                network = %self.network,
                "No configured Sui RPC endpoint is usable; using the public defaults"
            );
            let defaults: Vec<String> = default_endpoints(self.network)
                .iter()
                .map(|url| url.to_string())
                .collect();
            self.build(&defaults)
        })
    }

    fn build(&self, urls: &[String]) -> Vec<Endpoint> {
        urls.iter()
            .enumerate()
            .filter_map(|(position, url)| match sui_rpc::Client::new(url.as_str()) {
                Ok(client) => Some(Endpoint {
                    name: format!("endpoint {}", position + 1),
                    label: crate::redact::rpc_url(url),
                    client,
                }),
                Err(e) => {
                    warn!(
                        network = %self.network,
                        endpoint = %crate::redact::rpc_url(url),
                        error = %describe(&e),
                        "Sui RPC endpoint unusable; skipping it"
                    );
                    None
                }
            })
            .collect()
    }

    fn start(&self, len: usize) -> usize {
        self.preferred.load(Ordering::Relaxed) % len
    }

    fn prefer(&self, index: usize) {
        self.preferred.store(index, Ordering::Relaxed);
    }

    /// Ask `call` of each endpoint, starting at the one that answered last,
    /// until one answers. `Err` lists why each endpoint did not.
    async fn read<T, F, Fut>(&self, what: &str, call: F) -> Result<T, String>
    where
        F: Fn(sui_rpc::Client) -> Fut,
        Fut: Future<Output = Result<T, tonic::Status>>,
    {
        let endpoints = self.endpoints();
        if endpoints.is_empty() {
            return Err("no usable Sui RPC endpoint".to_string());
        }
        let start = self.start(endpoints.len());
        let mut failures = Vec::with_capacity(endpoints.len());
        for step in 0..endpoints.len() {
            let index = (start + step) % endpoints.len();
            let endpoint = &endpoints[index];
            let failure = match tokio::time::timeout(
                self.read_timeout,
                call(endpoint.client.clone()),
            )
            .await
            {
                Ok(Ok(value)) => {
                    self.prefer(index);
                    return Ok(value);
                }
                Ok(Err(status)) => describe(&status),
                Err(_) => format!("timed out after {:?}", self.read_timeout),
            };
            warn!(
                network = %self.network,
                endpoint = %endpoint.label,
                %what,
                error = %failure,
                "Sui RPC read failed on this endpoint"
            );
            failures.push(format!("{}: {failure}", endpoint.name));
        }
        Err(failures.join("; "))
    }

    /// The current version and digest of each object, or the one at a given
    /// version, in request order. One `BatchGetObjects` call.
    pub async fn objects(
        &self,
        keys: &[(ObjectID, Option<SequenceNumber>)],
    ) -> Result<Vec<ObjectRead>, String> {
        let requests: Vec<GetObjectRequest> = keys
            .iter()
            .map(|(id, version)| {
                let mut request = GetObjectRequest::default();
                request.object_id = Some(id.to_string());
                request.version = version.map(|v| v.value());
                request
            })
            .collect();
        self.read("BatchGetObjects", |mut client| {
            let mut request = BatchGetObjectsRequest::default();
            request.requests = requests.clone();
            request.read_mask = Some(FieldMask::from_paths(["object_id", "version", "digest"]));
            async move {
                let response = client
                    .ledger_client()
                    .batch_get_objects(request)
                    .await?
                    .into_inner();
                if response.objects.len() != keys.len() {
                    return Err(tonic::Status::internal(format!(
                        "returned {} objects for {} requested",
                        response.objects.len(),
                        keys.len()
                    )));
                }
                response.objects.iter().map(object_read).collect()
            }
        })
        .await
    }

    /// One object's `owner` and `object_type`, or `None` if the endpoint holds
    /// no such object.
    pub async fn object_owner_and_type(&self, id: ObjectID) -> Result<Option<Object>, String> {
        self.read("GetObject", |mut client| {
            let mut request = GetObjectRequest::default();
            request.object_id = Some(id.to_string());
            request.read_mask = Some(FieldMask::from_paths(["object_id", "owner", "object_type"]));
            async move {
                match client.ledger_client().get_object(request).await {
                    Ok(response) => Ok(response.into_inner().object),
                    Err(status) if status.code() == Code::NotFound => Ok(None),
                    Err(status) => Err(status),
                }
            }
        })
        .await
    }

    /// What `owner` holds of `coin_type` in coin objects. A node without the
    /// balance index answers `NotFound`, which is that node's problem: the
    /// next endpoint is asked.
    pub async fn coin_balance(&self, owner: &SuiAddress, coin_type: &str) -> Result<u64, String> {
        self.read("GetBalance", |mut client| {
            let mut request = GetBalanceRequest::default();
            request.owner = Some(owner.to_string());
            request.coin_type = Some(coin_type.to_string());
            async move {
                let response = client
                    .state_client()
                    .get_balance(request)
                    .await?
                    .into_inner();
                response
                    .balance
                    .as_ref()
                    .and_then(coin_balance_of)
                    .ok_or_else(|| tonic::Status::internal("answered without a balance"))
            }
        })
        .await
    }

    async fn lookup(&self, endpoint: &Endpoint, digest: &str) -> Lookup {
        let mut client = endpoint.client.clone();
        let mut request = GetTransactionRequest::default();
        request.digest = Some(digest.to_string());
        request.read_mask = Some(FieldMask::from_paths(["digest", "effects.status"]));
        match tokio::time::timeout(
            self.read_timeout,
            client.ledger_client().get_transaction(request),
        )
        .await
        {
            Ok(Ok(response)) => match response.into_inner().transaction {
                Some(transaction)
                    if transaction
                        .digest
                        .as_deref()
                        .is_some_and(|answered| answered != digest) =>
                {
                    Lookup::Malformed(format!(
                        "answered for transaction {}, not {digest}",
                        transaction.digest.as_deref().unwrap_or_default()
                    ))
                }
                Some(transaction) => Lookup::Found(effects_verdict(&transaction, digest)),
                None => Lookup::Malformed("answer carries no transaction".to_string()),
            },
            Ok(Err(status)) if status.code() == Code::NotFound => Lookup::NotFound,
            Ok(Err(status)) => Lookup::Unreadable(describe(&status)),
            Err(_) => Lookup::Unreadable(format!("timed out after {:?}", self.read_timeout)),
        }
    }

    /// Execute a sponsored transaction (its BCS `TransactionData` and the
    /// sender's and sponsor's signatures, in that order). See the module docs
    /// for which failures move on, which are final and which leave the outcome
    /// unknown.
    pub async fn execute(
        &self,
        transaction_bcs: Vec<u8>,
        signatures: Vec<Vec<u8>>,
        digest: &str,
    ) -> Execution {
        let endpoints = self.endpoints();
        if endpoints.is_empty() {
            return Execution::Unreachable("no usable Sui RPC endpoint".to_string());
        }
        let mut transaction = Transaction::default();
        let mut bcs = Bcs::default();
        bcs.name = Some("TransactionData".to_string());
        bcs.value = Some(transaction_bcs.into());
        transaction.bcs = Some(bcs);
        let signatures: Vec<UserSignature> = signatures
            .into_iter()
            .map(|bytes| {
                let mut bcs = Bcs::default();
                bcs.name = Some("UserSignature".to_string());
                bcs.value = Some(bytes.into());
                let mut signature = UserSignature::default();
                signature.bcs = Some(bcs);
                signature
            })
            .collect();

        let start = self.start(endpoints.len());
        // Why the outcome became unknown, once it did.
        let mut unknown: Option<String> = None;
        let mut not_sent = Vec::new();
        // When the first ExecuteTransaction went out, if one did.
        let mut first_send: Option<std::time::Instant> = None;
        for step in 0..endpoints.len() {
            let index = (start + step) % endpoints.len();
            let endpoint = &endpoints[index];

            match self.lookup(endpoint, digest).await {
                // Known to the chain before this call sent it anywhere, with
                // or without a verdict: an earlier call's execution, not this
                // one's. Nothing is sent, here or anywhere else.
                Lookup::Found(verdict) if first_send.is_none() => {
                    self.prefer(index);
                    info!(network = %self.network, endpoint = %endpoint.label, %digest, verdict = ?verdict, "Sui transaction already executed before this call; not sending it");
                    return Execution::AlreadyExecuted;
                }
                Lookup::Found(Ok(failure)) => {
                    self.prefer(index);
                    info!(network = %self.network, endpoint = %endpoint.label, %digest, "Sui transaction executed by this call's send; not sending it again");
                    return Execution::Executed { failure };
                }
                Lookup::Found(Err(why)) => {
                    // It executed, but this answer has no verdict: nothing is
                    // sent here, and from now on the answer can only be the
                    // effects an endpoint reports, or Unconfirmed.
                    unknown.get_or_insert(format!("{}: {why}", endpoint.name));
                    continue;
                }
                // Before any send, an answer about another transaction (or
                // none) says nothing about this one: this endpoint is skipped,
                // as one that did not answer.
                Lookup::Malformed(why) if first_send.is_none() => {
                    warn!(network = %self.network, endpoint = %endpoint.label, %digest, error = %why, "Sui digest lookup answered about another transaction; nothing sent to this endpoint");
                    not_sent.push(format!("{}: {why}", endpoint.name));
                    continue;
                }
                // After a send it may still hide that send's outcome.
                Lookup::Malformed(why) => {
                    unknown.get_or_insert(format!("{}: {why}", endpoint.name));
                    continue;
                }
                Lookup::NotFound => {}
                Lookup::Unreadable(why) => {
                    warn!(network = %self.network, endpoint = %endpoint.label, %digest, error = %why, "Sui digest lookup failed; nothing sent to this endpoint");
                    not_sent.push(format!("{}: {why}", endpoint.name));
                    continue;
                }
            }

            // A settle is bounded: once a whole execution timeout has passed
            // since the first send, the endpoints are asked, not sent to.
            if first_send.is_some_and(|sent| sent.elapsed() >= self.execute_timeout) {
                warn!(network = %self.network, endpoint = %endpoint.label, %digest, "Sui settle out of time; asking the endpoints instead of sending again");
                break;
            }

            let mut request = ExecuteTransactionRequest::default();
            request.transaction = Some(transaction.clone());
            request.signatures = signatures.clone();
            request.read_mask = Some(FieldMask::from_paths(["digest", "effects.status"]));
            let mut client = endpoint.client.clone();
            first_send.get_or_insert_with(std::time::Instant::now);
            let outcome = tokio::time::timeout(
                self.execute_timeout,
                client.execution_client().execute_transaction(request),
            )
            .await;
            match outcome {
                Ok(Ok(response)) => match response.into_inner().transaction {
                    Some(executed) => match effects_verdict(&executed, digest) {
                        Ok(failure) => {
                            self.prefer(index);
                            return Execution::Executed { failure };
                        }
                        Err(why) => {
                            warn!(network = %self.network, endpoint = %endpoint.label, %digest, error = %why, "Sui execution answered without a verdict; outcome unknown");
                            unknown.get_or_insert(format!("{}: {why}", endpoint.name));
                        }
                    },
                    None => {
                        warn!(network = %self.network, endpoint = %endpoint.label, %digest, "Sui execution answered without a transaction; outcome unknown");
                        unknown.get_or_insert(format!(
                            "{}: answer carries no transaction",
                            endpoint.name
                        ));
                    }
                },
                Ok(Err(status)) => match classify_execution_error(&status) {
                    Sent::Refused if unknown.is_none() => {
                        return Execution::Refused(crate::redact::scrub_urls(status.message()));
                    }
                    // After an unknown outcome a refusal may be the first send
                    // having landed; only an endpoint's effects can say.
                    Sent::Refused | Sent::Unknown => {
                        warn!(network = %self.network, endpoint = %endpoint.label, %digest, error = %describe(&status), "Sui execution outcome unknown on this endpoint");
                        unknown.get_or_insert(format!("{}: {}", endpoint.name, describe(&status)));
                    }
                    Sent::NotSent => {
                        warn!(network = %self.network, endpoint = %endpoint.label, %digest, error = %describe(&status), "Sui endpoint did not take the transaction");
                        not_sent.push(format!("{}: {}", endpoint.name, describe(&status)));
                    }
                },
                Err(_) => {
                    warn!(network = %self.network, endpoint = %endpoint.label, %digest, "Sui execution timed out; outcome unknown");
                    unknown.get_or_insert(format!(
                        "{}: no answer after {:?}",
                        endpoint.name, self.execute_timeout
                    ));
                }
            }
        }

        if unknown.is_none() && first_send.is_none() {
            // Nothing went out anywhere.
            return Execution::Unreachable(not_sent.join("; "));
        }
        // The last word: did any endpoint see it land? Also after door
        // refusals, in case one came from a proxy that had forwarded it.
        for (index, endpoint) in endpoints.iter().enumerate() {
            if let Lookup::Found(Ok(failure)) = self.lookup(endpoint, digest).await {
                self.prefer(index);
                return Execution::Executed { failure };
            }
        }
        match unknown {
            Some(why) => Execution::Unconfirmed(why),
            None => Execution::Unreachable(not_sent.join("; ")),
        }
    }
}

fn object_read(
    result: &sui_rpc::proto::sui::rpc::v2::GetObjectResult,
) -> Result<ObjectRead, tonic::Status> {
    match &result.result {
        Some(get_object_result::Result::Object(object)) => {
            let version = object
                .version
                .ok_or_else(|| tonic::Status::internal("object without a version"))?;
            let digest = object
                .digest
                .as_deref()
                .ok_or_else(|| tonic::Status::internal("object without a digest"))
                .and_then(|digest| {
                    ObjectDigest::from_str(digest)
                        .map_err(|e| tonic::Status::internal(format!("unreadable digest: {e}")))
                })?;
            Ok(ObjectRead::Found {
                version: SequenceNumber::from_u64(version),
                digest,
            })
        }
        Some(get_object_result::Result::Error(status)) if status.code == Code::NotFound as i32 => {
            Ok(ObjectRead::NotFound)
        }
        Some(get_object_result::Result::Error(status)) => Err(tonic::Status::new(
            Code::from_i32(status.code),
            status.message.clone(),
        )),
        _ => Err(tonic::Status::internal("empty object result")),
    }
}

#[cfg(test)]
mod tests {
    use super::super::stub::{self, Node, Reply};
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;
    use sui_rpc::proto::sui::rpc::v2::{
        execution_error, ExecuteTransactionResponse, ExecutionError, ExecutionStatus,
        GetTransactionResponse, Owner, TransactionEffects,
    };
    use sui_types::digests::TransactionDigest;

    fn digest() -> String {
        TransactionDigest::new([7u8; 32]).to_string()
    }

    fn rpc(urls: Vec<String>) -> SuiRpc {
        SuiRpc::new(
            Network::SuiTestnet,
            urls,
            Duration::from_secs(2),
            Duration::from_secs(2),
        )
    }

    fn executed(digest: &str, success: Option<bool>) -> ExecutedTransaction {
        let mut status = ExecutionStatus::default();
        status.success = success;
        if success == Some(false) {
            let mut error = ExecutionError::default();
            error.description = Some("InsufficientCoinBalance in command 0".to_string());
            error.kind = Some(execution_error::ExecutionErrorKind::InsufficientCoinBalance as i32);
            status.error = Some(error);
        }
        let mut effects = TransactionEffects::default();
        effects.status = Some(status);
        let mut transaction = ExecutedTransaction::default();
        transaction.digest = Some(digest.to_string());
        transaction.effects = Some(effects);
        transaction
    }

    fn found(digest: &str, success: bool) -> Reply<GetTransactionResponse> {
        let mut response = GetTransactionResponse::default();
        response.transaction = Some(executed(digest, Some(success)));
        Reply::Answer(Ok(response))
    }

    fn not_found() -> Reply<GetTransactionResponse> {
        Reply::Answer(Err(tonic::Status::not_found("Transaction not found")))
    }

    fn effects(digest: &str, success: bool) -> Reply<ExecuteTransactionResponse> {
        let mut response = ExecuteTransactionResponse::default();
        response.transaction = Some(executed(digest, Some(success)));
        Reply::Answer(Ok(response))
    }

    /// A node that does not know the transaction and executes it.
    fn executes(success: bool) -> Node {
        let mut node = Node::default();
        node.get_transaction = Box::new(|_| not_found());
        let d = digest();
        node.execute = Box::new(move |_| effects(&d, success));
        node
    }

    /// A node that does not know the transaction and answers its execution with `status`.
    fn execution_fails(status: fn() -> tonic::Status) -> Node {
        let mut node = Node::default();
        node.get_transaction = Box::new(|_| not_found());
        node.execute = Box::new(move |_| Reply::Answer(Err(status())));
        node
    }

    async fn run(rpc: &SuiRpc) -> Execution {
        rpc.execute(vec![1, 2, 3], vec![vec![4], vec![5]], &digest())
            .await
    }

    // ---------------------------------------------------------------- endpoints

    #[test]
    fn an_unset_or_blank_variable_gives_the_public_defaults() {
        let mainnet: Vec<String> = DEFAULT_ENDPOINTS_MAINNET
            .iter()
            .map(|u| u.to_string())
            .collect();
        let testnet: Vec<String> = DEFAULT_ENDPOINTS_TESTNET
            .iter()
            .map(|u| u.to_string())
            .collect();
        for raw in [None, Some(""), Some("   "), Some(" , ,, ")] {
            assert_eq!(endpoint_list(raw, Network::Sui), mainnet, "{raw:?}");
            assert_eq!(endpoint_list(raw, Network::SuiTestnet), testnet, "{raw:?}");
        }
        assert!(mainnet.len() >= 2 && testnet.len() >= 2);
        assert_ne!(mainnet, testnet);
    }

    #[test]
    fn a_comma_separated_list_is_kept_in_order_trimmed_and_without_repeats() {
        let raw =
            " https://a.example:443 ,https://b.example/, HTTPS://A.EXAMPLE:443/ ,http://c.example ";
        assert_eq!(
            endpoint_list(Some(raw), Network::Sui),
            vec![
                "https://a.example:443".to_string(),
                "https://b.example/".to_string(),
                "http://c.example".to_string(),
            ]
        );
        assert_eq!(
            endpoint_list(Some("https://only.example"), Network::Sui),
            vec!["https://only.example".to_string()]
        );
    }

    #[test]
    fn an_entry_that_is_not_an_http_url_is_skipped_and_garbage_alone_gives_the_defaults() {
        assert_eq!(
            endpoint_list(
                Some("wss://ws.example, not a url, https://ok.example, ftp://x.example, https://"),
                Network::SuiTestnet
            ),
            vec!["https://ok.example".to_string()]
        );
        let defaults: Vec<String> = DEFAULT_ENDPOINTS_TESTNET
            .iter()
            .map(|u| u.to_string())
            .collect();
        assert_eq!(
            endpoint_list(
                Some("sui-rpc.publicnode.com, grpc://x"),
                Network::SuiTestnet
            ),
            defaults
        );
    }

    /// The production values are the defaults, in the task definition, the
    /// balances Lambda and `.env.example`: one list, typed once in the code.
    #[test]
    fn every_configured_sui_endpoint_list_is_the_default_one() {
        let root = env!("CARGO_MANIFEST_DIR");
        let mainnet = DEFAULT_ENDPOINTS_MAINNET.join(",");
        let testnet = DEFAULT_ENDPOINTS_TESTNET.join(",");
        let read = |path: &str| {
            std::fs::read_to_string(format!("{root}/{path}"))
                .unwrap_or_else(|e| panic!("{path}: {e}"))
                .replace("\r\n", "\n")
        };
        // main.tf: `name = "VAR"` then, past comment lines, `value = "..."`.
        let main_tf = read("terraform/environments/production/main.tf");
        let task_value = |var: &str| -> String {
            let pattern = format!(
                "name\\s*=\\s*\"{var}\"\\s*\\n(?:\\s*#[^\\n]*\\n)*\\s*value\\s*=\\s*\"([^\"]*)\""
            );
            regex::Regex::new(&pattern)
                .unwrap()
                .captures(&main_tf)
                .unwrap_or_else(|| panic!("{var} not found in main.tf"))[1]
                .to_string()
        };
        assert_eq!(task_value("RPC_URL_SUI"), mainnet);
        assert_eq!(task_value("RPC_URL_SUI_TESTNET"), testnet);

        let lambda_tf = read("terraform/environments/production/lambda-balances.tf");
        let lambda_value = |var: &str| -> String {
            let pattern = format!("(?m)^\\s*{var}\\s*=\\s*\"([^\"]*)\"");
            regex::Regex::new(&pattern)
                .unwrap()
                .captures(&lambda_tf)
                .unwrap_or_else(|| panic!("{var} not found in lambda-balances.tf"))[1]
                .to_string()
        };
        assert_eq!(lambda_value("RPC_URL_SUI"), mainnet);
        assert_eq!(lambda_value("RPC_URL_SUI_TESTNET"), testnet);

        let env_example = read(".env.example");
        let example_value = |var: &str| -> String {
            let pattern = format!("(?m)^{var}=(.*)$");
            regex::Regex::new(&pattern)
                .unwrap()
                .captures(&env_example)
                .unwrap_or_else(|| panic!("{var} not found in .env.example"))[1]
                .trim()
                .to_string()
        };
        assert_eq!(example_value("RPC_URL_SUI"), mainnet);
        assert_eq!(example_value("RPC_URL_SUI_TESTNET"), testnet);

        // The Lambda's own fallback when its variables are unset.
        let handler = read("lambda/balances/handler.py");
        let handler_default = |network: &str| -> String {
            let pattern = format!("\"{network}\":\\s*\"([^\"]*)\"");
            regex::Regex::new(&pattern)
                .unwrap()
                .captures(&handler)
                .unwrap_or_else(|| panic!("SUI_GRPC_DEFAULTS[{network}] not found in handler.py"))
                [1]
            .to_string()
        };
        assert_eq!(handler_default("sui-mainnet"), mainnet);
        assert_eq!(handler_default("sui-testnet"), testnet);
    }

    // ----------------------------------------------------------- classification

    #[test]
    fn only_a_chain_verdict_is_a_refusal_and_only_a_closed_door_is_not_sent() {
        for (code, expected) in [
            (Code::InvalidArgument, Sent::Refused),
            (Code::FailedPrecondition, Sent::Refused),
            (Code::Unimplemented, Sent::NotSent),
            (Code::PermissionDenied, Sent::NotSent),
            (Code::Unauthenticated, Sent::NotSent),
            (Code::Unavailable, Sent::Unknown),
            (Code::DeadlineExceeded, Sent::Unknown),
            (Code::Cancelled, Sent::Unknown),
            (Code::Internal, Sent::Unknown),
            (Code::Unknown, Sent::Unknown),
            (Code::Aborted, Sent::Unknown),
            (Code::ResourceExhausted, Sent::Unknown),
            (Code::NotFound, Sent::Unknown),
            (Code::DataLoss, Sent::Unknown),
            (Code::AlreadyExists, Sent::Unknown),
            (Code::OutOfRange, Sent::Unknown),
            (Code::Ok, Sent::Unknown),
        ] {
            assert_eq!(
                classify_execution_error(&tonic::Status::new(code, "x")),
                expected,
                "{code:?}"
            );
        }
    }

    #[test]
    fn the_verdict_is_read_from_the_effects_status_of_this_digest() {
        let d = digest();
        assert_eq!(effects_verdict(&executed(&d, Some(true)), &d), Ok(None));
        let failed = effects_verdict(&executed(&d, Some(false)), &d).unwrap();
        assert!(
            failed
                .as_deref()
                .is_some_and(|f| f.contains("InsufficientCoinBalance")),
            "{failed:?}"
        );
        assert!(effects_verdict(&executed(&d, None), &d).is_err());
        assert!(effects_verdict(&ExecutedTransaction::default(), &d).is_err());
        let other = TransactionDigest::new([8u8; 32]).to_string();
        assert!(effects_verdict(&executed(&other, Some(true)), &d).is_err());
        // A failure with no description still says it failed.
        let mut bare = executed(&d, Some(false));
        bare.effects
            .as_mut()
            .unwrap()
            .status
            .as_mut()
            .unwrap()
            .error = None;
        assert_eq!(
            effects_verdict(&bare, &d),
            Ok(Some("execution failed".to_string()))
        );
    }

    #[test]
    fn the_coin_balance_leaves_out_the_address_balance() {
        let mut balance = sui_rpc::proto::sui::rpc::v2::Balance::default();
        balance.balance = Some(5_000_000);
        assert_eq!(coin_balance_of(&balance), Some(5_000_000));
        balance.address_balance = Some(4_500_000);
        balance.coin_balance = Some(500_000);
        assert_eq!(coin_balance_of(&balance), Some(500_000));
        balance.address_balance = Some(9_000_000);
        assert_eq!(coin_balance_of(&balance), Some(0));
        balance.balance = None;
        assert_eq!(coin_balance_of(&balance), None);
    }

    #[test]
    fn a_usdc_coin_is_matched_by_its_parsed_type_and_its_address_owner() {
        let usdc = crate::chain::sui::USDC_COIN_TYPE_TESTNET;
        let owner = SuiAddress::from(ObjectID::new([0xdd; 32]));
        let coin = |kind: OwnerKind, address: SuiAddress, object_type: String| {
            let mut o = Owner::default();
            o.kind = Some(kind as i32);
            o.address = Some(address.to_string());
            let mut object = Object::default();
            object.owner = Some(o);
            object.object_type = Some(object_type);
            object
        };
        let short = format!("0x2::coin::Coin<{usdc}>");
        // As a node renders it: every address at full length.
        let long = format!("{}::coin::Coin<{usdc}>", ObjectID::from_single_byte(2));
        assert!(is_coin_owned_by(
            &coin(OwnerKind::Address, owner, short.clone()),
            usdc,
            &owner
        ));
        assert!(is_coin_owned_by(
            &coin(OwnerKind::Address, owner, long),
            usdc,
            &owner
        ));
        let other = SuiAddress::from(ObjectID::new([0xcc; 32]));
        assert!(!is_coin_owned_by(
            &coin(OwnerKind::Address, other, short.clone()),
            usdc,
            &owner
        ));
        assert!(!is_coin_owned_by(
            &coin(OwnerKind::Object, owner, short.clone()),
            usdc,
            &owner
        ));
        assert!(!is_coin_owned_by(
            &coin(OwnerKind::ConsensusAddress, owner, short.clone()),
            usdc,
            &owner
        ));
        assert!(!is_coin_owned_by(
            &coin(
                OwnerKind::Address,
                owner,
                "0x2::coin::Coin<0x2::sui::SUI>".to_string()
            ),
            usdc,
            &owner
        ));
        assert!(!is_coin_owned_by(
            &coin(OwnerKind::Address, owner, usdc.to_string()),
            usdc,
            &owner
        ));
        assert!(!is_coin_owned_by(&Object::default(), usdc, &owner));
    }

    // -------------------------------------------------------------------- reads

    fn balance_node(balance: u64) -> Node {
        let mut node = Node::default();
        node.get_balance = Box::new(move |_| {
            let mut b = sui_rpc::proto::sui::rpc::v2::Balance::default();
            b.balance = Some(balance);
            let mut response = sui_rpc::proto::sui::rpc::v2::GetBalanceResponse::default();
            response.balance = Some(b);
            Reply::Answer(Ok(response))
        });
        node
    }

    fn owner() -> SuiAddress {
        SuiAddress::from(ObjectID::new([0xdd; 32]))
    }

    #[tokio::test]
    async fn a_read_moves_past_a_dead_endpoint_and_then_starts_at_the_live_one() {
        let (live, node) = stub::serve(balance_node(42)).await;
        let rpc = rpc(vec![stub::closed_endpoint().await, live]);
        assert_eq!(rpc.coin_balance(&owner(), "0x2::sui::SUI").await, Ok(42));
        assert_eq!(rpc.coin_balance(&owner(), "0x2::sui::SUI").await, Ok(42));
        assert_eq!(node.count("GetBalance"), 2);
        assert_eq!(rpc.preferred.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn a_read_moves_past_an_endpoint_that_answers_with_an_error() {
        let mut failing = Node::default();
        failing.get_balance =
            Box::new(|_| Reply::Answer(Err(tonic::Status::not_found("no index"))));
        let (first, first_node) = stub::serve(failing).await;
        let (second, _) = stub::serve(balance_node(7)).await;
        let rpc = rpc(vec![first, second]);
        assert_eq!(rpc.coin_balance(&owner(), "0x2::sui::SUI").await, Ok(7));
        assert_eq!(first_node.count("GetBalance"), 1);
    }

    #[tokio::test]
    async fn a_read_that_no_endpoint_answers_names_every_endpoint() {
        let mut hangs = Node::default();
        hangs.get_balance = Box::new(|_| Reply::Hang);
        let (hung, _) = stub::serve(hangs).await;
        let closed = stub::closed_endpoint().await;
        let rpc = SuiRpc::new(
            Network::SuiTestnet,
            vec![closed.clone(), hung.clone()],
            Duration::from_millis(300),
            Duration::from_secs(1),
        );
        let started = std::time::Instant::now();
        let err = rpc
            .coin_balance(&owner(), "0x2::sui::SUI")
            .await
            .unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        // By position: an error reaches the HTTP caller, a host must not.
        assert!(
            err.contains("endpoint 1: ") && err.contains("endpoint 2: "),
            "{err}"
        );
        for url in [&closed, &hung] {
            let host = url::Url::parse(url)
                .unwrap()
                .host_str()
                .unwrap()
                .to_string();
            assert!(!err.contains(&host), "{host} in {err}");
        }
        assert!(err.contains("timed out"), "{err}");
    }

    #[tokio::test]
    async fn a_missing_object_is_an_answer_not_a_failure() {
        let mut node = Node::default();
        node.get_object =
            Box::new(|_| Reply::Answer(Err(tonic::Status::not_found("Object not found"))));
        let (url, node) = stub::serve(node).await;
        let rpc = rpc(vec![url]);
        assert_eq!(
            rpc.object_owner_and_type(ObjectID::new([1; 32])).await,
            Ok(None)
        );
        assert_eq!(node.count("GetObject"), 1);
    }

    #[tokio::test]
    async fn a_batch_that_answers_for_the_wrong_number_of_objects_is_asked_of_the_next_endpoint() {
        let mut short = Node::default();
        short.batch_get_objects = Box::new(|_| {
            Reply::Answer(Ok(
                sui_rpc::proto::sui::rpc::v2::BatchGetObjectsResponse::default(),
            ))
        });
        let (first, _) = stub::serve(short).await;
        let mut right = Node::default();
        right.batch_get_objects = Box::new(|request| {
            let mut response = sui_rpc::proto::sui::rpc::v2::BatchGetObjectsResponse::default();
            response.objects = request
                .requests
                .iter()
                .map(|_| {
                    let mut status = sui_rpc::proto::google::rpc::Status::default();
                    status.code = Code::NotFound as i32;
                    let mut result = sui_rpc::proto::sui::rpc::v2::GetObjectResult::default();
                    result.result = Some(get_object_result::Result::Error(status));
                    result
                })
                .collect();
            Reply::Answer(Ok(response))
        });
        let (second, _) = stub::serve(right).await;
        let rpc = rpc(vec![first, second]);
        assert_eq!(
            rpc.objects(&[(ObjectID::new([1; 32]), None)]).await,
            Ok(vec![ObjectRead::NotFound])
        );
    }

    // --------------------------------------------------------------- execution

    #[tokio::test]
    async fn a_refusal_is_final_and_never_reaches_the_next_endpoint() {
        let (first, first_node) = stub::serve(execution_fails(|| {
            tonic::Status::invalid_argument("Invalid user signature: bad signature")
        }))
        .await;
        let (second, second_node) = stub::serve(executes(true)).await;
        let rpc = rpc(vec![first, second]);
        match run(&rpc).await {
            Execution::Refused(reason) => {
                assert!(reason.contains("Invalid user signature"), "{reason}")
            }
            other => panic!("expected the refusal, got {other:?}"),
        }
        assert_eq!(first_node.count("ExecuteTransaction"), 1);
        assert!(second_node.calls().is_empty(), "{:?}", second_node.calls());
    }

    #[tokio::test]
    async fn locked_objects_are_a_refusal_too() {
        let (url, _) = stub::serve(execution_fails(|| {
            tonic::Status::failed_precondition(
                "Failed to sign transaction by a quorum of validators because of locked objects",
            )
        }))
        .await;
        assert!(matches!(run(&rpc(vec![url])).await, Execution::Refused(_)));
    }

    #[tokio::test]
    async fn a_dead_endpoint_gets_nothing_and_the_next_one_executes() {
        let (live, node) = stub::serve(executes(true)).await;
        let rpc = rpc(vec![stub::closed_endpoint().await, live]);
        assert_eq!(run(&rpc).await, Execution::Executed { failure: None });
        assert_eq!(node.calls(), vec!["GetTransaction", "ExecuteTransaction"]);
        let sent = &node.executions()[0];
        assert_eq!(
            sent.transaction
                .as_ref()
                .unwrap()
                .bcs
                .as_ref()
                .unwrap()
                .value
                .as_deref(),
            Some(&[1u8, 2, 3][..])
        );
        let signatures: Vec<_> = sent
            .signatures
            .iter()
            .map(|s| s.bcs.as_ref().unwrap().value.clone().unwrap().to_vec())
            .collect();
        assert_eq!(signatures, vec![vec![4u8], vec![5u8]]);
    }

    #[tokio::test]
    async fn an_endpoint_that_does_not_serve_execution_passes_it_on() {
        let (first, first_node) = stub::serve(execution_fails(|| {
            tonic::Status::unimplemented("no transaction executor")
        }))
        .await;
        let (second, second_node) = stub::serve(executes(true)).await;
        let rpc = rpc(vec![first, second]);
        assert_eq!(run(&rpc).await, Execution::Executed { failure: None });
        assert_eq!(first_node.count("ExecuteTransaction"), 1);
        assert_eq!(second_node.count("ExecuteTransaction"), 1);
    }

    #[tokio::test]
    async fn an_unknown_outcome_is_looked_up_before_any_resend() {
        let (first, first_node) = stub::serve(execution_fails(|| {
            tonic::Status::unavailable("timed-out before finality could be reached")
        }))
        .await;
        // The second endpoint already holds the transaction: nothing is resent.
        let mut second = Node::default();
        let d = digest();
        second.get_transaction = Box::new(move |_| found(&d, true));
        let (second, second_node) = stub::serve(second).await;
        let rpc = rpc(vec![first, second]);
        assert_eq!(run(&rpc).await, Execution::Executed { failure: None });
        assert_eq!(first_node.count("ExecuteTransaction"), 1);
        assert_eq!(second_node.calls(), vec!["GetTransaction"]);
    }

    #[tokio::test]
    async fn an_unknown_outcome_not_found_anywhere_is_resent_as_the_same_bytes() {
        let (first, first_node) = stub::serve(execution_fails(|| {
            tonic::Status::internal("connection reset")
        }))
        .await;
        let (second, second_node) = stub::serve(executes(true)).await;
        let rpc = rpc(vec![first, second]);
        assert_eq!(run(&rpc).await, Execution::Executed { failure: None });
        assert_eq!(
            second_node.calls(),
            vec!["GetTransaction", "ExecuteTransaction"]
        );
        assert_eq!(first_node.executions(), second_node.executions());
    }

    #[tokio::test]
    async fn an_outcome_unknown_everywhere_is_unconfirmed_after_a_last_lookup() {
        let (first, first_node) = stub::serve(execution_fails(|| {
            tonic::Status::unavailable("system is overloaded")
        }))
        .await;
        let (second, second_node) = stub::serve(execution_fails(|| {
            tonic::Status::deadline_exceeded("deadline")
        }))
        .await;
        let rpc = rpc(vec![first, second]);
        assert!(matches!(run(&rpc).await, Execution::Unconfirmed(_)));
        // Lookup, execute, and the last lookup on each.
        assert_eq!(first_node.count("GetTransaction"), 2);
        assert_eq!(second_node.count("GetTransaction"), 2);
        assert_eq!(first_node.count("ExecuteTransaction"), 1);
        assert_eq!(second_node.count("ExecuteTransaction"), 1);
    }

    #[tokio::test]
    async fn after_an_unknown_outcome_a_refusal_is_not_believed() {
        let (first, _) = stub::serve(execution_fails(|| {
            tonic::Status::unavailable("timed-out before finality could be reached")
        }))
        .await;
        let (second, _) = stub::serve(execution_fails(|| {
            tonic::Status::invalid_argument("Object version unavailable for consumption")
        }))
        .await;
        assert!(matches!(
            run(&rpc(vec![first, second])).await,
            Execution::Unconfirmed(_)
        ));
    }

    #[tokio::test]
    async fn the_last_lookup_finds_a_transaction_that_landed_meanwhile() {
        let mut node = Node::default();
        let lookups = Arc::new(AtomicUsize::new(0));
        let seen = lookups.clone();
        let d = digest();
        node.get_transaction = Box::new(move |_| {
            if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                not_found()
            } else {
                found(&d, true)
            }
        });
        node.execute = Box::new(|_| {
            Reply::Answer(Err(tonic::Status::aborted(
                "The transaction is already finalized but with different user signatures",
            )))
        });
        let (url, _) = stub::serve(node).await;
        assert_eq!(
            run(&rpc(vec![url])).await,
            Execution::Executed { failure: None }
        );
        assert_eq!(lookups.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn an_execution_that_never_answers_is_unconfirmed_at_its_timeout() {
        let mut node = Node::default();
        node.get_transaction = Box::new(|_| not_found());
        node.execute = Box::new(|_| Reply::Hang);
        let (url, _) = stub::serve(node).await;
        let rpc = SuiRpc::new(
            Network::SuiTestnet,
            vec![url],
            Duration::from_secs(2),
            Duration::from_millis(300),
        );
        let outcome = tokio::time::timeout(Duration::from_secs(10), run(&rpc))
            .await
            .expect("the execution must end on its own timeout");
        assert!(matches!(outcome, Execution::Unconfirmed(_)), "{outcome:?}");
    }

    #[tokio::test]
    async fn an_already_executed_transaction_is_not_sent_again() {
        let mut node = Node::default();
        let d = digest();
        node.get_transaction = Box::new(move |_| found(&d, false));
        let (url, node) = stub::serve(node).await;
        assert_eq!(run(&rpc(vec![url])).await, Execution::AlreadyExecuted);
        assert_eq!(node.calls(), vec!["GetTransaction"]);
    }

    /// A transaction that executed before this call is not this call's
    /// execution, even when it succeeded: the answer is never `Executed`.
    #[tokio::test]
    async fn a_transaction_executed_before_this_call_is_not_its_execution() {
        let mut node = Node::default();
        let d = digest();
        node.get_transaction = Box::new(move |_| found(&d, true));
        let (url, node) = stub::serve(node).await;
        assert_eq!(run(&rpc(vec![url])).await, Execution::AlreadyExecuted);
        assert_eq!(node.calls(), vec!["GetTransaction"]);
    }

    /// "Before this call" is before any send, not "on the first endpoint": a
    /// first endpoint that cannot be asked sends nothing, so the second one
    /// holding the transaction still means an earlier call executed it.
    #[tokio::test]
    async fn found_on_the_next_endpoint_before_any_send_is_an_earlier_execution() {
        let mut unreadable = Node::default();
        unreadable.get_transaction =
            Box::new(|_| Reply::Answer(Err(tonic::Status::internal("down"))));
        let (first, first_node) = stub::serve(unreadable).await;
        let mut holder = Node::default();
        let d = digest();
        holder.get_transaction = Box::new(move |_| found(&d, true));
        let (second, second_node) = stub::serve(holder).await;
        assert_eq!(
            run(&rpc(vec![first, second])).await,
            Execution::AlreadyExecuted
        );
        assert_eq!(first_node.count("ExecuteTransaction"), 0);
        assert_eq!(second_node.calls(), vec!["GetTransaction"]);
    }

    /// Known without a verdict before any send is known all the same: nothing
    /// is sent anywhere, and a verdict found later does not make it this
    /// call's execution.
    #[tokio::test]
    async fn a_transaction_known_before_any_send_is_never_this_calls_execution() {
        let mut node = Node::default();
        let lookups = Arc::new(AtomicUsize::new(0));
        let seen = lookups.clone();
        let d = digest();
        node.get_transaction = Box::new(move |_| {
            if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                let mut response = GetTransactionResponse::default();
                response.transaction = Some(executed(&d, None));
                Reply::Answer(Ok(response))
            } else {
                found(&d, true)
            }
        });
        let (url, node) = stub::serve(node).await;
        assert_eq!(run(&rpc(vec![url])).await, Execution::AlreadyExecuted);
        assert_eq!(node.count("ExecuteTransaction"), 0);
    }

    /// Before any send, an answer about another transaction or about none is
    /// not evidence: that endpoint is skipped and the next one takes it.
    #[tokio::test]
    async fn an_answer_about_another_transaction_before_any_send_is_skipped() {
        for answer in [Some(TransactionDigest::new([8u8; 32]).to_string()), None] {
            let mut odd = Node::default();
            odd.get_transaction = Box::new(move |_| {
                let mut response = GetTransactionResponse::default();
                response.transaction = answer.as_deref().map(|other| executed(other, Some(true)));
                Reply::Answer(Ok(response))
            });
            let (first, first_node) = stub::serve(odd).await;
            let (second, second_node) = stub::serve(executes(true)).await;
            assert_eq!(
                run(&rpc(vec![first, second])).await,
                Execution::Executed { failure: None }
            );
            assert_eq!(first_node.count("ExecuteTransaction"), 0);
            assert_eq!(second_node.count("ExecuteTransaction"), 1);
        }
    }

    /// The only endpoint answering about another transaction: nothing was
    /// sent anywhere, so the answer is "nothing sent", not "unknown".
    #[tokio::test]
    async fn an_answer_about_another_transaction_alone_sends_nothing() {
        let mut odd = Node::default();
        odd.get_transaction = Box::new(|_| {
            let mut response = GetTransactionResponse::default();
            response.transaction = Some(executed(
                &TransactionDigest::new([8u8; 32]).to_string(),
                Some(true),
            ));
            Reply::Answer(Ok(response))
        });
        let (url, node) = stub::serve(odd).await;
        assert!(matches!(
            run(&rpc(vec![url])).await,
            Execution::Unreachable(_)
        ));
        assert_eq!(node.count("ExecuteTransaction"), 0);
    }

    /// A door refusal is a send for what follows: the next endpoint holding
    /// the transaction may be holding that very send.
    #[tokio::test]
    async fn found_after_a_door_refusal_is_this_calls_execution() {
        let (first, _) = stub::serve(execution_fails(|| {
            tonic::Status::permission_denied("proxy")
        }))
        .await;
        let mut holder = Node::default();
        let d = digest();
        holder.get_transaction = Box::new(move |_| found(&d, true));
        let (second, second_node) = stub::serve(holder).await;
        assert_eq!(
            run(&rpc(vec![first, second])).await,
            Execution::Executed { failure: None }
        );
        assert_eq!(second_node.count("ExecuteTransaction"), 0);
    }

    #[tokio::test]
    async fn an_executed_transaction_without_a_verdict_is_never_resent() {
        let mut first = Node::default();
        first.get_transaction = Box::new(|_| {
            let mut response = GetTransactionResponse::default();
            response.transaction = Some(executed(&digest(), None));
            Reply::Answer(Ok(response))
        });
        let (first, first_node) = stub::serve(first).await;
        let (second, second_node) = stub::serve(execution_fails(|| {
            tonic::Status::unavailable("unavailable")
        }))
        .await;
        assert_eq!(
            run(&rpc(vec![first, second])).await,
            Execution::AlreadyExecuted
        );
        assert_eq!(first_node.count("ExecuteTransaction"), 0);
        // Known before any send: the second one is not even asked.
        assert!(second_node.calls().is_empty(), "{:?}", second_node.calls());
    }

    #[tokio::test]
    async fn no_endpoint_reachable_is_unreachable_and_nothing_was_sent() {
        let mut unreadable = Node::default();
        unreadable.get_transaction =
            Box::new(|_| Reply::Answer(Err(tonic::Status::internal("down"))));
        let (url, node) = stub::serve(unreadable).await;
        let rpc = rpc(vec![stub::closed_endpoint().await, url]);
        assert!(matches!(run(&rpc).await, Execution::Unreachable(_)));
        assert_eq!(node.count("ExecuteTransaction"), 0);
    }

    /// An endpoint that holds the transaction without a verdict before any
    /// send: what any endpoint would say later cannot turn it into a refusal
    /// or into "nothing was sent", and no later endpoint is asked.
    #[tokio::test]
    async fn after_a_transaction_found_without_a_verdict_no_later_answer_is_a_refusal() {
        for later in [
            (|| tonic::Status::invalid_argument("Invalid user signature")) as fn() -> tonic::Status,
            || tonic::Status::unimplemented("no transaction executor"),
        ] {
            let mut first = Node::default();
            first.get_transaction = Box::new(|_| {
                let mut response = GetTransactionResponse::default();
                response.transaction = Some(executed(&digest(), None));
                Reply::Answer(Ok(response))
            });
            let (first, first_node) = stub::serve(first).await;
            let (second, second_node) = stub::serve(execution_fails(later)).await;
            let outcome = run(&rpc(vec![first, second])).await;
            assert_eq!(outcome, Execution::AlreadyExecuted);
            assert_eq!(first_node.count("ExecuteTransaction"), 0);
            assert!(second_node.calls().is_empty(), "{:?}", second_node.calls());
        }
    }

    /// An execution answered without its effects' verdict, or without a
    /// transaction at all, may be final: unconfirmed, never a success or a
    /// failure it did not report.
    #[tokio::test]
    async fn an_execution_answered_without_a_verdict_is_unconfirmed() {
        for status in [None, Some(())] {
            let mut node = Node::default();
            node.get_transaction = Box::new(|_| not_found());
            node.execute = Box::new(move |_| {
                let mut response = ExecuteTransactionResponse::default();
                if status.is_some() {
                    response.transaction = Some(executed(&digest(), None));
                }
                Reply::Answer(Ok(response))
            });
            let (url, node) = stub::serve(node).await;
            let outcome = run(&rpc(vec![url])).await;
            assert!(matches!(outcome, Execution::Unconfirmed(_)), "{outcome:?}");
            // Lookup, the send, and the last lookup.
            assert_eq!(
                node.calls(),
                vec!["GetTransaction", "ExecuteTransaction", "GetTransaction"]
            );
        }
    }

    /// A door refusal after the send is asked about once more: a proxy can say
    /// no after forwarding the transaction.
    #[tokio::test]
    async fn a_closed_door_after_a_send_is_looked_up_before_calling_it_unreachable() {
        let mut node = Node::default();
        let lookups = Arc::new(AtomicUsize::new(0));
        let seen = lookups.clone();
        let d = digest();
        node.get_transaction = Box::new(move |_| {
            if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                not_found()
            } else {
                found(&d, true)
            }
        });
        node.execute = Box::new(|_| Reply::Answer(Err(tonic::Status::permission_denied("proxy"))));
        let (url, _) = stub::serve(node).await;
        assert_eq!(
            run(&rpc(vec![url])).await,
            Execution::Executed { failure: None }
        );
        assert_eq!(lookups.load(Ordering::SeqCst), 2);

        // Not found then either: nothing reported it, so nothing was sent.
        let (url, _) = stub::serve(execution_fails(|| {
            tonic::Status::permission_denied("proxy")
        }))
        .await;
        assert!(matches!(
            run(&rpc(vec![url])).await,
            Execution::Unreachable(_)
        ));
    }

    /// Past one execution timeout since the first send, the next endpoint is
    /// asked about the digest, not sent the transaction.
    #[tokio::test]
    async fn no_new_send_once_an_execution_timeout_has_passed() {
        let mut slow = Node::default();
        slow.get_transaction = Box::new(|_| not_found());
        slow.execute = Box::new(|_| Reply::Hang);
        let (first, _) = stub::serve(slow).await;
        let (second, second_node) = stub::serve(executes(true)).await;
        let rpc = SuiRpc::new(
            Network::SuiTestnet,
            vec![first, second],
            Duration::from_secs(2),
            Duration::from_millis(300),
        );
        let outcome = tokio::time::timeout(Duration::from_secs(10), run(&rpc))
            .await
            .expect("bounded");
        assert!(matches!(outcome, Execution::Unconfirmed(_)), "{outcome:?}");
        assert_eq!(second_node.count("ExecuteTransaction"), 0);
        assert!(second_node.count("GetTransaction") >= 1);
    }

    #[tokio::test]
    async fn configured_urls_tonic_cannot_take_give_the_defaults() {
        // url::Url takes the host (IDNA); http::Uri, which tonic needs, does not.
        let rpc = rpc(vec!["https://bücher.example:443".to_string()]);
        assert_eq!(rpc.connect(), DEFAULT_ENDPOINTS_TESTNET.len());
        let rpc = rpc_empty();
        assert_eq!(rpc.connect(), 0);
    }

    fn rpc_empty() -> SuiRpc {
        rpc(Vec::new())
    }

    #[tokio::test]
    async fn a_failure_on_chain_is_an_execution_with_its_error() {
        let (url, _) = stub::serve(executes(false)).await;
        match run(&rpc(vec![url])).await {
            Execution::Executed {
                failure: Some(failure),
            } => {
                assert!(failure.contains("InsufficientCoinBalance"), "{failure}")
            }
            other => panic!("expected the failed execution, got {other:?}"),
        }
    }
}

/// Read-only measurement of the configured endpoints over the network, for an
/// operator before a deploy (`RPC_URL_SUI` / `RPC_URL_SUI_TESTNET` when set,
/// otherwise the defaults): chain id, reference gas price, the SUI balance of
/// the facilitator's public wallets, one `BatchGetObjects` and the `NotFound`
/// that the execution's digest lookup relies on. Nothing is signed or sent.
///
/// `cargo test --locked -p x402-rs --features sui -- --ignored sui_endpoints_live_smoke --nocapture`
#[cfg(test)]
mod live_smoke {
    use super::*;
    use sui_rpc::proto::sui::rpc::v2::{GetEpochRequest, GetServiceInfoRequest};

    const MAINNET_CHAIN_ID: &str = "4btiuiMPvEENsttpZC7CZ53DruC3MAgfznDbASZ7DR6S";
    const TESTNET_CHAIN_ID: &str = "69WiPg3DAQiwdxfncX6wYQ2siKwAe6L9BZthQea3JNMD";

    /// The facilitator's Sui wallet, read from `lambda/balances/handler.py`,
    /// the authoritative list of wallet addresses.
    fn wallet(name: &str) -> SuiAddress {
        let handler = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/lambda/balances/handler.py"
        ))
        .unwrap();
        let pattern = format!("(?m)^{name}\\s*=\\s*\"(0x[0-9a-fA-F]+)\"");
        let address = regex::Regex::new(&pattern)
            .unwrap()
            .captures(&handler)
            .unwrap_or_else(|| panic!("{name} not in handler.py"))[1]
            .to_string();
        SuiAddress::from_str(&address).unwrap()
    }

    #[tokio::test]
    #[ignore = "reads the public Sui endpoints over the network"]
    async fn sui_endpoints_live_smoke() {
        let mut failures = Vec::new();
        for (network, var, chain_id, wallet_name) in [
            (
                Network::Sui,
                "RPC_URL_SUI",
                MAINNET_CHAIN_ID,
                "SUI_MAINNET_ADDRESS",
            ),
            (
                Network::SuiTestnet,
                "RPC_URL_SUI_TESTNET",
                TESTNET_CHAIN_ID,
                "SUI_TESTNET_ADDRESS",
            ),
        ] {
            let wallet = wallet(wallet_name);
            let urls = endpoint_list(std::env::var(var).ok().as_deref(), network);
            for url in urls {
                let label = crate::redact::rpc_url(&url);
                let mut fail = |what: &str| failures.push(format!("{network} {label}: {what}"));
                let mut client = sui_rpc::Client::new(url.as_str()).unwrap();
                let timeout = Duration::from_secs(15);

                let started = std::time::Instant::now();
                match tokio::time::timeout(
                    timeout,
                    client
                        .ledger_client()
                        .get_service_info(GetServiceInfoRequest::default()),
                )
                .await
                {
                    Ok(Ok(info)) => {
                        let info = info.into_inner();
                        let id = info.chain_id.clone().unwrap_or_default();
                        println!(
                            "{network} {label}: chain_id={id} checkpoint={:?} server={:?} ({} ms)",
                            info.checkpoint_height,
                            info.server,
                            started.elapsed().as_millis()
                        );
                        if id != chain_id {
                            fail(&format!("chain id {id}, expected {chain_id}"));
                        }
                    }
                    Ok(Err(status)) => fail(&format!("GetServiceInfo: {}", describe(&status))),
                    Err(_) => fail("GetServiceInfo timed out"),
                }

                let mut request = GetEpochRequest::default();
                request.read_mask = Some(FieldMask::from_paths(["epoch", "reference_gas_price"]));
                match tokio::time::timeout(timeout, client.ledger_client().get_epoch(request)).await
                {
                    Ok(Ok(epoch)) => {
                        let price = epoch
                            .into_inner()
                            .epoch
                            .and_then(|e| e.reference_gas_price)
                            .unwrap_or(0);
                        println!("{network} {label}: reference_gas_price={price}");
                        if price == 0 {
                            fail("no reference gas price");
                        }
                    }
                    Ok(Err(status)) => fail(&format!("GetEpoch: {}", describe(&status))),
                    Err(_) => fail("GetEpoch timed out"),
                }

                let one = SuiRpc::new(network, vec![url.clone()], timeout, timeout);
                match one.coin_balance(&wallet, "0x2::sui::SUI").await {
                    Ok(mist) => println!(
                        "{network} {label}: facilitator SUI balance = {mist} MIST ({:.4} SUI)",
                        mist as f64 / 1e9
                    ),
                    Err(e) => fail(&format!("GetBalance: {e}")),
                }

                // 0x5 is the Sui system state object: present on every network.
                match one.objects(&[(ObjectID::from_single_byte(5), None)]).await {
                    Ok(reads) if matches!(reads.as_slice(), [ObjectRead::Found { .. }]) => {
                        println!("{network} {label}: BatchGetObjects ok")
                    }
                    Ok(reads) => fail(&format!("BatchGetObjects answered {reads:?}")),
                    Err(e) => fail(&format!("BatchGetObjects: {e}")),
                }

                // The digest lookup before an execution needs `NotFound` for a
                // transaction the node does not hold; any other code means this
                // endpoint would never be sent a transaction.
                let mut lookup = GetTransactionRequest::default();
                lookup.digest =
                    Some(sui_types::digests::TransactionDigest::new([0x42; 32]).to_string());
                lookup.read_mask = Some(FieldMask::from_paths(["digest", "effects.status"]));
                match tokio::time::timeout(timeout, client.ledger_client().get_transaction(lookup))
                    .await
                {
                    Ok(Err(status)) if status.code() == Code::NotFound => {
                        println!("{network} {label}: unknown digest -> NotFound")
                    }
                    Ok(Err(status)) => fail(&format!("digest lookup: {}", describe(&status))),
                    Ok(Ok(_)) => fail("digest lookup found a transaction that cannot exist"),
                    Err(_) => fail("digest lookup timed out"),
                }
            }
        }
        assert!(
            failures.is_empty(),
            "endpoints failing:\n{}",
            failures.join("\n")
        );
    }
}
