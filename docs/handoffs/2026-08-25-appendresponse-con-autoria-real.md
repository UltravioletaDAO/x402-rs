---
date: 2026-08-25
tags:
  - type/handoff
  - domain/identity
  - domain/blockchain
  - priority/p0
status: active
aliases:
  - appendResponse con autoría real
  - El último camino donde firmábamos nosotros
related-files:
  - src/erc8004/relay_v4.rs
  - src/handlers.rs
  - docs/handoffs/2026-08-25-v4-cableado-y-el-supersedable.md
---

# `appendResponse` ya no lo firmamos nosotros. Era el último.

> **Para:** Execution Market y Karma Kadabra
> **De:** el equipo del Facilitator (`x402-rs`)
> **Estado:** v1.97.0 — SDKs py **0.69.0** / npm **2.74.0**.

## 1. Qué cerró

De las tres escrituras que el `ReputationRegistry` acepta de cualquiera, ésta era
la última que seguía saliendo a nombre nuestro:

| escritura | autor on-chain | desde |
|---|---|---|
| `giveFeedback` | el rater | v1.74.0 (rail), v1.96.0 (EIP-712) |
| `revokeFeedback` | admin-only, fail-closed | v1.74.0 |
| **`appendResponse`** | **el responder** | **v1.97.0** |

`POST /feedback/response` no tenía autenticación y el registry graba `msg.sender`
como `responder`, así que **un POST sin credenciales nos hacía firmar**. No
destruye reputación como podía el revoke; ata nuestra identidad on-chain al
contenido de un tercero, que es su propia clase de mal y no tenía por qué seguir
abierta ahora que v4 la puede cerrar.

Dos rutas nuevas, mismo diseño que ya conocen:

```
POST /feedback/response/evm/prepare   -> typedData (EIP-712), deadline, nonce
POST /feedback/response/evm/submit    -> relaya; la tx tipo 4 va A LA DIRECCIÓN
                                          DEL RESPONDER, así que el registry lo
                                          ve a él y nosotros pagamos el gas
```

## 2. Es v4 y no hay fallback, a propósito

El delegate v3 acepta **exactamente dos selectores** y `appendResponse` no es
uno. Así que una red todavía en v3 recibe:

```
400  relay_response_needs_v4
```

**No cae al camino viejo.** Un fallback silencioso acá sería volver a firmar
nosotros justo cuando el llamador pidió lo contrario — el mismo tipo de error
correcto-pero-callado que nos costó una semana con el sobre EIP-191.

Consecuencia práctica: **base-sepolia no lo tiene**, porque sigue en v3. Es el
argumento más concreto para que le desplieguen v4: hoy no hay ningún testnet
donde se pueda probar este rail, y es donde conviene equivocarse.

## 3. Los dos campos que EM no había incluido, y por qué importan

`RelayedAppendResponse` lleva `clientAddress` y `feedbackIndex` **dentro del
struct firmado**. Ya se lo habíamos señalado cuando revisamos el struct, y con la
ruta construida se ve mejor por qué:

- sin **`feedbackIndex`**, una firma responde **cualquiera** de las
  calificaciones de ese cliente;
- sin **`clientAddress`**, responde ese índice de **cualquier** cliente.

En los dos casos el responder estaría firmando algo que no puede ver, que es
exactamente lo que EIP-712 viene a impedir. Los dos están fijados con tests que
mueven un campo y exigen que la firma deje de verificar.

## 4. Una afirmación falsa que veníamos repitiendo

Los dos SDKs decían, textualmente:

> *"Allows agents to respond to feedback they received. **Only the agent
> (identity owner) can append responses.**"*

**Es falso**, y se verificó on-chain el 2026-08-18: el registry acepta
`appendResponse` de **cualquier dirección**. No hay chequeo de dueño ni en el
endpoint ni en el contrato.

Importa más que una errata: quien leía esa línea creía que había una barrera que
nunca existió, y por lo tanto que no hacía falta ninguna. Corregido en py 0.69.0
y npm 2.74.0, junto con la deprecación.

Si alguno de ustedes tiene esa frase copiada en su documentación, conviene
borrarla ahí también.

## 5. Qué le toca a cada uno

### Execution Market

- **Suban los SDKs** (0.69.0 / 2.74.0) si van a exponer respuestas firmadas.
- **v4 en base-sepolia**, cuando puedan. Es lo único que impide probar este rail
  en un testnet.
- Nada más: el struct que desplegaron ya lo soporta, no hay contrato que tocar.

### Karma Kadabra

- Si sus agentes responden calificaciones, `prepare_relayed_response()` +
  `submit_relayed_response()` firman con **llave propia** igual que el rail de
  feedback: `Account.unsafe_sign_hash(prep.digest)` sobre el digest, o el
  `typed_data` si prefieren pasarlo por una wallet.
- Sigue valiendo lo del handoff anterior: en el próximo `prepare` sus raters
  delegados a v3 van a ver `delegated: false` y eso es correcto.

## 6. Estado del corte, completo

```
giveFeedback    autoría real  ✅  y con un rating real on-chain
revokeFeedback  admin-only    ✅  fail-closed desde v1.74.0
appendResponse  autoría real  ✅  v1.97.0, este documento
```

De nuestro lado no queda ninguna escritura al `ReputationRegistry` que salga a
nombre del facilitador **cuando existe la alternativa**. Los caminos viejos
siguen abiertos —son los únicos donde no hay delegate, o donde el delegate es
v3— y gritan `[WARN] DEPRECATED` en cada llamada. Avísennos cuando hayan migrado
y los cerramos.

Lo único que seguimos sin ver es volumen. El rail funciona, está verificado y
tiene un rating encima; uno. El sweep de KK es lo que va a decir si aguanta.

---

```bash
curl -s https://facilitator.ultravioletadao.xyz/version   # 1.97.0

# v4: devuelve typedData
curl -s -X POST https://facilitator.ultravioletadao.xyz/feedback/response/evm/prepare \
  -H 'content-type: application/json' -d '{
    "x402Version":1, "network":"base",
    "responder":"<EOA del agente>",
    "agentId":"2106", "clientAddress":"<wallet del rater>", "feedbackIndex":1,
    "responseUri":"ipfs://QmResponse"
  }' | jq '{delegate, primaryType: .typedData.primaryType, delegated}'

# v3: se niega nombrando la causa, no cae al camino viejo
curl -s -X POST https://facilitator.ultravioletadao.xyz/feedback/response/evm/prepare \
  -H 'content-type: application/json' -d '{
    "x402Version":1, "network":"base-sepolia",
    "responder":"0x0000000000000000000000000000000000000001",
    "agentId":"1", "clientAddress":"0x0000000000000000000000000000000000000002",
    "feedbackIndex":1, "responseUri":"u"
  }'
# espera 400 {"error":"relay_response_needs_v4"}
```
