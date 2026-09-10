---
date: 2026-09-10
tags:
  - type/handoff
  - domain/bazaar
  - domain/discovery
  - priority/p2
status: active
---

# El precio se vuelve a leer porque alguien lo miró, dentro del presupuesto que ya había

**Versión:** 2.24.0 · **Fase P2** del plan de precios de Astra 6 (sección 8) ·
PR 4 de 6. Sobre [P1](2026-09-10-bazar-precios-p1.md) (fechas y observaciones),
[A4](2026-09-10-a4-a5.md) (un propietario del trabajo periódico) y los dos
hotfixes de capacidad ([2.21.1](2026-09-10-hotfix-cpu.md),
[2.21.2](2026-09-10-hotfix-cpu-2.md)).

P1 dejó el catálogo capaz de decir *cuándo* se leyó un precio. Faltaba que se
volviera a leer por algo que no fuera el calendario.

## La regla que gobierna la fase

Hoy, entre las 16:41Z y las 20:49Z, este servicio estuvo caído porque el trabajo
de fondo creció con el catálogo. 2.21.2 lo devolvió a **120 sondeos por tick** y
la producción pasó a 1-3 % de CPU y 0,03 s de lectura.

Así que un refresco por demanda **no agrega un sondeo. Cambia cuál sondeo gasta
el tick con la asignación que ya tenía.**

```
budget = DISCOVERY_HEALTH_MAX_RPS * DISCOVERY_HEALTH_TICK   = 120
         ├── demanda   60 %  = 72
         └── barrido   40 %  = 48   (reservado, para que la cola larga no se muera de hambre)
```

Es una función, `split_budget`, con un test que afirma que las dos mitades suman
lo que había. No es una intención: es una aserción.

## La cola

**Deduplicación por almacenamiento, no por lock.** Con A4 el trabajo periódico
tiene un solo propietario, pero la demanda llega a la réplica que eligió el
balanceador. Una réplica que no es propietaria deja su pedido en un **conjunto de
strings en la tabla de leases** — misma tabla, mismo esquema de clave, mismo
statement de IAM — con `ADD`, que es idempotente: mil réplicas pidiendo el mismo
recurso mil veces producen una entrada. El propietario reclama un lote leyendo y
borrando **exactamente los valores que se llevó**, así que lo que se agregó entre
medio sobrevive. El ítem tiene TTL: una cola que nadie drena desaparece.

No inventé un tercer mecanismo. Es la tabla que ya está, con la semántica de
conjunto que ya tiene.

**Prioridad por lo que cuesta un precio equivocado**, no por lo reciente:

| razón | peso | por qué |
|---|---:|---|
| `purchase-intent` | 100 | alguien está por firmar; el error se paga en segundos |
| `owner-notified` | 60 | el dueño dice que se movió |
| `conflict` | 40 | una lectura vigente contradice al catálogo |
| `revision-changed` | 25 | el listado se revisó después de la lectura |
| `listing-stale` | 10 | se sirvió con una lectura vencida |
| `periodic` | 0 | el barrido habría llegado igual |

La demanda desempata y está topeada: **mil lectores son interés, no evidencia de
que el precio esté mal**, y un comprador les gana a todos.

**Cortesía.** Dos sondeos por host por tick desde la cola; el `Retry-After` del
origen gana sobre nuestro horario cuando lo manda; si no lo manda, exponencial
con jitter para que tres réplicas no vuelvan en el mismo instante. Un lote que se
toma **sale** de la cola: un origen roto para siempre no se queda al frente.

## Leer un listado vencido es lo que programa su refresco

Nadie tiene que pedirlo, y la respuesta nunca lo espera. Se compone con lo que se
sabe, se marca, y el trabajo lo hace el propietario dentro de su presupuesto.

El listado ahora dice **qué se está haciendo**, que es distinto de qué tan viejo
es lo que hay:

- `priceRevalidation: pending` — se está releyendo. **El importe de al lado es la
  lectura ANTERIOR.** Nunca se presenta la caché como actual.
- `not_verifiable` + `notVerifiableReason` — no se puede establecer por
  observación. `not-a-get-resource` (un MCP contesta un handshake, no un desafío
  de pago), `auth-gated`, `unprobeable` (plantilla de URL o dirección que el
  conector SSRF rechaza). Ninguno significa roto y ninguno significa gratis.
- `idle` — no hay nada pendiente.
- `observationExpiresAt` — cuándo deja de contar como fresca, para que un
  consumidor con caché propia revalide con el mismo reloj y no invente uno.

**El prober hace exactamente un tipo de solicitud: un `GET` sin autenticar de la
URL del listado.** Nunca un POST, nunca credenciales, nunca la operación
comercial real de un vendedor para ver cuánto cobra. Una compra que es un POST o
que lleva parámetros es otra solicitud y puede costar otra cosa: se informa como
no verificable en vez de adivinarla.

## Configuración: definida una vez, publicada

`src/discovery_config.rs` es ahora el único lugar donde existe un parámetro del
bazar. Antes eran diecisiete variables leídas en once archivos, cada una con su
default al lado, lo que tiene tres consecuencias que parecen chicas hasta un
incidente: el mismo parámetro puede leerse con dos defaults distintos y nadie lo
dice; no hay forma de preguntarle a una task *en marcha* qué resolvió; y quien
busca "qué gobierna la carga de fondo" tiene que grepear.

`GET /discovery/config` publica los valores resueltos y tres contadores vivos
(si esta réplica es propietaria, la profundidad de la cola, el catálogo que
sostiene). Público y sin autenticar a propósito: son números de tuning, no hay
nada ahí que no esté en el código, y el motivo de existir es poder leerlo durante
un incidente. **Ningún secreto puede definirse en ese registro**, así que ninguno
puede aparecer.

`POST /discovery/refresh` es la notificación autenticada de cambio de precio.
**Nunca acepta términos**: nombra un recurso, y el servidor vuelve al origen a
leer el desafío él mismo. Una notificación que pudiera escribir un precio sería
una forma de publicar el precio del endpoint ajeno mandándonos JSON. El recurso
tiene que estar ya en el catálogo, o sería una forma de hacernos buscar una
dirección arbitraria.

## Medición de fondo, antes de pushear

Contra el catálogo real de producción (`bazaar/resources.json`, 3,7 MB, 2.000
registros, leído 22:21Z), release, un escenario por proceso:

| | 2.21.2 | 2.24.0 |
|---|---:|---:|
| sondeos por tick | 120 | **120** |
| `probe_targets()` | 1,9 ms | 3,0 ms |
| trabajo nuevo de la cola por tick | — | `take_batch` 0,49 ms + `evict` 0,002 ms |
| encolar 600 pedidos | — | 1,0 ms |
| `list()` | 3,4 ms | 3,9 ms |
| residente | 53 MB | **98-100 MB** |

Lo que agrega esta fase a un tick es **~1,5 ms de CPU**, contra un tick que ya
gasta 120 handshakes TLS. El camino de lectura pública sube ~0,5 ms por la
anotación.

Los 98 MB residentes no son de esta fase: es el mismo proceso de prueba
sosteniendo el catálogo, la copia del `MemoryStore` y la página del feed a la vez.
El número comparable de 2.21.2 medido igual era 104 MB.

Y una confirmación al pasar: `import added=0 updated=0 skipped=1000` al
reimportar una página idéntica. Eso es el `content_fingerprint` de P1 haciendo lo
que dije que haría cuando avisé que 2.21.2 seguía escribiendo un snapshot por
ciclo.

## Para c0der

**Decisiones de la sección 14, tomadas acá.** Cada una con la suposición más
reversible:

- **Coordinación entre réplicas**: conjunto de strings en la tabla de leases, con
  `ADD` idempotente como deduplicación y TTL. Sin tabla nueva, sin índice, sin
  scan. *Reversible*: si la tabla no responde, la cola sigue funcionando dentro
  de cada réplica y sólo se pierde el pooling de demanda.
- **Identidad de contexto para la cola**: la URL, no un `(recurso, contexto)`
  compuesto. El prober produce **un** contexto — GET anónimo — así que una clave
  compuesta hoy tendría un solo valor posible y sería una estructura sin lector.
  `ObservationContext` de P1 ya guarda el contexto de cada lectura, que es donde
  la distinción se vuelve real cuando P3 traiga cotizaciones por parámetros.
- **Umbral de "vencido" que dispara demanda**: la misma ventana de frescura de P1
  (`DISCOVERY_TERMS_FRESH_SECS`, 7 días = la cadencia real de resondeo). Un
  segundo umbral habría sido un segundo lugar donde "fresco" significa algo.
- **Validadores `ETag`/`Last-Modified`**: **no implementados**, deliberadamente.
  El anexo advierte que un `304` de un documento no prueba nada sobre una
  cotización dinámica, y nuestro sondeo lee la **cabecera `PAYMENT-REQUIRED` de
  una respuesta 402**, que no es un documento cacheable y que ningún origen
  medido versiona. Habría sido complejidad que ahorra ancho de banda que no nos
  duele y que podría hacernos llamar "sin cambios" a un precio que cambió.
- **Notificación de cambio**: token de admin del bazar, o sea primera parte y
  operador. Abrirlo a dueños de recursos necesita credenciales por dueño, que es
  un diseño de gestión de claves y no un cambio de handler.
- **Rangos declarados contra observados**: sin cambios en esta fase. Sigue siendo
  P3.

**Qué NO trae esta fase, dicho en voz alta:**

- **Adaptadores por bazar (punto 9 de la sección 8).** Requieren APIs de terceros
  cuyo contrato el propio anexo pide revalidar antes de prometer nada, y la
  sección 14 lo lista como decisión abierta. Construí el mecanismo de refresco;
  no inventé adaptadores para APIs que no verifiqué. Medir propagación *por
  destino* necesita esos adaptadores; lo que sí se puede medir hoy es la
  propagación dentro de nuestro propio índice, que es lo que `termsObservedAt` y
  `observationExpiresAt` publican.
- **Política de precios del vendedor (punto 8).** Es trabajo de `x402-axum` y del
  contrato de P3; meterlo acá habría mezclado el índice con el vendedor.
- **Métricas de la sección 12.** Los estados están calculados y logueados con
  vocabulario acotado, no instrumentados en un exportador. El denominador honesto
  ya existe.

**Verificación después del deploy:**

| qué | dónde | esperado |
|---|---|---|
| carga de fondo | CPU max | igual que 2.21.2: sin ráfagas, 1-3 % |
| el presupuesto | `GET /discovery/config` | `budgetPerTick: 120` |
| la cola | mismo endpoint, `runtime.revalidationQueueDepth` | acotada, < 500 |
| un solo trabajo por ventana | logs, `queued stale listings for revalidation` | `accepted` mucho menor que `seen` |
| cortesía | logs, `holding off a host that refused` | aparece con 429, no en régimen |
| propietario | `runtime.ownsPeriodicJobs` | `true` en exactamente una réplica |

Kill-switch sin deploy: `DISCOVERY_ENABLE_REVALIDATION=false` deja el prober con
su barrido periódico, que es 2.21.2 exactamente.

**Qué queda para P3:** la política del comprador y las cotizaciones vigentes del
vendedor (sección 9 del anexo), que es donde `ObservationContext` deja de tener
un solo valor y donde `offer-receipt` entra.
