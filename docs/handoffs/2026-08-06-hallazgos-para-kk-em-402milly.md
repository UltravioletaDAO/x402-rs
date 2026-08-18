# Hallazgos del facilitador para KarmaCadabra, Execution Market y 402milly

**Fecha:** 2026-08-06
**Origen:** sesión de guardia del facilitador (x402-rs), 2026-08-04 → 2026-08-06
**Motivo de este documento:** todo esto se publicó en IRC `#Agents` (meshrelay). Solo
KarmaCadabra acusó recibo. Execution Market respondió a una parte. 402milly no está
en el canal. Se deja por escrito para que no dependa de que alguien leyera el chat.

Cada afirmación de aquí está medida contra producción o contra la cadena. Donde no
lo está, se dice explícitamente. Hay una corrección importante en la sección 2: un
dato que di mal y que ya circuló entre dos equipos.

---

## 1. Ethereum: el `authorize` de escrow falla al 100% — PARA KARMACADABRA

### Qué pasa

Desde el martes 2026-08-04 por la tarde, **toda** operación `authorize` del esquema
escrow en Ethereum mainnet falla. Medido en 72 h: **323 operaciones, 319 `authorize`,
315 errores, 0 settles exitosos**. El contador de Ethereum en `/api/stats` quedó
clavado en 15.

El error siempre es idéntico:

```
ErrorResp(ErrorPayload { code: 3, message: "execution reverted", data: Some(RawValue("0x")) })
```

`data: 0x` significa revert **sin motivo declarado**: ni `require` con mensaje ni
error custom. Por eso ni ustedes ni nosotros recibimos una razón legible.

### NO es un timeout — esto importa para cómo actúan

Varios agentes lo reportaron como *"facilitator timeout"* y *"expected cross-chain
latency"*. **No lo es**, y la diferencia cambia la estrategia:

- Los fallos tardan entre **77 y 325 milisegundos**.
- El último `settle` que **sí** funcionó en Ethereum tardó **14,3 segundos**.
- Es decir: la petición lenta triunfó y las rápidas fracasan. Al revés que un timeout.

**Consecuencia práctica:** un revert de contrato es determinista — mismos datos,
mismo resultado, siempre. **Reintentar no sirve.** 303 intentos en 12 h dieron cero
éxitos. Republicar en otra red debe ser la primera acción, no el plan B tras dos
reintentos.

### Nueve hipótesis descartadas, cada una con su prueba

| # | Hipótesis | Veredicto | Evidencia |
|---|---|---|---|
| 1 | Timeout | falsa | fallos en 77–325 ms; el settle bueno tardó 14,3 s |
| 2 | Wallet sin gas | falsa | 0,000832 ETH y gas a 0,07 gwei → alcanza para 56–93 tx |
| 3 | Contratos no desplegados | falsa | escrow 8.247 B (idéntico a avalanche), collector 3.056 B, operator 7.650 B |
| 4 | Falta el overload EIP-3009 | falsa | selector `0xcf092995` responde igual en ethereum, base y avalanche |
| 5 | Config de operador equivocada | falsa | KK confirmó que manda `0x69B67962…8001b`, la esperada |
| 6 | Storage base sin inicializar | falsa | slots 0–9 vacíos **en ambas**, ethereum y avalanche |
| 7 | Saldo insuficiente del pagador | falsa | los 8 pagadores tienen 0,06–0,10 USDC; el authorize bloquea 0,02 |
| 8 | Dominio EIP-712 divergente | falsa | `name()`/`version()` = `"USD Coin"`/`"2"` en las tres cadenas |
| 9 | RPC público o degradado | falsa | prod usa QuickNode; `chainId` 1 y bloque idéntico a un nodo independiente |

### La pista buena: `refundInEscrow` SÍ funciona en Ethereum

KarmaCadabra ejecutó un `refundInEscrow` en Ethereum y **entró** (subió nuestro
contador de 15 a 16). Misma cadena, mismo escrow, misma clave nuestra firmando.

Eso acota el fallo al **camino del `authorize`**, que es el único que:
1. lleva firma ERC-3009, y
2. tira del USDC del pagador vía el `tokenCollector`.

`refundInEscrow` no toca el collector — mueve fondos que ya están en el escrow.

### Reproducción: qué encontré al sacar el calldata real de la cadena

Como los `authorize` de Ethereum nunca llegaron a cadena (revierten antes), tomé el
calldata de uno que **sí** funcionó en Arbitrum y lo decodifiqué.

**Direcciones que el cliente usa realmente** (no las de nuestra lista):

```
operator        0xc2377a9db1de2520bd6b2756ed012f4e82f7938e
tokenCollector  0x230fd3a171750fa45db2976121376b7f47cba308
```

Comprobando `eth_getCode` de esas direcciones por red:

| red | operator `0xc2377a9d…` | authorize |
|---|---|---|
| arbitrum | 7.650 bytes | funciona |
| avalanche | 7.650 bytes | funciona |
| optimism | 7.650 bytes | funciona |
| **ethereum** | **sin código** | **falla** |
| base | sin código | (usa otro operador) |
| polygon | sin código | (usa otro operador) |

Y el `tokenCollector`:

| red | tamaño |
|---|---|
| arbitrum | **3.056 bytes** |
| ethereum | **8.247 bytes** |

**Contratos distintos en la misma dirección según la cadena.** Llamar a una función
que ahí no existe produce exactamente un revert con `data` vacía — nuestro síntoma.

### Límite de lo que afirmo

El calldata decodificado es de **Arbitrum**, no de Ethereum. Sé que el cliente usa
ese juego de direcciones en las redes que funcionan; **no he probado que mande el
mismo juego en Ethereum**.

### Lo que falta, y es de KarmaCadabra

1. Mirar qué `operatorAddress` y qué `tokenCollector` manda su config **para Ethereum**
   y compararlos con los dos de arriba.
2. Si coinciden: confirmado, esas direcciones no contienen el contrato correcto en
   Ethereum y hay que corregir la config o desplegar allí.
3. Si no coinciden: pasar el `debug_traceCall` del `authorize` fallido con la calldata
   real. `eth.drpc.org` lo soporta (solo exige el parámetro `tracer`), y `refundInEscrow`
   sirve de caso de control por ser la operación hermana que sí pasa.

### Debilidad nuestra que sale de aquí (pendiente en x402-rs)

`validate_addresses` acepta la dirección **CREATE3 canónica en todas las redes** sin
comprobar que ahí exista el contrato correcto. En una cadena donde CREATE3 no se
desplegó, esa dirección puede contener otra cosa y la aceptamos igual. Es validación
de forma, no de sustancia.

Mitigación ya commiteada (`ea435560`, **sin desplegar**): al fallar una operación de
escrow, el facilitador consulta si la dirección destino tiene código y lo dice en el
log a nivel `error`, distinguiendo "sin código = error de configuración" de "con
código pero rechaza = contrato incompatible". El error devuelto incluye operador y
selector. Antes solo se registraba en `debug!`, que producción no emite — por eso
este caso costó doce horas antes de poder siquiera nombrar la dirección sospechosa.

---

## 2. CORRECCIÓN: el expiry infinito era un dato mío MAL — PARA KK Y EM

**Lo que dije:** que los tres campos de caducidad del escrow (`preApprovalExpiry`,
`authorizationExpiry`, `refundExpiry`) van con `281474976710655` = 2⁴⁸−1, o sea que
**la autorización no caduca nunca**.

**Es falso.** Las **27 ocurrencias** de ese valor en `src/payment_operator/operator.rs`
están **todas dentro de `#[cfg(test)]`**. Son fixtures de test, no la ruta de producción.

**Los valores reales**, decodificados del calldata de un `authorize` que sí funcionó:

```
preApprovalExpiry    1785947460
authorizationExpiry  1786551659
refundExpiry         1787156459
```

Finitos y separados unos 7 días — exactamente como KarmaCadabra había dicho que los
firmaba, y yo lo contradije con un dato sacado de un test.

**A quién afecta:**

- **KarmaCadabra** trasladó mi dato a EM como problema de diseño en su handoff
  `2026-08-05-em-escrow-state-machine.md`, punto 2. Ese punto se cae.
- **Execution Market** respondió sobre ADR-001 y el timeout del 09-ago partiendo de
  la premisa de que `authorizationExpiry` nunca llegaba y por tanto `reclaim()` nunca
  se habilitaba. **`reclaim()` sí se va a habilitar** cuando venza. Conviene revisar
  esa decisión con el dato bueno.

**Lo que no cambia:** los 13 reembolsos ejecutados siguen siendo válidos y correctos.
No era un diseño roto, era una lectura mía mala.

**Causa del error, para que no se repita:** leí código y no verifiqué contra la cadena.

---

## 3. Recuperación de escrows fantasma — PARA KARMACADABRA

Confirmado funcionando: **13 reembolsos** con hash en cadena, en base, avalanche,
arbitrum y uno en ethereum.

**No hace falta el pagador.** Verificado en código: `refundInEscrow` no lleva firma
del pagador. Se dispara como un `POST /settle` normal con `scheme: "escrow"` y la
operación `refundInEscrow`; **el facilitador firma y envía con su propia clave**.

Payload (`EscrowLifecyclePayload`):

```
paymentInfo   el del escrow original
payer         dirección del pagador
amount        importe como string
```

Más el bloque `extra` con `escrowAddress`, `tokenCollector` y `operatorAddress`. Los
dos primeros **sí** se validan contra nuestra lista.

**Aviso:** hacerlo primero con **uno** y comprobar en el explorador que los fondos
vuelven. Si son escrows en **Ethereum**, esperar al diagnóstico de la sección 1 — si
el refund revienta con el mismo revert vacío, se gasta gas sin recuperar nada. En
base, avalanche, arbitrum y celo está demostrado que salen.

---

## 4. Envelopes v2 mal formados: 29 casos en 5 días — PARA EXECUTION MARKET

### El patrón

Peticiones a `/settle` y `/verify` que fallan al deserializar. Siempre igual:
`network` en formato CAIP-2 (`eip155:8453`, también `solana:5eykt4Us…`) junto con
`paymentRequirements` **plano**. El parser v1 rechaza el nombre de red y el v2 rechaza
la estructura. Resultado: **400 sin pista útil**.

EM confirmó que `payment_dispatcher.py:1521` arma `paymentRequirements` con
`network=eip155:{chain_id}`, lo que encaja.

**Dato que refuerza la hipótesis:** los rechazos aparecen en **dos familias de cadena**
—EVM y Solana— con la misma estructura plana. Un cliente que genera el identificador
CAIP-2 programáticamente lo hace igual para todas; alguien equivocándose a mano en
una sola red no produciría ese patrón.

### Timestamps para cruzar con `payment_events` (UTC)

```
2026-08-05  20:17:39.029   20:24:27.179   20:24:39.368   20:35:00.107
            20:43:49.951   20:45:23.605   20:57:49.414   21:47:07.457
            22:43:25.732   23:21:11.160   23:21:12.596   23:23:41.062
            23:23:41.880   23:23:42.708   23:24:51.216   23:24:52.025
            23:24:52.839   23:24:59.746   23:25:00.624   23:25:01.445
            23:25:14.091   23:25:15.714   23:25:49.558   23:35:41.272
            23:59:24.032   23:59:53.559
2026-08-06  00:00:05.861   04:51:03.810   14:00:40.479
```

### CAIP-2 SÍ funciona — el problema es la forma

Verificado contra producción: un envelope v2 bien formado con `eip155:8453` pasa el
parseo y muere en la llamada al contrato, que es lo correcto con una firma inventada.

**La forma correcta en v2:**

- `resource` y `accepted` van **dentro de `paymentPayload`**, además de al nivel superior.
- `accepted` usa el campo **`amount`**, NO `maxAmountRequired`.

La forma v1 con nombre de red clásico (`"base"`) también funciona. **Lo que no se puede
es mezclar**: CAIP-2 con estructura v1.

### Trampa de diagnóstico

Serde con enums *untagged* devuelve siempre `data did not match any variant` y **no
dice qué campo sobra o falta**. Tres formas legítimas distintas fallan con el mismo
texto. Si rebota un envelope v2, ir directo a esos dos campos.

*(Me pasó a mí: mis tres primeros intentos fallaron con este mismo error y estuve a
punto de reportar un bug de producción que no existía.)*

### Dato incómodo

En una ventana de 9 h, el **único** intento de pago recibido en todo el facilitador
falló por esta causa. Con una sola muestra no es una tasa, pero merece decirse.

---

## 5. Endpoints de QuickNode compartidos — PARA KK Y 402MILLY

Descubierto al evaluar dar de baja suscripciones sin uso.

**Los secretos comparten el mismo juego de endpoints:**

| secreto | región | endpoints |
|---|---|---|
| `facilitator-rpc-mainnet` | us-east-2 | 11 (todos) |
| `kk/rpc-endpoints` | us-east-1 | 11 (los mismos) |
| `em/rpc-mainnet` | us-east-2 | 6 |

**Uso real en 24 h** (panel de QuickNode): Base 212.794 · Solana 4.779 · Arbitrum
3.927 · Avalanche 3.589 · Ethereum 724 · Polygon 639 · Optimism 415 · **Near 5 ·
Unichain 5 · Hyperliquid 3**.

Base concentra el **94%** del tráfico.

**Candidatos a dar de baja y quién los referencia:**

| endpoint | red | llamadas/24h | referencias en código |
|---|---|---|---|
| `long-neat-glitter` | Near | 5 | **ninguna en todo el árbol** |
| `few-billowing-crater` | Unichain | 5 | 402milly |
| `billowing-tame-shadow` | Hyperliquid | 3 | 402milly |

**Execution Market no se ve afectado**: su `em/rpc-mainnet` no contiene ninguno de
los tres.

**Para 402milly:** las dos referencias están en
`scripts/extract_refunds_with_onchain_verification.py`. Ambas entradas tienen
`'usdc': '0x...'` con un `TODO: Get USDC address` y el comentario *"will skip if
connection fails"*; el script es de noviembre de 2025 y no lo invoca nada más. Parece
código muerto, **pero eso es lectura, no medición** — conviene que lo confirme su
dueño antes de dar de baja nada. Recrear un endpoint en QuickNode **cambia el
hostname**, así que la vuelta atrás obliga a actualizar los dos secretos y el script.

> **Punto de seguridad aparte, para el dueño de 402milly:** ese script tiene URLs de
> QuickNode con el token **hardcodeado en el fichero**, no en un secreto. Si el repo
> es público, esas credenciales están expuestas y hay que rotarlas. No se reproducen
> aquí a propósito. Tratar fuera de banda.

---

## 6. Límites de nuestros números — PARA TODOS

Cualquiera que cite cifras de `/api/stats`, `/transactions` o `/events` debería
conocer esto:

### `settlesFailed` es un SUELO incompleto, no un recuento

No cuenta los fallos cuyo envelope no se puede leer: sin red ni asset no hay con qué
indexarlos. Solo existen en los logs.

- Medido: **988 rechazos con 400 en una semana** mientras el contador marcaba **1**.
- Observado repetidamente: 18 settles fallidos en una hora → el contador subió **2**.

**No calculen tasa de éxito con ese contador.**

### El volumen NO es dinero cobrado neto

El **93%** del tráfico va por esquema `escrow`, y ahí un `authorize`, un `release` y
un `refundInEscrow` producen **filas indistinguibles que suman todas en positivo**.
Un reembolso engorda el volumen igual que un cobro; el tipo de operación no se
persiste en ningún campo.

**Para ingreso neto, filtrar por `scheme=exact`.** Un settle `exact` es un cobro y
punto.

*(Ejemplo real: 13 de los últimos settles fueron reembolsos — dinero saliendo — y el
volumen los contó como si entrara.)*

### El stream es lossy

`/events` se escribe **después** de liquidar y sin bloquear el pago. Un almacén
inalcanzable pierde filas y nunca frena una operación. **Ausencia de evento no es
evidencia de que no ocurriera.** El registro no es un libro mayor; la cadena sí.

### Contaminación por diagnóstico

`/api/stats` incluye tráfico de sondeo interno **sin marcarlo**. Concretamente: 2 de
los verifies contabilizados son sondas mías del 2026-08-04 ~14:11 UTC, no tráfico real.

---

## 7. Otros datos útiles medidos esta semana

**Franja horaria de la demanda.** Los pagos NO son continuos: caen a **0,75
operaciones/hora entre 07h y 14h UTC** frente a **12/hora** fuera. El máximo observado
sin ningún pago fue de **10,3 horas** (2026-08-06). El descubrimiento, en cambio, es
plano: `/discovery/resources` no baja de ~787 peticiones/hora **ninguna hora del día**.
Son dos poblaciones de usuarios distintas.

**Cómo distinguir "estamos caídos" de "nadie paga" en un minuto:** contar **intentos**,
no éxitos. `POST /settle` con cualquier status, más `/health` y `/supported`. Con 1
intento en 9 h y `/health` respondiendo en 0,27 s, la respuesta es demanda, no
disponibilidad.

**El catálogo encoge y NO es pérdida de datos.** El conteo visible baja ~25/hora
porque el probador va cuarentenando recursos nunca sondeados, de los que **el 99,8%
resulta estar muerto**. Nada se borra: con `health=any` siguen apareciendo los ~21.500.
De ellos solo **~1.700 están vivos (8%)**. **Filtrar por `health=alive` al elegir un
recurso**, o se pega contra sitios muertos el 92% de las veces.

**Límites de paginación.** Un recorrido completo del catálogo son ~51 páginas de 100
(o **214** si se usa `health=any`). Cualquier límite por debajo no frena abuso: corta
a un cliente honesto a mitad. Por eso el burst de `/discovery/register` se subió de 5
a 250. Regla: dimensionar por `total / page_size` + holgura.

**Parámetros aceptados en `/discovery/resources`:** `limit`, `offset`, `category`,
`network`, `provider`, `tag`, `source`, `sourceFacilitator`, `health`, `tier`, `q`.
Cualquier otro devuelve **400** — a propósito, para no fallar en silencio. No hay
ordenación ni paginación por cursor (alguien las sondeó seis veces esta semana).

**RPC públicos: usar siempre un control.** Consultando cadenas con RPC públicos, una
tanda de `eth_getCode` devolvió "sin código" para **todo**, incluidas direcciones que
sí existen. Eran respuestas **403 por rate limit**, no ausencia de contratos. Se
detectó por incluir un control conocido (USDC). Sin control se reportan averías
inexistentes.

---

## Resumen de acciones pendientes por equipo

**KarmaCadabra**
- Comparar `operatorAddress` y `tokenCollector` de su config de **Ethereum** con
  `0xc2377a9d…` y `0x230fd3a1…` (sección 1).
- Si coinciden, corregir config o desplegar en Ethereum; si no, pasar el
  `debug_traceCall`.
- Revisar el punto 2 de su handoff a EM: el expiry infinito era dato mío erróneo
  (sección 2).
- Confirmar si usan los endpoints de Near/Unichain/Hyperliquid (sección 5).

**Execution Market**
- Cruzar los 29 timestamps con `payment_events` (sección 4).
- Corregir el shape del envelope v2 si se confirma que son suyos.
- Revisar la decisión sobre ADR-001 / timeout del 09-ago con el dato correcto del
  expiry (sección 2).

**402milly**
- **Rotar las credenciales de QuickNode hardcodeadas** en
  `scripts/extract_refunds_with_onchain_verification.py` (sección 5).
- Confirmar si ese script sigue vivo antes de dar de baja los endpoints de Unichain
  y Hyperliquid.

**x402-rs (nosotros)**
- Desplegar `ea435560` (logging del operador en la rama de fallo).
- Validar que la dirección CREATE3 contiene el contrato correcto, no solo que la
  dirección coincide (sección 1).
- Subir el burst de `/discovery/register` antes de que tenjin.blog cruce los 250
  recursos: van **237**, crecen ~3/día. Al cruzarlo empezará a perder altas **en
  silencio** — sin error visible, solo recursos que faltan.
