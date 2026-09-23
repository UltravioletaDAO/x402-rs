# `/settle`: una falla después del envío dice `retryable: false` y trae el hash (2.39.6)

> Encargo de c0der (task_e3cfd7845e3b), 2026-09-23. Rama `c0der/emitido-no-retryable`,
> PR #102, sobre `main` con #101 (2.39.4) y #103 (2.39.5). Ronda 1 del refutador:
> MERGEABLE CON RONDA, sin P0 ni P1; esta entrega cierra sus P2 y dos P3.

## El contrato

Toda falla de `/settle` producida cuando la transacción pudo haber salido del
facilitador lo dice en el cuerpo: `"retryable": false`, sin `Retry-After`, y
`transaction` + `paymentId` cuando se conocen. Las fallas previas al envío no
cambian. La referencia para un vendedor es `docs/settle-errors.md`.

Dónde vive, por camino:

| Camino | Mecanismo |
|---|---|
| EVM exact y esquemas alternativos | `send_transaction_from` hace `fill` + `send_raw_transaction`; `broadcast_may_have_queued` decide; `SettlementUnconfirmed` con el hash de los bytes enviados |
| EVM, texto sin hash (guarda de nonce) | brazo `ContractCall` con `ChainFailure::may_have_broadcast()` |
| `upto`, escrow, `refund` | variantes tipadas `SettlementUnconfirmed` + `alt_scheme_unconfirmed` en `handlers.rs` |
| FHE | `FheProxyError::may_have_settled` + `fhe_settle_failure` |
| Forward al holder del writer lease | `ForwardFailure { delivered }` + `forward_failure_response` |
| Solana (exact y barrido de la cuenta de liquidación) | `TransactionInt::send_and_confirm` + `send_may_have_landed` |
| NEAR, Stellar, Algorand, Sui, XRPL | predicado por cliente + `settle_outcome` por familia (un `SettlementUnconfirmed` nunca sale como `200 success:false`) |
| Hedera | `unconfirmed()` en las dos escrituras del registro firmado |
| Riel de recibos | `may_have_sent` + `not_retryable` en `finish` |

Los tests fijan cada fila; `settle_outcome` existe para que el paso "resultado del
envío → respuesta de settle" tenga candado de test en cada familia. Hedera no tiene
test de punta a punta: llegar a la cofirma exige Mirror y consenso en red.

Hipótesis de texto sin medir, dichas así en el código: las variantes `AlreadyKnown`,
`known transaction` y `already imported` en EVM (`node_already_holds`), y el texto de
`AlreadyProcessed` en el preflight de Solana. Las dos se leen en la dirección segura:
"pudo haber aterrizado", nunca éxito confirmado.

## Backlog para c0der (P3 del refutador que no entran en este PR)

- **P3-4. Lectura de estado de Solana que falla una vez.** Hoy corta la espera de
  confirmación con `settlement_unconfirmed` (seguro). Reintentar la lectura hasta el
  timeout y dejar la última verificación como está daría un veredicto en más casos.
- **P3-5. Idle del ALB contra la espera de recibo en Ethereum.** `alb_idle_timeout =
  600` (`terraform/environments/production/production.auto.tfvars`) y la espera de
  recibo en Ethereum es 900 s (`src/chain/evm.rs`). Un settle que espere más de 600 s
  recibe el 504 del ALB, que no lleva el contrato. Bajar la espera o subir el idle por
  encima de 930 s (forward incluido).
- **P3-6. `503 idempotency_cache_corrupt`** (`src/handlers.rs`) responde por una clave
  cuyo registro guardado es un éxito, sin `retryable: false`.
- **P3-7. "Cuándo es seguro volver a firmar", por familia.** La doc dice "not found
  after that chain's finality window". Precisarlo: EVM `validBefore` vencido y
  `authorizationState` sin usar; Solana blockhash vencido; XRPL `LastLedgerSequence`;
  Hedera `validStart + validDuration`.
- **P3-8. Cuerpo reescrito del riel de recibos.** Conserva el token del proveedor
  (`upstream_rpc_unavailable (ref: …)`) junto a `retryable: false`, sin
  `success: false`. Los SDK deciden por `retryable`; el token contradice al campo.
- **P3-9. Registros de recibos guardados antes del deploy** con `retryable: true` en la
  respuesta almacenada se reenvían tal cual a un reenvío ligado. Transitorio.
- **`ENABLE_SETTLEMENT_ACCOUNT`.** El código lo trata como encendido por defecto
  (`src/chain/solana.rs`, `unwrap_or(true)`) y terraform no lo fija, mientras dos
  comentarios del mismo archivo dicen "OFF by default". Decidir el default y alinear
  código o comentarios.
