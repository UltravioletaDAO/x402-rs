# Handoff — observabilidad del facilitador y rate limit del bazaar

Sesión 2026-08-01 → 2026-08-03. Todo lo listado como desplegado está verificado
contra producción con sondas repetidas, no con una sola lectura.

## Desplegado

| Versión | Qué |
|---|---|
| v1.65.0 | Los esquemas alternos (escrow, upto, fhe-transfer) salían de `post_settle`/`post_verify` por `return` tempranos que esquivaban el registro. Liquidaban on-chain y no dejaban rastro. Cada rama devuelve ahora `AltSchemeOutcome { response, detail }`: el tipo obliga a decidir qué registra. |
| v1.65.1 | `canonical_network_name()`. Una misma cadena entraba con tres grafías y abría tres filas en `/api/stats`. |
| v1.66.0 | (a) el hash sale bajo `transaction`, `transactionHash` y `transaction_hash`; (b) el extractor mira dentro de `payload` a secas; (c) fallos de RPC → 502 + `Retry-After` en vez de 400. |
| v1.66.1 | Mismo mapeo en `post_escrow_state`, que se había quedado fuera. |
| v1.67.0 | `/discovery/register`: `burst_size` 5 → 250. |

### El bug que costó dinero

Nuestro SDK de TypeScript leía `result.transactionHash || result.transaction_hash`;
el servidor emitía `transaction`. El cliente oficial devolvía `undefined` en el
campo del que depende la entrega, y un consumidor **revocó un acceso ya pagado
on-chain**. Lo encontró MeshRelay siguiendo nuestro cliente, no leyendo mal la
doc. Arreglado en ambos lados; test de regresión en `settle-tx-alias.test.ts`,
publicado en npm 2.47.0.

## Pendiente

1. **Los fallos no se publican.** `settlesFailed=0` es falso por diseño. Toda
   cifra de `/api/stats` es un piso. Peor caso medido: el panel decía "24
   éxitos, 0 fallos" en una hora que fueron 24 de 38.
2. **~84 filas con `asset`/`amount` en null** (anteriores a v1.66.0). No están
   perdidas: conservan el hash. `scripts/backfill/recover_missing_volume.py`
   existe y corre **solo en seco**. Los RPC públicos devuelven 403 para la
   mayoría; **hace falta usar nuestros RPC premium, no la API key de explorer**
   — cité esa key como bloqueo varias veces y era incorrecto.
3. **`/supported` anuncia redes que nunca han liquidado** (Celo). Anunciar lo
   que no se verifica es una trampa: el cliente elige una opción que parece
   legal y recibe un rechazo que no distingue de un error propio.
4. **Trusted publishing en npm.** El `NPM_TOKEN` de larga vida sigue ahí; migrar
   exige enlazar el paquete desde npmjs.com. Actions ya pinneadas por SHA.
5. **Split-brain sin detectar.** El mismo evento fue éxito en nuestro registro y
   fallo en el del consumidor, ambos coherentes consigo mismos. Hizo falta
   cruzar registros de dos dueños. Sin propuesta buena.

## Criterios de verificación

Comprobables con un comando, no con confianza:

- Una red que haya liquidado alguna vez **tiene fila** en `/api/stats`. Si no
  aparece, nunca liquidó. Sirve para descartar redes muertas, no para
  certificar sanas; `lastTs` por fila añade la recencia.
- El lote del bazaar está arreglado cuando el 06:00 UTC muestre **~200 intentos
  con 0 rechazados** en `POST /discovery/register`.
- El extractor aguanta mientras `sin volumen` se mantenga plano al subir
  `settlesOk`. Llevaba 14 settles consecutivos sin fallar.

## Trampas de medición encontradas

- **Filtros de CloudWatch con `=` devuelven cero.** Los logs llevan ANSI, así
  que `network=celo` no existe en los bytes crudos. Filtrar por un término sin
  símbolos y contar en local tras quitar el ANSI.
- **Contar rechazos en vez de intentos** hace que el abandono parezca solución:
  cuando los clientes se rinden, los 429 caen a cero.
- **Ventana móvil contra hora de reloj** dibuja tendencias que no existen.
- **Durante un rollout, ECS sirve dos versiones a la vez.** Una sola sonda a
  `/version` puede reportar desplegado algo que va a medias.
