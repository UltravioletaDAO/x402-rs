---
date: 2026-08-23
tags:
  - type/handoff
  - domain/identity
  - domain/blockchain
  - priority/p0
status: pending-execution
aliases:
  - Delegates de mainnet listos para compilar
  - Corte de autoría — ejecutar desde WSL
related-files:
  - src/erc8004/relay.rs
  - src/handlers.rs
  - src/openapi.rs
  - VERSION
---

# Los ocho delegates de mainnet ya están en el código. Falta compilar.

> **Para:** la sesión que retome esto desde WSL.
> **Estado:** cambios de Rust **escritos y verificados on-chain, sin compilar**
> (esta sesión corre en Windows; el build vive en WSL). Nada commiteado, nada
> pusheado.
> **Responde a:** `execution-market/docs/handoffs/2026-08-23-corte-autoria-reputacion-y-avalanche.md`

## Lo único que hay que hacer al retomar

```bash
cd /mnt/z/ultravioleta/dao/x402-rs
cargo fmt --all --check
cargo clippy -p x402-rs --features solana,near,stellar,algorand,sui,xrpl -- -D warnings
cargo test  --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1
```

Si eso queda verde: commitear, y **pushear sólo con OK explícito de Saul**
(push a `main` = deploy a producción, `ci.yaml`).

Los cuatro tests nuevos viven en `src/erc8004/relay.rs` y no tocan la red — son
puramente la tabla de direcciones:

| test | qué fija |
|---|---|
| `the_mainnet_delegates_are_the_verified_ones` | las 8 direcciones, en minúsculas, una por red |
| `the_chains_without_a_delegate_claim_none` | Avalanche, Fuji, Scroll, SKALE y los 5 testnets restantes siguen sin delegate |
| `optimism_and_monad_share_an_address_on_purpose` | que la coincidencia OP/Monad no se "arregle" después |
| `every_delegate_network_has_erc8004_contracts` | ninguna red promete delegate sin registries |

---

## Qué cambió, y por qué

**QUÉ:** `delegate_address()` en `src/erc8004/relay.rs` pasó de servir sólo
`base-sepolia` a servir además las 8 mainnets donde Execution Market desplegó
`FeedbackDelegate`.

**POR QUÉ:** el `ReputationRegistry` guarda `msg.sender` como autor. Como el
facilitador relaya el feedback para pagar el gas, **el 87,2% de la reputación de
Base figura emitida por nuestra wallet** (1.384 de 1.587 feedbacks) — y esa misma
wallet puede revocarla. El camino EIP-7702 ya estaba construido y probado contra
producción desde v1.74.0; lo único que faltaba era que EM desplegara los
contratos y nos pasara las direcciones. Ya lo hicieron.

**RIESGO:** una dirección equivocada acá manda una transacción tipo 4 a una
cuenta sin delegate detrás. En la EVM **un `.call()` a una dirección sin código
devuelve ÉXITO**, así que el fallo se vería como una calificación exitosa que no
calificó a nadie. Por eso ninguna dirección se copió del handoff sin leerla de la
cadena, y por eso `assert_delegate_usable()` la vuelve a verificar en cada
request.

### Los archivos

| archivo | cambio |
|---|---|
| `src/erc8004/relay.rs` | tabla `delegate_address()` con las 8 mainnets + doc reescrito + 4 tests |
| `src/handlers.rs` | `POST /feedback` (camino EVM viejo) ahora emite un `[WARN] DEPRECATED` cuando la red **sí** tiene delegate. **Warn-only, sin cambio de comportamiento** |
| `src/openapi.rs` | la disponibilidad decía "Base Sepolia only"; ahora lista las 9 redes y explica Avalanche |
| `VERSION` | 1.92.0 → **1.93.0** (prod estaba en 1.92.0, verificado con `curl /version`) |

`docs/CHANGELOG.md` no se tocó — lleva atrasado desde 1.64.0 y arreglarlo no es
parte de esto.

---

## La verificación on-chain, para que no haya que repetirla

Ninguna dirección se escribió sin leerla de su propia cadena, **en dos RPC
independientes cada una**, el 2026-08-23. Por cada red se comprobó lo mismo que
`assert_delegate_usable()` comprueba en runtime, más el chainId del nodo:

1. `eth_chainId` devuelve el chain id que esperamos (que el RPC no esté apuntando a otra cadena),
2. `eth_getCode(delegate)` no está vacío,
3. `REPUTATION_REGISTRY()` sobre el delegate devuelve el registry de mainnet,
4. `eth_getCode(registry)` no está vacío en esa cadena.

| red | chainId | delegate | RPCs que confirmaron |
|---|---|---|---|
| base | 8453 | `0x754206C4247317768bD86459E829a174d9C68BA4` | mainnet.base.org, base.drpc.org |
| ethereum | 1 | `0xbeCeA4673C0105aF63d02688Be6DE6CA51D57dd9` | ethereum-rpc.publicnode.com, eth.drpc.org |
| polygon | 137 | `0xf670C69BCbb2453FaE5Ec009c2b6dd934BE46A7f` | polygon.drpc.org, polygon-bor-rpc.publicnode.com |
| arbitrum | 42161 | `0x794C907FdfC71BFaF0b86D0e463BBD6E949A31bA` | arb1.arbitrum.io, arbitrum-one-rpc.publicnode.com |
| bsc | 56 | `0x9551263b9B83b1A737D55fd5e67Fb6D60e4eF787` | bsc-dataseed1.defibit.io, bsc-dataseed2.bnbchain.org |
| optimism | 10 | `0x825E997F2F7Ed5d3F59466cd754189fb19b62b82` | mainnet.optimism.io, optimism.drpc.org |
| monad | 143 | `0x825E997F2F7Ed5d3F59466cd754189fb19b62b82` | rpc.monad.xyz, monad.rpc.thirdweb.com |
| celo | 42220 | `0xe25cF9B9F5A3B5faa7628c751466df0166d96B59` | forno.celo.org, celo.drpc.org, celo-rpc.quickapi.com |

Las ocho devuelven el mismo registry de mainnet
`0x8004BAa17C55a88189AE136b182e5fdA19dE9b63`, y las ocho tienen **runtime code
byte a byte idéntico**: 1996 bytes, `sha256 = 82adb6272f0f3f88...`. Eso es
exactamente lo que predice un `immutable` de constructor con el mismo registry en
todas las mainnets, y es una señal barata de que no hay una cadena con un
delegate distinto disfrazado.

**Optimism y Monad comparten dirección.** No es un copy-paste: CREATE2 con el
mismo deployer, salt e init code aterriza en la misma dirección en las dos.
Verificado por separado en cada cadena; hay un test que lo fija para que nadie lo
"corrija" después.

Script usado (efímero, en scratchpad, no commiteado): hace las 4 lecturas por RPC
en paralelo. Repetirlo es ~30 s si alguien duda.

> **Nota de método:** los RPC públicos devuelven **403 sin `User-Agent`**. El
> primer intento marcó 6 de 8 redes como inalcanzables y podía haberse leído como
> "el delegate no está". Un 403 no es un veredicto sobre la cadena.

---

## Avalanche: la respuesta es "nunca", no "todavía no"

EM midió `-32000 transaction type not supported` desde el nodo de la C-Chain.
Eso no es falta de tráfico: es el nodo rechazando el **tipo de transacción**. No
hay delegate que desplegar ahí porque no hay transacción tipo 4 que mandar.

**Del lado del facilitador no hay nada que construir.** La opción A de EM (rutear
la reputación de tasks pagadas en Avalanche a otra cadena) se resuelve entera en
`resolve_reputation_target()` de EM, con el flag que ya está ON en producción.
Nosotros recibimos un `/feedback/evm/prepare` que dice `network: base` y no nos
enteramos de que el pago fue en Avalanche — que es justo como debe ser: el pago y
la calificación son escrituras distintas.

Lo que sí hicimos es dejarlo escrito donde alguien lo va a leer antes de
equivocarse: hay un test (`the_chains_without_a_delegate_claim_none`) que falla si
alguien agrega Avalanche a la tabla, con el porqué en el comentario, y el OpenAPI
lo dice en prosa en vez de dejar que parezca un olvido.

**Precondición que le toca a EM y no a nosotros:** los agentId son per-chain, así
que un ratee de Avalanche necesita identidad ERC-8004 en la cadena destino. El
registro es gasless por nosotros y ya existe (`POST /register`).

---

## Lo que este cambio NO hace

- **No cierra el camino viejo.** `POST /feedback` sigue escribiendo con nosotros
  como autor en las 8 mainnets; ahora sólo lo grita en los logs. Cerrarlo es una
  decisión aparte (en SVM existe `ERC8004_ALLOW_FACILITATOR_AUTHORSHIP=false`;
  en EVM no hay switch todavía y no lo agregué). **El corte real ocurre cuando EM
  migre sus llamadas a `/feedback/evm/prepare` + `/submit`** — punto 3 de su
  handoff, y es trabajo de ellos.
- **No toca los 1.384 feedbacks históricos.** Las dos sesiones y el handoff de EM
  coinciden: revocarlos sería estrenar exactamente el poder del que nos estamos
  deshaciendo. Sigue esperando confirmación explícita de Saul.
- **No prende la fase 2 del gate** (`ERC8004_REQUIRE_PROOF=true`). Sigue
  bloqueada por falta de tráfico medido, no por código.
- **No agrega Scroll ni SKALE.** Scroll sirve ERC-8004 desde `5ac06380` pero no
  tiene delegate desplegado; SKALE tiene EVM anterior a Shanghai, así que 7702 no
  puede aterrizar ahí nunca.
- **`POST /feedback/response` sigue anónimo** y firmado por nosotros (hallazgo J,
  handoff del 2026-08-18 §4). No estaba en este alcance.

---

## Verificación después del deploy (cuando Saul autorice el push)

```bash
# 1) que la versión nueva esté viva
curl -s https://facilitator.ultravioletadao.xyz/version   # espera 1.93.0

# 2) que una mainnet ofrezca el delegate correcto
#    (rater = cualquier EOA; prepare no escribe nada en la cadena)
curl -s -X POST https://facilitator.ultravioletadao.xyz/feedback/evm/prepare \
  -H 'content-type: application/json' -d '{
    "network":"base",
    "feedback":{"agentId":"18896","rater":"0x0000000000000000000000000000000000000001",
                "value":100,"valueDecimals":0,"tag1":"quality","tag2":"api",
                "endpoint":"https://agent.example","feedbackUri":"https://example.com/f.json"}
  }' | jq '{delegate, chainId, delegated, deadline}'
# espera delegate = 0x754206C4247317768bD86459E829a174d9C68BA4, chainId = 8453

# 3) que Avalanche siga negándose EXPLÍCITAMENTE en vez de inventar una dirección
curl -s -X POST https://facilitator.ultravioletadao.xyz/feedback/evm/prepare \
  -H 'content-type: application/json' \
  -d '{"network":"avalanche","feedback":{"agentId":"1","rater":"0x0000000000000000000000000000000000000001","value":100,"valueDecimals":0,"tag1":"q","tag2":"a","endpoint":"e","feedbackUri":"u"}}'
# espera 400 con "no FeedbackDelegate is deployed there yet"

# 4) que el OpenAPI publique la lista nueva
curl -s https://facilitator.ultravioletadao.xyz/api-docs/openapi.json \
  | jq -r '.paths."/feedback/evm/prepare".post.description' | grep -A2 Availability
```

**Criterio de aceptación de punta a punta** (lo escribió EM y sigue siendo el
correcto): un rating nuevo aparece on-chain con `clientAddress` = wallet del
rater, comprobable con `getClients(agentId)`. Eso **no** lo podemos demostrar
solos — necesita que EM recolecte la firma del rater (punto 3 de su handoff). Lo
que este deploy demuestra es que el rail está servido en las 8 mainnets.

---

## Contexto que ahorra media hora

- El camino 7702 completo (digest, autorización, nonces, deadlines) ya estaba
  probado contra producción en Base Sepolia — ver
  `docs/handoffs/2026-08-18-erc8004-estado-y-pendientes.md` §3bis. Este cambio es
  **sólo la tabla de direcciones**; no hay lógica nueva que auditar.
- El P0 #1 del handoff de EM ("autenticar `POST /feedback/revoke`") **ya está en
  producción desde v1.74.0**: sin credenciales responde 401, y 404 si no hay
  token configurado. EM lo listó como pendiente porque su handoff no lo tenía al
  día.
- El punto 5 ("Solana no necesita nada de esto") **también está hecho**:
  `/feedback/solana/prepare` + `/submit`, con el rater como cuenta 0.
- O sea: de los 5 puntos de "Qué falta para el corte", los tres del facilitador
  (1, 2, 5) quedan cerrados con este commit. Los dos restantes (3 y 4) son de EM.
