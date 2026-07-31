# Backlog — publicar las operaciones que FALLAN

**Estado:** no implementado. No existe palanca de configuración.
**Por qué importa:** sin esto, `/stats` y `/events/live` muestran un riel que
**siempre** se ve sano.

---

## Corrección al registro

En la conversación del 2026-07-30 dije que esto "quedó configurable, hoy en
`false`". **Era falso.** Estaba en el plan y nunca se implementó. Un `grep` por
`PUBLISH_FAILURES` en `src/` no devuelve nada, y la publicación vive únicamente
en la rama `Ok(...)` de `post_settle` y `post_verify`.

Queda escrito acá para que nadie busque una variable de entorno que no existe.

## Qué pasa hoy

Tanto el evento como el registro se emiten solo desde la rama `Ok`. Una
operación que revienta —RPC caído, firma malformada, contrato que revierte— sale
por `Err` y **no produce ni evento ni fila**.

Consecuencia medible: `ok:false` solo aparece cuando la operación *se resolvió* y
dio negativa (`SettleResponse.success == false`, `VerifyResponse::Invalid`).
Nunca cuando falló de verdad.

Probado, no deducido: un `POST /verify` con firma falsa devuelve
`execution reverted: FiatTokenV2: invalid signature`, sale por `Err`, y ni el
stream ni la tabla ven absolutamente nada.

## Por qué se querría

Para un panel de observabilidad, **un settle que falla es justo lo que querés
ver** — un RPC caído o una firma mal formada son la señal, no el ruido. Con el
diseño actual una tasa de éxito del 100% significa "no se registró ningún
fallo", que no es lo mismo que "no hubo fallos". Las dos páginas lo dicen en su
propio texto, pero decirlo no lo arregla.

## Las dos trampas al implementarlo

1. **No difundir el `Display` del error.** Los `Err` llevan detalle interno:
   direcciones y, a veces, URLs de RPC con la API key adentro. `src/redact.rs`
   existe precisamente porque eso ya se filtró una vez. El evento debe llevar una
   **categoría acotada** (`rpc_error`, `invalid_signature`, `insufficient_funds`,
   `contract_revert`), nunca el texto crudo.
2. **El invariante sigue mandando.** Publicar debe seguir siendo lo último y
   seguir siendo infalible. Un fallo no puede volverse más caro por estar siendo
   observado — y la rama `Err` es exactamente donde el sistema ya está en
   problemas.

## Forma sugerida

- `X402_EVENTS_PUBLISH_FAILURES=true|false`, default `false` para no cambiar el
  comportamiento actual de nadie.
- Mismo interruptor para el stream y para el store: dos fuentes que discrepan
  sobre qué es un fallo son peores que ninguna.
- `kind` se mantiene (`verify` / `settle`); se agrega `error` con la categoría.
- Cuando esté en `true`, `/stats` deja de mostrar la advertencia de "100% no
  significa sin fallos" y pasa a mostrar la tasa real.

## Alternativa más barata, si solo se busca salud

Contadores por métricas (OpenTelemetry ya está cableado) en vez de ampliar lo
que el stream difunde públicamente. No da el detalle por operación, pero
responde "¿está fallando algo?" sin sumar exposición.

## Ver también

`docs/plans/traffic-events-stream.md` §B1, donde esto se registró primero.
