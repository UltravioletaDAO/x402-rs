# Facilitador x402: superficies agenticas listas, deploy preparado (no ejecutado)

**Fecha:** 2026-09-02
**Rama:** `0xultravioleta/x4-agentic`, worktree `/mnt/c/Users/lxhxr/orca/workspaces/x402-rs/x4-agentic`
**Base:** `aad5c4c6` · **HEAD:** `a6ec307f` · 4 commits, 19 archivos, +3377 lineas
**Release local y desplegado:** `2.9.0` (VERSION y `curl .../version` coinciden)
**Estado:** codigo completo, tests verdes, **cero push, cero deploy**

Continuacion de [`2026-09-02-superficies-agenticas-medicion.md`](2026-09-02-superficies-agenticas-medicion.md),
que dejo la medicion inicial: **0 de 35 (0.0%)**.

---

## 1. El resultado: 26 / 35 medido contra el binario local

Corrido con el checker real de c0der (`scripts/agentic_check.py`, cargado como
modulo y apuntado a `http://127.0.0.1:8402`), no con curls a mano ni con una
version relajada del criterio. Mismo `agentic-sites.toml`, mismo
`ok = codigo + content-type + cuerpo distinto de la raiz`.

```
LOCAL 26/35  74.3%
```

| sitio | HTTP | content-type | ok |
|---|---|---|---|
| `llms-txt` | 200 | `text/plain` | **si** |
| `llms-full-txt` | 200 | `text/plain` | **si** |
| `robots-txt` | 200 | `text/plain` | **si** |
| `sitemap-xml` | 200 | `application/xml` | **si** |
| `agent-card` | 200 | `application/json` | **si** |
| `agent-json-legacy` | 200 | `application/json` | **si** |
| `x402-discovery` | 200 | `application/json` | **si** |
| `mcp-server-card` | 500* | - | **no — no aplica, ver §4** |
| `agent-skills-index` | 200 | `application/json` | **si** |
| `api-catalog` | 200 | `application/linkset+json` | **si** |
| `oauth-protected-resource` | 200 | `application/json` | **si** |
| `openapi-json` | 200 | `application/json` | **si** |
| `skill-md` | 200 | `text/markdown` | **si** |
| `workflows-json` | 200 | `application/json` | **si** |
| `index-md` | 200 | `text/markdown` | **si** |
| `auth-md` | 200 | `text/markdown` | **si** |
| `is-agentic` | 400 | - | **no — ranking, se dispara post-deploy** |
| `ora-ai` | 404 | - | **no — ranking, se dispara post-deploy** |

**Los 15 checks de superficie que aplican estan en verde.** Los tres que faltan
son uno que no aplica y dos que no se pueden ganar antes del deploy.

\* El `500` de `mcp-server-card` en local es un artefacto de correr sin ALB, no un
bug nuevo: cualquier ruta inexistente contesta `500 "Unable To Extract Key!"`
porque `SmartIpKeyExtractor` de `tower_governor` no encuentra `X-Forwarded-For` en
una conexion directa. En produccion, detras del ALB, esa misma ruta contesta 404
(verificado hoy). No lo introduce esta rama.

### Como reproducir la medicion

```bash
# 1) binario local (WSL). La clave es efimera, generada al vuelo, sin fondos,
#    y no se escribe en ningun archivo.
cd /mnt/c/Users/lxhxr/orca/workspaces/x402-rs/x4-agentic
cp config/blacklist.json.example config/blacklist.json
export EVM_PRIVATE_KEY_TESTNET="0x$(python3 -c 'import secrets;print(secrets.token_hex(32))')"
HOST=127.0.0.1 PORT=8402 RUST_LOG=warn SIGNER_TYPE=private-key \
  RPC_URL_BASE_SEPOLIA=https://sepolia.base.org \
  ./target/debug/x402-rs &

# 2) el checker de c0der contra el local
cd /mnt/z/ultravioleta/dao/c0der
PYTHONUTF8=1 python3 - <<'PY'
import importlib.util
spec = importlib.util.spec_from_file_location("ac", "scripts/agentic_check.py")
ac = importlib.util.module_from_spec(spec); spec.loader.exec_module(ac)
reg = ac.cargar_registro()
nodo = {"id": "facilitator", "url": "http://127.0.0.1:8402", "status": "live"}
f = ac.medir([nodo], reg)["proyectos"][0]
print(f"{f['puntos']}/{f['posibles']}  {f['pct']}%")
for r in f["checks"]:
    print(("OK  " if r["ok"] else "--  ") + r["sitio"], r["http"], r["content_type"], r["motivo"] or "")
PY
```

---

## 2. Que se agrego

### Los archivos (`static/`)

| Archivo | Ruta servida | content-type |
|---|---|---|
| `static/llms.txt` | `/llms.txt` | `text/plain; charset=utf-8` |
| `static/llms-full.txt` | `/llms-full.txt` | `text/plain; charset=utf-8` |
| `static/robots.txt` | `/robots.txt` | `text/plain; charset=utf-8` |
| `static/sitemap.xml` | `/sitemap.xml` | `application/xml; charset=utf-8` |
| `static/index.md` | `/index.md` | `text/markdown; charset=utf-8` |
| `static/skill.md` | `/skill.md` | `text/markdown; charset=utf-8` |
| `static/auth.md` | `/auth.md` | `text/markdown; charset=utf-8` |
| `static/workflows.json` | `/workflows.json` | `application/json; charset=utf-8` |
| `static/.well-known/agent-card.json` | `/.well-known/agent-card.json` | `application/json; charset=utf-8` |
| `static/.well-known/agent.json` | `/.well-known/agent.json` | `application/json; charset=utf-8` |
| `static/.well-known/x402` | `/.well-known/x402` | `application/json; charset=utf-8` |
| `static/.well-known/api-catalog` | `/.well-known/api-catalog` | `application/linkset+json; charset=utf-8` |
| `static/.well-known/oauth-protected-resource` | `/.well-known/oauth-protected-resource` | `application/json; charset=utf-8` |
| `static/.well-known/agent-skills/index.json` | `/.well-known/agent-skills/index.json` | `application/json; charset=utf-8` |

Mas `/openapi.json`, que es un **alias** del documento que Swagger UI ya servia en
`/api-docs/openapi.json` (`src/openapi.rs`). Todo scanner agentico y todo catalogo
RFC 9727 lo busca en la raiz, y nuestro propio `api-catalog` declara
`service-desc -> /openapi.json`: sin el alias, el catalogo apuntaria a un 404.

### El codigo

- `handlers::agentic_routes()` (`src/handlers.rs:269`) — router **sin estado y sin
  rate limit**, con las 14 rutas declaradas una por una y el `content-type`
  explicito. Sin governor a proposito: son documentos estaticos, y un crawler que
  recibe 429 en `/llms.txt` reporta el servicio como inalcanzable.
- `src/main.rs:649` — el merge, junto a `openapi::swagger_routes()`.
- `src/openapi.rs` — la ruta `/openapi.json` (spec serializado UNA vez a `Bytes`,
  asi el clone por request es un refcount) y las 15 entradas `utoipa::path` que
  el `CLAUDE.md` del repo exige por cada endpoint nuevo.
- `scripts/build_llms_full.sh` — genera `llms-full.txt` desde sus cuatro fuentes.

### Los tests (11 nuevos; 698 unit tests en verde, 0 fallos)

`cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1`
(el gate de CI) pasa entero. `cargo clippy` no agrega ni un warning en los tres
archivos tocados.

En `handlers::agentic_surface_tests`:

1. las 14 rutas responden 200 **con su** content-type;
2. ninguna es HTML ni la landing page (el `distinto_de_raiz` del checker);
3. la tabla `SURFACES` cubre todas las `.route()` declaradas — agregar una ruta sin
   su fila pone el test en rojo;
4. los JSON traen los campos que un consumidor indexa (`name`/`description`/`url`,
   `skills`, `linkset`, `x402`, `resource`, `workflows`);
5. el discovery x402 sigue diciendo `role: "facilitator"` y `paidRoutes: []`;
6. las dos cards A2A siguen siendo byte-identicas;
7. `llms-full.txt` sigue sincronizado con sus cuatro fuentes;
8. el `sha256` del indice de skills sigue siendo el de `skill.md`;
9. todo link a `facilitator.ultravioletadao.xyz` dentro de estos archivos resuelve
   a una ruta que alguien sirve.

En `openapi::tests`: las 15 rutas estan en el spec, y `/openapi.json` devuelve el
mismo documento con `application/json`.

**7 y 8 se verificaron discriminantes**: con una linea de mas en `skill.md` los dos
se ponen rojos, y vuelven a verde al revertirla.

---

## 3. Lo que NO se invento

- Redes, esquemas y stablecoins salen de `/supported` vivo,
  `scripts/verify_landing_canonical.py` (21 mainnets) y
  `scripts/stablecoin_matrix.py` (6 stablecoins). Los CAIP-2 se copiaron de
  `Network::to_caip2` (`src/network.rs:576`).
- Los ejemplos de `/verify` y `/settle` salen de `src/openapi.rs`; las respuestas
  reales de `/verify` y `/accepts` de una llamada en vivo a produccion.
- **Ninguna direccion de contrato entra en estos archivos.** Un documento estatico
  con direcciones se desincroniza del codigo y nadie se entera; las redes llevan
  simbolos y el documento manda a leerlas de `POST /accepts`.
- `SEI` y `XDC` estan fuera: salen en `stablecoin_matrix.py` pero no en el
  `/supported` de produccion.

---

## 4. Los checks que NO aplican, y que hay que decidir en c0der

### `mcp-server-card` (peso 3) — NO se fabrico

El facilitador **no expone un servidor MCP**. Publicar una card que apunte a un
endpoint inexistente es exactamente el fallo que el propio `agentic-sites.toml`
documenta de meshrelay ("la card sola no alcanza: meshrelay publica la suya y el
endpoint que anuncia da 404 en GET y 401 en POST").

**Lo que hace falta en c0der** (una de dos, es decision del dueno):

- agregar `excluye = ["facilitator"]` al bloque `[[sitio]] id = "mcp-server-card"`
  con el motivo al lado; o
- construir un servidor MCP de verdad para el facilitador, que es otro encargo.

Mientras tanto el techo del facilitador es **32/35 (91.4%)**, no 35/35.

### `x402-discovery` (peso 3) — SI aplica, y esta en verde

El encargo preveia que un facilitador que no cobra quizas no pudiera cumplir este
check con verdad. **No fue el caso**: el check no exige campos de precio, solo
`200 + application/json + cuerpo distinto de la raiz`. El documento publicado es
honesto — declara `role: "facilitator"`, `capabilities.payments: false` y
`paidRoutes: []` — y pasa. No hace falta tocar su `aplica_a`.

### `oauth-protected-resource` (peso 1) — aplica, en verde, con una nota

El facilitador no autentica llamadores. El documento se publica igual, con
`authorization_servers: []` y un `x-note` que lo explica, para que un agente con
OAuth lo descubra en un GET en vez de intentar y fallar. Es la misma forma que ya
usa execution-market (`dashboard/public/.well-known/oauth-protected-resource`).

### `is-agentic` y `ora-ai` (peso 3 + 3) — post-deploy

Ver §6.

---

## 5. El deploy: PREPARADO, NO EJECUTADO

Nada se pusheo. `git status` limpio salvo `target/`, `contracts/` y
`node_modules/`, que ya estaban sin trackear.

```bash
# lo que falta, con OK explicito del dueno:
git push origin 0xultravioleta/x4-agentic   # o merge a main
```

**Pushear a `main` despliega a produccion**: `.github/workflows/ci.yaml` testea,
buildea la imagen, la sube a ECR y hace `terraform apply -auto-approve` targeteado
sobre la task definition y el servicio.

### Correccion al brief: el drift gate NO bloquea el deploy

El encargo decia que las tres ultimas corridas de CI fallan en "Terraform plan
(drift gate)" y **saltan el deploy**, y que "hoy ni un push despliega el
facilitador". **Medido hoy, eso es falso.** En la ultima corrida
([33587276281](https://github.com/UltravioletaDAO/x402-rs/actions/runs/33587276281),
2026-09-02 03:30 UTC):

```
✓ Check deploy prerequisites            3s
✓ Build & test                       5m13s
X Terraform plan (drift gate)          35s   <- falla
✓ Push to ECR & deploy to ECS        8m41s   <- CORRE IGUAL, y pasa
```

`ci.yaml:273` dice `deploy: needs: [test, preflight]` — **no** incluye `plan`, y el
comentario de `ci.yaml:92` lo dice textual: *"pre-existing drift must not be able
to block a release"*. El gate es informativo. La prueba independiente es que
produccion sirve `2.9.0`, la misma version que el `VERSION` de `main`.

Lo que el gate esta reportando son dos recursos que difieren de AWS y que ningun
paso del deploy targetea: `aws_s3_bucket_ownership_controls.alb_logs` y
`aws_s3_bucket_lifecycle_configuration.alb_logs`. Es una fila de backlog de
infraestructura, ajena a esta rama.

### Verificacion post-deploy

```bash
curl -s https://facilitator.ultravioletadao.xyz/version          # la version nueva
curl -s https://facilitator.ultravioletadao.xyz/llms.txt | head -3
curl -s -o /dev/null -w '%{http_code} %{content_type}\n' \
     https://facilitator.ultravioletadao.xyz/.well-known/api-catalog
cd /mnt/z/ultravioleta/dao/c0der && PYTHONUTF8=1 python3 scripts/agentic_check.py --project facilitator
```

Esperado: **26/35 (74.3%)**, y 32/35 despues de los dos escaneos de §6.

---

## 6. Los dos escaneos que se disparan DESPUES del deploy

**No los corrio nadie todavia. No los corras antes de que la version nueva este
en produccion**: los dos rankings cachean el resultado, asi que un escaneo
disparado hoy fotografia el sitio sin superficies y ese cero es el que queda.

```bash
npx is-agentic facilitator.ultravioletadao.xyz --json
npx @ora-ai/ax@0.5 audit https://facilitator.ultravioletadao.xyz
```

ora.ai limita a **30 escaneos por dia y por IP**: un solo disparo, no un bucle.

---

## 7. Notas operativas de esta rama

- **El worktree tiene punteros de Windows.** Su `.git` dice
  `gitdir: Z:/ultravioleta/dao/x402-rs/.git/worktrees/x4-agentic`, que git de WSL
  no resuelve. Desde WSL hay que trabajar asi (sin tocar ningun archivo del
  checkout del dueno):

  ```bash
  export GIT_DIR=/mnt/z/ultravioleta/dao/x402-rs/.git/worktrees/x4-agentic
  export GIT_WORK_TREE=/mnt/c/Users/lxhxr/orca/workspaces/x402-rs/x4-agentic
  git -c core.autocrlf=true <lo que sea>
  ```

  El `core.autocrlf=true` es obligatorio: los archivos estan en disco con CRLF
  (los escribio git de Windows) y sin el, `git status` muestra los 364 archivos
  del repo como modificados.

- **El CLI de Orca no corre desde esta WSL.** Ni `orca.exe` ni `orca.cmd`
  (`cannot execute binary file: Exec format error`, y `cmd.exe` falla igual): la
  interoperabilidad con binarios de Windows esta apagada en esta distro. El
  resultado va por `worker_done` y por este documento.

- Los tests normalizan CRLF antes de hashear. Un checkout de Windows guarda estos
  archivos con CRLF e `include_str!` lee lo que hay en disco, pero produccion
  compila en Linux desde un checkout con LF, y LF es la forma que se sirve y la
  que el digest publicado describe.

- **Al editar cualquiera de los cuatro documentos fuente hay que correr
  `./scripts/build_llms_full.sh` y commitear el resultado.** Si no, el test
  `llms_full_txt_is_in_sync_with_its_sources` pone el build en rojo — que es
  precisamente para lo que existe.
