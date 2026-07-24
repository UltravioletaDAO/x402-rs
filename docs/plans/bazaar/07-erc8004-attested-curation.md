# WS-E — ERC-8004 Attested Curation (Differentiator)

**Ships as**: v1.54.0 · **Depends on**: WS-B (prober generates the data), WS-C (tier model) · **Status**: design; validate gas + registry details before build
**Security**: attestation gaming, prober-key custody, and evidence-file hosting are covered in `08-security-hardening.md` §11 (F11) and §9 (F9 — evidence keyed by `sha256(canonical_url)`, public route accepts only `[0-9a-f]{64}`). Default kill-switch `ENABLE_BAZAAR_ATTESTATIONS=false`.
**Why**: CDP's `curated: true` is a bare boolean you must trust Coinbase for; QuestFlow's validation is opaque. Nobody has on-chain-attested, independently-verifiable curation. We already operate ERC-8004 identity + reputation registries on 11 mainnets with `/register` live — this workstream turns our health data into portable, trustless VIP credentials.

## 1. Concept

The facilitator's **prober wallet becomes an on-chain reviewer**: it writes ERC-8004 Reputation feedback about the services it probes and settles for. Anyone can verify our VIP claims by querying the reputation registry filtered to our prober address — no trust in our API required.

ERC-8004 primitives that map 1:1 (from the EIP):
- `giveFeedback(agentId, value, valueDecimals, tag1, tag2, endpoint, feedbackURI, feedbackHash)` — the **`endpoint` parameter scopes feedback to a specific resource URL**, not just the agent.
- `getSummary(agentId, clientAddresses[], tag1, tag2)` — cheap on-chain aggregation **filterable to trusted reviewers** (i.e., only our prober address) — this is what defeats reputation spam.
- Spec's own example tags are exactly our metrics: `uptime` (99.77% → value `9977`, decimals 2), `successRate`, `responseTime`, `starred`.
- Off-chain feedback files support `proofOfPayment {fromAddress, toAddress, chainId, txHash}` — and as settlement executor we are uniquely positioned to attach real settlement txs.

## 2. Design

### 2.1 Identity anchoring
- A VIP/first-party service links (or we register) an ERC-8004 agent whose registration file lists its x402 resource URLs in `services[]`, served from the service's own origin (`.well-known` path) — origin control closes the impersonation gap.
- Store the link in the curation manifest: `"erc8004": {"network": "eip155:8453", "agentId": <id>}` per entry (04 §3).

### 2.2 Attestation writer (extends WS-B prober)
- On **tier transitions and monthly checkpoints** (NOT per probe — bounds gas): write `giveFeedback(agentId, uptimeBps, 2, "uptime", "", resourceUrl, feedbackURI, hash)` where `feedbackURI` points to a JSON evidence file (probe window, method, results digest) we host, hash-committed on-chain.
- On settlement through us: periodic `successRate`/`responseTime` feedback with `proofOfPayment` from the actual settlement tx.
- Writer uses the existing EVM signing infra + a dedicated kill-switch `ENABLE_BAZAAR_ATTESTATIONS` (default **false** at launch; project precedent: `ENABLE_REGISTER_RECOVERY`). Gas: Base-first (cheap, where 99% of listings live), expand later.

### 2.3 VIP verification (consumer side)
- `curation` response object gains: `"verification": {"erc8004": {"network": "eip155:8453", "agentId": 123, "reputationSummary": {"uptime": 0.9977, "count": 12}}}` — populated from `getSummary(agentId, [proberAddress], "uptime", "")` cached per aggregation cycle.
- Docs page explains third-party verification: call `getSummary` yourself with our published prober address; compare with our API's claim.

### 2.4 VIP criteria upgrade (from 04 §5)
Phase-4 VIP = phase-2 criteria PLUS: ERC-8004 identity anchored + ≥1 prober attestation on-chain + registration-file origin proof.

## 3. Open questions to resolve before build (validation spike, ~1 day)

1. Confirm the deployed reputation-registry interface on our 11 mainnets matches the final EIP-8004 `giveFeedback` signature used here (our ERC-8004 integration predates parts of the spec's evolution — check `src/` ERC-8004 modules and deployed contract ABIs; do not assume).
2. Gas cost per `giveFeedback` on Base at current prices × expected cadence (transitions + monthly, ~4 first-party/VIP services → trivial, but measure).
3. Whether first-party products (EM, MeshRelay, 402Milly) already have ERC-8004 agent IDs from the execution-market integration — reuse before registering new ones.
4. Prober wallet: reuse EVM mainnet facilitator wallet vs dedicated attestation wallet (recommend dedicated — publishable, low-value, rotatable without touching settlement infra; needs its own Secrets Manager entry + funding).

## 4. Deliverables

- `src/discovery_attestation.rs` (new): attestation writer + summary reader (~250 LOC)
- Manifest schema extension + `verification` response field (~40 LOC)
- Evidence-file hosting: S3 `bazaar/attestations/` + public read route (~40 LOC)
- Docs: verification how-to for third parties
- Marketing: this is a `/ship-tweet`-worthy differentiator once live (follow the two-track tweet playbook; never mention internal audits)
