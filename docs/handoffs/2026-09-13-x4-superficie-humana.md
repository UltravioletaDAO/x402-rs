---
date: 2026-09-13
tags:
  - type/handoff
  - domain/landing
  - domain/i18n
  - domain/seo
  - priority/p1
status: active
---

# La superficie humana: SEO, i18n, compresión, límite de tasa y el changelog que sí existía

**Versión:** 2.29.0 · **Worker:** `x4-superficie-humana`, encargado
el 2026-09-13 · **Base:** `origin/main` = `331d31d4` (#49, 2.28.0, sobre #48 2.27.0)

Diez filas chicas de la misma superficie. Cuatro de ellas traían la evidencia del
triage equivocada; están marcadas abajo y no se ejecutaron a ciegas.

## Qué cambió

| Fila | Estado | Qué |
|---|---|---|
| 76 | **refutada: ya estaba cerrada** | Una URL por página, sin `/es/` ni `hreflang`. Ahora además lo afirma un test |
| 78 | **cerrada** (la mitad ya estaba) | `text_surface()` emite `Content-Language: en`; las 10 páginas ya lo emitían |
| 79 | **cerrada, con otro mecanismo** | gzip **precalculado una vez por proceso** para lo compilado en el binario; no `CompressionLayer` (medido: empeora p95) |
| 80 | **cerrada** (evidencia corregida) | `meta description` + `og:` + `canonical` en la landing; `canonical` en las otras 9; clave `data-i18n-content` en las 10 |
| 81 | **cerrada** | Línea `Language (idioma)` en `llms.txt`, `skill.md`, `index.md`, `auth.md` |
| 82 | **cerrada con el cierre sustituido** (respuesta P3-A) | `docs/CHANGELOG.md` ya existía (tope 2.16.0): completo 2.17.0 → 2.26.0 + 2.29.0 y en orden |
| 83 | **cerrada: archivado** | `docs/MONETIZATION_MASTER_PLAN.md` → `docs/plans/_archivo/` con nota de por qué quedó obsoleto (rename, +19 líneas) |
| 84 | **cerrada según el SPEC** (respuesta P1-C) | Governor por IP para las 9 páginas HTML; publicar los límites para humanos queda como fila P2 |
| 85 | **cerrada** | `skill.md` §12 DX402 y §13 Bazaar; `llms.txt` lista `GET /dx402/evidence/{paymentId}` |
| 86 | **cerrada** | Recuento: quedaban 3 (+1 nombre propio), no 24. Cubiertos + test N3 |

### 76 — ya estaba cerrada

Medido contra producción antes de tocar nada: las 10 páginas responden `200` con la
misma URL para `Accept-Language: en` y `es`, sin redirecciones, y `grep -c hreflang
static/*.html` da `0` en las 10. El selector es de cliente (`x402.lang`), y el test
`no_page_picks_a_language_from_the_browser` ya prohibía `navigator.language`. Lo único
que agregué es que `every_page_has_a_translatable_description_and_one_canonical_url`
también falla si aparece un `hreflang`.

### 78 — `Content-Language` en `text_surface()`

La evidencia hablaba de "las 4 páginas humanas": las **10** ya mandaban
`content-language: en` (medido en prod). Lo que faltaba era `text_surface()`, que sirve
`llms-full.txt`, `robots.txt`, `sitemap.xml` y todas las tarjetas `.well-known`
(`src/handlers.rs`, `fn text_surface`). Test: `every_surface_declares_its_language`
recorre las 16 rutas de `agentic_routes()`.

### 79 — compresión: medí `CompressionLayer` y no lo puse

`CompressionLayer` comprime en cada request, en los mismos workers de tokio que
liquidan. Banco en `release`, localhost, `oha -z 8s -c 16` (saturado) y `-q 200`
(abierto), sobre los bytes reales de `static/index.html`, `llms-full.txt` y una página
de producción de `/discovery/resources?limit=100`:

| Ruta | Variante | Bytes | p95 saturado | p95 a 200 req/s |
|---|---|---:|---:|---:|
| `/` | sin compresión (prod hoy) | 251.484 | 0,82 ms | 1,21 ms |
| `/` | `CompressionLayer` nivel default | 37.718 | **16,97 ms** | 7,13 ms |
| `/` | `CompressionLayer` nivel fastest | 53.322 | 2,94 ms | 1,57 ms |
| `/` | **gzip precalculado (lo que entra)** | 37.461 | **0,25 ms** | **0,34 ms** |
| catálogo | sin compresión | 194.485 | 0,51 ms | 0,63 ms |
| catálogo | `CompressionLayer` default | 33.453 | 11,39 ms | 4,39 ms |
| catálogo | `CompressionLayer` fastest | 43.296 | 1,90 ms | 1,46 ms |
| respuesta tamaño settle | cualquiera | 169 | igual | igual |

El cierre de la fila pedía que el bench no empeorara p95. `CompressionLayer` lo empeora
20x en la landing, y el servicio ya tuvo dos P0 de CPU (2.21.1, 2.21.2). Lo que entra es
`handlers::precompressed_static`: los helpers que sirven un `include_str!` (`html_page`,
`text_surface`, `negotiated_response`, css, js, licencia) marcan la respuesta con
`StaticBody`, y el middleware entrega el gzip de ese documento, calculado la primera vez
y guardado por dirección+longitud. Mejora p95 porque escribir 37 KB cuesta menos que
escribir 251 KB. Todo documento estático sale con `Vary: Accept-Encoding`, comprimido o
no. `llms-full.txt`: 58.531 → 20.214 bytes.

Lo dinámico (`/supported`, catálogo, stats) **no se comprime** (respuesta P2-A). La
opción B queda como fila de backlog con esta medición.

### 80 — descripción, `og:` y `canonical`

La evidencia decía "0 descripciones, 4 hits de `og:`/`canonical`" en `index.html`.
Medido: la landing tenía **0** `og:` y **0** `canonical`, y las otras **9** páginas ya
tenían descripción y `og:` pero ninguna tenía `canonical`. Ahora:

- La landing tiene descripción, `og:type/url/title/description/image` y `canonical`.
- Las 10 páginas tienen `<link rel="canonical">` igual a su `og:url`.
- Cada descripción lleva `data-i18n-content="meta.description"`, con la clave en `en` y
  `es`. El runtime de cada página la aplica. El literal del HTML queda en inglés, que es
  lo que indexa un buscador (decisión del dueño del 2026-09-02: "solo inglés para los
  buscadores").

`ATTRIBUTES` del módulo `i18n_tests` aprendió `data-i18n-content`, así que N1/N2 también
cubren esa clave.

### 81 y 85 — los documentos agénticos

- **Idioma:** una línea `Language (idioma)` en `static/llms.txt:8`, `static/skill.md:10`,
  `static/index.md:15` y `static/auth.md:6`. Dice lo que es verdad: el documento está en
  inglés y no tiene traducción, y las páginas humanas son bilingües en la misma URL. **No
  existe "versión ES" de estos cuatro**, así que no la anuncié.
- **Secciones nuevas:** `static/skill.md:683` es §12 DX402 (cómo detectar que está
  apagado, que nunca falla un pago, en qué ramificar) y `:745` es §13 Bazaar. Van al final,
  así que ninguna sección existente cambió de número.
- **Artefactos regenerados:** `static/llms-full.txt` con la lógica de
  `scripts/build_llms_full.sh`, y el digest de `skill.md` en
  `static/.well-known/agent-skills/index.json`.

### 82 — el changelog existía

`docs/CHANGELOG.md` estaba en `docs/`, donde el CLAUDE.md manda los documentos. El
triage buscó en la raíz. Su tope era 2.16.0.

- **Agregado:** 2.17.0 → 2.26.0, verificadas contra `git show <merge>:VERSION` y el
  `mergedAt` de cada PR, más 2.29.0.
- **Reordenado:** 2.1.0 estaba debajo de 2.0.0.
- **CLAUDE.md:** decía que el tope era 1.64.0; corregido.

El cierre `git describe --tags --abbrev=0` no puede cumplirse. El tag más nuevo del repo
es `v2.0.2`, pero no es ancestro de `main`, y el alcanzable desde `HEAD` es `v1.50.1`.
El CI etiqueta imágenes en ECR, no commits. Respuesta P3-A: el cierre es "primera entrada
== `VERSION`". Faltan las entradas de 2.27.0 (#48), 2.2, 2.4, 2.5, 2.8 y 1.65–1.73. La de 2.28.0 la trajo
#49 y quedó debajo de 2.29.0 al rebasear.

### 83 — archivado, no borrado

Rename a `docs/plans/_archivo/MONETIZATION_MASTER_PLAN.md` más una nota de 19 líneas.
El blob se guardó en CRLF como el original, así que el diff es solo la nota. La nota dice:

- **No hay comisión:** el facilitador no cobra (`static/.well-known/x402`, `llms.txt`),
  así que el "Enterprise SLA $2,499/mes" no existe.
- **La postura pública** es la del dueño: `/` para el stack y `/integrar` para quien
  integra desde afuera.
- **Los "precios P0-P4" no son este plan.** Tratan de que el Bazaar y DX402 digan la
  verdad sobre los precios de *otros* vendedores. La evidencia del SPEC los tomaba como
  "lo que el facilitador cobra hoy", y eso es falso.

`docs/INDICE.md` no existe en este repo; `git grep MONETIZATION_MASTER_PLAN` solo
devuelve el CHANGELOG.

### 84 — límite de tasa a las páginas

`handlers::human_page_routes()` saca las 9 páginas HTML de `routes()`.
`human_page_routes_governed(per_ms, burst)` las monta bajo un governor por IP (una sola
cubeta para las nueve) y `main.rs` monta esa versión.

- **Default:** ráfaga 60, un token cada 500 ms.
- **Override:** `HUMAN_PAGES_RATE_BURST` y `HUMAN_PAGES_RATE_PER_MS`. No están en
  terraform, así que rige el default.
- **Fuera de la cubeta:** `/verify`, `/settle`, `/supported`, los documentos agénticos,
  logos, css y fuentes.
- **Tamaño:** es generoso a propósito, porque un NAT de oficina pone muchos lectores
  detrás de una IP.

La fila de origen (backlog interno, fuera de este repo) pedía otra cosa: *mostrarle a un humano* los
límites que ya están publicados para máquinas. Respuesta P1-C: el governor queda, y la
publicación va como fila P2.

### 86 — prosa sin clave

Recontado con un parser (mismo criterio que el informe del 2026-09-02: nodo de texto
visible de ≥3 palabras sin ancestro `data-i18n*`). Eran **3 + 1**, no 24:
"Confidential Payments on Ethereum Sepolia", "Solana Agent Registry" y
"Base Mainnet Contracts:", ahora con clave en `en`/`es`. El cuarto, "SKALE Base Sepolia",
es un nombre propio y va en la lista blanca. El test
`the_landing_shows_no_prose_outside_the_dictionary` implementa el escáner en Rust y
también falla si la lista blanca nombra algo que la landing ya no muestra.

## Prueba rojo/verde

Cada test nuevo se corrió contra una mutación que devuelve el código a como estaba, y
después contra el código restaurado:

| Fila | Mutación (vuelve al código viejo) | Test | Contra la mutación |
|---|---|---|---|
| 78 | `text_surface()` sin `Content-Language` | `every_surface_declares_its_language` | **ROJO**: `/llms-full.txt does not declare Content-Language: en` (left `None`) |
| 79 | `main.rs` sin el middleware de compresión | `the_pages_are_metered_in_production_and_only_the_pages` | **ROJO** |
| 79 | `text_surface()` no marca `StaticBody` | `a_text_document_is_gzipped_for_a_client_that_asks` | **ROJO**: content-encoding `None`, esperado `gzip` |
| 84 | router de páginas sin governor | `a_burst_at_a_human_page_gets_429_for_that_address_only`, `every_human_page_spends_the_same_bucket` | **ROJO** los dos: `200`, esperado `429` |
| 84 | `/stats` de vuelta en `routes()` sin límite | `the_pages_are_metered_in_production_and_only_the_pages` | **ROJO** |
| 80 | las 10 páginas como están en `origin/main` | `every_page_has_a_translatable_description_and_one_canonical_url` | **ROJO**: `static/index.html: expected exactly one <meta name="description">` (left `0`) |
| 86 | la landing como está en `origin/main` | `the_landing_shows_no_prose_outside_the_dictionary` | **ROJO** |

Las primeras dos filas se corrieron antes del rebase y las otras cinco después, siempre
contra el mismo código de test. **VERDE** con el código restaurado: los 7 están en la
corrida completa de pre-CI de abajo. El script está en un directorio temporal de la sesión, no en el
repo, y restaura desde copias. Una trampa que conviene saber: la primera corrida "verde"
salió roja porque `shutil.copy2` preserva el mtime, y cargo no recompiló el
`include_str!` restaurado. Se repitió después de un `touch`.

## Pre-CI

Sobre el código rebaseado (`331d31d4` + estos dos commits), en la máquina local, con
`CARGO_BUILD_JOBS=1` y `--test-threads=1`. Hubo que partirlo en pasos porque el sistema
mató tres corridas largas por falta de memoria (ver "Para el mantenedor"). CI no corre clippy.

| Paso | Comando | Resultado |
|---|---|---|
| fmt | `cargo fmt --all -- --check` | OK |
| Portada canónica | `python3 scripts/verify_landing_canonical.py --offline` | OK |
| Build | `cargo build --locked --features solana,near,stellar,algorand,sui,xrpl` | OK |
| Unit tests x402-rs | `cargo test --locked -p x402-rs --features … --bin x402-rs -- --test-threads=1` | **1136 passed**, 0 failed, 1 ignored (los 13 tests nuevos en verde) |
| Integración x402-rs | `--test` bazaar_freshness 6, bazaar_pricing 23, dx402_anchor_sig_cross 3, dx402_cross_seal 6, dx402_escrow_sim 1, dx402_vector_gen 1, escrow_integration 9, wire_conformance 15 | **64 passed**, 0 failed |
| Doctests x402-rs | `cargo test --locked -p x402-rs --features … --doc` | 1 passed, 11 ignored |
| Crates | `cargo test --locked -p x402-axum` / `-p x402-reqwest` / `-p x402-compliance` | 40 / 55 / 10 passed, 0 failed |
| Clippy compliance | `cargo clippy --locked -p x402-compliance` | 0 warnings, 0 errores |
| Clippy x402-rs | `cargo clippy --locked -p x402-rs --features … --all-targets` | 0 errores; 351 warnings, todos preexistentes: **0** caen en las 796 líneas que agrega este PR (`src/handlers.rs` 775, `src/main.rs` 21), según el mapeo de cada `-->` de clippy contra `git diff -U0 origin/main...HEAD` |

## Cómo verificarlo en producción (después del deploy)

```bash
B=https://facilitator.ultravioletadao.xyz
curl -s $B/version                                                    # {"version":"2.29.0"}
curl -sI $B/llms-full.txt | grep -i content-language                  # 78: content-language: en
curl -s -o /dev/null -D - -H 'Accept-Encoding: gzip' $B/ | grep -i 'content-encoding\|vary'   # 79: gzip + vary
curl -s -o /dev/null -w '%{size_download}\n' -H 'Accept-Encoding: gzip' $B/   # 79: ~37-38 KB, no 245 KB
curl -s $B/ | grep -c 'name="description"'                            # 80: 1
curl -s $B/x402 | grep -o '<link rel="canonical"[^>]*>'               # 80
curl -s $B/llms.txt | grep -i 'language (idioma)'                     # 81
curl -s $B/skill.md -H 'Accept: text/markdown' | grep -c '^## 1[23]\.' # 85: 2
# 84: la ráfaga gasta la cubeta de TU IP para las páginas humanas ~30 s, nada más
for i in $(seq 1 70); do curl -s -o /dev/null -w '%{http_code}\n' $B/stats; done | sort | uniq -c   # ~60x200 + 429s
curl -s -o /dev/null -w '%{http_code}\n' $B/supported                 # 200: fuera de esa cubeta
```

## Cómo se despliega

Leído de `.github/workflows/ci.yaml`: el merge a `main` **es** el release.

1. `ci.yaml:36-68` es el filtro `paths`. Este PR toca `src/**`, `static/**`,
   `Cargo.toml`, `Cargo.lock` y `VERSION`, así que dispara el pipeline completo.
2. El job `test` (`ci.yaml:129-157`) corre el verificador canónico, `cargo build --locked`
   y los tests con `--test-threads=1`.
3. `deploy` (`ci.yaml:453-455`, `needs: [test, preflight]`) construye la imagen con tag
   `$(cat VERSION)-<sha>` y aplica terraform con `-target` (`ci.yaml:541`).
4. Después espera el rollout y verifica `/health` y `/version` (`ci.yaml:653-655`).

No hay cambio de terraform ni de secretos.

## Coordinación

- **#48 (PYUSD, 2.27.0) y #49 (x4-escrow-enforce, 2.28.0):** los dos entraron antes del
  push. Esta rama está rebaseada sobre `331d31d4`.
- **Conflictos:** solo `VERSION`, que queda en 2.29.0, y `docs/CHANGELOG.md`, donde la
  entrada 2.28.0 de #49 va debajo de 2.29.0.
- **`src/handlers.rs`:** lo tocaban #49 y este PR, y se mezcló sin conflicto. Los tests
  corrieron sobre el resultado.

## Para el mantenedor

**Lo que NO hice, y por qué:**

- **Comprimir las respuestas dinámicas:** respuesta P2-A. Queda como fila P2 abajo, con
  la medición.
- **Publicar los límites de tasa para humanos:** respuesta P1-C. Queda como fila P2.
- **Taggear releases:** respuesta P3-B. Queda como fila P2 de CI, otro PR.
- **`curl -I` contra un binario local:** el facilitador no arranca sin llaves ni AWS
  (`ProviderCache::from_env`, writer lease). Las filas 78, 79 y 84 se probaron en
  proceso, contra los routers y middleware que monta `main.rs`, y el comando de
  producción está arriba.
- **Tareas de otros PRs:** la entrada 2.27.0 (#48) del CHANGELOG le corresponde a ese PR.

**La memoria de la máquina local:** el pre-CI murió dos veces por falta de memoria. Al revisar,
había un proceso `python -` (PID 21220, 8,9 GB de RSS, huérfano de `launchd`, 3 días
vivo) cuyo `cwd` es el worktree `x4-precios-p0` de otro
worker ya mergeado (#35). No lo maté porque no es mío. Probablemente sea la causa de
los OOM de todos los workers que compilan x402-rs en esa máquina.

**Falsedades del SPEC, para el próximo triage:**

- **Fila 76:** ya estaba cerrada.
- **Fila 78:** "4 páginas": eran 10, y ya lo tenían.
- **Fila 80:** "4 hits `og:`/`canonical` en index.html": eran 0. Las otras 9 ya tenían
  descripción.
- **Fila 82:** "no existe CHANGELOG": existía en `docs/`.
- **Fila 83:** "precios P0-P4 = lo que cobra el facilitador": no cobra nada.
- **Fila 84:** la fila de origen pedía publicar los límites, no ponerlos.
- **Fila 86:** "24 nodos": eran 3.

## Filas de backlog nuevas

| Fecha | Fila | Detalle | Prioridad |
|---|---|---|---|
| 2026-09-13 | Publicar los límites de tasa en la superficie humana (`/x402` o `/integrar`) | Intención original de la fila 84: hoy solo los lee una máquina en `.well-known/oauth-protected-resource` | P2 |
| 2026-09-13 | Comprimir lo dinámico detrás de una perilla: `CompressionLayer` fastest solo en `/discovery/*` | Medido 2026-09-13: una página del catálogo baja de 194 KB a 43 KB, con p95 local de 0,51 a 1,90 ms (saturado) y de 0,63 a 1,46 ms (200 req/s). Default apagado | P2 |
| 2026-09-13 | El deploy taggea `v$(cat VERSION)` al terminar | Sin tags, `git describe` devuelve una release de hace meses y ningún cierre puede usarlo | P2 (CI, otro PR) |
| 2026-09-13 | `GET /mcp` (la guía HTML) gasta la cubeta de `/verify`/`/settle` | Medido en prod: `x-ratelimit-limit: 30`. Leer la guía consume presupuesto de pago de esa IP | P2 |
| 2026-09-13 | Extender el test N3 a las otras 9 páginas | Hoy: `networks.html` 3 listas de tokens, `mcp.html` 1 (candidatas a lista blanca); las otras 7 en 0 | P3 |
| 2026-09-13 | Entradas faltantes del CHANGELOG | 2.27.0, 2.2, 2.4, 2.5, 2.8, 1.65–1.73 | P3 |
| 2026-09-13 | El comentario de `/discovery/register` dice 5 req/min | `main.rs` pone 1 token cada 12 s con ráfaga 250 | P3 |
| 2026-09-13 | El 409 de `POST /discovery/register` sugiere `PUT /discovery/resources/{url}` | Esa ruta no existe | P3 |
| 2026-09-13 | `docs/BAZAAR_DISCOVERY.md` desactualizado | Dice 4 parámetros, storage solo en memoria y sin rate limit; el código tiene 11, S3 y governor | P3 |
| 2026-09-13 | `scripts/build_llms_full.sh` no corre en un checkout CRLF de macOS | `set -o pipefail\r`: bash 3.2 falla; CI (Linux, LF) no lo ve | P3 |

---

Listo para revisión: commits de código `3b334a31` (feat, 2.29.0) y `9399bb88` (archivo del plan
de monetización), sobre `331d31d4`. El head del único push es el commit de este handoff,
inmediatamente encima de `9399bb88`; su SHA exacto está en la descripción del PR.
