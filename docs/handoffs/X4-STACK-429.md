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
- **Falta (no es mío hacer):** crear los cuatro secretos, cablearlos en terraform, desplegar; cambio en el SDK
  (py y ts) y en los clientes. **Decisión pendiente del dueño:** si el stack tiene un tope diario de gas propio
  más alto (números abajo). Sin esa decisión el relleno de KK choca con el tope de Ethereum (100/día).
- **Próximo paso:** c0der refuta el worktree; si pasa, un único CI + deploy del facilitador con la lista vacía
  (no cambia nada para nadie salvo el tope de 512 en vuelo y `/config`), y después el orden de la sección
  "Orden del deploy".

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
- Una clave rechazada se loguea la primera vez y después cada 100, **sin valor**.
- La respuesta a un exento lleva `x-ratelimit-exempt: <servicio>` (la base de la sonda). Nunca la clave.

## 3. El mecanismo: una sola capa para todos los governors

Un `KeyExtractor` solo elige cubeta; no puede saltar el governor (una cubeta propia seguiría cortando al stack
en su burst). Por eso `PolicyLayer` envuelve al `GovernorLayer`: guarda el servicio gobernado y el desnudo y elige
por request. `RatePolicy::layer(&config)` es la **única** forma de montar un governor:

- `rate_policy::config(limit)` es el único `GovernorConfigBuilder` de `src/` (`client_ip::every_governor_keys_on_the_client_ip`
  lo exige: exactamente uno, en `rate_policy.rs`, con `ClientIpKeyExtractor`).
- `rate_policy::every_governor_goes_through_the_policy` falla si aparece `GovernorLayer::` fuera de
  `rate_policy.rs`, si un presupuesto no está montado, si `main.rs` dimensiona un governor con algo que no sea
  `rate_policy::<PRESUPUESTO>.limit()`, o si el tope global no queda dentro del layer de tracing.
- Los dos saltos entre tasks conservan la identidad: `forward_to_writer` copia todos los headers (la exención se
  repite en el holder) y la request sintética de MCP ahora copia `X-UVD-Stack-Key` además de `X-Forwarded-For`
  (`mcp::a_forwarded_settle_carries_the_stack_key_to_the_lease_holder`).

| | tercero | stack con clave válida |
|---|---|---|
| presupuestos por IP (8) | `429 rate_limited` al pasar el burst | **nunca** |
| tope en vuelo (`MAX_INFLIGHT_REQUESTS`, 512) | `503 overloaded`, `Retry-After: 1` | **igual** |
| tope diario ERC-8004 (gas) | `429 erc8004_daily_write_limit` | **igual** |
| throttle de RPC (`RPC_MAX_CU_PER_SECOND`) | sin cambios | sin cambios |

**El tope en vuelo es comportamiento nuevo para todos.** 512 por task: 1 vCPU / 2 GB (tfvars), cuerpos de 64 KiB
como máximo (32 MiB a 512), y la carga medida está dos órdenes por debajo (216 settles en 4 h, 2026-08-20). Un
request retiene el cupo hasta producir la respuesta, no mientras un stream escribe (un `/events` abierto no lo
ocupa). `/health` nunca se descarta: si el ALB viera 503 en una task ocupada, ECS la reemplazaría y se llevaría su
capacidad. Vive dentro del layer de tracing, así que un descarte se loguea como `status=503`.

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
| `UVD_STACK_SERVICES` | `execution-market,karmakadabra,describe-net,meshrelay` | lista de servicios |
| `UVD_STACK_KEY_SHA256_<SERVICIO>` | vacío (inactivo) | digests SHA-256 hex, separados por coma (dos durante una rotación) |

Ningún default cambió. Un override que no parsea a entero positivo se ignora (misma semántica de antes).
`GET /config` (gobernado como las lecturas baratas) publica: cada presupuesto con su valor efectivo, su default y
sus variables; `stackIdentities` (`active`, y por servicio `name`, `active`, `credentials` = cuántos digests);
`overload`; y el tope diario por red. Nunca una clave ni un digest (test `the_key_never_reaches_a_log_or_a_response`).

## 5. Tope diario de gas ERC-8004 — medido, **no** eximido

- **Defaults** (`daily_cap.rs:59,65-70`): 1000 escrituras por red y día UTC; `ethereum` 100, `solana` 150, `arc` 100,
  `arc-testnet` 100. **Producción no tiene overrides** (grep de `ERC8004_DAILY_WRITE_CAP` en
  `terraform/environments/production/*.tf`: cero).
- **Qué cuenta:** transacciones enviadas, no requests (`prepare` no cuenta; un `submit` = una tx). Vive **dentro**
  del writer lease (`handlers.rs`, `erc8004_write_routes`: `daily_cap::enforce` bajo `require_writer_lease`), así
  que en operación normal es un contador por servicio (el del holder), no por task.
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

Uno por servicio. Nombres siguiendo el patrón de `secrets.tf`:

- `facilitator-stack-key-execution-market`
- `facilitator-stack-key-karmakadabra`
- `facilitator-stack-key-describe-net`
- `facilitator-stack-key-meshrelay`

Forma (JSON), exactamente lo que escribe `python3 scripts/stack_key.py generate --service <s> --out <archivo>`
(modo 0600, no sobrescribe, no imprime la clave; `**/*stack-key*.json` quedó en `.gitignore`):

```json
{"service": "karmakadabra", "key": "uvdsk_<43 base64url>", "sha256": "<64 hex>"}
```

- **El facilitador** mapea **solo** el campo `sha256`, en `secrets` (no `environment`), como `ERC8004_ADMIN_TOKEN`:
  `UVD_STACK_KEY_SHA256_KARMAKADABRA` ← `${data.aws_secretsmanager_secret.stack_key_karmakadabra.arn}:sha256::`
  (un `data "aws_secretsmanager_secret"` por servicio, sumarlo a `local.all_secret_arns` para el
  `GetSecretValue` del execution role y a `local.all_task_secrets`). Placeholders de ARN: `<AWS_ACCOUNT_ID>`, `<nombre>-<SUFIJO>`.
- **El cliente** lee el campo `key` (del mismo secreto, o copiado al almacén de secretos del cliente). Si el rol
  del facilitador no debe poder leer la clave, la variante es un secreto con solo `sha256` para el facilitador y la
  clave en el almacén del cliente; cuesta un secreto más por servicio.
- Revocar: poner `"sha256": ""` (no borrar el campo: ECS no arranca si falta la clave JSON) y
  `aws ecs update-service --force-new-deployment` del facilitador. Rotar: `"sha256": "<nuevo>,<viejo>"`, reiniciar
  el facilitador, cambiar el cliente a la clave nueva, dejar solo `<nuevo>`, reiniciar.

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
- Tests en ambos: el header está si se configuró, no está si no, y nunca aparece en un error ni en un log.
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
   mismos números. Lo nuevo para todos: `GET /config` y el tope de 512 en vuelo. Sonda:
   `curl -s https://facilitator.ultravioletadao.xyz/config | jq '.stackIdentities.active, [.rateLimits.budgets[]|{name,periodMs,burst}]'`
   → `0` y los ocho presupuestos de la tabla de la sección 1.
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

## Notas para el refutador

- Tests de fuente existentes que cambiaron, y por qué: `client_ip::every_governor_keys_on_the_client_ip` (antes
  contaba ≥8 builders; ahora exige exactamente uno, en `rate_policy.rs`);
  `handlers::production_mounts_the_writes_and_the_bazar_on_separate_budgets` (el 12 s / 250 del bazar se lee de
  `rate_policy::DISCOVERY_REGISTER`); `every_erc8004_write_draws_on_one_bucket_of_thirty` e
  `identity_read_limit_leaves_headroom_over_measured_traffic` (leen el presupuesto central). Los
  `human_page_routes_governed(60_000, n)` de los tests pasaron a `(&RatePolicy::none(), Limit::every_ms(60_000, n))`.
- Las razones de cada número (los comentarios largos de `main.rs` y `handlers.rs`) se mudaron a la doc de cada
  `Budget` en `rate_policy.rs`.
- `Cargo.toml` suma `governor = "0.10"` solo para nombrar `StateInformationMiddleware`; ya estaba en el lock por
  `tower_governor` (misma versión y features): `Cargo.lock` cambia en una línea.
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
