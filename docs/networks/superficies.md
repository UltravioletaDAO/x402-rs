# Dónde aparece una red para quien la va a usar — inventario vigente

**Medido el 2026-09-23** sobre `origin/main` 2.39.1 (`a7e818db`) más los cambios de 2.39.2,
contra el `/supported` de producción del mismo día. Reemplaza al inventario del handoff de
la rama `0xultravioleta/x4-docs-uso` (`f1fbec72`, medido sobre `115b8c46` el 2026-09-16),
que nunca llegó a `main`. Las líneas envejecen: buscá por el nombre del archivo, la
constante o el test.

## Cómo leer la tabla

- **Generada**: sale sola de otra fuente (`/supported`, `src/network.rs`, un script). Una red
  nueva aparece sin tocarla.
- **A mano**: alguien la escribe. Una red nueva no aparece hasta que alguien la agrega.
- **Compilada**: el archivo entra al binario con `include_str!`/`include_bytes!`
  (`src/handlers.rs`, `src/mcp.rs`, `src/receipts/mod.rs`), así que cambiarlo es una imagen
  nueva y **mergearlo es un despliegue a producción**. Son 65 archivos de `static/` después
  de 2.39.2 (eran 66: se retiró `og-arc-hedera.png`); fuera quedan `LANDING_PAGE.md`,
  `README.md`, `SETUP.md`. Nada de `config/` se compila fuera de los tests: el Dockerfile
  copia `config/` a la imagen, pero `config/**` dispara el CI igual.
- **Qué lo ataja**: el test o el paso de CI que falla si la superficie deriva. «Nada» quiere
  decir que se buscó el nombre del archivo en `src/`, `tests/` y `.github/` sin resultado.

## Superficies

| Superficie | Archivo | Cómo se produce | Compilada | Qué lo ataja |
|---|---|---|---|---|
| `/supported` | `src/facilitator_local.rs` (`supported()`) | **generada** del mapa de proveedores (red con RPC configurado) | código | es la fuente |
| `/health/ready` | `src/readiness.rs` | **generada** del mapa de proveedores; sondea EVM y Hedera, el resto sale `unchecked` | código | tests de `readiness.rs` |
| Catálogo del bazar | `/discovery/resources`, `/bazaar` | **generado** del registro; `settleable` consulta el mapa de proveedores desde 2.39.2 | código | `an_offer_on_a_network_nothing_serves_is_not_settleable` (`tests/bazaar_pricing.rs`) |
| Página `/networks`, tabla principal y tabla por familia | `static/networks.html` | **generadas** desde `/supported` (la de familias era a mano el 2026-09-16); la regex que asigna familia es a mano y manda a EVM lo que no reconoce | sí | nada |
| `/networks`, `PUBLIC_RPCS` | `static/networks.html` | **a mano**, 39 claves, sin Arc ni Hedera (hay respaldo detrás de `/api/balances`) | sí | nada |
| `/networks`, tabla de wallets | `static/networks.html` | **a mano**; las filas de Hedera salen del `feePayer` de `/supported` | sí | nada |
| Página `/x402` | `static/x402.html` | **generada** desde `/supported` | sí | — |
| Portada: 44 tarjetas `network-badge` | `static/index.html` | **a mano** (23 mainnet + 21 testnet; `ethereum-sepolia` tiene dos: gas y Zama FHE) | sí | `tests/frontend-capabilities.test.cjs` («all landing cards map to a distinct network…»): unicidad, no completitud |
| Portada, `PUBLIC_RPCS` | `static/index.html` | **a mano**, 41 claves, con Arc, sin Hedera | sí | nada |
| Portada, meta description (en y es) | `static/index.html` | **a mano**, lista de familias | sí | `every_published_family_list_names_every_family` (`src/mcp.rs`, nuevo en 2.39.2) |
| Vista previa de enlaces (`og:image`) | las 10 páginas | **a mano**, todas `/logo.png` | sí | nada |
| Íconos de red, `ICONO_DE_RED` | `static/x402.js` | **a mano** | sí | `the_static_surfaces_name_every_chain_by_its_published_caip2_id` (`src/handlers.rs`, #98): solo la clave CAIP-2 de cada `Network::variants()`; las claves v1 no |
| Guía para agentes | `static/skill.md` (familias, tabla Arc/Hedera, tabla de dominios, ERC-8004) | **a mano** | sí | `the_skills_index_digest_matches_skill_md`, `llms_full_txt_is_in_sync_with_its_sources`: digest y sincronía, no las listas |
| Índices para LLM | `static/llms.txt`, `static/index.md` | **a mano** | sí | sincronía de `llms-full.txt` |
| `llms-full.txt` | `static/llms-full.txt` | **generado** por `scripts/build_llms_full.sh` | sí | `llms_full_txt_is_in_sync_with_its_sources` |
| Tarjeta A2A | `static/.well-known/agent-card.json` = `agent.json` | **a mano**, nombra Hedera | sí | `the_legacy_agent_json_is_the_same_card` |
| Descubrimiento x402 | `static/.well-known/x402` (`networks[]` 21, `testnets[]` 18) | **a mano** | sí | el test CAIP-2 de #98 (cada entrada listada), no la completitud |
| Catálogo ARD | `static/.well-known/ard.json` (descripción) | **a mano**, lista de familias | sí | `the_ard_catalog_meets_the_spec` (estructura) y `every_published_family_list_names_every_family` |
| Tarjeta del servidor MCP | `static/.well-known/mcp/server-card.json` (descripción) | **a mano**, lista de familias | sí | `the_server_card_describes_the_server_that_answers` (nombre, versión, endpoint, tools) y `every_published_family_list_names_every_family` |
| Descripción del servidor MCP | `src/mcp.rs` (`get_info`) | **a mano**, lista de familias | código | `every_published_family_list_names_every_family` |
| Guía MCP | `static/mcp.md` | **a mano** | sí | negociación de markdown, no el contenido |
| Sitemap | `static/sitemap.xml` (16 URLs, sin lista de redes) | **a mano**; cada `lastmod` es la fecha de commit del archivo que respalda la URL | sí | `the_sitemap_stamps_every_url` (formato, no frescura) |
| Estadísticas | `static/stats.html` | **generada** de `/api/stats` | sí | — |
| Capacidad de recibos | `src/receipts/mod.rs` | **a mano**, 4 ids CAIP-2 | código | `capability_lists_exactly_the_supported_networks` (`src/receipts/tests.rs`) |
| Guía de integración | `static/integrar.html` | **a mano**, nombra Arc y Hedera | sí | nada |
| `auth.md` | `static/auth.md` | **a mano**, conteos de ERC-8004 (13 / 23) | sí | nada sobre el conteo |
| OpenAPI / Swagger | `src/openapi.rs` | **a mano**: prosa de redes de pago, ERC-8004, escrow | código | `the_erc8004_prose_names_every_supported_network` y el test de la prosa del relé; la prosa de redes de pago, nada |
| JSON de tokens | `config/supported_tokens.json` | **a mano**; 41 nombres v1, sin Hedera | no | nada (lo leen `scripts/arc_canary.py` y un script de escaneo) |
| `.env.example` | `.env.example` | **a mano** | no, y no dispara el CI | nada |
| Matriz de stablecoins | `scripts/stablecoin_matrix.py` | **generada** de `src/network.rs`, solo mainnets | — | no corre en el CI |
| Verificador de la portada | `scripts/verify_landing_canonical.py` | lee `/supported`, `payment_operator` y `erc8004/mod.rs` | — | paso «Landing page matches its canonical sources (offline)» del CI: conteos de ERC-8004 y escrow en/es, hero, enlaces a upstream |
| Saldos de la portada | `lambda/balances/handler.py` | **a mano** (claves fijas + `arc-*` si hay `RPC_URL_ARC*`, `hedera-*` si hay `HEDERA_ACCOUNT_ID_*`) | no; `lambda/**` dispara el CI | paso «Native balance monitor unit tests» (comportamiento) |
| Alarmas de saldo | `terraform/environments/production/alerts.tf` | piso derivado de `/health/ready` o declarado, el mayor (2.39.2) | no | `the_low_balance_alarm_is_derived_from_these_thresholds` (`src/readiness.rs`) |
| Guías de uso por red | `docs/networks/arc.md`, `arc-operations.md`, `hedera.md` | **a mano** | no | nada |
| README | `README.md` (tablas de mainnets, testnets y stablecoins) | **a mano** | no, y no dispara el CI | nada |
| Checklists de alta | `guides/ADDING_NEW_CHAINS.md`, `.claude/skills/add-network/SKILL.md` | **a mano**, de operador | no | nada |
| SDKs y ejemplos | `crates/`, `examples/` | no listan redes: `git grep -il 'robinhood\|skale-base\|hedera' -- crates examples` → 0, y `arc` solo aparece como el tipo `Arc` de Rust | — | — |

## Conteos del mismo día

- `/supported`: 84 identificadores distintos, 41 nombres v1 y **43 redes** (los 41 más
  `hedera:mainnet` y `hedera:testnet`, que no tienen nombre v1); 42 sirven `exact`, porque
  `ethereum-sepolia` no lo sirve (sirve `fhe-transfer`, `escrow` y `commerce`).
- Portada: 44 tarjetas, 23 mainnet y 21 testnet. Las mainnet coinciden con `/supported`
  (23 = 23); la tarjeta de más es la segunda de `ethereum-sepolia`.

## Deriva abierta, medida el 2026-09-23 y no corregida en 2.39.2

1. `static/.well-known/x402` no lista `arc`, `arc-testnet` ni Hedera, y lista
   `ethereum-sepolia`, que no sirve `exact`. Comando: comparar los `name` de `networks[]` y
   `testnets[]` con los nombres v1 `exact` de `/supported`.
2. `src/openapi.rs` nombra una «Monad Testnet» en la prosa de testnets; no existe esa
   variante en `src/network.rs`.
3. `static/integrar.html` dice en inglés que Arc usa autorizaciones «USDC» y en español
   «USDC/EURC»; `/supported` anuncia USDC y EURC en `arc`. `static/mcp.md` también dice solo
   USDC.
4. `config/supported_tokens.json` no tiene Hedera.
5. La columna Token de la tabla de mainnets del README dice solo «USDC» en cadenas donde su
   propia tabla de stablecoins lista más.
6. Los checklists de alta (`SKILL.md`, `ADDING_NEW_CHAINS.md`) siguen hablando de 39
   tarjetas y no nombran `.well-known/x402`, `skill.md`, `llms.txt`, `index.md`, las
   tarjetas MCP/ARD ni `src/mcp.rs`.

## Cómo se re-mide

- Compilados: `git grep -n -E 'include_(str|bytes)!' -- src` antes del primer
  `#[cfg(test)]` de cada archivo, contra `git ls-tree -r HEAD --name-only static`.
- Tarjetas: `grep -o 'class="network-badge' static/index.html | wc -l`.
- Redes servidas: `curl -s https://facilitator.ultravioletadao.xyz/supported | jq -r '[.kinds[].network]|unique|.[]' | grep -v ':' | wc -l`.
- Qué lo ataja: `git grep -n '<archivo>' -- src tests .github`.
