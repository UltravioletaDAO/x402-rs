---
date: 2026-09-23
tags:
  - type/handoff
  - domain/networks
  - domain/celo
  - domain/hyperevm
status: active
---

# Dos testnets con un chain id ajeno: celo-sepolia (11142220) y hyperevm-testnet (998), dominio `USDC`/`2`, y `eip155:44787` contesta 400 (2.39.1)

**Base:** `91d12f94` (2.39.0, #97). **Rama:** `c0der/celo-sepolia-chain-id`, PR #98.
**Publica `2.39.1`**: patch, como pidió el encargo (task_bee37692a534). Son testnets.

Dos commits en el mismo PR. `0b57f768` es celo-sepolia, que el refutador dio
MERGEABLE. El segundo es la ronda 2 (`RONDA2-c0der.md`): hyperevm-testnet, con el
mismo defecto encontrado por el refutador, y sus tres P3 de comentarios. Van en un
solo release.

## Lo medido (sólo lectura, 2026-09-23 07:03Z a 08:30Z)

### celo-sepolia

| Qué | Resultado |
|---|---|
| `eth_chainId` en `forno.celo-sepolia.celo-testnet.org`, `celo-sepolia.drpc.org` y `rpc.ankr.com/celo_sepolia` (el que usa producción, `terraform/.../main.tf`) | `0xaa044c` = **11142220** en los tres |
| `alfajores-forno.celo-testnet.org` (el de `.env.example`) | NXDOMAIN |
| USDC `0x01C5…C44E` en 11142220 | 1.798 bytes de código; `name()` = `"USDC"`, `version()` = `"2"`, `decimals()` = 6 |
| `DOMAIN_SEPARATOR()` | `0x23f4…9578` = keccak(`"USDC"`, `"2"`, 11142220, dirección), calculado con `cast`. El dominio que se usaba (`"USD Coin"`, 44787) da `0x9a13…253d` |
| `eth_call` `transferWithAuthorization`, valor 0, llave aleatoria | (`USDC`, 11142220) **pasa**. (`USD Coin`, 44787), (`USDC`, 44787) y (`USD Coin`, 11142220) revierten con `FiatTokenV2: invalid signature` |
| ERC-8004 en 11142220 (forno y ankr) | los tres registros: proxies de 130 bytes, implementación de Base Sepolia, `getVersion()` = `2.0.0`. **Sin cambios** |
| chainid.network | 44787 = "Celo Alfajores Testnet", `status: deprecated`; 11142220 = "Celo Sepolia Testnet" |

### hyperevm-testnet (ronda 2)

| Qué | Resultado |
|---|---|
| `eth_chainId` en `rpc.hyperliquid-testnet.xyz/evm` (producción) y `hyperliquid-testnet.drpc.org` | `0x3e6` = **998** en los dos |
| `testnet.rpc.hyperevm.com` (el de `.env.example`) | NXDOMAIN |
| USDC `0x2B33…D8Ab` en 998 | 1.798 bytes; `name()` = `"USDC"`, `version()` = `"2"`, `decimals()` = 6 |
| `DOMAIN_SEPARATOR()` | `0xf26c…465e` = keccak(`"USDC"`, `"2"`, 998, dirección). El dominio viejo (`"USD Coin"`, 333) da `0xf0e7…ea09` |
| HyperEVM mainnet (`rpc.hyperliquid.xyz/evm`) | `0x3e7` = 999, sin cambios |
| **¿333 es otra cadena?** | Sí: chainid.network la asigna a **EthStorage Mainnet** (`es-m`, `status: incubating`). Su RPC registrado (`rpc.mainnet.ethstorage.io:9540`) resuelve pero no contestó (timeout de 20 s en `eth_chainId` y en `eth_blockNumber`) |
| Wallet testnet `0x3403…93A8` | 49,99 CELO en Celo Sepolia y 0,24 HYPE en HyperEVM testnet |

### Quién manda los ids viejos: nadie

- **SDKs**: `origin/main` de `uvd-x402-sdk-python` (`9944666`) y de
  `uvd-x402-sdk-typescript` (`b079792`), más los paquetes publicados (npm
  `uvd-x402-sdk` 2.96.0 y 2.97.0, PyPI 0.88.0). **Cero** apariciones de `44787`,
  `eip155:333`, `hyperevm-testnet` o un chain id 333/998. `celo-sepolia` sólo aparece
  como nombre v1 de ERC-8004, que no cambia.
- **Catálogo de discovery**: `total: 0` en `eip155:44787`, `eip155:11142220`,
  `eip155:333` y `eip155:998`. El filtro funciona: `eip155:8453` da 1377.
- **Logs de producción** (CloudWatch `/ecs/facilitator-production`, 14 días): **cero** líneas externas con `eip155:333` o `eip155:998`. Las únicas 2 + 2 son mis propias consultas de discovery de las 08:09:21Z (`GET /discovery/resources?network=…`).
  Para 44787, el refutador contó 0 líneas en 14 días.
- **Resto del stack**: ningún repo manda estos ids al facilitador. Lo que aparece son
  pantallas, capturas de `/supported` o scripts; la lista está en "Para c0der".

## Qué rompía de verdad

Fuera de Arc, una firma EOA **no** se recupera localmente (`assert_valid_payment`,
`src/chain/evm.rs`). Se juzga simulando `transferWithAuthorization` con `eth_call`,
y ahí decide el dominio del **contrato**, no el de la tabla estática. El refutador lo
midió injertando el test vivo en `main`: una EOA v1 firmada con el dominio correcto
ya salía `Valid`. Lo que sí estaba roto, en las dos redes:

1. **Un cliente v2 que nombraba la cadena real** (`eip155:11142220` o `eip155:998`)
   recibía 400 `Invalid CAIP-2 format`: no podía pagar.
2. **Un cliente v2 que confiaba en `/supported`** nombraba el id viejo y, si sacaba el
   `chainId` de ahí, firmaba para esa cadena: la simulación revierte on-chain.
3. **EIP-6492** (smart wallets contrafactuales): el validador recibe el hash calculado
   con NUESTRO dominio, que no coincidía con el del contrato.
4. **DX402** recupera la llave pública del pagador con nuestro dominio
   (`src/dx402/payer.rs`), así que habría recuperado otra llave. DX402 está apagado
   por defecto.

## Qué cambia

| Archivo | Cambio |
|---|---|
| `src/network.rs` | `to_caip2`/`from_caip2`: `eip155:11142220` y `eip155:998`. USDC de las dos redes: `name` = `"USDC"`. Tabla `RETIRED_NETWORK_IDS` (sólo `eip155:44787` → `CeloSepolia`), `retired_network_id()` y `RetiredNetworkId::explain()`; el deserializador v1/CAIP-2 usa ese mensaje. Un párrafo explica por qué `eip155:333` no está |
| `src/chain/evm.rs` | `EvmChain`: 11142220 y 998. El doc de `find_known_eip712_metadata` decía que HyperEVM testnet es `"USD Coin"`; ahora el ejemplo del cambio de nombre entre mainnet y testnet es Base |
| `src/handlers.rs` | `retired_network_refusal()`: en `/verify` y `/settle`, sobre el body ya decodificado (también el de `PAYMENT-SIGNATURE`), antes de rutear esquemas y antes del cache de idempotencia. Lee todo campo `network` a cualquier profundidad, y sólo parsea si el texto contiene un id retirado. `/accepts`: `reason` sigue `network_unknown` (vocabulario cerrado) y el `detail` nombra el reemplazo |
| `src/discovery_price.rs` | un feed que dice `alfajores`/`celo-alfajores` queda en `eip155:44787` (no liquidable). Antes se reasignaba a celo-sepolia |
| `src/openapi.rs` | documenta `network_retired` y por qué `eip155:333` no lo recibe |
| `src/caip2.rs` | el test de chain ids dice 11142220 y 998 |
| `static/.well-known/x402`, `static/x402.js` | CAIP-2 y claves del mapa de íconos: 11142220 y 998 |
| `static/index.html` | explorer de la tarjeta Celo Sepolia: `sepolia.celoscan.io` (`alfajores.celoscan.io/address/…` redirige a la portada y pierde la dirección) |
| `static/skill.md`, `static/llms-full.txt` (regenerado), `static/.well-known/agent-skills/index.json` (digest) | decían que HyperEVM testnet es `"USD Coin"`; ahora el ejemplo es Base (`"USD Coin"` en mainnet, `"USDC"` en Sepolia) |
| `config/supported_tokens.json` | `chainId` 11142220 y 998; explorer `sepolia.celoscan.io` |
| `.env.example` | los dos RPC de testnet que ya no resolvían, reemplazados por los medidos |
| `scripts/bench/run_bench.py` | los mocks contestan 11142220 y 998 |
| `tests/fixtures/frontend-supported.json` | el fixture de UI (espejo de producción): 11142220 y 998 |
| `README.md`, `CLAUDE.md`, `docs/CUSTOMIZATIONS.md`, `.claude/agents/task-decomposition-expert.md` | 11142220 y 998. El boceto de `CUSTOMIZATIONS.md` también tenía mal el de HyperEVM mainnet (998 en vez de 999) |
| `docs/handoffs/2026-09-17-x4-docs-arc-hedera.md` | nota fechada: "the chain id was always right" era falso. La fila vieja queda como estaba |
| `CHANGELOG.md`, `VERSION` | 2.39.1, con las dos redes |

### La respuesta a `eip155:44787` (P3-c: redacción neutra)

```json
{
  "error": "`eip155:44787` is not served; `celo-sepolia` is `eip155:11142220`. Through 2.39.0 `/supported` listed `celo-sepolia` under it by mistake; 44787 is the chain id of Celo Alfajores. Name it `eip155:11142220` (or by the v1 name `celo-sepolia`) and sign the EIP-712 domain with chainId 11142220. GET /supported lists every network served, in both spellings",
  "code": "network_retired",
  "hint": "send `eip155:11142220` (or `celo-sepolia`) and sign for that chain id",
  "network": "eip155:44787",
  "replacement": "eip155:11142220"
}
```

### Los otros dos P3

- **(a)** El comentario de `USDC_CELO_SEPOLIA` decía que ninguna firma correcta
  verificaba nunca. Ahora dice lo medido: una EOA sí verificaba, porque la simulación
  usa el dominio del contrato; las que no podían eran EIP-6492 y la recuperación de
  llave de DX402.
- **(b)** Los dos bloques nuevos que habían quedado bajo el `///` de otro ítem quedan
  cada uno con el suyo. `proof_of_payment_tests` recupera su doc, y
  `an_unknown_name_is_still_an_error` también.

## Decisiones sobre los ids viejos

- **`eip155:44787`: 400 `network_retired`, no alias.** Es lo que recomendaba el
  encargo. Un alias haría que un request que nombra una cadena se liquide en otra, y
  además no funcionaría: la firma hecha para 44787 no verifica en 11142220. Para
  revertir, se saca la entrada de `RETIRED_NETWORK_IDS`.
- **`eip155:333`: sin 400 propio, queda como red desconocida.** La regla del encargo
  era "sólo si 333 no es otra cadena viva; si hay duda, no". 333 está asignada a
  EthStorage Mainnet. No contestó, pero figura como `incubating`: puede lanzar, y
  un 400 que mandara a sus usuarios a HyperEVM sería un consejo equivocado. Hoy un
  request con `eip155:333` recibe el 400 genérico de cadena desconocida, sin mencionar
  HyperEVM; los tests lo fijan. Si se prefiere el puntero, es una entrada más en
  `RETIRED_NETWORK_IDS`.
- Cambio de comportamiento a sabiendas: antes un body **v1** con `"network":
  "eip155:44787"` o `"eip155:333"` se aceptaba como la testnet; ahora es 400.

## Verificación (checkout con LF)

`python3 .../c0der/master-5/scripts/preci.py --repo <worktree> --base origin/main`:
el diff dispara `ci.yaml` (job `Build & test`; `preflight`, `plan` y `deploy` necesitan
secretos de AWS) y `no-account-id.yml`. Corrido en local sobre el código final:

| Job | Comando | Resultado |
|---|---|---|
| Build & test | `python3 scripts/verify_landing_canonical.py --offline` | `[OK]` |
| Build & test | `node --test tests/frontend-capabilities.test.cjs` | 8/8 pass |
| Build & test | `python3 -m unittest discover -s tests/scripts -p 'test_*balances.py'` | 5 OK |
| Build & test | `cargo build --locked --features solana,near,stellar,algorand,sui,xrpl,hedera` | exit 0 |
| Build & test | `cargo test --locked -p x402-rs --features <las mismas> -- --test-threads=1` | exit 0: **2622 passed, 0 failed**, 25 ignored (11 suites). Son +8 sobre las 2614 del primer commit: 4 tests nuevos x (lib y binario) |
| Build & test | `cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance -- --test-threads=1` | exit 0: 109 passed, 0 failed, 9 ignored |
| No AWS account ID | las cuatro expresiones del workflow, en Python sobre los archivos versionados | sin coincidencias (909 archivos, 405 docs) |
| (encargo) | `cargo fmt --all -- --check` | 87 diffs, **los mismos 87 que `origin/main`**: ninguno en líneas de este PR |
| (encargo) | `cargo clippy --locked --features <las de CI> --all-targets` | exit 0, 0 errores. De 350 advertencias con ubicación, 3 caen en líneas de este PR: `await_holding_lock` en los tres tests nuevos de router, que usan el mismo `let _g = arm();` que los tests vecinos del módulo (que dan la misma advertencia). CI no corre clippy |
| (CLAUDE.md) | `cargo clippy --locked -p x402-compliance` | exit 0, sin advertencias |

Un test falló en la primera corrida de la ronda 2 y se arregló:
`the_skills_index_digest_matches_skill_md`. Editar `skill.md` cambia su sha256, que
`/.well-known/agent-skills/index.json` publica. El digest nuevo es el que pedía el test.

**Tests nuevos** (19 corren en CI y 1 es vivo con `#[ignore]`):

- `network::celo_sepolia_identity_tests` (4) y `network::hyperevm_testnet_identity_tests` (3):
  - chain id y CAIP-2 en los dos sentidos, con el nombre v1 intacto;
  - `eip155:44787` no resuelve y su mensaje nombra el reemplazo;
  - `eip155:333` no resuelve y su mensaje **no** menciona HyperEVM;
  - un id retirado nunca tapa uno vivo;
  - los dos USDC con `"USDC"`/`"2"`/6.
- `network::network_name_aliasing_tests::a_retired_identifier_is_refused_with_its_replacement`.
- `chain::evm::mislabeled_testnet_domain_tests` (4 + 1 vivo), una tabla con las dos redes:
  - el dominio que construimos da el `DOMAIN_SEPARATOR()` de cada contrato;
  - **el vector**: la misma autorización firmada con el dominio real (`"USDC"`, chain id
    real) recupera al pagador bajo nuestro dominio; firmada con el enviado hasta 2.39.0
    (`"USD Coin"`, chain id ajeno), no;
  - todo chain id EVM coincide con su CAIP-2.
- `facilitator_local::testnet_chain_id_supported_diff_tests` (2), **`/supported` entero
  antes y después**:
  - El fixture `tests/fixtures/supported-before-celo-sepolia.json` es el `/supported`
    de producción 2.39.0 (07:19Z). El test lo vuelve a publicar con el código nuevo por
    `advertise_under_both_network_forms` y exige que las únicas diferencias sean las
    cuatro entradas de las dos redes.
  - El segundo test compara la lista de tokens de las 26 redes EVM con `exact` contra
    `exact_payment_tokens`.
  - En el fixture, los cuatro identificadores Sui de 32 bytes están cortados a
    `0xabcd...wxyz`: el hook de pre-commit rechaza todo `0x` + 64 hex, y esos valores
    viven en `extra`, que el test trata como opaco.
- `handlers` (4):
  - `/verify` y `/settle` contestan `network_retired` en v1 y v2 y no dejan nada en el
    cache de idempotencia;
  - `eip155:11142220` y `eip155:998` en v2 convierten a su red y pasan el guard;
  - `eip155:333` recibe un 400 que no es `network_retired` ni nombra HyperEVM;
  - `/accepts` nombra el reemplazo en `detail`.
- `handlers::agentic_surface_tests::the_static_surfaces_name_every_chain_by_its_published_caip2_id`:
  `/.well-known/x402` y el mapa de íconos de `/x402.js` coinciden con `to_caip2`.

**Test vivo, corrido a mano** (`live_verify_accepts_only_the_contracts_domain`, sólo
`eth_call`, nunca broadcast). Es el `verify` real del facilitador contra las dos
cadenas, con los RPC por defecto y con los de producción (`rpc.ankr.com/celo_sepolia`
y `rpc.hyperliquid-testnet.xyz/evm`). En las dos redes y con los dos juegos de RPC, la
firma con el dominio real sale `Valid` y la del dominio viejo revierte con
`FiatTokenV2: invalid signature`.

**Mutaciones** (aplicadas sobre la rama y revertidas):

| Mutación | Resultado |
|---|---|
| celo-sepolia de vuelta a 44787 y `"USD Coin"` | fallan 13 de los 15 tests de la ronda 1. Siguen verdes los dos que no dependen de eso: la consistencia chain id ↔ CAIP-2 (la tabla vieja era consistente y errónea en los dos lados) y la lista de tokens |
| hyperevm-testnet de vuelta a 333 y `"USD Coin"` | fallan 9 de 11. Siguen verdes los mismos dos |
| desactivar `retired_network_refusal` | falla `a_request_naming_a_retired_chain_is_told_what_replaced_it`: el body v1 recibe `data did not match any variant of untagged enum VerifyRequestEnvelope`. El enum untagged se come el mensaje del deserializador; por eso el 400 útil lo da el guard |

## Suposiciones (reversibles)

1. **Patch (2.39.1)**, como pidió el encargo, aunque cambian respuestas observables:
   el 400 de `eip155:44787`, que `eip155:333` ya no se acepte, y los CAIP-2 de dos
   redes en `/supported`. Si otro PR sale antes con 2.39.1, este sube al siguiente.
2. **Documentos históricos sin tocar** (no se reescribe la historia): los análisis de
   v2 de 2025 (`docs/X402_V2_*.md`), `docs/UPSTREAM_MERGE_2025-11-06.md`,
   `docs/plans/SKALE_ERC8004_FIX_PLAN.md` y el reporte
   `docs/reports/onchain-scan/evm-testnets.json`. El escáner que genera ese reporte lee
   el chain id de `config/supported_tokens.json`, así que la próxima corrida dirá bien.
3. **Sin sonda de chain id al arrancar.** Queda en el backlog de c0der, como pidió la
   ronda 2.
4. **El explorer de HyperEVM testnet** (`testnet.purrsec.com`) no se tocó: es de la
   cadena correcta. Su página de dirección contestó 404 a `curl`, quizá por ser una
   SPA; no lo verifiqué en un navegador.

## Para c0der

**Sonda de cierre, después del deploy.** Es sólo lectura: el `/verify` a
`eip155:44787` contesta 400 en el guard, antes de registrar nada en el índice o en
`/events`.

```bash
F=https://facilitator.ultravioletadao.xyz
curl -s $F/version                                   # {"version":"2.39.1"}
curl -s $F/supported | jq -c '[.kinds[] | select(.networkAliases|index("celo-sepolia") or index("hyperevm-testnet")) | {network, networkAliases}]'
# esperado (4 entradas):
#  {"network":"celo-sepolia","networkAliases":["celo-sepolia","eip155:11142220"]}
#  {"network":"eip155:11142220","networkAliases":["celo-sepolia","eip155:11142220"]}
#  {"network":"hyperevm-testnet","networkAliases":["hyperevm-testnet","eip155:998"]}
#  {"network":"eip155:998","networkAliases":["hyperevm-testnet","eip155:998"]}
curl -s $F/supported | grep -c -e 44787 -e '"eip155:333"'   # 0
curl -s $F/supported | jq '.kinds | length'          # 156, igual que antes
curl -s $F/.well-known/x402 | jq -c '.x402.testnets[] | select(.name=="celo-sepolia" or .name=="hyperevm-testnet") | {name, caip2}'
# esperado: {"name":"celo-sepolia","caip2":"eip155:11142220"} y {"name":"hyperevm-testnet","caip2":"eip155:998"}
curl -s -X POST $F/verify -H 'content-type: application/json' \
  -d '{"x402Version":1,"paymentPayload":{"network":"eip155:44787"},"paymentRequirements":{"network":"eip155:44787"}}' \
  -w '\nHTTP %{http_code}\n'
# esperado: HTTP 400, "code":"network_retired", "replacement":"eip155:11142220"
```

**SDKs: no hay nada que despachar.** Ninguno manda los ids viejos, y los nombres v1 no
cambian.

**Fuera del facilitador, usos de pantalla, capturas o tooling** (ninguno le manda
requests; despacharlos o no es decisión tuya):

- `uvdweb`:
  - `src/pages/FacilitatorPage.js:84` muestra `chainId: 44787` para Celo Sepolia. Su
    `docs/planning/BACKLOG.md:31` ya tiene una fila sobre chain ids errados en esa página.
  - `src/data/ecosystem/replays/facilitator_supported.json` es una captura de
    `/supported` con 44787 y 333.
- `c0der`:
  - `docs/reports/rediseno/entregables/iconos/x402-iconos.js:20` es una copia del mapa
    de íconos.
  - `docs/reports/2026-09-16-hedera-*.json` son capturas de `/supported` con 333.
- `faro`: `backend/src/api/agent_card.rs:141`, una tarjeta "Celo Alfajores" (según el
  refutador).
- `execution-market`: `contracts/hardhat.config.ts` configura Alfajores como red de
  hardhat.
- `karmakadabra`: `scripts/check_all_balances.py:62` consulta saldos en 44787.

## Visto de paso (no es de este PR)

- **`MixedAddress` no lee todo lo que escribe.** Un token XRPL se publica en
  `/supported` como `currency.issuer` (`5553…0000.rGm7…`), y ese string no vuelve a
  deserializar como `MixedAddress` (`Invalid address format`). Un cliente en Rust que
  parsee `/supported` con los tipos de este crate fallaría.
- **En esta Mac `git grep -E` no entiende `\b`.** Un primer barrido del stack buscando
  333 con `\b` dio vacío en falso; repetido sin `\b`, aparecieron las capturas listadas
  arriba.
