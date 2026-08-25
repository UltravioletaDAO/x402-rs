---
date: 2026-08-25
tags:
  - type/handoff
  - domain/identity
  - domain/blockchain
  - priority/p0
status: active
aliases:
  - v4 cableado, typedData servido
  - El estado Supersedable
related-files:
  - src/erc8004/relay_v4.rs
  - src/erc8004/relay.rs
  - docs/handoffs/2026-08-25-confirmacion-struct-eip712-v4.md
---

# v4 cableado. Y encontramos algo que los habría dejado a todos afuera.

> **Para:** Execution Market (y Karma Kadabra, por §3)
> **De:** el equipo del Facilitator (`x402-rs`)
> **Responde a:** `2026-08-25-v4-desplegado-tabla-para-el-facilitador.md`
> **Estado:** v1.96.0 — `typedData` servido, versión detectada por cadena.

## 1. Verificamos las 8, y su vector reproduce exacto

Ninguna dirección se copió. Cada una leída de su propia cadena en dos RPC:
`VERSION()` = 4, registry correcto y con código, `supportsInterface(0x378a0c90)`
= true, y **ERC-1271 ahora sí anunciado** (`0x1626ba7e` = true) — gracias por
tomar ese pedido. 5857 bytes en las ocho.

Dos verificaciones independientes que salieron limpias:

**El interface id lo derivamos, no lo copiamos.** Calculamos el XOR de los tres
selectores partiendo de *nuestra* especificación del struct y da `0x378a0c90`
exacto. Eso confirma de paso que compilaron los structs tal como los
confirmamos, y nos dio la forma del calldata sin preguntar.

**Su vector reproduce con nuestra fórmula, los dos valores:**

```
domainSeparator    : 0xae792427…d09d   MATCH
giveFeedbackDigest : 0xa7a2a62d…f6e1   MATCH
```

Los dos están pinneados en un test (`the_vector_matches_the_deployed_v4_contract`).
Si alguno de los dos lados mueve un campo, falla acá antes de que alguien firme.

## 2. Lo que su §9 dio por sentado sobre nosotros, y no era así

Escribieron: *"conviene que eso se trate como `delegated: false` y no como error
— **y ya lo hacen así** para el estado `Foreign`"*.

**No lo hacíamos.** `delegation_state()` devolvía `Foreign` para cualquier cuenta
delegada a algo distinto del delegate exacto que servimos, y `prepare` lo
**rechazaba con 400**.

O sea: el día que cambiáramos la tabla a v4, jonesh5 —delegado a v3— habría
recibido un 400 y **no habría podido volver a calificar nunca**. Hoy es una
cuenta. Si el sweep de KK hubiera corrido antes, habrían sido cientos, y el
síntoma habría sido "el rail dejó de andar" sin nada que lo conectara con un
cambio de tabla.

**Y el arreglo obvio era peligroso.** Tratar todo `Foreign` como
`delegated: false` habría empezado a pedirle re-delegación a los **6 agentes de
Paybox** delegados al SMA de Alchemy — justo lo que los tres equipos acordamos no
tocar, porque les rompe las money-ops gasless. Ese rechazo es la barrera, no un
descuido.

Así que separamos dos cosas que estaban colapsadas en una:

| la cuenta está delegada a… | estado | qué pasa |
|---|---|---|
| el delegate actual | `Delegated` | relay directo |
| **un delegate NUESTRO de otra versión** | **`Supersedable`** | `delegated: false` → firma authorization nueva |
| cualquier otra cosa | `Foreign` | 400, y así se queda |

**Lo decidimos por comportamiento, no por una lista de direcciones viejas.** Con
CREATE una dirección no identifica un contrato entre cadenas — y ustedes acaban
de demostrarlo: `0xf670C69B` es la v1 de polygon **y** la v4 de arbitrum;
`0xe25cF9B9` es la v1 de celo **y** la v4 de bsc. Una lista plana de "direcciones
viejas" habría rechazado deploys perfectamente buenos. Lo que hacemos es
preguntarle al target si responde `REPUTATION_REGISTRY()` con el registry de
*esa* cadena: un FeedbackDelegate de cualquier versión sí; un SMA de Alchemy
revierte.

Cuesta un `eth_call` extra y sólo en el camino `Foreign`, que es raro.

## 3. Para Karma Kadabra, en una línea

**El rater de su primer rating va a poder volver a calificar.** Va a ver
`delegated: false` en su próximo `prepare` aunque la cuenta tenga código —
correcto, porque el código apunta a v3 y hace falta una authorization nueva hacia
v4. Es el flujo de la primera vez, otra vez, y su script ya lo maneja.

Y su allowlist propia de delegates sigue siendo la decisión correcta: las
direcciones v4 verificadas están en §1 y en `src/erc8004/relay.rs`.

## 4. Cómo quedó servido

**Las dos versiones en paralelo, elegidas por lo que hay desplegado en cada
cadena, leído en cada request.** No hay flag, no hay lista por release:

```
supportsInterface(0x378a0c90) == true   -> v4: se sirve `typedData`
supportsInterface(0x150b7a02) == true   -> v3: se sirve `digest` + `signingPayload`
ninguno (revierte)                      -> v1: se rechaza
```

Consecuencia práctica para ustedes, que es lo que pidieron: **una cadena empieza
a firmar EIP-712 en el momento en que despliegan v4 ahí, sin un deploy nuestro en
el medio.** base-sepolia sigue en v3 y sigue recibiendo `digest` +
`signingPayload`; las ocho mainnets ya reciben `typedData`.

En v4 **no** mandamos `signingPayload`, y no es un olvido: `signTypedData` no
tiene sobre que aplicar dos veces, así que el campo que existía sólo para
anticipar ese sobre no tiene nada que hacer. Un cliente que en v4 busque
`signingPayload` recibe ausente y debe caer al `typedData`, nunca al `digest`.

El `submit` también cambia por versión: en v4 el calldata externo es
`relayGiveFeedback(struct, signature)`, así que **el contrato arma el calldata
del registry desde exactamente lo que se firmó y se mostró**. No hay un `bytes
data` que pueda diferir.

## 5. Lo que fijamos con tests

- **Los tres typehashes**, contra los valores que les confirmamos antes de que
  desplegaran.
- **El vector contra el contrato desplegado** (§1). Comparado con la cadena, no
  con nosotros mismos.
- **El interface id derivado** del XOR de nuestros tres selectores.
- **Que ningún parámetro de `giveFeedback` viaje fuera del type string** — los
  once, incluido `valueDecimals`, que era el que faltaba en la propuesta
  original.
- **Que dos raters distintos den dominios distintos**, y el mismo rater en
  cadenas distintas también. Es el test que un `DOMAIN_SEPARATOR` cacheado
  rompe al instante, y nos gustó ver que ustedes tienen el equivalente.
- **Que un campo movido invalide la firma**: `valueDecimals`, `tag1`, `endpoint`,
  un `value` negativo y el `deadline`, uno por uno.

## 6. Nos falta lo que dijeron

Cuando quieran, comparamos el primer `typedData` real contra el vector de su §3
antes de que alguien firme. De nuestro lado ya está pinneado, así que es un
chequeo de minutos.

Y dos cosas menores que quedan pendientes de ustedes, sin urgencia:

- **base-sepolia sin v4.** Mientras siga en v3 no podemos probar EIP-712 en un
  testnet, que es donde conviene equivocarse.
- **`appendResponse` sigue anónimo de nuestro lado.** v4 ya lo admite como
  selector relayable, así que la autoría real ahí ya tiene dónde apoyarse; es
  trabajo nuestro y está en la cola.

## 7. Gracias por la nota de método

> *"la afirmación no era una suposición, era un hecho **con fecha de
> vencimiento** y la escribimos sin la fecha"*

Nos aplica igual, y más de una vez esta semana. Vale como regla compartida: un
hecho medido lleva la fecha de la medición, y si el documento afirma algo sobre
el estado del mundo, se vuelve a medir antes de mandarlo. A ustedes les costó un
`eth_getCode`; a nosotros, no haberlo hecho nos habría costado el 400 de §2.

---

```bash
curl -s https://facilitator.ultravioletadao.xyz/version   # 1.96.0

curl -s -X POST https://facilitator.ultravioletadao.xyz/feedback/evm/prepare \
  -H 'content-type: application/json' -d '{
    "x402Version":1, "network":"base",
    "feedback":{"agentId":"2106","rater":"<EOA>","value":95,"valueDecimals":0,
                "tag1":"quality","tag2":"api","endpoint":"e","feedbackUri":"u"}
  }' | jq '{delegate, typedData: (.typedData.primaryType), signingPayload, delegated}'
# base: typedData = "RelayedGiveFeedback", signingPayload ausente
# base-sepolia: typedData ausente, signingPayload presente
```
