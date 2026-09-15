# Handoff: redaccion neutra en la documentacion publicada

**Fecha:** 2026-09-15 · **Rama:** `0xultravioleta/x4-handoffs-neutros` · **Base:** `origin/main` = `54765380`
**Tipo:** solo documentacion. Sin cambios de codigo, sin `cargo build`/`cargo test`, sin `VERSION`.

## Que se hizo

El repositorio es publico. Tres handoffs mergeados el 2026-09-14 (y, al revisar con el mismo
criterio, otros 46 archivos de `docs/` desde 2026-08-23) nombraban herramientas y sesiones
internas de coordinacion: el nombre del coordinador, la herramienta de orquestacion, ids de
tareas y despachos, archivos de preguntas y specs locales, la maquina donde corria el trabajo,
rutas locales de maquinas con usuario (`/mnt/c/Users/...`, `C:\Users\...`) y rutas a un repo
interno. Se reemplazaron por texto neutro ("el mantenedor", "la revision", "el operador",
"`<worktree>`", "`<repo>`", "fuera de este repo", "no versionado").

No se toco contenido tecnico: fechas, commits, archivo:linea, mediciones y pasos de verificacion
quedan igual. Encabezados `## Para ...` pasaron a `## Para el mantenedor` y el ancla de
`2026-09-10-bazar-describe.md` se actualizo a `#para-el-mantenedor`.

Secciones que solo explicaban que la herramienta interna no conectaba desde WSL se redujeron a la
parte que si importa al repo (la interop de Windows apagada, el `gitdir` del worktree reescrito,
`core.autocrlf`). En `2026-09-02-paginas-listo.md` se borro un bloque de comando interno con ids
de sesion.

**No se reescribio historia:** no hay secretos de fondos, AWS ni RPC en lo corregido; el cambio
va en un commit nuevo.

**Fuera de alcance, a proposito:** las rutas `/mnt/z/ultravioleta/dao/x402-rs/...` de docs viejos
(upstream-sync, planes, auditorias) se quedan: son la ruta del checkout que documenta `CLAUDE.md`.

## Verificacion

- El `git grep -i` de aceptacion del encargo (nombre del coordinador, herramienta de
  orquestacion, ids de tarea y despacho, archivo de preguntas, maquina, usuario local y
  directorio temporal de sesion) sobre `docs/` da **sin salida** (exit 1). El patron literal
  no se copia aca a proposito: citarlo haria que el grep se encontrara a si mismo; esta en el PR.
- Las tres reglas del workflow `No AWS account ID in the repo`
  (`.github/workflows/no-account-id.yml`), corridas en local sobre el arbol: sin salida. No se
  agrego ningun numero de 12 digitos.

## Archivos tocados (49 + este handoff)

- `docs/handoffs/`: 2026-08-23-corte-autoria-mainnet-delegates-WSL, 2026-08-24-execution-market-struct-v4-cerrado,
  2026-08-25-em-respuesta-del-21-ago-entregada, 2026-08-25-kk-em-digest-semantics,
  2026-08-25-kk-retraccion-custodia-entregada, 2026-08-31-cierre-de-sesion-y-backlog,
  2026-09-02-agentic-ola2-listo, 2026-09-02-mcp-listo, 2026-09-02-mcp-medicion, 2026-09-02-paginas-listo,
  2026-09-02-paginas-medicion, 2026-09-02-superficies-agenticas-listo, 2026-09-02-superficies-agenticas-medicion,
  2026-09-03-friccion-implementada, 2026-09-03-portada-revertida, 2026-09-03-rediseno-listo,
  2026-09-04-dx402-snapshot-camino-al-pr, 2026-09-04-hint-y-forma-v2, 2026-09-04-paper-p0,
  2026-09-04-settle-sin-confirmar, 2026-09-05-lifecycle-auth-log-a-enforce,
  2026-09-05-tres-one-liners-de-la-landing, 2026-09-10-a4-a5, 2026-09-10-base-debian-vencida,
  2026-09-10-bazar-describe, 2026-09-10-bazar-precios-p0 a p4, 2026-09-10-bucket-versioning,
  2026-09-10-hotfix-cpu, 2026-09-10-hotfix-cpu-2, 2026-09-10-mint-atomico, 2026-09-10-p0-auditoria,
  2026-09-10-polygon-cola, 2026-09-10-ui-astra, 2026-09-13-pyusd-solana, 2026-09-13-x4-escrow-enforce,
  2026-09-13-x4-superficie-humana, 2026-09-14-x4-health-rpc, 2026-09-14-x4-hook-llaves,
  2026-09-14-x4-tip-floor-l2, 2026-09-14-x4-verify-liquidado (todos `.md`)
- `docs/reports/2026-09-10-benchmark-capacidad.md`
- `docs/sinergias/2026-09-02-commonware-keep-the-change.md`
- `docs/plans/bazaar/01-current-state-audit.md`, `docs/plans/bazaar/06-rollout-and-ops.md`
- `docs/plans/dx402/13-ISSUE-Y-PR-UPSTREAM.md`

`docs/CHANGELOG.md` no tenia coincidencias.

## Para el mantenedor

Nada que desplegar. `docs/` no esta en los `paths` de `on.push` de `.github/workflows/ci.yaml`,
asi que el merge a `main` no dispara build ni deploy; solo corre el workflow
`No AWS account ID in the repo`.
