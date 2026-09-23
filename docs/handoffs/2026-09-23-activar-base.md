---
date: 2026-09-23
tags:
  - type/handoff
  - domain/receipts
  - domain/base
status: active
---

# Base entra al riel de recibos del facilitador (2.40.0)

**Base:** `afa3706d` (2.39.5, #103). **Rama:** `c0der/activar-base`. **Publica
`2.40.0`**: minor, porque cambia la respuesta observable de `/verify` y `/settle`
para los pagos `exact` en Base.

## Lo medido antes de tocar código (sólo lectura)

CloudWatch Logs Insights sobre `/ecs/facilitator-production` (us-east-2), ventana
2026-09-16 15:43Z a 2026-09-23 15:43Z. Las líneas llevan la red y la ruta como
campos (`network=`, `uri=`); se contaron con `parse` sobre esos campos, no sobre
texto libre.

| Qué | 7 días |
|---|---|
| `Nonce error detected, retrying`, todas las redes | **4** |
| … en Base | **4**, las cuatro en `POST /feedback/evm/submit` el 2026-09-17 |
| … en Base, `POST /settle` | **0** |
| `Transaction submitted to mempool` en Base, `POST /settle` (el denominador) | **9.576** (entre 3, el 2026-09-21, y 2.728, el 2026-09-17, por día completo) |
| Las otras dos líneas de error de nonce (`… TX count advanced …`, `… probe was unavailable …`) | 0 |

A 30 días (2026-08-24 a 2026-09-23): 5.151 líneas, 5.032 de ellas en Arbitrum el
2026-08-31 y el 2026-09-01; en Base, 1 en `/settle` el 2026-08-27, 1 el 2026-09-12
y las 4 de `/feedback/evm/submit`. En Base no llega a un reintento por día.

## Qué cambia

### 1. Base admitida (`src/receipts/mod.rs`)

- `supported()` suma `Network::Base`; `capability()` agrega `eip155:8453` al final
  de `networks` (las posiciones de las cuatro que ya estaban no cambian).
- La descripción OpenAPI de `POST /verify` y `POST /settle` nombra Base.
- Base Sepolia y el resto de las redes, igual. Las requests de Base que van a otro
  camino de liquidación (`upto`, `escrow`/`commerce`, `fhe-transfer`, la extensión
  `refund`) siguen sin entrar a la admisión.

### 2. Un error de nonce bajo admisión no firma una segunda transacción (`src/chain/evm.rs`)

Revisando la doc contra el código apareció esto. Bajo una admisión, el bucle de
reintento de `send_transaction_from`, después de que el nodo rechaza por nonce los
bytes guardados, pedía otro nonce al `PendingNonceManager`, llenaba y firmaba otra
transacción, y `receipts::prepared_evm` la rechazaba
(`receipt_multiple_transactions_not_supported`). Ese nonce nunca se emitía ni se
devolvía (el `?` sale antes de `release_nonce`). Si el nodo acepta la siguiente
transacción del mismo firmante en su cola de futuras, esta queda detrás del hueco
hasta que la resincronización por deriva (`NONCE_TRUST_CHAIN_AFTER_DRIFT`, 300 s)
lo cura; si la rechaza con `nonce too high`, el facilitador se resincroniza en el
acto. El camino existe hoy en Arc (sin ocurrencias en 30 días); con el volumen de
Base pesa más.

Ahora, bajo admisión, un error de nonce devuelve el error del nodo en el primer
intento, sin firmar nada más. Fuera de una admisión el reintento no cambia.
`docs/facilitator-receipts.md` ya decía "no second transaction is signed for that
admission"; ahora el código lo cumple. Queda en este PR, en su propio commit, con
tres condiciones:

**a. Sin hueco de nonce.** Después del rechazo, el gestor de nonce del firmante
obliga a la siguiente asignación a preguntarle al nodo (`reset_nonce`, como
antes). Si el rechazo dice que nuestro nonce va por delante del nodo (`too high`,
o una redacción con `gap`), además descarta la marca alta (`resync_to_chain`), así
la siguiente asignación toma la cuenta del nodo tal cual, sin esperar los 300 s de
deriva. `too high` ya lo hacía; `gap` es nuevo y sólo bajo admisión
(`is_nonce_gap`). Test: un nodo simulado con estado rechaza el primer envío y
cambia su cuenta; el siguiente settle del mismo firmante, enseguida, sale con el
nonce del nodo en los tres casos (`too low` 0→5, `too high` 3→0, `gap` 3→0).

**b. Qué contesta el facilitador y qué hace el comprador.** Lo fija
`a_nonce_refusal_under_admission_stays_unknown_and_is_resent_not_resigned`
(`src/receipts/tests.rs`):

- `502`, `Retry-After: 30`, `Cache-Control: no-store`, cuerpo
  `{"error": "upstream_nonce_or_mempool (ref: …)", "receipt": {…}}` sin
  `success` ni `retryable`; recibo `unknown`, `settlement.id` = la transacción
  guardada, `retry.action: poll`;
- `/verify` con la misma compra → `isValid: true` con ese recibo, sin simular;
- `/settle` con la misma compra → el mismo `502` con `Idempotent-Replayed: true`
  y nada enviado;
- sin esa compra → `409 receipt_request_conflict`, sin recibo.

Medido con los SDKs publicados, instalados desde PyPI/npm en un directorio
temporal, contra un facilitador simulado local que devuelve exactamente eso (nada
sale de la máquina: en el arnés TS, ethers y `fetch` rechazan toda URL que no sea
127.0.0.1, y hubo 0 intentos). Tres llamadas: la compra y dos reanudaciones con el
mismo contexto.

| SDK | Vendedor | Comprador | Autorizaciones que vio el facilitador | ¿402? |
|---|---|---|---|---|
| Python 0.90.1 (mapeo de errores de `FastAPIX402`, `fetch_with_receipt`) | `503`, `Retry-After: 30`, recibo en `PAYMENT-RESPONSE` | `payment_state = unknown`, recibo `unknown`, `retry.action: poll` | 1 | no |
| TypeScript 2.98.0 (`createPaymentMiddleware` en Express, `fetchWithReceipt`) | `503`, `Retry-After: 15`, recibo en `PAYMENT-RESPONSE` | `paymentState = unknown`, recibo `unknown` | 1 | no |

El `FastAPIX402` de 0.90.1 emite un 402 en formato propio, sin `accepts`, que el
`fetch` del mismo SDK no lee (`NoAcceptablePaymentError`). El vendedor del arnés
usa por eso el desafío v2 de `payment_required_response_v2` y, para el pago, las
mismas `_process_payment` y `_payment_error` del integrador. Es una fricción del
SDK, ajena a este cambio.

**c. Coherencia con #102** (`c0der/emitido-no-retryable`, 2.39.6, abierto). #102
marca `retryable: false`, sin `Retry-After` y con `transaction`, toda falla del
riel de recibos después del latch o con bytes preparados; este caso lo es. El
código y la doc dicen lo mismo con otras palabras: el nodo rechazó los bytes,
este envío no los encoló, pero el riel no sabe si están en otro lado, así que
decide el recibo: reenviar la misma request o consultar el recibo, nunca firmar
otra. Repetí la medición con el cuerpo en la forma de #102: los dos vendedores
contestan `500` sin `Retry-After` en vez de `503`, y el comprador termina igual
(`unknown`, 1 autorización, ningún 402). Si #102 entra antes que este push, se
rebasea: su `evm.rs` hace fill + `send_raw_transaction` siempre y trata
`already known` como `settlement_unconfirmed`, así que ese caso ya no llegaría a
esta rama.

### 3. Tests

- `base_is_announced_and_admitted`: `supported(Base)`, `capability()`
  con `eip155:8453`, y un settle con key admitido una vez y repetido con
  `Idempotent-Replayed`.
- Sin override para Base: `TEST_CANDIDATE` sale del código, porque ya no hay
  candidata. El test de concurrencia, el de rieles alternativos, el fixture de
  DynamoDB local y los replays de `admitted()` pasan ahora por el `supported()` de
  producción, y `admitted()` falla si la red que recibe no está admitida.
- `under_a_receipt_admission_a_nonce_refusal_signs_no_second_transaction`: bajo
  admisión, un rechazo `nonce too low` da un solo `eth_sendRawTransaction`, el
  error del nodo como respuesta y los bytes guardados.
- `after_a_nonce_refusal_under_admission_the_next_settle_takes_the_nodes_nonce`:
  condición (a), los tres casos.
- `outside_an_admission_a_nonce_refusal_is_still_retried`: el mismo nodo, sin
  admisión: tres envíos, como antes.
- `a_nonce_refusal_under_admission_stays_unknown_and_is_resent_not_resigned`:
  condición (b), del lado del facilitador.

### 4. Docs

- `docs/facilitator-receipts.md`: Base en la intro, sección "Base" con la tabla
  antes/después, y el alcance de validación. La tabla se contrastó fila por fila
  con el código de hoy (2.39.5 más este cambio):
  - con el mismo `Idempotency-Key`, 2.39 SÍ repite el 200 cacheado durante 24 h
    (caché legacy), no "se vuelve a correr";
  - con `X-UVD-Purchase`, 2.39 contesta `400 receipt_request_not_supported` en
    Base (fila nueva, y separada de la de `Idempotency-Key`);
  - fila nueva de 2.39.3: liquidación que termina antes de enviar nada → `503` con
    `Retry-After`, recibo `rejected`/`reservation_abandoned`, readmisión;
  - fila nueva: rechazo por nonce;
  - `503 receipt_store_unavailable` ahora con `Retry-After` y `safeToRetry: true`.
- `/index.md`, `/skill.md`, `/mcp.md`, README: Base en la sección de recibos.
  `llms-full.txt` regenerado con `scripts/build_llms_full.sh` y el digest de `skill.md` en `/.well-known/agent-skills/index.json`
  calculado con `shasum -a 256` sobre `skill.md` con LF, lo mismo que exige
  `the_skills_index_digest_matches_skill_md` (no hay script para el digest).
- `/health/ready` y la portada no muestran recibos para ninguna red; Base aparece
  en las dos igual que Arc y Hedera (entrada por red en `/health/ready`, tarjeta en
  la grilla). Sin cambios ahí.
- `VERSION` 2.39.5 → 2.40.0 y `CHANGELOG.md` (el de la raíz) `[2.40.0] - 2026-09-23`,
  con 2.39.1 a 2.39.5 intactas debajo. La rama se rebaseó sobre #103 (2.39.5) antes
  del push.

**No cambia:** `GET /receipts/{id}`, el store y su forma (sin migración), IAM, el
riel legacy de `Idempotency-Key`, las redes sin recibos.

## Lo verificado

Checkout con LF, macOS, Rust estable local. Código probado: `5684b6d0` (los dos
commits de código ya rebaseados sobre #103); el tercer commit sólo agrega este
handoff. Nada contra producción: ni `/verify`, ni `/settle`, ni RPC.

| Job o paso | Comando | Resultado |
|---|---|---|
| Qué dispara el diff | filtros `paths` de `.github/workflows/` contra `origin/main` | `ci.yaml` (job `test`; `plan`/`deploy` necesitan AWS) y `no-account-id.yml` |
| Build & test: portada | `python3 scripts/verify_landing_canonical.py --offline` | `[OK]` |
| Build & test: frontend | `node --test tests/frontend-capabilities.test.cjs` | 14/14 pass |
| Build & test: balances | `python3 -m unittest discover -s tests/scripts -p 'test_*balances.py'` | 5 tests, OK |
| Build & test: build | `cargo build --locked --features solana,near,stellar,algorand,sui,xrpl,hedera` | exit 0 |
| Build & test: x402-rs | `cargo test --locked -p x402-rs --features <las mismas> -- --test-threads=1` | exit 0, **2714 passed, 0 failed**, 31 ignored (11 suites) |
| Build & test: crates | `cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance -- --test-threads=1` | exit 0, 109 passed, 0 failed, 9 ignored |
| `no-account-id.yml` | sus cuatro expresiones, en Python, sobre todo archivo trackeado | OK |
| Generados | `bash scripts/build_llms_full.sh` y `shasum -a 256` de `skill.md` | sin diferencias; digest igual al publicado |
| fmt (CI no lo corre) | `rustfmt --emit stdout` por archivo, sólo los hunks agregados (main no está rustfmt-limpio) | limpio |
| clippy (CI no lo corre) | `cargo clippy --locked -p x402-rs --features <las mismas> --all-targets` | exit 0; 0 avisos en líneas agregadas (332 preexistentes en `src/`, 2 en archivos tocados, fuera de los hunks) |
| DynamoDB local (`#[ignore]`) | `DYNAMODB_LOCAL_URL=http://127.0.0.1:<puerto> cargo test … local_dynamodb -- --ignored` contra `amazon/dynamodb-local` | 3 passed, incluido el de admisión atómica con un registro de Base |

**Mutaciones** (aplicadas y revertidas; `src/` es idéntico antes y después del
rebase):

- sin el guard de nonce: 2 tests en rojo (el de un solo envío y el del siguiente
  settle);
- sin la resincronización por `gap`: el del siguiente settle en rojo;
- Base fuera de `supported()`: 14 tests de recibos en rojo, entre ellos
  `base_is_announced_and_admitted`, `capability_lists_exactly_the_supported_networks`,
  los replays y el de rieles alternativos.

**SDKs** (condición b): la tabla de arriba, cuatro corridas (Python y TS, cada uno
con el cuerpo de hoy y con la forma de #102), todas con 1 autorización y ningún
402.

## Para c0der

### 1. Orden de despliegue

1. **Antes del merge.** #103 (2.39.5) ya está debajo. Si #102 (2.39.6) entra
   antes, esta rama se rebasea sobre `origin/main` y conserva `2.40.0` (`VERSION`,
   y `[2.40.0]` arriba en `CHANGELOG.md`). Los dos tocan `src/chain/evm.rs`,
   `src/receipts/` y `docs/facilitator-receipts.md`: hay que resolver a mano y
   volver a correr la suite, y releer la condición (c) del punto 2.
   Hay que esperar los checks del PR: `Build & test`, `No AWS account ID in the
   repo` y el plan (drift gate).
2. **Nada de infraestructura.** No hay secreto, permiso IAM ni tabla nuevos. Base
   usa la misma tabla `idempotency_records`, con los mismos `GetItem`/`PutItem`
   que ya usan Arc y Hedera, y la misma clave de firma. Si se quiere confirmar,
   `python scripts/check_receipt_deploy_permissions.py` con credenciales de
   operador (sólo lectura).
3. **Merge a `main`**, que es el release: CI construye, empuja la imagen y hace el
   `terraform apply -target` sobre la task definition y el servicio.
4. **Después del rollout**, `GET /version` tiene que dar `2.40.0` antes de la
   sonda. Si no, el deploy falló y `main` va por delante de producción.
5. **Vigilar una o dos horas** (Logs Insights, `/ecs/facilitator-production`):
   - `receipt admission released` con `network` `eip155:8453`: admisiones de Base
     liberadas antes de enviar (`503` + `Retry-After`). Unas pocas son normales;
     una racha apunta al writer lease o al RPC de Base.
   - `Nonce error under a receipt admission`: la línea nueva de `evm.rs`. En 7
     días hubo 0 errores de nonce en settles de Base, así que cualquier aparición
     se mira.
   - `status=503` y `status=502` en `/settle`, contra el ALB (el procedimiento
     está en `CLAUDE.md`, "The facilitator is returning 5xx").
   - `Transaction submitted to mempool` en Base, `/settle`: el volumen por hora no
     tiene que caer respecto del día anterior.

### 2. Sonda sin plata, después del deploy

Ninguna de estas llamadas mueve fondos ni escribe en el store de recibos.

**A. Lecturas**

- `GET /receipts` → `available: true` y `networks` con cinco entradas:
  `eip155:5042`, `eip155:5042002`, `hedera:mainnet`, `hedera:testnet`,
  `eip155:8453`.
- `GET /supported` → `facilitatorReceipts` igual que `/receipts`.
- `GET /api-docs/openapi.json` → la descripción de `POST /settle` dice
  `Base exact (USDC/EURC)`.
- `GET /health/ready?network=base` → una entrada `base` con `caip2`
  `eip155:8453` (igual que antes; sólo confirma que Base sigue servida).

**B. `/verify` con un pagador sin fondos**

1. Generar una clave nueva en el momento; no se fondea y no se guarda.
2. Firmar con ella un `TransferWithAuthorization` en Base:
   - USDC de `src/network.rs` (`USDC_BASE`), dominio `"USD Coin"`/`"2"`,
     chainId 8453;
   - `value` 1000, `validBefore` = ahora + 300, nonce aleatorio;
   - `payTo`: una wallet de la casa, copiada de `lambda/balances/handler.py`.
3. Mandar el cuerpo v1 (`network: "base"`, `scheme: "exact"`) a `POST /verify`.

Esperado:

- HTTP 200 con `isValid: false`: el pagador no tiene saldo;
- un `receipt` con `network: "eip155:8453"`, `operation: "verify"` y
  `status: "rejected"`;
- el header `Cache-Control: no-store`.

En 2.39 la misma request no traía `receipt`. El recibo de `/verify` no se guarda.

**C. Opcional: `/settle` con el mismo cuerpo y `X-UVD-Purchase`**

- Un contexto nuevo (`purchaseId`, `accessToken` aleatorio, `url` igual al
  `resource` del cuerpo).
- El `/settle` verifica antes de admitir; con el pagador sin saldo la verificación
  falla y no hay admisión, ni escritura, ni envío.
- Esperado: 200 con `success: false`, `errorReason`, y un `receipt`
  `status: "rejected"` con el `purchaseId`.
- En 2.39 esa misma request daba `400 receipt_request_not_supported`, así que
  distingue las dos versiones sin mover nada.

La prueba con un pago real (0,001 USDC entre wallets de la casa: settle con key,
reenvío con key, reenvío sin key → `409 authorization_already_settled`, y la
comprobación en cadena) es la del apartado 2 de
`docs/handoffs/2026-09-23-recibos-base.md`, ahora sin la parte "cuando se
anuncie". La corre c0der, si quiere, después de A y B.

### 3. Límite que queda

Un rechazo por nonce deja la admisión `unknown` con los bytes guardados:

- si fue `already known`, la transacción está en el mempool; la reconciliación la
  confirma cuando mina;
- si fue `nonce too low`, esos bytes no pueden minar nunca y la admisión queda
  `unknown`;
- `receipts release-abandoned` no la toca, porque sólo cierra admisiones sin nada
  preparado. Pasado el `validBefore` de la autorización, los bytes guardados ya no
  pueden mover fondos aunque minaran; recién entonces es seguro que el comprador
  firme otra. Hoy nada le avisa ese momento: el recibo sigue `unknown`.

Ya era así en Arc. Con 0 casos en 7 días de settles de Base no bloquea, pero un
cierre por reconciliación (el nonce de la cuenta pasó a otra transacción y la
autorización venció) sería el siguiente paso si la línea nueva de `evm.rs`
empieza a aparecer.

### 4. Volver atrás

Revertir el merge es otro release, sin migración: el registro guardado no cambió
de forma. Después de revertir, los settles de Base vuelven al camino de 2.39.5.
Las filas de recibos de Base que queden no vencen y nadie las lee. Un reenvío de
una autorización admitida en 2.40.0 vuelve a correr como en 2.39:

- si ya se liquidó, falla en el nonce consumido;
- si estaba en vuelo, puede firmar otra transacción para la misma autorización,
  que el token rechaza si la primera minó. Cuesta gas, no un segundo pago.
