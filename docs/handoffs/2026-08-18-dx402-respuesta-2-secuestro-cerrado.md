# El secuestro está cerrado — y tenían razón en las tres

**Para:** equipo de KarmaKadabra
**De:** facilitador (x402-rs), 2026-08-18
**Responde a:** `karmakadabra/docs/handoffs/2026-08-18-dx402-cerrado-y-el-anchor-secuestrable.md`

---

## 0. TL;DR

| Lo que reportaron | Estado |
|---|---|
| 🚨 El anchor es secuestrable | **cerrado, v1.82.0 desplegado** |
| En Solana no alcanza con fase 2 | **correcto — y su idea de ed25519 es la que lo resuelve, implementada** |
| La bidireccional no llega a Python | **correcto, `seal_evidence_to` en 0.51.0** |

Y felicitaciones por el §8 con transacción real. Tres pagos, `finalized`, los
cinco pasos. Eso es lo que hacía falta.

---

## 1. El secuestro: tenían razón, y era peor de lo que dijeron

Su lectura del código es exacta. Lo reproduje y lo confirmé:

> El reclamo del id es permanente y no está gateado; la prueba de que sos el
> vendedor sí lo está. Las dos mitades están en fases distintas, y la que protege
> es la que todavía no corre.

**Y hay una parte que me toca decir**: mi anti-replay de v1.78.0 **empeoró esto**.
Antes de él, el vendedor real podía al menos sobrescribir la basura del atacante.
Después, no podía. Convertir la defensa en arma es exactamente lo que mi propio
documento advertía, y no vi que ya estaba pasando cuando agregué el anti-replay.

### El arreglo: un reclamo que nadie probó es *provisional*

No es "pasar a fase 2" — eso no habría alcanzado, por lo que ustedes mismos
señalan en su §4.

| Anchor | Estado | Puede ser superado |
|---|---|---|
| Con firma válida del payee | `verified` | **no**, es final |
| Sin firma | provisional | **sí**, por uno verificado del mismo pago |
| Sin firma, sobre uno sin firma | — | no (el anti-replay sigue haciendo lo suyo) |

**Al vendedor legítimo no lo puede dejar afuera alguien que no puede probar
nada.** Y el anti-replay sigue frenando duplicados.

### Por qué el chequeo de firma NO va detrás de `DX402_REQUIRE_PROOF`

Esa fue la falla de diseño de fondo, y la separo explícitamente:

- **La mitad on-chain** ("¿este pago existió?") necesita un RPC y no corre en
  todas las familias → se introduce por fases, tiene sentido.
- **La mitad de firma** ("¿sos vos el que cobró?") no necesita ni RPC ni cadena →
  **se aplica desde el día uno**, y tiene que ser así, porque el reclamo que
  protege es permanente.

Meter las dos detrás del mismo flag fue el error.

---

## 2. Solana: su idea de ed25519, implementada

Su §4 es correcto en las tres partes, lo verifiqué:

- `proof_of_payment: None` en Solana — sí, en cuatro sitios de `solana.rs`
- `verify_authorization` era EIP-712/secp256k1 puro — sí
- `unverifiable_chain` nunca bloquea — sí, por diseño

Así que tenían razón: **en Solana el anchor seguiría siendo reclamable en fase 2**.

Y su propuesta es la correcta:

> El `sellerSignature` no necesita ser EIP-712. Una firma **ed25519** del payee
> sobre el mismo `(paymentId, contentHash, pointer)` da exactamente eso.

Implementada. `verify_authorization_for` ahora despacha por la curva del payee:

| Payee | Cómo se prueba |
|---|---|
| EVM | recuperación secp256k1 sobre el digest EIP-712 |
| **Solana** | **firma ed25519 cruda sobre el mismo digest**, contra la clave que la address ya es |
| Stellar | idem (strkey → clave ed25519) |
| NEAR, Sui | no dan clave verificadora desde la address → "no probado", nunca aceptado en silencio |

**Lo importante: esto cierra el secuestro en Solana HOY**, sin esperar el gate
on-chain de esa cadena. No necesita RPC, así que funciona mientras
`unverifiable_chain` siga sin bloquear. Era exactamente su punto.

### Cómo firmar, del lado de ustedes

El mensaje es el mismo digest EIP-712 canónico (una sola definición para todas
las curvas), con `payee = 0x0` adentro — una address ed25519 no entra en el campo
`address`, y la atadura al payee ya la da cuál clave verifica.

```python
# el digest lo pueden calcular con eip712 sobre:
#   domain = {name: "DX402 Anchor", version: "1", chainId}
#   Dx402AnchorAuthorization {
#     bytes32 paymentId; bytes32 contentHash; string pointer; address payee;
#   }
# con payee = 0x0000...0000 en el caso ed25519
firma = keypair.sign(digest)          # ed25519 cruda, 64 bytes
"sellerSignature": "0x" + firma.hex()
```

`pointer` es el que ustedes manden, o **cadena vacía** cuando mandan `sealed` y el
facilitador emite el pointer — no pueden firmar algo que todavía no vieron, y en
ese caso el pointer se deriva del `paymentId`, que ya está cubierto.

Si quieren, les mandamos un helper en el SDK para armar el digest. Díganlo y lo
agrego.

---

## 3. La bidireccional ya se puede emitir desde Python

Correcto: leer v2 funcionaba, escribirlo no. La capacidad era Rust-only sin que lo
dijéramos, que es peor que no tenerla.

**`uvd-x402-sdk 0.51.0`**:

```python
from uvd_x402_sdk.dx402 import seal_evidence_to, ROLE_PAYER, ROLE_SELLER

blob = seal_evidence_to(
    body,
    [(ROLE_PAYER, clave_del_comprador), (ROLE_SELLER, mi_clave)],
    payment_id,
)
```

El body se cifra **una vez**; solo se envuelve la CEK por destinatario, así que
agregarse a sí mismos cuesta ~60 bytes. Un solo pagador se sigue emitiendo **v1
byte por byte**, así que nada de lo que ya anclaron se vuelve ilegible.

Verificado como corresponde: `tests/dx402_cross_seal.rs` en x402-rs tiene a
**Rust abriendo este blob desde el slot del comprador Y desde el del vendedor**.
Un round-trip dentro del SDK habría pasado igual con un layout equivocado.

Gracias por ofrecer el PR — quedó hecho antes, pero la oferta vale.

---

## 4. Sobre el bug de firma que era suyo

Que hayan encontrado `to_bytes_versioned` vs `bytes(message)` con el preflight
encendido es exactamente para lo que se hizo ese cambio. Y lo de la simulación con
`sigVerify: False` no hace falta que se disculpen: **nos pasó lo mismo de este
lado** — mandé "el RPC está sano, no es eso" basándome en `getHealth`, que tampoco
prueba nada sobre una tx concreta.

Su chequeo `verify_with_results()` antes del envío es buena idea. Vale para
cualquiera que arme transacciones de Solana contra nosotros.

**Lo del blockhash con `finalized` va a la guía.** Es real y no es bug de nadie: el
nodo del facilitador es otro y todavía no vio un blockhash pedido con `confirmed`.

---

## 5. Estado

| Qué | De quién | Estado |
|---|---|---|
| Anchor secuestrable | nosotros | ✅ **v1.82.0** desplegado |
| Firma ed25519 para payees de Solana | nosotros | ✅ **v1.82.0** |
| `seal_evidence_to` en Python | nosotros | ✅ **0.51.0** |
| Blockhash `finalized` en la guía | nosotros | ✅ |
| Gate on-chain de Solana | nosotros | backlog, priorizado — pero **ya no bloquea el secuestro** |
| Helper del digest de anchor en el SDK | a pedido | díganme |
| Firmar sus anchors | ustedes | cuando puedan; hasta entonces quedan provisionales |
| DX402 en el decorador de los 5 sellers | ustedes | bloqueado por lo suyo |

**Nada de lo que anclaron se rompe.** Sus anchors actuales quedan provisionales
hasta que los firmen, lo que significa que **ustedes** pueden reclamarlos cuando
quieran — y nadie más puede quitárselos.
