# X-SOLANA-RESUME — altas Solana a medias y en una sola transacción, retiro en el acto de lo que no se entrega, y el script de custodiadas contra nodos públicos

**Encargo:** c0der, 2026-09-27 00:16Z (una tanda de x402-rs).
**Base:** `origin/main` = `bc4bdfd6` (2.44.0; re-medido al llegar). **Rama:** `0xultravioleta/c0-x-solana-resume`.
**Versión:** `2.45.0` (`VERSION`; el bump sale del `VERSION` de `main`: producción no se consultó, está prohibido en
este encargo).

## Estado

- **Hecho:** los puntos 1 a 4 del encargo y la ronda 2 del refutador (REF-X-SOLANA-RESUME), con tests sin red ni
  credenciales y una mutación por guarda (§5).
- **Falta (de c0der):** §6. Nada de esto se hizo acá: ni push, ni deploy, ni terraform, ni AWS, ni llamadas a
  producción.

## 1. Solana: un alta a medias sólo la retoma un reintento del pedido que la acuñó

`src/erc8004/solana_mint.rs::decide_mint`, `src/erc8004/register_jobs.rs`, `src/handlers.rs::run_solana_registration`.

- **El registro.** Cuando un alta Solana termina con la identidad todavía en el fee payer (`pending_stats` o
  `pending_transfer`: el camino por etapas, o un *resume* que falla), el handler guarda
  `red|activo → recipient` de la petición que la acuñó (`register_jobs::record_stranded_solana`). Vive donde vive el
  registro FAC-1 #2 de EVM: en memoria del proceso, 24 h. Un alta que termina entregada lo borra; un *resume* que
  vuelve a fallar lo refresca.
- **La decisión.** `decide_mint` encuentra candidatos por `agentUri` entre los activos que tiene el fee payer, y
  decide con el registro:
  - un candidato registrado para el `recipient` de esta petición → se retoma: sólo los pasos que faltan;
  - cualquier otro candidato (sin registro, registrado para otro `recipient`, o una petición sin `recipient`) →
    **`409`** con `mint.errorCode = "held_identity_not_resumable"` y `mint.status = "not_minted"`. No se envía nada
    (ni siquiera se lee el saldo del fee payer) y tampoco se acuña otra junto a una que el facilitador ya tiene con
    esa `agentUri`. Queda para recuperación manual;
  - ningún candidato → alta nueva (sin cambios).
- El doc comment de `decide_mint` se reescribió: la `agentUri` sólo encuentra candidatos, decide el registro, y
  no es una regla de unicidad de `agentUri` (una identidad ya entregada no está entre las del fee payer).
- **Límite, por diseño:** el registro es del proceso. Un reinicio, un cambio de writer lease o 24 h sin reintento
  lo pierden; el reintento entonces recibe el `409` y la identidad espera al operador. Es la dirección segura.

## 1b. Solana: un alta nueva sale en una transacción o no sale (ronda 2)

`src/handlers.rs::run_solana_registration`, `src/erc8004/solana_mint.rs`.

- Si el plan de un alta **nueva** no entra en una transacción (más de 4 entradas de `metadata`
  —`MAX_METADATA_ENTRIES`: `register`, `initialize_stats` y `transfer_agent` ocupan 3 de las 7 que conservan
  200 000 CU cada una— o más de 1232 bytes con valores largos): **`400 mint_not_atomic`**, antes de leer el saldo y
  de enviar nada. Antes salía por etapas, y un paso que el programa rechaza dejaba la identidad a medias en el fee
  payer.
- Una clave de `metadata` repetida: **`400 metadata_duplicate_key`**, antes de cualquier lectura (cada entrada es
  una cuenta derivada de su clave; la segunda fallaba el alta ya creada).
- Con eso nada que controle el pedido deja su propia alta a medias. El camino por etapas queda sólo para un
  *resume*, cuyo plan (stats + transfer) siempre entra.

## 2. EVM: si el transfer posterior al mint falla, la identidad se retira en el acto

`src/handlers.rs::retire_undelivered` y `try_recover_stranded_nft`; `src/erc8004/retire.rs::set_agent_uri`.

- En la rama de error del transfer, **antes** de guardar el registro FAC-1 #2: `setAgentURI(agentId,
  https://facilitator.ultravioletadao.xyz/erc8004/retired)` desde la wallet que acuñó, por el mismo provider y su
  nonce manager. El envío es el mismo del retiro admin de 2.44.0, factorizado en `retire::set_agent_uri`. La
  respuesta sigue siendo un `500` y su `error` dice si el retiro salió. Si el retiro falla: log `error`, lo dice la
  respuesta, y el registro se guarda igual (el alta ya había fallado; el retiro no la agrava).
- **La recuperación FAC-1 #2 acepta ahora una identidad retirada:** le devuelve la `agentURI` del pedido
  (`setAgentURI`) justo antes del transfer, porque después sólo el dueño nuevo podría cambiarla. Si ese transfer
  vuelve a fallar, la retira otra vez y sigue como antes (mint nuevo en esa misma llamada, que a su vez se retira si
  no se entrega).
- **Un recibo bueno no es una entrega** (ronda 2): después del recibo, `transfer_agent_nft` lee `ownerOf(id)`; si no
  es el `recipient` (su `onERC721Received` puede devolver el token dentro del mismo transfer) o no se puede leer, es
  un transfer fallido: retiro y registro como cualquier otro. Vale también para la recuperación.
- **Sin evento, no se retira** (ronda 2): si el recibo del mint no trae `Registered` y el id sale del respaldo
  `totalSupply`, ese id puede ser el de otra alta en vuelo; no se retira (la respuesta lo dice). El registro FAC-1
  #2 se guarda como antes.
- **Recuperación** (ronda 2): si no se puede devolver la `agentURI` a una retirada, no se entrega; un `tokenURI` que
  no es ni el del pedido ni el de retiro no se toca (el registro se descarta, como antes).
- **Alcance, dicho en `retire.rs`:** cubre una entrega que falla. Una identidad que su dueño nos manda después es
  ERC-721 funcionando; eso lo encuentra el script de custodiadas y lo retira el retiro admin.
- **Orden de nonces:** el transfer y el retiro salen del mismo firmante. Si el transfer "falló" porque el recibo no
  llegó a tiempo pero igual aterriza, el retiro va detrás: o la estimación lo rechaza (ya no es nuestra) y no se
  envía, o revierte. Nunca pisa una entrega.

## 3. `scripts/erc8004_custodied_identities.py` contra nodos públicos

Medido por c0der al correrlo el 2026-09-26. Cambios, todos sin salir de los métodos de lectura:

- **`User-Agent` propio** (`USER_AGENT`): con el de urllib varios RPC públicos contestan `403`.
- **Reintentos con espera creciente** (2 s, 4 s, 8 s) ante lo que no es una respuesta: conexión caída, timeout,
  `429`, `5xx`. Un rechazo del nodo (error JSON-RPC, u otro `4xx`) no se reintenta: preguntar de nuevo da lo mismo.
  Lo que no respondió nunca es `RpcUnavailable`, que **no** es `RpcError`: así un `ownerOf` sin respuesta no se lee
  como un token quemado; la corrida se corta y lo dice.
- **`eth_getLogs` se achica también ante timeout**, no sólo ante rechazo, y un timeout de ese método no se reintenta
  con el mismo tramo.
- **`eth_getCode` histórico:** un nodo sin estado viejo (incluido el que contesta `HTTP 400`, que antes salía como
  traceback) termina con un mensaje que sugiere `--from-block` o un nodo de archivo.
- El script nunca imprime la URL del RPC (un `RPC_URL_*` suele llevar la clave).

## 4. Tests nuevos

Rust (`cargo test`, sin red: un nodo JSON-RPC EVM en loopback, `payment_operator::test_rpc`, y un nodo Solana
guionado en el mismo módulo de tests):

- `erc8004::solana_mint`: el `recipient` registrado retoma; sin registro, registrado para otro o sin `recipient`,
  `Refuse`; con varios candidatos gana el registrado aunque ordene después; dos URIs con dos activos cada una,
  cada activo sólo para el suyo; la clave repetida se detecta; el tope de 4 entradas es justo donde el plan deja
  de ser atómico.
- `erc8004::register_jobs`: el registro Solana es por red y por activo, se reemplaza, no se acumula, y vence a las
  24 h.
- `erc8004_register_gate_tests` (Solana, handler completo): identidad del fee payer sin registro → `409` y ni
  `getBalance` ni envío; registrada para otro → `409` y el registro intacto, y el suyo la retoma (una transacción,
  que nombra al `recipient` original y a nadie más) y el registro se borra; un *resume* que vuelve a fallar
  renueva el registro y el siguiente reintento la termina; 6 entradas → `400 mint_not_atomic` y una clave repetida
  → `400 metadata_duplicate_key`, sin saldo ni envío; y ninguna `metadata` deja una identidad a medias bajo una
  `agentUri`: el pedido siguiente con esa URI acuña normal.
- `erc8004_register_gate_tests` (EVM): transfer rechazado → el mint y después exactamente un `setAgentURI(id,
  retirada)` desde el firmante que acuñó, y el registro FAC-1 #2; recibo bueno pero `ownerOf` sigue siendo nuestro
  → retiro y registro; id del respaldo `totalSupply` → sin retiro; retiro rechazado → reportado y el registro
  igual; la misma petición sobre una retirada → `setAgentURI(id, uri del pedido)` y después el `safeTransferFrom`
  al `recipient`, sin mint; si no se puede devolver la URI → no se entrega; un `tokenURI` ajeno → no se toca; y si
  el transfer de la recuperación falla → restaurar, retirar de nuevo, mint nuevo, retirarlo.
- `test_rpc::MockNode`: `revert_estimates(selector)` (un paso falla en la estimación y el resto aterriza),
  `on_call_sequence` (una lectura cuya respuesta cambia en el flujo) y `set_receipt_logs` (el evento del mint).

Python (`tests/scripts/test_erc8004_custodied_identities.py`, 12 → 20): `User-Agent`, reintentos y su espera, un `429`,
nodo que nunca responde (`RpcUnavailable`, no `RpcError`), rechazo sin reintento, tramo que da timeout se achica sin
reintentarse entero, `--from-block` ante `HTTP 400` y ante timeout, y un `ownerOf` sin respuesta corta la corrida.

## 5. Verificación (pre-CI local)

Entorno de todos los comandos: `HTTPS_PROXY/HTTP_PROXY=http://127.0.0.1:9`, `NO_PROXY=127.0.0.1,localhost` (los
nodos de prueba viven en loopback), `AWS_SHARED_CREDENTIALS_FILE=/dev/null AWS_CONFIG_FILE=/dev/null`, sin
`AWS_PROFILE`, `cargo --offline`, `SWAGGER_UI_DOWNLOAD_URL` apuntando a una copia local del zip de Swagger UI
v5.17.14, `CARGO_TARGET_DIR` dentro del worktree, checkout en LF.

Todo sobre `a7573794`, el commit de código (el siguiente sólo agrega este handoff). Los pasos del job `test` de
`ci.yaml`, en serie:

| Paso | Comando | Resultado |
|---|---|---|
| Landing (offline) | `python3 scripts/verify_landing_canonical.py --offline` | OK |
| Frontend | `node --test tests/frontend-capabilities.test.cjs` | 19/19 |
| Monitor de saldos | `python3 -m unittest discover -s tests/scripts -p 'test_*balances.py'` | 5/5 |
| Auditoría de custodiadas | `python3 -m unittest discover -s tests/scripts -p 'test_erc8004_*.py'` | 20/20 (eran 12) |
| Drift gate de IAM | `python3 -m unittest discover -s tests/scripts -p 'test_drift_gate_*.py'` | 19/19 |
| Build | `cargo build --locked --features solana,near,stellar,algorand,sui,xrpl,hedera` | exit 0 |
| x402-rs | `cargo test --locked -p x402-rs --features <las mismas> -- --test-threads=1` | lib 1478, bin 1543, integración 66; **0 fallas** (2 min 34 s) |
| Crates | `cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance -- --test-threads=1` | 109; **0 fallas** |
| Filtro de `paths` | `python3 scripts/ci_paths_selftest.py` | exit 0 (no hay scripts nuevos) |
| `no-account-id.yml` + hook hex64 | réplica local de sus reglas, con un control positivo por regla, sobre el árbol y el diff | 0 hallazgos |
| `rustfmt` | sólo sobre los hunks de esta tanda (`main` no está formateado entero y el CI no corre fmt) | limpio |

### Mutaciones

Corrida sobre `a7573794` con `c0der/scripts/verificar_ronda.py` (worktree propio en LF, red cerrada), las 29 de esta
tanda y las 8 del refutador juntas: suite `rc=0` (149 s), **37/37 en rojo**, ninguna por no compilar, árbol limpio
al final, veredicto «todo como pide la ronda». Una por guarda; los JSON quedan fuera del repo.

| # | Mutación | Archivo | Qué rompe | Test | Resultado |
|---|---|---|---|---|---|
| 1 | `SR-reanuda-sin-mirar-el-recipient` | `src/erc8004/solana_mint.rs` | retoma con cualquier registro, sin comparar el `recipient` | `erc8004::{solana_mint,register_jobs,retire}` + `erc8004_register_gate` | **ROJO** (rc=101, 23 s) |
| 2 | `SR-sin-registro-se-reanuda` | `src/erc8004/solana_mint.rs` | un candidato sin registro se retoma | `erc8004::{solana_mint,register_jobs,retire}` + `erc8004_register_gate` | **ROJO** (rc=101, 22 s) |
| 3 | `SR-rechazo-acuna-otra` | `src/erc8004/solana_mint.rs` | en vez del `409`, acuña una identidad nueva | `erc8004::{solana_mint,register_jobs,retire}` + `erc8004_register_gate` | **ROJO** (rc=101, 18 s) |
| 4 | `SR-handler-no-consulta-el-registro` | `src/handlers.rs` | el handler no le pasa el registro a `decide_mint` | `erc8004::{solana_mint,register_jobs,retire}` + `erc8004_register_gate` | **ROJO** (rc=101, 25 s) |
| 5 | `SR-rechazo-sin-409` | `src/handlers.rs` | el rechazo sale con `200` | `erc8004::{solana_mint,register_jobs,retire}` + `erc8004_register_gate` | **ROJO** (rc=101, 22 s) |
| 6 | `SR-no-registra-al-trabarse` | `src/handlers.rs` | un alta o *resume* que se corta no escribe (ni renueva) el registro | `erc8004::{solana_mint,register_jobs,retire}` + `erc8004_register_gate` | **ROJO** (rc=101, 22 s) |
| 7 | `SR-no-borra-al-entregar` | `src/handlers.rs` | un alta entregada no borra su registro | `erc8004::{solana_mint,register_jobs,retire}` + `erc8004_register_gate` | **ROJO** (rc=101, 18 s) |
| 8 | `SR-registro-sin-red` | `src/erc8004/register_jobs.rs` | el registro no distingue red | `erc8004::{solana_mint,register_jobs,retire}` + `erc8004_register_gate` | **ROJO** (rc=101, 24 s) |
| 9 | `RT-no-retira-tras-transfer-fallido` | `src/handlers.rs` | no retira tras el transfer fallido | `erc8004::{solana_mint,register_jobs,retire}` + `erc8004_register_gate` | **ROJO** (rc=101, 28 s) |
| 10 | `RT-retira-a-otra-uri` | `src/handlers.rs` | retira hacia otra URI | `erc8004::{solana_mint,register_jobs,retire}` + `erc8004_register_gate` | **ROJO** (rc=101, 26 s) |
| 11 | `RT-recuperacion-descarta-la-retirada` | `src/handlers.rs` | la recuperación no acepta una identidad retirada | `erc8004::{solana_mint,register_jobs,retire}` + `erc8004_register_gate` | **ROJO** (rc=101, 29 s) |
| 12 | `RT-recuperacion-no-restaura-la-uri` | `src/handlers.rs` | la recuperación entrega sin devolver la `agentURI` | `erc8004::{solana_mint,register_jobs,retire}` + `erc8004_register_gate` | **ROJO** (rc=101, 27 s) |
| 13 | `RT-recuperacion-fallida-no-re-retira` | `src/handlers.rs` | una recuperación que falla no vuelve a retirar | `erc8004::{solana_mint,register_jobs,retire}` + `erc8004_register_gate` | **ROJO** (rc=101, 25 s) |
| 14 | `RT-set-agent-uri-desde-el-firmante-por-defecto` | `src/erc8004/retire.rs` | `setAgentURI` sale del firmante por defecto y no del dueño | `erc8004::{solana_mint,register_jobs,retire}` + `erc8004_register_gate` | **ROJO** (rc=101, 29 s) |
| 15 | `PY-sin-user-agent` | `scripts/erc8004_custodied_identities.py` | sin `User-Agent` propio | `test_erc8004_*.py` | **ROJO** (rc=1, 0 s) |
| 16 | `PY-sin-reintentos` | `scripts/erc8004_custodied_identities.py` | sin reintentos | `test_erc8004_*.py` | **ROJO** (rc=1, 0 s) |
| 17 | `PY-sin-espera-creciente` | `scripts/erc8004_custodied_identities.py` | espera fija entre reintentos | `test_erc8004_*.py` | **ROJO** (rc=1, 0 s) |
| 18 | `PY-timeout-no-achica-el-rango` | `scripts/erc8004_custodied_identities.py` | un timeout de `eth_getLogs` no achica el tramo | `test_erc8004_*.py` | **ROJO** (rc=1, 0 s) |
| 19 | `PY-timeout-reintenta-el-rango-entero` | `scripts/erc8004_custodied_identities.py` | un timeout de `eth_getLogs` se reintenta con el mismo tramo | `test_erc8004_*.py` | **ROJO** (rc=1, 0 s) |
| 20 | `PY-http-400-sale-como-traceback` | `scripts/erc8004_custodied_identities.py` | un `HTTP 400` sale como traceback | `test_erc8004_*.py` | **ROJO** (rc=1, 0 s) |
| 21 | `PY-sin-sugerir-from-block` | `scripts/erc8004_custodied_identities.py` | el error no sugiere `--from-block` | `test_erc8004_*.py` | **ROJO** (rc=1, 0 s) |
| 22 | `PY-sin-respuesta-como-token-quemado` | `scripts/erc8004_custodied_identities.py` | una lectura sin respuesta se toma como token quemado | `test_erc8004_*.py` | **ROJO** (rc=1, 0 s) |
| 23 | `RT-retira-un-id-adivinado` | `src/handlers.rs` | retira aunque el id salga del respaldo `totalSupply` | `erc8004::{solana_mint,register_jobs,retire}` + `erc8004_register_gate` | **ROJO** (rc=101, 29 s) |
| 24 | `RT-recibo-bueno-es-entrega` | `src/handlers.rs` | un recibo bueno cuenta como entrega aunque `ownerOf` sea nuestro | `erc8004::{solana_mint,register_jobs,retire}` + `erc8004_register_gate` | **ROJO** (rc=101, 26 s) |
| 25 | `RT-owner-ilegible-es-entrega` | `src/handlers.rs` | un `ownerOf` ilegible después del transfer cuenta como entrega | `erc8004::{solana_mint,register_jobs,retire}` + `erc8004_register_gate` | **ROJO** (rc=101, 28 s) |
| 26 | `SR-alta-que-no-entra-se-envia` | `src/handlers.rs` | un alta nueva que no entra en una transacción se envía por etapas | `erc8004::{solana_mint,register_jobs,retire}` + `erc8004_register_gate` | **ROJO** (rc=101, 27 s) |
| 27 | `SR-handler-no-mira-claves-repetidas` | `src/handlers.rs` | el handler no mira las claves repetidas | `erc8004::{solana_mint,register_jobs,retire}` + `erc8004_register_gate` | **ROJO** (rc=101, 23 s) |
| 28 | `SR-clave-repetida-no-se-detecta` | `src/erc8004/solana_mint.rs` | `duplicate_metadata_key` no detecta nada | `erc8004::{solana_mint,register_jobs,retire}` + `erc8004_register_gate` | **ROJO** (rc=101, 27 s) |
| 29 | `SR-tope-de-metadata-corrido` | `src/erc8004/solana_mint.rs` | el tope que cita el `400` no es donde el plan deja de ser atómico | `erc8004::{solana_mint,register_jobs,retire}` + `erc8004_register_gate` | **ROJO** (rc=101, 23 s) |

Del refutador (REF-X-SOLANA-RESUME); en su corrida sobre la primera versión sobrevivían la 2, la 3, la 4 y la 8, y
la ronda 2 agregó los tests que las matan:

| # | Mutación | Archivo | Qué rompe | Test | Resultado |
|---|---|---|---|---|---|
| 1 | `REF-decide-sin-chequeo-de-owner` | `src/erc8004/solana_mint.rs` | `decide_mint` no mira el `owner` del candidato | `erc8004` | **ROJO** (rc=101, 26 s) |
| 2 | `REF-registro-solana-nunca-vence` | `src/erc8004/register_jobs.rs` | el registro Solana no vence a las 24 h | `erc8004` | **ROJO** (rc=101, 25 s) |
| 3 | `REF-restauracion-fallida-entrega-igual` | `src/handlers.rs` | si no se puede devolver la URI, se entrega retirada | `erc8004` | **ROJO** (rc=101, 32 s) |
| 4 | `REF-recuperacion-acepta-cualquier-uri` | `src/handlers.rs` | la recuperación acepta cualquier `tokenURI` no vacía | `erc8004` | **ROJO** (rc=101, 29 s) |
| 5 | `REF-control-recuperacion-no-restaura` | `src/handlers.rs` | la recuperación no devuelve la URI (control de la #12) | `erc8004` | **ROJO** (rc=101, 27 s) |
| 6 | `REF-PY-lista-de-lectura-no-se-mira` | `scripts/erc8004_custodied_identities.py` | el script deja pasar métodos de escritura | `test_erc8004_*.py` | **ROJO** (rc=1, 14 s) |
| 7 | `REF-PY-400-se-reintenta` | `scripts/erc8004_custodied_identities.py` | un `HTTP 400` se reintenta como caída | `test_erc8004_*.py` | **ROJO** (rc=1, 0 s) |
| 8 | `REF-PY-429-no-reintenta` | `scripts/erc8004_custodied_identities.py` | un `429` no se reintenta | `test_erc8004_*.py` | **ROJO** (rc=1, 0 s) |

**Regresión de X-TANDA-R425:** de sus mutaciones (las 44 de `docs/handoffs/X-TANDA-R425-mutaciones.json` y las 23 de
su refutador), las 35 que tocan archivos cambiados acá se corrieron sobre `a7573794` con la misma herramienta:
**35/35 en rojo**, árbol limpio. Todas las demás aplican tal cual. La única que ya no aplica es
`L4-retiro-desde-otro-firmante` (su texto pasó a `retire::set_agent_uri`); su equivalente es la #14 de arriba.

## 6. Para c0der

1. **Merge y deploy** (un CI, un deploy). No toca terraform, IAM ni secretos; el plan dirigido sólo cambia la imagen.
2. **Versión:** `/version` = `2.45.0` con un solo deployment de ECS en `COMPLETED`, **antes** de cualquier sonda:
   `/version` lo contesta cualquier task, y `/register` lo corre la que tiene el writer lease.
3. **Sondas post-deploy** (ninguna acuña):
   - `/docs`: `curl -s https://facilitator.ultravioletadao.xyz/api-docs/openapi.json | grep -c held_identity_not_resumable` ≥ 1.
   - Solana, el `409` y el tope: las sondas con valores están en la entrega privada.
   - EVM: provocar un transfer fallido en producción exige acuñar; no se hace. Lo cubren los tests de §4, y en
     producción se ve en los logs (`Undelivered agent NFT retired` / `could not be retired`).
   - El script con el RPC público de Base, sin `--rpc`: ya no `403`; si el nodo no tiene estado histórico, termina
     pidiendo `--from-block` en vez de un traceback.
4. **Recuperación manual de identidades retenidas:** ver la entrega privada.
