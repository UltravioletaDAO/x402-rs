# WS-SEC — Security Hardening (Cross-Cutting, MANDATORY)

**Status**: mandatory across WS-A…WS-E · derived from an adversarial security review (2026-07-23) that read the real code paths this plan builds on.
**Two items MUST be fixed before their workstream ships**: **F1** (public register → instant first-party impersonation) blocks WS-C; **F2/F3** (live AWS-metadata SSRF once the prober fetches attacker URLs) block WS-B. Everything else is hardening to fold into the owning workstream.

> **CODE-COMPLETE 2026-07-23 (compiled clean, 8 unit tests green; pending user build+deploy).** The reusable core of F1 and F2 is implemented in `src/discovery_security.rs` (+ `src/discovery.rs`, wired into crawler + aggregator):
> - **F1 core**: `canonical_url` (§0) and `match_manifest_prefix` (§1) — the host-exact + path-boundary matcher that WS-C's manifest resolution will call. Register-path userinfo rejection also live in `validate_resource`.
> - **F2 core**: `check_url_target` (resolve → reject-if-any-disallowed → return pinned addrs) and `safe_get` (pin + `redirect(none)` + manual ≤3-hop re-check + port allowlist), plus `is_disallowed_target_ip` extended (`240/4`, 6to4) and `host_as_encoded_ipv4`. Crawler now fetches via `safe_get`; aggregator uses `aggregator_redirect_policy`.
> What remains is **wiring these primitives into the not-yet-built consumers**: WS-C's manifest resolver must call `match_manifest_prefix`; WS-B's prober must fetch via `safe_get`. The primitives are done and tested; the consumers are built in their workstreams.

This document also defines the **one canonical URL normalizer** that WS-A merge, WS-B health keys, WS-C manifest matching + suppression, and WS-E evidence keys all MUST share — divergent normalization is itself a vulnerability (F9, F13).

## 0. Canonical URL handling (shared primitive — build first)

`fn canonical_url(raw: &str) -> Result<CanonicalUrl, Reject>` used everywhere a URL is a key or a match target:
1. `Url::parse`; reject non-`http`/`https` scheme.
2. **Reject any authority containing userinfo**: `!url.username().is_empty() || url.password().is_some()` → reject. Also reject a bare `@` in the authority with no colon (F13).
3. Host: `url.host_str()`, lowercase (ASCII), strip exactly one trailing FQDN dot, IDNA/punycode `to_ascii` normalize (defeats homographs). Reject empty host and hosts without a dot unless a public IP literal.
4. If host parses as an IP literal (any form), run `is_disallowed_target_ip` (see §2) — but note this is a fast-path, not the SSRF boundary.
5. Port: drop the default port for the scheme; keep explicit non-default ports.
6. Path: keep, but collapse `//`→`/` and resolve `.`/`..` segments; reject if traversal escapes root.
7. Produce a stable string form for use as the HashMap/S3 key.

Matching (F1, F13) is done on the **parsed** result, never `String::starts_with` on the raw URL. S3 object keys derived from a URL use `sha256(canonical_string)` hex, never the URL itself (F9).

Add a test asserting `canonical_url` produces the same key for `resources`, `health`, and `curation-state` stores given the same input.

## 1. F1 (CRITICAL) — Manifest tier matching must be host-exact + path-boundary

**Attack**: manifest stores prefix `https://api.meshrelay.xyz`; naive `starts_with` awards `first_party` to `https://api.meshrelay.xyz.evil.com/`, `https://api.meshrelay.xyz@evil.com/`, `https://api.meshrelay.xyzevil.com/`. Attacker registers one via public `POST /discovery/register` (passes `validate_resource`) → showcased as our product, paying the attacker.

**Partial mitigation already shipped** (register path, `src/discovery.rs`): the `@evil.com` userinfo variant is now rejected at `validate_resource` (real gap — `url` 2.5.8 parsed `https://trusted@evil.com/` with host `evil.com` and nothing rejected it). The `.evil.com` / `xyzevil.com` variants still parse fine and are only defused by the **matcher** below — which does not exist until WS-C, so F1 remains a WS-C blocker.

**Fix (WS-C `04` §3 matcher)**: after `canonical_url`:
- `host == manifest_host` exactly (or `host.ends_with(".{manifest_host}")` only if a subdomain is explicitly intended — never bare `starts_with`).
- then `path == manifest_path` OR (`manifest_path` ends in `/` AND `path.starts_with(manifest_path)`) — so `/api/` never matches `/api-evil`.
- scheme `https` exact.
- Unit test every payload above asserting resolved tier == `listed`.

## 2. F2 (CRITICAL) — SSRF: encoding bypass + DNS-pin in the connector

**Update (verified 2026-07-23)**: `url` 2.5.8 (the crate in use) follows the WHATWG host parser, which **already normalizes alternate IPv4 encodings** for http(s) — `http://0x7f000001/`, `http://2130706433/`, `http://017700000001/`, `http://127.1/` all parse to `host_str() == "127.0.0.1"`, so the existing `host.parse::<IpAddr>()` gate in `validate_resource` already catches encoded **IP literals**. The register-path hardening shipped as a first slice of this workstream (`src/discovery.rs`): (a) reject URL **userinfo** (`https://trusted@evil.com/`) — this WAS a real gap; (b) a `host_as_encoded_ipv4` fallback as defense-in-depth; (c) extend `is_disallowed_target_ip` for `240.0.0.0/4` and `192.88.99.0/24`. The **remaining, genuine F2 risk is a DNS name whose A-record points at a private/metadata IP** (and DNS-rebinding between check and connect) — this cannot be caught at validate time and only becomes live when the prober fetches attacker URLs.

**Attack (the live part, for WS-B)**: attacker DNS name with an A record of `169.254.169.254` (EC2 IMDS), `169.254.170.2` (ECS/Fargate task-role creds), `metadata.google.internal`, `192.0.0.192`, `100.100.100.200`, or `*.nip.io` tricks. The prober fetching these reads **our Fargate task-role credentials**. Prose like "resolve → check → connect" is insufficient; `reqwest` re-resolves at connect time (DNS-rebinding window) unless the socket is pinned.

**Fix (WS-B `03` §1, required implementation spec)**:
1. Enforcement lives in the **connector**, not a pre-check. Build the prober client with a custom resolver / `reqwest::ClientBuilder::dns_resolver` (or resolve manually + `.resolve_to_addrs(host, &[checked_ip])`) that: (a) resolves, (b) **rejects if ANY returned A/AAAA record satisfies `is_disallowed_target_ip`** (a mixed public+private answer is an attack — never "filter to the good ones"), (c) pins the connection to the single checked IP so connect cannot re-resolve.
2. Downgrade R2's literal check to "cheap fast-path"; the connector is the real boundary.
3. Extend `is_disallowed_target_ip` (`src/discovery.rs:652-722`) to cover IPv4 `240.0.0.0/4` (Class-E; only `255.255.255.255` caught today via `is_broadcast`) and `192.88.99.0/24` (6to4). Also handle `::ffff:x.x.x.x` IPv4-mapped IPv6.
4. Tests: mock resolver returning each of `{127.0.0.1, 169.254.169.254, 169.254.170.2, 10.x, ::1, ::ffff:169.254.169.254, mixed[public,169.254.169.254]}` → refused.

## 3. F3 (HIGH) — Manual redirect handling (redirects re-open SSRF)

**Attack**: prober fetches `https://attacker.com/x` (public, passes); origin returns `302 Location: http://169.254.169.254/latest/meta-data/iam/...`. reqwest's default policy auto-follows up to 10 redirects, re-resolving each hop with no SSRF check — the §2 connector only guards the first client call.

**Fix**: `.redirect(reqwest::redirect::Policy::none())`; follow manually, cap 3 hops, run the full §2 resolve-check-pin (+ §0 userinfo/encoding rejects) on every `Location` (relative→absolute resolved first). Reject `https→http` downgrades to internal-looking hosts. Test redirect-to-`169.254.169.254`.

## 4. F4 (HIGH) — Feed-poisoning URL collision hijacks/griefs manifest listings

**Attack**: any of 12 feeds (or a MITM of one) returns an item whose `url` collides with a first-party/VIP URL, newer `last_updated`, attacker `payTo`. WS-A merge "incoming wins for accepts" lets it through. Tenjin (no `expectedPayTo`) → silent VIP hijack, no drift alarm. First-party with `expectedPayTo` → drift-quarantine hides our own product on demand (griefing DoS).

Confirmed safe: `source`/`source_facilitator` are set by `from_aggregation`, not from feed JSON — a feed cannot forge `self_registered`. But merge can *downgrade* an existing `self_registered` to `aggregated` on collision.

**Fix (WS-A `02` §2.4 + WS-C `04` §3)**:
1. Manifest-matched URLs are **authoritative from the manifest**: for a URL matching a manifest entry, ignore feed `accepts`/`payTo`, use the manifest baseline; never let an aggregated import mutate a manifest-matched record's payment fields. Alert when a feed tries.
2. Merge: **never downgrade** `source` from `self_registered`/`settlement` to `aggregated` (existing wins for `source`).
3. `vip` must carry `expectedPayTo` OR origin proof (§10); do not ship Tenjin with drift-check disabled — learn per-creator payTo only from `self_registered`/origin-served entries, never from aggregated feeds.
4. First-party drift-quarantine pages an operator; UI shows "temporarily unavailable" (hardcoded card stays) — this path is attacker-triggerable, make it explicit.

## 5. F5 (HIGH) — Clamp feed `last_updated` (far-future = immortal top-ranked poison)

**Attack**: feed sets `last_updated = 4102444800` (2100). Sorts first (`discovery.rs:338`), wins every future merge race forever (`:405`, no real `now` can exceed it → record immutable, F4 hijack permanent), evades age-based decay/GC. Source: `unwrap_or(now)` with no clamp (`discovery_aggregator.rs:728-733`).

**Fix (WS-A)**: clamp `last_updated = min(feed_value, now + 300s)`; reject records with `last_updated > now + SKEW` as malformed (`bazaar_import_dropped_total{rule="future_timestamp"}`); floor absurd/negative. Strongly consider ordering the default list by `first_seen` (or health-then-`first_seen`) so the mutable feed timestamp is never a ranking lever.

## 6. F6 (MEDIUM) — Read routes are unauthenticated AND ungoverned → cheap DoS

**Attack**: `main.rs:385` merges `discovery_routes()` with **no `GovernorLayer`** (only register + verify/settle are governed). `GET /discovery/resources`, new `q=`, `/discovery/stats`, `/bazaar` have no rate limit; `q` length is unbounded (query string, not the body limit); each `q` call is O(items × |q|) under the registry read lock → sustained full-catalog scans + lock pressure that also slows aggregator/prober writers.

**Fix (WS-D `05` §4 + `06` §3)**: attach a read-side `GovernorLayer` (~30 req/min/IP, `SmartIpKeyExtractor` as `main.rs:338`) to listing/stats/bazaar; cap `q` ≤128 chars (400 otherwise), reject control chars; precompute lowercased search fields at import; serve `/discovery/stats` only from its 60s cache (never recompute on demand).

## 7. F7 (MEDIUM) — Prober weaponizable as DDoS/port-scanner

**Attack**: malicious feed injects thousands of distinct hostnames all resolving to one victim IP (attacker DNS). Per-*hostname* origin cap sees each as separate → only limiter is global concurrency 15, sustained for the whole sweep; GET-then-POST doubles it. New enabled source defaults `standard`/uncapped (`02` §2.3 fail-open).

**Fix (WS-B `03` §3)**: bucket the per-origin cap by **resolved destination IP/24** (post-§2 resolution), not hostname; add a **global outbound req/sec** limit (not just concurrency); cap total probeable URLs per destination-IP; change the fail-open default for **unknown** sources from `standard` to `probation`; POST only after a GET 404/405 (never speculative).

## 8. F8 (MEDIUM) — Admin API hardening

**Fixes (WS-C `04` §4)**:
- Constant-time token compare (`subtle::ConstantTimeEq`), only after the 404-when-unset check.
- Guarantee `Authorization` is **never logged**: redact in the tracing layer (`main.rs:389`) and never expose it in `#[instrument]` fields.
- Wire admin routes onto the **governored** `discovery_register` router (`main.rs:355-357`), NOT the ungoverned `discovery_routes` (`:385`).
- The `by` audit field is **server-derived** (token identity / source IP), never from the request body.
- Normalize the `?url=` param through `canonical_url` (§0) before `unregister` (`discovery.rs:288` keys the exact normalized string; a raw query silently no-ops) and return which key was acted on. No SQL/command-injection surface (HashMap key), confirmed.

## 9. F9 (MEDIUM) — S3 keys / evidence route must not embed raw URLs

JSON values are safe (`serde_json` escapes). The danger is any URL→S3-key or URL→path-param mapping (WS-E evidence files `bazaar/attestations/{url}.json` → traversal to `bazaar/resources.json`; a public evidence route mapping user path→S3 key = arbitrary bucket read).

**Fix (WS-E `07` §4, WS-B `03` §5)**: key evidence objects by `sha256(canonical_url)` hex; the public read route accepts only `[0-9a-f]{64}`, rejects anything else — no URL path segments. Use the §0 canonicalizer everywhere so resource key == overlay key (test it).

## 10. F10 (MEDIUM) — payTo allowlist normalization + VIP origin proof

**Fix (WS-C `04` §3/§5)**: normalize addresses per-chain before compare (EVM: lowercase / EIP-55 canonical; Solana: base58 canonical; etc. — reuse payment-path address types) or a legit checksum-case difference false-quarantines our own first-party (DoS). Require `first_party`/`vip` to satisfy **origin proof** (`.well-known/x402` on the entry's own domain, or `self_registered` from the product team) before the tier is honored at read time — URL-prefix alone is not proof of partnership. **Until Tenjin has origin proof, ship it as `verified` (health-earned), not `vip`.**

## 11. F11 (MEDIUM) — ERC-8004 attestation gaming + key custody

**Attack**: an endpoint that detects our UA/IP and serves clean 402 only during predictable probe windows farms genuine positive attestations signed by our "trusted reviewer" address → reputation laundering under our name. Prober key leak → arbitrary feedback under the un-spammable identity.

**Fix (WS-E `07` §2-3)**: keep `ENABLE_BAZAAR_ATTESTATIONS=false` default; randomize probe timing/UA/source for attestation-bearing probes; require multiple independent successes over time before any positive attestation; prefer settlement-backed `proofOfPayment` (real txs, unfakeable) over prober-only uptime; dedicate + rotate the attestation wallet (own Secrets Manager entry + rotation runbook); hash-commit + `nosniff` evidence files; gas/rate cap per agentId per period.

## 12. F12 (LOW) — Length caps (bloat, amplified by F5 permanence)

Add **R7** to WS-A `02` §2.2: cap `description` ≤2 KiB, each tag ≤64, tag count ≤20, `provider`/`category` ≤128; truncate-or-reject + count. Apply on both `Filtered` and `Strict` paths (`convert_single_resource` has no caps today, `discovery_aggregator.rs:739,746-751`).

## 13. F13 (LOW) — Suppression/dedup by normalized URL + prefix

Suppress by `canonical_url` (§0) and support **prefix suppression** (safe host+path matcher from §1) so `__bazaar_debug__?x=1`, trailing dot/slash, and the audit's 242 near-dupes can't evade a delist. R4 must reject any authority containing `@`.

## 14. F14 (LOW) — No trust from free-text fields; escape UI; sanitize logs

`POST /discovery/register` accepts `provider: "Execution Market"` on `https://evil.com/x` → phishing card. **UI (`05`)**: never confer trust styling (verified/first-party badge, logo, homepage) from free-text `provider`/`description` — only from server-set `curation.tier`; HTML-escape all item fields (XSS defense-in-depth). **WS-A**: strip/replace control chars in `url`/`description` before logging (CloudWatch log-injection, CLAUDE.md warns on encoding) and before storage.

## 15. F15 (LOW) — Aggregator + crawler SSRF is MANDATORY, not "while we're there"

The aggregator runs hourly in prod with a plain `Client` (default redirect, no IP guard, `discovery_aggregator.rs:513/527`); the crawler (`discovery_crawler.rs:154`) follows redirects to anywhere. The §2 connector + §3 manual-redirect policy are **required** for both, not optional. (Aggregator/crawler fetch trusted hardcoded/operator-set URLs, a lower risk than the prober's attacker-supplied URLs, but the redirect hole is identical.)

## 16. F16 (LOW) — Port allowlist (defense-in-depth)

After §2 resolution, allow only ports `{80, 443, 8080, 8443}`; reject others (classify `unprobeable`, don't fetch) so the prober can't be used as an internal-port scanner if the IP checks ever regress.

## Security acceptance checklist (gate for WS-B and WS-C deploys)

Checked = implemented + compiled + unit-tested this session (2026-07-23); pending user build+deploy. Unchecked = built in its workstream.

- [x] `canonical_url` implemented (`discovery_security.rs`); shared use by resources/health/curation/suppression/evidence keys lands as those stores are built (WS-A/B/C).
- [x] Manifest matcher (`match_manifest_prefix`) rejects all F1 payloads — unit tests green (`manifest_matcher_rejects_f1_payloads`). WS-C must call it from tier resolution.
- [x] Connector (`check_url_target`/`safe_get`) refuses IP-literal + encoded + port + userinfo F2 payloads (tested); resolves DNS and rejects if any address disallowed (mixed = attack); redirects manually capped + re-checked (F3). WS-B prober must fetch via `safe_get`. (Remaining test to add in WS-B: a mock resolver for DNS-name → private-IP; current tests cover the IP-literal + logic paths.)
- [ ] Feed cannot mutate a manifest-matched record's payTo (F4); `source` never downgraded. — WS-A merge.
- [ ] `last_updated` clamped to `now+300s`; future-timestamp rejects counted (F5). — WS-A.
- [ ] Read routes governed; `q` ≤128 chars (F6). — WS-D.
- [ ] Per-destination-IP/24 origin bucketing + global rps; unknown sources default `probation` (F7). — WS-B.
- [ ] Admin: constant-time compare, Authorization redacted, wired to governored router, server-derived `by` (F8). — WS-C.
- [ ] Evidence/overlay keys are `sha256` hex, never raw URLs (F9). — WS-B/E.
- [ ] Per-chain address normalization; Tenjin ships `verified` until origin-proven (F10). — WS-C (Tenjin already set to `verified` in the manifest).
- [x] Port allowlist {80,443,8080,8443} in the connector (F16) — tested.
- [x] Crawler + aggregator use the hardened connector/redirect policy (F15) — crawler via `safe_get`, aggregator via `aggregator_redirect_policy`; compiled, crawler tests green.
- [ ] Length caps R7 (F12); suppression by normalized-URL+prefix (F13, `canonical_url` ready); UI escapes fields + no trust from free text (F14). — WS-A/C/D.
