# Handoff: las tres filas del facilitador que c0der reabrio, hechas

> Fecha: 2026-09-05
> Rama: `0xultravioleta/x4-landing` (parte de `origin/main` = `f33bc50b`)
> Archivos: `static/index.html`, `docs/CHANGELOG.md`, `README.md`
> Estado: los tres `cierra:` pasan, la puerta de tests de CI en verde, PR abierto **sin mergear**

## Por que existe este handoff

Un worker de c0der (PR #4 de c0der, 2026-09-05) barrio el backlog del
facilitador y encontro que dos filas cerradas la noche anterior estaban **mal
cerradas**:

- la del `fee 0%` se habia cerrado con un `grep -c '0%'` sobre un HTML, que
  cuenta `width: 100%` y `linear-gradient(... 0%)` -- CSS, no un precio;
- la de los enlaces de GitHub se habia cerrado midiendo que el fork esta
  enlazado dos veces, que no es lo que pedia la fila: pedia que **los enlaces
  del defecto** dejaran de mandar al upstream, y el boton «Source Code» seguia
  yendo a `x402-rs/x402-rs`.

Las reabrio con un `cierra:` ejecutable y no las hizo porque x402-rs no es su
repo. Esto es esa deuda, saldada.

## Lo que se hizo, fila por fila

### 1. URL base en el hero (+ `fee 0%` en superficie humana)

La URL del facilitador estaba escrita para maquinas en cuatro archivos
(`llms.txt:90`, `.well-known/x402:5`, `agent-card.json:4`, `auth.md:44`) y en
**cero** superficies humanas de `index.html`: el unico hit del dominio era
`zama-facilitator.ultravioletadao.xyz` (`:1771`), que es otro subdominio.

Un injerto de una linea en el hero, debajo de la bajada
(`static/index.html:1175`):

```html
<p style="..." data-i18n-html="hero.baseUrl">Base URL <code style="...">https://facilitator.ultravioletadao.xyz</code> · 0% fee · no key, no account</p>
```

Tres detalles que no son estetica:

- **Estilos inline, no una regla de CSS.** Una regla en el `<style>` empuja el
  hero hacia abajo y el hit se sale de la ventana `:1160`-`:1185` a la que la
  fila ata su chequeo. Ademas es el idioma de la barra de stablecoins que esta
  justo debajo, que es todo inline.
- **`<code style='...'>` con comillas simples en el diccionario**, dobles en el
  markup. Es el patron que ya usa `endpoints.registerAgent`
  (`:2602` markup / `:2824` diccionario). Con comillas dobles adentro del string
  de JS el objeto `translations` entero se rompe y la pagina deja de traducir en
  los dos idiomas. Verificado con `node`: el objeto parsea, 145 claves EN / 145 ES.
- **La clave entra en los dos diccionarios en el mismo commit** (`:2838` EN,
  `:2990` ES), que es lo que exige N1.

El `fee 0%` como **texto visible** ya estaba y sigue: `footer.by` =
«Ultravioleta DAO · x402 payment facilitator · 0% fee» (`:2800` EN, `:2951` ES),
presente en las diez paginas humanas. El injerto del hero lo repite arriba del
pliegue, que es donde la fila lo queria.

### 2. Los dos enlaces de GitHub al fork

Quedaba **uno**: el boton «Source Code» (`static/index.html:2403`, texto en
`:2407`) apuntaba a `https://github.com/x402-rs/x402-rs`. Literalmente el dano
que describia la fila: un humano que hace clic en «Source Code» no llega al
codigo que corre.

Cambio de una linea a `https://github.com/UltravioletaDAO/x402-rs`.

**El remoto se verifico, no se asumio del backlog:**

```
$ git remote -v
origin    https://github.com/UltravioletaDAO/x402-rs.git (fetch)
upstream  https://github.com/x402-rs/x402-rs.git (fetch)
```

Los otros nueve `github.com/` de `index.html` **no** se tocaron y no son el
defecto: apuntan a repos de terceros que estan bien asi -- `tomi204/x402-zama`,
`erc-8004/erc-8004-contracts`, `BackTrackCo/x402r-{contracts,sdk}`,
`coinbase/x402/issues/864`. Los dos que ya iban al fork (`:2450`, `:2776`)
siguen igual.

### 3. CHANGELOG de `POST /mcp` + bump de VERSION + badge del README

De las cuatro mitades de la fila:

- **`VERSION`: ya estaba.** `2.14.0`, commiteado en `1cf251e6`. Sin cambios.
- **Badge del README: hecho.** `README.md:13` estaba en `2.11.0`, tres minors
  atras. Ahora `2.14.0`, leido de `VERSION`, no tipeado.
- **`2.13.0` y `2.14.0` en el CHANGELOG: hechos**, escritos desde los cuerpos de
  commit reales (`6833f9ed` y `1cf251e6`), no de memoria.
- **`POST /mcp`: hecho, y en el lugar donde de verdad salio.** No es una entrada
  nueva: `POST /mcp` shippeo en **2.9.0** (`445f2a15`, `git show 445f2a15:VERSION`
  -> `2.9.0`, y la propia fila registraba «`/version` = 2.9.0 con MCP
  desplegado»). La entrada 2.9.0 existia sin nombrar nunca el endpoint. Se le
  agrego una seccion `### Added` que lo nombra, con las dos cosas que un
  integrador necesita saber: que un tool call se despacha **a traves** del router
  REST (`ServiceExt::oneshot`) para no saltarse `settle_writer_gate`, y que
  `/mcp` comparte el `Arc` del governor con `/verify` y `/settle`.

Fechas y contenido salen de `git log`; produccion sirve `2.14.0` hoy
(`curl -s .../version` -> `{"version":"2.14.0"}`), asi que las dos entradas
nuevas van **sin** el sufijo «(committed, not yet deployed)».

## Los tres `cierra:`, corridos

```
########## CIERRA 1 -- URL base en el hero ##########
$ grep -c 'https://facilitator.ultravioletadao.xyz' static/index.html
3
$ grep -n ... | grep -v zama-facilitator
1175:  <p style="..." data-i18n-html="hero.baseUrl">Base URL <code ...>   <- markup, EN LA VENTANA :1160-:1185
2838:  "hero.baseUrl": "Base URL <code ...>                                <- diccionario EN
2990:  "hero.baseUrl": "URL base <code ...>                                <- diccionario ES
-- el hit debe caer entre :1160 y :1185 --
1175 INSIDE

########## CIERRA 2 -- los dos enlaces de GitHub al fork ##########
$ grep -c 'x402-rs/x402-rs' static/index.html
0

########## CIERRA 3 -- CHANGELOG + VERSION + badge ##########
$ grep -o 'version-[0-9.]*' README.md   /   $ cat VERSION
version-2.14.0
2.14.0                                        <- coinciden
$ grep -c '^## \[2.1[34]\.0\]' docs/CHANGELOG.md
2
$ grep -c 'POST /mcp' docs/CHANGELOG.md
1
```

Los hits `:2838` y `:2990` son los diccionarios que respaldan el de `:1175`;
el que la fila exige dentro de la ventana esta.

## Tests

**i18n (N1 y N2) verdes**, mas los otros tres del modulo:

```
$ cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl i18n_tests -- --test-threads=1
test handlers::i18n_tests::every_key_exists_in_both_languages ... ok                        <- N1
test handlers::i18n_tests::every_key_used_by_the_markup_is_defined_in_both_languages ... ok <- N2
test handlers::i18n_tests::every_page_title_is_english_and_translatable ... ok
test handlers::i18n_tests::no_page_picks_a_language_from_the_browser ... ok
test handlers::i18n_tests::the_language_choice_lives_under_one_key ... ok
test result: ok. 5 passed; 0 failed
```

**N1 verificado discriminante**, no solo verde: sacando la mitad ES de la clave
nueva y dejando todo lo demas igual, se pone rojo y **nombra la clave**:

```
test handlers::i18n_tests::every_key_exists_in_both_languages ... FAILED
static/index.html: ["hero.baseUrl"] exist in `en` and not in `es`.
```

**El chequeo de la landing que corre en CI** (`.github/workflows/ci.yaml:56`):

```
$ python3 scripts/verify_landing_canonical.py --offline
[OK] landing page matches /supported, escrow, and ERC-8004 sources.
EXIT=0
```

## Alcance del diff

```
$ git diff --stat origin/main
 README.md         |  2 +-
 docs/CHANGELOG.md | 68 ++++++++++++++++++++++++++++++++++++++++++++--
 static/index.html |  6 ++++-
 3 files changed, 72 insertions(+), 4 deletions(-)
```

Solo los archivos que nombran las tres filas. `VERSION` no aparece porque ya
estaba en `2.14.0`. Sin rediseno: tres injertos quirurgicos.

**Una cosa que toque y las filas no piden, declarada:** en `docs/CHANGELOG.md`
les saque el sufijo «(committed, not yet deployed)» a `2.11.0` y `2.12.0`.
Produccion sirve `2.14.0`, asi que ya estan desplegadas; dejarlo habria dejado
el archivo diciendo que dos versiones anteriores a las que yo agrego siguen sin
desplegar. Son cuatro palabras y las causa mi propia edicion. Si c0der lo
prefiere afuera, se revierte solo.

## Lo que NO se hizo, y por que

- **Cero deploy, cero merge.** Un merge a `main` de x402-rs ES un release. La
  rama esta pusheada y el PR abierto; el merge lo decide c0der.
- **El orden no monotono del CHANGELOG** (`2.0.0` en `:262` **antes** de `2.1.0`
  en `:329`) sigue ahi. El `cierra:` de la fila no lo pide, y reordenar el
  archivo es un diff mecanico grande con riesgo de perder contenido. Queda como
  fila propia si se quiere.
- **La entrada `2.8.0` que falta**: no es contenido perdido. `2.8.0` era
  `b4170d76` («el resync que existia para curar el nonce era lo que lo
  rompia»), y **ya esta documentado bajo el encabezado `2.9.0`**
  (`docs/CHANGELOG.md`, seccion 2.9.0, lo cita por hash). Es un encabezado
  plegado, no un agujero. Inventarle una entrada propia habria sido duplicar.
- **i18n**: solo la clave que hacia falta (`hero.baseUrl`), en los dos idiomas.

## Para c0der

Las tres filas viven en **tu** `docs/planning/BACKLOG.md` (lineas 65, 66 y 71 en
`origin/master` al 2026-09-05). No las edite -- es tu repo y tiene sesiones
adentro. Aca va el texto exacto de cierre para pegar en la columna de estado de
cada una, y el PR queda sin mergear esperando tu decision.

### Fila 65 -- «URL base en el hero + fee `0%` en superficie humana»

> **Cerrado** - 2026-09-05, x402-rs PR (rama `0xultravioleta/x4-landing`). Las dos mitades. (a) **URL base en el hero: HECHA** -- `static/index.html:1175`, debajo de la bajada del hero: `Base URL <code>https://facilitator.ultravioletadao.xyz</code> · 0% fee · no key, no account`, clave `hero.baseUrl` en los dos diccionarios (`:2838` EN, `:2990` ES). `grep -c 'https://facilitator.ultravioletadao.xyz' static/index.html` -> **3**, y el unico hit de markup cae en **`:1175`**, dentro de la ventana `:1160`-`:1185` que pedia la fila (los otros dos son los diccionarios que lo respaldan). El hit de `zama-facilitator` (`:1771`) sigue siendo otro subdominio y no cuenta. (b) **fee 0%: confirmado como texto visible**, no como CSS: `footer.by` en `:2800` (EN) y `:2951` (ES), y ahora tambien en el hero. Va con estilos inline a proposito: una regla en el `<style>` corre el hero fuera de la ventana del chequeo. N1/N2 verdes, y N1 verificado discriminante (sacando la mitad ES se pone rojo nombrando `hero.baseUrl`).

### Fila 66 -- «Los dos enlaces de GitHub al fork»

> **Cerrado** - 2026-09-05, mismo PR. `grep -c 'x402-rs/x402-rs' static/index.html` -> **0**. El que faltaba era el boton «Source Code» (`static/index.html:2403`, texto en `:2407`); ahora apunta a `https://github.com/UltravioletaDAO/x402-rs`. El remoto se verifico contra `git remote -v` (`origin` = `UltravioletaDAO/x402-rs`), no se asumio de la fila. Los otros nueve `github.com/` del archivo no se tocaron y no eran el defecto: son repos de terceros (`tomi204/x402-zama`, `erc-8004/erc-8004-contracts`, `BackTrackCo/x402r-*`, `coinbase/x402/issues/864`).

### Fila 71 -- «CHANGELOG de `POST /mcp` + bump de VERSION + badge del README»

> **Cerrado** - 2026-09-05, mismo PR. Los tres chequeos: `grep -o 'version-[0-9.]*' README.md` -> `version-2.14.0` == `cat VERSION` -> `2.14.0`; `grep -c '^## \[2.1[34]\.0\]' docs/CHANGELOG.md` -> **2**; `grep -c 'POST /mcp' docs/CHANGELOG.md` -> **1**. Detalles: (a) `VERSION` ya estaba en 2.14.0 (`1cf251e6`), no hizo falta bump; (b) el badge estaba en 2.11.0, tres minors atras; (c) las entradas 2.13.0 y 2.14.0 se escribieron desde los cuerpos de commit reales (`6833f9ed`, `1cf251e6`), sin sufijo «not yet deployed» porque produccion ya sirve 2.14.0; (d) **`POST /mcp` se documento en 2.9.0, que es donde salio de verdad** (`445f2a15`, `git show 445f2a15:VERSION` -> `2.9.0`, coincide con lo que la propia fila media: «/version = 2.9.0 con MCP desplegado») -- la entrada 2.9.0 existia sin nombrar el endpoint. **Dos cosas de la fila quedan fuera del `cierra:` y siguen abiertas**: el orden no monotono (`2.0.0` en `:262` antes de `2.1.0` en `:329`), que es un reordenamiento grande y merece fila propia; y la entrada `2.8.0`, que **no es contenido perdido** -- `2.8.0` era `b4170d76` y ya esta documentado bajo el encabezado 2.9.0, citado por hash. Es un encabezado plegado, no un agujero.
