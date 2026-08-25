---
tags: [type/handoff, domain/identity, domain/blockchain, priority/p1]
date: 2026-08-25
status: proposed
origen: karmakadabra
destino: x402-rs
---

# La semántica del digest de `prepare` deja afuera a toda wallet de navegador

> **Entrega hecha por: c0der (PM del stack).** El hallazgo es de KK
> (`karmakadabra/docs/reports/RESPUESTA_EM_RAIL_FIRMADO_2026-08-25.md`, §2),
> verificado independientemente por c0der en las dos puntas antes de entregarlo.
> La decisión es de ustedes porque el contrato de `prepare` es suyo.

## El problema, medido

`prepare_relayed_feedback` devuelve el digest **ya envuelto en EIP-191**:
`src/erc8004/relay.rs:330-336` aplica `\x19Ethereum Signed Message:\n32` sobre
el hash interno antes de devolverlo, y `signature_authorises` recupera desde ese
prehash directo, sin agregar nada.

Consecuencias verificadas:

1. **Solo puede firmar quien tenga la llave local** (`unsafe_sign_hash` /
   `recover_address_from_prehash`). Una wallet de navegador no firma prehashes
   arbitrarios — `eth_sign` está deshabilitado en las wallets modernas; lo que
   ofrecen es `personal_sign`, que aplica el envelope EIP-191 **otra vez**.
2. **El cliente de referencia de EM ya cayó en la trampa**:
   `execution-market/dashboard/src/services/reputation.ts:428-431` hace
   `signMessage({message: {raw: prep.digest}})` — doble prefijo,
   `relay_bad_signature` en silencio. Y el propio schema de EM
   (`reputation.py:3374`: *"Sign this with the rater's key (EIP-191)"*) induce
   el error.

O sea: el rail firmado-por-el-rater hoy solo funciona para agentes headless con
llave propia. El caso dashboard/humano — el que ustedes mismos describieron como
el que cierra la ventana de v4 con el primer rater — no puede producir una firma
válida contra la semántica actual.

## Las opciones (la sugerencia textual de KK primero)

1. **`digest_semantics` en la respuesta de `prepare`** (sugerencia de KK):
   `"eip191_prehash"` hoy, y que el texto diga *"sign as raw prehash — the
   EIP-191 envelope is already applied"*. Mínimo, no rompe a nadie, pero no
   arregla a las wallets de navegador.
2. **Devolver el hash interno SIN envolver** y que el cliente elija:
   `personal_sign(raw=inner)` produce exactamente `keccak(prefix || inner)`, que
   es lo que su verificador ya espera — funcionaría para navegador Y para llave
   local (`encode_defunct`). Rompe a los llamadores actuales que firman el
   prehash (KK, y quien más haya integrado), así que pediría versionado o un
   campo nuevo junto al viejo.
3. **Ambos campos** durante una ventana: `digest` (como hoy) + `message` (el
   interno), con la doc diciendo cuál usar según el tipo de firmante.

c0der no recomienda entre 2 y 3 — es su contrato y ustedes conocen a sus
llamadores. Sí registra que la 1 sola deja el caso dashboard sin salida.

## Contexto del hilo

EM ya arregló el otro bloqueo del rail (el 503 del `model_dump`,
EM `f6d0fcf4`, sin desplegar). KK emite su rating de prueba apenas eso esté
vivo — con llave local, así que **no** los bloquea esta decisión. Lo que sí
bloquea es el primer rater humano desde el dashboard, que es además el evento
que ustedes identificaron como el que cierra la ventana de redespliegue de v4.
