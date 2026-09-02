# MCP delante del facilitador: la medicion antes de escribir codigo

**Fecha:** 2026-09-02
**Rama:** `0xultravioleta/x4-mcp`, desde `0a7989fe` (merge del PR #8)
**Fase:** 0 de 4 — medir. Nada del servidor MCP esta escrito todavia.
**Encargo:** exponer `/mcp` (Streamable HTTP) con `verify` / `settle` / `supported` /
`accepts` y publicar la server-card, para que el check `mcp-server-card` de c0der
deje de estar excluido con el motivo "no tiene servidor MCP".

---

## 1. El resultado, en una linea

**rmcp 3.2.0 compila con este arbol** (`cargo check --locked -p x402-rs` con las
features de CI: **exit 0, 2m00s** contra **1m44s** de la misma corrida sin rmcp,
con cache caliente). No hace falta el JSON-RPC a mano que el encargo dejaba como
plan B.

---

## 2. Lo que hay que corregir del encargo

Tres cosas medidas que contradicen lo que decia el brief. Ninguna bloquea; dos
cambian el diseno.

### 2.1 rmcp NO depende de axum

El brief decia que rmcp "pide axum ^0.8 (features http1, tokio)". El indice de
crates.io dice otra cosa: entre las dependencias normales de `rmcp 3.2.0` **no
aparece axum**. El transporte se expone como `tower_service::Service`
(`StreamableHttpService`, `src/transport/streamable_http_server/tower.rs:1036`),
asi que se monta con `Router::route_service` y **no puede pinchar nuestra version
de axum**. Es mejor noticia que la del brief: un upgrade de axum no arrastra a rmcp.

Lo que si agrega al lock son **14 crates**: `base64 0.23` (convive con nuestro
0.22), `rand 0.10` (convive con nuestro 0.8), `schemars_derive`, `sse-stream`,
`pastey`, `darling{,_core,_macro}`, `syn 3`, `chacha20`, `cpufeatures`,
`rand_core 0.10`, `rmcp-macros`, `rmcp`. `schemars` y `chrono` ya estaban en el
lock, por eso solo entra el derive.

### 2.2 No hay ningun JSON Schema de utoipa que reusar

El encargo decia: "los inputSchema salen de los tipos que ya tiene utoipa
(VerifyRequest, SettleRequest): si utoipa ya genera el JSON Schema de esos tipos,
reusalo". **No los genera.** Medido:

```
grep -c 'ToSchema' src/types.rs src/openapi.rs
src/types.rs:0
src/openapi.rs:0
```

`src/openapi.rs` documenta `POST /verify` y `POST /settle` con
`request_body(content = Object, ...)` y un ejemplo en prosa; no hay derive
`ToSchema` en ningun tipo del protocolo. O sea: no existe el schema que reusar.

La fuente honesta que si existe es `src/types.rs`. `VerifyRequest`
(`src/types.rs:1471`) y `SettleRequest` declaran exactamente tres campos
(`x402Version`, `paymentPayload`, `paymentRequirements`), y `post_accepts`
(`src/handlers.rs:2342`) lee exactamente `x402Version` y `accepts` — y contesta
400 si falta el segundo. De ahi salen los `inputSchema`, con los objetos
anidados abiertos (`additionalProperties: true`), porque el handler acepta v1 y
v2 y auto-detecta cual es. **Ningun nombre de campo inventado.**

### 2.3 `allowed_hosts` de rmcp viene en loopback y eso es una trampa de produccion

`StreamableHttpServerConfig::default()` trae
`allowed_hosts: ["localhost", "127.0.0.1", "::1"]`
(`src/transport/streamable_http_server/tower.rs:172`) y rechaza con **403
Forbidden** cualquier `Host` que no este en la lista, antes de mirar el metodo
(`validate_dns_rebinding_headers`, `:869`). Detras del ALB el `Host` es
`facilitator.ultravioletadao.xyz`: con el default, **/mcp contestaria 403 a todo
el mundo en produccion y 200 en local**, que es exactamente la clase de fallo que
no se ve hasta despues del deploy.

El matcheo normaliza a minusculas y **una entrada sin puerto matchea cualquier
puerto** (`host_is_allowed`, `:762`), asi que `127.0.0.1` cubre `127.0.0.1:8402`.

---

## 3. Lo demas que se midio de rmcp 3.2.0

| Cosa | Medida | Donde |
|---|---|---|
| Versiones de protocolo que conoce | `2024-11-05`, `2025-03-26`, `2025-06-18`, `2025-11-25`, `2026-07-28` | `src/model.rs:170-186` |
| `ProtocolVersion::LATEST` | `2025-11-25` | `src/model.rs:175` |
| Sin estado | si: `legacy_session_mode: false` + `json_response: true` | `StreamableHttpServerConfig` |
| `GET /mcp` sin sesiones ni event store | **405** con `Allow: POST`, cuerpo `Method Not Allowed` en texto plano y **sin `content-type`** | `:1526-1537` |
| Errores de herramienta | `CallToolResult::error(...)` (lo ve el llamador) vs `Err(ErrorData)` (el cliente lo muestra opaco) | `src/model.rs:3892-3912` |
| Identidad del servidor | `Implementation::from_build_env()` usa `CARGO_PKG_VERSION` = **`0.0.0` congelado** | `src/model.rs:1428` |

Dos consecuencias directas para el diseno:

- El 405 de rmcp no sirve tal cual: el encargo pide `content-type` JSON o SSE y
  nunca HTML. Se resuelve montando `POST /mcp` como servicio y `GET /mcp` como
  handler propio (`post_service(svc).get(handler)`), que contesta 405 con un JSON
  que explica que en modo sin estado no hay stream SSE.
- La version del servidor **no** puede salir de `CARGO_PKG_VERSION` (es el
  placeholder congelado `0.0.0` de `Cargo.toml:3`). Sale de
  `crate::version::facilitator_version()`, igual que `/version` y el OpenAPI: en
  local dice `0.0.0` y en produccion la del `VERSION` file.

---

## 4. Donde se aplica el governor, y donde va `/mcp`

`src/main.rs:455-587` arma cinco configuraciones de `tower_governor`. La que
importa:

```rust
let verify_settle_config = Arc::new(GovernorConfigBuilder::default()
    .per_second(2).burst_size(30)
    .key_extractor(SmartIpKeyExtractor)
    .finish().unwrap());
let verify_settle = handlers::verify_settle_routes()
    .with_state(axum_state.clone())
    .layer(GovernorLayer::new(verify_settle_config));   // src/main.rs:585-587
```

`GovernorConfig` guarda `limiter: SharedRateLimiter`
(`tower_governor-0.8.0/src/governor.rs:257`), que es un `Arc`. **Dos
`GovernorLayer` construidos con el mismo `Arc<GovernorConfig>` comparten el
bucket**: `/mcp` con `GovernorLayer::new(Arc::clone(&verify_settle_config))`
queda en el MISMO presupuesto por IP que `/verify` y `/settle`, que es lo que
pedia el encargo. Unico cambio en `main.rs`: el `Arc` pasa a clonarse en vez de
moverse.

---

## 5. La invariante que obliga a no llamar los handlers directo

`POST /settle` no esta suelto: lleva `axum::middleware::from_fn(settle_writer_gate)`
(`src/handlers.rs:105-107`). Ese gate decide, **leyendo el body**, si el pago
apunta a una cadena EVM; si apunta y esta tarea no tiene el writer lease,
reenvia el request al holder en vez de firmar. Es lo que serializa el nonce del
unico EOA que gasta gas.

Llamar `post_settle::<A>(...)` como funcion desde el MCP **saltearia ese gate** y
dejaria que dos tareas de ECS firmaran a la vez. Por eso `/mcp` no llama a los
handlers: arma un `Request` sintetico y lo pasa por el **mismo `Router`** con
`ServiceExt::oneshot`. Middleware, validacion y forma de la respuesta salen
identicas a REST porque **son** las de REST. El governor no cuenta dos veces: el
router interno no lleva capa propia, y el request de afuera ya pago su token.

---

## 6. Entorno (para el que venga despues)

- El `.git` del worktree es un archivo que apunta a `Z:/...`; el git de WSL no lo
  resuelve y `git.exe` **no se puede ejecutar** (no hay `WSLInterop` en
  `/proc/sys/fs/binfmt_misc`, "Exec format error"). Se arregla apuntando ese
  archivo a la ruta WSL del mismo gitdir:
  `gitdir: /mnt/z/ultravioleta/dao/x402-rs/.git/worktrees/x4-mcp`.
- El checkout viene con CRLF y el git de WSL lo ve como 384 archivos modificados.
  Con `core.autocrlf=true` quedan 0. Se pasa por entorno para no escribir en el
  config comun (que vive en el checkout del dueno):
  `GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.autocrlf GIT_CONFIG_VALUE_0=true`.
- `target/` es un symlink al target del checkout del dueno (111 GB, cache caliente
  de rustc 1.97.0 linux-gnu). Por eso un check completo son ~2 minutos y no ~40.
- **El CLI de orca no corre desde esta shell**: solo hay `orca.exe`/`orca.cmd` y
  la interop de Windows esta apagada. Los heartbeats y el `worker_done` de este
  worker no pudieron enviarse por CLI; el estado queda en los handoffs.

---

## 7. Lo que sigue

Fase 1: `src/mcp.rs` con las cuatro herramientas sobre el router REST, `/mcp`
bajo el governor de verify/settle, la server-card, y los tests. Fase 2: docs y
sincronizacion (`llms-full.txt`, sha256 del indice de skills, OpenAPI). Fase 3:
verificacion en local con los cinco curl y el checker de c0der. Fase 4: handoff
de cierre.
