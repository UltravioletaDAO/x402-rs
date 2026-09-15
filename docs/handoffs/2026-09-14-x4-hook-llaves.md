# Handoff — pre-commit anti-llaves versionado en x402-rs

**Fecha:** 2026-09-14 · **Rama:** `0xultravioleta/x4-hook-llaves` · **Worker:** `x4-hook-llaves`

> Directiva de INC-2026-03-30: dos wallets drenadas por private keys
> hardcodeadas en repos publicos. `meshrelay`, `execution-market` (#273) y
> `describe-net` (#11) ya tienen este hook versionado. `x402-rs` no lo tenia
> (`git ls-tree origin/main -- .githooks` vacio antes de este PR).

## Que cambio

Copia tal cual de `UltravioletaDAO/meshrelay@main` (`.githooks/pre-commit`,
`.githooks/README.md`, `.githooks/pre-commit.local.example`), mas los ajustes
minimos para que funcione en este repo:

- `.githooks/pre-commit` — bit de ejecucion (`git update-index --chmod=+x`).
  Bloquea el commit si una linea AGREGADA del diff staged matchea `0x` + 64 hex
  (`hex64`), `.log:<digitos>` (`logptr`), `query:<letra>` al inicio de linea
  (`query`), o un string de una lista local opcional (`local-list`, no
  aplicable aca salvo que alguien cree `.githooks/pre-commit.local`).
- `.githooks/README.md` — copia con la linea `Regla escrita (LIFECRAWLER 1.11)`
  eliminada (es una regla del corpus de meshrelay/karmakadabra, no aplica a
  x402-rs). No habia otra mencion a "meshrelay" que ajustar.
- `.githooks/pre-commit.local.example` — copia tal cual, sin cambios.
- `.gitattributes` (nuevo, el repo no tenia uno) — fuerza `eol=lf` en
  `.githooks/*`: el hook corre con `/bin/sh` y un shebang con CR muere en
  silencio. `core.autocrlf=true` esta activo en este repo (confirmado durante
  la prueba), asi que esto es necesario, no cosmetico.
- `.gitignore` — agrega `.githooks/*.local` (mismo patron que meshrelay), para
  que la lista sensible opcional nunca se trackee por accidente.
- `CLAUDE.md` — una linea en "Other Security Rules": como activarlo
  (`git config core.hooksPath .githooks`) y por que (INC-2026-03-30).

## Como activarlo

Una vez por clon:

```bash
git config core.hooksPath .githooks
git config --get core.hooksPath   # debe imprimir: .githooks
```

Bypass deliberado (no usarlo salvo falso positivo real en un tx hash):
`git commit --no-verify`.

## Prueba (worktree local, no entra al PR)

Con `core.hooksPath .githooks` activo:

**Estado bloqueado** — archivo agregado con `0x` + 64 hex no-cero:

```
[pre-commit] COMMIT BLOCKED. Staged additions match a forbidden pattern:
  _precommit_test_secret.txt:1  [hex64]
  hex64      -> read keys from env / Secrets Manager; tx hashes: cite explorer URL or a prefix
  logptr     -> no pointers into raw chat logs; aggregate + key, never a line
  query      -> no raw query dumps
  local-list -> corpus path / disk / handle from the local list; aggregate only
EXIT_CODE=1
```

**Estado normal** — archivo sin patrones prohibidos: commit pasa, `EXIT_CODE=0`.

El archivo y el commit de prueba se borraron (`git reset --soft`, borrado del
archivo); el valor hex de prueba no llego a ningun commit del PR.

## CI

Ningun workflow de `.github/workflows/` se dispara por estos paths via
`paths:` filter — `ci.yaml` filtra por `src/**`, `crates/**`, `static/**`,
etc., y ninguno cubre `.githooks/**`, `.gitattributes`, `.gitignore` o
`CLAUDE.md`. `no-account-id.yml` SI corre (no tiene `paths:` filter, corre en
todo push/PR) pero es un grep de segundos, no el pipeline caro del
facilitador.

## Cierre

- `git ls-tree origin/0xultravioleta/x4-hook-llaves -- .githooks/pre-commit`
  lista el archivo con modo `100755`.
- La prueba de commit bloqueado devuelve exit code distinto de 0 (ver arriba).
