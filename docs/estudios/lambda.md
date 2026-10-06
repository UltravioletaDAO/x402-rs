# ¿El facilitador puede correr en AWS Lambda? (X402-LAMBDA-ESTUDIO, X402-LAMBDA-PLAN)

## Para el dueño (5 líneas)

1. **Ahorro:** contra la fase 1 del recorte (50,64 USD/mes en las filas que cambian), todo en Lambda cuesta 7,41 USD/mes (central) a 37,80 (pesimista): **ahorra 13-43 USD/mes**; el híbrido intermedio, ~20. La fase 1 ya baja hoy de 137,13 a 50,64 (§10).
2. **Rendimiento:** no mejora la experiencia. En caliente queda igual (verify y settle los domina el RPC o la cadena; las lecturas suman unos ms) y el p99 empeora por los arranques en frío (+1-4 s al request que los dispara, **[HIPÓTESIS]**). El pico de 5.913 req/h ocupa 1-3 entornos y la capacidad sobra en los dos mundos (§11).
3. **Trabajo:** L. Son unos 13 PRs en 4 fases, y el grueso es la fase 0 (nonces en DynamoDB, toca dinero). Lleva 6-10 semanas de calendario, la mayoría ventanas de canary de 7-14 días **[ESTIMADO]** (§12).
4. **Riesgo que decide el número:** Lambda cobra la espera del recibo de cada settle. Con 10x tráfico, el caso pesimista cuesta más que Fargate (221 contra 72 USD/mes). La duración media del target group `writes` (§10.5) cierra el rango y se mide en 5 minutos.
5. **Recomendación:** hacer **ya** solo la fase 0, que arregla fallos de hoy (carrera de nonce NEAR/Stellar, jobs y tope ERC-8004 que se pierden en cada deploy) y no depende de Lambda. Migrar (fases 1-3) solo si la medición del punto 4 confirma el caso central y después de 30 días de la fase 1 del recorte estable: no antes de diciembre de 2026.

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
sale del código; §7.3 da el techo que se ve en la configuración. **§10 lo reemplaza** con el
modelo fila por fila: 13-43 USD/mes con todo en Lambda, ~17-20 con el híbrido de una tarea.

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
**[ESTIMADO]** y para el híbrido A parece optimista **[HIPÓTESIS]**. §10.4-10.5 lo cuantifican.

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

## 10. Coste mensual: hoy, fase 1 del recorte y Lambda (X402-LAMBDA-PLAN)

Mes de 730 h, `us-east-2`. Etiquetas como arriba: **[DOC AWS]** = precio público de lista (revalidar
el día que se implemente), **[ESTIMADO]** = cifra que no sale ni del código ni de una medición, y
**[MEDIDO c0der]** = CloudWatch del ALB `facilitator-production` del 2026-10-06. El script que
calcula cada cifra es reproducible a mano con la fórmula de su fila.

### 10.1 Entradas

| Entrada | Valor | Fuente |
|---|---|---|
| Requests HTTP al mes (R) | 754.683 | [MEDIDO c0der], 30 días |
| Pico horario | 5.913 req/h = 1,64 req/s | [MEDIDO c0der], últimos 7 días |
| Bytes procesados | 43,9 GB/mes (~58 KB/request) | [MEDIDO c0der] |
| TargetResponseTime | p50 3,7 ms · p90 61 ms · p99 6,55 s | [MEDIDO c0der], 7 días, todo el ALB |
| p99 de lecturas (TG `main`) | 0,10-0,59 s | `terraform/environments/production/latency-split.tf:150-153` (2026-09-01) |
| p99 de escrituras (TG `writes`) | 7,0-7,7 s | `terraform/environments/production/latency-split.tf:185-186` |
| Fargate x86 | 0,04048 USD/vCPU-h + 0,004445 USD/GB-h | Price List, `docs/handoffs/2026-09-09-auditoria-arquitectura-costos-y-ui.md:114` |
| NAT Gateway | 0,045 USD/h + 0,045 USD/GB | [DOC AWS]; agosto facturó 33,48 h + 8,75 datos (auditoría `:100-101`) |
| IPv4 pública | 0,005 USD/h por dirección | [DOC AWS] |
| Endpoint de interfaz (Secrets Manager) | 0,01 USD/h, 1 AZ | `terraform/environments/production/variables.tf:35` |
| ALB | 0,0225 USD/h + 0,008 USD/LCU-h; un LCU = 1 GB/h a targets IP y **0,4 GB/h a targets Lambda** | [DOC AWS] |
| Lambda | 0,20 USD por millón de requests; arm64 0,0000133334 USD/GB-s; la fase INIT se factura | [DOC AWS] |
| Function URL | 0 USD (sin cargo propio) | [DOC AWS] |
| API Gateway HTTP API | 1,00 USD por millón | [DOC AWS]; **no sirve para `/settle`** (corta a 30 s, §3) |
| DynamoDB on-demand | 0,625 USD por millón de WRU · 0,125 USD por millón de RRU | [DOC AWS] |
| EventBridge Scheduler | 14 M invocaciones gratis al mes, luego 1 USD/M | [DOC AWS] |
| CloudWatch Logs | 0,50 USD/GB ingerido | [DOC AWS] |
| Secrets Manager API | 0,05 USD por 10.000 llamadas | [DOC AWS] |

No se descuenta el free tier de Lambda (1 M requests y 400.000 GB-s al mes): es de la cuenta y lo
comparten la Lambda de balances y los otros proyectos de UVD. Si quedara libre, el caso central de
Lambda bajaría a casi 0.

### 10.2 Memoria y duración que se recomiendan

- **Función de lecturas: 1024 MB, arm64.** Con 1769 MB Lambda da 1 vCPU **[DOC AWS]**, así que
  1024 MB ≈ 0,58 vCPU: lo mismo que la tarea de 0,5 vCPU de la fase 1. Le alcanza la memoria para
  el catálogo de 15 MB y su índice (`src/discovery_owner.rs:8`). Bajar a 512 MB alarga el arranque
  en frío, que es CPU (parseo e índice, §4.2).
- **Función de escrituras: 512 MB, arm64.** El settle pasa casi todo el tiempo esperando el recibo
  (§3). Lambda cobra esa espera por GB-s, y la mitad de memoria es la mitad de coste.
- **arm64** necesita compilar para `aarch64-unknown-linux-musl` (o gnu sobre `provided.al2023`).
  El precedente de Emporium es x86_64 (§6), y las crates nativas (OpenSSL, `hiero-sdk-proto`) en
  arm64 **[HIPÓTESIS]** hay que probarlas en la fase 0. En x86_64 los GB-s cuestan un 25 % más
  (0,0000166667) y las cifras de abajo suben en esa proporción.
- **Duración por request.** Solo hay percentiles del ALB entero, así que se arma con tramos:

| Tramo | Fracción | Central (representante) | Pesimista (borde superior) | Función |
|---|---|---|---|---|
| ≤ p50 | 50 % | 2 ms | 3,7 ms | lecturas |
| p50-p90 | 40 % | 20 ms | 61 ms | lecturas |
| p90-p99 | 9 % | 0,63 s (media geométrica de 61 ms y 6,55 s) | 6,55 s | escrituras |
| > p99 | 1 % | 10 s | 90 s (recibo de Base, `src/chain/evm.rs:1180`) | escrituras |

  El caso pesimista además cobra los tramos de escritura a 1 GB. Que el 10 % lento sea "escrituras"
  es una aproximación **[HIPÓTESIS]**; la medición de §10.5 la reemplaza.

  - Central: GB-s HTTP = R × (0,5 × 0,002 + 0,4 × 0,020) × 1 GB + R × (0,09 × 0,63 + 0,01 × 10) × 0,5 GB = **65.922 GB-s**.
  - Pesimista: GB-s HTTP = R × (0,5 × 0,0037 + 0,4 × 0,061 + 0,09 × 6,55 + 0,01 × 90) × 1 GB = **1.143.911 GB-s**.
- **Arranques en frío (C al mes) × init (I) × 1 GB.** Central: C = 3.000 (unos 100 por día) e
  I = 2 s, que da 6.000 GB-s. Pesimista: C = 30.000 e I = 5 s, que da 150.000 GB-s. **[HIPÓTESIS]**: el
  init no está medido (§4.2) y C depende de qué tan en ráfaga llega el tráfico (§11.3).

### 10.3 Los loops de fondo en EventBridge

De las 16 filas de §2.2, 12 dejan de ser `tokio::spawn`. Tres (3, 11 y 13) pasan a ser perezosas
dentro del request y no tienen schedule ni coste propio. Las otras nueve pasan a schedules de
EventBridge contra una función worker:

| Loop (§2.2) | Invocaciones/mes | Duración central / pesimista | Memoria | USD central / pesimista |
|---|---|---|---|---|
| 6 Health prober (tick 60 s, timeout por probe 12 s, `src/discovery_health.rs:60`) | 43.800 | 5 s / 15 s | 1 GB | 2,92 / 8,76 |
| 12 Recuperación de Hedera (30 s → 1 min) | 43.800 | 1 s / 3 s | 0,5 GB | 0,29 / 0,88 |
| 8 Stuck tx monitor (120 s, `src/stuck_tx_monitor.rs:54`) | 21.900 | 2 s / 6 s | 0,5 GB | 0,29 / 0,88 |
| 10 Autoverify (600 s) | 4.380 | 2 s / 10 s | 0,5 GB | 0,06 / 0,29 |
| 4 Agregación del Bazaar (3600 s) | 730 | 60 s / 300 s | 2 GB | 1,17 / 5,84 |
| 7 Atestación ERC-8004 (configurable; se supone horaria) | 730 | 2 s / 10 s | 0,5 GB | 0,01 / 0,05 |
| 14 Sweeper DX402 (se supone horario) | 730 | 5 s / 30 s | 0,5 GB | 0,02 / 0,15 |
| 9 Chain identity (reprobe horario) | 730 | 2 s / 10 s | 0,5 GB | 0,01 / 0,05 |
| 5 Crawler (diario, hoy apagado) | 30 | 300 s / 900 s | 1 GB | 0,12 / 0,36 |
| **Total** | **116.830** | | | **4,89 / 17,25** |

Fórmula de cada fila: invocaciones × duración × GB × 0,0000133334. Las duraciones son
**[HIPÓTESIS]**. La más grande, el health prober, está acotada por código: a lo sumo
`DISCOVERY_HEALTH_CONCURRENCY` (8) probes en vuelo de 12 s como máximo (`src/main.rs:441-449`).
EventBridge Scheduler queda dentro de las 14 M invocaciones gratis: 0 USD.

### 10.4 Tabla de coste (solo las filas que cambian entre columnas)

| Fila | Fórmula | Hoy | Fase 1 del recorte | Lambda: central / pesimista |
|---|---|---:|---:|---:|
| Fargate | tareas × 730 × (vCPU × 0,04048 + GB × 0,004445) | **72,08** (2 × 1 vCPU/2 GB) | **36,04** (2 × 0,5/1) | 0 |
| Container Insights | handoff 2026-08-07, `docs/COST_RIGHTSIZING_HANDOFF_2026-08-07.md:17` | 12,50 [ESTIMADO, rango 10-15] | 0 (B8 de #115) | 0 |
| NAT: horas | 0,045 × 730 | 32,85 | 0 (paso e de #115) | 0 |
| NAT: datos | agosto facturado | 8,75 | 0 | 0 |
| IPv4 pública | direcciones × 0,005 × 730 | 3,65 (EIP del NAT) | **7,30** (una por tarea, B5 de #115) | 0 |
| Endpoint Secrets Manager | 0,01 × 730 | 7,30 | 7,30 | 0 (Lambda fuera de la VPC, ver §12 fase 3) |
| Lambda: requests | (R + 116.830) × 0,20 / 10⁶ | — | — | 0,17 / 0,17 |
| Lambda: GB-s HTTP | §10.2 × 0,0000133334 | — | — | 0,88 / 15,25 |
| Lambda: GB-s init | C × I × 1 GB × 0,0000133334 | — | — | 0,08 / 2,00 |
| Lambda: loops (EventBridge) | §10.3 | — | — | 4,89 / 17,25 |
| ALB: LCU extra por target Lambda | (43,9/720/0,4 − 43,9/720/1) × 0,008 × 730 | — | — | 0,53 / 0,53 |
| Function URL / API Gateway | ALB target `lambda`: 0. API GW: R × 1/10⁶ = 0,75, descartado (§3) | — | — | 0 / 0 |
| DynamoDB nuevo (nonces, leases, contadores) | WRU × 0,625 / 10⁶; ver abajo | — | — | 0,64 / 1,11 |
| Logs de plataforma y boot | (invocaciones × 350 B o 1 KB + C × 20 KB o 50 KB) × 0,50/GB | — | — | 0,18 / 1,19 |
| Secrets Manager en el init | C × 2 llamadas (`BatchGetSecretValue`, 29 secretos) × 0,05/10⁴ | — | — | 0,03 / 0,30 |
| **Total de las filas que cambian** | | **137,13** | **50,64** | **7,41 / 37,80** |
| **Ahorro mensual neto contra la fase 1** | 50,64 − Lambda | | | **43,23 / 12,84** |

Desglose de DynamoDB (on-demand, todo con escritura condicional o `ADD`). En el central suma
1,02 M WRU y en el pesimista 1,78 M:

- nonces EVM/NEAR/Stellar: 2 WRU por settle, con settles ≤ 10 % de R;
- leases de los loops: 1 WRU por invocación;
- rate limiting: 1 WRU por request en el central, 2 en el pesimista (IP y cliente);
- tope ERC-8004, jobs de registro y veredictos de autoverify: unos miles al mes;
- almacenamiento: con TTL no llega a centavos.

Si en vez de DynamoDB el rate limiting va a WAF, la fila es 5 (web ACL) + 2 × 1 (reglas) +
0,60 × R / 10⁶ = **7,45 USD/mes** **[DOC AWS]**, y el ahorro central baja a 36.

**Lo que no cambia entre columnas** y por eso queda fuera de la suma:

- ALB: 16,43 USD/mes de horas, LCU base y las IPv4 de sus nodos. Lo usan las tres columnas.
- Transferencia a internet: los mismos 43,9 GB.
- Tablas DynamoDB existentes: nonces, idempotencia, transacciones, Hedera y DX402.
- S3 del discovery, almacenamiento de los secretos (0,40 USD por secreto), ECR, métricas custom
  y alarmas.

### 10.5 Sensibilidad y la medición que cierra el rango

- **"Hoy: 2 tareas" es el piso, no el promedio.** El autoscaling apunta a 15 req/min por tarea
  con techo 3 (`terraform/environments/production/production.auto.tfvars:73-77`). El tráfico
  medio, 17,5 req/min, entra en 2 tareas; el pico, 98,6 req/min, pide 7 y se queda en 3. La
  auditoría del 2026-09-09 midió 2,862 tareas de promedio (auditoría `:60`). Con ese promedio,
  Fargate hoy es 103,15 USD y en la fase 1 51,57, y el ahorro de Lambda sube unos 15 USD. Las
  cifras de arriba usan 2 tareas para no inflar el ahorro.
- **La incógnita que más pesa es la espera del settle** (filas HTTP pesimista, 15,25, y loops,
  17,25). Se cierra midiendo en CloudWatch, por target group, `RequestCount` (Sum) y
  `TargetResponseTime` (Average) de `writes` y `main` en 30 días. Así:
  - GB-s de escrituras = Σ(Average × RequestCount) × 0,5;
  - GB-s de lecturas = lo mismo × 1.

  Es una consulta de solo lectura; este estudio no la corre (no toca AWS).
- **Híbrido (fin de la fase 1 del plan):** lecturas en Lambda; escrituras, `/mcp`, `/events` y
  los loops en Fargate.
  - Con **1 tarea** de 0,5 vCPU: 18,02 + 3,65 (IPv4) + 7,30 (endpoint) + 1,39-5,12 (Lambda de
    lecturas) = **30,36-34,09 USD**, que ahorra **~17-20** contra la fase 1. Choca con
    `min_capacity = 2 # a single task is not a service` (`production.auto.tfvars:73`): un solo
    escritor es una caída de escrituras de minutos en cada reemplazo.
  - Con **2 tareas**: **52,03-55,76 USD**, más caro que la fase 1. El híbrido solo tiene sentido
    como paso de canary, no como destino.
- **Escenario x10 (7,55 M requests/mes; pico 16,4 req/s):**

| | Fase 1 (3 tareas, techo del autoscaling) | Lambda central | Lambda pesimista | Híbrido, 1 tarea |
|---|---:|---:|---:|---:|
| USD/mes | 72,31 (54,06 + 3 IPv4 10,95 + endpoint 7,30) | 28,05 | 221,43 | 41,89-80,16 |
| Ahorro contra la fase 1 | — | **+44,26** | **−149,12** | +30,42 / −7,85 |

  Supuestos del x10:
  - Lambda escala casi lineal, a 3,04 USD por millón de requests en el central y 27,03 en el
    pesimista.
  - Los loops no escalan.
  - Los arranques en frío crecen ×3 en el central y ×10 en el pesimista.
  - Que 3 tareas de 0,5 vCPU aguanten x10 es una **[HIPÓTESIS]**.
- **Punto de equilibrio contra la fase 1:**
  - central: (50,64 − 5,12 fijo) / 3,04 ≈ **15 M requests/mes (x20)**;
  - pesimista: (50,64 − 17,4) / 27,03 ≈ **1,2 M requests/mes (x1,6)**.

  Si el tráfico crece y la espera del settle se parece al pesimista, Lambda deja de ahorrar
  antes de llegar a x2.

## 11. Rendimiento por paso del flujo

### 11.1 Qué se mide hoy

El ALB no tiene métricas por ruta. Hay tres fuentes:

- el ALB entero ([MEDIDO c0der]): p50 3,7 ms, p90 61 ms, p99 6,55 s;
- el target group `main`, que sirve lecturas, `/verify` y `/mcp`: p99 0,10-0,59 s;
- el target group `writes` (`/settle`, `/feedback*`, `/register`, `/dx402/anchor`): p99 7,0-7,7 s
  (`latency-split.tf:128-137`, `:150-153`, `:185-186`).

Los p50 y p90 por paso salen de los access logs del ALB, que ya están encendidos
(`alb_access_logs_enabled = true`, `production.auto.tfvars`): una consulta de Athena por
`request_url` sobre `target_processing_time`. No se corrió acá. Mientras tanto, la tabla usa el
p50/p90 global para las lecturas, que son la mayoría del tráfico **[HIPÓTESIS]**.

### 11.2 Tabla por paso

Supuestos de las columnas Lambda **[HIPÓTESIS]**, a medir en el canary de la fase 1:

- Lambda tibio = la invocación ALB → Lambda y la conversión del evento suman 1-10 ms.
- Si el rate limiting pasa a DynamoDB, esa puerta suma 5-10 ms por la escritura condicional.
- Arranque en frío = el init de §4: 1-4 s con el snapshot del Bazaar en el init, 0,3-1 s con
  discovery perezoso (§4.4). Lo paga solo el request que crea el entorno.

| Paso | Hoy (medido) | Lambda tibio | Lambda en frío | Veredicto |
|---|---|---|---|---|
| `/supported`, `/networks.json`, `/accepts` | p50 3,7 ms · p90 61 ms · p99 ≤ 0,59 s (TG `main`) | p50 5-20 ms · p90 65-80 ms · p99 igual | +0,3-4 s | p50 empeora unos ms; p90 igual; **p99 empeora** con los arranques en frío |
| Discovery (`/discovery/resources`, búsqueda) | igual que la fila anterior | igual que la fila anterior | +0,5-2 s si el catálogo de 15 MB se carga perezoso en el primer uso | igual en caliente; **peor en frío** |
| `/verify` | dentro del p99 de `main` (≤ 0,59 s); objetivo de `/verify` p99 < 1 s a 50 concurrentes (`docs/handoffs/2026-08-20-diagnostico-performance-facilitador.md:639`) | igual + 1-10 ms; el pool HTTP al RPC vive con el entorno | +init y el handshake TLS al RPC de la red en la primera llamada (50-300 ms) | **igual** en caliente; peor en frío |
| `/settle` EVM | p99 7,0-7,7 s (`writes`); Base hasta 90 s, Ethereum hasta 900 s; la réplica sin lease reenvía al holder (`src/handlers.rs:1718-1747`) | igual: la cadena domina. Se va el salto de reenvío y entra una escritura condicional de nonce (+5-10 ms) | +init (≤ 4 s sobre una espera de 2-90 s) | **igual** |
| `/settle` Solana | confirmación ≤ 90 s, sondeo de 500 ms (`src/chain/solana.rs:2513-2516`) | igual | +init | **igual** |
| `/settle` NEAR | `broadcast_tx_commit`, ≤ 10 s | igual + el lock de nonce de la fase 0 | +init | **igual** |
| `/settle` Stellar y XRPL | sondeo de 1 s, hasta 30 intentos | igual (la granularidad de 1 s domina) | +init | **igual** |
| `/settle` Algorand | sondeo de 500 ms, hasta 20 | igual | +init | **igual** |
| `/settle` Sui y Hedera | quorum driver; Hedera ≤ 45 s | igual | +init | **igual** |
| MCP `POST /mcp` | va a `main` y hereda la latencia de la herramienta: `x402_settle` sale del p99 de lecturas, porque hoy cae en `main` | igual: es stateless y JSON (`src/mcp.rs:1015-1023`) | +init | **igual**, si se rutea al riel de escrituras (§12 fase 1) |
| `GET /events` (SSE) | streaming | no funciona tras un target Lambda (§3) | — | **se pierde** si Fargate se apaga sin decidir dónde vive |

Lo único que **mejora**: el health prober y la agregación dejan de compartir CPU con los
settles. En una tarea de 1 vCPU, los probes producían picos de CPU del 60-100 % cada minuto
(`src/main.rs:428-436`). En la fase 1 del recorte la tarea tiene la mitad de CPU, así que ese
riesgo crece. En Lambda cada loop corre en su propio entorno.

### 11.3 El pico de 5.913 req/h

- **Concurrencia (ley de Little):**
  - lecturas ≈ 1,64 × 0,9 × 0,02 s ≈ 0,03 entornos;
  - escrituras ≈ 1,64 × 0,1 × 0,63-6,55 s ≈ 0,1-1,1 entornos.

  Total: **1-3 entornos simultáneos**. A 1,64 req/s llega un request cada 0,6 s, así que el
  entorno de lecturas no llega a enfriarse; los arranques en frío vienen de ráfagas (N requests
  en el mismo instante crean N entornos) y del reciclado periódico de entornos **[HIPÓTESIS]**.
  La forma de las ráfagas no está medida; en el canary, `ConcurrentExecutions` y `Init Duration`
  la dan.
- **Fargate hoy:** 2-3 tareas con admisión de 512 en vuelo cada una (`src/rate_policy.rs:1126`).
  El pico pide 7 tareas al autoscaling y recibe 3, sin colas observadas. Ningún lado se queda
  corto.
- **Lambda:** escala en segundos. La concurrencia de la cuenta (1000 por defecto) y la tasa de
  escalado (1000 entornos cada 10 s por función) **[DOC AWS]** sobran con 1-3. La concurrencia
  reservada propuesta (50 lecturas, 20 escrituras) es un techo de gasto y de carga sobre los RPC,
  no una limitación.
- **x10 (16,4 req/s de pico):** 1-11 entornos. Fargate se queda en 3 tareas de 0,5 vCPU por el
  techo del autoscaling **[HIPÓTESIS: suficiente]**.

## 12. Plan ejecutable por fases

Reglas para todas las fases:

- cada tarea es un PR propio contra `main`;
- en las fases 1-3, un cambio de pesos del ALB por apply;
- nada se apaga antes de que su reemplazo pase el criterio de salida.

Tallas: S ≤ 1 día, M 2-4 días, L ≥ 1 semana de trabajo efectivo **[ESTIMADO]**, sin contar la
ventana de observación.

### Fase 0: prerequisitos que también mejoran el Fargate de hoy (L)

| # | Tarea (archivo y cambio) | Mejora hoy en Fargate | Criterio de salida | Cómo se prueba | Rollback | Talla |
|---|---|---|---|---|---|---|
| 0.1 | Asignador de nonce EVM en DynamoDB. Nuevo `src/nonce_allocator.rs` con `UpdateItem` condicional por `(chain, EOA)` en `facilitator-nonces` (`terraform/environments/production/main.tf:637-639`), detrás de `PendingNonceManager` (`src/chain/evm.rs:3160`). Conserva la deriva y los huecos de `NonceState` (`src/chain/evm.rs:3195`, `:3237`) y `NONCE_TRUST_CHAIN_AFTER_DRIFT`. Interruptor `NONCE_ALLOCATOR=lease\|dynamo` (default `lease`). | Las 2 tareas firman EVM sin reenviar al holder; desaparecen el salto de `src/handlers.rs:1718-1747` y la espera de handover del lease (TTL 30 s, margen 10 s, `src/writer_lease.rs:159-176`). | 7 días en producción con `dynamo` y 2 tareas firmando: 0 `nonce too low` / `replacement underpriced`, 0 `evm_signer_transactions_stuck` (`src/stuck_tx_monitor.rs`), tasa de settles OK ≥ la semana anterior. | Unitarios con N asignadores concurrentes contra DynamoDB Local: nunca un nonce repetido. Testnet (Base Sepolia): 100 settles concurrentes desde 2 procesos. | `NONCE_ALLOCATOR=lease` y redeploy; el lease sigue en el código. | L |
| 0.2 | NEAR y Stellar con el mismo asignador o con lock por cuenta: `src/chain/near.rs:755`, `src/chain/stellar.rs:1621-1632`. | Cierra la carrera que **ya existe** dentro de un proceso (§1.2, §1.3). | 7 días sin `InvalidNonce` (NEAR) ni `tx_bad_seq` (Stellar). | Test de 2 settles concurrentes por familia con mocks del RPC: el segundo usa nonce+1. | Interruptor por familia. | M |
| 0.3 | Sacar de los handlers los `tokio::spawn` que sobreviven a la respuesta (§2.3): `await` de idempotencia (`src/handlers.rs:5946`) y `track_settlement` (`:5844`); mint ERC-8004 asíncrono (`:11117-11129`) a una cola SQS con DLQ (nuevo `terraform/environments/production/erc8004-queue.tf`). Drenaje del discovery (`src/discovery.rs:1756`) y revalidación (`src/discovery_revalidation.rs:288`) con `await` o a la misma cola. | Un deploy o un scale-in ya no pierde el mint que respondió 202. | `grep` en CI: ningún `tokio::spawn` en `src/handlers.rs` fuera de una lista permitida; DLQ vacía 7 días. | Test que manda SIGTERM a mitad de un `/register` y verifica que el job sigue en la cola. | Revert del PR; la cola queda sin consumidores. | M |
| 0.4 | Estado por proceso a DynamoDB: tope diario (`src/erc8004/daily_cap.rs:83-93`) como contador atómico `(red, día)`; jobs de registro (`src/erc8004/register_jobs.rs:52-55`) como tabla; veredictos de autoverify (`src/payment_operator/autoverify.rs:39-44`) escritos por el loop y leídos por `/supported`. | Hoy un cambio de lease (cada deploy) o un reinicio **borra** los jobs y reinicia el tope (`src/erc8004/register_jobs.rs:53-55`). | Un deploy en medio de un registro: el `GET` del jobId responde el estado real; el tope del día sobrevive a un reinicio. | Unitarios del contador con escritura condicional y prueba manual del deploy en staging o testnet. | Interruptor `ERC8004_STATE=memory\|dynamo`. | M |
| 0.5 | `NONCE_STORE_TABLE_NAME` obligatorio en modo Lambda: fail-closed en `src/nonce_store.rs:522-541`. | — (Fargate ya la tiene) | El binario Lambda no arranca sin la variable. | Unitario. | Revert. | S |
| 0.6 | Modo `FACILITATOR_RUNTIME=lambda` en `src/main.rs`: apaga writer lease (`:162`), discovery owner (`:171`) y loops (`:343-545`); registry y overlays perezosos (§4.4). | Arranque más rápido también en ECS si se reutiliza lo perezoso. | `cargo run` con el modo: `/health` en < 1 s locales, sin loops en los logs. | Test de integración del router con el modo y un medidor de tiempo del init por paso (cierra §4.2). | Variable ausente = comportamiento de hoy. | M |
| 0.7 | Binario `src/bin/lambda.rs` (`bootstrap`): saca de `main()` la construcción del `Router` a una función compartida y llama `lambda_http::run` (feature `alb`; la dependencia entra en **ese** PR, no en este). `ConnectInfo` por defecto para `src/client_ip.rs:74-80`. Secretos con `BatchGetSecretValue` en el init (§4.3). Build `aarch64` con rustls u OpenSSL `vendored`. | — | El ZIP compila en CI; un evento ALB de prueba devuelve `/supported` idéntico al de Fargate (comparación byte a byte del JSON normalizado). | `cargo lambda` o `lambda_http` con eventos de prueba en tests. | No se despliega hasta la fase 1. | M |
| 0.8 | Medición: consulta de Athena por ruta sobre los access logs y `RequestCount` y `TargetResponseTime` por TG (§10.5). | Da p50/p90/p99 por paso también para Fargate. | La tabla de §11.2 con cifras medidas, no globales. | — | — | S |

### Fase 1: lecturas en Lambda, mismo dominio, pesos 5 % → 50 % → 100 % (M)

| # | Tarea | Criterio de salida | Cómo se prueba | Rollback | Talla |
|---|---|---|---|---|---|
| 1.1 | Nuevo `terraform/environments/production/lambda-facilitator.tf`: `aws_lambda_function` de lecturas (arm64, `provided.al2023`, 1024 MB, timeout 60 s, concurrencia reservada 50, **fuera de la VPC**, porque dentro necesitaría NAT para los RPC). Log group con `var.log_retention_days`. Rol IAM de lectura (S3 del discovery, tablas DynamoDB, Secrets). `aws_lb_target_group` `target_type = "lambda"`, `aws_lb_target_group_attachment` y `aws_lambda_permission`, como en `lambda-balances.tf:217-252`. | `terraform plan` solo agrega recursos; peso 0. | El plan revisado y un invoke directo con evento ALB. | `destroy` de lo agregado. | M |
| 1.2 | Antes de mover pesos: `/mcp` y `/events` a reglas que los fijen en Fargate. `/mcp` al riel `writes` (`latency-split.tf:128-137`), porque lleva `x402_settle`. `/events` a `main` con prioridad propia. | `curl` a `/mcp` y `/events` respondido por Fargate (header de versión o log). | Smoke test de MCP `tools/list` y `x402_supported`. | Quitar las reglas. | S |
| 1.3 | `default_action` del listener (`main.tf:495-498`) a `forward` por peso (patrón de `latency-split.tf:111-125`): `main` 95, `reads-lambda` 5. | **5 %, 7 días:** 5xx de Lambda ≤ 5xx de `main` + 0,1 pp; p99 del TG Lambda < 2 s (umbral de `latency_reads_p99`); Throttles = 0; `Init Duration` p99 < 3 s; `/supported` igual en las dos rutas (hash del JSON normalizado). | Alarmas nuevas copiadas de `latency_reads_p99` para el TG Lambda, más Errors, Throttles y un filtro de métrica sobre `Init Duration`. | Pesos `main` 100 en un apply (segundos). | S |
| 1.4 | Pesos 50/50 y luego 0/100. | **50 %, 7 días** y **100 %, 7 días** con los mismos umbrales; coste diario de Lambda en Cost Explorer ≤ 0,20 USD (1,39-5,12 al mes, §10.5). | Las mismas alarmas. | Pesos atrás. | S |
| 1.5 | Build y deploy del ZIP en `.github/workflows/ci.yaml`: otro PR, que sí toca workflows. | Cada release publica imagen y ZIP con el mismo `VERSION`. | El CI. | Revert. | M |

Al cerrar la fase 1, Fargate sigue con 2 tareas. Bajar a 1 ahorra unos 20 USD (§10.5) contra
`min_capacity = 2`: la decisión es del dueño.

### Fase 2: escrituras y loops (L)

| # | Tarea | Criterio de salida | Cómo se prueba | Rollback | Talla |
|---|---|---|---|---|---|
| 2.1 | Función de escrituras (512 MB, timeout 900 s, concurrencia reservada 20) con `TX_RECEIPT_TIMEOUT_SECS` por debajo del timeout (p. ej. 840 s), para devolver `SettlementUnconfirmed` y no un corte (§3). Requiere 0.1-0.5 en `dynamo` desde hace ≥ 7 días. | — | Invoke directo en testnet por familia. | No recibe tráfico. | M |
| 2.2 | Regla `writes` (`latency-split.tf:111-125`) a tres target groups: `main` 0, `writes` (Fargate) 95, `writes-lambda` 5. Como Fargate y Lambda usan **el mismo** asignador de 0.1, firmar desde los dos a la vez es seguro, y no hace falta rutear por familia (el ALB tampoco puede: la red viaja en el cuerpo). | **5 % → 50 % → 100 %, 14 días cada uno:** éxito de settles por familia ≥ Fargate; p99 < 15 s (umbral de `latency_writes_p99`); 0 errores de nonce; DLQ de ERC-8004 vacía. | Las alarmas de `latency_writes_p99` y del stuck monitor, ahora como worker. | Pesos `writes` Fargate 100; no hay nonces que reconciliar porque el asignador es compartido. | M |
| 2.3 | Worker `src/bin/worker.rs` con un evento por loop (§10.3). Schedules en `terraform/environments/production/lambda-facilitator-jobs.tf`. Cada job toma un lease DynamoDB, para que el owner de Fargate y el worker no corran el mismo loop a la vez. El stuck monitor guarda su "cabeza vista" en DynamoDB, porque necesita 10 min de historia (`src/stuck_tx_monitor.rs:47-50`). | 7 días con los loops en Lambda y el owner de Fargate apagado por variable: catálogo actualizado cada hora, overlays de health al día, alarmas de stuck tx vivas (inyectar un caso en testnet). | Un test por job con su evento y un apply en canary. | Encender de nuevo el owner de Fargate (variable) y apagar los schedules. | L |
| 2.4 | `/mcp` del riel Fargate al de escrituras Lambda por peso, como en 2.2. | Igual que 2.2. | Smoke test de MCP. | Pesos atrás. | S |

### Fase 3: apagar Fargate, listo para volver (S)

| # | Tarea | Criterio de salida | Cómo se prueba | Rollback | Talla |
|---|---|---|---|---|---|
| 3.1 | Decidir `/events` (pregunta abierta): Function URL con response streaming en un subdominio, o quitar `/events/live` de `static/bazaar.html:117` y `static/dx402.html:160`. | Ninguna página enlaza a algo que no responde. | Smoke test. | — | S |
| 3.2 | `min_capacity = 0` y `desired_count` 0 por CLI (`production.auto.tfvars:72-74`; `desired_count` tiene `ignore_changes`). Los target groups `main` y `writes` quedan registrados con peso 0. La task definition sigue al día en cada release (CI) y la imagen se conserva en ECR (anclas de `scripts/ecr_rollback_anchors.py`, B15 de #115). | 30 días con Fargate en 0 sin rollback; Cost Explorer: Fargate 0. | Simulacro de vuelta (abajo) en una ventana. | **Runbook de vuelta:** `min_capacity = 2`, apply, esperar `HealthyHostCount = 2`, pesos a Fargate 100. Tiempo ~5-10 min **[HIPÓTESIS]**; medirlo en el simulacro. | S |
| 3.3 | Endpoint de Secrets Manager (7,30 USD): quitarlo solo si Fargate sigue en 0 a los 30 días. Para volver, las tareas en subred pública (B5) llegan al endpoint público de Secrets Manager. Verificar antes que `private_dns_enabled` (`main.tf:376`) no deje un nombre colgado. | Fargate arranca sin el endpoint en el simulacro. | Simulacro. | Re-crear el endpoint (apply). | S |

**Total:** fase 0 L (8 PRs), fase 1 M (5), fase 2 L (4), fase 3 S (3): unos 20 PRs chicos, o 13
si se agrupan los S. Calendario de 6-10 semanas **[ESTIMADO]**, la mayoría ventanas de
observación (7 + 7 + 7 días en la fase 1 y 14 × 3 en la fase 2).
