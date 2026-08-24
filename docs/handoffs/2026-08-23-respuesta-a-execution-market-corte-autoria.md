---
date: 2026-08-23
tags:
  - type/handoff
  - domain/identity
  - domain/blockchain
  - priority/p0
status: active
aliases:
  - Respuesta al corte de autoría
  - Los 8 delegates entran al facilitador
---

# Los ocho delegates entraron. Avalanche queda como excepción permanente.

> **Para:** Execution Market
> **De:** el equipo del Facilitator (`x402-rs`)
> **Responde a:** *"Que los ratings dejen de salir a nombre del Facilitator"*,
> 2026-08-23
> **Estado:** código escrito y direcciones verificadas on-chain. **Sin compilar
> todavía** — la máquina de build (WSL) está caída; el build y el deploy salen
> apenas se recupere. Nada de esto está en producción aún.

## Los tres puntos que les tocaban a ustedes de nuestro lado

Su lista "Qué falta para el corte" tiene 5 puntos. Los 3 nuestros están
cerrados — y dos de ellos ya lo estaban antes de que llegara su handoff.

| # | punto | estado |
|---|---|---|
| 1 | Autenticar `POST /feedback/revoke` | **ya estaba en producción desde v1.74.0** (2026-08-18) |
| 2 | Armar la tx tipo 4 | **construido desde v1.74.0**; hoy le entraron sus 8 mainnets |
| 5 | Solana — tx parcialmente firmada | **ya estaba**: `/feedback/solana/prepare` + `/submit` |

### Punto 1 — ya no hace falta

`POST /feedback/revoke` es **admin-only** desde v1.74.0. Medido contra producción
entonces: un POST anónimo respondía **500** (o sea: había atravesado todas las
capas y llegado a la rutina que firma on-chain); hoy responde **401**. Y si no hay
token configurado la ruta responde **404**, indistinguible de una ruta que no
existe — fail-closed a propósito, para que desplegarla no la deje abierta por
omisión.

Tenían razón en que `require_writer_lease` no era autenticación. Lo era, y por eso
se cerró hace cinco días. El gate es `ERC8004_ADMIN_TOKEN`, deliberadamente **no**
`BAZAAR_ADMIN_TOKEN`: borrar reputación de terceros no debe compartir credencial
con nada más. Hay un test que falla si alguien los une.

### Punto 2 — sus 8 direcciones ya están en la tabla

Ninguna se copió del handoff. Cada una se leyó de su propia cadena, **en dos RPC
independientes**, comprobando las cuatro cosas que el facilitador vuelve a
comprobar en cada request: que el chainId del nodo sea el que decimos, que el
delegate tenga código, que su `REPUTATION_REGISTRY()` devuelva el registry de
mainnet, y que **el registry también tenga código ahí**.

Las ocho pasaron. Las ocho devuelven `0x8004BAa17C55a88189AE136b182e5fdA19dE9b63`
y las ocho tienen runtime code byte a byte idéntico — 1996 bytes,
`sha256 = 82adb6272f0f3f88...`. Buen trabajo: es exactamente lo que predice el
`immutable` de constructor con un registry compartido, y significa que no hay una
cadena con un delegate distinto colado en el lote.

Dos cosas que confirmamos porque parecían errores y no lo son:

- **Optimism y Monad comparten dirección** (`0x825E997F…2b82`). CREATE2 con el
  mismo deployer, salt e init code. Verificado por separado en cada cadena, y con
  un test que lo fija para que nadie lo "arregle" en seis meses.
- **Los RPC públicos devuelven 403 sin `User-Agent`.** Nuestro primer barrido
  marcó 6 de 8 redes como inalcanzables. Si alguien de su lado repite la
  verificación y ve 403, no es que el delegate no esté.

Cuando esto deploye, `/feedback/evm/prepare` responde en `base`, `ethereum`,
`polygon`, `arbitrum`, `optimism`, `celo`, `bsc`, `monad` y `base-sepolia`. El
resto sigue contestando 400 explícito en vez de inventar una dirección.

### Punto 5 — Solana ya estaba, y verificado

`/feedback/solana/prepare` devuelve la transacción **sin firmar**, con 2 firmas
requeridas, nuestra wallet como fee payer y **la cuenta 0 — el `client` que el
programa lee como autor — siendo el RATER**. Verificado decodificando la respuesta
de producción en `solana-devnet` el 2026-08-18. `/submit` rechaza cualquier
transacción que no sea byte a byte la que armó.

Un detalle por si repiten la decodificación: la transacción de Solana se
serializa con `short_vec` (compact-u16), no con prefijo de 8 bytes. Un decoder que
asuma `u64` corre todos los offsets siete bytes y devuelve basura que *parece* un
header válido.

---

## Avalanche: de acuerdo con la opción A, y del lado nuestro no hay nada que construir

Su lectura es correcta y su recomendación también. Tres cosas que agregamos:

**1. Coincidimos en tratarla como permanente.** `-32000 transaction type not
supported` es el nodo rechazando el tipo de transacción, no ausencia de tráfico.
No hay delegate que desplegar porque no hay tx tipo 4 que mandar. Lo dejamos
escrito donde alguien lo va a leer antes de equivocarse: un test que **falla si
alguien agrega Avalanche a la tabla**, con el porqué en el comentario, y el
OpenAPI diciéndolo en prosa en vez de dejar que parezca un olvido.

**2. La opción A no nos toca en absoluto, y eso es una virtud del diseño.**
Cuando ustedes ruteen la reputación a Base, nosotros recibimos un
`/feedback/evm/prepare` que dice `network: base` y **no nos enteramos** de que el
pago fue en Avalanche. El pago y la calificación son escrituras independientes.
No hay que pedirnos nada, ni un flag, ni una excepción en nuestro lado.

**3. La precondición que mencionan es la única trampa real.** Los agentId son
per-chain: un ratee que sólo existe en Avalanche no tiene identidad en Base, y el
rating no tiene a quién apuntarle. `POST /register` lo resuelve gasless y ya
existe — pero hay algo que conviene saber antes de automatizarlo:

> **`GET /identity/:network/owner/:address` contesta 404 y 503 por cosas
> distintas y no hay que colapsarlas.** 404 es "esta dirección no tiene agente";
> 503 es "la consulta no llegó a un veredicto" y lleva `"retryable": true`.
> Persistir "no registrado" ante un 503 convierte una falla transitoria de RPC en
> una respuesta permanentemente equivocada — y sobre un camino de registro, mintea
> un agente duplicado a alguien que ya tenía uno. Nos pasó (INC-2026-07-21). Lo
> mismo con un `/register` que da timeout: no es un fallo, el mint puede haber
> aterrizado igual.

Sobre la opción B (`FeedbackAccount` con CREATE2): coincidimos en descartarla
salvo que mantener la reputación **en** Avalanche sea requisito duro. Y hay un
costo que su handoff no menciona: nuestro camino 7702 verifica que el delegate
tenga código y esté pinneado al registry correcto antes de gastar gas. Un contrato
por agente multiplica esa superficie por la cantidad de agentes.

---

## Lo que sigue abierto, y de quién es

**De ustedes (sin cambios respecto a su handoff):**

- **Punto 3 — recolectar la firma del rater.** Es lo que de verdad hace el corte.
  Mientras `POST /feedback` siga siendo el camino que corre, seguimos siendo los
  autores por más delegates desplegados que haya. **Desplegar los contratos no
  cambió nada por sí solo, y servirlos nosotros tampoco.**
- **Punto 4 — la regla de Avalanche**, una línea de configuración.
- **Mandar el neto, no el bruto**, en el `ProofOfPayment`. Sus pagos llevan dos
  `Transfer` (comisión + neto al agente; medimos 2600 + 17400 sobre un bruto de
  20000). El gate necesita el neto que recibe el payee, o contesta
  `proof_transfer_not_found`. Sigue pendiente desde el 2026-08-18.
- **Limpieza que ustedes mismos anotaron:** `_CROSS_CHAIN_EVM_NETWORKS` todavía
  lista `skale`. Confirmamos que SKALE no puede soportar 7702 nunca — su EVM es
  anterior a Shanghai — así que sale por dos razones, no una.

**Nuestro, y lo decimos para que no lo den por hecho:**

- **El camino viejo sigue abierto en EVM.** `POST /feedback` sigue escribiendo
  con nosotros como autor en las 8 mainnets; con este cambio ahora **lo grita en
  los logs** en cada llamada. No le pusimos switch de apagado en EVM (en SVM sí
  existe: `ERC8004_ALLOW_FACILITATOR_AUTHORSHIP=false`). Cerrarlo el día que
  desplegamos habría roto sus integraciones sin avisar. **Díganos cuándo hayan
  migrado y lo cerramos.**
- **La fase 2 del gate** (`ERC8004_REQUIRE_PROOF=true`) sigue apagada. No por
  código: el gate **no ha visto ni una submission** desde que está vivo (0
  veredictos en toda la ventana de retención de logs, contra ~0,5 feedbacks por
  día). Prenderla hoy sería enforcear sin haber medido nada.
- **`POST /feedback/response` sigue anónimo** y firmado por nosotros. Es la misma
  forma del problema del revoke — un POST sin credenciales nos hace firmar — sólo
  que en vez de destruir reputación ata nuestra identidad on-chain a contenido de
  un tercero. **La vía de autoría real ahí necesita un cambio en su contrato**: el
  `FeedbackDelegate` admite dos selectores (`giveFeedback`, `revokeFeedback`), no
  `appendResponse`. Si van a ampliarlo, este es el momento de decidirlo.

**De Saul:**

- Los 1.384 feedbacks históricos con autoría nuestra. Las tres partes coincidimos
  en no tocarlos; falta la confirmación explícita.
- Si el revoke queda admin-only para siempre, o se reabre cuando el rater pueda
  firmar el suyo vía delegate.

---

## Cómo comprobarlo cuando deploye

```bash
curl -s https://facilitator.ultravioletadao.xyz/version   # 1.93.0

curl -s -X POST https://facilitator.ultravioletadao.xyz/feedback/evm/prepare \
  -H 'content-type: application/json' -d '{
    "network":"base",
    "feedback":{"agentId":"18896","rater":"<EOA del rater>",
                "value":100,"valueDecimals":0,"tag1":"quality","tag2":"api",
                "endpoint":"https://agent.example","feedbackUri":"https://example.com/f.json"}
  }' | jq '{delegate, chainId, delegated, deadline, nonce}'
```

`delegated: false` significa que la primera vez hay que mandar también la
autorización 7702 firmada (`accountNonce` viene en la respuesta); las siguientes
van sin ella.

Y el criterio de aceptación es el que ustedes escribieron, que sigue siendo el
correcto: un rating nuevo con `clientAddress` = wallet del rater, comprobable con
`getClients(agentId)`. **Ese no lo podemos demostrar solos** — necesita el punto 3.
Nosotros llegamos hasta dejar el rail servido y verificado en las 8 cadenas.

Los deadlines siguen cortos a propósito (15 min por defecto,
`ERC8004_RELAY_DEADLINE_SECS`): `relayFeedback` es permisionless por diseño, así
que una autorización firmada está viva en la naturaleza hasta que expire. Esa
mitigación quedó de nuestro lado y sigue aplicada.
