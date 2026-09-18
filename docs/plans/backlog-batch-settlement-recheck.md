# Backlog — pendientes del re-check de `batch-settlement` (2026-09-07)

**Estado:** el re-check está escrito y cerrado (§10 de
`docs/plans/batch-settlement/00-DECISION-MEMO.md`). El veredicto "no construir"
sigue en pie: el gate de §8.9 **no se disparó**. Lo de abajo es lo que quedó
abierto alrededor de esa sección.

**Contexto:** Solana Foundation anunció Payment Channels el 2026-09-03 y PayAI
tuiteó soportarlo el 09-04. Se midieron las cuatro condiciones del gate el
2026-09-07 — ninguna dispara. Detalle completo en §10 del memo.

---

| Fecha | Item | Contexto | Prioridad | Estado |
|---|---|---|---|---|
| 2026-09-07 | **Decidir si `docs/plans/batch-settlement/` entra a git** | El directorio entero (4 archivos, ~207 KB) está **sin trackear desde el 2026-08-25**, incluida la §10 recién escrita. No se commiteó porque meter tres archivos que el dueño dejó fuera de git es decisión suya. Si entra: `git add` por nombre, nunca `-A`. | P1 | Abierto |
| 2026-09-07 | **Fila propia en `docs/dev-tools/facilitators.md` de `x402-foundation/x402`** | Es un PR de **una fila** a una tabla markdown. Hay 15 competidores listados y nosotros no. El anuncio de Solana empuja atención hacia los facilitadores listados, y nosotros servimos `upto` en 22 kinds mientras PayAI sirve cero (medido 09-07). §4 y §9 del memo ya lo resolvieron a favor de shippear; sigue sin abrirse. **Es el ítem de mayor retorno de todo el dossier y no tiene nada que ver con batching.** | P1 | Abierto |
| 2026-09-07 | **Desambiguar el texto del gate en §8.9** | §8.9 dice *"aggregation depth exceeds 1 on any chain"* sin decir cuál profundidad. Contando instrucciones por tx da 3 y el gate dispararía; contando claims de valor da 1 y no dispara. La corrección quedó escrita en §10.1 pero **§8.9 sigue con el texto ambiguo** — se dejó así por cirugía de alcance. Editarlo a "aggregation depth of value-bearing claims". | P2 | Abierto |
| 2026-09-07 | **Próximo re-check del gate** | Con el barrido trimestral de la Fase 0f, o antes si x402.org o CDP mueven `batch-settlement` a una red mainnet. **Decodificar instrucciones antes de citar una profundidad** (ver §10.1). | P2 | Programado |

---

## Lo que NO quedó pendiente

- El re-check en sí: escrito, medido y fechado. §10, cuatro mediciones reproducibles.
- Coordinación con Execution Market: **no se debe ninguna**. Su rail de canales
  en Solana pasa por el sidecar de pay.sh, no por el facilitador, y su plan de
  11 pasos no nos nombra ni una vez. §10.3.
- Implementar `batch-settlement` SVM: sigue siendo **no**, y el rol SVM
  (custodia de renta además del gas) lo refuerza en vez de debilitarlo. §10.2.
