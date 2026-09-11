---
date: 2026-09-10
tags:
  - type/handoff
  - domain/dx402
  - domain/bazaar
  - priority/p4
status: active
---

# Anuncio, oferta, techo, cobro y entrega son cinco afirmaciones de cuatro partes distintas

**Versión:** 2.26.0 · **Fase P4** del plan de precios de Astra 6 (sección 10) ·
PR 6 de 6, **última del plan**.

dx402 ya prueba una cosa muy bien: que los bytes que el comprador recupera son
los bytes que se entregaron. `contentHash` es keccak del cuerpo en claro, el
facilitador firma un recibo sobre él, y cualquiera con nuestra clave pública lo
comprueba sin llamarnos.

Lo que no puede responder es comercial. *¿Era éste el precio que anunciaba el
catálogo? ¿El cobro entró en el techo que el comprador autorizó? ¿Cuál de las
opciones del vendedor se compró?* Son cinco afirmaciones de cuatro partes
distintas, y meterlas en el hash de entrega haría dos cosas inaceptables a la
vez: cambiaría lo que ese hash significa para todos los recibos que ya
circulan, y metería el contexto privado de la solicitud dentro de un valor que
circula.

## El envoltorio es complementario y se versiona solo

`CommercialEvidence` referencia la evidencia de entrega por `paymentId` y
**copia** `contentHash`; no lo recalcula y nada de lo que guarda entra en él.
Tiene su propia versión (`cv`), separada de `DX402_VERSION` a propósito: un
cambio comercial no debe parecer un cambio de entrega, ni al revés.

**Hay un test que falla si alguien alguna vez lo funde.** Probado en rojo:
haciendo que `with_listing` mezcle el digest del listado en el hash de entrega,
caen tres tests, incluido el que lleva el nombre de la invariante. Los 1.063
tests del facilitador siguen en verde, los de dx402 incluidos y sin tocar.

## Nadie habla por nadie

Es la propiedad más importante del archivo. Cada sección registra **quién la
afirmó** y **cómo está respaldada**:

| sección | quién la afirma | respaldo típico |
|---|---|---|
| snapshot del listado | **el comprador** | `stated` |
| oferta aceptada | el vendedor | `archived`, o `signed` si vino firmada |
| autorización (techo) | el comprador | `signed` |
| liquidación (cobro) | la cadena | `on_chain` |
| entrega | ya existía | el recibo del facilitador |

Un snapshot que entrega el comprador **no es una declaración firmada por el
bazar**: no presenciamos la lectura y el bazar no firmó nada. Una respuesta HTTPS
archivada **no equivale a una oferta firmada por el vendedor**. Etiquetarlas
igual produciría evidencia que parece autoritativa y no lo es, y el valor entero
de guardarla es poder decir después a qué se puede atener alguien.

**Una firma no certifica el contenido.** Prueba que el firmante emitió esos
bytes. No hace verdadero lo que dicen, no se extiende a la sección de al lado, y
**archivar un precio no lo mantiene vigente**. La documentación del módulo lo
dice con esas palabras.

## Evidencia parcial, con estado explícito

Toda sección es opcional y se agrega cuando existe. `completeness()` devuelve
`complete` o `partial` y `missing()` nombra lo que falta. Evidencia parcial con
estado explícito es mejor que nada, y mucho mejor que algo que parece completo
porque los huecos se llenaron con suposiciones.

Y **completo no es verificado**: significa que las cinco afirmaciones están
registradas con su procedencia, no que coincidan. Hay un test con una evidencia
completa cuyo cobro supera el techo.

`charge_within_authorization()` devuelve `None` cuando falta una de las dos
mitades. Una pregunta que no se puede responder no recibe un default
tranquilizador.

Para `upto`, techo y cobro efectivo viven en campos distintos: 0,10 autorizado y
0,03 cobrado es compatible por definición, y **el techo del catálogo no se
reescribe al último cobro**.

## Privacidad

`public_view()` es lo único que puede ir a un índice. Conserva los joins y las
etiquetas de procedencia y tira todo lo que podría llevar una solicitud privada,
una credencial o una cotización personalizada: los bytes de la oferta, el
contexto de la lectura, el digest del listado y el consumo que reportó el
vendedor. Hay un test que busca cada uno de esos literales en la salida.

La regla, dicha simple: **un índice dice que la evidencia existe y qué forma
tiene. Nunca dice qué hay adentro.**

## Los cuatro residuales del revisor

Tres resueltos acá, uno en backlog.

- **(b)** El contrato de los SDK ahora dice, en un bloque destacado y no al pie,
  que **sin `with_policy` el middleware es `permissive()` y no hay reja de
  activo**.
- **(c)** Documentada la trampa del `0x`: una dirección EVM escrita sin prefijo
  se compara exacta y **nunca** coincidirá, así que el pago se rechaza. Falla del
  lado seguro y ahora está pinchado con un test. No se normaliza por el usuario:
  agregar el prefijo es adivinar la familia, y un base58 pelado no se distingue
  de un hex pelado mirándolo.
- **(d)** Con `DISCOVERY_REVALIDATION_SHARED_CAP` menor que 100, la primera
  escritura podía aterrizar hasta 100: `attribute_not_exists(pending)` es
  verdadero en esa primera y el margen no la acota. **El comentario que yo había
  escrito estaba mal** — decía que un margen cero rechaza la escritura, y eso
  sólo vale de la segunda en adelante. Ahora el lote se recorta contra el tope.
- **(a)** `docs/plans/backlog-dx402-middleware-402-test.md`: un test que entre
  por `Middleware::handle` con wiremock. No se hizo acá porque necesita
  `wiremock` como dev-dependency y eso toca `Cargo.lock` en un PR que toca
  dinero.

## Para c0der

### Decisiones de la sección 14 tomadas acá

- **Retención y acceso a evidencia de cotizaciones personalizadas**: la
  cotización personalizada vive **sólo** dentro de la evidencia cifrada. El
  índice público no la lleva, y `public_view()` es la única forma de derivar lo
  que sí puede publicarse. La retención sigue siendo la de dx402
  (`DX402_RETENTION`), sin una segunda política que pudiera divergir.
- **Formato y versionado del envoltorio**: versión propia (`cv`), separada.
  Alternativa descartada: extender `AnchoredEvidence` con campos opcionales,
  que habría atado la evolución comercial a la de entrega y hecho que un lector
  viejo tratara un envoltorio nuevo como evidencia de entrega incompleta.
- **Identidad de la opción**: esquema, red, activo y destinatario. **Nunca el
  índice en `accepts`**, que es una posición en una lista que el vendedor
  reordena.

### Qué cambia para los SDK durable

Nada obligatorio. El envoltorio es complementario: un SDK que no lo conozca sigue
anclando y recuperando exactamente como hoy, y ningún recibo existente cambia.

Lo que un SDK que **sí** lo quiera tiene que respetar:

1. `cv` es la versión del envoltorio comercial, **no** `DX402_VERSION`.
2. `deliveryContentHash` se **copia**. Recalcularlo rompe todos los recibos.
3. Toda sección lleva `assertedBy` y `backing`. **No inventen una sección sin
   ellos**, y no promuevan `archived` a `signed` porque la respuesta llegó por
   TLS.
4. `observedAt` ausente es desconocido. No lo rellenen con el reloj propio.
5. `raw` de la oferta son **los bytes originales**. Una reserialización no se
   puede contrastar con una firma.
6. Lo que va a un índice sale de `publicView`, nunca del envoltorio entero.

### El cierre del plan: qué quedó fuera de P0-P4 y por qué

| lo que quedó fuera | de qué fase | por qué |
|---|---|---|
| Adaptadores por bazar y propagación por destino | P2, punto 9 | necesita APIs de terceros cuyo contrato el propio anexo pide revalidar; construí el mecanismo y no inventé adaptadores para APIs que no verifiqué |
| Política de precios del vendedor (discovery y challenge desde una fuente) | P3, punto 8 | es trabajo de `x402-axum` y del contrato de P3; mezclarlo habría unido el índice con el vendedor |
| Verificación de **firma** de `offer-receipt` | P3, punto 2 | necesita decidir qué clave firma una oferta y cómo se prueba que es la del vendedor: diseño de identidad, no un parser |
| Vinculación por input (método, parámetros, revisión) | P3, punto 4 | la extensión versionada es el lugar; el perfil queda sin definir hasta que haya un vendedor real que lo necesite |
| Contabilidad de `upto` y reconciliación de liquidaciones inciertas | P3, puntos 5 y 6 | el facilitador ya tiene anti-doble-cobro, nonces y `PendingNonceManager`; un segundo mecanismo al lado sin leer el primero es un segundo mecanismo para el mismo problema |
| Comprobación del facilitador sobre la oferta vigente | P3, punto 7 | requiere que la oferta firmada le llegue al facilitador, que es el punto 2 otra vez |
| Métricas de la sección 12 | todas | los estados están calculados y logueados con vocabulario acotado; instrumentarlos en un exportador es trabajo con su propio presupuesto de cardinalidad |

**Lo que sí quedó, en una línea cada uno:** P0 dejó de reescribir el esquema de
pago ajeno; P1 separó las fechas y guardó lo que el origen responde; P2 volvió a
leer el precio porque alguien lo miró, dentro del presupuesto que ya había; P3
movió la decisión de comprar del catálogo a la oferta en la mano; P4 permite
distinguir, después, qué se anunció, qué se ofreció, qué se autorizó, qué se
cobró y qué se entregó — y quién lo dice.

**Y una advertencia para quien siga:** el incidente del 16:41Z-20:49Z de hoy no
salió de ninguna de estas fases. Salió de que P0 arregló un parser y nadie
dimensionó lo que entraba por el agujero que destapó. La decisión de producto que
sigue abierta con el dueño — si este servicio quiere ser el índice completo del
ecosistema x402 — no se responde con un techo, y está en
[el handoff del segundo hotfix](2026-09-10-hotfix-cpu-2.md).
