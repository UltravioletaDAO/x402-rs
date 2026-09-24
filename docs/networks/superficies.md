# Dónde aparece una red para quien la va a usar — inventario vigente

**Medido el 2026-09-23** sobre `origin/main` 2.39.1 (`a7e818db`) más los cambios de 2.39.2,
contra el `/supported` de producción del mismo día. Reemplaza al inventario del handoff de
la rama `0xultravioleta/x4-docs-uso` (`f1fbec72`, medido sobre `115b8c46` el 2026-09-16),
que nunca llegó a `main`. Las líneas envejecen: buscá por el nombre del archivo, la
constante o el test. **Actualizado el 2026-09-24** (2.41.0, `GET /networks.json`): las filas
de íconos, exploradores, JSON de tokens, `/networks` y el visor de `/events` cambiaron de
«a mano» a «generada».

## Cómo leer la tabla

- **Generada**: sale sola de otra fuente (`/supported`, `src/network.rs`, un script). Una red
  nueva aparece sin tocarla.
- **A mano**: alguien la escribe. Una red nueva no aparece hasta que alguien la agrega.
- **Compilada**: el archivo entra al binario con `include_str!`/`include_bytes!`
  (`src/handlers.rs`, `src/mcp.rs`, `src/receipts/mod.rs`), así que cambiarlo es una imagen
  nueva y **mergearlo es un despliegue a producción**. Son 65 archivos de `static/` después
  de 2.39.2 (eran 66: se retiró `og-arc-hedera.png`); fuera quedan `LANDING_PAGE.md`,
  `README.md`, `SETUP.md`. De `config/` solo se compila `supported_tokens.json` (desde
  2.41.0, para `/networks.json`); el Dockerfile copia `config/` a la imagen y `config/**`
  dispara el CI.
- **Qué lo ataja**: el test o el paso de CI que falla si la superficie deriva. «Nada» quiere
  decir que se buscó el nombre del archivo en `src/`, `tests/` y `.github/` sin resultado.

## Superficies

| Superficie | Archivo | Cómo se produce | Compilada | Qué lo ataja |
|---|---|---|---|---|
| `/supported` | `src/facilitator_local.rs` (`supported()`) | **generada** del mapa de proveedores (red con RPC configurado); la salud de una red no la saca desde 2.39.4 | código | es la fuente; `a_ledger_failing_its_startup_health_is_still_served`, `from_env_serves_arc_whatever_its_rpc_answers_and_the_rest_up` |
| `/networks.json` | `src/networks_json.rs` | **generada** del cuerpo de `/supported` (una fila por red, mismos `id` y `caip2`); solo la presentación (`displayName`, ícono, explorador, `usdPegged`) sale de `config/supported_tokens.json`, compilado (2.41.0) | código + JSON | `networks_json::tests` (conjunto de ids igual a `/supported`, en la captura de producción y por el router; cada fila con explorador e ícono servido) |
| `/health/ready` | `src/readiness.rs` | **generada** del mapa de proveedores; sondea EVM (con chain id) y Hedera, el resto sale `unchecked`; lista toda red configurada con estado, motivo y `caip2` (2.39.4) | código | tests de `readiness.rs` |
| Portada, punto de estado por tarjeta | `static/index.html` + `cardHealth` en `static/x402.js` | **generado** de `/health/ready` (2.39.4): punto rojo si `degraded`/`down`, nada si `ok`, sin sondear o ilegible | sí | `tests/frontend-capabilities.test.cjs` («card health…») |
| Catálogo del bazar | `/discovery/resources`, `/bazaar` | **generado** del registro; `settleable` consulta el mapa de proveedores desde 2.39.2 | código | `an_offer_on_a_network_nothing_serves_is_not_settleable` (`tests/bazaar_pricing.rs`) |
| Página `/networks`, tabla principal y tabla por familia | `static/networks.html` | **generadas** desde `/supported`, con íconos de `/networks.json` (2.41.0); la regex que asigna familia es a mano y manda a EVM lo que no reconoce | sí | `tests/frontend-capabilities.test.cjs` (íconos) |
| `/networks`, `PUBLIC_RPCS` | `static/networks.html` | **a mano**, 39 claves, sin Arc ni Hedera (hay respaldo detrás de `/api/balances`) | sí | nada |
| `/networks`, tabla de wallets | `static/networks.html` | **a mano** (filas, orden y rótulos); desde 2.41.0 cada fila solo NOMBRA su red: ícono y explorador salen de `/networks.json`, las direcciones de Sui y las filas de Hedera del `feePayer` de `/supported` | sí | `static_types_no_explorer_and_no_icon` (ningún explorador tipeado) y «/networks keeps the wallet table origin/main shows…» (`tests/frontend-capabilities.test.cjs`) |
| Página `/x402` | `static/x402.html` | **generada** desde `/supported` | sí | — |
| Portada: 44 tarjetas `network-badge` | `static/index.html` | **a mano** (23 mainnet + 21 testnet; `ethereum-sepolia` tiene dos: gas y Zama FHE); desde 2.41.0 cada tarjeta solo NOMBRA su red (`data-explorer`, `data-net-icon`): ícono y enlace al explorador salen de `/networks.json`, y la dirección de Sui y Hedera del `feePayer` de `/supported` | sí | `tests/frontend-capabilities.test.cjs` («all landing cards map to a distinct network…», «static pages type no explorer and no icon…»): unicidad, no completitud |
| Portada, `PUBLIC_RPCS` | `static/index.html` | **a mano**, 41 claves, con Arc, sin Hedera | sí | nada |
| Portada, meta description (en y es) | `static/index.html` | **a mano**, lista de familias | sí | `every_published_family_list_names_every_family` (`src/mcp.rs`, nuevo en 2.39.2) |
| Vista previa de enlaces (`og:image`) | las 10 páginas | **a mano**, todas `/logo.png` | sí | nada |
| Íconos de red y de token, exploradores | `static/x402.js` (`loadNetworks`, `hydrateNetworks`) | **generados** de `/networks.json` (2.41.0; `ICONO_DE_RED` e `ICONO_DE_TOKEN` se fueron) | sí | `static_types_no_explorer_and_no_icon` (`src/networks_json.rs`: ningún host de explorador ni `"/<ícono>.png"` en `static/`) y `the_static_surfaces_name_every_chain_by_its_published_caip2_id` (x402.js no nombra ninguna red) |
| Visor `/events/live`, enlaces a transacciones | `static/events-viewer.html` | **generado** de `/networks.json` (2.41.0; antes 14 exploradores a mano) | sí | `static_types_no_explorer_and_no_icon` |
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
| JSON de tokens | `config/supported_tokens.json` | **a mano**, 43 redes con Hedera; compilado desde 2.41.0 (es la presentación de `/networks.json`); también lo leen `scripts/arc_canary.py` y `scripts/scan/scan_evm.py` | sí | `the_json_lists_the_tokens_supported_publishes` y `the_json_describes_every_servable_network` (`src/networks_json.rs`) |
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
4. ~~`config/supported_tokens.json` no tiene Hedera.~~ Cerrado en 2.41.0, junto con BSC (listaba
   USDC, que no se sirve) y Sui (listaba AUSD).
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
