---
date: 2026-09-10
tags:
  - type/handoff
  - domain/erc8004
  - domain/solana
  - priority/p1
status: active
---

# El mint de identidad en Solana ya no puede quedar a medias

**Versión:** 2.17.0 · **Reportado por:** KarmaKadabra, 2026-09-09
(`docs/reports/HANDOFF_EM_REPUTACION_SOLANA_CAE_A_BASE_2026-09-09.md` en su repo,
commits `93df7918` y `4f66fa1f`).

Acuñar una identidad ERC-8004 en Solana eran **tres transacciones**, y una
transacción que entra es final por su cuenta. Cualquier prefijo de esas tres era
un estado final alcanzable, y el endpoint devolvía `agent_id` y `success` sin
mirar si el prefijo estaba completo. Cuando el pagador de comisiones se quedó sin
SOL a mitad de un lote de veinte, dos agentes terminaron con la cuenta creada, el
facilitador como dueño, y una respuesta que el cliente leyó como éxito. Los
reintentos no podían saber que eran reintentos, así que cada uno acuñó **otra**
identidad a medias. Quedaron cuatro assets huérfanos a nombre del facilitador.

Este cambio hace tres cosas: manda las tres instrucciones en **una** transacción,
reconoce un reintento y lo **retoma** en vez de duplicar, y **comprueba el saldo
antes de mandar** para que un pagador vacío sea un error con nombre y no un
`-32002` que se lee como un bug del programa.

---

## Para c0der

### 1. El trazo real, antes de tocar nada

Todo pasaba dentro de `post_register` (`src/handlers.rs:9800` en `origin/main`,
commit `9893c48f`), en la rama Solana que abría en `:9866`:

| # | Instrucción | Se arma en | Se manda en | Si falla |
|---|---|---|---|---|
| 1 | `Register` \| `CreateV2` | `:9942` | `:9951` | 500, sin `agentId` |
| — | `set_metadata_pda` (0..n) | `:9971` | `:9979` | **solo un `warn!`**, sigue |
| 2 | `InitializeStats` | `:9997` | `:10003` | **solo un `error!`** (`:10022`), sigue |
| 3 | `TransferAgent` \| `Transfer` | `:10033` | `:10038` | 500 con `success: false` (`:10064`) |

`let agent_id = asset_pubkey.to_string();` en `:9960`: desde ahí el `agentId`
existe, pase lo que pase después. La respuesta de éxito se armaba en `:10093`,
con `success: true` literal en `:10096`.

**Tres ramas devolvían con el facilitador todavía dueño del asset, y dos de ellas
decían `success: true`:**

1. **Sin `recipient`.** No se intenta transferencia, `final_owner` queda en el
   pagador, y la respuesta es `200 success: true` con `agentId`. Es exactamente
   la forma que describe KK: `agent_id` + `success` habiendo corrido solo la (1).
2. **`InitializeStats` falla.** Se registra `atom_ready = false` y **se sigue**.
   Si la transferencia después entra, la respuesta es `200 success: true` para
   una identidad cuyo feedback no va a puntuar nunca — y ya no se puede arreglar,
   porque después del traspaso el facilitador dejó de ser dueño.
3. **La transferencia falla.** Esta ya era honesta: `500`, `success: false`.

Lo que **no** se puede determinar desde el código es cuál de las dos primeras
produjo los cuatro huérfanos de KK. Para cerrarlo hace falta la línea real:

```bash
S=$(( ($(date -u -d '2026-09-10' +%s) - 172800) * 1000 ))
aws logs filter-log-events --log-group-name /ecs/facilitator-production \
  --region us-east-2 --start-time $S \
  --filter-pattern '"Failed to initialize ATOM stats"' \
  --query 'events[].message' --output text
```

Si aparece, fue la rama 2. Si no aparece nada y tampoco hay
`"Agent minted but transfer to recipient failed"`, fue la rama 1 (el cliente no
mandó `recipient`). En cualquiera de los dos casos el arreglo es el mismo.

El lookup por dueño (`get_identity_by_owner_solana`, `src/handlers.rs:9648`) ya
estaba **bien**: devolvía 404 porque el agente genuinamente no era el dueño, y
504/503 —nunca 404— cuando el escaneo no llegaba a veredicto. No se tocó.

### 2. Qué cambió

Todo lo que **decide** algo vive en `src/erc8004/solana_mint.rs`, como funciones
puras sobre valores, y se prueba como tales. Las llamadas RPC quedaron en
`run_solana_registration` (`src/handlers.rs`), que es la única parte que necesita
una cadena.

**Atomicidad.** `plan_mint` arma `register` + `set_metadata_pda`\* +
`initialize_stats` + `transfer_agent` en una sola lista y **mide** las dos cotas
que importan:

| | medido | límite | margen |
|---|---|---|---|
| Tamaño con la URI de KK (46 chars) | 685 bytes | 1232 | 44 % |
| Tamaño con la URI más larga que acepta el programa (250) | 890 bytes | 1232 | 28 % |
| Compute (3 instrucciones) | 600 000 CU | 1 400 000 | — |

Las dos medidas están fijadas en `the_bundle_measurement_is_pinned`, así que un
cambio futuro en las instrucciones no puede empujarlas por encima del límite en
silencio.

El argumento de compute es el que hace que esto sea seguro y conviene no
perderlo: hoy cada una de las tres instrucciones corre **sola** en su transacción
con 200 000 CU y funciona. Una transacción de `n` instrucciones recibe
`min(200 000 × n, 1 400 000)`, así que mientras `n ≤ 7` cada instrucción conserva
al menos el presupuesto que ya tiene. Pasadas siete el techo empieza a diluirlas
y el argumento se cae; por eso `MAX_BUNDLED_INSTRUCTIONS = 7` y por encima de eso
el plan se manda por etapas.

Cuando no entra —por tamaño o por conteo— se manda una transacción por
instrucción, se **para en el primer fallo**, y la respuesta dice dónde paró. No
se sigue después de un `initialize_stats` fallido: transferir ahí entrega una
identidad cuyo feedback no puede puntuar nunca y nadie más que el dueño puede
crear esa cuenta después.

**Estado real en la respuesta.** `RegisterAgentResponse` gana un campo `mint`
(ausente en la ruta EVM, cuyo contrato no se tocó). `mint.status` es lo que hay
que leer:

| `mint.status` | Significa |
|---|---|
| `complete` | Todo lo que pidió la petición confirmó. |
| `pending_stats` | La identidad existe sin sus ATOM stats y el facilitador la tiene. Repetir la misma petición la termina. |
| `pending_transfer` | La identidad existe e inicializada, el facilitador la tiene. Repetir la misma petición la termina. |
| `not_minted` | No existe ninguna identidad y no quedó nada atrás. Reintentar es seguro. |

`success` es `true` **solo** en `complete`.

**Idempotencia.** `decide_mint` busca, entre los agentes que el pagador todavía
tiene (`find_agents_by_owner`), uno cuyo `agent_uri` coincida con el de la
petición. Si lo encuentra, esa es la identidad que se retoma: se corren solo las
instrucciones que faltan, sin `register`. El `agentUri` es el vínculo porque el
huérfano es del pagador, no del agente — que es justamente por qué
`GET /identity/solana/owner/<agente>` contestaba 404 correctamente mientras había
una identidad suya en el registro.

Dos reglas que van con eso:

- **Un `agent_uri` vacío nunca adopta nada.** Es `#[serde(default)]` en la
  petición; tratarlo como clave dejaría que una llamada sin URI se quede con un
  asset ajeno que tampoco tiene URI.
- **Un escaneo sin veredicto es un 503, no un mint.** Acuñar sobre un
  `getProgramAccounts` que falló es exactamente lo que convirtió un reintento en
  un segundo huérfano. Es la misma regla que ya sigue
  `/identity/:network/owner/:address`.

**Saldo.** `estimate_mint_cost` cotiza desde la renta on-chain, no desde un
número de titular:

| Concepto | lamports | SOL |
|---|---|---|
| Renta del agent PDA (748 bytes) | 6 096 960 | 0,006097 |
| Renta de las ATOM stats (561 bytes) | 4 795 440 | 0,004795 |
| Asset de Metaplex Core (medido) | 2 500 000 | 0,002500 |
| Comisiones (2 firmas) | 10 000 | 0,000010 |
| **Total** | **13 402 400** | **0,013402** |

Esto **corrige** el número de KK: sus 0,0044–0,0089 SOL son la transacción de
`register` sola. El mint completo paga además la renta de las ATOM stats, así que
cuesta ~0,0134 SOL. Es la diferencia entre dimensionar una alarma para 1,5 mints
y para 50.

Si el saldo no alcanza, la respuesta es `503` con
`mint.errorCode = "fee_payer_insufficient_balance"` y `mint.feePayer` llevando
`availableLamports`, `requiredLamports` y `mintsRemaining`. **No se manda nada a
la cadena**, así que no hay identidad parcial que limpiar.

Leer el saldo es una guardia, no la operación: si el RPC no contesta el saldo, se
acuña igual y se registra un `warn!`. Negarse a acuñar porque una llamada de
lectura falló sería una caída autoinfligida.

**Sobrestimar aquí solo endurece el preflight.** Los 748 bytes del agent PDA
salen del comentario que documenta la struct, no de una medición on-chain; si son
menos, rechazamos un mint que habría entrado raspando, en una banda estrecha, y
el error nombra los dos números. La acción correcta —fondear la billetera— es la
misma. Se puede bajar con `X402_SOLANA_MINT_ASSET_LAMPORTS`.

### 3. Alarmas — las crea el pipeline

En `terraform/environments/production/alerts-solana-mint.tf`, y sus tres
addresses están en la lista `-target` del paso **`Apply alerting`** de
`.github/workflows/ci.yaml`, al lado de todas las demás alarmas de este stack
(`chain_balance_low`, `chain_rpc_unreachable`, `latency_p99_early`, las de
NEAR…). **Al mergear a `main` se crean solas**; no hay comando a mano que correr.

Dos alarmas, deliberadamente de distinta naturaleza:

- **`facilitator-production-solana-mint-headroom-low`** — sobre el
  `ChainNativeBalance` que la Lambda de balances ya publica cada 15 min, umbral
  `0,0134 × 50 = 0,67 SOL`. Avisa **antes** de que se rompa nada.
- **`facilitator-production-solana-mint-fee-payer-dry`** — filtro de métrica
  sobre la línea `solana_mint_fee_payer_insufficient`, umbral 1 en 5 min. Avisa
  cuando un mint **ya** fue rechazado.

El piso que ya existía para `solana-mainnet` es **0,02 SOL**, dimensionado sobre
settles (~0,000005 SOL cada uno). Para el riel de mint eso es **un mint y medio**:
por eso el 2026-09-09 no sonó nada mientras diez mints fallaban con el pagador en
0,000866 SOL. No lo toqué —es correcto para lo que mide— y las dos de arriba van
al lado.

**Si agregás una tercera alarma acá, agregá también su address a esa lista.** El
drift gate de `ci.yaml` falla cualquier recurso declarado que no exista en AWS y
que ningún paso de deploy apunte — que es justamente cómo se detectó que a estas
tres les faltaba el `-target`. Esa es la señal de que funciona.

**No corrí `init`, `plan` ni `apply`**: el backend es el estado de producción y
el `apply` lo hace el pipeline al mergear. `terraform fmt -check` pasa en los dos
archivos, y el drift gate quedó en verde con los `-target` puestos.

### 4. Tests

Los cuatro que pidió el encargo, en `src/erc8004/solana_mint.rs`, más doce
alrededor. 24 en total; la suite completa del crate queda en 844.

| Encargo | Test |
|---|---|
| (a) mint completo → success con las 3 firmas | `full_mint_fits_in_one_transaction`, `a_complete_outcome_reports_complete`, `the_bundle_measurement_is_pinned` |
| (b) falla la (3) → sin success, estado pendiente | `a_mint_whose_transfer_failed_is_pending_not_successful`, `a_mint_that_stopped_before_its_stats_is_pending_stats` |
| (c) reintento sobre una identidad a medias → la completa sin crear otra | `a_retry_resumes_the_stranded_identity`, `a_resume_plan_does_not_mint_a_second_asset`, `all_four_stranded_assets_are_reachable_by_their_uri` |
| (d) saldo insuficiente → error claro sin tocar la cadena | `an_underfunded_fee_payer_is_refused_before_the_chain` |

**Probados en rojo**, cada uno rompiendo la conducta que afirma:

| Mutación | Test que cae |
|---|---|
| `MAX_BUNDLED_INSTRUCTIONS = 2` | `full_mint_fits_in_one_transaction` |
| `status()` sin la rama `PendingTransfer` | `a_mint_whose_transfer_failed_is_pending_not_successful` |
| `decide_mint` devuelve siempre `Fresh` | `a_retry_resumes_the_stranded_identity` |
| `is_sufficient()` devuelve siempre `true` | `an_underfunded_fee_payer_is_refused_before_the_chain` |

Los fixtures son los identificadores reales del reporte: los cuatro assets
huérfanos, el pagador `F742C4Vf…` y el saldo de 866 000 lamports. Las dos
billeteras de agente vienen truncadas en el reporte (`Drc9BkJc…`, `BXPKpV6f…`);
inventar una pubkey base58 sería peor que un placeholder honesto, así que los
tests usan claves frescas — nada depende de su valor, solo de su papel.

No hay pruebas contra surfpool ni devnet en el repo y no agregué una dependencia
de red: la lógica que decide está toda en funciones puras y se prueba sin cadena.

### 5. Verificado en esta Mac, y lo que no

```
cargo test --locked -p x402-rs \
  --features solana,near,stellar,algorand,xrpl -- --test-threads=1
    844 passed; 0 failed
```

**El feature `sui` no compila acá** y no es por este cambio: sus dependencias son
git de MystenLabs y el fetch falla con `no authentication methods succeeded`,
también con `CARGO_NET_GIT_FETCH_WITH_CLI=true`. Nada de lo tocado entra en
`#[cfg(feature = "sui")]`, y CI corre el set completo. Si el job `test` sale rojo
ahí, es lo primero a mirar.

`cargo` no está en el `PATH` por defecto en esta máquina: vive en
`~/.cargo/bin/cargo` vía `rustup` (1.98.1).

### 6. Lo que decidí y podés querer distinto

- **La metadata entró en la unidad atómica.** Antes un `set_metadata_pda` que
  fallaba dejaba un `warn!` y un mint "exitoso" sin la metadata que se pidió —
  la misma forma de medio-éxito que este ticket viene a sacar. Ahora una entrada
  malformada tumba el mint entero **sin dejar nada en la cadena**, que es
  estrictamente mejor que una identidad a la que le falta lo que pidió. La
  salida, si molesta, es reintentar sin el campo `metadata`.
- **Un resume NO reescribe la metadata**, y lo dice: `mint.metadataSkipped`.
  Repetir `set_metadata_pda` sobre una entrada que el primer intento ya creó
  tumbaría la transacción y dejaría la identidad varada para siempre. El dueño la
  puede poner cuando ya es suya.
- **Un `pending_*` responde 500.** No es un error del cliente y no es nada: la
  identidad existe y la tiene el facilitador. El `mint.status` es lo que
  distingue, y el texto del error dice que repetir la misma petición la termina.
- **Un resume adopta también un asset que el facilitador tiene a propósito**
  (registrado sin `recipient`) si la URI coincide y ahora sí viene `recipient`.
  Eso es entregarlo, que es lo que se pidió; y esos assets son inalcanzables para
  cualquier otro, así que adoptarlos no le saca nada a nadie.

---

## Para KK

### Qué cambia en la respuesta del mint

La respuesta de `POST /register` en Solana ahora trae un objeto `mint`. **Leé
`mint.status`, no `success`.** Los dos discrepan a propósito.

```json
{
  "success": true,
  "agentId": "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgHkv",
  "transaction": "5eykt4Us…",
  "transferTransaction": "5eykt4Us…",
  "owner": "Drc9BkJc…",
  "network": "solana",
  "mint": {
    "status": "complete",
    "resumed": false,
    "atomic": true,
    "metadataSkipped": false,
    "statsTransaction": "5eykt4Us…",
    "feePayer": {
      "address": "F742C4VfFLQ9zRQyithoj5229ZgtX2WqKCSFKgH2EThq",
      "availableLamports": 1000000000,
      "requiredLamports": 13402400,
      "mintsRemaining": 74
    }
  }
}
```

- `mint.atomic: true` significa que **una sola transacción** llevó las tres
  instrucciones. Por eso `transaction`, `mint.statsTransaction` y
  `transferTransaction` son la misma firma: no es un bug, es que ya no hay tres.
  Y significa que no hay prefijo posible: o entró todo o no entró nada.
- `mint.status: "complete"` es el único caso con `success: true`.
- `mint.status: "pending_transfer"` o `"pending_stats"` → HTTP 500, `success:
  false`, y `agentId` presente porque la identidad **existe** pero la tiene el
  facilitador. **Repetí la misma petición** y la termina.
- `mint.status: "not_minted"` → no existe nada y no quedó nada. Reintentar es
  seguro y no genera huérfanos.
- `mint.resumed: true` → esta llamada terminó una identidad que una llamada
  anterior había dejado a medias, en vez de acuñar otra.

Y el modo de falla que apagó el lote:

```json
{
  "success": false,
  "network": "solana",
  "error": "The facilitator's Solana fee payer F742C4Vf… holds 0.000866000 SOL and this mint needs 0.013402400 SOL, almost all of it rent for the accounts it creates. Nothing was sent to the chain, so there is no partial identity to clean up. Fund the wallet and retry.",
  "mint": {
    "status": "not_minted",
    "errorCode": "fee_payer_insufficient_balance",
    "feePayer": {
      "address": "F742C4VfFLQ9zRQyithoj5229ZgtX2WqKCSFKgH2EThq",
      "availableLamports": 866000,
      "requiredLamports": 13402400,
      "mintsRemaining": 0
    }
  }
}
```

HTTP `503`, con nombre y con los dos números, en vez de
`-32002 Transaction simulation failed: Error processing Instruction 0`.

**Un dato que corrige el suyo:** los 0,0044–0,0089 SOL que midieron son la
transacción de `register` sola. El mint completo paga además la renta de las ATOM
stats, así que sale **~0,0134 SOL**. Sigue siendo tres órdenes de magnitud por
encima de una calificación (0,00001 SOL), que era el punto.

### Cómo se completan las cuatro huérfanas

**No hace falta ningún endpoint nuevo ni ninguna herramienta.** Repetí el mismo
`POST /register`, con el mismo `agentUri` y el mismo `recipient`. El facilitador
busca entre los agentes que todavía tiene a su nombre uno con esa `agentUri`, y
corre solo lo que falta: `initialize_stats` si no está, y el traspaso.

```bash
curl -sS -X POST https://facilitator.ultravioletadao.xyz/register \
  -H 'Content-Type: application/json' \
  -d '{
        "x402Version": 1,
        "network": "solana",
        "agentUri": "<LA MISMA agentUri DEL PRIMER INTENTO>",
        "recipient": "<PUBKEY DEL AGENTE, ej. Drc9BkJc… para kk-0xyuls>"
      }' | jq '{success, agentId, owner, mint}'
```

Esperá `"resumed": true` y `"status": "complete"`. Verificá con el lookup, que es
lo que ya hacen:

```bash
curl -s https://facilitator.ultravioletadao.xyz/identity/solana/owner/<PUBKEY> | jq
```

Detalles que importan:

- **Una llamada rescata *un* asset.** Cada agente tiene **dos** huérfanas, así que
  hacen falta dos llamadas por agente si quieren rescatar las dos. La primera
  adopta una, la segunda adopta la otra. Al final el agente es dueño de las dos y
  el facilitador no tiene ninguna.
- **Si con una alcanza, paren después de la primera.** El lookup va a contestar
  200 y `balance: "1"`; la segunda huérfana queda donde está. Nada la borra, pero
  tampoco molesta.
- **La `agentUri` tiene que ser byte por byte la misma.** Es el único vínculo
  entre el reintento y el huérfano: el dueño registrado es el pagador del
  facilitador, no el agente, así que no hay forma de llegar por la wallet.
- **Si la `agentUri` del primer intento venía vacía, esto no las alcanza.** Una
  URI vacía nunca adopta nada, a propósito. Avísennos y lo resolvemos por asset.
- **Si el mint traía `metadata`, el resume no la reescribe** y lo dice en
  `mint.metadataSkipped: true`. Una vez que la identidad es del agente, el agente
  la puede poner.

Los cuatro assets, para que los tengan a mano al verificar:

| Agente | Assets a nombre del facilitador |
|---|---|
| `kk-0xyuls` (`Drc9BkJc…`) | `AjANABVeKCfVn3YimmDtVC5AKj4CxKA39SHzJHxZkkY4`, `43vWz9GrY4mztfRNdFU4k3dudJDRTrjGPU9VoorWqMTs` |
| `kk-0xjokker` (`BXPKpV6f…`) | `4xQguonkrykFNxmzkNMieZ66ZQmr4ACU4HzqrHYEoHpy`, `RfkykvJAAR5Dfzwxzt76w34QxLpXxBrtzm89e9G7goK` |

**Nada de esto corrió contra mainnet.** El comando de arriba es para que lo corran
ustedes, o nosotros cuando c0der lo pida.

### Lo que este cambio *no* arregla

Su reporte pedía tres cosas y esta entrega es la segunda: **«si el mint gasless
debe pasar, que pase — y si falla, que se vea»**. Ahora se ve, con estado y con
causa.

Las otras dos son de Execution Market, no del facilitador, y siguen abiertas:
que elegir `reputation_network: solana` **falle al elegir** con un 422 si la
parte no puede recibir reputación en esa red, y que el recibo lleve
`reputation_network_requested` al lado de la usada. El facilitador ahora les da
la señal para hacer la primera: un mint que no completa responde 500 con
`mint.status`, y ya no hay excepción que tragarse.
