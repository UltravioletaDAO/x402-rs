# Handoff: precios dinámicos y sincronización de Bazaar, agregadores y dx402

**Fecha:** 2026-09-09.  
**Para:** mantenimiento del facilitador `x402-rs`, discovery/Bazaar y SDKs comprador/vendedor.  
**Base revisada:** rama local `main`, commit `9893c48f`.  
**Estado:** investigación y plan de implementación; ninguna corrección del runtime de precios aplicada por este handoff. La auditoría posterior confirmó la imagen desplegada `2.16.0-9893c48`, consistente con la base revisada. No se midió una tasa de discrepancias de precios en producción. La reorganización local de la UI y la auditoría AWS se documentan en el [handoff de arquitectura, costos y UI](2026-09-09-auditoria-arquitectura-costos-y-ui.md).

El operador pidió investigar cómo evitar que un agente encuentre un precio en un bazar y otro al llamar al endpoint, tomando como referencia el refresco de Context7. Después solicitó este documento dentro del repositorio del facilitador.

## 1. Resultado buscado

El catálogo debe describir correctamente las condiciones comerciales y su antigüedad. El comprador debe evaluar las condiciones de su solicitud concreta antes de firmar. El vendedor debe poder mantener una cotización durante una vigencia explícita. dx402 puede conservar la evidencia de lo anunciado, aceptado, liquidado y entregado.

La actualización del índice y la protección de una compra son problemas relacionados, con mecanismos distintos. Un índice actualizado no elimina el cambio de precio entre cotización y pago. Una cotización firmada tampoco actualiza las copias que mantienen terceros.

Orden recomendado:

1. Corregir pérdida de esquemas, extensiones, unidades y significado en importación/UI.
2. Separar fechas de actividad, importación, cambio de contenido y verificación de precio.
3. Completar almacenamiento y lectura de condiciones observadas en el endpoint.
4. Incorporar refresco por demanda, volatilidad y notificación del proveedor.
5. Integrar cotizaciones vigentes en vendedor/comprador y evidencia complementaria en dx402.

## 2. Hallazgos confirmados en el código local

Las líneas son orientativas para el commit indicado; localizar también por símbolo. Los efectos potenciales descritos son inferencias del código, no incidentes de producción reproducidos.

| ID | Evidencia | Consecuencia y corrección |
| --- | --- | --- |
| F1 | [discovery_aggregator.rs](../../src/discovery_aggregator.rs), `convert_payment_requirement`, líneas 854–885: fuerza `Scheme::Exact` y `extra: None`; el DTO `CoinbasePaymentRequirement` sí recibe `scheme`, pero no conserva `extra`. | Una oferta `upto` u otro esquema puede publicarse como `exact`; se pierden parámetros necesarios para interpretar/pagar. Preservar esquema y datos adicionales desde el DTO hasta la API. |
| F2 | Misma función, líneas 871–875: importe ausente o no parseable acaba en cero. | Datos inválidos pueden convertirse en una oferta aparentemente gratuita, aunque filtros posteriores puedan excluirla. Validar en el límite de entrada y registrar la causa; distinguir cero explícito de error de parseo. |
| F3 | [bazaar.html](../../static/bazaar.html), `fmtPrice`, línea 195: usa `accepts[0]`, `Number`, seis decimales y prefijo `$`. | Ignora alternativas y esquema; puede representar otra moneda como dólares y perder precisión. Formatear según activo/red y semántica, sin convertir importes grandes a punto flotante. |
| F4 | [discovery_health.rs](../../src/discovery_health.rs), `HealthRecord`, `LiveTerms`, `pay_to_from_402` y `start_health_task`: conserva estado de salud y destinatarios, pero no los requisitos completos observados. | Un endpoint puede estar marcado como vivo y seguir mostrando un precio viejo. Capturar observaciones de términos y su contexto por separado de salud. |
| F5 | Mismo archivo, `HEALTHY_REPROBE_SECS`, línea 39: siete días para recursos saludables. [main.rs](../../src/main.rs), línea 270: agregación por defecto cada 3.600 segundos. | Son frecuencias de código/configuración por defecto, no SLA verificado. El tick de salud de 60 segundos no significa verificar cada recurso cada minuto. Separar cadencia de salud de cadencia de precio. |
| F6 | [discovery_aggregator.rs](../../src/discovery_aggregator.rs), `convert_single_resource`, línea 825: usa `now` si falta fecha de la fuente. [discovery.rs](../../src/discovery.rs), `bulk_import`, línea 771: decide reemplazo por `last_updated` más reciente. | Descargar otra vez contenido viejo puede darle apariencia de novedad y permitir que gane un merge. Conservar fecha desconocida y comparar contenido/revisión con procedencia. |
| F7 | [discovery.rs](../../src/discovery.rs), `track_settlement`, línea 1025: incrementa el contador del registro existente sin reemplazar sus términos. [types_v2.rs](../../src/types_v2.rs), `increment_settlement_count`, línea 1373: además cambia `last_updated`. | Actividad comercial puede rejuvenecer metadatos antiguos e interferir con la prioridad de importaciones. Crear `lastSettledAt`; no usarlo para afirmar vigencia del precio. |
| F8 | [03-health-checker.md](../plans/bazaar/03-health-checker.md), secciones 4–5: ya propone `probed_accepts` y `probed_accepts_at`. Esos campos no están en el comprobador actual. | Hay diseño previo aprovechable, pero no debe darse por implementado. Completarlo añadiendo contexto de solicitud y reglas de procedencia; una fecha mayor por sí sola no decide autoridad. |

Conservar las protecciones existentes al realizar los cambios: validación de importaciones, restricciones SSRF, control de timestamps futuros, preservación de procedencia, separación de overlays y límites de solicitudes por host.

## 3. Qué se puede tomar de Context7 y de otros catálogos

Context7 revisa la antigüedad cuando una librería es solicitada; si supera un umbral según popularidad, sirve el contenido existente y dispara un refresco en segundo plano. Los umbrales documentados son 1/15/30/45 días para top 100/1.000/5.000/resto. También documenta un refresco desde GitHub Actions cuando se actualiza el repositorio. No ofrece sincronización instantánea mediante ese mecanismo. [Política de actualización](https://context7.com/docs/library-updates), [GitHub Actions](https://context7.com/docs/integrations/github-actions).

Aplicación propuesta: lectura rápida del catálogo con antigüedad explícita, refresco por demanda y notificaciones del propietario. Para precios, sumar volatilidad y criticidad de compra a la popularidad; no trasladar los intervalos de documentación directamente.

CDP documenta indexación asíncrona y estados de aceptación de metadatos en `EXTENSION-RESPONSES`. Su cliente `awal` documenta además una caché local de 12 horas. Son capas independientes: refrescar nuestra fuente no implica que todos los agentes ya vean la actualización. No suponer que existe una API universal de actualización de bazares. [Publicación en CDP Bazaar](https://docs.cdp.coinbase.com/x402/seller/get-discovered), [caché de awal](https://docs.cdp.coinbase.com/agentic-wallet/cli/skills/search-for-service).

x402scan documenta OpenAPI con `x-payment-info.price` en modo `fixed` o `dynamic` con `min/max`, y da prioridad al comportamiento `402` del endpoint sobre metadatos estáticos. Es una convención de su discovery, no un campo universal de `PaymentRequirements`. [Discovery de x402scan](https://github.com/Merit-Systems/x402scan/blob/main/docs/DISCOVERY.md).

Estas fuentes se consultaron durante la investigación del 2026-09-09. Revalidar su contrato antes de implementar adaptadores externos: sus documentos y APIs pueden evolucionar.

## 4. Invariantes de precio y comparación

| Concepto | Significado | Regla de presentación/comparación |
| --- | --- | --- |
| `exact` | Importe exacto aceptado para una solicitud. | No implica que todas las solicitudes futuras tengan ese importe. Comparar el mismo contexto y opción de pago. |
| Rango declarado `min/max` | Límites publicados para un alcance definido. | Identificar moneda, unidad, alcance y si son límites comprometidos o indicativos. No deducirlos del orden de `accepts`. |
| Rango observado | Mínimo/máximo de muestras en una ventana temporal. | Etiquetarlo como histórico, con ventana y cantidad de muestras. No prometer que el mínimo sigue disponible. |
| `upto` | Techo que autoriza el cliente, con liquidación efectiva hasta ese techo. | Mostrar techo autorizado y cobro efectivo por separado. No reemplazar el techo del catálogo con el último cobro. |
| Precio estimado | Cálculo aproximado bajo supuestos de consumo. | Mostrar supuestos/unidad y pedir condiciones concretas antes de firmar. |
| Esquema desconocido | Semántica que este componente no implementa. | Conservar procedencia/datos donde el contrato lo permita y marcarlo no compatible; nunca convertirlo a `exact`. |

En `upto` EVM, `amount` en los requisitos de verificación representa el máximo y en liquidación puede representar el importe efectivo. Registrar siempre la fase de la observación. [Especificación de upto](https://github.com/x402-foundation/x402/blob/main/specs/schemes/upto/scheme_upto_evm.md).

Ejemplos de clasificación:

- Listing exacto 0,01 → cotización exacta 0,03, mismo contexto: discrepancia de términos; refrescar y reevaluar política de compra.
- Rango declarado 0,01–0,10 → cotización 0,03: compatible con el rango.
- Techo `upto` 0,10 → liquidación 0,03: compatible; no es una discrepancia de precio.
- Muestra para modelo básico → solicitud de modelo avanzado: contextos diferentes; no comparar como precio idéntico.
- Mismo importe pero distinto activo, red, destinatario o unidad: no es equivalencia comercial.

## 5. Modelo de datos propuesto

Los nombres de este apartado son una propuesta interna. No añadirlos a mensajes firmados ni presentarlos como campos estandarizados de x402.

Separar cuatro entidades lógicas; no es obligatorio crear cuatro almacenes físicos:

1. **Oferta declarada:** lo anunciado por una fuente, con documento original, identificador de fuente y alcance de la política de precios.
2. **Observación de términos:** respuesta del origen para un contexto concreto, con transporte/protocolo y fecha de observación.
3. **Cotización aceptada:** términos que el comprador evaluó y autorizó; opcionalmente oferta firmada y vinculación explícita a la solicitud.
4. **Liquidación:** autorización máxima, importe efectivo, red/activo, identificador de transacción y estado de reconciliación.

Campos mínimos a diseñar:

| Grupo | Campos propuestos | Regla |
| --- | --- | --- |
| Identidad | `resourceId`, método, ruta/plantilla, `optionId`, `requestContextId` | La URL sola puede agrupar variantes distintas. La estabilidad de `optionId` no debe depender del precio mutable ni del índice en `accepts`. |
| Procedencia | `sourceId`, `sourceKind`, `canonicalSourceUrl`, `verificationLevel` | Separar fuente agregada, declaración verificada del propietario, respuesta HTTPS observada y oferta con firma validada. |
| Fechas | `sourceUpdatedAt`, `ingestedAt`, `contentChangedAt`, `termsObservedAt`, `lastSettledAt` | Desconocido permanece desconocido. Ninguna fecha de actividad equivale a precio verificado. |
| Versión | `pricingRevision`, `contentHash`, validadores HTTP si existen | Revisiones se comparan dentro de la misma autoridad; el hash detecta cambio, no demuestra origen ni actualidad. |
| Precio | `scheme`, `network`, `asset`, importe en unidades atómicas, unidad comercial, extensiones | Conservar el original y una vista normalizada. Resolver decimales con metadatos confiables del activo/red. |
| Alcance | parámetros relevantes, variante/modelo, cantidad, modalidad de autenticación | No persistir credenciales ni cuerpos privados en el catálogo. Una huella no convierte datos sensibles en públicos. |
| Frescura | `priceFreshness`, `nextPriceCheckAt`, `observationExpiresAt` | Estado `fresh/stale/unknown/conflict` independiente de `health`. Fresco significa observado dentro de política, no precio garantizado. |
| Vigencia de oferta | `offerValidUntil`, oferta original firmada si existe | Separada del TTL del índice, de la expiración de la autorización y de `maxTimeoutSeconds`. |

Una observación de una solicitud no reemplaza todo `accepts` ni los rangos globales del servicio. Si una variante requiere autenticación que el prober no tiene, el precio queda sin verificar para esa variante; no se etiqueta como servicio muerto.

## 6. Fase P0: conservar semántica y corregir presentación

**Archivos principales:** [discovery_aggregator.rs](../../src/discovery_aggregator.rs), [types_v2.rs](../../src/types_v2.rs), [types.rs](../../src/types.rs), [discovery_crawler.rs](../../src/discovery_crawler.rs), [discovery_security.rs](../../src/discovery_security.rs), [bazaar.html](../../static/bazaar.html).

- [ ] Ampliar DTOs de entrada para conservar esquema, `extra`, extensiones de recurso y metadatos comerciales necesarios, con límites de tamaño/profundidad existentes o equivalentes.
- [ ] Normalizar v1/v2 explícitamente. El alias `maxAmountRequired` no debe interpretarse como un rango de precios ni cambiar el esquema. Conservar originales de artefactos firmados.
- [ ] Parsear `scheme`; mapear solo variantes reconocidas. Distinguir «se puede catalogar» de «nuestro facilitador/cliente puede liquidarlo», comprobando capacidades por red cuando corresponda.
- [ ] Rechazar cantidades ausentes, malformadas, negativas o fuera de rango. Tratar el cero explícito según esquema/fase/política del catálogo, sin inventarlo a partir de un error.
- [ ] Verificar todas las rutas de ingestión, incluido crawler/registro, para evitar que una ruta conserve datos que otra borra.
- [ ] Mantener importes como enteros de precisión suficiente/cadenas decimales; evitar `Number` para aritmética monetaria.
- [ ] Resolver moneda/decimales por activo y red. No asumir que seis decimales implican USD; una conversión a USD requiere su propia fuente y fecha.
- [ ] Mostrar alternativas de pago con contexto. Usar «desde» solo cuando el mínimo sea válido para opciones realmente comparables; usar «hasta» para un techo; mostrar «precio no verificado» si no hay evidencia suficiente.
- [ ] Añadir y revisar textos EN/ES existentes de la interfaz.

**Aceptación:** fixtures v1/v2 con `exact`, `upto` y otros esquemas reconocidos conservan significado al importar, persistir, leer y mostrar. Un esquema desconocido no aparece como exacto. Cantidad inválida nunca se convierte silenciosamente en cero. Dos activos con decimales distintos se muestran correctamente.

**Migración imprescindible:** registros guardados después de la conversión a `exact` han perdido información. No se puede reconstruir el esquema verdadero a partir del importe; reimportar fuentes o volver a observar el origen. Hasta entonces, marcar la calidad/semántica histórica como no verificada. La corrección del parser por sí sola no repara lo ya persistido.

## 7. Fase P1: fechas, observaciones y reglas de lectura

**Archivos principales:** [discovery.rs](../../src/discovery.rs), [discovery_store.rs](../../src/discovery_store.rs), [discovery_health.rs](../../src/discovery_health.rs), [types_v2.rs](../../src/types_v2.rs), [handlers.rs](../../src/handlers.rs).

- [ ] Separar `lastSettledAt` del timestamp de contenido. Mantener compatibilidad de `lastUpdated` con definición explícita; no usarlo para frescura económica.
- [ ] Preservar la ausencia de fecha de fuente. Comparar hash/revisión para detectar importaciones sin cambios; evitar que `ingestedAt` decida qué precio gana.
- [ ] Evolucionar `LiveTerms` para leer requisitos completos del header `PAYMENT-REQUIRED` y formatos legacy del cuerpo. Si ambos discrepan, conservar evidencia y resolver según versión de protocolo; no combinar campos creando una oferta que nadie emitió.
- [ ] Añadir un overlay de términos observados, reutilizando el diseño previo de `probed_accepts` o un módulo específico. Conservar contexto, fecha, fase y procedencia; salud sigue independiente.
- [ ] Tomar snapshots antes de adquirir el lock del registro para componer respuestas. Evitar nuevos `.await` bajo locks de recursos y escrituras concurrentes de varios componentes al mismo objeto S3.
- [ ] Preferir, para una compra y contexto compatibles, una oferta vigente validada o respuesta del origen frente a una copia agregada; no ordenar autoridades diferentes solo por el timestamp que ellas mismas publican.
- [ ] Conservar discrepancias y valores previos; una importación atrasada no borra una observación directa. Una nueva revisión comprobada del propietario sí puede volver obsoleta la observación previa y disparar revalidación.
- [ ] Mantener los cambios de destinatario en su tratamiento de identidad/seguridad. Precio cambiante no debe disparar automáticamente la misma cuarentena que un posible secuestro de `payTo`.
- [ ] Registrar settlements como actividad y evidencia de esa operación. No convertir el cobro efectivo en precio universal ni inferir todos los términos si el facilitador no los recibió.
- [ ] Versionar el formato persistido con lectura compatible de registros anteriores. No poblar fechas desconocidas con `now` al migrar. Diseñar rollback que no pierda datos nuevos al leer/escribir desde una versión vieja.

**Aceptación:** un settlement no rejuvenece el precio; reimportar un feed idéntico no simula un cambio; un feed viejo con timestamp reciente no pisa una observación válida solo por esa fecha. Reinicio y persistencia conservan contexto y procedencia. Un GET sin contexto no reemplaza el precio de un POST parametrizado.

## 8. Fase P2: refresco adaptativo y propagación

**Archivos principales:** scheduler de [discovery_health.rs](../../src/discovery_health.rs), agregador, [main.rs](../../src/main.rs), [handlers.rs](../../src/handlers.rs), [openapi.rs](../../src/openapi.rs); considerar módulos específicos de términos/frescura para no concentrar responsabilidades en salud.

- [ ] Crear una cola de revalidación con deduplicación por recurso/contexto, límites por host, prioridades y protección contra solicitudes repetidas. Coordinar trabajos si hay varias réplicas; un lock en memoria no basta para toda la flota.
- [ ] Refrescar al consultar un listing vencido sin bloquear toda la búsqueda. Si el cliente solicita términos actuales, devolver una observación adecuada o un estado explícito de pendiente/no verificable; no llamar «actual» a la caché mientras se refresca.
- [ ] Priorizar por demanda, variación observada, cambio de revisión y proximidad a una compra. Mantener presupuesto para recursos poco usados para evitar inanición.
- [ ] Respetar `Retry-After`, límites de origen, jitter y backoff. Evitar que salud y precio dupliquen la misma solicitud; reutilizar observaciones compatibles.
- [ ] Usar validadores `ETag`/`Last-Modified` cuando el origen los soporte y representen el documento correcto. Un `304` de OpenAPI no prueba una cotización dinámica por usuario.
- [ ] Probar métodos e inputs declarados por el vendedor. Para POST con efectos o autenticación, exigir una ruta de cotización/inspección segura o usar la solicitud real del comprador antes del pago; no lanzar operaciones comerciales de prueba a ciegas.
- [ ] Implementar notificación autenticada de cambio de precios para servicios propios: recurso, revisión e identificador idempotente. La notificación invalida/encola; el servidor vuelve a consultar el origen validado antes de confiar en nuevos términos.
- [ ] Para vendedores, generar discovery y challenge desde la misma política de precios. Un cambio en parámetros por solicitud no exige publicar cada cotización privada; publicar política/rangos apropiados y cotizar la solicitud individualmente.
- [ ] Crear adaptadores por bazar para los mecanismos realmente disponibles. Registrar envío, aceptación y observación posterior de la revisión. Reconsultar el catálogo externo para distinguir «solicitud aceptada» de «cambio visible».
- [ ] Publicar antigüedad y procedencia en nuestra API. Los consumidores que conserven cachés propias deben recibir documentación de revalidación y del carácter indicativo del listing.

**Cadencias iniciales propuestas, no SLA:** evaluar 1–5 minutos para un conjunto pequeño de precios muy variables y demandados, 15–60 minutos para recursos activos estables, y 6–24 horas o refresco bajo demanda para la cola larga. Antes de fijarlas, medir tamaño, capacidad, cuota por origen y latencia. Con 20.000 recursos, barrer cada minuto exigiría aproximadamente 333 solicitudes/segundo antes de reintentos; no reducir globalmente siete días a un minuto.

**Aceptación:** muchas lecturas del mismo registro vencido crean un solo trabajo por ventana; una caída/429 de origen no genera avalancha; los límites por host se cumplen; se puede medir propagación por destino. Un tercero sin API de refresco continúa con sus tiempos de indexación, sin prometer sincronización instantánea.

## 9. Fase P3: cotización, política del comprador y vigencia

**Responsabilidad compartida:** vendedor y comprador. El facilitador no está en la ruta del cuerpo de respuesta del recurso; no puede reconstruir por sí solo la solicitud comercial ni bloquear cambios del motor de precios.

**Puntos a revisar:** [x402-axum/price.rs](../../crates/x402-axum/src/price.rs), [layer.rs](../../crates/x402-axum/src/layer.rs), [x402-reqwest/middleware.rs](../../crates/x402-reqwest/src/middleware.rs), [builder.rs](../../crates/x402-reqwest/src/builder.rs), integración de ofertas/recibos disponible en las versiones de SDK seleccionadas.

- [ ] El cliente debe evaluar el `402` de su solicitud concreta antes de firmar: esquema, cantidad/techo, red, activo, destinatario, unidad y extensiones que afecten al servicio.
- [ ] Si difiere del listing, reevaluar la política ya autorizada del comprador: presupuesto absoluto, consumo, límite acumulado y preferencias. No incrementar automáticamente el permiso de gasto ni exigir confirmación humana si una política existente ya cubre la operación.
- [ ] Reutilizar la extensión opcional `offer-receipt` cuando sea compatible. Verificar firma, autoridad del firmante, coincidencia de campos y `validUntil`; no confiar solo en `acceptIndex`.
- [ ] El vendedor debe guardar o poder verificar los términos cotizados y respetarlos durante la vigencia definida. No recalcular silenciosamente otro precio en el reintento firmado. Si la oferta expiró antes de iniciar el pago, devolver nuevas condiciones para reevaluación.
- [ ] Para precios por input, diseñar vinculación explícita a método, parámetros/cuerpo y revisión de política. La oferta estándar contiene URL y términos económicos, pero no un compromiso general del cuerpo/método. Usar un perfil/extensión versionado o recurso de cotización asociado; no modificar los bytes de una oferta ya firmada.
- [ ] Para `upto`, conservar techo autorizado, tarifa/unidad y consumo reportado aparte del importe efectivo. La firma del techo no demuestra por sí sola que la medición de consumo fue correcta.
- [ ] Manejar expiración y fallos de liquidación sin pagos duplicados. Si una liquidación está pendiente o su resultado es incierto, reconciliar la autorización/identificador/transacción antes de emitir una nueva autorización.
- [ ] Ante esquema desconocido o condiciones no interpretables, detener la ruta automática de compra con causa concreta, manteniendo la posibilidad de descubrir el servicio.

La extensión de [ofertas y recibos firmados](https://github.com/x402-foundation/x402/blob/main/specs/extensions/extension-offer-and-receipt.md) documenta términos y expiración opcional. No constituye por sí sola un bloqueo de precio ejecutado por todos los servidores; hace falta implementar el comportamiento del vendedor. Su formato de transporte puede evolucionar.

**Aceptación:** un cambio de política posterior a una cotización vigente no cambia silenciosamente lo que se cobra por esa cotización. Oferta vencida, input distinto o presupuesto excedido impiden una nueva firma automática fuera de política. El vendedor y cliente pueden demostrar qué términos se aceptaron.

## 10. Fase P4: evidencia comercial complementaria en dx402

**Puntos a revisar:** [dx402/mod.rs](../../src/dx402/mod.rs), [types.rs](../../src/dx402/types.rs), [envelope.rs](../../src/dx402/envelope.rs), [service.rs](../../src/dx402/service.rs), [x402-axum/durable.rs](../../crates/x402-axum/src/durable.rs), [x402-reqwest/durable.rs](../../crates/x402-reqwest/src/durable.rs).

dx402 actual proporciona evidencia durable de entrega. Diseñar un envoltorio/versionado complementario, conservando sus garantías de privacidad y sin invalidar hashes/recibos existentes.

- [ ] Asociar snapshot del listing, fuente/fecha/contexto de lectura, oferta original aceptada, identidad de la opción, autorización máxima, liquidación efectiva y hash de lo entregado.
- [ ] Etiquetar quién aporta cada dato: el snapshot que entrega el comprador no es una declaración firmada por el bazar; una respuesta HTTPS archivada no equivale a una oferta firmada por el vendedor.
- [ ] Preservar literalmente firmas y payloads originales. Los hashes deben abarcar un formato/versionado documentado; no alterar el significado del hash de entrega actual para incluir datos nuevos sin migración.
- [ ] Almacenar información privada en evidencia cifrada, no en índices públicos. No publicar cuerpos de solicitud, credenciales ni cotizaciones personalizadas en las notificaciones de refresco.
- [ ] Permitir evidencia parcial con estado explícito. El recibo estándar mínimo no acredita por sí solo todos los términos ni el importe; conservar oferta y datos verificables de liquidación cuando se requiera ese vínculo.
- [ ] Mantener el principio actual: un fallo al archivar evidencia no hace fallar un pago ya válido. Las comprobaciones de presupuesto/vigencia pertenecen al flujo previo de compra, no al éxito del almacenamiento dx402.

**Aceptación:** recuperar la evidencia permite distinguir anuncio, oferta, máximo autorizado, cobro efectivo y entrega; verificaciones existentes siguen funcionando. No afirmar que archivar un precio lo mantiene vigente ni que una firma certifica la verdad de todo contenido adjunto.

## 11. Matriz de verificación para la implementación

Crear pruebas de comportamiento con fixtures y reloj controlado. Esta matriz es trabajo futuro; no se ejecutó al redactar el handoff.

| Caso | Resultado exigido |
| --- | --- |
| Feed anuncia `upto` con `extra` | Round-trip conserva esquema y extensiones; UI muestra techo. |
| Esquema no compatible | No se convierte a `exact`; capacidad de pago diferenciada de indexación. |
| Importe ausente, negativo, decimal en campo atómico o desbordado | Error clasificado; no oferta de cero fabricada. |
| Cero explícito permitido en una fase | Conservado sin confundirse con dato inválido; política de listing independiente. |
| Activos con 6/8/18 decimales e importes mayores que entero seguro JS | Formato exacto, moneda correcta y sin redondeo de aritmética binaria. |
| Múltiples opciones, misma URL, diferente red/variante | No se selecciona la primera ni se mezcla todo en un rango engañoso. |
| Feed idéntico sin fecha | No rejuvenece precio ni produce escritura por falso cambio. |
| Settlement sobre listing antiguo | Cambia actividad; no verifica ni reemplaza precio/techo. |
| Fuente atrasada con timestamp reciente o futuro | No pisa observación confiable por orden temporal; guardas de futuro permanecen. |
| Challenge solo en header; cuerpo es preview | Se leen términos del transporte apropiado. |
| Header y cuerpo contienen términos incompatibles | Conflicto explícito según protocolo; no mezcla sintética. |
| GET de salud versus POST con parámetros/autenticación | Salud y vigencia de esa variante permanecen separadas. |
| Cambia precio pero no destinatario | Refresco/reevaluación comercial; no cuarentena automática por secuestro. |
| Cambia destinatario | Conserva tratamiento de identidad/seguridad existente. |
| Reinicio con overlay y formato anterior | Lectura compatible; no fechas ni esquemas inventados al migrar. |
| Muchas lecturas simultáneas y varias réplicas | Deduplicación, cuotas por host y ausencia de tormenta de refresco. |
| Proveedor responde 429 o falla | Backoff, estado de precio explícito y sin amplificación. |
| Cotización vigente; precio general sube | Se respetan términos cotizados conforme al contrato implementado. |
| Cotización expirada o input cambiado | Nueva evaluación antes de firmar; no reutilización incorrecta. |
| `upto` máximo 0,10; efectivo 0,03 | No falso drift ni cambio del techo de catálogo a 0,03. |
| Liquidación de resultado incierto | Reconciliación antes de una autorización/pago nuevo. |
| Almacenamiento dx402 falla | Pago válido no se revierte; evidencia incompleta queda indicada. |

Por cada PR, ejecutar primero pruebas específicas de los módulos modificados y verificación visual EN/ES cuando cambie UI. Antes de integrar, completar los checks vigentes de [ci.yaml](../../.github/workflows/ci.yaml). A la fecha, incluyen:

```sh
python3 scripts/verify_landing_canonical.py --offline
cargo build --locked --features solana,near,stellar,algorand,sui,xrpl
cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1
cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance -- --test-threads=1
```

Usar el entorno que soporte las dependencias del repositorio; no convertir restricciones del Windows local en cambios de producto. Mantener pruebas automáticas sin dependencia de compras reales o disponibilidad de proveedores externos.

## 12. Métricas y criterios de operación

Nombres propuestos; implementarlos según convenciones del repositorio. No usar URL, wallet o huella de solicitud como etiquetas de alta cardinalidad.

- `discovery_price_comparison_total{result,source,scheme}`: resultados `match`, `within_range`, `changed`, `conflict`, `unknown`, `not_comparable`. El porcentaje de discrepancias usa solo observaciones comparables como denominador.
- `discovery_terms_age_seconds`: distribución de antigüedad observada; segmentar por clases de tráfico/configuración acotadas.
- `discovery_price_import_rejected_total{reason,source}`: esquema/cantidad/formato inválidos y causas de pérdida evitadas.
- `discovery_price_refresh_total{trigger,outcome}` y latencia/edad de cola: demanda, revisión, periódico, reintento.
- `discovery_price_propagation_seconds{destination}`: desde revisión conocida del origen hasta observarla en cada destino. Sin revisión comparable, informar medición desconocida.
- `purchase_quote_rejected_total{reason}`: vencida, contexto distinto, términos incompatibles, presupuesto o firma/autoridad inválida.
- `dx402_commercial_evidence_total{status}`: completa, parcial, omitida o fallida; sin equiparar presencia de evidencia con entrega comercial satisfactoria.

Medir línea base antes de imponer objetivos numéricos. Definir posteriormente percentiles de frescura/propagación, presupuesto de solicitudes y porcentaje de precio no verificable por clase de servicio. Mayor cantidad de probes no es por sí sola una mejora.

## 13. Secuencia de PRs, migración y despliegue

| PR | Alcance concreto | Dependencia / salida |
| --- | --- | --- |
| 1 | DTOs y normalización: esquemas, extras, importes, preservación de originales. | Fixtures de ida/vuelta; estrategia para recuperar registros ya degradados. |
| 2 | Modelo de fechas y observaciones, persistencia compatible, prioridad por procedencia/contexto. | PR 1; tests de merge, reinicio y settlement. |
| 3 | API y UI de opciones, unidades, frescura y rangos. | PR 1–2; revisión visual EN/ES y documentación OpenAPI. |
| 4 | Scheduler adaptativo, refresco por demanda, notificaciones y primer adaptador de proveedor. | PR 2; límites y métricas antes de ampliar tráfico. |
| 5 | Política del comprador y cotizaciones vigentes del vendedor. | Contrato de P3 definido; despliegue coordinado de los participantes. |
| 6 | Evidencia comercial opcional en dx402. | PR 5 y formato/versionado aprobado mediante revisión técnica del cambio. |

Despliegue propuesto:

1. Capturar línea base y copia recuperable de los datos que se migrarán, dentro del procedimiento operativo existente.
2. Leer/escribir el nuevo modelo sin cambiar inicialmente la selección pública de precio; comparar ambas vistas sobre muestras controladas.
3. Reimportar/revalidar registros afectados por pérdida de esquema o timestamps ambiguos. Migración reanudable e idempotente, con cuotas y métricas.
4. Activar la nueva vista en servicios propios y ampliar por fuente/clase según resultados.
5. Habilitar refresco adaptativo y push gradualmente; no crear pagos sintéticos para forzar indexación externa como parte de una prueba de lectura.
6. Publicar la semántica de campos para consumidores y observar sus tiempos reales de propagación.
7. Introducir cotizaciones y evidencia en fases separadas del cambio de catálogo.

Rollback: permitir desactivar refrescos/push o la nueva selección de observaciones sin borrar el overlay ni reescribirlo con formato antiguo. Si se retira temporalmente la nueva vista, preferir «precio no verificado» a volver a presentar datos conocidos como incorrectos. Mantener pagos existentes operativos según sus contratos actuales.

La CI revisada tiene ruta de despliegue desde `main`; al ejecutar este plan, tratar push/merge como acciones con posibles efectos en producción. Este handoff solo crea documentación y no ejecuta ninguna de esas operaciones.

## 14. Decisiones que debe resolver la implementación

Estas son decisiones técnicas pendientes, no preguntas bloqueantes para corregir F1–F3:

- Ubicación física del overlay de términos y estrategia de coordinación entre réplicas; aprovechar el almacenamiento existente con ownership claro de escritura.
- Identidad estable de opciones y contextos, especialmente variantes con/sin evidencia durable y precios por modelo/cantidad.
- Qué rangos son declarados, garantizados bajo un alcance o meramente observados; cómo conservar esa distinción al exportar a cada bazar.
- Política para esquemas desconocidos en el DTO público sin romper clientes existentes: vista extendida/versionada o registro diagnóstico separado.
- Disponibilidad de rutas de cotización seguras para vendedores con POST/autenticación; no inferirla desde salud HTTP.
- Capacidades reales de refresco de CDP, x402scan y otras fuentes seleccionadas; validar APIs antes de prometer push de extremo a extremo.
- Soporte de `offer-receipt` por SDK/red y perfil requerido para vincular input, revisión, consumo y cotización.
- Retención y acceso a evidencia de cotizaciones personalizadas, sin publicar datos privados en el catálogo.

## 15. Definición de terminado

- [ ] La ingestión y UI preservan el significado de los precios y las opciones de pago.
- [ ] Los registros históricos afectados se recuperaron o quedaron explícitamente sin verificar; no se dio la migración por terminada solo por desplegar código.
- [ ] Importar, observar y liquidar actualizan fechas distintas con semántica documentada.
- [ ] La respuesta del origen actualiza la observación del contexto correcto y no se pierde ante un feed atrasado.
- [ ] El catálogo expone frescura y discrepancias sin confundirlas con salud ni con variación legítima.
- [ ] Hay refresco acotado, medible y recuperable ante fallos; propagación por destino diferenciada de aceptación del trabajo.
- [ ] El comprador aplica sus límites a condiciones concretas antes de firmar; ofertas vigentes se respetan por el vendedor integrado.
- [ ] dx402 puede relacionar evidencia comercial y entrega sin alterar garantías existentes ni hacer fallar pagos por un problema de archivo.
- [ ] Pruebas, migración, documentación de API y verificación visual aplicables están completas y los límites restantes están documentados.

**Primer paso ejecutable para la siguiente sesión:** preparar PR 1 con fixtures que reproduzcan `upto` convertido a `exact`, pérdida de `extra` e importe inválido convertido a cero. Continuar con fechas y observaciones antes de aumentar la frecuencia de probes; aumentar la frecuencia del comprobador actual no corrige la pérdida de términos.
