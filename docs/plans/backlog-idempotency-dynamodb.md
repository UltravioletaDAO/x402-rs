# Backlog — cache de `Idempotency-Key` de `/settle` sobre DynamoDB (FAC-IDEMP-001)

**Estado:** decisión de almacén tomada (DynamoDB) y registrada acá. El cache ya corre sobre
DynamoDB con TTL, y la misma tabla guarda las reservas de receipts, que no vencen.
**Por qué existe este documento:** el dueño aprobó DynamoDB el 2026-09-17 y preguntó cuánto
cuesta al mes, pero el ticket no existía en este repo. Este archivo es ese ticket: la decisión, el
estado medido y el costo, con la fórmula a la vista para rehacer la cuenta con tráfico real.

| Fecha | Item | Contexto | Prioridad | Estado |
|---|---|---|---|---|
| 2026-09-17 | **FAC-IDEMP-001 — el cache de `Idempotency-Key` de `/settle` se implementa sobre DynamoDB** | Decisión del dueño, 2026-09-17 19:23Z: «utilizar Dynamo para la implementación, ya que tenemos los costos». Medido el 2026-09-29 sobre `origin/main`: el store ya es DynamoDB con TTL (`src/idempotency_store.rs:17-28`; tabla `idempotency_records`, `PAY_PER_REQUEST`, TTL sobre `expires_at` a ~24 h, `terraform/environments/production/main.tf:759`). La tabla es compartida con las reservas de receipts, sin TTL. Costo on-demand en us-east-2: **US$0,75 por millón de settles con la cabecera** en las redes que no pasan por receipts; los settles `exact` de Base, Arc, Arc testnet y Hedera escriben por receipts con o sin cabecera, a unos **US$11–13 por millón**, y su almacenamiento crece cada mes (ver abajo). | P1 | Decisión registrada; el almacén ya está en producción. Lo que se construya encima se mide contra la estimación de este documento. |

---

## La decisión

El 2026-09-17 a las 19:23Z, después de ver los costos, el dueño eligió DynamoDB para la
implementación: «utilizar Dynamo para la implementación, ya que tenemos los costos». No hay otra
aprobación pendiente sobre el almacén.

## Estado medido (2026-09-29, `origin/main`)

- `src/idempotency_store.rs:17-28` documenta el esquema del cache: clave de partición
  `idempotency_key` (S), `request_hash` (S, SHA-256 en hex del cuerpo canónico), `response_json`
  (S, la respuesta de `/settle` a repetir) y `expires_at` (N, TTL en segundos Unix).
- El TTL es de 24 h (`IDEMPOTENCY_TTL_SECONDS`, `src/idempotency_store.rs:109`) y lo aplica la
  propia tabla sobre `expires_at`.
- La tabla es `aws_dynamodb_table.idempotency_store` (`terraform/environments/production/main.tf:759`):
  `billing_mode = "PAY_PER_REQUEST"` y TTL habilitado; no declara índices secundarios, PITR ni
  streams.
- **La tabla es compartida con receipts.** Con `IDEMPOTENCY_TABLE_NAME` definido, y producción lo
  define (`main.tf:969`), el facilitador arranca también el servicio de receipts sobre la misma
  tabla (`src/receipts/store.rs:329`, `src/receipts/mod.rs:429`). Sus filas llevan el prefijo
  `receipt:` en la clave y **no tienen TTL**, por diseño (`src/receipts/store.rs:1-2`): el registro
  y sus alias quedan para siempre.
- **Qué pasa por receipts:** los settles `exact` en Base, Arc, Arc testnet y Hedera
  (`src/receipts/mod.rs:211-213`). Cada uno hace una lectura consistente y una
  `TransactWriteItems`, traiga o no la cabecera `Idempotency-Key`, y el cache de 24 h no corre para
  ellos: el handler le quita la cabecera antes de llegar a él (`src/handlers.rs:4806-4809`). Los
  `/verify` `exact` de esas redes leen la tabla una vez y no escriben (`src/receipts/mod.rs:790`).
- El tráfico a DynamoDB sale por el endpoint de gateway `aws_vpc_endpoint.dynamodb`
  (`main.tf:307`): no cruza el NAT, así que no suma procesamiento de datos del NAT.

## Costo mensual estimado (on-demand, us-east-2)

La cuenta tiene dos términos, uno por cada uso de la tabla, y se suman.

### Precios usados

De la Price List API pública de AWS (oferta `AmazonDynamoDB`, región `us-east-2`), publicación
del **2026-09-11T12:44:22Z**, vigentes desde el **2026-08-01**, consultados el **2026-09-29**:

| Concepto | `usagetype` | Precio |
|---|---|---|
| Lectura on-demand | `USE2-ReadRequestUnits` | US$0,125 por millón de RRU |
| Escritura on-demand | `USE2-WriteRequestUnits` | US$0,625 por millón de WRU |
| Almacenamiento (clase Standard) | `USE2-TimedStorage-ByteHrs` | US$0,25 por GB-mes, después de 25 GB-mes gratis |

Los 25 GB-mes gratis son de la cuenta y los comparten todas sus tablas; en on-demand no hay
lecturas ni escrituras gratis (el tier gratuito de capacidad es solo para modo aprovisionado).

### Término 1 — cache de 24 h (settles que no pasan por receipts)

Solo los `/settle` con la cabecera `Idempotency-Key` en redes que no pasan por receipts tocan esta
parte de la tabla. Uno sin la cabecera en esas redes no la toca.

| Operación | Cuándo | Unidades |
|---|---|---|
| `GetItem` con `consistent_read(true)` (`src/idempotency_store.rs:246`) | en cada `/settle` con la cabecera, esté o no la clave | 1 RRU (ítem de hasta 4 KB) |
| `PutItem` (`src/idempotency_store.rs:321`) | solo si el settle termina con éxito | ⌈S / 1 KB⌉ WRU |
| Borrado por TTL | al vencer el ítem | 0 (los borrados por TTL no consumen escrituras) |

Un reintento con la misma clave suma 1 RRU y ninguna escritura.

**Tamaño del ítem, `S`.** Los nombres de atributo también pesan: `idempotency_key`,
`request_hash`, `response_json` y `expires_at` suman 50 bytes. A eso se agregan el hash (64 bytes),
el número del TTL (unos 6 bytes), la clave que elige el cliente (`K`; un UUID son 36) y la
respuesta (`R`): `S ≈ 120 + K + R` bytes. Una respuesta EVM de éxito sin extensiones ronda los
0,35 KB (es una estimación a partir del serializador de `SettleResponse` en `src/types.rs`, que
emite el hash con tres nombres; no es una medición). Con eso `S` queda por debajo de 1 KB y cada
escritura es **1 WRU**. Si las extensiones la llevaran a entre 1 y 2 KB serían 2 WRU; el código no
guarda respuestas de más de 16 KiB (`MAX_RESPONSE_JSON_BYTES`, `src/idempotency_store.rs:114`).

**Fórmula.** Con `N` = settles por día con `Idempotency-Key` en redes que no pasan por receipts,
`f` = fracción que termina con éxito (cota superior: 1), `W` = WRU por escritura, un mes de 30
días y `D` = días que un ítem sigue ocupando espacio (1 de TTL más la demora del borrado, que AWS
no garantiza y describe como de unos días; acá `D = 3`):

```
lecturas        = 30 × N × 1 RRU  × 0,125 / 1.000.000
escrituras      = 30 × N × f × W  × 0,625 / 1.000.000
almacenamiento  = N × D × (S + 100 B) / 2^30 GB × 0,25        (solo por encima de los 25 GB-mes gratis)
término 1       = lecturas + escrituras + almacenamiento
```

Los 100 bytes son la sobrecarga que DynamoDB suma a cada ítem al medir el almacenamiento.

**Escenarios.** `f = 1`, `S = 1 KB`, `D = 3`. Son escenarios de volumen, no tráfico medido: la
cuenta se rehace con el `N` real.

| Settles/día con la cabecera | Lecturas/mes | Escrituras/mes (W = 1) | **Total/mes (W = 1)** | Total/mes si W = 2 | Almacenamiento residente | Almacenamiento/mes fuera del tier gratis |
|---|---|---|---|---|---|---|
| 1.000 | US$0,00375 | US$0,01875 | **US$0,0225** | US$0,04125 | 0,0031 GB | US$0,0008 |
| 10.000 | US$0,0375 | US$0,1875 | **US$0,225** | US$0,4125 | 0,031 GB | US$0,0079 |
| 100.000 | US$0,375 | US$1,875 | **US$2,25** | US$4,125 | 0,31 GB | US$0,079 |

Lectura corta: **US$0,75 por millón de settles con la cabecera** (1 RRU + 1 WRU), y el
almacenamiento del cache es ruido al lado de eso: sus ítems vencen, así que no pasa de 0,31 GB
residentes ni con 100k settles al día.

### Término 2 — receipts (settles `exact` en Base, Arc, Arc testnet y Hedera)

Aplica a esos settles con o sin `Idempotency-Key`. Cota superior: todo settle pasa la verificación
y prepara una transacción (uno que no la pasa solo lee).

| Operación | Cuándo | Unidades |
|---|---|---|
| `GetItem` consistente del alias de la autorización (`src/receipts/mod.rs:1063`) | en cada settle | 1 RRU |
| `GetItem` consistente del cache de 24 h (`src/receipts/mod.rs:1076`) | solo si trae `Idempotency-Key` | 1 RRU |
| `TransactWriteItems` de la reserva (`src/receipts/mod.rs:1131`, `src/receipts/store.rs:386`) | cada settle que pasa la verificación | 2 × (⌈S_r⌉ + A) WRU: 2 WRU por KB y por ítem, el registro y `A` alias de menos de 1 KB |
| `PutItem` condicional al preparar la transacción (`src/receipts/mod.rs:1303`, `:1333`) | una vez por settle | ⌈S_r⌉ WRU |
| `PutItem` condicional al cerrar (`src/receipts/mod.rs:749`) | una vez por settle | ⌈S_r⌉ WRU |

`A` = alias del registro (`src/receipts/mod.rs:852`): 1 por la autorización, 1 más si el pedido
trae `X-UVD-Purchase` y 1 más si trae `Idempotency-Key`. Ninguno vence.

**Tamaño del registro, `S_r`.** Los ocho recibos firmados de
`tests/fixtures/facilitator-receipts-v1.json` pesan entre 2,7 y 3,1 KB en JSON compacto; la
prueba JWS lleva el recibo entero en base64 y es 1,7–1,9 KB de eso. El registro guarda el recibo
más la respuesta y, mientras la transacción está preparada, los datos de esa transacción: se
estima **⌈S_r⌉ = 4 KB**. Es una estimación a partir de los fixtures, no una medición de la tabla. Cada
alias ronda los 150 bytes.

**Fórmula.** Con `N_r` = settles `exact` por día en esas cuatro redes, `h` = 1 si traen la
cabecera (0 si no) y `V_r` = `/verify` `exact` por día en las mismas redes:

```
lecturas        = 30 × (N_r × (1 + h) + V_r) RRU × 0,125 / 1.000.000
escrituras      = 30 × N_r × (2 × (⌈S_r⌉ + A) + 2 × ⌈S_r⌉) WRU × 0,625 / 1.000.000
GB acumulados   = meses × 30 × N_r × (S_r + 100 B + A × 250 B) / 2^30
almacenamiento  = GB acumulados × 0,25        (solo por encima de los 25 GB-mes gratis, compartidos)
término 2       = lecturas + escrituras + almacenamiento
```

Por settle, con `⌈S_r⌉ = 4`: sin cabecera ni `X-UVD-Purchase` (`A = 1`) son 1 RRU y
2 × 5 + 8 = 18 WRU, **US$11,375 por millón**; con la cabecera (`A = 2`), 2 RRU y 20 WRU,
**US$12,75 por millón**. Cada settle deja unos 4.446 bytes (`A = 1`) que no vencen.

**Escenarios.** Los mismos volúmenes que el término 1, sin tráfico inventado. Almacenamiento con
`A = 1`, y el tier gratis como si fuera solo de esta tabla:

| Settles/día por receipts | Lecturas + escrituras/mes, sin cabecera | Con cabecera | Almacenamiento que suma cada mes | Acumulado a 12 meses | Almacenamiento/mes a los 12 meses, fuera del tier gratis |
|---|---|---|---|---|---|
| 1.000 | US$0,34 | US$0,38 | 0,124 GB | 1,49 GB | US$0 (el tier alcanza para unos 200 meses) |
| 10.000 | US$3,41 | US$3,83 | 1,242 GB | 14,91 GB | US$0 (se agota hacia el mes 20) |
| 100.000 | US$34,13 | US$38,25 | 12,422 GB | 149,06 GB | **US$31,02** (se agota hacia el mes 2) |

Lectura corta: en esas redes el settle cuesta unas **15 veces** lo que cuesta en el término 1
(US$11,375–12,75 contra US$0,75 por millón), y su almacenamiento es el único término de la tabla
que crece con el tiempo y no solo con el tráfico.

### Qué no entra

- Procesamiento de datos del NAT: no aplica, el tráfico va por el endpoint de gateway.
- PITR, streams, backups e índices secundarios: la configuración de la tabla no declara ninguno.
  Si se agregan, se suman aparte.
- Transferencia de datos: DynamoDB y ECS están en la misma región.
- El comando de operador de receipts (`src/receipts/admin.rs`) hace un `Scan` consistente de la
  tabla entera cada vez que se corre: 1 RRU por cada 4 KB leídos, aparte.

## Cómo se re-verifica

- Precios: `https://pricing.us-east-1.amazonaws.com/offers/v1.0/aws/AmazonDynamoDB/current/us-east-2/index.json`,
  filas `USE2-ReadRequestUnits`, `USE2-WriteRequestUnits` y `USE2-TimedStorage-ByteHrs`, con su
  `publicationDate`.
- Consumo real: `ConsumedReadCapacityUnits` y `ConsumedWriteCapacityUnits` de la tabla
  `idempotency_records` en CloudWatch **suman los dos términos** y no se separan por prefijo de
  clave. Para separarlos hace falta `N_r` de otra fuente (`/api/stats` por red, que no es un libro
  contable y puede perder filas, o la cadena): el término 2 se calcula con él y lo que queda de las
  series es el término 1.
- Tamaño real: `TableSizeBytes / ItemCount` de `DescribeTable` (DynamoDB actualiza esos dos valores
  cada unas seis horas) también mezcla los dos usos, y con el tiempo lo dominan los receipts: los
  ítems del cache vencen y los de receipts se acumulan. Para contarlos por separado, un `Scan` con
  `Select=COUNT` y filtro `begins_with(idempotency_key, "receipt:")`, que lee la tabla entera y se
  cobra (0,5 RRU por cada 4 KB en lectura eventual).
