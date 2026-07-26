# Respuesta al feedback de Execution Market: concurrencia de settles y presión de RPC

**Fecha**: 2026-07-24
**Origen**: `docs/HANDOFF-2026-07-24-signer-pool-concurrencia.md` + backlog EM (filas 2026-07-07 #49/#50, 2026-07-20 #194, 2026-07-21 #10, 2026-07-24 #224) + `INC-2026-07-06` e `INC-2026-07-21` del repo Execution Market.
**Método**: auditoría orquestada de 5 dimensiones con verificación adversarial independiente, más pruebas directas contra producción y contra los RPC públicos de Base/Optimism.

---

## 1. Corrección al pedido original

El handoff pide construir un **signer pool por chain**. Ese pool **ya existe y está completo** en el código:

- `src/chain/evm.rs:217-222` — `signer_addresses: Arc<Vec<Address>>`, `signer_cursor: AtomicUsize`
- `src/chain/evm.rs:279-289` — `next_signer_address()` round-robin
- `src/chain/evm.rs:382` — se aplica en `send_transaction` vía `.with_from(from_address)`
- `src/from_env.rs:199-238` — `EVM_PRIVATE_KEY_MAINNET` ya acepta **lista separada por comas**

Viene de upstream (PR #16). Encenderlo no requiere cambios de Terraform ni de IAM: basta editar el secreto y forzar un deployment. Lo que falta no es el pool.

Además, la semántica de Alloy que lo sostiene es correcta: `WalletFiller` firma con la address que trae el `from` del request y falla ruidosamente si no tiene esa credencial — nunca cae en silencio al signer por defecto. Y el path principal de settle usa `transferWithAuthorization`, que es **agnóstico al `msg.sender`**; si usara `receiveWithAuthorization` un pool sería directamente imposible.

## 2. El bloqueador real: hay más de un escritor

El nonce manager (`PendingNonceManager`) y el cursor del round-robin viven **en memoria de cada proceso**. Verificado contra AWS en vivo:

```
deploymentConfiguration: minimumHealthyPercent=100, maximumPercent=200
autoscaling target:      min=1, max=3
```

Consecuencias:

- **Cada deploy rolling corre 2 tasks a la vez**, ambas firmando desde el mismo EOA único, cada una con su propio cache de nonce pendiente. Colisión garantizada en esa ventana.
- El autoscaling puede llegar a **3 escritores** legítimamente.
- Un signer pool **no arregla esto**: cada task nueva arranca su cursor en 0, así que todas eligen `signer_addresses[0]` primero.

Esto rompe hoy, con una sola llave. Es la razón por la que "20 settles concurrentes sin un solo nonce error" no se puede garantizar todavía.

**Decisión requerida del operador** (ver §6).

## 3. Lo que ya se arregló en esta sesión

### 3.1 Owner-scan roto en producción — P0, llevaba 19 días caído

Reproducido en vivo el 2026-07-24:

```
GET /identity/base/owner/0xa3279f744438f83bc75ce9f8a8282c448f97cc8a
-> 404  {"error": "... -32003: out of gas: gas exhausted during memory
                    expansion: 600000000", "balance": "14"}
```

`resolve_first_token_by_owner` (`src/handlers.rs`) armaba **un solo** `aggregate3` con `max_id` llamadas `ownerOf` — ~58.400 en el registry de Base. La optimización original (commit `f5459da6`, marzo) se calibró contra SKALE con 250 tokens; Base cruzó el techo de 600M de gas el 2026-07-05, exactamente la fecha del incidente.

Medición empírica hecha para fijar la constante:

| Límite | Valor medido |
|---|---|
| RPC de producción (Base) | revienta por gas a 58.400 llamadas (`-32003`, 600M) |
| Nodo público de Base | tope por tamaño de respuesta: **16.383** llamadas (~2,5 MB) |
| Elegido | **2.000** por lote (~6M gas, ~320 KB) |

Cambios aplicados:
- Lote acotado (`OWNER_SCAN_BATCH = 2_000`) con salida temprana al primer match, preservando la semántica de "menor agentId".
- Techo duro de lotes por scan (`OWNER_SCAN_MAX_BATCHES = 64`) para que una lectura no se convierta en una tormenta de RPC.
- Se eliminó la mentira del docstring (prometía un fallback secuencial que no existía).

### 3.2 Errores de RPC confundidos con reverts — la causa raíz real

El probe exponencial y la búsqueda binaria hacían `Err(_) => break`. Un `-32007` (rate limit) era **indistinguible** de "el token no existe", así que truncaba `max_id` en silencio y el scan devolvía un resultado incorrecto.

Verificado cómo frasean los nodos un revert real:

```
Base y Optimism: {"code":3,"message":"execution reverted",
                  "data":"0x7e273289..."}   <- ERC721NonexistentToken(uint256)
polygon-rpc.com: {"error":"message: API key disabled, ... json-rpc code: -32051"}
```

El segundo caso es justo el que no debe leerse como "token ausente". Ahora `is_execution_revert()` usa una **lista positiva**: solo un revert reconocido cuenta como "no existe"; cualquier error desconocido se trata como inconcluyente (fail-closed). Cubierto con tests.

### 3.3 El amplificador de mints duplicados

`POST /register` llamaba al scan para su chequeo de idempotencia y, ante un `Err`, **logueaba un warning y minteaba igual**:

```rust
// antes
Err(e) => { warn!(..., "proceeding with mint"); }
```

Cada duplicado engorda el registry que rompió el scan → más `-32003` → más duplicados. Es el bucle auto-amplificado que EM reportó ("ya re-minteó 2 NFTs en Base") y explica los 31 duplicados históricos del INC.

Ahora el resolver devuelve tres estados distintos y `/register` **falla cerrado** cuando no puede determinar si el recipient ya tiene identidad:

| Resultado | Significado | `/register` | `GET /identity/.../owner/...` |
|---|---|---|---|
| `Ok(Some(id))` | encontrado | devuelve el existente | `200` |
| `Ok(None)` | scan limpio, no tiene | mintea | `404` |
| `Err(_)` | **sin veredicto** | `503`, **no mintea** | `503 + retryable:true` |

### 3.4 El 404 que envenenaba la base de datos de EM

El error inconcluyente devolvía **404**. EM lee 404 como "no registrado", persiste `erc8004_agent_id = NULL` y por eso su short-circuit DB-first nunca aplicaba: cada request firmado volvía a pagar el lookup contra el RPC. Ese es el mecanismo del INC-2026-07-21 (fail-open bajo carga).

Ahora es **503 + `retryable: true`**. Esto cierra el ítem cross-repo "(a) resolver por owner" de la fila 2026-07-21 del backlog de EM.

### 3.5 Cache de resoluciones

Un scan en frío cuesta decenas de llamadas RPC, y EM hacía uno por request firmado: **6.965 lookups en 72h agotaron el presupuesto compartido de 50 req/s de Base → 258 errores `-32007`**, que a su vez mataron de hambre a `/settle`. Ese es el enlace directo entre el bug de lectura y los settles fallidos.

Se agregó un cache TTL de 5 minutos con clave `(network, registry, owner)`. La red va en la clave porque los registries ERC-8004 están desplegados en la **misma dirección determinista en todas las chains**; sin ella, un lookup de Polygon devolvería un agentId de Base. Solo se cachean resultados positivos.

## 4. Gaps confirmados que siguen abiertos

Ordenados por lo que realmente desbloquea el criterio de éxito del handoff.

| # | Gap | Sev | Evidencia |
|---|---|---|---|
| G1 | Múltiples tasks ECS, nonce state en memoria por proceso | **P0** | `terraform.tfvars:21-23`, config de deployment verificada en vivo |
| G2 | Rama de Ethereum L1 escribe el nonce a mano y **saltea** el nonce manager; `unwrap_or(0)` puede estampar nonce 0 en la TX | P1 | `src/chain/evm.rs:409-422` |
| G3 | `reset_nonce` borra de plano la marca de agua con TXs en vuelo | P1 | `src/chain/evm.rs:510/517` |
| G4 | Cero resiliencia de RPC: sin `RetryBackoffLayer`, sin clasificar `-32007`/`-32003`/429 | P1 | `src/chain/evm.rs:241-244` |
| G5 | `is_nonce_error` no matchea "already known" suelto ni "nonce too high" | P2 | `src/chain/evm.rs:568-573` |
| G6 | Guard pre/post nonce con `unwrap_or(0)` suprime el retry legítimo justo bajo rate limit | P1 | `src/chain/evm.rs:393-397, 523-527` |
| G7 | Escrituras ERC-8004 (`giveFeedback`, `revokeFeedback`, `appendResponse`, attestations) **saltean** `send_transaction`: sin reset de nonce y **sin timeout de receipt** | P1 | `src/handlers.rs:2675, 2927, 3189`; `src/discovery_attestation.rs:208-211` |
| G8 | Sin test de concurrencia ni harness local (no hay anvil ni dev-dependencies) | P1 | `Cargo.toml` |
| G9 | Sin observabilidad por signer; `signer_address()` solo reporta el default | P2 | `src/chain/evm.rs:576-580` |
| G10 | Monitor de balances cubre **una** address hardcodeada | P2 | `lambda/balances/handler.py:72` |
| G11 | `scripts/rotate_wallet.py` rota el secreto **equivocado** (`facilitator-evm-private-key`, legacy) | P2 | `scripts/rotate_wallet.py:57` |

## 5. Bloqueadores duros antes de encender pool > 1

Ninguno aplica hoy (pool = 1 llave), pero son GO/NO-GO:

- **H2 — UPTO**: `src/upto/permit2.rs:398-404` simula con el signer por defecto y transmite con el rotado. El proxy declara `error UnauthorizedFacilitator`. `ENABLE_UPTO=true` en producción. El código fuente del proxy no está en este repo → **hay que leer el bytecode desplegado** antes de decidir.
- **H3 — PaymentOperator**: en SKALE Base la `refundInEscrowCondition` es `OrCondition([CONDITION_RECEIVER, StaticAddressCondition(0x1030…13C7)])`. Los operators de las otras 10 redes los desplegó el merchant → hay que leer `RELEASE_CONDITION()` / `REFUND_IN_ESCROW_CONDITION()` on-chain en cada una.
- **H5 — ERC-8004 nunca debe rotar**: el feedback está atado al autor (`msg.sender`). `revokeFeedback` solo lo puede emitir quien escribió el feedback original, y las attestations del Bazaar publican **una** address de reviewer. Rutear estas escrituras por `send_transaction` para ganar el reset de nonce (G7) **sin fijar el `from`** rompería silenciosamente la revocación y partiría la identidad del attester en N direcciones.
- **H4 — fondeo**: 3 llaves × 14 mainnets EVM = 42 EOAs a fondear y monitorear. Hoy no existe ningún `get_balance` en `src/`, ni preflight de arranque, ni alarma.

## 6. Secuencia recomendada

1. **Decidir el modelo de escritor único (G1)** — es la precondición para que cualquier otro fix de nonce signifique algo. Opciones:
   - (a) barato: fijar `min=max=desired=1` y configurar el deployment para que nunca solapen dos writers;
   - (b) durable: mover la asignación de nonces a estado compartido (la tabla DynamoDB ya existe);
   - (c) si se quiere throughput: shardear el pool por índice de task.
2. **Endurecer el carril de nonces (G2 + G3 + G6)** — todo en `src/chain/evm.rs`. Esto es lo que de verdad elimina `nonce too low` a 20 settles concurrentes.
3. **Resiliencia de RPC (G4 + G5)** — `RetryBackoffLayer` + clasificador de rate limit con backoff y jitter. Fix directo del `-32007` de EM.
4. **Test de aceptación (G8)** — anvil + 20 settles concurrentes con 1 llave y con 3. Sin esto el criterio de éxito del handoff no es verificable.
5. **Observabilidad por signer (G9)** — es cómo se prueba en producción que el paso 2 funcionó.
6. **Solo entonces**: preparar pool > 1 (sizing por chain, preflight de balances, alarmas, tooling).
7. **Antes de encenderlo**: auditar los contratos atados a `msg.sender` (H2, H3), fijar sender explícito en UPTO/operator/escrow, y mantener toda escritura ERC-8004 pineada al signer por defecto (H5).

Los límites de gas explícitos (propuesta 3 del handoff) quedan al final: ahorran un round-trip por settle, pero no arreglan ningún fallo de nonce.

## 7. Preguntas abiertas para EM

- ¿Qué chain y qué endpoint exactamente? El defecto de Ethereum L1 (G2) solo dispara en `network=ethereum`, y el `-32003` del Multicall3 nace en un `eth_call` de **lectura** dentro de `/register` — nunca toca el carril de nonces.
- ¿Corría más de una task ECS durante la ventana del incidente? Si sí, G1 es la causa raíz y casi todo lo demás es secundario. Se puede verificar desde los eventos del servicio ECS.
- Los datos por chain que se le pidieron a KK (chain, count, error code, timestamp) siguen siendo la base para decidir el upgrade de tier de RPC. Con lo hallado aquí, buena parte del volumen `-32007` de Base era **auto-infligido** por el owner-scan roto, así que conviene re-medir después de desplegar §3 antes de comprar tier pago.
