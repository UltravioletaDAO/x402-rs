# Benchmark de capacidad del facilitador

**Fecha:** 2026-09-10. **Base de la rama:** `c5e8c62f`.
**Lo que estaba en produccion mientras se midio:** 2.20.0-b8051d4, task definition
410 y despues 411.
**Alcance:** paso 5 de la secuencia de la auditoria de Astra 6 — medir capacidad
por ruta para elegir la palanca de costo de A6/A7/A9 con numeros.
**Lo que se toco:** nada. AWS se leyo con credenciales de solo lectura; la carga
corrio contra un binario local con un RPC simulado. No hubo trafico contra
`facilitator.ultravioletadao.xyz` ni contra ninguna mainnet.

Los scripts estan en `scripts/bench/` y su README explica como repetir cada
medicion. Todo numero de este informe lleva el comando que lo produjo.

---

## 0. Lo primero: hay una regresion en curso en produccion

Empezo hoy y sigue mientras se escribe esto. No la causo el banco (el banco es
local); se encontro al levantar el perfil de referencia. Ultima lectura:
2026-09-10T18:25Z, con la task definition ya en la revision 411.

| Señal | Antes de hoy 12:45 EDT | 2026-09-10T18:20Z |
|---|---:|---:|
| Memoria por task (min/media/max) | 180 / 210 / 294 MiB | 1539 / 1591 / **1632 MiB** |
| CPU por task (media/max, 1024 = 1 vCPU) | 25 / 68 | 227 / **1024 sostenido 40 min** |
| `p95` del target group de lecturas | 78-207 ms | **2573-4378 ms** |
| `p99` del target group de lecturas | 93-472 ms | **3569-5817 ms** |
| `p50` de `/discovery/resources` | 52 ms | **457 ms** |

Las tres tasks subieron a la vez. El escalon chico es de las 12:45 EDT y el
grande coincide con el despliegue de la task definition 410 a las 13:03 EDT.
En esa ventana los logs muestran a `discovery_health` recorriendo el catalogo
con sondas 402 en vivo y poniendo recursos en cuarentena. No hay 5xx y los
targets siguen sanos: es lentitud, no caida.

```bash
# Memoria y CPU por task, 5 min, desde antes del escalon
aws cloudwatch get-metric-data --region us-east-2 \
  --start-time 2026-09-10T15:30:00Z --end-time 2026-09-10T18:30:00Z \
  --metric-data-queries '[{"Id":"mx","MetricStat":{"Metric":{"Namespace":"ECS/ContainerInsights",
  "MetricName":"MemoryUtilized","Dimensions":[{"Name":"ClusterName","Value":"facilitator-production"},
  {"Name":"ServiceName","Value":"facilitator-production"}]},"Period":300,"Stat":"Maximum"}}]'
```

**Por que importa para esta decision:** 1632 MiB son el 80 % de los 2 GiB de la
task y la puerta de Astra pide picos de memoria por debajo del 70 %. En una task
de 1 GiB — la opcion (b) de mas abajo — esto seria un OOM. Y el 1024 sostenido
dice que hoy el servicio ya usa el 100 % del vCPU que tiene, con un trafico de
0,67 requests por segundo. **No es trafico: es trabajo de fondo.** Eso es lo que
mide el resto del informe.

---

## 1. Perfil real de produccion, 7 dias

Ventana 2026-09-03T17:00Z → 2026-09-10T17:00Z.

### 1.1 Que esta corriendo

| Dato | Valor | Comando |
|---|---|---|
| Task definition | `facilitator-production:410` durante la ventana, `:411` al cierre; 1024 CPU / 2048 MiB en ambas | `aws ecs describe-task-definition --task-definition facilitator-production:410 --region us-east-2` |
| Arquitectura | `runtimePlatform` nulo → x86_64 por defecto | idem |
| Tasks | 3 deseadas, autoscaling min 2 / max 3, media 2,82 | `aws application-autoscaling describe-scalable-targets --service-namespace ecs --region us-east-2` |
| Escalado hacia arriba | `RequestCountPerTarget` (Sum/1 min) > 15, 3 minutos seguidos | `aws cloudwatch describe-alarms --alarm-name-prefix TargetTracking-service/facilitator-production --region us-east-2` |
| Escalado hacia abajo | < 13,5 durante 15 minutos | idem |
| Despliegue | rolling, minimo 100 % / maximo 200 %, sin circuit breaker | `aws ecs describe-services ...` |
| Reparto de rutas | el target group de escrituras recibe `/settle`, `/register`, `/feedback`, `/feedback/*`, `/dx402/anchor`; el resto va al de lecturas | `aws elbv2 describe-rules --listener-arn ...` |

El servicio oscila entre 2 y 3 tasks varias veces por dia. La causa registrada
de cada subida es siempre la alarma de `RequestCountPerTarget`, nunca la de
memoria.

### 1.2 Mezcla por ruta

389.257 eventos `http_request` en 7 dias. `/health` es el 58 % y lo genera el
propio ALB contra cada task, asi que no aparece en las cuentas del balanceador.

```bash
aws logs start-query --region us-east-2 --log-group-name /ecs/facilitator-production \
  --start-time 1788454800 --end-time 1789059600 --query-string \
  'filter @message like /x402_rs::telemetry/
   | parse @message /uri.{0,12}=.{0,8}(?<uri>\/\S*)/
   | parse @message /status=(?<st>\d+) elapsed=(?<el>\d+)ms/
   | parse uri /^\/(?<seg1>[^\/\?]*)(\/(?<seg2>[^\/\?]*))?/
   | stats count() as n, pct(el,50) as p50, pct(el,95) as p95, pct(el,99) as p99 by seg1, seg2, st
   | sort n desc | limit 400'
```

| Ruta | Eventos | Share sin `/health` | p50 | p95 | p99 |
|---|---:|---:|---:|---:|---:|
| `/health` 200 | 226.778 | — | 0 ms | 0 ms | 0 ms |
| `/discovery/resources` 200 | 60.187 | 37,1 % | 53 ms | 137 ms | 208 ms |
| `/settle` 502 | 13.142 | 8,1 % | 132 ms | 330 ms | 739 ms |
| `/supported` 200 | 13.773 | 8,5 % | 1 ms | 2 ms | 6 ms |
| `/` 200 (portada, 245 KB) | 11.502 | 7,1 % | 0 ms | 0 ms | 0 ms |
| `/verify` 429 | 5.091 | 3,1 % | 0 ms | 0 ms | 0 ms |
| `/escrow/state` 200 | 3.723 | 2,3 % | 92 ms | 201 ms | 284 ms |
| `/reputation/base` 200 | 3.467 | 2,1 % | 66 ms | 126 ms | 152 ms |
| `/version` 200 | 3.385 | 2,1 % | 0 ms | 0 ms | 0 ms |
| `/blacklist` 200+429 | 3.153 | 1,9 % | 0 ms | 0 ms | 0 ms |
| `/accepts` 400 | 2.800 | 1,7 % | 0 ms | 0 ms | 1 ms |
| `/settle` 200 | 2.653 | 1,6 % | 7141 ms | 7612 ms | 11560 ms |
| `/feedback` 500 | 1.590 | 1,0 % | 116 ms | 1163 ms | 1325 ms |
| `/dx402/anchor` 201 | 984 | 0,6 % | 813 ms | 1638 ms | 2741 ms |
| `/verify` 200 | 311 | 0,2 % | 76 ms | 259 ms | 443 ms |

Dos lecturas que cambian el resto del analisis:

* **`/verify` es mayoritariamente 429.** 5.091 de 5.944 llamadas se rechazan en
  el limitador. El presupuesto es un token cada 2 s con burst 30 por IP.
* **`/settle` es mayoritariamente 502.** 13.142 de 19.574. Es el hallazgo A1 de
  la auditoria (fondos del signer clasificados como RPC caido), no algo que este
  informe mida.

Del ALB, misma ventana: 241.773 requests y 32 respuestas 5xx en lecturas;
16.314 requests y 10.205 5xx en escrituras.

### 1.3 Hay dos picos distintos y no se parecen

Esto es lo que rompe la idea de un unico numero de "RPS que aguanta".

| Pico | Cuando | Tamaño | De que esta hecho |
|---|---|---:|---|
| Burst de escaneo | 2026-09-05T20:51Z | 1.980 req/min/target = **33 rps por task** | `/verify` 429, `/supported`, `/version`, `/accepts` 400, `/blacklist`. Casi todo rechazado o de 0 ms |
| Trabajo util | 2026-09-07T21:49Z | 1.976 req/min = **32,9 rps en todo el servicio** | La misma forma: 955 `/verify` 429, 583 `/supported`, 575 `/version`, 504 `/accepts` 400… y **36** `/discovery/resources` |
| Recorrido del catalogo | todos los dias 19:18Z | 208 req/min = **3,47 rps en todo el servicio** | `/discovery/resources`, p50 estable en 51-54 ms |

El pico que dispara el autoscaling es el primero: requests que el limitador
tira antes de tocar un handler. **La tercera task se paga para atender trafico
que el servicio rechaza.** Ese es el hallazgo A6 con numero encima.

El unico pico que cuesta CPU de verdad es el tercero, y es diez veces mas chico.

### 1.4 CPU y memoria

```bash
aws cloudwatch get-metric-data --region us-east-2 \
  --start-time 2026-09-03T17:00:00Z --end-time 2026-09-10T17:00:00Z \
  --metric-data-queries file://q_ecs.json   # ver scripts/bench/README.md
```

| Señal, 7 dias | media | p95 | max |
|---|---:|---:|---:|
| CPU por task (%) | 1,30 | 2,05 | 101,14 |
| CPU por task (unidades, 1024 = 1 vCPU) | 13,3 | 21,0 | 1009 |
| Memoria por task (MiB) | 289 | 372 | **427** |
| Tasks corriendo | 2,82 | 3,0 | 6 (despliegues) |

En 30 dias, y descontando el episodio de hoy, el maximo de memoria fue 392 MiB.

### 1.5 Cuanto de esa CPU es trabajo por request

Regresion por minimos cuadrados sobre **4.800 minutos** de produccion (2026-09-07
a 2026-09-10, antes del episodio), con la CPU total del servicio como variable
dependiente y dos tasas de request como independientes:

```
CPU_servicio = 30,0 unidades + 77,3 x rps(/discovery/resources) + 0,76 x rps(resto)
R2 = 0,41
```

| Termino | En unidades de Fargate | Traducido |
|---|---:|---|
| Fondo | 30,0 unidades en el servicio | **10,0 unidades por task**, ~1 % de un vCPU, sin trafico |
| `/discovery/resources` | 77,3 por rps | **75,5 ms de CPU por request** |
| Todo lo demas | 0,76 por rps | **0,75 ms de CPU por request** |

El `R2` de 0,41 es modesto — el trabajo de fondo tiene sus propias rafagas — pero
los coeficientes coinciden con lo que mide el banco local (seccion 2.3), que se
obtuvo de otra manera y en otra maquina.

Con esos coeficientes, en la media de 7 dias (0,0996 rps de
`/discovery/resources` y 0,544 rps de todo lo demas):

| | unidades de CPU en el servicio |
|---|---:|
| Trabajo por request | 8,1 |
| Fondo (termino independiente del ajuste) | 30,0 |
| Suma del modelo | 38,1 |
| Medido: 13,32 unidades/task x 2,82 tasks | 37,6 |

El modelo y la medida coinciden dentro del 1,5 %.

**El 79 % de la CPU que consume el facilitador no la pide ningun cliente.** Hoy,
durante el episodio de la seccion 0, esa proporcion pasa del 99 %: 20 unidades de
trabajo por request contra 2.340 unidades consumidas.

---

## 2. Banco local reproducible

### 2.1 Como

Binario compilado con las features de produccion, corriendo en la Mac, contra un
JSON-RPC simulado en loopback. Los `RPC_URL_*` de las 25 redes EVM apuntan al
mock, la red de los pagos es `base-sepolia` y las firmas EIP-712 se generan con
una llave efimera creada en el momento.

```bash
cargo build --release --locked --features solana,near,stellar,algorand,sui,xrpl
aws s3 cp s3://facilitator-discovery-prod/bazaar/resources.json /tmp/catalog.json --region us-east-2
python3 scripts/bench/run_bench.py --out-dir /tmp/bench-2w --label capacidad-2w \
  --catalog /tmp/catalog.json --duration 45 --rps 5,11,22,33,44,66 --clients 96 \
  --worker-threads 2 --per-route --rpc-latency-ms 25 --receipt-delay-ms 0
```

Cuatro decisiones que cambian el resultado y conviene tener a la vista:

1. **El catalogo se siembra.** El objeto de S3 tiene 103 MB y 39.589 recursos, y
   vive en memoria en cada task. Con el catalogo vacio `/discovery/resources`
   contesta en 0 ms y el banco no mide la ruta que se lleva el 95 % de la CPU.
   El banco carga los 39.589 (`seed_catalog.py`) y comprueba `/discovery/stats`.
2. **El generador rota `X-Forwarded-For`.** Todas las rutas calientes llevan
   limitador por IP. Un generador de una sola IP mide el limitador.
3. **Modelo abierto.** Llegadas a tasa fija. En modelo cerrado la tasa baja sola
   cuando el servidor se pone lento y los percentiles salen mejores de lo que son.
4. **Los percentiles se leen del log del servidor.** `telemetry.rs` ya emite
   `status=NNN elapsed=Nms` con la misma resolucion que produccion. El reloj del
   cliente en un laptop compartido metia p95 de segundos en rutas de 0,5 ms;
   `parse_server_log.py` evita ese ruido y hace los numeros comparables uno a uno
   con la tabla de la seccion 1.2.

Maquina: Apple M4, 10 nucleos, 24 GiB, macOS 26.2, binario `arm64`.
**No se mide Graviton aca.** Un laptop ARM no dice nada sobre un vCPU de Fargate
ARM; la seccion 3 trata esa opcion como no medida.

**La mezcla del banco, exactamente.** Los pesos son el share de cada ruta
*entre las requests que no son `/health`*, y el 25,8 % restante — las rutas que
el banco no modela: `/escrow/state`, `/reputation/*`, `/identity/*`,
`/feedback*`, `/dx402/*`, ruido de escaneo — se representa con `/health`, que es
la ruta barata de referencia.

| Ruta | Peso en el banco |
|---|---:|
| `/discovery/resources?limit=100` | 37,1 |
| `/health` (en lugar de las rutas no modeladas) | 25,8 |
| `/settle` | 12,0 |
| `/supported` | 8,5 |
| `/` | 7,1 |
| `/verify` | 3,7 |
| `/version` | 2,1 |
| `/blacklist` | 1,9 |
| `/accepts` | 1,8 |

Contando `/health` de verdad, `/discovery/resources` es el 15,5 % del trafico de
produccion, no el 37 %. **La mezcla del banco es 2,4 veces mas pesada en la ruta
cara que la realidad**, y por eso todas sus cifras de capacidad son
conservadoras. Las dos versiones, con los coeficientes de §1.5:

| Mezcla | CPU por request en Fargate x86 |
|---|---:|
| Del banco (37 % de catalogo) | **28,5 ms** |
| Real de produccion, contando `/health` (15,5 % de catalogo) | **12,3 ms** |

Este informe usa la del banco en todas las tablas de capacidad y costo.

### 2.2 El banco reproduce produccion

Antes de usar un numero conviene ver que el banco no invento otro sistema:

| Ruta | Produccion (7 dias) | Banco local | Diferencia |
|---|---:|---:|---:|
| `/discovery/resources` p50 | 53 ms | 59-65 ms | +11 a +23 % |
| `/settle` 200 p50 (con confirmacion de 6 s simulada) | 7141 ms | 6561 ms | −8 % |
| `/supported` p50 | 1 ms | 0 ms | — |
| Arranque hasta `/health` 200 | — | 0,3-1,1 s | — |

### 2.3 CPU por request, por ruta

Cada ruta medida sola, a tasa baja, con la CPU del proceso muestreada antes y
despues (`psutil`). El consumo en reposo se resto: son 0,00 nucleos en 20 s.

| Ruta | CPU/req medida (M4) | CPU/req en Fargate x86 (regresion §1.5) | Bytes por respuesta |
|---|---:|---:|---:|
| `/discovery/resources?limit=100` | **62 ms** (rango 52-68 en 4 corridas) | **75,5 ms** | 109 KB |
| `/settle` | 2,4 ms | \| | 433 B |
| `/verify` | 1,3 ms | \| | 69 B |
| `/accepts` | 0,9 ms | \| 0,75 ms | 494 B |
| `/supported` | 0,8 ms | \| (todas juntas) | 17 KB |
| `/` (portada) | 0,6 ms | \| | 245 KB |
| `/version` | 0,6 ms | \| | 20 B |
| `/blacklist` | 0,5 ms | \| | 264 B |
| `/health` | 0,5 ms | \| | 20 B |
| **Mezcla del banco** | **21 ms** | **28,5 ms** | |

Una ruta cuesta cien veces mas que todas las demas. `/discovery/resources` es el
15,5 % de las requests de produccion y el **95 %** de su CPU (98 % en la mezcla
del banco). Cualquier decision de tamaño de task es, en la practica, una decision
sobre esa ruta.

`/settle` cuesta 2,4 ms de CPU y 7 segundos de reloj: casi todo es espera de
confirmacion, que no consume vCPU pero si mantiene la peticion abierta.

### 2.4 Percentiles a 1x y 2x del pico, con la mezcla real

`1x` = 11 rps por task (los 32,9 rps del pico util repartidos en 3 tasks).
`2x` = 22 rps por task. Las columnas 33/44/66 buscan el codo.

Medido por el servidor, en milisegundos, con `TOKIO_WORKER_THREADS=2` (que es el
`available_parallelism` que reporta una task de 1 vCPU):

| Ruta | 5 rps | **11 rps (1x)** | **22 rps (2x)** | 33 rps | 44 rps | 66 rps |
|---|---|---|---|---|---|---|
| `/discovery/resources` | 74/266/947 | **65/210/693** | **59/203/705** | 61/110/593 | 64/172/821 | 62/105/594 |
| `/settle` | 236/443/843 | **264/669/1026** | **275/886/1068** | 325/782/1887 | 464/**5647**/9597 | 841/6435/7297 |
| `/verify` | 54/416/416 | **53/168/168** | **88/407/743** | 86/620/1240 | 160/**2985**/3152 | 248/1650/3510 |
| `/supported` | 0/9/9 | **0/4/22** | **0/4/19** | 0/0/3 | 0/1/3 | 0/2/4 |
| `/` | 0/0/6 | **0/0/0** | **0/2/23** | 0/0/2 | 0/0/0 | 0/0/0 |
| `/accepts` | 22/22/22 | **1/515/515** | **0/19/19** | 0/1/206 | 0/3/393 | 0/2/44 |
| `/health` | 0/0/8 | **0/0/0** | **0/0/6** | 0/0/0 | 0/0/0 | 0/0/0 |

Formato `p50/p95/p99`.

| Fase | rps logrado | nucleos usados | CPU/req | RSS pico | desfase max de llegada |
|---|---:|---:|---:|---:|---:|
| fria (5 rps, sin warmup) | 4,47 | 0,11 | 25,9 ms | 160 MiB | 7 |
| 5 rps | 4,48 | 0,12 | 27,3 ms | 113 MiB | 25 |
| **11 rps (1x)** | 10,08 | 0,24 | 24,9 ms | 105 MiB | 47 |
| **22 rps (2x)** | 19,83 | 0,37 | 19,2 ms | 123 MiB | 105 |
| 33 rps | 29,96 | 0,63 | 21,5 ms | 188 MiB | 149 |
| 44 rps | 38,98 | 0,86 | 22,3 ms | 254 MiB | 197 |
| 66 rps | 59,76 | 1,30 | 22,3 ms | 269 MiB | 290 |

Lecturas:

* **A 1x y a 2x no pasa nada.** Las lecturas se mantienen: `/discovery/resources`
  p95 210 → 203 ms, `/supported` p95 4 ms. Ninguna ruta se degrada al pasar de
  1x a 2x mas de un 33 % en p95, y las que suben lo hacen desde valores de un
  digito. Traducido a una task de Fargate de 1 vCPU, a 2x del pico el trabajo por
  request seria 19,83 rps x 28,5 ms = **565 ms de CPU por segundo, el 57 % del
  vCPU** con la mezcla del banco, y 244 ms/s (**24 %**) con la mezcla real de
  produccion. La puerta pide menos del 60 %.
* **El codo esta en 44 rps por task, y esta en la escritura**, no en la lectura:
  `/settle` p95 salta de 886 a 5647 ms y `/verify` de 407 a 2985 ms mientras
  `/discovery/resources` sigue en 172 ms. Con un round trip de RPC de 25 ms y
  ~10 llamadas por settle, el camino de firma se serializa mucho antes de que
  se acabe la CPU. **La capacidad de escritura por task esta en el orden de 4-6
  settles por segundo**, contra 0,18 rps de pico horario en todo el target group
  de escrituras de produccion (658 requests en la hora mas cargada de 7 dias).
* **La mezcla escalada castiga mas de lo que pide el encargo.** A 22 rps por
  task la mezcla entrega 24,5 rps de `/discovery/resources` en el servicio, que
  son **7,1x** el pico real de esa ruta, no 2x. El margen es mayor que el que
  muestra la tabla.
* **Frio contra caliente:** el arranque a `/health` 200 tarda 0,3-1,1 s y la
  primera pagina del catalogo cuesta 91 ms contra 65 ms en caliente. No hay
  penalidad de arranque que importe.

### 2.5 Control comparable para la opcion de media vCPU

No hay cgroups en macOS, asi que no se puede reproducir la cuota de Fargate. Lo
que si se puede es partir a la mitad el paralelismo del runtime
(`TOKIO_WORKER_THREADS=1`) y correr exactamente la misma carga. Es una
aproximacion, y se etiqueta como tal.

| Ruta | 11 rps: 2 hilos → 1 hilo (p95) | 22 rps: 2 hilos → 1 hilo (p95) |
|---|---|---|
| `/discovery/resources` | 210 → 853 ms (**+306 %**) | 203 → 393 ms (**+94 %**) |
| `/settle` | 669 → 3123 ms (**+367 %**) | 886 → 13230 ms (**+1393 %**) |
| `/verify` | 168 → 2647 ms (**+1476 %**) | 407 → 5363 ms (**+1218 %**) |
| `/supported` | 4 → 4 ms (0 %) | 4 → 1 ms (−75 %) |

Hay ademas un argumento que no necesita banco. Fargate con 512 unidades de CPU
da media cuota de un nucleo. Una request de `/discovery/resources` consume 75 ms
de CPU seguidos: con media cuota se ejecuta 50 ms, se le quita el turno 50 ms y
termina — **125 ms de reloj en vez de 75, estando sola en la maquina**. El p50 de
la ruta pasaria de 53 ms a mas de 100 ms por construccion, antes de aplicar
ninguna carga. Eso ya rompe la puerta del 10 %.

---

## 3. Modelo de costo

Precios de la Price List de AWS, us-east-2, Linux, on-demand, leidos el
2026-09-10:

```bash
aws pricing get-products --region us-east-1 --service-code AmazonECS \
  --filters Type=TERM_MATCH,Field=usagetype,Value=USE2-Fargate-vCPU-Hours:perCPU --output json \
 | jq -r '.PriceList[]|fromjson|.terms.OnDemand[].priceDimensions[].pricePerUnit.USD'
```

| | vCPU-hora | GiB-hora |
|---|---:|---:|
| x86 | 0,04048 | 0,004445 |
| ARM | 0,03238 | 0,003560 |

Mes normalizado de 730 horas. Cost Explorer confirma el orden de magnitud:
agosto cerrado facturo 2.255,65 vCPU-hora (91,31 USD) y 5.074,74 GiB-hora
(22,56 USD) de Fargate en us-east-2, 113,87 USD entre las dos. Pero **esa cifra
incluye el cluster `em-production`**: 2.255,65 vCPU-hora en 744 horas son 3,03
vCPU medios, y el facilitador solo justifica 3,00. ECS no propaga tags, asi que
no hay atribucion por proyecto y lo que sigue es un modelo de capacidad, no una
factura.

```bash
aws ce get-cost-and-usage --region us-east-1 --time-period Start=2026-08-01,End=2026-09-01 \
  --granularity MONTHLY --metrics UnblendedCost UsageQuantity \
  --filter '{"And":[{"Dimensions":{"Key":"REGION","Values":["us-east-2"]}},
             {"Dimensions":{"Key":"SERVICE","Values":["Amazon Elastic Container Service"]}}]}' \
  --group-by Type=DIMENSION,Key=USAGE_TYPE
```

```bash
python3 scripts/bench/cost_model.py --cpu-ms-per-req 28.5 --peak-rps 32.9 --background-ms-per-s 9.8
```

Capacidad calculada con la mezcla del banco (28,5 ms/req), que es la
conservadora; con la mezcla real de produccion (12,3 ms/req) los rps por task se
multiplican por 2,3.

| Opcion | USD/mes | vs hoy | rps/task (mezcla del banco) | Margen sobre el pico util | Margen sobre el pico del catalogo |
|---|---:|---:|---:|---:|---:|
| **Hoy** — x86, 3 × 1 vCPU / 2 GiB | 108,12 | — | 20,7 | 1,9x | 6,8x |
| **(a)** x86, 2 × 1 vCPU / 2 GiB | **72,08** | **−36,04 (−33 %)** | 20,7 | 1,3x | 4,5x |
| **(b)** x86, 3 × 0,5 vCPU / 1 GiB | **54,06** | **−54,06 (−50 %)** | 10,2 | 0,9x | 3,3x |
| **(c)** ARM, 3 × 1 vCPU / 2 GiB | **86,51** | **−21,62 (−20 %)** | 20,7 (no medido) | 1,9x (no medido) | 6,8x (no medido) |
| (referencia) ARM, 2 × 1 vCPU / 2 GiB | 57,67 | −50,45 (−47 %) | 20,7 (no medido) | 1,3x | 4,5x |

Las columnas de margen usan el techo del 60 % de CPU sostenida que pide la
puerta de Astra y descuentan los 10 ms/s de trabajo de fondo por task. Las
opciones no se suman entre si.

Dos avisos sobre esas columnas. **"Margen sobre el pico util"** vale los 32,9 rps
del pico observado *como si fueran la mezcla del banco*, y no lo son: ese pico es
casi todo requests de 0,75 ms. Es un piso, no una estimacion. **"Margen sobre el
pico del catalogo"** compara los 3,47 rps de `/discovery/resources` contra la
capacidad de la task en esa ruta sola, a 75,5 ms cada una — esa es la columna que
manda, porque esa ruta es el 95 % de la CPU.

### Que se pierde en cada una

**(a) Mismo tamaño, dos tasks, autoscaling recalibrado — 72,08 USD/mes.**
Se pierde la tercera replica como colchon: durante un despliegue rolling con
minimo 100 % el servicio sigue teniendo 2 tasks sanas, pero un fallo de AZ deja
una sola. El margen sobre el pico del catalogo baja de 6,8x a 4,5x, que sigue
siendo holgado. Lo que **no** se resuelve es el trabajo de fondo: el episodio de
hoy con dos tasks habria sido igual de malo. Y para que el ahorro sea real hay
que cambiar la metrica de escalado: hoy la tercera task la pide un escaneo que
el limitador rechaza.

**(b) Task mas chica — 54,06 USD/mes.** Es la unica opcion que **falla las
puertas hoy mismo**, por dos motivos independientes:

* *Memoria.* El pico de hoy es 1632 MiB. En una task de 1 GiB es un OOM, no una
  degradacion. Incluso con el historico limpio (392 MiB de maximo en 30 dias) el
  margen es 38 %, sin espacio para que el catalogo siga creciendo — y el catalogo
  vive entero en memoria en cada replica.
* *Latencia.* Media cuota de vCPU duplica el tiempo de servicio de
  `/discovery/resources` por construccion (+100 % de p50), y el control con la
  mitad del paralelismo mide +306 % de p95 en esa ruta y +367 % en `/settle` ya
  a 1x del pico. La puerta pide menos del 10 %.

Una variante intermedia, 3 × 0,5 vCPU / **2 GiB** (63,79 USD/mes, −41 %), salva
la memoria pero no la latencia.

**(c) ARM, mismo tamaño — 86,51 USD/mes.** No se pierde nada funcional *si* la
imagen compila y las dependencias criptograficas y los SDK se comportan. El
riesgo no es de capacidad sino de construccion: el `Dockerfile` usa
`$BUILDPLATFORM` en las dos etapas y ninguno de sus dos `cargo build` lleva
`--locked` (hallazgo A9), asi que cambiar `runtimePlatform` en ECS no alcanza.
**El rendimiento de Graviton en esta carga no esta medido y este informe no lo
estima**: la ruta que decide es `/discovery/resources`, que es memoria y
asignacion mas que aritmetica, y ahi la extrapolacion desde un M4 no vale nada.
Medirlo cuesta poco: una task canaria con `runtimePlatform` ARM64 en el mismo
servicio y comparar el p50 de esa ruta durante el recorrido diario del catalogo.

---

## 4. Recomendacion

**Ninguna de las tres todavia. Primero cerrar lo de la seccion 0; despues (a);
(c) solo con una canaria medida; (b) descartada.**

Los numeros que sostienen eso:

1. **El servicio no tiene un problema de capacidad de trafico.** A 2x del pico
   observado, con la mezcla del banco (que es 2,4 veces mas pesada que la real) y
   el catalogo entero de produccion, ninguna ruta de lectura se degrada:
   `/discovery/resources` p95 203 ms, `/supported` p95 4 ms. Traducido a Fargate,
   eso son 565 ms de CPU por segundo, el **57 % de un vCPU**, dentro de la puerta
   del 60 %. El codo esta a 4x del pico, y esta en el camino de escritura (4-6
   settles/s por task) que hoy corre a 0,18 rps de pico horario.
2. **Tiene un problema de trabajo de fondo.** El 79 % de la CPU que consume en
   media no la pide ningun cliente, y hoy esa proporcion supero el 99 % con las
   tres tasks al 100 % de su vCPU y 1,6 GiB de memoria. Achicar la task antes de
   arreglar eso achica lo que no sobra.
3. **La palanca mas segura es la metrica de escalado, no el tamaño.** La tercera
   task se paga porque un escaneo cruza el umbral de 15 req/min/target — trafico
   que el limitador tira con 429. Cambiar esa metrica por una que represente
   trabajo util (peticiones no rechazadas, o directamente la tasa de
   `/discovery/resources`) deja el servicio en 2 tasks la mayor parte del tiempo
   y ahorra 36 USD/mes sin tocar el tamaño ni la arquitectura.

### Contra las puertas de Astra

| Puerta | Estado con la evidencia de este informe |
|---|---|
| Sin regresion de p95/p99 mayor al 10 % frente al control comparable | **(a)** no cambia el tamaño de la task: no hay regresion por construccion, pero hay que verificarla con 2 tasks sostenidas. **(b)** falla: +306 % de p95 en la ruta dominante en el control de medio paralelismo, y +100 % de p50 por la cuota de CPU. **(c)** sin medir. |
| Memoria de pico por debajo del 70 % del tamaño elegido | Hoy: 1632 / 2048 = **80 %**, no pasa. Historico limpio: 392 / 2048 = 19 %, pasa. Con 1 GiB no pasa en ninguno de los dos casos (38 % historico, OOM hoy). |
| CPU sostenida de pico por debajo del 60 % | Hoy: **100 % sostenido 40 minutos**, no pasa. Historico: 1,3 % de media y 2,05 % de p95, pasa con enorme margen. |
| Ninguna duplicacion de nonce o broadcast atribuible al cambio | No aplica: ninguna de las tres opciones toca el camino de firma. El writer lease sigue eligiendo una sola task escritora. |
| Errores esperados separados de fallos de infraestructura | No aplica a este cambio; sigue abierto en A1 (`/settle` 502 por fondos del signer). |

---

## 5. Lo que este informe no mide

* **Rendimiento de Graviton.** Se midio en un M4 y eso no se traslada. La
  comparacion ARM contra x86 sale de Fargate o no sale.
* **RSS en Linux.** El RSS de macOS no es comparable: el mismo binario con el
  mismo catalogo dio entre 95 y 425 MiB segun la fase, por compresion de memoria
  del sistema. Para dimensionar memoria valen los numeros de CloudWatch.
* **Semantica on-chain.** El RPC es simulado: las firmas se validan de verdad y
  `/verify` y `/settle` recorren su camino completo, pero ninguna transaccion
  toca una cadena. Para semantica queda pendiente una corrida de volumen bajo
  contra `base-sepolia`, que este banco ya soporta cambiando un `RPC_URL_*`.
* **Las rutas no incluidas en la mezcla**: `/escrow/state`, `/reputation/*`,
  `/identity/*`, `/feedback*`, `/dx402/*`. Suman el 8 % del trafico util y
  algunas son caras (`/identity/.../owner` hace un barrido con Multicall3). No
  se midieron aca.
* **La causa del episodio de la seccion 0.** Se documenta la observacion y donde
  apuntan los logs. Diagnosticarlo no es parte de este encargo.
* **La atribucion del costo.** ECS no propaga tags, y Cost Explorer mezcla el
  facilitador con el cluster `em-production`. Las cifras de la seccion 3 son un
  modelo de capacidad, no una factura del proyecto.

---

## Para c0der

1. **Hay una regresion viva en produccion desde las 13:03 EDT de hoy**: las tres
   tasks al 100 % de su vCPU, 1,6 GiB de memoria (80 % del limite, subiendo) y el
   p95 de lecturas 25 veces peor. Decidi si se mira ahora o se deja correr.
2. **La palanca que recomiendo es (a), y su parte que ahorra no es bajar a dos
   tasks sino cambiar la metrica de autoscaling**: hoy la tercera task la dispara
   un escaneo que el limitador rechaza. Son 36 USD/mes sin tocar tamaño ni
   arquitectura. Decidi si se hace ese cambio de metrica.
3. **(b), la task de 0,5 vCPU / 1 GiB, la descarto con numeros** (OOM hoy, +100 %
   de p50 en la ruta dominante por cuota de CPU). **(c), ARM, no la puedo
   recomendar ni descartar sin una canaria en Fargate.** Decidi si se levanta esa
   canaria.
