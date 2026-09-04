# DX402 — Backlog v0.2: opt-in bidireccional y monetización

> **Estado 2026-09-04.** §2 (opt-in del comprador vía `accepts`) cerrado en 2.11.0; §2-bis (sobre bidireccional) cerrado en v1.79.0; §2-ter (el anchor no verificaba el pago) cerrado en v1.78.0→v1.82.0 y endurecido en 2.10.0/2.11.0. Lo que sigue se conserva por el razonamiento, no como estado. Fuente viva: `08-SPEC-v0.2.md` y `09-ESTADO-Y-CAMINO-A-UPSTREAM.md`.

**Fecha:** 2026-08-17
**Origen:** dos preguntas de Saul durante el despliegue de v1.76.0
**Estado:** diseño. **No implementado.** Resolver ANTES de proponer upstream,
porque la parte 2 cambia el formato del envelope.

---

## La pregunta

> ¿Se puede ofrecer la durabilidad como un tipo de servicio: el comprador la
> habilita al pagar, paga un poquito más, y le queda garantizado para siempre?
> ¿Es así como está hoy?

**No, hoy no es así.** Este documento dice qué hay realmente, qué falta, y cuál
parece el camino correcto.

---

## 1. Qué existe hoy (verificado en código)

| Pieza | Estado real |
|---|---|
| Opt-in **por ruta, decidido por el vendedor** | ✅ funciona. Una ruta declara `extensions: {"durable-evidence": {...}}` o no lo declara |
| `retention: "90d" \| "1y" \| "permanent"` | ✅ se respeta: lifecycle de S3 + TTL de DynamoDB |
| `paidBy: "seller" \| "buyer"` | ⚠️ **es solo una etiqueta**. `src/dx402/types.rs:165`. No hay sobreprecio, ni cobro aparte, ni liquidación del fee. No hace nada |
| Opt-in **del comprador** | ❌ no existe |
| Cobro diferencial | ❌ no existe |

Traducido: hoy el vendedor decide por todos sus compradores, y si alguien paga
por la durabilidad, es el vendedor, fuera de banda.

---

## 2. Lo importante: casi no hace falta protocolo nuevo

x402 ya devuelve un **array `accepts`** en el 402 — varias formas de pagar el
mismo recurso. El comprador elige cuál satisface.

Entonces el opt-in del comprador sale de ahí, sin inventar nada:

```jsonc
// 402 Payment Required
{
  "accepts": [
    {
      "scheme": "exact",
      "maxAmountRequired": "10000",        // 0.01 USDC — normal
      "resource": "https://kk.example/data/42",
      "extra": { }
    },
    {
      "scheme": "exact",
      "maxAmountRequired": "12000",        // 0.012 USDC — con evidencia durable
      "resource": "https://kk.example/data/42",
      "extensions": {
        "durable-evidence": {
          "mode": "direct",
          "retention": "permanent"
        }
      }
    }
  ]
}
```

El comprador (o su agente) elige la segunda si quiere que le quede. **El
vendedor ve cuál eligió** porque el `PaymentRequirements` que satisface viene en
el payload, y ahí decide si corre el post-hook.

Ventajas de esta forma:

- **Cero cambios al protocolo x402.** Cae directo del diseño multi-oferta que ya
  existe. Eso importa mucho para la propuesta upstream: una extensión que no pide
  cambiar el core se revisa mucho mejor.
- **El precio lo pone el vendedor**, no nosotros. No estamos en el medio de esa
  decisión comercial.
- **Un comprador viejo que no entiende DX402 elige la primera opción** y todo
  sigue funcionando. Degradación natural.

### Lo que sí habría que construir

1. **Que el post-hook lea qué requirement se satisfizo** y active la evidencia
   solo si ese trae `durable-evidence`. Hoy `with_durable_evidence(hook)` aplica
   a toda la ruta sin mirar el requirement elegido. Es el cambio central.
2. **Que `retention` salga del requirement elegido**, no de la config fija de la
   ruta.
3. Helpers en los SDKs para armar el par de ofertas sin escribirlo a mano.

---

## 2-bis. Bidireccionalidad: la evidencia tiene que servirle a los DOS

> "Debería vivir a un nivel en que el vendedor elija si lo quiere o no, como un
> seguro opcional, y si el vendedor lo provee, que sea de las dos partes."

Esto es un hueco real de v0.1, y es más grave de lo que parece.

### El problema

Hoy el envelope se cifra **solo hacia el pagador**. Consecuencia incómoda: en una
disputa, **el vendedor no puede abrir su propia evidencia**. Vendió algo, lo
ancló, y no puede demostrar qué entregó.

Las dos partes quieren cosas distintas y ambas legítimas:

| Parte | Qué quiere probar | ¿Puede hoy? |
|---|---|---|
| Comprador | "esto es lo que me entregaron" | ✅ sí |
| Vendedor | "esto es lo que entregué, no lo que dice que recibió" | ❌ **no** |

Un comprador de mala fe puede decir "me llegó basura" o "nunca me llegó" y el
vendedor no tiene con qué responder — aunque él mismo pagó por anclarlo. Eso
convierte a DX402 en un arma de una sola dirección, que no es lo que queremos si
la idea es que se adopte como estándar.

### El diseño: envelope multi-destinatario

La CEK se envuelve **N veces**, una por destinatario, en vez de una sola:

```
CEK          := random 32 bytes
ciphertext   := AES-256-GCM(CEK, body, aad = paymentId)   # una sola vez

recipients[] := [
  { role: "payer",   keyAlg, ephemeralPub, wrappedCEK },
  { role: "seller",  keyAlg, ephemeralPub, wrappedCEK },
  { role: "auditor", keyAlg, ephemeralPub, wrappedCEK },  # opcional
]
```

El ciphertext del body **no se duplica** — es el mismo blob. Solo se repite el
wrap de la CEK, que son ~60 bytes por destinatario. El costo de storage
prácticamente no se mueve.

Cada quien abre con su propia clave privada. Nadie más, y sigue sin haber una
parte confiable en el medio.

**Roles a considerar:**

- `payer` — siempre. Es el mínimo de v0.1.
- `seller` — para que pueda defenderse. Su clave sale de `payTo`… con un
  detalle: `payTo` suele ser una dirección de cobro, y en EVM **una address no
  da la clave pública**. El vendedor tendría que declarar una clave de cifrado
  explícita en su config (no la misma con la que cobra — separar roles de clave
  es sano).
- `auditor` — un tercero designado (árbitro, regulador, la DAO). Opt-in
  explícito y visible en el recibo, nunca implícito.

### Lo que hay que resolver antes de escribirlo

1. **Cómo obtiene el vendedor su clave pública de cifrado.** En ed25519 la
   address sirve; en EVM no. Probablemente un campo `sellerPublicKey` en la
   declaración de la ruta.
2. **Que el recibo diga quiénes pueden leer.** Si el recibo no lista los
   destinatarios, un comprador no puede saber que el vendedor —o un auditor—
   también tiene acceso. Eso sería una sorpresa desagradable y arruinaría la
   propiedad de privacidad que estamos vendiendo. **Los destinatarios son parte
   de la atestación firmada, no metadata suelta.**
3. **Versión del formato.** El envelope actual tiene un solo `wrappedCEK`.
   Agregar `recipients[]` es un bump del byte de versión (`FORMAT_VERSION = 1`
   → `2`) con lectura compatible hacia atrás: un blob v1 se interpreta como un
   único destinatario `payer`.

### Cómo se combina con el opt-in

Las dos cosas son ortogonales y se declaran juntas:

```jsonc
"durable-evidence": {
  "mode": "direct",
  "retention": "1y",
  "recipients": ["payer", "seller"]   // el vendedor también se guarda copia
}
```

Y el comprador elige si quiere la oferta con evidencia vía el array `accepts`
(§2). O sea: **el vendedor decide qué ofrece, el comprador decide si lo toma, y
si se toma, protege a los dos.** Que es exactamente el modelo de un seguro
opcional.

---

## 2-ter. `POST /dx402/anchor` no verifica que el pago exista

> **CERRADO** (v1.78.0 gate, v1.82.0 escalera de autoridad, 2.10.0 riel de escrow, 2.11.0 red team). Leído en frío parecía una vulnerabilidad abierta; no lo es.

**Superficie de abuso real, hoy, en producción.** Va acá para que no se descubra
sola.

El endpoint acepta cualquier `paymentId` / `txHash` que le manden. No consulta la
cadena. Entonces cualquiera puede:

- anclar blobs de hasta ~48 KB sin haber pagado nada → **storage gratis**;
- registrar evidencia con un `txHash` inventado → un recibo firmado por nosotros
  atestiguando un pago que no existió.

Lo segundo es lo más feo: el recibo lleva **nuestra firma**. No dice "este pago
ocurrió" —dice "esto fue anclado para este paymentId"— pero la distinción es
demasiado fina como para confiar en que nadie la va a malinterpretar.

Lo que hoy lo contiene (parcialmente):

- rate limit en las rutas `/dx402/*` (el mismo governor que las de lectura)
- tope de 64 KiB de body
- expiración por lifecycle a los 90 días
- el bucket es privado; solo se sirve por `/dx402/blob`

Lo que hay que hacer, en orden de peso:

1. **Verificar el pago on-chain antes de anclar**, igual que hace el gate de
   proof-of-payment de ERC-8004 (`src/erc8004/proof.rs`): que la tx exista, que
   haya tenido éxito, y que `payer`/`payee` coincidan. Ya está el patrón escrito,
   se reutiliza.
2. **Anti-replay por (paymentId)**: un pago ancla una vez. Hoy un segundo anchor
   con el mismo id pisa el registro.
3. Mientras tanto: **fase 1 como en ERC-8004** — verificar y *reportar* sin
   rechazar, mirar los logs, y recién ahí cerrar.

**No proponer upstream sin esto resuelto.** Un endpoint de escritura público sin
verificación es lo primero que va a saltar en una revisión, y con razón.

---

## 3. La parte incómoda: el costo no justifica el precio

Saul dijo "eso ya está accounted for" respecto al storage. Vale la pena poner
números, porque cambian el argumento.

Un body típico de agente (1–50 KB) sellado pesa prácticamente lo mismo. En S3
Standard a ~$0.023/GB-mes:

| Escenario | Costo real |
|---|---|
| 50 KB, 90 días | ~**$0.0000035** |
| 50 KB, permanente, 10 años | ~**$0.00014** |
| 100.000 anclajes de 50 KB, permanentes, 10 años | ~**$14** |

*(Aritmética de la lista de precios, no medición. La real sale del primer mes con
tráfico.)*

**El storage es ruido.** Cobrar "para cubrir el storage" sería deshonesto: no hay
costo material que cubrir a esta escala.

Lo que sí tiene valor —y es lo que se estaría vendiendo— es otra cosa:

- **poder reclamar.** Sin evidencia no hay a quién reclamarle ni qué mostrar.
- **poder auditar** una compra meses después.
- **no repudio**: un recibo firmado que un tercero verifica sin llamarnos.

Eso es un producto legítimo. Pero el pitch honesto no es "cubrimos el storage",
es **"te queda prueba de qué te entregaron"**. Si el precio se justifica con el
costo, el primero que haga la cuenta nos deja mal parados.

> **Riesgo a mirar de frente:** si `permanent` se vende barato y alguien ancla
> millones de blobs, el costo sí aparece — y `permanent` es irrevocable, no se
> puede deshacer después. Cualquier oferta de `permanent` necesita un tope de
> tamaño y algún límite de volumen antes de existir.

---

## 4. Dónde cobra el facilitador (nosotros) — abierto

Distinguir dos cosas que se mezclan fácil:

- **El vendedor le cobra al comprador** por la durabilidad → sale del `accepts`,
  arriba. Ahí nosotros no tocamos plata.
- **Nosotros le cobramos al vendedor** por notarizar e indexar → eso todavía no
  tiene diseño ninguno.

Opciones a investigar para lo segundo, sin recomendación todavía:

1. Gratis para todo el mundo, siempre. Es infraestructura y el valor está en que
   se adopte el estándar. (Es lo que hacemos hoy con verify/settle.)
2. Gratis hasta N anclajes/mes por vendedor, después algo.
3. El vendedor paga el anclaje por x402 — cerrando el círculo: DX402 se cobra a
   sí mismo con el protocolo que extiende. Elegante, y hay que ver si no es
   demasiado cute.

**Sin decidir.** Y no se decide antes de tener tráfico real: hoy no sabemos si el
volumen es de decenas o de millones al mes, y la respuesta cambia con eso.

---

## 4-bis. Pendientes con prioridad fijada (Saul, 2026-08-18)

### El gate del anchor en cadenas no-EVM

`verify_payment_facts` lee un receipt EVM. Fuera de EVM el gate reporta
`unverifiable_chain` y **nunca bloquea**, así que esas cadenas funcionan pero sin
verificación de pago.

| Cadena | Prioridad | Cómo se verifica |
|---|---|---|
| **Solana** | **ALTA — la única que se quiere sí o sí** | `getTransaction` + parseo de token balances. Ya hay algo parecido en `src/chain/solana.rs` para el camino del settlement account |
| Stellar | después | Horizon: operaciones de pago de la tx |
| Algorand | después | Indexer: transacciones de asset transfer |
| NEAR, Sui, XRPL | después | por familia |

**Todas tienen que terminar funcionando igual**, pero solo Solana bloquea. Las
demás quedan explícitamente para más adelante.

### Wallets en custodia — qué queda realmente

Una flota cuyas wallets están en custodia (Paybox, en el caso de KarmaKadabra)
toca DX402 en tres puntos. **Sólo uno es un hueco.**

| Qué necesita | ¿Bloquea a un custodio? |
|---|---|
| Ser destinatario del sobre (ECDH) | **No.** La clave de *cifrado* no tiene por qué ser la de *cobro*: un keypair local sin fondos ni gas, listado como destinatario `seller`. Probado |
| Firmar el anchor | **No.** `signer` es un callable: el custodio recibe el digest de 32 bytes y devuelve la firma, sin que la semilla salga nunca (`anchor_evidence`, SDK 0.53.0) |
| Descifrar como **comprador** | **Sí** — y es el único. Ver abajo |

> **Ítem retirado (2026-08-18).** Acá figuraba *"un custodio que sólo firma
> transacciones no puede firmar anchors"*. KarmaKadabra lo corrigió: **ese límite
> no existe** en su custodia. Lo abrí sobre una restricción que asumí de una
> conversación anterior en vez de confirmarla, y quedó en el backlog dándole peso
> a algo inexistente. Un backlog con un ítem falso es peor que uno corto: la
> siguiente persona diseña alrededor de un problema que nadie tiene.

**El único hueco real: el comprador en custodia.**

Es asimétrico respecto del vendedor, y por eso no se resuelve igual. El vendedor
elige su clave de cifrado, así que puede usar una local. El comprador **no elige**:
la evidencia se cifra hacia la clave pública que se deriva del pago, así que si
esa wallet está en custodia y el custodio no hace ECDH, no puede abrir lo que
compró.

Es exactamente el caso de uso del modo **`escrowed`**, que sigue sin
implementarse (`POST /dx402/recover` devuelve 501). Tercera vez que aparece.

#### La salida que propone KarmaKadabra: que el comprador DECLARE una llave de lectura

**No hace falta construir `escrowed` para cerrar este caso.** Execution Market lo
resolvió sin criptografía nueva: el comprador registra una clave pública que
**sólo descifra**, y el vendedor cifra hacia ella en vez de hacia la derivada del
pago.

```
PUT /api/v1/account/dx402-key   {"public_key": "0x02…", "key_alg": "ECIES-secp256k1"}
```

Medido por KK: **27 agentes declararon**, la evidencia de un trade real abrió con
la llave declarada, no abrió con otra wallet, y **tampoco abrió con la llave de
cobro** — que es la prueba de que la separación no es nominal.

Dos propiedades que lo hacen mejor que `escrowed`, no sólo más barato:

- **Cubre más que la custodia.** También al payer delegado por EIP-7702 y a las
  smart accounts, donde no hay clave recuperable desde la firma. Es el mismo
  agujero por otra puerta.
- **Es mejor seguridad.** Reusar la llave de cobro para descifrar convierte una
  filtración de *"me leen la evidencia"* en *"me vacían la wallet"*. La llave
  declarada no controla un centavo. Y a diferencia de `escrowed`, no centraliza
  en el facilitador la capacidad de descifrar — que es justo lo que esta misma
  nota advierte de `escrowed`.

**El filo, si alguna vez lo hospedamos nosotros:** una declaración de llave
**tiene que estar firmada por la dirección que dice ser**. Si cualquiera puede
declarar una llave para una dirección ajena, el atacante hace que el vendedor
cifre hacia él y se queda con la evidencia de compras que no hizo. Es el hijack
del anchor otra vez, movido del *escribir* al *leer*, y allá el mismo descuido
—una reclamación que nadie probaba— nos costó una versión. Un registro de
llaves sin autenticar es estrictamente peor que no tenerlo, porque el comprador
cree que declaró algo.

Segundo filo: una llave declarada **no puede sustituirse en silencio**. Rotarla
es legítimo; que la rotación sea invisible convierte el registro en un canal para
redirigir evidencia futura. La regla del anchor sirve igual — una declaración
firmada supera a una que no lo está, nunca al revés.

### El PR a la x402 Foundation — EN PAUSA

> **2026-09-04:** la precondición está cumplida — 116 `verified` en 7 redes EVM, 26/24 compradores/vendedores (`10-EVIDENCIA-PARA-EL-PR.md`). El camino que falta está en `09-ESTADO-Y-CAMINO-A-UPSTREAM.md`.

Los dos bloqueantes técnicos están cerrados (gate del anchor v1.78.0, envelope
bidireccional v1.79.0). **No se propone hasta tener tráfico real.** Una propuesta
sin uso en producción se descarta, y hoy el contador de anclajes son pruebas
nuestras, no compras.

Anotado acá para que no se pierda: cuando KarmaCadabra (o execution.market)
acumule N transacciones reales con evidencia recuperable, se retoma. Ahí el
argumento se escribe solo.

> **Estado 2026-08-19:** la precondición está empezando a cumplirse. El contador
> pasó de 4 a 30 anclajes, y KK reporta que toda compra del enjambre en Execution
> Market queda con evidencia durable, verificada sobre un trade real. Sigue en
> pausa — retomarlo es decisión de Saul, no un automatismo por contador.

---

## 5. Orden sugerido

1. **Primero probar que funciona** (KarmaCadabra, el objetivo de hoy). Sin un
   solo anclaje real, todo esto es especulación.
2. **Envelope multi-destinatario** (§2-bis). Va primero entre los cambios de
   diseño porque toca el formato en disco: cuanto más evidencia haya anclada en
   v1, más caro es migrar. Y sin esto la extensión sale al mundo protegiendo a
   una sola de las dos partes, que es una crítica obvia y justa.
3. Opt-in del comprador vía `accepts` (§2) — no toca el formato, solo la
   declaración, así que puede ir después sin costo de migración.
4. La monetización del facilitador al final, con números reales en la mano.

---

## 6. Para la propuesta upstream

Dos argumentos fuertes de cara a la Foundation, y conviene que el spec v0.2 los
diga explícitamente:

1. **El opt-in sale del `accepts` que x402 ya tiene.** La extensión no pide
   tocar el core, se apoya en un mecanismo existente y degrada sola con clientes
   viejos. Incluir el ejemplo del array de dos ofertas.
2. **La evidencia protege a las dos partes.** "Solo el comprador puede leerla"
   invita de inmediato a la pregunta *¿y el vendedor cómo se defiende?*. Mejor
   llegar con la respuesta escrita que con el hueco.

**No proponer con el formato de un solo destinatario.** Un cambio de formato
después de que otros ya implementaron es mucho más caro que hacerlo ahora, que
el único usuario somos nosotros.
