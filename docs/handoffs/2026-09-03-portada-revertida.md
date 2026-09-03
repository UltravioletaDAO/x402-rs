# La portada vuelve al diseno original, con el menu y el pie injertados

**2026-09-03 · rama `0xultravioleta/x4-revertir` · nueve commits, sin push**

El dueno rechazo dos redisenos seguidos de la portada y pidio, textual: *"por
ahora quiero que lo reviertas al formato que estaba antes... la pagina principal
tiene que ser tal cual como la teniamos antes, con ese formato"*.

Esto no fue un tercer diseno. Fue una cirugia: se trajo el archivo entero de
`1c4c33d9`, se le injertaron tres cosas y se le saco una. **Ninguna
decision visual propia entro a la portada.**

---

## 1. El antes y el despues, en bytes

Los tres archivos medidos con LF (lo que devuelve `git show`), para que se
puedan comparar entre si.

| | bytes | `<img>` | `<pre>` | `<table>` | `<section>` |
|---|---:|---:|---:|---:|---:|
| **Antes** — `origin/main`, el rediseno rechazado | 54.539 | 28 | 3 | 4 | 2 |
| **La base** — `1c4c33d9`, el original que se pidio | 243.550 | 73 | 4 | 0 | 12 |
| **Despues** — esta rama | 235.525 | **74** | **0** | **0** | 11 |

Los 73 `<img>` son los iconos de las redes que el dueno extranaba; el 74 es
RLUSD. Los cuatro `<pre>` del original salieron todos.

`static/integrar.html`: 26.634 -> 30.015 bytes, de 4 a 5 bloques de codigo (uno
se reemplazo por dos).

---

## 2. Que se injerto

### (a) El menu de arriba, con sus ocho enlaces

Overview · Integrate · x402 · MCP · Networks · ERC-8004 · DX402 · Bazaar.

El markup es el **literal** de las otras nueve paginas, byte por byte, para que
un `diff` entre cabeceras siga saliendo vacio.

### (b) El pie, con sus diez enlaces

`/health` `/version` `/stats` `/events/live` `/supported` `/docs`
`/openapi.json` `/llms.txt` `/skill.md` · GitHub, mas el bloque "And after
this" hacia `/integrar`, `/x402` y `/networks`.

### (c) `static/rlusd.png`

El archivo que agrego el dueno en el checkout principal (447x447, 3.660 B, md5
identico). Entra al binario con el mismo `include_bytes!` que los otros 28
iconos, se sirve en `GET /rlusd.png`, y se suma a la fila de stablecoins del
hero. El rotulo pasa de "6 Stablecoins Supported" a **7**, en los dos idiomas.

RLUSD es la septima stablecoin y la unica que no es un contrato EVM/SVM: es una
moneda emitida de XRPL (`RLUSD_XRPL`, `src/network.rs:1197`). Por eso
`scripts/stablecoin_matrix.py` reporta **seis** y no siete -- parsea la tabla de
deployments EVM. **No es un bug del script; es lo que el script mide.**

La fila de stablecoins de la seccion de escrow queda en seis a proposito: el
escrow es PaymentOperator sobre EVM y RLUSD no vive ahi.

### La decision del injerto: NO se trajo `/uv.css`

Es lo unico que hubo que decidir, y la respuesta esta escrita en el propio
`<style>` de la portada.

`uv.css` no es una hoja de menu: declara su propia `:root`, su tipografia, su
escala y sus reglas de tabla y de titulo. Cargarla en la portada **volveria a
pintarla entera con el sistema que el dueno rechazo dos veces** -- exactamente
lo que este encargo existe para deshacer.

Asi que el menu y el pie se visten con 160 lineas *scoped* a `.nav` y
`.footer`, escritas en los tokens de la portada (`--bg-card`, `--accent`,
`--text-muted`, `--border`, `'JetBrains Mono'`). El borde, el fondo y el ritmo
vertical los sigue poniendo la regla `header {}` que ya existia en el archivo:
no se duplicaron.

Con el injerto se fueron tambien las reglas que solo vestian el header y el pie
viejos (`.header-content`, `.logo-section`, `.logo-image`, `.lang-switcher`,
`.lang-btn`, `.footer-content`, `.footer-links` y sus entradas responsive) y las
7 claves de i18n que quedaron huerfanas. `header.subtitle` y `status` ya estaban
muertas en `1c4c33d9` y quedaron como estaban: no las creo este trabajo.

### Dos cosas que el injerto arrastro, y que son requisitos del repo

- **La portada pasa a ingles por defecto y deja de leer `navigator.language`.**
  Es el camino C que ya cumplen las otras nueve y lo que ata el test
  `no_page_picks_a_language_from_the_browser`. La eleccion guardada en
  `x402.lang` sigue siendo lo unico que pisa el default.
- **El `<title>` lleva `data-i18n`.** El literal del archivo sigue siendo el
  ingles canonico que indexa un crawler; quien eligio ES ve la pestana en
  espanol. Lo exige `every_page_title_is_english_and_translatable`.

---

## 3. Que se saco

### (a) La tabla de "machine-readable surfaces": no hubo nada que sacar

El diseno original es **anterior** a esa tabla y nunca la tuvo (`grep -i
'llms.txt\|well-known\|machine.readable'` sobre `1c4c33d9:static/index.html`
devuelve cero). El revert la elimino por si solo.

**Las quince superficies se siguen sirviendo igual** -- ni una se borro ni se
movio. El pie injertado deja `/llms.txt` y `/skill.md` como la unica linea que
las nombra para el humano que las busque, que es el maximo que pidio el dueno.

### (b) Los bloques de codigo: de cuatro a cero

Textual: *"lo que no me gusta mucho es ver comandos como CLI commands ahi en la
pagina. Nadie se va a poner a hacer eso, entre menos muestres mejor"*.

Salio la seccion **"Integrate in Minutes"** entera: sus dos pestanas de SDK, sus
`npm install` / `pip install` y sus dos ejemplos. Con ella se fueron
`.code-section` y sus tres reglas, `pre {}` (ya no queda ningun `<pre>`), los
cinco colores `.code-*`, la funcion `switchSdkTab` y las claves `code.title` y
`sdk.networks`. **`code {}` se quedo**: una docena de `<code>` en linea la usa.

La portada quedo en **cero** bloques de codigo -- no en uno. Sin la seccion la
pagina se entiende: el grid de redes, las tarjetas de features, "What is x402?",
las cuatro secciones de producto y la lista de endpoints siguen todas ahi (la
lista NO es una `<table>`: son divs, por eso la portada marca cero tablas).

---

## 4. El ejemplo que no servia, y donde quedo el que si

Textual: *"los ejemplos que se muestran para conectar el facilitador no tienen
mucho sentido. Ni siquiera me permiten poner la URL que quiero cobrar. Si yo
quiero cobrar en el /world, no puedo saber como utilizar este pedazo de codigo
para hacerlo, pero eso si esta en el readme de los SDK."*

Tiene razon. El ejemplo que habia construia un pago suelto (`createPayment` con
`recipient` y `amount`) y **nunca mostraba la ruta**.

Los dos nuevos viven en `/integrar` -- la pagina del que integra, no la portada
-- y muestran ruta y precio juntos:

- **Hono + `createHonoMiddleware`**, con el paywall colgado de
  `app.get('/world', paywall, ...)`.
- **Flask + `FlaskX402`**, con `@x402.require_payment(amount_usd=Decimal("0.05"))`
  sobre `@app.route("/world")`.

### Verificados contra el codigo del SDK, no solo contra el README

| lo que se pego | donde existe |
|---|---|
| `createHonoMiddleware`, `HonoMiddlewareOptions.accepts` | `uvd-x402-sdk` **2.76.0**, `src/backend/index.ts:1690` y `:239` |
| `scheme` omitido | el SDK lo completa con `'exact'` en `buildRequirementFromAcceptance`, `:1256` |
| facilitador por defecto | `DEFAULT_FACILITATOR_URL`, `src/facilitator.ts:21` -- ya es este |
| `FlaskX402(app, recipient_address=...)` | `uvd-x402-sdk` **0.72.0**, `integrations/flask_integration.py:46` |
| `require_payment(amount_usd=...)` | idem, `:115` |
| facilitador por defecto | `X402_FACILITATOR_URL` default en `init_app` -- ya es este |
| USDC en Base `0x8335...2913` | `src/network.rs:729` |

**Ningun README esta vencido.** Los dos muestran ruta y precio juntos: el de
Python alrededor de `:246` (Flask) y `:283` (FastAPI), el de TypeScript en
`:78-140` con `app.get('/api/premium', paywall, ...)`. El defecto estaba en la
portada, no en ellos. No hay nada que anotarle a c0der por este lado.

Se sumo una linea sobre el precio, porque los dos SDK lo dicen distinto: entero
on-chain en TypeScript (`50000`), dolares en Python (`0.05`). Equivocarse ahi
por un factor de un millon es de lo primero que muerde.

---

## 5. El gate visual, acotado

`858cad3c` agrego `sistema_visual_tests`: un solo eje, sin centrado, la regla
del borde, una hoja compartida, un `h1` y nada entre el header y ese `h1`.

**La portada original no lo cumple y no tiene que cumplirlo**, en tres clausulas
concretas (medidas, no supuestas): trae su propio `<style>`, su propia `:root` y
sus dos familias de Google; **no tiene ningun `<h1>`**, asi que el chequeo de
"nada entre el header y el h1" no tiene que medir; y lleva **24 `<h4>`**, un
nivel que el gate prohibe de plano. (Los `<h2>` no son el problema: tiene dos.)
Cada una de esas cosas es el diseno que se pidio conservar, no deriva.

`static/index.html` salio de `PAGINAS` y entro en `EXCLUIDAS`, **una lista de
una sola entrada con el porque escrito al lado**. El test no se borro: las otras
nueve lo siguen pasando y los once tests del modulo estan en verde.

Para que la exclusion no sea una puerta abierta, la portada sigue atada por:

- **`la_portada_lleva_la_cabecera_y_el_pie_compartidos`** (nuevo): compara su
  cabecera contra la de `/integrar` normalizando blancos y `aria-current` -- la
  portada se sirve con LF y las otras nueve estan en CRLF --, exige los ocho
  `data-nav`, y exige los nueve enlaces del pie.

  **Probado en sus dos mitades en rojo**, una compilacion cada una: sacandole el
  `data-nav` a un enlace del menu falla con *"the landing's menu has drifted
  from the one the other nine carry"*, y sacandole `/skill.md` al pie falla con
  *"the landing's footer lost /skill.md"*. `static/index.html` quedo
  byte-identico despues (`cmp`).
- Los cinco **`i18n_tests`**, que nunca la dejaron.

### `scripts/verify_landing_canonical.py`

Miraba dos claves que solo existen en el rediseno (`networks.summary`,
`feat.reach.n`). Ahora mira las dos que la portada original **si** tipea:

| clave | productor | valor |
|---|---|---|
| `erc8004.networksTitle` | `src/erc8004/mod.rs` (total) | 21 |
| `x402r.networksTitle` | `src/payment_operator/addresses.rs` | 9 |
| `#ovr-erc8004-networks` (markup) | idem erc8004 | 21 |

El conteo de mainnets de pago **deja de chequearse porque deja de estar
tipeado**: la portada original no lo afirma en ninguna frase, muestra el grid de
tarjetas. La regla que este archivo ya se habia puesto el 2026-09-02 es que se
chequea lo que se escribe a mano; un numero que no se escribe no puede derivar.

Probado en verde y en **cuatro rojos distintos**, cada uno con su mensaje:

1. `es` diverge sola de `en` (19 vs 21) -> dos drifts, incluido el de "mismo
   documento, una URL".
2. Los dos diccionarios de acuerdo en un numero que la fuente no tiene (14 vs
   9) -> dos drifts.
3. El stat card movido a 30 -> un drift.
4. La frase reescrita de forma que el numero deja de poder leerse -> el drift de
   "the number is now unchecked".

`static/index.html` quedo byte-identico despues de las cuatro pruebas (`cmp`).

---

## 6. Lo que no se toco

- Las otras nueve paginas (`/mcp`, `/x402`, `/networks`, `/dx402`, `/erc8004`,
  `/bazaar`, `/stats`, `/events/live`). Solo `/integrar`, y solo para los
  ejemplos.
- Las quince superficies agenticas y sus tests. Ninguna se borro ni se movio.
- `POST /mcp`, `verify`, `settle`, `/supported`.
- Las dos fuentes y los 28 iconos que ya se servian del binario: los usan las
  otras paginas. La portada usa lo suyo.
- `static/index.md` (la version markdown de `/`): describe el **servicio**, no
  el markup de la portada, y ya listaba RLUSD entre las siete stablecoins.

---

## 7. El binario local, mirado

Levantado en `127.0.0.1:8402` con `ENABLE_WRITER_LEASE=false` y credenciales
AWS falsas -- un binario local con credenciales reales intenta tomar el writer
lease de **produccion**, medido el 2026-09-02.

**La portada** (`GET /` -> 200, 235.421 B -- la corrida es sobre el binario de
`4498b9cb`; el commit siguiente le suma 104 B al **texto de un aviso de consola**
y no cambia nada de lo que se ve, asi que el archivo final mide 235.525 B):

| | |
|---|---|
| `<header class="nav">` | 1, con los **8** `data-nav` |
| `<footer class="footer">` | 1, con sus 10 enlaces |
| `<pre>` | **0** |
| `<table>` | **0** |
| `<img>` | **74** |
| tarjetas del grid de redes | **39**, cada una con icono, saldo y sus pills |
| RLUSD | presente, y el rotulo dice **7 Stablecoins** |

Leida como texto, la pagina abre asi: el menu de ocho, EN/ES, *"Gasless
Micropayments for the Agentic Economy"*, la barra de siete stablecoins, y el
grid de redes con Avalanche, Base, Celo, HyperEVM, Polygon... Cierra con el pie
nuevo. Es la portada de antes con el menu y el pie de ahora.

**Los ocho destinos del menu**: los ocho dan 200.
**Los enlaces del pie**: los nueve internos dan 200 (`/docs` da 303 al `/docs/`
de Swagger, que da 200).
**Los 29 iconos que la portada referencia**: 29 de 29 dan 200, RLUSD incluido.
**Las quince superficies agenticas**: 15 de 15 dan 200. Ninguna se movio.

Dos cosas del entorno local que NO son defectos, y son las mismas que documento
el handoff del 2026-09-02: sin `X-Forwarded-For` el `SmartIpKeyExtractor` de
`tower_governor` contesta 500 `Unable To Extract Key!` en las rutas que lleva
gobernadas (`/mcp` entre ellas). Con el header -- que detras del ALB siempre
esta -- da 200. Y `/version` dice `0.0.0` en local porque `FACILITATOR_VERSION`
lo pasa el build de CI.

Ademas: las nueve variables CSS que usa el injerto estan declaradas en el
`:root` de la propia portada (ninguna falta, que es como se apagan reglas
enteras en silencio), y los dos bloques de JavaScript del documento pasan
`node --check`.

---

## 8. Los nueve commits

| hash | que |
|---|---|
| `72909f8f` | `revert(portada)`: la home vuelve a `1c4c33d9`, tal cual |
| `cd25fb8c` | `feat(portada)`: injerta el menu y el pie, sin traer `uv.css` |
| `853de44c` | `feat(rlusd)`: el logo entra al binario y a la portada |
| `7a70ceb6` | `feat(portada)`: fuera los bloques de codigo; queda en cero |
| `858a6dfe` | `feat(integrar)`: el ejemplo pasa a mostrar como se cobra UNA ruta |
| `4498b9cb` | `test(gate)`: el sistema visual gobierna nueve paginas, no diez |
| `4910eea7` | `fix(portada)`: el aviso de consola ya no manda a buscar un 21 que no existe |
| `d2b04ad6` | `docs(gate)`: el comentario decia h2 donde la portada tiene h4 |
| *(HEAD)* | `docs(handoff)`: este documento. Sin hash a proposito: un commit no puede citar el suyo propio sin cambiarlo. |

`cargo check` en verde en cada uno, y al final la suite completa:

```
cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1
  lib   739 passed; 0 failed
  main  771 passed; 0 failed
  + dx402_anchor_sig_cross 3, dx402_cross_seal 6, dx402_vector_gen 1,
    escrow_integration 9, escrow 1 (11 ignored)
  exit 0
```

**Cero push, cero deploy, cero terraform, cero escritura fuera del worktree.**
