---
date: 2026-09-10
tags:
  - type/backlog
  - domain/x402
  - priority/p2
status: ready-to-build
urgency: 4/10
---

# La línea que llama a la decisión de pago no la toca ningún test

**En una frase:** `pay_for_challenge` decide si un `402` se firma y está bien
cubierta, pero la línea de `Middleware::handle` que la llama no la ejerce ningún
test, que es exactamente el hueco por el que el cableado de la vigencia faltó un
commit entero con todo en verde.

## Por qué 4 y no más ni menos

**No es un defecto activo.** La línea es correcta hoy (`x402-reqwest`,
`middleware.rs`, dentro de `handle`), la decisión está cubierta por cuatro tests
que entran por `pay_for_challenge`, y P3 v2 la dejó con una firma que toma el
desafío completo para que no se pueda volver a pasar las partes por accidente.

**Tampoco es cero.** El antecedente es concreto y de este mismo día: entre
`eef4455c` y `d5b5b52a`, `handle` llamaba a `build_payment_header(&accepts)` y
tiraba el mapa de `extensions`, así que `offer-expired` no podía dispararse nunca
en el camino automático. Todos los tests pasaban. Lo encontró una revisión de
seguridad, no la suite.

## Qué habría que hacer

Un test que entre por `Middleware::handle` de verdad: un `wiremock` que devuelva
un `402` con una oferta vencida y la aserción de que el cliente **no** reintenta
con `X-Payment`.

## Por qué no se hizo en P3 ni en P4

Necesita `wiremock` como dev-dependency de `x402-reqwest`, y eso toca
`Cargo.lock` — que en este repo es lo que hace fallar un build `--locked` si no
se actualiza en el mismo commit. Es un cambio de dependencias en un PR que tocaba
dinero, y separarlo era más barato que mezclarlo.

## Cómo se sabe que está hecho

El test existe, y falla si se restaura el cableado viejo (pasar `accepts` sin
`extensions`).
