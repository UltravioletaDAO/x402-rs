# DX402 está VIVO — qué puede hacer KarmaCadabra hoy

**Para:** equipo de KarmaCadabra
**De:** facilitador (x402-rs), 2026-08-17
**Reemplaza a:** `2026-08-14-dx402-karmakadabra.md` (ese era el diseño; esto es
lo que está corriendo)

Autocontenido. No hace falta leer nada más para arrancar.

---

## 0. TL;DR

El riel está desplegado en `facilitator.ultravioletadao.xyz`. Falta que alguien
lo use. **Ustedes son el primero.**

Lo que hay que hacer, en orden:

1. `pip install -U 'uvd-x402-sdk[dx402]>=0.48.0'` (tienen pineado `0.42.0`)
2. En el seller, después del settle: sellar el body y anclarlo (~20 líneas)
3. En el buyer, leer un header más
4. Avisarnos cuando haya un anclaje real

**No necesitan storage propio.** Le mandan el ciphertext al facilitador en la
misma llamada del anchor y él lo guarda. Límite: ~48 KB de ciphertext por el
tope de body de 64 KiB (base64 infla un tercio). Si un body es más grande,
pueden subirlo a su bucket y mandar `pointer` en vez de `sealed`.

---

## 1. Qué problema les resuelve, concreto

Hoy `agents_sdk/purchases.py:104` (`registrar(...)`) guarda cada compra en S3
**pero solo un extracto de 600 caracteres** (`_EXTRACTO = 600` en `:42`). O sea:
compraron data, la usaron, y de lo que quedó guardado no se puede reconstruir
qué recibieron.

DX402 es la evolución natural de exactamente ese registro: de *extracto* a
**cuerpo completo, cifrado, que solo el que pagó puede abrir**.

Y del lado vendedor: hoy cuando ustedes venden, la respuesta se entrega una vez y
no queda nada. Si un comprador reclama, no hay artefacto.

---

## 2. La idea en una línea

Una autorización de pago **ya es una firma**, y de una firma sale la **clave
pública** del que firmó — no solo la dirección. Así que el vendedor puede cifrar
hacia el comprador sin pedirle nada, sin registro previo, sin round-trip extra.

**Pagar es publicar tu clave de cifrado.**

En Solana es todavía más directo: la address **es** la clave pública ed25519.

---

## 3. Endpoints vivos

Base: `https://facilitator.ultravioletadao.xyz`

| Método | Ruta | Para qué |
|---|---|---|
| `POST` | `/dx402/anchor` | registrar evidencia (solo metadata) |
| `GET` | `/dx402/evidence/{paymentId}` | pointer + hash + recibo |
| `GET` | `/dx402/receipt/{paymentId}` | recibo firmado, verificable offline |
| `GET` | `/dx402/blob/{paymentId}` | el ciphertext (público, ilegible sin la clave) |
| `GET` | `/dx402/stats` | cuántos anclajes hay + quién firma los recibos |

Chequeo rápido de que está vivo:

```bash
curl -s https://facilitator.ultravioletadao.xyz/supported | jq '.extensions'
# ["bazaar","durable-evidence"]

curl -s https://facilitator.ultravioletadao.xyz/dx402/stats | jq
```

---

## 4. Lado VENDEDOR — dónde va exactamente

### 4.1 Los dos sitios que tienen hoy

Encontramos estos dos puntos leyendo su repo. En los dos, el body y el resultado
del settle ya están en el mismo scope:

| Archivo | Línea | Qué pasa ahí |
|---|---|---|
| `test-seller/main.py` | **256** | `return {"message": ..., "payer": payer, "tx_hash": tx_hash}` — Base |
| `test-seller-solana/main.py` | **199** | lo mismo, Solana |

**Ojo con un detalle**: `test-seller/main.py:256` devuelve un **dict pelado**, no
un `Response`. Para poder pegarle un header hay que envolverlo en `JSONResponse`.

### 4.2 El sitio que de verdad importa

`plans/SELLERS_EM_Y_X402_DIRECTO.md:110` dice que van a poner los 5 agentes
always-on a vender directo con el decorador `x402_required`. **Ese es el lugar
correcto para DX402**: adentro del decorador, una vez, y les queda para los 5
sellers.

El patrón histórico está en
`git show 702b1a57^:agents/karma-hello/x402_middleware.py`, líneas 313-324:

```python
result = await func(request, *args, **kwargs)   # <-- acá nace el body
...
if isinstance(result, JSONResponse):
    result.headers['X-Payment-Tx'] = tx_hash    # <-- DX402 va justo acá al lado
```

**Recomendación: métanlo cuando construyan el decorador, no como retrofit
después.** Es la diferencia entre 20 líneas en un lugar y 5 parches.

### 4.3 El código

```python
import base64
from uvd_x402_sdk.dx402 import (
    seal_evidence, content_hash, payment_id,
    payer_key_from_solana_address, payer_key_from_evm_signature,
)
import httpx

FACILITATOR = "https://facilitator.ultravioletadao.xyz"

def anclar_evidencia(body_bytes, *, network_caip2, network, tx_hash, payer, payee,
                     payer_key, retention="90d"):
    """Sella el body hacia el comprador y lo ancla. Nunca levanta hacia arriba.

    NO necesitan bucket propio: mandan el ciphertext y el facilitador lo guarda
    y les devuelve el pointer. Una sola llamada HTTP.
    """
    try:
        pid  = payment_id(network_caip2, tx_hash)
        blob = seal_evidence(body_bytes, payer_key, pid)

        r = httpx.post(f"{FACILITATOR}/dx402/anchor", timeout=15, json={
            "paymentId":   pid,
            "network":     network,             # "base" | "solana"
            "txHash":      tx_hash,
            "payer":       payer,
            "payee":       payee,
            "sealed":      base64.b64encode(blob).decode(),   # <- el ciphertext
            "backend":     "s3",
            "contentHash": content_hash(body_bytes),          # sobre el PLAINTEXT
            "keyAlg":      "ECIES-X25519",      # o "ECIES-secp256k1" en EVM
            "mode":        "direct",
            "retention":   retention,
        })
        r.raise_for_status()
        return r.json()          # -> va al header X-Durable-Evidence
    except Exception as e:
        log.warning("DX402 no ancló: %s", e)
        return {"v": 1, "skipped": "anchor_failed"}
```

Y en el handler:

```python
import base64, json
from fastapi.responses import JSONResponse

payload = {"message": "Hello World!", "status": "paid", ...}
body_bytes = json.dumps(payload).encode()

# Solana: la address ES la clave. No hace falta la firma.
payer_key = payer_key_from_solana_address(payer)

ev = anclar_evidencia(body_bytes,
                      network_caip2="solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
                      network="solana",
                      tx_hash=tx_hash, payer=payer, payee=PAY_TO,
                      payer_key=payer_key)

resp = JSONResponse(payload)
resp.headers["X-Durable-Evidence"] = base64.urlsafe_b64encode(
    json.dumps(ev).encode()).decode().rstrip("=")
return resp
```

### 4.4 En Base (EVM) la clave sale de la firma

En EVM la address **no** da la clave pública; hay que recuperarla de la firma
EIP-3009, y para eso hace falta el digest EIP-712 exacto que el comprador firmó.

```python
payer_key = payer_key_from_evm_signature(signature, digest)
```

**Cuidado con el digest**: si está mal, la función **no falla** — recupera una
clave pública distinta y perfectamente válida, y el body queda cifrado hacia un
desconocido con todos los logs en verde. El nombre del dominio EIP-712 varía por
cadena y hasta cambia entre mainnet y testnet de la misma cadena.

**Sugerencia: empiecen por Solana**, donde no existe este problema. Base después.

---

## 5. Lado COMPRADOR — dónde va

`agents_sdk/uvd_buyer.py`, líneas **502-515**. Ahí ya parsean
`payment-response`; el header de DX402 se lee tres líneas más abajo:

```python
from uvd_x402_sdk.dx402 import evidence_from_headers, EvidenceSkipped

evidencia = None
try:
    evidencia = evidence_from_headers(resp_hdrs)   # case-insensitive
except EvidenceSkipped as e:
    log.info("el vendedor no ancló: %s", e.reason)
except Exception:
    pass
```

Guarden `evidencia.pointer` + `evidencia.payment_id`. Eso es todo lo que hace
falta después.

**El upgrade natural de `purchases.py:104`**: en vez del extracto de 600
caracteres, guarden el pointer. El cuerpo completo queda recuperable.

Para recuperar, en cualquier momento:

```python
from uvd_x402_sdk.dx402 import recover_evidence

body = recover_evidence(evidencia, mi_clave_privada)  # 32 bytes
```

`recover_evidence` verifica el `contentHash` sola y levanta
`ContentHashMismatch` si el vendedor ancló algo distinto de lo que sirvió. **Ese
error no es un problema de red — es la detección de un fraude.** Trátenlo así.

---

## 6. Reglas que no se pueden romper

- **DX402 NUNCA puede hacer fallar un pago.** Todo el bloque va en `try/except`
  que devuelve un skip. Si el cifrado falla, si el storage no responde, si el
  comprador es una wallet de contrato sin clave recuperable: se entrega la
  respuesta igual. El facilitador está construido con esa regla; su lado también.
- **`contentHash` va sobre el PLAINTEXT**, no sobre el ciphertext. Sobre el
  ciphertext solo probaría que el blob no se corrompió.
- **`paymentId` es el AAD del cifrado.** Si el vendedor y el comprador lo derivan
  distinto, el descifrado falla sin causa aparente. Usen `payment_id()` del SDK
  en los dos lados, no lo escriban a mano.
- **Anclar es publicar.** `retention: "permanent"` es irrevocable. El default es
  90 días a propósito. No anclen permanentemente data sensible de un cliente.
- **404 y 410 no son lo mismo.** 404 = nunca existió; 410 = venció la retención.
  Y **503 es reintentable** — no lo guarden como "no hay evidencia".

---

## 7. Un detalle que les afecta

Ustedes mismos lo escribieron en `agents_sdk/uvd_buyer.py:1-6`: como
**compradores**, sus compras las liquida el facilitador **del vendedor**, no el
nuestro. Así que:

- Como **vendedores** → DX402 aplica directo, es su facilitador.
- Como **compradores** → solo van a recibir evidencia si el vendedor la produce.
  Hoy nadie más la produce todavía.

Por eso el primer test tiene que ser **ustedes vendiéndose a ustedes mismos**:
`test-seller-solana` como vendedor, un agente suyo como comprador. Es el ciclo
completo bajo su control.

---

## 8. Cómo sabemos que funcionó

El test de aceptación, punta a punta:

1. El comprador paga y recibe `X-Durable-Evidence` en la respuesta.
2. `GET /dx402/evidence/{paymentId}` devuelve pointer + contentHash + recibo.
3. El comprador baja el blob y lo descifra **con su propia clave**.
4. El `contentHash` coincide con lo que descifró.
5. **Otra wallet baja el mismo blob y NO puede abrirlo.** Esa es la prueba de
   privacidad, y es la que más nos interesa ver.

Cuando eso pase una vez con una transacción real, avísennos. Con N de esas es
cuando proponemos la extensión a la x402 Foundation — y sin uso real en
producción la descartan.

---

## 9. Lo que todavía no existe

Para que no lo busquen:

- **`POST /dx402/recover` devuelve 501.** Es a propósito: el modo `direct` no
  necesita endpoint de recuperación porque el comprador ya tiene la única clave.
- **No hay opt-in del comprador todavía.** Hoy el vendedor decide por ruta. Que
  el comprador elija (y pague un poco más) está diseñado pero no implementado —
  `docs/plans/dx402/04-BACKLOG-MONETIZACION.md`.
- **La evidencia hoy se cifra SOLO hacia el comprador.** El vendedor no puede
  abrir la suya, así que todavía no les sirve para defenderse de un reclamo
  falso. El envelope multi-destinatario está diseñado para v0.2 (mismo doc).

---

## 10. Referencias

- Guía completa: `docs/DX402.md` en x402-rs
- Spec normativa: `docs/plans/dx402/02-SPEC-v0.1.md`
- Por qué existe / estado del arte: `docs/plans/dx402/00-RESEARCH.md`
- Backlog v0.2: `docs/plans/dx402/04-BACKLOG-MONETIZACION.md`
- SDK Python ≥ 0.48.0, extra `dx402` (trae `cryptography`, `eth-utils`, `eth-keys`)
