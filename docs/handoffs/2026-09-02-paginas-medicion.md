# Las paginas del facilitador: la medicion previa y el plan

**Fecha:** 2026-09-02
**Rama:** `0xultravioleta/x4-paginas`, desde `origin/main` = `1c4c33d9` (verificado con `git log -1`)
**Worktree:** `/mnt/c/Users/lxhxr/orca/workspaces/x402-rs/x4-paginas` (WSL)
**Alcance:** medir y planear. Ni una linea de `static/` o de `src/` cambia en este commit.

---

## 0. Dos trampas del entorno, medidas hoy, antes de cualquier otra cosa

**1) El `.git` del worktree apunta a una ruta de Windows.** `cat .git` decia
`gitdir: Z:/ultravioleta/dao/x402-rs/.git/worktrees/x4-paginas` y el git de WSL
respondia `fatal: not a git repository` a todo. Reescrito a la ruta POSIX:

```
gitdir: /mnt/z/ultravioleta/dao/x402-rs/.git/worktrees/x4-paginas
```

**2) El checkout esta en CRLF y el repositorio en LF.** Con la configuracion
que hay (`core.autocrlf=false`), `git status` mostraba **392 archivos
modificados** sin que nadie los hubiera tocado: `git diff VERSION | cat -A`
devuelve `-2.10.0` / `+2.10.0^M`. Con el override el arbol esta limpio:

```bash
git -c core.autocrlf=true status --short   # 3 lineas, todas untracked (contracts, target, node_modules)
```

**Regla operativa de esta rama: TODO comando de git que toque el indice va con
`git -c core.autocrlf=true`.** Sin eso, un `git add static/index.html` mete
243 KB de CRLF al repositorio y el diff deja de ser legible. No se cambia la
config: `git config` escribe en `.git/config` del repo comun, que vive en
`/mnt/z/ultravioleta/dao/x402-rs` y es del dueno.

**3) No hay interop de Windows desde este WSL.** `orca.exe`, `git.exe` y
`cmd.exe` fallan los tres con `cannot execute binary file: Exec format error`
(no existe `/proc/sys/fs/binfmt_misc/WSLInterop`). El CLI de Orca **no se puede
ejecutar desde esta terminal**, asi que el `worker_done` de esta tarea queda
escrito literal al final del handoff de cierre para que lo despache quien pueda.

---

## 1. Como se agrega una pagina nueva sin romper nada

`static/` **no se sirve desde disco**. No hay `ServeDir` en `src/`; cada archivo
entra al binario con `include_str!` y tiene su `.route()` escrita a mano. Un
archivo nuevo en `static/` que no tenga las dos cosas **no existe** para el
servicio. La receta completa, con los cinco tests que se ponen rojos si falta un
paso:

| # | Paso | Donde | Que se rompe si falta |
|---|---|---|---|
| 1 | Escribir `static/<pagina>.html` | `static/` | nada (el archivo es inerte) |
| 2 | Handler con `include_str!` y `content-type: text/html; charset=utf-8` | `src/handlers.rs`, junto a `get_bazaar` (`:2304`) | la ruta no compila |
| 3 | `.route("/<pagina>", get(get_<pagina>))` en `routes()` (`src/handlers.rs:765`) | idem | 404 |
| 4 | Fila en `static/sitemap.xml` **con `<lastmod>`** | `static/sitemap.xml` | `the_sitemap_stamps_every_url` (`:13144`) |
| 5 | Alta en `SERVED_ELSEWHERE` | `every_internal_link_resolves_to_a_served_route` (`:13204`) | rojo apenas `llms.txt` o `index.md` la enlacen |
| 6 | Linea en `llms.txt`, `index.md`, `.well-known/api-catalog` | `static/` | `llms_full_txt_is_in_sync` si se toca una de las cuatro fuentes |

**Lo que NO hay que tocar.** `agentic_routes()` (`:269`) tiene su propio test de
conteo, `the_table_covers_every_route` (`:12808`), que compara el numero de
`.route(` dentro de esa funcion contra las filas de `SURFACES`. Las paginas
HTML **no van ahi** — van en `routes()`, que es donde ya viven `/`, `/bazaar`,
`/stats` y `/events/live`. Meter una pagina en `agentic_routes()` pone rojo ese
test y ademas la haria fallar `no_surface_is_html_or_the_landing_page`, que
prohibe explicitamente que una superficie agentica sea HTML.

**`llms-full.txt` es generado.** Lo arma `scripts/build_llms_full.sh` a partir de
`llms.txt` + `index.md` + `skill.md` + `auth.md`, y `llms_full_txt_is_in_sync`
falla si el archivo commiteado deja de coincidir. Tocar cualquiera de las cuatro
fuentes obliga a regenerarlo en el mismo commit. Lo mismo con el sha256 de
`skill.md`, que viaja dentro de `.well-known/agent-skills/index.json`.

### El caso especial de `/mcp`

`POST /mcp` ya es el servidor MCP y **no se toca**. `GET /mcp` hoy contesta un
405 JSON (`get_mcp_no_stream`, `src/mcp.rs:529`), registrado en la misma linea
que el POST:

```rust
.route("/mcp", post_service(service).get(get_mcp_no_stream))   // src/mcp.rs:645
```

La pagina humana reemplaza **solo el handler del GET**. El cambio es de una
linea en `mcp_routes()` mas el handler nuevo; el `post_service` queda intacto.
Se caen dos afirmaciones escritas que hay que corregir en el mismo commit:

- el test `get_mcp_answers_a_json_405` (`src/mcp.rs:1033`), que verifica que el
  GET no sea HTML — pasa a verificar exactamente lo contrario;
- `static/skill.md` seccion 10, que dice *"`GET /mcp` answers `405` with a JSON
  body"*. Es la fuente de `llms-full.txt`: hay que regenerarlo.

---

## 2. Como funciona el i18n hoy (medido, no supuesto)

**Un solo archivo por pagina.** El diccionario es un objeto JS literal dentro
del propio HTML: `const translations = {` en `static/index.html:2880`, con
`en:` en 2881 y `es:` en 3022. `updateTranslations(lang)` (`:3167`) recorre
`[data-i18n]` (`textContent`) y `[data-i18n-html]` (`innerHTML`, para valores
con `<code>` o `<strong>`), y una clave faltante **deja el texto del HTML** y
avisa por `console.warn`. Esa es exactamente la falla silenciosa que los dos
tests nuevos vienen a cerrar: la pagina se ve bien y simplemente deja de
cambiar de idioma.

Estado de las cuatro paginas:

| pagina | nodos `data-i18n` | clave de storage | idioma inicial sin eleccion previa | `<title>` |
|---|---|---|---|---|
| `index.html` | 143 | `x402.lang` | **`navigator.language`** (`:3309`) | sin `data-i18n`, ingles |
| `stats.html` | 30 | `x402.lang` | `'en'` — la linea `navigator.language` (`:213`) es **inalcanzable**: `LANG` ya vale `'en'` o un valor valido del storage | sin `data-i18n`, **espanol** |
| `events-viewer.html` | 18 | `x402.lang` | `'en'` — misma linea muerta (`:154`) | sin `data-i18n`, **espanol** |
| `bazaar.html` | 13 (+1 `data-i18n-ph`) | **`uvd-lang`** | **`navigator.language`** (`:347`) | sin `data-i18n`, ingles |

`bazaar.html` tiene ademas el atributo `data-i18n-ph` (traduce un `placeholder`);
el test N2 tiene que mirar los **tres** atributos, no dos.

Dos desvios del encargo, los dos del corte 0:

1. **Dos claves de storage.** `bazaar.html:343` y `:347` usan `uvd-lang`; las
   otras tres usan `x402.lang`. Por eso elegir espanol y navegar al Bazaar
   pierde la eleccion. Se unifica a `x402.lang` **migrando el valor viejo** si
   existe, para no resetear a quien ya visito.
2. **`navigator.language` es el `Accept-Language` del browser**, y el encargo
   dice que la pagina abre en INGLES por defecto y solo la eleccion explicita
   manda. Alcanzable en dos paginas (`index.html:3309`, `bazaar.html:347`) y
   muerto en las otras dos (`stats.html:213`, `events-viewer.html:154`, donde
   `LANG` ya vale `'en'` antes del `if`). Se saca de las cuatro: en dos es un
   cambio de comportamiento y en dos es sacar una linea que miente sobre lo que
   la pagina hace.

Y una consecuencia del camino C que hay que respetar al tocar los `<title>`:
**el literal del HTML se indexa, y tiene que ser INGLES.** `stats.html` y
`events-viewer.html` tienen hoy el titulo en espanol en el markup; se invierten
(ingles en el HTML, espanol en el diccionario `es`).

### Inventario de claves i18n que hace falta

Todo lo nuevo entra en `en` **y** en `es` en el mismo commit; el test N1 lo
cuida. Prefijos elegidos para no chocar con los 137 existentes:

**Fase 1 — seccion MCP en la landing (`mcp.*`), ~22 claves**

```
mcp.badge  mcp.title  mcp.description
mcp.endpoint.label  mcp.transport.label  mcp.transport.value
mcp.clients.title  mcp.clients.claudeCode  mcp.clients.claudeDesktop  mcp.clients.generic
mcp.tools.title  mcp.tools.th.tool  mcp.tools.th.rest  mcp.tools.th.money
mcp.tools.supported  mcp.tools.accepts  mcp.tools.verify  mcp.tools.settle
mcp.tools.no  mcp.tools.yes
mcp.warning.title  mcp.warning.text
mcp.link.page  mcp.link.card  mcp.link.skill
```

**Fase 1 — hero, endpoints, pie (claves sueltas), 6 claves**

```
hero.baseUrl        (la URL base, hoy no esta escrita para humanos en ningun lado)
hero.zeroFee        ("0% facilitator fee" — hoy solo esta en llms.txt, para maquinas)
endpoints.mcp       (la descripcion de la fila /mcp)
endpoints.group.mcp (el grupo, si la fila no entra en "Status & Info")
footer.mcp
footer.status       (ya existe: se reetiqueta el destino, no la clave)
```

**Fase 1 — titulos de las cuatro paginas, 4 claves**

```
page.title          (index.html)
title               (bazaar.html — su diccionario es plano, sin prefijos)
page.title          (stats.html, events-viewer.html — diccionarios propios)
```

**Fase 1 — el selector del Bazaar, 2 claves** (`opt.en`, `opt.es` para los
`<option>`, que hoy no tienen `data-i18n`).

**Fase 2 — `static/mcp.html` (`mcpPage.*`), ~45 claves**, diccionario propio del
archivo, con la estructura calcada de la guia de describe-net:
`mcpPage.title` · `mcpPage.intro` · `mcpPage.does.*` / `mcpPage.doesNot.*` ·
`mcpPage.transport.*` · `mcpPage.loop.*` · `mcpPage.tools.*` ·
`mcpPage.errors.*` · `mcpPage.ratelimit.*` · `mcpPage.traps.*` · `mcpPage.links.*`.

**Fase 3 — `static/networks.html` (`net.*`), ~15 claves.** La tabla se llena por
`fetch('/supported')`, asi que solo llevan traduccion los encabezados y la
prosa; **ningun numero de red se escribe a mano**.

---

## 3. Los dos tests de i18n (N1 y N2)

Van en Rust, en `src/handlers.rs`, porque son los que corre CI. Leen el HTML con
`include_str!` — el mismo texto que se sirve — y parsean el objeto `translations`
por conteo de llaves.

- **N1 (paridad):** toda clave de `en` esta en `es` y al reves, en las cuatro
  paginas. Falla nombrando la clave y el idioma que le falta.
- **N2 (cobertura):** toda clave usada en el HTML (`data-i18n=` y
  `data-i18n-html=`) esta definida en los **dos** diccionarios.

Los dos se verifican **por mutacion**, no por color: se borra una clave a mano,
se confirma el rojo, se restaura. El resultado de esa prueba se escribe en el
handoff de cierre. Un test verde que mira el objeto equivocado es peor que no
tener test.

---

## 4. `verify_landing_canonical.py`: por que no esta en CI y como entra

El docstring del script pide correrlo *"en cada deploy (pre-flight)"* y
`grep -rn verify_landing_canonical .github/` da **0**: nunca se cableo.

Lo que compara hoy son cuatro numeros del **markup ingles** de `index.html`
contra tres productores: `/supported` (vivo), `src/payment_operator/addresses.rs`
y `src/erc8004/mod.rs`. Los literales medidos hoy, iguales en las tres
superficies (markup, diccionario `en`, diccionario `es`):

| clave | markup | `en` | `es` |
|---|---|---|---|
| `sdk.networks` | 21 | 21 | 21 |
| `erc8004.networksTitle` | 21 | 21 | 21 |
| `x402r.networksTitle` | 9 | 9 | 9 |

**El diccionario `es` no lo mira nadie.** Hoy coincide; manana no tiene por que.
Ademas `features.reputation.description` y `endpoints.erc8004Note` repiten el
21 en prosa, en los dos idiomas.

Plan, en dos partes:

1. **Ampliar el script** para que las tres superficies (markup, `en`, `es`)
   tengan que coincidir entre si y con los productores.
2. **Cablearlo al job `Build & test`** de `.github/workflows/ci.yaml` (el que ya
   corre los tests, lineas 35-51) en un **modo `--offline`** nuevo: se saltea el
   unico productor que necesita red (`GET /supported`) y corre todo lo demas —
   escrow, ERC-8004 y la paridad EN/ES, que es justo la deriva que hay. Un check
   de CI que le pega a produccion se pone rojo cuando produccion se cae, que es
   el peor momento posible para bloquear un deploy. La variante viva
   (`--url https://facilitator.ultravioletadao.xyz`) queda para el pre-flight.

---

## 5. El binario local

Levanta con la receta de `docs/handoffs/2026-09-02-superficies-agenticas-listo.md`
seccion 2 (clave efimera de un solo uso, sin fondos, nunca escrita a disco:
solo hace falta para que `ProviderCache::from_env()` no aborte). Dos cosas del
entorno local que **no son defectos** y ya estaban documentadas: cualquier ruta
inexistente da **500** (`Unable To Extract Key!` de `tower_governor`, que sin ALB
no encuentra `X-Forwarded-For`), y `/version` dice `0.0.0` porque
`FACILITATOR_VERSION` solo lo pasa el build de CI.

---

## 6. Orden de trabajo

| fase | que | desplegable sola |
|---|---|---|
| 0 | esta medicion | — |
| 1 | corte 0: MCP en la landing, fila `/mcp`, URL base + 0% fee, GitHub al fork, `x402.lang` unificada, `<title>` bilingues, N1+N2, `Content-Language`, script en CI, pie | **si** |
| 2 | `static/mcp.html` en `GET /mcp` | si |
| 3 | `static/networks.html` desde `/supported` vivo, muro fuera de la home | si |
| 4 | hub + `/x402`, `/bazaar`, `/dx402`, `/erc8004`, `/integrar` | una pagina por commit |

Si el tiempo o el contexto se acaban, se corta **despues** de una pagina
completa. Ninguna pagina queda a medias.
