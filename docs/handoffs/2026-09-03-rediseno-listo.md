---
titulo: Rediseño del sitio del facilitador — implementado
fecha: 2026-09-03
autor: worker de Orca (claude-opus-5, effort xhigh) en x4-rediseno
estado: implementado, probado, SIN pushear
rama: 0xultravioleta/x4-rediseno (desde origin/main 72c64076)
spec: /mnt/z/ultravioleta/dao/c0der/docs/plans/facilitador-rediseno.md
tags:
  - type/handoff
  - domain/frontend
  - domain/rediseno
---

# El rediseño, implementado

> **QUÉ:** las seis fases de §8 de la especificación, en siete commits sobre
> `0xultravioleta/x4-rediseno`. Las diez páginas son un solo documento con una
> hoja, dos fuentes propias servidas desde el binario, una cabecera idéntica y
> un `h1` que contesta antes que nada.
>
> **POR QUÉ:** las seis quejas del dueño eran una sola — no había sistema. Las
> seis cierran con su verificación pegada abajo.
>
> **RIESGO:** nada está desplegado. `git push` sobre `main` **es** un despliegue
> a producción; el push lo hace c0der, no este worker.

## 1. Los siete commits

| commit | asunto |
|---|---|
| `19178c83` | feat(sitio): plomeria del rediseno - uv.css, dos fuentes propias y 28 PNG recodificados |
| `28bdf3f5` | refactor(sitio): el caparazon compartido en nueve paginas - una cabecera, un pie, una hoja |
| `13da9082` | feat(sitio): la portada, de quince bloques a ocho - y el gate de CI en el mismo commit |
| `7180895b` | feat(sitio): las paginas de producto reciben lo que la portada tenia de mas |
| `858cad3c` | test(sitio): el gate del sistema visual, y la limpieza |
| `29b2fcc6` | fix(sitio): la raya de inciso en la portada, donde el generador la habia dejado en guion |
| `ad574c68` | test(mcp): el test de negociacion de mcp.md deja de fijar el titular |

La fase 0 de la spec (re-subsetear las fuentes, parchar `uv.css`) no produce un
commit propio: no toca el repo hasta que sus salidas entran con la fase 1.

## 2. El antes y el después, página por página

```
pagina             antes B  despues B     delta   h1      h2       <style>   centrados
index              148,750     54,539   -94,211   0->1    2->8    1->0      37->0
integrar            22,117     26,634    +4,517   1->1    4->4    1->0       0->0
x402                28,141     37,595    +9,454   1->1    5->5    1->0       0->0
mcp                 43,527     43,611       +84   1->1    8->8    1->0       0->0
networks            37,628     50,489   +12,861   1->1    2->5    1->0       0->0
erc8004             32,339     41,262    +8,923   1->1    7->7    1->0       0->0
dx402               26,396     26,485       +89   1->1    6->6    1->0       0->0
bazaar              36,686     31,613    -5,073   1->1    0->7    2->0       2->0
stats               19,590     21,733    +2,143   1->1    4->4    1->0       0->0
events-viewer       14,540     16,703    +2,163   1->1    0->0    1->0       1->0
TOTAL HTML         409,714    350,664   -59,050

PNG (28)         3,139,656     33,861 -3,105,795   (los 29 de antes incluian hedera.png, ahora borrado)
uv.css                   0     32,074
x402.js                  0      5,508
fuentes+OFL              0     36,236
```

**El HTML baja 59.050 B y los PNG bajan 3.105.795 B (91×).** Contra eso entran
73.818 B de hoja, mapa de iconos y fuentes, que viajan una vez y se cachean: el
saldo del binario es **−3,0 MiB**.

Las seis páginas que *crecen* crecen porque **recibieron** contenido: las 255
líneas de endpoints que vivían en la portada aterrizaron donde se explican, y
`/networks` además se quedó con las 185 líneas de wallets. Nada se borró.

| página | qué cambió |
|---|---|
| `/` | de 15 bloques a 8. Primera pantalla nueva: `h1`, bajada, dos puertas de rol, `curl` con su salida impresa, línea de prueba enlazada a `/stats`. Tabla D (las features, con su cifra), el módulo *qué es x402* en tres frases, el hub como tabla de 6 filas con Zama de vuelta a una fila, los SDK, la pared de 21 iconos, `dl.kv` con lo que movió, las 16 superficies de máquina y la API. 29.800 B de `<style>` a 0 |
| `/integrar` | `h1` nuevo; **nivel 0** (el `curl` que funciona antes de la bifurcación); los dos caminos pierden la caja y ganan `id="selling"` / `id="buying"`; recibe 4 rutas |
| `/x402` | `h1` y bajada nuevos; glosario `dl.defs` de los cuatro términos; recibe 11 rutas repartidas en tres `h3` |
| `/mcp` | sólo el caparazón y el `h1`: era la mejor estructurada de las diez y no se reordenó |
| `/networks` | `h1` y bajada nuevos; la primera columna usa el **mapa** y no `split('-')[0]`; Tabla A (resumen por familia, con totales vivos); Tabla C (16 direcciones que pagan el gas) |
| `/erc8004` | `h1` y bajada nuevos; las dos tablas de rutas se funden en una de 12 filas |
| `/dx402` | lo mínimo: `h1`, bajada, caparazón |
| `/bazaar` | pierde Google Fonts y los dos `<style>`; los seis `div.section-title` pasan a `h2` (tenía **cero** secciones en el árbol del documento); los `h4` bajan a `h3`; recibe 2 rutas |
| `/stats` | caparazón, `h1`, y el markup por defecto pasa a inglés (estaba en español, así que un crawler leía otra frase) |
| `/events/live` | ídem, más el punto de estado dibujado con `.dot` del sistema |

## 3. Los ocho comandos de §10

```
############ LOS OCHO COMANDOS DE LA SECCION 10
# corridos en WSL, en /mnt/c/Users/lxhxr/orca/workspaces/x402-rs/x4-rediseno
# 2026-09-03T07:18:57Z | HEAD 29b2fcc6

=== (1) la hoja pide la fuente en la ruta que el binario sirve   -> 2
2

=== (2) ninguna clase del markup queda sin regla en uv.css
=== (3) ningun var() apunta a un token que uv.css no declara
clases sin regla: ninguna
tokens usados y no declarados: ninguno
exit=0

=== (4) el chip no vuelve a duplicar el aire
chip OK

=== (5) el gate de la portada
exit=0

=== (6) no queda un solo Plex en el name table de las dos fuentes
  static/fonts/uv-sans.woff2 limpio
  static/fonts/uv-mono.woff2 limpio

=== (7) las rutas nuevas: son CINCO, no dos
5

=== (8) el gate del sistema, y en los dos estados
test handlers::sistema_visual_tests::cada_pagina_abre_con_su_h1_y_nada_se_interpone ... ok
test handlers::sistema_visual_tests::el_arbol_del_documento_no_pasa_de_nueve_h2_y_no_tiene_h4 ... ok
test handlers::sistema_visual_tests::el_chip_no_duplica_el_aire_del_png ... ok
test handlers::sistema_visual_tests::el_h1_mide_por_lo_menos_el_doble_del_cuerpo ... ok
test handlers::sistema_visual_tests::la_hoja_no_se_vuelve_un_bundle ... ok
test handlers::sistema_visual_tests::la_hoja_pide_las_fuentes_donde_el_binario_las_sirve ... ok
test handlers::sistema_visual_tests::la_hoja_tiene_un_solo_eje_una_paleta_y_un_solo_borde ... ok
test handlers::sistema_visual_tests::las_diez_cabeceras_son_la_misma ... ok
test handlers::sistema_visual_tests::ningun_var_apunta_a_un_token_que_no_existe ... ok
test handlers::sistema_visual_tests::ninguna_pagina_declara_tokens_ni_una_familia_tipografica ... ok
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 702 filtered out; finished in 0.03s
```

## 4. Las seis quejas, cerradas

```
########## QUEJA 1 - quedo todo muy disperso
  text-align:center en index.html   0  (era 37)
  border:1px en index.html          0  (era 64)
  hexes en index.html               0  (era 43)
  <style> en index.html             0  (era 1, de 29.800 B)
  div class="wallet en index.html   0  (eran 185 lineas)
  <style> en las diez paginas       0

########## QUEJA 2 - los iconos de las redes quedaron guardados
  chips en la portada               28  (>= 21; eran 0)
  el aire no se duplica             inset: 0  OK
  split('-')[0] en networks.html    1  (solo balanceKey)
  chipRed( en networks.html         1
  peso de los PNG                   33861 B en 28 archivos  (era 3.139.656 en 29)
  ninguno sobre el techo de 12 KB   (sin salida = ok)

########## QUEJA 3 - esos bordes todos picados
  ffcc00 (el amarillo de Zama)      0 en todo static/
  rgba unicos en uv.css             1  (la sombra negra)
  network-badge zama                0
  ^  border: 1px en uv.css          1  (.group)

########## QUEJA 4 - no veo los fitchers
  <h1 en index.html                 1  (era 0)
  elementos entre </header> y <h1>  0
  el primer h2 de la portada        What you get
  claves feat.* en el markup        16  (4 x t/n/d)
  emojis en el bloque de features   0  (eran 4)
  (las 12 de fila + 4 de encabezado = 16 apariciones de data-i18n="feat.")

########## QUEJA 5 - que es la letra; parece todo generico
  /fonts/v1/uv- en uv.css           2
   uv-sans.woff2 -> limpio | familia UV Sans | wght 400 - 700
   uv-mono.woff2 -> limpio | familia UV Mono | wght 400 - 700
  paginas que salen a un CDN        0
  las cinco rutas nuevas, contra el binario local:
    200  32074B  text/css; charset=utf-8  /uv.css
    200  5508B  application/javascript; charset=utf-8  /x402.js
    200  19052B  font/woff2  /fonts/v1/uv-sans.woff2
    200  12728B  font/woff2  /fonts/v1/uv-mono.woff2
    200  4456B  text/plain; charset=utf-8  /fonts/v1/OFL.txt

########## QUEJA 6 - informacion valiosa, bien estructurada, para todo el mundo
  index.html           h1=1 h2=8 nav=8 cur=1 h4=0
  integrar.html        h1=1 h2=4 nav=8 cur=1 h4=0
  x402.html            h1=1 h2=5 nav=8 cur=1 h4=0
  mcp.html             h1=1 h2=8 nav=8 cur=1 h4=0
  networks.html        h1=1 h2=5 nav=8 cur=1 h4=0
  erc8004.html         h1=1 h2=7 nav=8 cur=1 h4=0
  dx402.html           h1=1 h2=6 nav=8 cur=1 h4=0
  bazaar.html          h1=1 h2=7 nav=8 cur=1 h4=0
  stats.html           h1=1 h2=4 nav=8 cur=0 h4=0
  events-viewer.html   h1=1 h2=0 nav=8 cur=0 h4=0
  LAS DIEZ CABECERAS SON LA MISMA: hashes distintos -> 1
  el agente lee lo mismo que el humano:
    # Take payment for a single HTTP request — no account, no API key, no gas
    
    This is the x402 payment facilitator Ultravioleta DAO runs: your endpoint answers 402,
```

## 5. Los tests

```
cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1
  -> 7 suites, 1.528 tests, 0 fallos
```

De ésos, los que este trabajo agrega o toca:

| suite | qué exige |
|---|---|
| `sistema_visual_tests` (10, nuevos) | cero `<style>`/`:root`/`font-family`/CDN por página; un `h1` y nada entre `</header>` y él; `--fs-600/--fs-300 >= 2`; `h2 <= 9` y cero `h4`; **las diez cabeceras son la misma**; la hoja pide las fuentes en `/fonts/v1/`; el chip a `inset: 0`; un eje, una paleta, un borde; ningún `var()` huérfano; la hoja `<= 40 KB` |
| `i18n_tests` (5, intactos) | paridad EN/ES, toda clave usada definida, una sola clave de almacenamiento, cero `navigator.language`, `<title>` en inglés y traducible |
| `agentic_surface_tests` (12, intactos) | las dieciséis superficies siguen contestando con su content-type; `llms-full.txt` en sincronía con sus cuatro fuentes |
| `mcp::tests::get_mcp_negotiates_markdown` (corregido) | dejaba fijado el titular literal de `mcp.md`, que la fase 4 cambió a propósito. Ahora chequea la forma, no la frase |

**El gate se probó también en rojo**, uno por uno: la fuente en la ruta vieja
(`/fonts/`), el `inset` de vuelta en el chip, un destino menos en la cabecera de
`/mcp`, y un `var(--esp-2)` inventado. Los cuatro ponen su test en rojo. Un gate
que sólo se vio en verde no es un gate.

**El gate de CI de la portada** (`scripts/verify_landing_canonical.py`) también,
en las cuatro combinaciones:

```
--offline verde  exit=0     --offline con EN=99 y ES=21   exit=1
en vivo   verde  exit=0     en vivo con el markup en 99   exit=1
```

**Clippy:** ningún warning cae sobre el código agregado (las tres `const`, las
cinco rutas, los cinco handlers, el módulo de tests). Los 327 que reporta
`cargo clippy --all-targets` son preexistentes y no los conté contra una línea
base recompilada — ver §8.

## 6. El binario local, mirado

Levantado con la receta segura (`ENABLE_WRITER_LEASE=false`, credenciales AWS
falsas, `AWS_EC2_METADATA_DISABLED=true`, clave EVM efímera de un solo uso que
nunca se escribe a disco). Puerto 8402, detenido al terminar.

```
las cinco rutas nuevas
  200  32074B  text/css; charset=utf-8                 /uv.css          cache 1 h, NO immutable
  200   5508B  application/javascript; charset=utf-8   /x402.js         cache 1 h
  200  19052B  font/woff2                              /fonts/v1/uv-sans.woff2   immutable 1 año
  200  12728B  font/woff2                              /fonts/v1/uv-mono.woff2   immutable 1 año
  200   4456B  text/plain; charset=utf-8               /fonts/v1/OFL.txt

las diez páginas   200 text/html las diez
las 16 superficies 200 las dieciséis
/hedera.png        404  (sigue sin ruta, y ahora tampoco está el archivo)
POST /mcp          sigue siendo el servidor MCP: tools/list devuelve las cuatro
GET  /mcp          sigue siendo la página humana
```

La portada servida, leída en orden:

```
[h1]      Take payment for a single HTTP request — no account, no API key, no gas.
[p.deck]  This is the x402 payment facilitator Ultravioleta DAO runs: your endpoint answers 402...
[p.roles] I want to charge for an endpoint · I want to pay for one
[p]       Paste this. It answers now, and it does not ask who you are:
[pre]     curl -sS https://facilitator.ultravioletadao.xyz/version
[p.out]   {"version":"2.10.0"}
[p]       Real, not a demo: 2,796 settlements on chain across 13 networks — failures are published on that page too.

los ocho h2:  What you get · What x402 is, in three sentences · What this facilitator runs ·
              Start in one line · Where it runs · What it has moved ·
              Machine-readable surfaces · The whole API

la pared:     EVM 14 redes (14 con imagen) · SVM 2 · NEAR 1 · Stellar 1 · Sui 1 · Algorand 1 · XRPL 1
```

**Sin navegador en este entorno.** Lo que está verificado es la estructura, el
orden de lectura, los bytes servidos y los content-type. Lo que NO está
verificado es el render: color, peso de la fuente, el aire entre bloques y si el
chip a 24 px se ve. **Esa parte la tiene que mirar el dueño.**

## 7. Lo que decidí distinto de la especificación, y por qué

Cinco cosas. Ninguna cambia una decisión de diseño; las cinco son la spec
corrigiéndose a sí misma o un detalle que el papel no podía ver.

1. **La tabla de superficies lleva 16 filas, no 13.** §5.1 dice trece y §6.1 de
   la misma spec dice *"son dieciséis, no quince"* y las enumera. Dieciséis es
   lo que devuelve `agentic_routes()`. Publicar trece habría dejado tres
   superficies sin mencionar en la página que existe para mencionarlas.

2. **La Tabla C lleva 16 direcciones, no 14.** Sui no viajaba en el bloque de la
   portada, pero `/networks` **ya publica su saldo de gas**: una dirección en una
   página y su saldo en otra es el defecto original en versión nueva. Las dos
   salen de `lambda/balances/handler.py`, que es la fuente autoritativa.

3. **Las dos tablas de rutas de `/erc8004` se funden en una.** La spec pedía
   agregar `h3 The routes` bajo el `h2 Reading it back` que ya tenía una tabla de
   seis rutas. Eso dejaba cuatro rutas dichas dos veces en la misma página, que
   es justo lo que el rediseño viene a sacar. La unión conserva las dos
   descripciones que sólo la curada tenía (que un 503 del owner lookup **no** es
   un 404, y que `/feedback/revoke` da 404 cuando no hay token) y suma las dos
   rutas que a ninguna de las dos le entraban.

4. **`redes.bajada` se llama `networks.summary`.** §4.5 nombra la clave
   `redes.bajada`, pero el parche a `verify_landing_canonical.py` de §8 —que es
   lo que corre en CI— deja `networks.summary` en `COUNT_KEYS`. Gana la clave que
   el gate sabe leer. El `h2` sí usa `redes.titulo`.

5. **`feat.reach.n` usa `·` y no `&middot;`.** `data-i18n` se aplica con
   `textContent`: la entidad saldría literal en pantalla. Y con `data-i18n-html`
   la expresión que `verify_landing_canonical.py` busca
   (`data-i18n="feat\.reach\.n"`) no engancha, y el gate pasaba a verde leyendo
   `None` — o sea, sin chequear nada. Se descubrió justamente probándolo en rojo.

## 8. Lo que quedó fuera

| # | qué | por qué |
|---|---|---|
| 1 | **El render, mirado por una persona.** | No hay navegador en este entorno. Está verificada la estructura y los bytes; el color, el aire y el tamaño real del chip los tiene que mirar el dueño |
| 2 | **La captura de componentes rehecha con los PNG al lado** (fase 0, punto 2) | Necesita un navegador. `componentes.html` no se despliega: es material de inspección |
| 3 | **El `gtag` de Google Analytics sigue en la portada** | Es la única URL externa que queda. No renderiza nada y sacarlo es una decisión de negocio, no mía. Si el dueño quiere el sitio sin terceros, es borrar seis líneas del `<head>` |
| 4 | **La línea base de clippy no se recompiló** | Verifiqué que ningún warning apunta a las líneas que agregué. Comparar el total contra `72c64076` costaba una recompilación completa |
| 5 | **`/events/live` se queda con cero `h2`** | Es un visor de una sola cosa. El tope de la spec es `<= 9`, no `>= 1` |
| 6 | **Sui no aparece en la Tabla A con sus tokens** | La spec la da como `— (feePayer)`, y así quedó. Si alguna vez liquida una stablecoin, la fila cambia |
| 7 | **No se corrigió la nómina §1.7 de c0der** (§9.3 de la spec) | Vive en `c0der`, y este worker no escribe ahí. La corrección: hoy fallan **0 de 39** filas de `/networks`, no "la mitad"; el defecto real es que no había suelo |

## 9. Lo que hay que saber antes de tocar esto

- **La cabecera se genera, no se tipea.** El generador quedó en el scratchpad de
  la sesión, no en el repo: si hay que cambiarla, se cambia en las diez y el test
  `las_diez_cabeceras_son_la_misma` avisa si quedó a medias. Vale la pena
  moverlo a `scripts/` la próxima vez que se toque.
- **`uv.css` va en 32.074 B contra el techo de 40.960.** Quedan ~8,8 KB. El
  techo está para subirlo a propósito, no para pasarlo de largo.
- **`bash` no arranca desde el shell de este worker** (`set -o pipefail` revienta
  porque `/bin/sh` es `dash`). `scripts/build_llms_full.sh` se replicó en Python
  respetando byte a byte el formato que el test de Rust rehace.
- **Este worktree tiene `core.symlinks=false`.** `target/` y `contracts/` son
  symlinks; un `git stash -u` los aplana a archivos de texto con la ruta adentro
  y `cargo` deja de compilar. Pasó una vez y se restauró con `ln -s`.
- **El `.git` del worktree apunta a una ruta de Windows.** Reescrito a
  `gitdir: /mnt/z/ultravioleta/dao/x402-rs/.git/worktrees/x4-rediseno` para que
  el git de WSL funcione. Todo git de esta sesión corrió con
  `-c core.autocrlf=true`: el árbol está en CRLF y sin esa bandera cada commit
  habría reescrito 176.834 líneas.

## 10. Un aviso para quien pushee

El diff de esta rama tiene **dos coincidencias** con el patrón que el hook de
pre-commit usa para bloquear private keys (`0x` + 64 hex). Son las **dos
direcciones públicas de Sui** de la Tabla C, y están verbatim en
`lambda/balances/handler.py` y en el `CLAUDE.md` del repo:

```
0xe7bbf2b13f7d72714760aa16e024fa1b35a978793f9893d0568a4fbf356a764a   mainnet
0xabbd16a2fab2a502c9cfe835195a6fc7d70bfc27cffb40b8b286b52a97006e67   testnet
```

Una dirección de Sui tiene exactamente la misma forma que una private key de
32 bytes, así que el grep no puede distinguirlas. **No hay ningún secreto en
este diff** (verificado también contra `PRIVATE_KEY=` y `mnemonic=`: cero
coincidencias). Si el hook las marca, es este falso positivo y no otra cosa.

## 11. Ping de recall (10 s)

> La hoja tenía las dos fuentes bien subseteadas, bien renombradas y pesando
> 31.780 B entre las dos, y **el sitio no las iba a cargar**. Sin mirar arriba:
> ¿por qué? Y la respuesta no es "están mal hechas".
>
> *(Segunda, más corta: el PNG del icono trae 75% de glifo y 25% de aire adentro.
> Si el CSS además le pone `inset: 4px` en un chip de 32 px, ¿a cuántos píxeles
> sale el glifo, y por qué ese número exacto es el que el dueño ya llamó "quedó
> por allá guardado"?)*
