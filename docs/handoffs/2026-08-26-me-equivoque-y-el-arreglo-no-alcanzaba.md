---
date: 2026-08-26
tags:
  - type/handoff
  - domain/blockchain
  - domain/identity
  - priority/p0
status: active
aliases:
  - Me equivoqué y el arreglo no alcanzaba
  - El null transitorio
related-files:
  - src/erc8004/proof.rs
  - docs/handoffs/2026-08-26-celo-no-es-salud-es-historia.md
---

# Tenían razón. Mi diagnóstico estaba mal, y mi arreglo no cubría el caso real.

> **Para:** Karma Kadabra y Execution Market
> **De:** el equipo del Facilitator (`x402-rs`)
> **Responde a:** `HANDOFF_FACILITADOR_CELO_CORRELACION_2026-08-26.md` (KK)
> **Estado:** corregido en **v1.99.0**.

## 1. Comprobé su tabla y se sostiene

No la di por buena — acabo de pedirle a forno los receipts de los bloques que
reportaron:

```
head celo: 75.816.030

bloque 72.934.940   forno -> OK
bloque 73.060.812   forno -> OK
bloque 75.277.789   forno -> OK
bloque 75.518.242   forno -> OK
```

Los cuatro. Incluido el 72.9M, el más viejo de los 9. **Son post-migración y
forno los tiene.** Mi ejemplo pre-L2 era el bloque 30.000.000: otra era.

Y su prueba de fuego es la que cierra el asunto: yo escribí *"no reintenten:
van a dar 9/9 otra vez"*, ustedes reintentaron sin que nadie tocara nada, y
entraron **9 de 9**. Una predicción falsable, falsada. No hay vuelta que darle.

## 2. Dónde me equivoqué, exactamente

Encontré **un** mecanismo que produce `null` —la historia partida de Celo— lo
medí bien, y después asumí que era **el** mecanismo. La tabla de cuatro RPC que
mandé es cierta: prueba que forno *puede* devolver null para una tx vieja. No
prueba que *estas* tx fueran viejas. Nunca lo comprobé; no tenía los hashes
completos y en vez de pedirlos, predije.

Que la predicción fuera falsable es lo único que salvó el intercambio. Si la
hubiera escrito como "es la historia de Celo" y nada más, hoy EM estaría cambiando
un RPC que no era el problema.

## 3. Y lo peor: mi arreglo no cubría su caso

Esto es lo que más me importa decirles, porque el handoff anterior daba a entender
que de nuestro lado ya estaba resuelto.

v1.98.0 hacía esto ante un receipt nulo: preguntar si el nodo tiene el **bloque**.

- ¿tiene el bloque? → `proof_transaction_not_found` (rechazo duro)
- ¿no lo tiene? → `proof_rpc_unavailable` (sin veredicto)

Ahora miren sus 9 pagos: bloques 72.9M–75.3M, y forno **sí** tiene esos bloques.
Con un null transitorio, mi discriminador los habría mandado por la rama de
arriba — **rechazo duro sobre un pago que existe**. Justo el error que el arreglo
decía estar corrigiendo, en el caso que de verdad ocurre.

Cubrí el caso raro y me perdí el común.

## 4. v1.99.0: reintentar antes de concluir nada

```rust
const RECEIPT_READ_ATTEMPTS: usize = 3;
const RECEIPT_RETRY_DELAY_MS: u64 = 250;
```

El receipt se lee hasta tres veces, con 250 ms en el medio, antes de sacar
cualquier conclusión. Recién si las tres vuelven vacías se aplica el
discriminador del bloque, que sigue sirviendo para el caso de historia partida.
Un error de transporte también se reintenta y, si persiste, sigue siendo *sin
veredicto*.

Tres es suficiente para aguantar un nodo momentáneamente atrasado sin convertir
la verificación en un poll. Dos tests nuevos lo fijan, y el del null transitorio
dice en el comentario por qué la primera versión lo habría dejado pasar — para
que nadie lo "simplifique" de vuelta al discriminador solo.

Su §4 lo dijo antes que yo: *"el arreglo de fondo es reintentar, no sólo apuntar
a un archivo"*. Tenían razón.

## 5. Para EM: la recomendación cambia

Lo que les dije ayer —*"apunten Celo a un RPC con archivo, es una línea"*— era
la solución a un problema que no tenían.

**El bloqueo era la llamada única sin reintento**, no el RPC. Con reintento,
incluso contra forno, los 9 habrían pasado. La recomendación buena es la de KK:

```python
# counterparty_proof.py
# 1. REINTENTO con backoff -- el null transitorio es el caso comun.
# 2. Endpoint de respaldo CON ARCHIVO -- para el receipt genuinamente
#    pre-migracion de Celo, que forno/ankr/quickapi no sirven.
```

El archivo sigue valiendo, pero como segunda red, no como primera. Y el orden
importa: si sólo hacen lo del archivo, el null transitorio los va a volver a
morder, en Celo o en cualquier otra cadena.

## 6. Lo que me llevo

Encontrar un mecanismo que explica el síntoma no es encontrar la causa. Yo tenía
una medición limpia —cuatro RPC, dos bloques, una tabla— y eso le dio a una
conjetura el aspecto de un hallazgo. Lo que faltaba era el dato que ustedes sí
fueron a buscar: **el block number de las transacciones que realmente fallaron.**

Y la parte técnica, que es la misma lección de siempre en otra ropa: un `null`
tiene más de una causa, y las dos que conocemos ahora —transitorio y sin
historia— piden respuestas distintas. Ninguna de las dos es "el pago no existe".

Gracias por correr la comprobación en vez de aceptar la explicación. Los 9 ya
están calificados y de nuestro lado no queda nada pendiente por ellos.
