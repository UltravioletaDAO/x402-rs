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
- **No hecho, a propósito:** B18 (ver abajo). Ningún `terraform apply`, deploy ni merge.
  Apagar el NAT NO es parte de este PR: es el paso e de abajo, en un PR aparte.
- **Falta:** nada en este PR. c0der decide el merge.
- **Próximo paso para c0der:** «Orden de aplicación», paso a.

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

Lo demás NO lo aplica el deploy y el drift gate lo lista como «Unapplied infrastructure»
(esperado): endpoints gateway con la tabla pública (B5), dashboards, alarma y 4 repos ECR
(lote A). Con `enable_nat_gateway = true` la configuración del NAT, su EIP y la ruta
privada queda igual a la de `main`, así que no deberían figurar (inferido del diff; el
`plan` necesita credenciales y no se corrió acá).

## Orden de aplicación (c0der)

a. Antes del merge: confirmar en los paneles de los RPC pagos que no haya allowlist por la
   IP del NAT.
b. Opcional, antes o después del merge (inocuo en cualquier orden):
   `terraform apply -target=aws_vpc_endpoint.dynamodb -target=aws_vpc_endpoint.s3`.
c. Merge = deploy (`-target`): task def B3, servicio B5 (tareas a subred pública, NAT
   VIVO), cluster (Insights off), Lambda B7, alarma B8.
d. Verificar antes de seguir: `aws ecs wait services-stable` OK; `describe-tasks`: 2
   tareas RUNNING, cada una en una subred pública y con IP pública; ninguna tarea en subred
   privada; `HealthyHostCount` = 2; `/health` OK; un settle de prueba; log
   `tokio worker threads` (`workers=1` → evaluar volver a 1024/2048); NAT
   `ActiveConnectionCount` = 0 durante 15 min.
e. Recién ahí, PR aparte con `enable_nat_gateway = false` (default de `variables.tf`,
   `production.auto.tfvars` y la aserción de
   `test_nat_stays_on_in_the_change_that_moves_the_tasks`) → apply completo revisado
   (`terraform plan -out=t.tfplan`, revisar, `apply t.tfplan`): borra NAT + EIP + ruta
   privada y lote A. Antes, vaciar los 4 repos ECR de observability (`otel_collector`,
   `prometheus`, `tempo`, `grafana`: no tienen `force_delete`, el destroy falla con
   `RepositoryNotEmptyException`) o sacarlos con `terraform state rm` si se quieren
   conservar.
f. B15 al final: `python3 scripts/ecr_rollback_anchors.py --tag`, después `--preview` en 0,
   y recién ahí `enable_facilitator_ecr_lifecycle = true` + apply.

Rollback de B5 después de e: `enable_nat_gateway = true` PRIMERO (apply, NAT sano), y
después `ecs_tasks_in_public_subnets = false` en un segundo apply. Nunca los dos en el
mismo apply. Antes de e, el rollback de B5 es solo `ecs_tasks_in_public_subnets = false`
(el NAT sigue vivo). El resto de los rollbacks: ver la tabla del PR (todos por tfvars
salvo B7, que es revertir el commit y redeployar la Lambda).

## Branch protection de `main`

Medido 2026-10-06 con la API pública de GitHub: `GET /repos/UltravioletaDAO/x402-rs/branches/main`
devuelve `protected: false` y `required_status_checks.contexts: []`; `GET
/repos/.../rules/branches/main` devuelve `[]` (ningún ruleset). El check
`Terraform plan (drift gate)` no es requerido para mergear. Con b aplicado y el NAT en
`true`, lo único que queda rojo en el gate es lote A, que también se puede aplicar antes.
