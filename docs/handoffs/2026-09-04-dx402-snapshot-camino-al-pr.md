# DX402 — snapshot del camino al PR upstream (2026-09-04)

**Para:** quien retome esto (Saul, o una sesión nueva). **Estado al cierre:** el
issue está abierto; el PR está a un comando, esperando reacción de maintainer o
72 h, y el go explícito de Saul.

## Lo que está vivo

| | |
|---|---|
| Facilitador en producción | **2.12.0** (`0274ccc4`, CI 33837812160). Hoy se desplegaron 2.11.0 y 2.12.0, cada uno con compuerta local + CI verdes |
| Corpus certificado | **119 `verified: true`, 100 % EVM, 7 redes, 26 compradores / 24 vendedores** — medido sobre `facilitator_dx402_evidence`; las 5 filas de Solana pre-gate pasaron a provisionales |
| Spec | v0.3 en formato registro: `docs/plans/dx402/12-SPEC-v0.3-foundation.md` = `specs/extensions/durable-evidence.md` del PR |
| Issue upstream | **https://github.com/x402-foundation/x402/issues/3371** (abierto 2026-09-04) |
| Rama del PR | fork `0xultravioleta/x402`, rama `spec/durable-evidence`, 3 archivos (spec, `docs/extensions/durable-evidence.mdx`, fila en `overview.mdx`), base `upstream/main` del 04-sep, commit **firmado y verificado por GitHub** (`ultravioletadao@gmail.com`) |
| Textos del PR | `docs/plans/dx402/13-ISSUE-Y-PR-UPSTREAM.md` §2 (poner `Closes #3371`) |

## Lo que se hizo hoy, en orden

1. **Siete agentes en paralelo** verificaron todo: proceso real de la Foundation (repo `x402-foundation/x402`, spec-only PR, issue primero, GPG+DCO, 5 propuestas rivales de hash-de-entrega sin revisar), spec vs código (18 desajustes, todos aplicados), red team (14 hallazgos, 4 P1 — 6 arreglados en código), paridad de SDK (deseable, no bloqueante; y encontró el bug del matcher), readiness de deploy (GO), evidence pack (número honesto 116→119), coherencia de docs (60 filas, aplicadas).
2. **Fixes de seguridad** (`f5e1f88f` tras rebase): riel clasificado desde el receipt (sellar al TokenStore ya no salta el escrow), `payee`/`txHash` del recibo atados a la prueba, `Uint::try_from` (panic remoto), `settle_before` corre el hook, declaración malformada falla cerrada, empate de precio → la que declara, sobrepago → la más cara cubierta, `SkipReason::Unknown`.
3. **Restructura al formato del registro** (`0274ccc4`): `extensions` top-level `{info, schema}` con `acceptIndexes`, evidencia bajo `SettlementResponse.extensions`, cliente honra el top-level, `extra.extensions` como fallback una versión.
4. **Issue #3371 abierto.**

## Lo que falta

| Qué | Dueño | Criterio |
|---|---|---|
| **Abrir el PR** desde la rama del fork con `Closes #3371` | Saul da el go; el comando está abajo | PR abierto, checks verdes, firma *Verified* |
| Un párrafo en slack.x402.org con el link del issue | Saul | — |
| Eco del `info` en el payload v2 del cliente (nuestro cliente es v1) | facilitador | test en `x402-reqwest` |
| Allowlist de tokens en el proof path (red team #3); `getHash` en `latest` (#5); normalizar `paymentId` (#6); `asset` en el matcher cuando v2 lo traiga (#14) | facilitador | — |
| Opt-in en los SDK py/ts (lista exacta en el reporte de paridad, resumida en `09-ESTADO`) | SDKs | e2e pagando la oferta durable desde py/ts |
| Fase 2 (`DX402_REQUIRE_PROOF=true`) tras ≥48 h de tráfico real en 2.12.0 | facilitador | logs sin rechazos sobre tráfico legítimo |
| Flota de KK reanudada post-demo para que el corpus siga creciendo | Saul / KK | el contador sube solo |

## Cómo abrir el PR (cuando haya go)

```bash
cd <clon de x402-foundation/x402 con remote fork=0xultravioleta/x402>   # hoy: scratchpad/x402-upstream
git fetch origin main && git rebase origin/main spec/durable-evidence   # por si upstream se movió
git push --force fork spec/durable-evidence
gh pr create --repo x402-foundation/x402 --head 0xultravioleta:spec/durable-evidence \
  --title "spec(extensions): durable-evidence — encrypted, retrievable delivery evidence" \
  --body-file <cuerpo del §2 del doc 13, con Closes #3371>
```

El clon en `scratchpad/` es efímero; si desapareció, `git clone` de
`x402-foundation/x402`, `git fetch fork spec/durable-evidence` desde
`https://github.com/0xultravioleta/x402.git` y seguir.

## Gotchas que costaron tiempo hoy (para no repetirlos)

- `cargo test -p x402-rs` **no compila `crates/`** ni sus `tests/`: barrer el workspace entero antes de dar verde. Costó tres builds.
- Un `git pull --rebase` aborta por un archivo sin trackear ajeno (`static/rlusd.png`): apartarlo, no borrarlo.
- Firmas GPG: `0xultravioleta@gmail.com` es de **otra** cuenta de GitHub; para el badge *Verified* firmar con `ultravioletadao@gmail.com`. Privada respaldada en Secrets Manager `github-gpg-signing-key-ultravioletadao`.
- La org UltravioletaDAO no permite fork sin admin → fork personal `0xultravioleta/x402`.
- Una defensa dentro de un `if` se salta evitando el `if`. Dos P1 de hoy eran eso.
