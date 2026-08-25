---
date: 2026-08-24
tags:
  - type/handoff
  - domain/identity
  - domain/blockchain
  - priority/p0
status: active
aliases:
  - v3 servido, y el agujero del struct EIP-712
  - Respuesta al handoff de delegates v3
related-files:
  - src/erc8004/relay.rs
  - docs/handoffs/2026-08-24-execution-market-rail-en-produccion.md
---

# v3 ya está servido. Y su struct EIP-712 deja cuatro campos fuera del digest.

> **Para:** Execution Market
> **De:** el equipo del Facilitator (`x402-rs`)
> **Responde a:** `2026-08-24-para-el-facilitador-v3-y-eip712.md`
> **Estado:** v3 **vivo en producción** (v1.94.0, verificado contra el endpoint
> real). El punto de EIP-712 necesita una decisión suya antes de que escriban el
> contrato.

## 1. Las 9 direcciones v3 están servidas

Ninguna se copió de su archivo. Cada una se leyó de su propia cadena, **en dos
RPC independientes**, comprobando cuatro cosas: chainId del nodo, que el
delegate tenga código, que su `REPUTATION_REGISTRY()` devuelva el registry de esa
red, y que **el registry también tenga código ahí**.

Las nueve pasaron. Las ocho mainnets tienen runtime code byte a byte idéntico —
**3216 bytes**, `sha256 a3094693799a3f8d…` — y base-sepolia difiere, que es lo
que predice un `immutable` de registry distinto. (v1 medía 1996; el salto es
consistente con los hooks nuevos.)

**Y verificamos que el digest es realmente idéntico, en vez de creerles.**
Llamamos `relayDigest()` sobre el v3 desplegado en Base con una entrada fija y lo
comparamos contra `relay_digest()` en nuestro Rust:

```
on-chain : 0xe0e04e0b35b6a7c7731098f795c69147d89a3d8dfb2f6539c754a51dc63d4752
nuestro  : 0xe0e04e0b35b6a7c7731098f795c69147d89a3d8dfb2f6539c754a51dc63d4752
```

Tenían razón: fue un cambio de dirección, no de protocolo. Nuestro cálculo no se
tocó.

**Los SDKs no necesitan release.** Ni el de Python ni el de TypeScript llevan
direcciones — llevan la lista de **redes**, y esa no cambió. Las versiones
publicadas ayer (PyPI 0.65.0, npm 2.70.0) siguen siendo las correctas.

## 2. CREATE, no CREATE2: agregamos un chequeo de versión

Esta parte de su handoff es la que más nos cambió el código, y vale la pena que
la sepan porque les aplica igual.

Como desplegaron con **CREATE**, la dirección sale de (deployer, nonce). O sea:
**la misma dirección puede tener versiones distintas en cadenas distintas.** No
es teórico, lo medimos:

- la v3 de **celo** (`0x794C907F…31bA`) es la **v1 de arbitrum**;
- la v3 de **bsc** (`0x825E997F…2b82`) es lo que **optimism y monad** corrieron
  como v1.

El problema es que v1 pasa todos nuestros chequeos: tiene código y está pinneada
al registry correcto. Si nos hubiéramos quedado con la tabla vieja, habríamos
seguido relayando contra la versión que rompe la wallet del rater — **en
silencio**, porque no hay nada en `eth_getCode` que distinga una versión de otra.

Así que `assert_delegate_usable()` ahora hace una cuarta comprobación: un probe
**ERC-165**. v1 no tiene `supportsInterface` (revierte), v3 responde `true`. Un
delegate obsoleto ahora se rechaza con `relay_delegate_superseded_version` en vez
de relayarse.

Dos detalles de implementación, por si les sirven del lado suyo:

- **Usamos `0x150b7a02` (ERC-721 receiver), no `0x1626ba7e` (ERC-1271)**, y esa
  elección es forzada — ver el punto 4.
- **Un revert es un veredicto; un fallo de RPC no.** Los separamos: si el nodo no
  contesta devolvemos `relay_rpc_unavailable`, no "delegate obsoleto". Colapsar
  los dos haría que una caída nuestra le niegue el rail a un rater que no puede
  hacer nada al respecto. Es la misma distinción que 404 vs 503 en
  `/identity/:network/owner/:address`.

Si en algún momento migran a CREATE2 con factory, esto se simplifica para los
dos. Mientras tanto, **una dirección sola ya no identifica una versión** y
conviene que sus propios clientes lo asuman.

## 3. EIP-712: sí, y hay un agujero en el struct

Estamos de acuerdo con la migración y con el diagnóstico. Un rater que firma un
`keccak256` opaco no puede distinguir una calificación de un revoke, y eso es
inaceptable. Cuenten con nosotros para la ventana coordinada.

**Pero el struct que proponen deja campos afuera del digest, que es exactamente
lo que ustedes dicen que no puede pasar.**

`giveFeedback` toma ocho parámetros:

```solidity
giveFeedback(
    uint256 agentId,
    int128  value,
    uint8   valueDecimals,   // <- no está en el struct
    string  tag1,            // <- no está
    string  tag2,            // <- no está
    string  endpoint,        // <- no está
    string  feedbackURI,
    bytes32 feedbackHash
)
```

Hoy los ocho están atados, porque el digest cubre `keccak256(data)` — el calldata
entero. Al pasar a campos nombrados, lo que no esté en el struct **queda suelto**.

El peor es `valueDecimals`. `value=100, decimals=0` es "100"; `value=100,
decimals=2` es "1.00". **Misma firma, calificación cien veces distinta.** Un
backend comprometido —el mismo modelo de amenaza que motivó la propuesta— cambia
un byte después de que el rater firmó. La migración reintroduciría la
vulnerabilidad que viene a cerrar, en un campo que la wallet ni siquiera va a
mostrar porque no está en el tipo.

Y para los otros dos selectores el struct no alcanza en absoluto:

- `revokeFeedback(uint256 agentId, uint64 feedbackIndex)` — no hay
  `feedbackIndex` en el struct, así que una autorización de revoke no puede
  decir **cuál** feedback revoca;
- `appendResponse(agentId, clientAddress, feedbackIndex, responseURI, responseHash)`
  — no está ninguno de los cuatro últimos.

**Nuestra recomendación: structs por selector, y que el contrato arme el calldata
desde el struct.** O sea, que `relayFeedback` deje de recibir `bytes data` y
reciba el struct tipado:

```solidity
RelayedGiveFeedback  { address registry; uint256 agentId; int128 value;
                       uint8 valueDecimals; string tag1; string tag2;
                       string endpoint; string feedbackURI; bytes32 feedbackHash;
                       uint256 deadline; bytes32 nonce; }
RelayedRevokeFeedback{ address registry; uint256 agentId; uint64 feedbackIndex;
                       uint256 deadline; bytes32 nonce; }
RelayedAppendResponse{ address registry; uint256 agentId; address clientAddress;
                       uint64 feedbackIndex; string responseURI;
                       bytes32 responseHash; uint256 deadline; bytes32 nonce; }
```

Así no hay `data` que pueda derivar del struct, porque no hay dos fuentes: el
contrato construye el calldata a partir de lo mismo que se firmó y se mostró.
Sale el campo `selector`, que pasa a ser el typehash.

Si prefieren un solo struct por simplicidad, la alternativa aceptable es dejar
`bytes32 dataHash` **dentro** del struct como el vínculo real, y los campos
legibles al lado sólo para display — pero entonces el contrato tiene que
verificar que los campos legibles reconstruyen exactamente `data`, o vuelven a
tener dos fuentes de verdad.

### `verifyingContract`: la cuenta del rater, y no es preferencia

Su inclinación es la correcta, y es más fuerte que una preferencia: **es
obligatorio, o hay replay.**

Si `verifyingContract` fuera el delegate, todas las cuentas delegadas a ese
contrato en esa cadena comparten dominio EIP-712. Y como el struct que proponen
no lleva la cuenta del rater en ningún campo, **la misma firma sería válida
contra cualquier otra cuenta delegada al mismo delegate**. Bajo 7702
`address(this)` es la cuenta, que es justo lo que hoy ata el digest (`address(this)`
está adentro del preimage). Mantengan eso.

Si por alguna razón necesitan que `verifyingContract` sea el delegate, entonces
hace falta un campo `address account` explícito en el struct. Una de las dos, no
ninguna.

### La ventana coordinada

De acuerdo, mismo día. Nuestro lado es un cálculo nuevo detrás de un flag, así
que podemos **servir los dos digests durante la transición** si les sirve: v3 con
EIP-191 y v4 con EIP-712 en paralelo, y apagamos el viejo cuando digan. Eso saca
la presión de que los dos deploys caigan en la misma hora.

Lo que necesitamos de ustedes antes de escribir código: el shape final del struct
y el `verifyingContract`. Con eso definido, nuestro lado es medio día.

## 4. Dos correcciones

**a) `POST /feedback/revoke` NO está sin autenticar.** Es la segunda vez que
aparece en su lista como pendiente, y lo volvimos a medir contra producción hoy:

```
sin credenciales     -> HTTP 401 {"error":"invalid or missing admin credentials"}
con token inválido   -> HTTP 401
```

Admin-only desde **v1.74.0** (2026-08-18), con secreto propio
(`ERC8004_ADMIN_TOKEN`, deliberadamente **no** el del Bazaar: borrar reputación
de terceros no comparte credencial con nada) y **fail-closed**: si no hay token
configurado la ruta responde 404, indistinguible de una que no existe.

Nos importa cerrarlo bien: si están probando algo y viendo otra cosa, hay un dato
que no tenemos. ¿Contra qué host y con qué payload lo midieron?

**b) v3 implementa ERC-1271 pero no lo anuncia.** `isValidSignature` está en el
bytecode desplegado y responde correctamente —le mandamos una firma basura y
devolvió `0xffffffff`, que es el magic value de rechazo—, pero:

```
supportsInterface(0x1626ba7e) -> false
```

Su `supportsInterface` sólo lista ERC-165, ERC-721 receiver y ERC-1155 receiver.
Los protocolos que llaman `isValidSignature` directo funcionan; los que consultan
ERC-165 primero para decidir si el contrato es 1271, no. Dado el motivo por el
que agregaron ERC-1271 —que Permit2 y Seaport dejaban de aceptar las firmas del
rater— vale la línea. Y todavía pueden redesplegar gratis.

(Es también por lo que nuestro probe de versión usa `0x150b7a02` y no
`0x1626ba7e`: preguntar por el que no anuncian habría reportado un falso negativo
sobre nueve delegates perfectamente buenos.)

## 5. Lo que nos alegró de su handoff

- **El neto en vez del bruto.** Era el punto que bloqueaba medir. Con eso
  arreglado, la fase 2 del gate (`ERC8004_REQUIRE_PROOF=true`) ya tiene sentido
  encenderla en cuanto haya tráfico.
- **`/relay/submit` era un bypass total** y lo cerraron. Que cualquiera pudiera
  saltear `prepare` y postear con un ratee arbitrario mientras nosotros pagamos
  el gas era, en la práctica, un grifo de gas abierto además de un problema de
  integridad.
- **Apagar los tres emisores automáticos.** Es la decisión correcta y el
  argumento es el mismo que el nuestro: on-chain, un default inventado es
  indistinguible de una opinión real. Que nos baje el volumen de `/feedback` es
  exactamente lo que queremos ver.
- Gracias por el dato del `escrow_tx`: si al prender la fase 2 vemos
  `proof_transfer_not_found` en tráfico viejo de streaming, ya sabemos que es el
  fondeo viewer→escrow y no un bug del gate.

## 6. Sigue abierto

**De ustedes:**

- **Recolectar la firma del rater.** Sigue siendo lo único que hace el corte.
  Dicen que el rail no tiene un solo llamador en producción; mientras eso siga
  así, seguimos siendo los autores por más delegates que haya.
- El shape del struct EIP-712 y el `verifyingContract` (punto 3).

**Nuestro:**

- **`POST /feedback` sigue abierto en EVM**, gritando `[WARN] DEPRECATED` en las
  9 redes con delegate. No le pusimos switch de apagado; avísennos cuando hayan
  migrado.
- **`POST /feedback/response` sigue anónimo.** Nos dicen que v3 ya admite
  `appendResponse` — bien, eso desbloquea la autoría real. Queda en nuestra cola,
  y depende de cómo cierre el punto 3: si el struct EIP-712 cubre los tres
  selectores como proponemos, `appendResponse` sale con la misma pieza.
- **Fase 2 del gate**, esperando tráfico.

**De Saul:** los 1.384 feedbacks históricos. Sin novedad — las tres partes
coincidimos en no tocarlos.

---

## Verificado en producción

No es una lectura del código: `assert_delegate_usable()` consulta la cadena en
cada request, así que esta tabla es la respuesta real del endpoint, y de paso
confirma que el probe de versión nuevo no rechaza delegates buenos.

```
$ curl -s https://facilitator.ultravioletadao.xyz/version
{"version":"1.94.0"}

base           0xa7ca33cae3c5890f25dfd08079db82701c9debc6  chainId=8453
ethereum       0x8bf13c5d612eda66d3aea954c95cb77362b4a868  chainId=1
polygon        0x77becfb266e3636c5cf4555348305f134a48fe55  chainId=137
arbitrum       0xce9871fd3d3a3f02a0d40ffa257c21c859c934a3  chainId=42161
optimism       0xde762cfc63551ad4d8c5be8f25ec0bcaa82df5ba  chainId=10
celo           0x794c907fdfc71bfaf0b86d0e463bbd6e949a31ba  chainId=42220
bsc            0x825e997f2f7ed5d3f59466cd754189fb19b62b82  chainId=56
monad          0xde762cfc63551ad4d8c5be8f25ec0bcaa82df5ba  chainId=143
base-sepolia   0x1aaea468fb156aabd2617a507771fc8fe5085b45  chainId=84532

avalanche      400  "relayed feedback is not available on avalanche:
                     no FeedbackDelegate is deployed there yet"
```

Para reproducirlo:

```bash
curl -s -X POST https://facilitator.ultravioletadao.xyz/feedback/evm/prepare \
  -H 'content-type: application/json' -d '{
    "x402Version":1,
    "network":"base",
    "feedback":{"agentId":"18896","rater":"<EOA del rater>",
                "value":100,"valueDecimals":0,"tag1":"quality","tag2":"api",
                "endpoint":"https://agent.example","feedbackUri":"https://example.com/f.json"}
  }' | jq '{delegate, chainId, delegated, deadline, nonce}'
```

`delegated: false` significa que esa cuenta todavía no está delegada: la primera
vez hay que mandar también la autorización 7702 firmada, y el `accountNonce` que
necesita viene en la misma respuesta. A partir de la segunda va sin ella.

Una advertencia de método que ya les dimos y sigue valiendo: **los RPC públicos
devuelven 403 sin `User-Agent`**. Un 403 no es un veredicto sobre la cadena.
