# Facilitador: auditoría de Rust, AWS, costos y reorganización de UI

**Inicio:** 2026-09-09, America/New_York. Cierre: 2026-09-10.  
**Repositorio:** `x402-rs`, base `9893c48f`; producción observada: `2.16.0-9893c48`, task definition 403.  
**Objetivo:** reducir latencia y errores sin aumentar el costo operativo.  
**Estado:** auditoría y plan terminados; cambios de UI implementados localmente. No se desplegó, modificó AWS, movieron fondos ni cambiaron los caminos de pago. Las modificaciones de `src/handlers.rs` corresponden exclusivamente a pruebas de presentación.

## 1. Decisión recomendada

Conservar el servicio Rust en ECS y corregir primero la coordinación, la persistencia y los errores de pago. La evidencia no justifica migrar a Kubernetes, añadir microservicios, contratar más cómputo ni sustituir Rust. La infraestructura tiene capacidad ociosa, pero reducirla sin revisar el trabajo en espera podría empeorar los picos.

Prioridades:

1. Resolver el margen de gas comprometido por transacciones pendientes y clasificar correctamente ese fallo.
2. Proteger la exclusividad del escritor y las escrituras del catálogo ante fallos/concurrencia.
3. Evitar trabajo de discovery duplicado y reutilizar conexiones entre réplicas.
4. Medir capacidad por ruta/red; después ajustar autoscaling y tamaño de tarea.
5. Implementar la sincronización comercial descrita en el [handoff de precios](2026-09-09-bazaar-precios-dinamicos-y-sincronizacion.md).

## 2. Alcance y evidencia

Se consultaron AWS ECS, Application Auto Scaling, EC2/VPC, CloudWatch Metrics/Logs, Cost Explorer y Price List mediante APIs de lectura. Se revisaron arranque, proveedores RPC, forwarding, writer lease, discovery/persistencia, telemetría, Docker, Terraform y las páginas estáticas.

El [snapshot de evidencia](2026-09-09-auditoria-evidencia.json) conserva las cifras relevantes sin credenciales, URLs de RPC, payloads de compradores ni la facturación completa de otros proyectos. No contiene todos los logs originales. Las consultas posteriores deben repetir ventanas y filtros, porque producción cambia.

- Infraestructura: captura 2026-09-10 03:36 UTC.
- Métricas: 2026-09-03 03:36 → 2026-09-10 03:36 UTC, intervalos de 10 minutos.
- HTTP de aplicación: 24 horas hasta aproximadamente 2026-09-10 03:42 UTC.
- Errores de settlement: 24 horas hasta aproximadamente 03:58 UTC.
- Costos: agosto cerrado y septiembre 1–10 parcial. Cost Explorer marcó septiembre como estimado y todavía mostraba aproximadamente 207 horas en partidas de NAT/ALB, no las 216 horas completas del intervalo.

Esta es una auditoría de arquitectura y operación con revisión dirigida de código. No constituye un pentest completo, un inventario exhaustivo de IAM, un benchmark de saturación ni una garantía sobre todas las redes/SDKs. No se reprodujeron pagos reales para medir rendimiento.

## 3. Qué está corriendo

| Componente | Observación |
|---|---|
| Servicio | `facilitator-production`, `us-east-2`, Fargate Linux |
| Tareas | 3 deseadas y activas; autoscaling mínimo 2, máximo 3 |
| Tamaño por tarea | 1 vCPU, 2 GiB; arquitectura por defecto x86_64 |
| Distribución | 2 tareas en `us-east-2b`, 1 en `us-east-2a` |
| Contenedores | Una aplicación; sin sidecar de OpenTelemetry en la task definition observada |
| Target groups | Lecturas y escrituras separados, apuntando al mismo servicio/tareas |
| Escalado | `ALBRequestCountPerTarget`, objetivo 15; memoria objetivo 80% |
| Discovery | Sin overrides de activación en la task definition: agregación y health usan defaults activos; crawler de semillas usa default desactivado |
| Egreso | Un NAT; endpoints Gateway de S3/DynamoDB, interfaz de Secrets Manager y endpoint administrado de GuardDuty |
| Observabilidad | Container Insights activo; logs de aplicación con retención de 30 días |
| Atribución ECS | `propagateTags=NONE`, `enableECSManagedTags=false` |

Los arranques inspeccionados registran `available_parallelism=2`. El mensaje se llama “tokio worker threads”, pero el código lee el paralelismo disponible, no un contador independiente del runtime. No hay override `TOKIO_WORKER_THREADS` en la task definition examinada. No hay evidencia actual para fijarlo arbitrariamente en 8/16.

## 4. Rendimiento observado

| Señal, últimos 7 días | Resultado |
|---|---:|
| CPU: media de promedios de 10 minutos | 1.25% |
| CPU: máximo de una muestra reportada | 58.52% |
| Memoria: media de promedios | 13.78%, aproximadamente 282 MiB |
| Memoria: máximo reportado | 19.34%, aproximadamente 392 MiB |
| Número medio de tareas | 2.862; pico de 6 durante despliegues |
| Requests del target group de lectura | 240,103 |
| 5xx del target group de lectura | 31 |
| Requests del target group de escritura | 14,493 |
| 5xx del target group de escritura | 8,551 |

El último cociente ronda 59%, pero mezcla rutas y tráfico real/reintentos; **no es una tasa de pagos únicos fallidos**. Los target groups incluyen otras operaciones además de `/settle`. Tampoco basta promediar percentiles para obtener un percentil global: la mediana de los p95 por intervalo fue 123 ms en lecturas y 6.78 s en escrituras; se conserva esa etiqueta estadística en la evidencia.

La consulta de logs por ruta proporciona otra perspectiva:

| Ruta / status, 24 horas | Eventos HTTP de aplicación | p95 aproximado de esos eventos |
|---|---:|---:|
| `/discovery/resources` 200 | 10,304 | 138 ms |
| `/supported` 200 | 2,058 | 1 ms |
| `/settle` 200 | 892 | 7.28 s |
| `/settle` 502 | 3,749 | 268 ms |
| `/feedback` 500 | 275 | 1.28 s |
| `/register` 500 | 64 | 10.17 s |
| `/dx402/anchor` 201 | 255 | 1.30 s |

Hay logs en el receptor y en el escritor cuando existe forwarding. Estos eventos no deben sumarse como transacciones independientes ni compararse directamente con ALB. Los 200 de settlement deben además separarse por éxito comercial del cuerpo. Los 26,360 logs de `/health` 200 en la misma ventana muestran una oportunidad de reducir ruido de telemetría; no se propone eliminar health checks.

### Hallazgo urgente: margen de gas y cola

De 2,238 eventos de handler que coincidían con los mensajes de fallo de settlement consultados, **2,232 contienen `insufficient funds`**. Las muestras de escrow dicen `insufficient funds for gas * price + value` e incluyen un `queued cost` que deja menos saldo utilizable que el costo de la siguiente transacción. El saldo total de la muestra era positivo. Por tanto, “la wallet tiene saldo” no descarta el problema.

Se confirmó el motivo del RPC en esas muestras, pero no la red ni el signer responsables de todos los eventos. Tampoco se correlacionó cada error con una petición externa única. No atribuirlo automáticamente a Celo, SKALE, un cliente o una wallet concreta. Un conteo inicial de la subcadena `429` coincidía con dígitos de saldos: se descartó; **no hay evidencia de rate limiting HTTP 429 en ese conteo**.

Acción operacional de diagnóstico: correlacionar `correlation_id`/request ID, red, signer, nonce, hash y categoría; consultar saldos y nonces `latest`/`pending` en el RPC de esa red; identificar qué ocupa la cola y su antigüedad. Resolver la causa de la cola antes de financiar a ciegas o incrementar reintentos. Reemplazar/cancelar transacciones y financiar wallets serían acciones separadas que deben revisarse con los hashes/importes concretos.

## 5. Costo: facturado, modelado y pendiente de atribuir

**Sí hay acceso a AWS y se usó facturación real.** Las etiquetas actuales no permiten afirmar un total mensual exacto del facilitador: ECS no propaga tags y varios cargos compartidos carecen de atribución. No se asignó al facilitador todo el gasto regional de la cuenta.

### Cargos identificados mediante Cost Explorer

Filtro: valores activos de la etiqueta `Name` que empiezan por `facilitator`. Se excluyeron otros nombres, incluyendo `em-production-payshell-facilitator` y Zama.

| Partida atribuida | Agosto, USD | Septiembre 1–10 parcial, USD |
|---|---:|---:|
| NAT: horas | 33.48 | 9.32 |
| NAT: procesamiento de datos | 8.75 | 6.04 |
| ALB: horas | 16.74 | 4.66 |
| ALB: LCUs | 0.19 | 0.06 |
| Endpoint VPC: horas etiquetadas | 7.44 | 2.08 |
| Resto etiquetado: transferencia, DynamoDB, logs, ECR, etc. | 6.14 | 3.00 |
| **Subtotal atribuible por tags** | **72.74** | **25.15** |

Son subtotales reales, **sin Fargate**. La cifra de septiembre es provisional; filas redondeadas pueden no sumar exactamente. Los tags tampoco demuestran por sí solos que todos los consumidores de un recurso compartido pertenezcan al facilitador.

### Cómputo mensual al tamaño actual

Price List de AWS, Linux on-demand en Ohio, consultado durante la auditoría:

- x86: USD 0.04048 por vCPU-hora + USD 0.004445 por GiB-hora.
- ARM: USD 0.03238 por vCPU-hora + USD 0.00356 por GiB-hora.
- Fórmula: tareas × horas × (vCPU × tarifa CPU + GiB × tarifa memoria).

Con un mes normalizado de 730 horas, **3 tareas actuales cuestan aproximadamente USD 108.12/mes de cómputo**. Con el promedio observado de 2.862 tareas, el ritmo equivale a USD 103.14/mes. Ambos son modelos de capacidad; no una factura ECS atribuida ni una proyección de agosto. Agosto tuvo 744 horas y un historial de configuración distinto.

Para orientar presupuesto: USD 108.12 de cómputo actual + USD 71.38 de infraestructura etiquetada normalizada desde agosto = **USD 179.50/mes de componentes identificados/modelados**. Extrapolar el subtotal etiquetado parcial de septiembre usando las aproximadamente 207 horas ya facturadas daría unos USD 88.70/mes de esa infraestructura, y unos USD 196.82 con el mismo cómputo. El tráfico y el rezago de facturación hacen esta segunda extrapolación menos estable. **USD 180–197 es una referencia parcial, no el total mensual completo ni un límite superior.**

Falta conciliar cargos de observabilidad compartida, seguridad/GuardDuty, IPv4, otros recursos sin tags, descuentos/compromisos, créditos e impuestos según aplique. El gas on-chain tampoco está en AWS. No se incorpora el gasto CloudWatch regional completo como si fuera exclusivo del facilitador.

### Opciones comparables de ahorro en cómputo

| Escenario, 730 h | USD/mes | Ahorro frente a 3 tareas actuales | Condición |
|---|---:|---:|---|
| Actual: x86, 3 × 1 vCPU / 2 GiB | 108.12 | — | Referencia |
| x86, 2 × 1 vCPU / 2 GiB | 72.08 | 36.04, 33% | Validar capacidad y failover con 2 tareas; recalibrar autoscaling |
| x86, 3 × 0.5 vCPU / 1 GiB | 54.06 | 54.06, 50% | Prueba de carga y memoria/CPU en picos; no asumir que baja media basta |
| ARM, 3 × 1 vCPU / 2 GiB | 86.51 | 21.62, 20% | Imagen ARM correcta, compatibilidad y benchmark |

Son alternativas, no ahorros que puedan sumarse. Volver a una sola réplica sacrificaría disponibilidad y no se recomienda como primer ahorro. No contratar Savings Plans ni adoptar Spot para el escritor crítico antes de fijar el tamaño estable y probar interrupciones. Las cifras siguen el modelo de [precio de Fargate](https://aws.amazon.com/fargate/pricing/).

**Corrección de un consejo histórico:** 1 vCPU con 1 GiB no es una combinación válida de Fargate. Para 1 vCPU el mínimo es 2 GiB; 0.5 vCPU sí admite 1 GiB. Reducir solamente memoria como sugería el documento de agosto produciría una task definition inválida. Ver [combinaciones oficiales de CPU/memoria](https://docs.aws.amazon.com/AmazonECS/latest/developerguide/task-cpu-memory-error.html).

## 6. Hallazgos de arquitectura y cambios propuestos

### A1 — P0: error de capacidad de gas presentado como RPC indisponible

**Evidencia:** `is_upstream_rpc_failure` en [handlers.rs](../../src/handlers.rs) trata los códigos `-32000`, `-32603`, `-32801` como infraestructura salvo revert explícito. El error de fondos de las muestras contiene `-32000`; escrow devuelve 502 y consejo de reintento.

**Cambio:** clasificación tipada por etapa y motivo: payload/revert, transporte, rate limit real, nonce/mempool, financiación del signer, broadcast incierto y receipt pendiente. Fondos del facilitador no deben convertirse en “payload malo”; tampoco en un reintento rápido sin posibilidad de progreso. Añadir backoff acotado, jitter y señal de recuperación por red/signer. Medir margen utilizable, gasto pendiente y edad de la cola, no solo saldo total.

**Aceptación:** fixtures de los errores reales sanitizados, ausencia de doble broadcast, clasificación específica para gas y cero reintentos agresivos mientras no cambie la condición. Investigar `/feedback` y `/register` por separado; sus 500 no tienen causa confirmada en esta auditoría.

### A2 — P0: el writer lease pierde exclusividad cuando falla DynamoDB

**Evidencia:** [writer_lease.rs](../../src/writer_lease.rs), `spawn`: ante `Err`, cada tarea pone `IS_WRITER=true`. Es una decisión explícita del código; un error del plano de control puede habilitar varios escritores con el mismo signer. Los nonces en memoria y su resincronización no prueban exclusividad distribuida.

**Cambio:** definir un contrato de lease con expiración conservadora, generación/fencing y handover. Mantener escritor solo mientras exista una concesión demostrablemente válida; al perderla, bloquear nuevas firmas y responder temporalmente indisponible. Reconciliar broadcasts en curso antes del relevo. Un fencing token en DynamoDB por sí solo no cerca una firma on-chain: la ruta de firma debe respetar la propiedad y sus plazos.

**Aceptación:** fallos DynamoDB, partición, pausa larga del proceso, tareas superpuestas durante deploy y pérdida de renovación; nunca dos emisores autorizados para el mismo `(chain, signer)`. Evitar cambiar a fail-closed sin probar disponibilidad/recuperación.

### A3 — P0: catálogo S3 puede perder datos en errores de lectura y escrituras concurrentes

**Evidencia:** [discovery_store.rs](../../src/discovery_store.rs), `S3Store::save/delete`, usa `load_all().await.unwrap_or_default()` y luego sobrescribe el objeto completo. Un fallo de lectura puede transformarse en catálogo vacío/parcial. El patrón read-modify-write no usa comparación de versión; varios procesos pueden pisarse.

**Cambio inicial:** propagar errores de lectura; distinguir objeto inicialmente inexistente de lectura fallida; rechazar publicación sobre una base desconocida. Añadir serialización efectiva entre escritores y control de versión/ETag con retry sobre conflictos. Versioning de S3 ayuda a recuperar, pero no sustituye control de concurrencia.

**Evolución:** registro por recurso con actualizaciones condicionales en DynamoDB ya disponible, más snapshot S3 de lectura, solo si el volumen y costo lo justifican. Alternativa inicial más pequeña: propietario único del snapshot, journal de cambios duradero y flush agrupado. Un mutex local no basta para 3 tareas.

**Aceptación:** lectura fallida no escribe; dos altas concurrentes sobreviven; delete no resucita tras flush tardío; merge no retrocede versiones; copia/versionado comprobados antes de migrar.

### A4 — P1: discovery ejecuta trabajo equivalente por réplica

**Evidencia:** [main.rs](../../src/main.rs) arranca agregación y health por proceso con defaults activos; no hay propietario distribuido de estos jobs. El crawler de semillas está apagado por defecto y no debe contarse como carga activa confirmada. El lease de pagos no cubre discovery.

**Cambio:** una concesión de trabajo separada o particiones estables por recurso; deduplicación de refrescos, límite global y por host, jitter y presupuesto de bytes/requests. Mantener API de lectura disponible en todas las tareas. Comenzar con las tareas existentes; no añadir un servicio persistente solo para implementar un loop.

**Beneficio:** menos fetches, probes y escrituras repetidas; potencial menor gasto NAT y mayor frescura por unidad de trabajo. Tres réplicas no implican un ahorro garantizado de 67%: hay que medir qué fracción del tráfico realmente se duplica. La subida de NAT por bytes es una señal para instrumentar, no prueba causal por sí sola.

### A5 — P1: forwarding reconstruye el cliente HTTP por petición

**Evidencia:** `forward_to_writer` en [handlers.rs](../../src/handlers.rs) crea un `reqwest::Client` nuevo en cada llamada; pierde el pool entre peticiones al escritor.

**Cambio:** cliente compartido en estado/servicio de forwarding; conservar timeout, límite de body, status, headers, protección de loops y comportamiento de failover. No reintentar automáticamente un POST cuyo broadcast sea incierto. Medir conexiones abiertas, tiempo de conexión y latencia del salto.

**Aceptación:** pruebas con dos servidores locales y relevo de escritor; cuerpo/status/headers equivalentes, pooling reutilizado, timeout y destino inválido controlados. La ganancia no elimina el tiempo de confirmación de la blockchain.

### A6 — P1: el escalado no representa capacidad útil de escritura

**Evidencia:** [main.tf](../../terraform/environments/production/main.tf) usa la métrica del target group de lecturas, objetivo 15; se observaron subidas y bajadas entre 2 y 3 con CPU baja. Ambos target groups apuntan a las mismas tareas. Un escritor EVM global limita el paralelismo efectivo de esa ruta.

**Cambio:** conservar el mínimo 2 inicialmente. Medir lecturas, validaciones, forwarding, espera de RPC/receipt y cola por separado. Recalibrar el objetivo con la capacidad por tarea obtenida del benchmark, con margen de seguridad y cooldown explícito. No sustituirlo simplemente por CPU: el servicio espera I/O.

**Futuro condicionado:** ownership por `(chain, signer)` o separación de roles read/write/background dentro del mismo binario si las métricas justifican el costo y la complejidad. Dos target groups ya separan métricas, pero no aíslan recursos ni procesos.

### A7 — P1: costo incompleto por falta de atribución y telemetría ruidosa

**Evidencia:** ECS no propaga tags; [telemetry.rs](../../src/telemetry.rs) nombra spans con URI completa, y emite un log por request; 26,360 eventos health/día en la ventana consultada.

**Cambio:** tags uniformes de proyecto/entorno/cost center en servicio, task definition y recursos; habilitar propagación y ECS managed tags en un rollout controlado, y confirmar activación de tags para facturación. No atribuye automáticamente el pasado. Inventariar cargos compartidos restantes antes de fijar el presupuesto total.

En telemetría usar plantilla de ruta para nombres/métricas; conservar IDs de correlación en campos apropiados, limpiar queries y reducir/sampling de éxitos triviales. Mantener errores, auditoría de pagos y señales de disponibilidad. Calcular ahorro real antes de desactivar Container Insights; la observabilidad hoy permite detectar el problema.

### A8 — P2: NAT único es un compromiso de disponibilidad ya existente

**Evidencia:** un NAT para el VPC, aunque hay tareas en dos AZ. S3 y DynamoDB ya tienen endpoints Gateway. Secrets Manager usa una interfaz en una AZ; existe endpoint administrado de GuardDuty en dos.

**Cambio:** medir flujo/bytes por origen y reducir duplicación antes de tocar topología. Añadir endpoints de pago solo con punto de equilibrio calculado; no eliminar GuardDuty para presentar ahorro. Documentar que dos AZ de ECS no garantizan egreso multi-AZ. Un NAT por AZ aumenta costo fijo; un NAT instance introduce administración y otro perfil de fallos. No se recomienda ninguno sin objetivo de disponibilidad y comparación completa.

### A9 — P2: build reproducible, ARM y arranque

**Evidencia:** [Dockerfile](../../Dockerfile) ya separa la compilación de dependencias, copia toolchain temprano, evita stubs y ejecuta como usuario no root. Sus dos `cargo build` no usan `--locked`; ambas etapas usan `$BUILDPLATFORM`. [provider_cache.rs](../../src/provider_cache.rs) inicializa proveedores secuencialmente.

**Cambio:** `--locked` en ambos builds; mantener cache de dependencias. Para ARM, build nativo o cross-compilación real con imagen/target coherentes; cambiar solo `runtimePlatform` de ECS no basta. Probar dependencias criptográficas/SDKs, startup y todas las redes habilitadas. Paralelizar arranque solo con concurrencia limitada, timeouts y semántica clara de readiness/redes disponibles.

**Opcional medido:** LTO/`codegen-units`/strip y reducción de features según uso. No prometer rendimiento por tamaño del binario ni añadir una compilación más lenta sin medir el ahorro. El lockfile, toolchain y matriz CI actual deben seguir siendo fuente de verdad.

### A10 — P2: mantener monolito modular y separar responsabilidades internas

**Evidencia:** `handlers.rs` reúne routing, errores, adaptación de esquemas, forwarding y numerosas pruebas. Ya existen módulos de chain, discovery y dx402; proveedores compartidos con `Arc` y clientes RPC reutilizables son una buena base.

**Cambio:** extraer pipeline de pago por etapas y módulos de API sin modificar wire contracts. Preservar idempotencia, propiedad de nonce, validación antes de firma y evidencia después de éxito. El mutex del nonce no se mantiene durante toda la confirmación en el código revisado: no reabrir como hecho un diagnóstico histórico ya resuelto. Hacer PRs pequeños con equivalencia observable.

## 7. Secuencia ejecutable y criterios de aprobación técnica

| Orden | Entrega concreta | Validación / salida |
|---|---|---|
| 0 | Dashboard temporal y correlación de la incidencia de gas | Red/signer/cola identificados; separar llamadas externas, hops y pagos únicos; clasificar también feedback/register |
| 1 | Corrección de errores + retry/backpressure por red | Fixtures reales; no doble broadcast; corte de reintentos sin progreso; recuperación comprobada |
| 2 | Lease seguro y persistencia S3 sin pérdida | Fault injection y concurrencia multi-proceso; snapshot recuperable |
| 3 | Cliente compartido y ownership de discovery | Menos conexiones/fetches por trabajo útil; misma semántica HTTP; frescura sin regresión |
| 4 | Atribución AWS + presupuesto conciliado | Tags visibles en nuevas tareas; CE/CUR concilia cómputo y compartidos; separar gas |
| 5 | Benchmark de capacidad | Mismos payloads/redes/mix, caché fría/caliente, p50/p95/p99 y errores por ruta; memoria y CPU de picos |
| 6 | Elegir un ajuste de costo: autoscaling, tamaño o ARM | Cambiar una variable; canary y rollback ensayados; verificar costo y latencia |
| 7 | Precios: importación → observación → refresco → quotes/dx402 | Cumplir matriz y fases del handoff específico |

**Benchmark propuesto:** usar RPCs simulados para capacidad CPU/pooling y staging/testnet para semántica on-chain. Escalar tráfico desde el perfil real hasta al menos 2× el pico observado compatible con límites de terceros; no usar un load test irrestricto contra producción ni efectuar pagos de mainnet como prueba. Medir tiempo antes del broadcast separado de tiempo de receipt, latencia por red, profundidad de cola y event-loop lag. Probar relevo con 2 tareas y pérdida de una tarea/AZ según el escenario.

**Puertas sugeridas, aún no resultados:** sin regresión de p95/p99 mayor al 10% frente al control comparable; ninguna duplicación de nonce/broadcast atribuible al cambio; errores esperados y fallos de infraestructura separados; memoria de pico menor al 70% y CPU de pico sostenido menor al 60% del tamaño elegido. Si estos límites no describen el workload, redefinirlos antes de ejecutar, no después de ver resultados.

**Rollout:** CI canónica y suite Rust/SDKs de la matriz vigente, imagen identificable, canary con observación suficiente y rollback a task definition/imagen anterior. Evaluar 24–48 horas de operación y una ventana de tráfico pico; confirmar facturación tras su rezago. No reducir réplicas, memoria y cambiar arquitectura en el mismo deploy. El push a `main` puede disparar despliegue; estos cambios permanecen locales.

## 8. UI implementada en esta entrega

Se reorganizaron `/dx402`, `/x402`, `/erc8004`, `/mcp`, `/integrar`, `/networks` y `/bazaar`, con CSS compartido y versiones EN/ES. La portada conserva su diseño actual.

- Apertura con propósito claro, acción principal/secundaria y flujo de tres pasos.
- Navegación interna por secciones; detalles avanzados desplegables que se abren al seguir un enlace directo.
- `/dx402`: guía por rol, explicación de recuperación, API antes de criptografía; configuración, contadores y signer dentro de detalles del servicio. Copy de recuperación de claves condicionado al flujo compatible; metadata social coherente.
- `/mcp`: conectar el cliente antes de la referencia detallada. Integración y redes con jerarquía común.
- `/bazaar`: búsqueda/listings antes de tablas de salud/fuentes, detalles de precios por opción conservados, controles y estados vacíos traducidos; modal usable en móvil, cards accesibles con Enter/Espacio y paginación que vuelve al catálogo.
- Sin framework/dependencias/assets externos nuevos. Hoja `/uv.css` compartida con versión de caché para estas páginas; fuentes locales. Menú y footer compartidos conservados.

La UI reorganizada **no implementa todavía el nuevo modelo de precios**: el formatter legado y la importación requieren el P0 del handoff comercial. Una mejora visual no debe confundirse con sincronización de Bazaar ya resuelta.

### Verificación efectuada

1. JavaScript inline de las siete páginas: `node --check`; IDs previos preservados, sin duplicados ni anchors locales rotos.
2. 19 pruebas existentes de `i18n_tests`, `sistema_visual_tests` y `landing_mcp_tests`: aprobadas. Se extrajeron sin modificar sus cuerpos de prueba finales y se compilaron con `rustc --test` contra los HTML/CSS reales; no se compiló toda la aplicación ni los SDKs para este cambio de presentación. La suite de presentación se ajustó para aceptar clases del body, versión de la hoja y agrupaciones con borde.
3. `python -X utf8 scripts/verify_landing_canonical.py --offline`: aprobado. El modo offline verifica referencias locales y traducciones; no valida contra `/supported` en vivo.
4. Chromium: siete páginas × escritorio 1440 px / móvil 390 px × EN/ES; sin errores de JavaScript ni desbordamiento horizontal del documento. Revisión visual de DX402, MCP y Bazaar y pruebas funcionales de datos en vivo de lectura, anchors/desplegables, búsqueda, modal, idioma y paginación.
5. `git diff --check`: aprobado.

No se ejecutó el binario Rust completo ni un build Docker de producción durante esta entrega; el preview sirve los archivos locales y proxy de GETs públicos permitidos. Antes del despliegue corresponde ejecutar la CI normal. No se midió una reducción de latencia o gasto producida por estos cambios: esos beneficios pertenecen a las fases operacionales pendientes.

## 9. Definición de terminado para la siguiente implementación

El trabajo operacional se considera completo cuando: la incidencia de gas tiene causa y recuperación verificadas; los cambios de lease/catálogo pasan pruebas de fallos; cada refresh tiene propietario y presupuesto; costo mensual está atribuido y conciliado; el ajuste de capacidad demuestra latencia igual o menor con costo igual o menor; y los precios publicados muestran semántica, origen y frescura conforme al handoff comercial. Mantener esta lista separada del estado de la UI, que ya está implementada localmente.
