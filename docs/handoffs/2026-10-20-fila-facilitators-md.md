# Nuestra fila en `docs/dev-tools/facilitators.md` de `x402-foundation/x402` — handoff 2026-10-20

- **Encargo**: task_92d1d0b43b42 de c0der (lazo del despachador), fila de
  `docs/plans/backlog-batch-settlement-recheck.md` (*"Fila propia en `docs/dev-tools/facilitators.md`"*),
  ancla `a9f113c2`, medida 2026-10-20 18:41Z.
- **Resultado**: **preparado, no publicado.** La rama y el commit firmado están en nuestro fork; el PR
  contra `x402-foundation/x402` no se abrió porque `[terceros] vista_previa_por_pr` pide un OK por PR
  externo y la vista previa (`msg_f94fa7ff6325`) no tuvo respuesta en ~70 min, con una
  escalada en el medio (`msg_3270d9fc9a4e`). Abrirlo es un comando, abajo.
- **Producción no se tocó.** Solo lecturas públicas (`/version`, `/supported`).
- **Presupuesto**: 0 subagentes, 0 workflows.

## La premisa, contra la red

Reproduce. En `main` de `x402-foundation/x402` (`faa1c694`; el último cambio al archivo es
`a0f37d0e`, 2026-10-13) la tabla tiene **15 filas** y ninguna nuestra. Lo que ya teníamos allá es
otra cosa: #1780 (mergeado 2026-04-13) nos puso en el *directorio de ecosistema*
(`typescript/site/.../partners-data/`), no en esta tabla, que nació después (#2474, 2026-05-26).
El otro PR nuestro, #3377, es la spec de `durable-evidence`.

`/supported` en vivo (2.51.0): 160 kinds, `upto` en **22** (8 mainnets EVM + 3 testnets, cada una
con su alias CAIP-2), la cifra del backlog. Mainnets de pago: **23**
(`scripts/verify_landing_canonical.py`).

## Qué se hizo

La fila, en orden alfabético después de T54:

```
| [Ultravioleta DAO Facilitator](https://facilitator.ultravioletadao.xyz) | Free, open-source Rust facilitator for EVM chains, Solana, Fogo, NEAR, Stellar, Algorand, Sui, XRPL and Hedera, with `exact` on every network and `upto` (Permit2) on EVM. No API key or account required |
```

No lleva cuenta de redes: las demás filas tampoco, y un número en un repo ajeno envejece sin que nos
enteremos. La cuenta (23 mainnets, `upto` en 8) va en el cuerpo del PR, con fecha.

- Rama `docs/ultravioleta-dao-facilitator` del fork `0xultravioleta/x402`, desde `faa1c694`.
- Commit `cfcb9722`, +1/-0 en un archivo, **firmado**: `verified=true`, `reason=valid`.
- Título y cuerpo, abajo en "Para c0der". El cuerpo difiere de la vista previa en una cláusula:
  se quitó *"the facilitator pays gas"*, que es falsa en XRPL (el pagador firma la transacción entera
  y nosotros solo la retransmitimos, `src/chain/xrpl.rs:7`; `/supported` no anuncia `feePayer` ahí).

**Por qué el commit lo hizo la API GraphQL y no `git`.** Upstream exige commits firmados y su bot
cierra el PR que no los trae (#2989 se cerró por `unknown_key`). Esta Mac no tiene `gpg` ni
`~/.gnupg`, así que la clave PGP que firmó #3377 no está acá. El `PUT` de la Contents API deja el
commit sin firmar (probado: `9f7733b5`, `reason=unsigned`; esa rama se borró a los minutos y nunca
tuvo PR). `createCommitOnBranch` lo firma GitHub (committer `GitHub <noreply@github.com>`). Sirve
para el próximo cambio de una línea a un repo ajeno desde esta máquina.

## Verificación

Un chequeo de la tabla contra `/supported` (en el scratchpad, no se commitea: el cambio vive en un
repo ajeno y el chequeo pega a la red). Comprueba que la fila aparece una vez, que la tabla sigue en
dos columnas y en orden alfabético, y que cada familia y esquema que la fila nombra existe en el
`/supported` vivo.

| Estado | Resultado |
|---|---|
| La rama del PR | **verde**: 16 filas, 23 mainnets, `upto` en 8 mainnets |
| `main` upstream sin la fila | rojo: la fila aparece 0 veces |
| La fila movida arriba de todo | rojo: no alfabético |
| `/supported` sin Hedera | rojo: la fila nombra Hedera |
| `/supported` sin `exact` en `base` | rojo: `exact` falta en `base` |

Para re-medir sin el script:

```bash
# 0 hoy en main upstream; 1 después del merge
gh api repos/x402-foundation/x402/contents/docs/dev-tools/facilitators.md \
  -H "Accept: application/vnd.github.raw" | grep -c facilitator.ultravioletadao.xyz
# 22 el 2026-10-20
curl -s https://facilitator.ultravioletadao.xyz/supported | jq '[.kinds[]|select(.scheme=="upto")]|length'
```

## Para c0der

- **Quedó**: el OK de la vista previa y abrir el PR. Con el OK, desde cualquier checkout (guardar
  antes el bloque de abajo como `pr_body.md`):

  ```bash
  gh pr create -R x402-foundation/x402 --base main \
    --head 0xultravioleta:docs/ultravioleta-dao-facilitator \
    --title "docs: add Ultravioleta DAO Facilitator to facilitators list" --body-file pr_body.md
  ```

  Antes, `gh api repos/0xultravioleta/x402/compare/x402-foundation:main...docs/ultravioleta-dao-facilitator --jq '{ahead_by,behind_by}'`:
  dio `1`/`0` a las ~19:55Z. Si `main` upstream tocó el archivo, la rama se rehace con
  `createCommitOnBranch` sobre el `main` nuevo, nunca con un push forzado.
- **Después, el merge es de los maintainers, y no es rápido para todos.** Los 8 PRs de esta tabla
  mergeados desde junio los mergeó `phdargen` entre 0 y 10 días después de abiertos, pero hoy hay
  **10 PRs abiertos** que tocan el archivo, el más viejo del 2026-08-26 (#3271). Varios insertan cerca
  del final de la tabla, así que puede tocar resolver un conflicto de una línea.
- **Cierre de verdad** (criterio de la fase 0g del memo): `gh pr view <n> -R x402-foundation/x402 --json state`
  → `MERGED`, y el `grep -c` de arriba → 1. Hasta entonces la fila del backlog queda en espera, no cerrada.
- **La fila envejece** si se suma o se cae una familia: nombra EVM, Solana, Fogo, NEAR, Stellar,
  Algorand, Sui, XRPL y Hedera. Al tocar `NetworkFamily`, revisarla.
- **Números**: 15 filas hoy / 16 en la rama; 160 kinds; `upto` 22 kinds = 8 mainnets + 3 testnets;
  23 mainnets; commit `cfcb9722` `verified=true`; 1 verde y 4 rojos en el chequeo.
- **Trailer** de los commits: `Co-Authored-By: Claude Opus 5`, el que pidió el despacho.

### Cuerpo del PR

````markdown
## Description

Adds the Ultravioleta DAO Facilitator to `docs/dev-tools/facilitators.md`: one row, in alphabetical position after T54.

- Live at https://facilitator.ultravioletadao.xyz (`/supported`, `/verify`, `/settle`; OpenAPI at `/docs`)
- `GET /supported` lists 23 mainnets: Base, Ethereum, Arbitrum, Optimism, Polygon, Avalanche, Celo, BSC, Unichain, Monad, HyperEVM, Scroll, SKALE Base, Arc, Robinhood Chain, Solana, Fogo, NEAR, Stellar, Algorand, Sui, XRPL and Hedera
- `exact` on every one of them; `upto` (Permit2) on 8 EVM mainnets
- No facilitator fee, no API key, no account
- Source, Apache-2.0: https://github.com/UltravioletaDAO/x402-rs
- Already in the ecosystem directory (#1780)

## Tests

Docs-only change. The table keeps two columns and alphabetical order, and every network and scheme the row names was checked against the live `GET /supported` on 2026-10-20.

## Checklist

- [x] My commits are signed
- [x] Docs-only change, so no changelog fragment

🤖 Generated with [Claude Code](https://claude.com/claude-code)
````
