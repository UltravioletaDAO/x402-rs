# Handoff — DX402 en MeshRelay y describe.net

**Para:** equipos de meshrelay.xyz y describe.net
**De:** facilitador (x402-rs), 2026-08-14

Los dos papeles acá son chicos y bien acotados. Ninguno de los dos toca plaintext
ni claves.

---

## Contexto en tres líneas

x402 entrega el recurso pagado una sola vez y no guarda nada: la liquidación es
durable, la entrega no. DX402 cifra una copia de la respuesta hacia la clave
pública del que pagó — recuperada de la firma del pago — y la ancla.

Spec: `docs/plans/dx402/02-SPEC-v0.1.md`. Guía: `docs/DX402.md`.

---

## 1. MeshRelay — nuevo tipo de evento

**Papel:** avisar que hay evidencia lista. Nada más.

Nuevo event type `durable_evidence`:

```json
{
  "type": "durable_evidence",
  "paymentId": "0x...",
  "network": "base",
  "pointer": "s3+https://.../abc.dx402",
  "contentHash": "0x...",
  "mode": "direct",
  "retentionUntil": 1767225600
}
```

**Regla dura: MeshRelay nunca transporta plaintext ni material de clave.** El
pointer y el hash son metadata; el contenido detrás del pointer está cifrado
hacia el pagador y es inútil para cualquier otro, incluido el relay. Si alguna
vez aparece un campo con la CEK o con el cuerpo en claro en un evento, es un bug
de quien lo emitió, no una feature.

Fuente de los eventos: el facilitador (`/dx402/anchor` responde con el recibo) o
directamente execution.market cuando ancla una submission.

Consumidores esperados: los agentes buyer que estén esperando su evidencia para
descargarla y descifrarla.

---

## 2. describe.net — marcar soporte en el descubrimiento

**Papel:** que un comprador pueda saber, **antes de pagar**, si ese recurso le va
a dejar evidencia durable.

Al registrar/describir un recurso pagado, permitir declarar:

```json
{
  "extensions": ["durable-evidence"],
  "durableEvidence": {
    "mode": "direct",
    "retention": "90d"
  }
}
```

Dos detalles que importan:

- **`mode` tiene que viajar.** `direct` y `escrowed` hacen afirmaciones
  materialmente distintas sobre quién puede leer el payload: en `direct` nadie
  más que el pagador, en `escrowed` el facilitador técnicamente puede. Un
  comprador que no pueda distinguirlos va a confiar de más en el más débil.
- **`retention` tiene que viajar.** "90 días" y "permanente" son promesas
  distintas, y `permanent` además es irrevocable.

No inventen un default. Si un recurso no lo declara, lo honesto es no mostrar
nada — no asumir que soporta DX402.

---

## Verificación (cualquiera de los dos, sin hablar con nosotros)

El recibo de evidencia está firmado EIP-712 con dominio
`{name: "DX402 Evidence", version: "1", chainId}`. Se verifica offline contra la
address que publica el facilitador en `GET /dx402/stats` (`receiptSigner`). No
hace falta llamarnos para saber si un recibo es legítimo — que es justamente la
propiedad que los drafts IETF de recibos x402 señalan como faltante en un
`PAYMENT-RESPONSE` pelado.
