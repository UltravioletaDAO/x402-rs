# X4-CLAVES-TF — el facilitador lee los cuatro digests de `X-UVD-Stack-Key` desde Secrets Manager

**Encargo:** c0der, 2026-09-26 09:35Z. Paso 2 de `docs/handoffs/X4-STACK-429.md` §8.
**Base:** `origin/main` = `fa76e745` (2.43.0 vivo, re-medido al llegar). **Rama:** `0xultravioleta/c0-x4-claves-tf`,
un push, PR contra `main`. Sin merge, sin deploy, sin plan ni apply y sin tocar AWS.

## Estado

- **Hecho.** `terraform/environments/production/secrets.tf` cablea los cuatro secretos del facilitador que c0der ya
  creó (`facilitator-stack-key-digest-<servicio>`, cuerpo `{"sha256": "<64 hex>"}`) en el execution role y en el
  task definition, por `secrets`. No hay nada en `environment`.
- **No cambia:** `src/`, `VERSION` (el binario es el mismo 2.43.0) ni `ci.yaml`.
- **Falta (de c0der):** leer el drift gate del PR, mergear (el merge despliega) y correr la sonda de abajo.

## Qué cambia

| Dónde | Qué |
|---|---|
| `secrets.tf:152-181` | Cuatro `data "aws_secretsmanager_secret"` por nombre, como `erc8004_admin_token`: `stack_key_digest_execution_market`, `_karmakadabra`, `_describe_net`, `_meshrelay`. Ninguno lleva `sha256` en el nombre: la regla 3 de `no-account-id.yml` lee `-sha256` como sufijo aleatorio de un secreto (medido) |
| `secrets.tf:239` → `:251` | `local.stack_key_digest_arns`, sumado a `local.all_secret_arns`. Entra en el `GetSecretValue` de `aws_iam_role_policy.secrets_access` (`main.tf:596`, `Resource` en `:608`). Solo esos cuatro ARN: el role no gana ningún secreto de cliente |
| `secrets.tf:455` → `:483` | `local.stack_key_digest_secrets`, sumado a `local.all_task_secrets`, que es el `secrets` del contenedor (`main.tf:1282`). Son cuatro entradas `UVD_STACK_KEY_SHA256_<SERVICIO>` ← `${data.aws_secretsmanager_secret.<x>.arn}:sha256::` |

**Los nombres salen del código.** `src/rate_policy.rs:566` define `ENV_STACK_KEY_SHA256_PREFIX = "UVD_STACK_KEY_SHA256_"`,
`:570` define `DEFAULT_STACK_SERVICES` (los cuatro servicios) y `:583` define `env_var_for`, que pasa el nombre a
mayúsculas y cambia `-` por `_`. Un script que lee esas tres cosas de `rate_policy.rs` y los bloques de `secrets.tf`
dio 4 de 4 (variable ↔ data source ↔ nombre del secreto ↔ `:sha256` ↔ presencia en la lista del IAM). El task no fija
`UVD_STACK_SERVICES`, porque el default del código ya es exactamente esa lista.

## CI

- **El deploy cubre el cambio.** El `terraform apply` de `ci.yaml:548-557` lleva
  `-target=aws_iam_role_policy.secrets_access` (`:551`), `aws_ecs_task_definition.facilitator` (`:552`) y
  `aws_ecs_service.facilitator` (`:553`). Los data sources son lecturas, no cambios.
- **El merge dispara el pipeline completo** aunque `src/` no cambie, porque `terraform/**` está en `paths`. Sale una
  imagen `2.43.0-<sha>` y `/version` sigue diciendo `2.43.0`. Ningún gate exige subir `VERSION`: el paso del tag solo
  exige que no esté vacío (`ci.yaml:486-487`).
- **Drift gate (`ci.yaml:258-318`).** Con este diff no necesita nada más. Descarta las acciones `read` y los tres
  cambios caen dentro de los `-target`. Además es lo primero que resuelve los cuatro nombres contra AWS: si alguno
  estuviera mal escrito, el plan con `-target` (`:285`) da error y el job queda en rojo. El usuario de CI lee
  metadatos de secretos con `ReadOnlyAccess` (`docs/CICD_SETUP.md:128`). Su deny cubre solo `GetSecretValue`, así que
  los otros data sources de `secrets.tf` ya pasan por ahí.

## Riesgo

- **Si un secreto no trae el campo `sha256`,** la task nueva no arranca (`ResourceInitializationError`). El servicio
  no tiene `deployment_*` propios, así que rigen los defaults de ECS: 100 % sano como mínimo y sin circuit breaker. Las
  tasks viejas siguen sirviendo, `aws ecs wait services-stable` se rinde y el CI queda en rojo. Se arregla con el
  cuerpo del secreto y `force-new-deployment`, o con un revert.
- **Consistencia eventual de IAM.** La policy y el task definition se aplican en el mismo `apply`, sin dependencia
  entre ellos. La primera task puede fallar con `AccessDenied` sobre un ARN nuevo y ECS la relanza. Es el mismo camino
  por el que salieron `facilitator_receipt_key` y `RPC_URL_ARC` (2.41.0).
- **Un digest que no parsea** (que no son 64 hex) no rompe nada: queda en el log por servicio y variable, nunca por
  valor, y ese servicio queda inactivo. La sonda lo muestra como `active` < 4.
- **El tráfico no cambia** hasta que un cliente mande la clave (pasos 3 y 4 de §8), y hoy ninguno la manda.

## Verificación (local, sin AWS)

| Comando | Resultado |
|---|---|
| `git rev-parse origin/main` al llegar | `fa76e745`, igual que HEAD |
| `terraform fmt -check -diff secrets.tf` (1.9.8, la versión de `ci.yaml`, bajada de releases.hashicorp.com y con SHA256SUMS OK) | exit 0 |
| `terraform fmt -check -recursive terraform` | exit 3, pero **ya estaba en rojo en `fa76e745`** con los mismos cuatro archivos, ninguno tocado acá: `cicd-iam-policy.tf`, `cloudwatch-near-metrics.tf`, `observability.tf`, `variables.tf`. El CI no corre fmt |
| `terraform init -backend=false` + `terraform validate`, sobre una copia de `HEAD` y otra del árbol con el cambio | `Success! The configuration is valid.` en las dos. Va sobre copias porque el `.terraform.lock.hcl` versionado no tiene el hash `h1:` de darwin_arm64: con `-lockfile=readonly` `validate` lo rechaza, y sin esa opción se escribiría el lock |
| Reglas 1, 1b, 2 y 3 de `no-account-id.yml` (mismas regex, en Python porque el grep de macOS no tiene `-P`) sobre las líneas agregadas | OK, 0 hits |
| Las mismas reglas sobre el árbol entero (1059 archivos, 417 docs) | OK, 0 hits |
| Controles positivos de esas regex | Dan hit en la regla 3 el nombre del secreto de RPC de mainnet con un sufijo de 6 caracteres pegado, y también `<prefijo del secreto>-sha256`. Un ARN con cuenta da hit en la regla 1. Los literales no se escriben acá porque este archivo también pasa por el gate |
| Cruce de nombres `rate_policy.rs` ↔ `secrets.tf` | 4 de 4 |
| `git grep UVD_STACK -- tests scripts src`, y qué test lee `secrets.tf` | Ningún test lee `secrets.tf`. Los tests de Rust que leen terraform solo leen `alerts.tf` (`src/readiness.rs:1177,1273`). No se compiló Rust porque el diff no toca `src/` |

## Para c0der

1. **Antes de mergear**, en el job "Terraform plan (drift gate)" del PR, la sección "Pending, but a deploy applies
   these" debería mostrar tres filas: `update` de `aws_iam_role_policy.secrets_access`, reemplazo de
   `aws_ecs_task_definition.facilitator` y `update` de `aws_ecs_service.facilitator`. Nada de este diff tendría que
   aparecer en "Outside the deploy's reach".
2. **Después del deploy:**

   ```bash
   curl -s https://facilitator.ultravioletadao.xyz/config | jq '.stackIdentities'
   ```

   Esperado: `active: 4` y en `services` los cuatro nombres, `execution-market`, `karmakadabra`, `describe-net` y
   `meshrelay`, cada uno con `active: true` y `credentials: 1`. Si alguno sale inactivo, la línea
   `stack key digest skipped` del log nombra el servicio y la variable.
3. Lo que sigue es el paso 3 de §8 (SDK py y ts con `stack_key` / `stackKey`) y después el 4 (clientes, EM primero).
