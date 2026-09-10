---
date: 2026-09-10
tags:
  - type/handoff
  - domain/bazaar
  - domain/x402
  - priority/p3
status: active
---

# La decisión de comprar se toma contra la oferta en la mano, no contra el catálogo

**Versión:** 2.25.0 · **Fase P3** del plan de precios de Astra 6 (sección 9) ·
PR 5 de 6. **Esta fase toca dinero.**

Y trae, al principio, **un arreglo de P2 que no llegué a avisar a tiempo**.

## Primero: lo que se coló en 2.24.0

`offer_to_owner` tomaba **una** URL. Una página de listados puede traer cien
registros vencidos, así que una lectura pública producía cien tareas lanzadas y
cien escrituras a DynamoDB. Y no se autolimitaba: cuando el conjunto compartido
llega a su tope, la escritura condicional empieza a **fallar**, y cien
escrituras que fallan cuestan lo mismo que cien que salen bien.

Lo encontré revisando mi propio código mientras corría el CI de #43, y el merge
llegó antes que el aviso. Un string set acepta muchos valores en un solo `ADD`,
así que ahora una página es **una** escritura. Es la misma lección que el resto
del 2026-09-10: el trabajo en un camino de lectura no puede escalar con el
tamaño del catálogo. Cuatro tests.

## El hallazgo de esta fase: un esquema ajeno hacía impagable al vendedor entero

Probado en rojo antes de tocar nada:

```
RED-PROOF the whole challenge failed to parse:
  unknown variant `batch-settlement`, expected one of `exact`, `fhe-transfer`,
  `escrow`, `commerce`, `upto`
```

`Scheme` es un enum cerrado, y con razón: un pago que no sabemos nombrar es un
pago que no podemos hacer. Pero **`accepts` es una lista**, y una entrada
ilegible tumbaba la lista entera. Un vendedor que ofrecía `exact` **al lado de**
cualquier cosa que este build no implementa quedaba impagable, y el comprador
nunca se enteraba de que había una oferta perfectamente pagable ahí mismo.

Es exactamente el error que el catálogo tenía antes de P0, una capa más arriba:
un tipo cerrado haciendo de colección abierta.

Ahora las ofertas legibles se conservan y las que no se cuentan con su nombre de
esquema, así que una negativa puede decir **qué ofreció el vendedor** en vez de
"no pude parsear":

```
no offer in this challenge is one this build can pay; offered: ["batch-settlement", "agent-pay"]
```

Descubrir el servicio sigue funcionando aunque comprarlo automáticamente no.

## La política del comprador

`x402-reqwest`: `PurchasePolicy`, evaluada **contra la oferta concreta**, antes
de firmar. Dos reglas que mandan sobre todo lo demás:

**Una divergencia respecto del listado no es, por sí sola, una negativa.** Si la
oferta cuesta más que lo que decía el catálogo pero entra en una política que el
operador ya autorizó, se paga. Parar a preguntar convertiría cada repricing
ordinario en un alto, y un agente que se detiene ante el comercio normal es un
agente que nadie puede dejar corriendo. No hay ningún gancho de confirmación
humana en este camino.

**Una política nunca se ensancha para que entre una oferta.** Ni un byte, ni una
vez, ni "porque el vendedor lo dice". No existe método que suba un límite desde
dentro de una evaluación. Si la oferta se pasa, la respuesta es una negativa con
causa concreta y el que decide autorizar más es quien llama.

Orden de evaluación, **fijo y parte del contrato**, porque la PRIMERA causa que
falla es la que se reporta:

1. ¿Había alguna oferta legible? → `no-readable-offer`
2. ¿Venció? → `offer-expired`
3. ¿El destinatario está permitido? → `recipient-not-permitted`
4. ¿Pasa el techo por pago? → `per-payment-limit`
5. ¿Pasa lo que queda del techo acumulado? → `cumulative-limit`

La comparación contra el listado se hace **al final** y no decide nada: es
evidencia para quien llama.

Tres decisiones que valen decir:

- **Evaluar no gasta.** Firmar puede fallar y una liquidación puede rechazarse;
  un límite que contara intentos dejaría a quien llama sin dinero que nunca
  gastó. `record_spend` es explícito y se llama cuando el pago liquidó.
- **Un clon gasta de la misma bolsa.** Un middleware se clona por request; si
  cada clon llevara su propio total, un límite acumulado no significaría nada.
- **Un lock envenenado reporta el techo, no cero.** Si el mutex del total se
  envenena, `spent()` devuelve el límite: para el dinero, la dirección segura es
  negarse, nunca permitir.

## La vigencia, dicha por el vendedor y leída por el comprador

`x402-axum` puede declarar cuánto vale su oferta:
`X402Error::with_offer_validity(Duration)` publica
`extensions["offer-receipt/1"].info.validUntil`.

Sin eso, "¿esta oferta venció?" no tiene respuesta, y cada comprador o trata
toda oferta como eterna o inventa una ventana propia — y una ventana que
inventó el comprador no es un compromiso que hizo el vendedor.

**La clave está definida una sola vez**, en `x402-rs`, porque los dos crates la
necesitan y no dependen entre sí. Dos constantes serían dos oportunidades de no
coincidir, y un vendedor y un comprador nombrando claves distintas no tienen
nada que decirse. Hay un test que lo afirma.

La versión va **en la clave**: el transporte de offer-and-receipt todavía puede
cambiar, y un valor leído de una clave sin versión no podría compararse con nada
después. Una clave que no reconocemos se ignora, lo que significa "sin
vencimiento declarado", que es distinto de "venció".

## Lo que esta fase NO trae, y por qué

Dicho fuerte, porque la sección 9 tiene más puntos que estos y un handoff que no
los liste sería un handoff que los da por hechos:

- **Verificación de firma de `offer-receipt` (punto 2).** Está el transporte y la
  vigencia; **no** la verificación de firma ni de autoridad del firmante. Eso
  necesita decidir qué clave firma una oferta y cómo se prueba que esa clave es
  la del vendedor, que es diseño de identidad y no un parser.
- **Vinculación por input (punto 4).** La extensión versionada existe y es donde
  va; el perfil que ata método, parámetros y revisión de política, no.
- **Contabilidad de `upto` (punto 5)** y **reconciliación de liquidaciones
  inciertas (punto 6)**: el facilitador ya tiene anti-doble-cobro, nonces y
  `PendingNonceManager`, y meter una segunda reconciliación al lado sin leer esa
  primero sería construir un segundo mecanismo para el mismo problema.
- **La comprobación del facilitador (punto 7).** Un `settle` que no pague más que
  la oferta vigente presentada. Requiere que la oferta vigente le llegue al
  facilitador, que es el punto 2 otra vez.

**El vendedor no recalcula en silencio.** Lo verifiqué en el código antes de
construir nada: `payment_requirements` es un `Arc` calculado por instancia de
middleware, no por request, así que el 402 y el reintento firmado se sirven del
mismo objeto. La falla que describe el anexo no puede ocurrir con el middleware
actual, y por eso no construí un libro de cotizaciones para prevenirla. Lo que
faltaba era la mitad positiva: decir cuánto vale la oferta. Eso sí está.

## Para c0der

### El contrato para los SDK

Esto es lo que `uvd-x402-sdk-python` y `-typescript` tienen que implementar, para
que esos encargos salgan de acá y no de una relectura.

**Campos de la política:**

| campo | tipo | significado |
|---|---|---|
| `perPayment[asset]` | entero en unidades atómicas | máximo de UN pago en ese activo |
| `cumulative[asset]` | entero en unidades atómicas | máximo total mientras viva la política |
| `spent[asset]` | entero | lo ya registrado; sólo lo mueve `recordSpend` |
| `onlyPay[]` | lista de direcciones | destinatarios permitidos, comparados en minúsculas |

**Orden de evaluación** (la primera que falla es la que se reporta):
`no-readable-offer` → `offer-expired` → `recipient-not-permitted` →
`per-payment-limit` → `cumulative-limit`.

**Códigos de error**, vocabulario cerrado, en kebab, para ramificar sin parsear
inglés: los cinco de arriba. Cada uno lleva los números que lo causaron
(`requested`, `allowed`, `spent`, `wouldTotal`, `asset`, `payTo`, `validUntil`,
`now`, `offered[]`).

**Comparación con el listado**: `not-compared` | `matches` | `amount-differs` |
`different-asset`. **Nunca decide**; se reporta.

**Reglas de comportamiento, no negociables:**

1. Evaluar **no** gasta. `recordSpend` es una llamada aparte, después de que la
   liquidación resolvió.
2. La política **no** se ensancha desde dentro de una evaluación. No expongan un
   método que lo permita.
3. **No** pidan confirmación humana si la política ya cubre la operación.
4. Un activo distinto **no** es el mismo precio: no comparen números entre
   activos.
5. `validUntil` se lee de `extensions["offer-receipt/1"].info.validUntil`, en
   segundos Unix. Ausente = sin vencimiento declarado. Ilegible = ausente,
   **nunca** cero.
6. `validUntil == now` todavía vale: es el último instante en que la oferta está
   en pie.
7. Un `accepts` con una entrada ilegible **conserva** las legibles y cuenta las
   otras por nombre de esquema.

### Decisiones de la sección 14 tomadas acá

- **Soporte de `offer-receipt` por SDK/red**: se adopta la **clave versionada y
  la vigencia**, no la firma. La firma necesita un modelo de identidad del
  vendedor que todavía no existe; adoptar media extensión y decirlo es más
  reversible que inventar la otra mitad.
- **Perfil requerido para vincular input/revisión/consumo**: la extensión
  versionada es el lugar; el perfil queda sin definir a propósito hasta que haya
  un vendedor real que lo necesite, porque un perfil sin implementador es una
  suposición con número de versión.
- **Retención de cotizaciones personalizadas**: nada nuevo se persiste en esta
  fase. La política vive en memoria del comprador y no se escribe.

### Qué queda para P4

La evidencia comercial en dx402 (sección 10), que es donde anuncio, oferta,
máximo autorizado, cobro efectivo y entrega se relacionan — y donde los puntos 2
y 7 de esta sección encuentran su lugar natural, porque ahí ya hay una firma y
una identidad con las que trabajar.
