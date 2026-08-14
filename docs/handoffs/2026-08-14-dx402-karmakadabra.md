# Handoff — DX402 en KarmaCadabra

**Para:** equipo de karmakadabra.ultravioletadao.xyz
**De:** facilitador (x402-rs), 2026-08-14
**Estado del riel:** implementado y testeado en el facilitador. Falta que ustedes lo usen.

Este documento es autocontenido. No hace falta leer nada más para implementar.

---

## 1. Qué problema resuelve para ustedes

Hoy, cuando un agente comprador paga por data y el agente vendedor la entrega, **esa
data existe una sola vez**: en el body del 200. Si el comprador no la persiste en
ese instante, se perdió. Y si después hay un reclamo ("pagué y me llegó basura"),
no hay artefacto que las dos partes hayan aceptado — solo el hash de la
transacción, que prueba que se pagó, no *qué* se entregó.

DX402 hace que esa respuesta:

1. **sobreviva** a la sesión,
2. sea **privada** — cifrada hacia la wallet que pagó, ni nosotros la leemos,
3. quede **acoplada al pago** sin registro previo ni round-trip extra.

## 2. La idea en una línea

La autorización de pago **ya es una firma**, y de una firma sale la **clave
pública** del firmante — no solo la dirección. Así que el vendedor puede cifrar
hacia el comprador sin pedirle nada.

En Solana (que es donde corre buena parte de lo suyo) es todavía más directo: la
address **es** la clave pública ed25519. Cero criptografía extra.

## 3. Dónde va exactamente en su flujo

```
buyer agent ──paga x402──► seller agent
                             │  produce el DATO   ◄── único lugar donde existe
                             │  [DX402] cifra hacia la wallet del buyer
                             │  [DX402] sube el ciphertext
                             │  [DX402] POST /dx402/anchor al facilitador
buyer agent ◄─ dato + X-Durable-Evidence ─┘
```

El punto de inyección es **el delivery path del seller agent**, justo después de
que el pago liquidó y antes de devolverle el payload al buyer. No es el
facilitador: el facilitador nunca ve el body.

## 4. Qué implementar

### 4.1 Lado seller (el que entrega la data)

Después de que el settle sale bien:

1. `content_hash = keccak256(payload_en_claro)`
2. Recuperar la pubkey del comprador:
   - **Solana**: `base58_decode(buyer_address)` → 32 bytes ed25519. Listo.
   - **EVM**: recuperar de la firma EIP-3009 (el SDK lo hace por ustedes).
3. Cifrar: CEK aleatoria de 32 bytes → AES-256-GCM sobre el payload, con
   `aad = paymentId`. Envolver la CEK con ECIES hacia la pubkey del comprador.
4. Subir el ciphertext a donde quieran (S3, IPFS) → obtienen un `pointer`.
5. `POST https://facilitator.ultravioletadao.xyz/dx402/anchor` con **solo
   metadata** (nunca el plaintext, nunca la CEK):

```json
{
  "paymentId": "0x...",
  "network": "solana",
  "txHash": "...",
  "payer": "F742C4Vf...",
  "payee": "...",
  "pointer": "s3+https://.../abc.dx402",
  "backend": "s3",
  "contentHash": "0x...",
  "keyAlg": "ECIES-X25519",
  "mode": "direct",
  "retention": "90d"
}
```

6. Devolver el header `X-Durable-Evidence` (base64url del JSON de respuesta) junto
   con el payload.

### 4.2 Lado buyer (el agente que compra)

1. Leer `X-Durable-Evidence` de la respuesta.
2. Guardar el `pointer` + `paymentId` (eso es todo lo que hace falta después).
3. Para recuperar, en cualquier momento: bajar el ciphertext del pointer,
   descifrar con **su propia private key**, y **verificar `contentHash`** contra
   lo que descifró.

Ese último chequeo no es opcional: es el que detecta a un vendedor que ancló
algo distinto de lo que entregó.

### 4.3 Herramienta MCP nueva (si exponen MCP)

`kk_recover_evidence(payment_id)` → devuelve el dato descifrado usando la wallet
del agente. No necesita permiso de nadie: en modo `direct` recuperar es
aritmética, no una decisión de permisos.

## 5. Los SDKs ya lo traen

Si usan `uvd-x402-sdk` (python o typescript), los helpers están publicados:
`anchor_evidence()` / `recover_evidence()`. No reimplementen la criptografía.

## 6. Reglas que NO se pueden romper

- **DX402 nunca puede hacer fallar un pago.** Si el cifrado o el anclaje falla,
  se entrega la respuesta igual y se manda un skip en el header
  (`too_large` / `anchor_failed` / `no_payer_key` / `disabled`). El facilitador
  está construido con esa regla; el lado de ustedes también tiene que estarlo.
- **`contentHash` va sobre el PLAINTEXT**, no sobre el ciphertext. Sobre el
  ciphertext solo probaría que el blob no se corrompió; sobre el plaintext prueba
  que el blob descifra exactamente a lo que se entregó.
- **`paymentId` es el AAD.** Si el buyer y el seller lo derivan distinto, el
  descifrado falla sin causa aparente. Fórmula:
  `keccak256(caip2_network || tx_hash_sin_0x)`.
- **Anclar es publicar.** `retention: permanent` es irrevocable. El default es
  90 días a propósito. No anclen permanentemente data sensible de un cliente.
- **Wallets de contrato inteligente no tienen clave recuperable.** Eso es un skip
  normal (`no_payer_key`), no un error.

## 7. Por qué esto les conviene primero a ustedes

Necesitamos **live proof** antes de proponer la extensión a la x402 Foundation.
La Foundation exige un PR revisado, y una propuesta sin uso real en producción se
descarta. KarmaCadabra tiene el flujo buyer/seller ya armado, así que es el
candidato natural para las primeras N transacciones reales con link durable
verificable.

## 8. Referencias

- Spec normativa: `docs/plans/dx402/02-SPEC-v0.1.md` en x402-rs
- Guía de integración: `docs/DX402.md`
- Research y estado del arte: `docs/plans/dx402/00-RESEARCH.md`
