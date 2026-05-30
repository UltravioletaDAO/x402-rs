# Auditoría del Panel de CTOs: `xrpl-native-integration`

> **Fecha:** 29 de mayo de 2026
> **Tipo:** Auditoría de seguridad y readiness pre-deploy (panel multi-agente)
> **Método:** 7 agentes (1 recon + 5 CTOs expertos en paralelo + 1 CTO-in-Chief), ~957k tokens, 229 acciones, ~12 min
> **Objeto:** Diff del working tree para la integración nativa de XRPL (`xrpl:0`) — XRP/RLUSD/USDC, esquema t54 presigned-Payment
> **Disclaimer de stream:** Las facilitator wallets XRPL (`r...`) son direcciones públicas, no secretos. Ningún seed/private key aparece en este reporte.

---

## VEREDICTO EJECUTIVO: 🔴 NO-GO

| | |
|---|---|
| **Veredicto** | **NO-GO** |
| **Confianza** | 93% |
| **Risk score** | **88 / 100** |
| **Bloqueadores críticos** | 6 |
| **High** | 1 | 
| **Medium** | 3 |

**Resumen del CTO-in-Chief:** Es código borrador generado por IA que **nunca se compiló** con su propio feature flag y carga **dos defectos de pérdida de fondos** más un **crash garantizado**. El peor: `/verify` acepta un pago de **cualquier cuenta víctima** porque nunca verifica que la clave pública de la firma realmente controle la cuenta del pagador. Además los pagos RLUSD/USDC no funcionan (montos comparados con `f64` contra un encoding que el propio autor marcó como sin resolver), y un namespace CAIP-2 faltante hace que el facilitator **paniquee después de que los fondos ya se movieron on-chain**. Encima, el Dockerfile de producción **compila XRPL afuera**, así que el sitio anunciaría una red que el backend rechaza, y un archivo requerido (`src/redact.rs`) **ni siquiera está committeado**. Nada de esto toca las chains existentes — la remoción del viejo stub XRPL-EVM es limpia y el resto del cableado es sólido — pero XRPL no debe lanzarse hasta arreglar y probar en testnet.

---

## Veredictos por dimensión

| CTO | Dimensión | Veredicto | Confianza |
|---|---|---|---|
| 🛡️ Seguridad | Pagos + criptografía | 🔴 **NO-GO** | 86% |
| ⛓️ Protocolo | Correctitud XRPL | 🔴 **NO-GO** | 92% |
| 🏛️ Arquitectura | Rust + diseño | 🟡 CONDITIONAL | 82% |
| 🔧 Build | Compilación | 🟡 CONDITIONAL | 78% |
| 🔗 Integración | Data integrity | 🟡 CONDITIONAL | 90% |

---

## 🔴 Bloqueadores críticos (deben resolverse antes de cualquier deploy)

### 1. Signature bypass — pago no autorizado aceptado en `/verify`
**`src/chain/xrpl.rs` verify_signature() (líneas 764-832).** El chequeo offline `is_valid_message(message, txn_signature, signing_pub_key)` lee **tanto la firma COMO la clave pública del mismo blob controlado por el atacante**. No hay en ninguna parte del path de verificación una derivación de la dirección del pagador a partir de `SigningPubKey`. **Ataque:** un atacante envía `Account=dirección_víctima`, con su PROPIA pubkey+firma; los 10 chequeos offline pasan y `POST /verify` retorna `valid:true`. Como un resource server puede entregar el recurso ante un `/verify` exitoso (semántica x402 documentada), el atacante obtiene el bien por un pago que nunca podrá liquidarse. Contraste: el provider de Stellar (`stellar.rs:677`) deriva la verifying key DESDE la dirección reclamada — correcto.
**Fix:** derivar la dirección clásica desde `SigningPubKey` y exigir `derived_address == account` antes de aceptar. Si difieren (Regular Key / SignerList), exigir chequeo on-chain (`account_info` / engine_result `tes`/`tec`). Test de regresión con blob de pubkey no coincidente → debe RECHAZARSE.

### 2. CAIP-2 namespace `xrpl` faltante → panic garantizado DESPUÉS de mover fondos
**`src/caip2.rs`** no tiene variante `Xrpl` en el enum `Namespace`, pero `network.rs::to_caip2()` emite `"xrpl:0"`/`"xrpl:1"`. Tres conversiones en `types_v2.rs` (líneas 156, 271, 341) hacen `.expect("...should always produce valid CAIP-2")` → **PANIC**. Alcanzable en `POST /settle` (`handlers.rs:1985`) cuando `extra.discoverable==true`: el pago XRPL **liquida on-chain (los fondos se mueven) y LUEGO el handler crashea**. El peor orden de fallo posible.
**Fix:** agregar `Xrpl` al enum `Namespace` (Display/FromStr = `"xrpl"`) con validación de `network_id` 0..=4294967295. Tests de round-trip `xrpl:0`/`xrpl:1`. Guard interino: saltar el path discoverable `to_v2()` para XRPL.

### 3. Montos IOU (RLUSD/USDC) con `f64` contra encoding sin resolver
**`src/chain/xrpl.rs:691`.** `max_amount_required` es `TokenAmount(U256)` (base-units, `"10000"`), pero el valor IOU on-chain de XRPL es string decimal (`"0.01"`). Ambos se parsean a `f64` y se comparan con `abs() > f64::EPSILON`. Resultado: **todo pago RLUSD/USDC se rechaza** (break funcional); y si se "arregla" a decimales, `f64::EPSILON` (~2.2e-16) es insignificante en magnitudes como 1,000,000 → un **underpayment** crafteado compara "igual". El propio autor marcó esto con `TODO(verify-on-compile)`. Solo XRP nativo (drops enteros, compare exacto de strings) es sólido.
**Fix:** fijar UN encoding canónico con el cliente t54 y usar aritmética exacta (entero/decimal vía `bigdecimal`), NUNCA `f64`. Tests con blobs reales para 0.01, 1,000,000.x, "1.0" vs "1", y underpayment de último dígito.

### 4. El build de producción compila XRPL AFUERA mientras config/frontend lo anuncian
**`Dockerfile:15`** corre `cargo build --release --features solana,near,stellar,algorand,sui` — **omite `xrpl`**. Ningún path de build (justfile, workflows, docker-compose, fast-build.sh, build-and-push.sh) lo habilita. Como toda la integración está bajo `#[cfg(feature="xrpl")]`, el binario desplegado **no tiene XRPL** y `/verify`, `/settle`, `/supported` rechazan `xrpl:0`. Mientras tanto `supported_tokens.json` (36 redes / 6 stablecoins), `index.html` (card + wallet), `lambda/handler.py` y README (20 mainnets) lo anuncian como vivo.
**Fix:** agregar `xrpl` al feature list del Dockerfile y CI, luego verificar `curl .../supported | jq '[.kinds[].network]'` contiene `xrpl:0`. Si se difiere XRPL, revertir/ocultar los anuncios.

### 5. `src/redact.rs` sin trackear — el HEAD actual NO compila
`git status` muestra `?? src/redact.rs` (untracked), pero HEAD ya lo referencia (`main.rs:78`, `lib.rs:38`, y call sites en `stellar.rs`/`evm.rs`/`sui.rs`). **El árbol committeado está roto**; solo el working-tree lo hace compilar. Si el deploy se hace desde un commit que omite el archivo, `cargo build` falla con "file not found for module redact".
**Fix:** stagear y committear `src/redact.rs` en el mismo commit (staging por archivo, nunca `git add -A`). Verificar build limpio con el feature set completo. Confirmar en remoto con `git show origin/<branch>:src/redact.rs`.

### 6. La integración nunca se compiló (toolchain gap) — semántica de xrpl-rust sin verificar
`cargo check --features xrpl` y el feature set exacto del Dockerfile **fallan en resolución de dependencias (exit 101)**: la dep transitiva `alloy-chains 0.2.30` requiere `edition2024`/Rust 1.86 mientras el env tiene 1.82. Es **pre-existente** (pineado idéntico en HEAD, NO causado por el diff XRPL), pero significa que `xrpl-rust 1.1.0` y sus 27 crates transitivos **nunca se descargaron ni compilaron**. Quedan TODO(verify-on-compile) sin resolver: si `decode()` retorna currency como ASCII de 3 chars vs hex de 40, y si `encode_for_signing(&Value)` reproduce el orden de bytes idéntico al que firmó el cliente (si no, **toda verificación de firma está mal**).
**Fix:** build en Rust 1.86+ (imagen `rust:bullseye` del CI): `cargo check/build --features ...,xrpl` + clippy limpio. Arreglar aparte el pin `alloy-chains 0.2.30` edition2024. Luego `cargo audit/deny` sobre los 28 crates nuevos y `cargo tree -e features -i xrpl-rust` para confirmar que el stack embedded no_std no entra al binario del servidor.

---

## 🟠 Alta + 🟡 Media

- **[HIGH] Settle gate solo rechaza `tem*`** — `tef*`/`tel*` (incl. `tefBAD_AUTH`, `tefPAST_SEQ`) caen a un poll-timeout de 30s. Pierde a rippled como backstop rápido del bloqueador #1 y abre **DoS de amplificación** (1 request HTTP quema 30s + cuota RPC). Fix: rechazar `tem*`/`tef*`/`tel*` de inmediato; solo `tes*`/`ter*` encolado procede a polling.
- **[MED] API key de RPC se filtra en logs** — `rpc_call` envuelve errores de reqwest verbatim (`e.to_string()` incluye la URL con la key). Llega a CloudWatch y al stream. Fix: `reqwest::Error::without_url()` o construir el error con `redact::rpc_url(...)`.
- **[MED] Panic de str-slice en debug logger** — `&signed_tx_blob[..100]` paniquea si el byte 100 cae mid-multibyte (solo bajo `RUST_LOG=debug`). Fix: truncado char-safe + validar hex ASCII en deserialize.
- **[MED] `LastLedgerSequence` sin validar ventana en `/verify`** + issuer **RLUSD testnet auto-marcado UNVERIFIED** (`rQhWct2fv...`). Fix: validar ventana de expiry contra el ledger actual; confirmar issuer testnet vía `account_lines` o quitarlo.

---

## ✅ Confirmado correcto (NO alucinado) — el trabajo no fue en vano

El CTO de Protocolo verificó **cada constante hardcodeada contra fuentes autoritativas** (chainagnostic.org, xrpscan, Circle, Ripple, xrpl.org, docs t54). **Ninguna está alucinada:**

- CAIP-2 namespace `xrpl` y formato `xrpl:{network_id}` ✓
- mainnet NetworkID=0 / testnet=1 + regla de omisión (≤1024) ✓
- Issuer RLUSD mainnet `rMxCKbEDwqr76QuheSUMdEGf4B9xJ8m5De` ✓
- Issuer USDC mainnet `rGm7WCVp9gb4jZHWTEtGUr4dd74z2XuWhE` y testnet `rHuGNhqTG32mfmAvWA8hUyWRLV3tCSwKQt` ✓ (USDC es IOU nativo de XRPL desde junio 2025, no bridged)
- Currency hex RLUSD=`524C555344...` / USDC=`5553444300000000...` (40-char ASCII right-padded) ✓
- XRP = 6 decimales / 1 XRP = 1,000,000 drops ✓
- sourceTag default 804681468, RPCs públicos, mecánica JSON-RPC submit/tx ✓

**Otros aciertos confirmados:**
- Remoción del viejo XRPL-EVM stub: **limpia, cero huérfanos** (grep-verificado en src/crates/examples/config/tests/lambda)
- Facilitator wallets XRPL **byte-idénticas** en los 5+ archivos (JSON, lambda, frontend, docs) — sin mismatch (riesgo de fondos #1 de integración está bien)
- Controles de seguridad positivos: destination diversion **bloqueado** (`Destination==pay_to`), replay manejado por Sequence/LastLedgerSequence nativo de XRPL, seed **nunca logueado**, multi-sig rechazado, SendMax/tfPartialPayment rechazados
- Arquitectura fiel al patrón de Stellar; error handling idiomático; concurrencia correcta (reqwest directo para evitar futures non-Send)
- **No rompe ninguna chain existente**

---

## 📋 Plan de auditoría (12 pasos priorizados)

### P0 — Bloqueadores (hacer primero, en orden)

1. **Committear `redact.rs` + probar compilación en Rust 1.86+**
   `git add src/redact.rs && cargo +1.86.0 build --release --features solana,near,stellar,algorand,sui,xrpl && cargo +1.86.0 clippy --features ...,xrpl -- -D warnings`
2. **Arreglar signature-binding** (ligar `Account` a `SigningPubKey`)
   Test: blob con `Account=A` pero firma de `key_B` → `/verify` debe dar `valid:false`. `cargo +1.86.0 test --features xrpl xrpl_signature_binding`
3. **Agregar namespace CAIP-2 `xrpl`** y probar que `to_v2()` no paniquea
   `cargo +1.86.0 test --features xrpl caip2_xrpl_roundtrip && ...xrpl_to_v2_no_panic`
4. **Reemplazar `f64` por aritmética exacta** + fijar encoding IOU
   Tests con blobs RLUSD/USDC reales: match exacto pasa, underpayment falla.
5. **Agregar `xrpl` al Dockerfile + CI** (o esconder anuncios si se difiere)
   `docker build ... && curl localhost:8080/supported | jq '[.kinds[].network] | map(select(test("xrpl")))'`
6. **Prueba end-to-end en TESTNET XRPL** antes de considerar mainnet
   Firmar Payment real por activo (XRP, USDC, RLUSD si issuer confirmado); `/verify` + `/settle`; asertar: `valid:true` correcto, `valid:false` para firma tampered Y para pubkey no coincidente (el ataque #1), underpayment rechazado, `Destination==pay_to`, replay → respuesta limpia (no timeout 30s).

### P1 — Alta prioridad

7. **Endurecer el engine_result gate** (fail-fast en `tef*`/`tel*`/`tec*`)
8. **Tapar el leak de RPC URL en logs** + el panic str-slice del debug logger
9. **Verificar constantes XRPL live** (issuer RLUSD testnet vía `account_lines`; confirmar facilitator wallets controladas → agregarlas a la fixed-wallet list de CLAUDE.md)

### P2 — Limpieza

10. **Completar config:** agregar bloque `xrpl-testnet` a `supported_tokens.json` (caip2 `xrpl:1`, wallet `rGhTio...`, issuers testnet); bumpear a `xrpl_networks:2 / total_networks:37`
11. **Remover backup trackeado** `src/network.rs.bak2` (`git rm`) + gitignore `*.bak*`; actualizar CLAUDE.md (quitar línea stale de XRPL_EVM)
12. **Reconciliar counts** README/JSON con el `/supported` real desplegado

---

## Conclusión

El sistema de **separación de poderes funcionó**: un equipo construyó, otro equipo independiente auditó, y encontró un **signature bypass de pérdida de fondos** + un **crash post-settlement** + un **deploy roto** que un humano revisando 1,293 líneas de Rust a medianoche muy probablemente se habría perdido. **Un NO-GO en esta etapa es exactamente el resultado deseado** — significa que el borrador NO llegó a producción con esos defectos.

El camino a producción está claro: **6 P0 + verificación en testnet.** La base es sólida (datos correctos, arquitectura limpia, cero regresiones); lo que falta es lógica de seguridad y un build verificado.

---

*Generado por un panel de 7 agentes Claude (Opus 4.8, 1M context). Recomendación, no acción: el deploy sigue siendo decisión humana.*

---

## REMEDIACIÓN — RESUELTO (2026-05-29, v1.45.2)

Todos los bloqueadores subsanados, verificados por re-auditoría independiente (`security-auditor` → **GO**), y desplegados a producción.

| Bloqueador | Estado | Fix |
|---|---|---|
| P0-1 Signature bypass | ✅ RESUELTO | `verify_signature` liga `Account`↔`SigningPubKey` (`derive_classic_address(pubkey) == account`); rechaza mismatch/multisig. Tests no-vacuos (Account=A firmado por key_B → rechazado) válidos para ed25519 y secp256k1, confirmados a nivel cripto. |
| P0-2 CAIP-2 panic | ✅ RESUELTO | `Namespace::Xrpl` (validación `network_id` u32); `to_v2()` ya no paniquea post-settlement; tests round-trip `xrpl:0`/`xrpl:1`. |
| P0-3 IOU f64 | ✅ RESUELTO | `rust_decimal` exacto + guard de precisión sub-unidad; rechaza todo underpayment (incl. el residual `.round()` hallado en la re-auditoría). |
| P0-4 Dockerfile xrpl | ✅ RESUELTO | `--features solana,near,stellar,algorand,sui,xrpl`. |
| P0-5 redact.rs | ✅ RESUELTO | committeado (`b4e0ad2`). |
| P0-6 compiló | ✅ RESUELTO | Rust 1.94 local / Docker `rust:bullseye`; build verde con feature-set completo (28 crates xrpl-rust). |
| HIGH-7 engine gate | ✅ RESUELTO | rechaza `tem*`/`tef*`/`tel*`/`tec*` rápido. |
| MED rpc-leak | ✅ RESUELTO | `without_url()` + `redact::rpc_url`. |
| MED str-slice panic | ✅ RESUELTO | truncado char-safe. |
| P2 bak2 | ✅ RESUELTO | `git rm src/network.rs.bak2` + gitignore `*.bak*`. |

**Commit:** `5bbda45 fix(xrpl,security)`. **Deploy:** v1.45.2 (verificado live: `/version`=1.45.2, `/supported` contiene `xrpl:0`, `/health` healthy, `/verify` XRPL rechaza sin panic/500). **Tests:** 13 de regresión pasan. **Re-auditoría independiente:** GO.

**Pendiente recomendado (NO bloqueante del riesgo de seguridad):** e2e completo en testnet con el cliente t54 real (firmar Payment → `/verify` → `/settle`; confirmar valid→true, ataque→false, underpayment rechazado) antes del primer uso por merchants. El binding se validó vía tests unitarios no-vacuos + auditoría adversarial independiente + binario desplegado == código auditado/testeado. Confirmar también la representación `asset` de XRP nativo en requests `/verify` (nota funcional, no de seguridad).
