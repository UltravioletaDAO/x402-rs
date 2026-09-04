# El issue y el PR a `x402-foundation/x402` — textos listos

Proceso verificado 2026-09-03: issue primero (Discussions está deshabilitado),
después un PR **spec-only** de un archivo (`specs/extensions/durable-evidence.md`)
más una fila en `docs/extensions/overview.mdx`. Commits **GPG y DCO** (`git
commit -S -s`). Disclosure de IA obligatoria. Sin nombre de producto en el spec.

## 1. Issue (feature request)

**Title:** `Extension proposal: durable-evidence — encrypted, retrievable delivery evidence`

**Body:**

> ### Summary
> x402 settles payment durably and delivers the resource once; afterwards neither party can prove *what* was delivered, and a buyer who did not persist the body cannot obtain it again. `durable-evidence` lets the resource server seal the delivered body to the parties' own public keys (derived from the payment signature — no registration, no extra round trip), anchor the ciphertext, and have the facilitator notarise it with an EIP-712 receipt. The buyer opts in per purchase through the existing `accepts` array; a client unaware of the extension degrades to the plain offer.
>
> ### Relationship to open proposals
> Several open PRs bind a **hash** of the delivered body to a receipt (#3186, #3140, #3304, #2666, #1932; issue #2833). A hash proves the body was what it was; it does not let anyone read it again. This proposal is the retrieval layer those imply: its `contentHash` is over the plaintext so it can bind to whichever hash commitment lands, rather than compete with it.
>
> ### Shape
> Registry conventions as merged: top-level `extensions["durable-evidence"] = { info, schema }` on the 402 with `info.acceptIndexes` naming the offers (as `offer-receipt` does), payload echo per core §5, evidence under `SettlementResponse.extensions`. No change to any core package.
>
> ### Status
> Running in production since 2026-08 on a facilitator serving 21 mainnets: 119 chain-verified anchors across 7 EVM networks, 26 distinct buyers / 24 sellers (as of 2026-09-04, reproducible from public `paymentId`s). Reference implementation in Rust (facilitator, server hook, client), plus Python and TypeScript SDKs. Independently red-teamed; findings and fixes are in the spec's Security Considerations.
>
> Draft spec: <link to `12-SPEC-v0.3-foundation.md` on our main>. If the maintainers are open to it, I will open the spec-only PR next.
>
> *Disclosure: parts of this proposal and its reference implementation were produced with AI assistance and reviewed by the authors.*

## 2. PR (spec-only)

**Branch:** `spec/durable-evidence` on a fork of `x402-foundation/x402`.
**Files (3):** `specs/extensions/durable-evidence.md` (= `12-SPEC-v0.3-foundation.md`
verbatim), `docs/extensions/durable-evidence.mdx` (la página que el índice enlaza —
cada extensión mergeada tiene una), y una fila en `docs/extensions/overview.mdx`.
La rama ya está preparada en local (`scratchpad/x402-upstream`, rama
`spec/durable-evidence`, commit firmado GPG + DCO) sobre `upstream/main` del
2026-09-04; falta sólo pushearla al fork y abrir el PR.

**Title:** `spec(extensions): durable-evidence — encrypted, retrievable delivery evidence`

**Body:**

> Closes #<issue>.
>
> Adds the `durable-evidence` extension specification. Spec only — no changes to core packages or SDKs, per CONTRIBUTING ("PR 1: Specification Only").
>
> **What it adds.** Sealed, retrievable evidence of *what* was delivered, readable only by the parties, with a facilitator-signed receipt verifiable offline. Buyer opt-in via `accepts`; declaration and echo follow core §5; evidence lands under `SettlementResponse.extensions`.
>
> **What it composes with.** `offer-receipt` (terms), `payment-identifier` (handle), and the open response-hash proposals (#3186 / #3140 / #3304 / #2666 / #1932): `contentHash` is over the plaintext so it binds to any of them.
>
> **Evidence it works.** Production since 2026-08: 119 chain-verified anchors, 7 EVM networks, 26 buyers / 24 sellers; reproducible counts and offline receipt verification in the linked evidence document. Reference implementation: <x402-rs link> (Rust facilitator + `x402-axum` hook + `x402-reqwest` client), Python and TypeScript SDKs.
>
> **Security.** Independently red-teamed before submission; the Security Considerations section records the attacks that were found and closed (anchor hijack via anti-replay, self-asserted finality, escrow-rail bypass, receipt field binding) and what `verified` does and does not assert.
>
> *Disclosure: this specification and its reference implementation were produced with AI assistance and reviewed by the authors.*
>
> Signed-off-by: <name> <email>

## 3. Checklist antes de abrir

- [x] 2.12.0 desplegado 2026-09-04 (la forma del spec es la que corre en producción)
- [ ] `12-SPEC-v0.3-foundation.md` releído una vez más en frío: cero menciones a DX402/producto, cero anécdotas
- [x] Fork: `0xultravioleta/x402` (la org no permite fork sin admin); rama `spec/durable-evidence` pusheada
- [x] Commit firmado y **verificado por GitHub** (`verified=true`): clave `AE07…8E75`, autor/committer `ultravioletadao@gmail.com` — `0xultravioleta@gmail.com` es de otra cuenta y no verifica en ésta
- [ ] Issue abierto; esperar una reacción de maintainer **o** 72 h antes del PR
- [ ] PR abierto con `Closes #<issue>`
- [ ] Mensaje en slack.x402.org (un párrafo, con link al issue)
