# /verify de Algorand, NEAR y Sui consulta el estado de replay — handoff 2026-09-14

- **Rama**: `0xultravioleta/x4-verify-liquidado` (desde `origin/main` fcc26a3c, 2.29.0 desplegado)
- **VERSION**: 2.29.0 -> 2.29.1
- **Que cambia**: el `/verify` de Algorand, NEAR y Sui pasa a consultar el estado de replay
  antes de responder valido, por paridad con EVM (el `eth_call` de `transferWithAuthorization`
  revierte con un nonce usado) y Stellar (`check_nonce_unused` contra el nonce store).
- **Que no cambia**: `/settle` de las tres redes (cada una conserva su barrera: el claim atomico
  de Algorand en `submit_group`, el runtime de NEAR, la ejecucion de Sui), `handlers.rs`, EVM y
  Stellar.

## 1. Por red

| Red | Lectura nueva en verify | Rechaza cuando | Si la lectura falla |
|---|---|---|---|
| Algorand | `NonceStore::is_used` sobre `algorand_nonce_key(chain, group_id)` — `AlgorandProvider::check_group_unused` | el group_id ya esta reclamado (lo reclama `submit_group` antes de transmitir) | **fail-closed**, igual que Stellar: `NonceStoreUnavailable("verification_unavailable (ref)")` |
| NEAR | `view_access_key(sender_id, public_key)` a finality `optimistic` — `NearProvider::check_delegate_nonce_unused` | `delegate_action.nonce <= access_key.nonce`, o la access key / cuenta no existe | **fail-closed**: `ContractCall`; acotado por `RPC_REQUEST_TIMEOUT_SECS` (10 s por defecto) con `tokio::time::timeout` |
| Sui | `sui_multiGetObjects` sobre los objetos owned de la tx (coin del PTB + gas) — `SuiProvider::check_inputs_current` | algun objeto esta en una version MAYOR que la referenciada, en la misma version con otro digest, o figura `Deleted`. Version MENOR o `NotExists`: no rechaza (seccion 1, Sui) | **fail-closed**: `ContractCall`; mismo limite via `SuiClientBuilder::request_timeout` |

Las tres lecturas son de solo lectura y viven en `Facilitator::verify`, no en `verify_payment*`
(que tambien usa settle).

### Algorand: por que alcanza con el store

La tx de fee (indice 0) solo la puede firmar el facilitador, y `submit_group` reclama el group_id
en el store ANTES de transmitir, asi que todo grupo que llego a cadena por este facilitador esta
registrado. El TTL (`algorand_ttl_seconds`) cubre la ventana de validez + 1 h; pasada la ventana,
`verify_payment_group` ya lo rechaza por `TransactionExpired`. algod no ofrece busqueda por txid en
el ledger sin indexer (`/v2/transactions/pending/{txid}` solo ve lo confirmado hace muy poco), asi
que no se agrego una segunda lectura en cadena (decision de c0der, opcion (a)).

- **Produccion usa DynamoDB**: `NONCE_STORE_TABLE_NAME` = `aws_dynamodb_table.nonce_store` =
  `facilitator-nonces` (`terraform/environments/production/main.tf:949`).
- **Un despliegue con el store en memoria** (sin `NONCE_STORE_TABLE_NAME`; `create_nonce_store`
  lo avisa con un WARN la primera vez que se usa el store, que se inicializa perezoso)
  **pierde esta guarda al reiniciar**, igual que la de settle y la de Stellar.
- **Con el store caido el verify es fail-closed, igual que Stellar**: rechaza, no vouchea.

### NEAR: por que el nonce cubre "ya ejecutado"

El runtime ejecuta un delegate action solo si su nonce supera el de la access key, y al
ejecutarlo sube la key a ese nonce. El hash de la tx del relayer no existe hasta el settle; el
nonce de la key si. Por eso no hay lectura por hash.

**Finality `optimistic`** (decision de c0der): ve una liquidacion recien aterrizada ~2 bloques
antes que `final`. **Costo**: si un reorg raro revierte el bloque optimista que subio el nonce,
el verify rechaza un pago que seguia siendo valido y el comprador tiene que firmar de nuevo.

### Sui: un cliente, no dos

`check_balance` ya abria un `SuiClient` (handshake `rpc.discover`) en cada verify. Ahora `verify`
abre uno solo, con `request_timeout`, y lo comparte entre `check_inputs_current` y
`check_balance`. `settle` abre su cliente como antes (sin limite nuevo) y se lo pasa a
`check_balance`. **Consecuencia**: en el camino de verify, `check_balance` pasa del
`request_timeout` por defecto del SDK (60 s, `SuiClientBuilder::default`) a 10 s
(`RPC_REQUEST_TIMEOUT_SECS`).

### Sui: un RPC atrasado no es replay (ronda 2, decision de c0der)

El cliente arma el PTB contra su propio RPC; el del facilitador puede ir detras. La ejecucion
solo mueve un objeto hacia adelante, asi que:

| Lo que ve el RPC del facilitador | Veredicto del verify |
|---|---|
| misma version y mismo digest | sigue |
| version MAYOR que la referenciada | rechaza (`already used`) |
| misma version, otro digest | rechaza (`already used`) |
| `Deleted` | rechaza (`already used`) |
| version MENOR que la referenciada | **no rechaza**: log `warn` sin datos del pagador, la guarda queda en settle |
| `NotExists` | **no rechaza**: mismo tratamiento |
| error de lectura / sin respuesta | fail-closed (`ContractCall`) |

Las dos filas que no rechazan dan exactamente lo que daba 2.29.0: cero falsos rechazos por un
RPC atrasado, a cambio de no frenar en verify ese caso. Ronda 1 rechazaba la version menor
(medido por el refutador: `references version 5, chain holds version 4` -> `Err`, 2.29.0 ->
`Valid`).

## 2. Que ve un cliente

Paridad literal con Stellar (decision de c0der, opcion (a)): el rechazo por replay sale por
`FacilitatorLocalError::Other`, que `handlers.rs` responde **HTTP 400
`{"error":"internal_error (ref: <uuid>)"}`** — el mismo cuerpo que Stellar con un nonce usado. El
detalle ("... already used ...") queda solo en el log del servidor bajo esa ref.

Una falla de RPC en NEAR/Sui sale por `ContractCall` y la clasifica `ChainFailure::classify`:
los textos nuevos (`Failed to query access key: ...`, `Access key query timed out after Nms`,
`Failed to read Sui transaction inputs: ...`) no contienen ningun patron de transporte, rate
limit ni nonce EVM, asi que salen **400 `contract_call_failed (ref)`** sin `Retry-After`; solo si
el error del RPC trae `-32000`/`-32603`/`-32801`, `max retries exceeded` o `transport(` sale
**502 `upstream_rpc_unavailable`** con `Retry-After: 30` (leido en `src/chain/failure.rs`, no
provocado en vivo). El store caido de Algorand sale 400 `internal_error (ref)`, como Stellar.

**SDK Python `uvd-x402-sdk` 0.83.0, medido** (venv limpio; `FacilitatorError` construido con los
mismos argumentos que `X402Client.verify_payment` usa ante un no-200, `client.py:1136-1143`):

| Respuesta del facilitador | Excepcion | `status_code` | `retryable` | `is_transient_error` | `spent_nonce_evidence` |
|---|---|---|---|---|---|
| 400 `internal_error (ref)` (replay en las 4 redes no-EVM; store caido en Algorand/Stellar) | `FacilitatorError` | 400 | `False` | **`False`** | `None` |
| 400 `contract_call_failed (ref)` (RPC de NEAR/Sui que no contesta, caso general) | `FacilitatorError` | 400 | `False` | **`False`** | `None` |
| 503/502 con `Retry-After` (solo si el texto del RPC matchea transporte) | `FacilitatorError` | 503 | `True` | `True` | `None` |

Es decir: un servidor que usa `is_transient_error` trata el rechazo como final (402 al
comprador, que firma de nuevo), y no puede distinguir "ya usado" de otro 400 opaco
(`spent_nonce_evidence` = `None`). Eso es lo que resolveria la fila B1 del backlog.

## 3. Latencia agregada al verify

Una llamada mas por red. Conexion HTTPS persistente (como la de los providers), 1 llamada de
calentamiento + 20 medidas, desde la Mac mini el 2026-09-14
(`scratchpad/latency.py`, no versionado):

| Red | Llamada nueva | Endpoint medido | mediana | p90 | max | Referencia: llamada que el verify ya hacia |
|---|---|---|---|---|---|---|
| Algorand | DynamoDB `GetItem` (clave inexistente, `ProjectionExpression=expires_at`) | `facilitator-nonces`, Mac -> us-east-2 | 48 ms | 94 ms | 135 ms | `GET /v2/status` algonode: 100 ms mediana |
| NEAR | `view_access_key` optimistic | `free.rpc.fastnear.com` | 60 ms | 93 ms | 139 ms | ninguna: antes el verify de NEAR no hacia RPC |
| Sui | `sui_multiGetObjects` (2 ids) | `sui-rpc.publicnode.com` | 53 ms | 58 ms | 127 ms | `suix_getCoins` publicnode: 52 ms mediana |

Notas honestas:
- Desde ECS en us-east-2 el `GetItem` deberia ser menor que desde la Mac; el verify de Stellar ya
  paga esa misma lectura.
- El RPC de NEAR de produccion es el premium del secreto `facilitator-rpc-mainnet:near`, no medible
  desde aca. `rpc.mainnet.near.org` (publico) tardo ~3,1 s por llamada en frio, 6 de 6: si algun
  despliegue cae a ese endpoint, el verify de NEAR hereda esos 3 s.
- Sui no suma handshake: el cliente se comparte (seccion 1).

## 4. Tests

Modulo `replay_verify_tests` en cada archivo, escrito solo con API que ya existia en fcc26a3c
(`verify`, constructores, el `GLOBAL_NONCE_STORE`), para poder correr el MISMO modulo contra la
base. Los RPC son stubs locales (axum) con respuestas en la forma que parsean los clientes reales:
algod `GET /v2/status`; NEAR JSON-RPC `query/view_access_key`; Sui `rpc.discover`,
`suix_getCoins` y `sui_multiGetObjects` (serializados desde los tipos de `sui_json_rpc_types`).
Algorand usa un `NonceStore` con guion, instalado como store global del modulo.

Rojo: el modulo copiado tal cual sobre un `git archive fcc26a3c`
(`scratchpad/graft.py`, no versionado), `cargo test --locked -p x402-rs --features
solana,near,stellar,algorand,sui,xrpl --lib replay_verify -- --test-threads=1`.

| Test | fcc26a3c | rama |
|---|---|---|
| `algorand::replay_verify_tests::verify_rejects_a_group_settle_already_claimed` | **FAILED** (`Valid`) | ok |
| `algorand::replay_verify_tests::verify_fails_closed_when_the_nonce_store_cannot_be_read` | **FAILED** (`Valid`) | ok |
| `algorand::replay_verify_tests::verify_accepts_a_group_not_yet_settled` | ok | ok |
| `near::replay_verify_tests::verify_rejects_a_delegate_action_the_access_key_nonce_has_reached` | **FAILED** (`Valid`) | ok |
| `near::replay_verify_tests::verify_fails_closed_when_the_rpc_cannot_answer` | **FAILED** (`Valid`) | ok |
| `near::replay_verify_tests::verify_accepts_a_delegate_action_with_a_fresh_nonce` | ok | ok |
| `sui::replay_verify_tests::verify_rejects_a_transaction_whose_coin_moved_past_the_signed_version` | **FAILED** (`Valid`) | ok |
| `sui::replay_verify_tests::verify_rejects_a_transaction_whose_coin_no_longer_exists` (`Deleted`) | **FAILED** (`Valid`) | ok |
| `sui::replay_verify_tests::verify_rejects_a_transaction_whose_coin_has_another_digest_at_the_referenced_version` | **FAILED** (`Valid`) | ok |
| `sui::replay_verify_tests::verify_fails_closed_when_the_inputs_cannot_be_read` | **FAILED** (`Valid`) | ok |
| `sui::replay_verify_tests::verify_accepts_a_transaction_whose_inputs_are_current` | ok | ok |
| `sui::replay_verify_tests::verify_accepts_a_transaction_when_the_rpc_is_behind_the_referenced_version` | ok | ok |
| `sui::replay_verify_tests::verify_accepts_a_transaction_whose_coin_the_rpc_does_not_know` (`NotExists`) | ok | ok |
| `near::replay_verify_bound_tests::access_key_read_gives_up_at_its_timeout` | n/a (llama al helper nuevo) | ok |
| `sui::replay_verify_bound_tests::input_read_gives_up_at_its_timeout` | n/a (llama al helper nuevo) | ok |

Base (ronda 2): `5 passed; 8 failed`. Rama: `15 passed; 0 failed`.

Los cinco `accepts` pasan en ambos lados a proposito: prueban que la lectura nueva no rechaza un
pago sano (ni un RPC atrasado, en Sui), y que en la rama el stub contesta lo que el cliente real
espera. El stub de NEAR contesta `view_access_key` solo para la `public_key` con la que firmo el
pagador; medido con mutacion (el verify consulta la clave del relayer): `accepts` y `rejects` de
NEAR fallan, `2 passed; 2 failed`.

## 5. Como verificarlo en produccion despues del release

1. `curl -s https://facilitator.ultravioletadao.xyz/version` -> `{"version":"2.29.1"}`.
2. **NEAR (sin gastar)**: con una cuenta de testnet propia, firmar un delegate action de USDC con
   `nonce` <= el nonce actual de su access key (`view_access_key`) y mandar `POST /verify`
   (`network: near-testnet`). Esperado: 400 `internal_error (ref)`. Con `nonce` = actual + 1:
   200 `isValid: true`.
3. **Sui (sin gastar)**: firmar el PTB de USDC referenciando una version ANTERIOR de una coin
   propia (`sui_getObject` da la actual; cualquier tx previa sobre esa coin deja una vieja) y mandar
   `POST /verify` (`sui-testnet`). Esperado: 400. Con la version actual: 200 valido.
4. **Algorand (necesita un settle)**: liquidar un pago de testnet y reenviar el MISMO cuerpo a
   `POST /verify`. Esperado: 400.
5. Logs (el detalle vive bajo la ref; la CLI pagina, sumar las cuentas):
   ```bash
   S=$(( ($(date +%s) - 86400) * 1000 ))
   for p in '"already used"' '"Nonce store read failed during verify"' \
            '"Failed to query access key"' '"Access key query timed out"' \
            '"Failed to read Sui transaction inputs"'; do
     echo "$p: $(aws logs filter-log-events --log-group-name /ecs/facilitator-production \
       --region us-east-2 --start-time $S --filter-pattern "$p" \
       --query 'length(events)' --output text | paste -sd+ - | bc)"
   done
   ```
   `"Nonce store read failed during verify"` es el mismo texto que Stellar; el campo `group_id`
   lo distingue de `from`/`nonce`. Un pico de `Failed to query access key` o
   `Failed to read Sui transaction inputs` es el RPC de esa red, no pagos rechazados.

## 6. Pre-CI local

Ver la tabla del PR (mismos comandos que el job `test` de `ci.yaml`, mas fmt y clippy).

## 7. Backlog (fuera de alcance)

| # | Fila |
|---|---|
| B1 | Veredicto con nombre (p. ej. 200 `isValid:false`, `invalidReason: "nonce_already_used"`) para Stellar, Algorand, NEAR y Sui en vez del 400 `internal_error` opaco; cambia el cable de Stellar y el mapeo de `handlers.rs` (opcion (b) de c0der). |
| B2 | Las fallas de RPC del camino de verify en NEAR/Sui salen 400 `contract_call_failed` no reintentable salvo que el texto matchee transporte; clasificarlas como `upstream_rpc_unavailable` (502 + `Retry-After`). |
| B3 | Algorand: segunda senal en cadena (`GET /v2/transactions/pending/{txid}`) si algun despliegue corre con store en memoria (opcion (b) de c0der). |
| B4 | NEAR: `verify` no compara `max_block_height` del delegate action contra la altura actual. |
| B5 | NEAR: `settle` registra al receptor (`storage_deposit`, lo paga el facilitador) antes de saber si el runtime va a aceptar el delegate action; podria correr la misma lectura de nonce antes. |
| B6 | Sui: el cliente de `settle` (balance y `execute_transaction_block`) sigue con el `request_timeout` por defecto del SDK (60 s). |
| B7 | Algorand: `simulate_group` sigue sin llamadores. |
| B8 | `DynamoNonceStore::is_used` lee sin `consistent_read(true)`; afecta al verify de Algorand y al de Stellar. |
| B9 | El cliente de DynamoDB del nonce store no tiene `operation_timeout`. |
| B10 | NEAR: tratar `UnknownAccessKey` como falla de RPC (hoy es rechazo `Other`). |
