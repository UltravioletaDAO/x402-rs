# 2026-09-13 — escrow `log`→`enforce` (medido, NO movido: el bloqueo son llaves y roles), estimate-antes-del-nonce en los 5 escritores ERC-8004, `invalid_asset` con nombre y `base-mainnet`

**Worker:** Orca x4-escrow-enforce (despachado por c0der, spec `SPEC-c0der.txt`, 2026-09-13 16:29Z; ronda 2 `RONDA2-c0der.txt`, 20:xxZ), macOS.
**Rama:** `0xultravioleta/x4-escrow-enforce`, rebaseada sobre `origin/main` = `c2346897` (PR #48 PYUSD).
**Versión:** `VERSION` 2.27.0 → **2.28.0**; entrada en `docs/CHANGELOG.md`.
**Base medida en producción:** `/version` = `2.26.0`, `GET /settle` → `escrowLifecycleAuth = "log"` (2026-09-13 ~16:45Z).

## Estado de cada fila del spec

| Fila | Estado | Resumen |
|---|---|---|
| [89] guard estimate-antes-de-reservar-nonce en los 5 handlers | **cerrada** | `send_call_estimated` en `src/chain/evm.rs`; los 14 `send()` de los 5 handlers pasan por él; los dos tests de regresión son rojos sin el guard y verdes con él |
| [103] activo desconocido → `internal_error (ref: <uuid>)` | **cerrada en código** (prod pendiente del deploy) | variante `UnsupportedAsset` → veredicto `invalid_asset` (200, `isValid:false`), EVM y Solana |
| [5] `base-mainnet` → 400 | **cerrada en código** (prod pendiente del deploy) | `"base" \| "base-mainnet"` en `Network::from_str`; el nombre de wire sigue siendo `base` |
| [126] `escrow_lifecycle_auth` a `enforce` | **no hecha — bloqueada por llaves y roles** | EM y el SDK Python YA firman; las órdenes llegan sin firma porque la llave de EM no tiene rol sobre esos escrows. Se queda en `log` |

## [126] Por qué sigue en `log`

### Lo que ve el facilitador

Ventana declarada: desde que `log` quedó activo (`2026-09-06T00:41:14Z`, handoff
`2026-09-05-lifecycle-auth-log-a-enforce.md`) hasta 2026-09-13 ~16:50Z. CloudWatch Logs
Insights sobre `/ecs/facilitator-production`, líneas `escrow lifecycle order accepted` /
`escrow lifecycle order NOT authorized` (`src/payment_operator/lifecycle_auth.rs:586-610`),
con los códigos ANSI quitados antes de parsear.

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

**Órdenes no son escrows.** Contando escrows distintos por (operador, red, payer, receiver)
entre las 1624 órdenes de mainnet sin firma: **298** (285 con `release`, 17 con
`refundInEscrow`). El refutador de c0der contó **303** (286 / 17); la diferencia es de clave,
porque el log no trae `salt` ni monto y ninguna de las dos cuentas identifica el escrow
exacto. El grueso de los refunds es reintento: las 892 `refundInEscrow` de polygon caen sobre
**un solo par (payer, receiver)**, entre `2026-09-08T22:09Z` y `2026-09-10T18:32Z` (primera y
última línea de polygon). Con `enforce` hoy quedarían sin mover **~300 escrows**, no "el 99,7 %
de 1625 movimientos" como decía la ronda 1.

### Quién llama y por qué no firma

- **El llamador es el `mcp-server` de Execution Market.** `/ecs/em-production/mcp-server`,
  2026-09-07 a 2026-09-11: **976** líneas `lifecycle_auth: EM no tiene rol para firmar %s en %s
  … — se manda sin firma` (**603** `release`, que cuadran con las 606 del facilitador; **373**
  `refundInEscrow`). Además `/ecs/em-production/payshell-mcp`: **50** `refundInEscrow` con la
  misma línea (no estaba en el reporte del refutador).
- **EM sí firma** (`execution-market` `origin/main` = `2975ae7c`, contiene `dff9af68`
  "EM firma release y refundInEscrow — y la llave que tiene no alcanza", 2026-09-06):
  - `mcp_server/integrations/x402/lifecycle_auth.py` arma la orden; la llave sale de
    `EM_LIFECYCLE_SIGNER_KEY` (dedicada, preferida) o de la llave general (`:88`, `:104`).
  - Cuando esa llave no es payer, receiver ni owner del operador, loguea `:280-281` y manda
    **sin firma**.
  - Cableado en `payment_dispatcher.py:2328-2349` (release), `:2818-2839` (refund) y
    `:5145-5160` (refund vía `sdk_lifecycle_kwargs`).
  - El gate de firma del payer, `EM_LIFECYCLE_PAYER_SIGNS`, está **apagado por default**
    (`lifecycle_auth.py:411-418`) y no aparece en `infrastructure/` (`git grep` sin hits).
- **El SDK Python también firma** (`uvd-x402-sdk-python` `origin/main`): `build_lifecycle_auth`
  en `src/uvd_x402_sdk/escrow_signing.py:770`, llamado desde `advanced_escrow.py:1023-1038`;
  entró con `5a7007c` (0.78.0, 2026-09-05). (El spec de ronda 2 citaba `84e20ae`, que está en
  `main` pero es un commit de `orca.yaml`.)

**Error de la ronda 1, dicho claro:** medí sobre los checkouts locales sin `git fetch`. EM
local estaba en `main` = `10800a5c`, que NO contiene `dff9af68`; el SDK local estaba en la
rama `feat/x402client-fetch-buyer-loop` = `cfdd270`, que NO contiene `5a7007c`. El `grep`
vacío era cierto para esos árboles y falso para lo que corre. De ahí salían los dos encargos
"SDK firma → EM consume", que quedan **borrados**.

### Orden para llegar a `enforce`

1. **El dueño decide qué firmante con rol usa EM**: la llave del `FEE_RECIPIENT()` del
   operador cargada como `EM_LIFECYCLE_SIGNER_KEY`, u operadores cuyo owner sea el firmante
   que EM ya tiene. El secreto se nombra; su valor nunca.
2. **Encender `EM_LIFECYCLE_PAYER_SIGNS`** para que los `release` los firme el payer.
3. **Los refunds** necesitan firma de receiver u owner del operador; el paso 1 los cubre si
   el firmante es el owner.
4. **Ventana en `log`** hasta que `verdict=missing` sea 0 en los operadores de EM (misma
   query de arriba).
5. Recién ahí, **una línea** en `terraform/environments/production/production.auto.tfvars`
   (`escrow_lifecycle_auth = "enforce"`); rollback, la misma línea a `"log"`.

`production.auto.tfvars` **no se tocó**.

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

**Tests de regresión** (`chain::evm::tests`), sobre un transporte JSON-RPC guionado por
método (`ScriptedRpc`) con el stack de fillers de producción y un `PendingNonceManager` real.
`eth_estimateGas` contesta tras 50 ms, como un round-trip HTTP: sin ese retardo el
`try_join!` de alloy terminaría antes de pollear el nonce y un envío sin guard pasaría.
- `a_reverting_estimate_consumes_no_nonce` — la estimación revierte. Afirma: nunca se llama
  `eth_getTransactionCount`, el manager no asignó, no hubo `eth_sendRawTransaction`, y el
  error es `Reverted`.
- `a_passing_estimate_is_sent_with_the_nonce_reserved_after_it` — la estimación pasa. Afirma
  que la estimación **ya contestó** (marcador `eth_estimateGas answered`) antes de que se
  **pida** `eth_getTransactionCount`, que el nonce va antes de `eth_sendRawTransaction`, que
  hay una sola estimación y que el nonce se asignó (7 → next 8). **Ronda 2:** en la ronda 1 este
  test comparaba solo el orden de los pedidos y pasaba igual sin el guard (el filler también
  pide la estimación primero); el marcador de respuesta es lo que lo vuelve discriminante.

## [103] `invalid_asset`

`FacilitatorLocalError::UnsupportedAsset(payer, network, asset)` + `chain::assert_supported_asset`
(`src/chain/mod.rs`), usado por la allow-list de EVM (`evm.rs`) y por verify y settle de
Solana (`solana.rs`). `IntoResponse` lo contesta como `invalid_network`:
`200 {"isValid":false,"invalidReason":"invalid_asset","payer":…}`. `failure_category` (stream
`/events`) lo nombra `invalid_asset`. El `Display` sigue diciendo
`unsupported_asset: network=…, asset=…`, así que los logs y los tests de PYUSD (#48, ya en
`main`) que buscan `unsupported_asset` en el mensaje no cambian.

Base en producción (2026-09-13T16:56:34Z, `POST /verify`, v1, network `base`, asset
inventado `0x…dEaD`, firma basura, timing válido): **`HTTP 400 {"error":"internal_error (ref: <uuid>)"}`**.

Tests (`handlers::rejection_reason_tests`): fila nueva en la tabla `causes()` (ahora 8, y
`no_two_rejection_causes_share_a_reason` exige 8 tokens distintos) y
`an_unknown_asset_is_a_named_rejection_not_an_internal_error`, que pasa por
`assert_supported_asset` y por la respuesta HTTP real.

No tocado: `upto` (`src/upto/permit2.rs:243`) contesta su propio `400 "Upto scheme error:
unsupported_asset…"`, ya con nombre en el texto; ver backlog.

## [5] `base-mainnet`

`src/network.rs`: `"base" | "base-mainnet" => Ok(Network::Base)`. Solo `FromStr` (y
`resolve_network`, que lo usa); el serde derivado y `Display` siguen en `base`, así que
`/supported` y el wire no cambian. Producción hoy: `/identity/base-mainnet/1` → `400`.
Cierra `UltravioletaDAO/uvd-x402-sdk-typescript#6` (referenciado en el commit).

## Rojo / verde (discriminante)

Ronda 1 — con los tres arreglos quitados a la vez (guard reemplazado por `call.send()`,
`assert_supported_asset` devolviendo `Other(...)` como antes, alias borrado):

```
test chain::evm::tests::a_reverting_estimate_consumes_no_nonce ... FAILED
  a nonce was fetched for a call that reverted on estimation: ["eth_estimateGas", "eth_getTransactionCount", "eth_chainId"]
test handlers::rejection_reason_tests::an_unknown_asset_is_a_named_rejection_not_an_internal_error ... FAILED
  a rejected payment is a verdict, not a transport error  left: 400  right: 200
test network::tests::base_mainnet_is_an_alias_for_base ... FAILED
test result: FAILED. 0 passed; 3 failed
```

Ronda 2 — el test del camino feliz con el guard quitado:

```
test chain::evm::tests::a_passing_estimate_is_sent_with_the_nonce_reserved_after_it ... FAILED
  the nonce was requested before the estimate answered: ["eth_estimateGas", "eth_getTransactionCount", "eth_chainId", "eth_estimateGas answered", "eth_sendRawTransaction"]
test chain::evm::tests::a_reverting_estimate_consumes_no_nonce ... FAILED
  a nonce was fetched for a call that reverted on estimation: ["eth_estimateGas", "eth_getTransactionCount", "eth_chainId", "eth_estimateGas answered"]
test result: FAILED. 0 passed; 2 failed
```

Archivos restaurados desde copia y comparados con `cmp`. Con el código del PR: verdes (tabla
de pre-CI).

## Pre-CI (ronda 2, sobre `c2346897` + esta rama)

macOS, `rustc` stable, árbol rebaseado sobre `c2346897`:

| Paso | Comando | Resultado |
|---|---|---|
| fmt | `cargo fmt --check` | ok (la primera pasada marcó una línea larga del test nuevo; `cargo fmt` la partió, solo espacios, y los dos tests de nonce se re-corrieron verdes después) |
| landing (CI `ci.yaml:148`) | `python3 scripts/verify_landing_canonical.py --offline` | `[OK]` |
| tests x402-rs (CI `:154`) | `cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1` | lib **1074 passed**, bin **1123 passed** (+7 respecto de la ronda 1: los tests de PYUSD de #48), integraciones ok; **0 failed** |
| tests crates (CI `:156`) | `cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance -- --test-threads=1` | 9 suites ok, 0 failed |
| clippy (CI no lo corre) | `cargo clippy --locked -p x402-rs --features … --tests` | 288 avisos preexistentes en el crate, **0 en líneas que cambia este PR** (diff contra `origin/main`) |
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

- **No hice [126]** y no se debe hacer todavía. El bloqueo no es código en otro repo: EM y
  el SDK ya firman. Es **qué llave con rol usa EM** (decisión del dueño) y encender
  `EM_LIFECYCLE_PAYER_SIGNS`; el orden está arriba. Los dos encargos "SDK firma → EM consume"
  de la ronda 1 quedan borrados: salieron de checkouts locales viejos.
- **Rebase sobre #48 hecho:** el único conflicto fue `VERSION` (queda 2.28.0);
  `solana.rs`, `handlers.rs` y `network.rs` se auto-mergearon, y la suite completa corrió
  sobre el resultado (tabla de pre-CI).
- **Issue #6 del SDK TS** se cierra con el merge (el commit lo referencia con `Closes`). Si
  preferís cerrarlo recién tras el `curl` de producción, quitá la referencia antes de
  mergear.
- **La fila P1 "escritores ERC-8004 sin writer lease" de la ronda 1 era falsa**:
  `require_writer_lease` envuelve `/register`, `/feedback`, `/feedback/response` y el resto
  del router ERC-8004 (`src/handlers.rs:1828`) y `/feedback/revoke` (`:1790`). Queda un
  residual P2 (abajo).
- **El reporte del refutador** (`REFUTACION-x4-49.md`) está en una ruta de Windows que esta
  Mac no ve; trabajé con lo que cita `RONDA2-c0der.txt` y verifiqué cada archivo:línea contra
  los `origin/main` de EM y del SDK.
- Worktree: `contracts/` untracked es previo a este worker; no lo toqué ni lo stageé.

## Filas de backlog nuevas

| Prioridad | Fila | Evidencia |
|---|---|---|
| P1 | [126] firmante con rol para EM (decisión del dueño: `FEE_RECIPIENT` vía `EM_LIFECYCLE_SIGNER_KEY` u operadores cuyo owner sea el firmante de EM) → `EM_LIFECYCLE_PAYER_SIGNS` on → refunds con firma de receiver/owner → ventana en `log` con `missing = 0` → `enforce` | sección [126] |
| P2 | Writer lease residual en los escritores ERC-8004: `require_writer_lease` se evalúa al entrar el request (`src/handlers.rs:1790`, `:1828`), pero no hay `signing_permit()` por intento como en `settle` (`src/chain/evm.rs`, antes de reservar el nonce), y el job asíncrono de `/register` firma después de contestar sin volver a mirar `is_writer` | `grep -n require_writer_lease src/handlers.rs`; `grep -rn 'signing_permit()' src` |
| P2 | Escritores ERC-8004: reserva explícita del nonce + `release_nonce` cuando el nodo rechaza antes del broadcast (lo que `settle` ya hace con `is_pre_broadcast_rejection`); el guard de este PR evita el revert en estimación, no el rechazo en envío | `src/chain/evm.rs` rama `Err` de `send_transaction` en `settle` |
| P2 | `discovery_attestation::attest_uptime` sigue con `send()` pelado sobre el signer compartido: debería pasar por `send_call_estimated` | `src/discovery_attestation.rs:203`, `:205` |
| P2 | EM: `refund_trustless_escrow` reintenta sin tope un único escrow en polygon — 892 órdenes `refundInEscrow` sobre un solo par (payer, receiver) entre 2026-09-08T22:09Z y 2026-09-10T18:32Z, vía `stranded_escrow_sweeper` según el refutador | query de la sección [126] |
| P2 | Escrow en silencio desde 2026-09-11 23:21Z (última orden de ciclo de vida; `settle_escrow` ~0 desde entonces): confirmar con EM si es pausa deliberada | query de la sección [126] |
| P3 | `upto` contesta un activo desconocido como `400 "Upto scheme error: unsupported_asset…"`, no con el veredicto `invalid_asset` | `src/upto/permit2.rs:243-247`, `src/handlers.rs` rama `upto_error` |

## Declaración

Ningún `settle`, `release`, `refundInEscrow` ni escritura ERC-8004 contra producción ni
contra ninguna cadena. Contra producción: `GET /version`, `GET /settle`,
`GET /identity/base-mainnet/1` y un `POST /verify` con activo inventado y firma basura
(sin fondos). CloudWatch solo lectura (facilitador y logs de EM). En los repos de EM y del
SDK solo `git fetch` + lectura de `origin/main`, sin tocar sus working trees. Ningún secreto
leído ni impreso. `terraform/` sin tocar. Ronda 1: un push; ronda 2: un push
(`--force-with-lease`, por el rebase), de la rama `0xultravioleta/x4-escrow-enforce`.

LISTO PARA c0der 59e8d45b — ronda 2: código hasta `59e8d45b` (test discriminante) sobre `1940d3fd` (estimate antes del nonce + `invalid_asset`) y `1cc55158` (`base-mainnet`), rebaseado sobre `c2346897`; este handoff y `docs/CHANGELOG.md` van en el commit siguiente, cuyo SHA (head del push) está en el cuerpo del PR #49.
