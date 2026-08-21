---
date: 2026-08-20
tags:
  - type/handoff
  - domain/payments
  - priority/p0
status: active
---

# Respuesta al handoff de Execution Market

> **Para:** Execution Market
> **De:** el equipo del Facilitator (`x402-rs`)
> **Responde a:** *"Handoff al equipo del Facilitator — tres episodios de
> degradación en 10 días"*, 2026-08-20

## Antes que nada: gracias, y tenían razón en lo que importaba

El handoff de ustedes disparó una auditoría completa del facilitador y encontramos
más de lo que buscábamos. **Tres de sus cinco recomendaciones eran correctas**, una
apuntaba al síntoma correcto por el mecanismo equivocado, y una era una pista falsa
—pero una pista falsa que valía la pena seguir, porque nos llevó a otra cosa.

Y hay algo que les debemos decir de entrada: **parte de sus fallos eran nuestros y
no tenían nada que ver con la degradación.** Está abajo, en "Lo que ustedes no
podían ver".

## Punto por punto

### 1. El writer lease — acertaron el síntoma, el mecanismo es otro

Preguntaron: *"¿Cuántas instancias corren, y qué pasa con una request que llega a
la que no tiene el lease?"*

**Corre UNA sola instancia.** `desired_count=1`, y el servicio **nunca ha
autoescalado** — existe una política de autoescalado, pero escala por CPU al 75% y
la CPU nunca pasó del 25% en ninguno de los tres episodios. Un servicio que se
pasa la vida esperando respuestas de RPC no mueve un autoescalador de CPU.

Así que no era contención entre réplicas compitiendo. El mecanismo real:

```
18:25:40.933Z  arranca la task nueva
18:26:09.695Z  registered 1 targets          <- el ALB YA le manda trafico
18:26:37-38Z   3x "does not hold the EVM writer lease"
18:26:51.339Z  ECS: "has stopped 1 running tasks"
18:27:36.991Z  la vieja: "Released EVM writer lease"   <- 45,6s despues
18:27:39.303Z  la nueva: "Acquired EVM writer lease"   <- recien aca puede escribir
```

**89,6 segundos** entre "el balanceador la considera sana" y "tiene permiso de
escribir". En las cinco tasks del período, el 100% de los rechazos ocurre antes de
su propio `Acquired`. Dos defectos encadenados: `/health` no sabe nada del lease, y
el release espera a que drenen las requests en vuelo.

**Y acá está lo que les va a interesar:** las requests en vuelo que retrasan ese
release **son settles esperando confirmación on-chain**. Cuanto más lento el
settle, más larga la ventana sin escritor. **Un incidente de latencia alarga su
propia ventana de deploy.**

Respondiendo lo que faltaba:
- **¿TTL que venza con una tx en vuelo?** TTL 15s, renovación cada 5s. Descartado
  como causa: cero eventos de pérdida de lease en dos días completos.
- **¿Es retryable de nuestro lado?** No, y es un defecto: en la ruta de escrow les
  devolvemos el error **crudo** con un 400. Eso les dice "el problema es tuyo"
  cuando es nuestro.

### 2. `txpool is full` en polygon y monad — acá se equivocaron, y nosotros también

Medimos los eventos de `txpool is full` en la ventana del 19-20: **fueron 7, y los
7 en Celo.** Cero en monad. Cero en polygon.

Y polygon está **limpio**: de 16 transacciones emitidas, 16 minadas.

Pero **acertaron en el fix**: no hay retry con backoff para ese código. El
`RetryBackoffLayer` existe pero su política solo cubre códigos de rate-limit; el
`-32003` no está. Falla duro al primer intento. Lo estamos arreglando.

**Monad sí tiene un problema grave, pero es otro:** de 181 transacciones emitidas
en esa ventana, **~159 no existen en la cadena** — 88% de pérdida, verificado
contra dos RPC independientes. El nonce del signer avanzó de 254 a 276 mientras se
emitían 181. Todas devolvieron un hash. **Todavía no sabemos qué las tira.**
Descartamos falta de fondos (monad tiene ~1.400 settles de gas) y descartamos el
piso de gas (alloy firma a 202 gwei contra un piso de 100).

### 3. Las tres alarmas mudas — son cinco, y las que hablan les hablan a ustedes

Confirmado y peor de lo que reportaron. De 8 alarmas del facilitador, **5 tienen
`AlarmActions: []`**. Las otras 3 sí avisan — **al topic `em-production-mcp-alerts`,
el de ustedes.**

Es decir: **nos enteramos de nuestras propias caídas por los correos de ustedes.**
Eso explica por qué su handoff fue la primera noticia de los tres episodios.

Ninguna de las 8 cubre CPU ni HTTP 460. Está en nuestra lista arreglarlo con un
topic propio.

### 4. El umbral de p99 en 10s — confirmado

El episodio 3 corrió dos horas entre 5 y 8 segundos sin disparar nunca. Confirmado
tal cual lo describieron.

### 5. La alarma sobre el log en vez del código HTTP — confirmado, y nos falta a nosotros también

Su punto sobre el HTTP 460 nos aplica de lleno, y peor: **el ALB del facilitador
tiene `access_logs.s3` deshabilitado**, así que del lado nuestro el 460 no es ni
observable. Vamos a habilitarlo.

## Lo que ustedes no podían ver, y les afecta directo

### a) Nuestro timeout y el suyo empatan en 30 segundos exactos

`POST /settle` mantiene la conexión abierta esperando la confirmación on-chain:

```rust
// src/chain/evm.rs:663-679
Network::Ethereum => 900,   // 15 minutos
Network::Base     => 90,
_                 => 30,    // optimism, polygon, monad, celo...
```

Ese `30` es la mayoría de las cadenas que ustedes usan. **Y su
`FACILITATOR_TIMEOUT_SECONDS` es 30.** Se rinden en el mismo instante en que
nosotros nos íbamos a rendir.

**Sus 201 HTTP 460 son eso.** No es que el facilitador no respondiera: es que los
dos relojes vencen juntos. Su "latencia clavada en 30,1-30,8s" es literalmente
nuestro timeout de recibo.

Un valor de cliente por encima de 35s les cambiaría el resultado hoy mismo, sin
esperar ningún fix nuestro. No es la solución de fondo —estamos evaluando un
`/settle` asíncrono con 202 + polling— pero es gratis y es de ustedes.

### b) La wallet de Celo estaba sin gas

```
0,028436 CELO disponibles   ·   0,1134 CELO por settle de escrow
440 errores "insufficient funds" el 20-ago entre 18:00 y 21:45
409 de 973 settlements de escrow fallidos en 24 horas
```

**Buena parte de sus fallos de Celo no eran degradación ni escrow: era una wallet
vacía.** Ya está recargada (342,8 CELO). Y no había ninguna alarma de balance — el
dato se mide y se publica en nuestra landing page, y nadie lo vigilaba.

### c) Sui mainnet estaba caído hace tiempo

El endpoint que teníamos configurado dejó de servir JSON-RPC por completo
(`-32601` a los seis métodos que probamos, incluido el handshake del SDK). **Sui
mainnet no podía liquidar nada.** Arreglado hoy.

Si tienen tráfico de Sui, revisen qué pasó con él.

### d) Dos transacciones se minaron mientras les devolvimos un 400

`0x097890ad379cacbb…` (monad, block 97453656) y `0xebc487425044186f…` (base,
block 50159925). **Plata movida, error reportado.**

Si su reconciliación se fía del código HTTP para decidir si un pago ocurrió, esos
dos casos quedaron mal contabilizados de su lado. Vale la pena que revisen si hay
más: es el modo de falla más caro que encontramos y no sabemos cuántos son.

## Lo que estamos haciendo

**Ya desplegado** (commit `f5bc8500`):
- El RPC de Sui, mainnet y testnet.
- Celo recargada.

**En camino, por orden:**
1. Health check y alarma de balance **por cadena** — Celo y Sui se cayeron sin que
   nadie se enterara, por causas distintas. No hay vigilancia de ningún tipo.
2. `alarm_actions` en las 5 alarmas mudas, con topic propio. Dejamos de depender
   del de ustedes.
3. Devolver el nonce ante `txpool is full`, y **después** —nunca antes— marcarlo
   retryable con 502 + `Retry-After`. En ese orden: si lo hacemos retryable primero,
   ustedes reintentan más y cada reintento agrava el problema.
4. Timeouts explícitos en las llamadas RPC. Hoy no hay ninguno en el camino de
   pago; un RPC colgado sostiene la request hasta los 600s del ALB.
5. Bajar el umbral de latencia y habilitar los access logs del ALB.

**Evaluando:** `/settle` asíncrono (202 + `jobId` + polling), que subiría la
capacidad de 0,02-10 req/s según la cadena a ~10-13 req/s parejo. Es un cambio de
contrato, así que si lo hacemos va a ser opt-in por header y lo conversamos con
ustedes antes.

## Lo que les pedimos

1. **Su `FACILITATOR_TIMEOUT_SECONDS`** — ¿lo pueden subir por encima de 35s? Es la
   mitigación más barata que existe hoy para los 460.
2. **Los request ids de sus 502** de la ventana 21:49→01:53, si los tienen. Con
   eso podemos cruzar contra nuestros logs y cerrar el 88% de monad.
3. **Si tienen tráfico de Sui**, díganos qué vieron — estuvo caído y no sabemos
   desde cuándo.

Y una devolución honesta sobre su propio handoff: **la lección del modo 460 es la
mejor cosa que leímos esta semana.** "Falla rápida → visible, falla lenta →
invisible" describe exactamente por qué nuestras alarmas no vieron ninguno de los
tres episodios. Nos la robamos.

## Referencia

El diagnóstico completo, con las doce hipótesis que descartamos y por qué, está en
`docs/handoffs/2026-08-20-diagnostico-performance-facilitador.md` de nuestro repo.
Incluye lo que **no** logramos explicar, que es tan útil como lo que sí.
