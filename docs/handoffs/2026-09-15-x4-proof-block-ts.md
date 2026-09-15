# La prueba de pago ERC-8004 lleva el timestamp del bloque — handoff 2026-09-15

- **Rama**: `0xultravioleta/x4-proof-block-ts` (desde `origin/main` 3384af25, 2.29.5)
- **VERSION**: 2.29.5 -> 2.29.6
- **Que cambia**: `create_proof_of_payment` (`src/chain/evm.rs`) arma el `timestamp` de la
  `ProofOfPayment` con el del bloque que mino el pago, no con el reloj del facilitador. Si no lo
  puede obtener, no emite prueba.
- **Que no cambia**: el verificador (`src/erc8004/proof.rs`), el settle sin la extension
  `8004-reputation`, Solana (sigue con `proof_of_payment: None`) y los demas caminos de settle.
  Ningun otro camino emite `proof_of_payment: Some`.

## 1. El defecto

En 3384af25, `create_proof_of_payment` corria despues de `receipt.status()` ok con:

- `block_number = receipt.block_number.unwrap_or(0)` (`src/chain/evm.rs:1833`)
- `timestamp = SystemTime::now()` (`:1835`, comentario: *Use current timestamp as fallback (block
  timestamp requires additional RPC call)*)

`verify_payment_facts` (`src/erc8004/proof.rs:514-540`) lee `eth_getBlockByNumber(proof.block_number)`
y exige `block_ts == proof.timestamp`; si no, `proof_timestamp_mismatch`. El reloj del facilitador
casi nunca coincide al segundo con el bloque, asi que la prueba que devolvia el propio settle
fallaba su propia verificacion. `payment_hash` no cubre `timestamp`, por eso el verificador no
puede confiar en el campo y lo contrasta con la cadena: el lado equivocado era el emisor.

## 2. Medicion: donde esta el timestamp del bloque

Tipo que usa el repo: `alloy::rpc::types::TransactionReceipt` (alloy-rpc-types-eth 1.7.3). El
recibo **no tiene** campo de timestamp; cada `Log` tiene `block_timestamp: Option<u64>`
(`blockTimestamp`, execution-apis #295), que el nodo puede incluir o no.

Medido el 2026-09-15 contra RPC publicos, sin claves: un recibo con logs de un bloque reciente
(`eth_getBlockByNumber latest` o `eth_getLogs` del topic `Transfer`) y el header de ese bloque.

| Red | Endpoint | `blockTimestamp` en el recibo | en los logs | igual al header |
|---|---|---|---|---|
| base | `mainnet.base.org` | no | si | si |
| base-sepolia | `sepolia.base.org` | no | si | si |
| optimism | `mainnet.optimism.io` | no | si | si |
| arbitrum | `arb1.arbitrum.io/rpc` | no | si | si |
| ethereum | `ethereum-rpc.publicnode.com` | no | si | si |
| polygon | `polygon-bor-rpc.publicnode.com` (`polygon-rpc.com` respondio 401) | no | si | si |
| celo | `forno.celo.org` | no | si | si |
| bsc | `bsc-dataseed.bnbchain.org` | no | si | si |
| unichain | `mainnet.unichain.org` | no | si | si |
| monad | `rpc.monad.xyz` | no | si | si |
| hyperevm | `rpc.hyperliquid.xyz/evm` | no | si | si |
| avalanche | `api.avax.network`, `avalanche-c-chain-rpc.publicnode.com` | no | **no** | — |
| scroll | `rpc.scroll.io`, `scroll-rpc.publicnode.com` | sin logs `Transfer` en la muestra | | |
| skale-base | `skale-base.skalenodes.com/v1/base` | sin logs `Transfer` en la muestra | | |

Conclusion: en la mayoria de las redes el dato ya viene en el recibo y no hace falta RPC, pero no
en todas (Avalanche, que tiene ERC-8004), y los endpoints premium de produccion no se pudieron
medir. Por eso hace falta una lectura del bloque como respaldo.

## 3. El cambio

`create_proof_of_payment` pasa a `async` y recibe el provider del settle (`self.inner()`).

1. Las mismas compuertas de antes, **antes** de cualquier RPC: extension presente,
   `include_proof`, red con ERC-8004. Un settle sin la extension no hace ninguna llamada nueva.
2. `receipt.block_number` ausente -> `None` (antes: una prueba con bloque 0).
3. `proof_block_timestamp`:
   - el primer `log.block_timestamp` de un log del mismo bloque del recibo -> sin RPC;
   - si no hay, **un** `eth_getBlockByNumber(block_number)` dentro de
     `tokio::time::timeout(PROOF_BLOCK_READ_TIMEOUT)`, 2 s;
   - error, `null` o timeout -> `None`.
4. Cualquier `None` se devuelve como `proof_of_payment: None` y el settle responde
   `success: true` con su hash, igual que antes. La funcion devuelve `Option`, no `Result`: no
   hay camino por el que la lectura haga fallar el settle.

**Reintentos**: el `RpcClient` del `EvmProvider` tiene `RetryBackoffLayer` (3 reintentos, 200 ms
iniciales), que reintenta rate limits y el mensaje `header not found`
(`alloy-json-rpc 1.7.3`, `ErrorPayload::is_retry_err`). El timeout envuelve la llamada entera, asi
que esos reintentos corren dentro de los 2 s y no alargan el settle mas alla del tope. No se
agrego ningun reintento propio.

**Logs** (`warn`, sin datos del pagador): `ERC-8004 proof of payment omitted: ...` con `tx`,
`block`, `network` y, en el caso de error, el error pasado por `redact::scrub_urls`.

| Mensaje | Causa |
|---|---|
| `the receipt names no block` | recibo sin `blockNumber` |
| `the node did not return the block` | `eth_getBlockByNumber` -> `null` |
| `the block read failed` | error de RPC (URL borrada del texto) |
| `the block read timed out` | 2 s sin respuesta, `timeout_ms=2000` |

**Costo por settle con la extension**: 0 llamadas si el nodo pone `blockTimestamp` en los logs; 1
llamada, tope 2 s, si no.

## 4. Tests

Modulo `proof_of_payment_tests` al final de `src/chain/evm.rs`. Los seis tests de settle usan
solo API que existia en 3384af25 (`EvmProvider::try_new`, `Facilitator::settle`,
`verify_payment_facts`), asi que el mismo modulo corre contra la base. El RPC es un stub local
(axum) que contesta lo que un settle legacy pide (`eth_chainId`, `eth_getTransactionCount`,
`eth_gasPrice`, `eth_estimateGas`, `eth_call` de `balanceOf`, `eth_sendRawTransaction`,
`eth_getTransactionReceipt`) y `eth_getBlockByNumber`. `eth_blockNumber` queda fijo 10 bloques
debajo del bloque del pago: el heartbeat del watcher lee solo esos, asi que cada lectura del
bloque del pago que cuenta el stub es de la prueba. La prueba que devuelve el settle se verifica
con `verify_payment_facts` contra el mismo stub. El timestamp del bloque es `ahora - 30 s`, para
que no pueda coincidir con el reloj.

Rojo: el modulo, con una constante temporal `PROOF_BLOCK_READ_TIMEOUT` local para compilar,
sobre el `evm.rs` de 3384af25, `cargo test --locked -p x402-rs --features
solana,near,stellar,algorand,sui,xrpl --lib proof_of_payment_tests -- --test-threads=1`.

| Test | 3384af25 | rama |
|---|---|---|
| `the_proof_a_settle_emits_passes_the_proof_verifier` (logs sin `blockTimestamp`) | **FAILED**: `Err(TimestampMismatch)` | ok |
| `a_receipt_whose_logs_carry_the_block_timestamp_costs_no_block_read` | **FAILED**: `Err(TimestampMismatch)` | ok |
| `a_settle_that_asks_for_no_proof_reads_no_block` | ok | ok |
| `a_block_read_that_fails_settles_without_a_proof` (`-32603`) | **FAILED**: prueba emitida | ok |
| `a_node_without_the_block_settles_without_a_proof` (`null`) | **FAILED**: prueba emitida | ok |
| `a_block_read_that_hangs_holds_the_settle_no_longer_than_its_bound` (60 s sin respuesta) | **FAILED**: prueba emitida | ok: `>= 2 s` y `< 7 s` |
| `a_receipt_without_a_block_number_yields_no_proof` | n/a (llama a la firma nueva) | ok |

Base: `1 passed; 5 failed`. Rama: `7 passed; 0 failed` (2,07 s; el test que cuelga pesa 2 s).

`a_settle_that_asks_for_no_proof_reads_no_block` pasa en ambos lados a proposito: fija que un
settle sin la extension no paga la lectura nueva.

El error del test de falla es `-32603 internal error` a proposito: `header not found` lo reintenta
la capa de alloy y el conteo de lecturas dejaria de ser 1. Ese caso lo cubre el tope (test que
cuelga).

## 5. Mutantes

Cada mutante sobre el `evm.rs` de la rama, restaurado despues (sha256 verificado), mismo comando
de la seccion 4.

| Mutante | Resultado | Lo mata |
|---|---|---|
| M1: `timestamp` vuelve a `SystemTime::now()` | `2 passed; 5 failed` | el ida y vuelta y el de logs (`Err(TimestampMismatch)`); falla, `null` y cuelgue (prueba emitida) |
| M2: `receipt.block_number.unwrap_or(0)` | `6 passed; 1 failed` | `a_receipt_without_a_block_number_yields_no_proof`: `Some(ProofOfPayment { block_number: 0, .. })` |
| M3: error de RPC propagado al settle (si la prueba pedida no sale, `settle` devuelve `Err(ContractCall)`) | `4 passed; 3 failed` | falla, `null` y cuelgue: `the transfer is confirmed on chain, so the settle must succeed; got ContractCall(..)` |
| M4: lectura del bloque sin tope propio (timeout de 1 h) | `6 passed; 1 failed` | el de cuelgue: `the settle waited 10.005930208s on a proof it does not need` |
| M5: ignorar `blockTimestamp` de los logs (siempre leer el bloque) | `6 passed; 1 failed` | `a_receipt_whose_logs_carry_the_block_timestamp_costs_no_block_read`: 1 lectura, esperada 0 |

M4 tardo 10 s y no los 60 del stub: sin el tope propio, lo que corta es el timeout de request del
cliente HTTP (`RPC_REQUEST_TIMEOUT_SECS`, 10 s por defecto). Ese es el tope que tenia un settle con
la extension si solo se agregaba la lectura; `PROOF_BLOCK_READ_TIMEOUT` lo baja a 2 s.

## 6. Pre-CI local

Los comandos del job `test` de `ci.yaml`, mas fmt y clippy, con target dir propio del worktree y
`CARGO_BUILD_JOBS=4`.

| Comando | Resultado |
|---|---|
| `cargo fmt --all -- --check` | ok (despues de `cargo fmt --all`, que solo toco `src/chain/evm.rs`) |
| `cargo clippy --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl --all-targets` | sin hallazgos en el codigo nuevo. El unico en la zona es `useless_conversion` sobre `TokenAmount::from(payment.value)`, linea que ya estaba en 3384af25. CI no corre clippy |
| `python3 scripts/verify_landing_canonical.py --offline` | `[OK]` |
| `cargo build --locked --features solana,near,stellar,algorand,sui,xrpl` | ok |
| `cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1` | lib 1138 ok (1 ignored), bin 1187 ok (1 ignored), integracion 64 ok, doctests 1 ok (11 ignored); 0 fallos |
| `cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance -- --test-threads=1` | unit e integracion 99 ok, doctests 10 ok (9 ignored); 0 fallos. No los toca el diff; CI los corre igual |
| `.githooks/pre-commit` sobre el diff staged | ok |
| `git merge-tree --write-tree origin/main <commit>` | limpio (exit 0, sin conflictos) contra `origin/main` 3384af25 |

## 7. Como verificarlo en produccion despues del release

1. `curl -s https://facilitator.ultravioletadao.xyz/version` -> `{"version":"2.29.6"}`.
2. Un settle en **testnet** (`base-sepolia`, fondos de testnet) con
   `paymentRequirements.extra = {"8004-reputation": {}}`: la respuesta trae `proofOfPayment` con
   `timestamp` igual al del bloque de `blockNumber` (`eth_getBlockByNumber` en un explorador o RPC
   publico).
3. Esa prueba en `POST /feedback` (con `rater` = pagador): el log del gate de prueba reporta el
   veredicto verificado, no `proof_timestamp_mismatch`.
4. Logs, 24 h (la CLI pagina: sumar las cuentas):
   ```bash
   S=$(( ($(date +%s) - 86400) * 1000 ))
   for p in '"ERC-8004 proof of payment omitted"' '"Created ERC-8004 ProofOfPayment"' \
            '"proof_timestamp_mismatch"'; do
     echo "$p: $(aws logs filter-log-events --log-group-name /ecs/facilitator-production \
       --region us-east-2 --start-time $S --filter-pattern "$p" \
       --query 'length(events)' --output text | paste -sd+ - | bc)"
   done
   ```
   `omitted` sostenido en una red es su RPC sin `blockTimestamp` en los logs y con la lectura del
   bloque fallando o venciendo, no pagos fallidos: el settle ya respondio `success: true`.

## 8. Riesgos y como revertir

- Un settle con la extension en una red cuyo nodo no pone `blockTimestamp` suma una lectura, hasta
  2 s. Sin la extension: nada.
- Un nodo balanceado que todavia no tiene el bloque recien minado contesta `null`: sin prueba en
  ese settle (antes: prueba invalida). No se reintenta a proposito.
- Revertir: revertir el commit; la unica superficie es `create_proof_of_payment` y su llamada.

## 9. Backlog (fuera de alcance)

| # | Fila |
|---|---|
| P1 | Si `omitted` resulta frecuente en alguna red, reintentar una vez el `null` dentro del mismo tope de 2 s. |
| P2 | El emisor y el verificador leen el bloque por numero; con un reorg la prueba nombra un bloque que ya no contiene la transaccion y el verificador la rechaza (`proof_block_mismatch`). Leer por `blockHash` en el emisor no lo arregla del lado del verificador. |
| P3 | Solana sigue sin prueba (`proof_of_payment: None`). |
