---
date: 2026-09-23
tags:
  - type/handoff
  - domain/networks
  - domain/celo
status: active
---

# celo-sepolia deja de anunciar la cadena de Alfajores: chain id 11142220, dominio `USDC`/`2`, y `eip155:44787` contesta 400 (2.39.1)

**Base:** `91d12f94` (2.39.0, #97). **Rama:** `c0der/celo-sepolia-chain-id`. **Publica
`2.39.1`**: patch, como pedía el encargo (task_bee37692a534). Es testnet.

## Lo medido (sólo lectura, 2026-09-23 07:03Z a 07:45Z)

| Qué | Resultado |
|---|---|
| `eth_chainId` en `forno.celo-sepolia.celo-testnet.org`, `celo-sepolia.drpc.org` y `rpc.ankr.com/celo_sepolia` (el que usa producción, `terraform/.../main.tf`) | `0xaa044c` = **11142220** en los tres |
| `alfajores-forno.celo-testnet.org` | NXDOMAIN (no resuelve) |
| `/supported` de producción, 2.39.0, 07:19Z | `celo-sepolia` (v1) y `eip155:44787` (v2), `networkAliases: ["celo-sepolia","eip155:44787"]`, USDC `0x01C5C0122039549AD1493B8220cABEdD739BC44E`, 6 decimales. 156 entradas en total |
| USDC `0x01C5…C44E` en 11142220 | 1.798 bytes de código; `name()` = `"USDC"`, `version()` = `"2"`, `symbol()` = `"USDC"`, `decimals()` = 6 |
| `DOMAIN_SEPARATOR()` del contrato | `0x23f4…9578`, igual a keccak(`"USDC"`, `"2"`, 11142220, dirección) calculado con `cast`. El dominio que usaba el facilitador (`"USD Coin"`, `"2"`, 44787) da `0x9a13…253d` |
| `eth_call` a `transferWithAuthorization` en 11142220, valor 0, llave aleatoria | firmada con (`USDC`, 2, 11142220): **pasa**. Con (`USD Coin`, 2, 44787), (`USDC`, 2, 44787) o (`USD Coin`, 2, 11142220): revierte con `FiatTokenV2: invalid signature` |
| ERC-8004 en 11142220 (forno y ankr) | identidad `0x8004A818…BD9e`, reputación `0x8004B663…8713` y validación `0x8004Cb1B…4272`: proxies de 130 bytes, implementación EIP-1967 igual a la de Base Sepolia, `getVersion()` = `2.0.0`. **Sin cambios** |
| Wallet testnet `0x3403…93A8` en Celo Sepolia | 49,99 CELO de gas |
| Explorer de la tarjeta del landing | `alfajores.celoscan.io/address/…` redirige (301) a la portada de `sepolia.celoscan.io` y pierde la dirección; `sepolia.celoscan.io/address/…` contesta 200 |
| Catálogo de discovery en producción | `GET /discovery/resources?network=eip155:44787` → `total: 0` (el filtro funciona: `eip155:8453` → 1377) |

### Quién manda `eip155:44787` hoy: nadie

- **SDKs** (sólo lectura): `origin/main` de `uvd-x402-sdk-python` y
  `uvd-x402-sdk-typescript`, y los paquetes publicados (npm `uvd-x402-sdk` 2.96.0,
  PyPI `uvd-x402-sdk` 0.88.0, bajados y revisados): **cero** apariciones de `44787`,
  `11142220` o `alfajores`. Los dos usan `celo-sepolia` sólo como nombre v1 de
  ERC-8004, que no cambia.
- **Resto del stack** (búsqueda en los checkouts locales): nadie lo manda al
  facilitador. Hay usos de pantalla o de tooling, listados abajo en "Para c0der".

## Qué rompía de verdad (más angosto de lo que decía el triage)

Leyendo `assert_valid_payment` (`src/chain/evm.rs`): fuera de Arc, una firma EOA **no**
se recupera localmente. Se juzga simulando `transferWithAuthorization` con
`eth_call`, y ahí manda el dominio del **contrato**, no el de la tabla estática. Así
que el dominio equivocado no rechazaba por sí solo una firma EOA correcta en v1. Lo que
sí estaba roto:

1. **Un cliente v2 que nombraba la cadena real** (`eip155:11142220`) recibía 400
   `Invalid CAIP-2 format`: no podía pagar.
2. **Un cliente v2 que confiaba en `/supported`** nombraba `eip155:44787` y, si sacaba
   el `chainId` de ahí, firmaba para 44787: la simulación revierte on-chain.
3. **EIP-6492** (smart wallets contrafactuales): el validador recibe el hash calculado
   con NUESTRO dominio, que no coincidía con el del contrato.
4. **DX402**: recupera la llave pública del pagador con nuestro dominio
   (`src/dx402/payer.rs`), así que habría recuperado otra llave. DX402 está apagado
   por defecto.

Que el camino v1 EOA con firma correcta pasaba en el código viejo **está leído en el
código, no medido contra producción**: medirlo exige un `/verify` a producción, que
escribe una fila en el índice de transacciones y un evento en `/events`, y el encargo
prohíbe escribir en producción. El test vivo de abajo sí muestra, contra la cadena
real, que el veredicto EOA sale de la simulación.

## Qué cambia

| Archivo | Cambio |
|---|---|
| `src/network.rs` | `to_caip2`/`from_caip2`: `eip155:11142220`. USDC de Celo Sepolia: `name` = `"USDC"`. Nueva tabla `RETIRED_NETWORK_IDS` (sólo `eip155:44787` → `CeloSepolia`), `retired_network_id()` y `RetiredNetworkId::explain()`; el deserializador v1/CAIP-2 usa ese mensaje |
| `src/chain/evm.rs` | `EvmChain` de `CeloSepolia`: 11142220 |
| `src/handlers.rs` | `retired_network_refusal()`: en `/verify` y `/settle`, sobre el body ya decodificado (también el de `PAYMENT-SIGNATURE`), antes de rutear esquemas y antes del cache de idempotencia. Lee todo campo `network` a cualquier profundidad. `/accepts`: `reason` sigue `network_unknown` (vocabulario cerrado), el `detail` nombra el reemplazo |
| `src/discovery_price.rs` | `alfajores`/`celo-alfajores` de un feed ya no se mapean a celo-sepolia: quedan en `eip155:44787` y se leen como no liquidables. Antes "acertaban" sólo porque las dos redes compartían 44787 por error |
| `src/openapi.rs` | documenta el código `network_retired` |
| `src/caip2.rs` | el test de chain ids dice 11142220 |
| `static/.well-known/x402`, `static/x402.js`, `static/index.html` | CAIP-2 y clave del mapa de íconos a 11142220; link del explorer a `sepolia.celoscan.io` |
| `config/supported_tokens.json` | `chainId` 11142220, explorer `sepolia.celoscan.io` |
| `.env.example` | `RPC_URL_CELO_SEPOLIA` = forno Celo Sepolia (el de Alfajores ya no resuelve) |
| `scripts/bench/run_bench.py` | el mock de `RPC_URL_CELO_SEPOLIA` contesta 11142220 |
| `tests/fixtures/frontend-supported.json` | el fixture de UI (espejo de producción) a 11142220 |
| `README.md`, `CLAUDE.md`, `.claude/agents/task-decomposition-expert.md` | 11142220 |
| `docs/handoffs/2026-09-17-x4-docs-arc-hedera.md` | nota fechada: "the chain id was always right" era falso. La fila vieja queda como estaba |
| `CHANGELOG.md`, `VERSION` | 2.39.1 |

### La respuesta a `eip155:44787`

```json
{
  "error": "`eip155:44787` is Celo Alfajores (chain id 44787), which this facilitator no longer serves: its public RPC no longer answers. `celo-sepolia` is `eip155:11142220`. Name it that way (or by the v1 name `celo-sepolia`) and sign the EIP-712 domain with chainId 11142220. GET /supported lists every network served, in both spellings",
  "code": "network_retired",
  "hint": "send `eip155:11142220` (or `celo-sepolia`) and sign for that chain id",
  "network": "eip155:44787",
  "replacement": "eip155:11142220"
}
```

## Decisión sobre `eip155:44787` (la más reversible)

Seguí la recomendación del encargo: **no queda como alias**. Un alias haría que un
request que nombra una cadena se liquide en otra, y además no funcionaría: la firma
hecha para 44787 no verifica en 11142220. Revertir es trivial: sacar la entrada de
`RETIRED_NETWORK_IDS` deja `eip155:44787` como red desconocida con el mensaje genérico;
volverlo alias sería una línea en `from_caip2` (no lo recomiendo).

Nadie del stack lo manda (medido arriba), así que el 400 no rompe a ningún consumidor
conocido. Cambio de comportamiento a sabiendas: antes un body **v1** con
`"network": "eip155:44787"` se aceptaba como celo-sepolia; ahora es 400.

## Verificación (checkout con LF)

`python3 .../c0der/master-5/scripts/preci.py --repo <worktree> --base origin/main`:
el diff dispara `ci.yaml` (job `Build & test`; `preflight`, `plan` y `deploy` necesitan
secretos de AWS) y `no-account-id.yml`. Corrido en local:

| Job | Comando | Resultado |
|---|---|---|
| Build & test | `python3 scripts/verify_landing_canonical.py --offline` | `[OK]` |
| Build & test | `node --test tests/frontend-capabilities.test.cjs` | 8/8 pass |
| Build & test | `python3 -m unittest discover -s tests/scripts -p 'test_*balances.py'` | 5 OK |
| Build & test | `cargo build --locked --features solana,near,stellar,algorand,sui,xrpl,hedera` | exit 0 |
| Build & test | `cargo test --locked -p x402-rs --features <las mismas> -- --test-threads=1` | exit 0: **2614 passed, 0 failed**, 25 ignored (11 suites). Sin los tests nuevos eran 2584/23: +30 = 15 tests x (lib y binario), +2 ignorados = el test vivo x 2 |
| Build & test | `cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance -- --test-threads=1` | exit 0: 109 passed, 0 failed, 9 ignored |
| No AWS account ID | las cuatro expresiones del workflow, en Python sobre los 908 archivos versionados | sin coincidencias |
| (encargo) | `cargo fmt --all -- --check` | 87 diffs, **los mismos 87 que `origin/main`**: ninguno en líneas de este PR |
| (encargo) | `cargo clippy --locked --features <las de CI> --all-targets` | exit 0, 0 errores. De 349 advertencias con ubicación, 2 caen en líneas de este PR: `await_holding_lock` en los dos tests nuevos de router, que usan el mismo `let _g = arm();` que los cuatro tests vecinos del módulo (que dan la misma advertencia). CI no corre clippy |
| (CLAUDE.md) | `cargo clippy -p x402-compliance` | exit 0, sin advertencias |

**Tests nuevos** (16; 15 corren en CI y uno es vivo e `#[ignore]`):

- `network::celo_sepolia_identity_tests` (4): chain id y CAIP-2 en los dos sentidos,
  el nombre v1 intacto; `eip155:44787` no resuelve y su mensaje nombra el reemplazo;
  un id retirado nunca tapa uno vivo; el USDC con `"USDC"`/`"2"`/6.
- `network::network_name_aliasing_tests::a_retired_identifier_is_refused_with_its_replacement`.
- `chain::evm::celo_sepolia_domain_tests` (4 + 1 vivo): el dominio que construimos da
  el `DOMAIN_SEPARATOR()` del contrato; **el vector**: la misma autorización firmada con
  (11142220, `"USDC"`) recupera al pagador bajo nuestro dominio y firmada con (44787,
  `"USD Coin"`) no; todo chain id EVM coincide con su CAIP-2.
- `facilitator_local::celo_sepolia_supported_diff_tests` (2): **`/supported` entero,
  antes y después**. El fixture `tests/fixtures/supported-before-celo-sepolia.json` es
  el `/supported` de producción 2.39.0 (07:19Z); el test lo vuelve a publicar con el
  código nuevo por `advertise_under_both_network_forms` y exige que la única diferencia
  sean las dos entradas de celo-sepolia (`44787` → `11142220`). El segundo compara la
  lista de tokens de las 26 redes EVM con `exact` contra `exact_payment_tokens`. En el
  fixture, los cuatro identificadores Sui de 32 bytes quedaron cortados a
  `0xabcd...wxyz`: el hook de pre-commit del repo rechaza todo `0x` + 64 hex, y viven
  en `extra`, que el test trata como opaco.
- `handlers` (3): `/verify` y `/settle` contestan `network_retired` en v1 y v2 y no
  dejan nada en el cache de idempotencia; `eip155:11142220` en v2 convierte a
  celo-sepolia y pasa el guard; `/accepts` nombra el reemplazo en `detail`.
- `handlers::agentic_surface_tests::the_static_surfaces_name_every_chain_by_its_published_caip2_id`:
  `/.well-known/x402` y el mapa de íconos de `/x402.js` coinciden con `to_caip2`.

**Test vivo, corrido a mano** (`celo_sepolia_live_verify_accepts_only_the_contracts_domain`,
sólo `eth_call`, nunca broadcast): el `verify` real del facilitador contra
`forno.celo-sepolia` y contra `rpc.ankr.com/celo_sepolia` → la firma (11142220,
`"USDC"`) sale `Valid`; la de (44787, `"USD Coin"`) revierte con `FiatTokenV2: invalid
signature`. Pasa en los dos.

**Mutaciones** (aplicadas sobre la rama y revertidas):

| Mutación | Resultado |
|---|---|
| volver a 44787 (`to_caip2`, `from_caip2`, `EvmChain`) y a `"USD Coin"` | fallan 13 de los 15 tests nuevos. Siguen verdes los dos que no dependen de eso: la consistencia chain id ↔ CAIP-2 (la tabla vieja era consistente y errónea en los dos lados) y la lista de tokens (no cambió) |
| desactivar `retired_network_refusal` | falla `a_request_naming_a_retired_chain_is_told_what_replaced_it`: el body v1 recibe `data did not match any variant of untagged enum VerifyRequestEnvelope`. El enum untagged se come el mensaje del deserializador; por eso el 400 útil lo da el guard |

## Suposiciones (reversibles)

1. **Patch (2.39.1)**, como pidió el encargo, aunque cambia una respuesta observable
   (el 400 de `eip155:44787`) y el CAIP-2 que publica `/supported` para celo-sepolia.
   Si otro PR sale antes con 2.39.1, este sube al siguiente.
2. **Documentos históricos sin tocar** (no se reescribe la historia): los análisis de
   v2 de 2025 (`docs/X402_V2_*.md`), `docs/UPSTREAM_MERGE_2025-11-06.md`,
   `docs/plans/SKALE_ERC8004_FIX_PLAN.md` y el reporte
   `docs/reports/onchain-scan/evm-testnets.json`. El escáner que genera ese reporte lee
   el chain id de `config/supported_tokens.json`, así que la próxima corrida dirá 11142220.
3. **Sin sonda de chain id al arrancar** para celo-sepolia. Arc la tiene, y habría
   atrapado esto; pero hace fallar el arranque entero si el RPC no contesta (el cache de
   providers es fail-fast), y agregarla a una testnet es riesgo de disponibilidad en
   producción. Ver "Visto de paso".

## Para c0der

**Sonda de cierre, después del deploy** (sólo lectura; el `/verify` a `eip155:44787`
contesta 400 en el guard, antes de registrar nada en el índice o en `/events`):

```bash
F=https://facilitator.ultravioletadao.xyz
curl -s $F/version                                   # {"version":"2.39.1"}
curl -s $F/supported | jq -c '[.kinds[] | select(.networkAliases|index("celo-sepolia")) | {network, networkAliases}]'
# esperado: [{"network":"celo-sepolia","networkAliases":["celo-sepolia","eip155:11142220"]},
#            {"network":"eip155:11142220","networkAliases":["celo-sepolia","eip155:11142220"]}]
curl -s $F/supported | grep -c 44787                 # 0
curl -s $F/supported | jq '.kinds | length'          # 156, igual que antes
curl -s $F/.well-known/x402 | jq -c '.x402.testnets[] | select(.name=="celo-sepolia")'
# esperado: {"name":"celo-sepolia","caip2":"eip155:11142220",...}
curl -s -X POST $F/verify -H 'content-type: application/json' \
  -d '{"x402Version":1,"paymentPayload":{"network":"eip155:44787"},"paymentRequirements":{"network":"eip155:44787"}}' \
  -w '\nHTTP %{http_code}\n'
# esperado: HTTP 400, "code":"network_retired", "replacement":"eip155:11142220"
```

**SDKs: no hay nada que despachar.** Ninguno manda 44787, y `celo-sepolia` (v1) no cambia.

**Fuera del facilitador, usos de pantalla o tooling con 44787** (ninguno le manda
requests; despacharlos o no es decisión tuya):

- `uvdweb`: `src/pages/FacilitatorPage.js:84` muestra `chainId: 44787` para Celo Sepolia
  (y `src/data/ecosystem/replays/facilitator_supported.json`, una captura vieja de
  `/supported`). Arreglo de una línea en la página.
- `c0der`: `docs/reports/rediseno/entregables/iconos/x402-iconos.js:20` y el informe de
  iconografía, copias del mapa de `static/x402.js` de 2026-09-03.
- `execution-market`: `contracts/hardhat.config.ts:149` y `:299` configuran Alfajores
  (44787) como red de hardhat; esa red ya no responde.
- `karmakadabra`: `scripts/check_all_balances.py:62` consulta saldos en 44787.

## Visto de paso (no es de este PR)

- **`MixedAddress` no lee todo lo que escribe.** Un token XRPL se publica en
  `/supported` como `currency.issuer` (`5553…0000.rGm7…`), y ese string no vuelve a
  deserializar como `MixedAddress` (`Invalid address format`). Un cliente en Rust que
  parsee `/supported` con los tipos de este crate fallaría. Lo encontré al armar el
  fixture; el test lo esquiva sin arreglarlo.
- **Una sonda de chain id no fatal para todas las redes EVM** (warn + métrica al
  arrancar, comparando `eth_chainId` con `EvmChain`) habría visto esto el primer día sin
  arriesgar el arranque. La de Arc es fatal a propósito.
