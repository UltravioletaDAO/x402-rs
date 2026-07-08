# Handoff — Tareas del facilitador tras postmortem ERC-8004 (2026-07-08)

> **ESTADO (2026-07-08): P1 + P2 + P3 IMPLEMENTADOS en v1.48.0.** `POST /register`
> ahora es async-opcional (`Prefer: respond-async` → `202` + `jobId`, poll en
> `GET /register/status/{jobId}`), la espera de receipt está acotada por
> `TX_RECEIPT_TIMEOUT_SECS` en sync y async (P2), y un lock in-flight por
> `network|agentUri|recipient` frena el doble-mint concurrente (async devuelve el
> job existente, sync devuelve `409`). Core EVM extraído a `run_evm_registration`
> (compartido). Nuevo módulo `src/erc8004/register_jobs.rs` (job store in-memory,
> TTL 1h, 3 tests). El path sync por defecto conserva su contrato (SDK-compat).
> Ver commit + `/ship`.

**TL;DR:** El postmortem con Execution Market y KarmaCadabra cerró con **una sola tarea comprometida de mi lado** (`/register` async+pollable, P1) y **dos follow-ups opcionales** (P2 investigar techo ~28s, P3 idempotencia server-side). El bug de los "5 mints stuck" **no era del facilitator** — era retry-on-504 sin idempotencia del lado de exec-market; los 7 workers ya tenían su NFT on-chain (`balanceOf=1` verificado). Ninguna tarea se buildea/deploya automáticamente: eso lo hace el usuario manual.

Contexto completo en memoria: `~/.claude/projects/.../memory/erc8004-register-idempotency.md`. Canal: IRC `#Agents` (meshrelay), sesión 3-agentes.

---

## Estado del código verificado (2026-07-08, contra `main`)

Antes de tomar cualquier tarea, esto es lo que **ya existe** en el código (verificado leyendo `src/handlers.rs` y `src/chain/evm.rs` esta sesión — cambia el alcance de P2 y P3):

1. **Idempotencia por recipient YA EXISTE** — `post_register` (EVM) hace un check `balanceOf(recipient)` + `resolve_first_token_by_owner(...)` **antes** de mintear (`src/handlers.rs:4387-4439`). Si el `recipient` ya posee un agente, responde `200` con el `agent_id` existente sin mintear. **PERO** es un check de *estado ya confirmado on-chain*: no defiende contra el reintento concurrente en vuelo (mint #1 aún sin confirmar → `balanceOf` sigue 0 → el retry mintea de nuevo y revierte). Esto es exactamente el patrón del incidente y por qué **P1 (async) es el arreglo de raíz**, no la idempotencia. Además, el check solo dispara si `recipient` es `Some(Evm(..))`; y **no hay** idempotencia keyed por `agent_uri`.
2. **El techo ~28-30s ya tiene fuente localizada** — el `get_receipt` de `/settle` está envuelto por un timeout configurable `TX_RECEIPT_TIMEOUT_SECS` (default 30s; Base=90s, Ethereum=900s) en `src/chain/evm.rs:487-501`. `28138ms` cae justo bajo ese muro de 30s. **Inconsistencia clave:** el `get_receipt` de `/register` (`src/handlers.rs:4538` y el del transfer en `4691`) NO usa `.with_timeout(...)` — corre con el default de alloy. Ver P2.

---

## P1 — `/register` async + pollable  ⬅ COMPROMETIDO en el canal

**QUÉ:** Convertir `POST /register` (hoy síncrono: mint + transfer, p95=28.1s) en asíncrono. Enviar la tx, responder de inmediato con `tx_hash` (o `job_id`), y exponer un status pollable donde eventualmente aparece el `agent_id` final.

**POR QUÉ:** El p95 del mint (28.1s) pega justo contra el timeout de 30s de exec-market → 504. El mint **sí** tiene éxito on-chain, pero el 504 dispara reintentos que revierten (`execution reverted`, error code 3) por la unicidad del registry. Sacar mi latencia on-chain de su critical path elimina el 504 de raíz, sin que ellos toquen su timeout.

**RIESGO si se hace mal:** romper la compatibilidad del SDK (Python + TypeScript envuelven este endpoint) o introducir doble-mint si se pierde la idempotencia por `agent_uri`.

**Opciones de diseño (decisión abierta — no elegir en silencio):**
- **A (mínima):** devolver el `tx_hash` del mint tras enviarlo sin esperar confirmación; el cliente pollea la cadena. Problema: hay **2 tx** (mint + transfer del NFT al recipient) y el `agent_id` solo se conoce tras confirmar el mint (evento `Registered`). El cliente quedaría sin `agent_id` fácil.
- **B (job store, recomendada):** devolver `job_id`; estado `{pending → mint_confirmed → transferred → done | failed}` en un store (in-memory con TTL, o DynamoDB si se quiere durabilidad). Nuevo endpoint `GET /register/status/{job_id}`. Más completo, más código.
- **C:** mantener síncrono pero responder `202 + Location` cuando detecta lentitud. Poco elegante.

**Criterio de éxito / verify:**
1. `POST /register` responde en <2s con `tx_hash`/`job_id` (antes de la confirmación on-chain).
2. El status pollable devuelve el `agent_id` final tras confirmación.
3. Idempotente por `agent_uri`: reintentar un registro en curso/ya hecho NO produce doble-mint.
4. Test: registrar en base y comprobar que la respuesta llega antes que la confirmación y que el status eventualmente da `agent_id`.

**Archivos:**
- `src/handlers.rs:4147` — `post_register` (hoy síncrono, mint+transfer).
- `src/handlers.rs:183` — `erc8004_write_routes()` (aquí se monta `POST /register`; añadir la ruta de status).
- `src/main.rs:378` — monta `erc8004_writes` (gateado por env `ENABLE_ERC8004_WRITES`).
- `src/erc8004/mod.rs`, `src/erc8004/abi.rs` — lógica/ABI de registro.
- `src/openapi.rs` — documentar el nuevo endpoint de status (la versión auto-sincroniza de `Cargo.toml`).

---

## P2 — Techo interno ~28-30s (28138ms recurrente)  · CASI RESUELTO en esta sesión

**HALLAZGO:** El techo es la **espera de confirmación de tx (receipt)**, no un timeout de RPC HTTP. El valor `28138ms` idéntico al milisegundo en p95 de `/register` y p99 de `/settle` es el tell de un tope compartido, no timing on-chain aleatorio.

- `/settle` envuelve `get_receipt` con un timeout **configurable** `TX_RECEIPT_TIMEOUT_SECS`: default **30s**, con overrides por red (Base=90s, Ethereum=900s). Ver `src/chain/evm.rs:487-501`. `28138ms` cae justo bajo ese muro de 30s.
- `/register` (`src/handlers.rs:4538`, y el transfer en `4691`) NO aplica ese timeout — usa el `get_receipt()` default de alloy. Corre con el mismo comportamiento de ~30s por otra vía, de ahí que ambos p-tails converjan al mismo número.

**LO QUE FALTA (decisión, no investigación):**
1. Uniformar el receipt-wait de `/register` para que use `TX_RECEIPT_TIMEOUT_SECS` como hace `/settle` (hoy es inconsistente).
2. Decidir el default para el mint en base: si P1 saca el receipt-wait del critical path (async), este techo deja de importar para latencia percibida — se vuelve solo el corte interno del job.

**Archivos:** `src/chain/evm.rs:482-511` (fuente del timeout de settle, ya confirmada), `src/handlers.rs:4538` + `4691` (register/transfer sin timeout — a uniformar), `src/from_env.rs` (si se decide exponer el override por red).

---

## P3 — Idempotencia server-side de `/register`  · PARCIALMENTE HECHA — cerrar el hueco de la carrera

**YA IMPLEMENTADO (verificado):** El check `balanceOf(recipient)` + `resolve_first_token_by_owner(...)` ya vive en `src/handlers.rs:4387-4439`. Un `/register` de un recipient que **ya** posee agente devuelve `200` con el `agent_id` existente, sin revert ni gas. Es la variante "por recipient".

**HUECO REAL (lo que queda):**
- **Carrera en vuelo:** el check solo ve estado ya confirmado. Dos requests para el mismo agente dentro de la ventana de confirmación (≤~28s) pasan ambos el check con `balanceOf=0` y el segundo mint revierte — el patrón exacto del incidente. Defensa: un **lock de registro pendiente** (in-memory por `agent_uri`/recipient con TTL) que serialice o rechace el duplicado mientras el mint #1 está in-flight. Encaja natural con el job store de **P1-B**.
- **Idempotencia por `agent_uri`:** hoy no existe (solo por recipient, y solo si `recipient` es `Some(Evm)`). Registros sin recipient, o con recipient distinto pero mismo `agent_uri`, no están cubiertos.

**DECISIÓN DE PRODUCTO:** ¿el lock/idempotencia extra vive server-side, o se deja al cliente (exec-market ya adoptará su propio `balanceOf`-check pre-retry)? **Preguntar antes de codear** — puede solaparse con P1 y no valer código aparte.

**Criterio:** dos `POST /register` concurrentes del mismo `agent_uri` producen **un** mint (el segundo recibe el `agent_id` en curso/existente, sin revert ni gas). Test: disparar 2 registros en paralelo en base y verificar un solo `Registered`.

**Archivos:** `src/handlers.rs:4387-4439` (idempotencia por recipient existente — extender), `src/handlers.rs:4147` (`post_register`), `src/erc8004/abi.rs` (resolver por `agent_uri` si se quiere esa clave).

---

## Lo que NO es mío (para claridad del handoff)

- **Execution Market:** desacoplar el write-path con SQS (`create_task` hace verificación de identidad on-chain + verify inline; `assign` hace escrow-lock inline → se apila con 25 agentes). Workaround acordado: bajar concurrencia de publishes/assigns a ~2-3. Adoptar `balanceOf(worker)>0` en el registry **antes** de cualquier retry de register (selector `0x70a08231`).
- **KarmaCadabra:** cerrar los 5 "stuck" como **registrados** (ya verificado on-chain).

---

## Datos de referencia (verificados esta sesión)

**Latencias `/ecs/facilitator-production`, 24h:**

| Endpoint | n | p50 | p95 | p99 | max |
|---|---|---|---|---|---|
| POST /register | 19 | 3.5s | 28.1s | 28.1s | 28.1s |
| POST /verify | 1 | 41ms | 41ms | — | 41ms |
| POST /settle | 112 | 5.4s | 7.2s | 28.0s | 28.2s |

**Registry ERC-8004 (base, determinista CREATE2 en todas las chains):** `0x8004A169FB4a3325136EB29fA0ceB6D2e539a432`
**Facilitator wallet (msg.sender del mint):** `0x103040545AC5031A11E8C03dd11324C7333a13C7`
**Ventana del incidente:** 2026-07-08 register ~03:45–04:10 UTC; 33 intentos en base para 7 workers; 2 confirmados in-window, resto revertidos por duplicado. Los 7 con `balanceOf=1`.

**Nota operativa:** los logs del facilitator traen códigos ANSI que rompen el parseo `key=value` en CloudWatch Logs Insights. Parsear por frases contiguas (ej. `"Registration transaction confirmed"`, o `POST /register` como literal) es robusto.
