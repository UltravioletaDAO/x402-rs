---
date: 2026-08-26
tags:
  - type/handoff
  - domain/blockchain
  - domain/identity
  - priority/p0
status: active
aliases:
  - Celo no es salud de RPC, es historia
  - El null que acusaba en vez de callarse
related-files:
  - src/erc8004/proof.rs
  - docs/handoffs/2026-08-25-appendresponse-con-autoria-real.md
---

# Celo: no es nuestro RPC, y no es salud. Es historia — y su reporte encontró un bug nuestro en otro lado.

> **Para:** Karma Kadabra y Execution Market
> **De:** el equipo del Facilitator (`x402-rs`)
> **Responde a:** `HANDOFF_FACILITADOR_CELO_RECEIPT_2026-08-26.md`
> **Estado:** medido. El arreglo que les toca es de EM y es una línea; el que nos
> tocaba a nosotros ya está en **v1.98.0**.

## 1. Ese error no sale de nosotros

`failed to fetch receipt for 0x…` **no existe en el código del facilitador.**
Está en Execution Market:

```
execution-market/mcp_server/integrations/reputation/counterparty_proof.py:275
    f"failed to fetch receipt for {tx_hash[:12]}...: {exc}"
```

Dos pistas más que apuntan al mismo lado: el body usa la clave `detail`
(FastAPI), y nosotros contestamos `error` — cero ocurrencias de `detail` en
`handlers.rs`. Y ese módulo **construye su propio cliente web3** con
`get_rpc_url(network)` de EM, no con el nuestro:

```python
w3 = Web3(Web3.HTTPProvider(rpc_url, request_kwargs={"timeout": 15}))
receipt = w3.eth.get_transaction_receipt(tx_hash)   # una sola vez, sin reintento
```

Ese `get_rpc_url("celo")` resuelve a **`https://forno.celo.org`**
(`network_registry.py:181`, y el default de `erc1271.py:51`).

Así que el `503` sale de un proceso de EM consultando un RPC de EM. **Nosotros no
estamos en esa llamada.**

## 2. Qué significa el mensaje, exactamente

`Transaction with hash: '0x3aa0a99ea9…'` es el texto de **`TransactionNotFound`
de web3.py**, que se lanza cuando `eth_getTransactionReceipt` devuelve `null`.

No es un timeout. No es "la tx salió pero el receipt no volvió". Es **el nodo
diciendo que no conoce esa transacción**.

Y ahí está la clave, porque un nodo dice exactamente lo mismo cuando la tx no
existe y cuando simplemente **ya no la recuerda**.

## 3. Lo medimos: es historia, no salud

Celo migró a L2 y eso partió su historia. Tomamos una transacción **pre-migración**
(bloque 30.000.000) y otra reciente, y le pedimos el receipt a cuatro RPC:

| RPC | receipt PRE-L2 | receipt reciente |
|---|---|---|
| `forno.celo.org` (**el de EM**) | **null** | OK |
| `rpc.ankr.com/celo` | **null** | OK |
| `celo-rpc.quickapi.com` (el nuestro) | **null** | OK |
| `celo.drpc.org` | **OK** | OK |

Los cuatro están sanos y a la misma altura de bloque (~75.810.000). Ninguno está
caído ni con lag. Tres de ellos **no cargan esa historia**, y uno sí.

Esto explica su reporte entero, sin necesidad de saturación:

- **Reproducible 9/9**: no es azar, es que esas transacciones están del lado de la
  historia que forno no sirve.
- **Sólo Celo**: es la única cadena de las 8 que partió su historia en una
  migración.
- **"Intermitente" (1 pasó)**: predice que ese rating es sobre un pago
  **reciente** y los 9 sobre pagos **más viejos**. Es comprobable de su lado en un
  minuto: miren el block number de los pagos. Si esa correlación se sostiene, se
  acabó el misterio.

## 4. Sus tres preguntas, contestadas

**¿Las 9 tx confirmaron, o nunca entraron?** Casi con certeza **confirmaron**. La
ruta que falla es `counterparty_proof`, que verifica un pago **que ya ocurrió** y
se pasa como prueba — no una tx que el facilitador acaba de emitir. Un pago que
nunca hubiera entrado no tendría hash que declarar.

**¿Hay un mint de identidad huérfano que reconciliar?** **No.** Esa ruta no
mintea nada; sólo lee. Y el facilitador no participó, así que no hay nada que
limpiar de nuestro lado. Su lectura de "riesgo on-chain: ninguno" es correcta,
por una razón todavía más simple que la que dieron.

**¿Necesitan un RPC de Celo mejor de nuestro lado?** No para esto. Lo que hace
falta es que **EM apunte Celo a un RPC con archivo**. Hoy `celo.drpc.org` es el
único de los cuatro que sirve.

Y una nota que le va a doler un poco a EM: **ya lo sabían.** Su propio handoff de
ayer, §7:

> *"El RPC público de Celo (`forno.celo.org`) rechaza el deploy con: `no
> historical RPC is available for this historical (pre-L2) execution request`. Es
> la migración de Celo a L2. Sale con `https://rpc.ankr.com/celo`."*

Lo arreglaron en el script de deploy y no en `counterparty_proof.py`. (Ojo: para
**receipts** viejos ankr tampoco alcanza — ver la tabla. drpc sí.)

## 5. Lo que su reporte SÍ encontró de nuestro lado

Esta es la parte que hace que el reporte haya valido, aunque la causa estuviera
mal atribuida.

Fuimos a mirar cómo trata **nuestro** gate de proof un receipt nulo, porque el
mismo `celo-rpc.quickapi.com` que usamos tiene el mismo punto ciego. Y estaba mal:

```rust
Ok(None) => return Err(ProofRejection::TransactionNotFound),   // ANTES
```

Un nodo sin historia devuelve `Ok(None)` para un pago **que existe y confirmó**, y
nosotros lo llamábamos `proof_transaction_not_found` — o sea, le decíamos a
alguien que su pago no existe mientras está en un bloque que cualquiera con un
nodo de archivo puede leer. **Con la fase 2 encendida eso habría rechazado todos
los pagos de Celo anteriores a la migración, para siempre, por un motivo que nunca
fue cierto.**

Peor todavía estaba la lectura del bloque justo abajo: devolvía
`proof_block_number_mismatch`, que es *acusar a la prueba de nombrar el bloque
equivocado* mientras teníamos en la mano un receipt de ese mismo bloque.

**Arreglado en v1.98.0** con un discriminador barato: si el nodo tampoco tiene el
**bloque**, no es "no existe", es "no me acuerdo" → `proof_rpc_unavailable`, que
es *sin veredicto* y **retryable**, y nunca bloquea una escritura. Si sirve el
bloque pero no el receipt, entonces sí: la tx no está ahí y el rechazo es
legítimo.

Es la misma regla que ya teníamos escrita para 404 vs 503 en
`/identity/:network/owner/:address` (INC-2026-07-21) y que no habíamos aplicado
acá. Tres tests nuevos la fijan.

## 6. Qué hace cada uno

**Execution Market** — el arreglo, una línea:

```python
# counterparty_proof.py / network_registry.py
# celo necesita un RPC con ARCHIVO: la migracion a L2 partio la historia y
# forno/ankr/quickapi devuelven null para receipts pre-migracion.
CELO_RPC_URL = "https://celo.drpc.org"
```

Y de paso: `get_transaction_receipt` se llama **una sola vez, sin reintento**. Con
un RPC con archivo alcanza para esto, pero un solo intento contra un nodo público
va a dar falsos negativos tarde o temprano. Vale un reintento con otro endpoint.

**Karma Kadabra:**

- Comprueben la correlación de §3 (block number de los 9 vs el que pasó). Si se
  sostiene, ya no hace falta mirar logs de nadie.
- **No reintenten todavía**: hasta que EM cambie el RPC van a dar 9/9 otra vez.
- Cuando EM lo cambie, su `--solo <task_ids> --execute` debería pasar sin que
  nosotros toquemos nada.
- Gracias por haber logueado el body. Con el `503` pelado esto se leía como carga
  —ustedes mismos lo dicen— y habríamos terminado buscando saturación en el lugar
  equivocado. El texto exacto del error es lo que lo resolvió.

**Nosotros:** v1.98.0 desplegado. Seguimos con `celo-rpc.quickapi.com`, que tiene
el mismo punto ciego pero ahora falla como *sin veredicto* en vez de acusar.
Cambiar el RPC primario es una decisión aparte y no se toma con una sonda: para
eso está el método de `rpc-health`.

## 7. La regla, dicha corta

**Un `null` no es un `no`.** Vale para `eth_getTransactionReceipt`, para
`eth_getBlockByNumber`, para el lookup de identidad y para cualquier lectura de
cadena: el nodo puede estar diciendo "eso no existe" o "yo no lo tengo", y sólo
una de las dos justifica rechazarle algo a alguien.
