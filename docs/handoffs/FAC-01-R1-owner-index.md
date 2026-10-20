# FAC-01-R1 - indice de owner ERC-8004 (PR #123), ronda del refutador

## Estado

- Hecho: P2-1 (un owner de mas de 8.000 agentes tiene veredicto; por encima de lo
  demostrable, 192.000, el lookup se rechaza sin walk ni mutex), P3-2 (tests que matan
  M7, M9, M10, M11), P3-3 (tamano de Base medido de nuevo), P1-1 (tabla de pre-CI,
  Auto-refutacion y Rojos de base en el cuerpo del PR).
- Falta: P3-1 (opcional, cola-primero / `store_scan_hint` antes del walk lineal) NO se
  hizo en esta ronda; queda como fila aparte.
- Proximo paso: la refutacion de c0der sobre el head de esta ronda. Nada mergeado ni
  desplegado.

## Que cambio en `src/handlers.rs`

- `OWNER_INDEX_MAX_PROBES = OWNER_SCAN_MAX_BATCHES * OWNER_SCAN_BATCH` (192.000):
  `owner_index_answer` re-lee hasta esa cantidad de candidatos (los mas bajos), en lugar
  del tope anterior de `OWNER_SCAN_WAVE * OWNER_SCAN_BATCH` (8.000) que devolvia
  `Ok(None)` y dejaba el walk sin veredicto para siempre.
- `owner_index_rebuild`: si `balanceOf` supera `OWNER_INDEX_MAX_PROBES`, devuelve el
  error (503 retryable) antes de tomar `OWNER_INDEX_WALK` y antes de cualquier `eth_call`.

## Tests nuevos

- `owner_index_tests::refuter_r1_big_holder_past_cap` (P2-1)
- `owner_index_tests::a_holder_of_exactly_what_the_index_can_prove_gets_a_verdict` (borde)
- `owner_index_tests::a_holder_past_what_the_index_can_prove_is_refused_without_a_walk`
- `owner_index_tests::below_the_cap_a_second_lookup_is_answered_by_the_index_not_a_scan` (M7, M9)
- `erc8004_register_gate_tests::a_minted_identity_is_indexed_from_its_registered_event` (M10)
- `erc8004_register_gate_tests::a_delivered_identity_is_indexed_under_its_recipient` (M11)

La evidencia (tabla job -> comando -> resultado, mutaciones, rojos de base) va en el
cuerpo del PR #123.
