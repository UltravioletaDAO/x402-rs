# 2026-09-13 — escrow `log`→`enforce` (medido, NO movido), estimate-antes-del-nonce en los 5 escritores ERC-8004, `invalid_asset` con nombre y `base-mainnet`

**Worker:** Orca x4-escrow-enforce (despachado por c0der, spec `SPEC-c0der.txt`, 2026-09-13 16:29Z), macOS.
**Rama:** `0xultravioleta/x4-escrow-enforce` desde `origin/main` = `d8a33603`.
**Versión:** `VERSION` 2.26.0 → **2.28.0** (2.27.0 la toma el PR #48 de PYUSD; ver "Para c0der").
**Base medida en producción:** `/version` = `2.26.0`, `GET /settle` → `escrowLifecycleAuth = "log"` (2026-09-13 ~16:45Z).

## Estado de cada fila del spec

| Fila | Estado | Resumen |
|---|---|---|
| [89] guard estimate-antes-de-reservar-nonce en los 5 handlers | **cerrada** | `send_call_estimated` en `src/chain/evm.rs`; los 14 `send()` de los 5 handlers pasan por él; test de regresión rojo sin el guard, verde con él |
| [103] activo desconocido → `internal_error (ref: <uuid>)` | **cerrada en código** (prod pendiente del deploy) | variante `UnsupportedAsset` → veredicto `invalid_asset` (200, `isValid:false`), EVM y Solana |
| [5] `base-mainnet` → 400 | **cerrada en código** (prod pendiente del deploy) | `"base" \| "base-mainnet"` en `Network::from_str`; el nombre de wire sigue siendo `base` |
| [126] `escrow_lifecycle_auth` a `enforce` | **no hecha — bloqueada** | medido: 1625 de 1630 órdenes de mainnet sin firma, todas de Execution Market; ni EM ni `uvd-x402-sdk-python` firman. Se queda en `log` (la suposición reversible del spec) |

## [126] La medición — por qué sigue en `log`

Ventana declarada: desde que `log` quedó activo (`2026-09-06T00:41:14Z`, handoff
`2026-09-05-lifecycle-auth-log-a-enforce.md`) hasta 2026-09-13 ~16:50Z. CloudWatch Logs
Insights sobre `/ecs/facilitator-production`, líneas `escrow lifecycle order accepted` /
`escrow lifecycle order NOT authorized` (`src/payment_operator/lifecycle_auth.rs:586-610`),
con los códigos ANSI quitados antes de parsear (el primer intento agrupó mal por eso).

| Veredicto | Acción | Órdenes |
|---|---|---:|
| `missing` | `refundInEscrow` | 1019 |
| `missing` | `release` | 606 |
| `unauthorized_role` | `release` | 2 (base-sepolia) |
| `ok` | `release` | 2 (base-sepolia) |
| `owner_unverifiable` | `release` | 1 (base-sepolia) |
| **total** | | **1630** |

Por operador (direcciones de contrato públicas, abreviadas):

| Operador | Red | Órdenes sin firma |
|---|---|---:|
| `0xb87f…102e` | polygon | 917 |
| `0xc237…938e` | avalanche / arbitrum / celo / optimism | 437 |
| `0x271f…f0eb` | base | 250 |
| `0x9620…8cc3` | monad | 18 |
| `0x69b6…001b` | ethereum | 2 |

- Los cinco operadores de mainnet están en `execution-market/shared/networks.generated.ts`:
  **el llamador sin firma es Execution Market**, en todas las redes.
- `grep -rl 'lifecycleAuth\|LifecycleOrder\|lifecycle_auth'` sobre `uvd-x402-sdk-python`
  (`cfdd270`, 2026-09-02) y `execution-market` (`10800a5c`, 2026-09-08) no devuelve nada:
  **ningún llamador firma todavía**. Las únicas órdenes `ok` son 2 en base-sepolia, del
  worker del 2026-09-05.
- Con `enforce` hoy: **99,7 % de las órdenes de mainnet rechazadas** con 4xx, que EM trata
  como permanente. No es una degradación, es un corte del rail.
- Dato lateral: la última orden de ciclo de vida es de `2026-09-11T23:21:31Z`; desde entonces
  el tráfico `settle_escrow` cayó a ~0. No lo investigué (fuera de alcance); si EM pausó
  escrow, esa pausa es también la ventana más barata para desplegar la firma.

"Arreglar a los llamadores" vive en otros dos repos (SDK Python → EM) y el spec no los
incluye: queda como fila para c0der con el orden ya escrito en el handoff del 2026-09-05
(SDK firma `LifecycleOrder` EIP-712 → EM consume y firma como dueño del operador → ventana
en `log` con `missing = 0` → una línea en `production.auto.tfvars`). `production.auto.tfvars`
**no se tocó**.

## [89] Estimate antes del nonce

**Qué cambió.** `src/chain/evm.rs` gana `send_call_estimated(call, network)` y
`EstimatedSendError { Reverted, Send }`. Estima gas contra `latest` ANTES de que el
`NonceFiller` reserve nada; si la estimación ejecuta y revierte, devuelve `Reverted` sin
enviar; si falla por transporte, cae al filler como antes (mismas reglas que
`EvmProvider::settle`, que ya tenía este guard). En éxito fija `gas` = la estimación, que es
lo que `GasFiller` habría puesto (`alloy-provider-1.7.3/src/fillers/gas.rs:91-93`), así que
cambia el orden, no el precio.

Por qué es seguro estimar por el mismo `FillProvider`: su `estimate_gas` solo corre
`prepare_call_sync` (`alloy-provider-1.7.3/src/fillers/mod.rs:473-477`), que para el wallet
pone `from` y para el nonce no hace nada; el nonce se reserva en `prepare`, que solo corre al
enviar (`fillers/nonce.rs:178-188`).

Los 14 envíos de los 5 handlers de `src/handlers.rs` pasan por el helper:
`post_feedback` (2), `post_revoke_feedback` (2), `post_append_response` (2),
`run_evm_registration` (6: tres overloads × legacy/1559), `transfer_agent_nft` (2).
Cada rama de error existente recibe el mismo `Err` que antes recibía de `send()` (mismo
status, mismo `release_feedback_proof` en `/feedback`, misma respuesta del job de
`/register`), así que la máquina de estados `pending → mint_confirmed → done/failed` no ve
un camino nuevo. Los dos comentarios `KNOWN GAP` (evm.rs y `post_feedback`) se reemplazaron.

**Test de regresión** (`chain::evm::tests`):
- `a_reverting_estimate_consumes_no_nonce` — transporte JSON-RPC guionado por método
  (`ScriptedRpc`), con el stack de fillers de producción y un `PendingNonceManager` real.
  `eth_estimateGas` revierte tras 50 ms, como un round-trip HTTP: sin ese retardo el
  `try_join!` de alloy terminaría antes de pollear el nonce y un envío sin guard pasaría el
  test. Afirma: nunca se llama `eth_getTransactionCount`, el manager no asignó, no hubo
  `eth_sendRawTransaction`, y el error es `Reverted`.
- `a_passing_estimate_is_sent_with_the_nonce_reserved_after_it` — orden
  `eth_estimateGas` < `eth_getTransactionCount` < `eth_sendRawTransaction`, una sola
  estimación, nonce asignado (7 → next 8).

## [103] `invalid_asset`

`FacilitatorLocalError::UnsupportedAsset(payer, network, asset)` + `chain::assert_supported_asset`
(`src/chain/mod.rs`), usado por la allow-list de EVM (`evm.rs`, antes `:2159`) y por verify y
settle de Solana (`solana.rs`, antes `:1972`, `:2004`). `IntoResponse` lo contesta como
`invalid_network`: `200 {"isValid":false,"invalidReason":"invalid_asset","payer":…}`.
`failure_category` (stream `/events`) lo nombra `invalid_asset`. El `Display` sigue diciendo
`unsupported_asset: network=…, asset=…`, así que los logs y el test del PR #48 que busca
`unsupported_asset` en el mensaje no cambian.

Base en producción hoy (2026-09-13T16:56:34Z, `POST /verify`, v1, network `base`, asset
inventado `0x…dEaD`, firma basura, timing válido): **`HTTP 400 {"error":"internal_error (ref: <uuid>)"}`**.

Tests (`handlers::rejection_reason_tests`): fila nueva en la tabla `causes()` (ahora 8, y
`no_two_rejection_causes_share_a_reason` exige 8 tokens distintos) y
`an_unknown_asset_is_a_named_rejection_not_an_internal_error`, que pasa por
`assert_supported_asset` y por la respuesta HTTP real.

No tocado: `upto` (`src/upto/permit2.rs:243`) contesta su propio `400 "Upto scheme error:
unsupported_asset…"`, ya con nombre en el texto; ver backlog.

## [5] `base-mainnet`

`src/network.rs:222`: `"base" | "base-mainnet" => Ok(Network::Base)`. Solo `FromStr`
(y `resolve_network`, que lo usa); el serde derivado y `Display` siguen en `base`, así que
`/supported` y el wire no cambian. Producción hoy: `/identity/base-mainnet/1` → `400`.
Cierra `UltravioletaDAO/uvd-x402-sdk-typescript#6` (referenciado en el commit).

## Rojo / verde (discriminante)

Con los tres arreglos quitados a la vez (guard reemplazado por `call.send()`,
`assert_supported_asset` devolviendo `Other(...)` como antes, alias borrado), los tres tests
nuevos, en una corrida:

```
test chain::evm::tests::a_reverting_estimate_consumes_no_nonce ... FAILED
  a nonce was fetched for a call that reverted on estimation: ["eth_estimateGas", "eth_getTransactionCount", "eth_chainId"]
test handlers::rejection_reason_tests::an_unknown_asset_is_a_named_rejection_not_an_internal_error ... FAILED
  a rejected payment is a verdict, not a transport error  left: 400  right: 200
test network::tests::base_mainnet_is_an_alias_for_base ... FAILED
test result: FAILED. 0 passed; 3 failed
```

Archivos restaurados desde copia y comparados con `cmp`. Con el código del PR: verdes (tabla
de pre-CI).

## Pre-CI

macOS, `rustc` stable, contra el árbol de este PR:

| Paso | Comando | Resultado |
|---|---|---|
| fmt | `cargo fmt --check` | ok |
| landing (CI `ci.yaml:148`) | `python3 scripts/verify_landing_canonical.py --offline` | `[OK]` |
| tests x402-rs (CI `:154`) | `cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1` | lib **1067 passed**, bin **1116 passed**, integraciones ok; **0 failed** |
| tests crates (CI `:156`) | `cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance -- --test-threads=1` | 9 suites ok, 0 failed |
| clippy (CI no lo corre) | `cargo clippy --locked -p x402-rs --features … --tests` | 288 avisos preexistentes en el crate, **0 en líneas que cambia este PR** |
| terraform | — | no aplica: `terraform/` sin tocar |

CI no corre `cargo fmt` ni `clippy` (`.github/workflows/ci.yaml:128-157`: landing offline,
build, test x402-rs, test crates). Los corrí igual; el paso `build` de CI lo cubre la
compilación de `cargo test` con las mismas features.

## Cómo verificar en producción (después del deploy)

```bash
curl -s https://facilitator.ultravioletadao.xyz/version                       # {"version":"2.28.0"}
curl -s -o /dev/null -w '%{http_code}\n' https://facilitator.ultravioletadao.xyz/identity/base-mainnet/1   # 200
curl -s https://facilitator.ultravioletadao.xyz/settle | jq .escrowLifecycleAuth   # "log" (sin cambio, a propósito)
```

Para [103]: el mismo `POST /verify` de arriba (v1, `network: "base"`, `asset`
`0x000000000000000000000000000000000000dEaD`, `validBefore` = ahora + 600, `to` = `payTo`)
debe contestar `200` con `"invalidReason":"invalid_asset"` en vez de `400 internal_error`.

[89] no tiene sonda segura en producción (haría falta mandar una escritura ERC-8004 que
revierta); lo que se observa es la línea nueva `Gas estimation reverted; call not sent, no
nonce consumed` en CloudWatch cuando ocurra, y la ausencia de congelamientos de nonce en
Monad detrás de un `/feedback` fallido.

## Cómo se despliega

Leído de `.github/workflows/ci.yaml`: el merge a `main` dispara `test`; `deploy`
(`:453-460`) tiene `needs: [test, preflight]` y corre solo en push a `main`; construye la
imagen con `VERSION`, hace `terraform apply` acotado a task definition + service +
autoscaling (`:541-547`), espera `aws ecs wait services-stable` (`:643-645`) y verifica
`/health` (`:650-659`). **Mergear es desplegar.** Nada de este PR toca `terraform/`.

## Para c0der

- **No hice [126]** y no se debe hacer todavía: medido arriba. El siguiente paso son dos
  encargos fuera de este repo (SDK Python firma `LifecycleOrder`; EM lo consume) y después
  una ventana en `log` con `missing = 0`. Quién decide la ventana: el dueño.
- **VERSION choca con el PR #48** (2.27.0). El que mergee segundo rebasea `VERSION`; no hay
  otro solapamiento de lógica: #48 usa `is_supported_asset` solo en `src/network.rs` (tests),
  y su test sobre Solana busca `unsupported_asset` en el `Display`, que no cambió. Sí
  tocamos los dos `src/chain/solana.rs` y `src/handlers.rs`: conflicto textual posible,
  semántico no.
- **Issue #6 del SDK TS** se cierra con el merge (el commit lo referencia con `Closes`). Si
  preferís cerrarlo recién tras el `curl` de producción, quitá la referencia antes de
  mergear.
- **Mientras mergeás**: `run_evm_registration` es el único de los 5 con máquina de estados;
  el diff ahí son 6 líneas `send()` → helper, sin otra rama nueva. Vale un vistazo del
  refutador.
- Worktree: `contracts/` untracked es previo a este worker; no lo toqué ni lo stageé.

## Filas de backlog nuevas

| Prioridad | Fila | Evidencia |
|---|---|---|
| P1 | Los escritores ERC-8004 firman con el signer EVM compartido **sin writer lease**: solo `/settle` lleva `settle_writer_gate` (`src/handlers.rs:113`) y `signing_permit()` (`src/chain/evm.rs:848`). Con más de una tarea ECS, dos tareas pueden asignar nonces del mismo signer a la vez. El guard de este PR no lo cubre (evita nonces quemados por revert, no colisiones entre tareas) | `grep -n settle_writer_gate src/handlers.rs`; `grep -rn 'signing_permit()' src` |
| P1 | [126] SDK Python firma `LifecycleOrder` → EM firma → ventana en `log` con `missing = 0` → `enforce` | tabla de arriba; handoff 2026-09-05 §"Orden para llegar a enforce" |
| P2 | Escrow en silencio desde 2026-09-11 23:21Z (última orden de ciclo de vida; `settle_escrow` ~0 desde entonces): confirmar con EM si es pausa deliberada | query de la sección [126] |
| P3 | `upto` contesta un activo desconocido como `400 "Upto scheme error: unsupported_asset…"`, no con el veredicto `invalid_asset` | `src/upto/permit2.rs:243-247`, `src/handlers.rs` rama `upto_error` |

## Declaración

Ningún `settle`, `release`, `refundInEscrow` ni escritura ERC-8004 contra producción ni
contra ninguna cadena. Contra producción: `GET /version`, `GET /settle`,
`GET /identity/base-mainnet/1` y un `POST /verify` con activo inventado y firma basura
(sin fondos). CloudWatch solo lectura. Ningún secreto leído ni impreso. `terraform/` sin
tocar. Un push, de la rama `0xultravioleta/x4-escrow-enforce`.

LISTO PARA c0der — código en `a2ea7dd9` (estimate antes del nonce + `invalid_asset`) sobre `41d9dbc7` (`base-mainnet`); este handoff y `VERSION` 2.28.0 van en el commit siguiente, cuyo SHA (head del push) está en el cuerpo del PR.
