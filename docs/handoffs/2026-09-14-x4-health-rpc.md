# Handoff 2026-09-14 -- settles con `upstream_rpc_unavailable` y un `/health` que no podia ponerse en rojo (2.29.5)

## Resumen

Durante horas del 2026-09-14 los settles de un consumidor en **Base mainnet** fallaron con
`502 upstream_rpc_unavailable` mientras `GET /health` contestaba `200 {"status":"healthy"}`.

- **No era el RPC.** El nodo contesto `-32000 gas required exceeds allowance (27979)`: al estimar,
  limita el gas a `saldo / maxFeePerGas`, y el firmante EVM mainnet ya no podia reservar el gas de un
  settle. 27979 es exactamente su saldo sobre un fee cap de 1.005 gwei.
- **El clasificador no conocia esa frase.** `failure.rs` reconocia el firmante sin gas solo por
  `insufficient funds` (la frase del txpool al broadcast). Esta cayo por su codigo `-32000` en
  `is_transport`.
- **`/health` es una constante**, por diseno (es el check del ALB), y no habia otra superficie.

Este PR arregla la clasificacion y agrega `GET /health/ready`. La causa de que el firmante se
vaciara (el piso de tip) la arregla #57 (2.29.4), que entra antes; esta rama va rebasada encima.

## Por que se vacio el firmante (arreglado en #57)

Desde e57c5b18 (en main con 2.19.0, 2026-09-10 16:15Z) `eip1559_fee_floor` ponia
`min_priority: 1 gwei` en la rama por defecto, que cubre Base y las demas L2 sin piso medido. Con un
baseFee de 0.005 gwei, cada settle en Base pago `effectiveGasPrice` 1.005 gwei: medido en dos
settles del dia, 0.0001038 ETH por settle, contra ~0.0000009 ETH por tx antes del cambio.

| ventana (UTC) | tx | ETH por tx |
|---|---|---|
| 09-08 00Z -> 09-09 00Z | 120 | 0.00000092 |
| 09-10 00Z -> 09-10 16Z | 158 | 0.00000062 |
| 09-10 16Z -> 09-11 00Z (entra e57c5b18) | 77 | 0.00006429 |
| 09-14 00Z -> 09-14 12Z | 43 | 0.00009754 |

Las escrituras ERC-8004 del mismo firmante siguen a precio normal (~0.0000008 ETH): ese camino no
pasa por el piso.

La alarma `facilitator-chain-balance-low-base-mainnet` estaba en ALARM desde 2026-09-13 15:56Z. Su
piso de 0.005 ETH asume ~100 settles; con el tip de 1 gwei eran ~38 reservas.

## Por que `is_pre_broadcast_rejection` ya conocia la frase y aun asi salio como RPC caido

`chain/evm.rs` ya matcheaba `gas required exceeds` (test en `evm.rs:3802`), pero ese predicado solo
decide si el nonce reservado se devuelve. La respuesta al cliente la decide
`ChainFailure::classify` en `failure.rs`, que es otra lista. El nonce se devolvio bien; el cliente
recibio "RPC caido". Un test en `failure.rs` fija ahora las dos mitades juntas.

## Que cambia

1. **`failure.rs`**: `gas required exceeds allowance (N)` con N < 1M es `SignerUnfunded` ->
   `503 facilitator_signer_unfunded`, `Retry-After` ~300 s. Con N >= 1M es el gas cap del nodo o el
   limite del bloque, no el saldo, y conserva la clasificacion anterior. Nada que firme el
   facilitador pide 1M (un settle usa ~103k), y el limite de bloque mas bajo medido en las cadenas
   servidas es 3M (hyperevm; sei 12.5M, scroll 20M, el resto 30M o mas).
2. **Red en el log**: `post_verify` y `post_settle` registran `network` en su span, y
   `log_chain_failure` deja de escribir `network="unknown"`.
3. **`GET /health/ready`** (`src/readiness.rs`), con las cuatro trampas del encargo:
   - *No amplifica*:
     - cache con TTL (`HEALTH_READY_TTL_SECS`, 60);
     - el refresh corre en su propia tarea y escribe la cache ella misma. Los callers lo esperan pero
       no son duenos: uno que corta no lo cancela, y el siguiente se une o lee la cache;
     - la ruta lleva el mismo `GovernorLayer` por IP que las demas lecturas on-chain;
     - la respuesta dice `checkedAtUnix`, `ageSecs` y `probeTimeoutMs`.
   - *No filtra*: ni URL, ni key, ni direccion, ni saldo literal. Test que busca las cuatro en el
     cuerpo, en los dos estados. `settlesRemaining * 130000 * feeCap` acota el saldo con resolucion
     de un settle; ese saldo es on-chain y publico.
   - *Vivo vs listo*: `/health` no cambia y sigue siendo el check del ALB (`path = "/health"` exacto
     en terraform). `/health/ready` no debe apuntarse desde un balanceador.
   - *Se pone en rojo*:
     - 503 cuando una mainnet no puede settlear: el RPC no responde en
       `HEALTH_READY_PROBE_TIMEOUT_MS` (5000), o un firmante admite menos de
       `HEALTH_READY_MIN_SETTLES` settles (1-100000, default 10; el 0 que apagaba el rojo en
       silencio se rechaza);
     - `degraded` bajo `HEALTH_READY_WARN_SETTLES` (100).
   - Por firmante: `gasOk` (booleano contra el umbral) y `settlesRemaining` =
     `saldo / (130000 gas * fee cap)`. El fee cap sale del mismo codigo del send path
     (`EvmProvider::quote_fee_cap`), asi que el piso de tip de #57 se refleja solo.
   - `?network=base` o `?network=eip155:8453` acota estado y codigo a esa cadena. Las familias no
     EVM salen en `unchecked`, nunca como verdes.

## Ronda 2 (refutacion independiente de #56 y #57)

La refutacion dio MERGEABLE_CON_CAMBIOS. Esta rama entra despues de #57 (2.29.4), rebasada encima,
con VERSION 2.29.5.

| hallazgo | que era | arreglo |
|---|---|---|
| H56-2 (P1) | El probe corria dentro del future del request, con el lock tomado. Un cliente que cortaba a mitad lo cancelaba y el siguiente lo relanzaba: 10 requests abortados = 10 rondas de RPC. La ruta no tenia governor. | El refresh corre en su propia tarea (`tokio::spawn`), que escribe la cache ella misma. Los callers esperan un `watch` y no son duenos del probe: cortar no lo cancela, y el siguiente se une o lee la cache. Si la tarea muere sin respuesta, el canal se olvida y el caller recibe 503 `probe_failed`. `/health/ready` lleva el mismo `GovernorLayer` que `secondary_read_routes`. Tests portados del refutador: `concurrent_callers_share_one_probe` y `callers_that_hang_up_mid_probe_share_one_probe`. |
| H56-1 (P1) | `it_turns_red...` esperaba `settlesRemaining: 380`, valido solo con el piso de tip de 1 gwei; con #57 da 34965. | El mock cotiza un tip de 1 gwei (`0x3b9aca00`), por encima de todo piso de la tabla: el cap esperado ya no depende de la tabla. |
| H56-3 (P2) | Techo de allowance de 10M justificado con "todo limite de bloque es >= 30M", que es falso: hyperevm 3M, sei 12.5M, scroll 20M. | Techo de 1_000_000 y doc con las cifras medidas. El test del gas cap agrega 3M, 12.5M y 20M. |
| H56-4 (P2) | `HEALTH_READY_MIN_SETTLES` aceptaba 0 (apaga el rojo por gas en silencio) y el timeout del probe no se publicaba. | Rango 1-100000, con test que rechaza el 0. `probeTimeoutMs` en el cuerpo y en openapi. |
| H56-5 (info) | `settlesRemaining * 130000 * feeCap` reconstruye el saldo con resolucion de un settle. | Sin cambio: el saldo es on-chain y publico. La frase "no balance" del handoff queda en "sin saldo literal". |

## Prueba con el binario real contra anvil haciendose pasar por Base

anvil con chain id 8453 y baseFee 0.005 gwei, llave desechable, AWS neutralizado (credenciales
falsas y endpoint inalcanzable), `HEALTH_READY_TTL_SECS=5`. Binario de esta rama rebasada sobre #57.
Las consultas mandan `X-Forwarded-For`, como el ALB en produccion.

```
=== 0. GOVERNOR: sin cabecera de IP del cliente la ruta responde como toda lectura con governor
$ curl -s localhost:18080/health/ready   (sin X-Forwarded-For)
{"code":"rate_limit_key_unavailable","error":"Unable To Extract Key!","hint":"The rate limiter could not identify the caller: no X-Forwarded-For, X-Real-IP or Forwarded header reached it. Behind the production load balancer one is always present; a direct connection to the service has to set it."} HTTP 500
$ curl -s localhost:18080/blacklist      (sin X-Forwarded-For, ruta de secondary_read_routes)
HTTP 500

=== 1. VERDE: RPC respondiendo, firmante con 0.05 ETH
$ curl -s -H 'X-Forwarded-For: 203.0.113.7' localhost:18080/health
{"status":"healthy"} HTTP 200

$ curl -s -H 'X-Forwarded-For: 203.0.113.7' localhost:18080/health/ready
{"ageSecs":0,"checkedAtUnix":1789422038,"networks":[{"mainnet":true,"network":"base","rpc":"ok","signers":[{"gasOk":true,"index":0,"settlesRemaining":380,"status":"ok"}],"status":"ok"}],"probeTimeoutMs":5000,"status":"ok","summary":{"degraded":0,"down":0,"ok":1},"thresholds":{"minSettles":10,"settleGasBudget":130000,"warnSettles":100},"ttlSecs":5,"unchecked":[]} HTTP 200

=== tip que cotiza anvil: 0x3b9aca00 (1 gwei)
=== 2a. firmante con 28119576771839 wei (el saldo del firmante de Base el 2026-09-14); su color depende del fee cap
$ curl -s -H 'X-Forwarded-For: 203.0.113.7' localhost:18080/health/ready
{"ageSecs":0,"checkedAtUnix":1789422044,"networks":[{"mainnet":true,"network":"base","reason":"signer_gas_critical","rpc":"ok","signers":[{"gasOk":false,"index":0,"settlesRemaining":0,"status":"down"}],"status":"down"}],"probeTimeoutMs":5000,"status":"down","summary":{"degraded":0,"down":1,"ok":0},"thresholds":{"minSettles":10,"settleGasBudget":130000,"warnSettles":100},"ttlSecs":5,"unchecked":[]} HTTP 503

=== 2b. ROJO: firmante con 0.000001 ETH (0 settles con cualquier cap de la tabla)
$ curl -s -H 'X-Forwarded-For: 203.0.113.7' localhost:18080/health/ready
{"ageSecs":0,"checkedAtUnix":1789422050,"networks":[{"mainnet":true,"network":"base","reason":"signer_gas_critical","rpc":"ok","signers":[{"gasOk":false,"index":0,"settlesRemaining":0,"status":"down"}],"status":"down"}],"probeTimeoutMs":5000,"status":"down","summary":{"degraded":0,"down":1,"ok":0},"thresholds":{"minSettles":10,"settleGasBudget":130000,"warnSettles":100},"ttlSecs":5,"unchecked":[]} HTTP 503

$ curl -s -H 'X-Forwarded-For: 203.0.113.7' localhost:18080/health/ready?network=eip155:8453
{"ageSecs":0,"checkedAtUnix":1789422050,"networks":[{"mainnet":true,"network":"base","reason":"signer_gas_critical","rpc":"ok","signers":[{"gasOk":false,"index":0,"settlesRemaining":0,"status":"down"}],"status":"down"}],"probeTimeoutMs":5000,"status":"down","summary":{"degraded":0,"down":1,"ok":0},"thresholds":{"minSettles":10,"settleGasBudget":130000,"warnSettles":100},"ttlSecs":5,"unchecked":[]} HTTP 503

=== 2c. GOVERNOR: 150 requests seguidos desde una IP (rafaga de 100)
  46 429
 104 503

=== 3. ROJO: RPC inalcanzable (anvil detenido; el firmante se recargo antes)
$ curl -s -H 'X-Forwarded-For: 203.0.113.7' localhost:18080/health/ready
{"ageSecs":0,"checkedAtUnix":1789422060,"networks":[{"mainnet":true,"network":"base","reason":"rpc_unreachable","rpc":"unreachable","signers":[],"status":"down"}],"probeTimeoutMs":5000,"status":"down","summary":{"degraded":0,"down":1,"ok":0},"thresholds":{"minSettles":10,"settleGasBudget":130000,"warnSettles":100},"ttlSecs":5,"unchecked":[]} HTTP 503

=== /health durante el corte (sin cambios, solo vida)
$ curl -s -H 'X-Forwarded-For: 203.0.113.7' localhost:18080/health
{"status":"healthy"} HTTP 200
```

- **Semaforo**: verde 200 con 380 settles (0.05 ETH / (130000 gas * 1.01 gwei)). Rojo 503
  `signer_gas_critical` con el saldo del incidente y con el drenado. Rojo 503 `rpc_unreachable` con
  el RPC caido. `/health` sigue en 200.
- **`probeTimeoutMs`** sale publicado.
- **Governor**: sin cabecera de IP, `/health/ready` y `/blacklist` responden igual. De 150 requests
  seguidos desde una IP, 46 reciben 429.
- **Escaneo de los cuerpos**: se buscaron `://`, `0x`, la IP local, las IPs de prueba y el saldo en
  decimal y en hex. Ninguno aparece.

## Tests

Nuevos, en `readiness.rs`:
- verde, rojo por gas y rojo por RPC caido contra un JSON-RPC simulado;
- un nodo que acepta y no responde (`rpc_timeout`, acotado por el timeout);
- cuerpo sin URL, direccion ni saldo literal;
- 20 llamadas dentro del TTL sin RPC nuevas;
- 20 callers concurrentes que comparten un probe, y 10 que cortan a mitad sin relanzarlo (portados de
  la refutacion);
- `HEALTH_READY_MIN_SETTLES=0` rechazado;
- `?network=` con 200/503/404/400;
- calificacion pura con los numeros del incidente.

En `failure.rs` y `handlers.rs`: el error exacto de produccion del 2026-09-14 da `SignerUnfunded` y
503 en el brazo `ContractCall`, y un allowance igual al gas cap o a un limite de bloque medido (3M,
12.5M, 20M, 30M, 50M, 150M) no se lee como saldo.

**Fallan sin el cambio (ronda 1).** Con el predicado viejo (`lower.contains("insufficient funds")`
solo):

```
test chain::failure::tests::a_balance_capped_estimate_is_not_an_upstream_outage ... FAILED
  left: "upstream_rpc_unavailable"  right: "upstream_rpc_unavailable"   (assert_ne)
test chain::failure::tests::a_gas_shortfall_is_named_as_one ... FAILED
  left: Transport  right: SignerUnfunded
test chain::failure::tests::an_allowance_at_the_gas_cap_is_not_read_as_our_balance ... FAILED
  left: Unclassified  right: SignerUnfunded
test handlers::chain_failure_response_tests::a_balance_capped_estimate_on_plain_settle_names_the_signer ... FAILED
  left: 502  right: 503
test result: FAILED. 28 passed; 4 failed
```

**Fallan sin el cambio (ronda 2).** Mutantes corridos en local, con recompilacion verificada:

```
# readiness.rs original de la ronda 1 (probe dentro del request) + los dos tests portados
test readiness::tests::callers_that_hang_up_mid_probe_share_one_probe ... FAILED
  10 aborted callers sent 10 RPC calls
test readiness::tests::concurrent_callers_share_one_probe ... ok
test result: FAILED. 1 passed; 1 failed

# techo de allowance de vuelta en 10M
test chain::failure::tests::an_allowance_at_the_gas_cap_is_not_read_as_our_balance ... FAILED
  assertion `left != right` failed: 3000000
  left: SignerUnfunded  right: SignerUnfunded
```

El test de concurrencia pasa tambien con el codigo de la ronda 1: el single-flight con callers
vivos ya funcionaba. Lo que no funcionaba era el caso de cancelacion, y ese test lo fija.

**Suite local de CI**, sobre la rama rebasada encima de `250a969d` (#57), con un target dir propio y
recompilacion forzada y verificada: `cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1`

| target | resultado |
|---|---|
| lib | 1131 ok, 0 fallos, 1 ignorado |
| bin | 1180 ok, 0 fallos, 1 ignorado |
| integracion | 65 ok, 0 fallos, 11 ignorados |

Los ignorados ya lo estaban antes de este cambio. La suite incluye los tests de #57
(`a_zero_or_missing_tip_estimate_still_tips_one_mwei`, `only_ethereum_and_polygon_tip_above_one_mwei`),
asi que la union de los dos PRs esta probada.

Ademas: `cargo fmt --check` ok, `cargo clippy --all-targets` sin hallazgos en el codigo cambiado, las
reglas del check de account-ID limpias sobre los docs, y el hook anti llaves ok.

## Pendiente / backlog

- **Recarga de gas en Base**: decision del dueno. Con #57 desplegado, el saldo actual admite ~19
  settles sin recarga. Se verifica con
  `curl -s https://facilitator.ultravioletadao.xyz/health/ready?network=base`.
- **Alarma sobre `/health/ready`**: por ejemplo un canary que pagine con 503 sostenido. No se agrego
  aca.
- **Familias no EVM** (Solana, NEAR, Stellar, Algorand, Sui, XRPL) en `/health/ready`: hoy
  `unchecked`.
- **CI y VERSION**: `ci.yaml` solo valida que `VERSION` no este vacio. Nada impide que un cambio de
  binario salga con `/version` sin moverse.
- **Piso de la alarma de saldo en Base** (0.005 ETH = "~100 settles"): con el precio de #57 son
  ~3.500 reservas; recalibrarla.
- **Corte del cliente a traves del ALB** (refutacion, hipotesis sin medir): si el ALB no propaga el
  corte a la conexion con la task, el caso de cancelacion solo aplica a clientes internos. Con el
  refresh desacoplado ya no importa para el costo de RPC.

## Para c0der

- **Causa (A)**: el firmante EVM mainnet en Base no podia reservar el gas de un settle. El nodo
  respondio `-32000 gas required exceeds allowance (27979)` en las tres referencias, y 27979 es su
  saldo sobre el fee cap de 1.005 gwei. El clasificador lo convirtio en `upstream_rpc_unavailable`.
  El firmante se vacio por el tip de 1 gwei en L2 que entro con e57c5b18: el costo por tx salto de
  ~0.0000006 a ~0.00006 ETH en la ventana de ese merge. La alarma de saldo bajo de Base estaba en
  ALARM desde el 09-13 15:56Z. Evidencia completa, con los datos del consumidor, en
  `DIAGNOSTICO-c0der.md` (fuera de git).
- **La hipotesis del RPC se cayo.** El RPC estaba sano (la alarma `chain-unreachable` de Base
  seguia en OK) y el error no era de transporte.
- **Semaforo**: salidas arriba, con el binario de esta rama rebasada sobre #57.
- **El consumidor todavia no puede correr sus pagos pendientes** hasta que #57 este desplegado (el
  saldo actual alcanza para ~19 settles) o haya recarga. Confirmarlo con
  `/health/ready?network=base` (`status: ok`).
- **Versiones**: #57 es 2.29.4 y esta rama, 2.29.5.
