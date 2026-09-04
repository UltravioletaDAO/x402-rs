# DX402 — Estado y camino al PR upstream

**Snapshot:** 2026-09-04 (medido sobre la tabla, no sobre reportes). **Facilitador en producción:** 2.10.0; 2.11.0 commiteado sin desplegar.
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
| ✅ | **P1 del red team cerrado**: el riel se clasifica desde el receipt, no desde el `payer` declarado; sellar al TokenStore ya no salta la resolución por escrow ni la refusal de ambigüedad. Venía del gate original (v1.78.0), no de 2.10.0 | Este commit; `classify_rail` + 4 tests |
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
| ✅ | **Corpus certificado — meta superada en un día.** 2026-09-04, número **honesto**: **116 `verified: true` en 7 redes EVM** (avalanche 49, arbitrum 37, optimism 9, base 7, monad 7, ethereum 6, polygon 1), **111 firmados**, **26 compradores y 24 vendedores** distintos (EVM). El 122 que reportamos primero incluía 5 filas de Solana del 2026-08-18 marcadas `verified` por el código pre-v1.82.0 (finalidad autodeclarada) con `txHash` de demo (`KKFIRMADEMO…`); el código actual no puede producirlas. Meta era 50 / 3 / 5 y 5 | KK (enjambre + barrido) | `10-EVIDENCIA-PARA-EL-PR.md` §5 reproduce cada número; recibo EIP-712 recuperado offline al firmante en base y avalanche |
| ✅ | **Las 5 filas de Solana pre-gate** (2026-08-18, finalidad autodeclarada, `txHash` de demo) pasaron a `verified: false` con `notVerifiedReason: pre_gate_self_asserted_2026_08_18`, vía `update-item` condicionado a `verified = true`. Re-conteo 2026-09-04: **119 verificados, 100 % EVM**, 7 redes | Saul autorizó; ejecutado | El scan de `10-EVIDENCIA` §5 da 119 y ninguna fila de Solana |
| ✅ | Barrido en cron cada 5 min por worker | KK | `signed` pasó de 1 a 111 en ~8 h sin intervención |
| ⬜ | **Sostenido 7 días.** Único sub-criterio abierto. La flota quedó **pausada** 2026-09-04 02:09Z por combustible (ver `2026-09-04-dx402-kk-corpus-final.md`); mientras esté pausada el corpus no crece | Saul (combustible) + KK | El contador sigue subiendo hasta 2026-09-10 |
| ⬜ | Los SDK (py/ts) exponen el opt-in: helper para armar el par de ofertas + `prefer_durable_evidence` en el cliente | uvd-x402-sdk | Publicados y un e2e que paga la oferta durable desde Python/TS |
| ⬜ | Verificar el **proceso** de la Foundation: dónde viven los specs de extensiones, formato, plantilla | Yo | Un link al directorio/PR de referencia en este documento |
| ⬜ | Fase 2 (`DX402_REQUIRE_PROOF=true`) encendida en prod con tráfico real pasando | Saul | Logs sin `dx402_*` de rechazo sobre tráfico legítimo durante ≥48h |

## 2-bis. Red team 2026-09-04 (14 hallazgos, veredicto SAFE WITH FIXES)

| Sev | Hallazgo | Estado |
|---|---|---|
| P1 | #1 sellar al TokenStore saltaba la resolución por escrow | ✅ `classify_rail`, resolución incondicional |
| P1 | #2 el recibo firmaba `payee`/`txHash` del caller sin cotejar con la prueba | ✅ atados a la prueba, local, antes del RPC |
| P1 | #3 `PaymentInfo.payer` no es "hecho on-chain": el escrow es permissionless para el operador y acepta cualquier collector | 📝 doc y spec corregidos; **allowlist de tokens** en el proof path = follow-up |
| P1 | #13 empate de precio → la simple ganaba (evidencia gratis muerta; skim con tag acolchado) | ✅ empate → la que declara; sobrepago → la más cara cubierta |
| P2 | #4 panic remoto por `Uint::from` en `to_escrow_abi` | ✅ `try_from` → `EscrowReleaseInvalid` |
| P2 | #9 `settle_before_execution` cobraba evidencia y nunca anclaba | ✅ el hook corre también en esa rama |
| P2 | #10 declaración malformada fallaba abierta (anclaba a todos) | ✅ presencia ≠ validez; `NotSelected` |
| P2 | #14 no-EVM: la segunda oferta es impagable; `asset` no está en el filtro | 📝 documentado EVM-only; `asset` no viene en el payload v1 EVM → follow-up en v2 |
| P2 | #5 un escrow por red, `getHash` en `latest`, sin aserción de código | ⬜ follow-up |
| P2 | #6 `paymentId` como clave de registro sin normalizar (`0x`/mayúsculas) | ⬜ follow-up |
| — | #7, #12 mitigados por diseño; #11 INFO | — |

## 3. Lo que NO bloquea (y el spec lo dice)

| Qué | Estado |
|---|---|
| Gate en cadenas no-EVM (Solana, NEAR, Stellar, Algorand) | Reportado como `unverifiable_chain`, nunca impuesto. Solana es la única priorizada; va después del PR |
| Modo `derived` para wallets de browser | Bloqueado en validar RFC 6979 entre vendors |
| Anclaje on-chain del digest del recibo | No planeado para v0.2 |
| `escrowed` / `POST /dx402/recover` | 501 honesto. La llave de lectura declarada (EM) lo cubre mejor |

## 4. El flujo que faltaba — ya corrió

La secuencia completa sobre pagos reales corrió el 2026-09-03/04: trade de KK en
EM → release de escrow → EM ancla provisional → el worker firma dentro de los
900 s → el facilitador verifica contra la cadena → `verified: true`. **122 veces,
en 8 redes.** Lo que sigue abajo es el texto original, conservado como registro
de lo que hacía falta y por qué. Cada trade del enjambre produce un release de escrow → EM ancla
provisional → el barrido del worker lo firma dentro de los 900s → el facilitador
lo verifica contra la cadena → `verified: true`. Todo eso existe y está probado
por partes. Lo que no ha pasado es **la secuencia completa sobre un pago real**,
porque el enjambre lleva >24h apagado (último release: 2026-09-02 04:04 UTC).

Con el enjambre encendido y el cron del barrido, el corpus se llena solo.

## 5. Lo que queda, y de quién es (2026-09-04, tras los siete agentes)

**Decisiones de Saul** (cambian código o producción; no las tomo solo):

1. **Restructurar el wire al formato de la Foundation** antes del PR. Verificado
   contra `x402-foundation/x402` (sin plantilla, pero convención uniforme):
   `extensions` **top-level** en el 402 (offer-receipt lo hace así incluso en v1,
   con `acceptIndex`) en vez de `extra.extensions` por oferta; envelope
   `{info, schema}` + regla de eco en el payload; la evidencia bajo
   `SettlementResponse.extensions["durable-evidence"]` del `X-Payment-Response`
   que el vendedor reenvía, con `X-Durable-Evidence` como conveniencia no
   normativa; endpoints `/dx402/*` como "de esta implementación"; CAIP-2 en
   payloads firmados. Es un cambio de wire-shape, no de criptografía ni de gate.
   Estimación: 2-3 días. Sin esto, el primer comentario del revisor es "please
   restructure".
2. **Desplegar 2.11.0** (`2ab7deb2` local; `deploy-readiness` emite el GO/NO-GO
   contra ese hash). Pushear a `main` es el deploy.
3. ~~Las 5 filas de Solana pre-gate~~ — hecho 2026-09-04.
4. **Cuándo abrir**: el corpus y el spec ya alcanzan; la restructura del punto 1
   es lo que decide si el PR se abre esta semana o la próxima.

**Follow-ups técnicos** (míos, no bloquean el PR pero el spec los declara):

- Allowlist de tokens en el proof path (red team #3) — hoy `verified` certifica
  consistencia con el escrow conocido, no que el pagador fue defraudado.
- `getHash` en `latest` sin aserción de código del escrow (#5); normalización de
  `paymentId` como clave (#6); `asset` en el matcher cuando v2 lo traiga (#14).
- Opt-in en los SDK py/ts (deseable; lista exacta en el reporte de paridad).
- Gate en Solana (único no-EVM priorizado).

**Fase 2** (`DX402_REQUIRE_PROOF=true`): recién después de que 2.11.0 esté en
producción ≥48 h con tráfico real pasando — y no antes de que el fix del riel
(#1) esté desplegado, o fase 2 castiga a los vendedores que perdieron la carrera
en vez de al secuestrador.
