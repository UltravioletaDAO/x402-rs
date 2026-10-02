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
| `aws_iam_role_policy.ip_allowlist_read` | `secretsmanager:GetSecretValue` para el **rol de la tarea**, por nombre (`<nombre>-??????`, us-east-2) | no |
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
  1. `terraform apply -target=aws_iam_role_policy.ip_allowlist_read -target=aws_secretsmanager_secret.stack_key_digest_emporium`.
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
- El drift gate (`ci.yaml`, job `plan`) va a listar los recursos de la tabla como no desplegados hasta el paso 1.
  No bloquea el deploy.
- **Emporium (para su worker):** la llave exime al bazar de la cuota por IP. Si Emporium reenviara una a una al
  bazar las consultas de sus propios usuarios con su llave, la exención heredaría su superficie pública. Que la
  use solo para sus lecturas propias (caché, crawler) o que tenga su propio límite por cliente antes.
