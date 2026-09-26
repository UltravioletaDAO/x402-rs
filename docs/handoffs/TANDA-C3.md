# TANDA-C3 — X4-STACK-429 + X-1 (escrow en Arc) + INT-09 (manifiesto de interop), en un CI, un merge y un deploy

**Encargo:** c0der, 2026-09-26T00:08Z. Regla del dueño (2026-09-23): en el facilitador se paga un CI y un deploy por tanda.
**Base:** `origin/main` = `f3786f3e` (2.41.0). **Rama:** `0xultravioleta/tanda-c3`, commits locales, **sin push**.

## Estado

- **Hecho.** Las tres piezas entraron con `git merge --no-ff`, en el orden pedido, y sus SHA verificados quedan como
  ancestros (sin cherry-pick, rebase ni squash). Los conflictos entre X4 e INT-09 se resolvieron con las decisiones
  (a)–(e) de c0der. La unión compila, la suite completa del CI da verde y cada test de cada pieza sigue y pasa. Hay
  una o más mutaciones por cada conflicto resuelto: 19, corridas con `scripts/verificar_ronda.py` sobre
  `446181aa`. 18 dan rojo y 1 no compila por el tipo (`E0583`), con el árbol limpio al final (§4).
- **Versión:** `2.43.0`, una sola para la tanda, con una entrada por pieza en `CHANGELOG.md` (§7).
- **No verde, y ya no lo estaba en `main`:** `cargo fmt --check` y `cargo clippy -D warnings`. La tanda no agrega
  ningún hunk de fmt y solo agrega dos avisos de clippy, de la misma clase que los de `main` (§5).
- **Falta (no es mío):** que c0der corra las mutaciones, pushee y siga el orden de deploy de §6.

| Commit | Qué |
|---|---|
| `2307c722` | merge de X4-STACK-429 (`2fc8ba7b`), sin conflictos |
| `e8215685` | merge de X-1 (`70d7bc83`), sin conflictos, más la línea de la fila del backlog en `static/index.html` |
| `0a8236d0` | merge de INT-09 (`24cf6d88`), con la resolución de §2 |
| `446181aa` | `VERSION` 2.43.0 y `CHANGELOG.md` |
| este commit | este handoff y `TANDA-C3.mutaciones.json` |

## 1. Qué entró

| Pieza | SHA | Qué trae |
|---|---|---|
| X4-STACK-429 | `2fc8ba7b` | Exención de los ocho presupuestos por IP para el stack (`X-UVD-Stack-Key`, solo SHA-256 en el facilitador). Admisión: techo por dirección (32, `429`), cuerpo entero en 5 s (`408`) y tope por task (512, `503`, para todos). `GET /config`. Todo en `src/rate_policy.rs`. Handoff: `docs/handoffs/X4-STACK-429.md` |
| X-1 | `70d7bc83` | Arc y Arc testnet en el set canónico v1 de escrow, con la ABI de PaymentOperator v3 (capture/void). Las escrituras v3 exigen que el operador tenga código y sea `paymentInfo.operator`. `/supported` anuncia el operador de Arc recién cuando se autoverifica (C-2 todavía no corrió: el anuncio aparece solo) |
| INT-09 | `24cf6d88` | `GET /.well-known/uvd-stack.json` (`uvd.stack/1`), cabeceras `RateLimit-Policy`/`RateLimit` (draft-11) y, en MCP, clase en `_meta["uvd/clase"]` y `outputSchema` + `structuredContent` en `x402_supported`. `FACILITATOR_GIT_SHA` como build-arg |

Medido antes de mergear: `git merge-tree` de X4 con X-1 y de X-1 con INT-09 dio limpio, como midió c0der. INT-09
con X4 chocó en los siete archivos que él listó.

## 2. Conflictos y cómo se resolvieron

### 2.1 Textuales (merge de INT-09)

| Archivo | Resolución |
|---|---|
| `Cargo.toml` | Queda el lado de X4: `governor` (nombrar `StateInformationMiddleware`) y `http-body-util` (reconocer `LengthLimitError`). El `governor` de INT-09 era la misma línea con otro comentario. `jsonschema` (dev-dependency de INT-09) entró sin conflicto |
| `Cargo.lock` | La lista de dependencias de `x402-rs` lleva las dos líneas: `http-body-util` (X4) y `jsonschema` (INT-09). `cargo build --locked` y `cargo test --locked` pasan con el lock tal cual, sin regenerarlo |
| `src/lib.rs` | `pub mod rate_policy;` queda y `pub mod rate_limit;` no entra: el módulo de INT-09 se funde en `rate_policy` (§2.2). `pub mod interop;` entró sin conflicto |
| `src/main.rs` | Base: el `main.rs` de X4+X-1. Todos los governors salen de `rate_policy::config(<BUDGET>.limit())` y se montan con `policy.layer(..)`, y los `Bucket::new(Policy::new("verify-settle", Duration::from_secs(2), 30))` de INT-09 no entran porque duplicaban los números. De INT-09 entran `mod interop;`, CORS que expone `ratelimit-policy`, `ratelimit` y `retry-after`, y `/mcp` montado en la puerta `mcp`: `policy.layer_on(&verify_settle_config, rate_policy::Door::Mcp)` |
| `src/client_ip.rs` | El test de fuente suma lo de los dos: exactamente un `GovernorConfigBuilder` y ningún `GovernorLayer::new(` fuera de `rate_policy.rs`. Antes, X4 exigía uno en `rate_policy.rs` e INT-09 uno en `rate_limit.rs` |
| `src/handlers.rs` | `human_page_routes_governed(policy, limit)` y `erc8004_write_governed(policy, ..)` quedan como en X4 (`policy.govern(..)`). Las versiones de INT-09 (`Bucket::new(..).govern(..)`) no eximían al stack. `secondary_read_rate_limit()`, que INT-09 conservaba y X4 había borrado, no vuelve: su número vive en `rate_policy::SECONDARY_READ`. De INT-09 entran `get_uvd_stack`, la ruta en `agentic_routes`, el texto nuevo del `429` y las filas nuevas de los tests de superficie |
| `static/.well-known/agent-skills/index.json` | El digest se recalculó sobre el `skill.md` final de la unión: no es el de X4 ni el de INT-09 |

### 2.2 Semánticos (no dieron conflicto de texto y había que resolverlos)

- **(a) Una sola capa y una sola fuente de números.** `src/rate_limit.rs` de INT-09 no entra. Lo que agregaba vive
  ahora en `src/rate_policy.rs`:
  - `Limit` lleva el nombre de su `Budget`, en un campo privado. Fuera de los tests, un `Limit` solo sale de
    `Budget::limit()`/`default_limit()` (`every_ms` y `named` son `#[cfg(test)]`).
  - `Bucket` guarda el `Limit` con el que se construyó.
  - `PolicyLayer` pone `RateLimit-Policy` y `RateLimit` **por fuera** del governor, así el `429` del governor las
    lleva, con los números del bucket mismo. No hay un segundo juego de números.
  - La traducción `q = burst`, `w = ceil(burst·period)`, `t = ceil(period)` y su justificación (doc del módulo de
    INT-09) se mudaron enteras.
- **Nombres.** Los presupuestos toman los nombres de INT-09: `verify-settle`, `discovery-register`, `discovery-read`,
  `events`, `identity-read`, `secondary-read`, `human-pages`, `erc8004-writes`. X4 no está desplegado, así que
  ningún cliente leyó los `verify_settle` de `/config`. Con esto `/config` y `RateLimit-Policy` usan el mismo
  nombre, y la documentación de INT-09 (`"verify-settle";q=30;w=60` en `skill.md`, `mcp.md` y OpenAPI) sigue
  siendo cierta.
- **(b) Exento contra tercero.** Un llamador con clave válida pasa por el servicio desnudo. Recibe
  `x-ratelimit-exempt` y **ninguna** de `RateLimit`, `RateLimit-Policy`, `x-ratelimit-limit` ni
  `x-ratelimit-remaining`. Un tercero recibe las de su presupuesto, en el `200` y en el `429`. Lo fija
  `a_stack_identity_is_told_no_budget_and_a_third_party_its_own`, sobre los ocho presupuestos.
- **(c) `/config` y `uvd-stack.json`.** El registro de montajes de INT-09 (`mounted()`) ahora lo escribe
  `RatePolicy::layer_on`, con el `Limit` del bucket y la puerta. El manifiesto publica (puerta, `q`, `w`) de esos
  montajes y `/config` publica los `BUDGETS`, con el mismo nombre y los mismos números. Lo fija
  `the_config_document_and_the_interop_manifest_publish_the_same_budgets`: las dos listas son iguales, cada nombre de
  `/config` es el de su cabecera y ninguno de los dos documentos trae una clave ni un digest.
- **Test de X4 auto-mergeado con los nombres de INT-09.** En
  `handlers::erc8004_write_rate_tests::production_mounts_the_writes_and_the_bazar_on_separate_budgets`, git metió los
  `discovery_register_limit.govern(` de INT-09 en líneas fuera del conflicto. Volvieron al cuerpo de X4
  (`discovery_register_config`), que es lo que monta `main.rs`.
- **Tests que comparaban `Limit` por valor.** Ahora un `Limit` lleva nombre, así que cambiaron tres asserts:
  `a_budget_override_replaces_only_what_parses` (X4), `every_erc8004_write_draws_on_one_bucket_of_thirty` (X4) y
  `production_mounts_the_writes_and_the_bazar_on_separate_budgets` (X4). Comparan contra
  `Limit::named("<presupuesto>", ms, burst)`, es decir, los mismos números y además el nombre.
  `every_governor_goes_through_the_policy` (X4) acepta `policy.layer_on(&<bucket>,` como montaje, además de
  `policy.layer(&<bucket>)`.
- **Tests de INT-09 adaptados.** Los seis tests de `rate_limit.rs` siguen con el mismo nombre en
  `rate_policy::tests`. Montan con `RatePolicy::none().layer_on(&config(Limit::named(..)), door)` y, donde fijaban
  el formato de `verify-settle` y `discovery-read`, leen el `default_limit()` del presupuesto. En `interop.rs`,
  `production_like_mounts()` ya no escribe los números a mano: los arma desde `BUDGETS`, cada presupuesto en `api` y
  `verify-settle` también en `mcp`.
- **`src/mcp.rs`** mergeó solo: X4 copia `X-UVD-Stack-Key` a la request sintética e INT-09 agrega la clase y el
  `outputSchema`. Los tests de los dos pasan. X4 no tocó ninguna definición de tool, así que la huella de
  `x402_supported` que midió INT-09 no cambia.
- **Documentos.** `skill.md` dice que `/config` publica cada bucket con el nombre de su `RateLimit-Policy`.
  `llms-full.txt` se regeneró con `scripts/build_llms_full.sh`. En OpenAPI, el párrafo del stack y la descripción de
  `/config` aclaran que el exento no recibe `RateLimit` y que `/config` y `rate_limits` son los mismos presupuestos,
  y el ejemplo de `/config` pasa a `verify-settle`.

### 2.3 La fila del backlog (decisión (e))

En `static/index.html`, la grilla de escrow que agrega X-1 nombra a Arc con `data-net-icon="arc"` en vez de
`src="/arc.png"`. Va dentro del commit de merge de X-1 (`e8215685`). Sin ese cambio,
`networks_json::tests::static_types_no_explorer_and_no_icon` da rojo en la unión (mutación T3-19).

## 3. Tests

### 3.1 Cada test que agregó cada pieza, en la unión

Tomado de la corrida completa del CI sobre `446181aa`. "Corridas" = cuántas veces aparece (lib y bin: `main.rs`
re-declara los módulos, así que un test de un módulo que está en los dos corre dos veces). No falta ninguno y
ninguno falla. El único `ignored` es `every_announced_arc_address_has_code_live`, que también es `#[ignore]` en X-1
(lee Arc mainnet en vivo).

#### X4-STACK-429 (25 tests)

| Test | Ruta en la unión | Corridas | Estado |
|---|---|---|---|
| `a_stack_identity_still_spends_the_daily_gas_cap` | `handlers::erc8004_write_rate_tests::a_stack_identity_still_spends_the_daily_gas_cap` | 2 | ok |
| `the_erc8004_writes_exempt_the_stack` | `handlers::erc8004_write_rate_tests::the_erc8004_writes_exempt_the_stack` | 2 | ok |
| `a_forwarded_settle_carries_the_stack_key_to_the_lease_holder` | `mcp::tests::a_forwarded_settle_carries_the_stack_key_to_the_lease_holder` | 1 | ok |
| `a_body_past_the_limit_is_a_json_413` | `rate_policy::tests::a_body_past_the_limit_is_a_json_413` | 2 | ok |
| `a_budget_override_replaces_only_what_parses` | `rate_policy::tests::a_budget_override_replaces_only_what_parses` | 2 | ok |
| `a_malformed_or_false_key_is_a_third_party_not_a_500` | `rate_policy::tests::a_malformed_or_false_key_is_a_third_party_not_a_500` | 2 | ok |
| `a_revoked_key_is_a_third_party_again` | `rate_policy::tests::a_revoked_key_is_a_third_party_again` | 2 | ok |
| `a_rotation_accepts_both_keys_then_only_the_new_one` | `rate_policy::tests::a_rotation_accepts_both_keys_then_only_the_new_one` | 2 | ok |
| `a_short_key_never_authenticates_whatever_its_digest` | `rate_policy::tests::a_short_key_never_authenticates_whatever_its_digest` | 2 | ok |
| `a_stack_identity_is_never_refused_by_any_budget` | `rate_policy::tests::a_stack_identity_is_never_refused_by_any_budget` | 2 | ok |
| `below_admission_the_key_debugs_as_sensitive` | `rate_policy::tests::below_admission_the_key_debugs_as_sensitive` | 2 | ok |
| `every_budget_has_its_own_name_and_variables` | `rate_policy::tests::every_budget_has_its_own_name_and_variables` | 2 | ok |
| `every_governor_goes_through_the_policy` | `rate_policy::tests::every_governor_goes_through_the_policy` | 2 | ok |
| `one_address_cannot_fill_the_ceiling_and_the_stack_skips_its_limit` | `rate_policy::tests::one_address_cannot_fill_the_ceiling_and_the_stack_skips_its_limit` | 2 | ok |
| `over_real_tcp_an_upload_that_never_arrives_holds_no_slot` | `rate_policy::tests::over_real_tcp_an_upload_that_never_arrives_holds_no_slot` | 2 | ok |
| `the_ceiling_is_never_zero` | `rate_policy::tests::the_ceiling_is_never_zero` | 2 | ok |
| `the_ceiling_sheds_the_stack_too` | `rate_policy::tests::the_ceiling_sheds_the_stack_too` | 2 | ok |
| `the_config_document_publishes_the_policy_in_force` | `rate_policy::tests::the_config_document_publishes_the_policy_in_force` | 2 | ok |
| `the_digest_is_what_shasum_prints` | `rate_policy::tests::the_digest_is_what_shasum_prints` | 2 | ok |
| `the_fixture_keys_are_well_formed` | `rate_policy::tests::the_fixture_keys_are_well_formed` | 2 | ok |
| `the_gas_cap_knows_nothing_of_the_stack` | `rate_policy::tests::the_gas_cap_knows_nothing_of_the_stack` | 2 | ok |
| `the_human_pages_exempt_the_stack_too` | `rate_policy::tests::the_human_pages_exempt_the_stack_too` | 2 | ok |
| `the_key_never_reaches_a_log_or_a_response` | `rate_policy::tests::the_key_never_reaches_a_log_or_a_response` | 2 | ok |
| `the_service_list_and_its_digests_are_read_defensively` | `rate_policy::tests::the_service_list_and_its_digests_are_read_defensively` | 2 | ok |
| `with_no_identity_configured_everybody_is_a_third_party` | `rate_policy::tests::with_no_identity_configured_everybody_is_a_third_party` | 2 | ok |

#### X-1 (25 tests)

| Test | Ruta en la unión | Corridas | Estado |
|---|---|---|---|
| `supported_announces_an_arc_operator_only_once_it_verified` | `facilitator_local::tests::supported_announces_an_arc_operator_only_once_it_verified` | 2 | ok |
| `supported_arc_announces_generation_d_only` | `facilitator_local::tests::supported_arc_announces_generation_d_only` | 2 | ok |
| `the_escrow_prose_names_every_escrow_network` | `openapi::tests::the_escrow_prose_names_every_escrow_network` | 1 | ok |
| `v3_selectors_are_the_generation_d_ones` | `payment_operator::abi::tests::v3_selectors_are_the_generation_d_ones` | 2 | ok |
| `arc_operator_address_matches_recorded_compute_address` | `payment_operator::arc_chain_tests::arc_operator_address_matches_recorded_compute_address` | 2 | ok |
| `arc_payer_agnostic_nonce_matches_the_escrow` | `payment_operator::arc_chain_tests::arc_payer_agnostic_nonce_matches_the_escrow` | 2 | ok |
| `every_announced_arc_address_has_code` | `payment_operator::arc_chain_tests::every_announced_arc_address_has_code` | 2 | ok |
| `every_announced_arc_address_has_code_live` | `payment_operator::arc_chain_tests::every_announced_arc_address_has_code_live` | 2 | ignored (`#[ignore]` en X-1: red viva) |
| `judge_needs_every_v3_selector_and_the_declared_escrow` | `payment_operator::autoverify::tests::judge_needs_every_v3_selector_and_the_declared_escrow` | 2 | ok |
| `lifecycle_owner_reads_fee_receiver_on_v3` | `payment_operator::lifecycle_auth::tests::lifecycle_owner_reads_fee_receiver_on_v3` | 2 | ok |
| `arc_authorize_requires_the_payers_eoa_signature` | `payment_operator::operator::arc_tests::arc_authorize_requires_the_payers_eoa_signature` | 2 | ok |
| `arc_authorize_waits_for_a_verified_operator` | `payment_operator::operator::arc_tests::arc_authorize_waits_for_a_verified_operator` | 2 | ok |
| `arc_autoverify_failure_does_not_block_release_or_refund` | `payment_operator::operator::arc_tests::arc_autoverify_failure_does_not_block_release_or_refund` | 2 | ok |
| `arc_escrow_state_query_uses_generation_d` | `payment_operator::operator::arc_tests::arc_escrow_state_query_uses_generation_d` | 2 | ok |
| `arc_partial_refund_is_rejected_without_tx` | `payment_operator::operator::arc_tests::arc_partial_refund_is_rejected_without_tx` | 2 | ok |
| `arc_refund_encodes_void` | `payment_operator::operator::arc_tests::arc_refund_encodes_void` | 2 | ok |
| `arc_rejects_create3_and_generation_c_addresses` | `payment_operator::operator::arc_tests::arc_rejects_create3_and_generation_c_addresses` | 2 | ok |
| `arc_release_encodes_capture` | `payment_operator::operator::arc_tests::arc_release_encodes_capture` | 2 | ok |
| `arc_transient_rpc_failure_on_refund_is_retryable_5xx` | `payment_operator::operator::arc_tests::arc_transient_rpc_failure_on_refund_is_retryable_5xx` | 2 | ok |
| `arc_v3_write_target_must_be_the_payment_operator` | `payment_operator::operator::arc_tests::arc_v3_write_target_must_be_the_payment_operator` | 2 | ok |
| `arc_v3_writes_require_operator_code` | `payment_operator::operator::arc_tests::arc_v3_writes_require_operator_code` | 2 | ok |
| `arc_void_with_zero_capturable_is_not_labeled_partial` | `payment_operator::operator::arc_tests::arc_void_with_zero_capturable_is_not_labeled_partial` | 2 | ok |
| `arc_zero_amount_refund_is_rejected_without_tx` | `payment_operator::operator::arc_tests::arc_zero_amount_refund_is_rejected_without_tx` | 2 | ok |
| `generation_is_unchanged_for_every_existing_escrow_network` | `payment_operator::operator::snapshot_tests::generation_is_unchanged_for_every_existing_escrow_network` | 2 | ok |
| `release_refund_calldata_snapshot_base_and_skale` | `payment_operator::operator::snapshot_tests::release_refund_calldata_snapshot_base_and_skale` | 2 | ok |

#### INT-09 (23 tests)

| Test | Ruta en la unión | Corridas | Estado |
|---|---|---|---|
| `a_broken_manifest_is_refused` | `interop::tests::a_broken_manifest_is_refused` | 2 | ok |
| `every_mounted_limit_is_published_on_its_door` | `interop::tests::every_mounted_limit_is_published_on_its_door` | 2 | ok |
| `every_url_is_on_the_public_origin_and_listed_for_the_router_test` | `interop::tests::every_url_is_on_the_public_origin_and_listed_for_the_router_test` | 2 | ok |
| `the_manifest_validates_against_the_vendored_schema` | `interop::tests::the_manifest_validates_against_the_vendored_schema` | 2 | ok |
| `the_rules_beyond_the_schema_hold` | `interop::tests::the_rules_beyond_the_schema_hold` | 2 | ok |
| `the_served_document_validates` | `interop::tests::the_served_document_validates` | 2 | ok |
| `the_validator_agrees_with_every_vendored_fixture` | `interop::tests::the_validator_agrees_with_every_vendored_fixture` | 2 | ok |
| `the_vendored_interop_files_match_origen` | `interop::tests::the_vendored_interop_files_match_origen` | 2 | ok |
| `a_tool_without_an_output_schema_answers_without_structured_content` | `mcp::tests::a_tool_without_an_output_schema_answers_without_structured_content` | 1 | ok |
| `every_tool_declares_its_house_class_in_meta` | `mcp::tests::every_tool_declares_its_house_class_in_meta` | 1 | ok |
| `every_url_the_interop_manifest_publishes_is_served` | `mcp::tests::every_url_the_interop_manifest_publishes_is_served` | 1 | ok |
| `only_the_read_tool_publishes_an_output_schema` | `mcp::tests::only_the_read_tool_publishes_an_output_schema` | 1 | ok |
| `supported_returns_structured_content_that_matches_its_output_schema` | `mcp::tests::supported_returns_structured_content_that_matches_its_output_schema` | 1 | ok |
| `the_class_agrees_with_the_annotations` | `mcp::tests::the_class_agrees_with_the_annotations` | 1 | ok |
| `the_supported_output_schema_describes_a_production_body` | `mcp::tests::the_supported_output_schema_describes_a_production_body` | 1 | ok |
| `a_caller_that_keeps_to_the_published_quota_is_never_refused` | `rate_policy::tests::a_caller_that_keeps_to_the_published_quota_is_never_refused` (antes `rate_limit::tests`) | 2 | ok |
| `a_mount_is_recorded_once_however_often_it_is_governed` | `rate_policy::tests::a_mount_is_recorded_once_however_often_it_is_governed` (antes `rate_limit::tests`) | 2 | ok |
| `a_token_bucket_publishes_its_burst_over_the_time_it_takes_to_refill` | `rate_policy::tests::a_token_bucket_publishes_its_burst_over_the_time_it_takes_to_refill` (antes `rate_limit::tests`) | 2 | ok |
| `an_unkeyed_request_names_the_policy_and_reports_no_remaining` | `rate_policy::tests::an_unkeyed_request_names_the_policy_and_reports_no_remaining` (antes `rate_limit::tests`) | 2 | ok |
| `one_bucket_on_two_doors_is_one_budget_under_one_name` | `rate_policy::tests::one_bucket_on_two_doors_is_one_budget_under_one_name` (antes `rate_limit::tests`) | 2 | ok |
| `sub_second_periods_round_up_and_never_to_zero` | `rate_policy::tests::sub_second_periods_round_up_and_never_to_zero` (antes `rate_limit::tests`) | 2 | ok |
| `a_commit_in_the_environment_is_reported` | `version::tests::a_commit_in_the_environment_is_reported` | 2 | ok |
| `anything_that_is_not_a_commit_falls_back_to_the_placeholder` | `version::tests::anything_that_is_not_a_commit_falls_back_to_the_placeholder` | 2 | ok |

### 3.2 Tests nuevos de la unión (`rate_policy::tests`)

| Test | Qué fija |
|---|---|
| `a_stack_identity_is_told_no_budget_and_a_third_party_its_own` | (b). Recorre los ocho presupuestos: el tercero recibe `"<nombre>";q=<burst>;w=<ventana>` y `r=burst-1` en el `200`, y la misma política en el `429`. El exento, más allá del burst, recibe `x-ratelimit-exempt` y ninguna de `ratelimit-policy`, `ratelimit`, `x-ratelimit-limit` ni `x-ratelimit-remaining` |
| `the_config_document_and_the_interop_manifest_publish_the_same_budgets` | (c). `/config` y `rate_limits` son el mismo conjunto (puerta, `q`, `w`), cada nombre de `/config` es el de su cabecera y ninguno de los dos trae una clave ni un digest |
| `every_budget_name_is_a_plain_structured_field_string` | Los nombres entran sin escapar en un sf-string, y el `Limit` de cada presupuesto lleva su nombre |
| `the_mcp_door_draws_on_the_verify_settle_bucket` | `main.rs` monta `/mcp` sobre `verify_settle_config` en la puerta `mcp` |
| `a_browser_can_read_the_ratelimit_headers` | CORS expone `ratelimit-policy`, `ratelimit` y `retry-after`. Cierra el P3-2 de la refutación de INT-09 (su R11 sobrevivía) |

## 4. Mutaciones

Una o más por conflicto resuelto; cada una deshace la resolución (o la decisión que la sostiene) y tiene que dar
rojo. El JSON está también en `docs/handoffs/TANDA-C3.mutaciones.json`, listo para el runner:

```bash
python3 scripts/verificar_ronda.py --repo <clon> --sha <punta de 0xultravioleta/tanda-c3> \
  --suite 'cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl,hedera -- --test-threads=1' \
  --mutaciones docs/handoffs/TANDA-C3.mutaciones.json
```

Qué deshace cada una:

| # | Conflicto o decisión | Qué deshace | Qué la mata |
|---|---|---|---|
| T3-01 | (b) | el exento recibe `RateLimit-Policy`/`RateLimit` | `a_stack_identity_is_told_no_budget_and_a_third_party_its_own` |
| T3-02 | (a) | la capa deja de poner las cabeceras | `a_caller_that_keeps_to_the_published_quota_is_never_refused` |
| T3-03 | (a) | las cabeceras solo en el `200` (el equivalente de la R1 de INT-09) | el mismo |
| T3-04 | (a) | las cabeceras salen de otra fuente que el bucket | `a_stack_identity_is_told_no_budget_and_a_third_party_its_own` |
| T3-05 | (c) | `/config` publica otro nombre que la cabecera | `the_config_document_and_the_interop_manifest_publish_the_same_budgets` |
| T3-06 | (c) | el manifiesto calcula la ventana distinto | el mismo |
| T3-07 | traducción de INT-09 | `w` redondeado hacia abajo (su R2, ahora en `rate_policy`) | `sub_second_periods_round_up_and_never_to_zero` |
| T3-08 | `main.rs` | `/mcp` fuera de la puerta `mcp` | `the_mcp_door_draws_on_the_verify_settle_bucket` |
| T3-09 | `main.rs` | CORS sin `ratelimit-policy` | `a_browser_can_read_the_ratelimit_headers` |
| T3-10 | `main.rs` | un número duplicado en `main.rs`, al estilo de INT-09 | `every_governor_goes_through_the_policy` |
| T3-11 | `client_ip.rs` | el lado de INT-09 (el governor en `rate_limit.rs`) | `every_governor_keys_on_the_client_ip` |
| T3-12 | `handlers.rs` | escrituras ERC-8004 con un bucket que no exime al stack | `the_erc8004_writes_exempt_the_stack` |
| T3-13 | `handlers.rs` | páginas humanas con otro presupuesto que el que pasa `main` | `the_human_pages_exempt_the_stack_too` |
| T3-14 | `handlers.rs` | el test del bazar con los nombres de INT-09 | `production_mounts_the_writes_and_the_bazar_on_separate_budgets` |
| T3-15 | `Cargo.toml` | el lado de INT-09 (sin `http-body-util`) | `--locked`: el lock ya no cuadra |
| T3-16 | `Cargo.lock` | el lado de X4 (sin `jsonschema`) | `--locked`: el lock ya no cuadra |
| T3-17 | `src/lib.rs` | el lado de INT-09 (`pub mod rate_limit;`) | el tipo: no compila, `E0583` |
| T3-18 | `index.json` | el digest de X4 | `the_skills_index_digest_matches_skill_md` |
| T3-19 | fila del backlog | el icono de Arc tipeado a mano | `static_types_no_explorer_and_no_icon` |

T3-15 y T3-16 dan rojo porque `cargo test --locked` se niega a correr. No es un error de compilación, así que el
runner las cuenta como rojo, no como "no compila". T3-12 y T3-13 dejan un parámetro sin usar: es un aviso, no un
error.

```json
[
 {
  "nombre": "T3-01 el exento recibe RateLimit (decision b)",
  "archivo": "src/rate_policy.rs",
  "viejo": "                let bare = self.bare.clone();\n                Box::pin(async move {\n                    let mut response = bare.oneshot(request).await?;\n",
  "nuevo": "                let bare = self.bare.clone();\n                let limit = self.limit;\n                Box::pin(async move {\n                    let mut response = bare.oneshot(request).await?;\n                    limit.stamp(response.headers_mut());\n",
  "test": "cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl,hedera --lib rate_policy::tests::a_stack_identity_is_told_no_budget_and_a_third_party_its_own -- --exact --test-threads=1"
 },
 {
  "nombre": "T3-02 la capa deja de poner las cabeceras al tercero (decision a)",
  "archivo": "src/rate_policy.rs",
  "viejo": "                    let mut response = governed.oneshot(request).await?;\n                    limit.stamp(response.headers_mut());\n",
  "nuevo": "                    let mut response = governed.oneshot(request).await?;\n                    let _ = limit;\n",
  "test": "cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl,hedera --lib rate_policy::tests::a_caller_that_keeps_to_the_published_quota_is_never_refused -- --exact --test-threads=1"
 },
 {
  "nombre": "T3-03 cabeceras solo en el 200: el 429 del governor sale sin RateLimit-Policy (decision a)",
  "archivo": "src/rate_policy.rs",
  "viejo": "                    limit.stamp(response.headers_mut());\n                    Ok(response)",
  "nuevo": "                    if response.status().is_success() {\n                        limit.stamp(response.headers_mut());\n                    }\n                    Ok(response)",
  "test": "cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl,hedera --lib rate_policy::tests::a_caller_that_keeps_to_the_published_quota_is_never_refused -- --exact --test-threads=1"
 },
 {
  "nombre": "T3-04 las cabeceras salen de otra fuente que el bucket (decision a)",
  "archivo": "src/rate_policy.rs",
  "viejo": "            limit: bucket.limit,\n        }\n",
  "nuevo": "            limit: VERIFY_SETTLE.default_limit(),\n        }\n",
  "test": "cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl,hedera --lib rate_policy::tests::a_stack_identity_is_told_no_budget_and_a_third_party_its_own -- --exact --test-threads=1"
 },
 {
  "nombre": "T3-05 /config publica otro nombre que RateLimit-Policy (decision c)",
  "archivo": "src/rate_policy.rs",
  "viejo": "                \"name\": budget.name,\n",
  "nuevo": "                \"name\": budget.env_period_ms,\n",
  "test": "cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl,hedera --lib rate_policy::tests::the_config_document_and_the_interop_manifest_publish_the_same_budgets -- --exact --test-threads=1"
 },
 {
  "nombre": "T3-06 el manifiesto calcula la ventana distinto que /config (decision c)",
  "archivo": "src/interop.rs",
  "viejo": "mount.limit.window_s());",
  "nuevo": "mount.limit.reset_s());",
  "test": "cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl,hedera --lib rate_policy::tests::the_config_document_and_the_interop_manifest_publish_the_same_budgets -- --exact --test-threads=1"
 },
 {
  "nombre": "T3-07 w redondeado hacia abajo (R2 de INT-09, ahora en rate_policy)",
  "archivo": "src/rate_policy.rs",
  "viejo": "    let secs = nanos.div_ceil(1_000_000_000);\n",
  "nuevo": "    let secs = nanos / 1_000_000_000;\n",
  "test": "cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl,hedera --lib rate_policy::tests::sub_second_periods_round_up_and_never_to_zero -- --exact --test-threads=1"
 },
 {
  "nombre": "T3-08 main.rs: /mcp fuera de la puerta mcp (conflicto de main.rs)",
  "archivo": "src/main.rs",
  "viejo": "    .layer(policy.layer_on(&verify_settle_config, rate_policy::Door::Mcp));",
  "nuevo": "    .layer(policy.layer(&verify_settle_config));",
  "test": "cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl,hedera --lib rate_policy::tests::the_mcp_door_draws_on_the_verify_settle_bucket -- --exact --test-threads=1"
 },
 {
  "nombre": "T3-09 main.rs: CORS deja de exponer ratelimit-policy (conflicto de main.rs)",
  "archivo": "src/main.rs",
  "viejo": "                        \"ratelimit-policy\",\n",
  "nuevo": "",
  "test": "cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl,hedera --lib rate_policy::tests::a_browser_can_read_the_ratelimit_headers -- --exact --test-threads=1"
 },
 {
  "nombre": "T3-10 main.rs: un numero duplicado, al estilo de INT-09 (conflicto de main.rs)",
  "archivo": "src/main.rs",
  "viejo": "rate_policy::config(rate_policy::VERIFY_SETTLE.limit())",
  "nuevo": "rate_policy::config(rate_policy::Limit::every_ms(2_000, 30))",
  "test": "cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl,hedera --lib rate_policy::tests::every_governor_goes_through_the_policy -- --exact --test-threads=1"
 },
 {
  "nombre": "T3-11 client_ip.rs: el lado de INT-09 (el governor vive en rate_limit.rs)",
  "archivo": "src/client_ip.rs",
  "viejo": "                if file != \"rate_policy.rs\" {",
  "nuevo": "                if file != \"rate_limit.rs\" {",
  "test": "cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl,hedera --lib client_ip::tests::every_governor_keys_on_the_client_ip -- --exact --test-threads=1"
 },
 {
  "nombre": "T3-12 handlers.rs: escrituras ERC-8004 con un bucket que no exime al stack (lado de INT-09)",
  "archivo": "src/handlers.rs",
  "viejo": "    policy.govern(routes, crate::rate_policy::ERC8004_WRITES.limit())",
  "nuevo": "    crate::rate_policy::RatePolicy::none().govern(routes, crate::rate_policy::ERC8004_WRITES.limit())",
  "test": "cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl,hedera --lib handlers::erc8004_write_rate_tests::the_erc8004_writes_exempt_the_stack -- --exact --test-threads=1"
 },
 {
  "nombre": "T3-13 handlers.rs: paginas humanas con otro presupuesto que el que les pasa main",
  "archivo": "src/handlers.rs",
  "viejo": "    policy.govern(human_page_routes(), limit)",
  "nuevo": "    policy.govern(human_page_routes(), crate::rate_policy::IDENTITY_READ.limit())",
  "test": "cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl,hedera --lib rate_policy::tests::the_human_pages_exempt_the_stack_too -- --exact --test-threads=1"
 },
 {
  "nombre": "T3-14 handlers.rs: el test del bazar con los nombres de INT-09",
  "archivo": "src/handlers.rs",
  "viejo": "statement_after(\"let discovery_register_config\")",
  "nuevo": "statement_after(\"let discovery_register_limit\")",
  "test": "cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl,hedera --lib handlers::erc8004_write_rate_tests::production_mounts_the_writes_and_the_bazar_on_separate_budgets -- --exact --test-threads=1"
 },
 {
  "nombre": "T3-15 Cargo.toml: el lado de INT-09 (sin http-body-util)",
  "archivo": "Cargo.toml",
  "viejo": "http-body-util = { version = \"0.1\" }\n",
  "nuevo": "",
  "test": "cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl,hedera --lib rate_policy::tests::a_body_past_the_limit_is_a_json_413 -- --exact --test-threads=1"
 },
 {
  "nombre": "T3-16 Cargo.lock: el lado de X4 (sin jsonschema)",
  "archivo": "Cargo.lock",
  "viejo": " \"http-body-util\",\n \"jsonschema\",\n",
  "nuevo": " \"http-body-util\",\n",
  "test": "cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl,hedera --lib interop::tests::the_manifest_validates_against_the_vendored_schema -- --exact --test-threads=1"
 },
 {
  "nombre": "T3-17 lib.rs: el lado de INT-09 (pub mod rate_limit)",
  "archivo": "src/lib.rs",
  "viejo": "pub mod rate_policy;\n",
  "nuevo": "pub mod rate_limit;\n",
  "test": "cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl,hedera --lib rate_policy::tests::every_governor_goes_through_the_policy -- --exact --test-threads=1",
  "espera": "no_compila",
  "error": "E0583"
 },
 {
  "nombre": "T3-18 agent-skills/index.json: el digest de X4",
  "archivo": "static/.well-known/agent-skills/index.json",
  "viejo": "sha256:680f2afca60c91fd55f1f566a3939878a559ad559f8f2cfbe8a734ea09d92553",
  "nuevo": "sha256:ce60879b7f35cf390de82e7eb5ee3e5e6f366c2e6d90f6a9367c5ea43a51839f",
  "test": "cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl,hedera --lib handlers::agentic_surface_tests::the_skills_index_digest_matches_skill_md -- --exact --test-threads=1"
 },
 {
  "nombre": "T3-19 static/index.html: el icono de Arc tipeado a mano (la linea de la fila del backlog)",
  "archivo": "static/index.html",
  "viejo": "<img data-net-icon=\"arc\" alt=\"\" style=\"width: 20px; height: 20px; border-radius: 50%;\"",
  "nuevo": "<img src=\"/arc.png\" alt=\"\" style=\"width: 20px; height: 20px; border-radius: 50%;\"",
  "test": "cargo test --locked -p x402-rs --features solana,near,stellar,algorand,sui,xrpl,hedera --lib networks_json::tests::static_types_no_explorer_and_no_icon -- --exact --test-threads=1"
 }
]
```

### Corrida del runner (mía, sobre `446181aa`)

```
$ python3 scripts/verificar_ronda.py --repo <este clon> --sha 446181aa --suite '<la suite del CI>' --mutaciones TANDA-C3.mutaciones.json
suite: rc=0 (140s)    Doc-tests x402_rs
T3-01 el exento recibe RateLimit (decision b): ROJO (atrapada) rc=101 (24s) error: test failed, to rerun pass `-p x402-rs --lib`
T3-02 la capa deja de poner las cabeceras al tercero (decision a): ROJO (atrapada) rc=101 (25s) error: test failed, to rerun pass `-p x402-rs --lib`
T3-03 cabeceras solo en el 200: el 429 del governor sale sin RateLimit-Policy (decision a): ROJO (atrapada) rc=101 (25s) error: test failed, to rerun pass `-p x402-rs --lib`
T3-04 las cabeceras salen de otra fuente que el bucket (decision a): ROJO (atrapada) rc=101 (24s) error: test failed, to rerun pass `-p x402-rs --lib`
T3-05 /config publica otro nombre que RateLimit-Policy (decision c): ROJO (atrapada) rc=101 (18s) error: test failed, to rerun pass `-p x402-rs --lib`
T3-06 el manifiesto calcula la ventana distinto que /config (decision c): ROJO (atrapada) rc=101 (15s) error: test failed, to rerun pass `-p x402-rs --lib`
T3-07 w redondeado hacia abajo (R2 de INT-09, ahora en rate_policy): ROJO (atrapada) rc=101 (20s) error: test failed, to rerun pass `-p x402-rs --lib`
T3-08 main.rs: /mcp fuera de la puerta mcp (conflicto de main.rs): ROJO (atrapada) rc=101 (20s) error: test failed, to rerun pass `-p x402-rs --lib`
T3-09 main.rs: CORS deja de exponer ratelimit-policy (conflicto de main.rs): ROJO (atrapada) rc=101 (15s) error: test failed, to rerun pass `-p x402-rs --lib`
T3-10 main.rs: un numero duplicado, al estilo de INT-09 (conflicto de main.rs): ROJO (atrapada) rc=101 (15s) error: test failed, to rerun pass `-p x402-rs --lib`
T3-11 client_ip.rs: el lado de INT-09 (el governor vive en rate_limit.rs): ROJO (atrapada) rc=101 (15s) error: test failed, to rerun pass `-p x402-rs --lib`
T3-12 handlers.rs: escrituras ERC-8004 con un bucket que no exime al stack (lado de INT-09): ROJO (atrapada) rc=101 (16s) error: test failed, to rerun pass `-p x402-rs --lib`
T3-13 handlers.rs: paginas humanas con otro presupuesto que el que les pasa main: ROJO (atrapada) rc=101 (17s) error: test failed, to rerun pass `-p x402-rs --lib`
T3-14 handlers.rs: el test del bazar con los nombres de INT-09: ROJO (atrapada) rc=101 (16s) error: test failed, to rerun pass `-p x402-rs --lib`
T3-15 Cargo.toml: el lado de INT-09 (sin http-body-util): ROJO (atrapada) rc=101 (0s) help: to generate the lock file without accessing the network, remove the --locked flag and use --offline instead.
T3-16 Cargo.lock: el lado de X4 (sin jsonschema): ROJO (atrapada) rc=101 (0s) help: to generate the lock file without accessing the network, remove the --locked flag and use --offline instead.
T3-17 lib.rs: el lado de INT-09 (pub mod rate_limit): NO COMPILA (atrapada por el tipo) rc=NO-COMPILA (24s) error[E0583]: file not found for module `rate_limit` [codigos: E0432,E0433,E0583]
T3-18 agent-skills/index.json: el digest de X4: ROJO (atrapada) rc=101 (19s) error: test failed, to rerun pass `-p x402-rs --lib`
T3-19 static/index.html: el icono de Arc tipeado a mano (la linea de la fila del backlog): ROJO (atrapada) rc=101 (19s) error: test failed, to rerun pass `-p x402-rs --lib`
arbol limpio al final: True
VEREDICTO: todo como pide la ronda
exit=0
```

## 5. Pre-CI, con la red cerrada

Con `HTTPS_PROXY=http://127.0.0.1:9 HTTP_PROXY=http://127.0.0.1:9 NO_PROXY=127.0.0.1,localhost`,
`AWS_SHARED_CREDENTIALS_FILE=/dev/null`, `AWS_CONFIG_FILE=/dev/null` y
`SWAGGER_UI_DOWNLOAD_URL=file://<swagger-ui.zip local>`. Checkout en LF (`core.autocrlf=false`), `CARGO_TARGET_DIR`
propio, toolchain `cargo 1.98.1`. Disco antes de compilar: 76 GiB libres. Las features son las de `ci.yaml`:
`solana,near,stellar,algorand,sui,xrpl,hedera`. `CLAUDE.md` nombra seis y `ci.yaml` suma `hedera`.

| Paso | Resultado |
|---|---|
| `python3 scripts/verify_landing_canonical.py --offline` | `[OK] landing page matches /supported, escrow, and ERC-8004 sources.` |
| `node --test tests/frontend-capabilities.test.cjs` | 19 tests, 19 pass, 0 fail |
| `python3 -m unittest discover -s tests/scripts -p 'test_*balances.py'` | `Ran 5 tests` · `OK` |
| `cargo build --locked --features …` | exit 0 |
| `cargo test --locked -p x402-rs --features … -- --test-threads=1` | **exit 0**: lib 1415 passed / 0 failed / 11 ignored; bin 1479 / 0 / 11; integración 6 + 24 + 3 + 6 + 1 + 1 + 9 + 15, 0 failed; doctests 1 passed / 11 ignored |
| `cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance -- --test-threads=1` | **exit 0**: 31, 1, 3, 14, 43, 7, 5 (+7 ignored), 0 (+2 ignored), 5; 0 failed |
| `cargo fmt --all -- --check` | **exit 1, igual que en `main`.** Unión: 82 bloques `Diff in`. `main` (`f3786f3e`, mismo comando sobre `git archive`): 83. Por archivo, la única diferencia es `main.rs`, que baja de 4 a 3 (INT-09). `rate_policy.rs`, `interop.rs` y `client_ip.rs` están limpios (`rustfmt --check`). La tanda no agrega ningún hunk |
| `cargo clippy --locked -p x402-rs --all-targets --features … -- -D warnings` | **exit 101, igual que en `main`**: `could not compile x402-rs (lib test) due to 131 previous errors`, todos avisos previos promovidos a error. Con `--message-format=json` sin `-D`: `main` 327 avisos en `src/`, unión 328. La diferencia por (archivo, mensaje) es `+ path_config is never used` (X4), `+ path_uvd_stack is never used` (INT-09) y `- fields protocol_fee_config and refund_request are never read` (los tests de Arc de X-1 los leen). Los dos `path_*` son el falso positivo de utoipa de los otros 68 `path_*` de `main`. Uno mío (`Limit::name()`, solo usado en tests) apareció y se quitó |

Por qué no están verdes fmt y clippy: `main` ya da rojo en los dos y `ci.yaml` no corre ninguno. Ponerlos en verde
es formatear o limpiar decenas de archivos fuera de la tanda, y la regla del repo es no formatear el árbol entero. Lo
que sí vale es que la tanda no suma ni un hunk de fmt, y de clippy solo los dos `path_*` de utoipa.

## 6. Para c0der

### 6.1 Orden

1. **Mutaciones** con el runner (§4) sobre la punta de la rama.
2. **Push a `main`** cuando lo digas: un CI y un deploy. El facilitador sale en 2.43.0 **sin ninguna variable
   `UVD_STACK_*`** (paso 1 de §8 de `docs/handoffs/X4-STACK-429.md`): nadie es exento y los presupuestos tienen los
   mismos números de hoy. Lo nuevo para todos es la admisión de X4 (32 en vuelo por dirección, cuerpo en 5 s, 512
   por task), `/config`, `/.well-known/uvd-stack.json`, las cabeceras `RateLimit` y el escrow en Arc (anunciado solo
   cuando el operador se autoverifique). `FACILITATOR_GIT_SHA` lo pasa el CI (`github.sha`), según el cambio de
   INT-09 en `ci.yaml`. Terraform no cambia.
3. Después, los pasos 2 a 5 de §8 de X4 (claves, SDK, clientes, KK), cada uno con su deploy de facilitador o de
   cliente.

### 6.2 Sondas de solo lectura, después del deploy

GET públicos, en serie, con 2 s entre uno y otro. Ninguno toca cadena ni necesita credenciales.

```bash
F=https://facilitator.ultravioletadao.xyz

# Versión
curl -s $F/version                       # -> "2.43.0"

# X4: la política en vigor, sin identidades todavía
curl -s $F/config | jq '.stackIdentities.active, [.rateLimits.budgets[]|{name,periodMs,burst}], (.overload|{maxInflightRequests, perClient: .perClient.maxInflightRequests, bodyDeadlineMs})'
# -> 0; ocho presupuestos con los nombres NUEVOS (verify-settle 2000/30, discovery-register 12000/250,
#    discovery-read 200/120, events 2000/10, identity-read 500/60, secondary-read 300/100, human-pages 500/60,
#    erc8004-writes 12000/30); 512 / 32 / 5000

# INT-09: el manifiesto, del build desplegado
curl -s $F/.well-known/uvd-stack.json | jq '.version, .git_sha, .rate_limits'
# -> "2.43.0"; git_sha = el commit desplegado (0000000 = el build-arg no llegó);
#    ocho entradas: api 30/60, mcp 30/60, api 250/3000, api 120/24, api 10/20, api 60/30, api 100/30, api 30/360

# (c): /config y el manifiesto publican los mismos presupuestos -> true
C=$(curl -s $F/config); sleep 2; M=$(curl -s $F/.well-known/uvd-stack.json)
jq -n --argjson c "$C" --argjson m "$M" '
  ([$c.rateLimits.budgets[] | {limit: .burst, window_s: (((.periodMs * .burst) + 999) / 1000 | floor)}] | unique)
  == ([$m.rate_limits[] | select(.applies_to == "api") | {limit, window_s}] | unique)'

# INT-09 + (b): un tercero recibe la política de su presupuesto (y NO x-ratelimit-exempt)
curl -s -o /dev/null -D - $F/health/ready | grep -i -E '^(ratelimit|x-ratelimit)'
# -> ratelimit-policy: "secondary-read";q=100;w=30   ratelimit: "secondary-read";r=<n>;t=1   x-ratelimit-limit: 100 ...

# INT-09: MCP (una llamada: gasta 1 ficha del bucket de verify/settle)
curl -s -D /dev/stderr -X POST $F/mcp -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' \
  | jq -c '.result.tools[] | {name, clase: ._meta["uvd/clase"], outputSchema: (.outputSchema != null)}'
# -> x402_supported lectura true; x402_accepts y x402_verify riel_de_pago false; x402_settle mueve_dinero false
#    y, en stderr, ratelimit-policy: "verify-settle";q=30;w=60

# X-1: Arc en /supported. Mientras el operador no tenga código (C-2 no corrió): exact y commerce, SIN escrow.
curl -s $F/supported | jq -c '[.kinds[] | select(.network == "arc" or .network == "arc-testnet" or .network == "eip155:5042" or .network == "eip155:5042002") | {network, scheme}] | unique'
# X-1: /docs lista las 13 redes de escrow, arc y arc-testnet incluidas
curl -s $F/api-docs/openapi.json | jq -r '.paths["/supported"].get.description' | grep '^\*\*Escrow networks'
# X-1 + la fila del backlog: Arc en la grilla de escrow de la portada, con su icono desde /networks.json
curl -s $F/ | grep -c 'data-net-icon="arc"'        # -> 3 (en main hoy: 2)
```

Cuando el operador de Arc tenga código (C-2), la entrada `escrow` de Arc aparece sola en `/supported`, a lo sumo 10
minutos después (el refresco de la autoverificación), sin deploy.

Durante los primeros días, `status=408`, `status=429` con `too_many_concurrent_requests` y `status=503` en el log
dicen si la admisión de X4 le corta a alguien legítimo (§8 de X4).

### 6.3 Qué cambia respecto de lo que cada pieza prometía por separado

- **Los nombres de `/config`** pasan a llevar guion (`verify-settle` en vez de `verify_settle`), para ser los mismos
  de la cabecera. La sonda de X4 no los fijaba.
- **INT-27b** re-declara la misma fila que dijo la refutación de INT-09: `facilitador / x402_supported`, huella
  `sha256:dda236c3056f7ead65ebdd99efb5f7c23b8958781446836f041aee0d92948db4`. X4 no toca las tools, así que la unión
  no la mueve.
- **Un exento no ve `RateLimit`.** La frase "every response carries RateLimit-Policy" de `mcp.md` vale para
  terceros. OpenAPI ya lo dice. `mcp.md` no nombra el stack, igual que en X4.

## 7. Notas

- **Versión.** La más alta de las tres era 2.42.0 (X4; X-1 e INT-09 dejaban 2.40.0), así que la siguiente minor es
  **2.43.0**. 2.42.0 existió solo en la rama local de X4 y nunca se publicó. Si preferís no saltarla, cambiá
  `VERSION` y el encabezado del CHANGELOG: son dos líneas.
- **CHANGELOG.** El encargo dice `docs/CHANGELOG.md`, pero ese archivo está congelado en 2.31.0 (H5 de la refutación
  de networks-json: "nadie lo lee"). Las entradas de 2.32 en adelante, incluida la de X4, viven en el `CHANGELOG.md`
  de la raíz. La entrada de la tanda va ahí: una sección 2.43.0 con tres subsecciones, una por pieza. No toqué
  `docs/CHANGELOG.md`.
- **P3 de la refutación de INT-09.** El P3-2 (CORS sin test) queda cerrado. Los P3-1, P3-3, P3-4, P3-5, P3-6 (el
  `429` del tope diario lleva `RateLimit: "erc8004-writes";r>0`), P3-7 y P3-8 siguen igual: la unión no los mueve.
- **P3 de las refutaciones de X-1** (tests de `/verify` en Arc, M21, orden gate/código con `enforce`, paralelismo):
  siguen igual. La ronda 2 de X-1 decía "C-3 no sale antes de C-2"; el encargo de esta tanda dice que sí puede
  (plan de Arc, línea 200), y así queda: el anuncio aparece solo.
- **Red.** Nada de esto llamó a producción: todas las cifras de §5 son locales.
