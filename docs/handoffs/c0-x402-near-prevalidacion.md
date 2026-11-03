# Pre-validación del escrow NEAR antes de relayar

Fecha: 2026-11-03. Código: `src/chain/near/escrow.rs`, tests en `src/chain/near/escrow/tests.rs`.

## Qué hace

Antes de relayar, el facilitador rechaza lo que la cadena rechazaría, hasta donde la cadena lo deja ver:

El token (lo que recibe el cliente) nombra la condición en inglés, como el resto del módulo. El error que la
cadena habría dado (el código del contrato o el del runtime) va en la línea de log del rechazo
(`EscrowRejectReason::chain_error`).

| Qué | Dónde se lee | Token | Error de la cadena (log) |
|---|---|---|---|
| Autorización: token del escrow | `get_config` | `near_escrow_token_not_allowed` | `E01_TOKEN_NO_PERMITIDO` |
| Autorización: pausa | `get_config` | `near_escrow_paused` | `E02_PAUSADO` |
| Autorización: operador | `get_operator` | `near_escrow_operator_not_found` (el mismo del ciclo de vida) | `E04_OPERADOR_INEXISTENTE` |
| Autorización: mínimo del operador | `get_operator` | `near_escrow_below_operator_minimum` | `E05_DEBAJO_DEL_MINIMO` |
| Autorización: tope por pago | `get_config` | `near_escrow_over_per_payment_cap` | `E06_TOPE_POR_PAGO` |
| Autorización: tope por payer | `get_operator` | `near_escrow_over_payer_tvl_cap` | `E08_TVL_PAYER` |
| Autorización: TVL del operador | `get_operator` | `near_escrow_over_operator_tvl_cap` | `E09_TVL_OPERADOR` |
| Autorización: TVL global | `get_config` | `near_escrow_over_global_tvl_cap` | `E10_TVL_GLOBAL` |
| Autorización: fee del operador | `get_operator` | `near_escrow_fee_over_signed_maximum` | `E12_FEE_MAYOR_AL_MAXIMO` |
| Autorización: id ya usado | `get_payment` | `near_escrow_payment_id_used` | `E13_ID_USADO` |
| Autorización: storage | `storage_balance_of(payer)` del escrow y `get_operator` | `near_escrow_no_storage` | `E14_SIN_STORAGE` |
| `release` parcial | `get_payment`: `amount - released - refunded` | `near_escrow_amount_exceeds_live` | `E24_MONTO` |
| Todo delegate (autorización y ciclo de vida) | la altura del head (`block`, finalidad optimista) | `near_escrow_delegate_expired` si `max_block_height <= head` | `DelegateActionExpired` |
| Todo delegate | la misma altura del head | `near_escrow_delegate_nonce_too_large` si `nonce >= head * 1_000_000` (`AccessKey::ACCESS_KEY_NONCE_RANGE_MULTIPLIER`) | `DelegateActionNonceTooLarge` |

`judge_authorize` es una función pura que sigue el orden de guardas de `plan_authorize` del contrato
(`escrow-v1/src/authorize.rs`): E01, E02, (E03 local), E04, (E15 local), E05, E06, E08, E09, E10, (E11 local),
E12, E13, E14. La primera que falla decide.

## Regla que no se rompe

**Nunca se rechaza algo que el contrato o el runtime aceptarían** (con la excepción acotada del techo del
nonce, abajo). Cada chequeo usa solo lo que las vistas muestran. Una guarda que lee más de lo que exponen las vistas se chequea sobre la parte que sí exponen, y en ese
caso el código de error puede ser otro de los que también rechazan, nunca un rechazo de más.

- **E14**: el piso de storage (`v1_storage_floor`) cuenta solo los bytes que escribe todo pago de `arras-v1`.
  Se rechaza solo si el `available` NEP-145 del payer en el escrow queda debajo del piso **y** el patrocinio
  del operador (pool o tope por payer) también. Verificado contra la cadena el 2026-11-03: un pago real de
  `arras-v1b-1101.testnet` que escribió solo esos registros cobró el piso, ni un byte más (532 bytes).
- E14 se juzga solo si `get_config.version == "arras-v1"`, porque el piso sigue el layout de v1. Los demás
  chequeos corren con cualquier versión cuyas vistas conserven los campos de v1.
- **Vencimiento**: el bloque que ejecuta un delegate relayado siempre es posterior al head leído (si el nodo
  va atrasado, ese bloque queda todavía más adelante). Un `max_block_height` igual o menor que el head falla
  donde sea que corra; uno mayor puede aterrizar y se relaya.
- **Techo del nonce**: el runtime topa el nonce con la altura del bloque que ejecuta por 10^6, y ese bloque
  siempre es posterior al head leído: un nonce debajo de `head * 10^6` queda debajo del techo del runtime
  donde sea que corra. `max_block_height` no sube nada, solo acota el bloque que lo puede correr. Es la única
  guarda que podría rechazar algo que el runtime aceptaría: un nonce en `[head * 10^6, altura que ejecuta *
  10^6)`. Ninguna wallet lo firma, porque la llave nace con `(altura del bloque que la agregó - 1) * 10^6`
  (doc de `AccessKey::nonce` en near-primitives-core 0.34.6) y cada transacción usa el siguiente.

Una vista que responde sin un campo, o con uno que no es U128 canónico, no da veredicto: no se relaya nada
(error, como ya hacía el gate del ciclo de vida).

## Caché

`get_config` y `get_operator` se reutilizan `VIEW_CACHE_SECS` (10 s) por contrato y por operador. Las claves
están acotadas, porque contrato y operador se validan contra la config antes de leer. **Un rechazo nunca se
decide con una respuesta cacheada**: si la caché dice que no, se leen las dos vistas de nuevo y se decide con
eso. La caché solo ahorra lecturas en el camino a un relay. `get_payment` y el storage del payer se leen
siempre.

## Log

Cada rechazo previo deja una línea `info` con la red, el token y el error de la cadena (`chain_error`), sin el
sender ni montos:
`NEAR escrow request refused before relaying: the chain would refuse it` (para CloudWatch, filtrar por ese
texto). Los rechazos que ya existían siguen con su `warn` de siempre.

## Para extender

- Si una versión nueva del contrato expone más estado en sus vistas, `judge_authorize` puede usarlo.
- Si v2 cambia el layout de `Payment`, `v1_storage_floor` no aplica: agregar el de v2 detrás de su `version`.

## Cómo verificar

```bash
cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl,hedera --lib -- chain::near::escrow --test-threads=1
```
