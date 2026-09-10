---
date: 2026-09-10
tags:
  - type/handoff
  - domain/bazaar
  - domain/discovery
  - priority/p0
status: active
---

# El bazar decía que todo costaba un importe exacto, y no lo sabía

**Versión:** 2.20.0 · **Fase P0** del plan de precios dinámicos
(handoff de Astra 6, 2026-09-09, base `9893c48f`) · **PR 1 de 6** de la secuencia
de ese plan.

Un catálogo de precios que reescribe el esquema de pago no está describiendo el
mercado: está afirmando algo sobre el dinero ajeno. Al importar, el facilitador
escribía `exact` en **todas** las opciones de pago, tirara lo que tirara la
fuente; borraba `extra` entero; y cuando no lograba leer un importe lo guardaba
como **cero**, que en un catálogo se lee como *gratis*.

Las tres cosas están medidas, no inferidas.

## Lo que había, medido

Snapshot de producción `s3://facilitator-discovery-prod/bazaar/resources.json`,
leído el 2026-09-10 15:54Z (15,2 MB, ETag `35e1d306…`):

| | |
|---|---:|
| registros en el catálogo | 24 636 |
| de origen agregado | 24 078 |
| opciones de pago totales | 24 728 |
| **escritas como `exact`** | **24 728** |
| con cualquier otro esquema | **0** |
| que conservan `extra` | 176 (todas de registro propio) |
| que conservan `extra`, de origen agregado | **0** |
| con `sourceUpdatedAt` | 0 (campo nuevo) |

Cero opciones no-`exact` en 24 728 no es un catálogo homogéneo. La primera página
del feed de Coinbase CDP, leída el mismo día, publica **26 `batch-settlement` y 1
`agent-pay` sobre 178 opciones** — el 15 %. El campo no describía la realidad;
la sobrescribíamos.

Igual con los importes: **3 de esas 178 declaran un decimal (`"0.002"`) en un
campo de unidades atómicas**. `U256::from_str` falla con eso y el conversor
mapeaba el fallo a cero. No se ven ceros en el snapshot porque la regla `R3`
(`unpayable`) tira el recurso entero cuando todas sus opciones quedan en cero:
la pérdida no aparece como ofertas gratis, aparece como **recursos que
desaparecieron**.

### Dos hallazgos que no estaban en el encargo

**1. El feed de Coinbase se estaba cayendo entero, todos los ciclos.**
`CoinbasePaymentRequirement.amount` llevaba `#[serde(alias = "maxAmountRequired")]`.
Un alias hace que las dos grafías sean **el mismo campo**, así que un documento
que trae las dos es un *duplicate field* para `serde`. El feed real trae las dos
en 55 de 178 opciones, y en las 5 páginas muestreadas el 2026-09-10 **todas**
tenían al menos una. `parse_discovery_response` devuelve `Err` y
`fetch_from_facilitator` **aborta la fuente completa**, no la página. Resultado:
14 235 recursos publicados por CDP, 335 en nuestro catálogo. Un 2,4 %.

**2. Las direcciones no-EVM se tiraban en silencio.** El agregador traía su
propio parser (`0x` + 42 caracteres) en vez del de `MixedAddress`. La primera
página de CDP trae 33 opciones en Solana; el catálogo entero tiene **1**.

## Lo que cambia

**Un tipo de catálogo propio.** `DiscoveryResource.accepts` pasa de
`PaymentRequirementsV2` a `CatalogPaymentOption` (`src/discovery_price.rs`).
`PaymentRequirementsV2` es un tipo de **protocolo**: su `scheme` es el enum
cerrado `Scheme` porque un pago que no sabemos nombrar es un pago que no podemos
hacer. Un **catálogo** tiene la obligación contraria: tiene que poder cargar una
oferta que no sabe liquidar y decirlo, en vez de reetiquetarla como una que sí.
Compartir el tipo era la causa raíz de F1. El JSON en el cable no cambia para
ningún esquema conocido.

**Una sola regla de normalización, para todas las rutas.**
`normalize_declared_option` es el único punto por el que entra una opción de
pago, la use el agregador, el crawler, `POST /discovery/register` o
`bulk_import`. Antes cada ruta decidía por su cuenta: el registro directo
conservaba `scheme` y `extra` y rechazaba de plano un esquema desconocido (422 de
todo el body); el agregador conservaba nada. Ahora las cuatro leen el mismo JSON
igual, y hay un test que lo afirma con los mismos bytes por las dos puertas.

**`maxAmountRequired` explícito, no alias.** Dos campos separados que se
reconcilian: si sólo viene uno, ése es; si vienen los dos y **coinciden**, ése es
(55 de 55 coinciden hoy en el feed real); si vienen los dos y **discrepan**, se
rechaza. Elegir uno sería adivinar y promediarlos inventaría un precio que nadie
publicó. No es un rango y no cambia el esquema: para `exact` el número es el
precio, para `upto` es el techo, y quién de los dos es lo decide `scheme`.

**Un importe ilegible rechaza la opción, con causa.** `amount-missing`,
`amount-not-an-integer`, `amount-negative`, `amount-overflow`,
`amount-conflicting`. Las causas se cuentan por fuente y se registran. Un `"0"`
que la fuente declaró de verdad sí se conserva: cero explícito y error de parseo
dejaron de ser el mismo resultado. Si el catálogo publica o no un recurso gratis
sigue siendo política (`curation_check`), decidida después de parsear.

**Catalogable ≠ liquidable.** Cada opción de una respuesta lleva `settleable` y
`unsupportedReason` (`unknown-scheme`, `network-not-served`,
`upto-proxy-not-deployed`). Un `batch-settlement` en Base es una oferta real; sólo
que no la podemos pagar nosotros, y el catálogo tiene que poder decir las dos
cosas a la vez.

**Moneda y decimales por despliegue.** `assetSymbol` y `assetDecimals` se
resuelven por (red, activo) contra la tabla de `network.rs`. Ausentes significa
**desconocido**, que no es lo mismo que seis decimales y un signo de dólar. USDC
es 6 decimales en Base, **18 en BSC** y 7 en Stellar.

Los cuatro campos anteriores son **sólo de respuesta**: se calculan al componer
el listado y no se persisten nunca, igual que `health` y `curation`. Un registro
guardado no puede seguir afirmando seis decimales después de que aprendamos que
son dieciocho. Y `strip_response_only` corre en las tres construcciones de
`DiscoveryResource`, así que un registrante no puede afirmar por sí mismo qué
puede liquidar *este* facilitador.

**Las fechas desconocidas siguen desconocidas.** El agregador ya no estampa `now`
cuando la fuente no trae fecha. `sourceUpdatedAt` es la fecha que la fuente
declaró — ausente cuando no declaró ninguna — y `lastUpdated` queda documentado
como lo que siempre fue: cuándo escribimos *nosotros* el registro.
`import_supersedes` decide el merge por la fecha de la **fuente**, nunca por
nuestro reloj:

| entrante | almacenado | veredicto |
|---|---|---|
| con fecha | con fecha | gana la más nueva |
| con fecha | sin fecha | gana la fechada |
| **sin fecha** | **con fecha** | **no** — éste era el caso invertido (F6) |
| sin fecha | sin fecha | sí; sin fecha nada decide, y un cambio de contenido tiene que poder entrar |

Decidir *qué contenido es el correcto* cuando ninguno trae fecha necesita un hash
de contenido y una escalera de procedencia. Eso es P1, y `import_supersedes` es a
propósito el único sitio que va a tener que cambiar.

**La página del bazar.** Sólo el formateador y lo mínimo alrededor; el diseño no
se toca. `fmtPrice` leía `accepts[0]`, asumía seis decimales y ponía `$` delante,
con `Number()`. Ahora:

- Todo pasa por `BigInt`. Un activo de 18 decimales pasa el entero seguro de
  JavaScript (2^53) a los 9,007 tokens.
- Se muestran **todas** las alternativas, cada una con su unidad y su esquema.
- «desde» sólo cuando el mínimo lo es de un conjunto realmente comparable: misma
  moneda **y** mismo esquema. Un techo y un precio fijo no son dos puntos de un
  rango, y dos monedas tampoco. Si difieren: «N formas de pagar».
- «hasta X» para `upto`, con la explicación de que el cobro efectivo se liquida
  aparte y puede ser menor.
- «precio no verificado» cuando no hay unidad conocida. No afirma que sea gratis.
- Textos EN/ES para todo lo anterior, y `applyLang` ahora repinta la grilla
  (servida de su propia caché) porque las tarjetas llevan texto traducido.

## Fixtures

- `tests/fixtures/bazaar/cdp-pricing-page.json` — cuatro entradas capturadas del
  feed real de Coinbase CDP el 2026-09-10, recortadas (se eliden los cuerpos de
  JSON Schema dentro de `extensions` y un JWT de 700 bytes; nada de lo que toca
  al precio). Trae `batch-settlement`, `agent-pay`, `extra` en todas, una opción
  en Solana, un decimal en campo atómico, las dos grafías del importe en la misma
  opción, y una entrada sin fecha.
- `tests/fixtures/bazaar/v1-pricing-page.json` — la grafía x402 v1 de lo mismo
  (nombres de red v1, `maxAmountRequired`), más un listado `upto`, que ningún feed
  público publica todavía y que es el caso en el que el conversor viejo estaba
  más equivocado, y dos activos con distinta cantidad de decimales.

23 tests en `tests/bazaar_pricing.rs`, más 4 en `src/discovery.rs` para el merge
por fechas y la anotación de lectura. **Probados en rojo**: con la semántica
vieja restaurada a mano, 11 de los 23 y 2 de los 4 fallan.

Medido contra la página real de CDP de 100 ítems, antes y después:

| | antes | después |
|---|---:|---:|
| la página parsea | **no** | sí |
| recursos convertidos | 0 | 100 |
| opciones conservadas | 0 | 174 / 178 |
| de ellas, no-`exact` | — | 26 |
| de ellas, con `extra` | — | 174 |
| de ellas, en Solana | — | 33 |
| opciones rechazadas | — | 4, todas `network-unrecognized` |

Las 4 rechazadas son `aws:base`, `hyperliquid:mainnet` (×2) y una de Algorand:
namespaces CAIP-2 que este código no sabe nombrar. Rechazadas con causa, no
reetiquetadas.

## Compatibilidad hacia atrás

`serde_json::from_slice::<Vec<DiscoveryResource>>` sobre el snapshot es
todo-o-nada: **un** registro que no parsee tumba el catálogo entero al arrancar.
Como este cambio *endurece* qué parsea, lo verifiqué contra datos reales antes de
darlo por bueno: **1 693 registros distintos** servidos hoy por
`/discovery/resources` (las cuatro fuentes, offsets de 0 a 24 500) deserializan
con el tipo nuevo, 1 693 de 1 693.

Un susto que sí apareció por el camino y quedó cubierto: aplicarle a
`extensions` el límite de profundidad de `extra` (16, calibrado para
`{name, version}`) **rechazaba la página entera de CDP**, porque su JSON Schema
anida 26 niveles. Ahora `extensions` tiene su propio límite (64 niveles, 16 KiB)
y, sobre todo, fuera de límite se **descarta el blob**, nunca se falla el
documento. Un tercero verboso no puede tumbar una página de precios. Hay test.

## Migración: lo ya guardado no se repara solo

**No se puede reconstruir el esquema verdadero a partir del importe.** Un `3000`
etiquetado `exact` es indistinguible de un `3000` que era `batch-settlement`. El
parser arreglado no repara nada de lo ya persistido.

Los **24 078 registros de origen agregado** están afectados: todos perdieron
`scheme` y `extra`. Los 558 de registro propio conservan lo suyo (176 opciones
con `extra`), porque esa ruta nunca reescribía.

Comando de conteo, **sólo lectura**, contra el snapshot que es la fuente de
verdad:

```sh
aws s3 cp s3://facilitator-discovery-prod/bazaar/resources.json - --region us-east-2 | jq -r '
  "records total ............ \(length)",
  "  aggregated ............. \([.[]|select(.source=="aggregated")]|length)",
  "  self_registered ........ \([.[]|select(.source=="self_registered")]|length)",
  "payment options total .... \([.[].accepts[]]|length)",
  "  written as exact ....... \([.[].accepts[]|select(.scheme=="exact")]|length)",
  "  any other scheme ....... \([.[].accepts[]|select(.scheme!="exact")]|length)",
  "  carrying extra ......... \([.[].accepts[]|select(has("extra"))]|length)",
  "  amount == 0 ............ \([.[].accepts[]|select(.amount=="0")]|length)",
  "records with sourceUpdatedAt \([.[]|select(has("sourceUpdatedAt"))]|length)"'
```

Estrategia, en orden:

1. **Copia recuperable antes de nada.** El snapshot se escribe entero en cada
   import; copiar el objeto con su ETag actual es el rollback.
2. **Reimportar, no migrar.** Los 24 078 se recuperan solos: la tarea de
   agregación corre cada hora, y con `amount`/`maxAmountRequired` separados la
   fuente de Coinbase vuelve a entrar. Cada reimport reescribe `scheme` y `extra`
   con lo que la fuente publica hoy. **No se puede acelerar con una migración en
   sitio** — el dato correcto no está en nuestro lado.
3. **Los que no vuelvan quedan sin verificar.** Un recurso cuya fuente ya no lo
   publica se queda con su `exact` histórico y sin `extra`, y no hay forma de
   distinguirlo de uno realmente exacto. Cuando P1 traiga el modelo de
   procedencia, la marca correcta para esos es «semántica histórica no
   verificada», no `exact`.
4. **Medir el barrido con el mismo comando.** Cuando `any other scheme` deje de
   ser 0 y `carrying extra` suba de 176, el reimport está entrando. Si a las 24 h
   `records total` sube fuerte, es la fuente de Coinbase recuperada (14 235
   publicados contra 335 nuestros hoy) y hay que mirar el coste del snapshot: son
   ~15 MB para 24 636 registros, y multiplicar por diez el catálogo multiplica
   por diez ese PUT horario. Ese dimensionamiento es trabajo de P2, pero el
   disparador puede llegar el primer día.

## Para c0der

**Qué cambió.** `src/discovery_price.rs` (nuevo) es la semántica de precio del
catálogo: `CatalogScheme`, el parser de importes, `CatalogPaymentOption` y
`normalize_declared_option`, que es el único punto de entrada de una opción de
pago. `types_v2.rs` cambia el tipo de `accepts`, añade `sourceUpdatedAt` y
`extensions`. `discovery_aggregator.rs` pierde su conversor y sus dos parsers
propios. `discovery.rs` gana `import_supersedes` y anota los precios al listar.
`discovery_crawler.rs` acepta una fecha del publicador. `bazaar.html` cambia el
formateador. `openapi.rs` documenta la semántica nueva. Nada del camino de pago
(`verify`/`settle`/escrow/upto) cambia: `PaymentRequirementsV2` queda intacto.

**Fixtures.** Dos páginas capturadas, `tests/fixtures/bazaar/{cdp,v1}-pricing-page.json`,
y 23 tests en `tests/bazaar_pricing.rs`. Probados en rojo contra la semántica
vieja (11/23 fallan). Los recortes de las fixtures son sólo cuerpos de JSON
Schema y un JWT; ningún campo de precio está tocado.

**Qué queda para P1** (PR 2 de la secuencia):

- `lastSettledAt` separado del timestamp de contenido: hoy `track_settlement`
  todavía rejuvenece `lastUpdated`. Un cobro es actividad, no verificación de
  precio.
- Hash de contenido y escalera de procedencia, para decidir el merge cuando
  ninguna de las dos partes trae fecha — el caso `(None, None)` de
  `import_supersedes`, que hoy deja entrar al último.
- El overlay de términos observados (`probed_accepts` / `probed_accepts_at`), que
  es lo que convierte «precio no verificado» en «verificado a tal hora».
- Versionado del formato persistido con lectura compatible. Este PR ya es
  compatible en las dos direcciones (campos nuevos opcionales, esquemas conocidos
  con el mismo string), pero no lo declara en ningún sitio.

**Límites conocidos, deliberados:**

- No hay namespace CAIP-2 para Algorand en `src/caip2.rs`, aunque
  `Network::from_caip2` sí conoce `algorand:mainnet`. Las opciones en Algorand de
  un feed se rechazan con `network-unrecognized`. Arreglarlo toca el parsing
  CAIP-2 del camino de pago y no entra en P0.
- `parse_catalog_address` rechaza a propósito el fallback `MixedAddress::Offchain`
  para lo que llega de un feed: su patrón acepta palabras corrientes, así que
  aceptarlo dejaría pasar cualquier cadena como dirección. El registro directo lo
  sigue admitiendo.
- Los campos que CDP mete dentro de `accepts` y que son restos de v1
  (`description`, `mimeType`, `outputSchema`, `resource`) no se conservan. El
  recurso ya tiene descripción y el esquema útil está en `extensions`.
- `POST /discovery/register` con un importe ilegible ahora es 400. Es el
  comportamiento correcto y es un cambio de contrato: antes ese cuerpo también
  fallaba (`TokenAmount` no parseaba `"0.002"`), así que no hay regresión, pero el
  mensaje de error cambia.
