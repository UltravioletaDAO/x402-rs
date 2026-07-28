# Plan — Stream de eventos en tiempo real del facilitador (`GET /events`, SSE)

**Estado:** plan aprobado, F1 en implementación
**Origen:** KarmaCadabra quiere pintar el tráfico del facilitador en su observatorio 3D
(la "ola del facilitador"). Hoy ese tráfico solo vive en CloudWatch (us-east-2), que no
sirve para tiempo real.
**Fecha:** 2026-07-28

---

## Hechos verificados (no supuestos)

| Hecho | Cómo se verificó | Implicación |
|---|---|---|
| `axum 0.8` + `tokio` full | `Cargo.toml` | SSE y WS ya vienen — **cero dependencias nuevas** |
| `desired_count = 1`, 1 task viva | `aws ecs describe-services` (prod, us-east-2) | Un bus en-proceso ve **todos** los eventos |
| Detrás de un **ALB** (:8080) | mismo describe-services | Soporta SSE nativamente |
| `CorsLayer::allow_origin(Any)` | `src/main.rs:511` | El dashboard consume cross-origin sin proxy |
| **~1.427 settlements / 2h ≈ 12/min** | `aws logs filter-log-events "[SETTLEMENT]"` | Por-evento es viable; **no hace falta agregación** |
| No existe bus de eventos ni audit log | grep en `src/` | Hay que añadirlo (~30 líneas) |

## Decisiones tomadas (Saul, 2026-07-28)

1. **Transporte: SSE**, no WebSocket. Es una vía (que es la forma real del dato), el
   browser reconecta solo con `EventSource`, pasa el ALB sin upgrade y son pocas líneas
   en axum. WS queda como upgrade si algún día el cliente necesita hablar de vuelta.
2. **Detalle: completo** (`payer`, `transaction`, monto, red). Se dejó constancia de que
   ~la mayoría de ese tráfico NO es de KarmaCadabra y que difundirlo en vivo expone la
   actividad de pagos de otros clientes; Saul asumió esa decisión a propósito. Por eso el
   nivel de detalle **también es una variable** (`X402_EVENTS_DETAIL`), para poder bajarlo
   sin tocar código.
3. **Alcance: todo el tráfico por ahora, pero configurable** a solo los settlements de
   KarmaCadabra — mediante una **allowlist de direcciones**, no una regla hardcodeada:
   el facilitador es un producto genérico y no debe saber qué es "KK".

## Configuración (todo por env, fail-safe)

| Variable | Default | Qué hace |
|---|---|---|
| `X402_EVENTS_ENABLED` | `true` | Kill switch. `false` → el endpoint responde 404 y no se publica nada |
| `X402_EVENTS_SCOPE` | `all` | `all` = todo el tráfico · `allowlist` = solo payers en la lista |
| `X402_EVENTS_ALLOWLIST` | *(vacío)* | Direcciones separadas por coma (case-insensitive). Aquí irían las wallets del swarm KK |
| `X402_EVENTS_DETAIL` | `full` | `full` = payer + tx + monto · `minimal` = solo `{ts, kind, network, ok}` |
| `X402_EVENTS_BUFFER` | `256` | Capacidad del canal broadcast |
| `X402_EVENTS_MAX_SUBSCRIBERS` | `64` | Suscriptores concurrentes admitidos. Al tope, `/events` responde 503 + `Retry-After` |

## Contrato del evento

```
event: settle            (o: verify)
data: {"ts":"2026-07-28T15:04:05Z","kind":"settle","network":"base","ok":true,
       "payer":"0x…","tx":"0x…","amount":"0.02","asset":"usdc"}
```

En `minimal`, `payer`/`tx`/`amount`/`asset` se omiten. Un comentario SSE (`:keepalive`)
cada 15s mantiene viva la conexión a través del ALB.

## Invariantes de seguridad (no negociables)

- **El camino del dinero nunca se bloquea por el stream.** El canal es
  `tokio::sync::broadcast` (lossy): si no hay suscriptores o el buffer se llena, el
  evento se descarta en silencio. `send()` jamás propaga error al handler de settle.
- **Publicar es lo ÚLTIMO** que ocurre, después de que el settle ya se resolvió. Un
  panic/error en el publisher no puede afectar el resultado del pago.
- **Rate limit** con `tower_governor` (ya es dependencia) sobre `/events`, y tope de
  suscriptores concurrentes: es un endpoint público sin auth.
- **Sin secretos jamás**: ni llaves, ni firmas, ni payloads de autorización.

## Fases

### F1 · Facilitador (este repo) — COMPLETO, desplegado en v1.59.5
- `src/events.rs`: `TrafficEvent`, `EventBus` (broadcast + config desde env), `publish()`.
- Hook en `verify`/`settle` — al final, best-effort.
- `GET /events` → `Sse<impl Stream>` con keepalive.
- Rate limit (`tower_governor`, 1 token/2s, burst 10) + tope de suscriptores concurrentes
  con 503 al llegar: el invariante de arriba, que la primera pasada dejó sin implementar.
- `network` sale por `Display` (el slug canónico), **no** por `{:?}`: Debug imprime el
  nombre de la variante, así que `SkaleBase` viajaba como `skalebase` y no correspondía a
  ninguna placa ni a ningún nombre de `/supported`.
- Documentado en `src/openapi.rs` (`/docs`).
- Tests: el bus no bloquea sin suscriptores; `allowlist` filtra; `minimal` no filtra campos
  sensibles; el tope corta, libera slot al desconectar y nunca afecta a `publish()`.

### F2 · Dashboard KarmaCadabra (repo karmakadabra)
- `EventSource("https://facilitator.ultravioletadao.xyz/events")` en `live.js` → bus.
- Nodo del **facilitador** como tercer hub en la escena.
- Cada evento = **onda** viajando hacia la placa de su cadena — visualmente distinta de
  la pepita de KK (pepita = dinero de un agente nuestro; onda = tráfico del facilitador).
- Throttle en el cliente: a 12/min no hace falta, pero el cap protege de una ráfaga.

### F3 · Endurecer (cuando aplique)
- Si `desired_count > 1`: un cliente pega a UNA instancia y ve solo sus eventos → la ola
  sub-reporta. Opciones: documentarlo, o fan-out por Redis. **Hoy no aplica** (1 task).
- Métricas del propio stream (suscriptores, eventos descartados).

## Backlog

### B1 · Emitir también las operaciones que FALLAN

**Decisión de Saul (2026-07-28): por ahora NO. Queda anotado por si algún día lo queremos.**

Hoy solo se publica desde la rama `Ok(...)` de `verify` y `settle`. Una operación que
revienta sale por `Err(...)` y **no emite nada**. Consecuencia concreta: `ok:false` solo
aparece cuando la operación se resolvió y dio negativa (`SettleResponse.success == false`,
`VerifyResponse::Invalid`), nunca cuando falló de verdad.

Probado empíricamente, no deducido: un `POST /verify` con la autorización expirada
devuelve `InvalidTiming` por `Err`, y el stream no ve absolutamente nada.

Por qué podría quererse algún día: para un panel de observabilidad, un settle que falla es
justo lo que querés ver — un RPC caído o una firma mal formada son la señal, no el ruido.
Con el diseño actual el observatorio pinta un riel que siempre se ve sano, porque los
fallos son invisibles por construcción.

Si se implementa, dos cosas que no se pueden pasar por alto:

1. **No difundir el motivo del error tal cual.** Los `Err` llevan detalle interno
   (direcciones, a veces URLs de RPC — ver `src/redact.rs`, que existe justamente porque
   ya se filtraron claves de RPC en strings de error). El evento debería llevar una
   categoría acotada, nunca el `Display` del error.
2. **El invariante sigue mandando.** Publicar tiene que seguir siendo lo último y seguir
   siendo infalible; un fallo no puede volverse más caro por estar siendo observado.

Alternativa más barata si solo se busca visibilidad de salud: exponer contadores por
métricas (F3) en vez de ampliar lo que el stream difunde.

## Criterio de "listo"

1. `curl -N https://facilitator.ultravioletadao.xyz/events` imprime eventos reales según
   ocurren. Contrastar con el filtro `"POST /settle"` en CloudWatch — **no** con
   `[SETTLEMENT]`, que no existe como marcador y devuelve 0 siempre.
2. Con `X402_EVENTS_ENABLED=false` el endpoint da 404 y el facilitador sigue liquidando igual.
3. Con `X402_EVENTS_SCOPE=allowlist` + las wallets de KK, solo aparecen settlements del swarm.
4. En el observatorio, las ondas llegan a las placas de cadena correctas.

**Ritmo real, medido 2026-07-28 (corrige el ~12/min que se venía repitiendo):** 467
`POST /settle` en 6 h = **~1,3/min**, más 8 `verify` en las mismas 6 h. Es ~9x menos de lo
que decía el handoff, y es **a ráfagas**: hubo tramos de 35 min con CERO settlements. Quien
diseñe la UI tiene que asumir que el estado normal es "quieto" — escuchar 40 s y no ver
nada es lo esperado, no un síntoma.
