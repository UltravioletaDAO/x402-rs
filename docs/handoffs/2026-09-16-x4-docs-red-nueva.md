# El checklist de alta de red nueva, corregido contra la medicion — handoff 2026-09-16

- **Rama**: `0xultravioleta/x4-docs-red-nueva` (desde `origin/main` dc109511, 2.29.6)
- **VERSION**: sin cambios. No toca `src/`, ni `static/`, ni `scripts/`, ni
  `terraform/`, asi que **no dispara CI y no despliega** (`paths` de
  `.github/workflows/ci.yaml:36-67`: `.claude/**`, `docs/**` y `guides/**` no
  estan en la lista).
- **Que cambia**: `.claude/skills/add-network/SKILL.md` y
  `guides/ADDING_NEW_CHAINS.md` — las dos copias del checklist que se usan para
  dar de alta una red.
- **Que no cambia**: nada de codigo, nada de infraestructura, ningun script.
  `.claude/skills/add-network/scripts/research_network.py` y
  `references/known_networks.md` se revisaron y no traian ninguna de las derivas.

## 1. Las cuatro derivas: medicion y correccion

| # | Deriva | Medicion mia | Correccion |
|---|---|---|---|
| a | Manda editar `variants()`, `mainnet_variants()`, `testnet_variants()` y `evm_variants()` | `grep -rn -e mainnet_variants -e testnet_variants -e evm_variants src/ crates/ examples/` -> **0 hits**. Lo unico que existe son **cuatro copias de la misma `Network::variants()`**, cada una bajo un `#[cfg]` distinto de `algorand`/`sui`: `src/network.rs:346` (39 entradas), `:394` (37), `:440` (37), `:486` (35) | SKILL.md §3.1 y guia §1.1: tabla con los cuatro `cfg`, la linea y la cantidad de entradas; se dice que la de produccion es solo la primera y que las otras tres pueden estar mal con build verde |
| b | Manda `git add -A` | `.claude/skills/add-network/SKILL.md:560` (antes de este cambio): `git add -A && git commit -m "feat: add {Network}..."` | SKILL.md §5.2 y guia §5.2: staging por archivo con la lista completa de rutas del alta, `git status --short` + `git diff --cached --stat` antes de commitear, y `git config core.hooksPath .githooks` |
| c | Describe un despliegue manual que ya no existe | El repo no tiene `task-def-final.json` (`find . -name 'task-def*'` vacio). `git tag` termina en `v2.0.2` y `VERSION` dice `2.29.6`. `Cargo.toml:3` es un placeholder congelado `0.0.0`. El deploy real: `ci.yaml:473-546` calcula `image_tag=$(cat VERSION)-$(git rev-parse --short HEAD)`, construye con `--build-arg FACILITATOR_VERSION`, empuja a ECR y hace `terraform apply -auto-approve` con `-target=aws_ecs_task_definition.facilitator -target=aws_ecs_service.facilitator` + autoscaling, y despues espera el rollout y consulta `/health` (`:643-659`) | SKILL.md Fase 5 y guia Fase 5 reescritas: bump de `VERSION` (no de `Cargo.toml`), staging por archivo, `git push origin main`, y los cinco pasos que corre CI. Se dice explicito que `aws ecs update-service --force-new-deployment` **reejecuta la task definition actual y no mueve la imagen**, y que ya no se taggea |
| d | No nombra `src/caip2.rs`, `VERSION`, `docs/CHANGELOG.md`, el piso de gas, las listas de escrow/upto, las variables RPC de la task definition ni el guard de la portada | Ver §2 | Ver §2 |

## 2. Deriva (d), item por item

| Item | Medicion | Donde quedo |
|---|---|---|
| `src/caip2.rs` | **La deriva no es tal para una cadena EVM.** El archivo es generico sobre `eip155:<chain-id>` (`Caip2NetworkId::eip155`, `src/caip2.rs:175`) y no tiene tabla por red; su diff en el ultimo alta real (`7dbe194e`) fue `rustfmt` puro. Solo cambia para una **familia** nueva (un `Namespace` nuevo, `:52`). Lo que si hay que editar es `to_caip2()` (`src/network.rs:576`, exhaustivo) y `from_caip2()` (`:643`, con `_ => None` en `:704`, **no** exhaustivo) | SKILL.md §3.11b y guia (seccion "No en el inventario"), dicho como lo que es |
| `VERSION` | `Cargo.toml:3` = `version = "0.0.0" # frozen placeholder`. `src/version.rs:30` cae a `CARGO_PKG_VERSION` solo si falta `FACILITATOR_VERSION`. CI falla el run si `VERSION` esta vacio (`ci.yaml:481`) | SKILL.md §3.14 + §5.1; guia §5.1. Corregido tambien el §3.10 de SKILL.md, que decia que la version "auto-sincroniza desde `Cargo.toml` via `env!("CARGO_PKG_VERSION")`" |
| `docs/CHANGELOG.md` | Sin tags despues de `v2.0.2`, el CHANGELOG es el unico registro fechado de un release | SKILL.md §3.14; guia §7.2 ("no opcional", ya no "if exists") |
| Piso de gas | `eip1559_fee_floor()`, `src/chain/evm.rs:329`, es un `match` con brazo `_`: una red nueva hereda el default (tip 1 mwei) **sin que el compilador diga nada**. Brazos explicitos: Ethereum/Sepolia y Polygon/Amoy | SKILL.md §3.3, con la advertencia de no copiar el brazo de Ethereum (incidente 2026-09-10..14, corregido en 2.29.4) ni poner cero |
| Listas de escrow y `upto` | `ESCROW_NETWORKS`, `src/payment_operator/addresses.rs:185`, con `assert_eq!(ESCROW_NETWORKS.len(), 11)` en `:452`. `UPTO_DEPLOYED_NETWORKS`, `src/upto/types.rs:60`, 11 redes | SKILL.md §3.3b y la tabla de condicionales de los dos archivos. Se aclara que son opt-in: una red nueva recibe `exact` gratis y nada mas |
| Variables RPC de la task definition | `git show 7dbe194e -- terraform/environments/production/main.tf` agrega `RPC_URL_ROBINHOOD` y `RPC_URL_ROBINHOOD_TESTNET` al bloque `environment`. Declarar solo en `src/from_env.rs` no le da la URL al contenedor | SKILL.md §3.13 y guia §2.3, que reemplaza `task-def-final.json` por `main.tf` / `secrets.tf`. Marcado como **el paso que decide si la red aparece en `/supported`** |
| Guard de la portada | `scripts/verify_landing_canonical.py:306`: `--expect-mainnets` default `21`, hardcodeado; el header `:11` repite el numero. Su propio docstring (`:30-33`) pide que se enganche a este skill | SKILL.md §3.11 y guia §5.3. Se agrega la forma `--offline` (la que corre CI) y el bump del default |

Dos correcciones extra que salieron de medir §3.11 y que estaban al reves:

- **`data-i18n="sdk.networks"` ya no existe.** `python scripts/verify_landing_canonical.py --offline`
  imprime `typed 'N mainnets' : not typed on this page`. El skill mandaba
  actualizar ese string en EN y ES.
- **El muro de saldos de `/` sigue siendo a mano** (39 tarjetas
  `class="network-badge ..."` en `static/index.html`, confirmadas vivas:
  `curl -s https://facilitator.ultravioletadao.xyz/ | grep -c 'class="network-badge'`
  -> 39), pero **`/networks` no**: `static/networks.html` arma toda su tabla
  desde `GET /supported`. Lo que si hace falta ahi es `ICONO_DE_RED`
  (`static/x402.js:14`), cuatro claves por red — un archivo que ninguna de las
  dos copias del checklist nombraba.

## 3. Inventario real de archivos

Medido sobre `7dbe194e` (Robinhood Chain, 2026-07-20), el ultimo alta completa:
`git show --stat 7dbe194e` -> **24 archivos, 689 inserciones, 216 borrados**.

Cinco de esos 24 no eran trabajo del alta y quedan fuera:

| Archivo | Por que no cuenta |
|---|---|
| `src/caip2.rs`, `src/chain/xrpl.rs`, `src/facilitator_local.rs`, `examples/x402-reqwest-example/src/main.rs` | `rustfmt` de archivo entero. `git show 7dbe194e -- examples/x402-reqwest-example/src/main.rs \| grep -ci robinhood` -> 0 |
| `src/upto/permit2.rs` | Arreglo de seguridad (`assert_proxy_deployed`) que viajo en el mismo commit |

Dos se sumaron despues: `VERSION` (no existia; `git cat-file -e 7dbe194e:VERSION`
falla, lo creo `37e9e68b`) y `static/x402.js` (lo creo `19178c83`).

**Resultado: 17 archivos siempre, hasta 24 con los condicionales.** La tabla
completa, con la columna "¿lo agarra el compilador?", esta en SKILL.md
("File inventory") y en la guia ("Quick Reference: File Changes Summary").

Se documentan los dos numeros por separado en vez de uno solo: cual aplica depende
de si la red trae stablecoin nueva, escrow, ERC-8004 o RPC con API key. Un solo
numero global habria sido falso en los dos sentidos segun el caso.

## 4. Criterio de cierre: `/supported`, nunca "compila"

Agregado a los dos documentos, con la prueba:

`Sei`, `SeiTestnet` y `XdcMainnet` estan declaradas en el enum `Network` y
cableadas en **todos** los sitios que el compilador exige — `Display`
(`src/network.rs:161`, `:174`, `:175`), `FromStr` (`:223`, `:236`, `:237`),
`to_caip2`, `from_caip2`, `NetworkFamily` (`:293`, `:306`, `:307`). Compilan,
serializan, y las dos formas de cada una resuelven.

Estan en **cero de las cuatro copias de `variants()`**. Medido sobre `dc109511`:

| | cantidad |
|---|---|
| variantes del enum `Network` | 42 |
| union de las cuatro copias de `variants()` | 39 |
| en el enum y en ninguna copia | 3 — `Sei`, `SeiTestnet`, `XdcMainnet` |
| nombres v1 distintos en `/supported` vivo | 39 |
| union vs `/supported` vivo | **identicos, nombre por nombre** |

Esa ultima fila es el mecanismo: `ProviderCache::from_env` itera
`Network::variants()` (`src/provider_cache.rs:113`) y `/supported` recorre el
mapa de proveedores (`src/facilitator_local.rs:289`). Una variante fuera de
`variants()` no recibe proveedor y no se publica en ningun lado — sin error, sin
warning y sin test rojo. `variants()` devuelve un array, no un `match`: el
compilador no puede avisar.

Reproducir:

```bash
curl -s https://facilitator.ultravioletadao.xyz/supported \
  | jq -r '[.kinds[].network]|unique|.[]' | grep -v ':' | wc -l   # 39
curl -s https://facilitator.ultravioletadao.xyz/supported \
  | jq -r '[.kinds[].network]|unique|.[]' | grep -iE 'sei|xdc'    # nada
```

Los tres sitios por red que el compilador **no** exige, y por lo tanto donde una
red se pierde en silencio, quedan listados en los dos documentos: las cuatro
copias de `variants()`, `FromStr` (`_ => Err`, `src/network.rs:269`) y
`from_caip2()` (`_ => None`, `:704`).

## 5. Verificacion de este cambio

```bash
git diff --name-only
# .claude/skills/add-network/SKILL.md
# guides/ADDING_NEW_CHAINS.md
```

Entero dentro de `.claude/` y `guides/`, mas este handoff en `docs/`. Ninguno de
los tres arboles esta en el `paths` de `.github/workflows/ci.yaml`, asi que el PR
no corre workflows ni despliega.

## 6. Lo que queda

- `src/openapi.rs` esta al dia en ERC-8004 (`:59` dice 21 redes, 12 mainnets + 9
  testnets, que coincide con `supported_networks()`), pero **desfasado en
  escrow**: `:826` lista 9 nombres y dice "9 total", mientras `ESCROW_NETWORKS`
  (`src/payment_operator/addresses.rs:185`) tiene 11 entradas — faltan Optimism y
  SKALE Base. Es codigo, fuera del alcance de esta rama. (De paso: el checklist
  citaba numeros de linea de `src/openapi.rs` que ya habian derivado; se
  reemplazaron por un `grep`.)
- `scripts/verify_landing_canonical.py:11` dice `[11 mainnets / 20 total]` para
  ERC-8004 y el propio script midio 12 / 21. Es `scripts/`, fuera de alcance a
  proposito (tocarlo mete el PR en los paths de CI).
- `CLAUDE.md` repite el conteo viejo de ERC-8004 (20) y de escrow. Esta en la
  raiz, fuera del alcance de esta rama.
