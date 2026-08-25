---
date: 2026-08-25
tags:
  - type/handoff
  - domain/identity
  - domain/blockchain
  - priority/p0
status: active
aliases:
  - Calificar convierte al calificador en smart account
  - Paridad de 7702 cerrada en TypeScript
related-files:
  - docs/handoffs/2026-08-25-confirmacion-struct-eip712-v4.md
  - docs/handoffs/2026-08-25-actualicen-el-sdk-firma-del-rail.md
---

# El corte está hecho. Y calificar convierte al calificador en una smart account — para siempre.

> **Para:** Karma Kadabra y Execution Market
> **De:** el equipo del Facilitator (`x402-rs`)
> **Responde a:** `HANDOFF_FACILITADOR_RAIL_FIRMADO_2026-08-25.md` (KK)
> **Estado:** SDK de TypeScript al día (**2.72.0**); el hueco de paridad de 7702
> quedó cerrado.

## 1. Verificamos el rating de KK on-chain, y confirma todo

No lo dimos por bueno: leímos la tx de Base directamente. Cada pieza del diseño
aparece en ella.

```
tx     : 0x6e7cbe77d4de81d605d76fcf001de7056af9b682d60fc14f7d34c0021e142482
status : 0x1                     bloque 50442319
tipo   : 0x4                     <- EIP-7702
from   : 0x1030…13c7             <- nosotros, pagando el gas
to     : 0x09c3…beee             <- LA CUENTA DEL RATER, no el registry

log del ReputationRegistry (0x8004BAa1…9b63):
  topic2 (clientAddress) = 0x09c32b8fc0a94a1eed424499a42180e29667beee
  == rater            True
  == nuestra wallet   False
```

Ese `to` apuntando a la cuenta de jonesh5 es el truco entero. Después de 1.384
feedbacks atribuidos a nuestra wallet, el primero que no lo está ya existe.
Gracias a los dos equipos.

## 2. La consecuencia que ninguno de los tres handoffs escribió

**Calificar cambia el tipo de cuenta del calificador. Para siempre, y para todos
sus pagos, no sólo para el rating.**

Un worker que emite un rating por el rail queda delegado al `FeedbackDelegate`.
Desde ese momento su EOA **tiene código**, y eso cambia el comportamiento de todo
lo que ramifica por `code.length > 0`: USDC, `SignatureChecker`, Permit2,
Seaport, cualquier order book, y los contratos de escrow.

KK ya se topó con una instancia — su wrap de pre-auth — y la arregló bien. Lo que
nos preocupa es la forma general: **el que se tope con la próxima no va a ver
"alguien calificó"**. Va a ver "este pago dejó de funcionar", en otro servicio,
días después, sin nada que lo conecte con el rating. Es exactamente el modo de
falla que veníamos teniendo toda la semana: correcto, silencioso, y atribuible a
la cosa equivocada.

**Escríbanlo donde lo vaya a leer quien depure ese pago.** Y anoten la salida,
porque existe y no es obvia:

> Un rater puede **des-delegarse** firmando una authorization EIP-7702 hacia
> `address(0)`. La cuenta vuelve a ser un EOA plano. No hace falta pedirle nada a
> nadie — es una tx tipo 4 más.

Vale sobre todo para los 6 agentes de Paybox: acordamos no re-delegarlos, y ese
acuerdo se sostiene sólo si nadie los pasa por el rail por descuido. Nuestro
facilitador los rechaza con `Foreign` → 400, que es la red de seguridad, pero es
la última, no la primera.

## 3. Cerramos el hueco de paridad: SDK de TypeScript 2.72.0

Auditando el fix de KK encontramos que **el SDK de TypeScript no tenía nada de
7702**. Ni módulo, ni detección, ni gate por target — `escrow-preauth.ts` firmaba
siempre igual.

O sea: no tenía el bug del over-wrap porque nunca envolvió, pero tampoco podía
firmar por una cuenta delegada a un SMA de Alchemy. Hoy no muerde porque esos 6
están fuera del rail, pero era una asimetría real entre dos SDKs que se venden
como equivalentes.

**Portado en 2.72.0**, siguiendo la implementación de Python:

| pagador | firma |
|---|---|
| EOA plano | EIP-712 ordinaria |
| delegado a un SMA de Alchemy | replay-safe hash + sobre de la cuenta |
| delegado al `FeedbackDelegate` (o cualquier 1271 plano) | EIP-712 ordinaria |
| delegación **DESCONOCIDA** con resolver puesto | **lanza** |

`buildEscrowPreAuth` acepta ahora un `delegationResolver` opcional. **Sin
resolver el comportamiento no cambia**, así que no rompe a nadie que ya lo esté
usando.

Tres decisiones que copiamos de Python a propósito:

- **El wrap se decide por el TARGET, no por "está delegado".** Es el fix de KK, y
  vale igual acá.
- **`null` es DESCONOCIDO y nunca "no delegado".** Una cadena ilegible no es un
  veredicto. Colapsar los dos es como el bug original sobrevivió ocho días.
- **Un string que no es una address es basura, y basura es DESCONOCIDO.** No se
  firma sobre un valor que nadie validó.

Los vectores de los tests **los genera el SDK de Python**, no este port. Un port
comparado sólo contra sí mismo no prueba nada — ya nos pasó con tres hashes SEAL
inventados que pasaron CI durante meses.

19 tests nuevos, 433 en total, typecheck y lint limpios.

## 4. Qué le toca a cada uno

### Karma Kadabra

- **Subir el SDK de TypeScript a 2.72.0** si alguna superficie suya firma desde
  ahí. Su flota Python ya está en 0.67.0 y no necesita nada más.
- **Para el sweep de los 465**: su plan de filtrar avalanche+skale (200 de 1.405)
  es correcto. Avalanche rechaza el tipo de transacción y SKALE tiene EVM anterior
  a Shanghai — 7702 no puede aterrizar en ninguna de las dos, y nuestro
  `/feedback/evm/prepare` las rechaza con 400 explícito.
- **Avísennos el volumen** cuando corran el sweep. Vamos a estar mirando la caída
  de `POST /feedback` contra la subida del rail firmado; es la primera vez que
  vamos a poder medir el corte en vez de anunciarlo.
- Su allowlist propia de delegates, validada antes de firmar la authorization, es
  la decisión correcta y nos gustaría que quede así. Cuando salga v4 les pasamos
  las direcciones nuevas por el canal de siempre, con la verificación on-chain
  hecha de nuestro lado.

### Execution Market

- **Subir los dos SDKs**: 0.67.0 (Python) y 2.72.0 (TypeScript).
- **El struct EIP-712 está confirmado** — `2026-08-25-confirmacion-struct-eip712-v4.md`,
  con los tres typehashes computados y un vector de dominio. Desplieguen cuando
  quieran: vamos a servir los dos digests en paralelo, elegidos por la versión que
  el discriminador detecte en la cadena, así no hace falta coordinar la hora.
- **La trampa del v4, otra vez porque cuesta cara**: no cacheen el
  `DOMAIN_SEPARATOR` en el constructor. Bajo 7702 `address(this)` es la cuenta del
  rater, distinta en cada llamada; un separador congelado en el deploy llevaría la
  dirección del delegate y **todas** las firmas recuperarían a un desconocido. Es
  el patrón por defecto de OpenZeppelin `EIP712`, así que hay que salirse de él a
  propósito.
- **Cuando desplieguen v4**, necesitamos poder distinguirlo de v3 tan barato como
  hoy: un interface id nuevo en `supportsInterface`, o una constante `VERSION()`.
  Cualquiera sirve; díganos cuál.
- Y aprovechen para **anunciar ERC-1271** (`0x1626ba7e` sigue devolviendo `false`
  aunque `isValidSignature` esté implementado). Es una línea.

## 5. Estado

| | |
|---|---|
| Facilitator | v1.95.0, `signingPayload` servido |
| PyPI `uvd-x402-sdk` | 0.67.0 (el fix de KK) |
| npm `uvd-x402-sdk` | **2.72.0** (paridad de 7702) |
| Primer rating con autoría real | ✅ verificado on-chain |
| Struct EIP-712 v4 | ✅ confirmado, esperando su deploy |
| Fase 2 del gate (`ERC8004_REQUIRE_PROOF`) | ⏳ esperando el volumen del sweep |

Lo único que seguimos sin ver es tráfico. El rail funciona, está verificado y
tiene un rating real encima — pero uno. Cuando el sweep corra, ahí sabremos si
aguanta.
