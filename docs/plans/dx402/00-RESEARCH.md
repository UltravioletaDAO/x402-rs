# DX402 — Research & Findings

**Fecha:** 2026-08-14
**Origen:** transcript de Grok en `.unused/dx402` (brainstorm de Saul)
**Estado:** research cerrado. Ver `01-MASTER-PLAN.md` para la ejecución.

Este documento registra lo que se **verificó**, lo que se **refutó**, y lo que
quedó **abierto**. Todo lo que aquí se afirma como hecho fue comprobado contra el
código o contra una fuente citada; lo que es criterio propio está marcado como tal.

---

## 1. El problema, enunciado con precisión

x402 entrega el recurso pagado **una sola vez**, en el body de un 200 OK, y no
guarda nada. La liquidación es durable (on-chain); **la entrega no lo es**.

Consecuencias medibles:

- Si el cliente/agente no persiste la respuesta en ese instante, se pierde.
- No hay forma posterior de probar *qué* se entregó — solo *que se pagó*.
- Una disputa ("pagué y me llegó basura", "nunca me llegó") no tiene árbitro:
  no existe artefacto que ambas partes hayan aceptado.

Esto no es una opinión nuestra. El paper académico
[*Five Attacks on x402 Agentic Payment Protocol*](https://arxiv.org/html/2605.11781v1)
lo enuncia como asimetría estructural del protocolo:

> "the HTTP response has already been sent, so RR cannot claw the resource back"

La entrega es irreversible; la liquidación es reversible (reorgs). El paper
**no propone solución** — explícitamente no cubre no-repudio ni disputas. Es
exactamente el hueco donde entra DX402.

---

## 2. Estado del arte verificado (agosto 2026)

### 2.1 Lo que SÍ existe

| Pieza | Qué cubre | Qué NO cubre |
|---|---|---|
| [Signed Offers & Receipts](https://docs.x402.org/extensions) (`offer-receipt`, extensión oficial) | Firma el *offer* en el 402 y el *receipt* en el 200. Artefacto portátil, verificable por terceros. | El **contenido**. El receipt dice "se entregó algo", no *qué*. No persiste el body. |
| [IETF drafts de Vauban](https://datatracker.ietf.org/doc/draft-vauban-x402-stark-receipts/00/) (STARK receipts, VPSF claim algebra, retention chains) | Recibos criptográficos offline-verificables, post-cuánticos, anclados en Starknet. | Idem: metadata de pago. Cero durabilidad del payload entregado. |
| [x402.storage](https://www.x402.storage/) | Storage permanente IPFS, $0.01/archivo, pagado por x402. Agent-friendly. | **Público**. Opt-in, client-side, desacoplado del pago. El agente tiene que acordarse de subirlo. |
| [Lighthouse](https://docs.lighthouse.storage/tutorials/x402-pay-per-use-file-upload) / [Pinata](https://pinata.cloud/blog/pay-to-pin-on-ipfs-with-x402/) | x402 como riel de cobro para pinning IPFS. | Son *vendedores de storage que aceptan x402*, no una capa de evidencia del protocolo. Público. |
| ERC-8004 (ya lo corremos) | Identidad + reputación del agente. | Reputación agregada, no evidencia por transacción. |

### 2.2 Lo que NO existe — el hueco

Búsquedas dirigidas (`x402 encrypted response body payer only decrypt`,
`wallet-gated retrieval extension`) devuelven **cero** resultados de producto o
spec. El registro oficial de extensiones tiene 7 entradas
(`bazaar`, `builder-code`, `eip2612-gas-sponsoring`,
`erc20-approval-gas-sponsoring`, `payment-identifier`, `sign-in-with-x`,
`offer-receipt`) y **ninguna** persiste el contenido entregado.

> **Conclusión:** el gap es real y está abierto. Todo el ecosistema resolvió
> *"prueba de que pagaste"*. Nadie resolvió *"prueba de qué te entregaron, y que
> solo vos podés leerla"*.

### 2.3 Corrección a Grok

Grok afirmó que existen los hooks `onAfterSettle` y `enrichSettlementResponse`
**en nuestro facilitador** y que ahí se captura el body.

**Es falso, y el error es arquitectónico, no de nombres.**

```
Cliente ──(1) GET ─────────────► Resource Server (seller)
       ◄─(2) 402 + requirements ─┘
       ──(3) GET + X-PAYMENT ───► Resource Server
                                    │ (4) POST /verify  ──► Facilitador
                                    │ (5) POST /settle  ──► Facilitador ──► chain
       ◄─(6) 200 + BODY ───────────┘
```

El **facilitador nunca ve el body**. Solo participa en (4) y (5). El body existe
únicamente en el paso (6), dentro del resource server.

`grep -rn "onAfterSettle\|enrich_settlement" src/ crates/` → 0 resultados
reales. Esos hooks son del SDK TypeScript de la Foundation, no de x402-rs.

**Dónde sí está el body, verificado en código:**
`crates/x402-axum/src/layer.rs:947-967`, rama `settle_after_execution` (default):

```rust
let response = match Self::call_inner(inner, req).await { ... };   // ← el BODY está acá
if response.status().is_client_error() || ... { return ...; }
let settlement = match self.settle_payment(&verify_request).await { ... };
let header_value = ...;
res.headers_mut().insert("X-Payment-Response", header_value);
```

Ese punto —después del handler, después del settle, antes de devolver— es el
único lugar del stack donde coexisten **el body entregado** y **la prueba de
liquidación**. Es el punto de inyección de DX402.

**Reparto de responsabilidades que se deriva de esto:**

- `x402-axum` (seller middleware) → captura, cifra, ancla. *Es el post-hook.*
- Facilitador (x402-rs) → registro/notario de evidencia, `/supported`, recovery.
  *No ve plaintext nunca.*
- `x402-reqwest` (buyer) → descifra automáticamente.

---

## 3. El núcleo criptográfico: **el pago ya es un intercambio de claves**

Esta es la idea que hace a DX402 distinto de "súbelo a IPFS y ya".

El requisito de Saul: *"que solo el que pagó pueda acceder, vía firma
criptográfica de la wallet, automáticamente después de la compra"*.

La tentación es el flujo obvio: cifrar con una clave que guarda el servidor y
soltarla contra un challenge firmado. Eso funciona pero **el servidor puede
leerlo todo** — es "confiá en Ultravioleta", no es evidencia.

La observación clave:

> **La autorización de pago ya viene firmada por el comprador. De una firma ECDSA
> se recupera la clave pública, no solo la dirección. Entonces el vendedor puede
> cifrar hacia el comprador sin pedirle nada, sin registro previo, sin round-trip
> extra — usando material criptográfico que el propio acto de pagar produjo.**

Pagar *es* publicar tu clave de cifrado. Cero fricción, cero custodia.

### 3.1 Disponibilidad de la clave pública del pagador, por familia

Verificado contra cómo firma cada familia (7 familias en `src/network.rs:275`):

| Familia | Curva | ¿Pubkey del pagador disponible? | Cómo |
|---|---|---|---|
| EVM | secp256k1 | **Sí** | `ecrecover` sobre la firma EIP-3009/EIP-712 devuelve la pubkey completa, no solo la address |
| Solana / Fogo | ed25519 | **Sí, gratis** | La address **es** la pubkey. Convertir ed25519→X25519 (mapa birracional) para ECDH |
| NEAR | ed25519 | **Sí** | `ed25519:...` en el account key |
| Stellar | ed25519 | **Sí, gratis** | La address `G...` es la pubkey codificada |
| Algorand | ed25519 | **Sí, gratis** | La address es pubkey + checksum |
| Sui | ed25519/secp256k1 | **Sí** | La address es un *hash* de la pubkey → no sirve; pero la firma Sui **incluye** la pubkey |
| XRPL | secp256k1/ed25519 | **Sí** | `SigningPubKey` va en la tx firmada |

**7 de 7.** En 4 familias sale gratis de la dirección; en 3 sale de la firma que
ya tenemos en el payload. En ningún caso hace falta pedirle nada al comprador.

Esto no es casualidad: toda blockchain de firma digital expone material de clave
pública. DX402 lo reutiliza como canal de cifrado.

### 3.2 Los tres modos (deben coexistir; no es un solo diseño)

**Modo A — `direct` (E2E, default, sin parte confiable).**
Sobre-cifrado ECIES hacia la pubkey recuperada del pagador:
CEK aleatoria → AES-256-GCM sobre el body → CEK envuelta con ECDH(ephemeral,
payer_pubkey) → HKDF → AES-KW. Nadie más que el dueño de la private key abre
nada. Ni el facilitador, ni el storage, ni nosotros.
*Requisito:* el comprador controla su clave privada en crudo.
*Encaja perfecto con:* agentes (Karmakadabra, execution.market workers), que es
exactamente nuestro mercado.

**Modo B — `derived` (wallets de browser).**
MetaMask retiró `eth_decrypt`, así que Modo A no corre en browser. Alternativa:
el comprador firma **un mensaje EIP-712 fijo y determinista** ("DX402 Key
Derivation v1") → HKDF sobre la firma → keypair X25519 estable → se publica la
pubkey en un registro. ECDSA en Ethereum usa nonce determinista (RFC 6979), así
que la misma firma sale siempre igual y la clave es reproducible.
*Costo:* requiere registro previo. *Riesgo:* una wallet que no cumpla RFC 6979
rompe la derivación — hay que validar contra un vector conocido antes de confiar.

**Modo C — `escrowed` (fallback explícito, marcado como tal).**
El facilitador guarda la CEK envuelta y la libera contra firma SIWX/EIP-712
ligada al `paymentId`. Es el modo cómodo y el único que funciona si el comprador
pierde la clave. **Debe declararse en el recibo** para que nadie confunda su
garantía con la del Modo A.

> Criterio propio: A es el default y es la contribución novedosa. C existe porque
> negarlo empujaría a la gente a soluciones peores. B es trabajo posterior.

---

## 4. Crítica adversarial (abogado del diablo)

Riesgos que hay que responder en el diseño, no en el marketing:

1. **"Ciphertext hoy, plaintext en 2040."** Lo que se ancla es permanente; ECDH
   secp256k1 no lo es. Un blob cifrado hoy y roto en 15 años sigue publicado.
   → Mitigación: `retention` explícito por ruta, y no anclar públicamente lo que
   no debe sobrevivir. Ver §5.
2. **Anclar = publicar para siempre.** Si un seller mete PII por error, no hay
   botón de borrar en Arweave. → Default a un backend con TTL; permanente es
   opt-in consciente.
3. **La CEK es el punto único de falla en Modo C.** Si el endpoint de recovery se
   compromete, se abre todo el histórico. → Modo A no tiene ese endpoint. Modo C
   debe rotar y auditar.
4. **Replay del challenge de recovery.** Nonce + timestamp + binding al
   `paymentId` y a la red. Sin eso, una firma vieja capturada abre el blob.
5. **Costo a escala.** $0.01/archivo se ve barato hasta 10k tx/día = $100/día.
   → El fee de persistencia debe poder cobrarse al comprador (otro micropago) o
   limitarse a high-value. Decisión de producto por ruta, no global.
6. **Publicar la extensión sin uso real es postureo.** La Foundation exige PR
   revisado; los compradores van a preguntar por el live proof. → No se propone
   upstream hasta tener N transacciones reales en producción con link durable
   verificable.
7. **No inventar storage.** Ya tenemos settlements multi-chain, escrow con
   evidencia, ERC-8004 y un índice DynamoDB. La capa nueva es *cifrado +
   control de acceso + anclaje*, no un storage nuevo.

---

## 4-bis. Hallazgo propio durante la implementación (2026-08-14)

**`ed25519-dalek` acepta puntos de orden pequeño en `VerifyingKey::from_bytes`.**

Salió de un test que escribí esperando que `from_bytes([0xff; 32])` fallara. No
falló. `VerifyingKey::from_bytes` valida que el punto descomprima, **no** que sea
canónico ni de orden grande.

Para firmas da igual. Para **ECDH no**: un punto de orden pequeño colapsa el
secreto compartido a una constante. Como la pubkey del pagador es un dato que
entra desde afuera (la registra el seller al anclar), alguien que pudiera
influirla forzaría la `wrapKey` a un valor reproducible y abriría la evidencia.

Mitigación implementada: rechazo del resultado ECDH todo-en-cero en tiempo
constante (RFC 7748 §6.1), en `seal` y en `open`
(`EnvelopeError::DegenerateSharedSecret`).

Trampa en el propio test: la primera versión usaba un valor inventado que
*parecía* de orden pequeño (`e0 00…00 57`) y el test falló porque ese punto es
perfectamente válido y de orden grande. Ahora usa las **7 coordenadas canónicas
que libsodium pone en su blacklist**. Un vector inventado habría dejado pasar el
test sin probar nada — el mismo error que ya nos costó meses con SEAL v1 en
ERC-8004 Solana.

Estado: 34/34 tests de `dx402::` en verde, incluida la rejección de las 7.

---

## 5. Refuerzo inesperado: DX402 mitiga un ataque publicado

Attack III del paper (*HTTP/Proxy-level confusion*) midió **100% de fuga por
caché con nginx**: un intermediario cachea la respuesta pagada y la sirve a
clientes que no pagaron.

Si el body viaja y se persiste **cifrado hacia el pagador**, un proxy que lo
cachee guarda ciphertext inútil. DX402 no fue diseñado para eso, pero lo mitiga
de forma natural. Es un argumento fuerte para la propuesta upstream.

---

## 6. Nombre y posicionamiento

`DX402` = **D**urable **X402**. Producto/marca.
Clave de extensión en el protocolo: **`durable-evidence`** (kebab-case, alineado
con `offer-receipt`, `payment-identifier` — convención verificada en el registro).

Posicionamiento en una línea:

> `offer-receipt` prueba que hubo un trato. **DX402 prueba qué se entregó — y se
> lo entrega solo a quien pagó.**

---

## 7. Preguntas abiertas (van al master plan como decisiones)

1. Backend de anclaje por defecto: S3-nuestro / IPFS pinned / Arweave.
2. ¿Se cobra la persistencia al comprador, o la absorbe el seller?
3. Alcance de esta entrega: ¿solo x402-rs + crates + frontend + docs, o también
   los repos de SDK y execution-market?

---

## Fuentes

- [Five Attacks on x402 Agentic Payment Protocol (arXiv)](https://arxiv.org/html/2605.11781v1)
- [x402 Extensions — registro oficial](https://docs.x402.org/extensions)
- [draft-vauban-x402-stark-receipts-00](https://datatracker.ietf.org/doc/draft-vauban-x402-stark-receipts/00/)
- [x402 Cryptographic Receipts (IETF, consolidado)](https://www.ietf.org/archive/id/draft-vauban-x402-consolidated-00.html)
- [VPSF Claim Algebra for x402 Payment Receipts](https://datatracker.ietf.org/doc/html/draft-vauban-x402-vpsf-algebra-01)
- [x402.storage](https://www.x402.storage/)
- [Lighthouse — x402 pay-per-use upload](https://docs.lighthouse.storage/tutorials/x402-pay-per-use-file-upload)
- [Pinata — Pay to Pin on IPFS with x402](https://pinata.cloud/blog/pay-to-pin-on-ipfs-with-x402/)
- Código propio: `crates/x402-axum/src/layer.rs:947-967`, `src/types.rs:1528-1552`, `src/network.rs:275`
