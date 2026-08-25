---
date: 2026-08-25
tags:
  - type/handoff
  - domain/identity
  - domain/blockchain
  - priority/p0
status: active
aliases:
  - Confirmación del struct EIP-712 para v4
  - Typehashes fijados
related-files:
  - src/erc8004/relay.rs
  - docs/handoffs/2026-08-24-respuesta-a-execution-market-v3-y-eip712.md
---

# Confirmado: el struct EIP-712 para v4. Desplieguen.

> **Para:** Execution Market
> **De:** el equipo del Facilitator (`x402-rs`)
> **Responde a:** *"El facilitador debe confirmar el struct EIP-712 para que
> despliegue v4."*
> **Estado:** confirmado. Typehashes computados abajo, para que los dos lados
> fijen el mismo valor y no se descubra una diferencia con firmas reales.

## El dominio

```solidity
EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)

name              = "FeedbackDelegate"
version           = "1"
chainId           = <la cadena>
verifyingContract = address(this)   // LA CUENTA DEL RATER, no el delegate
```

Sin `salt`. `chainId` y la cuenta del rater entran por acá, así que **salen del
struct**: hoy viven dentro del preimage y en v4 los cubre el dominio.

### La trampa que hay que evitar sí o sí

**No cacheen el `DOMAIN_SEPARATOR` en el constructor.** Es el patrón por defecto
de OpenZeppelin `EIP712` y de casi todo contrato con typed data, y acá está mal:
bajo 7702 `address(this)` es **la cuenta del rater**, distinta en cada llamada,
mientras que un separador calculado en el deploy congelaría la dirección del
**delegate**.

Si lo cachean, todas las firmas del mundo recuperan a un desconocido — el mismo
modo de falla que acabamos de pasar con el sobre EIP-191, otra vez silencioso y
otra vez visible sólo como `relay_bad_signature`. Calcúlenlo **por llamada**.

Corolario: `OZ EIP712` con su cache no sirve tal cual acá. Y si exponen
`eip712Domain()` (EIP-5267), tiene que devolver `address(this)` en vivo.

## Los tres structs

Uno por selector, con **la lista completa de parámetros del registry**. Así no
hay un `bytes data` separado que pueda diferir de lo que se firmó y se mostró: el
contrato arma el calldata a partir de lo mismo que el rater vio.

```solidity
struct RelayedGiveFeedback {
    address registry;
    uint256 agentId;
    int128  value;
    uint8   valueDecimals;
    string  tag1;
    string  tag2;
    string  endpoint;
    string  feedbackURI;
    bytes32 feedbackHash;
    uint256 deadline;
    bytes32 nonce;
}

struct RelayedRevokeFeedback {
    address registry;
    uint256 agentId;
    uint64  feedbackIndex;
    uint256 deadline;
    bytes32 nonce;
}

struct RelayedAppendResponse {
    address registry;
    uint256 agentId;
    address clientAddress;
    uint64  feedbackIndex;
    string  responseURI;
    bytes32 responseHash;
    uint256 deadline;
    bytes32 nonce;
}
```

Notas de por qué está así:

- **El orden y los tipos son los del registry, exactos.** `giveFeedback` toma
  `(uint256, int128, uint8, string, string, string, string, bytes32)`;
  `revokeFeedback` toma `(uint256, uint64)`; `appendResponse` toma
  `(uint256, address, uint64, string, bytes32)`. Nada se convierte ni se
  reordena, para que armar el calldata sea mecánico.
- **`registry` se queda dentro**, aunque el delegate lo tenga `immutable`. Hoy
  entra al preimage y sacarlo sería aflojar una atadura que ya existe. Defensa en
  profundidad barata.
- **`valueDecimals` está**, y era el que faltaba. `value=100, decimals=0` es
  "100" y `decimals=2` es "1.00": sin él en el struct, un backend comprometido
  cambia un byte después de firmado y la wallet ni siquiera lo muestra, porque no
  está en el tipo.
- **`feedbackIndex` está** en los otros dos. Sin él, una autorización de revoke no
  puede decir cuál feedback revoca — cubriría *cualquiera*.
- **Sale `selector`.** El typehash ya lo identifica; un campo aparte sería una
  segunda fuente de verdad sobre qué se está llamando.

## Los typehashes, para pinnearlos de los dos lados

Computados sobre las cadenas de tipo canónicas (campos en orden de declaración,
sin espacios):

| struct | typehash |
|---|---|
| `RelayedGiveFeedback` | `0x1303f838650b9f1619400e61b9bda2d6b484ea75b3af6a9ae58337d57a0585c0` |
| `RelayedRevokeFeedback` | `0x4216eda31386c6ee7eab53320b15e7b13ced07582db035f719fb73b640a5a1d7` |
| `RelayedAppendResponse` | `0xcf4215ad5e2b816cea8e759d7e5429a2803265ee8d67e9fdb2069fa87b169243` |
| `EIP712Domain` | `0x8b73c3c69bb8fe3d512ecc4cf759cc79239f7b179b0ffacaa9a75d522b39400f` |

Cadenas de tipo exactas, por si alguna difiere de lo que compilen:

```
RelayedGiveFeedback(address registry,uint256 agentId,int128 value,uint8 valueDecimals,string tag1,string tag2,string endpoint,string feedbackURI,bytes32 feedbackHash,uint256 deadline,bytes32 nonce)
RelayedRevokeFeedback(address registry,uint256 agentId,uint64 feedbackIndex,uint256 deadline,bytes32 nonce)
RelayedAppendResponse(address registry,uint256 agentId,address clientAddress,uint64 feedbackIndex,string responseURI,bytes32 responseHash,uint256 deadline,bytes32 nonce)
```

Constantes auxiliares:

```
keccak256("FeedbackDelegate") = 0x83ef18929bb23bf6a1a26d92e500b2807c06847b7f6e9e8e1dfcf685a2eadd50
keccak256("1")                = 0xc89efdaa54c0f20c7adf612882df0950f5a951637e0307cdcb4c672f298b8bc6
```

**Vector de dominio** para que comparen el primer valor antes de firmar nada —
chainId 8453, cuenta del rater `0x0B3520435d7Bc7197C55204f01261706e5c7DcA5` (la
efímera de su propia medición de ayer):

```
domainSeparator = 0xf274e7ca4a3deccb7511095cc219ef98b58c557abd488e332f24d5e5a3d3e719
```

Si su contrato da otro valor para esa cuenta en esa cadena, es el cache del
constructor.

## Codificación, sin sorpresas

Estándar EIP-712, pero lo escribo porque es donde se cuela un error silencioso:

- `string` → `keccak256(bytes(s))`, **no** el string padeado.
- `bytes32` → tal cual.
- `address` → padeado a 32 bytes por izquierda.
- `int128` → **extensión de signo** a 32 bytes. Es el único con signo; un
  `value` negativo mal extendido da un hash distinto y una firma que no recupera.
- `uint8`, `uint64`, `uint256` → padeados por izquierda.

Digest final:

```
digest = keccak256(0x1901 || domainSeparator || hashStruct(message))
```

## Qué vamos a servir nosotros

Cuando desplieguen, `prepare` devolverá, además de lo de hoy:

```jsonc
{
  "typedData": { "domain": {...}, "types": {...}, "primaryType": "RelayedGiveFeedback",
                 "message": {...} },   // para signTypedData, tal cual
  "digest":    "0x…"                   // el mismo, para quien firma con llave cruda
}
```

Y **`signingPayload` desaparece en v4**: existe sólo porque `personal_sign`
aplica un sobre que hay que anticipar. `signTypedData` no tiene sobre que aplicar
dos veces, que es el punto 2 de su handoff de hoy y es correcto.

Vamos a servir **los dos digests en paralelo** durante la transición: el v3
(EIP-191) y el v4 (EIP-712), elegidos por la versión del delegate que
`assert_delegate_usable` detecte en la cadena. Así el despliegue de ustedes y el
nuestro no tienen que caer en la misma hora, y ningún cliente a medio migrar se
queda sin poder firmar.

O sea: **desplieguen cuando quieran.** No los esperamos con un flag.

## Lo que les pedimos a cambio

1. **Que el discriminador de versión siga funcionando.** Nuestro
   `assert_delegate_usable` distingue v1 de v3 con `supportsInterface(0x150b7a02)`.
   Para v4 necesitamos poder distinguirlo de v3 igual de barato: agreguen un
   interface id nuevo a `supportsInterface`, o expongan una constante
   `VERSION()`. Cualquiera de las dos nos sirve; díganos cuál y la usamos.
2. **Aprovechen y anuncien ERC-1271.** Sigue implementado pero no listado en
   `supportsInterface` (`0x1626ba7e` devuelve `false`). Es una línea, y ya que
   redesplegan…
3. **Un vector de prueba** con el contrato ya desplegado: un `RelayedGiveFeedback`
   completo con su digest. Comparamos contra nuestro cálculo antes de servirlo, y
   así no descubrimos una diferencia con la firma de un rater real. Es lo que
   hicimos con `relayDigest()` en v3 y por eso el cambio de v1→v3 fue un cambio de
   dirección y nada más.

## Lo que esto arregla, dicho corto

El rater ve en su wallet el `agentId`, el `value` con sus decimales, los tags, el
endpoint, la URI y el deadline — en texto, con nombres. Hoy ve un blob
hexadecimal y no puede distinguir una calificación de un revoke. Un backend
comprometido que hoy le hace firmar `revokeFeedback` mientras el JSON dice
`score: 95`, en v4 tiene que mostrárselo.

Ese era el punto, y el struct que confirmamos lo cumple para los tres selectores,
no para uno.
