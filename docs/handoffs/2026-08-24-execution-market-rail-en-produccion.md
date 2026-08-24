---
date: 2026-08-24
tags:
  - type/handoff
  - domain/identity
  - domain/blockchain
  - priority/p0
status: active
aliases:
  - El rail de autoría ya está vivo
  - Delegates servidos en las 8 mainnets
related-files:
  - src/erc8004/relay.rs
  - docs/handoffs/2026-08-23-respuesta-a-execution-market-corte-autoria.md
---

# El rail está vivo en las 8 mainnets. Falta lo suyo.

> **Para:** Execution Market
> **De:** el equipo del Facilitator (`x402-rs`)
> **Continúa:** `docs/handoffs/2026-08-23-respuesta-a-execution-market-corte-autoria.md`,
> que quedó escrito con el código sin compilar. Ese documento sigue siendo la
> respuesta punto por punto; **este sólo dice qué cambió al desplegarlo**.
> **Estado:** desplegado y verificado contra producción.

## Lo que cambió desde ayer

`v1.93.0` está viva. Lo que ayer eran direcciones verificadas en un archivo sin
compilar, hoy es un endpoint que responde.

```
facilitator.ultravioletadao.xyz/version -> 1.93.0
```

Y, esto es lo nuevo y lo que más les sirve: **los dos SDKs ya traen el rail**, así
que no hay que armar las llamadas a mano.

| paquete | versión | qué agrega |
|---|---|---|
| PyPI `uvd-x402-sdk` | **0.65.0** | `prepare_relayed_feedback()` + `submit_relayed_feedback()` |
| npm `uvd-x402-sdk` | **2.70.0** | `prepareRelayedFeedback()` + `submitRelayedFeedback()` |

Ambos exponen además `RELAYED_FEEDBACK_NETWORKS` / `supports_relayed_feedback()`
(`supportsRelayedFeedback()` en TS) para que puedan rutear **sin pagar un round
trip para recibir un 400**. Es una pista de ruteo, no la autoridad: el facilitador
vuelve a verificar el delegate contra la cadena en cada request.

`submit_feedback()` / `submitFeedback()` quedaron marcados deprecated en las redes
que tienen delegate. **Siguen funcionando** — no los cerramos, y no los vamos a
cerrar sin avisar.

## La verificación, contra el endpoint real

No es una relectura del código: `assert_delegate_usable()` consulta la cadena en
cada request, así que esta tabla vuelve a probar que las ocho direcciones tienen
código y están pinneadas al registry correcto.

```
base          0x754206c4…68ba4   chainId 8453
ethereum      0xbecea467…57dd9   chainId 1
polygon       0xf670c69b…46a7f   chainId 137
arbitrum      0x794c907f…a31ba   chainId 42161
optimism      0x825e997f…62b82   chainId 10
celo          0xe25cf9b9…96b59   chainId 42220
bsc           0x9551263b…ef787   chainId 56
monad         0x825e997f…62b82   chainId 143     <- misma que Optimism, a propósito
base-sepolia  0x3a680854…d3768   chainId 84532

avalanche / scroll / skale-base
  -> 400 "relayed feedback is not available on <red>: no FeedbackDelegate is deployed there yet"
```

Las tres que se niegan lo hacen **con un mensaje que nombra la causa**, en vez de
inventar una dirección. Importa por qué: en la EVM un `.call()` a una cuenta sin
código **devuelve éxito**, así que una entrada equivocada acá no se vería como un
error — se vería como una calificación exitosa que no calificó a nadie.

## Avalanche: cerrado como excepción permanente, y no les debemos nada

La respuesta corta: **nunca**, no "todavía no". Y del lado nuestro **no hay nada
que construir ni que pedirnos**.

Tres cosas, para que quede zanjado:

**1. Es el nodo rechazando el tipo de transacción, no falta de tráfico.** El
`-32000 transaction type not supported` que midieron con `rehearse_7702.py` es la
C-Chain diciendo que no acepta transacciones tipo 4. No hay delegate que
desplegar porque no hay transacción que mandar. Mientras no haya un upgrade de la
C-Chain que traiga EIP-7702, esto no cambia.

**2. Quedó escrito donde alguien lo va a leer antes de equivocarse.** No basta
con que Avalanche esté ausente de la tabla: una ausencia se lee como olvido y
alguien la "arregla" en seis meses. Entonces:

- hay un test, `the_chains_without_a_delegate_claim_none`, que **falla si alguien
  agrega Avalanche**, con el porqué en el comentario de arriba;
- el OpenAPI lo dice en prosa, no lo deja como omisión;
- los SDKs tienen el mismo test de los dos lados (`test_avalanche_is_out_and_stays_out`
  en Python, `leaves Avalanche out, and that is not a "not yet"` en TS).

**3. La opción A no nos toca, y eso es virtud del diseño, no suerte.** Cuando
ustedes ruteen la reputación de una task pagada en Avalanche hacia Base, nosotros
recibimos un `/feedback/evm/prepare` que dice `network: base` y **no nos enteramos**
de que el pago fue en Avalanche. El pago y la calificación son escrituras
independientes, en cadenas independientes. No hace falta un flag, ni una
excepción, ni una coordinación con nosotros. El pago se queda donde se hizo.

La única trampa real es la que ustedes ya identificaron: **los agentId son
per-chain**. Un ratee que sólo existe en Avalanche no tiene identidad en la cadena
destino y el rating no tiene a quién apuntarle. `POST /register` lo resuelve
gasless y ya existe — pero antes de automatizarlo, lean esto:

> `GET /identity/:network/owner/:address` contesta **404** y **503** por cosas
> distintas y no hay que colapsarlas. 404 es "esta dirección no tiene agente";
> 503 es "la consulta no llegó a un veredicto" y lleva `"retryable": true`.
> Persistir "no registrado" ante un 503 convierte una falla transitoria de RPC en
> una respuesta permanentemente equivocada — y sobre un camino de registro,
> mintea un agente duplicado a alguien que ya tenía uno. Nos pasó
> (INC-2026-07-21). Lo mismo con un `/register` que da timeout: no es un fallo,
> el mint puede haber aterrizado igual.

Sobre la opción B (`FeedbackAccount` con CREATE2): coincidimos en descartarla
salvo que mantener la reputación **en** Avalanche sea requisito duro, y hay un
costo que su handoff no menciona — nuestro camino verifica el delegate antes de
gastar gas, y un contrato por agente multiplica esa superficie por la cantidad de
agentes.

---

## Lo que falta, y es de ustedes

**Desplegar los contratos no cortó nada. Servirlos nosotros tampoco.** Mientras
`POST /feedback` siga siendo el camino que corre, seguimos siendo los autores por
más delegates que haya en las ocho cadenas. El corte ocurre cuando migren las
llamadas.

1. **Recolectar la firma del rater** (su punto 3). Es lo único que hace el corte.
   Con los SDKs publicados son dos llamadas: `prepare` → firmar el `digest` con la
   llave del rater → `submit`. La primera vez que un rater califica hay que mandar
   además la autorización EIP-7702 (`delegated: false` en la respuesta de
   `prepare`, y el `accountNonce` que necesita viene ahí mismo); a partir de la
   segunda va sin ella.

2. **La regla de Avalanche** (su punto 4): una línea en
   `resolve_reputation_target()`, con el flag que ya tienen ON en producción.

3. **Mandar el NETO, no el bruto**, en el `ProofOfPayment`. Sus pagos llevan
   **dos** `Transfer` — comisión y neto al agente; medimos 2600 + 17400 sobre un
   bruto de 20000. El gate necesita el neto que efectivamente recibe el payee, o
   contesta `proof_transfer_not_found`. Pendiente desde el 2026-08-18.

4. **Limpieza que ustedes mismos anotaron:** `_CROSS_CHAIN_EVM_NETWORKS` todavía
   lista `skale`. Confirmamos que SKALE no puede soportar 7702 **nunca** — su EVM
   es anterior a Shanghai — así que sale por dos razones, no una.

## Lo que sigue abierto de nuestro lado

- **El camino viejo sigue abierto en EVM.** `POST /feedback` sigue escribiendo con
  nosotros como autor en las 8 mainnets; desde v1.93.0 **lo grita en los logs** en
  cada llamada (`[WARN] DEPRECATED`). No le pusimos switch de apagado en EVM (en
  SVM sí existe: `ERC8004_ALLOW_FACILITATOR_AUTHORSHIP=false`). Cerrarlo el día
  del deploy habría roto sus integraciones sin aviso. **Avísennos cuando hayan
  migrado y lo cerramos.**
- **La fase 2 del gate** (`ERC8004_REQUIRE_PROOF=true`) sigue apagada, y no por
  código: el gate **no ha visto ni una submission** desde que está vivo. Prenderla
  hoy sería enforcear sin haber medido nada.
- **`POST /feedback/response` sigue anónimo** y firmado por nosotros. Es la misma
  forma del problema del revoke — un POST sin credenciales nos hace firmar — sólo
  que en vez de destruir reputación ata nuestra identidad on-chain a contenido de
  un tercero. **La autoría real ahí necesita un cambio en su contrato:** el
  `FeedbackDelegate` admite dos selectores (`giveFeedback`, `revokeFeedback`), no
  `appendResponse`. Si van a ampliarlo, este es el momento de decidirlo.

## De Saul, no de ninguno de los dos equipos

- Los **1.384 feedbacks históricos** con autoría nuestra. Las tres partes
  coincidimos en no tocarlos — revocarlos sería estrenar exactamente el poder del
  que nos estamos deshaciendo. Falta la confirmación explícita.
- Si el revoke queda admin-only para siempre, o se reabre cuando el rater pueda
  firmar el suyo vía delegate.

---

## El criterio de aceptación sigue siendo el que escribieron ustedes

Un rating nuevo aparece on-chain con `clientAddress` = wallet del rater,
comprobable con `getClients(agentId)`.

**Eso no lo podemos demostrar solos** — necesita el punto 1. Lo que este deploy
demuestra es que el rail está servido y verificado en las ocho mainnets, y que
los SDKs lo exponen en las dos lenguas.

Los deadlines siguen cortos a propósito (15 min por defecto,
`ERC8004_RELAY_DEADLINE_SECS`): `relayFeedback` es permissionless por diseño, así
que una autorización firmada está viva en la naturaleza hasta que expire.

Una advertencia de método que casi nos cuesta el resultado, por si repiten la
verificación on-chain: **los RPC públicos devuelven 403 sin `User-Agent`**.
Nuestro primer barrido marcó 6 de 8 redes como inalcanzables. Un 403 no es un
veredicto sobre la cadena.
