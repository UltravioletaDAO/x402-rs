# Facilitador x402: la segunda ola agentica esta escrita, servida y probada

**Fecha:** 2026-09-02
**Rama:** `0xultravioleta/x4-ola2` — 8 commits sobre `a2508ca8` (= `main` con los PR #7 a #10)
**Estado:** codigo listo, **sin pushear**. El deploy queda descrito abajo, no ejecutado.
**Encargo:** las brechas de protocolo de `is-agentic` (72/100) y `ora.ai` (61/100),
medidas el 2026-09-02 08:57 EDT. Reporte de origen:
`c0der/docs/reports/2026-09-02-facilitador-escaneos-y-drift.md`, secciones 2 y 3.
**Alcance:** solo protocolo y servidor. **No se toco el diseno ni el contenido de la
landing** (`index.html`, `bazaar.html`, `stats.html`, `events-viewer.html`).

---

## 1. El resultado, en una linea

Los nueve items A-I estan resueltos o declarados fuera con motivo **medido**: siete
cambian el servicio, uno es solo documentacion, y uno (`$schema` del indice de
skills) queda fuera porque el dominio del schema **no resuelve** — `NXDOMAIN`,
re-medido al cerrar.

Suite completa verde con las features de CI: **lib 717, bin 747, EXIT=0**.
`clippy` no agrega ni un warning en los archivos tocados (330 antes, 330 despues).
`cargo fmt -p x402-rs --check` queda limpio.

---

## 2. Tabla item -> archivo -> test -> curl

| Item | Que hace | Archivos | Test |
|---|---|---|---|
| **A** Negociacion de markdown | `/` y `/llms.txt` sirven markdown cuando el `Accept` lo prefiere; las cinco rutas llevan `Vary: Accept, Accept-Encoding` | `src/negotiate.rs` (nuevo), `src/handlers.rs`, `src/lib.rs`, `src/main.rs` | `negotiate::tests` (11), `markdown_negotiation_tests` (4) |
| **B** 404 amigable | cuerpo markdown con los cinco documentos de recuperacion; JSON si el `Accept` lo pide | `src/handlers.rs`, `src/main.rs` | `agent_404_tests` (4) |
| **C** Errores JSON | 405, 404 y las negativas del rate limiter pasan a `{error, code, hint}`; los rechazos de cuerpo ganan `code` y `hint` | `src/handlers.rs`, `src/main.rs` | `json_error_tests` (4) |
| **D** Rate-limit headers | `use_headers()` en los 6 configs; documentado en `skill.md` y en el OpenAPI | `src/main.rs`, `static/skill.md`, `src/openapi.rs` | medido contra el binario (no hay test unitario: el header lo pone la libreria) |
| **E** Discovery ARD | `/.well-known/ard.json`, ARD v0.91 | `static/.well-known/ard.json` (nuevo), `src/handlers.rs`, `src/openapi.rs` | `the_ard_catalog_meets_the_spec` |
| **F** Sitemap `lastmod` | fecha del ultimo commit del archivo que respalda cada url | `static/sitemap.xml`, `src/handlers.rs` | `the_sitemap_stamps_every_url` |
| **G** `$schema` en agent-skills | **NO se agrega** — ver seccion 4 | — | — |
| **H** "Cuando usarme" en llms.txt | seccion con los seis trabajos y las cinco cosas que **no** es | `static/llms.txt`, `static/llms-full.txt` | `llms_txt_says_when_to_use_this_service_and_when_not_to` |
| **I** Los tres LOW del MCP | LOW-3 via `error_handler` del governor; LOW-4 en la descripcion; LOW-5 en el handoff | `src/mcp.rs`, `src/main.rs`, `docs/handoffs/2026-09-02-mcp-listo.md` | `the_idempotency_key_description_demands_a_unique_unguessable_value` |

### Los 8 commits

```
48092af0 feat(agentic): negotiate markdown on / and the .md surfaces, with Vary: Accept
8b173729 feat(agentic): give the 404 a body an agent can recover from
9ab5c839 feat(agentic): type every refusal as JSON, and report the rate-limit budget
55b1ac19 docs(agentic): publish the error shape and the rate-limit conventions
ec5da4a4 feat(agentic): publish the ARD v0.91 catalog at /.well-known/ard.json
b44a2c9d docs(agentic): tell agents when to reach for this facilitator, and when not to
6fc936e1 feat(agentic): stamp every sitemap url with the commit date of its source
002843bf fix(mcp): close the three LOW findings from the audit re-verification
```

1690 insertions / 37 deletions en 13 archivos. Ni un `.html` de la landing entre ellos.

---

## 3. La verificacion, con los curl

Binario local, receta de `docs/handoffs/2026-09-02-superficies-agenticas-listo.md`
seccion 2 (clave efimera, `HOST=127.0.0.1 PORT=8402`, `X-Forwarded-For` en cada curl).
Salidas recortadas a lo que importa.

### A — negociacion de markdown

```
$ curl -sI -H 'X-Forwarded-For: 1.2.3.4' http://127.0.0.1:8402/
HTTP/1.1 200 OK
content-type: text/html; charset=utf-8
vary: Accept, Accept-Encoding

$ curl -sI -H 'X-Forwarded-For: 1.2.3.4' -H 'Accept: text/markdown' http://127.0.0.1:8402/
HTTP/1.1 200 OK
content-type: text/markdown; charset=utf-8
vary: Accept, Accept-Encoding

$ curl -s  -H 'X-Forwarded-For: 1.2.3.4' -H 'Accept: text/markdown' http://127.0.0.1:8402/ | head -1
# x402 Payment Facilitator — Ultravioleta DAO      <- es /index.md, byte a byte

$ curl -sI ... /llms.txt                        -> text/plain; charset=utf-8   + vary: Accept
$ curl -sI ... /llms.txt -H 'Accept: text/markdown' -> text/markdown; charset=utf-8

$ curl -si ... /skill.md -H 'Accept: application/pdf'
HTTP/1.1 406 Not Acceptable
content-type: application/json; charset=utf-8
{"error":"No representation matches the Accept header","code":"not_acceptable",
 "available":["text/markdown"],"hint":"This resource is available as: text/markdown. ..."}
```

El header de un Chrome real (`text/html,...,*/*;q=0.8`) sigue dando `text/html` en `/`
y `text/markdown` en `/skill.md` — el `*/*` del final cubre markdown. **Solo un cliente
que nombre `text/html` y nada mas llega al 406**, y eso no lo manda ningun navegador;
hay test con las cadenas de Chrome, Firefox y Safari.

### B — 404 amigable

```
$ curl -si -H 'X-Forwarded-For: 1.2.3.4' http://127.0.0.1:8402/no-existe
HTTP/1.1 404 Not Found
content-type: text/markdown; charset=utf-8
vary: Accept, Accept-Encoding
x-ratelimit-limit: 100
x-ratelimit-remaining: 99

# 404 Not Found

No route on the x402 payment facilitator serves this path.

## Where the real routes are described

- <https://facilitator.ultravioletadao.xyz/llms.txt> - ...
- <https://facilitator.ultravioletadao.xyz/sitemap.xml> - ...
- <https://facilitator.ultravioletadao.xyz/openapi.json> - ...
- <https://facilitator.ultravioletadao.xyz/.well-known/api-catalog> - ...

$ curl -si ... /no-existe -H 'Accept: application/json'
HTTP/1.1 404 Not Found
content-type: application/json; charset=utf-8
{"error":"No route serves this path","code":"not_found","hint":"Read .../llms.txt ...",
 "documentation":{"llms":...,"sitemap":...,"openapi":...,"apiCatalog":...,"skill":...}}
```

**Antes y despues, 14 rutas inexistentes seguidas desde una IP:**

```
antes:  404 404 404 404 404 404 404 404 404 404 429 429 429 429
ahora:  404 404 404 404 404 404 404 404 404 404 404 404 404 404
```

Ver seccion 5: eso es una desviacion consciente del encargo, con motivo medido.

### C — errores JSON

```
$ curl -si -X DELETE ... /verify
HTTP/1.1 405 Method Not Allowed
content-type: application/json; charset=utf-8
allow: GET,HEAD,POST                       <- axum lo sigue calculando solo
{"error":"DELETE is not supported on /verify","code":"method_not_allowed",
 "hint":"The `Allow` response header lists the methods this path accepts. ..."}

# antes: HTTP/1.1 405, content-length: 0, sin content-type

$ curl -si -X POST -H 'content-type: application/json' -d '{not json' ... /verify
HTTP/1.1 400 Bad Request
{"error":"Failed to deserialize VerifyRequest: key must be a string at line 1 column 2",
 "code":"invalid_request_body","hint":"The body must be a JSON object with ..."}

# 429 del limitador (antes: "Too Many Requests! Wait for 1s", sin content-type)
HTTP/1.1 429 Too Many Requests
retry-after: 1
content-type: application/json; charset=utf-8
{"error":"Too Many Requests! Wait for 1s","code":"rate_limited","hint":"Wait for the
 number of seconds in the `retry-after` header, then retry. ..."}
```

### D — rate-limit headers

```
$ curl -sI -H 'X-Forwarded-For: 1.2.3.4' ... /verify     # ruta CON governor
HTTP/1.1 200 OK
x-ratelimit-limit: 30
x-ratelimit-remaining: 29

$ curl -sI ... /supported                                 # ruta SIN governor
HTTP/1.1 200 OK
(sin x-ratelimit)

# 429, con el presupuesto completo
x-ratelimit-after: 1 · retry-after: 1 · x-ratelimit-limit: 30 · x-ratelimit-remaining: 0
```

**El test del encargo decia `/supported` y `/supported` no tiene governor.** Medido:
`handlers::routes()` se mergea en `main.rs` sin `GovernorLayer`, asi que ninguna de sus
rutas puede traer headers de rate limit. El encargo tambien decia "activala en las capas
que ya existen", y ponerle un governor nuevo a `/supported` seria un limite nuevo en una
ruta hoy libre — mas de lo pedido. La prueba equivalente se corrio contra `GET /verify`,
que si tiene governor. **Si se quiere el header en `/supported`, hace falta decidir
primero que se le pone un rate limit**; no es un olvido.

### E — `/.well-known/ard.json`

```
$ curl -sI ... /.well-known/ard.json     -> 200, application/json; charset=utf-8
$ curl -s  ... /.well-known/ard.json | jq -r '.specVersion, (.entries[].identifier)'
0.91
urn:air:facilitator.ultravioletadao.xyz:mcp:x402-facilitator
urn:air:facilitator.ultravioletadao.xyz:agent:x402-facilitator
urn:air:facilitator.ultravioletadao.xyz:skill:x402-facilitator-settlement
urn:air:facilitator.ultravioletadao.xyz:api:x402-facilitator
urn:air:facilitator.ultravioletadao.xyz:doc:llms-txt
```

**De donde salio el formato** (nada inventado): el catalogo de checks de ora
(`GET https://ora.ai/api/checks`) nombra el spec en el check `ard-catalog` ->
`https://agenticresourcediscovery.org/` — ARD **v0.91**, publicado el 2026-08-26.
Se leyo el spec entero y se valido el documento contra el **schema autoritativo**
(`ards-project/ard-spec`, `spec/schemas/ard-entry.schema.json`, bajado hoy):

```
ArdManifest errors: 0
ArdEntry errors:    0   (los 5)
D.2: urn ok, publisher = facilitator.ultravioletadao.xyz, 2-4 representativeQueries, url XOR data
```

Decisiones que conviene no re-litigar:

- **Solo `ard.json`, no `ai-catalog.json`.** Seccion 5.1 fija `ard.json` y hace su
  fetch normativo; el path viejo es *opcional* de consultar y el spec le dice al
  publicador que se mude. Servir los dos es un documento mas para mantener
  sincronizado, para consumidores que no estan obligados a leerlo.
- **Sin `trustManifest`, a proposito.** Seccion 4.5.1 ata `trustManifest.identity` al
  dominio del URN y espera que un registry verifique una atestacion emitida por ese
  dominio. Este host no publica documento DID ni atestacion: un `did:web:` aca seria
  una afirmacion que nadie puede chequear. El check de ora es un bonus que nunca resta
  puntos, asi que una afirmacion no verificable costaria mas de lo que el bonus vale.

### F — `<lastmod>` en el sitemap

```
$ curl -s ... /sitemap.xml | grep -c '<lastmod>'   # 10 urls, 10 con fecha
$ curl -s ... /sitemap.xml | head
  https://facilitator.ultravioletadao.xyz/            2026-08-21T00:30:38-04:00
  https://facilitator.ultravioletadao.xyz/docs        2026-09-02T15:46:32-04:00
  https://facilitator.ultravioletadao.xyz/bazaar      2026-07-24T18:45:49-04:00
  https://facilitator.ultravioletadao.xyz/stats       2026-08-03T16:26:02-04:00
```

Cada fecha es `git log -1 --format=%cI -- <archivo que respalda la url>`, no la fecha
en que se edito el sitemap. La dispersion es el punto: un sitemap que re-estampa las
diez urls cada vez que una cambia le dice al crawler que relea nueve documentos que no
se movieron. El mapeo url -> archivo no es derivable (`/docs` sale de `src/openapi.rs`,
`/` de `static/index.html`), asi que vive en el doc comment del test.

El test **no** chequea frescura a proposito: exigir que la fecha mas nueva sea reciente
pone el build en rojo cada semana tranquila. Chequea que ninguna url quede sin fecha,
que es lo que de verdad se rompe. **Verificado discriminante:** borrando un `<lastmod>`
se pone rojo y nombra la url.

### H — "cuando usarme" en `llms.txt`

Seccion nueva arriba de todo (linea 8), con los seis trabajos como llamadas
(`/verify`, `/settle`, `/supported`, `/accepts`, `/mcp`, ERC-8004) y — la mitad que
hace mas trabajo — las cinco cosas que **no** es: no es una wallet, no es un
marketplace, no cobra ni contesta 402, no arbitra disputas, y `/events` no es un ledger.
`llms-full.txt` y el sha256 del indice de skills regenerados.

### I — los tres LOW del MCP

```
$ # 31 llamadas a /mcp desde una IP, luego una mas
HTTP/1.1 429 Too Many Requests
content-type: application/json; charset=utf-8            <- LOW-3
{"error":"Too Many Requests! Wait for 1s","code":"rate_limited","hint":"..."}

$ # el 406 propio de rmcp sigue tipado, sin cambios
HTTP/1.1 406 Not Acceptable
content-type: application/json
{"error":"Not Acceptable","status":406,"detail":"...","hint":"Accept must name BOTH ..."}
```

**LOW-3 se cerro por un camino distinto al que propuso la auditoria, y conviene saber
por que.** La recomendacion era mover `json_content_type_on_errors` afuera del
`GovernorLayer` en `main.rs`. En vez de eso, cada governor lleva ahora el
`error_handler` propio de `tower_governor`, que corre **adentro** del limitador. Mismo
resultado para `/mcp`, y ademas alcanza a los otros once governors: un 429 en
`/discovery/resources` estaba tan sin tipar como uno en `/mcp`. La capa
`json_content_type_on_errors` se queda donde estaba: atiende el 406/415/403 de rmcp,
que el limitador nunca ve.

**LOW-4:** la descripcion de `idempotencyKey` pide ahora un valor fresco e inadivinable
por pago y nombra UUIDv4. El store es un namespace unico compartido por todos los
llamadores; el llamador de esta puerta es un modelo, y un modelo escribe `"retry-1"`.

**LOW-5:** el bloque de evidencia de `docs/handoffs/2026-09-02-mcp-listo.md` decia
"15 de esos son mcp::tests" cuando la rama cerro con 21. Queda marcado como historico
y debajo va la corrida real de hoy.

---

## 4. Lo que quedo fuera, y por que

### Fuera por decision del dueno: el rediseno de la landing

Estos checks son de `index.html` y de paginas nuevas. El dueno esta decidiendo aparte
si redisena la landing (c0der escribe ese plan), asi que **no se toco nada de eso**:

| Check | Escaner | Que pide |
|---|---|---|
| `content-no-js` | is-agentic (essential, partial) | H1 claro, headings secuenciales, >=500 chars de contenido en HTML crudo |
| `json-ld` / `org-schema-completeness` / `JSON-LD entity linking` / `Schema type breadth` | ambos | JSON-LD de Organization con `contactPoint` y `address`, `sameAs` |
| `trust-anchors` / `Trust anchor pages` | ambos | paginas `/about`, `/contact`, `/privacy` con >=500 chars cada una |
| `metadata-completeness` | ambos (partial) | `<link rel=canonical>`, `<html lang>`, `og:image`, `og:type` |
| `Agent mode view` | ora | `?mode=agent` |

### Fuera por medicion: `$schema` en el indice de agent-skills (item G)

El check `agent-skills-index-v2` pide exactamente
`"$schema": "https://schemas.agentskills.io/discovery/0.2.0/schema.json"`.
**Ese dominio no resuelve.** Medido dos veces hoy, al empezar y al cerrar:

```
$ getent hosts schemas.agentskills.io
(vacio)
$ curl -sS -m 12 -o /dev/null -w '%{http_code}\n' https://schemas.agentskills.io/discovery/0.2.0/schema.json
curl: (6) Could not resolve host: schemas.agentskills.io
000
```

Agregar un `$schema` que apunta a un 404 de DNS es peor que no tenerlo: un validador que
intente resolverlo falla, y el documento pasa a declarar conformidad con algo que no
existe. **El resto del check ya se cumple** — cada entrada tiene `type`, `url` y
`digest` en el formato `sha256:<64 hex>`. Cuando el dominio resuelva, es una linea.

### Fuera por alcance: checks de ora que nadie pidio en este encargo

Se listan porque estan a mano y son baratos, no porque falten aca:

- **`markdown-link-alternate`**: anunciar el gemelo markdown con
  `Link: <...>; rel="alternate"; type="text/markdown"`. La negociacion ya funciona; falta
  el anuncio. Es una linea en `negotiated_surface`.
- **`markdown-frontmatter`**: abrir los `.md` servidos con un bloque `---` (title +
  description/canonical/last-updated).
- **`modular-llms-txt`**: `llms.txt` por seccion.
- **`idempotency-key-support`**: declarar el header `Idempotency-Key` como parametro de
  `POST /settle` en el OpenAPI. El soporte existe; falta declararlo.
- **`api-versioning-policy` / `REST versioning`**: politica de versionado y `Sunset`/
  `Deprecation`.
- **`Web Bot Auth directory`**, **`NLWeb /ask`**, **`WebMCP`**: superficies nuevas.

**Lo que no es del facilitador** (lo dice el reporte de origen): Wikipedia/Wikidata, app
de ChatGPT, skills.sh, paquete SDK propio — el SDK es `uvd-x402-sdk`, ya publicado, y el
escaner lo busca por el nombre del dominio.

---

## 5. Las dos desviaciones del encargo, declaradas

### 5.1 El 404 cambio de bucket de rate limit

El encargo decia: *"el fallback de 404 tiene que quedar DESPUES del governor igual que
hoy"*. **Sigue detras de un governor** — un 404 sin limite es una superficie de
amplificacion gratis y el escaneo de paths es justo el trafico que la encuentra. Lo que
cambio es **cual** presupuesto usa.

Medido antes de tocar nada: el 404 lo gobernaba el bucket de **`/events`** (burst 10,
un token cada 2 s), y a la request 11 desde una IP contestaba 429. Y no era una decision:
`Router::merge` de axum se queda con el fallback del router mergeado **ultimo**
(`(true, true) => use the one from other`) y `.layer()` envuelve el fallback por defecto
junto con las rutas — asi que el 404 heredaba el governor del ultimo router gobernado de
la cadena, sea cual fuere.

Dejarlo ahi hubiera saboteado el propio item B: un escaner que prueba once paths
inexistentes recibe 429 en vez de 404 y puntua peor el check que estamos arreglando. Asi
que el fallback pasa a ser **explicito** y toma el bucket de lecturas secundarias
(1 token cada 300 ms, burst 100), que es el que ya esta dimensionado para lecturas
baratas — un 404 es una constante, y el bucket de `/events` existe para acotar la
apertura de conexiones SSE largas.

`the_fallback_survives_being_merged_with_other_routers` hace que un reordenamiento
futuro no se lo lleve en silencio.

### 5.2 El test de D apunta a `GET /verify`, no a `/supported`

Ver seccion 3 (D). `/supported` no tiene governor hoy; ponerle uno seria un limite nuevo
en una ruta libre, que es mas de lo pedido.

---

## 6. Archivos

| Archivo | Que le paso |
|---|---|
| `src/negotiate.rs` | **nuevo** — parser de `Accept` (RFC 9110 12.5.1) y 11 tests |
| `src/handlers.rs` | `negotiated_surface`/`negotiated_response`, `agent_not_found`, `method_not_allowed`, `rate_limit_error`, `get_ard`; 5 rutas negociadas; 13 tests nuevos |
| `src/main.rs` | `use_headers()` en los 6 configs, `error_handler` en los 12 layers, fallback 404 explicito y `method_not_allowed_fallback`, ambos despues del ultimo `.merge()` |
| `src/mcp.rs` | descripcion de `idempotencyKey` (LOW-4) + su test |
| `src/openapi.rs` | secciones Errors / Rate limits / Content negotiation; `path_ard_json` |
| `src/lib.rs` | `pub mod negotiate;` |
| `static/.well-known/ard.json` | **nuevo** — catalogo ARD v0.91, 5 entradas |
| `static/llms.txt` | seccion "When to use this, and when not to"; link al ARD |
| `static/skill.md` | seccion de rate limits y forma de los errores |
| `static/sitemap.xml` | `<lastmod>` en las 10 urls |
| `static/llms-full.txt` | regenerado |
| `static/.well-known/agent-skills/index.json` | sha256 de `skill.md` actualizado |
| `docs/handoffs/2026-09-02-mcp-listo.md` | evidencia de la suite corregida (LOW-5) |

**Sin tocar:** `static/index.html`, `static/bazaar.html`, `static/stats.html`,
`static/events-viewer.html`, terraform, `Cargo.toml`, `VERSION`.

---

## 7. La suite y el linter

```
rustup run stable cargo test --locked -p x402-rs \
  --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1
  lib  717 passed, 0 failed
  bin  747 passed, 0 failed      (22 son mcp::tests)
  + dx402_anchor_sig_cross, dx402_cross_seal, dx402_vector_gen,
    escrow_integration y los doctests: todos ok
  EXIT=0

rustup run stable cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance \
  -- --test-threads=1
  EXIT=0

rustup run stable cargo clippy --features ... --all-targets
  EXIT=0 · 330 warnings, las mismas 330 que ya habia en a2508ca8.
  Cero en src/negotiate.rs y cero en los rangos nuevos de handlers.rs y main.rs.
  (El unico warning nuevo que aparecio fue `TEXT_MARKDOWN_UTF8 is never used`,
   huerfano de este cambio: se borro la constante.)

rustup run stable cargo fmt -p x402-rs -- --check
  EXIT=0
```

Los escaneos de terceros (`npx is-agentic`, `npx @ora-ai/ax@0.5 audit`) **no** se
corrieron: los corre c0der despues del deploy, por la cuota de ora.ai.

---

## 8. El deploy

**No se pusheo nada.** El deploy es el merge a `main`: `.github/workflows/ci.yaml`
testea, construye la imagen, la sube a ECR y hace un `terraform apply -auto-approve`
dirigido a la task definition y al servicio, espera el rollout y chequea `/health`.
El drift gate sigue rojo por `aws_iam_policy.cicd_infra` y es **advisory** — no bloquea
(seccion 5 del reporte de origen).

Un merge es un release. Despues del deploy conviene:

```bash
curl -s https://facilitator.ultravioletadao.xyz/version
curl -sI -H 'Accept: text/markdown' https://facilitator.ultravioletadao.xyz/ | grep -i vary
curl -s  https://facilitator.ultravioletadao.xyz/.well-known/ard.json | jq .specVersion
curl -so /dev/null -w '%{http_code}\n' https://facilitator.ultravioletadao.xyz/no-existe
```

`VERSION` **no se toco**: la version se bumpea desde la desplegada, no desde la local
(`curl -s https://facilitator.ultravioletadao.xyz/version`), y esa decision es del dueno
al momento de mergear.

---

## 9. Nota de entorno: el CLI de Orca no corre en esta terminal

El mensaje al buzon del run **no se pudo mandar desde aca**. La interoperabilidad de
WSL con binarios de Windows esta apagada en esta terminal:

```
$ /mnt/c/Users/lxhxr/AppData/Local/Programs/orca/resources/bin/orca ...
cannot execute binary file: Exec format error
$ ls /proc/sys/fs/binfmt_misc/WSLInterop
No such file or directory
```

`resources/bin/` solo tiene `orca.exe` y `orca.cmd`; `orca.cmd` delega en el `.exe` y no
hay entrypoint de node, asi que no hay camino nativo. Mismo motivo por el que `git.exe`
no corria — eso se resolvio reescribiendo el gitfile del worktree a
`/mnt/z/ultravioleta/dao/x402-rs/.git/worktrees/x4-ola2`, con OK del dueno, y el `git`
de WSL trabaja normal desde entonces (con `-c core.autocrlf=true`, porque el checkout es
de Windows y sin eso 388 archivos aparecen modificados por CRLF).

El resumen va igual en el `worker_done`.
