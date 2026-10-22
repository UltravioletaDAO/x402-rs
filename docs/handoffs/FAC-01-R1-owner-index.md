# FAC-01-R1 - indice de owner ERC-8004 (PR #123), rondas del refutador

## Estado

- R1 hecho: P2-1 (un owner de mas de 8.000 agentes tiene veredicto; por encima de lo
  demostrable el lookup se rechaza sin walk ni mutex), P3-2 (tests que matan
  M7, M9, M10, M11), P3-3 (tamano de Base medido de nuevo), P1-1 (tabla de pre-CI,
  Auto-refutacion y Rojos de base en el cuerpo del PR).
- R2 hecho (REFUTACION-c0der FAC-01V): P2-1 (el `balanceOf` del veredicto se lee en el
  mismo bloque que los `ownerOf` del probe), P2-2 (los probes se pagan del presupuesto
  de la fase y no se repiten hasta que el indice puede alcanzar el balance leido), P3-1
  (test del `truncate`), P3-2 (docs que el PR dejaba falsas), P3-3 (el rechazo de un
  owner con demasiados agentes sale `retryable: false` con `errorCode`), P3-4 (la
  seccion "Rojos de base" del cuerpo).
- Falta: el P3-1 opcional de la ronda 1 (cola-primero / `store_scan_hint` antes del
  walk lineal) sigue sin hacerse; queda como fila aparte.
- Proximo paso: la refutacion de c0der sobre el head de R2. Nada mergeado ni desplegado.

## Que cambio en `src/handlers.rs`

### R1

- `owner_index_answer` re-lee hasta `OWNER_INDEX_MAX_PROBES` candidatos (los mas
  bajos), en lugar del tope anterior de `OWNER_SCAN_WAVE * OWNER_SCAN_BATCH` (8.000) que
  devolvia `Ok(None)` y dejaba el walk sin veredicto para siempre.
- `owner_index_rebuild`: si `balanceOf` supera `OWNER_INDEX_MAX_PROBES`, rechaza antes de
  tomar `OWNER_INDEX_WALK` y antes de cualquier `eth_call`.

### R2

- `multicall_balance_and_owner_of`: el probe lee `balanceOf(target)` en el primer batch
  del mismo `aggregate3` que los `ownerOf`; si el probe necesita mas de un batch, todos
  se fijan al numero de bloque que devuelve `eth_blockNumber` (nunca `block.number` del
  EVM: en Arbitrum es el bloque de L1). `owner_index_verdict` compara contra ese balance.
- `owner_index_probe_cost` + `OwnerIndexLookup`: cada probe cuesta sus batches (mas el
  `eth_blockNumber` si son varios) y se paga del presupuesto de la fase
  (`OWNER_SCAN_MAX_BATCHES`); en el rebuild, walk y probes comparten ese presupuesto. Un
  probe que no entra no se manda. El balance que lee pasa a ser el umbral del siguiente.
- `OWNER_INDEX_MAX_PROBES = (OWNER_SCAN_MAX_BATCHES - 1) * OWNER_SCAN_BATCH - 1`
  (189.999): lo que entra en una fase con el `balanceOf` y el `eth_blockNumber` pagados.
- `OwnerLookupError`: el rechazo de un owner con mas de eso responde 503 con
  `retryable: false` y `errorCode: owner_holds_too_many_agents`; todo otro "sin
  veredicto" sigue siendo 503 `retryable: true`. Nunca 404.

## Tests nuevos

### R1

- `owner_index_tests::refuter_r1_big_holder_past_cap` (P2-1)
- `owner_index_tests::a_holder_of_exactly_what_the_index_can_prove_gets_a_verdict` (borde)
- `owner_index_tests::a_holder_past_what_the_index_can_prove_is_refused_without_a_walk`
- `owner_index_tests::below_the_cap_a_second_lookup_is_answered_by_the_index_not_a_scan` (M7, M9)
- `erc8004_register_gate_tests::a_minted_identity_is_indexed_from_its_registered_event` (M10)
- `erc8004_register_gate_tests::a_delivered_identity_is_indexed_under_its_recipient` (M11)

### R2 (todos en `owner_index_tests`)

- `refuter_r2_concurrent_gain_answers_a_non_lowest_agent` (P2-1, texto del refutador)
- `refuter_r2_probe_cost_per_request_when_the_index_knows_more_than_the_balance` (P2-2, texto del refutador)
- `a_probe_that_spans_batches_reads_them_all_at_one_block` (el pin de bloque)
- `a_probe_is_paid_from_the_budget_and_not_sent_when_it_does_not_fit` (el presupuesto)
- `a_probe_that_finds_the_index_short_waits_for_the_walk` (el balance leido como umbral)
- `an_owner_grown_past_what_one_probe_proves_is_refused_after_that_probe`
- `an_index_holding_more_candidates_than_one_probe_reads_still_answers` (P3-1, el `truncate`)
- `the_largest_probe_fits_one_phase_and_one_more_candidate_does_not`
- Ampliados: `a_holder_past_what_the_index_can_prove_is_refused_without_a_walk`
  (`retryable: false`, `errorCode`) y
  `below_the_cap_a_second_lookup_is_answered_by_the_index_not_a_scan` (R7: la respuesta
  del indice llena el cache positivo).

La evidencia (tabla job -> comando -> resultado, mutaciones, rojos de base) va en el
cuerpo del PR #123.
