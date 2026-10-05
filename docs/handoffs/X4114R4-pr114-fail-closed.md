# X4114R4 - PR #114, headers presentes fail-closed

**Estado:** fail-closed implementado, workspace/preflight/clippy offline verdes y
20 mutaciones distintas muertas. Entrega lista en la rama existente para revisión
de c0der en [PR #114](https://github.com/UltravioletaDAO/x402-rs/pull/114). Próximo
paso de c0der: refutar antes de decidir el merge. Esta ronda no mergea ni despliega.

## Cambio

Solo `src/discovery_health.rs` y este handoff. La presencia del header ya no se
confunde con ausencia: `get_all` de ambos aliases, `as_bytes`, bytes obs-text
preservados y todas las líneas retenidas. Vacíos, ilegibles, multilínea, challenges
sin opciones utilizables o lecturas distintas activan `LiveTerms::untrusted_header`.
Eso es `Recipients::Drifted` y cuarentena inmediata, incluso sin destinatarios
catalogados. El body nunca puede rehabilitar ese header.

Se comparan JSON directo, string JSON desenvuelto, base64 Node forgiving existente
y Python no estricto (ruido descartado, padding intermedio saltado, padding final
requerido). No hay dependencias nuevas. Para una lectura decodificada que no admite
`serde_json::Value`, `IgnoredAny` distingue sintaxis completa de basura/truncamiento.
BOM/NUL y sintaxis permisiva de surrogates/non-finite llevan a cuarentena; el texto
transformado **jamás** se usa como términos de pago. Los controles de R3 (11 grafías,
incluyendo URL-safe, sin padding e invalid-UTF8 leído lossily por Node) permanecen
saludables. Los tests de cuarentena ejercitan también `probe_once` y `probe_and_record`.

## Verificación

Base medida `origin/main` = `f544fa0c`; rama inicial `f5473ba8`. Fetch/merge final:
`Already up to date`. Base: 3593 passed, 0 failed, 44 ignored (26 grupos).
Rojos de base de la suite: ninguno. No fallos nuevos.

Workspace, después del fetch/merge: **3699 passed, 0 failed, 44 ignored**, 26 grupos.
El último grupo de doctests: `1 passed; 0 failed; 11 ignored`.

```sh
cargo test --locked --offline --workspace --features solana,near,stellar,algorand,sui,xrpl,hedera -j 4 -- --test-threads=1
```

Todos los pasos se ejecutan dentro de `unshare -rn`, habilitando solo loopback,
con `HTTPS_PROXY=http://127.0.0.1:9 HTTP_PROXY=http://127.0.0.1:9`
y `NO_PROXY=127.0.0.1,localhost`. Cargo offline, jobs=4, perfiles dev/test sin debug;
Swagger UI usa el zip local previamente disponible. Node v22.23.3, Python 3.10.12,
Rust stable 1.97.1. No se ejecutan integraciones contra producción ni pasos de deploy.

Focused: 115 passed, 0 failed (`--lib discovery_health`, mismas features).
Python 3.10.12 lee B en los 4 casos NaN/Infinity/surrogate; Node 22.23.3 rechaza los 4.
Los tests Rust esperan `Drifted` y no que el wrapper con A termine `Declared`.

## Auto-refutación

Bordes: ausencia/vacío/null, JSON sin challenge, padding ausente/intermedio/separado,
ruido, JSON entre comillas y escapes, bytes obs-text/unicode, aliases/case HTTP,
duplicados entre y dentro de aliases, baseline vacío, `0x`/`0X` y espacios EVM,
JSON divergente, profundidad/surrogates/non-finite/BOM/UTF-16.

20 mutaciones distintas, 22 ejecuciones (M1/M3 repetidas en el parche posterior).
Todas exit 101 con tests fallidos, nunca errores de compilación. Árbol limpio antes
de mutar y restaurado con `git checkout -- src/discovery_health.rs` después de cada
mutación. Tabla generada de los resultados locales; se cita un test representativo.

| ID | Mutación | Test en rojo |
|---|---|---|
| M1 | JSON early return | `discovery_health::ver3_refutation_tests::alternate_json_rejected_by_serde_cannot_be_hidden` |
| M2 | first header plus to_str | `discovery_health::ver3_refutation_tests::all_header_aliases_preserve_presence_and_raw_bytes` |
| M3 | remove Python reading | `discovery_health::ver3_refutation_tests::sane_json_and_base64_headers_remain_declared` |
| M4 | unreadable header becomes absent | `discovery_health::ver3_refutation_tests::all_header_aliases_preserve_presence_and_raw_bytes` |
| M5 | ignore multiple header lines | `discovery_health::ver3_refutation_tests::duplicate_headers_cannot_be_hidden_by_a_valid_first_line` |
| M6 | ignore obs-text | `discovery_health::ver3_refutation_tests::all_header_aliases_preserve_presence_and_raw_bytes` |
| M7 | remove Drifted guard | `discovery_health::ver3_refutation_tests::all_header_aliases_preserve_presence_and_raw_bytes` |
| M8 | skip drift without a baseline | `discovery_health::ver3_refutation_tests::unreadable_headers_are_quarantined_even_without_declared_recipients` |
| M9 | drop non-ASCII lines using to_str | `discovery_health::ver3_refutation_tests::all_header_aliases_preserve_presence_and_raw_bytes` |
| M10 | Python stops at first padding | `discovery_health::ver3_refutation_tests::python_padding_rules_are_independent_of_the_node_reading` |
| M11 | discard errors from both decoded JSON readings | `discovery_health::ver3_refutation_tests::alternate_json_rejected_by_serde_cannot_be_hidden` |
| M12 | ignore syntactically valid JSON beyond Value limits | `discovery_health::ver3_refutation_tests::alternate_json_rejected_by_serde_cannot_be_hidden` |
| M13 | ignore BOM reading | `discovery_health::ver3_refutation_tests::alternate_json_rejected_by_serde_cannot_be_hidden` |
| M14 | ignore UTF16/32 reading | `discovery_health::ver3_refutation_tests::alternate_json_rejected_by_serde_cannot_be_hidden` |
| M15 | do not unwrap JSON strings | `discovery_health::ver3_refutation_tests::escaped_quoted_base64_is_unwrapped_and_remains_declared` |
| M16 | trust first reading without comparing alternatives | `discovery_health::ver3_refutation_tests::alternate_json_rejected_by_serde_cannot_be_hidden` |
| M17 | accept header challenges with no usable option | `discovery_health::declared_method_tests::a_402_verifies_only_the_listings_own_answer_offering_a_payment` |
| M18 | Python accepts incomplete unpadded quad | `discovery_health::ver3_refutation_tests::python_padding_rules_are_independent_of_the_node_reading` |
| M19 | ignore nonfinite JSON tokens | `discovery_health::ver3_refutation_tests::python_nonfinite_and_raw_surrogates_cannot_hide_behind_declared_json` |
| M20 | ignore raw surrogate UTF8 sequences | `discovery_health::ver3_refutation_tests::python_nonfinite_and_raw_surrogates_cannot_hide_behind_declared_json` |

Revisión independiente: dos rondas, child `normal` (la herramienta disponible no
ofrecía selector `opus`). La primera encontró lecturas válidas para clientes pero
no para Value; la segunda, NaN/Infinity y surrogates crudos de Python. Quedaron
fijados en `alternate_json_rejected_by_serde_cannot_be_hidden` y
`python_nonfinite_and_raw_surrogates_cannot_hide_behind_declared_json`, con mutaciones
M11-M14 y M19-M20. No se afirma una tercera revisión ni un veredicto nuevo del child.

## Mediciones que corrigen supuestos

- `arbitrary_precision` está activo con las features reales: `1e400` sí parsea;
  se juzga como lectura discrepante. El harness mínimo del child lo rechazaba.
- Una heurística de mero prefijo JSON rechazaba URL-safe/mid-padding sanos. Se
  reemplazó por validación de sintaxis completa; los controles sanos pasan.
- Escaneo genérico hex64 de `origin/main...HEAD`: dos matches ya existían en
  `f5473ba8` (dirección pública del coin type USDC Sui y destinatario ficticio de
  tests). En el diff de esta ronda: cero. No son llaves ni se agregó un allowlist;
  no se modificó ninguna política ni hook. Los matches no se reproducen aquí.
- `cargo fmt --all -- --check` también alcanza formato preexistente ajeno al cambio;
  se usó `rustfmt --edition 2021 --check src/discovery_health.rs`, sin churn global.

## Jobs y gates locales

Todos exit 0 en el mismo aislamiento de red indicado arriba. El script
`bash scripts/preflight.sh` se ejecutó completo con `CARGO_BUILD_JOBS=4` y
`CARGO_NET_OFFLINE=true`, sin modificarlo.

| Job/gate | Comando exacto | Resultado |
|---|---|---|
| Landing | `python3 scripts/verify_landing_canonical.py --offline` | exit 0 |
| Frontend (Node 22) | `node --test tests/frontend-capabilities.test.cjs` | 19 passed |
| Balances | `python3 -m unittest discover -s tests/scripts -p 'test_*balances.py'` | 20 tests OK |
| ERC-8004 | `python3 -m unittest discover -s tests/scripts -p 'test_erc8004_*.py'` | 19 tests OK |
| Drift gate | `python3 -m unittest discover -s tests/scripts -p 'test_drift_gate_*.py'` | 29 tests OK |
| CI scripts | `python3 -m unittest discover -s tests/scripts -p 'test_ci_*.py'` | 5 tests OK |
| Build (preflight) | `cargo build --locked --features "$features"` | exit 0 |
| Root (preflight) | `cargo test --locked -p x402-rs --features "$features" -- --test-threads=1` | exit 0 |
| Crates (preflight) | `cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance -- --test-threads=1` | exit 0 |
| Workspace | `cargo test --locked --offline --workspace --features solana,near,stellar,algorand,sui,xrpl,hedera -j 4 -- --test-threads=1` | 3699 passed; 0 failed; 44 ignored |
| Clippy | `cargo clippy --locked --offline --workspace --all-targets --features solana,near,stellar,algorand,sui,xrpl,hedera -j 4` | exit 0, con warnings |
| Formato del archivo tocado | `rustfmt --edition 2021 --check src/discovery_health.rs` | exit 0 |
| Diff | `git diff --check` | exit 0 |
| Hooks | `sh .githooks/pre-commit` | exit 0 en cada commit, sin bypass |

`features=solana,near,stellar,algorand,sui,xrpl,hedera` es la variable literal del
preflight. El run de `no-account-id.yml` también terminó en 0. El formato del
`src/types.rs` de `origin/main`, verificado por separado, da rustfmt exit 1: no se
corrige formato ajeno en esta ronda. Clippy no equivale a cero warnings.

Límites: solo repo asignado, cero secretos leídos/escritos, cero cambios de
workflows, dependencias, producción o infraestructura; un único push planificado
a la rama existente, ningún PR nuevo, merge, deploy ni llamada al servicio
prohibido. No hay preguntas de diseño abiertas.
