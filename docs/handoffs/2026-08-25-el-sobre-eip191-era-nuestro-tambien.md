---
date: 2026-08-25
tags:
  - type/handoff
  - domain/identity
  - domain/blockchain
  - priority/p0
status: active
aliases:
  - El sobre EIP-191 también estaba mal en nuestros SDKs
  - signingPayload servido desde v1.95.0
related-files:
  - src/erc8004/relay.rs
  - src/erc8004/types.rs
  - docs/handoffs/2026-08-24-respuesta-a-execution-market-v3-y-eip712.md
---

# Sí había algo roto de nuestro lado: nuestros propios SDKs prescribían el camino que falla.

> **Para:** Execution Market y Karma Kadabra
> **De:** el equipo del Facilitator (`x402-rs`)
> **Responde a:** `2026-08-25-el-digest-no-se-podia-firmar.md`
> **Estado:** arreglado en las tres capas — facilitador **v1.95.0**, PyPI
> **0.66.0**, npm **2.71.0**.

## Su §4 dice "nada roto de su lado". Gracias, pero no es exacto

El contrato de wire sí era coherente: `relay_digest` envuelve y
`signature_authorises` recupera con prehash, sin agregar nada. Eso está bien.

**Lo que estaba mal era nuestra documentación, y prescribía exactamente el bug.**
Los dos SDKs que publicamos anteayer decían, textualmente:

```python
# uvd-x402-sdk 0.65.0, README y docstring
# 1. Sign the digest with the RATER's key (EIP-191 personal-sign).
signature = sign_message(prep.digest)          # <- el camino roto
```

```typescript
// uvd-x402-sdk 2.70.0
// 1. Sign the digest with the RATER's key (EIP-191 personal-sign).
const signature = await signMessage(prep.digest!);   // <- el mismo
```

`sign_message` / `signMessage` **son** `personal_sign`. Cualquiera que siguiera
nuestra documentación al pie de la letra producía la firma doble-envuelta. Así
que no fueron sólo sus tres superficies: la nuestra también, en dos lenguajes,
publicada en PyPI y npm.

Lo medimos antes de arreglarlo, con una cuenta efímera, en vez de deducirlo de su
reporte:

```
rater esperado : 0x19E7E376E7C213B7E7e7e46cc70A5dD086DAff2A

A) personal_sign(digest)     -> el facilitador recupera 0x3fa325a5…Ab00   ✗
B) personal_sign(inner)      -> el facilitador recupera 0x19E7E376…ff2A   ✓
C) unsafe_sign_hash(digest)  -> el facilitador recupera 0x19E7E376…ff2A   ✓
```

Su medición y la nuestra dan lo mismo. Tenían razón, y KK tenía razón.

## Lo que hicimos: servir el valor, en vez de que lo reconstruyan

Ustedes lo resolvieron reconstruyendo el hash previo al sobre a partir de `data`.
Funciona, y su guard —envolver su reconstrucción y exigir que dé igual al digest
nuestro— es la parte que lo hace seguro publicar. Pero es **una segunda
implementación del preimage**, y una segunda implementación deriva: en silencio,
hacia un payload que no firma nada.

Desde **v1.95.0** el `prepare` lo sirve directo:

```jsonc
{
  "digest":         "0x…",   // YA lleva el sobre. Firmar CRUDO (prehash).
  "signingPayload": "0x…",   // el mismo hash SIN el sobre. personal_sign ESTE.
  ...
}
```

Con la relación explícita, para que puedan mantener el guard sin reimplementar
nada:

```
keccak256("\x19Ethereum Signed Message:\n32" || signingPayload) == digest
```

Es un chequeo de una línea contra dos campos que ya reciben. Si algún día
divergen, es porque algo se movió de nuestro lado y lo van a ver de inmediato.

**Pueden borrar la reconstrucción desde `data` cuando quieran.** No corre prisa:
su versión y la nuestra dan el mismo valor, así que conviven sin problema.

### Y sobre `data`: no lo vamos a quitar

Su punto 1 pide aviso si alguna vez lo sacamos por peso. No hay tal plan, y ahora
hay una razón adicional para conservarlo: es lo que le permite a un cliente
diligente verificar que el calldata que firma es el que dice ser, sin confiar en
nuestro `digest`. Queda como parte del contrato.

## Los tres caminos, dichos de una vez

| quién firma | qué firma | cómo |
|---|---|---|
| agente con llave propia (**KK**) | `digest` | prehash — `Account.unsafe_sign_hash`, `sign_hash_sync`, `signingKey.sign` |
| wallet de navegador / móvil / custodio | `signingPayload` | `personal_sign` — la wallet aplica el sobre y aterriza en `digest` |
| ✗ **nadie** | `digest` | `personal_sign` — recupera un desconocido, muere en `relay_bad_signature` sin pista |

Confirmamos lo que le dicen a KK: **el camino de llave propia nunca estuvo roto y
no cambia.** Los que firman con `unsafe_sign_hash(digest)` funcionan hoy y
seguirán funcionando; `signingPayload` es para wallets.

## Lo que dejamos fijado para que no vuelva

Un arreglo de esto no se relee, se ataca. Los tests nuevos son:

- **En el facilitador** (`double_wrapping_the_digest_recovers_a_stranger`):
  firma real con una llave real por los dos caminos, y **asserta que el
  doble-envuelto NO autoriza**. Está escrito como desigualdad a propósito: si
  alguien "arregla" el bug haciendo que el camino doble-envuelto funcione, el
  test falla — porque eso rompería a todos los que firman con llave cruda.
- **`the_signing_payload_is_the_digest_without_the_envelope`**: fija que los dos
  valores difieren por exactamente el sobre. Si una edición futura envuelve el
  payload, o desenvuelve el digest, salta ahí en vez de en producción.
- **En los dos SDKs**: el mismo par, más uno que fija que contra un facilitador
  viejo `signingPayload` llega **ausente** y nunca igual a `digest` — un fallback
  a `digest` sería servirle a la wallet justo el valor que no puede firmar.

Y `relay_digest()` ahora está construido sobre `relay_signing_payload()`, así que
los dos valores salen de un solo preimage. No hay dos fórmulas que puedan
separarse.

## Sobre su §6

> *"El riel estuvo desplegado, verificado en nueve cadenas, con handoffs cruzados
> entre tres equipos — y no podía completar una sola firma."*

Suscribimos, y nos incluimos. Verificamos delegates en dos RPC por cadena,
medimos bytecode byte a byte, comparamos el digest contra el contrato desplegado
— y ninguna de esas verificaciones podía encontrar esto, porque todas miraban la
cadena y el bug estaba en el borde entre nuestro campo y una wallet.

Lo que lo encontró fue leer el código de un cliente. Vale la pena anotar que
publicamos SDKs con instrucciones de firma que **nunca ejecutamos contra una
wallet real**: los tests que escribimos mockeaban el HTTP y verificaban que los
campos llegaran al wire, no que la firma resultante verificara. Un test que
mockea el transporte no puede encontrar un bug de criptografía. Eso lo corregimos
arriba.

## EIP-712 (v4): su punto 2 es correcto y refuerza el caso

Tienen razón en que `signTypedData` hace desaparecer esta clase de problema: no
hay ambigüedad de sobre porque no hay sobre que aplicar dos veces. Súmenlo a los
argumentos.

Sigue en pie lo del handoff de ayer, y sigue siendo lo que les bloquea escribir
el contrato:

1. **El struct deja cuatro campos fuera del digest** — `valueDecimals`, `tag1`,
   `tag2`, `endpoint`. `valueDecimals` es el peor: `value=100, decimals=0` es
   "100" y `decimals=2` es "1.00", misma firma. Y no cubre `revokeFeedback`
   (falta `feedbackIndex`) ni `appendResponse` (faltan los cuatro últimos).
   Propusimos structs por selector con el contrato armando el calldata.
2. **`verifyingContract` = la cuenta del rater**, y no es preferencia: con el
   delegate como dominio y sin campo `account`, la misma firma se replayea contra
   cualquier otra cuenta delegada al mismo delegate.

Cuando confirmen el shape, nuestro lado es medio día, y podemos servir los dos
digests en paralelo durante la transición.

## Estado

| | |
|---|---|
| `signingPayload` en `/feedback/evm/prepare` | ✅ v1.95.0 |
| Docs de firma corregidas en los SDKs | ✅ PyPI 0.66.0, npm 2.71.0 |
| Tests que atacan el doble-sobre | ✅ facilitador + los dos SDKs |
| `data` en la respuesta | ✅ se queda, es contrato |
| Struct EIP-712 | ⏳ esperando su decisión |

Y lo de siempre: cuando vean el primer rating con `authored_by` distinto de
nuestra wallet, avísennos. Ese es el corte.

```bash
curl -s -X POST https://facilitator.ultravioletadao.xyz/feedback/evm/prepare \
  -H 'content-type: application/json' -d '{
    "x402Version":1, "network":"base",
    "feedback":{"agentId":"18896","rater":"<EOA>","value":100,"valueDecimals":0,
                "tag1":"quality","tag2":"api","endpoint":"e","feedbackUri":"u"}
  }' | jq '{digest, signingPayload}'
```
