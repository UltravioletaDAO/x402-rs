# DX402 — todo cerrado de nuestro lado

**Para:** equipo de KarmaKadabra
**De:** facilitador (x402-rs), 2026-08-18
**Cierra:** el ciclo de handoffs del 17 y 18 de agosto

---

## 0. Qué cambia para ustedes

**Actualicen a `uvd-x402-sdk` 0.53.0** (npm 2.58.0) y borren el código de
anclaje que escribieron a mano. Ahora es una llamada.

```python
from uvd_x402_sdk.dx402 import anchor_evidence, evidence_header

ev = anchor_evidence(
    body_bytes,
    payment_id_value=pid, network="solana", tx_hash=tx,
    payer=buyer_address, payee=mi_address,
    payer_key=payer_key_from_solana_address(buyer_address),
    seller_encryption_key=mi_clave_de_cifrado,   # copia legible para ustedes
    signer=firmar_con_paybox,                    # prueba que el anchor es suyo
)
resp.headers["X-Durable-Evidence"] = evidence_header(ev)
```

Sella, firma, sube y postea. **Nunca levanta**: todo fallo devuelve un skip,
porque la evidencia es una adición al camino de pago y jamás una compuerta
delante. Un facilitador inalcanzable les cuesta el recibo, no la venta.

Probado contra producción antes de escribir esto:

```
anclado           : True
abre el COMPRADOR : True
abre el VENDEDOR  : True
wallet ajena      : rechazada
```

---

## 1. Su problema de custodia: resuelto, y no como pensaban

Dijeron que un vendedor en custodia no puede ni firmar el anchor ni ser
destinatario del sobre. **Las dos mitades tienen salida ahora.**

### Ser destinatario: nunca estuvo bloqueado

La clave de **cifrado** no tiene por qué ser la de **cobro** — y no debería
serlo. Generen un keypair local que solo descifra evidencia: sin fondos, sin gas,
sin custodio. Va como `seller_encryption_key`.

Aparte de desbloquearlos, separarlas es lo correcto: usar la clave con la que
cobran para también descifrar convierte una filtración de *"leen mi evidencia"* en
*"me vacían la wallet"*.

### Firmar: `signer` es un callable, no una clave

```python
def firmar_con_paybox(digest: bytes) -> str:
    return "0x" + paybox.sign_raw(digest).hex()
```

El custodio recibe el digest y devuelve la firma. **La semilla no sale nunca.**
Es exactamente la separación en dos pasos que ustedes ya habían hecho a mano, y
ahora es la interfaz.

Si su custodio solo firma transacciones y no bytes arbitrarios, ahí sí no hay
salida hoy: sus anchors quedan provisionales. No es urgente —nadie se los puede
quitar sin firmar tampoco— y está en el backlog con dos caminos.

---

## 2. Todo lo que quedó cerrado

| Lo que reportaron | Estado |
|---|---|
| El riel de Solana no liquida | ✅ v1.81.0 — `skip_preflight` era lo que lo escondía |
| `payer_key_from_solana_address("")` daba clave válida | ✅ 0.50.0 |
| `dx402.__all__` sin el lado vendedor | ✅ 0.50.0 |
| Compute unit price real = 1.000.000 | ✅ confirmado, era doc de ustedes |
| §4.4: la cadena fácil para el vendedor puede ser imposible para el comprador | ✅ en la guía |
| 🚨 El anchor era secuestrable | ✅ **v1.82.0** — un reclamo sin firma es provisional |
| En Solana no alcanzaba con fase 2 | ✅ **v1.82.0** — firmas ed25519, su idea |
| La bidireccional no llegaba a Python | ✅ 0.51.0 |
| Helper del digest | ✅ 0.52.0 |
| El extra sin backend de hashing | ✅ 0.52.1 |
| `chainId: 0` en no-EVM | ✅ documentado con crédito |
| Armar el anchor a mano | ✅ **0.53.0 / 2.58.0** |

---

## 3. Qué está corriendo

| | Versión |
|---|---|
| Facilitador | **1.82.0** |
| PyPI `uvd-x402-sdk` | **0.53.0** |
| npm `uvd-x402-sdk` | **2.58.0** |

Los dos SDKs están **a la par**: los dos sellan v1 y v2, leen los dos, decodifican
addresses de Solana, arman el digest, firman en ambas curvas y anclan en una
llamada.

Verificación cruzada: **Rust abre lo que sellan Python y TypeScript**, en ambas
curvas y en ambos slots de un sobre bidireccional, contra fixtures commiteados.
Y **Rust verifica las firmas de anchor que producen los dos SDKs**. Los tres
coinciden byte a byte.

---

## 4. Lo que NO está, dicho claro

Para que nadie lo busque:

- **`POST /dx402/recover` devuelve 501.** El modo `escrowed` no existe. Es el
  caso de un comprador cuya clave está en custodia — su §4.4 original. Backlog.
- **El gate on-chain no corre fuera de EVM.** En Solana reporta
  `unverifiable_chain` y **nunca bloquea**. Ya no es lo que sostiene la seguridad
  del anchor: eso lo cierra la firma, que sí funciona en Solana. Backlog, y es la
  única no-EVM priorizada.
- **El gate on-chain está en fase 1** (`DX402_REQUIRE_PROOF=false`): verifica y
  reporta, no rechaza. Sus pruebas con `txHash` sintético siguen pasando.
- **Un custodio que solo firma transacciones** no puede firmar anchors. Backlog.

---

## 5. Sobre cómo trabajamos esto

Vale dejarlo escrito. Encontraron tres cosas que nuestros tests no: la address
vacía, el `__all__`, y el secuestro del anchor — que además **yo empeoré** con el
anti-replay de v1.78.0, exactamente el riesgo que mi propio documento describía sin
que yo viera que ya estaba ocurriendo.

Y los dos mandamos, en su momento, un chequeo verde que no cubría lo que estaba
roto: su simulación con `sigVerify: False` y mi `getHealth`. La regla que queda es
la que ustedes escribieron: **antes de usar un chequeo para exculparse, verificar
qué valida.**

El bug del backend de hashing lo encontró instalar el paquete publicado en un venv
vacío, no la suite de tests — que corre en un entorno que ya lo tenía. Ese es el
mismo error otra vez, en otra ropa.

Nada pendiente de nuestro lado que los bloquee. Cuando corran el ciclo con el
decorador de los 5 sellers, avisen — y si algo falla, el error ahora dice qué fue.
