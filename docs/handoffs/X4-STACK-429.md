# X4-STACK-429 — el facilitador nunca le da un 429 de política a nuestro propio stack

**Encargo:** c0der, 2026-09-25 (ticket de KarmaKadabra, `inbox/2026-09-25-karmakadabra-facilitador-sin-429-para-el-stack.md`).
**Base:** `origin/main` = `f3786f3e` (re-medido al llegar). **Rama:** `0xultravioleta/x4-stack-429`, commits locales, sin push.

## Estado

- **Hecho.** Exención por identidad autenticada (`X-UVD-Stack-Key`, una clave por servicio, guardada en el
  facilitador solo como SHA-256) que salta los **ocho** governors por IP con un único mecanismo
  (`rate_policy::PolicyLayer`). Tope global nuevo de peticiones en vuelo (`503` + `Retry-After`, para todos,
  stack incluido). Presupuestos, identidades y tope en un solo módulo (`src/rate_policy.rs`) con default en
  código y override por env. `GET /config` publica los valores en vigor (identidades por nombre, nunca claves).
  El tope diario de gas ERC-8004 **no** cambia de comportamiento ni se exime (solo gana un accesor de lectura,
  `DailyWriteCap::configured`, para publicarlo en `/config`). 17 tests nuevos de `rate_policy` + 1 de MCP,
  mutaciones corridas (tabla abajo), suite de CI local en verde.
- **Ronda 1 (REF-X4-STACK-429, firmada por c0der) — hecha.**
  - **P1-1:** `admit` toma el cupo global recién cuando el cuerpo llegó entero, con un plazo **total** de 5 s (`408
    request_timeout` y cierre), y suma un techo de 32 requests en vuelo por dirección (`429
    too_many_concurrent_requests`) que el stack con clave válida saltea (decisión de c0der); el tope global sigue
    para todos. Hay un test sobre TCP real.
  - **P2-1:** el tope de gas queda atado por un test de comportamiento y uno de fuente.
  - **P2-2:** M12, M14 y M15 ya no compilan (tipo opaco `Bucket` y constructores privados), y M13 y la variante de
    M15 que sí compila en tests dan rojo.
  - **P3:** los cinco van.
  - Las tablas de la ronda están en §9 y §10.
- **Falta (no es mío hacer):** crear los cuatro secretos, cablearlos en terraform, desplegar; cambio en el SDK
  (py y ts) y en los clientes. **Decisión pendiente del dueño:** si el stack tiene un tope diario de gas propio
  más alto (números abajo). Sin esa decisión el relleno de KK choca con el tope de Ethereum (100/día).
- **Próximo paso:** c0der refuta la ronda 1; si pasa, un único CI + deploy del facilitador con la lista vacía
  (para terceros solo cambian la admisión de §3 y `/config`), y después el orden de la sección "Orden del deploy".

## 1. Lo medido en `origin/main`

Ocho presupuestos por IP, todos con `ClientIpKeyExtractor` (última entrada de `X-Forwarded-For`, si no el peer):

| Presupuesto | Dónde (origin/main) | Período / burst | Rutas |
|---|---|---|---|
| verify_settle | `main.rs:593` | 2 s / 30 | `/verify`, `/settle`, `/receipts*`, `POST /mcp` (misma cubeta) |
| discovery_register | `main.rs:627` | 12 s / 250 | `/discovery/register`, admin del bazar |
| discovery_read | `main.rs:644` | 200 ms / 120 | lecturas del bazar, `/transactions`, `/api/stats*`, `/dx402/*` |
| events | `main.rs:659` | 2 s / 10 | `GET /events` |
| identity_read | `main.rs:681` + `handlers.rs:145` | 500 ms / 60 | `/identity/*` |
| secondary_read | `main.rs:706` + `handlers.rs:223` | 300 ms / 100 | `/reputation`, `/blacklist`, `/escrow/state`, `/health/ready`, 404 |
| human_pages | `handlers.rs:290,336` | 500 ms / 60 | las 9 páginas HTML |
| erc8004_writes | `handlers.rs:2148,2167` | 12 s / 30 | `/register`, `/feedback`, `/feedback/*` (prepare **y** submit) |

Además, fuera de los governors:

- **Tope diario ERC-8004** (`src/erc8004/daily_cap.rs`): `429 erc8004_daily_write_limit`. Protege gas. Sin tocar.
- **No existía** ningún tope global de concurrencia: los únicos semáforos son por función (salud del bazar,
  `erc8004/summary.rs`) y el cupo de suscriptores de `/events`.
- Ningún otro productor de 429 en `src/` (grep de `TOO_MANY_REQUESTS`: solo tests y el mapeo de errores de RPC).

**Por qué KK se queda sin cupo** (leído en los repos, commits en la nota del final): los scripts de relleno de
KK no llaman al facilitador; llaman a EM (`POST /reputation/relay/prepare` y `/relay/submit`,
`karmakadabra services/em_client.py:2250,2269`). EM reenvía cada rating como **dos** requests al facilitador
(`mcp_server/integrations/erc8004/relayed_feedback.py:64,71` → SDK `erc8004.py:1341` `/feedback/evm/prepare` y
`:1456` `/feedback/evm/submit`), las dos bajo `erc8004_writes` y cargadas a las IPs de salida de EM: 30 de burst y después una cada 12 s para todo el ecosistema a la vez. EM no distingue el 429: cualquier
fallo del cliente del facilitador termina en `503` (`execution-market mcp_server/api/reputation.py:4190-4193,4228`),
que es el `503 :: ... 429` que vio KK.

## 2. Decisión: (a) una clave por servicio, validada contra SHA-256 en tiempo constante

| Criterio | (a) clave por servicio (elegida) | (b) firma de wallet en allowlist |
|---|---|---|
| Revocar una filtrada sin redeploy de los demás | Sí: vaciar el `sha256` de ESE servicio y reiniciar el facilitador (misma imagen). EM/KK/describe/meshrelay no se tocan. | Sí, igual (quitar la dirección). |
| Que nunca se loguee | El facilitador **no tiene** la clave: solo su SHA-256. No puede imprimirla ni por error. El header se marca `sensitive` al entrar (`Debug` imprime `Sensitive`). | Mejor en teoría (la firma caduca), pero el facilitador tiene que parsear y loguear menos. |
| Fuerza bruta | 256 bits aleatorios; el formato (`uvdsk_` + ≥43 base64url) se exige antes de hashear, así que una clave débil no autentica nunca; cada intento fallido se cobra como tercero (governor por IP). | Imposible. |
| Costo de implementación | ~1 módulo, sin crates nuevos, sin reloj ni nonces. Cliente: un header. | Parsear RFC 9421 + EIP-191 en Rust, anti-replay con nonce/ventana, reloj sincronizado, y firmar **cada** request en py/ts (el ERC-8128 del SDK existe pero no está cableado al cliente del facilitador). |

Lo que decide: la credencial solo compra la exención de una **política** por IP. No mueve fondos, no firma nada,
no salta el tope de hardware ni el de gas. Una clave filtrada, en el peor caso, deja a un tercero hacer lo que
hoy hace cualquiera desde muchas IPs, con el techo del tope global y del tope diario. (b) cuesta un orden de
magnitud más para proteger ese mismo valor. Si algún día la exención cubre algo que cueste plata, (b) es el camino.

Detalles que el refutador puede verificar en `src/rate_policy.rs`:

- Se compara el SHA-256 de los bytes **exactos** del header contra **todos** los digests configurados con
  `subtle::ConstantTimeEq`, sin salida temprana. Dos líneas del header, espacios, prefijos, sufijos, mayúsculas,
  no-UTF-8 o el digest en vez de la clave: tercero (test `a_malformed_or_false_key_is_a_third_party_not_a_500`).
- Con cero identidades configuradas no se hashea nada y nadie es exento (el primer deploy).
- Una clave rechazada se loguea la primera vez y después cada 100, con `client_ip` y `path` y **sin valor**
  (lo loguea `admit`, una vez por request; `authenticate` no loguea).
- La respuesta a un exento lleva `x-ratelimit-exempt: <servicio>` (la base de la sonda). Nunca la clave.

## 3. El mecanismo: una sola capa para todos los governors

Un `KeyExtractor` solo elige cubeta; no puede saltar el governor (una cubeta propia seguiría cortando al stack
en su burst). Por eso `PolicyLayer` envuelve al `GovernorLayer`: guarda el servicio gobernado y el desnudo y elige
por request. `RatePolicy::layer(&bucket)` es la **única** forma de montar un governor, y lo sostienen los tipos
(ronda 1, P2-2):

- `rate_policy::config(limit)` devuelve un `Bucket` opaco (campo privado; el tipo de config de `tower_governor`
  es privado del módulo). Un `GovernorLayer` hecho con él no compila (M12).
- `RatePolicy::new` y `StackIdentities::from_lookup` son privados: fuera del módulo solo hay
  `RatePolicy::from_env()`, así que montar un governor con **otra** política que no exime a nadie no compila en
  `cargo build` (M14, M15, M16). Para tests fuera del módulo hay `RatePolicy::none()` y `RatePolicy::for_tests(..)`,
  los dos `#[cfg(test)]`: no existen en el binario.
- `rate_policy::config(limit)` es el único `GovernorConfigBuilder` de `src/` (`client_ip::every_governor_keys_on_the_client_ip`
  lo exige: exactamente uno, en `rate_policy.rs`, con `ClientIpKeyExtractor`).

Lo que el tipo no ve, lo ve `rate_policy::every_governor_goes_through_the_policy`, que lee la fuente:

- En las líneas de código (fuera de `rate_policy.rs` y `client_ip.rs`) no aparece `GovernorLayer`,
  `GovernorConfig`, `tower_governor::governor`, `tower_governor::key_extractor`, `tower_governor as`, ni un
  `use tower_governor…` que no sea `…::GovernorError` (así se atajan también los alias).
- Cada presupuesto está usado (`rate_policy::<PRESUPUESTO>.limit()`), y cada `let <x> = rate_policy::config(…)` de
  `main.rs` se dimensiona con uno de ellos **y está montado** al menos una vez como `policy.layer(&<x>)` (M13).
  Ojo: el test ve que el bucket se monta, no **qué** rutas monta.
- `admit` queda dentro del layer de tracing y `/config` está montado.

Y dos tests de comportamiento sobre los routers que arma `handlers` con la política de verdad:
`the_human_pages_exempt_the_stack_too` y `erc8004_write_rate_tests::the_erc8004_writes_exempt_the_stack` (el camino
de los ratings de KK: 35 POST del stack sin 429 y un tercero con 429 en el 31; M15b).
- Los dos saltos entre tasks conservan la identidad: `forward_to_writer` copia todos los headers (la exención se
  repite en el holder) y la request sintética de MCP ahora copia `X-UVD-Stack-Key` además de `X-Forwarded-For`
  (`mcp::a_forwarded_settle_carries_the_stack_key_to_the_lease_holder`).

| | tercero | stack con clave válida |
|---|---|---|
| presupuestos por IP (8) | `429 rate_limited` al pasar el burst | **nunca** |
| techo en vuelo por dirección (`MAX_INFLIGHT_PER_CLIENT`, 32) | `429 too_many_concurrent_requests`, `Retry-After: 1` | **nunca** (es política) |
| plazo del cuerpo (`REQUEST_BODY_DEADLINE_MS`, 5000) | `408 request_timeout` y cierre | **igual** |
| tope global en vuelo (`MAX_INFLIGHT_REQUESTS`, 512) | `503 overloaded`, `Retry-After: 1` | **igual** (es hardware) |
| tope diario ERC-8004 (gas) | `429 erc8004_daily_write_limit` | **igual** (es plata) |
| throttle de RPC (`RPC_MAX_CU_PER_SECOND`) | sin cambios | sin cambios |

**La admisión es comportamiento nuevo para todos**, en este orden (`rate_policy::Admission`, `admit`), antes de
que corra ninguna ruta y con `/health` siempre afuera:

1. **Techo por dirección, 32.** Para que una IP no pueda llenar el tope global (hacen falta 16). Terceros sí, stack
   no: todo el stack sale por las IPs de EM y un límite por IP es política (decisión de c0der). Por encima del
   burst de todos los presupuestos salvo el del bazar, así que un integrador normal no lo ve.
2. **El cuerpo entero, con plazo total de 5 s.** Para todos. **Total**, no entre frames: un
   `RequestBodyTimeoutLayer` de tower-http se reinicia en cada frame, y un goteo de un byte cada pocos segundos lo
   mantiene vivo para siempre. Por eso no uso la feature `timeout` (y no toca el lock por eso). Pasado el plazo:
   `408` + `Connection: close`. Mientras tanto no se retiene nada global. Un cuerpo cortado por el límite de 64 KiB
   pasa a ser un `413 payload_too_large` en JSON (`a_body_past_the_limit_is_a_json_413`).
3. **Tope global, 512.** Para todos. El cupo se toma **recién ahora**, con el cuerpo ya adentro, y se retiene hasta
   producir la respuesta (no mientras un stream escribe: un `/events` abierto no lo ocupa). 1 vCPU / 2 GB
   (tfvars), cuerpos de 64 KiB como máximo (32 MiB a 512), y la carga medida está dos órdenes por debajo (216
   settles en 4 h, 2026-08-20).

El ataque de P1-1 (subidas con headers y sin cuerpo) ya no toca el tope global: esas subidas esperan el cuerpo
**antes** de pedir cupo y mueren a los 5 s con `408`. Lo que les queda es el techo de su propia dirección
(`over_real_tcp_an_upload_that_never_arrives_holds_no_slot`, sobre `axum::serve` real). `/health` nunca se
rechaza: si el ALB viera 503 en una task ocupada, ECS la reemplazaría y se llevaría su capacidad. `admit` vive
dentro del layer de tracing, así que cada rechazo se loguea con su `status=`.

**P1-1 d), el ALB (HIPÓTESIS, no medida):** no pude medir si el ALB bufferiza el cuerpo antes de pasarlo al target:
haría falta un ALB que no sea el de producción, y el encargo prohíbe AWS. Con este diseño la respuesta ya no decide
si el tope global se puede secuestrar (no se puede: el cupo se toma después del cuerpo). Solo decide si una subida
lenta llega a la task: si el ALB bufferiza, no llega; si reenvía en streaming, llega y muere a los 5 s con `408`.
La medición barata, cuando haya un ALB de prueba, es mandar headers con `Content-Length` sin cuerpo y ver en el
log del target si aparece la request antes del idle timeout del ALB.

## 4. Configuración, en un solo lugar (`src/rate_policy.rs`)

| Variable | Default en código | Qué |
|---|---|---|
| `VERIFY_SETTLE_RATE_PER_MS` / `_BURST` | 2000 / 30 | nuevas |
| `DISCOVERY_REGISTER_RATE_PER_MS` / `_BURST` | 12000 / 250 | nuevas |
| `DISCOVERY_READ_RATE_PER_MS` / `_BURST` | 200 / 120 | nuevas |
| `EVENTS_RATE_PER_MS` / `_BURST` | 2000 / 10 | nuevas |
| `IDENTITY_READ_RATE_PER_MS` / `_BURST` | 500 / 60 | **mismos nombres que antes** |
| `SECONDARY_READS_RATE_PER_MS` / `_BURST` | 300 / 100 | **mismos nombres que antes** |
| `HUMAN_PAGES_RATE_PER_MS` / `_BURST` | 500 / 60 | **mismos nombres que antes** |
| `ERC8004_WRITES_RATE_PER_MS` / `_BURST` | 12000 / 30 | nuevas |
| `MAX_INFLIGHT_REQUESTS` | 512 | tope global; inválido → default con warning |
| `MAX_INFLIGHT_PER_CLIENT` | 32 | techo por dirección (terceros); inválido → default con warning |
| `REQUEST_BODY_DEADLINE_MS` | 5000 | plazo total del cuerpo; inválido → default con warning |
| `UVD_STACK_SERVICES` | `execution-market,karmakadabra,describe-net,meshrelay` | lista de servicios |
| `UVD_STACK_KEY_SHA256_<SERVICIO>` | vacío (inactivo) | digests SHA-256 hex, separados por coma (dos durante una rotación) |

Ningún default cambió. Un override que no parsea a entero positivo se ignora (misma semántica de antes).
`GET /config` (gobernado como las lecturas baratas) publica: cada presupuesto con su valor efectivo, su default y
sus variables; `stackIdentities` (`active`, y por servicio `name`, `active`, `credentials` = cuántos digests);
`overload` (el tope global, `perClient`, `bodyDeadlineMs` y qué saltea el stack); y el tope diario por red. Nunca una clave ni un digest (test `the_key_never_reaches_a_log_or_a_response`).

## 5. Tope diario de gas ERC-8004 — medido, **no** eximido

- **Defaults** (`daily_cap.rs:59,65-70`): 1000 escrituras por red y día UTC; `ethereum` 100, `solana` 150, `arc` 100,
  `arc-testnet` 100. **Producción no tiene overrides** (grep de `ERC8004_DAILY_WRITE_CAP` en
  `terraform/environments/production/*.tf`: cero).
- **Qué cuenta:** transacciones enviadas, no requests (`prepare` no cuenta; un `submit` = una tx). Vive **dentro**
  del writer lease (`handlers.rs`, `erc8004_write_routes`: `daily_cap::enforce` bajo `require_writer_lease`), así
  que en operación normal es un contador por servicio (el del holder), no por task.
- **Lo ata un test (ronda 1 P2-1)**, no solo la construcción:
  - `handlers::erc8004_write_rate_tests::a_stack_identity_still_spends_the_daily_gas_cap` monta envíos con
    `enforce_with` y un tope de 1 para `base` bajo `erc8004_write_governed` con la política del stack. El primer
    POST del stack da `200` con `x-ratelimit-exempt` y el segundo `429 erc8004_daily_write_limit`.
  - `rate_policy::the_gas_cap_knows_nothing_of_the_stack` exige que `daily_cap.rs` no nombre el header, ni
    `STACK_KEY_HEADER`, ni `rate_policy`.
  - M08 y M09 dan rojo.
- **Costo de una escritura (MEDIDO):** el único `/feedback/evm/submit` con hash en el repo
  (`docs/handoffs/2026-08-25-calificar-cambia-la-cuenta-y-el-sdk-ts.md:31`, hash completo ahí), tx
  `0x6e7cbe77…c0021e142482` en Base, bloque 50442319: `gasUsed` 246.452,
  `effectiveGasPrice` 0,006 gwei, `l1Fee` 14.086.789.311 wei → 1,49e-6 ETH ≈ **US$0,004** (ETH a US$2.680,89,
  CoinGecko, leído 2026-09-25 22:01Z; recibo leído de `mainnet.base.org`).
- **Otras redes (ESTIMADO):** los 246.452 gas medidos en Base por el gas price público del 2026-09-25 22:01Z; sin el
  L1 fee de las OP-stack:

  | Red | gas price (gwei) | ≈ US$ por escritura |
  |---|---|---|
  | ethereum | 0,123 | 0,082 |
  | arbitrum | 0,020 | 0,013 |
  | bsc | 0,05 | 0,0095 |
  | polygon | 279,3 | 0,0087 |
  | celo | 202,5 | 0,0047 |
  | base | 0,006 | 0,004 (medido, con L1 fee) |
  | optimism | 0,001 | 0,0007 + L1 fee |
  | monad | 102,0 | 0,0007 |

- **El relleno de KK (141 + 936 = 1077 ratings):** van por el riel firmado, así que son 1077 tx y 1077 cupos del
  tope diario, en las 8 redes con `FeedbackDelegate` (arbitrum, base, bsc, celo, ethereum, monad, optimism,
  polygon; `karmakadabra agents_sdk/rating_rail.py:92`). La red la fija el `payment_network` de cada tarea. Con la
  única distribución fechada que hay (el barrido del 2026-08-26, **n=38**: base 45 %, arbitrum 16 %, ethereum 13 %…),
  ~142 caerían en Ethereum: **al menos 2 días UTC solo por Ethereum**, y más si la flota viva también califica
  allí ese día. Las demás redes entran en un día. Costo total del orden de **US$15–17**, casi todo Ethereum
  (estimación sobre estimación: tomarlo como orden de magnitud).
- **La flota viva** (27 agentes) califica por defecto por el riel `legacy` (`KK_RATING_RAIL`, `agents_sdk/tools.py:3682`):
  EM → `POST /feedback` del facilitador, que **también** consume el tope diario de su red. No encontré una tasa
  diaria medida de la flota: es un hueco, no una cifra.

**Para c0der y el dueño:** el gas no es lo que limita (centavos); el tope sí, en Ethereum. Opciones, ninguna
implementada: (1) subir `ERC8004_DAILY_WRITE_CAP_ETHEREUM` durante la ventana del relleno (una variable, sin
código); (2) un tope propio del stack más alto, que exigiría pasar la identidad al `daily_cap` (código nuevo);
(3) dejarlo y que el relleno tarde 2+ días. Recomiendo (1): es reversible y no mezcla la identidad con la plata.

## 6. Secretos a crear (sin valores)

**Estado, 2026-09-26:** los 8 secretos creados por c0der; el facilitador lee los 4 digests desde este PR
(`[X4-CLAVES-TF]`, `docs/handoffs/X4-CLAVES-TF.md`).

**Dos por servicio (receta por defecto, ronda 1 P3):** uno del facilitador con **solo** el digest y uno del
cliente con **solo** la clave. Con un solo secreto con los dos campos, el execution role del facilitador (que
necesita `GetSecretValue` sobre él) podría leer las cuatro claves en claro, aunque ECS inyecte solo `sha256`.

`python3 scripts/stack_key.py generate --service <s> --out-dir <dir>` escribe los dos cuerpos, en modo 0600, sin
sobrescribir, y sin imprimir la clave (imprime el digest y la variable). `**/*stack-key*.json` está en
`.gitignore`.

| Secreto | Dónde | Cuerpo (JSON) | Quién lo lee |
|---|---|---|---|
| `facilitator-stack-key-digest-execution-market` | Secrets Manager del facilitador | `{"sha256": "<64 hex>"}` (`<s>-stack-key.facilitator.json`) | execution role del facilitador → `UVD_STACK_KEY_SHA256_EXECUTION_MARKET` |
| `facilitator-stack-key-digest-karmakadabra` | ídem | ídem | → `UVD_STACK_KEY_SHA256_KARMAKADABRA` |
| `facilitator-stack-key-digest-describe-net` | ídem | ídem | → `UVD_STACK_KEY_SHA256_DESCRIBE_NET` |
| `facilitator-stack-key-digest-meshrelay` | ídem | ídem | → `UVD_STACK_KEY_SHA256_MESHRELAY` |
| `<servicio>/uvd-stack-key` (nombre según la convención de cada repo) | el almacén de secretos **del cliente** | `{"key": "uvdsk_<43 base64url>"}` (`<s>-stack-key.client.json`) | el cliente → `UVD_STACK_KEY` |

- **El facilitador** mapea el campo `sha256` en `secrets` (no `environment`), como `ERC8004_ADMIN_TOKEN`:
  `UVD_STACK_KEY_SHA256_KARMAKADABRA` ← `${data.aws_secretsmanager_secret.stack_key_digest_karmakadabra.arn}:sha256::`.
  Hace falta un `data "aws_secretsmanager_secret"` por servicio, sumado a `local.all_secret_arns` (para el
  `GetSecretValue` del execution role) y a `local.all_task_secrets`. Placeholders de ARN: `<AWS_ACCOUNT_ID>` y
  `<nombre>-<SUFIJO>`. El execution role del facilitador **no** recibe acceso a los secretos de los clientes.
- Revocar: poner `"sha256": ""` (no borrar el campo: ECS no arranca si falta la clave JSON) y
  `aws ecs update-service --force-new-deployment` del facilitador; ningún otro servicio se toca. Rotar:
  `"sha256": "<nuevo>,<viejo>"`, reiniciar el facilitador, cambiar el secreto del cliente a la clave nueva y
  reiniciarlo, dejar solo `<nuevo>`, reiniciar el facilitador.

## 7. Cómo presenta cada cliente, y qué cambia en el SDK (upstream-first)

Cada request al facilitador lleva `X-UVD-Stack-Key: <clave>` (en todas las rutas; en las no gobernadas no hace
nada). El valor viene de `UVD_STACK_KEY` en el entorno del cliente.

- **uvd-x402-sdk-python:** `X402Config` suma `stack_key: Optional[str]` (y `from_env()` lee `UVD_STACK_KEY`);
  `X402Client` lo mezcla en los `headers={"Content-Type": ...}` fijos de verify/settle/accepts
  (`client.py:869,1068,1134,1284`), y `Erc8004Client` (`erc8004.py`: entre otros `:1341` prepare y `:1456` submit,
  el camino de los ratings de KK) en sus lecturas y escrituras. `repr` de la config lo enmascara.
- **uvd-x402-sdk-typescript:** `FacilitatorClientOptions.stackKey?: string` mezclado en los headers de
  `verify()`/`settle()` (`backend/index.ts:733,798`); `Erc8004Client` ya tiene un `writeJson(url, body, extraHeaders)`
  privado (`:4040-4054`) donde entra.
- **Una clave mal leída no puede romper un pago (ronda 1 P3).** Una `UVD_STACK_KEY` leída de un archivo con
  `\r` o `\n` es un valor de header inválido: `requests` levanta `InvalidHeader` y axios/undici tiran antes de
  enviar, así que CADA `/verify` y `/settle` del cliente fallaría. El SDK hace `strip()`, valida
  `^uvdsk_[A-Za-z0-9_-]{43,128}$` (el mismo formato que exige el facilitador) y, si no valida, **no manda el
  header** y avisa una vez **sin el valor**: el cliente cae a tercero, nunca rompe el pago.
- Tests en ambos: el header está si se configuró bien; no está si no se configuró; con `\r\n` al final se manda
  la clave recortada; con una clave inválida no se manda y el aviso no contiene el valor; nunca aparece en un error
  ni en un log.
- **La exención hereda la superficie pública de cada servicio** (nota del refutador). Si EM o describe.net mandan
  la clave en TODO `/verify` y `/settle` que hacen por cuenta de cualquier comprador, ese tráfico queda sin
  presupuesto por IP en el facilitador (le quedan el tope global, el plazo del cuerpo y el throttle de RPC). Es lo
  que pidió el dueño; lo que cambia es **dónde** vive el control por cliente: en cada servicio. Antes de cablear la
  clave en uno, que ese servicio tenga su propio límite por cliente, o que la clave vaya solo en los caminos de
  tráfico propio (el relay de ratings de KK).
- **EM** (`mcp_server/integrations/erc8004/facilitator_client.py` y `integrations/x402/sdk_client.py`): configura
  su clave en el SDK. **Es la que importa para KK:** los ratings de KK salen por EM, así que la clave de EM los cubre.
  De paso, que EM propague un 429 como 429 (hoy lo vuelve 503).
- **KK:** su propia clave solo para lo que llama directo (`shared/x402_client.py`, `agents_sdk/dx402_*.py`, `bazaar.py`).
- **describe.net:** `describenet/paywall.py:483,824` (`X402Client.process_payment`).
- **meshrelay:** `FacilitatorClient` en `turnstile/payments.js:108` y `multibrain/payments.js:82`; los dos `fetch`
  a mano (`/supported`, `/health`) no están gobernados.

Los números de línea del SDK son de ramas de trabajo (py `cfdd2709`, ts `6d707be4`), no de `main`: indicativos.

## 8. Orden del deploy

1. **Facilitador con este cambio y ninguna variable `UVD_STACK_*`.** Nadie es exento; los presupuestos son los
   mismos números. Lo nuevo para todos es la admisión de §3 (32 en vuelo por dirección, cuerpo entero en 5 s,
   512 en vuelo por task) y `GET /config`. Sonda:
   `curl -s https://facilitator.ultravioletadao.xyz/config | jq '.stackIdentities.active, [.rateLimits.budgets[]|{name,periodMs,burst}], .overload|{maxInflightRequests,perClient,bodyDeadlineMs}'`
   → `0`, los ocho presupuestos de la tabla de la sección 1 y 512 / 32 / 5000. Durante los primeros días,
   `status=408`, `status=429` con `too_many_concurrent_requests` y `status=503` en el log dicen si la admisión le
   está cortando a alguien legítimo.
2. **Claves:** `scripts/stack_key.py generate` por servicio, cargar los secretos, cablear `sha256` en terraform
   (sección 6), desplegar el facilitador. Sonda: `.stackIdentities.active` = N con los nombres esperados. Ningún
   cliente cambió todavía, así que el tráfico sigue igual.
3. **SDK** py y ts con `stack_key` / `stackKey`, publicados.
4. **Clientes**, EM primero (cubre los ratings de KK). Sonda desde el runtime de cada cliente, con la clave en un
   archivo para que no quede en el historial:

   ```bash
   printf 'X-UVD-Stack-Key: %s\n' "$UVD_STACK_KEY" > /tmp/h && chmod 600 /tmp/h
   curl -s -o /dev/null -D - -H @/tmp/h https://facilitator.ultravioletadao.xyz/config | grep -i '^x-ratelimit-'
   rm /tmp/h
   # esperado: x-ratelimit-exempt: execution-market   (y NINGÚN x-ratelimit-limit)
   # sin la clave, o con una revocada: x-ratelimit-limit / x-ratelimit-remaining, sin x-ratelimit-exempt
   ```

   Es de solo lectura, no toca cadena y no consume nada del cliente.
5. **KK** vuelve a correr `sweep_ratings_firmados.py` y `emitir_publisher_rates_executor.py`: cero `rate_limited`.
   Lo que puede seguir apareciendo es `erc8004_daily_write_limit` en Ethereum (sección 5).

## 9. Tests y mutaciones

Todos con la red cerrada (routers en memoria con `oneshot`; el único socket es el `127.0.0.1` del test de MCP).

| Pedido | Test |
|---|---|
| exento: N+1 por encima del burst de CADA governor, ningún 429 | `rate_policy::a_stack_identity_is_never_refused_by_any_budget` (los 8 presupuestos, burst de producción, período congelado para que no rellene durante el test) + `the_human_pages_exempt_the_stack_too` (el router que arma `handlers`) |
| tercero con la misma cadencia: 429 | el mismo test, en la request N+1 de cada presupuesto; además comprueba que el exento no gastó la cubeta de su IP |
| clave revocada vuelve a ser tercero | `a_revoked_key_is_a_third_party_again` (y EM sigue exento: la revocación es por servicio); `a_rotation_accepts_both_keys_then_only_the_new_one` |
| clave mal formada o falsa: tercero, no 500 | `a_malformed_or_false_key_is_a_third_party_not_a_500`: 18 variantes (vacía, sin prefijo, corta, con sufijo, primer/último carácter cambiado, mayúsculas, espacios, `Bearer`, el digest, no-UTF-8, 10 KB, dos líneas, falsa bien formada…) |
| tope de hardware: 503 también al exento | `the_ceiling_sheds_the_stack_too` (503 + `retry-after: 1` + `code: overloaded`; `/health` sí contesta; el cupo vuelve al terminar) |
| la clave nunca en logs ni en la respuesta | `the_key_never_reaches_a_log_or_a_response` (captura de `tracing` que prueba que capturó algo; headers y cuerpos de exento, 429, rechazado y `/config` pedido CON la clave; `Debug` del registro; una clave cruda pegada por error en la variable de digest) + `below_admission_the_key_debugs_as_sensitive` |
| primer deploy: nadie exento | `with_no_identity_configured_everybody_is_a_third_party` |
| el digest es el de `shasum` | `the_digest_is_what_shasum_prints` (vector fijado con `printf %s KEY | shasum -a 256`, no con este crate) |
| un solo mecanismo para todos | `every_governor_goes_through_the_policy`, `client_ip::every_governor_keys_on_the_client_ip` |
| la clave llega al holder del lease | `mcp::a_forwarded_settle_carries_the_stack_key_to_the_lease_holder` |
| `/config` | `the_config_document_publishes_the_policy_in_force` |

Mutaciones (cada una aplicada sobre `HEAD`, tests corridos, archivo restaurado; runner en el scratchpad de la sesión):

| # | Mutación | Resultado | Tests en rojo |
|---|---|---|---|
| M1 | el exento vuelve a pasar por el governor por IP (`authenticate(..).filter(\|_\| false)`) | **rojo** | `a_stack_identity_is_never_refused_by_any_budget`, `the_human_pages_exempt_the_stack_too`, `a_revoked_key_is_a_third_party_again`, `the_ceiling_sheds_the_stack_too` |
| M2a | la comparación deja de ser exacta: se hashean solo los primeros 49 bytes | **rojo** | `a_malformed_or_false_key_is_a_third_party_not_a_500` (la clave con sufijo pasaba) |
| M2b | cualquier clave bien formada coincide (`ct_eq(..) \|\| true`) | **rojo** | malformed, revoked, rotation, ceiling, key_never |
| M2c | se aceptan varias líneas del header | **rojo** | malformed ("la clave dos veces") |
| M2d | se recortan espacios antes de hashear | **rojo** | malformed ("espacio adelante/atrás") |
| M3a | `/config` imprime los digests | **rojo** | `the_key_never_reaches_a_log_or_a_response` |
| M3b | `/config` devuelve la clave que presentó el caller | **rojo** | `the_key_never_reaches_a_log_or_a_response` |
| M4 | el tope en vuelo exime al que trae clave | **rojo** | `the_ceiling_sheds_the_stack_too` |
| M5 | la clave aceptada se loguea | **rojo** | `the_key_never_reaches_a_log_or_a_response` |
| M6 | MCP deja de copiar `X-UVD-Stack-Key` a la request sintética | **rojo** | `mcp::a_forwarded_settle_carries_the_stack_key_to_the_lease_holder` |

Ninguna sobrevivió. La primera corrida de M6 también tiró el test de `X-Forwarded-For` de MCP: no era la mutación
sino los dos tests en paralelo pisándose el writer lease global (CI corre `--test-threads=1`). Quedó arreglado con un
mutex local en `settle_through_a_recording_holder_with`: cuatro corridas en paralelo en verde, y M6 repetida da rojo
solo en el test de la clave.

### Ronda 1 (REF-X4-STACK-429)

Tests nuevos, todos con la red cerrada (`HTTPS_PROXY=http://127.0.0.1:9 HTTP_PROXY=http://127.0.0.1:9
NO_PROXY=127.0.0.1,localhost`; el único socket es el de `127.0.0.1`):

| Pedido | Test |
|---|---|
| P1-1 c) TCP real: N sockets con headers y sin cuerpo no dejan en 503 al stack; la subida colgada recibe 408 | `rate_policy::over_real_tcp_an_upload_that_never_arrives_holds_no_slot`: `axum::serve` en 127.0.0.1 con tope global 2 y 4 subidas colgadas. El stack da `200` **mientras** cuelgan, cada subida recibe `HTTP/1.1 408` + `request_timeout` y se cierra al plazo, y después el stack y una subida completa dan `200` |
| P1-1 b) techo por dirección, de los dos lados | `one_address_cannot_fill_the_ceiling_and_the_stack_skips_its_limit`: con techo 2 y dos subidas colgadas de una IP, la tercera de esa IP da `429 too_many_concurrent_requests` + `retry-after: 1`, otra IP da `200`, el stack desde la misma IP da `200`, y el cupo vuelve al terminar una |
| 413 en JSON ahora que la admisión lee el cuerpo | `a_body_past_the_limit_is_a_json_413` |
| P2-1 comportamiento | `handlers::erc8004_write_rate_tests::a_stack_identity_still_spends_the_daily_gas_cap` |
| P2-1 fuente | `rate_policy::the_gas_cap_knows_nothing_of_the_stack` |
| P2-2 M15 comportamiento | `handlers::erc8004_write_rate_tests::the_erc8004_writes_exempt_the_stack` (35 POST del stack sin 429 sobre todas las rutas de escritura; un tercero da 429 en el 31) |
| P2-2 M13 fuente | `every_governor_goes_through_the_policy`, ahora con "cada bucket construido está montado" y la prohibición de alias |
| P3 clave corta | `a_short_key_never_authenticates_whatever_its_digest` (`uvdsk_` + 10 caracteres, con su digest configurado) |
| P3 log con dirección y ruta | `the_key_never_reaches_a_log_or_a_response` ahora pasa por `admit` y exige `client_ip=` y `path=` en el log de rechazo |

Mutaciones de la ronda: cada una se aplica sobre el working tree, se corren los tests de `rate_policy`,
`client_ip`, `erc8004_write_rate_tests`, `daily_cap`, `human_surface_tests` y `owner_scan` (o `cargo build` del
binario en las que el tipo tiene que impedir) y se restaura el archivo. Al final se verificó la restauración byte a
byte.

| # | Mutación | Resultado | Qué la mata |
|---|---|---|---|
| P1a | el cupo global se toma antes del cuerpo (el orden refutado) | **rojo** | `over_real_tcp_an_upload_that_never_arrives_holds_no_slot` |
| P1b | sin plazo del cuerpo (3600 s) | **rojo** | `over_real_tcp_an_upload_that_never_arrives_holds_no_slot` |
| P1c | sin techo por dirección | **rojo** | `one_address_cannot_fill_the_ceiling_and_the_stack_skips_its_limit` |
| P1d | el stack sujeto al techo por dirección | **rojo** | el mismo |
| M06 | el tope global saltea al stack | **rojo** | `the_ceiling_sheds_the_stack_too`, `the_key_never_reaches_a_log_or_a_response` |
| M07 | el cupo global se suelta al tomarlo | **rojo** | `the_ceiling_sheds_the_stack_too` |
| M08 | `daily_cap::enforce` saltea al que trae el header | **rojo** | `the_gas_cap_knows_nothing_of_the_stack` |
| M09 | `daily_cap::enforce_with` saltea al que trae el header | **rojo** | `a_stack_identity_still_spends_the_daily_gas_cap`, `the_gas_cap_knows_nothing_of_the_stack` |
| M10b | la clave rechazada se loguea por valor (en `admit`) | **rojo** | `the_key_never_reaches_a_log_or_a_response` |
| M12 | governor crudo en `main.rs` con alias (`use … GovernorLayer as RawGov`) | **no compila** (`cargo build`: `E0308 mismatched types`, el `Bucket` no es un `GovernorConfig`) | el tipo |
| M13 | bucket construido y no montado (`/events` sin governor) | **rojo** | `every_governor_goes_through_the_policy` |
| M14 | `main.rs` monta con OTRA política (`RatePolicy::new(StackIdentities::from_lookup(..))`) | **no compila** (`E0624`: `new` y `from_lookup` son privados) | el tipo |
| M15 | escrituras ERC-8004 con OTRA política (la misma forma) | **no compila** (`E0624`) | el tipo |
| M15b | escrituras ERC-8004 con `RatePolicy::none()` (solo existe con `cfg(test)`) | **rojo** en tests; **no compila** en `cargo build` (`E0599`) | `the_erc8004_writes_exempt_the_stack`, `a_stack_identity_still_spends_the_daily_gas_cap` |
| M16 | páginas humanas con OTRA política | **no compila** (`E0624`) | el tipo |
| M17 | formato débil: 1 carácter de secreto alcanza | **rojo** | `a_short_key_never_authenticates_whatever_its_digest` |

M01 (`==` en vez de `ct_eq`) y M02 (salida temprana) siguen sobreviviendo, y está bien. Lo que se compara es el
SHA-256 de la entrada del atacante contra digests configurados: una fuga por tiempo revelaría bytes de un digest, y
un digest no autentica. Es la lectura del refutador (P3-8) y la comparto.

## 10. Pre-CI local

Disco antes de compilar: 163 GB libres (`df -h /System/Volumes/Data`); 120 GB al final. `ci.yaml` no corre
`cargo fmt` ni `clippy` (y `origin/main` no está limpio en ninguno de los dos), así que esos dos se midieron sobre
las líneas de este cambio. Los pasos de `ci.yaml` que el diff dispara, sobre el estado commiteado:

| Paso (`.github/workflows/ci.yaml`) | Resultado |
|---|---|
| `python3 scripts/verify_landing_canonical.py --offline` | `[OK]` |
| `node --test tests/frontend-capabilities.test.cjs` | 19/19 |
| `python3 -m unittest discover -s tests/scripts -p 'test_*balances.py'` | 5/5 OK |
| `cargo build --locked --features solana,near,stellar,algorand,sui,xrpl,hedera` | exit 0 |
| `cargo test --locked -p x402-rs --features … -- --test-threads=1` | exit 0: lib 1364 ok, bin 1420 ok, integración 6+24+3+6+1+1+9+15 ok, doctests ok |
| `cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance -- --test-threads=1` | todo ok |
| `rustfmt --check src/rate_policy.rs` (archivo nuevo, entero) | limpio |
| rustfmt sobre las líneas agregadas del resto | 0 cambios pendientes (los hunks previos de `origin/main` quedan como estaban) |
| `cargo clippy --locked -p x402-rs --all-targets --features …`, filtrado a líneas agregadas | 1: `path_config is never used`, el mismo aviso que los 68 `path_*` de utoipa |
| `rate_policy` en paralelo (sin `--test-threads=1`) | 17/17, cinco corridas |

### Pre-CI de la ronda 1

Con la red cerrada (`HTTPS_PROXY=http://127.0.0.1:9 HTTP_PROXY=http://127.0.0.1:9 NO_PROXY=127.0.0.1,localhost`)
y en un checkout LF. Disco antes de compilar: 100 GB libres.

| Paso | Resultado |
|---|---|
| `cargo build --locked --features solana,near,stellar,algorand,sui,xrpl,hedera` | exit 0 |
| `cargo test --locked -p x402-rs --features … -- --test-threads=1` | exit 0: lib 1371 ok, bin 1427 ok, integración y doctests ok |
| `cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance -- --test-threads=1` (el lock cambió) | exit 0 |
| `rustfmt --check src/rate_policy.rs` | limpio |
| rustfmt sobre las líneas agregadas del resto | 0 cambios pendientes |
| `cargo clippy --locked -p x402-rs --all-targets --features …`, filtrado a las líneas agregadas contra `origin/main` | 1: `path_config is never used`, la clase de los otros 68 `path_*` de utoipa. Un `concat!` innecesario en un test se corrigió, y después `rate_policy` (lib 22/22) y los tests de política del binario (74/74) volvieron a pasar |
| `verify_landing_canonical.py --offline`, `frontend-capabilities`, `test_*balances.py` | `[OK]`, 19/19, 5/5 |

## Notas para el refutador

- Tests de fuente existentes que cambiaron, y por qué: `client_ip::every_governor_keys_on_the_client_ip` (antes
  contaba ≥8 builders; ahora exige exactamente uno, en `rate_policy.rs`);
  `handlers::production_mounts_the_writes_and_the_bazar_on_separate_budgets` (el 12 s / 250 del bazar se lee de
  `rate_policy::DISCOVERY_REGISTER`); `every_erc8004_write_draws_on_one_bucket_of_thirty` e
  `identity_read_limit_leaves_headroom_over_measured_traffic` (leen el presupuesto central). Los
  `human_page_routes_governed(60_000, n)` de los tests pasaron a `(&RatePolicy::none(), Limit::every_ms(60_000, n))`.
- Las razones de cada número (los comentarios largos de `main.rs` y `handlers.rs`) se mudaron a la doc de cada
  `Budget` en `rate_policy.rs`.
- `Cargo.toml` suma `governor = "0.10"` solo para nombrar `StateInformationMiddleware`, y en la ronda 1
  `http-body-util = "0.1"` solo para reconocer `LengthLimitError` por tipo. Los dos ya estaban en el lock, con la
  misma versión y features, así que `Cargo.lock` suma una línea por cada uno y ningún crate nuevo. La feature
  `timeout` de tower-http **no** se usa (ver §3). La línea de `serde`, que el primer commit había tocado sin querer
  (`serde ={`), volvió a como estaba en `origin/main`.
- `scripts/stack_key.py generate` pasó de `--out` (un JSON con clave y digest) a `--out-dir` (dos archivos, uno por
  secreto; §6).
- **CRLF (P3-6 del refutador, entorno, no del diff):** en un checkout con `core.autocrlf=true`,
  `erc8004::daily_cap::tests::every_send_is_counted_and_every_sending_route_is_capped` parte la fuente por `"\n}\n"`
  y falla. Este worktree está en LF (memoria del repo: re-checkout con `core.autocrlf=false`), y así pasa.
- `static/skill.md` suma el `503 overloaded` y `/config`; `static/llms-full.txt` regenerado con
  `scripts/build_llms_full.sh`; el digest de `static/.well-known/agent-skills/index.json` actualizado.
- Recarga en caliente de identidades: no. Revocar exige reiniciar el facilitador (misma imagen). Suficiente para
  el criterio; si hiciera falta, el registro ya está detrás de un `Arc` y se puede cambiar por un `ArcSwap`.
- `VERSION` pasa a `2.42.0` y `CHANGELOG.md` (el de la raíz, que es el que actualizan los PRs recientes) suma su
  entrada. Asumí producción en `2.41.0` = `main`; no leí `/version` porque el encargo prohíbe llamar a producción.
  Si otra tanda ya tomó 2.42.0, renumerar antes del merge.
- CLAUDE.md dice que el gate de CI compila `solana,near,stellar,algorand,sui,xrpl`; `ci.yaml` suma `hedera`. Usé el de `ci.yaml`.

Repos leídos para las secciones 1, 5 y 7 (solo lectura): karmakadabra `d7eceda2`, execution-market `10800a5c`,
uvd-x402-sdk-python `cfdd2709`, uvd-x402-sdk-typescript `6d707be4`, describe-net `e37373a6`, meshrelay `0b39ca33`.
