# Facilitador x402: las superficies agenticas estan escritas, servidas y probadas

**Fecha:** 2026-09-02
**Rama:** `0xultravioleta/x4-agentic` — 5 commits sobre `aad5c4c6`
**Estado:** codigo listo, **sin pushear**. El deploy queda descrito abajo, no ejecutado.
**Medicion previa:** `docs/handoffs/2026-09-02-superficies-agenticas-medicion.md` (0/35)

---

## 1. El resultado, en una linea

Contra el binario corriendo en local, `agentic_check.py` da **26/35 (74.3%)**, con
**las 15 superficies aplicables en verde**. Los 9 puntos que faltan son tres checks
que no se pueden ganar desde esta rama: `mcp-server-card` (3) porque el facilitador
no tiene MCP, e `is-agentic` (3) y `ora-ai` (3) porque son escaneos de terceros que
se disparan **despues** del deploy.

---

## 2. La tabla local

Corrida con el propio `scripts/agentic_check.py` de c0der (mismo criterio, mismo
codigo: codigo HTTP + content-type + cuerpo distinto de la raiz), apuntado a
`http://127.0.0.1:8402`:

| sitio | HTTP | content-type | ok |
|---|---|---|---|
| `llms-txt` | 200 | `text/plain` | OK |
| `llms-full-txt` | 200 | `text/plain` | OK |
| `robots-txt` | 200 | `text/plain` | OK |
| `sitemap-xml` | 200 | `application/xml` | OK |
| `agent-card` | 200 | `application/json` | OK |
| `agent-json-legacy` | 200 | `application/json` | OK |
| `x402-discovery` | 200 | `application/json` | OK |
| `mcp-server-card` | 500 | — | **no aplica** (ver seccion 5) |
| `agent-skills-index` | 200 | `application/json` | OK |
| `api-catalog` | 200 | `application/linkset+json` | OK |
| `oauth-protected-resource` | 200 | `application/json` | OK |
| `openapi-json` | 200 | `application/json` | OK |
| `skill-md` | 200 | `text/markdown` | OK |
| `workflows-json` | 200 | `application/json` | OK |
| `index-md` | 200 | `text/markdown` | OK |
| `auth-md` | 200 | `text/markdown` | OK |
| `is-agentic` | 400 | `application/problem+json` | post-deploy |
| `ora-ai` | 404 | `application/json` | post-deploy |

**LOCAL 26/35 · 74.3%**

### Como reproducirla

```bash
# 1) binario local. La clave es efimera, de un solo uso, sin fondos y nunca se
#    escribe a disco: solo hace falta para que ProviderCache::from_env() no aborte.
cd /mnt/c/Users/lxhxr/orca/workspaces/x402-rs/x4-agentic
cp config/blacklist.json.example config/blacklist.json
rustup run stable cargo build --features solana,near,stellar,algorand,sui,xrpl
HOST=127.0.0.1 PORT=8402 RUST_LOG=warn SIGNER_TYPE=private-key \
  EVM_PRIVATE_KEY_TESTNET="0x$(python3 -c 'import secrets;print(secrets.token_hex(32))')" \
  RPC_URL_BASE_SEPOLIA=https://sepolia.base.org \
  ./target/debug/x402-rs &

# 2) el checker de c0der contra el local (no tiene flag --url; se le inyecta el nodo)
cd /mnt/z/ultravioleta/dao/c0der
PYTHONUTF8=1 python3 - <<'PY'
import importlib.util
spec = importlib.util.spec_from_file_location("ac", "scripts/agentic_check.py")
ac = importlib.util.module_from_spec(spec); spec.loader.exec_module(ac)
nodo = {"id": "facilitator", "url": "http://127.0.0.1:8402", "status": "live"}
f = ac.medir([nodo], ac.cargar_registro())["proyectos"][0]
print(f["puntos"], "/", f["posibles"], f["pct"])
for r in f["checks"]:
    print(("OK " if r["ok"] else "-- "), r["sitio"], r["http"], r["content_type"], r["motivo"] or "")
PY
```

**Dos cosas del entorno local que NO son defectos:**

1. `mcp-server-card` da **500** en local y **404** en produccion. El 500 es
   `Unable To Extract Key!` de `tower_governor`: `SmartIpKeyExtractor` lee
   `X-Forwarded-For`/`X-Real-IP`/`Forwarded` y en una conexion directa no hay
   ninguno. Detras del ALB el header existe y la ruta desconocida contesta 404
   (verificado con `curl` contra produccion). Es preexistente: cualquier ruta
   inexistente da 500 en local, tambien antes de esta rama.
2. `/version` y `info.version` del OpenAPI dicen `0.0.0` en local porque
   `FACILITATOR_VERSION` solo lo pasa el build de CI. En produccion es `2.9.0`.

---

## 3. Los archivos y sus rutas

14 archivos nuevos en `static/`, cada uno con su `.route()` explicita en
`handlers::agentic_routes()` (`src/handlers.rs:269`) y su `content-type` a mano,
mas el alias `/openapi.json` en `src/openapi.rs`.

| ruta servida | archivo | content-type |
|---|---|---|
| `/llms.txt` | `static/llms.txt` | `text/plain; charset=utf-8` |
| `/llms-full.txt` | `static/llms-full.txt` (generado) | `text/plain; charset=utf-8` |
| `/robots.txt` | `static/robots.txt` | `text/plain; charset=utf-8` |
| `/sitemap.xml` | `static/sitemap.xml` | `application/xml; charset=utf-8` |
| `/index.md` | `static/index.md` | `text/markdown; charset=utf-8` |
| `/skill.md` | `static/skill.md` | `text/markdown; charset=utf-8` |
| `/auth.md` | `static/auth.md` | `text/markdown; charset=utf-8` |
| `/workflows.json` | `static/workflows.json` | `application/json; charset=utf-8` |
| `/.well-known/agent-card.json` | idem | `application/json; charset=utf-8` |
| `/.well-known/agent.json` | idem (copia byte a byte) | `application/json; charset=utf-8` |
| `/.well-known/x402` | idem | `application/json; charset=utf-8` |
| `/.well-known/api-catalog` | idem | `application/linkset+json; charset=utf-8` |
| `/.well-known/oauth-protected-resource` | idem | `application/json; charset=utf-8` |
| `/.well-known/agent-skills/index.json` | idem | `application/json; charset=utf-8` |
| `/openapi.json` | alias de `/api-docs/openapi.json` | `application/json; charset=utf-8` |

**Nada de esto se sirve desde disco.** No hay `ServeDir` en `src/`; todo entra al
binario con `include_str!`, igual que `/logo.png`. Un archivo nuevo en `static/`
que no tenga su `include_str!` y su `.route()` no existe para el servicio — y el
test `the_table_covers_every_route` se pone rojo si alguien agrega una ruta sin
su fila en la tabla de pruebas.

### Los tests (11 nuevos, 698 unit tests verdes con las features de CI)

```bash
rustup run stable cargo test --locked -p x402-rs \
  --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1
# test result: ok. 698 passed; 0 failed
```

Lo que cubren, y por que cada uno:

- **200 + su content-type, por ruta.** Es el fallo silencioso que importa: un
  `skill.md` servido como `text/html` se ve perfecto en un browser y puntua cero.
- **Ninguna es HTML ni la landing page** — el `distinto_de_raiz` del checker.
- **Los JSON traen los campos que un consumidor indexa** (`name`/`description`/`url`,
  `skills`, `linkset`, `x402`, `resource`, `workflows`).
- **El discovery x402 sigue declarando `role: facilitator` y `paidRoutes: []`.**
  Si alguien pone precio a una ruta, este test avisa que el documento quedo mintiendo.
- **`llms-full.txt` sigue sincronizado** con sus cuatro fuentes, y **el sha256 del
  indice de skills sigue siendo el de `skill.md`**. Los dos verificados
  discriminantes: con una linea de mas en `skill.md` los dos se ponen rojos.
- **Todo link interno a `facilitator.ultravioletadao.xyz` resuelve a una ruta que
  alguien sirve.** Un catalogo que apunta a un 404 es justo el fallo que estos
  archivos existen para evitar.
- **Las 15 rutas estan en el spec de OpenAPI** (mismo test que ya existia para DX402)
  y `/openapi.json` devuelve el mismo documento, con `info.version` resuelta en runtime.

`clippy` limpio en las tres lineas tocadas; los warnings que quedan son
preexistentes y viven en archivos que esta rama no toca.

---

## 4. Los commits

```
a6ec307f feat(agentic): servir las 14 superficies y alias /openapi.json, con tests
c51add11 feat(agentic): los artefactos JSON de descubrimiento en /.well-known
0911a5b6 feat(agentic): los documentos de texto que un agente lee antes de llamar
97c27cca docs(agentic): medicion inicial de superficies agenticas del facilitador
```

19 archivos, +3377 / -2. `git status` limpio (solo `contracts/`, `target/` y
`node_modules/` sin trackear, que ya lo estaban).

---

## 5. Los checks que NO se pueden ganar con verdad, y que hay que decidir en c0der

### `mcp-server-card` (peso 3) — hace falta un `excluye` en el registro

El facilitador **no expone un servidor MCP**. Publicar una
`/.well-known/mcp/server-card.json` que apunte a un endpoint inexistente es
exactamente el fallo que el propio `agentic-sites.toml` documenta de meshrelay
("publica la suya y el endpoint que anuncia da 404 en GET y 401 en POST"). No se
fabrico.

**Lo que hay que decidir en c0der** — una de estas dos, ninguna la puede tomar esta
rama:

1. Agregar `excluye = ["facilitator"]` al bloque `[[sitio]]` de `mcp-server-card`
   con el motivo al lado (el campo ya existe y esta documentado en el TOML), o
2. levantar un servidor MCP de verdad delante del facilitador, que es un proyecto
   aparte y no una superficie estatica.

Sin esa decision el techo del facilitador es **32/35 (91.4%)**, no 100%.

### `oauth-protected-resource` — se publico, pero conviene revisarlo

El check solo pide 200 + `application/json` + cuerpo distinto de la raiz, asi que
un documento honesto lo pasa. Se publico uno con `authorization_servers: []` y un
`x-note` que explica que el servicio **no autentica llamadores**, con la misma forma
que el de execution-market. Sirve: un agente con OAuth descubre en un GET que aca
no hay nada que negociar, en vez de intentar y fallar. Si a c0der le parece que un
RFC 9728 degenerado no deberia contar, el cambio va en el `aplica_a` del TOML.

### `x402-discovery` — se publico y es honesto

El TOML no exige campos, asi que no hubo que inventar nada. El documento declara
`role: "facilitator"`, `paidRoutes: []` y los endpoints verify/settle/supported.
**No lleva direcciones de contrato a proposito**: un documento estatico con
direcciones se desincroniza del codigo y nadie se entera. Las redes llevan su
nombre v1, su CAIP-2 (copiado de `Network::to_caip2`) y los simbolos de sus
tokens; las direcciones se leen vivas de `POST /accepts`.

---

## 6. El deploy — PREPARADO, NO EJECUTADO

### Como sale

Pushear esta rama y mergearla a `main`. `.github/workflows/ci.yaml` testea,
buildea la imagen, la sube a ECR y hace `terraform apply -auto-approve` con
`-target` sobre la task definition y el servicio, espera el rollout y chequea
`/health`. **Un merge es un release.**

### CORRECCION A LA PREMISA DEL ENCARGO

El encargo decia que el drift gate rojo "salta el deploy" y que "hoy ni un push
despliega el facilitador". **Medido, eso es falso.** En la ultima corrida
(`33587276281`, 2026-09-02 03:30Z):

```
✓ Check deploy prerequisites          3s
✓ Build & test                     5m13s
X Terraform plan (drift gate)        35s   <- falla
✓ Push to ECR & deploy to ECS      8m41s   <- corre igual, y termina bien
```

El job `deploy` declara `needs: [test, preflight]` — **no** `plan`
(`.github/workflows/ci.yaml:273`), y el comentario de la linea 92 lo dice textual:
*"pre-existing drift must not be able to block a release"*. El gate es **advisory**.
La prueba independiente: `VERSION` dice `2.9.0` y
`curl -s https://facilitator.ultravioletadao.xyz/version` devuelve `2.9.0`, o sea
que el commit `chore: 2.9.0` llego a produccion con el gate en rojo.

Lo que el gate esta reportando hoy no tiene relacion con esta rama:

```
X aws_s3_bucket_ownership_controls.alb_logs differs from AWS and no deploy step targets it.
X aws_s3_bucket_lifecycle_configuration.alb_logs differs from AWS and no deploy step targets it.
```

Es drift de los buckets de logs del ALB. Se arregla aparte; no bloquea este release
y esta rama no lo toca.

### Verificacion despues del deploy

```bash
curl -s https://facilitator.ultravioletadao.xyz/version   # debe subir de 2.9.0
for p in /llms.txt /llms-full.txt /robots.txt /sitemap.xml /index.md /skill.md \
         /auth.md /workflows.json /openapi.json \
         /.well-known/agent-card.json /.well-known/agent.json /.well-known/x402 \
         /.well-known/api-catalog /.well-known/oauth-protected-resource \
         /.well-known/agent-skills/index.json; do
  printf "%-45s %s\n" "$p" \
    "$(curl -s -o /dev/null -w '%{http_code} %{content_type}' \
       https://facilitator.ultravioletadao.xyz$p)"
done

cd /mnt/z/ultravioleta/dao/c0der
PYTHONUTF8=1 python3 scripts/agentic_check.py --project facilitator
# esperado: 26/35 antes de disparar los escaneos, 32/35 despues
```

### Los dos escaneos que van DESPUES del deploy

**No los corrio esta rama** (miden la URL publica, y hoy esa URL todavia no sirve
los archivos). `ora.ai` permite 30 escaneos por dia y por IP: no los repitas en
bucle.

```bash
npx is-agentic facilitator.ultravioletadao.xyz --json
npx @ora-ai/ax@0.5 audit https://facilitator.ultravioletadao.xyz
```

Los dos `404` de la medicion inicial **no eran ceros**: significan que nadie habia
disparado el escaneo. `agentic-sites.toml` lo advierte para `is-agentic`. Una vez
que el reporte existe en su cache, el check pasa a leerlo y los 6 puntos entran.

---

## 7. Lo que queda esperando OK

1. **`git push` de la rama y merge a `main`** — un merge despliega a produccion.
2. **Los dos escaneos** (`is-agentic`, `ora.ai`), despues del deploy.
3. **La decision sobre `mcp-server-card`** en `c0der/config/agentic-sites.toml`.
   Sin ella el techo es 91.4%, no 100%.

## 8. Nota operativa: el CLI de Orca no corre desde WSL

Los tres binarios (`orca`, `orca.exe`, `orca.cmd` en
`/mnt/c/Users/lxhxr/AppData/Local/Programs/orca/resources/bin/`) fallan con
`cannot execute binary file: Exec format error`, y `cmd.exe` tambien: la
interoperabilidad Windows esta desactivada en esta instancia de WSL
(Ubuntu-24.04). Por eso el mensaje al buzon del Run no se pudo enviar desde aca y
el resultado vive en este handoff.
