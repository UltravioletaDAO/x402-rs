---
date: 2026-09-10
tags:
  - type/handoff
  - domain/bazaar
  - domain/discovery
  - type/incident
  - priority/p0
status: active
---

# El catálogo creció 39x en veinte minutos y nadie le había puesto un techo

**Versión:** 2.21.1 · hotfix sobre 2.21.0 · regresión viva entre 16:34Z y 18:42Z
del 2026-09-10, mitigada con rollback a 2.19.0.

Nada se rompió. Un arreglo funcionó demasiado bien.

## Lo que pasó

P0 de precios (#35, 2.20.0) arregló el parser del feed de Coinbase CDP, que
venía fallando **entero** por el alias de `maxAmountRequired`. El feed empezó a
entrar. Se ve en un solo minuto de logs, con dos tasks todavía en 2.19.0 y una ya
en 2.20.0:

| hora | `Total resources aggregated` | imagen |
|---|---:|---|
| 16:48:20Z | **752** | 2.19.0 |
| 16:48:23Z | **752** | 2.19.0 |
| 16:48:27Z | **29 133** | 2.20.0 |
| 17:06:50Z | **43 410** | 2.20.0 |

**39x**, y el handoff de P0 lo había anticipado ("14 235 publicados contra 335
nuestros"). Lo que no se dimensionó fue lo que hay del otro lado del import.

El catálogo en S3 pasó de **14,5 MB a 98,5 MB** entre las 16:48Z y las 17:06Z:
**39 593 registros**. Y todas las operaciones de este servicio son sobre el
catálogo *entero*.

Medido en release contra ese objeto exacto:

| | |
|---|---:|
| registros | 39 593 |
| objeto en S3 (pretty) | 98 MB |
| **residente al parsearlo** | **552 MB** |
| por registro | 13,4 KB |
| deserializar | 317 ms |
| clonarlo entero | 102 ms |
| `to_vec_pretty` | 143 ms (98 MB) |
| `to_vec` compacto | 93 ms (54 MB) |

La task tiene **2 GiB y una vCPU**. El pico de un ciclo de import, sumado:

```
  552 MB   el catálogo residente
+ 552 MB   la copia que bulk_import clona para publicarla
+  98 MB   el cuerpo que save_all descarga...
+ 552 MB   ...y parsea entero, para leer un ETag, y tira
+  98 MB   el pretty-print del snapshot
─────────
  1,85 GiB   de 2 GiB
```

Eso es el 76-80 % que midió CloudWatch. No hay una línea con un bug: hay una
decisión de dimensionamiento que nadie tomó, porque hasta las 16:48Z de hoy la
fuente más grande contestaba 752 recursos y el número era teórico.

**Por qué las escrituras (12-25 s) sufrieron más que las lecturas (2,6-4,1 s).**
Toda liquidación EVM se reenvía al único titular del lease de escritura. Las
lecturas se reparten entre tres tasks; las escrituras van todas a una. Si esa
task es la que está en medio de su ciclo de import con el runtime saturado en su
única vCPU, cada escritura espera detrás. Por eso el camino del dinero se veía
5x peor con el mismo trabajo de fondo.

## El arreglo

Cuatro cambios. Ninguno toca `/verify`, `/settle`, escrow, ERC-8004 ni el
camino de pago.

**1. Un techo al catálogo, por procedencia.** `DISCOVERY_MAX_RESOURCES`, 20 000
por defecto — ~270 MB residentes de los 2 GiB, medido, no estimado. Se aplica
**al cargar** y **después de cada import**.

El orden de desalojo es procedencia antes que antigüedad, y ahí está todo el
argumento: una copia agregada es por construcción copia de algo que sigue
publicado en otro lado, y perderla cuesta un re-fetch. Un recurso que alguien
registró con nosotros, o por el que vimos liquidar un pago, o que leímos del
documento del propio origen, es de primera mano y no se recupera preguntándole a
un tercero. **Nunca se desaloja uno de esos**, ni aunque el catálogo quede por
encima del techo: pasarse de capacidad es un problema con una respuesta conocida
(una task más grande), y borrar la única copia del listado de un vendedor para
quedar debajo de un número no es esa respuesta.

Dentro de las agregadas, sale primero la de `last_updated` más viejo: es nuestro
reloj de escritura, así que "la que hace más que no tocamos" es la que menos se
va a notar.

**2. `save_all` deja de descargar el catálogo para leer un ETag.** Una escritura
condicional necesita la versión y nada más — está por reemplazar todos los bytes.
La sacaba de `load_snapshot`, que baja y parsea el objeto entero y tira el
resultado: 98 MB por el cable y 552 MB de estructuras, por ciclo, por task,
descartados en el acto. Ahora es un `HeadObject`. Es el mismo ETag, así que es la
misma garantía.

**3. El snapshot se escribe compacto.** Es estado de máquina, no lo lee nadie a
ojo. En el objeto real la indentación eran **44 MB de los 98**: producidos,
subidos, versionados en S3 y vueltos a bajar por cada task que arranca.

**4. Un techo a la entrada por fuente.** `DISCOVERY_MAX_ITEMS_PER_SOURCE`, 20 000
(era una constante de 50 000). Acota el pico **durante** el ciclo — los 43 410
items convertidos se acumulan en un solo `Vec` antes de que el import empiece —
que es un costo distinto del que acota el techo del catálogo.

### Y un hallazgo del camino

El prober ahora poda `bazaar/health.json` contra el catálogo vivo, y eso
introducía un agujero que **hay que decir en voz alta**: cuando la lectura de S3
falla al arrancar, `main` no se cae, cae a un registro **vacío** en memoria y
sigue sirviendo. Ese prober habría ofrecido un conjunto vacío y la poda habría
borrado el overlay entero — el historial de liveness de todos los recursos y los
acumulados sobre los que se construye la atestación de uptime — por un GET que
falló. Es la misma clase de defecto que A3 arregló en el catálogo. Hay guarda y
hay test: **con el catálogo vacío no se poda nada.**

## Medido después

Contra el mismo objeto de 39 593 registros, con el techo en 20 000:

| | antes | después |
|---|---:|---:|
| registros retenidos | 39 593 | 20 000 |
| de ellos auto-registrados | 558 | **558** (ninguno desalojado) |
| residente | 552 MB | ~280 MB |
| objeto en S3 | 98 MB | **44 MB** |
| pico de un import | ~1,85 GiB (**92 %**) | ~604 MB (**29,5 %**) |

## Tests

15 nuevos: 6 de capacidad en `discovery.rs`, 3 del store en `discovery_store.rs`,
3 del overlay en `discovery_health.rs`, y los de guarda. **Probados en rojo**:
restaurando a mano las cuatro semánticas viejas caen siete:

```
discovery::tests::a_cap_never_evicts_a_first_hand_record
discovery::tests::a_snapshot_written_before_the_cap_is_trimmed_on_the_way_in
discovery::tests::an_import_cannot_grow_the_catalog_past_the_cap
discovery::tests::over_capacity_the_oldest_aggregated_copies_go_first
discovery_health::tests::the_overlay_drops_records_for_resources_that_left_the_catalog
discovery_store::tests::publishing_a_snapshot_reads_the_version_not_the_catalog
discovery_store::tests::the_snapshot_is_written_compact
```

## Para c0der

**Las dos preguntas que hiciste, contestadas derecho.**

**1. ¿Restauro la versión chica del catálogo en S3 antes del deploy?**
**No hace falta, y te recomiendo no hacerlo.** El hotfix tolera y poda el objeto
gordo por código:

- El primer arranque después del deploy sí baja y parsea los 98 MB: pico puntual
  de ~650 MB, **32 % de los 2 GiB** — muy por debajo del 76-80 % que estabas
  viendo, y una sola vez.
- Acto seguido lo poda a 20 000 en memoria, conservando **los 558
  auto-registrados** y las 19 442 copias agregadas más recientes.
- El primer snapshot que escriba ya sale podado y compacto: **44 MB**.

Restaurar la versión de 16:48:24Z también funcionaría, pero tira ~9 000 recursos
que llegaron legítimamente después y que el ciclo de agregación va a volver a
traer dentro de la hora. No compra nada que el código no haga solo, y agrega un
paso manual con S3 al medio de un incidente.

**Lo único que sí conviene mirar después:** el bucket está versionado, así que la
versión de 98 MB queda como no-actual y sigue ocupando hasta que la expire el
lifecycle. Es costo, no corrección.

**2. ¿Y `bazaar/health.json` (9,8 MB)?**
**También lo poda el hotfix, sin tocar nada a mano.** Nunca se había podado:
nada borraba un registro cuando su recurso salía del catálogo, y el objeto se
sube **entero cada 60 segundos**, así que eran 9,8 MB por minuto de subida por
recursos que ya no existen. Ahora el prober lo poda contra el catálogo vivo en
cada tick: después de que el catálogo baje a 20 000, el primer tick tira los
~19 593 huérfanos y el overlay cae más o menos en proporción, a ~5 MB.

Borrar el objeto a mano también es seguro (el tracker lo reconstruye sondeando),
pero **no es necesario** y perderías los acumulados de uptime.

**Cómo verificar después del deploy** (CloudWatch, us-east-2,
cluster/servicio `facilitator-production`):

| qué | dónde | esperado |
|---|---|---|
| memoria por task | `MemoryUtilization`, ECS | pico de arranque < 35 %, régimen **< 30 %** |
| CPU | `CPUUtilization`, max | **< 50 %** |
| escrituras | target group `facilitator-production-writes`, p95 | **< 1 s** |
| lecturas | target group `facilitator-production`, p95 | vuelta a ~0,2 s |
| el objeto | `aws s3api head-object --bucket facilitator-discovery-prod --key bazaar/resources.json` | `ContentLength` ~44 MB (tarda un ciclo) |
| la poda | logs, `catalog loaded over capacity` y `dropped health records` | una vez por task al arrancar |
| la entrada | logs, `source truncated at the per-source cap` | si aparece, el feed tiene más de lo que traemos: decisión de producto |

El objeto de S3 no se achica en el instante del deploy: la poda en memoria es
inmediata, pero llega a S3 en el primer ciclo de import que cambie algo (dentro
de la hora; los feeds cambian todo el tiempo). Si querés forzarlo, un
`force-new-deployment` no alcanza — hay que esperar el ciclo.

**Qué le falta a los otros PRs por esto:**

- **#38 (A4, "las tres réplicas hacían el mismo trabajo de discovery entero")**
  es el complemento natural y ataca el otro factor de tres: hoy las tres tasks
  hacen el mismo ciclo completo. Este hotfix baja el costo por task; #38 baja la
  cantidad de tasks que lo pagan. **Va a chocar** en `discovery.rs` (import) y en
  `discovery_health.rs` (el tick del prober, donde metí la poda del overlay). El
  encargo dice que el hotfix gana y #38 rebasea: al rebasear, conservar la poda
  del overlay **con su guarda de catálogo vacío**, que es justamente el caso que
  un lease hace más probable (una réplica sin lease no importa, pero una que
  arranca con la lectura fallada sí).
- **#40 (precios P1)** no choca con esto en semántica, pero sí en archivos
  (`discovery.rs`, `discovery_health.rs`, `discovery_store.rs`). Dos cosas a
  cuidar al rebasear: el overlay de términos observados que agrega P1 es **otro**
  objeto que se escribe entero, y ya nace con su propio tope
  (`DISCOVERY_TERMS_MAX_RECORDS`, 20 000) y su propio debounce (300 s) — mismo
  patrón que este hotfix, por la misma razón. Y el `content_fingerprint()` de P1
  hace que un feed sin cambios no genere escritura, lo que reduce todavía más los
  ciclos que publican snapshot.
- **Lo que este hotfix NO arregla y hay que mirar con calma:** `list()` recorre
  el catálogo entero por request (el worker del benchmark midió p50 de 457 ms
  contra 52 ms). Bajar a 20 000 registros lo mejora proporcionalmente, pero sigue
  siendo O(catálogo) por request en una ruta pública. Un índice es trabajo de P2,
  no de un hotfix.
- **La constante de 20 000 es la provisión, no una preferencia.** Si la task
  sube a 4 GiB, el techo puede subir con ella. Al revés también.
