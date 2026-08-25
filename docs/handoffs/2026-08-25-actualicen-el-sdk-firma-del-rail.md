---
date: 2026-08-25
tags:
  - type/handoff
  - domain/identity
  - priority/p0
status: active
aliases:
  - Actualicen el SDK — firma del rail
  - Qué le toca a cada equipo
related-files:
  - docs/handoffs/2026-08-25-el-sobre-eip191-era-nuestro-tambien.md
---

# Actualicen el SDK. Qué le toca a cada uno.

> **Para:** Karma Kadabra y Execution Market
> **De:** el equipo del Facilitator (`x402-rs`)
> **Contexto largo:** `2026-08-25-el-sobre-eip191-era-nuestro-tambien.md`. Este
> documento es sólo **qué tienen que hacer ustedes**.

## Lo que hay que subir

| | versión nueva | antes |
|---|---|---|
| Facilitator | **v1.95.0** | 1.94.0 |
| PyPI `uvd-x402-sdk` | **0.66.0** | 0.65.0 |
| npm `uvd-x402-sdk` | **2.71.0** | 2.70.0 |

```bash
pip install --upgrade uvd-x402-sdk    # 0.66.0
npm install uvd-x402-sdk@2.71.0
```

**Nada de esto rompe lo que ya tienen andando.** Es aditivo: un campo nuevo en la
respuesta de `prepare`, y documentación corregida.

## Qué era el problema, en tres líneas

`prepare` devuelve un `digest` que **ya lleva el sobre EIP-191**. El facilitador
recupera contra él como prehash, sin agregar nada. `personal_sign` aplica el
sobre por su cuenta — así que firmarle el `digest` a una wallet lo envuelve **dos
veces** y recupera a un desconocido. La firma sale bien formada, el request sale
bien formado, y el único síntoma es `relay_bad_signature`.

Nuestros dos SDKs documentaban exactamente ese camino. Corregido en 0.66.0 /
2.71.0.

## Lo nuevo: `signingPayload`

`prepare` ahora devuelve, además del `digest`, el mismo hash **sin** el sobre:

```jsonc
{
  "digest":         "0x…",   // firmar CRUDO (prehash)
  "signingPayload": "0x…",   // personal_sign ESTE
}
```

Y la relación, para que puedan verificar uno contra el otro sin reconstruir nada:

```
keccak256("\x19Ethereum Signed Message:\n32" || signingPayload) == digest
```

Los tres caminos:

| quién firma | qué firma | cómo |
|---|---|---|
| agente con llave propia | `digest` | prehash — `unsafe_sign_hash`, `sign({hash})` |
| wallet de navegador / móvil / custodio | `signingPayload` | `personal_sign` / `signMessage` |
| ✗ **nadie** | `digest` | `personal_sign` — recupera a un desconocido |

---

# Karma Kadabra

**No estaban rotos, y no cambien cómo firman.**

Ustedes firman con llave propia (`Account.unsafe_sign_hash(digest)`). Ese camino
nunca estuvo afectado, funciona hoy contra producción y sigue siendo el correcto.
`signingPayload` es para wallets, no para ustedes.

Qué hacer:

1. **Subir a `uvd-x402-sdk` 0.66.0.** No es urgente para su camino, pero el
   README de 0.65.0 les da instrucciones equivocadas si algún día agregan una
   superficie con wallet, y el campo nuevo queda tipado.
2. **El rating de prueba no está bloqueado.** Pueden emitirlo ya:

```python
from eth_account import Account
from uvd_x402_sdk import Erc8004Client

async with Erc8004Client() as client:
    prep = await client.prepare_relayed_feedback(
        network="base", agent_id=..., rater=rater_address,
        value=95, score=95, tag1="quality",
    )
    # Llave propia: PREHASH sobre el digest. NO Account.sign_message.
    signature = Account.unsafe_sign_hash(prep.digest, rater_key).signature.hex()

    result = await client.submit_relayed_feedback(
        network="base", agent_id=..., rater=rater_address, value=95,
        score=95, tag1="quality",
        deadline=prep.deadline, nonce=prep.nonce, signature=signature,
        authorization=None if prep.delegated else authorization,
    )
```

3. **La primera vez que un rater califica**, `prep.delegated` viene `False` y hay
   que mandar también la autorización EIP-7702 sobre
   `(prep.chain_id, prep.delegate, prep.account_nonce)`. De la segunda en
   adelante va sin ella.

4. **Gracias.** Esto lo encontraron ustedes leyendo el código antes de emitir
   nada. Nosotros habíamos verificado delegates en dos RPC por cadena, comparado
   bytecode byte a byte y medido el digest contra el contrato desplegado —
   ninguna de esas verificaciones podía encontrarlo, porque todas miraban la
   cadena y el bug estaba en el borde entre nuestro campo y una wallet.

---

# Execution Market

Ustedes tienen las dos superficies, así que les toca en los dos lenguajes.

Qué hacer:

1. **Subir los dos SDKs**: `uvd-x402-sdk` 0.66.0 (el `mcp_server`) y 2.71.0
   (dashboard, móvil, `em-plugin-sdk`).

2. **Pueden borrar la reconstrucción desde `data`.** Su arreglo `signing_payload`
   —reconstruir el hash previo al sobre y envolverlo para comparar contra nuestro
   digest— es correcto y su guard es la parte que lo hace seguro publicar. Pero
   ahora servimos el valor, así que esa reconstrucción es una segunda
   implementación del preimage que ya no hace falta mantener.

   **Conserven el guard, cambiándole la fuente**: en vez de reconstruir desde
   `data`, verifiquen la relación entre los dos campos que ya reciben.

```python
from eth_hash.auto import keccak

payload = bytes.fromhex(prep.signing_payload[2:])
assert keccak(b"\x19Ethereum Signed Message:\n32" + payload).hex() == prep.digest[2:], \
    "el facilitador cambió el preimage: NO servir un payload que no verifica"
```

   Si algún día divergen, es porque algo se movió de nuestro lado, y lo van a ver
   de inmediato en vez de servir un payload que firma al vacío.

   No corre prisa: su versión y la nuestra dan el mismo valor, así que conviven.

3. **En las superficies de wallet**, el cambio es una línea:

```typescript
// dashboard / em-mobile
const signature = await walletClient.signMessage({
  account,
  message: { raw: prep.signingPayload! },   // NO prep.digest
});
```

4. **`data` se queda.** Su punto 1 pedía aviso si lo quitábamos por peso: no hay
   tal plan, y ahora hay una razón más para conservarlo — es lo que le permite a
   un cliente verificar que el calldata que firma es el que dice ser, sin
   confiar en nuestro `digest`. Queda como parte del contrato.

5. **Lo que sigue bloqueado en ustedes, sin cambios**: el struct EIP-712.
   Recordamos los dos puntos porque son los que les impiden escribir el contrato:

   - **Deja cuatro campos fuera del digest**: `valueDecimals`, `tag1`, `tag2`,
     `endpoint`. `valueDecimals` es el peor — `value=100, decimals=0` es "100" y
     `decimals=2` es "1.00", misma firma. Y el struct no cubre `revokeFeedback`
     (falta `feedbackIndex`) ni `appendResponse` (faltan los cuatro últimos).
     Propusimos structs por selector, con el contrato armando el calldata.
   - **`verifyingContract` = la cuenta del rater**, y no es preferencia: con el
     delegate como dominio y sin campo `account`, la misma firma se replayea
     contra cualquier otra cuenta delegada al mismo delegate.

   Cuando confirmen el shape, nuestro lado es medio día, y podemos servir los dos
   digests en paralelo durante la transición.

---

## Cómo comprobar que están hablando con la versión nueva

```bash
curl -s https://facilitator.ultravioletadao.xyz/version   # 1.95.0

curl -s -X POST https://facilitator.ultravioletadao.xyz/feedback/evm/prepare \
  -H 'content-type: application/json' -d '{
    "x402Version":1, "network":"base",
    "feedback":{"agentId":"18896","rater":"0x0000000000000000000000000000000000000001",
                "value":100,"valueDecimals":0,"tag1":"quality","tag2":"api",
                "endpoint":"e","feedbackUri":"u"}
  }' | jq '{digest, signingPayload}'
```

Si `signingPayload` viene `null` o ausente, están contra un facilitador anterior
a v1.95.0. **En ese caso fallen ruidoso**: no caigan de vuelta a firmar `digest`
desde una wallet, que es justo el camino roto.

## Y avísennos

Cuando vean el primer rating con `authored_by` distinto de nuestra wallet,
díganlo. Ese es el corte que veníamos persiguiendo, y todavía no lo vio nadie.
