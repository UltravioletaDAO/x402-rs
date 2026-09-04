# Restructura del wire al formato de la Foundation

**Estado:** diseño aprobado para ejecutar (Saul, 2026-09-04: "lo más rápido
posible"). Se implementa **después** de desplegar 2.11.0, que lleva los fixes
del red team y no puede esperar a un cambio de forma.

**Por qué.** `foundation-process` verificó contra `x402-foundation/x402`
(2026-09-03) que toda extensión mergeada sigue una convención uniforme aunque no
haya plantilla. Nuestro v0.2 la viola en cuatro puntos y el primer comentario
del revisor sería "please restructure". Nada de esto toca criptografía, gate,
escalera de autoridad ni el riel de escrow — es **forma**, y conviene cambiarla
mientras el único consumidor somos nosotros.

## 1. Mapa: lo que hay → lo que la Foundation espera

| Hoy (v0.2) | Formato Foundation | Qué cambia en código |
|---|---|---|
| Declaración por oferta en `extra.extensions["durable-evidence"]` | `extensions["durable-evidence"]` **top-level del 402**, hermano de `accepts`, con `{ info, schema }`. Qué ofertas la llevan se dice con `info.acceptIndex` (uno) o `info.acceptIndexes` (varios) — el mismo mecanismo que `offer-receipt` | `X402Error`/`PaymentRequiredResponse` gana `extensions`; `with_durable_offer` escribe ahí; `OfferDecision::decide` lee el índice de la oferta pagada en `accepts` |
| Objeto bare `{retention, mode, ...}` | `info: {retention, mode, backend, maxBodyBytes, paidBy, acceptIndexes}` + `schema`: JSON Schema del `info` | `DurableEvidenceConfig` se serializa dentro de `info`; el `schema` es una constante generada del struct |
| El comprador no ecoa nada | El `PaymentPayload` v2 **ecoa al menos el `info` recibido** en `extensions["durable-evidence"]` (regla core §5) | `x402-reqwest`: al pagar una oferta con la extensión, copiar `info` al payload; el vendedor puede usar el eco como confirmación explícita además del `acceptIndex` |
| Evidencia en header propio `X-Durable-Evidence` | Objeto bajo `extensions["durable-evidence"]` del **`SettlementResponse`** que el vendedor reenvía en `X-Payment-Response` | `layer.rs`: antes de `settlement_to_header`, insertar el objeto en `settlement.extensions`; el header propio queda como conveniencia **no normativa** (v1) |
| Endpoints `/dx402/*` como parte del spec | "Endpoints de esta implementación", no normativos; el spec define **sólo** el objeto de evidencia, el `sellerSignature`, el `escrowRelease` y el recibo | Sin cambio de código; cambio de spec |
| `network` acepta nombres v1 en el anchor | Payloads **firmados** usan CAIP-2 (regla que offer-receipt fija incluso bajo v1) | El anchor sigue aceptando ambos en el wire; el recibo ya ata `chainId`; el spec lo declara |

## 2. Lo que NO cambia

Sobre, cripto, `paymentId`, gate, escalera de autoridad, `escrowRelease`,
`getHash`, `classify_rail`, recibo EIP-712, autorización EIP-712, todos los
verdicts y códigos de error. Los 119 anclajes verificados siguen válidos: el
formato del sobre y del recibo no se tocan.

## 3. Compatibilidad

- `from_requirements` sigue leyendo `extra.extensions` como **fallback** durante
  una versión, para no romper a KK/EM si tardan en migrar. Prioridad: top-level
  `extensions` + `acceptIndex` primero.
- `X-Durable-Evidence` se sigue emitiendo; los SDK ya lo parsean. Lo nuevo es
  que **además** va bajo `SettlementResponse.extensions`.
- `not_selected` y `Unknown` no cambian.

## 4. Orden de implementación (cada paso con test)

1. `PaymentRequiredResponse.extensions` (v1) — hoy no existe el campo; agregarlo
   como `Option<HashMap<String, Value>>` con `skip_serializing_if`.
2. `DurableEvidenceConfig::{declare_top_level, from_response}` + `acceptIndexes`.
3. `with_durable_offer` escribe top-level; `match_paid_requirement` devuelve
   también el **índice**; `OfferDecision::decide(accepts, extensions, paid_index)`.
4. `x402-reqwest`: eco del `info` en el payload v2 al elegir la oferta durable.
5. `layer.rs`: evidencia bajo `settlement.extensions` + header de compatibilidad.
6. `schema` constante + test que valida `info` contra él.
7. Spec **v0.3** = v0.2 reescrito al formato: título `# Extension:
   \`durable-evidence\``, Summary, PaymentRequired / PaymentPayload /
   SettlementResponse con ejemplos v1 **y** v2, Security, Privacy, Version
   History; sin nombre de producto ni anécdotas; §18.1 al frente.
8. Bump a 2.12.0, deploy, y el PR spec-only a `specs/extensions/durable-evidence.md`
   + fila en `docs/extensions/overview.mdx`, tras abrir el issue.

Estimación: 2-3 días de trabajo efectivo.
