# Respuesta a KarmaKadabra — el riel de Solana, y sus tres hallazgos

**Para:** equipo de KarmaKadabra
**De:** facilitador (x402-rs), 2026-08-18
**Responde a:** `karmakadabra/docs/handoffs/2026-08-17-dx402-respuesta-a-facilitador.md`

Verifiqué todo lo que reportaron. Casi todo era correcto; hay dos correcciones que
les ahorran perseguir la pista equivocada.

---

## 0. TL;DR

- **El riel de Solana**: confirmado y arreglado en **v1.81.0**, ya desplegado.
  **No era el RPC** — es premium (QuickNode), no público, y estaba sano. Y **no
  estaba muerto: estaba intermitente** (2 settles exitosos contra 10 timeouts en
  24h), que es un diagnóstico distinto.
- **Sus tres hallazgos menores**: los tres correctos. Dos arreglados en el SDK
  **0.50.0**; el tercero es documentación de ustedes.
- **Su corrección al §4.4**: correcta e importante. Ya está en la guía.
- Gracias por publicar 0.48.0. Ese fue un error nuestro y era real.

---

## 1. El riel de Solana

### Lo que medimos

Sus 10 `confirmation timed out` en 24h: confirmados en
`/ecs/facilitator-production`. Pero además hubo **2 settles de Solana exitosos en
la misma ventana**. El riel no estaba caído; fallaba ~5 de cada 6 veces.

Esa distinción importa: un riel muerto apunta a config, uno intermitente apunta a
timing.

### Lo que NO era

**No es el RPC.** `RPC_URL_SOLANA` es un endpoint premium de QuickNode, no uno
público. `getHealth` responde `ok` y `getLatestBlockhash` contesta al instante.
Su hipótesis del rate-limit era razonable con la información que tenían, pero
descartada.

### Lo que sí era

`send()` usaba **`skip_preflight: true`**.

Con eso el RPC **no valida nada** — ni el blockhash, ni las firmas — y devuelve
una firma igual. Una transacción que no puede aterrizar se ve **idéntica** a una
que sí: firma, treinta segundos de silencio, timeout nombrando una firma que no
existe on-chain.

Es exactamente la forma que observaron. Y explica por qué su simulación pasaba:
la simulación valida, el envío con `skip_preflight` no.

### Qué cambió en v1.81.0

| Cambio | Por qué |
|---|---|
| **Preflight ENCENDIDO** por defecto | El mismo fallo ahora vuelve al instante diciendo *qué* fue ("blockhash not found", "signature verification failure") en vez de un misterio de 30s. `SOLANA_SKIP_PREFLIGHT=true` restaura lo viejo |
| **Ventana 30s → 90s** | Un blockhash de Solana vive ~150 slots (~60–90s). Rendirse a los 30 abandonaba transacciones que **todavía podían aterrizar**, y les decía que el pago falló mientras seguía en vuelo |
| **Una última lectura de estado** antes de declarar el fallo | *"TX may have been submitted"* es la peor respuesta posible: la plata pudo moverse mientras al vendedor se le dice que no |
| `max_retries` 5 → 20 | Re-forwarding a leaders bajo congestión |

**Lo importante para ustedes**: si vuelve a fallar, el error ahora les va a decir
la causa. Y si aparece `no pude leer el estado`, **revisen la firma on-chain antes
de reintentar** — ese es el único caso donde el pago pudo haber ocurrido.

**Honestidad sobre el alcance**: los tres cambios hacen visible la causa raíz y
dejan de reportar un resultado equivocado. **No afirmo haber probado cuál era la
causa exacta** — no pude reproducirlo en vivo. Si con preflight encendido siguen
viendo fallos, ahora el mensaje va a nombrar el motivo, y con eso lo cerramos.

---

## 2. Sus tres hallazgos

### (a) `payer_key_from_solana_address("")` — correcto, arreglado en 0.50.0

Confirmado, y la causa es fea: el decodificador base58 de fallback hacía
`rjust(32)` sobre un decode corto, así que una address vacía producía `0100…`.

**Una precisión sobre el impacto**, porque cambia la urgencia: el cuerpo **no**
quedaba "sellado hacia la nada con todos los logs en verde". `0100…` es un punto
Curve25519 de **orden pequeño**, y la defensa RFC 7748 que tenemos lo rechaza — el
sellado fallaba. Pero fallaba **una capa después** y como un
`Error computing shared key` genérico que no apunta ni cerca de la causa.

O sea: no era el fallo mudo que temían, pero sí un fallo mal ubicado y mal
explicado. Motivo suficiente. Ahora se rechaza cualquier cosa que no decodifique a
exactamente 32 bytes, en el lugar correcto.

Su blindaje del lado de ustedes sigue siendo buena práctica; ya no es necesario.

### (b) `dx402.__all__` sin el lado vendedor — correcto, arreglado en 0.50.0

Confirmado. Cosmético hasta que alguien usa `import *`, y entonces deja de serlo.

### (c) El máximo real de compute unit price es 1.000.000 — correcto

Confirmado en `src/chain/solana.rs:261`: el default de Solana mainnet es
`1_000_000`. Los 5.000.000 no están en ningún lado de nuestro código.

`SOLANA_SPEC.md` no existe en x402-rs, así que ese documento es de ustedes o de
otro repo. **La fuente autoritativa es `max_compute_unit_price_from_env`**, y es
configurable por red con `X402_SOLANA_MAX_COMPUTE_UNIT_PRICE_SOLANA`.

Su punto sobre `Payment failed: Unknown error` es justo: un vendedor que solo
reenvía el error tapa uno perfectamente claro. Eso es del lado del vendedor, pero
vale como advertencia general.

---

## 3. Su corrección al §4.4 — correcta, y va a la guía

Tienen razón y es un punto que no habíamos visto:

> La cadena más fácil para el VENDEDOR puede ser la imposible para el COMPRADOR.

En Solana la address **es** la clave pública, así que sellar es trivial. Pero
descifrar necesita los 32 bytes de la privada, y una wallet **en custodia** (Paybox
en su caso) firma pero **no hace ECDH**. Su comprador no puede abrir su propia
evidencia.

Esto es exactamente el caso de uso del modo `escrowed` que dejamos sin
implementar. Lo anotamos con su nombre.

Mientras tanto, para probar el ciclo completo necesitan un comprador cuya clave
privada controlen directamente — que es lo que ya tienen con el desechable.

---

## 4. El SDK 0.48.0

Tienen razón y fue error nuestro: los commits existían solo en el disco de esta
sesión. El paso 1 del handoff no podía funcionar para nadie. Gracias por
publicarlo.

Desde entonces: **0.49.0** (leer envelopes multi-destinatario) y **0.50.0** (sus
dos bugs). `pip install -U 'uvd-x402-sdk[dx402]>=0.50.0'`.

---

## 5. Qué hay de nuevo que les sirve

Además de los arreglos, desde su handoff salieron dos cosas:

**El gate del anchor** (v1.78.0). `/dx402/anchor` ahora verifica contra la cadena
que el pago existe, que el payer es la address a la que cifraron, y que el anchor
viene firmado por el payee. Un pago ancla **una sola vez**.

Está en **fase 1** (verifica y reporta, no rechaza), así que sus pruebas con
`txHash` sintético siguen pasando. **En Solana el gate reporta
`unverifiable_chain` y nunca bloquea** — el chequeo on-chain de Solana está en el
backlog como el siguiente, y es el único no-EVM priorizado.

**Evidencia bidireccional** (v1.79.0). El envelope puede llevar `payer` + `seller`,
así que ustedes como vendedores pueden abrir su propia evidencia y responder a un
"eso no es lo que me mandaste" falso. El body se cifra una vez; sumar al vendedor
cuesta ~60 bytes. Un envelope de un solo pagador se sigue emitiendo v1 byte por
byte, así que **nada de lo que ya anclaron se volvió ilegible**.

---

## 6. Estado

| Qué | De quién | Estado |
|---|---|---|
| Que el settle de Solana propague | nosotros | **v1.81.0 desplegado** — reintenten |
| Los dos bugs del SDK | nosotros | **0.50.0 publicado** |
| El máximo de compute unit price en la doc | ustedes | es 1.000.000 |
| Correr `dx402_pago_real.py` | ustedes | desbloqueado |
| DX402 en el decorador de los 5 sellers | ustedes | bloqueado por lo suyo |
| Gate del anchor en Solana | nosotros | backlog, priorizado |
| Modo `escrowed` para wallets en custodia | nosotros | backlog, con su caso anotado |

El §8 con transacción real debería ser un comando. Avísennos cómo va — y si
vuelve a fallar, manden el error nuevo: ahora dice la causa.
