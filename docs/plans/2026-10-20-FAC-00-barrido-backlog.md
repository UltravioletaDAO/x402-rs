---
date: 2026-10-20
tags:
  - type/backlog
  - domain/facilitator
  - priority/p0
status: active
---

# FAC-00 — barrido verificado del backlog del facilitador (2026-10-20)

> Medido contra `origin/main` = `235cdcd0` (2.51.0), no contra un árbol local.
> Cada fila cerrada cita su evidencia (`archivo:línea` de ese commit, o el SHA que la
> resolvió). Cada ficha trae el comando que hoy está rojo y mañana verde. Lo que no
> se pudo citar está en «Para investigar», no en las fichas.
> Este documento **no cambia código**.

## 0. Hallazgos sobre el encargo mismo

| # | Lo que decía el encargo | Lo medido |
|---|---|---|
| R1 | «Lo primero que se lee: el `CLAUDE.md` y el `AGENTS.md`» | **No existe `AGENTS.md`** en `origin/main` (`git ls-tree origin/main AGENTS.md` vacío). Manda solo `CLAUDE.md`. |
| R2 | «No cierres issues de GitHub: hay otra sesión cerrándolos» | **El repo tiene los issues deshabilitados** (`gh issue list` → *the repository has disabled issues*). No hay issues que cerrar ni que pisar; el backlog vive solo en `docs/` y en el código. |
| R3 | «Antes del push: `cargo fmt --check` y `cargo clippy --all-targets -- -D warnings`» | **Los dos ya están rojos en `origin/main`, antes de este PR**: `cargo fmt --check` da 45 hunks en 14 archivos (`src/caip2.rs`, `src/chain/evm.rs`, `src/chain/solana.rs`, `src/chain/hedera/*`, `src/facilitator_local.rs`, `src/handlers.rs`, `src/network.rs`, `src/main.rs`, `src/lib.rs`, `src/openapi.rs`, `src/types.rs`, `src/chain/mod.rs`); `cargo clippy --all-targets -- -D warnings` falla con **133 errores** (27 «the borrowed expression implements the required traits», 26 «held across an await point», imports y variables sin usar, uso de una función `deprecated`, …). Ningún gate de CI los corre: `ci.yaml:203-209` solo hace `cargo build` y `cargo test`. Ver fila N4 y ficha F6. |
| R4 | `FAC-01` (índice de owner) «ya despachada» | Confirmado: PR abierto [#123](https://github.com/UltravioletaDAO/x402-rs/pull/123). El arreglo lo pide el propio código en `src/handlers.rs:9860`, `:10498` y `:14041`. No se duplica acá. |
| R5 | `src/discovery.rs:690` dice «deliberate» | Confirmado, textual: `// deliberate: going over the cap is a capacity problem with a known answer`. **Ninguna fila del backlog lo trata como bug**: nada que cerrar. |
| R6 | Los 4.265 × `429` son el rate limiter | **Ninguna fila del backlog los trata como incidente** (`git grep -w 429` en los archivos de backlog solo aparece en `backlog-idempotency-dynamodb.md` y `backlog-rpc-failover.md`, por otros motivos). Nada que cerrar. |
| R7 | polygon-amoy / hyperevm-testnet sin alarma «porque solo existen para mainnets» | Confirmado, y es **deliberado**: `terraform/environments/production/alerts.tf:121-123` («Mainnet only -- a broken testnet RPC is not worth waking anyone»). Va como fila N5 al dueño, no como ficha. |

## 1. Dónde está el backlog hoy

No hay un único archivo. Las fuentes, todas en `origin/main`:

| Fuente | Qué tiene | Última edición |
|---|---|---|
| `docs/TODO.md` | 9 ítems numerados (P0/P1/P2), generado de un brainstorming del 2026-01-19 | 2026-09-16 (`67b45686`, solo agregó el #9) |
| `docs/handoffs/2026-08-31-cierre-de-sesion-y-backlog.md` §«BACKLOG DEL FACILITADOR» | 3 P0 + 5 P1 + 4 P2 de infra | 2026-09-15 |
| `docs/plans/backlog-*.md` (8 archivos) | un ítem por archivo, o una tabla | 2026-07-30 … 2026-09-29 |
| `docs/plans/dx402/04-BACKLOG-MONETIZACION.md` | su propio encabezado (línea 3) dice que se conserva «por el razonamiento, no como estado» | 2026-09-03 |
| `TODO`/`FIXME` en el código | 12 en `src/`, 3 en `crates/x402-compliance` | — |
| Issues de GitHub | **deshabilitados** (R2) | — |

Además hay **31 handoffs** en `docs/handoffs/` con `priority/p0` o `priority/p1` en el
frontmatter (27 `status: active`, 3 `proposed`, 1 `pending-execution`). Son registros de
sesión, no filas de backlog, y **no se cuentan abajo**; su estado `active` está, en
general, vencido (el frontmatter nunca se actualiza al cerrar).

### El conteo real

**Ningún documento declara un total.** Contando las filas P0/P1 abiertas de las fuentes
de arriba:

| Fuente | P0 | P1 |
|---|---|---|
| `docs/TODO.md` (#1, #2, #3 / #4, #5, #6, #9) | 3 | 4 |
| handoff 2026-08-31 (#1-#3 / #4-#8) | 3 | 5 |
| `backlog-batch-settlement-recheck.md` (filas 1 y 2) | — | 2 |
| `backlog-idempotency-dynamodb.md` (FAC-IDEMP-001) | — | 1 |
| `backlog-nonce-ratchet.md` (`priority/p1`) | — | 1 |
| **Total** | **6** | **13** |

**19 filas P0/P1 abiertas** (18 distintas: `backlog-nonce-ratchet.md` y el P1 #5 del
handoff son el mismo trabajo). Los otros `backlog-*.md` no tienen prioridad P0/P1
(`backlog-dx402-middleware-402-test.md` es P2; `fhe-gateway-timeout`, `onchain-backfill`,
`publish-failures` y `rpc-failover` no declaran ninguna) y se barren igual en §3.

**Resultado: se cierran 6 de las 19 (5 distintas) con evidencia, más 3 filas sin
prioridad. Quedan 13 abiertas. Se agregan 7 filas nuevas.**

## 2. Barrido de las filas P0/P1

### 2.1 Cerradas, con evidencia

| Fila | Qué decía | Por qué se cierra |
|---|---|---|
| Handoff 08-31, **P0 #1** | «Avisarle a Execution Market… el handoff `2026-08-20-respuesta-a-execution-market.md` nunca se envió» | Se envió y EM contestó el 2026-08-21: `docs/handoffs/2026-08-25-em-respuesta-del-21-ago-entregada.md:17-30` registra las respuestas de EM a «sus tres preguntas del 20-ago» (timeout a 45s, request ids de los 502, Sui sin tráfico). |
| Handoff 08-31, **P1 #4** | «El guard de los 14 `send()` crudos» | `331d31d4` (2.28.0, 2026-09-13): `send_call_estimated` (`src/chain/evm.rs:221-254`) estima contra `latest` antes de que el filler tome nonce. Lo usan los 5 escritores ERC-8004 (`src/handlers.rs:7119`, `:7141`, `:8353`, `:8368`, `:8995`, `:9010`, `:12509-12529`, `:13014-13016`) y `src/erc8004/retire.rs:201`, `:209`; escrow y PaymentOperator van por `MetaEvmProvider::send_transaction`, que estima antes (`src/chain/evm.rs:1019-1029`). El test fuente que lo sostiene: `src/erc8004/daily_cap.rs:637`. **Queda un `send()` crudo sobre el mismo `PendingNonceManager` fuera de esa lista**: fila nueva N3. |
| Handoff 08-31, **P1 #5** y `backlog-nonce-ratchet.md` | «Que el hueco sane bajo tráfico: `NONCE_TRUST_CHAIN_AFTER` se reinicia con cada asignación» | `16042013` (2026-09-10): `NONCE_TRUST_CHAIN_AFTER_DRIFT` = 300s (`src/chain/evm.rs:3195`) y la rama de `resync_target` que abandona la marca alta cuando la cadena reporta menos **de forma sostenida**, haya o no tráfico (`src/chain/evm.rs:3218-3224`). Tests: `sustained_drift_gives_up_the_high_water_mark` (`:4058`), `drift_below_the_threshold_still_protects_in_flight_transactions` (`:4045`). |
| Handoff 08-31, **P1 #7** | «Token de servicios propios para el rate limiting» | `fa76e745` (2.43.0) + `b206eac5`: `X-UVD-Stack-Key` salta los presupuestos por IP (`src/rate_policy.rs:31`, header en `:564`), digests desde Secrets Manager. |
| `backlog-idempotency-dynamodb.md`, **P1 FAC-IDEMP-001** | «El cache de `Idempotency-Key` se implementa sobre DynamoDB» | Su propio estado ya lo dice: decisión del dueño 2026-09-17 y almacén en producción. Código: `src/idempotency_store.rs:17-28`. Ojo: la fila cita la tabla en `main.tf:759`; hoy está en `terraform/environments/production/main.tf:925` (la línea se corrió, el recurso es el mismo). Lo que se construya encima se mide contra ese documento, que queda como referencia de costo. |

### 2.2 Abiertas, y qué las mantiene vivas

| Fila | Estado medido | Qué la mantiene viva |
|---|---|---|
| Handoff 08-31, **P0 #2** «Celo pierde el 59%» | Los dos mecanismos citados están cerrados: el nonce quemado por `/feedback` (`331d31d4`, ver P1 #4) y `txpool is full` ahora es rechazo pre-broadcast que libera el nonce (`68860bbe`, `src/chain/evm.rs:1424`, `:1519-1521`). **La tasa de hoy no es verificable sin los logs de 7 días de Celo** (esta sesión no toca AWS). | La espera síncrona del recibo con 30s por defecto (`src/chain/evm.rs:1178-1182`): el propio diagnóstico midió esperas de 447 a 12.179s en Celo (`docs/handoffs/2026-08-20-diagnostico-performance-facilitador.md:343`). Ficha F3. |
| Handoff 08-31, **P0 #3** «68 tx minadas mientras devolvíamos error» | **Mitigada, no resuelta**: desde `1cf251e6` (2.14.0) una espera vencida responde `502 settlement_unconfirmed` **con el hash y el `paymentId`** (`src/handlers.rs:6332-6350`), sin `Retry-After`. El cliente ya puede buscar la tx; sigue recibiendo un error por un pago que puede haber salido. | La misma causa: `src/chain/evm.rs:1178-1182`. Ficha F3. Si los SDK del stack tratan bien el 502 no es verificable desde este repo. |
| Handoff 08-31, **P1 #6** «Failover de RPC» | No implementado: ni `FallbackLayer` ni ninguna lista de URLs en `src/` (`git grep FallbackLayer origin/main -- src` vacío). | Deuda **consciente**, decisión del dueño del 2026-08-03 (`docs/plans/backlog-rpc-failover.md`, «Por qué NO se hizo»). Toca el hot path de pagos: decisión del dueño, no ficha automática. |
| Handoff 08-31, **P1 #8** «DX402 fase 1 (streaming)» | `docs/plans/dx402/04-STREAMING-EVIDENCE-HANDOFF.md` sigue en `status: phase-0-done`; el código solo tiene el techo configurable de la fase 0 (`DX402_MAX_BODY_BYTES`). | Diseño escrito, sin dueño ni demanda medida. DX402 está apagado por defecto (`ENABLE_DX402`). |
| `docs/TODO.md` **#9** EIP-6492 | Sigue inalcanzable: `assert_valid_payment` corre antes de `SignedMessage::extract` (`src/chain/evm.rs:1686` → `:1688`, y `:1849` → `:1851`) y rechaza todo lo que no mida 65 bytes (`:2745-2753`). | Decisión de producto pendiente (resucitar o retirar). **FIRMAS**. Ficha F4, al dueño. |
| `docs/TODO.md` **#6** Agent Card | **Parcial**: existe una A2A agent card en `GET /.well-known/agent-card.json` (`src/handlers.rs:373`, `:872`) y el manifiesto `uvd-stack.json` (`src/interop.rs`). No hay skills OASF ni el vínculo bidireccional con un NFT AGID que pedía la fila. | Falta decidir si lo que falta se sigue queriendo. |
| `docs/TODO.md` **#2** AGID payment identity | **Parcial**: la infraestructura ERC-8004 existe (`src/erc8004/`, `/register`, `/reputation`), pero nada registra al facilitador o a gateways como AGID con reputación por settles (`git grep -i agid origin/main -- src` vacío). | Producto, sin dueño. |
| `docs/TODO.md` **#1** Superfluid, **#3** x402cloud, **#4** Voice2Earn, **#5** IRC bounty bot | Nada en el código: `git grep -i superfluid` vacío; `crates/` solo tiene `x402-axum`, `x402-reqwest`, `x402-compliance`; `examples/` no tiene bot de IRC; ninguna referencia a Voice2Earn/v2e en `src/`. | Nada más que el propio documento: son ideas de un brainstorming del 2026-01-19 marcadas «P0 — This Week» hace nueve meses. **No se cierran porque no están resueltas; se le pregunta al dueño si se retiran o se bajan a P2** (ver Preguntas). |
| `backlog-batch-settlement-recheck.md` **P1 fila 1** «¿`docs/plans/batch-settlement/` entra a git?» | El directorio **no está** en `origin/main` (`git ls-tree -r origin/main docs/plans/batch-settlement/` vacío). | Decisión del dueño sobre archivos que viven solo en su máquina: **no verificable sin acceso a esa máquina**. |
| `backlog-batch-settlement-recheck.md` **P1 fila 2** «Fila propia en `docs/dev-tools/facilitators.md` de `x402-foundation/x402`» | Medido el 2026-10-20 con `gh api repos/x402-foundation/x402/contents/docs/dev-tools/facilitators.md`: 15 facilitadores listados, **ninguno es Ultravioleta**. | Acción externa (PR a otro repo): del dueño. |

## 3. Filas sin prioridad P0/P1 y `TODO` del código

### 3.1 Cerradas (vencidas)

| Fila | Por qué se cierra |
|---|---|
| `docs/plans/backlog-publish-failures.md` «no implementado, no existe palanca» | **Está implementado desde `8c8da633` (2026-07-30)**: `X402_EVENTS_PUBLISH_FAILURES` (`src/events.rs:36`, lectura en `:206-215`, default off), usado en `src/handlers.rs:1517` y probado en `:23484-23540`. El documento quedó escrito antes que el commit del mismo día. |
| `src/network.rs:1465` `TODO(UNVERIFIED)`: emisor RLUSD de XRPL testnet `rQhWct2fv4Vc4KRjRgMrxa8xPN9Zx9iLKV` | Verificado en vivo el 2026-10-20: `gateway_balances` contra `https://s.altnet.rippletest.net:51234/` (ledger validado 21.685.806) devuelve obligaciones en la moneda `524C5553440000…` (= RLUSD) por 1.254.511.280,91, y `account_info` da `Domain` = `https://ripple.com/`. El comentario puede quitarse en el próximo PR de código. |
| `src/network.rs:1622` `TODO`: dominio EIP-712 de USDC en Scroll | Verificado en vivo el 2026-10-20: `eth_call` a `0x06eFdBFf2a14a7c8E15944D1F4A48F9F95F663A4` en `https://rpc.scroll.io` devuelve `name()` = `"USD Coin"` y `version()` = `"2"`, lo mismo que declara `src/network.rs:1631-1634`. Igual: comentario a quitar en un PR de código. |

### 3.2 Siguen abiertas

| Fila | Estado |
|---|---|
| `backlog-rpc-failover.md` | Ver P1 #6 arriba. |
| `backlog-fhe-gateway-timeout.md` | Inerte mientras `ENABLE_ZAMA` siga apagado (decisión 171, `CLAUDE.md`). El disparador que el documento fija (medir p99 de FHE contra mainnet) no ocurrió. |
| `backlog-onchain-backfill.md` | Bloqueado por una `ETHERSCAN_API_KEY` paga. Decisión de gasto, del dueño. |
| `backlog-dx402-middleware-402-test.md` (P2) | Sigue: `crates/x402-reqwest/Cargo.toml` no tiene `wiremock`. Agregar la dev-dependency es exactamente lo que la fila pide decidir. |
| `src/facilitator_local.rs:630` FAIL-OPEN de Solana | **Bug vivo**, no un pendiente cosmético: fila nueva N1, ficha F1. |
| `src/facilitator_local.rs:657`, `:674`, `:690`, `:698`, `:711-713` sin screening en NEAR/Stellar/Algorand/Sui/XRPL | Fila nueva N2. |
| `src/chain/algorand.rs:683` lease opcional, `:857` simulación salteada | «Para investigar» (§5). |
| `src/chain/solana.rs:775` ALT sin resolver, `:1007` delta de saldo salteado | «Para investigar» (§5). |
| `crates/x402-compliance/src/checker.rs:178`, `:283`, `:379` (listas UN/UK/EU, nombre de entidad, recarga) | «Phase 2», P2, sin disparador. |

## 4. Filas nuevas (no estaban en el backlog)

| # | Qué | Evidencia | Va a |
|---|---|---|---|
| N1 | **El screening OFAC de Solana mira las cuentas equivocadas, y deja pasar todo pago que no logra parsear.** | `crates/x402-compliance/src/extractors/solana.rs:30` deserializa como `solana_sdk::transaction::Transaction` **legacy**, cuando el facilitador recibe `VersionedTransaction` (`src/chain/solana.rs:1255`). Toma como «payer» `account_keys[0]` (`extractors/solana.rs:39-42`), que en x402 SVM es el fee payer = **la wallet del facilitador** (`src/chain/solana.rs:592`), y como «payee» `account_keys[1]` (`:51-54`), que no es el destino del `TransferChecked`. Si el parseo falla, `src/facilitator_local.rs:629-639` **permite** el pago («FAIL-OPEN until blacklist is properly configured»). La lista **sí** tiene direcciones Solana: 4 de 1.051 en `config/ofac_addresses.json` (2026-09-29). | Ficha F1 (dueño) |
| N2 | NEAR, Stellar, Algorand, Sui y XRPL pasan sin screening OFAC. | `src/facilitator_local.rs:657`, `:674`, `:690`, `:698`, `:711-713`. La lista actual tiene 0 direcciones de NEAR/Stellar/Algorand/Sui y **1** de XRPL (`ripple`), así que hoy el agujero real es XRPL. | Ficha F2 (dueño) |
| N3 | Un `send()` crudo sobre el `PendingNonceManager` compartido con `/settle`, fuera del guard de `331d31d4`. | `src/discovery_attestation.rs:203` y `:205` (`call.gas_price(gp).send()` / `call.send()` sobre `provider.inner()`, el mismo `EvmProvider`, `:186`). **Dormido**: solo corre con `ENABLE_BAZAAR_ATTESTATIONS=true` (`src/discovery_attestation.rs:54-56`, default `false`), y ningún archivo de `terraform/` lo setea. | Ficha F5 |
| N4 | `cargo fmt --check` y `cargo clippy --all-targets -- -D warnings` rojos en `main`, y ningún gate los corre. | Ver R3. `.github/workflows/ci.yaml:203-209`. | Ficha F6 (formato); el gate en CI es `.github/workflows/`: dueño |
| N5 | polygon-amoy (`signer_gas_critical`) y hyperevm-testnet (`rpc_unreachable`) caídas sin alarma. | Deliberado: `terraform/environments/production/alerts.tf:121-123`. Terraform: **no es ficha automática**. Pregunta al dueño: ¿una alarma de testnet de baja severidad (sin page), o se acepta? | Dueño |
| N6 | Un registro duplicado en loop deja un WARN por intento (588 en 24 h, un solo cliente). | `src/handlers.rs:3108` registra a `warn!` todo `Err` de `/discovery/register`, incluido `DiscoveryError::AlreadyExists`, que ya se contesta `409` (`:3120`). Es un error del cliente, no nuestro. | Ficha F7 |
| N7 | Un 402 en vivo que agrega una opción en una red que el listing no declara se registra pero no se pone en cuarentena (3.145 WARN en 24 h). | `src/discovery_health.rs:3573-3584` (`log_extra_networks`: «recorded, not quarantined»), comparado con `:3381-3385`, donde un destinatario no declarado **sí** se pone en cuarentena. Los 1.697 WARN de header ≠ cuerpo (`:3744-3748`, «keeping both») son listados sucios aguas arriba y **no** llevan ficha. | Ficha F8 (decisión del dueño primero) |

## 5. Para investigar (sospechas: sin `archivo:línea` que pruebe la causa, o sin la medición)

- **`src/chain/solana.rs:1007`**: el path 2 (smart wallets por CPI) **omite la verificación del delta de saldo** del ATA de `payTo` cuando la simulación no lo devuelve, y deja pasar con un WARN. **MONTOS.** Hace falta contar en los logs de producción cuántas veces aparece «simulation did not return post-balance for pay_to ATA» antes de decidir si hay que endurecerlo.
- **`src/chain/algorand.rs:683`**: un pago sin `lease` se acepta y la protección de replay queda solo en el seguimiento de `group_id`. **NONCES/REPLAY.** Hace falta medir qué clientes mandan `lease` hoy.
- **`src/chain/algorand.rs:857`**: la simulación previa al broadcast está apagada por un problema de encoding msgpack. Sin medición de cuántos broadcasts fallan por algo que la simulación habría atajado.
- **`src/chain/solana.rs:775`**: transacciones v0 con Address Lookup Tables se rechazan aunque sean legítimas (falla del lado seguro). Sin datos de cuántas.
- Si los SDK del stack tratan el `502 settlement_unconfirmed` (P0 #3) como «buscá este hash» o como «falló»: no verificable desde este repo.
- Frecuencia real del FAIL-OPEN de N1: el log «ALLOWING transaction temporarily» no estaba en el resumen de 24 h que trajo el encargo. **El defecto de qué cuentas se miran (N1) es independiente de esa frecuencia** y está probado por el código.

## 6. Fichas propuestas, ordenadas por lo que más duele hoy

> Comandos con los features que usa CI (`ci.yaml:119`):
> `F=solana,near,stellar,algorand,sui,xrpl,hedera`.

### F1 — El screening OFAC de Solana mira al facilitador en vez de al pagador (P0, M)

1. **Síntoma:** un pago Solana cuyo pagador o destinatario está en la lista OFAC no se bloquea: se screenea la wallet del facilitador y una cuenta arbitraria, y si la transacción es v0 directamente no se screenea nada.
2. **Causa:** `crates/x402-compliance/src/extractors/solana.rs:30` (deserializa legacy), `:39-42` (payer = `account_keys[0]` = fee payer, que es el facilitador según `src/chain/solana.rs:592`), `:51-54` (payee = `account_keys[1]`); `src/facilitator_local.rs:629-639` (fail-open).
3. **Qué se cambia:** `crates/x402-compliance/src/extractors/solana.rs` (deserializar `VersionedTransaction` y sacar authority + destino del `TransferChecked`, como ya hace `src/chain/solana.rs`) y `src/facilitator_local.rs` (fail-closed; con qué código de error es parte de la decisión).
4. **Comando que prueba el cierre:** escribir en `crates/x402-compliance/src/extractors/solana.rs` el test `extracts_the_authority_and_destination_of_a_v0_transfer_checked` (fee payer F ≠ authority A, mensaje v0, un `TransferChecked` a D; afirma `(A, D)`), y en `src/facilitator_local.rs` `an_unparseable_solana_payload_is_refused_not_allowed`. Hoy el primero da `Err` o `(F, …)`, y el segundo da `Ok`.
   `cargo test --locked -p x402-compliance extractors::solana -- --test-threads=1` y
   `cargo test --locked -p x402-rs --features $F an_unparseable_solana_payload_is_refused -- --test-threads=1`
5. **Riesgo: TOCA EL CAMINO DE `/verify` Y `/SETTLE`** (el gate de compliance corre antes de liquidar). Un fail-closed mal hecho rechaza todos los pagos Solana. **No se despacha a una sesión automática: va al dueño.**
6. **Tamaño M · Prioridad P0.**

### F2 — XRPL (y las otras familias no-EVM) sin screening OFAC (P1, M)

1. **Síntoma:** un pago XRPL desde la dirección `ripple` de la lista OFAC se liquida.
2. **Causa:** `src/facilitator_local.rs:711-713` (XRPL), `:657` (NEAR), `:674` (Stellar), `:690` (Algorand), `:698` (Sui): `Ok(())` sin mirar nada.
3. **Qué se cambia:** `src/facilitator_local.rs` y un extractor XRPL en `crates/x402-compliance/src/extractors/`. Las otras cuatro familias pueden esperar a que la lista tenga direcciones suyas (hoy 0).
4. **Comando:** escribir `an_ofac_listed_xrpl_payer_is_refused`; hoy pasa el pago.
   `cargo test --locked -p x402-rs --features $F an_ofac_listed_xrpl_payer_is_refused -- --test-threads=1`
5. **Riesgo: CAMINO DE `/SETTLE`.** Al dueño.
6. **Tamaño M · Prioridad P1.**

### F3 — `/settle` espera el recibo de forma síncrona: en Celo y Polygon, error por un pago que sale (P1, L)

1. **Síntoma:** el cliente recibe `502 settlement_unconfirmed` (antes, un error) por transacciones que se minan minutos u horas después.
2. **Causa:** `src/chain/evm.rs:1178-1182` (30s por defecto para todo salvo Ethereum y Base), respuesta en `src/handlers.rs:6332-6350`. Esperas medidas: `docs/handoffs/2026-08-20-diagnostico-performance-facilitador.md:338-344`.
3. **Qué se cambia:** `src/chain/evm.rs`, `src/handlers.rs`, `src/types.rs` (un estado `pending` con hash y una consulta posterior). Cambia el contrato de `/settle` hacia los SDK.
4. **Comando:** escribir `a_receipt_past_the_timeout_answers_pending_with_its_hash` (patrón de `src/chain/evm.rs:4621` con `TX_RECEIPT_TIMEOUT_SECS=1`); hoy da `SettlementUnconfirmed`.
   `cargo test --locked -p x402-rs --features $F a_receipt_past_the_timeout_answers_pending -- --test-threads=1`
5. **Riesgo: CAMINO DE `/SETTLE`, y contrato público.** Al dueño, con los dueños de los SDK.
6. **Tamaño L · Prioridad P1.**

### F4 — EIP-6492: resucitar o retirar (P1, S si se retira / M si se resucita)

1. **Síntoma:** un payer con una wallet contrafactual recibe `invalid_signature_length` en todas las redes.
2. **Causa:** `src/chain/evm.rs:2745-2753` (regla de 65 bytes) corre en `assert_valid_payment` antes de `SignedMessage::extract` (`:1686` → `:1688`, `:1849` → `:1851`); la rama `StructuredSignature::EIP6492` (`:1692`, `:1854`) no se alcanza.
3. **Qué se cambia:** `src/chain/evm.rs` (mover la regla al brazo `EIP1271`, o borrar la rama) y la documentación pública que la anuncia.
4. **Comando:** si se resucita, escribir `a_6492_envelope_reaches_the_validator_on_a_chain_with_one` (hoy rojo: `invalid_signature_length`); si se retira, `the_6492_branch_is_gone` debería fallar mientras `StructuredSignature::EIP6492` exista.
   `cargo test --locked -p x402-rs --features $F 6492 -- --test-threads=1`
5. **Riesgo: FIRMAS.** Al dueño. Es la decisión de producto que ya pide `docs/TODO.md` #9.
6. **Tamaño S/M · Prioridad P1.**

### F5 — El `send()` crudo de las atestaciones del Bazaar (P2, S)

1. **Síntoma:** el día que alguien encienda `ENABLE_BAZAAR_ATTESTATIONS`, un `giveFeedback` que revierte en estimación quema un nonce del signer que comparte `/settle`, y los settles siguientes de esa red quedan en cola detrás del hueco.
2. **Causa:** `src/discovery_attestation.rs:203` y `:205`.
3. **Qué se cambia:** `src/discovery_attestation.rs` (pasar por `crate::chain::evm::send_call_estimated`, como `src/erc8004/retire.rs:201`).
4. **Comando:** escribir un test fuente al estilo de `src/erc8004/daily_cap.rs:637`, `no_raw_send_on_the_shared_nonce_manager`, que recorra `src/` y falle ante un `.send().await` de un `CallBuilder` fuera de `send_call_estimated`; hoy rojo por esas dos líneas.
   `cargo test --locked -p x402-rs --features $F no_raw_send_on_the_shared_nonce_manager -- --test-threads=1`
5. **Riesgo:** toca la asignación de **NONCES** del signer EVM, aunque en un camino apagado. Bajo en la práctica, pero por la regla del encargo **va al dueño** para que confirme el despacho.
6. **Tamaño S · Prioridad P2** (dormido).

### F6 — `cargo fmt` en `main` (P2, S)

1. **Síntoma:** `cargo fmt --check` falla en `main` limpio, así que todo PR hereda un rojo que no causó.
2. **Causa:** 45 hunks en los 14 archivos listados en R3 (p. ej. `src/caip2.rs:126`, `src/chain/evm.rs:48`, `src/handlers.rs:118`). Con el toolchain `stable` sin fijar (`rust-toolchain.toml`) no está claro si son de un commit sin formatear o de un rustfmt más nuevo (aquí: rustfmt 1.9.0, cargo 1.97.1).
3. **Qué se cambia:** solo formato, `cargo fmt --all`. Los 133 errores de clippy son otra ficha (M) y no se mezclan.
4. **Comando:** `cargo fmt --check` (hoy exit 1, mañana 0) y `cargo test --locked -p x402-rs --features $F -- --test-threads=1` igual de verde que antes.
5. **Riesgo:** es mecánico, pero **TOCA TEXTUALMENTE `src/chain/evm.rs` Y `src/facilitator_local.rs` (FIRMAS Y `/SETTLE`)**. El dueño confirma que un diff solo de whitespace en esos archivos se puede revisar como tal. Agregar el gate a CI es `.github/workflows/`: del dueño.
6. **Tamaño S · Prioridad P2.**

### F7 — Un `409` de registro duplicado no es un WARN nuestro (P2, S)

1. **Síntoma:** un cliente que reintenta un alta duplicada llena los logs de WARN (588 en 24 h) y tapa los WARN que sí importan.
2. **Causa:** `src/handlers.rs:3108` registra a `warn!` todo `Err`, incluido `DiscoveryError::AlreadyExists` (`src/discovery.rs:76`), que ya se contesta `409` (`src/handlers.rs:3120`).
3. **Qué se cambia:** `src/handlers.rs` (`AlreadyExists` a `debug!`/`info!`; el resto sigue en `warn!`).
4. **Comando:** escribir `a_duplicate_registration_is_not_logged_at_warn`, con la captura de `tracing` de `src/discovery_health.rs:9628-9640`; hoy rojo.
   `cargo test --locked -p x402-rs --features $F a_duplicate_registration_is_not_logged_at_warn -- --test-threads=1`
5. **Riesgo:** bajo. No toca firmas, nonces, montos ni `/settle`. **Despachable.**
6. **Tamaño S · Prioridad P2.**

### F8 — ¿Poner en cuarentena un 402 que agrega una red no declarada? (P2, M; decisión primero)

1. **Síntoma:** un listing del Bazaar cuyo 402 en vivo cobra también en una red que no declaró sigue expuesto; queda solo un WARN (3.145 en 24 h).
2. **Causa:** `src/discovery_health.rs:3573-3584` («recorded, not quarantined»). En cambio, un destinatario no declarado en una red declarada sí se pone en cuarentena (`:3381-3385`).
3. **Qué se cambia:** `src/discovery_health.rs` (la clasificación `Recipients::ExtraNetworks` pasa a retener, quizás detrás de un switch en `src/discovery_config.rs`, como `DISCOVERY_REQUIRE_ORIGIN_PROOF`).
4. **Comando:** escribir `an_extra_network_holds_the_listing_when_the_switch_is_on`; hoy no existe el switch. Los tests de hoy que hay que ajustar a conciencia: `an_extra_network_is_verified_and_a_swapped_recipient_stays_held` (`:5683`) y `the_extra_option_is_logged_at_warn_with_its_network_and_recipient` (`:9628`).
   `cargo test --locked -p x402-rs --features $F extra_network -- --test-threads=1`
5. **Riesgo:** no toca dinero; sí puede **vaciar el catálogo** si 3.145 eventos son unos pocos cientos de listings legítimos que cobran en más redes de las que declararon. Hay que contar los listings distintos detrás de los 3.145 antes de decidir. **Decisión del dueño.**
6. **Tamaño M · Prioridad P2.**

## 7. Preguntas para el dueño

1. `docs/TODO.md` #1, #3, #4 y #5 (Superfluid, x402cloud, Voice2Earn, bot de IRC): «P0/P1 — This Week» desde el 2026-01-19, sin una línea de código. ¿Se retiran o se bajan a P2?
2. F1: ¿el screening de Solana pasa a fail-closed y con qué código de error? ¿Se despacha ya, o espera a medir cuántos pagos v0 entran hoy por el fail-open?
3. N5: ¿alarmas de testnet de baja severidad (sin page) para polygon-amoy y hyperevm-testnet, o se acepta que una testnet caída se vea solo en `/health/ready`?
4. N4/F6: ¿se fija la versión del toolchain y se agrega `fmt`/`clippy` a CI (`.github/workflows/`, fuera del alcance de una sesión automática)?
5. `backlog-batch-settlement-recheck.md` fila 1: el directorio sigue fuera de git: ¿entra?
