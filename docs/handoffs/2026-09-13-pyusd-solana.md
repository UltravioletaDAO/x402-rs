# PYUSD en Solana (Token-2022) — handoff 2026-09-13

- **Rama**: `0xultravioleta/x4-pyusd-solana` (desde `origin/main` c0ff8d53)
- **VERSION**: 2.26.0 (desplegado) -> 2.27.0
- **Pedido**: "yo creia que soportabamos PYUSD en el facilitador ... solo veo USDC y
  Agora Finance ... es un Token-2022 y ya tenemos soporte para eso".
- **Veredicto**: no estaba. PYUSD existia solo en EVM (Ethereum). En Solana el
  allow-list (`supported_asset_addresses`) rechazaba el mint con `unsupported_asset`
  antes de tocar el RPC. Ahora esta, con una guarda de comision de transferencia
  que antes no existia para NINGUN mint Token-2022.

## 1. Lo medido

Todo leido con `getAccountInfo` (jsonParsed y base64) contra
`api.mainnet-beta.solana.com` / `api.devnet.solana.com`.

### PYUSD mainnet `2b1kV6DkPAnxd5ixfnxCpjxmKwqjjaYmCZfHsFu24GXo`

Slot 446743095, epoch 1034. Owner `TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb`
(Token-2022), 866 bytes, decimals 6.

| Extension | Estado |
|---|---|
| mintCloseAuthority | `2apBGMsS6ti9RyF5TwQTDswXBWskiJP2LD4cUEDqYJjk` |
| permanentDelegate | `2apBGMsS…` (Paxos puede mover saldo de cualquier cuenta; igual que AUSD) |
| **transferFeeConfig** | older = newer = `{epoch 605, transferFeeBasisPoints 0, maximumFee 0}`, withheld 0. **`transferFeeConfigAuthority` = `2apBGMsS…`: la autoridad PUEDE cambiarla.** |
| confidentialTransferMint | authority `2apBGMsS…`, autoApprove false, auditor null |
| confidentialTransferFeeConfig | presente |
| **transferHook** | authority `2apBGMsS…`, **`programId: null`** (sin hook; la autoridad podria poner uno) |
| metadataPointer / tokenMetadata | "PayPal USD" / "PYUSD" |

### PYUSD devnet `CXk2AMBfi3TwaEL2468s6zP8xq9NxTXjp9gjMgzeUynM` (verificado, no supuesto)

Owner Token-2022, 869 bytes, decimals 6, epoch 1152. Mismo set de extensiones;
comision `{epoch 644, 0 bps, maximumFee 0}`, hook `null`. Es el mint que reparte el
faucet de Paxos (`faucet.paxos.com`, config `SOLANA_PYUSD`).

### Correccion a la premisa del spec (§1.d)

El spec dice que `transferFeeConfig` y `confidentialTransferMint` son "DOS
extensiones que AUSD no tiene". **AUSD las tiene** (slot 446743098, epoch 1034):
`transferFeeConfig {epoch 724, 0 bps, maximumFee 1_000_000}`,
`confidentialTransferMint`, `confidentialTransferFeeConfig`, `transferHook` con
`programId: null`. El comentario de `AUSD_SOLANA` ("PermanentDelegate,
TransferHook, Metadata") estaba incompleto; lo corregi. Consecuencia: el hueco de
la comision ya existia para AUSD, y la guarda no es un caso especial de PYUSD.

## 2. Decision de diseno

**Comision efectiva hoy = 0 en ambos mints**, asi que el verify sigue comparando
el `amount` exacto de la instruccion y ademas lee el mint y RECHAZA si la
comision deja de ser 0 (`invalid_exact_svm_payload_transaction_transfer_fee_not_zero`,
sin firmar nada).

Por que hacia falta: `TransferChecked` sobre un mint con `TransferFeeConfig` le
descuenta `amount` al pagador pero le acredita `amount - fee` al payee (la
comision queda retenida en la cuenta destino). Path 1 compara el amount de la
instruccion y el settle no mira saldos, asi que con una comision > 0 un cobro
exacto se volvia un cobro corto que igual verificaba. (Path 2 compara el delta de
saldo simulado del payee, pero solo cuando la simulacion devuelve ese saldo: la
rama `(_, None)` de `find_transfer_in_inner_instructions` se salta el chequeo y
acepta. Path 1 no compara saldos nunca.)

Como (`src/chain/solana.rs`):

- `transfer_fee_upper_bound(owner, data, epoch, amount)` — funcion pura. Devuelve
  el **maximo entre la comision vigente en `epoch` y la `newer_transfer_fee`**,
  aunque esta todavia este programada. spl-token-2022 hace efectiva una comision
  nueva dos epochs despues de fijarla (`newer_fee_start_epoch = epoch.saturating_add(2)`,
  `extension/transfer_fee/processor.rs:101`), asi que leer las dos entradas
  significa que un cambio nunca puede caer entre un verify que vio 0 y el
  settlement que aprobo. Mint de SPL Token clasico (USDC) = 0 sin leer
  extensiones. Cuenta que no pertenece a un programa de tokens = rechazo, no "sin
  comision".
- `verify_mint_charges_no_transfer_fee(mint, amount)` — lee `[mint, SysvarClock]`
  en UN `getMultipleAccounts` (el epoch es del mismo slot que la config). Error de
  RPC = rechazo: sin veredicto no es comision cero.
- Se llama al final de `verify_transfer` cuando `token_program == spl_token_2022`,
  para Path 1 (top-level) y Path 2 (CPI de smart wallets). `settle` vuelve a
  correr `verify_transfer` inmediatamente antes de firmar, asi que la guarda corre
  en ambos. USDC (SPL clasico) no paga el RPC extra.

**TransferHook**: el facilitador no arma cuentas extra para nadie, ni hoy para
AUSD: el cliente construye la transaccion completa y el facilitador solo firma
como fee payer; la simulacion (`simulateTransaction`, ya existente) rechaza una
transferencia a la que le faltan las cuentas del hook. Con `programId: null` en
los dos mints no hay cuentas extra. PYUSD usa exactamente el mismo camino que
AUSD, sin caso especial.

**Confidential transfers**: fuera de alcance por decision del dueno. Una
transferencia confidencial no es un `TransferChecked`, asi que el parser no la
reconoce y el verify la rechaza; no toque nada de eso.

## 3. Cambios

| Archivo | Cambio |
|---|---|
| `src/network.rs` | `PYUSD_SOLANA` y `PYUSD_SOLANA_DEVNET` (forma exacta de `AUSD_SOLANA`: decimals 6, eip712 None); `PYUSDDeployment::by_network` y `supported_networks` con Solana y SolanaDevnet; comentario de `AUSD_SOLANA` corregido; tests. `get_token_deployment`, `supported_asset_addresses` e `is_supported_asset` ya derivan de `by_network`: no hay otra lista blanca de mints (grep de los mints en `src/`: solo `openapi.rs` con ejemplos USDC). |
| `src/chain/solana.rs` | `transfer_fee_upper_bound`, `verify_mint_charges_no_transfer_fee`, llamada en `verify_transfer`; tests con mock RPC y bytes reales de los mints. |
| `src/handlers.rs` | test de drift: las entradas SVM de `/.well-known/x402` == `supported_tokens_for_network`, y solana lista PYUSD. |
| `static/.well-known/x402` | `"PYUSD"` en tokens de `solana`. `solana-devnet` solo aparece en `testnets` (name + caip2, sin `tokens`): nada que agregar. |
| `static/index.html` | `TOKEN_SUPPORT['solana-mainnet']` y `['solana-devnet']` + `'pyusd'`. La tarjeta decide sus iconos con ese mapa (`data-tokens="solana-mainnet"` -> `displayTokenSupport()`); `TOKEN_INFO.pyusd` y `.token-pill.pyusd` ya existian. |
| `README.md` | Fila Solana, fila PYUSD, matriz (coincide con `python scripts/stablecoin_matrix.py --md`: `SOLANA | Y | - | Y | Y`), seccion Solana. |
| `config/supported_tokens.json` | `pyusd` + `pyusdMint` en `solana` y `solana-devnet`. |
| `tests/pyusd-solana-e2e/` | `pay.mjs`: un pago x402 `exact` real en PYUSD por un facilitador (verify + settle). Es el comando del settle de prueba de mainnet. |
| `VERSION` | 2.27.0 |

## 4. Tests (rojo contra origin/main, verde con el cambio)

Rojo medido compilando los tests nuevos contra la implementacion de main (tests
escritos antes que el codigo), `cargo test --lib -- --test-threads=1`:

| Test | En main | Con el cambio |
|---|---|---|
| `network::tests::test_pyusd_solana_address` | FAILED: `get_token_deployment(Solana, Pyusd)` es `None` | ok |
| `network::tests::test_pyusd_solana_devnet_address` | FAILED: `None` en SolanaDevnet | ok |
| `network::tests::test_pyusd_supported_on_solana_not_on_base_or_polygon` | FAILED: `is_token_supported(Solana, Pyusd)` false | ok (Base, Polygon y Fogo siguen false) |
| `network::tests::test_pyusd_supported_networks` (reemplaza `test_pyusd_ethereum_only`) | FAILED: left 1, right 3 | ok |
| `network::tests::test_supported_tokens_for_solana` | FAILED: left 2, right 3 | ok |
| `chain::solana::tests::test_verify_pyusd_token2022_transfer_listed_mint_passes_unlisted_mint_fails` | FAILED: `Err(Other("unsupported_asset: network=solana, asset=2b1k…"))` | ok: listado verifica; mint Token-2022 no listado -> `unsupported_asset` |
| `chain::solana::tests::test_verify_rejects_token2022_mint_with_nonzero_transfer_fee` | FAILED por la razon correcta: `AUSD with a 1 bps fee in force: Valid { payer: … }` (main aprobaba un cobro corto de AUSD) | ok: los 5 casos (AUSD y PYUSD tal cual -> verifica; 1 bps en AUSD, 50 bps vigente y 50 bps programada a epoch+2 en PYUSD -> rechazo) |
| `chain::solana::tests::test_transfer_fee_upper_bound_on_the_deployed_mints` | no compila en main (funcion nueva) | ok: 0, 50, 50 programada, tope `maximumFee` 3, mint 2022 sin extensiones 0, SPL clasico 0, owner ajeno y datos truncados -> error |
| `handlers::agentic_surface_tests::the_x402_document_lists_the_svm_tokens_the_allow_list_accepts` | FAILED: `solana must list PYUSD` (el drift de solana/fogo pasaba) | ok |

Los tests de verify manejan `Facilitator::verify` completo contra
`RpcClient::new_mock_with_mocks_map`. Los mints son los **bytes reales** on-chain
(base64, slots arriba), no vectores inventados; los casos con comision reescriben
solo `newer_transfer_fee` sobre esos bytes.

## 5. Verificacion real

### (a) cargo test completo

macOS, rust 1.98.1, mismos comandos que `.github/workflows/ci.yaml` (job `test`):

- `cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1`:
  **11 binarios, 2254 passed, 0 failed, 13 ignored** (exit 0).
- `cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance -- --test-threads=1`:
  **9 binarios, 109 passed, 0 failed, 9 ignored** (exit 0). Corrido con `-j 2`: la
  primera corrida la mato macOS por memoria (14% libre, varios workers en la misma
  Mac) mientras ejecutaba, con los 4 binarios que alcanzaron a correr en verde; no fue
  un fallo de test.
- `cargo build --locked --features solana,near,stellar,algorand,sui,xrpl`: exit 0.
- `python3 scripts/verify_landing_canonical.py --offline` (paso del job `test`): `[OK]`.
- Fuera de CI (el yml no corre fmt ni clippy): `cargo fmt --all -- --check` limpio;
  `cargo clippy --locked -p x402-rs --features … --all-targets` y
  `cargo clippy --locked -p x402-compliance` exit 0, **0 warnings en lineas agregadas**
  (los warnings que hay son preexistentes).
- `python scripts/stablecoin_matrix.py --md`: `SOLANA | AUSD Y | EURC - | PYUSD Y | USDC Y`.

### (b) e2e devnet contra facilitador local — PASO

Condiciones del spec cumplidas: el mint de devnet verifico on-chain y el fee payer
de devnet `6xNPewUdKRbEZDReQdpyfNUdgNg8QRc8Mt263T5GZSRv` tenia 4.96 SOL.

Preparacion (todo devnet, wallets descartables en un directorio temporal, no versionado):

- Pagador `DZFwWaE1Vz9Yes5TVv8qezCRwYR5VrK2Zu1GvaCuapF8`: 100 PYUSD del faucet de
  Paxos (`POST https://api.sandbox.paxos.com/v2/treasury/faucet/transfers`, sin captcha).
- El airdrop publico de SOL de devnet respondio 429, asi que la renta de la ATA del
  payee salio del fee payer de devnet: 0.01 SOL `6xNPew -> DZFw`, tx
  `5Sx3dGZgj986WjMmF9qXdKH4Sj9wtTijs9cF3PbLxXe91pYNFZF3G3Qxi2sBd6zyafHk2eRnWUYU6qgTo3FoU6gi`
  (la pregunta sobre la clave quedo planteada al mantenedor, fuera de git).
- ATA PYUSD del payee `38aongSEWXENBnmbQ7MRUYbSMTx8GJsaeJhDcQsL3MYS` =
  `APFxpjXPTHYqbvQvNAMFw4rY3eb8gWETptqxHTztZKJF`, tx
  `5TvQ1cXq3gAiwQM5Z4UTZU513yYaHmhH7PoKUjvAtP4FvTeFrF9raiEYTKuGmCBpaMp7Pzywk3fx83RMFKNaAsXc`.

Facilitador local: `target/debug/x402-rs` de esta rama (`cargo build --locked --features
solana,near,stellar,algorand,sui,xrpl`), `127.0.0.1:18402`, solo `RPC_URL_SOLANA_DEVNET`,
**sin acceso a AWS** (`env -i`, credenciales en `/dev/null`: el lease y el nonce store
apuntan por defecto a la tabla DynamoDB de produccion), `ENABLE_WRITER_LEASE=false`,
discovery apagado, `config/blacklist.json` vacio temporal (gitignored; lo borre).
`/supported` sirvio `solana-devnet` y `solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1` con
`feePayer 6xNPew…`.

Trazo exacto (`node tests/pyusd-solana-e2e/pay.mjs --network solana-devnet --facilitator
http://127.0.0.1:18402 --payer <payer.json> --pay-to 38aong… --amount 10000`):

```
network     solana-devnet mint CXk2AMBfi3TwaEL2468s6zP8xq9NxTXjp9gjMgzeUynM
facilitator http://127.0.0.1:18402 feePayer 6xNPewUdKRbEZDReQdpyfNUdgNg8QRc8Mt263T5GZSRv
payer       DZFwWaE1Vz9Yes5TVv8qezCRwYR5VrK2Zu1GvaCuapF8 payTo 38aongSEWXENBnmbQ7MRUYbSMTx8GJsaeJhDcQsL3MYS amount 10000
before      payer 100000000 payTo 0
verify      200 {"isValid":true,"payer":"DZFwWaE1Vz9Yes5TVv8qezCRwYR5VrK2Zu1GvaCuapF8"}
settle      200 {"success":true,"payer":"DZFwWaE1Vz9Yes5TVv8qezCRwYR5VrK2Zu1GvaCuapF8","transaction":"4QjrYmejPJtxEiXwMCDtLKs9BHsP9JEC6e7v13RVKMFotqdoE1SZejDXsDm1k5vbRAethJt4D8FMzd6QCXVYZw9h",...,"network":"solana-devnet"}
after       payer 99990000 payTo 10000
```

Leido de la cadena despues (`getTransaction` jsonParsed, devnet):
tx `4QjrYmejPJtxEiXwMCDtLKs9BHsP9JEC6e7v13RVKMFotqdoE1SZejDXsDm1k5vbRAethJt4D8FMzd6QCXVYZw9h`,
slot 497800132, `err: null`, fee 10001 lamports pagados por `6xNPew…` (firmantes:
`6xNPew…` y `DZFw…`), instrucciones: 2x ComputeBudget + `transferChecked` del
programa Token-2022 (`TokenzQd…`) `DZFw… -> APFx…` mint `CXk2…`. Saldos Token-2022:
payee 0 -> 10000, pagador 100000000 -> 99990000. **El payee recibio exactamente el
amount firmado**: comision 0, que es lo que la guarda leyo del mint en el verify y
otra vez en el settle.

Dos cosas que aparecieron en el camino, preexistentes y ajenas a PYUSD (no las toque):

1. **Rate limiter en conexion directa**: sin `X-Forwarded-For` el facilitador local
   responde 500 `rate_limit_key_unavailable` a `/verify` y `/settle`. Detras del ALB
   siempre llega. `pay.mjs` lo agrega solo si el facilitador es loopback.
2. **Commitment de verify vs settle**: `/verify` simula con `confirmed`, pero el
   preflight de `sendTransaction` en `/settle` usa `RpcSendTransactionConfig::default()`,
   es decir el commitment por defecto de `RpcClient::new` (`finalized`). Un cliente
   que pide el blockhash con `confirmed` y paga enseguida obtuvo `verify 200` y luego
   `settle 400 contract_call_failed` ("Transaction simulation failed: Blockhash not
   found", correlation `69dbe272-24af-4576-b836-74343841a8d1`); no se movio nada. Con
   blockhash `finalized` paso. Pasa igual en produccion para cualquier token de
   Solana. Si se quiere, arreglo aparte: `preflight_commitment: Some(confirmed)`.

### (c) mainnet

No movi fondos. Pendiente para el mantenedor con el dueno despues del release: el
comando exacto quedo en una nota fuera de git.

## Para el mantenedor

Tras el release (merge a main = deploy):

1. `curl -s https://facilitator.ultravioletadao.xyz/version` -> `2.27.0`.
2. `curl -s https://facilitator.ultravioletadao.xyz/.well-known/x402 | jq '.x402.networks[] | select(.name=="solana") | .tokens'` -> `["USDC","AUSD","PYUSD"]`.
3. Landing: la tarjeta Solana (pestana mainnet) muestra el icono PYUSD junto a USDC y AUSD; la de Solana Devnet (testnet) USDC y PYUSD.
4. `/supported` **no cambia**: para `solana` / `solana:5eykt4Us…` sigue trayendo solo `extra.feePayer` (no enumera assets en SVM; `tokens: None` es un TODO preexistente en `SolanaProvider::supported`). `POST /accepts` tampoco lista PYUSD en Solana: `post_accepts` copia el `extra` de `/supported` por (scheme, network), asi que en SVM solo agrega `feePayer`. Preexistente: la nota del manifiesto ("Read them live from POST /accepts (extra.tokens) or GET /supported") no se cumple para SVM; hoy el manifiesto es el unico lugar publico donde un integrador ve que PYUSD se acepta en Solana.
5. Settle de prueba en mainnet con 0.01 PYUSD: comando en la nota fuera de git.
6. Refutador: la guarda vive en `src/chain/solana.rs` (`transfer_fee_upper_bound`, `verify_mint_charges_no_transfer_fee`, paso 6 de `verify_transfer`).

## Lo que NO hice

- **Settle en mainnet**: no movi fondos (spec §2.5.c).
- **Confidential transfers** (`confidentialTransferMint`): fuera de alcance; el verify las sigue rechazando.
- **`TransferCheckedWithFee`** (instruccion 26, la que declara la comision): no se acepta; solo `TransferChecked` (12), como antes.
- **Guarda de hook**: no rechazo un mint con `transferHook.programId` no nulo. Si Paxos pone un hook, el cliente tiene que pasar las cuentas extra y la simulacion decide, igual que hoy para AUSD.
- **Camino settlement-account (Crossmint)**: `sweep_settlement_account` fija `spl_token::id()` y decimals 6 de USDC, asi que no sirve para ningun mint Token-2022 (tampoco AUSD, preexistente). No lo toque; con un mint 2022 el sweep falla on-chain, no cobra corto. El camino sin sweep compara deltas de saldo on-chain, que ya son netos de comision.
- **`/supported`**: no enumera assets en SVM y no lo cambie.
- **Fogo**: no hay PYUSD alli; sigue sin PYUSD (test lo fija).
- **CHANGELOG**: los PRs recientes no lo tocan (va atrasado segun CLAUDE.md); no agregue entrada.
- **`src/openapi.rs`, `lambda/balances/handler.py`, terraform**: sin listas de tokens de Solana que actualizar; terraform no se toco (el drift gate no se dispara).
- **`docs/STABLECOIN_EXPANSION_PLAN.md`**: es un plan historico de 2025; no lo reescribi.
