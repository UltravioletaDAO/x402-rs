---
date: 2026-08-20
tags:
  - type/handoff
  - domain/performance
  - priority/p0
status: active
---

# Diagnóstico de performance del facilitador — tres episodios en 10 días

> **Método:** 6 agentes en paralelo, read-only, sobre código + AWS en vivo + dos
> handoffs externos (Execution Market y KarmaKadabra).
> **Nada se aplicó.** Todo lo de acá es diagnóstico y fixes propuestos.

## La frase que resume todo

**El facilitador no se cae por exceso de carga. Se traba con casi nada, y una
vez trabado el reintento del cliente le impide curarse.**

216 settles en 4 horas son 0,015 req/s. Con eso alcanzó para 4 horas de
degradación. El problema no es que no escale a miles: es que **no escala a uno**.

## Los tres episodios

| # | Ventana (UTC) | p99 | Tráfico | CPU máx |
|---|---|---|---|---|
| baseline | — | 0,23-0,44s | 900-1200 req/h | 1,6-2,9% |
| 1 | 08-10 17:00→18:35 | logs expirados | — | — |
| 2 | 08-19 21:45 → 08-20 01:53 | 4,0-7,6s (pico 28-29s) | 1800-3800 req/h | — |
| 3 | 08-20 17:20→19:30 | 3,5-7,3s (pico **14,02s**) | 2600-3300 req/h | **25%** |

La CPU nunca pasó del 25%. Memoria 16,7%. Conexiones pico 70/min contra un
límite de decenas de miles. DynamoDB sin throttling. **Nada estaba saturado.**

---

## P0 OPERATIVO — Celo sin gas, ahora mismo

**No es un bug de diseño: es una wallet vacía perdiendo pagos hoy.**

```
celo        0,028436 CELO   ->  0 settles restantes   (un settle de escrow cuesta 0,1134)
ethereum    0,003855 ETH    ->  115 settles
arbitrum    0,002679 ETH    ->  239 settles
polygon     97,64 POL       ->  628 settles
```

Wallet `0x103040545AC5031A11E8C03dd11324C7333a13C7`, confirmada contra 3 RPC
independientes. Consecuencia medida: **440 errores `insufficient funds` el 20-ago
entre 18:00 y 21:45**, y **409 de 973 settlements de escrow fallidos en 24h**.

Las L2 (base, optimism, arbitrum, scroll, unichain, robinhood) están
**sobreestimadas** en esa tabla: el fee de datos L1 no aparece en `eth_gasPrice`.

> **Y no existe ninguna alarma de balance.** La Lambda de `lambda/balances/`
> mide y publica estos números en la landing page — se miden, se muestran, y
> nadie los vigila. Una cadena de producción se secó sin que se enterara nadie.

**Nota a favor del código:** `insufficient funds` **sí** está en
`is_pre_broadcast_rejection` (`evm.rs:821`), así que devuelve el nonce
correctamente. Los 440 errores no contaminaron la secuencia.

---

## P0 OPERATIVO — El RPC de Sui está muerto en producción

`RPC_URL_SUI` (`terraform/environments/production/main.tf:874`) apunta a
`https://fullnode.mainnet.sui.io:443`. **Ese endpoint dejó de hablar JSON-RPC.**
Verificado directamente:

```
$ curl -s -X POST https://fullnode.mainnet.sui.io:443 \
    -d '{"jsonrpc":"2.0","id":1,"method":"sui_getChainIdentifier","params":[]}'
{"error":{"code":-32601,"message":"Method not found. JSON-RPC on public
 fullnodes has been deprecated. Please migrate to gRPC or GraphQL endpoints."}}
```

No es un método viejo: son los seis métodos probados, el endpoint entero.
**Sui mainnet no puede estar funcionando.**

Reemplazo verificado y funcionando: `https://sui-rpc.publicnode.com`
(`sui_getChainIdentifier` → `35834a8a`, `suix_getReferenceGasPrice` → 100).
`RPC_URL_SUI_TESTNET` (`main.tf:878`) es del mismo host family — casi seguro igual,
sin verificar.

> **Mismo patrón que Celo: algo de producción dejó de funcionar y nadie se
> enteró.** Y acá ni siquiera hacía falta una alarma de balance — alcanzaba con
> un health check por cadena llamando un método barato.

**La conclusión se ensancha: no existe ninguna vigilancia de salud por cadena,
ni de fondos ni de conectividad.**

---

## Balances no-EVM (mainnet) — ninguno crítico

| Cadena | Usable | Txs restantes |
|---|---|---|
| **stellar** | 16,888 XLM | **168** ← el más bajo de la familia |
| sui | 20,815 SUI | ~5.200 |
| near | 8,888 NEAR | 7.406 |
| solana | 0,1148 SOL | 22.960 |
| algorand | 124,33 ALGO | 124.326 |
| xrpl | 22,162 XRP | 1.108.099 |
| fogo | 9,619 FOGO | 1.923.798 |

Stellar es el único que vale mirar. El costo de Soroban usado (~0,1 XLM) es una
estimación, no una medición — si las invocaciones reales cuestan más, esas 168
bajan rápido. Los de solana, algorand y xrpl son fees de protocolo y son firmes.

---

## P0 — Monad pierde el 88% de sus transacciones

Verificado contra 2 RPC independientes, ventana 08-19 21:45Z → 08-20 01:53Z:

| Cadena | Emitidas | Existen en la cadena |
|---|---|---|
| **Monad** | 181 | ~22 — **88% se evaporan** |
| **Polygon** | 16 | 16 — limpio |

El nonce del signer de Monad avanzó de 254 a ~276 mientras se emitían 181
transacciones. Todas devolvieron un hash. La cadena no las tiene.

**No es falta de fondos** (monad tiene 81,8 MON, ~1.400 settles) y **no es
`txpool is full`** — cero eventos de txpool en monad; los 7 de la ventana fueron
todos en Celo.

**La hipótesis del piso de gas fue REFUTADA.** Se replicó la fórmula real del
`GasFiller` leyéndola de la fuente (`alloy-provider-1.7.3/src/utils.rs:111-121`):
una sola llamada a `eth_feeHistory(10, "latest", [20.0])`, mediana de los rewards
p20, y `max_fee = base_fee * 2 + priority`. Contra monad, ahora:

| | valor |
|---|---|
| `feeHistory.baseFeePerGas` | 100,0 gwei — **idéntico al header, sin discrepancia** |
| alloy `maxPriorityFee` | 2,0 gwei (lo que sugiere el nodo) |
| **alloy `maxFeePerGas`** | **202,0 gwei** |
| piso de la cadena | 100,0 gwei |

**Alloy paga el doble del piso, no por debajo.** El fix de una línea en
`evm.rs:534` no serviría de nada.

**Lo que abre el primer hueco en monad sigue sin identificarse.** El nonce hoy
está limpio (311/311), el episodio se curó, y desde afuera no queda rastro. Para
cerrarlo hacen falta los logs con `evm=debug` — con `info`, `resyncing nonce
against chain` (`evm.rs:2376`, `trace!`) **no se emite**.

**Datos colaterales del mismo test:** en **Base** alloy calcula un tip 160x por
debajo del sugerido por el nodo (0,00000625 vs 0,001 gwei) — queda sobre el piso,
así que no explica pérdidas, pero es un tip de casi cero. En **polygon** es al
revés: alloy calcula 114,7 gwei de tip contra los 30 del nodo, o sea **paga ~4x
de más**.

---

## CAUSA RAÍZ #1 — El nonce quemado que no sana (P0)

`src/chain/evm.rs:811-825`, `evm.rs:2378-2390`

1. El RPC responde `txpool is full`. La transacción **nunca entró al mempool**.
2. `txpool is full` **no está** en `is_pre_broadcast_rejection`, así que cae en
   `reset_nonce` y **el nonce N no se devuelve**.
3. El siguiente settle resincroniza, ve `pending <= high_water`, entrega **N+1**.
4. **Hueco permanente en N.** Las transacciones N+1, N+2… quedan *queued* y no
   pueden minarse nunca.
5. `send_transaction` **devuelve Ok con un hash**. No hay error visible.
6. El watcher sondea 4 veces en 30s → `TxWatcher(Timeout)` → `reset_nonce` →
   resync → **el hueco se ensancha**.

**El agravante que lo vuelve indefinido:** la única curación es
`NONCE_TRUST_CHAIN_AFTER` = 120s de silencio, pero `last_allocated` se refresca
en **cada** asignación (`evm.rs:2396`), incluidas las envenenadas.

> **Mientras el cliente reintente más seguido que cada 120 segundos, el hueco
> nunca sana.** Execution Market reintentaba hasta 39 veces por submission.
> El reintento es lo que impide la curación.

Y explica lo que ninguna otra hipótesis explicaba: **cada red tiene su propio
`PendingNonceManager`** (`evm.rs:273`, uno por provider). Una cadena trabada no
toca a las otras 20 — eso es exactamente "intermitente y no total".

### Alcance real, medido (corrige la primera versión de este documento)

El mecanismo está **confirmado en el código**, pero **no es la explicación
dominante** de las transacciones fantasma:

- `txpool is full`: **7 eventos, los 7 en Celo.** Cero en monad y polygon. La
  atribución de "monad+polygon = 84% por txpool" era **incorrecta**.
- **El orden temporal no cierra:** en Celo, 12 transacciones ya se habían
  evaporado entre 22:59 y 23:05, **antes** del primer `txpool is full` (23:06:53).
- Saltos de nonce **sí verificados on-chain en Celo**: 398 → 400 → 403 → 412
  (contra 4 RPC). 25 de 29 emisiones de esa hora no existen.
- El signer de Celo **no quedó trabado**: hoy `pending=429 latest=429`.

**Punto ciego que impide medir el resto:** `resyncing nonce against chain` es
`trace!` (`evm.rs:2376`) y `reset nonce cache` / `released unbroadcast nonce` son
`debug!` (`evm.rs:2431`, `:2452`). Producción corre en `info`. **El conteo de 0 en
los logs es un artefacto del nivel, no evidencia de ausencia.** Para observarlo:
`RUST_LOG=info,x402_rs::chain::evm=debug`.

---

## CAUSA RAÍZ #2 — /settle es síncrono con la blockchain (P0 estructural)

`src/chain/evm.rs:663-679`

```rust
let default_timeout = match self.chain.network {
    Network::Ethereum => 900,   // 15 minutos
    Network::Base => 90,
    _ => 30,                     // optimism, polygon, monad, celo...
};
return match watcher.get_receipt().await {   // el cliente espera acá
```

**Todos** los paths esperan confirmación, no solo el escrow — incluido el
EIP-3009 estándar, que es >95% del tráfico. Y las 7 familias de cadenas hacen lo
mismo (Solana 90s, Stellar 30s, Algorand 10s, XRPL 30s, NEAR **sin timeout**).

Coincidencias que esto explica de una sola vez:

- La "latencia clavada en 30,1-30,8s" que midió EM = nuestro default de 30s.
- El timeout de ~30s de Celo que reportó KarmaKadabra.
- Los **201 HTTP 460**: el cliente de EM corta a los 30s
  (`FACILITATOR_TIMEOUT_SECONDS`) — **empata exactamente con nuestro timeout**.

**Capacidad resultante** (C=20 concurrentes, throughput = C/D):

| Cadena | Típico | Peor caso |
|---|---|---|
| Ethereum | 1,7 req/s | **0,02 req/s** (1 cada 45s) |
| Base | 10 req/s | 0,22 req/s |
| Resto EVM | 10-80 req/s | 0,67 req/s |
| **Con fix async** | **10-13 req/s parejo en todas** | — |

**Historial:** el timeout de Ethereum se multiplicó por 30 tratando el síntoma —
30s → 120s → 300s → 600s → 900s, en 4 commits el 20-21 de febrero. La espera de
recibo viene del commit inicial: es herencia de upstream, no algo que rompimos.

**Matiz medido (corrige la primera versión):** el histograma de 831 settles en 7
días tiene la **moda en 7s**, con **13 requests clavadas en 30s** y 1 en 90s;
p99 = 30.117 ms. La pared de timeout existe, pero es **angosta, no una masa**.
El problema de capacidad es real; la latencia típica no está dominada por el
timeout.

**Y el modo de falla más caro que encontramos:** dos transacciones **se minaron
correctamente mientras el cliente recibió `status=400`** —
`0x097890ad379cacbb…` (monad, block 97453656) y `0xebc487425044186f…` (base,
block 50159925). Plata movida, error devuelto.

---

## CAUSA RAÍZ #3 — No hay ningún timeout en el camino del dinero (P0)

| Dónde | Estado |
|---|---|
| Router axum | **sin `TimeoutLayer`** |
| Cliente HTTP → RPC (EVM/alloy) | **sin timeout** (`Client::new()`, reqwest default = None) |
| `src/chain/algorand.rs:366` | `reqwest::Client::new()` **sin timeout** |
| `src/chain/stellar.rs:488` y `:2110` | **sin timeout** |
| `src/chain/xrpl.rs:330` | **sin timeout**, y construye un `Client` nuevo **por request** (tira el pool, handshake TLS cada vez) |

Un RPC que acepta el TCP y no responde deja el request colgado **hasta los 600s
del ALB**. Los 5 round-trips previos al envío no tienen ningún límite.

**Y la ironía:** `discovery_aggregator.rs:590` (60s), `discovery_security.rs:251`
y `fhe_proxy.rs:85` **sí** tienen timeout. **El crawler del bazar, que no mueve
un centavo, está mejor protegido que el camino del dinero.**

---

## CAUSA RAÍZ #4 — 90 segundos sirviendo tráfico sin permiso de escribir (P1, CONFIRMADO)

**Cada deploy abre una ventana de ~90 segundos en la que la task nueva ya recibe
tráfico de producción y rechaza toda escritura EVM.** No son los 15s del TTL ni
los 4s de un handoff: son 6x el TTL.

Cronología real del cutover del 20-ago (task `f0806f92`), correlacionada por
`logStreamName` contra el `Acquired` de esa misma task:

```
18:25:40.933Z  has started 1 tasks (f0806f92)
18:26:09.695Z  registered 1 targets          <- el ALB YA le manda trafico
18:26:37-38Z   3x "does not hold the EVM writer lease"   <- YA sirviendo, YA rechazando
18:26:51.339Z  ECS: "has stopped 1 running tasks (313ac0a5)"
18:27:36.991Z  313ac0a5: "Released EVM writer lease"     <- el release REAL, 45,6s despues
18:27:39.303Z  f0806f92: "Acquired EVM writer lease"     <- recien aca puede escribir
```

**Hueco: 89,6 segundos** entre "el ALB la considera sana" y "tiene el lease".
Mismo patrón en las 5 tasks: **el 100% de los rechazos de cada task ocurre antes
de su propio `Acquired`**, nunca después.

Dos defectos de diseño encadenados:

1. **`/health` no sabe nada del lease.** `handlers.rs:1864-1868` devuelve 200
   incondicional, sin una línea de lógica; el target group (`main.tf:408-416`)
   apunta ahí. El ALB declara sana a una instancia que no puede escribir.
2. **El release del lease espera al drenaje.** El evento ECS "has stopped" es
   cuando ECS manda SIGTERM; el `Released` real llega 45,6s después porque el
   graceful shutdown de axum (`main.rs:634-646`) drena las requests en vuelo
   **antes** de llamar `lease.release()`.

> **Y acá está el acoplamiento: la causa raíz #2 produce la causa raíz #4.**
> Las requests en vuelo que retrasan el release son settles esperando
> confirmación on-chain (30-900s). Cuanto más lento el settle, más tarda el
> release, más larga la ventana sin writer. Un incidente de latencia **alarga
> su propia ventana de deploy**.

Sigue **sin verificar** si en esa ventana ambas tasks firmaron a la vez
(`IS_WRITER` arranca en `true`, `writer_lease.rs:54`, y el lease falla abierto,
`:163-166`). Lo confirmado es el rechazo, no la doble firma.

**Fix — NO gatear `/health` al lease**: las réplicas sin lease deben seguir
sirviendo lecturas, es intencional (`writer_lease.rs:19-20`). Las dos opciones:

- **(b) La de menor riesgo:** mover `lease.release()` **antes** del drenaje de
  axum. El release no depende de que terminen las requests en vuelo, y ataca
  directo los 45s medidos.
- (a) Un `/ready` separado, o que el middleware de escritura espere un tick
  corto (≤5s) al primer `try_acquire()` en vez de rechazar de una.

---

## Hallazgos operativos

**Las alarmas del facilitador le llegan a Execution Market, no a nosotros.**
De 8 alarmas, **5 están mudas** (`AlarmActions: []`); las 3 que avisan apuntan a
`em-production-mcp-alerts`. Ninguna sobre CPU ni sobre HTTP 460.

**El autoescalado mide lo que no importa.** `min=1/max=3` por CPU al 75%. La CPU
nunca pasó de 25%. Un servicio I/O-bound **jamás** va a mover un autoescalador de
CPU. Nunca disparó: corre con **una sola task**.

**El load test nunca probó nada.** `k6_load_test.js:91` firma con bytes
aleatorios; el revert se detecta en `estimate_gas` **antes** de reservar nonce y
transmitir. **Nunca llega a la espera de confirmación.** Y ambos scripts apuntan
por default a **producción** (`k6_load_test.js:33`, `artillery_config.yml:16`).

**Los logs del episodio 1 ya no existen** — retención de 7 días.

**Encendimos Pinata en medio del incidente.** Los commits `6713f76d` (13:54),
`b90431d9` (14:27) y `ceaec661` (14:45) dispararon deploys; los 4 primeros
intentos fallaron con `AccessDeniedException` sobre `facilitator-dx402-pinata`.
No causó el incidente, pero es una coincidencia operacional que conviene no
repetir.

**CI corre sin `terraform.tfvars`** (gitignored, nunca trackeado). El 14 de
agosto eso revirtió el idle_timeout del ALB de 600 a 180. Hoy no hay drift de
capacidad (`desired_count` está protegido por `ignore_changes`), pero el riesgo
sigue abierto.

---

## Lo que NO era (hipótesis refutadas con evidencia)

Se documentan para que nadie las vuelva a perseguir:

| Hipótesis | Por qué se cayó |
|---|---|
| Writer lease como cuello de botella *de throughput* | Es un `AtomicBool`, cero RPC por request. **Pero sí es causa de rechazos** — ver causa raíz #4 |
| Inanición del renovador del lease | 0 eventos `Lost EVM writer lease` en 2 días. La rama `#owner = :me` no tiene chequeo de tiempo: un owner solo no puede vencerse a sí mismo |
| CPU / memoria / conexiones / DynamoDB | 25% / 16,7% / 70 por minuto / cero throttling |
| Logging en `trace` | Efectivo `info`; OTEL está apagado (`enable_observability=false`) |
| Crawler del bazar | Apagado, y además secuencial |
| Aggregator como disparador | Ciclos de 20-24s cada hora; no coincide con ningún inicio de episodio |
| Write-lock de `apply_retention` | No toca `/verify` ni `/settle`; el daño es solo por CPU |
| RPC lentos | Medidos hoy: monad 246-264ms, polygon 101-158ms, celo 150-203ms |
| RPC gratis como causa | **Polygon y celo son premium.** Solo monad es público |
| Arranque en frío caro | ~2s de proceso (20-29s a nivel ECS, casi todo ENI + image pull) |
| El facilitador estuvo caído | `RunningTaskCount` = 1,0 sostenido, minuto a minuto. Nunca llegó a 0 |
| Fuga de API keys al cliente | `scrub_urls()` protege todas las rutas de escrow |
| ALB idle timeout muy bajo | Es 600s, no el default de 60s |

---

## Los fixes, en el orden seguro

**Primero lo operativo, que no toca código y está costando plata hoy:**

| # | Acción | Por qué primero |
|---|---|---|
| **0a** | **Fondear la wallet de Celo.** Ethereum y arbitrum detrás; stellar a vigilar | Está en cero; 409 de 973 settlements fallidos en 24h |
| **0b** | **Cambiar `RPC_URL_SUI`** a `https://sui-rpc.publicnode.com` | El endpoint actual dejó de hablar JSON-RPC: Sui mainnet no funciona |
| **0c** | **Health check por cadena** — un método barato, y alarma si responde error | Celo y Sui se cayeron sin que nadie se enterara. No hay vigilancia de ningún tipo |
| **0d** | **Alarma de balance mínimo por cadena** | El dato ya se mide en `lambda/balances/` y se publica; nadie lo vigila |
| **0e** | **Subir el nivel de log de `evm` a `debug`** | Sin eso, el mecanismo del nonce es inobservable en producción |

**Después el código: `#1 → #4 → #3 → #2 → #5`. El #3 NUNCA antes del #1.**

| # | Fix | Archivo | Riesgo |
|---|---|---|---|
| 1 | Devolver el nonce ante `txpool is full` (`is_mempool_full`, match por mensaje **no** por `-32003` — ese código está sobrecargado) | `evm.rs:811-825` | Bajo |
| 4 | Timeout explícito en reqwest: 10s request / 3s connect, + los 4 sitios de algorand/stellar/xrpl | `evm.rs:263-272` | Bajo |
| 3 | `-32003` retryable → 502 + `Retry-After` en vez de 400 | `handlers.rs:302-313` | Medio — **solo después del #1** |
| 2 | Que el hueco sane bajo tráfico (contador de `pending` congelado, **no** "última confirmada") | `evm.rs:2378-2390` | **Alto — considerar follow-up** |
| 5 | Failover con `FallbackLayer`, **`active_transport_count=1`** | `evm.rs:262-270` | Medio |

**Por qué el #3 nunca va solo:** hoy un `txpool is full` devuelve 400 y el cliente
no reintenta. Si lo hacemos retryable primero, EM empieza a reintentar y **cada
reintento quema otro nonce** — convertimos un wedge ocasional en uno veloz.

**Por qué el #2 es delicado:** la versión literal de "medir desde la última
confirmada" rompe el invariante *nunca entregar un nonce ≤ a uno en vuelo* justo
cuando la cadena está caída. La alternativa es preguntarle a la cadena
(`pending` congelado en N resyncs), no al reloj.

**Trampas al aplicar:**

- `released unbroadcast nonce` (`evm.rs:2429`) es `debug!` y prod corre en `info`
  → **subirlo o el #1 es inverificable**.
- El helper de test `resync_nonce` (`evm.rs:2606`) es una **copia literal** del
  match de producción. Si se toca uno y no el otro, **los tests quedan verdes
  probando el código viejo**.
- `FallbackLayer::default()` consulta **3 transportes en paralelo** y **no**
  incluye `eth_getTransactionCount` en `sequential_methods` → dos nodos con
  vistas distintas del mempool corren una carrera y el resync de nonce se vuelve
  no determinista. `count=1` no es preferencia de carga: es **requisito de
  corrección**.

### Infra (Terraform, propuesto sin aplicar)

1. `min_capacity = 2` — con 1 task no hay nada que absorba.
2. Autoescalado por `ALBRequestCountPerTarget`, no por CPU.
3. `alarm_actions` en las 5 alarmas mudas, con topic SNS **propio**.
4. Alarma temprana de p99 > 2s (la actual es >10s y no vio 2 horas a 5-8s).
5. `access_logs.s3` en el ALB — sin eso el 460 es invisible.
6. Subir la retención de logs (7 días perdió el episodio 1).

---

## Criterio de éxito

| Qué | Umbral |
|---|---|
| `/verify` p99 @ 50 concurrentes | **< 1s** |
| `/verify` tasa de error | **0%** |
| `/settle` hoy | *sin umbral* — su latencia mide la red, no nuestra capacidad |
| `/settle` post-fix, tiempo a aceptar | **< 1s** |
| `/settle` post-fix, jobs completados | **≥ 99%** dentro de 3× el típico **de esa cadena** |
| Errores de writer lease en la corrida | **0** |
| Throughput sostenido de aceptación | **≥ 10 req/s** |

**Ninguna métrica pasiva sirve para declarar esto arreglado.** Si mañana no hay
tráfico de escrow, todo se ve perfecto sin haber cambiado nada — que es
exactamente lo que pasó entre el episodio 1 y el 2. El veredicto lo da un
**canario reproducible corrido a demanda**, no un dashboard.

**Reproducción local del bug de nonce** (gratis, sin fondos reales):
`anvil --fork-url $RPC_URL_BASE_SEPOLIA --chain-id 84532`, dos instancias del
facilitador contra el mismo nodo, `evm_setAutomine(false)` para ensanchar la
ventana, settles concurrentes. Con `ENABLE_WRITER_LEASE=false` se reproduce la
colisión; con `true` la segunda instancia se autoexcluye.

---

## Lo que quedó sin verificar

- **Worker threads de tokio.** `#[tokio::main]` sin argumentos →
  `available_parallelism()`, que en Linux usa `sched_getaffinity()` (máscara de
  afinidad) y **no** la cuota cgroup. En Fargate puede resolver a 1 worker
  (head-of-line blocking) o a N workers con presupuesto de 1 (thrashing). **Son
  patologías distintas con fixes opuestos.** Se resuelve con una línea de log al
  boot o habilitando ECS Exec — ninguna de las dos se hace durante un incidente.
- **Si las dos tasks del rolling deploy firmaron concurrentemente.** Confirmado
  el rechazo de escrituras durante 90s; no confirmada la doble firma.
- **Cuántas private keys tiene el secret de prod** (1 vs N signers): si son
  varias, el impacto del hueco se divide entre ellas.
- **Si el nodo de Monad encola o rechaza las txs con nonce gapped.**
- **La confirmación empírica del nonce gap en logs de producción** — falta cruzar
  hashes de `"Transaction submitted to mempool"` contra la cadena.
