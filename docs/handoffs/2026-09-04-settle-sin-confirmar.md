# Un `/settle` que expira ya no tira el hash de la transaccion

**Fecha:** 2026-09-04
**Rama:** `0xultravioleta/x4-hash` (worktree `x4-hash`, WSL)
**Encargo:** `docs/handoffs/2026-09-04-paper-p0.md` seccion 3, item P1 (P0 del
backlog de ese mismo handoff). El dueno contesto "implementalo ya".
**Estado:** 3 commits. Suite en verde. **Cero push, cero deploy.**

---

## 1. El veredicto en cinco lineas

1. **El defecto era el que estaba escrito, y el diseno se implemento tal cual.**
   Variante nueva, `502` con `transaction` + `paymentId` + `retryable: false`,
   sitios de construccion por familia.
2. **El diseno subestimo el alcance: no era solo EVM.** Cinco familias tienen
   el patron -- EVM, Solana, Stellar, XRPL y Algorand -- y **tres de ellas
   tenian un segundo defecto encima**: contestaban `success: false` con
   `transaction: null`, que le dice al llamador que el pago **no ocurrio**.
   Eso es peor que tirar el hash: es afirmar algo falso.
3. **Solana necesitaba un corte fino que el diseno no menciona.** Su timeout
   tiene dos brazos y solo uno es "sin confirmar"; el otro es un fracaso
   definitivo cuyo consejo de reintentar es correcto. Convertir los dos habria
   dicho "no reintentes" sobre transacciones que no pueden aterrizar nunca.
4. **NEAR y Sui no tienen el patron.** Una sola llamada bloqueante que devuelve
   el hash solo al triunfar: en el punto de la falla no hay hash en mano.
   Detalle y el trabajo que costaria darselos, en §4.
5. **`retryable: false` se queda en `false`** y el `paymentId` sale de
   `dx402::payment_id`, la misma expresion del camino de exito. Un pago que
   despues aparezca confirmado lleva el mismo identificador.

---

## 2. Que se hizo

| Commit | Que |
|---|---|
| `495230bf` | La variante, el `502`, los cinco sitios de construccion, 5 tests, y los contratos publicados |
| `665d3ec6` | `VERSION` -> 2.14.0 |
| *este* | Handoff |

### 2.1 El defecto, medido

`src/chain/evm.rs`, el `Err` de `watcher.get_receipt()`. El comentario del
propio codigo ya lo decia -- *"TX may have been mined, retrying could
double-spend"* -- y aun asi devolvia `FacilitatorLocalError::ContractCall`, que
`IntoResponse` traduce a `contract_call_failed (ref: <uuid>)`. El uuid solo lo
puede resolver el facilitador leyendo sus propios logs.

Asi que el cliente quedaba con: un pago que **puede estar minado**, ningun hash,
ninguna forma de mirar la cadena. **Con nada que consultar, el unico movimiento
que le queda es el reintento** -- y re-firmar es una autorizacion nueva, con
nonce nuevo, perfectamente valida para la misma compra. El `authorizationState`
de EIP-3009 no la para (esto ya estaba medido: `2026-09-04-paper-p0.md` §5.5).
Ahi esta el gasto duplicado, completo.

Es ademas el unico punto donde "liquidacion enviada" y "liquidacion confirmada"
siguen siendo estados distintos **despues** de que la respuesta salio: `/settle`
es sincronico y en cualquier otro camino ya espero el recibo.

### 2.2 La forma

`FacilitatorLocalError::SettlementUnconfirmed(TransactionHash, Network)`
(`src/chain/mod.rs`). Se verifico que el unico `match` exhaustivo aguas abajo es
el `IntoResponse` de `handlers.rs`: no hay ningun `impl From<FacilitatorLocalError>`
en el repo y ni `crates/` ni `examples/` matchean sobre el enum.

```json
502 {"error":"settlement_unconfirmed",
     "transaction":"0x...","paymentId":"0x...","retryable":false}
```

Tipo propio (`types::SettlementUnconfirmedResponse`) y no `ErrorResponse`: es el
unico error del camino del dinero que **tiene que llevar datos**. Un
`{"error": "..."}` pelado aca es exactamente lo que dejaba al llamador sin poder
distinguir "el pago no ocurrio" de "el pago quiza ocurrio y no lo puedo nombrar".

**Sin `Retry-After`**, a diferencia del otro `502` (`upstream_rpc_unavailable`).
Son los dos unicos `502` de `/settle` y significan cosas opuestas; los tres
documentos publicados lo dicen con esas palabras y mandan a ramificar sobre
`error`, no sobre el status.

El `paymentId` se deriva llamando `crate::dx402::payment_id(network, &tx.to_string())`
-- **la misma expresion**, con el mismo argumento, que el `Serialize` de
`SettleResponse`. Una segunda derivacion sutilmente distinta es exactamente como
un puntero de evidencia deja de abrir; hay un test que ata las dos.

### 2.3 Los cinco sitios, uno por familia

`grep -rn "get_receipt()" src/chain/` no alcanza: cada familia espera a su
manera y solo EVM usa ese nombre. La enumeracion real salio de buscar
`with_timeout`, `wait_for_*`, `confirm_transaction*` y leer cada `settle`.

| Familia | Donde | Que cambio |
|---|---|---|
| EVM | `chain/evm.rs`, `Err` de `get_receipt()` | Cubre **todo** lo que pasa por `send_transaction_from`: EIP-3009, escrow, upto, ERC-8004 |
| Solana | `chain/solana.rs`, `send_and_confirm` | **Solo** el brazo donde la ultima lectura de estado falla (ver 2.4) |
| Stellar | `chain/stellar.rs`, `wait_for_transaction` | Agotar el poll **y** el error de lectura del RPC a mitad del poll |
| XRPL | `chain/xrpl.rs`, `wait_for_validation` | Agotar el poll de validacion |
| Algorand | `chain/algorand.rs`, `wait_for_confirmation` | `TransactionNotConfirmed` ahora lleva el `tx_id` |

**Stellar, XRPL y Algorand tenian un segundo defecto que el diseno no vio.** Sus
`settle` no propagaban el error: lo tragaban en `SettleResponse { success:
false, transaction: None }`. Eso nunca llega a `IntoResponse`, y le dice al
llamador con un `200` que el pago **no ocurrio** -- una afirmacion falsa, no una
omision. Ahora esos tres re-lanzan el error para que lo conteste el `502`.

**Stellar decodifica estricto.** El camino de exito rellena con ceros un hash
mas corto de 32 bytes (`if hash_bytes.len() >= 32`). Un hash de ceros nombraria
una transaccion inexistente en el unico error cuyo proposito entero es que lo
busquen, asi que si no decodifica limpio se degrada al error opaco de antes.
(El relleno con ceros del camino de exito **sigue ahi**: es un defecto aparte,
anotado en el backlog, y arreglarlo no era este encargo.)

### 2.4 Donde el diseno se quedaba corto, y que se implemento en su lugar

El despacho pedia parar y decirlo con archivo:linea. Un caso:

**`src/chain/solana.rs`, `send_and_confirm`.** Despues del timeout el codigo le
pregunta a la cadena una vez mas, y ese ultimo chequeo tiene **dos** salidas:

- `Ok(c) if !c.value` -- la cadena **si** contesto, y la ventana del blockhash
  ya paso. La transaccion no puede aterrizar nunca. Es un fracaso **definitivo**
  y su mensaje actual ("Retry with a fresh blockhash") es correcto.
- `Err(e)` -- no se pudo leer el estado. El texto ya decia *"It MAY still have
  settled -- check the signature on chain before retrying"*, o sea, ya era el
  caso sin confirmar... **en prosa que solo un humano parsea**, y sin la firma
  en un campo.

Convertir los dos brazos, que es lo que "el sitio de construccion de cada
familia" sugiere leido rapido, habria puesto `retryable: false` sobre
transacciones que **no pueden aterrizar**, diciendole a un cliente que no
reintente algo que solo se arregla reintentando. Se convirtio **solo el
segundo**. `send_and_confirm` toma ahora un `network` porque el wrapper de
transaccion no la tenia.

---

## 3. Verificacion

### 3.1 El test discriminante, y su rojo

`chain::evm::settlement_unconfirmed_tests::a_settle_that_never_confirms_returns_the_transaction_hash`
levanta un JSON-RPC falso (axum, puerto efimero) que acepta la transaccion y
contesta `null` a **todo** recibo, con `TX_RECEIPT_TIMEOUT_SECS=1`. Es el
timeout de verdad, no un error inyectado.

El test **ademas asegura `elapsed >= 1s`**, y eso es lo que lo hace honesto: un
fallo de transporte tomaria el mismo brazo del codigo y haria pasar el test sin
haber ejercitado nunca un timeout.

**Rojo sacando SOLO el sitio de construccion** (EVM vuelve a `ContractCall`):

```
running 5 tests
test chain::evm::settlement_unconfirmed_tests::a_settle_that_never_confirms_returns_the_transaction_hash ... FAILED
test handlers::settlement_unconfirmed_response_tests::a_contract_call_failure_still_carries_no_hash ... ok
test handlers::settlement_unconfirmed_response_tests::an_unconfirmed_settlement_answers_502_with_its_hash_and_payment_id ... ok
test handlers::settlement_unconfirmed_response_tests::each_chain_family_prints_its_own_hash_encoding ... ok
test handlers::settlement_unconfirmed_response_tests::the_payment_id_matches_the_one_a_successful_settle_would_print ... ok

panicked at src/chain/evm.rs:3315:22:
a broadcast transaction whose receipt never arrived must report SettlementUnconfirmed
with its hash, so the caller has something to look up on chain;
got Err(ContractCall("TxWatcher(Timeout)"))

test result: FAILED. 4 passed; 1 failed
```

`TxWatcher(Timeout)` es la prueba de que el timeout corrio de verdad.

**Rojo sacando SOLO el brazo de `IntoResponse`** (contesta como `ContractCall`):

```
running 5 tests
test chain::evm::settlement_unconfirmed_tests::a_settle_that_never_confirms_returns_the_transaction_hash ... ok
test handlers::settlement_unconfirmed_response_tests::a_contract_call_failure_still_carries_no_hash ... ok
test handlers::settlement_unconfirmed_response_tests::an_unconfirmed_settlement_answers_502_with_its_hash_and_payment_id ... FAILED
test handlers::settlement_unconfirmed_response_tests::each_chain_family_prints_its_own_hash_encoding ... FAILED
test handlers::settlement_unconfirmed_response_tests::the_payment_id_matches_the_one_a_successful_settle_would_print ... FAILED

an_unconfirmed_settlement_answers_502_with_its_hash_and_payment_id:
  assertion `left == right` failed
    left: 400
   right: 502

each_chain_family_prints_its_own_hash_encoding:
  assertion `left == right` failed: on base
    left: 400
   right: 502

the_payment_id_matches_the_one_a_successful_settle_would_print:
  assertion `left == right` failed
    left: Null
   right: String("0x411fe7c2e9a1b4fbecf94b48cc628d9c69c0752b90bfd313965a3607d322d466")

test result: FAILED. 2 passed; 3 failed
```

Las dos mitades caen por separado y en cada una **los controles quedan verdes**.
Un solo test no habria visto la segunda: el timeout puede producir el error
correcto y aun asi llegarle al cliente como `contract_call_failed`, que es
justo la forma que este cambio existe para sacar.

De regalo, ese `0x411fe7...` es el **mismo vector** que la sesion anterior pineo
contra un keccak256 independiente (pycryptodome) para la preimagen
`"eip155:8453" ++ "11"*32` (`2026-09-04-paper-p0.md` §2, item C). Los dos
caminos derivan lo mismo sin haberlo coordinado.

Los otros cuatro tests:

| Test | Que ata |
|---|---|
| `an_unconfirmed_settlement_answers_502_with_its_hash_and_payment_id` | El `502` y los cuatro campos, incluido `retryable: false` |
| `the_payment_id_matches_the_one_a_successful_settle_would_print` | El `paymentId` del error **es** el que imprimiria un settle exitoso, comparado contra un `SettleResponse` serializado de verdad |
| `each_chain_family_prints_its_own_hash_encoding` | Cada familia imprime su propia codificacion. Un base32 de Algorand escrito como hex `0x` es impegable en un explorador, y pegarlo es el remedio entero que ofrecemos |
| `a_contract_call_failure_still_carries_no_hash` | **Control.** El brazo vecino sigue opaco: si algun dia le crece un hash, un fracaso genuino empezaria a nombrar una transaccion que nunca existio |

### 3.2 Suite

```bash
CARGO_TARGET_DIR=$HOME/x4-hash-target cargo test --locked -p x402-rs \
  --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1
```

799 lib + 841 bin + 3 + 6 + 1 + 1 + 9 + 15 + 1 (integracion), **0 failed**.
`x402-axum` / `x402-reqwest` / `x402-compliance`: 31 + 1 + 15 + 5 + 5 + 10,
**0 failed**. `cargo clippy --all-targets`: **0 errores**. `cargo fmt --check`:
**0 diffs**.

Las dos unicas advertencias que rozan los archivos tocados -- `bad_request` sin
usar en `handlers.rs:4957` y `provider` sin usar en `stellar.rs` -- son
**preexistentes**, verificado contra `git show HEAD:`.

### 3.3 Contratos publicados

En el **mismo commit** que el codigo, como pide el despacho:

- `static/skill.md` -- el bloque nuevo en §4 (`POST /settle`) y dos filas en la
  tabla de errores de §9.
- `static/llms-full.txt` -- **generado**, no editado a mano
  (`./scripts/build_llms_full.sh`). Lo ata `handlers::llms_full_txt_is_in_sync`.
- `static/.well-known/agent-skills/index.json` -- el `digest` de `skill.md`.
  Lo ata `handlers::the_skills_index_digest_matches_skill_md`, que fue lo unico
  rojo de la suite completa hasta regenerarlo.
- `src/openapi.rs` -- el cuerpo del `502` en la descripcion de `POST /settle`
  y una entrada `502` en sus `responses`.
- `tests/x402/TROUBLESHOOTING.md` -- seccion propia con que hacer (buscar el
  hash) y que no hacer (reintentar), y por que.

Los tres documentos legibles dicen explicitamente que **los dos `502`
significan cosas opuestas** y que hay que ramificar sobre `error`.

---

## 4. Lo que NO tiene el patron, y por que

Se reviso familia por familia. **NEAR y Sui no lo tienen**, y no es que se
hayan salteado:

- **NEAR** (`src/chain/near.rs`, `submit_meta_transaction`): una sola llamada
  `broadcast_tx_commit`, que espera la ejecucion y devuelve el hash **dentro de
  la respuesta**. Si la llamada falla, no hubo hash. (Aparte: su `settle`
  tampoco propaga -- traga todo en `success: false`, igual que hacian Stellar,
  XRPL y Algorand.)
- **Sui** (`src/chain/sui.rs`): igual, `execute_transaction_block` en una sola
  llamada y el digest sale de la respuesta.

En las dos, el hash **es derivable del lado del cliente antes de mandar**
(`SignedTransaction::get_hash()` en NEAR, el digest de la `Transaction` en Sui),
asi que se podria calcular y guardar antes del envio. **No se hizo**: es
trabajo de otra forma -- calcular una identidad y llevarla -- y no el brazo de
timeout que este encargo describe. Queda en el backlog.

Dos sitios mas que quedaron afuera, con motivo:

- **`chain/solana.rs`, el barrido de la settlement account**
  (`send_and_confirm_transaction_with_spinner_and_config`): un error ahi puede
  ser un rechazo de preflight (nunca se difundio) o un timeout, y el codigo
  **hoy no los distingue**. Afirmar `settlement_unconfirmed` sobre un preflight
  rechazado seria decirle a alguien que no reintente un pago que nunca salio.
  Distinguirlos primero, convertir despues.
- **Los caminos de escrow / `payment_operator`**: pasan por
  `send_transaction_from`, asi que **ya heredan** la variante en EVM, pero
  varios de ellos re-envuelven el error en `OperatorError` y lo clasifican
  aparte (`handlers.rs`). No se verifico cada uno de esos re-envolvimientos:
  puede que alguno vuelva a aplastar el hash a string. Anotado.

---

## 5. Backlog

| Date | Item | Context | Priority | Status |
|---|---|---|---|---|
| 2026-09-04 | Escrow / `payment_operator` pueden re-aplastar `SettlementUnconfirmed` | Heredan la variante desde `send_transaction_from`, pero re-envuelven en `OperatorError`. Sin verificar uno por uno | P1 | Nuevo |
| 2026-09-04 | El barrido de settlement account en Solana no distingue preflight de timeout | `chain/solana.rs`. Hasta distinguirlos no se le puede dar el hash | P2 | Nuevo |
| 2026-09-04 | NEAR y Sui podrian llevar el hash calculandolo antes de enviar | `SignedTransaction::get_hash()` / digest de la `Transaction`. Otra forma de trabajo, no un brazo de timeout | P2 | Nuevo |
| 2026-09-04 | `wait_for_transaction` de Stellar rellena con ceros un hash corto en el camino de EXITO | `if hash_bytes.len() >= 32`. Emitiria un hash de ceros como si fuera la transaccion | P2 | Nuevo |
| 2026-09-04 | `settle` de NEAR traga todo error en `success: false` | Igual que hacian Stellar/XRPL/Algorand antes de este commit | P2 | Nuevo |
| 2026-09-04 | SDKs: py/npm deben entender el `502 settlement_unconfirmed` | Un cliente que trate todo `502` como reintentable ahora **gasta dos veces**. Es lo mas urgente que deja este cambio | **P0** | Nuevo |
| 2026-09-04 | Recibo firmado de PAGO (monto/activo/recurso) | Heredado. `2026-09-04-paper-p0.md` §3 P2 | P1 | Pendiente |
| 2026-09-04 | `paymentId` en `TransactionRecord` | Heredado. Derivable en el camino de lectura, sin migrar | P1 | Pendiente |
| 2026-09-04 | Llenar `error` en `/accepts` cuando se descarto todo | Heredado del 2026-09-03. Decision de Saul | P1 | Pendiente |
| 2026-09-04 | SDKs: py/npm deben leer el `invalidReason` nuevo | Heredado | P1 | Sin verificar |

---

## 6. Como reproducir

```bash
cd /mnt/c/Users/lxhxr/orca/workspaces/x402-rs/x4-hash

# El worktree lo creo el git de Windows: sin esto, git status muestra cientos
# de archivos modificados y no sirve para nada.
git config core.autocrlf true
git status --short              # limpio salvo 3 sin trackear

CARGO_TARGET_DIR=$HOME/x4-hash-target cargo test --locked -p x402-rs \
  --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1

# los tests de esta sesion
... --bin x402-rs settlement_unconfirmed -- --test-threads=1   # 5
```

`CARGO_TARGET_DIR` en `$HOME` (ext4): sobre `/mnt/c` por 9P la compilacion es
varias veces mas lenta. `scripts/build_llms_full.sh` corrio tal cual esta vez
(el archivo esta en LF en este worktree).

---

## Para c0der

**Que hice.** Tres commits en `0xultravioleta/x4-hash`. `495230bf` es el
cambio entero -- la variante `SettlementUnconfirmed`, el `502` con
`transaction` + `paymentId` + `retryable:false`, los cinco sitios de
construccion, los 5 tests y **los contratos publicados en ese mismo commit**
(`skill.md`, `llms-full.txt` regenerado, el digest del indice de skills,
`openapi.rs`, `TROUBLESHOOTING.md`). `665d3ec6` es `VERSION` -> **2.14.0**
(produccion sirve 2.13.0, verificado con `curl`). Este es el handoff. Suite,
clippy y fmt en verde, **cero push, cero deploy, cero terraform**.

**Que encontre que el diseno no decia.** Dos cosas.

La primera: **no era solo EVM, eran cinco familias, y en tres de ellas el
defecto era peor de lo descrito.** Stellar, XRPL y Algorand no solo tiraban el
hash: contestaban `200` con `success: false` y `transaction: null`, que le dice
al llamador que **el pago no ocurrio**. Tirar el hash es una omision; afirmar
que no paso es una mentira sobre el camino del dinero. Esos tres ahora propagan
el error.

La segunda: **Solana necesitaba un corte que el diseno no contempla.** Su
timeout tiene dos brazos y solo uno es "sin confirmar". El otro -- la cadena
contesta y la ventana del blockhash ya paso -- es un fracaso definitivo donde
reintentar **si** es lo correcto. Convertir los dos habria puesto
`retryable: false` sobre transacciones que no pueden aterrizar nunca. Se
convirtio solo el brazo donde no se pudo leer el estado.

NEAR y Sui **no** tienen el patron y no se tocaron: una sola llamada que
devuelve el hash solo al triunfar, asi que en la falla no hay hash en mano. El
hash es derivable antes de enviar en las dos, pero eso es otra forma de trabajo;
esta en el backlog.

**Lo que hay que decidir.** El backlog tiene un **P0 nuevo que no es de este
repo**: los SDKs (`uvd-x402-sdk` npm y PyPI) tienen que entender el `502`
nuevo. Un cliente que hoy trata todo `502` como reintentable -- que es lo
razonable, porque el otro `502` del facilitador **lleva `Retry-After`** -- ahora
**gasta dos veces** exactamente en el caso que este cambio existe para evitar.
El facilitador quedo bien y el riesgo se mudo aguas abajo. Eso deberia salir
antes o junto con el deploy de 2.14.0, no despues.
