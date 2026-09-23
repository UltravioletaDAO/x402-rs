//! A reputation summary that one `getSummary` call cannot answer.
//!
//! `GET /reputation/{network}/{agentId}` asks the registry for the summary over
//! every client `getClients` names, in one `eth_call`. The registry walks every
//! feedback entry of every client inside that call, so a single client with
//! enough entries makes it revert whatever else is in the list. Measured on
//! 2026-09-23 against Arc testnet (registry `0x8004B663...8713`, agent 1): 1,315
//! clients, one of them with 77,447 entries; `getSummary` over that client alone
//! reverts, over the other 1,314 in groups of 100 it answers. Until 2.39.2 the
//! route turned that revert into a 500 for the whole agent.
//!
//! [`summarize_in_chunks`] is the fallback the route takes when the single call
//! fails: groups of [`CLIENTS_PER_CALL`], a group the node refuses is split in
//! half until the refusal is pinned to single clients, and the whole read stops
//! after [`MAX_CALLS`]. The answer says what it covered ([`Coverage`]).
//!
//! The registry returns an AVERAGE (`summaryValue` over `count` entries, at the
//! decimals most entries use; on Arc testnet, entries of 95 and 80 answer 87),
//! so groups
//! are combined weighted by their counts. Each group's average is already
//! truncated by the registry, so the combined value can differ from what a
//! single call would have returned in its last digit; the coverage says so.

use std::collections::{BTreeMap, VecDeque};
use std::future::Future;

use alloy::primitives::{Address, I256, U256};
use serde::{Deserialize, Serialize};

use super::abi::IReputationRegistry::IReputationRegistryInstance;

/// Clients per `getSummary` call once the single call has failed.
pub const CLIENTS_PER_CALL: usize = 100;

/// Most `getSummary` calls one request spends on the fallback. Clients left
/// when it runs out are counted in [`Coverage::clients_not_read`], never
/// silently dropped. 1,315 clients with one unreadable take 28.
pub const MAX_CALLS: usize = 32;

/// `getSummary` calls in flight at once during the fallback. Arc testnet's
/// public RPC answered a burst of 8 with 429 on 2026-09-23; 4, through the
/// retry layer production uses, read agent 1 in 2.1 s (28 calls).
pub const CALLS_IN_FLIGHT: usize = 4;

/// Parts a refused group is split into.
pub const SPLIT_INTO: usize = 4;

/// Unreadable clients named in the answer; the count is always complete.
pub const MAX_LISTED_UNREADABLE: usize = 20;

/// The registry's `MAX_VALUE_DECIMALS`: values are normalized to 18 decimals.
const WAD_DECIMALS: u8 = 18;

/// One `getSummary` answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Summary {
    pub count: u64,
    pub value: i128,
    pub decimals: u8,
}

/// Why one `getSummary` call gave no answer.
#[derive(Debug)]
pub enum ReadError {
    /// The node answered the call with an error: a revert, out of gas. The
    /// same clients would fail again, so the group is split.
    Refused,
    /// The node could not be asked. Splitting would only repeat it, so the
    /// whole read fails.
    Unavailable(String),
}

/// What a fallback read covered. Serialized as `coverage` next to the summary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    /// Every client `getClients` named was read.
    pub complete: bool,
    /// `chunked`: the summary is combined from several calls.
    pub method: String,
    pub clients_total: usize,
    pub clients_read: usize,
    /// Clients whose summary the registry refuses even alone.
    pub clients_unreadable: usize,
    /// Clients left unread when [`MAX_CALLS`] ran out.
    pub clients_not_read: usize,
    /// Up to [`MAX_LISTED_UNREADABLE`] of the unreadable clients.
    pub unreadable_clients: Vec<Address>,
    pub calls: usize,
    pub clients_per_call: usize,
    pub max_calls: usize,
    pub note: String,
}

const NOTE: &str = "The registry could not summarize every client in one call. This summary \
combines calls over groups of clients, weighted by their counts, and leaves out the clients \
listed as unreadable and not read. Each group's average is truncated by the registry, so the \
value can differ from a single read in its last digit. Feedback entries are not returned.";

/// Whether the node answered a call and refused it (a revert, out of gas), as
/// opposed to not answering at all. Only a refusal is worth splitting.
pub fn is_refusal(error: &alloy::contract::Error) -> bool {
    matches!(error, alloy::contract::Error::TransportError(t) if t.is_error_resp())
}

/// One `getSummary` call over `group`, classified for [`summarize_in_chunks`].
pub async fn read_group<P: alloy::providers::Provider>(
    registry: &IReputationRegistryInstance<P>,
    agent_id: U256,
    group: Vec<Address>,
    tag1: String,
    tag2: String,
) -> Result<Summary, ReadError> {
    match registry
        .getSummary(agent_id, group, tag1, tag2)
        .call()
        .await
    {
        Ok(r) => Ok(Summary {
            count: r.count,
            value: r.summaryValue,
            decimals: r.summaryValueDecimals,
        }),
        Err(error) if is_refusal(&error) => Err(ReadError::Refused),
        // Server-side only, and scrubbed: a transport error can quote the RPC
        // URL, key included.
        Err(error) => Err(ReadError::Unavailable(crate::redact::scrub_urls(
            &error.to_string(),
        ))),
    }
}

/// Summarize `clients` in groups, splitting any group `read` refuses.
///
/// Up to [`CALLS_IN_FLIGHT`] calls run at once. A refused group is split into
/// [`SPLIT_INTO`] parts, so one unreadable client among 100 is pinned down in
/// four rounds rather than seven.
///
/// `Err` when not one client could be read, or when the node could not be
/// asked at all: there is no summary to give, and a zero would read as "no
/// reputation".
pub async fn summarize_in_chunks<F, Fut>(
    clients: &[Address],
    read: F,
) -> Result<(Summary, Coverage), String>
where
    F: Fn(Vec<Address>) -> Fut,
    Fut: Future<Output = Result<Summary, ReadError>> + Send + 'static,
{
    let mut queue: VecDeque<Vec<Address>> = clients
        .chunks(CLIENTS_PER_CALL)
        .map(<[Address]>::to_vec)
        .collect();
    let mut in_flight = tokio::task::JoinSet::new();
    let mut calls = 0;
    let mut clients_read = 0;
    let mut unreadable: Vec<Address> = Vec::new();
    let mut total = Aggregate::default();

    loop {
        while in_flight.len() < CALLS_IN_FLIGHT && calls < MAX_CALLS {
            let Some(group) = queue.pop_front() else {
                break;
            };
            calls += 1;
            let call = read(group.clone());
            in_flight.spawn(async move { (group, call.await) });
        }
        // Nothing in flight: the queue is empty or the budget is spent.
        let Some(joined) = in_flight.join_next().await else {
            break;
        };
        let (group, answer) = joined.map_err(|e| format!("summary task failed: {e}"))?;
        match answer {
            Ok(summary) => {
                clients_read += group.len();
                total.add(summary)?;
            }
            Err(ReadError::Refused) if group.len() == 1 => unreadable.push(group[0]),
            Err(ReadError::Refused) => {
                // In front of the queue, in order: the refusal is pinned down
                // before the budget goes to the rest.
                let part = group.len().div_ceil(SPLIT_INTO);
                for piece in group.chunks(part).rev() {
                    queue.push_front(piece.to_vec());
                }
            }
            Err(ReadError::Unavailable(error)) => {
                in_flight.abort_all();
                return Err(error);
            }
        }
    }

    let clients_not_read = queue.iter().map(Vec::len).sum::<usize>();
    if clients_read == 0 {
        return Err(format!(
            "no client could be summarized ({} unreadable, {clients_not_read} not read)",
            unreadable.len()
        ));
    }
    let clients_unreadable = unreadable.len();
    unreadable.sort();
    unreadable.truncate(MAX_LISTED_UNREADABLE);
    Ok((
        total.finish()?,
        Coverage {
            complete: clients_read == clients.len(),
            method: "chunked".to_string(),
            clients_total: clients.len(),
            clients_read,
            clients_unreadable,
            clients_not_read,
            unreadable_clients: unreadable,
            calls,
            clients_per_call: CLIENTS_PER_CALL,
            max_calls: MAX_CALLS,
            note: NOTE.to_string(),
        },
    ))
}

/// Group averages, turned back into a weighted sum at 18 decimals.
#[derive(Default)]
struct Aggregate {
    wad_sum: I256,
    count: u64,
    /// Entries per decimals, from each group's own (most used) decimals.
    by_decimals: BTreeMap<u8, u64>,
}

impl Aggregate {
    fn add(&mut self, summary: Summary) -> Result<(), String> {
        if summary.count == 0 {
            return Ok(());
        }
        let scale = wad_scale(summary.decimals)?;
        let part = I256::try_from(summary.value)
            .ok()
            .and_then(|v| v.checked_mul(scale))
            .and_then(|v| v.checked_mul(I256::try_from(summary.count).ok()?))
            .ok_or("reputation summary overflows")?;
        self.wad_sum = self
            .wad_sum
            .checked_add(part)
            .ok_or("reputation summary overflows")?;
        self.count += summary.count;
        *self.by_decimals.entry(summary.decimals).or_default() += summary.count;
        Ok(())
    }

    fn finish(self) -> Result<Summary, String> {
        if self.count == 0 {
            return Ok(Summary {
                count: 0,
                value: 0,
                decimals: 0,
            });
        }
        // The decimals most entries use, the lower on a tie: the registry's own
        // rule (`getSummary` in erc-8004/erc-8004-contracts'
        // ReputationRegistryUpgradeable.sol).
        let decimals = self
            .by_decimals
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
            .map(|(d, _)| *d)
            .unwrap_or(0);
        let average = self.wad_sum / I256::try_from(self.count).map_err(|e| e.to_string())?;
        let value = i128::try_from(average / wad_scale(decimals)?)
            .map_err(|_| "reputation summary overflows".to_string())?;
        Ok(Summary {
            count: self.count,
            value,
            decimals,
        })
    }
}

fn wad_scale(decimals: u8) -> Result<I256, String> {
    let shift = WAD_DECIMALS
        .checked_sub(decimals)
        .ok_or_else(|| format!("reputation value with {decimals} decimals"))?;
    Ok(I256::exp10(usize::from(shift)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A registry double with the real contract's arithmetic: every client
    /// holds `entries` feedback entries of `value` at 0 decimals, and a call
    /// that includes a client with more than `gas_cap` entries in total
    /// reverts, as Arc testnet's agent 1 does.
    struct Registry {
        entries: Vec<(Address, u64, i128)>,
        gas_cap: u64,
    }

    impl Registry {
        fn summary(&self, group: &[Address]) -> Result<Summary, ReadError> {
            let mut sum = 0i128;
            let mut count = 0u64;
            for client in group {
                let (_, n, value) = self.entries.iter().find(|(a, ..)| a == client).unwrap();
                count += n;
                sum += value * i128::from(*n);
            }
            if count > self.gas_cap {
                return Err(ReadError::Refused);
            }
            Ok(Summary {
                count,
                value: if count == 0 {
                    0
                } else {
                    sum / i128::from(count)
                },
                decimals: 0,
            })
        }
    }

    fn address(i: usize) -> Address {
        let mut bytes = [0u8; 20];
        bytes[12..].copy_from_slice(&(i as u64 + 1).to_be_bytes());
        Address::from(bytes)
    }

    /// Arc testnet's agent 1, as measured: 1,315 clients, one of them with
    /// 77,447 entries. The single call reverts; the fallback reads the rest.
    fn agent_one(values: impl Fn(usize) -> i128) -> Registry {
        let entries = (0..1315)
            .map(|i| {
                let n = if i == 5 { 77_447 } else { 1 + (i as u64 % 3) };
                (address(i), n, values(i))
            })
            .collect();
        Registry {
            entries,
            gas_cap: 10_000,
        }
    }

    async fn run(registry: Registry) -> (Result<(Summary, Coverage), String>, usize) {
        let clients: Vec<Address> = registry.entries.iter().map(|(a, ..)| *a).collect();
        let registry = Arc::new(registry);
        let asked = Arc::new(AtomicUsize::new(0));
        let result = summarize_in_chunks(&clients, |group| {
            asked.fetch_add(1, Ordering::SeqCst);
            let registry = Arc::clone(&registry);
            async move { registry.summary(&group) }
        })
        .await;
        (result, asked.load(Ordering::SeqCst))
    }

    #[tokio::test]
    async fn over_a_thousand_clients_with_one_unreadable_answer_for_the_rest() {
        let registry = agent_one(|_| 90);
        let clients: Vec<Address> = registry.entries.iter().map(|(a, ..)| *a).collect();
        assert!(
            registry.summary(&clients).is_err(),
            "premise: the single call reverts"
        );

        let (result, asked) = run(registry).await;
        let (summary, coverage) = result.expect("1,314 clients are readable");
        assert_eq!(coverage.clients_total, 1315);
        assert_eq!(coverage.clients_read, 1314);
        assert_eq!(coverage.clients_unreadable, 1);
        assert_eq!(coverage.unreadable_clients, vec![address(5)]);
        assert_eq!(coverage.clients_not_read, 0);
        assert!(!coverage.complete);
        assert_eq!(coverage.calls, asked);
        assert_eq!(
            asked, 28,
            "13 whole groups, and 15 calls to pin the one client"
        );

        let expected: u64 = (0..1315)
            .filter(|i| *i != 5)
            .map(|i| 1 + (i as u64 % 3))
            .sum();
        assert_eq!(
            summary,
            Summary {
                count: expected,
                value: 90,
                decimals: 0
            }
        );
    }

    /// Group averages are truncated by the registry, so the combination can
    /// be one unit under the true average -- never more, and never above it.
    #[tokio::test]
    async fn the_combined_average_is_within_one_unit_of_the_true_one() {
        let registry = agent_one(|i| [95, 80, 70, 100, 13][i % 5]);
        let entries = registry.entries.clone();
        let (result, _) = run(registry).await;
        let (summary, _) = result.unwrap();

        let (sum, count) = entries
            .iter()
            .filter(|(a, ..)| *a != address(5))
            .fold((0i128, 0u64), |(s, c), (_, n, v)| {
                (s + v * i128::from(*n), c + n)
            });
        let exact = sum / i128::from(count);
        assert_eq!(summary.count, count);
        assert!(
            summary.value == exact || summary.value == exact - 1,
            "{} against {exact}",
            summary.value
        );
    }

    #[tokio::test]
    async fn the_call_budget_is_declared_and_what_it_leaves_is_counted() {
        let registry = Registry {
            entries: (0..(CLIENTS_PER_CALL * (MAX_CALLS + 5)))
                .map(|i| (address(i), 1, 50))
                .collect(),
            gas_cap: u64::MAX,
        };
        let (result, asked) = run(registry).await;
        let (summary, coverage) = result.unwrap();
        assert_eq!(asked, MAX_CALLS);
        assert_eq!(coverage.clients_read, CLIENTS_PER_CALL * MAX_CALLS);
        assert_eq!(coverage.clients_not_read, CLIENTS_PER_CALL * 5);
        assert!(!coverage.complete);
        assert_eq!(summary.count, (CLIENTS_PER_CALL * MAX_CALLS) as u64);
        assert_eq!(summary.value, 50);
    }

    #[tokio::test]
    async fn a_node_that_cannot_be_asked_is_an_error_not_a_split() {
        let clients: Vec<Address> = (0..2_500).map(address).collect();
        let asked = AtomicUsize::new(0);
        let result = summarize_in_chunks(&clients, |_| {
            asked.fetch_add(1, Ordering::SeqCst);
            async { Err(ReadError::Unavailable("connection refused".into())) }
        })
        .await;
        assert_eq!(result.unwrap_err(), "connection refused");
        // What was already in flight, and nothing after: no group is split.
        assert!(asked.load(Ordering::SeqCst) <= CALLS_IN_FLIGHT);
    }

    #[tokio::test]
    async fn nothing_readable_is_an_error_not_a_zero() {
        let clients: Vec<Address> = (0..3).map(address).collect();
        let result = summarize_in_chunks(&clients, |_| async { Err(ReadError::Refused) }).await;
        assert!(
            result.is_err(),
            "a zero summary would read as no reputation"
        );
    }

    /// Read-only, against the public Arc testnet RPC: `getClients`, the single
    /// call that fails, then the fallback. Run explicitly:
    /// `cargo test --lib erc8004::summary -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "Read-only live RPC check against Arc testnet; run explicitly"]
    async fn live_arc_testnet_agent_one_is_read_in_groups() {
        use alloy::providers::ProviderBuilder;
        use alloy::rpc::client::RpcClient;
        use alloy::transports::layers::RetryBackoffLayer;
        // The retry layer production's EVM providers carry (`EvmProvider::try_new`):
        // a public RPC answers a burst with 429, and production retries it.
        let client = RpcClient::builder()
            .layer(RetryBackoffLayer::new(3, 200, 10_000_000))
            .http("https://rpc.testnet.arc.io".parse().expect("url"));
        let rpc = ProviderBuilder::new().connect_client(client);
        let registry = super::super::abi::IReputationRegistry::new(
            super::super::ARC_TESTNET_CONTRACTS.reputation_registry,
            rpc,
        );
        let agent = U256::from(1);
        let clients = registry.getClients(agent).call().await.expect("getClients");
        let single = registry
            .getSummary(agent, clients.clone(), String::new(), String::new())
            .call()
            .await;
        assert!(
            single.as_ref().is_err_and(is_refusal),
            "premise: the single call is refused"
        );

        let started = std::time::Instant::now();
        let (summary, coverage) = summarize_in_chunks(&clients, |group| {
            let registry = registry.clone();
            async move { read_group(&registry, agent, group, String::new(), String::new()).await }
        })
        .await
        .expect("the readable clients are summarized");
        println!(
            "clients {} read {} unreadable {:?} not read {} calls {} in {:?}: count {} value {} decimals {}",
            coverage.clients_total,
            coverage.clients_read,
            coverage.unreadable_clients,
            coverage.clients_not_read,
            coverage.calls,
            started.elapsed(),
            summary.count,
            summary.value,
            summary.decimals
        );
        assert!(coverage.clients_read > 1000);
        assert!(coverage.calls <= MAX_CALLS);
    }

    /// `/docs` states the limits; this keeps the prose on the constants.
    #[test]
    fn the_documented_limits_are_these() {
        let openapi = include_str!("../openapi.rs");
        let stated = format!(
            "groups of {CLIENTS_PER_CALL} clients, at most {MAX_CALLS} calls and \
             {CALLS_IN_FLIGHT} at a time"
        );
        assert!(
            openapi.contains(&stated),
            "src/openapi.rs no longer says: {stated}"
        );
        assert!(openapi.contains(&format!(
            "up to {MAX_LISTED_UNREADABLE} `unreadableClients`"
        )));
    }

    #[test]
    fn mixed_decimals_take_the_most_used_like_the_registry() {
        let mut total = Aggregate::default();
        total
            .add(Summary {
                count: 3,
                value: 9_500,
                decimals: 2,
            })
            .unwrap();
        total
            .add(Summary {
                count: 1,
                value: 80,
                decimals: 0,
            })
            .unwrap();
        // (95 * 3 + 80) / 4 = 91.25, at the 2 decimals three of four use.
        assert_eq!(
            total.finish().unwrap(),
            Summary {
                count: 4,
                value: 9_125,
                decimals: 2
            }
        );
    }
}
