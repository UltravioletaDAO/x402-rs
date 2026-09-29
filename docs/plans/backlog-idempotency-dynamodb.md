# Backlog — cache de `Idempotency-Key` de `/settle` sobre DynamoDB (FAC-IDEMP-001)

**Estado:** decisión de almacén tomada (DynamoDB) y registrada acá. El cache ya corre sobre
DynamoDB con TTL.
**Por qué existe este documento:** el dueño aprobó DynamoDB el 2026-09-17 y preguntó cuánto
cuesta al mes, pero el ticket no existía en este repo. Este archivo es ese ticket: la decisión, el
estado medido y el costo, con la fórmula a la vista para rehacer la cuenta con tráfico real.

| Fecha | Item | Contexto | Prioridad | Estado |
|---|---|---|---|---|
| 2026-09-17 | **FAC-IDEMP-001 — el cache de `Idempotency-Key` de `/settle` se implementa sobre DynamoDB** | Decisión del dueño, 2026-09-17 19:23Z: «utilizar Dynamo para la implementación, ya que tenemos los costos». Medido el 2026-09-29 sobre `origin/main`: el store ya es DynamoDB con TTL (`src/idempotency_store.rs:17-28`; tabla `idempotency_records`, `PAY_PER_REQUEST`, TTL sobre `expires_at` a ~24 h, `terraform/environments/production/main.tf:759`). Costo on-demand en us-east-2: **US$0,75 por millón de settles con la cabecera**, más un almacenamiento que no llega a medio GB ni con 100k settles al día (ver abajo). | P1 | Decisión registrada; el almacén ya está en producción. Lo que se construya encima se mide contra la estimación de este documento. |

---

## La decisión

El 2026-09-17 a las 19:23Z, después de ver los costos, el dueño eligió DynamoDB para la
implementación: «utilizar Dynamo para la implementación, ya que tenemos los costos». No hay otra
aprobación pendiente sobre el almacén.

## Estado medido (2026-09-29, `origin/main`)

- `src/idempotency_store.rs:17-28` documenta el esquema: clave de partición `idempotency_key` (S),
  `request_hash` (S, SHA-256 en hex del cuerpo canónico), `response_json` (S, la respuesta de
  `/settle` a repetir) y `expires_at` (N, TTL en segundos Unix).
- El TTL es de 24 h (`IDEMPOTENCY_TTL_SECONDS`, `src/idempotency_store.rs:109`) y lo aplica la
  propia tabla sobre `expires_at`.
- La tabla es `aws_dynamodb_table.idempotency_store` (`terraform/environments/production/main.tf:759`):
  `billing_mode = "PAY_PER_REQUEST"` y TTL habilitado; no declara índices secundarios, PITR ni
  streams.
- El tráfico a DynamoDB sale por el endpoint de gateway `aws_vpc_endpoint.dynamodb`
  (`main.tf:307`): no cruza el NAT, así que no suma procesamiento de datos del NAT.

## Costo mensual estimado (on-demand, us-east-2)

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

### Qué consume cada `/settle`

Solo los `/settle` que traen la cabecera `Idempotency-Key` tocan la tabla. Uno sin la cabecera
cuesta cero aquí.

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

### Fórmula

Con `N` = settles por día con `Idempotency-Key`, `f` = fracción que termina con éxito (cota
superior: 1), `W` = WRU por escritura, un mes de 30 días y `D` = días que un ítem sigue ocupando
espacio (1 de TTL más la demora del borrado, que AWS no garantiza y describe como de unos días;
acá `D = 3`):

```
lecturas        = 30 × N × 1 RRU  × 0,125 / 1.000.000
escrituras      = 30 × N × f × W  × 0,625 / 1.000.000
almacenamiento  = N × D × (S + 100 B) / 2^30 GB × 0,25        (solo por encima de los 25 GB-mes gratis)
total           = lecturas + escrituras + almacenamiento
```

Los 100 bytes son la sobrecarga que DynamoDB suma a cada ítem al medir el almacenamiento.

### Escenarios

`f = 1`, `S = 1 KB`, `D = 3`. Son escenarios de volumen, no tráfico medido: la cuenta se rehace
con el `N` real.

| Settles/día con la cabecera | Lecturas/mes | Escrituras/mes (W = 1) | **Total/mes (W = 1)** | Total/mes si W = 2 | Almacenamiento residente | Almacenamiento/mes fuera del tier gratis |
|---|---|---|---|---|---|---|
| 1.000 | US$0,00375 | US$0,01875 | **US$0,0225** | US$0,04125 | 0,0031 GB | US$0,0008 |
| 10.000 | US$0,0375 | US$0,1875 | **US$0,225** | US$0,4125 | 0,031 GB | US$0,0079 |
| 100.000 | US$0,375 | US$1,875 | **US$2,25** | US$4,125 | 0,31 GB | US$0,079 |

Lectura corta: **US$0,75 por millón de settles con la cabecera** (1 RRU + 1 WRU), y el
almacenamiento es ruido al lado de eso, dentro del tier gratuito en los tres casos.

### Qué no entra

- Procesamiento de datos del NAT: no aplica, el tráfico va por el endpoint de gateway.
- PITR, streams, backups e índices secundarios: la configuración de la tabla no declara ninguno.
  Si se agregan, se suman aparte.
- Transferencia de datos: DynamoDB y ECS están en la misma región.

## Cómo se re-verifica

- Precios: `https://pricing.us-east-1.amazonaws.com/offers/v1.0/aws/AmazonDynamoDB/current/us-east-2/index.json`,
  filas `USE2-ReadRequestUnits`, `USE2-WriteRequestUnits` y `USE2-TimedStorage-ByteHrs`, con su
  `publicationDate`.
- Consumo real: en CloudWatch, `ConsumedReadCapacityUnits` de la tabla `idempotency_records`
  cuenta las lecturas (`N` más los reintentos) y `ConsumedWriteCapacityUnits` cuenta
  `N × f × W`. Con esas dos series la cuenta deja de depender de escenarios.
- Tamaño real: `TableSizeBytes / ItemCount` de `DescribeTable` da el tamaño medio del ítem
  (DynamoDB actualiza esos dos valores cada unas seis horas).
