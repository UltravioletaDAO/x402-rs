# Premium RPC API key leaked to unauthenticated clients in escrow-state / escrow-settle / upto error responses (P2)

> Audit ID: `key-handling-and-secret-leakage` · Reported P1 → **verifier-adjusted P2** · Status: confirmed real, fix below is implementable cold.

## Summary

The EVM "exact" `/verify` and `/settle` path already redacts provider errors before returning them to the client (opaque message + correlation id at `src/handlers.rs:2127-2138`), but the **escrow**, **commerce**, **refund-extension**, **upto**, and **escrow-state** sub-paths do not. They interpolate the raw `alloy`/`reqwest` transport error straight into the HTTP JSON response body (`format!("...: {}", e)`). On a transport-layer error (timeout, connection-reset, DNS, 429 that surfaces as a transport error), that error string contains the full outbound RPC URL — which for production mainnet networks is a **premium, API-keyed** endpoint loaded from the `facilitator-rpc-mainnet` secret. An unauthenticated remote caller can therefore extract a production RPC credential from the response body of `POST /escrow/state` or `POST /settle`. The credential is paid infrastructure (not a signing key), so impact is availability/billing abuse (quota exhaustion → halts all on-chain reads/sends → halts settlements) rather than direct fund loss — hence P2, not P1.

## Root cause

### 1. The redactor exists but is only wired into startup logging, never into error responses

`src/redact.rs:17` — `rpc_url()` correctly strips the API key from a single URL, but it is only called at provider-init log sites (`src/chain/evm.rs:265`, `stellar.rs:481`, `sui.rs:737`, `xrpl.rs:294`). No error-formatting site and no response-builder uses it. It also cannot help the leak below: it parses **one** URL, whereas the leak is a URL embedded *inside* a longer alloy error string — `url::Url::parse` on that whole string fails and returns `"<redacted-rpc>"`, discarding the (still useful) error context. We need a `scrub_urls()` that replaces every `https?://…` substring inside an arbitrary string.

### 2. Errors carry the URL because alloy/reqwest put it there

`src/chain/evm.rs:239-241` builds the provider over the reqwest HTTP transport:

```rust
let client = RpcClient::builder()
    .connect(rpc_url)            // http(s) -> alloy-transport-http (reqwest)
    .await
```

On a transport failure, `reqwest::Error`'s `Display`/`Debug` emit ` for url ({url})` / `field("url", &url.as_str())` (reqwest 0.12.x — confirmed in the verifier's lockfile read). `alloy-transport`'s `TransportErrorKind::Custom(#[error("{0}")] ...)` and `alloy-json-rpc`'s `RpcError::Transport(#[error(transparent)])` both delegate to that inner reqwest error, so **both `{}` and `{:?}` reproduce the API-keyed URL**.

### 3. The five error-formatting sites embed that error verbatim, and the handlers return it to the client

Error sources (each wraps the alloy error into a `String` carrying the URL):

| File:line | Code (verbatim) |
|---|---|
| `src/payment_operator/operator.rs:909` | `.map_err(\|e\| OperatorError::ContractCall(format!("{:?}", e)))?;` |
| `src/payment_operator/operator.rs:930` | `.map_err(\|e\| OperatorError::ContractCall(format!("eth_call failed: {:?}", e)))?;` |
| `src/escrow.rs:862` | `.map_err(\|e\| EscrowError::ContractCall(format!("{:?}", e)))?;` |
| `src/upto/permit2.rs:370` | `UptoError::VerificationFailed(format!("settlement simulation reverted: {e}"))` |
| `src/upto/permit2.rs:399` | `.map_err(\|e\| UptoError::SettlementFailed(format!("{e}")))?;` |
| `src/upto/permit2.rs:503` | `.map_err(\|e\| UptoError::ContractCall(format!("eth_call failed: {e}")))?;` |

The `#[error("Contract call failed: {0}")]` / `#[error("Settlement failed: {0}")]` thiserror derives (`src/payment_operator/errors.rs:55-56`, `src/upto/errors.rs:27-34`, `src/escrow.rs:251-252`) then surface that string via `Display`.

Handler sinks that put `Display` of those errors **into the client response body**:

| File:line | Code (verbatim) — reachable, unauthenticated |
|---|---|
| `src/handlers.rs:1117` | `"invalidReason": format!("Escrow verification error: {}", e)` (verify, nested) |
| `src/handlers.rs:1155` | `"invalidReason": format!("Escrow verification error: {}", e)` (verify, top-level) |
| `src/handlers.rs:1618` | `"errorReason": format!("Upto scheme error: {}", e)` (settle, upto) |
| `src/handlers.rs:1654` | `"errorReason": format!("Escrow scheme error: {}", e)` (settle, escrow nested) |
| `src/handlers.rs:1692` | `"errorReason": format!("Escrow scheme error: {}", e)` (settle, escrow top-level) |
| `src/handlers.rs:1733` | `"errorReason": format!("Escrow error: {}", e)` (settle, refund extension) |
| `src/handlers.rs:2242` | `"error": format!("Escrow state query failed: {}", e)` (`POST /escrow/state`) |

All of these are reachable in production: `ENABLE_PAYMENT_OPERATOR=true`, `ENABLE_ESCROW=true`, `ENABLE_UPTO=true`, and none of `/settle`, `/verify`, `/escrow/state` require authentication.

### 4. Server-side logs leak it too (defense-in-depth gap)

The same unredacted error is logged with `error = %e` at `src/handlers.rs:1112,1150,1613,1649,1687,1728,2238` and `warn!(error = %e, ...)` at `src/upto/permit2.rs:369`. Per project policy ("All facilitator log output may be viewed live by the user on stream" — `redact.rs:1-4`), these log lines also leak the API key on stream.

REDACTED example of the string an attacker receives today (key masked here; live response would show the real key):

```
{"error":"Escrow state query failed: Contract call failed: eth_call failed:
 Transport(Custom(reqwest::Error { kind: Request,
 url: \"https://<host>.quiknode.pro/<REDACTED_API_KEY>/\", source: ... }))"}
```

## Exploit

Production config has all three sub-systems enabled and the routes are unauthenticated (`recon.md` §1, §13).

1. Send `POST /escrow/state` (or `POST /settle` with `scheme=escrow` or `scheme=upto`) with a structurally valid v2 escrow/upto body targeting a mainnet network with a premium API-keyed RPC (e.g. Base / Arbitrum / Avalanche). `validate_addresses(..., false)` is lenient (`operator.rs:935-948`), so address checks do not block reaching the on-chain call.
2. Induce an `alloy` transport error on the `eth_call` / `send_transaction`: sustain load until the provider returns 429 / drops the connection (reqwest surfaces these as `Transport` errors that carry `.with_url(...)`), or simply catch any transient RPC outage/timeout — these happen naturally and frequently. (The leak only fires on transport-layer errors; JSON-RPC reverts go through `ErrorResp`, which does not carry the URL. This is why severity is P2 — opportunistic, not on-demand.)
3. Read the JSON `error` / `errorReason` / `invalidReason` field. It contains `... reqwest::Error { ... url: "<api-keyed RPC URL>" ... }`.
4. Extract the API key from the URL path/query and abuse the premium endpoint: exhaust its quota (DoS — halts the facilitator's on-chain reads/sends, which halts *all* settlements), and run up the operator's RPC bill.

## Fix

Three layers. Layer 1 kills the leak at the source even if a future handler re-interpolates the error; Layer 2 makes the handlers opaque like the EVM-exact path; Layer 3 scrubs the server-side logs. Implement all three.

### Layer 1 — collapse transport errors at the `map_err` source (highest leverage)

Map `alloy` `RpcError` variants explicitly: keep the safe `ErrorResp` revert data, drop the URL-bearing `Transport` inner error entirely. Add a shared helper next to the existing error types so all five sites use it.

**New helper — add to `src/redact.rs`** (after `rpc_url`, before `#[cfg(test)]` at line 33):

```rust
use std::sync::LazyLock;

/// Matches any http/https URL substring (greedy to end of token).
static URL_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"https?://\S+").expect("valid url regex"));

/// Replace every `http(s)://…` substring in an arbitrary string with
/// `<redacted-url>`. Use this on any error string before it is logged or
/// returned to a client, because alloy/reqwest transport errors embed the
/// full (API-keyed) RPC URL in their Display/Debug output.
pub fn scrub_urls(raw: &str) -> String {
    URL_RE.replace_all(raw, "<redacted-url>").into_owned()
}
```

> `regex` (1.11.1), `url` (2.5.8) and `uuid` (1.21, v4) are already in `Cargo.toml` — no new dependency. `std::sync::LazyLock` is stable on the project's Rust 1.82 (CLAUDE.md edition 2021).

**Then change the six source sites to scrub before constructing the error string.** Before → after:

`src/payment_operator/operator.rs:909`
```rust
// before
.map_err(|e| OperatorError::ContractCall(format!("{:?}", e)))?;
// after
.map_err(|e| OperatorError::ContractCall(crate::redact::scrub_urls(&format!("{e:?}"))))?;
```

`src/payment_operator/operator.rs:930`
```rust
// before
.map_err(|e| OperatorError::ContractCall(format!("eth_call failed: {:?}", e)))?;
// after
.map_err(|e| OperatorError::ContractCall(crate::redact::scrub_urls(&format!("eth_call failed: {e:?}"))))?;
```

`src/escrow.rs:862`
```rust
// before
.map_err(|e| EscrowError::ContractCall(format!("{:?}", e)))?;
// after
.map_err(|e| EscrowError::ContractCall(crate::redact::scrub_urls(&format!("{e:?}"))))?;
```

`src/upto/permit2.rs:370`
```rust
// before
UptoError::VerificationFailed(format!("settlement simulation reverted: {e}"))
// after
UptoError::VerificationFailed(crate::redact::scrub_urls(&format!("settlement simulation reverted: {e}")))
```

`src/upto/permit2.rs:399`
```rust
// before
.map_err(|e| UptoError::SettlementFailed(format!("{e}")))?;
// after
.map_err(|e| UptoError::SettlementFailed(crate::redact::scrub_urls(&format!("{e}"))))?;
```

`src/upto/permit2.rs:503`
```rust
// before
.map_err(|e| UptoError::ContractCall(format!("eth_call failed: {e}")))?;
// after
.map_err(|e| UptoError::ContractCall(crate::redact::scrub_urls(&format!("eth_call failed: {e}"))))?;
```

Why this closes the hole: the URL never enters the `OperatorError`/`UptoError`/`EscrowError` string in the first place, so *any* downstream interpolation (`{}` or `{:?}`, response or log) is automatically safe. This is the defense the EVM-exact path lacks and the reason the EVM path's opaque-message trick is incidental rather than load-bearing.

### Layer 2 — make the seven handler sinks opaque (mirror the EVM-exact pattern at `handlers.rs:2127-2138`)

Even with Layer 1, return an opaque message + correlation id so revert reasons and any future un-scrubbed error can't leak, matching the existing `FacilitatorLocalError::ContractCall` handling. `uuid::Uuid::new_v4()` is already used inline in this file (e.g. `handlers.rs:2130`), no import needed.

`src/handlers.rs:1117` and `:1155` (verify escrow, both branches) — before → after:
```rust
// before
Err(e) => {
    error!(error = %e, "Escrow verification failed");
    return (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "isValid": false,
            "invalidReason": format!("Escrow verification error: {}", e)
        })),
    ).into_response();
}
// after
Err(e) => {
    let id = uuid::Uuid::new_v4();
    error!(%id, error = %crate::redact::scrub_urls(&e.to_string()), "Escrow verification failed");
    return (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "isValid": false,
            "invalidReason": format!("escrow_verification_failed (ref: {id})")
        })),
    ).into_response();
}
```

Apply the identical shape to the settle sinks, keeping each one's existing JSON field name and label:

- `src/handlers.rs:1613/1618` (upto): log → `error!(%id, error = %crate::redact::scrub_urls(&e.to_string()), "Upto settlement failed");` body → `"errorReason": format!("upto_failed (ref: {id})")`.
- `src/handlers.rs:1649/1654` (escrow nested) and `:1687/1692` (escrow top-level): body → `"errorReason": format!("escrow_failed (ref: {id})")`.
- `src/handlers.rs:1728/1733` (refund extension): body → `"errorReason": format!("escrow_failed (ref: {id})")`.
- `src/handlers.rs:2238/2242` (`/escrow/state`): log → `error!(%id, error = %crate::redact::scrub_urls(&e.to_string()), "Escrow state query failed");` body → `"error": format!("escrow_state_failed (ref: {id})")`.

### Layer 3 — scrub the server-side log lines

The `error!(error = %e, …)` / `warn!(error = %e, …)` calls at `handlers.rs:1112,1150,1613,1649,1687,1728,2238` and `upto/permit2.rs:369` print to a stream that may be live-viewed. After Layer 1 the `e` reaching these is already scrubbed for the escrow/upto error strings, but the **direct** alloy error at `permit2.rs:369` (`warn!(error = %e, "Upto settlement simulation failed")`) is the raw transport error and is still unredacted. Change it:

`src/upto/permit2.rs:369`
```rust
// before
warn!(error = %e, "Upto settlement simulation failed");
// after
warn!(error = %crate::redact::scrub_urls(&e.to_string()), "Upto settlement simulation failed");
```

(Layer 2 already wraps the handler log lines with `scrub_urls`.)

## Test plan

Add Rust `#[test]`s. The redactor test is pure and fast; the handler/error tests are unit-level over the error string, avoiding live RPC.

1. **`src/redact.rs` — extend the existing `mod tests` (currently 4 tests):**
   - `scrub_urls_strips_quicknode_in_error_string`: input
     `r#"Contract call failed: eth_call failed: Transport(Custom(reqwest::Error { url: "https://x.quiknode.pro/SECRETKEY123/", source: ... }))"#`
     → assert the result does **not** contain `"SECRETKEY123"`, does **not** contain `"quiknode.pro"`, and **does** contain `"<redacted-url>"` and still contains `"eth_call failed"` (context preserved).
   - `scrub_urls_strips_infura_query`: input with `https://mainnet.infura.io/v3/DEADBEEF…` → assert `"DEADBEEF"` absent, `"<redacted-url>"` present.
   - `scrub_urls_handles_multiple_urls`: a string with two distinct API-keyed URLs → assert neither key survives and two `<redacted-url>` markers exist.
   - `scrub_urls_noop_without_url`: `"plain revert: insufficient allowance"` → returned unchanged.

2. **`src/payment_operator/operator.rs` `mod tests` (and mirror in `escrow.rs`, `upto/permit2.rs`):**
   - `contract_call_error_display_has_no_url`: construct `OperatorError::ContractCall(crate::redact::scrub_urls(&format!("{e:?}")))` from a synthetic error string carrying `https://host/SECRET/` and assert `error.to_string()` contains neither `"SECRET"` nor `"host"`. Repeat with `EscrowError::ContractCall` and `UptoError::SettlementFailed`.

3. **Handler-level (extend the integration suite under `tests/`):** a `/escrow/state` request that forces an error path (e.g. point a test network at an unreachable RPC, or inject a provider that returns a `Transport` error) and assert the JSON `error` field matches `^escrow_state_failed \(ref: [0-9a-f-]+\)$` and contains no `http`. If a live-RPC harness is unavailable, this can be a documented manual `curl` step (see Verification) rather than a CI test.

## Rollback notes

- Pure additive + string-swap change; no schema, DB, or on-chain change. Roll back by reverting the diff in `src/redact.rs`, `src/payment_operator/operator.rs`, `src/escrow.rs`, `src/upto/permit2.rs`, `src/handlers.rs`.
- Behavioral change visible to clients: error **bodies** become opaque (`escrow_failed (ref: <uuid>)`) instead of verbose. If any downstream integration parses the old verbose `errorReason` text (it should not — these are human-readable strings), it will need to use the correlation id + server logs instead. The `success:false` / `isValid:false` flags and HTTP status codes are unchanged.
- No key rotation is required to roll back. (See residual risk for rotation of the *currently-exposed* key, which is independent of this code change.)

## Verification

Build and run locally, then confirm the leak is closed on each path:

1. Build: `cargo build --release` (do **not** auto-deploy — per CLAUDE.md the user deploys manually).
2. Unit tests: `cargo test -p x402-rs redact::tests` then `cargo test scrub_urls contract_call_error_display_has_no_url`.
3. Reproduce the error path against a deliberately-bad RPC. With the facilitator running and an unreachable/placeholder mainnet RPC configured (so the on-chain call hits a transport error):
   ```bash
   curl -s -X POST http://localhost:8080/escrow/state \
     -H 'content-type: application/json' \
     -d '{"network":"base","paymentInfo":{ ... minimal valid escrow-state body ... }}' \
   | tee /tmp/resp.json
   # MUST be empty (closed):
   grep -Eo 'https?://[^"]+' /tmp/resp.json || echo "OK: no URL in response"
   ```
   Repeat for `POST /settle` with `scheme=upto` and `scheme=escrow` bodies.
   - **Before fix:** response `error`/`errorReason` contains `reqwest::Error { ... url: "https://…/<key>/" ... }`.
   - **After fix:** response is `{"error":"escrow_state_failed (ref: <uuid>)"}` (or `escrow_failed`/`upto_failed`), and `grep` finds no `http`.
4. Confirm the server log line for the same request shows `<redacted-url>` (not the key) — e.g. `RUST_LOG=info` and inspect stdout: the `error = …` field must not contain the API key.
5. Production smoke (post-deploy): `curl -s -X POST https://facilitator.ultravioletadao.xyz/escrow/state -d '{bad body}'` and confirm the response carries no `http(s)://` substring.

## Residual risk / related findings

- **Rotate any exposed RPC key.** Because the leak was reachable by unauthenticated callers in production for the deployed window, treat any premium key in `facilitator-rpc-mainnet` that could have been surfaced as potentially exposed and rotate it after the fix ships (update the secret via `aws secretsmanager update-secret --secret-id facilitator-rpc-mainnet`). This is operational, not a code change.
- **Provider-init connect error** at `src/chain/evm.rs:241` (`format!("Failed to connect to {network}: {e}")`) also embeds the URL via `{e}`, but it only flows to startup/`main` and is not returned in a client response — lower priority; wrap it in `scrub_urls` opportunistically while in the file.
- **Out of scope here (separate findings):** these escrow/commerce/upto paths *also* bypass OFAC/blacklist compliance screening (findings `blocklist-enforcement-coverage` / `payment-operator-escrow-authz`) and the escrow release/refund operator-authz gap. This fix only closes the secret-leak channel; it does not address those. Fixing them in the same handlers is complementary but must be tracked independently.
- After this fix the residual leak vector is effectively nil for URLs; the remaining theoretical risk is a future contributor adding a new escrow/upto error path that interpolates a raw alloy error without `scrub_urls`. Layer 1 (scrub at the `map_err` source for the known sinks) plus a grep-able convention (`scrub_urls` on every alloy error string) mitigates this; consider a clippy-style lint or a code-review checklist item.
