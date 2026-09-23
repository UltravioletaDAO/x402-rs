# Recibos: una admisión que no envió nada se libera en su lugar, y la misma autorización se liquida (2.39.2)

> Encargo de c0der (task_94b55dcc193f), 2026-09-23. Rama `c0der/recibos-reserva-abandonada`
> sobre `origin/main` @ `a7e818db` (2.39.1). Origen: auditoría de solo lectura de c0der
> del mismo día (informe privado, puntos 1 y 4 del lado del facilitador).

## Qué cambia

### 1. Liberar en su lugar, nunca borrar (`src/receipts/mod.rs`)

Antes: `settle()` reservaba la admisión y recién después corría el settle; si éste
fallaba antes de preparar bytes (writer lease, lecturas, estimación, llenado, firma,
persistencia de `prepared`), `finish()` guardaba `unknown` sin `prepared`,
`reconcile()` no tenía nada que mirar y no hay TTL. Todo reenvío sin el vínculo que
admitió contestaba `409 authorization_in_flight` para siempre.

Ahora, al terminar la llamada del dueño, la admisión se cierra en su lugar como
`rejected` con `refusalReason: reservation_abandoned` sólo si se cumplen **todas**:

1. El proveedor dijo que terminó sin enviar (`receipts::unsent(site)`), y el cerrojo
   de envío (`receipts::sending()`) nunca se activó. Una marca después del cerrojo se
   ignora: es una afirmación sobre el pasado y deja de poder hacerse al primer envío.
2. No hay `prepared` ni `settlement`, y la respuesta no nombra transacción.
3. El `save` de cierre es un CAS sobre la revisión del dueño. Si una escritura
   ambigua de `prepared` sí aterrizó (bytes durables que un reenvío podría
   retransmitir), o si otro escritor movió el registro, el CAS falla y no se libera
   nada: queda como estaba y va por reconciliación.

Dónde se marca y dónde se latchea:

- `src/chain/mod.rs`, brazo EVM de `NetworkProvider::settle`: **todo** error del settle
  EVM marca `unsent("evm_settle")`. Es seguro porque el cerrojo lo anula después del envío.
- `src/chain/evm.rs`, `send_transaction_from`: `receipts::sending()` inmediatamente
  antes de `send_raw_transaction` (después de `prepared_evm`) y antes de
  `send_transaction` en la rama sin recibos (fuera de una admisión es un no-op).
- `prepared_evm` / `prepared_hedera`: si el guardado falla, marcan `unsent`. En Hedera
  es la única marca: `prepared_hedera` es lo primero del settle, antes de su store
  propio y de la red.

La respuesta al dueño liberado: `503` + `Retry-After` (el del proveedor si lo había,
p. ej. 5 del writer lease; si no, 2), el `error` del proveedor y
`success:false, retryable:true, safeToRetry:true, safeToReplay:false`. El recibo:
`rejected`, `reservation_abandoned`, `diagnosticCode` = el código del proveedor,
`settlement: null`, `retry: {"action":"resend","afterSeconds":N}`.

### 2. Re-admisión de la MISMA request bajo el MISMO recibo (`readmit`)

`settle()` ve el registro abandonado por el alias de autorización; si es la misma
request (`same_request`: mismo cuerpo y misma capacidad o ninguna) verifica como hoy
y re-admite con un CAS de revisión + los alias nuevos (sólo Idempotency-Key puede ser
nuevo) en una transacción de puts (`Store::readmit`). `/verify` ignora un registro
abandonado y simula como si no existiera.

- Mismo `receiptId`, revisión siguiente: los dos SDK (Python 0.89.0 y TS 2.98.0)
  lanzan `receipt revision conflict` si un comprador reanudado recibe otro
  `receiptId` o una revisión menor. Borrar alias y re-admitir con un id nuevo rompía
  exactamente al comprador que hace lo correcto.
- Otra capacidad, o ninguna para un pago hecho con una: sigue `409 receipt_request_conflict`.
- Idempotency-Key: la key del reenvío queda ligada; la que admitió primero conserva su vínculo.
- Carrera: N reenvíos concurrentes → uno gana el CAS; los demás ven la admisión en
  vuelo (409 / 202 según vínculo). El CAS además **cerca** a cualquiera que aún tenga
  la revisión vieja: su `prepared_evm` falla, así que nunca envía.

### 3. Escrituras del store que no se pudieron confirmar

`reserve` que devuelve `Err`, o `Ok(false)` con NUESTRO propio `receiptId` visible (lo
que hace `DynamoStore` tras un timeout): la admisión nunca se corre y se cierra igual
(`release_unrun`, CAS sobre la revisión escrita). Antes quedaba huérfana en vuelo para
siempre. Mismo trato para un `readmit` ambiguo.

### 4. P3: 503 previos al settle con `Retry-After` (`unavailable()`)

`receipt_store_unavailable`, `receipt_signing_unavailable` y
`receipt_reservation_uncertain` antes de enviar: `503` + `Retry-After: 2` +
`retryable:true, safeToRetry:true`, en `/settle` y `/verify`. Un cliente conservador
que sólo reintenta la misma credencial ante un 5xx con `Retry-After` (o que prueba que
no se ejecutó) deja de leerlos como "posiblemente minado".

### 5. Comando de operador (`src/receipts/admin.rs`, enganchado en `src/main.rs`)

`x402-rs receipts release-abandoned` corre EN LUGAR del servidor (antes de telemetría,
providers y elección del writer lease). Ver "Para c0der".

## Por qué esta variante (la más segura)

- **Cerrar en su lugar, no borrar alias.** El rol de la tarea sólo tiene
  `GetItem`/`PutItem`/`DescribeTable` sobre `idempotency_records`
  (`terraform/environments/production/main.tf`, `dynamodb_idempotency_access`, y
  `scripts/check_receipt_deploy_permissions.py`). Borrar alias necesita `DeleteItem`:
  en producción el borrado fallaría por permisos y el CI no puede aplicar IAM. Además
  cambiaría el `receiptId` (ver §2). Todo lo nuevo es `PutItem` condicional.
- **Marca explícita + cerrojo + sin bytes + CAS**, no "sin `prepared` ⇒ liberar". La
  regla ancha dependería sólo de que todo envío pase por `prepared_*`; con el cerrojo,
  hasta un fallo posterior a un envío que no pasara por ahí queda fuera, porque el
  camino de envío latchea antes de difundir.
- **503 siempre al liberar**, aunque el proveedor dijera 400 (p. ej.
  `contract_call_failed` de una estimación que revierte o de un llenado fallido). Tras
  liberar, la única acción correcta es reenviar la MISMA autorización; un 400 invita a
  firmar otra mientras la primera sigue válida y re-admisible. Si la causa es
  determinista, el reenvío la devuelve como veredicto de `/verify` (no se guarda).

## Tests (`src/receipts/tests.rs`, `store.rs`, `src/chain/evm.rs`)

Por las cuatro redes de `supported()` + Base candidata:

- `a_settlement_that_sent_nothing_releases_its_admission_and_the_same_request_settles`:
  writer lease perdido → 503 + `Retry-After: 5` + recibo abandonado; `/verify` simula
  de nuevo; el reenvío pelado liquida bajo el mismo `receiptId` con revisión mayor; un
  solo envío; el siguiente reenvío pelado es `409 authorization_already_settled`.
- `only_the_request_that_admitted_a_released_payment_takes_it_back`: capacidad (otra o
  ninguna → 409 conflict; la misma → mismo recibo) e Idempotency-Key (otra key liquida
  y queda ligada; la primera también replaya).
- `nothing_is_released_once_a_transaction_may_have_left`: bytes preparados, cerrojo
  activo, transacción nombrada; cada caso marca `unsent` también y debe seguir
  `unknown`/`pending` con reenvío pelado `409 authorization_in_flight`.
- `a_release_racing_resends_of_the_same_authorization_admits_one_payment`: mientras el
  dueño retiene, el reenvío es `409 in_flight`; tras liberar, 20 reenvíos concurrentes
  → 1 envío, 1 éxito.
- `the_operator_command_fences_a_settlement_still_holding_the_admission`: el comando
  cierra una admisión viva (`--min-age-secs 0`); su dueño no puede guardar bytes, no
  envía y contesta 503; el reenvío liquida bajo el mismo recibo.
- `failures_before_anything_is_sent_carry_retry_after_and_say_resending_is_safe`: sin
  servicio con capacidad, store caído, binding caído en `/verify` y `/settle`, y
  reserva no confirmada (`Err` y `Ok(false)` propio) → cerrada y el reenvío liquida.
- Comando: parseo (lectura por defecto, `--dry-run`/`--write` excluyentes, ids UUID) y
  selección (varado viejo y expirado → sí; joven, con bytes, confirmado → no; el
  dry-run no escribe; id inexistente; firmado con otra key → no se re-firma).
- EVM con RPC simulado: lease perdido → nada enviado (contador de
  `eth_sendRawTransaction`), nada latcheado, sin `prepared`; enviado sin confirmar →
  `prepared` guardado antes, cerrojo activo, la marca posterior no cuenta. Y el
  cableado real: un error de `NetworkProvider::Evm(..).settle` queda marcado.
- DynamoDB local (`--ignored`): 20 `readmit` concurrentes desde dos clientes → un
  ganador, sólo su alias existe, revisión vieja o alias tomado no escriben, el scan
  del operador ve el registro y ningún alias, sin TTL.

Mutaciones (hechas a mano y revertidas):

- Liberar ignorando bytes preparados / transacción nombrada (`released = durable &&
  unknown`) → rojo `nothing_is_released_once_a_transaction_may_have_left`.
- Cerrojo inerte (`sending()` no latchea) → rojo ese mismo test y
  `under_a_receipt_admission_only_a_send_that_never_started_can_be_released`.

## Lo verificado (local, LF, nada contra producción)

- `cargo build --locked --features solana,near,stellar,algorand,sui,xrpl,hedera`: OK.
- `cargo test --locked -p x402-rs --features <las del CI> -- --test-threads=1`: 2642 passed,
  0 failed, 27 ignored (lib 1262, bin 1315, integración 65).
- `cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance -- --test-threads=1`:
  109 passed, 0 failed.
- DynamoDB local (`amazon/dynamodb-local`, `DYNAMODB_LOCAL_URL=http://127.0.0.1:<port>`,
  `--ignored receipts::store::integration`): 2 passed, 8 corridas seguidas verdes.
- `cargo fmt`: `main` no está limpio; las líneas agregadas sí (rustfmt sobre el diff).
- `cargo clippy --all-targets` con las features del CI: ningún aviso en líneas nuevas
  (los que quedan en `chain/mod.rs:82`, `receipts/mod.rs:297`, `evm.rs:1914/5773` son
  previos).
- `verify_landing_canonical.py --offline`, `node --test tests/frontend-capabilities.test.cjs`,
  balances `unittest`: verdes. `llms-full.txt` regenerado y digest de `skill.md` al día.

## Para c0der

### 1. Comando de operador (después del deploy)

Lectura (por defecto; también `--dry-run`). Necesita `dynamodb:Scan`, que el rol de
la tarea NO tiene: correrlo con credenciales de operador desde un checkout de esta rama:

```bash
IDEMPOTENCY_TABLE_NAME=idempotency_records AWS_REGION=us-east-2 \
  cargo run --locked -- receipts release-abandoned
# opcional: --min-age-secs 3600
```

Una línea JSON por registro (`would_release` o `skip_*`) y un resumen. Nada privado
(sin payer ni capacidad). Elegibles: `settle`, `unknown`, sin `prepared` ni
`settlement`, y con más de `--min-age-secs` (900 por defecto) o autorización vencida.

Escritura con los ids listados. Firma con la key del servicio y rechaza un registro
firmado por otra, así que va como tarea única de la task definition del servicio (la
key no sale de la tarea; sólo `GetItem`/`PutItem`):

```bash
REGION=us-east-2 CLUSTER=facilitator-production SERVICE=facilitator-production
TD=$(aws ecs describe-services --region $REGION --cluster $CLUSTER --services $SERVICE \
  --query 'services[0].taskDefinition' --output text)
NET=$(aws ecs describe-services --region $REGION --cluster $CLUSTER --services $SERVICE \
  --query 'services[0].networkConfiguration' --output json)
TASK=$(aws ecs run-task --region $REGION --cluster $CLUSTER --task-definition "$TD" \
  --launch-type FARGATE --network-configuration "$NET" \
  --overrides '{"containerOverrides":[{"name":"facilitator","command":["receipts","release-abandoned","--write","--receipt-id","<uuid>"]}]}' \
  --query 'tasks[0].taskArn' --output text)
aws ecs wait tasks-stopped --region $REGION --cluster $CLUSTER --tasks "$TASK"
aws ecs describe-tasks --region $REGION --cluster $CLUSTER --tasks "$TASK" \
  --query 'tasks[0].containers[?name==`facilitator`].exitCode'   # 0 = sin fallos
aws logs get-log-events --region $REGION --log-group-name /ecs/facilitator-production \
  --log-stream-name "ecs/facilitator/${TASK##*/}" --query 'events[].message' --output text
```

`--receipt-id` se repite para varios. Sin ids, dentro de ECS falla por falta de
`Scan` (es a propósito: el scan lo hace el operador). La misma request reenviada
después se re-admite; una autorización vencida simplemente no verifica.

### 2. Sonda de cierre

1. `curl -s https://facilitator.ultravioletadao.xyz/version` → `2.39.2`.
2. `curl -s https://facilitator.ultravioletadao.xyz/api-docs/openapi.json | jq -r '.paths["/settle"].post.responses["503"].description' | grep -c reservation_abandoned` → `1`.
3. `GET /receipts` → las mismas cuatro redes que antes (esto no anuncia Base).
4. Tras escribir los ids del §1, repetir la lectura: ningún `would_release` de esos ids
   (pasan a `skip_not_unknown`).
5. Logs Insights sobre `/ecs/facilitator-production`, para ver cuánto dispara en vivo:
   `fields @timestamp, @message | filter @message like /receipt admission released/ or @message like /taken back by the request/ | sort @timestamp desc`.
   Un "released" sin "taken back" posterior es un comprador que no reenvió (no es error).

### 3. Coordinación de versión

El PR #99 (`c0der/residuos-2026-09-23`) también declara 2.39.2. Éste sale de `main` en
2.39.1 como pide el encargo; el que se mergee segundo sube a 2.39.3 (VERSION y
CHANGELOG; la doc no nombra la versión).

### 4. Fuera del alcance, anotado

- `send_transaction_from` no devuelve el nonce reservado cuando falla el llenado, la
  firma o `prepared_evm` (sale por `?`): el contador queda uno adelante hasta el
  próximo resync. Previo a este cambio; ahora que el reenvío liquida, el siguiente
  envío de ese signer puede quedar detrás del hueco hasta que el timeout de recibo
  resetea el nonce.
- Un proceso que muere entre la admisión y el envío sigue dejando `unknown` hasta que
  corre el comando (documentado en "Known limits").
- Los fallos DESPUÉS de difundir siguen recuperándose del lado del cliente, con el
  vínculo que admitió (la Idempotency-Key del SDK 0.89.0 o `X-UVD-Purchase`).

### 5. Volver atrás

Revertir el commit. Registros que el código nuevo dejó `reservation_abandoned` y nadie
re-admitió: el código viejo los trata como `rejected` (replay de su 503 guardado,
`/verify` inválido con ese motivo), o sea vuelven a quedar bloqueados como antes del
arreglo, sin riesgo de plata. El comando de operador no existe en el binario viejo.
