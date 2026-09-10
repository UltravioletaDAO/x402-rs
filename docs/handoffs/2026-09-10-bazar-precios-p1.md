---
date: 2026-09-10
tags:
  - type/handoff
  - domain/bazaar
  - domain/discovery
  - priority/p1
status: active
---

# Un cobro no es un precio, y una copia no es una lectura

**Versión:** 2.22.0 · **Fase P1** del plan de precios dinámicos
(handoff de Astra 6, 2026-09-09, base `9893c48f`) · **PR 2 de 6** de la
secuencia de ese plan. Continúa
[precios P0](2026-09-10-bazar-precios-p0.md), que arregló *qué* se guarda;
esto arregla *cuándo se supo* y *quién lo dijo*.

El catálogo tenía cuatro maneras de parecer más actual de lo que era. Tres
estaban en el código y una era una ausencia:

1. **Un pago rejuvenecía el contenido.** `track_settlement` movía
   `last_updated` — el campo por el que se ordena el listado, el que un lector
   usa para juzgar la antigüedad, y el que `merge_resource` usa para rechazar
   una escritura fuera de orden. Que alguien pagara movía los tres, y un pago no
   dice nada sobre si esos términos siguen en pie (F7).
2. **Una copia ajena le ganaba a la declaración del dueño, por una fecha que
   el dueño no tenía.** Los 24 636 registros guardados antes de P0 no llevan
   `sourceUpdatedAt`: **0 de 24 636**. La regla de P0 era «una fecha le gana a
   ninguna fecha», así que *cualquier* entrada fechada de un feed pisaba
   *cualquier* listado auto-registrado. La fecha era del que copiaba.
3. **Reimportar el mismo feed contaba como un cambio.** Sin hash de contenido,
   `(None, None)` dejaba entrar al último y se reescribía un snapshot de 15 MB
   por ciclo para decir lo mismo.
4. **Nadie guardaba lo que el origen responde.** El comprobador de salud lee un
   402 vivo en cada sondeo y se quedaba **sólo con los destinatarios**. Un
   recurso podía estar marcado vivo y seguir anunciando un precio de hace meses
   (F4).

## Cuatro fechas, cuatro preguntas

| Campo | Responde | Reloj |
|---|---|---|
| `lastUpdated` | cuándo escribimos nosotros el registro | nuestro |
| `sourceUpdatedAt` | la fecha que la fuente declaró para el contenido | ajeno |
| `lastSettledAt` | cuándo liquidó por acá el último pago | nuestro, observado |
| `termsObservedAt` | cuándo leímos el desafío de pago en vivo del origen | nuestro, observado |

Ninguna es intercambiable con otra y ninguna prueba por sí sola que un precio
esté vigente. `lastSettledAt` es **actividad**: alguien pagó, lo cual no dice si
esos términos siguen ofreciéndose, y en `upto` el importe que liquidó está
legítimamente por debajo del techo autorizado. La única que habla del precio es
`termsObservedAt`, y sólo para el contexto de solicitud que está guardado al
lado.

## El overlay de términos observados

`probed_accepts` / `probed_accepts_at` del diseño previo
([03-health-checker.md](../plans/bazaar/03-health-checker.md)), completado con
contexto, fase y procedencia, y puesto **en su propio objeto**:
`bazaar/terms.json`, junto al de salud.

Que sea un objeto aparte no es prolijidad, es la regla:

- **Un solo escritor.** El comprobador es el único componente que ve un 402, así
  que es el único que escribe acá. No hace falta lease ni coordinación con el
  agregador.
- **Estructuralmente inalcanzable para una importación.** Un import escribe
  `bazaar/resources.json`. «Un feed atrasado no borra una observación directa»
  deja de ser una regla que hay que hacer cumplir y pasa a ser una forma que no
  se puede expresar.
- **Sobrevive un rollback.** Una imagen anterior a este PR ni lo lee ni lo
  escribe, así que volver atrás deja las observaciones intactas. Inline habrían
  desaparecido en el primer snapshot que escribiera la imagen vieja.

Cada registro lleva: los `accepts` completos que anunció el origen, la fecha, el
**contexto** (`GET`, tipo de recurso, sin autenticar), la **fase**
(`verification` para un desafío, donde un importe `upto` es el techo;
`settlement` para un pago hecho, donde puede ser el cobro), la **procedencia**,
el **transporte**, la versión de protocolo, el estado HTTP, las causas de las
opciones que no se pudieron leer, y el **hash del contenido del registro tal como
estaba** cuando se hizo la lectura.

Ese último campo es el que distingue «observado contra estos mismos términos» de
«observado, y el listado se revisó desde entonces» — que es la diferencia entre
un precio fresco y uno que toca revalidar.

## `LiveTerms` lee el desafío entero

Y de los dos transportes. Cuando la cabecera `PAYMENT-REQUIRED` y el cuerpo
declaran cosas distintas **no se mezclan**: gana la versión de protocolo más
alta, la cabecera desempata (es donde los vendedores reales lo ponen: 36 de 36
medidos el 2026-08-20), y la lectura perdedora se conserva entera en `conflict`.
Un registro con la red de una cabecera y el importe de un cuerpo describe una
oferta que nadie emitió, y después es indistinguible de una real.

Los requisitos pasan por `normalize_declared_option`, la misma regla única que
usan el agregador, el crawler y `POST /discovery/register`. Una observación con
sus propias reglas de parseo no sería comparable con nada — y tendría permiso
para inventar el `exact` que P0 sacó.

`pay_to` sigue siendo la **unión** de los dos transportes a propósito: un
destinatario declarado en cualquier parte de la respuesta es un destinatario
declarado, y la comprobación de secuestro tiene que verlo aunque los términos
finalmente se tomen del otro lado.

**Un cambio de precio no dispara la cuarentena.** El importe no entra en
`pay_to_drifted` y no debe entrar nunca: repreciar es comercio normal y
esconder un recurso vivo por eso sería peor que el problema. El cambio de precio
se registra como observación, donde un lector lo ve y decide.

## `priceFreshness`, y por qué el orden de las preguntas es el comportamiento

`fresh` / `stale` / `unknown` / `conflict`, **independiente de `health`**. Un
endpoint puede responder 402 perfectamente sin que nadie haya leído nunca cuánto
cobra, y uno en cuarentena puede tener un precio leído hace una hora.

1. ¿La lectura no dejó ninguna opción legible? → `unknown`. Fechamos la mirada,
   no el precio. Tratar una lectura vacía como acuerdo sería el mismo error que
   una comprobación de secuestro que pasa porque no vio nada.
2. ¿La lectura venció la ventana? → `stale`. No es creencia actual, así que no
   afirma un conflicto sobre evidencia que nadie volvió a mirar.
3. ¿El **origen** revisó el listado después de la lectura? → `stale`. Un
   vendedor repreciando no puede publicarse como contradicción durante los días
   que tarda el siguiente sondeo.
4. ¿Discrepan? → `conflict`, con los dos lados servidos. Acá llega el caso en
   que se movió *la copia de un tercero*: que un agregador cambie su copia no es
   el vendedor hablando, y una lectura directa le gana a una copia en la
   escalera.
5. Si no, `fresh` — por exactamente lo que es: una lectura pasada de un contexto
   de solicitud, y nunca una cotización.

Dos opciones son **comparables** sólo si coinciden esquema, red, activo y
`payTo`. El mismo número en otra moneda, en otra cadena o a otro destinatario no
es la misma oferta comercial, así que nunca se reporta como cambio de precio.

Ventana por defecto: `DISCOVERY_TERMS_FRESH_SECS`, 7 días = la cadencia real de
resondeo de un recurso sano. Frescura tiene que significar «observado dentro de
la política que de verdad corremos», no una aspiración. Sondear cada siete días
y después llamar rancio a todo lo de más de un día marcaría el catálogo entero
como rancio el primer día y no diría nada.

## La escalera de procedencia en el merge

`import_supersedes` era una comparación de fechas. Ahora son tres preguntas, en
este orden:

1. **¿Mismo contenido y misma fecha declarada?** No pasó nada. `Unchanged`, sin
   escritura. El hash cubre url, tipo, descripción, metadata, extensions y cada
   opción de pago declarada — y **nada más**: ninguna fecha, ni `firstSeen`, ni
   `settlementCount`, ni ningún campo de respuesta. Si entrara cualquiera de
   esos, un feed sin cambios hashearía distinto en cada ciclo y la comprobación
   no serviría para nada.
2. **Autoridades distintas no se ordenan por reloj.** Observación directa >
   declaración del dueño > documento propio del origen > copia agregada. Un feed
   que estampa su copia con la fecha de hoy escribió una fecha, no aprendió un
   precio.
3. **Dentro de una autoridad, la fecha que la fuente declara ordena las
   versiones.** Y cuando no hay fecha que las separe, decide **el publicador**:
   si mandó las dos, es ese publicador revisando su propia entrada sin mover su
   propio reloj, y es la autoridad sobre su listado. Si son dos publicadores
   distintos del mismo escalón sin fecha entre medio, se queda el que está —
   quedarse con el último descargado haría que el registro se dé vuelta en cada
   ciclo, en orden de crawl, para siempre.

## El formato persistido, versionado

`recordVersion` en cada registro. **1** es todo lo escrito antes de este modelo
de fechas: ahí un `lastSettledAt` ausente significa *nunca lo registramos*, no
*nunca pasó*. **2** lleva las fechas separadas.

El objeto sigue siendo un array JSON pelado, y eso es deliberado. La lectura es
`from_slice::<Vec<DiscoveryResource>>` y es todo-o-nada: **un** registro que no
parsee tumba el catálogo entero al arrancar. Envolver el array en
`{"version": N, "resources": [...]}` le haría exactamente eso a toda imagen
anterior al envoltorio — el rollback encontraría un objeto donde espera un array
y no cargaría nada. Un marcador de versión cuya introducción rompe la versión de
la que protege no es un marcador de versión.

Hacia atrás y hacia adelante funciona por construcción: los campos nuevos son
opcionales con default, y `serde` ignora los desconocidos. Lo que una imagen
vieja **no** puede es *preservar* lo que no conoce — el siguiente snapshot que
escriba tira los campos que ignoró. Por eso las observaciones viven en su propio
objeto.

Al migrar **no se inventa ninguna fecha**. Un registro v1 se lee como está.

`format_census()` cuenta registros por versión y se loguea en cada carga: es el
denominador de la migración, y lo que distingue «el formato nuevo está
desplegado» de «el catálogo está reescrito en él», que un deploy solo no conecta.

## API y UI

`GET /discovery/resources` (y con él el detalle de la página, que se compone del
mismo item) suma `lastSettledAt`, `recordVersion`, `contentHash`,
`priceFreshness`, `termsObservedAt` y `observedTerms`. Los últimos cuatro son
**sólo de respuesta**, con la misma disciplina que `health` y `curation`: se
resuelven al componer el listado y no se guardan nunca, así que un registrante no
puede afirmar que su propio precio es fresco y un registro guardado no puede
seguir afirmando una frescura que nadie volvió a comprobar. Hay test de las dos
cosas.

`openapi.rs` documenta las cuatro fechas, los cuatro estados de frescura, la
regla de comparabilidad y el contrato de `observedTerms`.

En `bazaar.html`, sin rediseñar nada: una etiqueta más en la tarjeta
(«observado hace 2h» / «leído hace 9d» / «el precio no coincide» / «precio no
verificado») con su explicación al pasar el mouse, y en el detalle una sección
«Lo que respondió el endpoint» con el contexto de la lectura, sus opciones, la
nota de conflicto entre transportes si la hubo, y la fecha de la última
liquidación etiquetada como actividad. EN/ES completos: 120 claves cada
diccionario, verificado.

## Tests

41 nuevos: 8 en `discovery.rs`, 9 en `discovery_health.rs`, 4 en
`discovery_store.rs`, 14 en `discovery_terms.rs` y 6 en
`tests/bazaar_freshness.rs`, que asserta sobre el **listado serializado** porque
los nombres de campo son el contrato que van a leer el SDK, la página y el
scheduler de P2.

**Probados en rojo.** Restaurando a mano la semántica vieja — el pago moviendo
`last_updated` y `import_verdict` decidiendo sólo por fechas — caen seis:

```
discovery::tests::a_settlement_is_activity_and_does_not_rejuvenate_the_price
discovery::tests::an_aggregated_copy_never_outranks_the_owners_own_declaration
discovery::tests::reimporting_an_identical_feed_is_not_a_change
discovery::tests::the_provenance_ladder_decides_when_neither_side_is_dated
discovery::tests::two_undated_feeds_of_equal_rank_do_not_flip_the_record_every_cycle
bazaar_freshness::a_settlement_moves_its_own_date_and_only_its_own_date
```

Los otros 35 no tienen versión roja que correr: prueban capacidades que antes no
existían (`LiveTerms` no tenía dónde guardar unos requisitos, y no había overlay
que leer). Se dice acá en vez de contarlos como rojos.

Un fallo del primer intento que vale anotar: el test de liquidación pasaba en
rojo por accidente, porque el registro y el pago caían en el mismo segundo y
`lastUpdated` era el mismo número por las dos razones. Ahora fija una fecha de
escritura vieja explícita.

## El rebase sobre A5

Llegué segundo. A5 (`#37`, «el reenvío al holder abría una conexión nueva por
escritura») aterrizó en `main` como **2.21.0** mientras esto estaba en curso, y
esta rama se rebaseó encima. El único choque fue `VERSION`: 2.20.0 → 2.21.0 de
ellos contra 2.20.0 → 2.22.0 de esto, resuelto a **2.22.0**, que es la de ellos
más una minor. Los dos cambios quedan.

No hubo choque de código: A5 toca `handlers.rs` (el cliente HTTP del forwarding)
y esto no lo toca. El scheduler de `discovery_health.rs` sí es territorio
compartido y el reparto se respetó — este PR no cambia quién hace el trabajo
periódico ni quién escribe el snapshot, sólo qué se lee del 402 y dónde se
guarda; en `start_health_task` son un puñado de líneas dentro del cierre por
sonda y una llamada de `persist` al final del tick, y ninguna firma cambia.
`probe_targets()` se dejó igual a propósito, aunque la observación necesita el
hash del registro: se resuelve con un `registry.get()` dentro de la tarea ya
lanzada, que cuesta un lock de lectura y no cambia una firma que la otra rama
podía estar tocando.

Los cuatro pasos de CI corrieron verdes **después** del rebase, no antes.

## Y un segundo rebase, sobre el hotfix 2.21.1

Entre medio entró el hotfix de capacidad (#41, 2.21.1): el catálogo había crecido
39x al arreglarse el parser de Coinbase en P0, y con 39 593 registros y 98 MB en
S3 cada operación sobre el catálogo entero se volvió cara. Esta rama se rebaseó
encima. Conflictos en `VERSION` (a 2.22.0), `.env.example` y dos archivos:

- **`discovery.rs`**: los dos lados insertaban en el mismo punto. Quedan los dos
  — el techo del catálogo (`enforce_capacity`) y el veredicto de import
  (`import_verdict` + `provenance_rank`).
- **`discovery_store.rs`**: dos bloques de tests en el mismo anclaje. Quedan los
  siete.
- **`discovery_health.rs`**: fusionó solo.

**Los dos lados describen la misma escalera de procedencia y ahora conviven en el
mismo archivo**, así que está dicho por qué el desalojo nombra `Aggregated` en
vez de preguntarle a `provenance_rank`: son la misma escalera pero responden
preguntas distintas. `provenance_rank` ordena autoridades para que un merge elija
ganador; el desalojo pregunta algo más angosto — *¿este registro es
reemplazable?* — y sólo lo es el escalón de abajo. Desalojar "lo que rankee más
bajo" empezaría a borrar registros crawleados en cuanto un catálogo no tuviera
agregados, que es justo el dato de primera mano que la regla existe para
proteger.

**Una interacción que el rebase creó y que se arregló acá**: el overlay de
términos que agrega esta fase es un **segundo** objeto escrito entero, así que
acumulaba entradas huérfanas exactamente como `bazaar/health.json` las acumulaba
antes de 2.21.1. Se poda contra el mismo conjunto vivo, en el mismo tick, con la
misma guarda: **con el catálogo vacío no se poda nada**, porque cuando la lectura
de S3 falla al arrancar `main` cae a un registro vacío y sigue sirviendo, y un
conjunto vacío significa "no pudimos leer el catálogo", nunca "el catálogo no
tiene recursos". Dos tests.

La suite completa corrió verde después de este rebase, incluidos los 15 tests del
hotfix.

## Y un tercero, sobre 2.21.2

2.21.1 no restauró la línea base y hubo un [segundo hotfix](2026-09-10-hotfix-cpu-2.md):
el techo bajó de 20.000 a 2.000 registros, la entrada por fuente a 1.000, el
prober a 2 rps, y el overlay de salud dejó de subirse cada minuto. Esta rama se
rebaseó encima. Conflictos en `VERSION` (a 2.22.0), `discovery.rs` y los imports
de `discovery_health.rs`; todo se conserva de los dos lados.

Dos cosas que se corrigieron al pasar:

- **El techo del overlay de términos bajó de 20.000 a 2.000.** Está keyeado por
  URL del catálogo y podado contra el conjunto vivo, así que nunca puede tener
  más lecturas que recursos hay: 20.000 era inalcanzable, y después del 10 de
  septiembre un 20.000 suelto en la configuración se lee como una escala de
  catálogo para la que este servicio está dimensionado, que es exactamente lo que
  ese incidente desmintió.
- **Un comentario de documentación que quedó mal en 2.21.2.** La explicación
  entera de `enforce_capacity` había quedado pegada a `admission_threshold`, y
  `enforce_capacity` se quedó con una sola línea. Sin efecto en el comportamiento;
  se devolvió cada bloque a su función al resolver, que es donde tocaba.

## Para c0der

**Qué cambió.** `src/discovery_terms.rs` (nuevo): el overlay de términos
observados, la escalera de procedencia y `assess_freshness`. `types_v2.rs`:
`lastSettledAt`, `recordVersion`, `content_fingerprint()`,
`strip_response_only()` y `record_settlement()` (que reemplaza a
`increment_settlement_count`). `discovery.rs`: `import_verdict` con hash y
escalera, el conteo de `unchanged` / `superseded`, y la anotación de frescura al
listar. `discovery_health.rs`: `LiveTerms` con requisitos completos, la
reconciliación de transportes y el registro de la observación. `discovery_store.rs`:
el contrato del formato persistido documentado y `format_census()`. `openapi.rs`
y `bazaar.html` al día. Nada del camino de pago cambia.

**Decisiones de la sección 14 del anexo, tomadas acá.** Cada una por la
suposición más reversible:

- **Ubicación del overlay y coordinación entre réplicas.** Objeto propio
  (`bazaar/terms.json`), escrito sólo por el comprobador de salud, con su propio
  debounce (`DISCOVERY_TERMS_PERSIST_SECS`, 300 s) y tope de registros
  (`DISCOVERY_TERMS_MAX_RECORDS`, 20 000, desalojando la observación más vieja).
  Sin lease: hay un solo escritor por diseño. *Reversible*: borrar el objeto
  cuesta las anotaciones de frescura hasta el siguiente barrido, y nada más.
- **Identidad estable de contextos.** `ObservationContext { method, resourceType,
  authenticated }` con una clave derivada (`"GET http anon"`), no un id opaco.
  Hoy el comprobador sólo produce un contexto; cuando P2 agregue POST con
  parámetros, el campo ya está y no hay migración. *Reversible*: es un string
  derivado, no un identificador guardado en otro lado.
- **Rangos declarados vs. observados.** No se introduce ningún tipo de rango en
  esta fase. Un `upto` se conserva como techo con su fase anotada, y una
  observación de fase `settlement` **nunca** se compara contra un techo de
  catálogo. Inventar un rango a partir de dos muestras es exactamente el «falso
  drift» que el anexo nombra.
- **Esquemas desconocidos en el DTO público.** Sin cambios: P0 ya los conserva
  con su nombre y `settleable: false`. La observación usa el mismo tipo, así que
  un esquema que no sabemos liquidar se puede observar igual.
- **Rutas de cotización seguras para POST/auth.** No se infieren. El contexto
  guardado dice `authenticated: false` y `method: GET`; una variante detrás de un
  login queda **sin verificar para esa variante**, que no es lo mismo que muerta.
- **Capacidades reales de refresco de CDP/x402scan.** Fuera de P1. Este PR no
  hace ninguna llamada nueva a terceros: reusa el sondeo de salud que ya existe.
- **`offer-receipt` y retención de cotizaciones personalizadas.** P3/P4. La fase
  `settlement` y el campo `provenance` están en el formato para que entren sin
  romperlo.

**Qué queda para P2** (PR 4 de la secuencia):

- Cola de revalidación con deduplicación por recurso y contexto, límites por
  host y prioridad por demanda. Hoy la única cadencia de precio es la de salud, y
  son dos preguntas distintas con dos ritmos distintos.
- Refresco bajo demanda al consultar un listado vencido, sin bloquear la
  búsqueda; y `nextPriceCheckAt` / `observationExpiresAt`, que están en el modelo
  del anexo y todavía no en el código.
- Métricas. La sección 12 del anexo propone
  `discovery_price_comparison_total{result,source,scheme}` y
  `discovery_terms_age_seconds`; esta fase deja los estados calculados y logueados
  pero no instrumentados. El denominador honesto ya existe: sólo observaciones
  comparables.
- Observaciones de fase `settlement`. El formato las admite y **ningún componente
  las escribe**: una liquidación mueve `lastSettledAt` y nada más. Escribirlas
  encima de la lectura del desafío borraría la única respuesta que hay sobre el
  techo, así que necesitan su propia entrada, no la misma.
- Dimensionar el snapshot. Si la fuente de Coinbase vuelve entera (14 235
  publicados contra 335 nuestros), el catálogo se multiplica y con él el PUT
  horario de 15 MB.

**Límites conocidos, deliberados:**

- **Los recursos `mcp` no observan términos.** `probe_mcp` hace un POST JSON-RPC
  y no captura cuerpo ni cabecera; su señal sana es un 200, no un 402. Quedan en
  `priceFreshness: unknown` para siempre, que es la verdad.
- **Una copia agregada ya no puede pisar un registro auto-registrado, nunca.**
  Es lo que pide la escalera, y tiene una consecuencia: un listado cuyo dueño lo
  abandonó queda congelado hasta que el dueño vuelva a registrarlo. La frescura
  lo marca (`unknown` o `conflict`), pero el merge no lo destraba.
- **`recordVersion` cuesta ~24 bytes por registro** en un snapshot de 15 MB con
  24 636 registros: unos 590 KB, ~4 %. Es el precio de que un registro se
  describa a sí mismo, y se paga a sabiendas.
- **Un rollback conserva el overlay pero pierde `lastSettledAt` y
  `recordVersion`** en el primer snapshot que escriba la imagen vieja. No hay
  forma de evitarlo para datos que viven dentro del registro; por eso lo que
  importaba de verdad no vive ahí.
- **El overlay se lee entero o se degrada por registro**, nunca falla el
  arranque: un registro que esta imagen no sabe parsear cuesta ese registro y se
  cuenta.
