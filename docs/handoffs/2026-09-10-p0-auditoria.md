---
date: 2026-09-10
tags:
  - type/handoff
  - domain/settle
  - domain/writer-lease
  - domain/discovery
  - priority/p0
status: active
---

# Los tres P0 de la auditoría de arquitectura

**Versión:** 2.18.0 · **Origen:** auditoría de Rust/AWS/costos hecha por otra
sesión los 2026-09-09/10 sobre la base `9893c48f`, con producción en
`2.16.0-9893c48`. Hallazgos A1, A2 y A3 de esa auditoría.

Los tres tienen la misma forma: **un camino de error que decidía mal**. Un
código JSON-RPC leído como caída del nodo cuando era nuestra propia billetera
sin gas; un fallo del plano de control leído como permiso para firmar; una
lectura fallida leída como catálogo vacío. Ninguno era un bug de lógica de
negocio, y por eso ninguno se veía en las métricas de éxito.

---

## Para c0der

### Qué cambió, por hallazgo

**A1 — clasificación tipada del fallo de escritura en cadena.**
`is_upstream_rpc_failure` (booleano sobre códigos JSON-RPC) desaparece.
`src/chain/failure.rs` clasifica por **etapa** (request / broadcast /
confirmation) y **motivo**, y cada par lleva su propio status, su token acotado
y su propio consejo de reintento — incluido *ninguno*:

| `error` | Status | `Retry-After` |
|---|---|---|
| `contract_call_failed` | 400 | — |
| `facilitator_signer_unfunded` | **503** | ~300 s, con jitter |
| `upstream_nonce_or_mempool` | 502 | 30 s |
| `upstream_rate_limited` | 503 | 60 s |
| `upstream_rpc_unavailable` | 502 | 30 s |
| `broadcast_uncertain` | 502 | **ninguno** |
| `receipt_pending` | 502 | **ninguno** |
| `writer_lease_unavailable` (A2) | 503 | 5 s |

Dos reglas mandan sobre el resto: un revert gana siempre (la cadena contestó),
y nada que haya pasado la etapa de broadcast lleva `Retry-After`. Los tres
sitios que reportaban una escritura fallida — la rama escrow de `/settle`, el
brazo `ContractCall` de `IntoResponse` y `/escrow/state` — pasan por el mismo
mapeo; antes daban tres respuestas distintas a la misma condición.

`GasShortfall` extrae `balance` / `queued cost` / `tx cost` / `overshot` del
mensaje del nodo y loguea el **margen utilizable**, que es lo que faltaba.

**A2 — el lease del escritor es ahora una concesión con vencimiento.**
La rama `Err` del bucle de renovación ponía `IS_WRITER = true`. Un fallo del
plano de control lo ven las tres tareas a la vez, así que esa línea autorizaba
a las tres para el mismo signer EVM.

Ahora un acquire exitoso gana una concesión que vence `HANDOVER_MARGIN` **antes**
que el registro que escribió, medida desde que la petición **salió**. Un error
no la extiende ni la revoca: sigue corriendo sola. Un parpadeo se absorbe (caben
más de cinco renovaciones en una concesión); una caída sostenida la termina sin
que nadie decida. Cada relevo sube una `generation` y la renovación es
condicional a que siga siendo la nuestra. La ruta de firma toma un
`SigningPermit`, emitido solo con `SIGNING_HEADROOM` por delante y sostenido
sobre la sección exclusiva (reserva de nonce + broadcast), **no** sobre la espera
del receipt; `release()` drena los permisos pendientes antes de entregar el
registro.

Números: `LEASE_TTL` 15 s → 30 s, `RENEW_INTERVAL` 5 s → 3 s, `HANDOVER_MARGIN`
10 s (concesión útil 20 s), `SIGNING_HEADROOM` 3 s. Un test fija cada relación
entre ellos.

**A3 — el catálogo S3 ya no se puede vaciar con una lectura fallida.**
`load_all().await.unwrap_or_default()` seguido de un PUT del objeto entero.
Ahora `Version` distingue "el objeto no existe" (base conocida) de un error de
lectura (sobre el que no se escribe), las escrituras son condicionales
(`If-Match` / `If-None-Match: *`), `save`/`delete` son read-modify-write con
reintento acotado sobre una lectura fresca, y `merge_resource` se niega a
retroceder `last_updated`. `save_all` **no** reintenta a propósito: un snapshot
es autoritativo sobre las bajas, así que reaplicarlo sobre un catálogo más nuevo
resucitaría lo que se borró. Dentro del proceso, las escrituras van por una cola
llenada sincrónicamente en el orden en que se mutó la caché.

### A1: qué red y qué signer están sin gas

**Polygon mainnet**, signer `0x103040545AC5031A11E8C03dd11324C7333a13C7` (la
billetera EVM de mainnet, `lambda/balances/handler.py`).

Es **una sola red y un solo signer**: las 7.196 líneas de error de las 24 h hasta
2026-09-10 04:00 UTC llevan el mismo `balance 82861633384675957709`, agrupadas
por CloudWatch Logs Insights. El tráfico escrow de esa ventana fue polygon 2.337,
base 418, avalanche 61, arbitrum 17, monad 11, ethereum 3, optimism 1; ninguna
otra red produjo este error.

El mensaje real, sin sanear más que quitarle el marco de tracing:

```
insufficient funds for gas * price + value: balance 82861633384675957709,
queued cost 82799377752610042973, tx cost 78463640160630732,
overshot 16208008094715996
```

Leído en POL: saldo 82,8616 · comprometido por la cola 82,7994 · **utilizable
0,0623** · costo de la siguiente transacción 0,0785 · falta 0,0162.

Estado de la cadena, leído el 2026-09-10 contra un RPC público (solo lectura):

| Medida | Valor |
|---|---|
| `eth_getBalance` | `0x47defa88b2a425bcd` = 82,86163338467595 POL |
| `eth_getTransactionCount` latest | 1157 |
| `eth_getTransactionCount` pending | 1557 |
| **Transacciones atascadas** | **400** |

El saldo on-chain de hoy es **idéntico byte a byte** al que aparece en los logs
del 2026-09-09: no se movió nada.

Línea de tiempo, de los logs (retención 30 días):

- **2026-08-20 18:06 UTC** — primera línea `insufficient funds`, con
  `balance 108666361139778311` (0,1087 POL) y **sin** `queued cost`. Ahí la
  billetera estaba genuinamente vacía.
- **2026-09-04 03:48 UTC** — primera aparición del saldo actual, 82,8616 POL. La
  billetera se recargó… y el error **siguió**, ahora con `queued cost 82,7994`.
- **2026-09-10** — igual.

La lectura operativa: **recargar de nuevo no arregla esto.** Las 400
transacciones pendientes (nonces 1157–1556) retienen el 99,92 % del saldo en el
txpool, y cualquier POL nuevo se reparte en el mismo agujero. Lo que hay que
resolver es la cola: reemplazar o cancelar esos nonces con transacciones de
precio suficiente, en orden, antes de tocar el saldo. Eso es una acción
deliberada con hashes e importes concretos, y **no** se hizo desde aquí: este
trabajo fue solo de lectura contra AWS y contra la cadena.

Nota aparte que ayuda a leer los logs: el `PendingNonceManager` en memoria estaba
en 1738 mientras el nodo reportaba 1557 pendientes. Los 181 de diferencia son
reservas que el nodo rechazó y que `release_nonce` devolvió — el camino de
`is_pre_broadcast_rejection` funciona, no hay hueco de nonce por esto.

### Qué quedó fuera, a propósito

- **`/feedback` 500 y `/register` 500.** La auditoría dice explícitamente que no
  tienen causa confirmada. No se tocaron.
- **Backoff escalado por (red, signer) y sonda de recuperación** (A1). El
  `Retry-After` de la carencia de gas es fijo (300 s ±10 %) en vez de creciente.
  El motivo es honesto: el brazo `ContractCall` de `IntoResponse` no tiene la red
  en alcance, así que una tabla por red solo cubriría la mitad de los sitios, y
  media señal es peor que una constante bien elegida. Lo que sí hay por red es la
  categoría acotada en `/events` y el log con el margen utilizable.
- **Resurrección entre procesos** (A3). La cola ordena *un* proceso. Entre las
  tres tareas ECS, la escritura condicional evita la actualización perdida pero
  no que un `save` de la tarea A, emitido antes de un `delete` de la tarea B,
  aterrice después. Eso necesita un registro **por recurso** con actualización
  condicional propia, que es la evolución que describe la auditoría (sección 6,
  A3) y que este cambio no intenta.
- **Todo lo de A4–A10**: autoscaling, tamaño de tarea, ARM, NAT, cliente HTTP
  compartido, propietario de discovery. Otra tanda.
- **Nada en `terraform/`.** Se leyó, no se escribió. Ver las dos comprobaciones
  de infraestructura abajo, que salen de esa lectura.

### Dos cosas de infraestructura que hay que saber

**1. El bucket de discovery NO tiene versionado.**
`aws s3api get-bucket-versioning --bucket facilitator-discovery-prod` responde
vacío (sin `Status`), y `list-object-versions` sobre `bazaar/resources.json`
devuelve una sola versión. **No hay red de seguridad detrás de ese objeto**: la
escritura condicional que trae A3 es la única protección que existe hoy. No se
cambió — el encargo era comprobarlo y decirlo, no tocarlo. Si se quiere activar,
es un cambio de terraform aparte y **el puerto de escape**, no el mecanismo.

El objeto pesa **15,2 MB**, así que cada alta cuesta un GET y un PUT de ese
tamaño. Va por el endpoint Gateway de S3, no por el NAT, así que no aparece en la
factura de egreso; sí es el argumento más fuerte a favor del registro por recurso.

**2. La política IAM de la tabla de nonces no concede `dynamodb:UpdateItem`.**
`terraform/environments/production/main.tf` (`DynamoDBNonceStoreAccess`) da
`PutItem`, `GetItem`, `DescribeTable` y `DeleteItem`. Por eso el fencing de A2
está construido sobre `PutItem` condicional con una generación elegida por el
reclamante y validada en la condición, y no sobre un `UpdateItem` con
`if_not_exists(generation) + 1`, que habría sido más corto y habría fallado con
`AccessDenied` en producción. Si alguien reescribe esa parte, esto es lo primero
que hay que mirar.

### La contrapartida de A2, dicha en claro

Si DynamoDB queda inalcanzable desde **todas** las tareas durante más de una
concesión (20 s), las escrituras EVM pasan a **no disponibles** en vez de
servirse con tres firmantes compitiendo por el mismo nonce. Eso es exactamente el
cambio que pidió la auditoría, y es una posición fail-closed sobre la
exclusividad.

Lo que **no** cambia es la disponibilidad en los casos normales, y eso está
probado, no supuesto:

- un parpadeo del plano de control no cuesta el lease (caben más de cinco
  reintentos fallidos dentro de una concesión);
- quien no tiene la concesión **reenvía** al que la tiene, no rechaza;
- un deploy normal deja la vía sin escritor a lo sumo un intervalo de renovación
  (3 s), porque el apagado ordenado hace `release()` y borra el registro;
- la simulación de una hora con deploy rodante, partición, caída regional con
  las dos tareas vivas, congelación de una y muerte sin `release` da **cero**
  segundos con dos escritores y más de 3.000 de 3.600 con escritor.

El break-glass documentado para una caída prolongada del plano de control es
`ENABLE_WRITER_LEASE=false`, que restaura el comportamiento anterior al lease.

### Cómo se probó que los tests sirven

Cada tanda se corrió **en rojo** restaurando el comportamiento previo, y se
restauró el arreglo después:

| Hallazgo | Inyección | Resultado |
|---|---|---|
| A1 | el chequeo de transporte antes que el de gas (el orden del booleano viejo) | 6 en rojo; el mensaje real salía como `Transport`, es decir 502 + `Retry-After: 30` |
| A2 | la rama `Err` volviendo a `IS_WRITER = true` | 2 en rojo; la simulación contó **617 s** de la hora con dos escritores autorizados |
| A3 | `unwrap_or_default()` en la lectura, un PUT incondicional, sin reintento | 5 de 16 en rojo |

### Verificación

```
cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1
cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance -- --test-threads=1
python3 scripts/verify_landing_canonical.py --offline
```

Todo en verde: 904 tests de lib + 946 de bin + los de integración, y los tres
crates del workspace.

---

## Consultas que reproducen las cifras

Producción cambia; estas son las ventanas exactas que se usaron.

```bash
# Todas las líneas de carencia de gas llevan el mismo saldo -> un signer, una red
aws logs start-query --region us-east-2 \
  --log-group-name /ecs/facilitator-production \
  --start-time $(( $(date -u +%s) - 86400 )) --end-time $(date -u +%s) \
  --query-string 'fields @message | filter @message like /insufficient funds for gas/
                  | parse @message "balance *," as bal | stats count(*) as n by bal'

# Estado de la cola en cadena (solo lectura)
curl -s -X POST https://polygon.drpc.org -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"eth_getTransactionCount",
       "params":["0x103040545AC5031A11E8C03dd11324C7333a13C7","pending"]}'

# Versionado del bucket de discovery (vacío = no está activado)
aws s3api get-bucket-versioning --bucket facilitator-discovery-prod --region us-east-2
```
