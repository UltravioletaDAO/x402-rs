# `GET /networks.json` y el drift de `config/supported_tokens.json` (DUP-07) — 2.41.0

**Rama** `0xultravioleta/x4-networks-json`, base `origin/main` `8ee44114` (2.40.0). Commits
locales, **sin push** (regla de la tanda del facilitador). Encargo de c0der del 2026-09-24,
más el agregado del RPC de Arc (commit aparte, sección final).

| Commit | Qué |
|---|---|
| `4551a79b` | `GET /networks.json`, JSON igual a lo servido, `static/` sin exploradores ni íconos (2.41.0) |
| `2deb2c3e` | Arc mainnet: `RPC_URL_ARC` del secreto `facilitator-rpc-mainnet:arc` |
| este | este handoff |

## Qué quedó

1. **`GET /networks.json`** (`src/networks_json.rs`, ruta al lado de `/supported` en
   `handlers::routes`; `/networks` sigue siendo la página HTML). Una fila por red servida:
   `id`, `caip2`, `family`, `chainId|null`, `testnet`, `displayName`,
   `explorer {base, tx, address}` (plantillas con `{tx}` / `{address}`), `icon` (URL absoluta
   de un PNG que el facilitador sirve), `schemes[]`,
   `tokens[{symbol, address, decimals, eip712|null, usdPegged, icon|null}]`.
   `Cache-Control: public, max-age=300`. **`/supported` no cambia** (su cuerpo sale ahora de
   `supported_body()`, la misma función que usa `/networks.json`: byte a byte igual).
2. **De dónde sale cada campo** (nada tipeado en el handler):
   - Las **filas** son el cuerpo de `/supported` agrupado por cadena: una red tiene fila si y
     solo si `/supported` la nombra, con los mismos dos identificadores. Hedera no tiene nombre
     v1, así que su `id` es `hedera:mainnet` (igual que en `/supported`).
   - `schemes`, y `address`/`decimals` de cada token: de esas mismas entradas.
   - `family`, `chainId`, `testnet`: del enum `Network`. `eip712`: de
     `find_known_eip712_metadata`, la tabla con la que `/verify` resuelve el dominio.
   - Solo la presentación (`displayName`, ícono, explorador, `usdPegged`, ícono de token) sale
     de `config/supported_tokens.json`, que ahora se compila (`include_str!`).
   - Una red servida sin presentación **no desaparece**: sale con `explorer`/`icon` null (los
     tests lo ponen en rojo antes de que llegue a producción).
   - `icon` usa `FACILITATOR_URL` (la variable que ya lee `discovery_attestation`) o, sin ella,
     `https://facilitator.ultravioletadao.xyz`. Las páginas propias usan solo la ruta.
3. **`config/supported_tokens.json` coincide con lo servido** (y lo dice donde difiere):
   BSC `[ausd]` (su USDC no tiene ERC-3009, nunca se sirvió), Sui `[usdc]`, XRPL testnet
   `[xrp, rlusd, usdc]` (el JSON decía solo `xrp`), Ethereum Sepolia `[]` con
   `exactServed: false` y `_note` (producción no configura `RPC_URL_ETHEREUM_SEPOLIA`; ahí sirve
   `fhe-transfer`, `escrow` y `commerce`), y **Hedera mainnet/testnet agregados** (grupo
   `hedera_networks`, claves CAIP-2; fee payer = el `feePayer` que publica `/supported`).
   Cada red suma `displayName`, `icon`, `explorerPaths`; cada token `usdPegged` e `icon`.
   La forma del archivo (grupos por familia, claves v1, `facilitatorWallet`, `chainId`) no
   cambió: la siguen leyendo `scripts/arc_canary.py` y `scripts/scan/scan_evm.py`.
4. **Cero exploradores ni íconos tipeados en `static/`**:
   - `static/x402.js`: se fueron `ICONO_DE_RED` e `ICONO_DE_TOKEN`; entran `loadNetworks()`,
     `networkIcon()`, `tokenIcon()`, `explorerUrl()` e `hydrateNetworks()`. Cargado como
     `?v=20260924` en la portada y en `/networks`.
   - `static/index.html`: las 43 tarjetas (más Zama, que no es explorador) solo NOMBRAN su red
     (`data-explorer`, `data-explorer-address`, `<img data-net-icon>`); lo mismo los enlaces de
     wallets, los del contrato de `upto` y los de escrow en Base, las listas de íconos de
     ERC-8004 y escrow, y los íconos de stablecoins. Sui y Hedera toman la dirección del
     `feePayer` de `/supported` (`data-explorer-fee-payer`). La tarjeta de Solana Devnet vuelve
     a abrir su explorador: su `onclick` tenía un error de sintaxis (faltaba el `)`).
   - `static/networks.html`: íconos y familia desde `/networks.json` (se fue la regex que
     mandaba a EVM lo que no reconocía); la tabla de wallets sale del `feePayer` de
     `/supported` para toda red que lo publica; quedan tipeadas solo EVM (DeBank, que no es
     el explorador de una red) y XRPL (en modo relay no publica fee payer).
   - `static/events-viewer.html`: plantillas `tx` de `/networks.json` (antes 14 exploradores a
     mano; ahora enlaza toda red descrita). Hedera: `/events` la nombra `hedera`, y su id de
     transacción `0.0.X@S.N` se escribe `0.0.X-S-N` en la URL, como antes.
   - Si `/networks.json` no responde: monograma y dirección en texto, nunca un enlace adivinado.
5. **Dos exploradores muertos reemplazados** (verificados el 2026-09-24, tabla abajo):
   HyperEVM testnet `testnet.purrsec.com` (404 hasta en `/`) → `explore-testnet.hyperpc.app`
   (Blockscout de terceros titulado "Hyperliquid EVM Testnet"; chainlist no lista ninguno para
   998), y Algorand testnet `testnet.allo.info` (no resuelve) → `testnet.explorer.perawallet.app`
   (el que la portada ya usaba).
6. Docs: `CHANGELOG.md` (2.41.0), `VERSION` 2.41.0, OpenAPI (`path_networks_json` + test),
   `CLAUDE.md`, `docs/networks/superficies.md` (filas que pasaron a «generada»; drift #4
   cerrado), `guides/ADDING_NEW_CHAINS.md` y `.claude/skills/add-network/SKILL.md` (la fila 9
   «`ICONO_DE_RED`» ya no existe; la 10 es la entrada del JSON, ahora con test).
   `scripts/verify_landing_canonical.py` busca la tarjeta de Hedera por su atributo nuevo.

## Tests (rojo si algo de esto vuelve)

`src/networks_json.rs`:

| Test | Qué pone en rojo |
|---|---|
| `the_rows_are_the_networks_supported_names` | `set(id ∪ caip2)` ≠ `set(network)` sobre la captura real de `/supported` 2.40.0 (`tests/fixtures/supported-2.40.0.json`, Sui recortado por el hook); 43 filas, 23 mainnet |
| `networks_json_names_what_supported_names` | lo mismo por el router: `GET /networks.json` vs `GET /supported` con los handlers reales, cada red del enum servida; cache-control; una fila sin explorer o icon |
| `every_row_has_a_name_an_icon_and_an_explorer` | una fila sin `displayName`, `icon` o `explorer` con sus dos plantillas |
| `bsc_sui_and_hedera_read_as_served` | BSC con USDC, Sui con AUSD, Hedera ausente; dominios EIP-712 (Base `USD Coin`, Base Sepolia `USDC`, AUSD `Agora Dollar`), `usdPegged` de EURC y XRP, XRP sin ícono |
| `a_served_network_without_metadata_keeps_its_row` | que una red sin presentación desaparezca en vez de salir con null |
| `the_json_lists_the_tokens_supported_publishes` | el JSON lista un token que `/supported` no publica para esa red, o le falta uno (mismas funciones que cada proveedor: `exact_payment_tokens`, `xrpl::payment_tokens`, `hedera::payment_tokens`) |
| `the_json_describes_every_servable_network` | una variante de `Network::variants()` sin entrada, una entrada que no es red, o un `chainId`/`caip2` distinto del código |
| `every_served_token_is_described` | un token servido sin `token_info`, o `usdPegged` que contradice `currency_symbol()` |
| `every_icon_is_served` | un ícono del JSON que el router no sirve como `image/png` (`handlers::image_routes()`) |
| `static_types_no_explorer_and_no_icon` | un host de explorador del JSON, un `"/<ícono>.png"` o `ICONO_DE_*`/`const EXPLORER` en cualquier archivo de `static/` |

Más: `openapi::tests::the_networks_document_is_documented`, el test de superficies de
`handlers.rs` (x402.js no nombra ninguna red) y 4 tests nuevos en
`tests/frontend-capabilities.test.cjs` (plantillas, `hydrateNetworks`, atributos de la portada
y de `/networks`, tamaño de logo leyendo el ícono de `/networks.json`).

**Mutaciones mínimas, todas en rojo** (script en el scratchpad, cada una revertida):

| Mutación | Tests en rojo |
|---|---|
| BSC vuelve a listar `usdc` | `the_json_lists_the_tokens_supported_publishes` |
| Sui vuelve a listar `ausd` | `the_json_lists_the_tokens_supported_publishes` |
| `hedera:mainnet` sale del JSON | 5 (`bsc_sui_and_hedera…`, `every_row…`, `networks_json_names…`, `the_json_describes…`, `the_json_lists…`) |
| Base pierde `explorerPaths` | `every_row…`, `networks_json_names…` |
| Base pierde `icon` | `every_row…`, `networks_json_names…` |
| `document()` descarta Hedera | 4 (`the_rows_are…`, `networks_json_names…`, `a_served_network…`, `bsc_sui_and_hedera…`) |
| un `href="https://basescan.org/…"` vuelve a `index.html` | `static_types_no_explorer_and_no_icon` |

## Pre-CI local

`df -h /System/Volumes/Data`: 92 GiB libres al empezar (79 GiB al final). CI dispara
`ci.yaml` (job `test`) y `no-account-id.yml`.

| Comando | Resultado |
|---|---|
| `python3 scripts/verify_landing_canonical.py --offline` | OK |
| `node --test tests/frontend-capabilities.test.cjs` | 17/17 |
| `python3 -m unittest discover -s tests/scripts -p 'test_*balances.py'` | OK |
| `cargo build --locked --features solana,near,stellar,algorand,sui,xrpl,hedera` | OK |
| `cargo test --locked -p x402-rs --features … -- --test-threads=1` (sobre `2deb2c3e`) | lib 1347 ok, bin 1402 ok, integración ok, 0 fallas |
| `cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance -- --test-threads=1` | 109 ok, 0 fallas |
| `cargo clippy --locked -p x402-rs --features … --all-targets` | exit 0; sobre líneas de esta rama queda 1 aviso, `path_networks_json is never used`, la misma clase que los otros ~80 `path_*` de `openapi.rs` |
| `cargo clippy -p x402-compliance` | limpio |
| `rustfmt` | `src/networks_json.rs` entero; en el resto solo los hunks propios (`origin/main` no está fmt-limpio) |
| `terraform fmt -check` (arc.tf, secrets.tf, lambda-balances.tf) / `init -backend=false` / `validate` | OK / OK / OK (el `fmt -check` del directorio falla en 4 archivos que esta rama no toca, igual que en `origin/main`) |
| `no-account-id.yml` | ningún ARN con cuenta en el diff |

Navegador (headless shell, servidor local con `/supported` de la captura y `/networks.json`
armado del JSON; todo lo que no era 127.0.0.1 abortado): 65 íconos de red y todos los de token
con `src`, 0 imágenes rotas, 43 tarjetas con click, 0 enlaces sin `href`, los 8 filtros de
stablecoin habilitados, 0 errores de JS. Capturas:
`docs/handoffs/assets/2026-09-24-networks-json-landing.png` y `…-wallets.png`.

## Exploradores verificados (2026-09-24)

Un subagente pidió cada plantilla con un id real de esa red (y uno falso) con User-Agent de
navegador. Sin cambios salvo los dos de arriba. Detrás de Cloudflare/WAF (403/202 a curl, el
dominio resuelve y el cuerpo devuelve el id): snowtrace (+testnet), bscscan, basescan sepolia,
solscan, nearblocks, perawallet testnet. SPA que no se puede distinguir por curl (200 para
todo): stellar.expert, xrpl.org, suiscan, allo, el Blockscout de HyperEVM testnet.
**Hashscan (Hedera) quedó sin verificar**: sirve 404 a curl para toda ruta, incluso `/mainnet`
(bucket de GCS sin reescritura); se mantuvo la forma que ya usaban la portada y el visor.

## El RPC de Arc (commit `2deb2c3e`, agregado de c0der)

- `arc.tf`: con `arc_mainnet_enabled`, `local.arc_rpc_secrets` =
  `RPC_URL_ARC` → `"${data.aws_secretsmanager_secret.rpc_mainnet.arn}:arc::"` (el molde de
  `RPC_URL_BASE`), que `all_task_secrets` (`secrets.tf`) suma a la task definition.
  `local.arc_rpc_environment` queda solo con `RPC_URL_ARC_TESTNET` (público): la variable ya no
  está en `environment`, y no puede estar en los dos.
- Rol de ejecución: ya lee el secreto. `rpc_secret_arns` (`secrets.tf:200`) incluye
  `data.aws_secretsmanager_secret.rpc_mainnet.arn`, que entra en `all_secret_arns` y de ahí en
  el `Resource` de la política del rol (`main.tf:608`). Nada que agregar.
- Lambda de saldos (`lambda-balances.tf:113`): usa `local.arc_balance_rpc_environment`, los dos
  públicos, iguales a los de hoy. Razón (comentada en `arc.tf`): solo lee saldos, y una Lambda
  no tiene bloque `secrets`: darle la URL premium la pondría en texto plano.
- Docs: `SECRETS_MANAGEMENT.md`, `IMPLEMENTATION_SUMMARY.md`, `CLAUDE.md` y `CHANGELOG.md`
  nombran la clave `arc`. La URL no aparece en ningún archivo, test, log ni commit.
- `terraform fmt -check` (los tres `.tf`), `init -backend=false` y `validate`: OK. Sin plan ni
  apply (el Terraform local es 1.5.7 y el estado es 1.9.8; el plan es el job `plan` del PR).
  `init` tocó `.terraform.lock.hcl` y se restauró.

**Ojo: el merge lo aplica solo.** El job `deploy` de `ci.yaml` corre en cada push a `main`
`terraform apply -auto-approve -target=aws_ecs_task_definition.facilitator …` (y, como cambió
`arc.tf`, también `-target=aws_lambda_function.balances`). No hace falta aplicarlo a mano; si
se quiere mirar antes, con Terraform 1.9.x y credenciales:

```bash
cd terraform/environments/production && terraform init -input=false
terraform plan -input=false \
  -target=aws_ecs_task_definition.facilitator -target=aws_lambda_function.balances \
  -var="image_tag=$(aws ecs describe-task-definition --task-definition facilitator-production \
      --region us-east-2 --query 'taskDefinition.containerDefinitions[0].image' --output text | sed 's/.*://')"
```

Lo que tiene que mostrar: `aws_ecs_task_definition.facilitator` **reemplazada** (una task
definition es inmutable), cuyo único cambio en `container_definitions` es `RPC_URL_ARC` saliendo
de `environment` y entrando en `secrets` con `valueFrom = "arn:…:secret:facilitator-rpc-mainnet-…:arc::"`;
`aws_lambda_function.balances` **sin cambios**. Cualquier otra cosa en ese plan no viene de
este commit. Después del deploy: `/health/ready?network=arc` sin `rpc_*` y los logs sin 429 de
Arc; si el secreto no tuviera la clave `arc`, la task no arranca (ECS falla al resolver el
`valueFrom`), que es lo que conviene ver en el primer rollout.

## Para c0der

Después del deploy (el push a `main` lo despliega por CI), el comando de cierre:

```bash
python3 -c "import json,urllib.request as u;F='https://facilitator.ultravioletadao.xyz';s=json.load(u.urlopen(F+'/supported'));n=json.load(u.urlopen(F+'/networks.json'));a={k['network'] for k in s['kinds']};b={i for r in n['networks'] for i in (r['id'],r['caip2'])};assert a<=b,sorted(a-b);assert all(r.get('explorer') and r.get('icon') for r in n['networks'])"
```

Además mirar: `curl -s …/version` = `2.41.0`; `curl -sI …/networks.json` con
`cache-control: public, max-age=300`; la portada y `/networks` con logos (un navegador con
`/x402.js?v=20260923` cacheado no aplica: la URL cambió); `/events/live` con enlaces.

Lo que NO hace este PR y queda para después:
- El SDK de TS (catálogo y NetworkPicker sobre `/networks.json`): otro encargo.
- La portada sigue teniendo sus 44 tarjetas a mano (ahora sin exploradores ni íconos): una red
  nueva aparece en `/networks.json`, `/networks` y `/events/live` sola, pero en la pared de la
  portada hace falta su tarjeta.
- `static/.well-known/x402` sigue siendo una lista de redes a mano (drift #1 de
  `superficies.md`): candidata a generarse de lo mismo.
- `/networks.json` no se nombra todavía en `llms.txt`/`skill.md` (cambiarlos mueve digests y
  `llms-full.txt`): una línea en la próxima tanda de superficies agénticas.

## Duplicación

- **Duplicación (en este repo):** `static/.well-known/x402` (`networks[]` con `caip2`,
  `family`, `tokens`), la tabla `PUBLIC_RPCS` + constantes de wallets de `static/networks.html`
  y de `static/index.html`, y `get_network_configs()` de `lambda/balances/handler.py`
  resuelven cada uno «qué redes hay y cómo se llaman» por su cuenta.
- **Duplicación (ecosistema, según el barrido DUP-07, no re-medido acá):** execution market,
  meshrelay, 402milly y el SDK de TS mantienen su propia tabla de nombre/ícono/explorador por
  red; `/networks.json` es la fuente para reemplazarlas.
- Observación del subagente: el JSON-RPC público de Sui (`fullnode.mainnet.sui.io`) responde
  `-32601`; `lambda-balances.tf` ya lo evita, pero conviene revisar cualquier otro lugar que lo
  use (p. ej. los `PUBLIC_RPCS` de respaldo de las páginas).
