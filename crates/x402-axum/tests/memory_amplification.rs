//! What the memory budget is actually worth.
//!
//! `EvidenceBudget` charges a capture `MEMORY_AMPLIFICATION` times the body
//! size before a byte is buffered. That factor was written as an estimate, and
//! the handoff that asked for a bigger body limit listed the measurement as the
//! missing piece. An estimate is a poor thing to size an OOM guard with: set it
//! too low and a burst the budget cheerfully admits still kills the task, which
//! is the exact failure the budget exists to prevent.
//!
//! So this measures it. A counting allocator watches one whole capture --
//! plaintext, keccak, seal, envelope serialisation, sink write -- and reports
//! the real peak against the body size.
//!
//! It lives in `tests/` rather than beside the unit tests because a
//! `#[global_allocator]` applies to a whole binary, and the unit tests should
//! not pay for the counting.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use x402_axum::durable::{
    DurableConfig, DurableEvidenceHook, EvidenceSink, SettledContext, MEMORY_AMPLIFICATION,
};
use x402_rs::dx402::envelope::PayerPublicKey;
use x402_rs::dx402::types::DurablePointer;
use x402_rs::network::Network;
use x402_rs::types::MixedAddress;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// Tracks live heap bytes and the high-water mark.
///
/// The default `realloc` on this trait routes through `alloc`/`dealloc`, so a
/// growing `Vec` is accounted without any extra work here.
struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// A sink that keeps the blob, the way a real one holds a request body while it
/// uploads. A sink that dropped the bytes on the floor would measure a cost
/// nobody actually pays.
#[derive(Debug, Default)]
struct HoldingSink {
    held: std::sync::Mutex<Vec<u8>>,
}

#[async_trait::async_trait]
impl EvidenceSink for HoldingSink {
    async fn put(&self, payment_id: &str, blob: &[u8]) -> Result<DurablePointer, String> {
        *self.held.lock().unwrap() = blob.to_vec();
        Ok(DurablePointer(format!("mem://{payment_id}")))
    }
}

fn addr(s: &str) -> MixedAddress {
    serde_json::from_value(serde_json::Value::String(s.to_string())).unwrap()
}

fn ctx() -> SettledContext {
    SettledContext {
        payment_id: format!("0x{}", "11".repeat(32)),
        network: Network::Base,
        tx_hash: format!("0x{}", "33".repeat(32)),
        payer: addr("0x103040545AC5031A11E8C03dd11324C7333a13C7"),
        payee: addr("0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8"),
        proof: None,
        offer: None,
    }
}

/// One capture, end to end, with the facilitator unreachable on purpose -- the
/// receipt is not what costs memory, the body is.
async fn one_capture(body_bytes: usize) {
    let sk = k256::SecretKey::random(&mut rand::rngs::OsRng);
    let payer_key = PayerPublicKey::Secp256k1(Box::new(sk.public_key()));
    let hook = DurableEvidenceHook::new(
        DurableConfig::default(),
        Arc::new(HoldingSink::default()),
        "http://127.0.0.1:1",
    );
    // Not zeroes: a compressible body would flatter any layer that compresses.
    let body: Vec<u8> = (0..body_bytes).map(|i| (i % 251) as u8).collect();
    let _ = hook.capture(&body, Ok(payer_key), &ctx()).await;
}

/// Peak heap over baseline for one capture of `body_bytes`, as a multiple of
/// the body.
///
/// Warms up first: one-time allocations -- lazy statics, the HTTP client's
/// internals, k256 tables -- are real, but they are paid once per process, not
/// once per capture, and billing them to the body would inflate the factor.
async fn measure(body_bytes: usize) -> f64 {
    one_capture(body_bytes).await;

    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);

    one_capture(body_bytes).await;

    let peak = PEAK.load(Ordering::Relaxed);
    let over_baseline = peak.saturating_sub(baseline);
    let measured = over_baseline as f64 / body_bytes as f64;
    println!(
        "  {:>5} KiB body -> peak {over_baseline:>12} bytes = {measured:.2}x",
        body_bytes / 1024
    );
    measured
}

// ONE test, not two. `LIVE`/`PEAK` are process-global, so two measuring tests
// in the same binary race each other's high-water mark and fail only under the
// default parallel runner -- a flake that looks like a real regression.

#[tokio::test]
async fn the_budget_factor_covers_what_a_capture_actually_allocates() {
    // The factor was once measured on a 4 MiB body and applied to a 32 MiB one.
    // That extrapolation is only sound if the ratio is flat with size, and
    // nothing guarantees it is: fixed envelope overhead shrinks relative to a
    // bigger body, while a reallocation that doubles a growing buffer does not.
    // So measure the range, and hold the budget to the WORST of it -- the
    // number that has to be right is the one at the ceiling, because that is
    // the capture the budget is sized for.
    println!("amplification across the range:");
    let mut worst: f64 = 0.0;
    for body in [
        1024 * 1024,
        4 * 1024 * 1024,
        16 * 1024 * 1024,
        DurableConfig::default().max_body_bytes,
    ] {
        worst = worst.max(measure(body).await);
    }

    assert!(
        worst <= MEMORY_AMPLIFICATION as f64,
        "a capture peaked at {worst:.2}x the body but the budget only charges \
         {MEMORY_AMPLIFICATION}x, so the budget admits bursts it cannot afford \
         -- raise MEMORY_AMPLIFICATION to at least {} and re-check the default \
         limits",
        worst.ceil() as usize
    );

    // The other direction matters too, just less urgently: a factor far above
    // the truth spends the budget on memory nobody uses, and turns real
    // captures into `busy` skips for no reason. One whole body of slack is
    // what this is meant to carry -- room for a copy somebody adds later
    // without re-running this, and no more.
    assert!(
        worst + 2.0 >= MEMORY_AMPLIFICATION as f64,
        "a capture only peaked at {worst:.2}x but the budget charges \
         {MEMORY_AMPLIFICATION}x; that over-reservation costs capacity, so \
         lower MEMORY_AMPLIFICATION toward {}",
        (worst + 1.0).ceil() as usize
    );
}
