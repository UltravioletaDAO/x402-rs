# Handoff — DX402 en Execution Market

**Para:** equipo de execution.market
**De:** facilitador (x402-rs), 2026-08-14
**Estado del riel:** implementado y testeado en el facilitador.

Autocontenido. No hace falta leer nada más.

---

## 1. Por qué encaja acá mejor que en ningún otro lado

Execution Market **ya exige evidencia**: los workers suben GPS, documentos y data
con `em_submit_work`, y el publisher aprueba con `em_approve_submission`. O sea
que ya tienen el concepto de "paquete de evidencia atado a un pago".

Lo que falta es que esa evidencia sea:

1. **durable** — que sobreviva más allá de la sesión y del registro actual,
2. **privada** — hoy, quien tenga acceso al store la lee; debería leerla **solo
   quien pagó**,
3. **no repudiable** — con un recibo firmado que un tercero pueda verificar
   offline, sin llamarnos.

Eso es exactamente DX402.

## 2. La idea

La autorización de pago ya es una firma, y de una firma sale la clave pública del
firmante. Entonces el worker puede cifrar la evidencia hacia la wallet del
publisher **sin pedirle nada** y sin registro previo.

Ustedes ya tienen ERC-8128 wallet auth, así que la identidad del publisher ya está
resuelta del lado de ustedes — DX402 la reutiliza como destino de cifrado.

## 3. Dónde va exactamente

```
worker ──em_submit_work──►  backend EM
                              │ paquete de evidencia (GPS, docs, data)
                              │ [DX402] cifrar hacia la wallet del PUBLISHER
                              │ [DX402] subir ciphertext → pointer
                              │ [DX402] POST /dx402/anchor
                              │ guardar SOLO pointer + contentHash en la submission
publisher ──em_approve_submission──► libera el pago
publisher ──em_check_submission──►  recibe el pointer
publisher ──em_recover_evidence──►  descifra con SU wallet
```

Dos cambios concretos:

- **`em_submit_work`**: en vez de guardar el paquete en claro, guardar
  `pointer + contentHash + keyAlg + mode`. El plaintext no queda en su base.
- **`em_check_submission` / `em_approve_submission`**: devolver el pointer.

## 4. Herramienta MCP nueva

`em_recover_evidence(task_id)` → baja el ciphertext, lo descifra con la wallet del
publisher, verifica `contentHash`, devuelve el paquete.

Reutiliza la auth ERC-8128 que ya tienen. No hace falta un endpoint de recovery
en el facilitador: en modo `direct` el publisher ya tiene la única clave que abre
el payload, así que recuperar es aritmética, no una decisión de permisos.

## 5. Contrato con el facilitador

`POST https://facilitator.ultravioletadao.xyz/dx402/anchor` — **solo metadata**,
nunca el plaintext ni la clave:

```json
{
  "paymentId": "0x...",
  "network": "base",
  "txHash": "0x...",
  "payer": "0x...",          // el publisher: quien paga y quien podrá leer
  "payee": "0x...",          // el worker
  "pointer": "s3+https://.../abc.dx402",
  "backend": "s3",
  "contentHash": "0x...",    // keccak256 del PLAINTEXT
  "keyAlg": "ECIES-secp256k1",
  "mode": "direct",
  "retention": "90d"
}
```

Responde `201` con un `EvidenceReceipt` firmado EIP-712 (dominio
`{name: "DX402 Evidence", version: "1", chainId}`) que cualquiera puede verificar
sin llamarnos.

Lectura: `GET /dx402/evidence/{paymentId}` y `GET /dx402/receipt/{paymentId}`.

## 6. Reglas que NO se pueden romper

- **DX402 nunca puede hacer fallar un pago ni un submit.** Todo fallo degrada a
  un skip (`too_large`, `anchor_failed`, `no_payer_key`, `disabled`).
- **`contentHash` va sobre el PLAINTEXT.** Es el chequeo que detecta a un worker
  que ancló algo distinto de lo que entregó — que en su modelo de negocio es
  exactamente el fraude que importa.
- **`paymentId` es el AAD del cifrado.** Derivarlo distinto en los dos lados hace
  fallar el descifrado sin causa aparente:
  `keccak256(caip2_network || tx_hash_sin_0x)`.
- **404 y 410 significan cosas distintas.** `dx402_unknown_payment` es "nunca
  existió"; `dx402_evidence_expired` es "venció la retención". En una disputa no
  son la misma respuesta. Y **`dx402_store_unavailable` es retryable** — no lo
  persistan como "no hay evidencia" (así fue INC-2026-07-21).
- **Anclar es publicar.** `retention: permanent` es irrevocable. El default de 90
  días existe porque un worker que suba PII por error no tiene vuelta atrás.
- **Escrow ≠ DX402.** El escrow x402r que ya usan sigue igual; esto es una capa
  aparte que no toca la liquidación.

## 7. Un beneficio lateral que les sirve

El paper *Five Attacks on x402 Agentic Payment Protocol* midió **100% de fuga por
caché** de respuestas pagadas a través de nginx. Si la evidencia viaja y se
persiste cifrada hacia el publisher, un proxy que la cachee guarda ciphertext
inútil.

## 8. Referencias

- Spec normativa: `docs/plans/dx402/02-SPEC-v0.1.md` en x402-rs
- Guía de integración: `docs/DX402.md`
- Research: `docs/plans/dx402/00-RESEARCH.md`
