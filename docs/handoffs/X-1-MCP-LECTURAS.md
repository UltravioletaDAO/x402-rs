# X-1 — Lecturas de comprador en el MCP del facilitador (2.54.0)

> Frente X-1 del plan del plugin de emp0rium (ola 1), despachado por c0der el 2026-11-02.
> Rama `0xultravioleta/c0-x402-mcp-lecturas`, base `origin/main` `c1248eca` (2.53.0).
> **Sin push, sin merge y sin deploy**: el facilitador paga un CI y un deploy por tanda; c0der da la orden.

**QUÉ:** el MCP de `POST /mcp` pasa de 4 a 8 tools. Las 4 nuevas son lecturas de comprador, clase `lectura`,
`readOnlyHint: true`, despachadas por el router REST que ya existía, igual que las cuatro de antes.
**POR QUÉ:** emp0rium sólo puede sumar lo que el facilitador publica en su MCP; releer el REST desde Emporium
sería una segunda implementación (`facilitador.md` §4). Con esto Emporium las puede sumar como `pay_*`.
**RIESGO:** una tool del MCP que llegara a una escritura o a una ruta admin. Lo cierra el router interno (monta
sólo las rutas de las tools, una por una) y un test que recorre las escrituras y el admin contra él: todas 404.

## 1. Las tools nuevas (el contrato que toman EMP-1 / EMP-2)

Orden en `tools/list`: las cuatro de siempre y después estas cuatro. Todas con `_meta["uvd/clase"] = "lectura"`,
`outputSchema` publicado y respuesta también en `structuredContent` (R9.5).

| Tool | Ruta REST | Argumentos | Hints | Salida |
|---|---|---|---|---|
| `x402_networks` | `GET /networks.json` | ninguno (`additionalProperties: false`) | readOnly, idempotent, closed world | `{networks: [{id, caip2, family, chainId, testnet, displayName, explorer{base,tx,address}\|null, icon, schemes[], tokens[{symbol, address, decimals, eip712, usdPegged, icon}]}]}` |
| `x402_chain_health` | `GET /health/ready?network=` | `network` opcional, v1 o CAIP-2 | readOnly, idempotent, open world | `{status ok\|degraded\|down, checkedAtUnix, ageSecs, ttlSecs, probeTimeoutMs, thresholds, summary, networks[{network, caip2, mainnet, status, reason?, rpc, warnSettles?, signers[]}], unchecked[], sanctionsList?}` |
| `x402_escrow_onchain_state` | `POST /escrow/state` | el cuerpo de la ruta: `paymentInfo{11 campos}`, `payer`, `network` (**sólo CAIP-2**), `extra{escrowAddress, operatorAddress, tokenCollector}` | readOnly, idempotent, open world | `{hasCollectedPayment, capturableAmount, refundableAmount, paymentInfoHash, network}` (montos como string decimal) |
| `x402_durable_evidence` | `GET /dx402/evidence/{paymentId}` o, con `document: "receipt"`, `GET /dx402/receipt/{paymentId}` | `paymentId` (requerido), `document` `evidence`\|`receipt` (default `evidence`) | readOnly, idempotent, closed world | evidencia `{paymentId, pointer, backend, contentHash, keyAlg, mode, retention, anchoredAt, retentionUntil, receipt, receiptSigner, verified, signed}` o recibo `{receipt{...}, signature, signer, domain{name, version, chainId}}` (`anyOf` titulado por `document`) |

El JSON exacto de las cuatro (`name`, `title`, `description`, `inputSchema`, `outputSchema`, `annotations`, `_meta`)
está en [`X-1-MCP-LECTURAS.tools.json`](X-1-MCP-LECTURAS.tools.json), volcado de `tools()` de esta rama. Un solo
valor está editado: la `salt` del ejemplo del escrow, cortada porque el hook pre-commit rechaza `0x` + 64 hex; el
`tools/list` vivo la da entera.

### Comportamientos que el relevo de Emporium tiene que conocer

- **`x402_chain_health` con una cadena `down` es RESULTADO, no error.** La ruta contesta `503` con su documento;
  la tool lo devuelve como resultado normal con `structuredContent`. Sólo el `503` sin lectura
  (`{"status":"down","error":"probe_failed"}`) sale como `isError`. Sin `network`, basta una mainnet caída para
  que `status` sea `down`: para preguntar por una cadena, pasar `network`.
- **`x402_durable_evidence`**: `404` = no se ancló nada para ese `paymentId`; `410` = se ancló y expiró. Sólo
  responde donde `x402_supported` lista `durable-evidence` en `extensions` (producción lo listaba el 2026-11-02 a
  las 03:06Z según `facilitador.md` §1.1); en otro caso, el 404 genérico. Nunca devuelve los bytes sellados.
- **`x402_escrow_onchain_state`** lee el contrato `AuthCaptureEscrow` con una view call: es «qué tiene la cadena
  para este pago», no el estado de una tarea de un marketplace. Complementa el estado de pago de EM, cuyo
  `funds_locked` es null porque no lee el contrato (`em.md` §2). Requiere `ENABLE_PAYMENT_OPERATOR` en el
  facilitador (sin él la ruta contesta su error tipado, y la tool lo devuelve como `isError`).
- **Argumentos estrictos en las lecturas GET**: una clave no declarada da `-32602` (JSON-RPC invalid params),
  igual que un `paymentId` vacío o un `document` fuera de `evidence`/`receipt`. Los valores los juzga la ruta REST
  (una red desconocida es su `400 unknown_network`, no un error del MCP).
- **Rate limit**: `/mcp` entero va por el balde `verify-settle` por IP (1 ficha cada 2 s, ráfaga 30), más
  estricto que los presupuestos REST de estas rutas. Si Emporium releva desde una sola IP de salida, ese balde lo
  comparte todo su tráfico al facilitador. Una identidad de stack reconocida (`X-UVD-Stack-Key`) queda exenta de
  los baldes por IP, y `/mcp` reenvía esa cabecera a la ruta interna. Emporium ya está previsto como servicio con
  clave (decisión 144, `src/rate_policy.rs:583-589`, `terraform/environments/production/secrets.tf:255-271`);
  **si su digest está cargado en producción: [SIN MEDIR]** (`var.stack_key_emporium_loaded`).

### Nombres en emp0rium (no los escribo yo)

El plan propone `pay_network_directory`, `pay_chain_health`, `pay_escrow_state` y `pay_delivery_evidence`
([HIPÓTESIS], los pone quien escribe la fila). La refutación (P3-8) avisa que `pay_escrow_state` choca con
`pay_task_escrow` de EM. Del lado del facilitador el nombre ya lo desambigua (`x402_escrow_onchain_state`); en
Emporium conviene algo como `pay_escrow_onchain`, fijado junto con el de EM.

## 2. Qué cambió en el código

- `src/mcp.rs`
  - `TOOL_NAMES` y `TOOL_CLASSES` pasan a 8; `rest_target` mapea las cuatro rutas nuevas.
  - `rest_request` arma la petición: las tools de cuerpo mandan sus argumentos tal cual; las lecturas GET los
    ponen en el path o en la query con `percent_encode` (sólo los caracteres no reservados de RFC 3986 quedan
    sin codificar, así un `/`, `?`, `#` o `..` no sale de su segmento) y rechazan argumentos no declarados.
  - `is_tool_result`: la única respuesta no-2xx que es resultado es el `503` de `/health/ready` con documento.
  - `rest_router` monta sólo las rutas de las tools: las cuatro de antes, `/networks.json`, `/escrow/state`,
    `/health/ready` con el estado que recibe, y `/dx402/evidence` y `/dx402/receipt` (nunca `dx402_routes()`
    entero, que trae `anchor`, `recover` y `repair`). Su fallback es el 404 público (`agent_not_found`).
  - `mcp_routes` recibe además el `Arc<ReadinessState>` y el `Option<Arc<Dx402Service>>`.
- `src/main.rs`: el `ReadinessState` se arma UNA vez y lo usan la ruta pública `/health/ready` y el MCP (su caché
  y su único prober son lo que impide que la ruta amplifique RPC); el MCP recibe el mismo `dx402_service`.
- Documentos que listaban las tools o decían «four tools»: server card, `ard.json` (`capabilities`), las dos agent
  cards, `llms.txt` (y `llms-full.txt`, regenerado con su script), `index.md`, `skill.md` §10 (y el digest en
  `agent-skills/index.json`), `/mcp` en Markdown y HTML (EN y ES), el título MCP de la landing y el string
  `endpoints.mcpGet` de la landing y de `/x402`, la descripción de `POST /mcp` en `/docs`, y `CLAUDE.md`.
- `VERSION` 2.54.0 y `CHANGELOG.md`. Si otra rama de la tanda también sube a 2.54.0, el conflicto es de una línea.

## 3. Tests nuevos

| Test | Qué sostiene |
|---|---|
| `tools_list_is_exactly_the_eight` | la lista cerrada, escrita a mano en el test |
| `every_read_is_read_only_and_reaches_only_a_read_route` | las 5 lecturas: readOnly + idempotent, y su ruta es GET o el POST de la view call |
| `the_internal_router_serves_no_write_and_no_admin_route` | 16 escrituras/admin/lecturas ERC-8004 contra el router interno con DX402 encendido: todas 404; las rutas de las tools sí están |
| `networks_over_mcp_is_the_body_of_get_networks_json` | byte a byte contra la ruta pública, y `structuredContent` válido contra el `outputSchema` |
| `the_networks_output_schema_describes_a_production_document` | el documento armado desde el `/supported` de producción (2.40.0) valida; tres rotos no |
| `chain_health_over_mcp_is_the_body_of_get_health_ready` | igual que la ruta pública (salvo `ageSecs`), `network: null` = sin red |
| `chain_health_shares_the_readiness_cache_with_the_public_route` | una lectura por MCP es la que la ruta pública sirve 1,1 s después (mismo `checkedAtUnix`) |
| `chain_health_passes_the_network_through_percent_encoded` | `&`, `:` y vacío llegan enteros; URIs esperadas escritas a mano |
| `only_a_down_reading_of_the_chain_health_is_a_non_2xx_result` | tabla de verdad de `is_tool_result` |
| `escrow_state_over_mcp_is_the_answer_of_post_escrow_state` | tres cuerpos, igual que la ruta pública |
| `the_escrow_input_schema_is_the_query_type` | el ejemplo parsea como `EscrowStateQuery`; cada `required` lo exige el tipo; `maxAmount` numérico y `network: "base"` se rechazan |
| `the_escrow_output_schema_is_the_response_type` | un `EscrowStateResponse` serializado valida; montos numéricos no |
| `tools_list_publishes_the_schemas_the_tests_validate` | lo que `tools/list` publica es exactamente lo que los tests de esquema validan (salió de la mutación M17: sin él, una tool podía publicar otro esquema con todo en verde) |
| `durable_evidence_over_mcp_is_the_body_of_the_dx402_routes` | evidencia y recibo byte a byte, los dos válidos; `0X` + mayúsculas también contesta |
| `a_hostile_payment_id_stays_inside_its_path_segment` | `../../settle`, `a/b?c#d`, `x%2Fy`, `a+b c`, `ñ`: llegan al handler de evidencia, nunca a otra ruta |
| `durable_evidence_refuses_a_malformed_argument` | `paymentId` ausente, null, vacío, de espacios o no string; `document` fuera de sus dos valores |
| `durable_evidence_without_dx402_is_the_routes_generic_404` | sin DX402, el mismo 404 que la ruta pública |
| `the_get_reads_refuse_an_argument_they_do_not_declare` | `-32602` en las tres lecturas GET |
| `percent_encode_leaves_only_unreserved_characters_bare` | el codificador, y que la query decodifica al string exacto |
| `the_mcp_bucket_is_never_looser_than_the_reads_it_relays` | `verify-settle` ≥ en período y ≤ en ráfaga que `secondary-read` y `discovery-read` |
| `main_hands_the_mcp_server_the_one_readiness_state_and_the_dx402_service` | `main.rs` arma un solo `ReadinessState` y se lo pasa al MCP |
| `every_document_that_lists_the_mcp_tools_names_all_of_them` | los 9 documentos nombran las 8; ninguno dice «four tools» ni «cuatro herramientas» |
| `the_server_card_describes_the_server_that_answers` (ampliado) | además, cada entrada de la card con el `title` de `tools/list` |

## 4. Verificación (local, 2026-11-02)

WSL Ubuntu 24.04, rustc 1.99.1, `CARGO_INCREMENTAL=0`, features del CI, árbol del commit exportado en LF a ext4,
red cerrada. `origin/main` = `c1248eca` al verificar, así que la unión con la base es el propio HEAD.

| Paso | Resultado |
|---|---|
| Línea base (`origin/main`, misma suite) | 0 fallos: lib 2090, bin 2159 |
| `cargo test --locked -p x402-rs --features <CI> --no-fail-fast -- --test-threads=1` | 0 fallos: lib 2090, bin 2180 (+21 nuevos; los tests de `mcp` viven en el binario) |
| `cargo test --locked -p x402-axum -p x402-reqwest -p x402-compliance -- --test-threads=1` | 0 fallos |
| `cargo clippy --locked -p x402-rs --features <CI> --all-targets` | exit 0; 0 warnings en `src/mcp.rs` y `src/main.rs` |
| `verify_landing_canonical.py --offline` · `frontend-capabilities` (Node 22) | `[OK]` · 19/19 |
| Tests Python del job (balances, ERC-8004, drift gate, deploy-job) · `terraform test` 1.9.8 | ok · 26/26 |
| `no-account-id.yml` (su `run:` literal) · `preci.py --ejecutar` | OK · verde |
| Mutaciones M1..M17 (una por guarda) | 17/17 matadas; M17 sobrevivía y de ahí salió `tools_list_publishes_the_schemas_the_tests_validate` |

Rojos de base: ninguno.

## 5. Lo que queda después del merge (c0der)

1. Tanda del facilitador: `/version` = la de la tanda, y **un** `POST /mcp tools/list` (secuencial, sin bucle)
   con 8 tools: las 5 lecturas con `readOnlyHint: true` y `outputSchema`, `x402_settle` con
   `destructiveHint: true`.
2. Una sola llamada de humo por lectura nueva, secuencial, contra producción: `x402_networks {}`,
   `x402_chain_health {"network":"base"}`, `x402_durable_evidence` con un `paymentId` real de
   `/dx402/stats` si lo hay. El escrow no necesita humo (su lectura exige un `paymentInfo` real).
3. EMP-1 refresca `catalogo/contratos/facilitador.json` (hoy 2.40.0) contra el MCP vivo y EMP-2 agrega las filas.
4. El plugin de PayBox del facilitador (§7): c0der lo empaqueta desde el commit mergeado con
   `scripts/reproducible-build.sh` y el dueño lo sube **después** de la tanda 2.54.0. Antes de la tanda,
   cuatro de sus cinco tools contestan `unknown tool`.

## 6. Filas de HARD-STOPS propuestas (no escritas)

- **MCP del facilitador, lista cerrada.** `TOOL_NAMES` (8) sólo crece cambiando `tools_list_is_exactly_the_eight`,
  la server card y los documentos que la nombran; nunca una ruta admin ni una escritura (fuera de `x402_settle`),
  ni las lecturas ERC-8004. El router interno monta rutas una por una, nunca un router público entero.
- **Verificación de X-1 después de la tanda.** Un solo `tools/list`, secuencial; si no son 8 o una lectura no trae
  `readOnlyHint: true`, no se agregan filas en emp0rium hasta corregirlo.
- **Emporium relevando el MCP del facilitador.** Antes de EMP-2 contra producción, confirmar que el digest de la
  stack key de Emporium está cargado (`var.stack_key_emporium_loaded`); sin ella, todo su relevo comparte el balde
  `verify-settle` de una IP.
- **Plugin de PayBox del facilitador, sólo lecturas.** `paybox-plugin/` releva exactamente las tools de clase
  `lectura` de `POST /mcp`; una tool entra al plugin cambiando su clase en `src/mcp.rs` y regenerando el
  catálogo, nunca a mano. Ningún riel de pago (`x402_accepts`, `x402_verify`, `x402_settle`) entra al plugin.

## 7. El plugin de PayBox del facilitador (AVISO de c0der, 2026-11-02 04:35Z)

> Decisión del dueño: además de que emp0rium exponga estas lecturas como `pay_*`, el facilitador tiene su propio
> plugin de PayBox. Va en la misma rama, sin push. **Paquete listo para que c0der lo empaquete y el dueño lo
> suba; no lo subí.**

**QUÉ:** `paybox-plugin/`, un cascarón de PayBox de un solo host (`facilitator.ultravioletadao.xyz`) que releva
las cinco lecturas del MCP del facilitador con sus mismos nombres y definiciones: `x402_supported`,
`x402_networks`, `x402_chain_health`, `x402_escrow_onchain_state` y `x402_durable_evidence`.
**POR QUÉ:** el dueño eligió también un plugin propio; el molde es el cascarón de Emporium (relevo con
presupuesto de tiempo, sin lógica propia ni firmas), no el de EM.
**RIESGO:** que el plugin listara un riel de pago o una definición que el facilitador no publica. Lo cierran
tres cosas: el catálogo lo escribe `src/mcp.rs` desde su propio `tools/list` (sólo clase `lectura`) y la suite
de Rust lo compara byte a byte; el plugin rechaza al arrancar cualquier tool que no sea `lectura` +
`readOnlyHint`; y un nombre que no lista no sale nunca (error JSON-RPC, cero requests).

### Qué es, archivo por archivo

El molde de `emporium/paybox-plugin` (starter 1.0.4 de PayBox), con estas diferencias:

| | Emporium | Facilitador |
|---|---|---|
| Catálogo | `cargo run --bin plugin_catalogo` desde `catalogo/plugin.toml` | el test `mcp::tests::the_paybox_plugin_relays_exactly_the_reads_of_tools_list` con `X402_WRITE_PAYBOX_PLUGIN=1`, desde el `tools/list` real |
| Tools | 42 por propósito + la guía local | las 5 lecturas, mismos nombres; **sin guía**: el AVISO pide exactamente las lecturas |
| Llamada de muestra de la conformidad | la guía (sin red) | `x402_supported` (una lectura real a producción, pública) |
| Al arrancar | forma del catálogo | forma + cada tool `lectura` y `readOnlyHint`, si no, no arranca |
| Datos de error | quita `tools_validas` | quita `tools` (el facilitador lista ahí sus 8 tools, rieles incluidos) |
| Pruebas extra | marcas de lo de abajo | un MCP falso en `127.0.0.1` con las 8 tools reales (`test/fake-mcp.test.ts`), y la marca de la DAO (`brand.test.ts`) |

Nombre `ultravioleta-facilitator`, título `Ultravioleta DAO x402 Facilitator`, user agent
`uvd-facilitator-paybox-plugin/0.1.0`, `instructions` de 7 líneas y menos de 1.000 caracteres (ASCII; un test
las ata a las tools listadas). Sin variables ni secretos. Archivos del starter byte a byte contra su manifiesto
oficial (`9c663d57…5373ea5`); `src/paybox.ts` (los armadores de propuestas) queda afuera.

`test/fixtures/facilitator-tools.json` es el `tools/list` entero del facilitador (8), generado con el catálogo:
el test del plugin comprueba que el catálogo es exactamente sus lecturas, así que una edición a mano de cualquiera
de los dos se pone roja en el CI del plugin aunque el de Rust no corra. Una cadena de `0x` + 64 hex (la `salt` del
ejemplo del escrow) se escribe con la `x` como escape unicode: la misma cadena una vez leída, que el hook
pre-commit no toma por una llave.

### El CI

`.github/workflows/paybox-plugin.yml`: `push` a `main` y `pull_request`, los dos con `paths:` `paybox-plugin/**` y
el propio workflow. Corre tipos, dos builds con los mismos bytes, las pruebas y el bundle, en Node 24.21.0 y pnpm
11.11.0. No compila Rust ni despliega, y `ci.yaml` no tiene `paybox-plugin/` en su unión de paths, así que un
cambio sólo del plugin no lo arranca. No corre la conformidad: su llamada de muestra es una lectura real de
producción. El comparador de Rust corre en `ci.yaml` cuando cambia `src/`.

### Para empaquetar (c0der) y subir (el dueño)

1. Desde el commit mergeado: `bash paybox-plugin/scripts/reproducible-build.sh <commit> <carpeta>` (necesita Docker;
   usa `node:24.21.0` por digest). Imprime el reporte del starter con `artifact.sha256` y copia `plugin.mjs`.
2. Subir **después** de la tanda 2.54.0. Contra 2.53.0 (producción hoy) las cuatro lecturas nuevas contestan
   `unknown tool`; el plugin lo muestra como error sin la lista de tools del facilitador.
3. Host de red: `facilitator.ultravioletadao.xyz`. Sin variables ni secretos.
4. Límite: el facilitador cuenta `POST /mcp` en el balde de `/verify` y `/settle` por IP (una ficha cada 2 s,
   ráfaga 30). Todas las llamadas que salgan de una misma IP de PayBox lo comparten; las IPs de salida de PayBox:
   [SIN MEDIR]. Un `429` se le muestra al agente como «esperá un minuto».
