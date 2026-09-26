//! Self-check of the PaymentOperators declared on the canonical v1 networks.
//!
//! An operator listed for Arc or Arc testnet in
//! `OperatorAddresses::payment_operators` is announced as an `escrow` entry in
//! `/supported`, and accepts NEW `authorize` requests, only once the chain says
//! it is what the facilitator will treat it as: its `ESCROW()` is the escrow
//! declared for the network, and its bytecode carries the v3 selectors the
//! facilitator calls. The check runs at startup and then every [`REFRESH`], and
//! the last verdict is kept per `(network, operator)`.
//!
//! It governs exactly those two things. `release`, `refundInEscrow` and
//! `/escrow/state` never consult it: a payment that is already authorized can
//! always be released or voided, whatever this check last saw. A read that
//! fails is a verdict of its own ([`Verdict::Unreachable`]); it never stops the
//! process and never takes another network down with it.
//!
//! The networks whose operators speak the older ABIs are not checked here and
//! are announced exactly as before.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use alloy::network::TransactionBuilder as _;
use alloy::primitives::{Address, Bytes};
use alloy::providers::Provider;
use alloy::rpc::types::TransactionRequest;
use alloy::sol_types::SolCall;
use tracing::{info, warn};

use super::abi::OperatorV3Contract;
use super::addresses::{OperatorAddresses, CANONICAL_V1_NETWORKS};
use crate::chain::evm::{EvmProvider, MetaEvmProvider};
use crate::chain::NetworkProvider;
use crate::network::Network;
use crate::provider_cache::ProviderMap;

/// How often the background task re-reads every declared operator.
pub const REFRESH: Duration = Duration::from_secs(600);

/// A request that finds no verified verdict re-reads the chain, but not more
/// often than this per operator, so a burst of requests against an operator
/// that is not deployed yet costs one read, not one per request.
pub const RECHECK_FLOOR: Duration = Duration::from_secs(60);

/// What the chain said about one operator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Bound to the declared escrow and carrying every v3 selector.
    Verified,
    /// The chain answered, and the operator is not what the facilitator would
    /// call: no code (not deployed yet), another escrow, a missing selector.
    Mismatch(String),
    /// The chain could not be read.
    Unreachable(String),
}

/// The selectors the facilitator calls on, or reads from, a v3 operator.
pub fn v3_selectors() -> [(&'static str, [u8; 4]); 4] {
    [
        ("authorize", OperatorV3Contract::authorizeCall::SELECTOR),
        ("capture", OperatorV3Contract::captureCall::SELECTOR),
        ("void", OperatorV3Contract::voidCall::SELECTOR),
        (
            "FEE_RECEIVER",
            OperatorV3Contract::FEE_RECEIVERCall::SELECTOR,
        ),
    ]
}

/// Whether `code` pushes `selector` as a 4-byte immediate (`PUSH4`, 0x63): the
/// form solc's dispatcher compares the calldata selector against.
pub fn pushes_selector(code: &[u8], selector: [u8; 4]) -> bool {
    code.windows(5)
        .any(|w| w[0] == 0x63 && w[1..] == selector[..])
}

/// Judge an operator from its bytecode and, once the bytecode passes, the
/// raw answer of its `ESCROW()`. `None` for the answer means it was not read
/// because the bytecode already failed.
pub fn judge(code: &[u8], escrow_answer: Option<&[u8]>, escrow: Address) -> Verdict {
    if code.is_empty() {
        return Verdict::Mismatch("no code at the operator address".to_string());
    }
    for (name, selector) in v3_selectors() {
        if !pushes_selector(code, selector) {
            return Verdict::Mismatch(format!("bytecode has no {name}()"));
        }
    }
    let Some(raw) = escrow_answer else {
        return Verdict::Mismatch("ESCROW() was not read".to_string());
    };
    match OperatorV3Contract::ESCROWCall::abi_decode_returns(raw) {
        Ok(bound) if bound == escrow => Verdict::Verified,
        Ok(bound) => Verdict::Mismatch(format!("ESCROW() is {bound}, declared {escrow}")),
        Err(e) => Verdict::Mismatch(format!("ESCROW() did not decode: {e}")),
    }
}

/// Whether [`judge`] needs the `ESCROW()` answer for this bytecode.
pub fn code_passes(code: &[u8]) -> bool {
    !code.is_empty()
        && v3_selectors()
            .iter()
            .all(|(_, selector)| pushes_selector(code, *selector))
}

/// Read the chain and judge `operator` against `escrow`: `eth_getCode`, then
/// `ESCROW()` only if the bytecode passes.
pub async fn check(provider: &EvmProvider, operator: Address, escrow: Address) -> Verdict {
    check_with(provider.inner(), operator, escrow).await
}

/// [`check`] over any provider.
pub async fn check_with<P: Provider>(provider: &P, operator: Address, escrow: Address) -> Verdict {
    let unreachable = |what: &str, e: &dyn std::fmt::Display| {
        Verdict::Unreachable(format!(
            "{what}: {}",
            crate::redact::scrub_urls(&e.to_string())
        ))
    };
    let code = match provider.get_code_at(operator).await {
        Ok(code) => code,
        Err(e) => return unreachable("eth_getCode", &e),
    };
    if !code_passes(&code) {
        return judge(&code, None, escrow);
    }
    let tx = TransactionRequest::default()
        .with_to(operator)
        .with_input(Bytes::from(OperatorV3Contract::ESCROWCall {}.abi_encode()));
    match provider.call(tx).await {
        Ok(raw) => judge(&code, Some(&raw), escrow),
        Err(e) => unreachable("ESCROW()", &e),
    }
}

struct Entry {
    verdict: Verdict,
    at: Instant,
}

fn cache() -> &'static Mutex<HashMap<(Network, Address), Entry>> {
    static CACHE: OnceLock<Mutex<HashMap<(Network, Address), Entry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn record(network: Network, operator: Address, verdict: Verdict) {
    let mut cache = cache().lock().unwrap_or_else(|e| e.into_inner());
    let previous = cache.insert(
        (network, operator),
        Entry {
            verdict: verdict.clone(),
            at: Instant::now(),
        },
    );
    if previous.map(|p| p.verdict) != Some(verdict.clone()) {
        match &verdict {
            Verdict::Verified => info!(
                %network, %operator,
                "[OK] escrow operator verified: bound to the declared escrow, v3 selectors present; announced"
            ),
            Verdict::Mismatch(reason) => warn!(
                %network, %operator, %reason,
                "escrow operator not verified; not announced, new authorizations refused"
            ),
            Verdict::Unreachable(reason) => warn!(
                %network, %operator, %reason,
                "escrow operator could not be checked; not announced until a read succeeds"
            ),
        }
    }
}

/// The last verdict for `operator` on `network`, without touching the chain.
/// What `/supported` reads.
pub fn status(network: Network, operator: Address) -> Option<Verdict> {
    cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&(network, operator))
        .map(|e| e.verdict.clone())
}

/// Whether `/supported` may announce `operator` on `network` right now.
pub fn is_verified(network: Network, operator: Address) -> bool {
    status(network, operator) == Some(Verdict::Verified)
}

/// The verdict a NEW authorization against `operator` is judged by.
///
/// A cached [`Verdict::Verified`] stands: an operator's code and immutables do
/// not change once deployed. Anything else is re-read, at most once per
/// [`RECHECK_FLOOR`], so an operator deployed after the last refresh does not
/// wait for the next one.
pub async fn verdict_for_new_authorization(
    provider: &EvmProvider,
    network: Network,
    operator: Address,
    escrow: Address,
) -> Verdict {
    {
        let mut cache = cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = cache.get_mut(&(network, operator)) {
            if entry.verdict == Verdict::Verified || entry.at.elapsed() < RECHECK_FLOOR {
                return entry.verdict.clone();
            }
            // Claim the re-read: concurrent requests inside the floor keep the
            // standing verdict instead of each reading the chain.
            entry.at = Instant::now();
        }
    }
    let verdict = check(provider, operator, escrow).await;
    record(network, operator, verdict.clone());
    verdict
}

/// Check every operator declared on the canonical v1 networks this process
/// has a provider for. Sequential: a handful of reads, and a public RPC that
/// rate-limits is better served one call at a time.
pub async fn refresh_all<M>(providers: &M)
where
    M: ProviderMap<Value = NetworkProvider>,
{
    for &network in CANONICAL_V1_NETWORKS {
        let Some(NetworkProvider::Evm(provider)) = providers.by_network(network) else {
            continue;
        };
        let Some(addrs) = OperatorAddresses::for_network(network) else {
            continue;
        };
        debug_assert_eq!(provider.chain().network(), network);
        for &operator in &addrs.payment_operators {
            let verdict = check(provider, operator, addrs.escrow).await;
            record(network, operator, verdict);
        }
    }
}

/// Run [`refresh_all`] now and then every [`REFRESH`], in the background.
/// Startup waits on none of it.
pub fn spawn<M>(providers: Arc<M>)
where
    M: ProviderMap<Value = NetworkProvider> + Send + Sync + 'static,
{
    tokio::spawn(async move {
        loop {
            refresh_all(providers.as_ref()).await;
            tokio::time::sleep(REFRESH).await;
        }
    });
}

/// Set a verdict directly. Tests only: the verdict is otherwise always the
/// chain's.
#[cfg(test)]
pub fn set_for_test(network: Network, operator: Address, verdict: Option<Verdict>) {
    let mut cache = cache().lock().unwrap_or_else(|e| e.into_inner());
    match verdict {
        Some(verdict) => {
            cache.insert(
                (network, operator),
                Entry {
                    verdict,
                    at: Instant::now(),
                },
            );
        }
        None => {
            cache.remove(&(network, operator));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolValue;

    const ESCROW: Address = alloy::primitives::address!("BdEA0D1bcC5966192B070Fdf62aB4EF5b4420cff");

    fn bytecode(names: &[&str]) -> Vec<u8> {
        let mut code = Vec::new();
        for (name, selector) in v3_selectors() {
            if names.contains(&name) {
                code.push(0x63);
                code.extend_from_slice(&selector);
            }
        }
        code
    }

    /// Every selector and the declared escrow, and nothing less, is verified:
    /// a missing selector is a mismatch even when `ESCROW()` answers right.
    #[test]
    fn judge_needs_every_v3_selector_and_the_declared_escrow() {
        let all = ["authorize", "capture", "void", "FEE_RECEIVER"];
        let bound = ESCROW.abi_encode();
        assert_eq!(
            judge(&bytecode(&all), Some(&bound), ESCROW),
            Verdict::Verified
        );
        for missing in all {
            let some: Vec<&str> = all.iter().copied().filter(|n| *n != missing).collect();
            let code = bytecode(&some);
            assert!(!code_passes(&code), "{missing}");
            assert_eq!(
                judge(&code, Some(&bound), ESCROW),
                Verdict::Mismatch(format!("bytecode has no {missing}()")),
            );
        }
        let elsewhere = Address::repeat_byte(0x42).abi_encode();
        assert!(matches!(
            judge(&bytecode(&all), Some(&elsewhere), ESCROW),
            Verdict::Mismatch(_)
        ));
        assert!(matches!(judge(&[], None, ESCROW), Verdict::Mismatch(_)));
    }
}
