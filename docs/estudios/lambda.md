# ¿El facilitador puede correr en AWS Lambda? (X402-LAMBDA-ESTUDIO)

Estudio de solo lectura, fase 2 del recorte de costes. Medido sobre `main` en `c3b694b0`
(2026-10-06). Ningún cambio de código, de Terraform ni de AWS acompaña a este documento.

Convenciones:

- `archivo:línea` = medido en el código de este repo en ese commit.
- **[DOC AWS]** = límite de la plataforma según la documentación pública de AWS; no se puede
  medir en este repo y conviene revalidarlo antes de implementar.
- **[HIPÓTESIS]** = no medido; razonamiento o expectativa a confirmar con una medición.
- **[ESTIMADO]** = cifra de coste que no sale del código.

Este documento reemplaza, para la decisión de hoy, a `docs/LAMBDA_MIGRATION.md` (último commit
2026-02-09): ese texto supone una sola tarea y no conoce el writer lease, el discovery owner,
Hedera ni XRPL, que hoy condicionan la respuesta.

## 0. Resumen

**Veredicto: viable con cambios, y no como lift-and-shift.** El router axum entra en Lambda sin
reescribir handlers (§6), y la mitad de lectura del tráfico migra con riesgo bajo. La mitad de
escritura no: la seguridad de los nonces EVM descansa hoy en que **un solo proceso** firma por
EOA (`src/writer_lease.rs:30-40`, `src/chain/evm.rs:3160`), y en Lambda cada entorno de ejecución
sería un firmante independiente que además **se sale de la elección del lease por construcción**
(§1.1). Con el código actual, el único tope seguro para la función que firma EVM es
**concurrencia reservada = 1 y ningún Fargate firmando a la vez**, lo que serializa todos los
settles del facilitador detrás de esperas de hasta 90 s (Base) o 900 s (Ethereum)
(`src/chain/evm.rs:1178-1181`). Eso es inaceptable en producción.

Lo que hay que cambiar antes de mover escrituras: (a) asignación de nonce EVM/NEAR/Stellar fuera
del proceso (DynamoDB) o un pool de firmantes con un lease por EOA; (b) sacar de los handlers todo
`tokio::spawn` que sobreviva a la respuesta (§2.3); (c) mover a EventBridge los loops de fondo de
§2.2 (12 de las 16 filas); (d) mover a DynamoDB tres estados que hoy son por proceso y que valen dinero o
corrección: el tope diario ERC-8004, los jobs de registro y el rate limiting (§5).

El ahorro que motiva el cambio, 45-50 USD/mes sobre la fase 1, es **[ESTIMADO]** de c0der y no
sale del código; §7.3 da el techo que se ve en la configuración.

## 1. Nonces y concurrencia por familia

Hecho de plataforma que gobierna toda esta sección: **un entorno de ejecución Lambda atiende una
invocación a la vez** y la concurrencia se logra con más entornos **[DOC AWS]**. Hoy un proceso
Fargate atiende hasta 512 requests a la vez (`src/rate_policy.rs:1126`) y serializa los nonces en
memoria. En Lambda, "N requests concurrentes" = "N procesos con N copias del estado".

### 1.1 EVM (25+ redes; firma local, nonce del facilitador)

- **Firma:** llaves locales del facilitador; con varias llaves elige en round-robin
  (`src/chain/evm.rs:661-671`).
- **Nonce:** `PendingNonceManager` (`src/chain/evm.rs:3160`), un `DashMap` por dirección con un
  `NonceState` por EOA (`src/chain/evm.rs:3237`). La primera asignación lee
  `get_transaction_count(address).pending()` del RPC (`src/chain/evm.rs:3286`); las siguientes
  salen del contador local. Se instala como `NonceFiller` del provider
  (`src/chain/evm.rs:629-638`).
- **Coordinación entre réplicas:** el writer lease en DynamoDB (tabla `facilitator-nonces`):
  TTL 30 s, renovación cada 3 s, margen de handover 10 s, holgura de firma 3 s, drenaje 15 s
  (`src/writer_lease.rs:159-198`). Quien no tiene el lease no firma EVM
  (`src/chain/evm.rs:886`, permiso por intento en `src/chain/evm.rs:1058`) y reenvía el settle
  al holder con un salto marcado por `x-facilitator-forwarded-for-writer`
  (`src/writer_lease.rs:143`, `src/handlers.rs:1718-1747`). El SG solo abre el puerto 8080 entre
  tareas para ese reenvío (`terraform/environments/production/main.tf:210-218`); el lease vive en
  DynamoDB, no en el SG.
- **Qué pasa en Lambda con el código tal cual:**
  1. `discover_own_endpoint()` necesita `WRITER_LEASE_ENDPOINT` o la metadata de ECS
     (`src/writer_lease.rs:415-440`); en Lambda no existe ninguna de las dos.
  2. Sin endpoint, `lease_refusal` decide abstenerse (`src/writer_lease.rs:509-511`) y `spawn`
     devuelve `None` **"y sigue escribiendo sus propias transacciones EVM"**
     (`src/writer_lease.rs:699-716`), porque el grant arranca en modo standalone = escritor
     (`src/writer_lease.rs:228-234`).
  3. Resultado: cada entorno Lambda es un firmante con su propio contador. Dos entornos que
     arrancan juntos leen el mismo `pending` y asignan el mismo nonce: una transacción reemplaza
     o invalida a la otra ("nonce too low" / "replacement underpriced"). Si Fargate sigue
     sirviendo, el holder de Fargate también firma con la misma EOA.
- **Tope seguro con el código actual:** concurrencia reservada **1** en la función que firma EVM
  **y** Fargate sin firmar EVM al mismo tiempo (o EOAs distintas). Con 1, un settle de Base
  bloquea al resto hasta 90 s. Por eso el tope "seguro" no es operable.
- **Salidas reales:**
  - (A) Híbrido: Lambda sirve lecturas y familias sin nonce del facilitador; los settles EVM
    siguen en Fargate (1 tarea con el lease).
  - (B) Asignador de nonce en DynamoDB por `(chain, EOA)` con escritura condicional, más la
    lógica de deriva y huecos que hoy vive en `NonceState` (`src/chain/evm.rs:3195`,
    `NONCE_TRUST_CHAIN_AFTER_DRIFT`). Reescritura delicada de dinero.
  - (C) Pool de firmantes: K EOAs, un lease por EOA en DynamoDB, concurrencia reservada = K.
    Ojo: el `releaseCondition` de los PaymentOperators apunta a la EOA actual
    (`docs/HANDOFF-2026-07-24-signer-pool-concurrencia.md:18`), así que una EOA nueva puede no
    poder liberar escrows.

### 1.2 NEAR (firma local, nonce de la access key)

- Nonce = `view_access_key` con `Finality::Final` (`src/chain/near.rs:301-312`) **+ 1**
  (`src/chain/near.rs:755`); firma el relayer y envía con `broadcast_tx_commit`
  (`src/chain/near.rs:787-800`).
- No hay lock ni cache: dos settles concurrentes **ya hoy** dentro de un proceso pueden leer el
  mismo nonce. En Lambda la carrera se multiplica por N.
- Tope seguro con el código actual: concurrencia 1 para settles NEAR, o lock/contador en
  DynamoDB por access key.

### 1.3 Stellar (firma local, sequence de la cuenta)

- Sequence de la cuenta del facilitador leída de Horizon (`src/chain/stellar.rs:620-636`)
  **+ 1** por envío (`src/chain/stellar.rs:1621-1632`); replay del payload por `NonceStore`.
- Misma carrera que NEAR, mismo tope: 1, o lock en DynamoDB.

### 1.4 Solana (co-firma de fee payer; sin nonce del facilitador)

- El facilitador es fee payer de una transacción que arma el cliente
  (`src/chain/solana.rs:592`); no hay secuencia del facilitador que asignar. Envío y
  confirmación en `send_and_confirm` (`src/chain/solana.rs:2475`).
- Concurrencia: segura respecto a nonces con N entornos. Replay: `NonceStore` en DynamoDB si
  `NONCE_STORE_TABLE_NAME` existe; si falta, cae a memoria (`src/nonce_store.rs:522-541`), lo
  que en Lambda sería una protección por entorno. La variable es obligatoria en Lambda.

### 1.5 Algorand (co-firma de la tx de fee; validez por rondas)

- El grupo lo arma el cliente; el facilitador firma la tx de fee (`src/chain/algorand.rs:835`) y envía el grupo
  (`src/chain/algorand.rs:860-890`). No hay sequence de cuenta. Replay por `NonceStore`.
- Concurrencia: segura con N entornos, con la misma condición de `NONCE_STORE_TABLE_NAME`.

### 1.6 Sui (patrocinio de gas; objetos versionados)

- El facilitador firma como dueño del gas; exige `gas_data.owner == facilitator`
  (`src/chain/sui.rs:428-435`) y rechaza entradas cuya versión ya avanzó
  (`src/chain/sui.rs:645-660`).
- El recurso en disputa no es un nonce sino los objetos de gas que referencia la transacción.
  Dos settles que reutilizan la misma moneda de gas chocan por versión: uno falla. Eso ya puede
  ocurrir hoy; Lambda no lo empeora porque el proceso no asigna monedas
  **[HIPÓTESIS: la elección de la moneda de gas la hace el cliente; no se encontró selección de
  gas del lado del facilitador]**.

### 1.7 XRPL (relay de un blob firmado por el cliente)

- El cliente manda la transacción ya firmada; el facilitador solo hace `submit`
  (`src/chain/xrpl.rs:1027`) y sondea hasta validación (`src/chain/xrpl.rs:1124-1125`).
- Sin nonce del facilitador: segura con N entornos.

### 1.8 Hedera (cuenta patrocinadora; registros durables)

- Los registros de settlement y los bytes firmados se persisten en DynamoDB; "leases y
  escrituras condicionales coordinan las réplicas; solo se reenvían los bytes persistidos
  originales" (`src/chain/hedera/mod.rs:353-354`).
- Concurrencia: es la familia mejor preparada; el problema en Lambda es el loop de recuperación
  (§2.2), no la firma.

### 1.9 Tabla de topes

| Familia | Nonce/recurso del facilitador | Coordinación hoy | Tope seguro en Lambda sin cambios |
|---|---|---|---|
| EVM | contador local sobre `pending` | writer lease DynamoDB + reenvío | 1, y Fargate sin firmar |
| NEAR | nonce de access key +1 | ninguna | 1 |
| Stellar | sequence de cuenta +1 | ninguna | 1 |
| Solana | ninguno (fee payer) | replay en DynamoDB | sin tope por nonce |
| Algorand | ninguno | replay en DynamoDB | sin tope por nonce |
| Sui | objetos de gas del cliente | chequeo de versiones | sin tope propio [HIPÓTESIS] |
| XRPL | ninguno (relay) | ninguna necesaria | sin tope por nonce |
| Hedera | registros persistidos | leases DynamoDB | sin tope por nonce |

## 2. Procesos de fondo

### 2.1 Arranque que hoy depende de ECS

- Writer lease: se abstiene fuera de ECS (§1.1).
- Discovery owner: `runs_in_ecs()` lee la metadata de ECS (`src/discovery_owner.rs:142-145`);
  sin ella **cada proceso corre sus propios loops de discovery**
  (`src/discovery_owner.rs:184-191`). En Lambda, cada entorno haría su propia agregación y su
  propio PUT del catálogo de 15 MB (`src/discovery_owner.rs:8`, `:42`), compitiendo con el owner
  de Fargate durante el canary. Hace falta un modo "Lambda" que apague estos loops: es un cambio
  de código.

### 2.2 Loops periódicos (`tokio::spawn` de larga vida)

Un entorno Lambda se congela entre invocaciones **[DOC AWS]**: un loop de `tokio` no corre
mientras no haya requests y retoma a destiempo al llegar la siguiente.

| # | Tarea | Dónde | Periodo | En Lambda |
|---|---|---|---|---|
| 1 | Renovación del writer lease | `src/writer_lease.rs:728` | 3 s | No aplica: no hay endpoint (§1.1) |
| 2 | Renovación del discovery owner | `src/discovery_owner.rs:218` | renovación del lease | No aplica; apagar loops (§2.1) |
| 3 | Refresco de snapshot (no owner) | `src/discovery_owner.rs:314` | 60 s (`:109`) | Perezoso: chequeo con TTL al servir |
| 4 | Agregación del Bazaar | `src/discovery_aggregator.rs:1291` | 3600 s (`src/main.rs:343-346`) | EventBridge cada hora, función aparte |
| 5 | Crawler | `src/discovery_crawler.rs:463` | 86400 s (`src/main.rs:371`), apagado por defecto | EventBridge diario si se enciende |
| 6 | Health prober del Bazaar | `src/discovery_health.rs:2436` | tick 60 s | EventBridge por minuto, función aparte |
| 7 | Atestación ERC-8004 (uptime) | `src/discovery_attestation.rs:352` | configurable | EventBridge |
| 8 | Stuck tx monitor | `src/stuck_tx_monitor.rs:193` | 120 s (`:54`) | EventBridge cada 2 min |
| 9 | Chain identity | `src/chain_identity.rs:98` | arranque + reprobe | Al arrancar + EventBridge |
| 10 | Autoverify de PaymentOperator | `src/payment_operator/autoverify.rs:246` | 600 s (`:39`) | EventBridge que persiste el veredicto (hoy en memoria y decide `/supported`) |
| 11 | Refresco del allowlist de IPs | `src/ip_allowlist.rs:570` | 300 s (`:80`) | Perezoso con TTL en cada entorno |
| 12 | Recuperación de Hedera | `src/chain/hedera/mod.rs:355-358` | 30 s | EventBridge por minuto (los leases ya coordinan) |
| 13 | Health de arranque de Hedera | `src/chain/hedera/mod.rs:483` | una vez | Al arrancar o perezoso |
| 14 | Sweeper de retención DX402 | `src/dx402/service.rs:350` → `src/dx402/store_pinata.rs:830` | periódico | EventBridge |
| 15 | Export de métricas OTel | `src/telemetry.rs:226` | 30 s | Solo si hay `OTEL_EXPORTER_OTLP_*` (`src/telemetry.rs:41-43`); en Lambda, flush al final de cada invocación o extensión |
| 16 | Señales SIGTERM/SIGINT | `src/sig_down.rs` | — | `axum::serve` con graceful shutdown no existe en Lambda; no necesario |

Métricas Facilitator/Chains: no son un loop; se sirven al request (stats de discovery con caché
en memoria, `src/discovery.rs:855`, `:1165-1167`). En Lambda siguen funcionando, por entorno.

### 2.3 Trabajo que sobrevive a la respuesta (el riesgo menos visible)

`lambda_http` responde y el entorno se congela; lo que quedó en un `tokio::spawn` puede correr
tarde o perderse si el entorno se recicla **[DOC AWS]**.

| Trabajo | Dónde | Consecuencia si se pierde |
|---|---|---|
| Registro de transacción | `src/handlers.rs:1366-1371` | índice incompleto (no crítico por diseño) |
| `track_settlement` del Bazaar | `src/handlers.rs:5844` | contador de settles incompleto |
| Registro de idempotencia del settle | `src/handlers.rs:5946` | un retry con la misma clave vuelve a intentar el settle |
| Mint ERC-8004 asíncrono (`202` + jobId) | `src/handlers.rs:11117-11129` | **se responde 202 y el mint puede no ocurrir nunca** |
| Drenaje de escrituras del discovery | `src/discovery.rs:1756` | escrituras del catálogo perdidas |
| Rebuild del índice de búsqueda | `src/discovery.rs:1696-1699` | índice viejo |
| Revalidación ofrecida al owner | `src/discovery_revalidation.rs:288` | revalidación perdida |

Reemplazo: esperar (`await`) lo que sea consistencia antes de responder (idempotencia,
`track_settlement`), y mandar a SQS → Lambda worker lo que sea largo (mint ERC-8004).

## 3. Latencia de settlement (lo que espera el handler antes de responder)

Límites de la plataforma **[DOC AWS]**: API Gateway HTTP API corta la integración a 30 s;
Lambda (y por tanto Function URL o un target group de ALB) admite hasta 15 min.

| Red | Espera dentro del handler | Dónde | ¿≤ 30 s? |
|---|---|---|---|
| Ethereum | recibo hasta 900 s | `src/chain/evm.rs:1179` | No |
| Base | recibo hasta 90 s | `src/chain/evm.rs:1180` | No |
| Otras EVM | recibo hasta 30 s + estimación y envío | `src/chain/evm.rs:1181-1188` | No en el peor caso |
| `TX_RECEIPT_TIMEOUT_SECS` | reemplaza todos los anteriores | `src/chain/evm.rs:1184` | depende |
| Reenvío al holder (hoy) | recibo + 30 s, máx. 930 s | `src/handlers.rs:1597`, `:1616-1623` | No |
| Solana | confirmación hasta 90 s, sondeo 500 ms | `src/chain/solana.rs:2513-2516`, `:2541` | No |
| NEAR | `broadcast_tx_commit` acotado por el timeout RPC de 10 s | `src/chain/near.rs:798`, `src/chain/mod.rs:51` | Sí [HIPÓTESIS: el cliente NEAR usa ese timeout] |
| Stellar | 30 sondeos × 1 s + envío | `src/chain/stellar.rs:1810-1815` | No en el peor caso |
| Algorand | 20 sondeos × 500 ms + envío | `src/chain/algorand.rs:897-901` | Sí |
| Sui | `execute_transaction_block` (quorum driver) | `src/chain/sui.rs:781-788` | Sí [HIPÓTESIS] |
| XRPL | 30 sondeos × 1 s + `submit` | `src/chain/xrpl.rs:69-71`, `:1124-1125` | No en el peor caso |
| Hedera | 45 s por defecto (rango 5-60) + ejecución 10 s | `src/chain/hedera/config.rs:108`, `:113`, `src/chain/hedera/mod.rs:429` | No |

Conclusión: **API Gateway no sirve** para `/settle`. Sirven Function URL o, mejor para este
repo, un **target group de ALB de tipo `lambda`**: ya hay uno en producción
(`terraform/environments/production/lambda-balances.tf:217-252`) y el ALB ya reparte por peso
entre target groups (`terraform/environments/production/latency-split.tf:93-145`), que es
justo el mecanismo del plan de §8. Límites a revisar en esa ruta **[DOC AWS]**: cuerpo de
request y respuesta de 1 MB (el asset estático más grande, `static/index.html`, pesa unos
266 KB) y sin streaming: `GET /events` es SSE (`src/handlers.rs:2182-2190`) y no funciona tras
un target group Lambda; necesitaría Function URL con streaming o quedarse en Fargate.

El timeout de la función de escrituras debería cubrir el peor recibo configurado, y ahí hay un
borde: el de Ethereum (900 s, `src/chain/evm.rs:1179`) ya iguala el máximo de Lambda (15 min
**[DOC AWS]**), así que recibo + estimación + envío lo excede y Lambda mataría la invocación sin
respuesta. En Lambda hay que bajar `TX_RECEIPT_TIMEOUT_SECS` (`src/chain/evm.rs:1184`) por
debajo del timeout de la función para que el handler alcance a devolver
`SettlementUnconfirmed` (`src/chain/evm.rs:1232`, `:1274`; Sui en `src/chain/sui.rs:789-799`)
en vez de un corte de plataforma. El reenvío al holder (recibo + 30 s) no aplica en Lambda.

## 4. Arranque en frío

### 4.1 Qué hace `main()` (en orden)

1. `.env`, comando `receipts` y telemetría (`src/main.rs:128-141`).
2. `ProviderCache::from_env()` (`src/main.rs:142`): recorre **todas** las variantes de red en
   serie (`src/provider_cache.rs`, `src/chain/mod.rs:140-175`). Con todas las features hay
   **43** variantes, 20 de ellas testnets (`src/network.rs:398`); solo se construyen las que
   tienen RPC configurado. Construir un provider EVM no hace RPC (`src/chain/evm.rs:618`);
   Hedera sí carga la config de AWS y lanza recuperación y health
   (`src/chain/hedera/mod.rs:63-93`).
3. Writer lease (`src/main.rs:162`) y discovery owner (`src/main.rs:171`): en Lambda ambos
   salen rápido por falta de metadata de ECS.
4. Compliance: OFAC desde `config/ofac_addresses.json` (unos 208 KB) y blacklist
   (`src/main.rs:176-177`).
5. Stuck monitor, chain identity, autoverify (`src/main.rs:202-215`).
6. Transaction store (`src/main.rs:234`).
7. Registry del Bazaar desde S3: carga el snapshot completo (`src/main.rs:252`,
   `src/discovery.rs:1443`), el catálogo de 15 MB (`src/discovery_owner.rs:8`), y arma el
   índice de búsqueda.
8. Auto-registro si existe `FACILITATOR_URL` (`src/main.rs:284`).
9. Overlays de health y terms desde S3 (`src/main.rs:475-485`, `src/discovery_health.rs:912-920`).
10. Loops de discovery y atestación (`src/main.rs:343-545`).
11. Idempotencia y recibos (`src/main.rs:562-568`).
12. Rate policy y refresco del allowlist desde Secrets Manager (`src/main.rs:623-626`).
13. `TcpListener::bind` y `axum::serve` (`src/main.rs:887-900`).

### 4.2 Cuánto tarda

No medido. **[HIPÓTESIS]** el grueso está en el paso 7 (descarga y parseo de 15 MB más el
índice) y en las varias cargas de config de AWS; el resto es CPU local. Lambda limita la fase de
init a 10 s antes de reintentarla dentro de la invocación **[DOC AWS]**. Medición propuesta: el
campo `Init Duration` de la línea `REPORT` de CloudWatch en el canary de lecturas.

### 4.3 Secretos

ECS inyecta los secretos en variables de entorno (`secrets = local.all_task_secrets`,
`terraform/environments/production/main.tf:1324`; la lista se arma en
`terraform/environments/production/secrets.tf:518`). Lambda no tiene ese `valueFrom`: hay que
leer Secrets Manager en el init (o con la extensión de Parameters and Secrets) **[DOC AWS]**. Las
variables de entorno de Lambda tienen un tope total de 4 KB **[DOC AWS]**; decenas de URLs RPC con
API key probablemente no entran **[HIPÓTESIS]**. Es código nuevo en el arranque.

### 4.4 Qué puede ser perezoso

- Sí: provider por red al primer uso; registry del Bazaar e índice solo en las rutas de
  discovery (o una función de discovery aparte); overlays de health/terms; atestación;
  auto-registro (a EventBridge).
- No: compliance (antes de cualquier verify o settle), idempotencia, `NonceStore`, admisión y
  rate policy, y el parseo de llaves.

## 5. Estado en memoria que se pierde

| Estado | Dónde | Efecto en Lambda | Reemplazo |
|---|---|---|---|
| Nonces EVM | `src/chain/evm.rs:3160` | firmantes paralelos (§1.1) | DynamoDB o pool + lease |
| Rate limiting por IP (GCRA) | `src/rate_policy.rs:134`, `:493` | buckets por entorno; reinicio en cada cold start | WAF con reglas por tasa o contadores DynamoDB |
| Admisión (512 global / 32 por cliente) | `src/rate_policy.rs:1126-1139`, `:1175-1195` | sin sentido con una invocación por entorno | concurrencia reservada como techo |
| Tope diario ERC-8004 | `src/erc8004/daily_cap.rs:83-93` | el tope se multiplica por N y se reinicia en cada cold start: **gasto de gas sin control** | contador atómico DynamoDB por `(red, día)` |
| Jobs de registro ERC-8004 + lock en vuelo | `src/erc8004/register_jobs.rs:152-165` | el `GET` del jobId cae en otro entorno | tabla DynamoDB |
| Idempotencia | `src/idempotency_store.rs:55` (backend DynamoDB) | ya durable; ver §2.3 | esperar la escritura |
| Replay Solana/Stellar/Algorand | `src/nonce_store.rs:522-541` | memoria si falta la tabla | exigir `NONCE_STORE_TABLE_NAME` |
| Catálogo del Bazaar + índice | `src/discovery.rs:1434-1460` | 15 MB por entorno | S3 (ya existe) con ETag |
| Stats del discovery | `src/discovery.rs:855` | caché por entorno | aceptable |
| Bus de eventos SSE | `src/events.rs:216` | cada entorno ve solo lo suyo | Fargate o Function URL con streaming |
| Veredictos de autoverify | `src/payment_operator/autoverify.rs:39-44` | `/supported` distinto por entorno | DynamoDB/S3 escrito por EventBridge |
| Caché de readiness | `src/readiness.rs:87` | por entorno | aceptable |
| Cliente de reenvío y caché de estáticos | `src/handlers.rs:327`, `:1877` | por entorno | aceptable |

Regla: DynamoDB para todo lo atómico (nonces, leases, contadores, idempotencia, jobs); S3 para
snapshots grandes (catálogo, overlays). S3 no sirve como contador.

## 6. Montaje del router axum en Lambda

- Hoy: `TcpListener::bind` + `axum::serve(... into_make_service_with_connect_info::<SocketAddr>())`
  (`src/main.rs:887-900`). `Cargo.toml` no tiene `lambda_http` ni `lambda_runtime`.
- Precedente en `UltravioletaDAO/emporium` (solo leído): `lambda_http = "=1.3.1"` con la feature
  `apigw_http` (`rust/Cargo.toml:44`); el binario `rust/src/bin/lambda.rs:5-11` arma la app una
  vez por proceso y llama `lambda_http::run(app.router)`; `make zip` compila `bootstrap` para
  `x86_64-unknown-linux-musl` en `provided.al2023` (`rust/Makefile:11`, `:58-62`). Su Terraform
  usa Function URL sin auth, 512 MB, 15 s y concurrencia reservada 10
  (`infra/terraform/variables.tf:85-152`, `infra/terraform/lambda.tf:167-243`).
- Para x402-rs: un segundo binario `bootstrap` que reutilice la construcción del `Router` de
  `main()` y llame `lambda_http::run`. Diferencias con Emporium:
  - Features de `lambda_http`: `alb` (el plan usa ALB), no solo `apigw_http`.
  - IP del cliente: el rate limiting usa la última entrada de `X-Forwarded-For` y, si no hay
    header, el `ConnectInfo<SocketAddr>` del socket (`src/client_ip.rs:1-13`, `:74-80`). Con
    `lambda_http` no hay socket: detrás del ALB el header existe y la clave sigue igual, pero
    un request sin `X-Forwarded-For` respondería 500 en vez de 200, como documenta el test
    de `src/client_ip.rs:269-271`. El binario Lambda debe inyectar un `ConnectInfo` o un
    valor por defecto.
  - Timeout de 900 s, no 15 s (§3).
  - musl + `openssl-sys`: el árbol de dependencias trae OpenSSL; compilar para musl puede pedir
    `vendored` o rustls **[HIPÓTESIS]**.
- Alternativa: Lambda Web Adapter (extensión que traduce eventos a HTTP local) deja `main()`
  intacto con su `bind`; cuesta una capa más y no resuelve nada de §1, §2 ni §5.

## 7. Veredicto

### 7.1 Viable con cambios

| Riesgo | Severidad sin cambios | Cambio que lo cierra |
|---|---|---|
| Doble firma EVM/NEAR/Stellar | **P1 dinero** | asignador en DynamoDB o pool + lease por EOA |
| Settles > 30 s | P1 | ALB target `lambda` o Function URL, timeout 900 s |
| Trabajo tras la respuesta | P1 (mint ERC-8004), P2 (idempotencia) | `await` o SQS |
| Tope ERC-8004 por entorno | P1 gasto | contador DynamoDB |
| Loops de fondo | P2 | EventBridge + modo Lambda que los apague |
| Cold start (snapshot de 15 MB) | P2 latencia | discovery perezoso o en función aparte |
| SSE `/events` | P3 | dejarlo fuera de Lambda |
| Rate limiting por entorno | P2 abuso | WAF o DynamoDB |

### 7.2 Recomendación

Primero el híbrido A (§1.1): lecturas y discovery en Lambda; `/settle`, `/register`,
`/feedback` y `/dx402/anchor` (la regla `writes`, `latency-split.tf:128-137`) siguen en una
tarea Fargate. Mover escrituras solo después de B o C, con su propio estudio.

### 7.3 Coste

En la configuración: 2 tareas de 1 vCPU y 2 GB, piso 2 y techo 3
(`terraform/environments/production/production.auto.tfvars:66-74`), en `us-east-2`
(`production.auto.tfvars:57`). Con precios públicos de Fargate Linux/x86, eso es del orden de
70 USD/mes de cómputo **[ESTIMADO, revalidar con Cost Explorer]**, y es el **techo** del ahorro:
el ALB se queda (lo usa el plan), el híbrido A conserva una tarea Fargate, y las funciones de
EventBridge cuestan (health prober cada minuto). El ahorro de 45-50 USD/mes de c0der es
**[ESTIMADO]** y para el híbrido A parece optimista **[HIPÓTESIS]**.

## 8. Plan de migración sin riesgo

Todo detrás del mismo dominio y del mismo ALB.

0. **Preparación (código, PRs aparte):** binario `bootstrap`; modo Lambda que apague loops,
   writer lease y discovery owner; `await` de la idempotencia y `track_settlement`;
   ERC-8004 asíncrono a SQS; tope diario y jobs en DynamoDB; secretos leídos en el init;
   EventBridge para las tareas de §2.2. Ningún settle EVM en Lambda.
1. **Target group `lambda`** junto a `main` en la acción por defecto del listener
   (`terraform/environments/production/main.tf:495-498`), con `forward` por peso como en
   `latency-split.tf:107-125`. La regla `writes` queda 100 % Fargate.
2. **5 %** de lecturas a Lambda. Medir: tasa de 5xx, p50/p95/p99 por ruta, `Init Duration`,
   throttles, coherencia de `/supported` entre Lambda y Fargate. Ventana: 7 días
   **[HIPÓTESIS]**.
3. **50 %**, misma vigilancia; 7 días más.
4. **100 %** de lecturas. Fargate baja a **1 tarea** (sigue sirviendo escrituras y el lease) y
   el autoscaling deja de exigir 2.
5. **Escrituras:** solo tras B o C, por familia, empezando por las que no tienen nonce del
   facilitador (Solana, Algorand, XRPL, Hedera) mediante una ruta o header que el ALB pueda
   distinguir **[HIPÓTESIS: hoy la red viaja en el cuerpo y el ALB no puede rutear por ella]**.
6. **Rollback:** pesos de vuelta a `main` 100; si Fargate está en 0, primero subir
   `min_capacity` (el piso real, `production.auto.tfvars:69-73`). "Apagado pero listo"
   significa: imagen vigente en ECR, task definition al día, target group registrado y alarmas
   activas; arrancar una tarea tarda minutos **[HIPÓTESIS]**, así que no se apaga del todo
   mientras haya escrituras en Fargate.

## 9. Lo que este estudio no midió

- Duración real del init y de cada `from_env` (§4.2).
- Elección de la moneda de gas en Sui por los clientes (§1.6).
- Precios y volumen reales (§7.3).
- Los límites marcados **[DOC AWS]**: se citan de memoria de la documentación pública y deben
  revalidarse en la fecha de implementación.
