# COSTO-X402 — recorte de costos AWS del facilitador

## Estado: qué está hecho, qué falta, próximo paso

- **Hecho** (rama `devin/COSTO-X402-recorte-aws`, PR #115, un commit por recorte):
  B3 (512 CPU / 1024 MB, dos tareas), B5 (tareas en subred pública con IP pública; el NAT
  queda detrás de `enable_nat_gateway`, **prendido en este PR**), B7 (Lambda publica
  `Facilitator/Chains` solo de las 16 redes con alarma), B8 (Container Insights apagado,
  alarma `no-running-tasks` sobre `AWS/ApplicationELB HealthyHostCount`), B15 (lifecycle de
  ECR `facilitator` detrás de `enable_facilitator_ecr_lifecycle = false` +
  `scripts/ecr_rollback_anchors.py`), lote A (dashboards v2-migration y near-operations,
  alarma v1-traffic-sudden-drop y 4 repos ECR de observability fuera del TF).
- **Ronda R1 (REF-X402-115) hecha:** `enable_nat_gateway = true` en default y tfvars (P2-1);
  test que lo fija y tests del egress del writer lease y del endpoint de Secrets Manager
  (P3-3, M4 y M5 del refutador); alcance del deploy corregido (P3-1: el merge aplica
  también `aws_ecs_cluster.main`); orden de aplicación con vaciado de repos ECR (P3-2).
- **Ronda R2 (VER-X402-115) hecha, solo documentación:** rollback de B5 después de e
  reescrito (NAT a mano primero, verificación, recién después las subredes, en dos PRs);
  corregido «el drift gate lo lista» (hoy el gate muere sin listar: ver «Rojos de base»);
  paso e con el orden merge → `plan -out` desde `main` y las filas esperadas. La guarda
  mecánica del NAT (punto 2 del refutador) quedó para el PR del paso e: hecha en
  X402-NAT-OFF (abajo).
- **Ronda R3 (COSTO-X402-R3) hecha: el drift gate lista en vez de morir.** `row()` del
  step `Report changes the deploy will never apply` (`.github/workflows/ci.yaml`) ya no
  falla con un recurso borrado (fila `delete` con «(removed from the configuration)» y
  «(whole resource)»); las anotaciones `Unapplied infrastructure` y la lista
  `Drift the pipeline will never apply:` salen ANTES del summary; el gate sigue en rojo
  (`exit 1`) si `uncovered.addrs` no está vacío. En el step `Plan`, `|| code=$?` en el plan
  completo. Tests: `tests/scripts/test_drift_gate_report.py` (ver «R3: drift gate»).
- **No hecho, a propósito:** B18 (ver abajo). Ningún `terraform apply`, deploy ni merge.
  Apagar el NAT NO es parte de este PR: es el paso e de abajo, en un PR aparte.
- **PR #115 mergeado** (`7bf9df9b`). Medido por c0der el 2026-10-08 ~01:55Z, 24 h
  después del COMPLETED del deploy: las 2 tareas (task def 475) RUNNING y HEALTHY, en
  subredes cuya `0.0.0.0/0` va al internet gateway, cada ENI con IP pública; ALB en 24 h:
  20.812 requests, `HTTPCode_ELB_5XX` = 0, `HTTPCode_Target_5XX` = 4 (uno cada ~5 h a los
  :47, el mismo patrón, y más alto, que del 4 al 6 de octubre, antes de mover las tareas).
  El resto de d (NAT `ActiveConnectionCount` en 0, settle de prueba, log de `workers`), a
  y b no constan en esa medición: los confirma c0der. El run de R3 listó lo que el deploy
  no aplica (lista medida en «R3: drift gate»).
- **X402-NAT-OFF (paso e, PR aparte):** `enable_nat_gateway = false` en `variables.tf` y
  `production.auto.tfvars`, con dos guardas que leen AWS, `route = []` explícito en la
  tabla privada, `tests/nat_guard.tftest.hcl` y su paso en el CI. Ver «X402-NAT-OFF: el
  paso e».
- **Próximo paso para c0der:** «Orden de aplicación», paso e.1 (vaciar o `state rm` de
  los 4 repos ECR de observability, si lote A no se aplicó), después el merge de
  X402-NAT-OFF y el `plan -out` de e.3.

## Refutaciones del encargo

- **B18 ya está hecho para lo estático y NO se hace para lo dinámico.**
  `src/handlers.rs::precompressed_static` (montado en `src/main.rs`) ya sirve gzip
  calculado una vez por proceso para todo documento compilado (`/`, `llms.txt`, hojas,
  scripts). Lo que no se comprime son las respuestas armadas por request (`/supported`,
  `/discovery/resources`, `/networks.json`, `/api-docs/openapi.json`), y el propio código
  documenta por qué: gzip por request cuesta CPU en los mismos workers que liquidan pagos
  (medido 2026-09-13: p95 de un catálogo de 194 KB de 0,51 ms a 1,90 ms al nivel más
  rápido; dos P0 previos por CPU, 2.21.1 y 2.21.2). Con B3 bajando a 0,5 vCPU ese trade es
  peor, por 2-3 USD/mes. Queda como pregunta para c0der, no como código.
- **B5 «cambiar dos líneas» rompería el plan.** Las tablas de ruteo privadas tienen
  `count = nat_count` y las indexan las asociaciones y los endpoints gateway (DynamoDB,
  S3); el endpoint de Secrets Manager vive en las subredes privadas. Se conservan
  subredes y tablas privadas, la ruta por defecto pasa a ser un bloque `dynamic` y los
  endpoints gateway se asocian también a la tabla pública.
- **B5 «el SG solo acepta 8080 desde el ALB» es falso hoy y así debe seguir:** el SG
  `ecs_tasks` tiene además la regla `self` en 8080 del writer-lease entre las dos tareas
  (`src/writer_lease.rs`), de entrada y de salida. Se conserva; no hay ingreso desde
  0.0.0.0/0.
- **B5 allowlists por EIP del NAT:** no hay ninguna en el repo (grep de la EIP, de
  allowlists de RPC y de `aws_eip.nat` fuera de main.tf). La única allowlist de IP es la de
  clientes del rate limiting (entrada, no salida). Lo que el repo no puede ver (un
  proveedor de RPC con allowlist configurada en su panel) lo tiene que confirmar c0der.

## Alcance del deploy (qué aplica el merge)

El deploy de `main` aplica con `-target` (lista en `.github/workflows/ci.yaml`), y un
`-target` arrastra sus dependencias. De este PR entran en ese apply:

- `aws_ecs_task_definition.facilitator` (B3).
- `aws_ecs_service.facilitator` (B5: las tareas pasan a las subredes públicas con IP
  pública; rolling deployment, el NAT sigue vivo).
- `aws_ecs_cluster.main` (B8: dependencia del servicio, `main.tf` `cluster =
  aws_ecs_cluster.main.id`): **Container Insights se apaga con el merge**. Entre el step
  `Terraform apply (roll ECS…)` y `Deploy observability` la alarma vieja sigue leyendo
  `ECS/ContainerInsights` (2x300 s, `breaching`); en el camino normal son minutos y no
  dispara, pero si el step de alarmas falla queda una página falsa de "no running tasks".
- `aws_lambda_function.balances` (B7) y `aws_cloudwatch_metric_alarm.orphan_no_running_tasks`
  (B8, step `Deploy observability`).

Lo demás NO lo aplica el deploy: endpoints gateway con la tabla pública (B5), dashboards,
alarma y 4 repos ECR (lote A). **El drift gate los lista desde R3** (antes moría en la
primera fila de un recurso borrado; ver «Rojos de base») y sale rojo con ellos: es el rojo
esperado de este PR. Para leer el plan a mano:

```
terraform plan -input=false -lock=false -out=full.tfplan
terraform show -json full.tfplan | jq -r '.resource_changes[]
  | select(.change.actions != ["no-op"] and .change.actions != ["read"])
  | "\(.change.actions | join("+")) \(.address)"'
```

Con `enable_nat_gateway = true` la configuración del NAT, su EIP y la ruta privada queda
igual a la de `main`: medido, el gate del run de R3 no listó ninguna fila de NAT, EIP ni
`aws_route_table.private` (lista en «R3: drift gate»).

## Orden de aplicación (c0der)

a. Antes del merge: confirmar en los paneles de los RPC pagos que no haya allowlist por la
   IP del NAT.
b. Opcional, antes o después del merge de #115 (inocuo en cualquier orden):
   `terraform apply -target=aws_vpc_endpoint.dynamodb -target=aws_vpc_endpoint.s3`.
   **Pero antes del merge de X402-NAT-OFF, nunca después:** ese `-target` arrastra
   `aws_route_table.private`, `aws_nat_gateway.main` y `aws_eip.nat` (grafo), así que con
   `enable_nat_gateway = false` en `main` planea el destroy del NAT fuera del plan revisado
   de e.3. Después de ese merge, b es parte de e.3 y no se corre aparte.
c. Merge = deploy (`-target`): task def B3, servicio B5 (tareas a subred pública, NAT
   VIVO), cluster (Insights off), Lambda B7, alarma B8.
d. Verificar antes de seguir: `aws ecs wait services-stable` OK; `describe-tasks`: 2
   tareas RUNNING, cada una en una subred pública y con IP pública; ninguna tarea en subred
   privada; `HealthyHostCount` = 2; `/health` OK; un settle de prueba; log
   `tokio worker threads` (`workers=1` → evaluar volver a 1024/2048); NAT
   `ActiveConnectionCount` = 0 durante 15 min.
e. Recién ahí, PR aparte con `enable_nat_gateway = false` (default de `variables.tf`,
   `production.auto.tfvars` y la aserción, que pasó a llamarse
   `test_the_nat_is_off_in_its_own_change`): es X402-NAT-OFF. Orden:
   1. Antes, vaciar los 4 repos ECR de observability (`otel_collector`, `prometheus`,
      `tempo`, `grafana`: no tienen `force_delete`, el destroy falla con
      `RepositoryNotEmptyException`) o sacarlos con `terraform state rm` si se quieren
      conservar.
   2. **Merge del PR del NAT primero.** El deploy no toca el NAT (fuera de todo
      `-target`), así que el merge solo deja `main` pidiendo el NAT apagado.
   3. Después, `terraform plan -out=t.tfplan` **desde `main` en el SHA mergeado** (no desde
      la rama del PR), revisar y `apply t.tfplan`.
   4. Si el plan para en la postcondición de `data.aws_network_interfaces.private_subnets`
      («these ENIs live in a subnet that routes through the private table»), es G2: algo
      que no es un endpoint de VPC tiene una ENI en una subred asociada a la tabla privada.
      No aplicar: ver qué es (`aws ec2 describe-network-interfaces --network-interface-ids
      <ids>`) y volver a c0der. Si para con «query returned no results» en
      `data.aws_route_table.private_without_nat`, la tabla privada no tiene el tag `Name`
      que el código espera: también un stop.
   5. Filas esperadas en `t.tfplan`, y ninguna otra:
      - las lecturas de G2: `data.aws_route_table.private_without_nat[0]` y
        `data.aws_network_interfaces.{private_subnets,private_subnet_endpoints}[0]`, leídas
        EN EL PLAN. Si alguna dice `will be read during apply`, es un stop: G2 se evaluaría
        cuando el NAT ya no existe;
      - destroy de `aws_nat_gateway.main[0]` y `aws_eip.nat[0]`;
      - update in-place de `aws_route_table.private[0]`: sale la ruta `0.0.0.0/0` al NAT
        (desde X402-NAT-OFF `route = []` explícito la borra; antes quedaba en blackhole).
        En el apply Terraform destruye primero el NAT y después actualiza la tabla: la
        ruta queda en blackhole ese rato, sin nadie detrás (lo verificó G2 en el plan).
        Si la fila no figura, o es un destroy o replace de la tabla, es un stop;
      - lote A: destroy de `aws_cloudwatch_dashboard.x402_v2_migration`,
        `aws_cloudwatch_dashboard.near_operations`,
        `aws_cloudwatch_metric_alarm.v1_traffic_unexpected_drop` y
        `aws_ecr_repository.{otel_collector,prometheus,tempo,grafana}` (si no se hizo
        `state rm`), si no se aplicaron antes;
      - las asociaciones de los endpoints gateway: update de `route_table_ids` en
        `aws_vpc_endpoint.dynamodb` y `aws_vpc_endpoint.s3` (si b no se aplicó).
      **Cualquier otra fila es un stop:** no aplicar, volver a c0der.
f. B15 al final: `python3 scripts/ecr_rollback_anchors.py --tag`, después `--preview` en 0,
   y recién ahí `enable_facilitator_ecr_lifecycle = true` + apply.

### Rollback de B5

**Antes de e:** solo `ecs_tasks_in_public_subnets = false` (el NAT sigue vivo; lo aplica
el deploy).

**Después de e** (el NAT ya no existe y el deploy nunca lo crea: `aws_nat_gateway`,
`aws_eip` y `aws_route_table` no están en ningún `-target`):

1. **Apply A MANO del NAT.** Desde una rama con `enable_nat_gateway = true` (default de
   `variables.tf` y `production.auto.tfvars`):
   `terraform plan -out=nat.tfplan -target=aws_route_table.private` (arrastra
   `aws_nat_gateway.main` y `aws_eip.nat`), revisar el plan y `terraform apply nat.tfplan`.
2. **Verificar** que el NAT esté `available` (`aws ec2 describe-nat-gateways`, filtro por
   la VPC) y que la ruta por defecto (`0.0.0.0/0`) de la tabla privada esté `active` (no
   `blackhole`) y apunte al NAT nuevo (`aws ec2 describe-route-tables`, `NatGatewayId` =
   el id del paso 1).
3. **Recién ahí**, merge de esa rama, y después **otro PR** con
   `ecs_tasks_in_public_subnets = false` (este lo aplica el deploy: el servicio está en el
   `-target`).

**NO revertir el PR #115 con git revert después de e sin hacer antes (1) y (2).** Un
revert vuelve a la configuración de `main` de una sola vez (tareas en las subredes
privadas, NAT sin flag): el deploy mueve las tareas, que está en su `-target`, y no crea el
NAT, que no lo está: **un PR que cambia los dos valores juntos deja las tareas sin salida.**

Desde X402-NAT-OFF el deploy ya no llega a moverlas en esos órdenes, mientras la guarda
siga en el árbol: con `ecs_tasks_in_public_subnets = false`, la segunda precondición de
`aws_ecs_service.facilitator` (G3) exige que cada `aws_subnet.private` esté asociada en AWS
a una tabla privada (`data.aws_route_table.private_for_tasks`, por tag) con una ruta
`0.0.0.0/0` a un NAT que AWS reporte `available` (`data.aws_nat_gateways.available`). Si
no, el plan del deploy para antes de tocar el servicio: también si el NAT existe pero sin
su ruta (un apply con `-target=aws_nat_gateway.main`, o hecho a mano). Un `git revert` de
#115 tampoco es un rollback: la guarda lee `var.ecs_tasks_in_public_subnets`, que #115
introdujo, así que el revert choca con ella. El orden (1), (2), (3) sigue siendo el único
camino.

El resto de los rollbacks: ver la tabla del PR (todos por tfvars salvo B7, que es revertir
el commit y redeployar la Lambda).

## Branch protection de `main`

Medido 2026-10-06 con la API pública de GitHub: `GET /repos/UltravioletaDAO/x402-rs/branches/main`
devuelve `protected: false` y `required_status_checks.contexts: []`; `GET
/repos/.../rules/branches/main` devuelve `[]` (ningún ruleset). El check
`Terraform plan (drift gate)` no es requerido para mergear. Con b aplicado y el NAT en
`true`, lo único que el plan completo debería mostrar fuera del deploy es lote A (que
también se puede aplicar antes); desde R3 el gate lo lista en las anotaciones y en
`Drift the pipeline will never apply:` (antes iba rojo sin anotaciones).

## Rojos de base

- **Drift gate, step `Report changes the deploy will never apply` (`.github/workflows/ci.yaml`,
  función `row()`, línea 345):** `file=$(grep -lE ... ./*.tf | head -1 | sed ...)` corre
  con `set -o pipefail` y el `bash -e` del runner. Para una dirección que ya no está
  declarada en ningún `.tf` (un recurso borrado, como lote A) el `grep` sale 1, la
  asignación falla y el step muere en esa fila: sin las anotaciones `::error
  title=Unapplied infrastructure::`, sin `Drift the pipeline will never apply:` y con la
  tabla del summary cortada. Reproducido local con un `row()` mínimo bajo `bash -e`
  (sale 1 en la primera dirección sin `.tf`; la fila siguiente y el resto del step no se
  escriben). Coherente con el run del head `e3ba941e` (job `Terraform plan (drift
  gate)`): el step termina en `Process completed with exit code 1.` sin ninguna línea de
  salida (que muera justo en `row()` es inferido: el log no dice la línea). El bug estaba
  en `main` y no lo introdujo este PR; lo destapó lote A, el primer borrado de recursos. **Arreglado en este PR en R3** (c0der decidió el arreglo dentro de #115): ver
  «R3: drift gate».
- Observado en el mismo run: el step anterior imprime `Full plan exit code: 0`, aunque
  este PR borra recursos (con `-detailed-exitcode`, cambios = 2). **Diagnosticado en R3:**
  `hashicorp/setup-terraform@v3` instala un wrapper (`terraform_wrapper` por defecto) que
  devuelve 0 cuando terraform sale 2 (`wrapper/terraform.js`: `if (exitCode === 0 ||
  exitCode === 2) return;`). No es un bug del gate (el step `Report` no usa `code`), pero
  el `code=$?` pelado dependía de ese wrapper: sin él, `bash -e` cortaba el step en 2 y en
  1 antes de imprimir el log. R3 lo deja en `|| code=$?`.

## R3: drift gate

**Bug** (en `main`): en `row()`, `file=$(grep -lE ... ./*.tf | head -1 | sed ...)` bajo
`set -o pipefail` y el `bash -e` del runner. Un recurso borrado del `.tf` (lote A) no
matchea, `grep` sale 1 y el step muere en esa fila, antes de las anotaciones y la lista.

**Cambio** (`.github/workflows/ci.yaml`, job `plan`):
- `row()`: `grep ... || true`, primera línea con `read` (sin `head`/`sed` en tubería),
  acción con `action_of()` (`first // "?"`, no muere sin fila); sin `.tf`: «(removed from
  the configuration)» si la acción tiene `delete`, `?` si no; `delete` puro: atributos
  «(whole resource)»; el `jq` de atributos con `|| attrs="?"`.
- Anotaciones `::error title=Unapplied infrastructure::<addr> (<acción>)` y la lista
  `<acción> <addr>` ANTES de armar el summary: si algo del summary fallara, el rojo ya dijo
  sobre qué es.
- El `exit 1` con `uncovered.addrs` no vacío queda igual (decisión REF-X402-115).
- Step `Plan`: `code=0; terraform plan ... || code=$?`.

**Tests** (`tests/scripts/test_drift_gate_report.py`, 12): ejecutan el texto de los dos
steps tal cual está en `ci.yaml`, con `bash -e`, un `terraform` falso que imprime planes de
fixture y `jq`/`grep`/`comm` reales. Sin AWS ni Terraform. Un test compara la extracción por
texto con la de PyYAML y fija que el step no declara `shell:`.

**Bordes revisados:** recurso borrado sin `.tf` (fuera y dentro del alcance del deploy);
`count = 0` (`aws_nat_gateway.main[0]`: la dirección con índice sigue declarada en
`main.tf`); replace `delete+create` (lista atributos, no «whole resource»); `update` de un
recurso sin `.tf` (`?`); solo borrados (sigue rojo); plan limpio con `no-op`/`read`
(verde, `**Clean.**`); plan completo con 0, 2 y 1 (el 1 imprime el log). Que las
direcciones de fixture no estén entre los `-target` del deploy (test propio). Sin
mayúsculas, espacios ni rutas: las direcciones las arma Terraform.

**Mutaciones** (a mano, revertidas con `git checkout --`, árbol limpio):

| # | Mutación | Cae |
|---|---|---|
| M0 | `ci.yaml` de antes de R3 entero (la versión que crashea) | 7 tests, entre ellos `test_a_destroyed_resource_is_a_row_not_a_crash` |
| M1 | `file=$(grep ... \| head -1 \| sed ...)` de `main` en `row()` | `test_a_destroyed_resource_is_a_row_not_a_crash`, `test_only_destroys_still_go_red`, `test_a_destroy_inside_the_deploys_reach_is_a_pending_row`, `test_an_undeclared_change_that_is_not_a_destroy_says_unknown` |
| M2 | sin `\|\| true` en el `grep` | los mismos 4 |
| M3 | ablandar: sin `exit 1` | 5 tests (`test_only_destroys_still_go_red`, ...) |
| M4 | borrado sin `.tf` → `?` | 3 tests |
| M5 | `terraform plan` pelado + `code=$?` | `test_a_full_plan_with_changes_does_not_end_the_step`, `test_a_broken_full_plan_prints_its_log` |
| M6 | anotación sin acción | `test_a_destroyed_resource_is_a_row_not_a_crash` |
| M7 | lista con `cat uncovered.addrs` | 2 tests |
| M8 | sin la rama «(whole resource)» | 3 tests |
| M9 | anotaciones solo después del summary (orden de `main`) | 2 tests |

**Lista exacta que imprimió el gate en rojo** (medida por VER2-X402-115 en el log del job
`Terraform plan (drift gate)` del run del head `ecfe0532`, 9 anotaciones `Unapplied
infrastructure` y después `Process completed with exit code 1.`):

```
Drift the pipeline will never apply:
delete aws_cloudwatch_dashboard.near_operations
delete aws_cloudwatch_dashboard.x402_v2_migration
delete aws_cloudwatch_metric_alarm.v1_traffic_unexpected_drop
delete aws_ecr_repository.grafana
delete aws_ecr_repository.otel_collector
delete aws_ecr_repository.prometheus
delete aws_ecr_repository.tempo
update aws_vpc_endpoint.dynamodb
update aws_vpc_endpoint.s3
```

Ninguna fila de NAT, EIP ni `aws_route_table.private`. La lista depende del estado de AWS
al momento del plan: si c0der aplicó a mano el paso b o lote A, esas filas ya no salen.

## X402-NAT-OFF: el paso e

**Cambio** (`terraform/environments/production/`):
- `enable_nat_gateway = false` en el default de `variables.tf` y en
  `production.auto.tfvars` (tienen que coincidir: `test_defaults_match_tfvars`).
- `aws_route_table.private`: `route` pasa de un bloque `dynamic "route"` a sintaxis de
  atributo, `[]` explícito con el NAT apagado. `route` está en modo attributes-as-blocks:
  cero bloques es «no tocar las rutas» (la `0.0.0.0/0` quedaba en blackhole) y solo `[]` la
  borra. El provider (5.100.0, `flattenRoutes`) no lee a `route` las rutas `vpce-` de los
  endpoints gateway ni la `local`, así que `[]` deja esas en paz. Con el NAT prendido el
  valor es el mismo que daba el bloque (una ruta, los demás atributos en null).
- **Por qué las guardas no referencian nada gestionado** (refutación de este PR, P2-1):
  una data source que referencia un recurso gestionado con un cambio pendiente se lee
  durante el apply, y en el apply que saca el NAT Terraform destruye el NAT ANTES de
  actualizar la tabla que lo referenciaba. Una guarda leída ahí frena cuando el daño ya
  está hecho. Las tablas privadas se buscan por su tag `Name` desde variables
  (`data.aws_route_table`, que falla si no encuentra exactamente una), y todo lo demás sale
  de ahí: se leen siempre en el plan, aunque las subredes tengan un cambio pendiente.
- **Guarda G2** (lo que pide el encargo: que el plan no pueda sacar el NAT mientras algo
  dependa de una subred privada): `postcondition` de
  `data.aws_network_interfaces.private_subnets`. Con el NAT apagado toma las subredes
  asociadas a las tablas privadas (`data.aws_route_table.private_without_nat`: las que
  pierden la salida, sean o no `aws_subnet.private`), lee sus ENIs y las de tipo
  `vpc_endpoint` (`data.aws_network_interfaces.private_subnet_endpoints`), y para si queda
  alguna que no sea de un endpoint. No pregunta qué es: una tarea, una Lambda o algo hecho a
  mano frenan igual, y el error lista los ids. `aws_eip.nat` y `aws_nat_gateway.main`
  dependen (`depends_on`) de esa data source, así que cualquier plan que los toque la lee:
  el completo de e.3, el del drift gate, `-target=aws_nat_gateway.main`, o un `-target` que
  arrastre la tabla privada (los endpoints gateway del paso b).
- **Guarda G3** (VER-X402-115, punto 2, y P2-2 de la refutación de este PR): segunda
  precondición en `aws_ecs_service.facilitator`. Con `ecs_tasks_in_public_subnets = false`
  exige que cada `aws_subnet.private` esté asociada a una tabla privada
  (`data.aws_route_table.private_for_tasks`) con una ruta `0.0.0.0/0` a un NAT que AWS
  reporte `available` (`data.aws_nat_gateways.available`). Un NAT que existe sin su ruta,
  una tabla sin NAT o una subred asociada a otra tabla frenan el plan del deploy antes de
  mover las tareas.
- Las data sources tienen `count` por modo: con los valores de este PR el deploy no lee
  nada de G3 (`count = 0`) y solo el plan completo (drift gate, apply a mano) lee G2.
  Permisos: `ec2:DescribeRouteTables`, `ec2:DescribeNetworkInterfaces` y
  `ec2:DescribeNatGateways`, que la identidad del CI tiene por `ReadOnlyAccess` según el
  comentario de `cicd-iam-policy.tf` (no verificado contra IAM acá). El drift gate de este
  PR es la primera lectura real de G2.
- Grafo (`terraform graph -type=plan`, 1.9.8): los 38 `-target` del deploy arrastran, de
  esta familia, solo las dos data sources de G3; el rollback
  `-target=aws_route_table.private` arrastra lo mismo que en la base (EIP, IGW, NAT, subred
  pública, VPC) más las data sources de G2, que con el NAT prendido no se leen.

**Tests:**
- `tests/nat_guard.tftest.hcl` (15 corridas, `mock_provider "aws"`, sin credenciales,
  1.9.8 y 1.14.3): valores comprometidos sin NAT ni EIP y sin ruta; una ENI de tarea y una
  de Lambda frenan el plan; G2 frena aunque las subredes privadas tengan un cambio
  pendiente; un plan con `-target=aws_nat_gateway.main` también lee G2; una tabla sin
  subredes no lee ENIs; NAT prendido = una ruta al NAT; apagar desde un estado con NAT borra
  la ruta; G1 por variables; G3 frena sin NAT disponible, con NAT pero sin ruta y con una
  subred fuera de la tabla, y pasa con el NAT ruteado; con las tareas públicas G3 no lee
  nada. Paso nuevo del CI `Terraform NAT guards` (job `Build & test`, Terraform 1.9.8), que
  bloquea el deploy como el resto del job.
- `tests/scripts/test_ci_cost_defaults.py`: `test_the_nat_is_off_in_its_own_change`
  (antes `test_nat_stays_on_in_the_change_that_moves_the_tasks`),
  `test_whatever_runs_in_a_private_subnet_is_guarded_by_the_nat` (todo bloque que use
  `aws_subnet.private` lleva una precondición sobre `var.enable_nat_gateway`, salvo la lista
  cerrada de lo que no sale a internet), `test_the_live_nat_guards_read_the_right_filters`
  (lo que un mock no ve: el tag de las tablas, `vpc_endpoint`, `state = available`),
  `test_the_live_nat_guards_reference_nothing_managed` (P2-1 hecho test) y
  `test_the_nat_guards_run_in_ci`.

**Límites:**
- Los mocks prueban la evaluación de la config, no lo que hace el provider: que `[]` sea
  un update y no un replace de la tabla sale del esquema (`route` no es ForceNew) y del
  fuente de `flattenRoutes`, no de un test.
- G3, en un ambiente nuevo con las tareas privadas, para en el primer apply (ni la tabla
  ni el NAT existen): primero `-target=aws_route_table.private`, después el resto. En
  producción no aplica.
- G3 no distingue un NAT privado (`connectivity_type = private`) de uno público: uno así
  hecho a mano y ruteado desde la tabla privada pasaría la guarda.
- Una ENI hecha a mano en una subred asociada a la tabla privada también frena G2: es a
  propósito, se mira antes de sacar la salida.
- `test_whatever_runs_in_a_private_subnet_is_guarded_by_the_nat` busca el literal
  `aws_subnet.private`: un `local` que pase las subredes con otro nombre lo esquiva. G2 ve
  igual lo que eso cree, pero recién en el plan siguiente a crearlo.
