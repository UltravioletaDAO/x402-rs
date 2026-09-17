# Publicación: recibos portables, SDK y grid de redes

Fecha: 2026-09-17. Facilitador **2.36.1**, Python **0.88.0** y TypeScript
**2.96.0** publicados y verificados. Ocho pagos reales de USDC confirmados:
ambos SDK en Arc y Hedera, mainnet y testnet. El grid ya cambia de orden al recargar.

## Versiones y publicación

| Componente | Versión | Referencia |
| --- | --- | --- |
| Facilitador | 2.36.1 | [PR #81: recibos y grid](https://github.com/UltravioletaDAO/x402-rs/pull/81), [PR #82: consulta de recibos](https://github.com/UltravioletaDAO/x402-rs/pull/82) |
| Python | 0.88.0 | [PyPI](https://pypi.org/project/uvd-x402-sdk/0.88.0/), [release](https://github.com/UltravioletaDAO/uvd-x402-sdk-python/releases/tag/v0.88.0) |
| TypeScript | 2.96.0 | [npm](https://www.npmjs.com/package/uvd-x402-sdk/v/2.96.0), [release](https://github.com/UltravioletaDAO/uvd-x402-sdk-typescript/releases/tag/v2.96.0) |

Commit desplegado: `3582a8a6463f430b8cdc3ef035d948e00569f59b`.
[CI y despliegue](https://github.com/UltravioletaDAO/x402-rs/actions/runs/35273196108):
segundo intento aprobado; dos réplicas sanas, despliegue ECS completado.
Las instalaciones de aceptación provienen de PyPI/npm, no del árbol de trabajo.

## Comportamiento entregado

- Recibo junto a la respuesta: red, activo, importe atómico, destinatario, hash
  de petición, identificador de liquidación, estado y motivo de rechazo definitivo.
- Firma JWS Ed25519 verificable con claves públicas del facilitador; consulta
  privada por capacidad, sin depender de un dashboard.
- Contexto y autorización persistidos antes del envío; reanudación con la misma
  autorización. Reserva transaccional y CAS en DynamoDB, sin TTL; bytes/ID
  persistidos antes de emitir la transacción.
- Propagación en FastAPI, Express y Hono, más helpers para otras integraciones.
  Swagger, esquema JSON, documentación MCP y guías actualizados.
- Grid mainnet/testnet barajado una vez por carga con Fisher–Yates. Filtros,
  pestañas e idioma conservan esa permutación. Iconos de stablecoins de 32 px.
- Hedera acepta USDC como pago; HBAR financia comisiones. Arc incluye USDC/EURC.
  Los badges de XRPL reflejan las stablecoins configuradas, incluido RLUSD.

## Ocho pagos y recuperación de compra

Cada pago transfirió **0,001 USDC** al destinatario controlado de aceptación.
Principal total: **0,008 USDC**, aparte de las comisiones. En cada compra el
merchant respondió HTTP 500 después de liquidar; al reanudar el contexto
persistido respondió 200 conservando autorización, recibo y transacción.

El verificador independiente comprobó el evento USDC exacto en Arc y los
movimientos del token nativo en Hedera Mirror Node: pagador, destinatario,
importe y resultado. Python usa x402 v1 en Arc; TypeScript usa v2. Hedera usa v2
con ambos SDK. Esta campaña no equivale a ejecutar en cadena todas las
combinaciones de protocolo ni incluye EURC.

Los hashes EVM se muestran sin el prefijo `0x`; añadirlo al consultarlos en
[Arc mainnet](https://explorer.arc.io/) o [Arc testnet](https://testnet.arcscan.app/).
Los IDs Hedera se consultan en [HashScan](https://hashscan.io/).

| SDK publicado | Red | Hash hexadecimal / ID nativo |
| --- | --- | --- |
| python | eip155:5042 | `55907a4d4c61d99704ce1a6a4ac3995457eec0d07ef1161431853f519dfacb57` |
| python | eip155:5042002 | `577bc5ee22b63bd7ee245405badaa5f5657e6260abd62ff7eecda2326b32b572` |
| python | hedera:mainnet | `0.0.10868300@1789676206.503674268` |
| python | hedera:testnet | `0.0.10576385@1789675863.692200899` |
| typescript | eip155:5042 | `5e6508b48d2709a2f97650b1b349f13be35b63dafe56d85036512f1678a4a66b` |
| typescript | eip155:5042002 | `36daa1f2fe1dc3ce3dfbc3715eb8de31c53efec4da6379ebad50e91310024321` |
| typescript | hedera:mainnet | `0.0.10868300@1789676260.615608413` |
| typescript | hedera:testnet | `0.0.10576385@1789679906.146328612` |

El [artefacto público de evidencia](2026-09-17-facilitator-receipts-production.json)
conserva los ocho recibos JWS íntegros, las claves públicas, versiones, direcciones,
importes, hashes, bloques o timestamps de consenso, consulta privada y reintentos.
Ambos SDK publicados verificaron las **nueve firmas** exportadas: ocho pagos y el
intento bloqueado descrito abajo. El payload de cada `signedReceiptJws` contiene
el recibo completo; las capacidades y autorizaciones de pago permanecen privadas.

## Intento bloqueado y corrección de consulta

Hubo **nueve intentos de compra y ocho pagos**. El primer intento de TypeScript
con Hedera testnet agotó la cuota diaria de reservas máximas del patrocinador.
Se conservó su recibo `unknown`: autorización expirada, reserva nativa ausente,
Mirror Node 404 y ningún nuevo movimiento conocido. El diagnóstico identificó
el fallo transaccional antes de la cofirma y el envío.

La consulta privada heredaba HTTP 400 del intento de liquidación y dificultaba
recuperar ese recibo con los SDK. En 2.36.1, `GET /receipts/{receiptId}` devuelve
200 al recuperar un recibo autorizado, incluso si su estado es `unknown`.
La firma, el estado y la respuesta original de POST permanecen iguales.
Los SDK Python y TypeScript verificaron esa recuperación y el rechazo 404
con una capacidad incorrecta, sin emitir pagos.

Se ajustó deliberadamente la cuota de testnet a **12 HBAR/día**; mainnet sigue
en **10 HBAR/día**. Son reservas por tarifa máxima, no las comisiones reales;
no se reintegran al presupuesto al terminar. Después se ejecutó una compra
independiente y explícita de aceptación, manteniendo el intento anterior.
No se creó automáticamente una autorización sustitutiva.

## Validación

- [Preflight de 2.36.1](2026-09-17-receipt-lookup-local.json): 2.610 ejecuciones
  Rust aprobadas, ocho pruebas frontend y cinco de balances; Terraform,
  permisos efectivos y revisión de seguridad correctos. El conteo Rust incluye
  biblioteca/binario y doctests, no 2.610 casos distintos.
- [CI del cambio 2.36.1](https://github.com/UltravioletaDAO/x402-rs/actions/runs/35272319139)
  aprobado; [validación inicial](2026-09-17-facilitator-receipts-local.json).
- Python: 1.236 pruebas aprobadas, empaquetado y `twine check` correctos.
- TypeScript: 771 pruebas aprobadas, tipos, lint, build y paquete correctos.
- Seis vectores firmados compartidos, instalación limpia Python y TS ESM/CJS;
  validación de concurrencia, CAS y reinicio con DynamoDB local.
- [Chrome contra producción 2.36.1](2026-09-17-random-grid-production.json):
  tres recargas con permutaciones diferentes en ambos grids, tarjetas completas,
  filtros USDC/EURC/RLUSD, teclado, idioma, wallets y diseño móvil/escritorio;
  sin errores JavaScript.
- `/version`, `/supported`, `/receipts`, Swagger, esquema y claves públicas
  comprobados; evidencia de disponibilidad incluida en el artefacto público.

## Incidentes del despliegue

El primer intento de 2.36.0 no tenía permiso efectivo para incorporar la clave
nueva al rol de ejecución ECS desde GitHub. Se aplicó únicamente la concesión
revisada para leer ese secreto con credenciales de operador. La restricción
IAM de GitHub permaneció intacta y se reejecutó solo el job fallido, con éxito.
El script `scripts/check_receipt_deploy_permissions.py` ahora permite revisar
roles y permisos antes de desplegar, sin leer valores secretos ni modificar IAM.

En 2.36.1 algunas réplicas agotaron la consulta inicial de consenso Hedera de
cinco segundos. El despliegue se recuperó y ambas réplicas quedaron sanas,
pero la espera de GitHub había vencido. Se reejecutó solo el job de despliegue,
usando la imagen ya construida; terminó aprobado sin cambios adicionales de código.
El arranque depende de esa consulta externa: queda registrado como mejora
operativa de resiliencia, sin desactivar la comprobación de disponibilidad.

## Límites y siguientes fases

Los pagos reales EURC siguen diferidos por instrucción del usuario. Hay soporte
implementado y vectores offline; **no hay liquidación EURC real en esta campaña**.
La emisión de estos recibos se anuncia para Arc y Hedera, no para todas las redes.

El recibo confirma el pago, no la entrega del producto. El merchant debe conservar
un contexto por compra y deduplicar la entrega. Una capacidad nueva es otra compra.
Las reservas abandonadas antes de preparar transacción requieren revisión; no
se genera otra autorización. No hay monitor continuo de reorganizaciones ni
archivo automático. Antes de rotar claves deben conservarse las claves públicas
anteriores. El helper comprador Python es síncrono; FastAPI sí es asíncrono.

[Contrato y operación](../facilitator-receipts.md).
[Plan maestro actualizado](../plans/facilitator-receipts-master-plan.md).
