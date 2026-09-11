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

## Dos cosas que estaban construidas y no conectadas

Las encontré revisando mi propio trabajo mientras corría el CI, antes del merge.
Las dos eran de la misma clase: la función existía y el camino real no la
llamaba, que es peor que no tenerla, porque los tests de la pieza pasan y la
capacidad no está.

- **El middleware no pasaba las `extensions` del desafío.** Llamaba a
  `build_payment_header(&challenge.accepts)`, así que el mapa donde el vendedor
  declara `validUntil` se tiraba antes de que la política pudiera leerlo. El
  vendedor lo declaraba, la política sabía comprobarlo, y entre las dos no había
  cable. Ahora usa `build_payment_header_in` con el mapa del desafío.
- **La negativa que nombra los esquemas no se alcanzaba.** Un desafío con todas
  las ofertas ilegibles daba `NoSuitablePaymentMethod { accepts: [] }` — que es
  justo el mensaje que manda a alguien a buscar un bug en su propio código. Ahora
  `handle()` la detecta antes de intentar construir nada.

Dos tests nuevos, y los dos afirman el camino de punta a punta y no la pieza.

## Lo que corrigió la revisión de seguridad

Un revisor independiente dejó el PR en CONDITIONAL con cuatro hallazgos. Los
cuatro eran ciertos; dos cambian el comportamiento del dinero.

**P1-1, la vigencia muerta en el camino automático.** El revisor leyó el primer
push, donde `handle` llamaba a `build_payment_header(&accepts)` y tiraba las
`extensions`. El segundo push ya lo había cableado
(`eef4455c:465` → `d5b5b52a`), pero **el punto de fondo era correcto y yo no lo
tenía cubierto**: no había ningún test que ejerciera la decisión de punta a
punta, así que el cable pudo faltar un commit entero con todos los tests en
verde. Ahora la decisión entera vive en `pay_for_challenge`, que toma **el
desafío completo y nunca sus partes** — una firma que toma las partes invita a
volver a tirar el mapa — y hay cuatro tests que la manejan. Probado en rojo:
restaurando el cableado viejo, `an_expired_offer_is_refused_on_the_path_the_middleware_takes`
falla.

**P1-2, la política era abierta por activo.** Los techos son un mapa, y un mapa
no tiene opinión sobre una clave que no contiene: un presupuesto en USDC **no
era un presupuesto** para ningún otro token. El mismo recurso cotizado en algo no
enumerado pasaba de largo, y el firmante EVM lo hubiera firmado, porque toma el
dominio EIP-712 del `extra` del propio vendedor y firma para un token y una red
que nunca vio.

Ahora un activo sin techo declarado se rechaza (`asset-not-budgeted`), y la
comprobación corre **antes** de los techos, porque a quien llama hay que decirle
"presupuestá ese activo", no "subí un techo que no existe". `PurchasePolicy::new()`
deniega; `PurchasePolicy::permissive()` es lo que sostiene el middleware cuando
nadie escribió una política, y existe sólo por retrocompatibilidad: este crate no
tenía presupuesto antes de P3 y encenderlo en silencio rechazaría pagos que hoy
funcionan. La asimetría es a propósito — quien se sienta a **escribir** una
política merece el default seguro.

**P2-1, `to_lowercase()` sobre la dirección entera.** Correcto para hex y
destructivo para base58: en Solana y XRPL la caja es un símbolo, no una grafía.
Bajar una dirección base58 no produce la misma dirección escrita distinto, produce
una cadena que no es una dirección — así que una lista blanca escrita con la
grafía del vendedor no coincidiría nunca y todo pago legítimo a ese payee se
rechazaría. Y en la dirección peligrosa, dos direcciones base58 distintas pueden
plegarse a la misma minúscula, lo que dejaría entrar a una que nadie puso en la
lista. Ahora se canonicaliza **por familia**: hex se pliega, el resto se compara
exacto.

**P2-2, el tope del conjunto compartido.** DynamoDB evalúa la condición **antes**
del `ADD`, así que `size < cap` dejaba pasar una escritura que después sumaba
hasta cien valores: una réplica podía dejar el conjunto en `cap + batch - 1`, y
tres compitiendo más lejos. El margen ahora es el lote que se está por agregar, y
la resta es saturante: un tope menor que un lote da cero, que rechaza la
escritura, que es la dirección segura para un límite.

**Y el test tautológico.** `the_key_has_one_definition` comparaba un `pub use`
consigo mismo y no podía fallar nunca. Ahora fija el literal y que la versión esté
en la clave, que es la propiedad que importa.

## Una nota sobre el parseo de `accepts`

`accepts` pasó a `#[serde(default)]` en la forma de cable. Un desafío **sin** el
campo ya no es un error de deserialización: es una lista vacía. Combinado con la
tolerancia por oferta, eso significa que el comprador ahora distingue tres cosas
que antes eran un solo error: "el vendedor no mandó ofertas", "mandó ofertas que
no sabemos leer" (con sus nombres de esquema) y "mandó ofertas pagables". La
primera sigue terminando en `NoSuitablePaymentMethod`, la segunda en
`no-readable-offer`.

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
| `onlyPay[]` | lista de direcciones | destinatarios permitidos, **canonicalizados por familia** |
| `allowUnlistedAssets` | booleano, **false por defecto** | si se puede pagar un activo sin techo declarado |

**Orden de evaluación** (la primera que falla es la que se reporta):
`no-readable-offer` → `offer-expired` → `recipient-not-permitted` →
**`asset-not-budgeted`** → `per-payment-limit` → `cumulative-limit`.

**Códigos de error**, vocabulario cerrado, en kebab, para ramificar sin parsear
inglés: los cinco de arriba. Cada uno lleva los números que lo causaron
(`requested`, `allowed`, `spent`, `wouldTotal`, `asset`, `payTo`, `validUntil`,
`now`, `offered[]`).

**Comparación con el listado**: `not-compared` | `matches` | `amount-differs` |
`different-asset`. **Nunca decide**; se reporta.

> **Sin `with_policy` no hay reja de activo.** El middleware sostiene
> `PurchasePolicy::permissive()` cuando nadie le pasó una política, y permissive
> **permite cualquier activo**, incluido uno que nadie presupuestó. Es
> retrocompatibilidad deliberada — este crate no tenía presupuesto antes de P3 —
> y significa que un integrador que nunca llama `with_policy` **no tiene la reja
> de la regla 4b**. Si su SDK expone un cliente sin política, dígalo con estas
> palabras en su documentación, no como nota al pie.
>
> **Nota sobre el `0x`.** Sólo se trata como hex lo que empieza con `0x`. Una
> entrada de allowlist escrita como hex pelado se compara exacta y **nunca
> coincidirá** con el `payTo` de una oferta, que siempre llega con prefijo: el
> pago se rechaza con `recipient-not-permitted`. Falla del lado seguro y es una
> trampa; no la normalicen por el usuario, porque agregar el prefijo es adivinar
> la familia y un base58 pelado no se distingue de un hex pelado mirándolo.

**Reglas de comportamiento, no negociables:**

1. Evaluar **no** gasta. `recordSpend` es una llamada aparte, después de que la
   liquidación resolvió.
2. La política **no** se ensancha desde dentro de una evaluación. No expongan un
   método que lo permita.
3. **No** pidan confirmación humana si la política ya cubre la operación.
4. Un activo distinto **no** es el mismo precio: no comparen números entre
   activos.
4b. **Denieguen por defecto un activo sin techo.** Un presupuesto en un token no
   es un presupuesto en otro, y el firmante acepta la red y el token que diga el
   `extra` del vendedor. Si exponen un modo permisivo, que haya que pedirlo por
   nombre.
4c. **Canonicalicen la dirección por familia, no con `toLowerCase()`.** Hex se
   pliega; base58 (Solana, XRPL) se compara exacto. Plegar base58 rechaza pagos
   legítimos y, peor, puede admitir una dirección que nadie puso en la lista.
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
