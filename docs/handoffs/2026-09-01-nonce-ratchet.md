---
date: 2026-09-01
tags:
  - type/handoff
  - domain/evm
  - priority/p0
status: active
---

# El trinquete del nonce: por qué reiniciar lo curaba diez minutos

> **Síntoma:** `/settle` devolviendo 502 en ráfagas, con `nonce too high` en los
> logs. Dos reinicios y dos deploys lo "arreglaron" ese día. Ninguno lo arregló.

## La frase que lo resume

**El resync que existía para curar el nonce era lo que lo rompía.** Cada fallo
resincronizaba a `high_water + 1`, que es uno PAST el nonce que acababa de
fallar. Un trinquete que sube y nunca vuelve.

## Cómo se ve

Medido en producción el 2026-09-01, minutos después de que un reinicio hubiera
limpiado el estado:

```
22:59:39   tx 1556  state 1555    brecha de 1, benigna
22:59:40   tx 1587  state 1555    +31 en UN segundo
23:00:44   tx 1603  state 1555    brecha de 48, 65 segundos despues
```

El salto de 31 en un segundo es la parte que explica la velocidad: son intentos
concurrentes, cada uno pide un nonce, y cada uno sube el `high_water` al que el
siguiente se va a anclar.

## El mecanismo

`resync_target` (`src/chain/evm.rs`) tiene dos salidas:

```rust
(_, Some(last)) if last.elapsed() >= NONCE_TRUST_CHAIN_AFTER => pending,  // sana
(Some(high_water), _) if pending <= high_water => high_water + 1,          // trinquete
```

La primera confía en la cadena y sana — **pero sólo tras 120 segundos sin
asignar nonces**. Bajo una ráfaga de fallos continua, cada intento fallido
asigna un nonce y refresca `last_allocated`, así que esa ventana nunca
transcurre. Siempre cae en la segunda.

El diseño asume que los fallos son lo bastante raros como para que haya pausas.
Con fallos continuos no hay pausa.

## El disparador

Dos cosas ese día, ninguna de ellas el trinquete:

1. **La wallet de Ethereum se quedó sin gas** (0,000042 ETH). Su alarma estuvo
   en ALARM 42 horas sin que nadie actuara. Cada settle fallido alimentaba el
   trinquete.
2. `execution reverted: ERC20: transfer amount exceeds balance` — alguien
   mandando settles sin fondos suficientes. Mismo efecto.

Fondear Ethereum resolvió (1) pero no el trinquete: para entonces ya estaba
enganchado y se sostenía solo.

## El arreglo

**`nonce too high` es la única señal de nonce que dice quién está equivocado.**
`nonce too low` y `replacement underpriced` significan que la cadena está en o
pasado nuestro nonce — algo nuestro aterrizó, y el `high_water` que protege a un
hermano en vuelo sigue siendo válido. `nonce too high` dice lo contrario: el nodo
no tiene registro de los nonces entre su estado y el nuestro, así que la marca
está clavada en transacciones que no se van a minar nunca.

`resync_to_chain()` descarta la marca; `reset_nonce()` la sigue preservando. Son
dos recuperaciones para dos evidencias distintas y colapsarlas desharía la
protección contra rebobinar por debajo de un hermano en vuelo.

**El riesgo del arreglo**, dicho explícito: un hermano que este nodo todavía no
vio haría que dos transacciones reclamen el mismo nonce y fallen como
`replacement transaction underpriced` — un error **reintentable**, que el bucle
de reintentos ya maneja. El riesgo de no arreglarlo es un firmante trabado hasta
que alguien note y reinicie. Ese es el canje.

## Lo que no sirve

**Reiniciar.** El estado del nonce vive en memoria, así que arranca con
`high_water = None` y lee la cadena. Compra unos diez minutos. Lo hicimos dos
veces ese día, más dos deploys que reiniciaron de paso; las cuatro veces pareció
sano y las cuatro volvió.

Y por eso el log del arreglo es `info!` y no `debug!`: producción corre en
`info`, y esta es la única señal de que el trinquete se rompió en vez de
sobrevivirse. Misma lección que `release_nonce` y que el incidente de p99 del
mismo día.

## Pendiente

- **El umbral de la alarma de balance de Ethereum está mal dimensionado**: 0,0035
  ETH, cuando un settle de escrow cuesta ~0,001-0,002. Avisa con dos settles de
  margen. Debería ser ~0,02 ETH.
- Verificar con tráfico real que la brecha ya no se abre. El criterio: cero
  `nonce too high` sostenido, y `resync_to_chain` apareciendo en los logs
  (`info!`) cuando alguna vez se dispare.
