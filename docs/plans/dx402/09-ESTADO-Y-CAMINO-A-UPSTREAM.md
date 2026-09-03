# DX402 — Estado y camino al PR upstream

**Snapshot:** 2026-09-03. **Facilitador en producción:** 2.10.0.
**Pregunta que responde este documento:** ¿podemos abrir el PR a la x402
Foundation? Y si no, ¿qué falta exactamente, quién lo hace, y cómo sabemos que
está?

Cada fila lleva su **evidencia** — un commit, un test que corre, o una medición
con fecha. Una fila sin evidencia no está hecha, por más que el código exista.

---

## 1. Lo que está hecho

| # | Qué | Evidencia |
|---|---|---|
| ✅ | Sobre cifrado al pagador, derivando la clave pública de la firma de pago (7 familias) | `src/dx402/envelope.rs`, `pubkey.rs`; vectores cruzados Rust/Python/TS (`tests/dx402_cross_seal.rs`) |
| ✅ | Sobre **multi-destinatario** (payer + seller + auditor), v1 byte-idéntico para un solo destinatario | v1.79.0; `sealed_roles` en los SDK |
| ✅ | Gate del anchor: prueba de pago on-chain, payer == destinatario, firma del payee, anti-replay, ventana 900s | v1.78.0 → v1.82.0; `src/dx402/gate.rs`, 19 tests |
| ✅ | Escalera de autoridad: provisional < firmado < verificado; un reclamo débil nunca bloquea uno más fuerte | v1.82.0 (secuestro reportado por KK, reproducido en prod y cerrado); `registry.rs` |
| ✅ | Firmas **ed25519** para payees de Solana/Stellar | v1.82.0; `gate.rs::verify_ed25519` |
| ✅ | **Riel de escrow (x402r)**: el comprador se resuelve por `getHash` del propio escrow | **2.10.0**, commit `e55fcf83`; fixture pineada contra optimism `0x5a2822cc…`; muestreo 23/23 |
| ✅ | Lote ambiguo rechazado (`dx402_escrow_release_ambiguous`) | 2.10.0 |
| ✅ | Gate probado hasta el escalón final con un release **real** reejecutado en fork | `tests/dx402_escrow_sim.rs` + `07-SIMULACION-ESCROW.md`, commit `52cc0af8` |
| ✅ | Barrido del worker que firma y superpone anclajes provisionales | KK `261df4f8`; **primer `signed: true` en producción** (monad `0xf170a205…`, 2026-09-03) |
| ✅ | Paybox firma el digest crudo y recupera al payee (no aplica EIP-191) | Medido 2026-09-03 contra el custodio real |
| ✅ | **Opt-in del comprador vía `accepts`** — el vendedor ofrece dos veces, el cliente elige, el hook honra la elección | Este commit: `DurableEvidenceConfig::{from_requirements,declare_on,offered_in}`, `X402Middleware::with_durable_offer`, `X402Payments::prefer_durable_evidence`, `OfferDecision`; tests en los 3 crates |
| ✅ | Anuncio honesto: `/supported` lista la extensión sólo si está servible; el API expone `verified`/`signed`/`notVerifiedReason` | `handlers.rs:2118`; `types.rs:325-343` |
| ✅ | Backends: S3 + Pinata (IPFS privado), fallback automático, reparación de punteros huérfanos | 1.85+; `/dx402/stats` los enumera con `revocable`/`public` |
| ✅ | Presupuesto de memoria medido (x5), techo configurable, `busy` distinto de `anchor_failed` | `04-STREAMING-EVIDENCE-HANDOFF.md`, `memory_amplification.rs` |
| ✅ | Spec v0.2 describiendo **sólo lo shippeado** | `08-SPEC-v0.2.md` (este commit) |

## 2. Lo que falta para el PR

| # | Qué | Dueño | Cómo sabemos que está |
|---|---|---|---|
| ⬜ | **Corpus certificado.** Hoy: 699 anclajes, **1 firmado, 12 verificados (tests), 0 multi-destinatario en prod**. Sin esto el PR dice "lo construimos", no "lo corrimos" | **Saul** (encender el enjambre) + barrido corriendo | `aws dynamodb scan facilitator_dx402_evidence` con decenas de `verified: true` sobre trades reales; ≥1 semana de tráfico |
| ⬜ | Barrido en cron: `dx402_firmar_anclajes.py --horas 0.2` cada ~5 min por agente (la ventana son 900s) | KK | Anclajes nuevos aparecen `signed: true` sin intervención |
| ⬜ | Los SDK (py/ts) exponen el opt-in: helper para armar el par de ofertas + `prefer_durable_evidence` en el cliente | uvd-x402-sdk | Publicados y un e2e que paga la oferta durable desde Python/TS |
| ⬜ | Verificar el **proceso** de la Foundation: dónde viven los specs de extensiones, formato, plantilla | Yo | Un link al directorio/PR de referencia en este documento |
| ⬜ | Fase 2 (`DX402_REQUIRE_PROOF=true`) encendida en prod con tráfico real pasando | Saul | Logs sin `dx402_*` de rechazo sobre tráfico legítimo durante ≥48h |

## 3. Lo que NO bloquea (y el spec lo dice)

| Qué | Estado |
|---|---|
| Gate en cadenas no-EVM (Solana, NEAR, Stellar, Algorand) | Reportado como `unverifiable_chain`, nunca impuesto. Solana es la única priorizada; va después del PR |
| Modo `derived` para wallets de browser | Bloqueado en validar RFC 6979 entre vendors |
| Anclaje on-chain del digest del recibo | No planeado para v0.2 |
| `escrowed` / `POST /dx402/recover` | 501 honesto. La llave de lectura declarada (EM) lo cubre mejor |

## 4. El flujo que falta, dicho concreto

Lo que más falta **no es código**: es que KarmaCadabra opere sobre Execution
Market. Cada trade del enjambre produce un release de escrow → EM ancla
provisional → el barrido del worker lo firma dentro de los 900s → el facilitador
lo verifica contra la cadena → `verified: true`. Todo eso existe y está probado
por partes. Lo que no ha pasado es **la secuencia completa sobre un pago real**,
porque el enjambre lleva >24h apagado (último release: 2026-09-02 04:04 UTC).

Con el enjambre encendido y el cron del barrido, el corpus se llena solo.

## 5. Decisión pendiente

- **Abrir hoy como borrador/RFC** (con la spec v0.2) para conversación temprana, o
- **Esperar ~1-2 semanas** de corpus certificado y abrir con números.

Recomendación: la segunda. Una propuesta descartada por falta de uso es más cara
que dos semanas.
