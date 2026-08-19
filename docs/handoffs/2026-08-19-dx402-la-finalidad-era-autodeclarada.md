# La finalidad era autodeclarada — y `verified` va a cambiar de significado

**Para:** KarmaKadabra
**De:** facilitador (x402-rs), 2026-08-19 (segundo envío del día)
**Versión:** 1.84.0

---

## 0. Lo que tienen que hacer

Una línea: **dejen de leer `verified` como "mi firma sirvió" y lean `signed`.**

Hoy a la mañana les dije lo contrario. Me equivoqué, y el motivo por el que me
equivoqué es una vulnerabilidad crítica que encontramos auditando el arreglo que
les mandé. No rompe nada de lo suyo — sus anclajes siguen entrando, siguen
ocupando el slot y siguen descifrando. Cambia qué campo mirar.

| Antes (1.83.0) | Ahora (1.84.0) |
|---|---|
| firma válida → `verified: true` | firma válida → `signed: true`, `verified: false`, `notVerifiedReason: "proof_missing"` |
| firma rechazada → `422` | igual, `422` |
| sin firma → `verified: false` | igual, más `signed: false` |

---

## 1. Por qué

`verified` es el flag que vuelve un registro **insuperable**. Se decidía
comparando la `sellerSignature` contra `req.payee` — **un campo que manda quien
llama**. O sea que demostrar *"controlo la dirección que yo mismo escribí en mi
propio request"* alcanzaba para quedarse con la evidencia de un pago ajeno,
para siempre.

Y `paymentId` es `keccak256(caip2 || txHash)` sobre datos públicos. Cualquiera
que mire una liquidación puede calcularlo, adelantarse al vendedor, declarar su
propia dirección como payee, firmar con su propia clave — y quedar registrado
como FINAL.

Es peor que el secuestro que ustedes reportaron el 18. Aquel dejaba al atacante
con un reclamo *provisional* que el vendedor real podía superar. Este lo dejaba
con uno *definitivo*. **El arreglo que les mandé abrió una puerta peor que la que
cerró**, y sólo apareció porque fuimos a atacarlo en vez de a releerlo.

Lo reprodujimos como test antes de tocar nada
(`an_attacker_cannot_self_sign_a_final_anchor_for_someone_elses_payment`).

---

## 2. El modelo nuevo: una escalera de tres escalones

El error de fondo fue meter dos preguntas distintas en un solo booleano:

| escalón | qué significa | a quién puede superar |
|---|---|---|
| **2** `verified` | **la cadena** dice que esa dirección es el payee | a cualquiera de abajo |
| **1** `signed` | el reclamante se comprometió con una identidad que controla | sólo al escalón 0 |
| **0** | reclamo anónimo | sólo un slot vacío |

Escalones iguales **no** se pisan: el anti-replay sigue valiendo, el primero se
queda con el slot.

Su firma sigue haciendo exactamente lo que hacía de útil: los pone en el escalón
1, que es lo que los protege de que un anclaje anónimo les gane el lugar. Lo que
ya no hace es certificar una autoría que nadie verificó contra la cadena.

### Cómo llegar al escalón 2

Manden `proofOfPayment` en el anchor. Ahí el gate lee el recibo on-chain, compara
la firma contra el payee **que leyó de la cadena**, y si cierra, quedan
`verified: true` y su registro es final.

Hoy eso funciona sólo en EVM. En Solana el gate no puede leer el pago
(`unverifiable_chain`), así que el escalón 2 no es alcanzable ahí todavía — es
exactamente el ítem que Saul priorizó en el backlog, y esta auditoría le sube la
urgencia: **sin el gate de Solana, en Solana nadie puede pasar del escalón 1.**

---

## 3. Otros dos que les pueden pegar

### Un duplicado rechazado borraba la evidencia que perdía

El blob `sealed` se subía al objeto de S3 (indexado por `paymentId`) **antes** de
que el anti-replay decidiera, y el bucket tiene versionado desactivado. Cualquiera
podía POSTear un `paymentId` ya anclado con basura, destruir irreversiblemente el
ciphertext real, y recibir un 409 prolijo como si no hubiera pasado nada. El
`contentHash` registrado ya no se podía reproducir nunca más.

Ahora el slot se reserva **antes** de escribir un solo byte.

### Una respuesta pagada demasiado grande se entregaba vacía

Del lado del hook de vendedor (`x402-axum`): si el body superaba el límite, se
devolvía la respuesta con `Body::empty()`. Con `settle_after_execution` el pago
**ya liquidó** y el nonce está gastado — el comprador pagaba y recibía un 200 con
cero bytes, sin posibilidad de reintentar.

Si alguno de sus vendedores usa el hook de Rust con cuerpos grandes, esto los
tocaba. Ya está: el cuerpo se entrega siempre, y uno que se anuncia gigante pasa
de largo sin bufferizarse (antes se cargaba entero en memoria sólo para medirlo,
lo que convertía una descarga grande en un OOM de la task en vez de un skip).

---

## 4. Lo que no cambia

- Sus anclajes actuales siguen sirviendo. `verified: false` no los invalida.
- El sobre, el `contentHash`, el descifrado: intactos.
- Los 422 y el techo de 64 KiB que arreglamos esta mañana: igual.
- Los SDKs no necesitan cambios para esto. Si quieren leer el escalón, está en
  la respuesta del anchor y en `GET /dx402/evidence/{paymentId}`.

---

## 5. Autocrítica, porque corresponde

Les mandé un handoff a la mañana diciendo *"miren `verified`, es la señal de una
sola llamada de que su firma sirvió"*. Era un consejo sobre un campo que en ese
momento **cualquiera podía poner en true para el pago de otro**. El campo existía
desde v1.82.0, o sea que estuvo mal dos releases, y lo documenté como garantía
sin haberlo atacado.

Lo que lo encontró no fue leer el código otra vez — fue escribir el exploit.
