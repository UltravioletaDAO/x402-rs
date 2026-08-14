# Spec de despliegue del `FeedbackDelegate` — para Execution Market

**Fecha:** 2026-08-14
**De:** sesión del facilitador (x402-rs)
**Para:** la sesión de Execution Market, que es quien tiene el repo, las llaves y el deploy
**Contexto:** la integración EIP-7702 del lado del facilitador ya está **terminada y en
producción** (v1.74.0): `POST /feedback/evm/prepare` + `POST /feedback/evm/submit`. Sirve en
**una sola red**, `base-sepolia`, porque es el único delegate que existe. Este documento es
exactamente lo que hace falta para que existan los demás.

---

## 1. Resumen en una línea

Hacen falta **16 despliegues**, no 18: SKALE queda afuera por imposibilidad técnica medida, y
Solana no necesita delegate. De esos 16, **7 redes ya tienen EIP-7702 probado con tráfico real**
y las otras 9 piden un ensayo de una transacción antes de gastar en el deploy.

---

## 2. Lo que NO hace falta, y por qué

**Solana / Solana Devnet: ningún contrato.** La instrucción `give_feedback` ya declara la cuenta
0 como `[signer, writable] client (feedback author)`, y SVM soporta varios firmantes por
transacción de forma nativa. Ya está resuelto y desplegado por el lado de transacción
parcialmente firmada (`/feedback/solana/prepare` + `/submit`): el rater firma como `client` y
el facilitador queda de fee payer. No hay delegación, no hay hardfork, no hay contrato.

**SKALE Base y SKALE Base Sepolia: imposible, no "todavía no".** Medido el 2026-08-14: el header
de sus bloques no trae **ningún** hito de hardfork — ni `withdrawalsRoot` (Shanghai), ni
`blobGasUsed` / `parentBeaconBlockRoot` (Cancun), ni `requestsHash` (Prague). Es un EVM anterior
a Shanghai, y EIP-7702 llegó con Prague. Es la misma raíz por la que el escrow de x402r sigue
bloqueado ahí. **No desplegar el delegate en SKALE**: sería gas gastado en un contrato que
ninguna cuenta puede delegar. Si la gente de SKALE completa el upgrade, se reevalúa.

---

## 3. El argumento del constructor, que es lo único que se puede equivocar sin darse cuenta

`FeedbackDelegate(address reputationRegistry)` es `immutable`, sin setter y sin upgrade. Si se
despliega con la address equivocada **no se nota nunca**, y hay una razón concreta:

> `relayFeedback` termina en `(bool ok, ) = REPUTATION_REGISTRY.call(data)`, y en el EVM **una
> llamada a una dirección sin código devuelve éxito**. Un delegate apuntado a un registry que no
> existe en esa cadena responde `ok = true`, emite `FeedbackRelayed`, gasta el nonce del rater y
> **no califica a nadie**.

Lo vimos pasar en el ensayo end-to-end del 2026-08-14, no es teoría. Por eso el facilitador ahora
se niega a servir un delegate sin verificar las tres cosas de §5.

| clase de red | argumento del constructor |
|---|---|
| **mainnets** | `0x8004BAa17C55a88189AE136b182e5fdA19dE9b63` |
| **testnets** | `0x8004B663056A597Dffe9eCcC1965A193B7388713` |

Verificado el 2026-08-14: **el registry tiene código en las 18 redes**, así que ninguna está
bloqueada por eso.

---

## 4. Las 16 redes, con la evidencia de 7702 que medimos

`eth_estimateGas` con `authorizationList` **no sirve** para decidir esto: un nodo sin 7702 puede
ignorar el campo y contestar igual. Lo que no es ambiguo es una transacción **tipo 4 incluida en
un bloque**. Eso es lo que buscamos.

Segundo matiz, que cambia la lectura: **en Polygon (Bor), Arbitrum (Nitro) y Avalanche el header
no es señal** — usan headers propios y no traen los campos de Prague aunque soporten 7702. De
hecho Polygon y Arbitrum mainnet tienen tipo-4 **probado** con headers "vacíos". Así que la
ausencia de hitos ahí no significa nada; en la familia OP-Stack/geth sí.

### A. 7702 PROBADO — hay transacciones tipo 4 en bloques recientes (7 redes)

Desplegar directo, sin ensayo previo.

| red | chain id | registry para el constructor | evidencia |
|---|---|---|---|
| `ethereum` | 1 | mainnet | 24 tx tipo-4 en 6 bloques |
| `base` | 8453 | mainnet | 7 tx tipo-4 en 6 bloques |
| `polygon` | 137 | mainnet | 1 tx tipo-4 en 6 bloques |
| `bsc` | 56 | mainnet | 20 tx tipo-4 en 6 bloques |
| `arbitrum` | 42161 | mainnet | 1 tx tipo-4 |
| `ethereum-sepolia` | 11155111 | testnet | 1 tx tipo-4 en 6 bloques |
| `base-sepolia` | 84532 | testnet | 1 tx tipo-4 — **YA DESPLEGADO**: `0x3A68085499B62286468A35b7D9Dfc237ef2d3768` |

### B. Header de Prague confirmado, sin tráfico 7702 observado (5 redes)

Casi seguro que andan; el ensayo de §6 lo confirma en un minuto.

| red | chain id | registry | evidencia |
|---|---|---|---|
| `optimism` | 10 | mainnet | header Prague (`requestsHash`), 0 tipo-4 en 60 bloques / 2242 tx |
| `celo` | 42220 | mainnet | header Prague, 0 en 60 bloques / 834 tx |
| `monad` | 143 | mainnet | header Prague, 0 en 60 bloques / 714 tx |
| `optimism-sepolia` | 11155420 | testnet | header Prague |
| `celo-sepolia` | 11142220 | testnet | header Prague |

### C. El header no es señal en este stack, y no vimos tráfico (4 redes)

Sus hermanas de mainnet están **probadas** (Polygon y Arbitrum en la tabla A), así que el stack
soporta 7702. Igual: **ensayo antes de desplegar**.

| red | chain id | registry | por qué el header no decide |
|---|---|---|---|
| `avalanche` | 43114 | mainnet | C-Chain, header propio (sí tiene Cancun) |
| `polygon-amoy` | 80002 | testnet | Bor, header propio |
| `arbitrum-sepolia` | 421614 | testnet | Nitro, header propio |
| `avalanche-fuji` | 43113 | testnet | C-Chain, header propio |

### D. Imposible (2 redes) — no desplegar

`skale-base` (1187947933) y `skale-base-sepolia` (324705682). Ver §2.

---

## 5. Lo que el facilitador verifica antes de servir una address

No alcanza con pasarnos la dirección: la cableamos sólo después de que estas tres pasen contra la
red correspondiente. Si alguna falla, el endpoint responde 503 con un motivo acotado y no gasta
gas.

1. **`eth_getCode(delegate)` no vacío.** Una address en una tabla es una afirmación;
   `eth_getCode` es evidencia. El proxy `upto` con una address sin código en ninguna cadena ya
   produjo settles de éxito falso una vez.
2. **`delegate.REPUTATION_REGISTRY()` == el registry de ESA red.** Atrapa el delegate desplegado
   con el argumento de la clase de red equivocada, que es el error de §3.
3. **`eth_getCode(registry)` no vacío**, por lo mismo del `.call()` que devuelve éxito.

---

## 6. Ensayo de 7702 para las redes de B y C, antes de gastar en el deploy

Cuesta una transacción mínima y convierte "probablemente" en "sí". Se auto-delega una cuenta
descartable a **cualquier** dirección (no hace falta que sea el delegate) y se mira el código:

```bash
# OJO: mandar la tx a un TERCERO, no a la cuenta que se delega. Si se manda a la propia
# cuenta, esa misma tx es la primera llamada al código nuevo, y con un delegate sin
# receive/fallback revierte en la estimación -- la delegación nunca aterriza y se lee
# igual que un fallo de firma o de nonce. Nos pasó armando el control negativo.
cast send --rpc-url <RPC> --private-key <LLAVE_DESCARTABLE> \
  --auth 0x0000000000000000000000000000000000000000 <OTRA_DIRECCION>

cast code <DIRECCION_DE_LA_LLAVE> --rpc-url <RPC>
# 0xef0100...  -> 7702 vivo en esta cadena. Desplegar.
# 0x           -> la delegación no aterrizó. NO desplegar; reportarlo.
```

(Delegar a la dirección cero es la forma canónica de **revocar**, así que el ensayo se limpia
solo y deja la cuenta como estaba.)

---

## 7. Qué necesitamos de vuelta

Una línea por red desplegada, y nada más:

```
<nombre de red x402>  <chain id>  <address del delegate>  <tx hash del deploy>
```

Con eso: verificamos §5 contra la cadena, agregamos la entrada a la tabla de
`src/erc8004/relay.rs` (una línea por red) y sale en el próximo release. **No vamos a inventar
ninguna entrada por adelantado** — una address sin contrato detrás es exactamente el bug de §3.

Prioridad sugerida, si hay que elegir por dónde empezar: `base` y `ethereum` mainnet concentran
el problema real — el 87,2% de los feedbacks mal atribuidos está en Base.

---

## 8. Recordatorio de lo que ya está de nuestro lado

- `/feedback/evm/prepare` devuelve `delegate`, `data`, `digest`, `deadline` (15 min por defecto,
  `ERC8004_RELAY_DEADLINE_SECS`), `nonce`, `delegated` y `accountNonce`.
- `/feedback/evm/submit` reconstruye el calldata desde los parámetros declarados y **rechaza
  cualquier firma que no cubra exactamente eso**, cualquier autorización que apunte a otro
  delegate, y cualquiera que no esté firmada por el rater.
- El digest está fijado contra **su** contrato: se calculó llamando `relayDigest()` **sobre la
  cuenta delegada**, que es la única forma de que el contrato lo compute con `address(this)`
  igual al rater. Hay un test con ese vector.
- Ensayo end-to-end con el `FeedbackDelegate` real en `anvil --hardfork prague`: el registry
  registró al **rater** como autor y al sponsor como pagador de gas.
