# DX402 — Master Plan

**Fecha:** 2026-08-14
**Research previo:** `00-RESEARCH.md` (leer primero)
**Definición de "listo":** la extensión `durable-evidence` corriendo en
`facilitator.ultravioletadao.xyz`, con un comprador real recuperando un body
cifrado que solo él puede abrir, verificable desde afuera, documentado y
anunciado. Los repos de terceros (Karmakadabra, execution.market) reciben
handoffs, no código nuestro.

---

## 0. Qué estamos construyendo (una pantalla)

```
                    ┌──────────────── SELLER (x402-axum) ─────────────────┐
Cliente ─GET+PAY──► │ handler → BODY                                       │
                    │   ├─ 1. CEK aleatoria → AES-256-GCM(body)            │
                    │   ├─ 2. recover payer_pubkey (de la firma del pago)  │
                    │   ├─ 3. ECIES(CEK → payer_pubkey)                    │
                    │   └─ 4. anclar ciphertext → EvidenceStore → pointer  │
                    └──────────────────────┬───────────────────────────────┘
                                           │ POST /dx402/anchor (metadata, sin plaintext)
                                           ▼
                    ┌────────── FACILITADOR (x402-rs) ─────────────────────┐
                    │  notario: firma EvidenceReceipt (EIP-712)            │
                    │  índice: paymentId → pointer + contentHash           │
                    │  NUNCA ve plaintext ni CEK (modo direct)             │
                    └──────────────────────┬───────────────────────────────┘
                                           │
Cliente ◄─ 200 + BODY + X-Payment-Response + X-Durable-Evidence ────────────┘
                     (pointer + contentHash + receipt)

...meses después...
Comprador ─► fetch(pointer) → ciphertext → descifra local con SU private key ✓
```

Tres propiedades, en orden de importancia:

1. **Durable** — la respuesta sobrevive a la sesión.
2. **Privada** — cifrada hacia el pagador; nadie más (nosotros incluidos) la lee.
3. **Acoplada** — sin registro, sin round-trip extra. El acto de pagar produce la
   clave de cifrado (§3 del research).

---

## 1. Decisiones tomadas

| # | Decisión | Razón |
|---|---|---|
| D1 | Clave de extensión: **`durable-evidence`**; marca: **DX402** | Convención del registro oficial (`offer-receipt`, `payment-identifier`) |
| D2 | Modo **`direct`** (E2E/ECIES) por defecto | Es la contribución novedosa y la única sin parte confiable |
| D3 | Modo **`escrowed`** existe, pero **declarado en el recibo** | Sin él la gente usa algo peor; con él, nadie confunde garantías |
| D4 | El **seller** cifra y ancla; el **facilitador** notariza e indexa | El facilitador no está en el camino del body (§2.3 research) |
| D5 | Backend de storage **pluggable** vía trait `EvidenceStore`, **S3 nuestro por defecto** (TTL 90d); IPFS y Arweave opcionales | Cero dependencias externas para arrancar y cero costo por archivo. El blob va cifrado igual: la privacidad no depende del backend, así que migrar a Arweave después no toca el protocolo |
| D6 | Todo detrás de `ENABLE_DX402` (default OFF) | Ninguna ruta de pago existente puede degradarse por esto |
| D7 | Propuesta upstream a la Foundation **solo tras uso real en prod** | La Foundation exige PR revisado; sin live proof te descartan |
| D8 | Alcance: x402-rs + crates + frontend + docs + handoffs **+ SDKs py/ts** | Decidido por Saul 2026-08-14. Los SDKs son repos compartidos con KarmaCadabra y Execution Market — revisar `git log` antes de tocar |
| D9 | La entrega termina en **commit local**; el push lo hace Saul | En este repo `push a main` == deploy a producción. El release es decisión suya |

**No negociable:** DX402 nunca puede hacer fallar un pago. Si el anclaje falla, la
respuesta se entrega igual y el evento se marca `anchor_failed`. Mismo principio
que el transaction store (fire-and-forget, la cadena es el ledger).

---

## 2. Fases

Cada fase es shippable por separado. El orden importa: F1→F2→F3 son bloqueantes
entre sí; F5–F7 pueden ir en paralelo una vez F2 está en verde.

### F1 — Spec `durable-evidence` v0.1
**Entrega:** `docs/plans/dx402/02-SPEC-v0.1.md`
- Forma de declaración por ruta (`extensions: { "durable-evidence": {...} }`)
- `X-Durable-Evidence` header + campo en `SettleResponse.extensions`
- Formato del `DurablePointer` (URI + backend + contentHash + alg)
- `EvidenceReceipt` EIP-712 (dominio, tipos, campos)
- Challenge de recuperación EIP-712/SIWX con anti-replay
- Modos `direct` | `escrowed`, y su declaración obligatoria
- Códigos de error
- **Vectores de prueba** (fijos, para que cualquiera pueda implementar y validar)

### F2 — Núcleo del facilitador (`src/dx402/`)
**Archivos nuevos:**
| Archivo | Responsabilidad |
|---|---|
| `mod.rs` | config desde env, feature gate, wiring |
| `types.rs` | `DurableEvidence`, `EvidenceReceipt`, `DurablePointer`, `RecoveryChallenge` |
| `pubkey.rs` | **el núcleo**: recuperar pubkey del pagador por familia (7 familias) |
| `envelope.rs` | AES-256-GCM + ECIES (secp256k1 y X25519) |
| `store.rs` | trait `EvidenceStore` + backends |
| `receipt.rs` | firma EIP-712 del recibo por el facilitador |
| `registry.rs` | índice `paymentId → pointer` (DynamoDB, reusa el patrón de `transaction_store`) |

**Endpoints nuevos** (en `handlers.rs`, registrados en el router):
- `POST /dx402/anchor` — el seller registra evidencia (metadata; nunca plaintext)
- `GET  /dx402/evidence/:paymentId` — pointer + contentHash + receipt
- `GET  /dx402/receipt/:paymentId` — recibo firmado, verificable offline
- `POST /dx402/recover` — solo modo `escrowed`: libera CEK contra firma del pagador

**Modificado:** `SettleResponse.extensions` poblado en el settle path;
`/supported` anuncia `durable-evidence`; `src/openapi.rs`.

**Env vars:** `ENABLE_DX402`, `DX402_STORE_BACKEND`, `DX402_STORE_BUCKET`,
`DX402_RETENTION_DAYS`, `DX402_SIGNING_KEY` (→ Secrets Manager en prod).

### F3 — Post-hook del vendedor (`crates/x402-axum`)
**Archivo nuevo:** `crates/x402-axum/src/durable.rs`
- Buffer del body en la rama `settle_after_execution` (`layer.rs:947`)
- `.with_durable_evidence(DurableConfig)` en `X402Middleware`
- Inyección de `X-Durable-Evidence`
- Límite de tamaño configurable; por encima → `skipped_too_large`, nunca error

### F4 — Lado comprador (`crates/x402-reqwest`)
- `recover_evidence(pointer, signer)` → plaintext
- Verificación automática de `contentHash` contra el body recibido (detecta
  seller que ancla algo distinto de lo que entregó — el caso de fraude obvio)

### F5 — Frontend (`static/index.html`)
- Sección DX402 con el diagrama de 3 propiedades
- Contador en vivo (`data-live-count`) de evidencias ancladas
- i18n **EN + ES** (ambos bloques del `<script>` final — obligatorio)
- Enlace a `/docs` y al spec

### F6 — Documentación
- `docs/DX402.md` — guía de integración (seller + buyer)
- `src/openapi.rs` — 4 endpoints nuevos + mención en prosa
- `README.md`, `docs/CHANGELOG.md`, `CLAUDE.md` (subsistema nuevo)
- **Handoffs por proyecto** (`docs/handoffs/2026-08-14-dx402-<proyecto>.md`):
  Karmakadabra, execution.market, MeshRelay, describe.net, SDKs py/ts.
  Autocontenidos: flujo actual → punto de inyección → qué implementar → contrato.

### F7 — Tests y prueba de durabilidad
- Vectores de cifrado fijos por familia (no comparados contra nosotros mismos —
  el error de SEAL v1 en ERC-8004 Solana no se repite)
- Round-trip: encrypt → anchor → fetch → decrypt en las 7 familias
- Anti-replay del challenge
- Test de que un fallo de anclaje **no** rompe el pago
- E2E en producción: N transacciones reales con link durable verificable

### F8 — Publicación
- `VERSION` bump desde la versión **desplegada** (no la local)
- `just format-all` + `clippy-all` + `cargo clippy -p x402-compliance`
- Test gate de CI: `cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl -- --test-threads=1`
- Push a `main` = deploy a producción (CI arma y despliega)
- Verificar `/version`, `/supported`, y un recovery real contra prod
- `/ship-tweet` (EN+ES)
- **Después** y solo después: draft de PR a `x402-foundation/x402`

---

## 3. Riesgos y mitigaciones

| Riesgo | Mitigación |
|---|---|
| Anclar PII permanentemente sin vuelta atrás | Default a backend con TTL; permanente es opt-in explícito por ruta |
| ECDH secp256k1 roto en 15 años sobre un blob permanente | `retention` explícito; no anclar públicamente lo que no debe sobrevivir |
| Recovery endpoint como single point of failure | Modo `direct` no lo usa. En `escrowed`: nonce+timestamp+binding a paymentId+red |
| Costo de storage a escala | Fee de persistencia cobrable al comprador; límite de tamaño; por ruta, no global |
| Wallet sin RFC 6979 rompe modo `derived` | Modo `derived` queda fuera de v0.1 hasta validar contra vector conocido |
| DX402 degrada pagos que hoy funcionan | `ENABLE_DX402` default OFF + fire-and-forget + test explícito |

---

## 4. Fuera de alcance de esta entrega

- Modo `derived` (wallets de browser) — v0.2
- Código dentro de Karmakadabra / execution.market — les pasamos handoffs
- Contratos on-chain de anclaje — el recibo firmado alcanza para v0.1
- El PR a la Foundation — se redacta, no se envía, hasta tener live proof
- El `git push` (== deploy a producción) — lo hace Saul (D9)

### F9 — SDKs (repos aparte, ver D8)
- `uvd-x402-sdk-python` — helpers `anchor_evidence()` / `recover_evidence()`
- `uvd-x402-sdk-typescript` — idem
- Ambos repos son compartidos: `git log` y `git status` antes de tocar, y commit
  acotado a los archivos propios ([[feedback-shared-worktree-index]])

---

## 5. Secuencia de trabajo

```
F1 spec ──► F2 facilitador ──► F3 seller ──► F4 buyer ──┐
                    │                                    ├──► F8 publicar
                    └──► F5 frontend ──► F6 docs ────────┤
                                          F7 tests ──────┘
```
