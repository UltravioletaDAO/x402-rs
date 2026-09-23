---
date: 2026-09-23
tags:
  - type/handoff
  - domain/receipts
  - domain/base
status: active
---

# Recibos: el replay sin contexto de compra no repite el éxito, y Base queda lista sin anunciar (2.39.0)

**Base:** `cc2cf345` (2.38.0). **Rama:** `c0der/recibos-base`. **Publica `2.39.0`**:
minor, porque cambia la respuesta observable de `/verify` y `/settle` en las redes
que ya tienen recibos (Arc y Hedera nativo).

## Qué cambia

### 1. Replays de una autorización ya admitida (`src/receipts/mod.rs`)

La respuesta original vuelve solo a quien trae el vínculo de compra con el que se
admitió el pago: la misma capacidad `X-UVD-Purchase` o el mismo `Idempotency-Key`.
Así se recupera una respuesta perdida, y eso no cambia:

- 200 repetido con `Idempotent-Replayed: true`;
- `202 settlement_in_progress` mientras está en vuelo;
- el error pegajoso de un resultado incierto.

Tener el pago firmado no alcanza como vínculo de compra:

| Reenvío | `/settle` | `/verify` |
|---|---|---|
| Misma capacidad o mismo `Idempotency-Key` | Respuesta original repetida | Veredicto y recibo guardados |
| Sin vínculo, pago `confirmed` | `409 authorization_already_settled` con el recibo | `isValid: false`, `authorization_already_settled` |
| Sin vínculo, pago `pending`/`unknown` | `409 authorization_in_flight` con el recibo | `isValid: false`, `authorization_in_flight` |
| Sin vínculo, pago `rejected` | El rechazo original, como antes | El rechazo guardado |
| Otra capacidad, o ninguna para un pago hecho con una | `409 receipt_request_conflict` sin recibo, como antes | `isValid: false`, sin recibo |

Detalles de esos 409:

- Nunca llevan `success: true`, `transaction` en el primer nivel ni
  `Idempotent-Replayed`. Sí llevan `Cache-Control: no-store`.
- El recibo que incluyen permite probar el pago a quien lo hizo. Un recibo hecho con
  contexto de compra nunca se devuelve sin ese contexto.
- `/verify` contesta desde el recibo guardado y no vuelve a simular la
  autorización consumida.
- Si el store falla mientras se resuelve el vínculo, `/verify` y `/settle`
  responden `503 receipt_store_unavailable`: no hay veredicto.
- El `Idempotency-Key` tiene que ser el mismo valor en `/verify` y en `/settle`.
  Una key distinta por operación ata solo la llamada que admitió el pago.
- La reconciliación y la retransmisión de los bytes guardados siguen igual.

Cómo está hecho:

- Una sola función decide: `replay()`, la usan los dos caminos de settle (el alias
  de autorización y el conflicto de reserva).
- `bound()` mira si el reenvío trae el vínculo. Para la capacidad reusa
  `same_request`. Para la key, relee el alias `receipt:idem:v1:…` y exige que
  resuelva al mismo `receiptId`.
- No hay campos nuevos en el registro guardado.

### 2. Rieles alternativos

`parse_request` ya no admite una request que `/settle` manda a otro camino de
liquidación, aunque sus requirements digan `exact`:

- el `scheme` `upto`, `escrow`/`commerce` o `fhe-transfer`, ya esté en el primer
  nivel, en `paymentPayload` o en `paymentPayload.accepted`;
- la extensión x402r `refund`.

Hoy ninguna red con recibos ofrece esos esquemas, pero tiene que valer antes de
sumar una que sí los ofrezca. La función es `alternative_route`.

### 3. Direcciones EVM y Base

- **Direcciones EVM en minúsculas por familia de red.** Antes la regla nombraba a
  Arc. En Arc no cambia nada.
- **Base, preparada sin anunciar.** `supported()` no incluye Base. En los tests
  acepta una red candidata (`TEST_CANDIDATE`, `#[cfg(test)]`, el mismo patrón que
  `TEST_SERVICE`), y así el camino `exact` de Base pasa por la admisión, los
  replays, la normalización y DynamoDB local. `/receipts` y
  `/supported.facilitatorReceipts` siguen listando Arc y Hedera, y un settle de
  Base responde lo mismo que en 2.38.0.

### Tests (`src/receipts/tests.rs`, `store.rs`)

- **Replays, parametrizados** por las cuatro redes de `supported()` (`eip155:5042`,
  `eip155:5042002`, `hedera:mainnet`, `hedera:testnet`) y por Base como candidata.
  El request de Hedera sale del vector oficial `03-hts-usdc` que ya está en el repo:
  - `a_bare_resend_of_a_settled_authorization_is_refused_with_its_receipt`
  - `the_binding_that_admitted_a_payment_recovers_its_lost_response` (con key y
    con capacidad; key ajena, sin key y otra capacidad)
  - `a_bare_resend_in_flight_is_refused_and_only_the_binding_gets_the_202`
  - `an_uncertain_outcome_is_replayed_to_its_binding_and_refused_bare`
  - `concurrent_bare_resends_admit_one_payment_and_one_success`: 20 reenvíos
    pelados, un envío y un solo 200.
  - `a_store_fault_while_resolving_the_binding_is_no_verdict`: un store que falla
    solo en el alias `receipt:idem:v1:` da 503 en `/verify` y en `/settle`.
  - `every_network_in_supported_is_covered_by_the_replay_tests`: falla si una red
    entra a `supported()` sin estar en esos tests.
- **Base y rieles:**
  - el test de concurrencia con contexto corre en `arc`, `arc-testnet` y `base`;
  - `base_escrow_and_refund_requests_keep_their_own_settlement_path`: cubre
    `refund`, `scheme: "escrow"` en el primer nivel, y `paymentPayload.scheme`
    `commerce`, `upto` y `fhe-transfer`. También un cuerpo v2 con
    `paymentPayload.accepted.scheme = "escrow"`, cuyo control `exact` sí se admite;
  - `base_is_not_admitted_until_it_is_announced`;
  - `capability_lists_exactly_the_supported_networks`.
- **Vectores:** `tests/fixtures/facilitator-receipts-v1.json` suma Base USDC y
  EURC, sintéticos y firmados con la clave de test `[7; 32]`. Los 6 existentes
  quedan idénticos byte a byte. El test
  `shared_vectors_are_synthetic_receipts_and_cover_base` reconstruye los 8 desde
  sus entradas.
- **DynamoDB local** (`#[ignore]`): admite un registro Arc y uno Base, cada uno con
  20 reservas concurrentes desde dos clientes.

### Docs

- `docs/facilitator-receipts.md`: sección nueva "Replays of an admitted
  authorization", más la regla de rieles alternativos, la tabla de estados y el
  alcance de validación.
- La OpenAPI de `POST /settle` (`202` y `409`).
- La sección de recibos de `/index.md`, `/skill.md` y `/mcp.md`, `llms-full.txt`
  regenerado y el digest de `skill.md` en `/.well-known/agent-skills/index.json`.
- `VERSION` 2.38.0 → 2.39.0 y `CHANGELOG.md` (el de la raíz).

**No cambia:**

- el riel legacy (`src/idempotency_store.rs`, `post_settle_inner`) y
  `src/chain/evm.rs`;
- `GET /receipts/{id}`;
- `/receipts` y `/supported.facilitatorReceipts`;
- las redes sin recibos;
- el replay de una respuesta legacy cacheada antes de los recibos.

## Lo verificado

- **Mutaciones:**
  - si el settle vuelve a repetir el éxito a cualquier reenvío, fallan 5 tests;
  - si el verify vuelve a validar a cualquier reenvío, fallan 2;
  - si un error del store al resolver el vínculo vuelve a leerse como "no atado",
    falla el test del 503;
  - sin la guardia de rieles alternativos, falla su test;
  - con la normalización vieja (solo Arc), falla el de concurrencia en Base.
- **USDC en Base**, leído en `mainnet.base.org` y `base-rpc.publicnode.com` (solo
  lectura, ninguna request al facilitador): `decimals()` = 6 y `DOMAIN_SEPARATOR()`
  = `"USD Coin"`/`"2"`/8453, igual que `USDC_BASE`.

Checkout con LF. Los pasos que lista `scripts/preci.py --base origin/main`
(dispara `ci.yaml` y `no-account-id.yml`), corridos en local:

| Paso | Resultado |
|---|---|
| `python3 scripts/verify_landing_canonical.py --offline` | `[OK]` |
| `node --test tests/frontend-capabilities.test.cjs` | 8/8 pass |
| `python3 -m unittest discover -s tests/scripts -p 'test_*balances.py'` | 5 tests, OK |
| `cargo build --locked --features solana,near,stellar,algorand,sui,xrpl,hedera` | exit 0, ningún warning en `src/receipts/` |
| `cargo test --locked -p x402-rs --features <las mismas> -- --test-threads=1` | exit 0, **2584 passed, 0 failed**, 23 ignored (11 suites). Los 7 tests nuevos de replays y del 503 corren en la lib y en el binario: +14 sobre la primera ronda de esta rama, sin C, que dio 2570 |
| `cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance -- --test-threads=1` | exit 0, 109 passed, 0 failed, 9 ignored |
| `DYNAMODB_LOCAL_URL=http://127.0.0.1:18000 cargo test ... local_dynamodb_atomic_admission_cas_and_no_ttl -- --ignored` contra `amazon/dynamodb-local` en Docker | 1 passed: un registro Arc y uno Base admitidos 1 vez de 20 cada uno, CAS, 6 filas, ninguna con TTL |
| `no-account-id.yml` (sus tres expresiones, en Python, sobre los archivos del diff) | sin coincidencias en los 13 archivos; ninguna línea nueva con `0x` + 64 hex |
| `rustfmt --check` sobre `src/receipts/` | limpio |

## Para c0der

### 1. Sonda en vivo después de este release (2.39.0), sin liquidar nada

- `GET /version` → `2.39.0`.
- `GET /receipts` → `available: true` y `networks` igual que antes:
  `eip155:5042`, `eip155:5042002`, `hedera:mainnet`, `hedera:testnet`, **sin**
  `eip155:8453`.
- `GET /supported` → `facilitatorReceipts`, igual que `/receipts`.
- `GET /api-docs/openapi.json` → el `409` de `POST /settle` nombra
  `authorization_already_settled` y `authorization_in_flight`.

### 2. Prueba del contrato con un pago real (la corre c0der; Arc hoy, Base cuando se anuncie)

La plantilla es `scripts/arc_canary.py`. Su helper `request()` descarta los headers,
así que los pasos 3 y 4 tienen que leerlos aparte.

1. Firmar un `TransferWithAuthorization` de 1000 unidades atómicas (0,001 USDC)
   entre dos wallets de la casa.
2. Guardar los bytes exactos del cuerpo (0600). **Enviarlo a `/settle` con
   `Idempotency-Key: <uuid4 nuevo>`** → 200, `success: true`, transacción T,
   `receipt.status` = `confirmed`.
3. Reenviar los **mismos bytes con la misma key** → 200, `Idempotent-Replayed: true`,
   la misma T y el mismo `receiptId`.
4. Reenviar los mismos bytes **sin key** → **409 `authorization_already_settled`**,
   sin `Idempotent-Replayed`, con `receipt.settlement.id` = T.
5. `/verify` con los mismos bytes, sin key → 200, `isValid: false`,
   `invalidReason: authorization_already_settled`.
6. En cadena: el receipt de T tiene `status` 1 y un único `Transfer` por 1000.
   `authorizationState(pagador, nonce)` (selector `0xe94a0102`) = true. Un solo log
   con topic0 = `keccak256("AuthorizationUsed(address,bytes32)")`, topic1 = el
   pagador, topic2 = el nonce, y su hash es T. El saldo del payTo sube exactamente
   1000.

Si el paso 2 no da 200 con `success`, no se vuelve a firmar: se reenvían los mismos
bytes con la misma key. Un 202 significa que sigue en vuelo.

Para Base, cuando se anuncie:

- USDC `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913`, dominio `"USD Coin"`/`"2"`,
  chainId 8453;
- cuerpo v1 con `network: "base"` y `extra: {name: "USD Coin", version: "2"}`;
- `payTo` = otra wallet de la casa sin tráfico entrante, por ejemplo la EVM de
  testnet `0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8`
  (`lambda/balances/handler.py:73`);
- además, `GET /receipts` tiene que listar `eip155:8453`.

### 3. Anunciar Base

No es parte de este PR. El cambio está preparado y probado aparte, y lo tiene
c0der. Toca:

- `supported()` y `capability()`;
- la OpenAPI;
- un test, que pasa a `base_is_announced_and_admitted`;
- la guía con la tabla antes/después de Base;
- `skill.md`, `index.md`, `mcp.md`, README, `llms-full.txt` y el digest;
- CHANGELOG y VERSION.

### 4. Volver atrás

Revertir el merge es otro release. No hay datos que migrar: el registro guardado no
cambió de forma. Después de revertir, un reenvío pelado de una autorización admitida
vuelve a recibir el replay 200 de 2.38.0.
