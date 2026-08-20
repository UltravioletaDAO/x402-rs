# DX402: elegir dónde vive la evidencia, y por qué `verified` se les puso más difícil

**Para:** KarmaKadabra
**De:** facilitador (x402-rs), 2026-08-19
**Versiones:** facilitador **1.87.0** · Python **0.58.0** · npm **2.63.0**

---

## 0. Lo que tienen que hacer

Dos cosas, en orden:

1. **Actualizar a `uvd-x402-sdk` 0.58.0 (PyPI) / 2.63.0 (npm).**
2. **Si quieren `verified: true`, ahora hay que mandar `proofOfPayment`.** Antes
   les alcanzaba con la firma. Cambió por una vulnerabilidad crítica, y el SDK
   recién ahora tiene cómo mandarlo.

Todo lo demás es opcional.

---

## 1. Lo que cambió y les afecta: `verified` ya no sale de la firma

Se lo adelantamos en el handoff anterior, pero ahora hay acción concreta.

`verified` es el flag que vuelve un anclaje **insuperable**. Se decidía
comparando la `sellerSignature` contra `req.payee` — **un campo que manda quien
llama**. Demostrar *"controlo la dirección que yo mismo escribí"* alcanzaba para
quedarse con la evidencia de un pago ajeno, para siempre.

Y había un segundo agujero, peor: aun atándolo al payee on-chain, alcanzaba con
**un auto-pago de un wei**. Te mandás un token a vos mismo, obtenés un
`proofOfPayment` perfectamente válido donde sos payer y payee, y lo presentás
contra el `paymentId` de otro. Nadie comprobaba que la prueba fuera de *esa*
transacción.

Hoy el escalón final exige las dos cosas: prueba on-chain **y** que esa prueba
corresponda al pago que se reclama.

### Cómo llegar a `verified: true` desde el SDK

```python
from uvd_x402_sdk import anchor_evidence

res = anchor_evidence(
    body,
    payment_id_value=pid, network="base", tx_hash=tx,
    payer=payer_addr, payee=seller_addr, payer_key=payer_pubkey,
    proof_of_payment={                      # <- esto es lo nuevo
        "transactionHash": tx,
        "blockNumber": blk,
        "network": "base",
        "payer": payer_addr,
        "payee": seller_addr,
        "amount": str(neto),                # OJO: el NETO, ver abajo
        "token": token_addr,
        "timestamp": block_ts,
        "paymentHash": payment_hash,
    },
    signer=lambda d: firmar(d),
)
res["verified"]            # True si la cadena lo confirmó
res["signed"]              # True si su firma fue aceptada
res["notVerifiedReason"]   # p.ej. "dx402_proof_missing"
```

**Miren `signed`, no sólo `verified`.** Si su firma se aceptó pero la cadena no
se pudo leer (toda familia no-EVM hoy), van a ver `signed: true` con
`verified: false`. Eso NO es un error de firma.

**El detalle que ya los mordió una vez:** los pagos de Execution Market llevan
**dos** `Transfer` (comisión y neto). El `amount` de la prueba tiene que ser el
**neto que el payee realmente recibe**, o el gate contesta
`proof_transfer_not_found`.

**En Solana todavía no se puede llegar al escalón 2.** El gate no sabe leer ese
pago (`unverifiable_chain`). Es el ítem que Saul priorizó; hasta entonces, en
Solana el máximo es `signed: true`.

---

## 2. Lo nuevo: elegir dónde vive la evidencia

`DX402_STORE_BACKEND=ipfs` ya funciona. Pinata va **delante** de S3, no en su
lugar: si Pinata se cae, el anclaje aterriza en S3 igual y el registro dice
dónde quedó de verdad.

### Pregunten qué ofrece, no asuman

```python
from uvd_x402_sdk import available_backends

for b in available_backends("https://facilitator.ultravioletadao.xyz"):
    print(b["id"], b["retention"], "revocable" if b["revocable"] else "IRREVERSIBLE")
```

```ts
import { availableBackends } from 'uvd-x402-sdk';
const backends = await availableBackends();
```

Lo que existe **depende del despliegue**. Un facilitador sin credencial de Pinata
ofrece sólo `s3`. Y ustedes pueden terminar apuntando a uno que no es el nuestro,
así que una lista escrita a mano en su código es una promesa que tiene que
cumplir otro.

### Los tres backends y en qué se diferencian de verdad

| id | retención | ¿se puede borrar? | ¿lo resuelve cualquiera? |
|---|---|---|---|
| `s3` | 90d | **sí** | no |
| `ipfs-private` | 90d | **sí** | no |
| `ipfs-public` | permanente | **NO** | sí |

`revocable` no es decoración. En `ipfs-public`, despinnear saca *nuestra* copia,
no la de la red — así que el `retentionUntil` que **nosotros firmamos** en el
recibo dejaría de ser cierto. Por eso `ipfs-public` está **apagado** aunque la
credencial funcione: el ciphertext que se vuelve permanente es el **del
comprador**, y el comprador todavía no tiene cómo consentir. Se prende cuando
exista el opt-in por `accepts`.

Para elegir:

```python
anchor_evidence(body, ..., storage="ipfs-private")
```

Si piden uno que ese facilitador no ofrece, contesta con error nombrándolo. **No
lo guarda en otro lado calladamente** — creer que su evidencia es permanente
cuando no lo es sería peor que un rechazo.

---

## 3. Lo que ya venía de antes y sigue valiendo

- El techo del anclaje es **64 KiB de request** → ~47 KB de plaintext. Los SDKs
  cortan con `skipped: "too_large"` **antes** de tocar la red.
- `anchor_evidence()` / `anchorEvidence()` **nunca levantan**. Todo fallo vuelve
  como skip, con `status` y `error` del facilitador cuando los hay.
- `409 dx402_already_anchored` ahora significa duplicado de verdad. Si su firma
  no verifica reciben **`422 dx402_signature_not_verified`**.
- El `signer` es un **callable**, no una clave: recibe el digest y devuelve la
  firma, para que un custodio firme sin exponer la semilla.
- La forma del digest la elige el SDK según la curva del payee. **No la armen a
  mano** — firmar la forma equivocada no da error, sólo produce una firma que
  nunca verifica.

---

## 4. Qué pueden probar hoy

1. Actualizar y correr un anclaje normal: debe seguir andando idéntico.
2. `available_backends()` contra nuestro facilitador: deberían ver `s3` y
   `ipfs-private` habilitados, y `ipfs-public` deshabilitado con su motivo.
3. Un anclaje con `storage="ipfs-private"`.
4. **El que más nos sirve:** un anclaje con `proofOfPayment` real de un pago de
   EM en Base, a ver si llegan a `verified: true`. Es el camino que nunca se
   ejercitó de punta a punta con tráfico real, y es donde más probable es que
   quede algo mal — sobre todo el asunto de los dos `Transfer`.

Si el (4) falla, mándennos el `notVerifiedReason` y el hash de la transacción;
con eso se diagnostica sin adivinar.

---

## 5. Lo que todavía NO está

Para que no lo prueben y se frustren:

- **Gate on-chain en Solana** — priorizado, sin hacer.
- **`ipfs-public`** — apagado hasta que exista el opt-in del comprador.
- **Opt-in del comprador vía `accepts`** — diseñado, sin implementar.
- **Barredor de retención en Pinata** — sin él, en el camino Pinata "90 días"
  todavía no se cumple solo. En S3 sí, lo hace una regla del bucket.
- **`escrowed` / `POST /dx402/recover`** — sigue devolviendo 501 honesto.
