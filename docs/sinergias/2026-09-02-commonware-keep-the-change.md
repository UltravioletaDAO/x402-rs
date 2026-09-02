# Sinergias con "Keep the Change" (Commonware) — 3.1 x402-rs — el facilitador

> Depositado por c0der el 2026-09-02. Fuente: `c0der/docs/plans/commonware-clearing-que-adoptar.md`
> (análisis de los 15 proyectos x402 del stack: 66 sinergias propuestas, 40 sostenidas por un refutador
> que abrió cada `archivo:línea`; las descartadas y su motivo están en la sección 4 del documento fuente).
> Post original: <https://commonware.xyz/blogs/clearing> (Patrick O'Grady, 2026-08-19). Esta carpeta
> `docs/sinergias/` es donde c0der deja lo que otros análisis encuentren para este proyecto.

## Principios transversales que aplican a todo el stack (títulos; el detalle está en la fuente, sección 2)

- P1 · La preconfirmación es un par firmado transferible, no un booleano
- P2 · El reintento devuelve el mismo recibo, y la clave se DERIVA de la identidad del pedido
- P3 · Una escritura cara por cuenta cambiada, no por evento
- P4 · La retención de evidencia se ata a la ventana de disputa — y la ventana no existe
- P5 · La ventana de idempotencia y la de retención de evidencia son dos relojes
- P6 · Disputa de un solo tiro: el que reclama presenta el par, y un predicado lo resuelve
- P7 · Un piso es seguro para gastar; el estado que se reconcilia tarde se ajusta, nunca se sobrescribe
- P8 · El benchmark declara qué variable NO aparece
- P9 · El identificador de deduplicación lo pone quien ya lo usa, no vos *(no sale del post)*
- P10 · Cada componente declara su postura ante fallo en su propio doc-comment *(no sale del post)*
- P11 · El valor efectivo de un parámetro se publica en un endpoint legible *(regla del CLAUDE.md global, no del post)*

## Lo específico de este proyecto (sección 3.1 de la fuente, verbatim)

### 3.1 x402-rs — el facilitador

| Idea (sección del post) | Aplicación concreta | archivo:línea | Esf. | Valor | Riesgo | Cómo se verifica |
|---|---|---|---|---|---|---|
| Reintento idempotente ("Payments as Fast as Browsing the Web") | Derivar la clave del cuerpo cuando falta el header (nonce EIP-3009 + `from` + red); el material ya está y `hash_request_body` también | `src/handlers.rs:2929-2933`, import en `:40`, `hash_request_body` en `:171-177`, carrera declarada en `src/idempotency_store.rs:30-38` | M | alto | **medio** | Dos POST del mismo cuerpo sin header ⇒ **una** `send_transaction`. **No** por `errorReason` (no existe el campo ni el código) |
| El cierre no crece con los pagos ("One Row per Changed Account" + "The Close Never Grows (with Payments)") | Medir pagos / pares `(payer, payTo)` distintos por día: el número que decide si la compresión del post vale algo acá | `src/transaction_store.rs:91` (payer), `:106` (pay_to); ruta `GET /transactions` en `src/handlers.rs:11641` | **S** | alto | bajo | Script de solo lectura que imprima T, A y A/T por día |
| Salida unilateral ("A Deadline to Exit") | Medir **por red** si el payer puede recuperar con el facilitador apagado (release/refund) | `src/chain/evm.rs:445-455`, `src/payment_operator/addresses.rs:411-415`, `mod.rs:15-16`, `src/handlers.rs:3925` | M medir / L arreglar | alto | **alto si el resultado es "no hay salida"** | Leer la condición de cada operador desplegado y probar en fork que el payer releasea con el facilitador apagado |
| Validar antes de encolar ("Validate Everything Up Front") | Extender el guard **estimate-antes-de-reservar-nonce** a los 5 handlers que no lo tienen | `src/handlers.rs:4590-4617` (nombra los cinco y el costo medido); patrón bueno en `src/chain/evm.rs:631-639` y `:663-678` | M | alto | medio | El test que falta: `estimate_gas` mockeado a revert y afirmar que `get_transaction_count` **no avanzó** |

**Notas de la refutación (parte del ítem, no comentario al margen).**
1. **Idempotencia derivada:** derivarla para **toda** petición convierte la rama fail-closed
   de `src/handlers.rs:3065-3081` en **dependencia dura de DynamoDB para todo el settle del
   stack**; hoy solo ata a quien manda el header. Va **con flag o con fail-OPEN en el camino
   derivado**. Y la verificación original propuesta no es implementable: `TransactionRecord`
   (`src/transaction_store.rs:81-111`) no tiene campo `errorReason` y `FacilitatorErrorReason`
   (`src/types.rs:1499-1518`) no tiene variante de nonce gastado.
2. **A/T:** `docs/plans/batch-settlement/` está **UNTRACKED** (`git status`:
   `?? docs/plans/batch-settlement/`) — un worker en worktree nuevo **no lo verá**. Es la
   trampa 1 de `uvd-orca-worker`.
3. **Salida unilateral:** no es una contradicción. `SkaleBase` lista **tres** operadores
   (`addresses.rs:411-415`, el de `:414` es *"EM operator v2 (fixed OrCondition, OR release
   payer|facilitator)"*) y Base **dos** (test en `:489`): un `StaticAddressCondition` en uno
   y un `OrCondition` en otro conviven. Y como el facilitador **solo autoriza**
   (`mod.rs:15-16`), el trabajo de arreglo es de quien desplegó los contratos.
4. **Guard de nonce:** el hueco está documentado en el repo **desde el 2026-08-28**. El post
   lo enmarca bien pero **no lo descubre**: es backlog existente con mejor argumento. Costo
   medido en Monad el 2026-08-24: nonces 379/380/381 congelados entre 151 y 283 s.
