# Plan maestro: recibos del facilitador y reintentos de compra

Fecha: 2026-09-17. Estado: primera implementación en validación local; publicación y aceptación en producción pendientes.

## Objetivo y decisión de producto

Cada operación de pago debe devolver un recibo estructurado junto al resultado de la llamada, conservable por el comprador y verificable sin un dashboard. Debe identificar red, activo, importe, destinatario, solicitud, liquidación y causa de rechazo. El facilitador y los SDK Python/TypeScript compartirán el mismo contrato y los mismos vectores de prueba.

Origen: comentario de Axiom compartido por el usuario: incluir `chain`, `asset`, `amount`, `payTo`, `request hash`, `settlement id` y `refusal reason` junto a la llamada del SDK. El usuario solicitó este plan y corrigió además el alcance de Hedera: **pagos exclusivamente en USDC; HBAR solo para comisiones del patrocinador**.

Entregables separados: la retirada de HBAR se publica primero. Este documento planifica los recibos; no afirma que ya estén implementados ni que la prueba de pago demuestre entrega del servicio.

## 1. Estado comprobado en el código

Base inspeccionada: facilitador 2.34.0 (`4143a128`), Python 0.86.0 (`ae2ad1c`), TypeScript 2.94.0 (`4ffb5c8`).

| Pieza existente | Ubicación | Aprovechamiento y brecha |
| --- | --- | --- |
| Respuesta de liquidación | `src/types.rs`, `SettleResponse` y su serializador | Ya expone `success`, red, pagador, transacción, `paymentId` derivado y error. Mantener los alias existentes. Falta el recibo completo e idéntico entre SDK. |
| Idempotencia HTTP | `src/handlers.rs`, `src/idempotency_store.rs` | Ya hay clave, hash del cuerpo y caché. Auditar reserva atómica antes del envío, concurrencia, expiración y estados inciertos; la existencia de una caché no demuestra exclusión entre réplicas. |
| Recuperación Hedera | `src/chain/hedera/{mod,store}.rs` | Persiste intención y bytes cofirmados antes de enviar; recupera el mismo ID. Reutilizar esta frontera durable, no crear un segundo mecanismo competidor. |
| Prueba ERC-8004 | `src/erc8004/proof.rs`, SDK TS `ProofOfPayment` | Prueba opcional con activo, importe, participantes y datos de cadena. No convertir su disponibilidad limitada en una promesa para todas las redes. |
| Recibos DX402 | `src/dx402/{receipt,gate,handlers}.rs` | Ya hay recibos firmados y consulta por `paymentId`. Su afirmación y dominio son específicos; reutilizar validación donde aplique, sin renombrarlos como prueba universal. |
| Python | `models.py`, `client.py`, `config.py`, `hedera.py` | Los resultados no exponen todos los campos de forma uniforme. `send_idempotency_key` es opt-in y necesita `idempotency_scope`. No cambiarlo silenciosamente para clientes existentes. |
| TypeScript | `src/backend/index.ts`, `src/types/index.ts`, `src/client/X402Client.ts` | Ya conserva hashes, errores y prueba opcional en algunas rutas; completar el recorrido comprador → merchant → facilitador → comprador. |
| Evidencia operativa | `docs/reports/2026-09-16-*` y `2026-09-17-*` | Hay pagos USDC reales de Arc/Hedera y pruebas publicadas. Los informes de una campaña no sustituyen al recibo por llamada. EURC aún no tiene aceptación con pagos financiados. |

## 2. Alcance

Primera entrega funcional: `exact`, Arc mainnet/testnet (USDC/EURC; x402 v1/v2), Hedera mainnet/testnet (USDC; v2). El formato será independiente de la familia de red. Extenderlo después a las demás rutas de pago, con pruebas específicas antes de anunciar cobertura.

No habilitar nuevas monedas, escrow, `upto`, Gateway ni ERC-8004 en redes que no los admiten. El recibo no autoriza cobros, swaps, reembolsos ni una segunda firma. Los pagos reales EURC siguen pendientes por instrucción del usuario.

El repositorio también contiene soporte previo de XRP nativo. Este cambio retira HBAR de Hedera; no modifica por inferencia la política de otras redes. Un eventual catálogo global de solo stablecoins requiere su propia revisión y migración explícita.

## 3. Contrato de recibo propuesto

JSON Schema versionado, Rust/TypeScript/Python generados o contrastados con los mismos fixtures. Nombre SDK: `FacilitatorReceipt`. No presentar este nombre como una extensión oficial x402.

| Campo | Regla |
| --- | --- |
| `schemaVersion` | Entero `1`; versiones desconocidas no se interpretan como confirmación. |
| `receiptId`, `revision` | Identificador opaco estable desde la admisión; revisiones monotónicas e inmutables al evolucionar. |
| `issuer`, `issuedAt` | Identidad del facilitador y fecha UTC. No inferir emisor de datos fabricados localmente. |
| `operation` | `verify` o `settle`; un `verify` válido nunca equivale a liquidación. |
| `purchaseId` | Identificador opaco del pedido, creado antes de firmar y con alcance de comercio/servicio. No usar solo precio o pagador. |
| `network` | Identificador canónico CAIP-2; representa el `chain` pedido en el comentario. |
| `scheme`, `x402Version` | Mecanismo y versión realmente procesados. |
| `asset`, `amount`, `decimals` | Activo canónico y unidades atómicas como cadena decimal; sin floats ni conversión implícita USD/EUR. `amount` describe el principal acordado, no prueba por sí mismo que se transfirió. |
| `payTo`, `payer` | Destinatario acordado y pagador verificado. `payer` puede ser nulo antes de verificar. |
| `requestHash`, `requestHashVersion` | Compromiso de la compra, especificado en la sección siguiente. |
| `paymentRequestHash`, `authorizationId` | Correlación separada de la autorización exacta y sus requisitos. No publicar bytes firmados reutilizables. |
| `status` | `verified`, `pending`, `confirmed`, `rejected` o `unknown`, con las reglas de la sección 5. |
| `settlement` | Nulo antes del envío; después contiene `id`, `idType`, `paymentId` y evidencia de confirmación disponible. `idType`: hash EVM, ID Hedera u otra familia explícita. |
| `refusalReason` | Código estable y detalle seguro, solo si existe rechazo definitivo. Nunca usar timeout como motivo de rechazo. |
| `diagnosticCode`, `retry` | Diagnóstico técnico separado; acción `retry_same_authorization`, `poll`, `none` y demora sugerida. Un error reintentable no permite volver a firmar. |
| `proof` | Firma de la afirmación exacta del facilitador, cuando esté disponible; nunca inventada por el SDK. |

Una petición malformada puede impedir determinar activo, importe o compra: devolver error validado con los campos conocidos, sin inventar un recibo financiero completo. El esquema distinguirá ese error de un recibo emitido para una operación admitida.

Los códigos se serializan como cadenas extensibles. Un consumidor antiguo conserva códigos desconocidos y adopta un estado conservador; no debe convertir un valor nuevo en éxito ni en autorización para otro pago.

## 4. Compra, autorización y hashes

Mantener tres identidades diferentes: compra estable, autorización reutilizada y transacción de liquidación. El `paymentId` actual nace de red/transacción; no sirve como ID de compra anterior al envío y se mantiene compatible.

Definir `requestHash` sobre un descriptor versionado de compra: identidad del comercio, `purchaseId`, método HTTP, destino canónico, digest del cuerpo de compra y términos aceptados (red, esquema, activo, cantidad y destinatario). El merchant debe validar ese descriptor contra su pedido real. El facilitador atestigua el descriptor recibido; no puede afirmar que vio un cuerpo HTTP que no recibió.

Fijar canonicalización mediante JSON Canonicalization Scheme (JCS) y SHA-256 con separación de dominio `uvd-x402-purchase-v1`. Especificar concatenación en bytes, UTF-8, campos ausentes/nulos, normalización por familia y tratamiento de consulta/puerto; no normalizar de forma que dos recursos distintos produzcan la misma compra. Publicar vectores completos antes de escribir adaptadores.

`paymentRequestHash` corresponde a los requisitos y autorización enviados para liquidar, con su propia versión. No cambiar retroactivamente el hash del cuerpo que utiliza el almacén de idempotencia existente: introducir migración explícita y lectura de registros antiguos.

No incluir cookies, bearer tokens, claves, datos personales ni el cuerpo de negocio en recibos públicos. Un hash de un dato predecible tampoco lo anonimiza: usar IDs opacos, controlar acceso y evitar exponer una consulta pública de compras.

## 5. Estados y reintentos

| Estado | Evidencia | Comportamiento del agente |
| --- | --- | --- |
| `verified` | Autorización válida en `/verify`; no hay prueba de movimiento | Puede solicitar liquidación de esa autorización. |
| `pending` | Operación admitida/enviada aún sin resultado final | Consultar o reenviar exactamente la misma autorización; conservar hash/ID. |
| `confirmed` | Confirmación suficiente según la familia y evidencia concordante | Recuperar el resultado o entrega del mismo pedido; no volver a pagar. |
| `rejected` | No habrá liquidación exitosa de esta operación según evidencia definitiva | Informar causa. Una compra o autorización nueva necesita decisión explícita. |
| `unknown` | No se pudo determinar si hubo admisión/envío/confirmación | Conservar autorización, reconciliar; no interpretarlo como impago. |

Un fallo HTTP o timeout solo permite un resultado local de transporte con estado desconocido. El SDK debe marcar que no recibió un recibo del facilitador y conservar el último recibo auténtico; no firmarlo ni fabricarlo.

Transiciones de almacenamiento: `reserved → prepared → submitted → confirmed/rejected`; `unknown` puede aparecer alrededor del envío y volver a reconciliación. No crear otra autorización para resolver un estado incierto. Un evento de reorganización de cadena genera una revisión explícita; un recibo histórico no se reescribe en silencio.

Garantía a demostrar: como máximo una liquidación admitida por autorización, con resultado recuperable; como máximo un cumplimiento del pedido en el merchant. No prometer entrega HTTP exactamente una vez ni exactamente una ejecución global solo por añadir un recibo.

## 6. Facilitador: persistencia y API

1. Auditar las rutas `/verify`, `/settle`, cabeceras v1/v2, MCP, errores y compatibilidad `transaction`/`transactionHash`/`transaction_hash`. Documentar qué capas reconstruyen o pierden campos.
2. Reservar operación mediante escritura condicional antes de cualquier emisión de transacción. Misma clave + mismos términos devuelve estado/recibo original; misma clave + otra compra o cuerpo se rechaza con conflicto.
3. La clave se vincula al comercio autenticado o a un contexto firmado verificable; un `merchantId` aportado libremente no autentica un namespace. Si ese vínculo no existe, no exponer búsqueda por `purchaseId`.
4. Vincular también autorización → compra para evitar que dos claves de idempotencia diferentes intenten liquidar lo mismo. Una nueva autorización para una compra ya pagada no produce otro cargo automáticamente.
5. Persistir ID, bytes necesarios para recuperación y estado antes de enviar. Ante caída después del broadcast, recuperar esa transacción. Almacenamiento inaccesible antes de admisión: rechazo de admisión sin envío. Después de envío: estado incierto conservando referencias.
6. Emitir recibos desde los resultados reales de cada proveedor; no reconstruir un supuesto pago a partir de parámetros del comprador. Separar comprobación de términos, firma del facilitador y confirmación de cadena.
7. Proponer `GET /receipts/{receiptId}` como consulta de estado con autorización/capacidad opaca, límites y retención. No aceptar transacciones desde este endpoint. Endpoint y estrategia de autenticación se cierran en F1.
8. Retener registros al menos durante validez de autorización, finalización de red y ventana documentada de reintento. TTL no es una autorización para cobrar otra vez. Historial público y bytes firmados tienen políticas de retención diferentes; fijarlas con pruebas de expiración.
9. Las migraciones permiten leer registros anteriores y no reactivan HBAR. El rollback desactiva emisión de recibos nuevos sin borrar operaciones en curso.

## 7. Transporte, procedencia e interoperabilidad

Preservar los campos x402 existentes y transportar el objeto de forma aditiva, con negociación de capacidad. Candidato: respuesta `receipt` y metadatos de capacidad bajo un nombre propio versionado. F1 debe contrastarlo con el mecanismo de extensiones vigente antes de fijar el wire. No introducir campos en `accepted`, protobuf Hedera o autorizaciones de transferencia sin soporte expreso.

El merchant debe propagar el recibo de liquidación al comprador mediante la respuesta de pago compatible con la versión. Cubrir `PAYMENT-RESPONSE` v2, `X-PAYMENT-RESPONSE` v1, CORS `Access-Control-Expose-Headers`, proxies, cabeceras múltiples y límites de tamaño. Una prueba ampliada se consulta por referencia; el recibo esencial viaja junto al resultado.

El recibo firmado del facilitador atestigua liquidación/estado, no entrega del servicio. La extensión oficial `offer-receipt` pertenece al servidor del recurso. Mantenerla intacta y asociarla por compra/solicitud solo cuando existan comprobaciones suficientes. No reutilizar el plan `receipt-gated-release-plan.md` como si su recibo de entrega/escrow resolviera este requerimiento.

Propuesta de procedencia portable: JWS con clave dedicada del servicio, ID de clave, publicación de claves públicas y rotación; jamás reutilizar automáticamente las claves financiadas de los firmantes. Seleccionar algoritmo, custodia y verificador en F1, con pruebas de manipulación y rotación. Hasta tener esa firma, un objeto recibido por HTTPS se etiqueta como tal; no se anuncia como prueba criptográfica offline.

## 8. SDK Python y TypeScript

- Compartir esquema y fixtures; importes siempre cadenas atómicas. Evitar el nombre `amount_usd` en el nuevo recibo.
- Agregar APIs hermanas propuestas `fetch_with_receipt` / `fetchWithReceipt` que entreguen `{response, receipt, paymentState}` conservando las APIs existentes. El esquema de red permanece igual en ambos lenguajes; las convenciones de nombres de métodos pueden diferir.
- `verify`/`settle` conservan el recibo completo y campos desconocidos compatibles. Los errores incluyen último recibo, estado, IDs y política de reintento, sin perder el cuerpo HTTP original.
- Dar un contexto persistible de compra y autorización. No depender de estado de memoria de un proceso para recuperar después de un reinicio.
- Exponer verificación de firma/procedencia y consulta del recibo. La firma prueba quién emitió la afirmación; la comprobación de cadena valida su contenido financiero.
- No activar idempotencia globalmente con una clave derivada solo de precio/pago. Mantener compatibilidad del opt-in actual; las APIs nuevas requieren contexto estable para prometer recuperación de compra.
- Cubrir parsers v1/v2, Node/navegador y Python síncrono/asíncrono donde existan APIs públicas. Nunca consumir el cuerpo de la respuesta que necesita la aplicación.
- Ante facilitador antiguo o merchant que no propaga recibos: `receipt = null`, capacidad no disponible y estado explícito; no degradar silenciosamente a un recibo inventado.

## 9. Fases, entregables y salida

| Fase | Entregables | Criterio de salida |
| --- | --- | --- |
| F0 — Hedera USDC | Retirar HBAR de admisión, registros SDK, badges, docs, Swagger, MCP/agent docs y OG; conservar saldo HBAR para comisiones e historial | Ambas redes anuncian solo USDC; ofertas HBAR rechazadas antes de nueva firma/envío; consultas de recibos históricos funcionan. |
| F1 — Contrato | ADR de transporte/procedencia/autenticación, JSON Schema, estados, canonicalización, errores, matriz por familia y fixtures | Rust/Python/TS interpretan idénticamente los casos; request hash y purchase scope no ambiguos; referencia normativa fijada por commit. |
| F2 — Persistencia | Reserva atómica, referencias de autorización, recuperación y API de consulta | Pruebas con dos réplicas, reinicios y fallos alrededor del broadcast demuestran ausencia de segunda liquidación. |
| F3 — Recibo y propagación | Emisión en facilitador, prueba del emisor, adaptación merchant/cabeceras y consulta | Confirmado, rechazo e incertidumbre recuperables sin dashboard; pago confirmado + HTTP 500 de merchant no dispara nuevo pago. |
| F4 — SDK | Tipos, métodos con recibo, errores, verificación y persistencia del contexto de compra en Python/TS | Paridad de vectores, ninguna firma automática nueva al reintentar, compatibilidad de APIs existentes. |
| F5 — Validación por red | Arc USDC/EURC y Hedera USDC con adaptadores de evidencia propios | Matriz offline/negativa completa; pruebas reales donde estén autorizadas y financiadas, sin confundirlas con simulaciones. |
| F6 — Publicación | Facilitador, PyPI/npm, documentación, Swagger, MCP, ejemplos y release notes | Instalaciones limpias verificadas, consulta pública/controlada funcional, evidencias y versiones vinculadas. |
| F7 — Resto de redes | Integrar familia por familia y esquemas existentes | Cada combinación anunciada tiene pruebas propias; no anunciar cobertura automática por compartir tipo JSON. |

Dependencias: F0 es independiente. F1 precede F2/F3/F4; F2 precede cualquier garantía de reintento seguro. F5 precede las afirmaciones de producción en F6. La implementación prioriza facilitador y luego ambos SDK, según la preferencia del usuario.

## 10. Matriz mínima de pruebas

| Caso | Resultado exigido |
| --- | --- |
| Arc 2 redes × USDC/EURC × v1/v2 × Python/TS | Activo y dominio correctos, seis decimales, request hash común y recibo concordante. EURC real permanece pendiente. |
| Hedera 2 redes × USDC × v2 × Python/TS | Token propio del ledger, ID nativo preservado, importe exacto y HBAR solo como comisión. |
| HBAR `0.0.0`, nombre `hbar`, HTS arbitrario y USDC de otro ledger | Rechazo antes de firmar/admitir nuevo pago; no gasto de presupuesto del patrocinador. |
| Misma compra/autorización, reintento normal y concurrente en dos réplicas | Una transacción, mismo receiptId/paymentId, ningún segundo crédito al destinatario. |
| Misma clave con cambio de precio, activo, destinatario, cuerpo o compra | Conflicto; ninguna liquidación nueva. |
| Misma autorización con otra clave; misma compra con firma nueva | Reconocer la operación existente; no cobrar de nuevo automáticamente. |
| Timeout antes/durante/después de envío; pérdida de respuesta; fallo durable | Estado correcto y recuperación del mismo ID/bytes; no falso rechazo ni éxito. |
| Expiración de autorización/TTL/reinicio; reorganización EVM | No renovar firma automáticamente; reconciliación y revisión con evidencia. |
| Pago confirmado pero merchant devuelve 500 o pierde respuesta | Separar pago de entrega; reintentar cumplimiento por purchaseId, sin cobrar otra vez. |
| Firma de recibo alterada, emisor inesperado, clave rotada o esquema desconocido | Fallo verificable y conservador; nunca éxito por omisión de verificación. |
| Facilitador antiguo, header ausente/duplicado/truncado, CORS, streaming | No fabricar recibo ni consumir indebidamente respuesta; compatibilidad documentada. |
| Consulta sin autorización y cruce de comercios | No filtrar datos de compra ni permitir colisiones entre namespaces. |
| Reintento de HBAR confirmado antes de F0 | Devolver referencia histórica, sin nueva firma ni nueva reserva. |

Las pruebas reales guardarán versión, red, activo, importe, destinatario, IDs, recibo emitido, consulta independiente y cambio de saldo. Excluir claves y autorizaciones reutilizables. Mantener separados `offline`, `verify-only`, `eth_call` y `settled-on-chain`.

## 11. Publicación y operación

Añadir guías de recibos y reintentos, ejemplos para ambos SDK, esquemas OpenAPI, referencias MCP, `skill.md`, `llms.txt` y `llms-full.txt`; regenerar digests. La landing debe anunciar capacidades publicadas, no trabajo planificado. Los badges representan monedas aceptadas; el balance de gas se identifica por separado.

Métricas: pendientes por antigüedad, latencia hasta confirmación, recuperaciones, conflictos de idempotencia, fallos de almacenamiento y recibos no propagados. No usar purchaseId, direcciones o hashes individuales como etiquetas de métricas. Definir alertas/runbook con umbrales medidos antes de escalar volumen.

Despliegue gradual por capacidad, empezando por testnet. Validar `/supported`, `/version`, Swagger, una compra y su recuperación, consulta de recibo e instalaciones limpias. No anunciar EURC como probado con liquidaciones reales hasta contar con evidencia financiada. Conservar las restricciones de cuotas Hedera vigentes salvo cambio deliberado posterior.

## 12. Checklist y registro

- [x] Revisar respuesta del facilitador, idempotencia y resultados de ambos SDK.
- [x] Distinguir recibo de liquidación, recibo de entrega y prueba DX402/ERC-8004.
- [x] Registrar decisión USDC-only para Hedera y preservar pagos históricos.
- [x] Escribir este plan maestro y matriz de aceptación.
- [x] Publicar/verificar F0: facilitador 2.35.0, Python 0.87.0 y TypeScript 2.95.0. [Evidencia](../reports/2026-09-17-hedera-usdc-only-release.json).
- [ ] Completar F1: ADR, esquema, contrato de transporte y vectores.
- [ ] Completar F2–F3: persistencia, emisión, consulta y propagación.
- [ ] Completar F4: APIs y paridad de ambos SDK.
- [ ] Completar F5–F6: aceptación por red, publicación y evidencia.
- [ ] Completar F7 antes de anunciar recibos en todas las redes/esquemas.
- [ ] Pagos reales EURC: pendientes por instrucción del usuario; sin ejecución automática.

## Referencias

Referencias upstream contrastadas el 2026-09-17; commit `c8c71f244c0d45a6a4fd990a96c69aa34781cd05`. Volver a contrastarlas al cerrar F1, sin asumir que un issue es estándar aprobado.

- [x402 v2: respuestas, errores y extensiones](https://github.com/x402-foundation/x402/blob/c8c71f244c0d45a6a4fd990a96c69aa34781cd05/specs/x402-specification-v2.md).
- [Offer and Receipt Extension: artefactos del servidor del recurso](https://github.com/x402-foundation/x402/blob/c8c71f244c0d45a6a4fd990a96c69aa34781cd05/specs/extensions/extension-offer-and-receipt.md).
- [Guía Hedera y política vigente](../guides/hedera-native.md), [Arc](../networks/arc.md).
- [Plan previo de recibos de entrega/escrow](receipt-gated-release-plan.md); objetivo distinto, no se sustituye.
- [Evidencia histórica Hedera](../reports/2026-09-16-hedera-transaction-ledger.md).

## Avance de implementación — 2026-09-17

El contrato definitivo de esta primera entrega está en [facilitator-receipts.md](../facilitator-receipts.md).
Ese documento prevalece sobre las propuestas de diseño anteriores: dominio
`uvd-x402-request-v1`, canonicalización restringida a claves ASCII/números seguros,
alcance de compra por capacidad secreta, JWS Ed25519 y consulta privada Bearer.

- F0 publicado: Hedera USDC exclusivo en facilitador y ambos SDK.
- F1 implementado: esquema público y seis vectores firmados compartidos Rust/Python/TS.
- F2 implementado: reserva transaccional DynamoDB sin TTL, CAS y persistencia previa al envío; validación local de concurrencia, reinicio y fallos.
- F3 implementado: respuesta aditiva, JWS, consulta privada, propagación FastAPI/Express/Hono y helpers para otros frameworks.
- F4 implementado: contexto persistible y fetch con recibo en ambos SDK, verificación de firma y conservación de errores.
- F5 parcial: vectores/pruebas offline; aceptación USDC con estas versiones aún pendiente. EURC real diferido por el usuario.
- F6 pendiente: preflight completo, publicación y aceptación en producción.
- F7 pendiente: demás redes/esquemas; no se anuncia cobertura global.

Limitaciones conservadoras: las reservas abandonadas antes de preparar transacción
requieren investigación; no hay monitor continuo de reorgs ni archivo automático
de recibos. La rotación requiere conservar claves públicas anteriores. El recibo
no implementa entrega exactamente una vez del merchant. El comprador Python
usa la API síncrona existente; la propagación FastAPI sí es asíncrona.
