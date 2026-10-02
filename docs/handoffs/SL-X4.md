# SL-X4 — el facilitador sin límites de política para el stack ni para las pruebas del operador (decisión 144)

**Encargo:** c0der, 2026-10-02 (plan `c0der/docs/plans/2026-10-02-stack-sin-limites.md`, decisión 144).
**Base:** `origin/main` = `ff0c6404`. **Rama:** `c0der/sl-x4`, commits locales, sin push (va con la tanda).

## Estado

- **Hecho.** Lista blanca de IP (`src/ip_allowlist.rs`) que exime de lo mismo que exime una `X-UVD-Stack-Key`
  reconocida: los ocho presupuestos por IP y el techo en vuelo por dirección. Se lee de un secreto de Secrets
  Manager **en tiempo de ejecución** y se vuelve a leer cada 300 s: una IP nueva entra sin deploy. Ninguna IP
  de la lista llega a un log de la aplicación, a una respuesta ni a `GET /config` (publica conteo y estado).
- `emporium` se suma a los servicios del stack (`UVD_STACK_KEY_SHA256_EMPORIUM`), inactivo hasta que se cargue
  su digest: Emporium lee el bazar con su propia llave.
- Confirmado contra el código: **todos** los limitadores de política del facilitador ya respetaban la llave
  (§1); los que no la respetan son de hardware, de cuerpo o de plata, y siguen sin respetarla ni la llave ni la
  lista.
- El facilitador **no hace ninguna llamada propia a otro servicio del stack** (§2): todo pedido suyo hacia un
  host del stack lleva una URL que puso un tercero, así que la llave no debe viajar ahí, y no viaja (tests).
- **Falta (no es mío hacer):** aplicar terraform (§4), cargar el secreto de la lista, y el digest de Emporium.

## 1. Lo medido: los limitadores que encuentra un llamador del facilitador

App, en `ff0c6404`. Todos los presupuestos por IP pasan por `rate_policy::PolicyLayer` (`src/rate_policy.rs:979`),
que es el único punto donde se decide la exención (`every_governor_goes_through_the_policy` lo exige).

| # | Limitador | Dónde (`@ff0c6404`) | Clase | Llave | Lista blanca |
|---|---|---|---|---|---|
| 1-8 | presupuestos por IP: verify-settle, discovery-register, discovery-read, events, identity-read, secondary-read, human-pages, erc8004-writes | `src/rate_policy.rs:304,328,345,360,377,396,414,431` | política | exime (ya) | exime (nuevo) |
| 9 | `POST /mcp` en la cubeta de verify-settle | `src/main.rs:644` | política | exime (ya) | exime (nuevo) |
| 10 | techo en vuelo por dirección (32), `429 too_many_concurrent_requests` | `src/rate_policy.rs:1061`, `admit` `:1215` | política | exime (ya) | exime (nuevo) |
| 11 | techo global en vuelo (512), `503 overloaded` | `src/rate_policy.rs:1048` | hardware | no | no |
| 12 | plazo total del cuerpo (5 s), `408` | `src/rate_policy.rs:1072` | anti-abuso | no | no |
| 13 | tamaño del cuerpo (64 KiB), `413` | `src/main.rs:559`, `:846` | anti-abuso | no | no |
| 14 | tope diario de escrituras ERC-8004, `429 erc8004_daily_write_limit` | `src/erc8004/daily_cap.rs:270`, `:312` | plata (gas) | no | no |
| 15 | cupo de suscriptores de `/events` (64), `503` | `src/events.rs:42`, `:251` | capacidad global, no por llamador | no | no |
| 16 | throttle hacia el proveedor de RPC | `chain::evm` (`RPC_MAX_CU_PER_SECOND`) | saliente: no responde a nadie | n/a | n/a |

- **Bans por IP: ninguno.** El único estado por dirección es el contador del techo en vuelo
  (`src/rate_policy.rs:1100`), que se libera al terminar cada pedido. El único log con IP de cliente en `src/` es
  el de una llave rechazada (`note_rejected`), que ahora no nombra una IP de la lista.
- **Infraestructura (terraform, `@ff0c6404`):** ningún limitador de política configurado.
  - ALB `aws_lb.main` (`terraform/environments/production/main.tf:426`): sin rate limiting.
  - WAF y CloudFront: ninguno (`aws_wafv2`, `rate_based`, `aws_cloudfront` en `terraform/`: 0 coincidencias).
  - API Gateway: el stage del Lambda de saldos (`lambda-balances.tf:167`) y el del entorno Zama
    (`zama-testnet/main.tf:259`) no declaran `default_route_settings` ni throttling: solo aplican los límites
    de cuenta de AWS, que no son política nuestra. Ninguno es la API del facilitador.
  - Lambda: el de saldos sin concurrencia reservada (`lambda-balances.tf:86`); Zama con concurrencia
    aprovisionada (`zama-testnet/main.tf:197`), que precalienta y no limita.
  - Por eso no hay cambio de throttling en terraform: la lista blanca vive en la app, que es donde están todos
    los limitadores de política.

## 2. Lo medido: las llamadas salientes hacia el stack

| Llamada | Dónde (`@ff0c6404`) | A quién | ¿Lleva llave? | ¿Debe? |
|---|---|---|---|---|
| sondas de salud del Bazaar | `src/discovery_health.rs:752` (`safe_get`, a lo sumo 3 por host y tick, `:50`) | cada recurso listado, incluidos los de primera parte (`src/discovery_curation.rs:200`) | no | **no**: la URL la registra cualquiera en `POST /discovery/register` |
| ancla del feedback | `src/erc8004/proof.rs:686` (`safe_get`) | el `feedbackUri` que manda quien califica | no | **no**: URL de un tercero |
| crawler `/.well-known/x402` | `src/discovery_crawler.rs:235` | semillas de `DISCOVERY_CRAWL_URLS` (apagado, `src/main.rs:361`) | no | **no** |
| agregador del Bazaar | `src/discovery_aggregator.rs:415` | 12 facilitadores externos, ninguno del stack | no | n/a |
| proxy FHE | `src/fhe_proxy.rs:203`, `:242` | el Lambda Zama de este repo; solo el cuerpo JSON | no | **no**: reenvía verify/settle de terceros |
| reenvío al holder del lease | `src/handlers.rs:1913`, copia los headers del llamador en `:1958` | otra task del mismo servicio | la del **llamador**, si trajo; nunca agrega | correcto |
| pedido sintético de MCP | `src/mcp.rs:754` | el router REST del mismo proceso | la del **llamador**, si trajo; nunca agrega | correcto |

El facilitador no tiene llave propia (ninguna lectura de `UVD_STACK_KEY`, la variable de un cliente). Lo atan
`rate_policy::the_facilitator_holds_no_stack_key_to_send` (fuente) y
`mcp::a_third_partys_forwarded_settle_carries_no_stack_key` (comportamiento).

## 3. La lista blanca

- **Qué IP cuenta:** la misma que usan los presupuestos, `ClientIpKeyExtractor`: la última entrada de
  `X-Forwarded-For`, la que agrega el ALB. Escribir una IP de la lista delante de la propia no sirve
  (`naming_an_allowlisted_address_in_forwarded_for_buys_nothing`).
- **Qué exime:** exactamente lo que exime una llave (§1, filas 1-10). La respuesta lleva
  `x-ratelimit-exempt: ip-allowlist` y ningún `RateLimit`. Si el pedido trae además una llave válida, gana la
  llave (el header nombra el servicio; lo fija `an_allowlisted_address_is_never_refused_by_any_budget`).
- **El secreto:** `UVD_IP_ALLOWLIST_SECRET` (nombre; producción `uvd/allowlist/home`),
  `UVD_IP_ALLOWLIST_REFRESH_SECS` (30–3600, default 300; fuera de rango → default con warning). Sigue la regla
  única del stack (aviso de c0der, 2026-10-02): **uno para todo el stack, lo crea y lo carga c0der fuera de
  terraform** (primario en us-east-2, réplica en us-east-1), cada servicio lo lee en su región por nombre. El
  facilitador está en us-east-2: lee el primario.
- **Formato:** el canónico, un arreglo JSON de strings; también lee texto separado por comas, `;` o espacios. Un
  documento que empieza como JSON y no es un arreglo (un objeto, o JSON que no parsea) es una lista vacía.
- **Rechaza** (por posición y razón, nunca por valor): lo que no es dirección o prefijo; prefijos más anchos
  que /24 (IPv4) o /48 (IPv6); direcciones no públicas (privadas, loopback, link-local, 100.64/10, multicast,
  reservadas, ULA); más de 64 entradas.
- **Falla cerrado:**
  - sin la variable, desactivada y sin llamar a AWS;
  - secreto inexistente o sin valor → lista vacía;
  - un documento que no es una lista → lista vacía, **también encima de una lista leída antes** (`lastRead:
    "unreadable"`);
  - una lectura sin respuesta en 10 s cuenta como fallida; tres fallos seguidos vacían la lista (una IP vieja
    puede ser de otro);
  - y, pase lo que pase con la tarea que refresca, una lista sin ninguna lectura que llegue al secreto durante
    `4 × (refresco + plazo de lectura)` (≈ 20 min en producción) no exime a nadie (`lastRead: "stale"`).
  - Una IPv4 vista como IPv6 mapeada es la misma IPv4.
- **Ninguna IP en un log de la aplicación, una respuesta o `/config`:** `Debug` y el resumen publican conteos;
  `Prefix` no implementa `Debug`; una llave rechazada desde una IP de la lista se loguea como
  `allowlisted (withheld)`. `/config` publica `ipAllowlist` (`enabled`, `entries` en vigor, `lastRead`,
  `refreshSecs`, qué exime), tal como está en cada pedido, y ya no el nombre del secreto. Fuera de la aplicación:
  los access logs del ALB (activos, `production.auto.tfvars`) guardan la IP de todo cliente, como siempre; y una
  llave rechazada desde una IP que **todavía** no está en la lista (recién cambiada) se loguea con su IP, como la
  de cualquier tercero.

## 4. Terraform (sin apply; lo aplica c0der)

| Recurso | Qué | ¿Lo aplica el deploy de imagen? |
|---|---|---|
| (ninguno) `uvd/allowlist/home` | **no se declara**: lo crea y lo carga c0der fuera de terraform (regla única del stack) | — |
| `aws_iam_role_policy.ip_allowlist_read` | `secretsmanager:GetSecretValue` para el **rol de la tarea**, por nombre (`<nombre>-??????`, us-east-2) | sí, desde la tanda X4-BAZAAR-TANDA: va en el `-target` del deploy de imagen (CI puede escribir las políticas inline del rol de la tarea; el drift gate la pedía cubierta) |
| `UVD_IP_ALLOWLIST_SECRET` en la task definition | el **nombre** del secreto, nunca su contenido | sí |
| `aws_secretsmanager_secret.stack_key_digest_emporium` | digest de Emporium, **sin valor** | no |
| `data.aws_secretsmanager_secret.stack_key_digest_emporium` + mapeo `UVD_STACK_KEY_SHA256_EMPORIUM` | solo con `var.stack_key_emporium_loaded = true` (default `false`) | no, mientras sea `false` |

- **Ningún objetivo del deploy referencia un recurso nuevo.** Una referencia, aunque esté en la rama no tomada de
  un condicional, mete ese recurso en el cierre del `-target` del deploy, y el usuario de CI no puede crear
  secretos. Por eso Emporium se lee con un `data` con `count` y la política de lectura usa el nombre.
- **Si el deploy llega antes que el apply:** la task pide el secreto, recibe `AccessDenied`, lo loguea (sin IP) y
  no exime a nadie. Falla cerrado.
- **Un secreto que todavía no existe da `failing`, no `missing`:** con el permiso por nombre (`-??????`), AWS
  contesta `AccessDenied` y no `ResourceNotFound` para un nombre que no existe. Si el secreto se cifra con una CMK
  propia, el rol de la tarea necesita además `kms:Decrypt` sobre esa llave (con la llave administrada de Secrets
  Manager no hace falta).
- **Antes de cargar la lista (precondición, no código):**
  - el ALB es solo IPv4 (`aws_lb.main` no declara `ip_address_type`), así que la entrada útil es la IPv4 de salida;
    las entradas IPv6 no calzan nunca acá;
  - comprobar que esa IPv4 **no** sea de CGNAT (100.64/10 en la WAN del router): detrás de una IP compartida, todos
    sus clientes quedarían exentos, incluido el techo en vuelo por dirección;
  - preferir la dirección exacta a un /24: un prefijo exime también a los vecinos del mismo bloque.
- **Orden para c0der:**
  1. `terraform apply -target=aws_secretsmanager_secret.stack_key_digest_emporium` (a mano: CI no puede crear
     secretos). `aws_iam_role_policy.ip_allowlist_read` lo aplica el deploy de imagen.
  2. El secreto `uvd/allowlist/home` (fuera de terraform, como dice el aviso): cargarlo desde un archivo, sin
     imprimirlo, p. ej. `aws secretsmanager put-secret-value --secret-id uvd/allowlist/home --secret-string file://<archivo>`.
  3. Sondas (no muestran ninguna IP):
     `curl -s https://facilitator.ultravioletadao.xyz/config | jq '.ipAllowlist | {enabled, entries, lastRead}'`
     → `true`, N, `"ok"` a más tardar 300 s después de la carga; y desde la IP de la lista
     `curl -s -o /dev/null -D - https://facilitator.ultravioletadao.xyz/config | grep -i '^x-ratelimit-exempt'`
     → `x-ratelimit-exempt: ip-allowlist`.
  4. Emporium: `scripts/stack_key.py generate --service emporium`, cargar `{"sha256": ...}`, y en el mismo cambio
     que pone `stack_key_emporium_loaded = true` aplicar a mano `aws_iam_role_policy.secrets_access` (el CI no
     puede escribir el rol de ejecución; `scripts/drift_gate_iam.py` imprime el comando).
- El drift gate (`ci.yaml`, job `plan`) da rojo con `aws_secretsmanager_secret.stack_key_digest_emporium` hasta
  el paso 1: el deploy no depende de ese job, pero el run queda rojo. La política de lectura ya no aparece ahí
  porque el deploy la cubre.
- **Emporium (para su worker):** la llave exime al bazar de la cuota por IP. Si Emporium reenviara una a una al
  bazar las consultas de sus propios usuarios con su llave, la exención heredaría su superficie pública. Que la
  use solo para sus lecturas propias (caché, crawler) o que tenga su propio límite por cliente antes.

## 5. Tests y auto-refutación

Todos con la red cerrada; el único socket es el `127.0.0.1` de los tests de MCP. Lo que pide el encargo:

| Pedido | Test |
|---|---|
| con llave válida no hay 429 de política | `rate_policy::a_stack_identity_is_never_refused_by_any_budget` (ya estaba) |
| sin llave o con llave inválida, el límite de siempre | `a_malformed_or_false_key_is_a_third_party_not_a_500`, `a_revoked_key_is_a_third_party_again` (ya estaban) |
| una IP de documentación en la lista pasa | `an_allowlisted_address_is_never_refused_by_any_budget` (los 8 presupuestos al burst de producción; la vecina, 429) |
| suplantarla en `X-Forwarded-For` no sirve | `naming_an_allowlisted_address_in_forwarded_for_buys_nothing` |
| un pedido reenviado de un tercero no lleva la llave | `mcp::a_third_partys_forwarded_settle_carries_no_stack_key`, `rate_policy::the_facilitator_holds_no_stack_key_to_send` |
| los límites anti-abuso siguen | `an_allowlisted_address_skips_the_per_address_ceiling_but_no_protection` (techo global 503, 408, 413), `handlers::…::an_allowlisted_address_still_spends_the_daily_gas_cap`, `the_gas_cap_knows_nothing_of_the_stack` |
| ninguna IP en logs ni respuestas | `an_allowlisted_address_never_reaches_a_log_or_a_response`, `ip_allowlist::nothing_prints_an_address`, `the_config_document_publishes_the_allowlist_by_count_only` |
| la lista falla cerrado | `ip_allowlist::` `a_document_that_starts_like_json_and_is_no_array_exempts_nobody`, `a_failing_read_keeps_the_list_briefly_then_empties_it`, `a_read_that_never_answers_is_a_failed_read`, `a_list_nobody_re_reads_goes_stale_and_exempts_nobody`, `what_is_refused_and_why` |
| la IP cambia sin redeploy | `a_new_read_replaces_the_list_and_a_missing_secret_empties_it`, `the_refresher_picks_up_a_new_address_without_a_restart` |

**Bordes revisados:** varias líneas de `X-Forwarded-For`, entradas escritas delante de la del ALB, IPv6 entre
corchetes, IPv4 mapeada en IPv6 (entrada y cliente), prefijos `/0`, `/16`, `/23`, `/47`, `/+24`, `/33`, `/129`,
`a/b/c`, rangos no públicos y sus bordes (100.64/10, fc00::/7, fe80::/10), JSON roto, objeto JSON, ítems que no
son texto, más de 64 entradas, secreto vacío o inexistente, lecturas que fallan o no responden, refresco muerto,
llave y lista a la vez.

**Gate de CI local** (red cerrada, features de CI, LF en ext4): base `ff0c6404` y cabeza, 0 fallos en las dos;
lib 1498 → 1518 y bin 1563 → 1584 tests; integración, doctests, crates del workspace, pasos de Python y Node,
`terraform validate`: verdes en las dos. El último ajuste (datos de test con direcciones de documentación) se
verificó aparte sobre el árbol commiteado: 61 tests de `rate_policy`, `ip_allowlist` y `erc8004_write_rate_tests`,
0 fallos.

**Mutaciones** (cada una sobre la cabeza exportada a ext4, con los tests de `rate_policy`, `ip_allowlist` y
`erc8004_write_rate_tests`; restaurada byte a byte después; al final el árbol, idéntico al original). 24 de 24 en
ROJO:

| # | Mutación | Resultado |
|---|---|---|
| K1 | cualquier llave bien formada coincide (`ct_eq(..) \|\| true`) | ROJO (9 tests) |
| K2 | se aceptan varias líneas del header | ROJO (`a_malformed_or_false_key_…`) |
| K3 | una llave corta está bien formada | ROJO (`a_short_key_never_authenticates_…`) |
| A1 | cualquier IP exenta en cuanto la lista no está vacía | ROJO (presupuestos, suplantación) |
| A2 | cuenta cualquier entrada de `X-Forwarded-For`, no la del ALB | ROJO (suplantación) |
| A3 | la lista sujeta al techo en vuelo por dirección | ROJO (`…_but_no_protection`) |
| A4 | la lista se salta el techo global | ROJO (`…_but_no_protection`) |
| A6 | una llave rechazada desde una IP de la lista loguea la IP | ROJO (logs) |
| A7 | `/config` publica la lista como estaba al arrancar | ROJO (`/config`) |
| A8 | la dirección gana sobre una llave reconocida | ROJO (presupuestos) |
| W1 | producción nunca lee la lista (`IpAllowlist::disabled()` en `from_env`) | ROJO (`production_reads_the_allowlist_…`) |
| S1 | código de producción lee una llave propia (`UVD_STACK_KEY`) | ROJO (`the_facilitator_holds_no_stack_key_…`) |
| S3 | código de producción de `handlers.rs`, después de su primer módulo de test, nombra el header | ROJO (`the_facilitator_holds_no_stack_key_to_send`); la primera corrida no compiló (la línea quedó entre `#[instrument]` y su función) y se repitió |
| L1 | sin piso de prefijo | ROJO (`what_is_refused_and_why`) |
| L2 | se aceptan direcciones no públicas | ROJO (`what_is_refused_and_why`) |
| L3 | un cliente IPv4 mapeado no es su IPv4 | ROJO (bordes de prefijo) |
| L4 | una lectura nueva no reemplaza la lista | ROJO (4 tests) |
| L5 | una lista que no se puede releer se queda para siempre | ROJO (2 tests) |
| L6 | el resumen imprime una dirección | ROJO (3 tests) |
| L7 | un objeto JSON se lee como texto | ROJO |
| L8 | el piso IPv6 más ancho que /48 | ROJO |
| L9 | un documento que no es lista deja en vigor la lista vieja | ROJO |
| L10 | una lectura sin respuesta se espera para siempre | ROJO (`a_read_that_never_answers_…`) |
| L12 | una lista que nadie relee nunca vence | ROJO (`a_list_nobody_re_reads_…`) |

No corridas, por duplicar una guarda que otra mutación ya cubre (sin medir, así que no cuentan): K4 (recortar
la llave antes del hash; es la M2d de X4-STACK-429, que mataban los casos de espacio de
`a_malformed_or_false_key_…`), A5 (saltarse el plazo del cuerpo; el test de A4 también exige el 408), L11 (subir
el 3; el test fija el literal) y S2 (el tope de gas nombra la lista; el test de fuente busca `allowlist`).

Refutador adversarial (un agente, opus), sobre el diff, el encargo y este documento: **CONDITIONAL**, sin P1.
Los dos P2 (garantías de falla cerrada sin test; el escaneo de fuente cortaba `handlers.rs` en su primer módulo
de test) quedan cerrados con los tests que matan L9, L10, L12 y S3, más el literal 3 en el test de fallos. De los P3 se aplicaron: test de "gana la llave",
test del cableado de producción, bordes de `is_public`, redacción neutra y `/config` sin el nombre del secreto,
"ningún log de la aplicación", y el respaldo por vencimiento cuando el refresco muere. Los que son precondición
operativa están en §4.
