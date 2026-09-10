---
date: 2026-09-10
tags:
  - type/handoff
  - domain/evm
  - domain/polygon
  - domain/escrow
  - priority/p0
status: active
---

# La cola de Polygon: una sola transaccion mal tarifada congelo el firmante seis dias

**Reportado por:** el dueno, 2026-09-10 ("eso si estaba funcionando y no ha cambiado").
**Y tenia razon: no cambiamos nada. Cambio Polygon.**

El 2026-09-03 a las 21:28:00Z el `baseFee` de Polygon estaba en un hoyo de
**1,072 gwei** — se habia desplomado desde sus ~250 habituales y venia subiendo.
El facilitador tarifo ahi un `release` de escrow con el estimador por defecto de
Alloy, cuyo margen entero es `2 x baseFee`, asi que salio con un techo de
**32,247 gwei**. Cuarenta minutos despues el `baseFee` estaba en **248 gwei** y
esa transaccion — la nonce **1157** — ya no se podia minar nunca.

Los nonces son estrictamente ordenados. Detras de ella se apilaron 399
transacciones **bien tarifadas** que no pueden adelantarla, su reserva conjunta
de `gasLimit x maxFeePerGas` llego a **82,799 de los 82,862 POL** del firmante, y
desde entonces el nodo rechaza cada nuevo settle de Polygon con
`insufficient funds for gas * price + value`.

**Causa raiz en una linea:** una unica transaccion tarifada durante un hoyo del
`baseFee` de Polygon quedo por debajo del `baseFee` de recuperacion y, al ser la
cabeza de la cola de nonces, congelo el firmante entero; el saldo nunca se movio
porque estaba *reservado*, no gastado, que es exactamente por que ninguna alarma
lo vio.

---

## Para c0der

### 1. Las tres hipotesis, contestadas con medicion

Todo lo de abajo es lectura, contra el **proveedor real del facilitador**
(`rpc_mainnet:polygon` en Secrets Manager, leido dentro del proceso; el host es
un endpoint de QuickNode y no se transcribe aqui).

#### H1 — hueco de nonce / transacciones en el bucket `queued`: **REFUTADA**

`txpool_contentFrom` para `0x103040545AC5031A11E8C03dd11324C7333a13C7`:

| | |
|---|---|
| pending | **400** (nonces 1157 … 1556) |
| queued | **0** |
| huecos en ese rango | **0** |
| `eth_getTransactionCount` latest / pending | 1157 / 1557 |

No hay hueco y no hay nada en el bucket no-ejecutable. La palabra **"queued"**
del mensaje del nodo enganaba: en geth/bor ese texto viene de
`ExistingExpenditure(from)`, o sea **el costo total de todo lo que la cuenta ya
tiene en el pool**, sin distinguir bucket. Se comprueba numericamente — la suma
de `gasLimit x maxFeePerGas` de las 400 pending da

```
82_799_377_752_610_042_973 wei
```

byte por byte el `queued cost` que aparece en las 7.196 lineas de log. Contra un
saldo de `82_861_633_384_675_957_709` quedan **0,062256 POL** libres, menos que el
techo de cualquier settle nuevo (~0,2 POL). De ahi el rechazo.

#### H2 — sub-tarifadas: **CONFIRMADA, pero solo una**

De las 400, **exactamente una** esta por debajo del `baseFee`:

| nonce | maxFeePerGas | maxPriorityFeePerGas | gas | funcion |
|---|---|---|---|---|
| **1157** | **32,247 gwei** | 30,103 gwei | 206.433 | `release(PaymentInfo,uint256)` (selector legacy `0xecf39b0a`) |
| 1158 … 1556 | **505,4 – 692,0 gwei** | 69,6 – 158,9 gwei | ~357.783 | `authorize(...)` `0x41d66202` (398) + un `0x3c036a7e` a ERC-8004 |

O sea: el pool **no** esta mal tarifado. Lo esta la **cabeza**, y el orden de
nonces hace el resto. La aritmetica cierra exacto con el estimador de Alloy
(`alloy-provider 1.7.3`, `eip1559_default_estimator`):

```
maxFee = 2 * baseFee + priority = 2 * 1,072065664 + 30,103229849 = 32,247361177 gwei
```

#### H3 — doble escritor (A2): **REFUTADA**

Cuatro tareas ECS mandaron transacciones de Polygon en la ventana del incidente,
con ventanas **estrictamente disjuntas** — relevo secuencial del lease, no
concurrencia:

| tarea | n | primera | ultima |
|---|---|---|---|
| `fc9d6a034b2a` | 26 | 09-03 20:00:57Z | 09-03 20:30:50Z |
| `9dc02bf143ac` | 67 | 09-03 21:53:09Z | 09-03 22:29:57Z |
| `340723add913` | 275 | 09-03 22:31:04Z | 09-04 00:59:09Z |
| `b4f72269c05c` | 58 | 09-04 02:58:56Z | 09-04 03:48:40Z |

Y la evidencia fisica es mas fuerte que el log: **400 nonces contiguos, cada uno
exactamente una vez**. Un doble escritor deja duplicados o huecos. No hay ninguno
de los dos.

> Nota de metodo: el log group va **coloreado con ANSI**, y los codigos de color
> parten los tokens `key=value`. Un filtro por `network=polygon` devuelve **cero**
> resultados aunque haya 524 lineas. Hay que filtrar por la subcadena suelta.

### 2. La cronologia, y por que nadie la vio

El `baseFee` de Polygon **se desploma a ~0 y vuelve a ~250 gwei de forma
recurrente**. Tres episodios en tres dias, medidos bloque a bloque y verificados
contra un nodo publico independiente (`polygon-bor-rpc.publicnode.com`):

| se desploma | se recupera |
|---|---|
| 09-01 23:02Z | 09-02 00:28Z |
| 09-02 01:54Z | 09-02 06:11Z |
| **09-03 17:53Z** | **09-03 21:28Z → 22:05Z** |

No es congestion: el `gasLimit` es 160.000.000 y la utilizacion se mantuvo entre
**7 % y 24 %** todo el tiempo, antes, durante y despues.

```
bloque 93177231  09-03 21:28:00Z  baseFee     1,0721 gwei   <- se firma la nonce 1157
bloque 93177331  09-03 21:30:30Z  baseFee     2,2493 gwei
bloque 93177731  09-03 21:40:30Z  baseFee    10,7827 gwei
bloque 93178731  09-03 22:05:30Z  baseFee   248,8907 gwei   <- 1157 ya es inminable
```

* **09-03 21:28:00,099Z** — `post_settle:settle_escrow:execute_release` manda
  `0x056222b3a614859e7cb15b7e7b287716adcea052910c9a629411bdef0e420f4f` desde la
  tarea `fc9d6a034b2a`. Es la unica linea de log que menciona ese hash.
* **09-03 ~22:05Z** — el `baseFee` vuelve a 248 gwei. La cabeza queda muerta.
* **09-04 03:48Z** — se recarga la billetera a 82,86 POL. **El error sigue**,
  porque nunca fue el saldo.
* **09-10** — 400 en el pool, 0,062 POL libres, y **2.928 lineas de
  `insufficient funds` en las ultimas 6 horas** (hasta 1.197 en una sola hora).
  Sigue costando trafico ahora mismo.

**Por que ninguna alarma sono:**

| alarma | por que no |
|---|---|
| `chain_balance_low` | el saldo **no se movio**: 82,861633 POL identico durante seis dias. Estaba reservado, no gastado |
| `chain_rpc_unreachable` | el RPC respondio todo, siempre |
| `orphan_5xx_errors` | el facilitador devolvia 4xx limpio. Nada estaba caido |
| `evm_nonce_desync` | matchea `"nonce too high"`, que es **el fallo contrario**: nuestro contador adelantado. Aqui el contador estaba bien y la **cadena** era la atascada |

### 3. Los cuatro hashes que no aparecian (respuesta a tu mensaje de 15:35Z)

Era la primera rama de tu propia disyuntiva: **no eran de Polygon.** En las
ultimas 3 horas el facilitador mando 7 transacciones — **3 en base, 4 en
avalanche-fuji, 0 en polygon** — y las 7 estan minadas en su propia cadena.
Ninguna esta en el pool de Polygon. Desde el 09-04 no sale ni una transaccion de
Polygon: todas se rechazan antes del mempool.

### 4. Plan de destrabe, con numeros (POL a 0,092382 USD)

Antes de decidir hay un dato que cambia todo el riesgo: **simule las 400 con
`eth_call` contra el estado actual.**

| resultado | n |
|---|---|
| revierte `AfterPreApprovalExpiry(uint48,uint48)` | **398** |
| revierte `InsufficientAuthorization(...)` | 1 (es la propia 1157) |
| tendria exito | 1 (nonce 1556, un registro ERC-8004, inocuo) |

**No se mueve dinero de nadie si se ejecutan.** Y no es una foto: el
`preApprovalExpiry` mas nuevo de las 398 es **2026-09-04 04:48:36Z**, ya pasado, y
la expiracion es monotona — una autorizacion vencida no puede volver a ser
ejecutable. Eso convierte la opcion barata en operaciones en la opcion segura.

`gasUsed` real medido con `debug_traceCall`: 114.070 por `authorize` que revierte,
169.832 el ERC-8004 que pasa, 93.861 la `release`.

#### (a) Reemplazar SOLO la cabeza — **RECOMENDADA**

Una transaccion de 0 POL a si mismo en la nonce 1157, 21.000 de gas, tarifada por
encima del `baseFee`. Las otras 399 ya estan por encima del `baseFee`: drenan
solas en cuanto la cabeza se libera.

```
nuestra tx:            0,006090 POL
las 399 que ejecutan: 15,513000 POL   (398 reverts + 1 exito)
TOTAL                 15,521 POL  ($1,43)     -- 1 transaccion firmada
```

#### (b) Cancelar las 400

Un reemplazo de 0 POL por cada nonce, con fee >= el de la reemplazada +12,5 %
(bor hereda el `PriceBump = 10` de geth; pedimos mas margen), en orden ascendente.

```
TOTAL                  2,974 POL  ($0,28)     -- 400 transacciones firmadas
```

#### Recomendacion: **(a)**

La diferencia entera es **$1,16**. Por ese peso, (b) pide 400 firmas con la llave
de produccion en vez de una, cada una una ocasion de equivocarse, y ~20 minutos
de ventana. (a) es una transaccion, se verifica de un vistazo, y la simulacion ya
dice que lo que se ejecuta detras no mueve fondos de nadie. Si algun dia la
simulacion dijera que alguna **si** ejecutaria, la respuesta cambia a (b) sin
discusion.

### 5. Como correr el script

`scripts/polygon_destrabar_cola.py`. Lee la llave de
`facilitator-evm-mainnet-private-key` y el RPC de `facilitator-rpc-mainnet:polygon`
**por nombre y dentro del proceso**; no imprime ninguno de los dos ni los escribe
a disco (del RPC solo muestra el host). `--apply` se niega a correr si la
direccion derivada no es la esperada, si el `chainId` no es 137, o sin
confirmacion interactiva.

**Orden de operaciones — importa, y no es el obvio:**

> **Primero mergear este PR, despues destrabar.** Dos razones. Una: el
> `PendingNonceManager` en memoria del escritor esta **derivado hacia arriba** —
> el worker midio 1738 local contra 1557 en la cadena. Cada rechazo libero su
> nonce, pero con trafico continuo un hermano ya habia tomado el siguiente, asi
> que el rollback se declinaba y la marca subio sola. Si destrabas con ese
> proceso vivo, el primer settle aceptado sale en 1738 y deja **1557..1737
> vacios**: un hueco de nonce de verdad, que es el unico fallo que no se cura
> solo. Un deploy reinicia las tareas y el manager arranca limpio. Dos: el piso
> de tarifa nuevo entra en vigor antes de que salga ningun settle nuevo.

```bash
# 1) merge del PR -> CI despliega y reinicia las tareas
# 2) leer el plan (no escribe nada)
python3 scripts/polygon_destrabar_cola.py --dry-run
# 3) con el go, destrabar la cabeza
python3 scripts/polygon_destrabar_cola.py --apply
# 4) confirmar que drena
python3 scripts/polygon_destrabar_cola.py --dry-run   # deberia decir "nothing pending"
```

Dry-run corrido contra el proveedor real el 2026-09-10 15:47Z, sin escribir nada:

```
provider host: <endpoint de QuickNode>
signer 0x103040545AC5031A11E8C03dd11324C7333a13C7
  nonce latest=1157 pending=1557 (400 transaction(s) not mined) | pooled here: 400
  pooled transactions priced BELOW the current base fee (250.01 gwei): [1157]

plan: head, 1 replacement transaction(s)
  base fee now 250.01 gwei | balance 82.861633 POL | pool reserves 82.799378 POL | free 0.062256 POL

   nonce     replaces (maxFee/prio gwei)    sends (maxFee/prio gwei)           charged
    1157          32.247 /      30.103        1040.0 /      40.0      0.006090 POL

  total charged to the signer: 0.006090 POL ($0.00)
  tightest free balance during the walk: 0.047072 POL (at nonce 1157)

--dry-run: nothing was sent.
```

Y el mismo comando con `--mode cancel-all`:

```
plan: cancel-all, 400 replacement transaction(s)
    1157          32.247 /      30.103        1040.7 /      40.0      0.006094 POL
    1158         576.969 /      78.000        1088.5 /      87.8      0.007096 POL
     ...  (389 more)
    1556         580.866 /      75.000        1085.1 /      84.4      0.007026 POL

  total charged to the signer: 2.973797 POL ($0.28)
  tightest free balance during the walk: 0.047058 POL (at nonce 1157)
```

**`--apply` NO se corrio.** Espera tu go.

Ese `tightest free balance` no es decorativo: el nodo revalida el costo de **todo**
lo que la cuenta tiene en el pool en cada insercion, y reemplazar la cabeza — la
unica entrada barata — **sube** la reserva momentaneamente. Con 0,062 POL libres
ese es el paso que puede fallar, y falla primero. Por eso el dry-run lo calcula
caminando el plan en vez de descubrirlo a mitad del `--apply`.

### 6. Que se cambio para que no vuelva a pasar

**(iii) Tarifa explicita en toda cadena EIP-1559, no solo Ethereum**
(`src/chain/evm.rs`). El caso `Network::Ethereum` escrito a mano paso a ser una
tabla de pisos por red, `eip1559_fee_floor`. Ethereum conserva sus numeros
exactos (1 / 5 gwei). Polygon y Amoy reciben **30 gwei de priority y 1000 gwei de
techo**. Las demas redes se quedan **sin piso**, tarifando igual que hoy: no
invento numeros para cadenas que no medi.

El multiplicador sigue en 2x a proposito. **El termino que salva es el piso, no
el multiplicador**: el hoyo duro tres horas y media, asi que ni un maximo sobre
una ventana de bloques habria servido. Y `maxFeePerGas` es un **techo, no un
pago** — EIP-1559 cobra `baseFee + priority` y devuelve el resto — asi que un piso
generoso no cuesta gas, solo reserva de pool: 0,36 POL por settle en vuelo contra
82 POL de saldo.

**(ii) Detector de transacciones propias atascadas**
(`src/stuck_tx_monitor.rs`, nuevo). Cada dos minutos, por red EVM, lee
`eth_getTransactionCount` en `latest` y en `pending`. Si la **cabeza no avanza**
durante diez minutos con cola detras, emite un WARN con `network`, `signer` y
`first_unmined_nonce` — el numero que hay que reemplazar.

Atascado es *la cabeza no se mueve*, no *hay cola*. Una cola que drena despacio es
un problema de capacidad y **deliberadamente no dispara**; confundirlos pondria
una pagina en cada rafaga y en una semana nadie mira la alarma.

**Alarma**: `terraform/environments/production/alerts-evm-stuck-tx.tf`, con sus dos
direcciones agregadas al `-target` del paso "Deploy observability" de `ci.yaml`
para que el pipeline las cree y el drift gate quede conforme.

De paso: el gate de ese paso solo aplicaba si cambiaba
`alerts.tf|alerts-imported.tf|cloudwatch-*.tf|variables.tf`. **`alerts-solana-mint.tf`
no estaba en esa lista**, asi que esas alarmas solo se aplicaban cuando algun otro
fichero de observabilidad cambiaba en el mismo push. Ahora el patron es
`alerts-.*\.tf`.

**(i) Resincronizacion del nonce hacia abajo por deriva sostenida**
(`src/chain/evm.rs`). `NONCE_TRUST_CHAIN_AFTER` suelta la marca de high-water solo
tras 120 s **sin asignaciones**, y con trafico continuo esa pausa no llega nunca.
El nuevo `NONCE_TRUST_CHAIN_AFTER_DRIFT` (300 s) cuenta desde otro reloj: cuanto
lleva la **cadena** reportando menos de lo que creemos. Cinco minutos sin que
ningun nodo reconozca lo que asignamos significa que no esta propagandose.

Que quede claro: **(i) no es la causa del incidente** — aqui no hubo hueco de
nonce. Es el fallo que la *recuperacion* habria provocado, y por eso el orden de
operaciones de arriba.

### 7. Pruebas

Todo probado en rojo antes que en verde:

| se revirtio | test que fallo |
|---|---|
| `min_max_fee: 1000 gwei → 0` en Polygon | `polygon_priced_in_a_base_fee_trough_still_mines_after_the_recovery`, con el valor real: *"a settle priced in the trough (32247361177 wei cap) would not survive…"* |
| quitar la rama de deriva de `resync_target` | `sustained_drift_gives_up_the_high_water_mark` — `left: 1738, right: 1557` |
| que el monitor no cronometre la cabeza quieta | 3 de 7 tests del monitor |

* `src/chain/evm.rs` — 7 tests de tarifa + 5 de deriva (37 en `chain::evm::tests`)
* `src/stuck_tx_monitor.rs` — 7 tests, incluido `a_draining_queue_never_fires`,
  que es el falso positivo que volveria inutil la alarma
* `tests/scripts/test_polygon_destrabar_cola.py` — 24 tests, con RPC mock.
  Encontraron dos bugs reales al escribirlos: `bump()` daba 111 en vez de 110 por
  imprecision de coma flotante, y `read_pool` evaluaba `tx["gasPrice"]` de forma
  ansiosa como default y reventaba en cualquier entrada sin ese campo.

`cargo fmt` limpio; `clippy` no anade ni un diagnostico nuevo (los 121 con
`-D warnings` son previos y ninguno toca fichero mio); `terraform validate` OK y
`terraform fmt` limpio en el fichero nuevo.

### 8. Que quedo fuera

* **No se corrio `--apply`.** Es tuyo.
* **No hay re-tarifacion automatica (RBF) de transacciones propias atascadas.**
  Es el arreglo completo — el que quitaria la intervencion manual del todo — pero
  toca el camino de firma y el writer lease, y merece su propio PR con su propio
  analisis de reentrada. Hoy el piso hace improbable el caso y el detector acota
  el dano a ~15 minutos.
* **Los pisos de Polygon son medidos; los de las otras 18 redes EVM no existen a
  proposito.** Si alguna de ellas tiene el mismo patron de hoyo, todavia es
  vulnerable. Medir antes de poner un numero.
* **No se toco `alerts-solana-mint.tf`**, solo el gate de `ci.yaml` que lo
  ignoraba. Sus alarmas se crearan en este mismo push.

---

## Para el dueno

**Que paso.** El 3 de septiembre el precio del gas de Polygon se desplomo a casi
cero durante tres horas y volvio a su nivel normal en cuarenta minutos. Justo en
ese hueco el facilitador mando un pago con el precio de gas de ese momento —
correcto entonces, ridiculamente bajo cuarenta minutos despues. Como las
transacciones de una misma billetera se procesan en orden estricto, esa unica
transaccion barata se quedo bloqueando la fila, y las 399 que venian detras nunca
pudieron pasar. La billetera parecia llena porque el dinero seguia ahi, solo que
comprometido con una fila que no avanzaba. Tenias razon: no cambiamos nada
nosotros.

**Que se hizo.** Un script que desbloquea la fila reemplazando esa unica
transaccion atascada (ya probado en seco, esperando tu visto bueno para
ejecutarlo), y tres cambios para que no se repita: un precio minimo de gas para
Polygon que aguanta esos desplomes, un vigilante que avisa a los diez minutos si
una fila deja de avanzar, y una correccion en el contador interno que evitaba que
la recuperacion abriera otro agujero.

**Que cuesta.** Un dolar y medio de gas para vaciar la fila. Nada mas: ninguna de
las 399 transacciones atrapadas mueve dinero de nadie — todas caducaron hace
dias, y lo comprobamos una por una antes de proponer nada.
