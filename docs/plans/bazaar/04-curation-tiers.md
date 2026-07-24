# WS-C — Curation Tiers, VIP Manifest & Admin API

**Ships as**: v1.52.0 (with WS-B) · **Depends on**: WS-A merge; full ordering effect needs WS-B health
**Effect**: Execution Market, MeshRelay and 402Milly always rank first; curated VIP partners (Tenjin) next; tier + curation metadata exposed in the API; internal debug entries delisted; an authenticated admin path exists for delist/suppress/release.

## 1. Tier model

| Tier | Who | How assigned | Ordering rank |
|---|---|---|---|
| `first_party` | Execution Market, MeshRelay, 402Milly | manifest (`config/bazaar_curation.json`) + origin proof | 0 (always first) |
| `vip` | curated partners (future: onboarded via criteria §5) | manifest + origin proof | 1 |
| `verified` | any listing whose latest probe = `alive` (402-verified) — incl. **Tenjin until origin-proven** (08 §10) | automatic (WS-B) | 2 |
| `listed` | passes WS-A static filters, health unknown/degraded/auth_gated | automatic | 3 |

- Tier is a **trust label**; health is a **liveness state** — they are orthogonal. `quarantined` is a *health* state (hidden by default), NOT a tier — do not model it as a tier row. Visibility = f(tier, health): a quarantined first_party/VIP is hidden (health always wins; we never showcase a dead partner). The UI may show first-party cards from the manifest even when the registry item is missing (hardcoded hydration, see WS-D) — but the API never fabricates items.
- Default ordering in `list()` (`src/discovery.rs:337-338`) becomes: `tier rank ASC, health rank ASC (alive < auth_gated < degraded < unknown), last_updated DESC`.
- New query param `tier=first_party|vip|verified|listed` (additive to existing filters in `matches_filters`, `src/discovery.rs:451-530`).

## 2. Response shape (additive, spec-compliant — bazaar.md leaves item schema open; CDP `curated`/`quality` precedent)

```jsonc
{
  "url": "https://api.meshrelay.xyz/payments/access/alpha-test",
  "...": "existing fields unchanged",
  "curation": {                    // omitted entirely when tier == "listed" AND no flags set
    "tier": "first_party",
    "label": "MeshRelay",          // display name from manifest
    "firstParty": true,
    "nonTls": false,               // true for http:// listings (11.5% of catalog) — UI badges it (fulfils 02 SS6.2 promise)
    "routeTemplate": false         // true for /:param template URLs (R6) — UI shows "template" chip, unprobeable
  },
  "health": { "status": "alive", "lastChecked": 1784800000, "latencyMs": 240 }  // WS-B
}
```

## 3. `config/bazaar_curation.json` — the manifest

Loaded at runtime (like `config/blacklist.json` via `fs::read_to_string`, NOT `include_str!` — that precedent was misstated; see 02 §2.3), env override `BAZAAR_CURATION_PATH` (default `config/bazaar_curation.json`) so adding a VIP does not require a full rebuild+deploy.

**Matching is host-exact + path-boundary on the PARSED URL, never `starts_with` on a raw string (08 §1/F1 — CRITICAL).** The naive "lowercase host, no trailing slash, string prefix" would award `first_party` to `https://api.meshrelay.xyz.evil.com/` and `https://api.meshrelay.xyz@evil.com/`, which an attacker can register via the public `POST /discovery/register`. Use `canonical_url` (08 §0) then: exact host equality (or explicit `.subdomain` suffix), path equals manifest path or begins with a `/`-terminated manifest path, scheme https. Unit-test every F1 payload → resolves to `listed`.

**Every URL/payTo below is live-verified evidence from the 2026-07-23 product inventory (01 §6) — do not add entries from memory (project rule: never type addresses from memory).**

```jsonc
{
  "$comment": "Curated tiers. urlPrefix matching, longest-prefix wins.",
  "entries": [
    {
      "name": "Execution Market",
      "tier": "first_party",
      "urlPrefixes": ["https://mcp.execution.market/", "https://api.execution.market/"],
      "homepage": "https://execution.market",
      "expectedPayTo": ["0x857fe6150401bFB4641Fe0D2B2621cc3B05543Cd"],
      "$evidence": "bazaar self_registered listing + api.execution.market/openapi.json (402 on POST /api/v1/tasks)"
    },
    {
      "name": "MeshRelay",
      "tier": "first_party",
      "urlPrefixes": ["https://api.meshrelay.xyz/", "https://meshrelay.xyz/"],
      "homepage": "https://meshrelay.xyz",
      "expectedPayTo": ["0xe4dc963c56979E0260fc146b87eE24F18220e545"],
      "$evidence": "live 402 probe POST /payments/access/alpha-test 2026-07-23 (x402 v2 body, Base + SKALE Base)"
    },
    {
      "name": "402Milly",
      "tier": "first_party",
      "urlPrefixes": ["https://mcp.402milly.xyz/", "https://api.402milly.xyz/"],
      "homepage": "https://402milly.xyz",
      "expectedPayTo": ["0x80238a1C73367591BF17e2f4DBAc652e479b077A"],
      "$evidence": "live 402 probe POST api.402milly.xyz/purchase 2026-07-23 + bazaar self_registered listing"
    },
    {
      "name": "Tenjin",
      "tier": "verified",
      "urlPrefixes": ["https://tenjin.blog/"],
      "homepage": "https://tenjin.blog",
      "$evidence": "121 self_registered items in our bazaar; live 402 with v2 PAYMENT-REQUIRED header 2026-07-23. Identity CONFIRMED by user 2026-07-23: 'tenjin.blog no tangent' — the requested VIP is Tenjin (no x402 product named 'Tangent' exists, 01 SS6).",
      "$note": "Shipped as VERIFIED, not vip (08 SS10/F10): payTo varies per creator so there is no expectedPayTo baseline, and a bare URL-prefix is not proof of partnership — a feed-poisoned tenjin.blog item with attacker payTo would otherwise ride the vip tier with no drift alarm (08 SS4/F4). User identity gate is now satisfied; promote to vip once Tenjin serves an origin proof (.well-known/x402 on its domain). Until then it earns rank via health like any other verified listing."
    }
  ],
  "suppressed": [
    { "url": "https://facilitator.ultravioletadao.xyz/__bazaar_debug__", "reason": "internal debug entry" }
  ]
}
```

`expectedPayTo` doubles as a **first-party payTo allowlist**: if a probe or feed update shows a first_party/vip listing paying to an address outside the manifest, quarantine + WARN + **page an operator** immediately (impersonation/hijack defense; note this path is attacker-triggerable via feed collision — 08 §4/F4). **Address comparison MUST be per-chain-normalized** (EVM lowercase/EIP-55 canonical, Solana base58 canonical — reuse payment-path address types, 08 §10/F10) or a legit checksum-case difference false-quarantines our own product. For a manifest-matched URL, feed-supplied `accepts`/`payTo` are ignored entirely — the manifest is authoritative (08 §4).

`suppressed` = permanent manifest-level delist (survives re-imports; checked in `matches_filters`).

## 4. Curation state overlay + admin API

Manifest handles the *planned* curation; ops needs a *runtime* path for surprises (spam waves, malicious listings, DMCA-style requests):

- Overlay `bazaar/curation-state.json` (same debounced-snapshot mechanism as health, WS-B §5): `{canonical_url → {suppressed: bool, reason, at, by}}` — keyed by `canonical_url` (08 §0), suppression supports host+path-prefix (safe matcher, F1) so `?x=1`/trailing-dot/slash variants can't evade a delist (08 §13/F13).
- Admin endpoints — **security spec in 08 §8/F8, do not implement without it**: wired onto the **governored** `discovery_register` router (`src/main.rs:355-357`), NOT the ungoverned `discovery_routes` (`:385`); `Authorization: Bearer $BAZAAR_ADMIN_TOKEN` compared **constant-time** (`subtle::ConstantTimeEq`) only after the 404-when-unset check; `Authorization` redacted in the tracing layer and never in `#[instrument]` fields; `by` audit field **server-derived** (never from request body):
  - `DELETE /discovery/resources?url=...` → `canonical_url` the param first, then `registry.unregister()` (exists unexposed, `src/discovery.rs:285`) + tombstone (WS-B §2); return which normalized key was acted on.
  - `POST /discovery/admin/suppress {url, reason}` / `POST /discovery/admin/release {url}` — soft hide/unhide; also releases WS-B security quarantines (payTo rotation approvals).
- Fixes en passant: the 409 response of `POST /discovery/register` references a nonexistent `PUT /discovery/resources/{url}` (`src/handlers.rs:375`) — reword the hint.

## 5. VIP onboarding criteria (future externals — the "curated VIP endpoints" process)

A listing qualifies for `vip` when ALL hold (mirrors CDP's ~99%-availability curated bar, plus our on-chain leverage):
1. 30-day probe uptime ≥ 99% (WS-B data) and currently `alive`.
2. ≥ N settled payments in 30 days through any facilitator we can verify (ours via `settlement_count`; start N=10).
3. Origin proof: serves `.well-known/x402` on its domain, or its bazaar entry is `self_registered` from the product team.
4. Manual review + manifest PR (curation stays a human decision; automation feeds the shortlist).
5. (Phase 4) ERC-8004 identity + prober-attested uptime — see `07-erc8004-attested-curation.md`.

A quarterly review demotes VIPs that fell below the bar (probe data makes this a query, not an investigation).

## 6. Phase 0 housekeeping (ops-only, can run TODAY — before any deploy)

Ready-to-run actions against production (all data live-verified in 01 §6):

```bash
# 1) MeshRelay is NOT listed at all — register it (via the public, validated register path).
#    Uses the channel-catalog endpoint as the canonical discoverable URL; per-channel 402s hang off it.
curl -X POST https://facilitator.ultravioletadao.xyz/discovery/register \
  -H 'Content-Type: application/json' \
  -d '{
    "url": "https://api.meshrelay.xyz/payments/access/alpha-test",
    "type": "http",
    "description": "MeshRelay premium IRC channel access for agents and humans. Pay-per-access channels (catalog: https://api.meshrelay.xyz/payments/channels). x402 v2, USDC on Base and SKALE Base.",
    "accepts": [{
      "scheme": "exact",
      "network": "eip155:8453",
      "asset": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
      "amount": "100000",
      "payTo": "0xe4dc963c56979E0260fc146b87eE24F18220e545",
      "maxTimeoutSeconds": 300
    }],
    "metadata": { "provider": "MeshRelay", "category": "communication", "tags": ["irc", "chat", "agents", "x402", "skale"] }
  }'
# NOTE: amounts/payTo above are from the live 402 body of /payments/access/alpha-test (01 SS6).
# Repeat per premium channel, or wait for WS-A and let MeshRelay serve .well-known/x402 + enable the crawler for our own domains.

# 2) Register the missing first-party REST endpoints (MCP wrappers are listed; REST is not):
#    - POST https://api.execution.market/api/v1/tasks   (verify exact accepts with EM's team first - only its OpenAPI 402 is confirmed, not the terms)
#    - POST https://api.402milly.xyz/purchase           (accepts known: $1.00 USDC; today it answers v1-style JSON - registering it is fine, but ALSO file the 402milly-side fix below)

# 3) Delist the debug entry - blocked until WS-C ships the admin DELETE (unregister is not exposed today).
#    Until then it is in the manifest suppressed[] list, which hides it at WS-C deploy time.

# 4) External-side fixes to file (not this repo):
#    - MeshRelay landing: meta tag agent:payments-endpoint points to /turnstile which 404s -> point to /payments/access/{channel}
#    - 402milly API: /purchase advertises chain IDs 998/1301 (HyperEVM mainnet is 999, Unichain is 130) and replies v1-style JSON -> upgrade to x402 v2 accepts array
#    - 402milly bazaar entry: non-EVM rails (Solana/NEAR/Stellar/Algorand/Sui) it actually supports are absent from its accepts
```

## 7. Files touched

| File | Change | Est. LOC |
|---|---|---|
| `config/bazaar_curation.json` (new) | manifest above | ~60 |
| `src/discovery_curation.rs` | manifest loader, prefix matcher, tier resolver, suppression | ~180 (shared file with WS-A rules) |
| `src/discovery.rs` | tier-aware sort, `tier` filter, suppression in `matches_filters` | ~40 |
| `src/types_v2.rs` | `curation` response field | ~25 |
| `src/handlers.rs` | `tier` param; admin endpoints | ~90 |
| `src/main.rs` | admin token env, route wiring | ~15 |
| `src/openapi.rs` | **rewrite stale Bazaar docs** (wrong params/shapes at `openapi.rs:1004-1085`) + new params/fields/admin | ~80 |

## 8. Tests

- Prefix matching: longest-prefix wins; `https://tenjin.blog/api/read/x/y` → vip; unrelated host → none.
- Ordering: first_party < vip < verified < listed; quarantined absent by default; `health=any&tier=...` combinations.
- Suppression: manifest + runtime overlay both hide; release restores.
- Admin: no token env → 404; wrong token → 401; governor applies.
- first-party payTo allowlist: foreign payTo on a first_party prefix → quarantined.

## 9. Verification after deploy

```bash
curl -s '.../discovery/resources?limit=5' | jq '[.items[].curation.tier]'   # first_party first
curl -s '.../discovery/resources?tier=verified&limit=3' | jq '.items[].url' # includes tenjin.blog items
curl -s '.../discovery/resources?limit=100' | jq '[.items[].url | select(contains("__bazaar_debug__"))] | length'  # 0
# admin auth: LOCAL (token unset) returns 404; PROD (token set) returns 401 without a valid bearer
curl -s -o /dev/null -w '%{http_code}' -X POST '.../discovery/admin/suppress' -d '{}'   # local:404  prod:401
```
