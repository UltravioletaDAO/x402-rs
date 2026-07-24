# Handoff → Facilitator (x402-rs): wallets de firma y concurrencia de settles (2026-07-24)

**QUÉ**: rediseño de la arquitectura de firma del Facilitator para aguantar concurrencia de settles sin `nonce too low` / `replacement underpriced` / Multicall3 out-of-gas (P0s del BACKLOG de EM, filas 2026-07-20/21, origen INC-2026-07-06).
**POR QUÉ AHORA**: EM acaba de encender el rail de streaming (escrow-sessions, ADR-005 en el repo EM). Con el default settle-at-close son solo ~2 TXs por sesión, pero el volumen de sesiones crece la carga concurrente sobre el EOA único del Facilitator.
**Pedido por**: Saul (2026-07-24).

## Contexto técnico (para calibrar la solución)

El Facilitator firma TODO (settles, refunds, registros 8004) desde **un solo EOA** (`0x1030…13C7`). Los nonces de una EOA son **independientes por chain** — la misma wallet en Base y en Arbitrum NO se estorban entre sí. El problema es la concurrencia **dentro de una misma chain**: dos settles simultáneos en Base compiten por el mismo nonce y uno pierde (`nonce too low` / `replacement underpriced`).

> ⚠️ **Nota sobre la idea original** ("diferentes wallets para diferentes chains"): separar por chain **no resuelve el problema**, porque las chains ya no comparten nonce. La versión que SÍ lo resuelve es el mismo espíritu aplicado donde duele: **varias wallets POR chain** (signer pool) — abajo.

## Propuesta (en orden de esfuerzo)

1. **Nonce manager in-process (barato, primero)**: un allocator de nonces serializado por chain en el proceso del Facilitator (cola por chain + nonce local persistente con resync on-error). Elimina la carrera sin infra nueva. La mayoría de los facilitators de producción viven con esto + retries.
2. **Signer pool por chain (el fix estructural)**: N EOAs derivadas de una seed (BIP-32/44, patrón HD wallet ya usado en KK Phase 13), cada una con su propio carril de nonces, asignación round-robin o por hash del payment. Config: `SIGNER_POOL_SIZE` por chain (empezar N=3 en Base, N=2 resto). Requisitos:
   - Cada signer necesita gas nativo por chain → extender el balance monitor + el runbook de fondeo (skill `fund-distribution` de EM tiene el patrón multi-chain).
   - **Allowlist del operator**: el `releaseCondition` de los PaymentOperators (StaticAddress facilitator) apunta al EOA actual — verificar si acepta múltiples addresses (OrCondition) o si el pool debe firmar-y-relayar a través del EOA canónico. Si toca redeploy de operators → skill `deploy-operator` de EM (9 chains).
   - Keys en Secrets Manager, jamás en config/repo (INC-2026-03-30).
3. **Multicall3 out-of-gas**: independiente del pool — poner gas limit explícito por batch y partir batches grandes (el error -32003 del INC).

## Datos que faltan (pedidos a KK, 2026-07-24)

Se le pidió a KK (vía kk-feedback-sync, como práctica continua de ahora en adelante) reportar **estadísticas por chain** de cada fallo de settle/refund: chain, count, error code (-32007 / nonce / out-of-gas), timestamp. **La decisión de upgrade de RPC (tier pago) se toma con esos datos por chain, no a ciegas** — es posible que solo 1-2 chains necesiten tier pago y el resto queden en el tier actual.

## Criterio de éxito

Una ráfaga de 20 settles concurrentes en la misma chain (test sintético o la suite de streaming de KK con settles periódicos activados) completa sin un solo `nonce too low` ni -32007, con los tx hashes verificables.

## Fuera de scope

Cambiar el modelo de custodia (el Facilitator sigue pagando gas y firmando — eso es su rol), y cualquier cambio a los 5 schemes existentes.
