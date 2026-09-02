# Facilitador x402: medicion inicial de superficies agenticas

**Fecha:** 2026-09-02
**Rama:** `0xultravioleta/x4-agentic` (worktree `/mnt/c/Users/lxhxr/orca/workspaces/x402-rs/x4-agentic`)
**Base:** `aad5c4c6` (origin/main al 2026-09-01 23:29 EDT)
**Release desplegado:** `2.9.0` (`curl -s https://facilitator.ultravioletadao.xyz/version` -> `{"version":"2.9.0"}`, y `VERSION` local dice lo mismo)

Este documento es la FASE 0 del encargo "de 0 a lo mas cerca de 100 en
`agentic_check.py`". Solo mide. No cambia nada del servicio.

---

## 1. El criterio, y de donde sale

El registro de superficies vive fuera de este repo, en c0der:

- `Z:/ultravioleta/dao/c0der/config/agentic-sites.toml` — el DATO: 24 bloques
  `[[sitio]]` con `id`, `tipo`, `que_mide`, `check`, `peso`, `aplica_a`, `estado`.
- `Z:/ultravioleta/dao/c0der/scripts/agentic_check.py` — el verificador. Solo
  hace `GET`/`HEAD` sin credenciales.

Comando (desde WSL):

```bash
cd /mnt/z/ultravioleta/dao/c0der
PYTHONUTF8=1 python3 scripts/agentic_check.py --project facilitator
```

Un check pasa solo si se cumplen **las tres cosas a la vez**
(`agentic_check.py:172-196`):

1. el codigo HTTP esperado (200),
2. el `content-type` base esta en la lista `espera_content_type`,
3. con `distinto_de_raiz = true`, el sha256 del cuerpo difiere del de `/`.

La tercera condicion existe porque las SPA del stack devuelven `index.html` con
200 en cualquier ruta. **El facilitador no tiene ese problema**: hoy responde
404 real en todas las rutas medidas, asi que cualquier archivo que sirvamos
sera automaticamente distinto de la raiz.

El facilitador esta declarado en los dos perfiles del TOML (`cobran` y
`con_api`), de modo que **los 18 checks puntuables le aplican**.

---

## 2. La medicion: 0 de 35 puntos (0.0%)

`data/agentic.json` de c0der, corrida `2026-09-02T07:11:11Z`:

| # | sitio | peso | ruta / url | HTTP | content-type | motivo |
|---|-------|------|------------|------|--------------|--------|
| 1 | `llms-txt` | 3 | `/llms.txt` | 404 | ausente | http 404 |
| 2 | `llms-full-txt` | 1 | `/llms-full.txt` | 404 | ausente | http 404 |
| 3 | `robots-txt` | 2 | `/robots.txt` | 404 | ausente | http 404 |
| 4 | `sitemap-xml` | 1 | `/sitemap.xml` | 404 | ausente | http 404 |
| 5 | `agent-card` | 3 | `/.well-known/agent-card.json` | 404 | ausente | http 404 |
| 6 | `agent-json-legacy` | 1 | `/.well-known/agent.json` | 404 | ausente | http 404 |
| 7 | `x402-discovery` | 3 | `/.well-known/x402` | 404 | ausente | http 404 |
| 8 | `mcp-server-card` | 3 | `/.well-known/mcp/server-card.json` | 404 | ausente | http 404 |
| 9 | `agent-skills-index` | 2 | `/.well-known/agent-skills/index.json` | 404 | ausente | http 404 |
| 10 | `api-catalog` | 2 | `/.well-known/api-catalog` | 404 | ausente | http 404 |
| 11 | `oauth-protected-resource` | 1 | `/.well-known/oauth-protected-resource` | 404 | ausente | http 404 |
| 12 | `openapi-json` | 2 | `/openapi.json` | 404 | ausente | http 404 |
| 13 | `skill-md` | 2 | `/skill.md` | 404 | ausente | http 404 |
| 14 | `workflows-json` | 1 | `/workflows.json` | 404 | ausente | http 404 |
| 15 | `index-md` | 1 | `/index.md` | 404 | ausente | http 404 |
| 16 | `auth-md` | 1 | `/auth.md` | 404 | ausente | http 404 |
| 17 | `is-agentic` | 3 | `is-agentic.com/api/v1/report?url=...` | 404 | `application/problem+json` | no hay reporte cacheado |
| 18 | `ora-ai` | 3 | `ora.ai/api/score/{dominio}` | 404 | `application/json` | no hay score cacheado |

**Total: 0 / 35 (0.0%).** Suma de superficies: 29 puntos. Suma de rankings: 6.

Ademas hay 6 sitios en `estado = "por-verificar"` que **no puntuan** y no
restan: `isitagentready`, `coinbase-bazaar`, `pay-sh-catalog`, `mcp-registry`,
`erc-8004`, `describe-net-listing`.

### Lo que dicen los dos 404 de los rankings

No son ceros. `agentic-sites.toml` lo advierte para `is-agentic`: la API solo
devuelve reportes ya cacheados, y `report_not_found` significa que **nadie
disparo el escaneo**, no que el sitio saco cero. Los dos se disparan DESPUES
del deploy, no ahora (ora.ai limita a 30 escaneos por dia y por IP).

---

## 3. Como sirve hoy sus archivos el facilitador (lo que hay que copiar)

Medido en el codigo de esta rama, no supuesto:

- **Rutas estaticas explicitas, una por archivo**, registradas en
  `handlers::routes()` (`src/handlers.rs:252` en adelante; las de archivos
  arrancan en `:284` con `/logo.png`).
- **El contenido va embebido en el binario en tiempo de compilacion**:
  `include_str!("../static/index.html")` para texto (`src/handlers.rs:1745`) e
  `include_bytes!("../static/logo.png")` para binario (`src/handlers.rs:1803`).
  No hay `ServeDir` ni `ServeFile` en todo `src/` (grep verificado): el binario
  nunca lee `static/` en runtime. El `Dockerfile:114` si copia `/app/static` a
  la imagen, pero ahi no lo sirve nadie, asi que **un archivo que no tenga su
  `include_*!` y su `.route(...)` no existe para el servicio**.
- El `content-type` se pone a mano en cada handler
  (`text/html; charset=utf-8`, `image/png`, `image/x-icon`).
- `main.rs` mergea los routers: `handlers::routes()` con estado en
  `src/main.rs:620`, y routers sin estado como `openapi::swagger_routes()` en
  `src/main.rs:646`.

### Donde esta el OpenAPI hoy

`src/openapi.rs:2066`:

```rust
Router::new().merge(SwaggerUi::new("/docs").url("/api-docs/openapi.json", api_doc))
```

O sea: el JSON se sirve en **`/api-docs/openapi.json`**, y el check
`openapi-json` del TOML pide **`/openapi.json`**. Hace falta un alias en la raiz
que devuelva el mismo documento con `application/json`. La version del spec no
hay que tocarla: `swagger_routes()` la parcha en runtime desde `VERSION`
(`src/version.rs`).

---

## 4. Los numeros reales del servicio (para no inventarlos al escribir los archivos)

Todos medidos hoy contra produccion y contra el codigo de esta rama:

- **39 identificadores de red v1** en `/supported`
  (`curl -s .../supported | jq -r '[.kinds[].network]|unique|.[]' | grep -v ':' | wc -l`).
  `/supported` lista cada red dos veces (nombre v1 + alias CAIP-2): 114 `kinds`.
- **21 redes mainnet de pago**, segun `python3 scripts/verify_landing_canonical.py`
  (fuente canonica): algorand, arbitrum, avalanche, base, bsc, celo, ethereum,
  fogo, hyperevm, monad, near, optimism, polygon, robinhood, scroll, skale-base,
  solana, stellar, sui, unichain, xrpl.
- **5 esquemas**: `exact`, `upto`, `escrow`, `commerce`, `fhe-transfer`.
- **6 stablecoins** (`python3 scripts/stablecoin_matrix.py --json`): USDC, USDT,
  EURC, AUSD, PYUSD, USDG.
- **Escrow en 9 mainnets**; **ERC-8004 en 12 mainnets / 21 redes en total**.
- `SEI` y `XDC` salen en `stablecoin_matrix.py` pero **no** en el `/supported`
  de produccion (son enum-only). No van en los archivos publicos.

---

## 5. Alcance realista del score

- `mcp-server-card` (peso 3) **no se puede ganar con verdad**: el facilitador no
  expone un servidor MCP. Publicar una card que apunte a un endpoint inexistente
  es exactamente el fallo que el propio TOML documenta de meshrelay. Se reporta,
  no se fabrica.
- `is-agentic` y `ora-ai` (3 + 3) **solo se pueden ganar despues del deploy**,
  disparando los dos escaneos.
- Techo de esta rama, medible en local: **26 de 29 puntos de superficie**.
- Techo despues del deploy + los dos escaneos: **32 de 35 (91.4%)**, y el 100%
  requiere una decision en c0der sobre `mcp-server-card` (un `excluye` para
  `facilitator`, o un `aplica_a` distinto).

## 6. Bloqueo de deploy conocido (no es de esta rama)

El CI (`.github/workflows/ci.yaml`) despliega a ECS al pushear `main`, pero
medido el 2026-09-01 22:30 EDT sus tres ultimas corridas fallan en el paso
"Terraform plan (drift gate)" y saltan el deploy. Hoy ningun push despliega el
facilitador. Se reporta; no se arregla en este encargo.
