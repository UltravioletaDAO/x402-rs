# COSTO-X402 — recorte de costos AWS del facilitador

## Estado: qué está hecho, qué falta, próximo paso

- **Hecho** (rama `devin/COSTO-X402-recorte-aws`, un commit por recorte, sin push todavía):
  B3 (512 CPU / 1024 MB, dos tareas), B5 (tareas en subred pública, NAT detrás de
  `enable_nat_gateway = false`), B7 (Lambda publica `Facilitator/Chains` solo de las 16
  redes con alarma), B8 (Container Insights apagado, alarma `no-running-tasks` sobre
  `AWS/ApplicationELB HealthyHostCount`), B15 (lifecycle de ECR `facilitator` detrás de
  `enable_facilitator_ecr_lifecycle = false` + `scripts/ecr_rollback_anchors.py`), lote A
  (dashboards v2-migration y near-operations, alarma v1-traffic-sudden-drop y 4 repos ECR
  de observability fuera del TF). Línea base verde en `origin/main` (c3b694b0).
- **No hecho, a propósito:** B18 (ver abajo). Ningún `terraform apply`, deploy ni merge.
- **Hecho también:** 23 mutaciones (todas en rojo), merge de `origin/main` (al día).
- **Falta:** refutador adversarial, preflight sobre la unión, UN push y UN PR.
- **Próximo paso para c0der (en la tanda, no en esta sesión):** ver «Orden de aplicación».

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
  (`src/writer_lease.rs`). Se conserva; no hay ingreso desde 0.0.0.0/0.
- **B5 allowlists por EIP del NAT:** no hay ninguna en el repo (grep de la EIP, de
  allowlists de RPC y de `aws_eip.nat` fuera de main.tf). La única allowlist de IP es la de
  clientes del rate limiting (entrada, no salida). Lo que el repo no puede ver (un
  proveedor de RPC con allowlist configurada en su panel) lo tiene que confirmar c0der.

## Orden de aplicación (c0der, en la tanda)

El deploy de `main` aplica con `-target` (lista en `.github/workflows/ci.yaml`). De este PR
entran en ese apply: `aws_ecs_task_definition.facilitator` (B3),
`aws_ecs_service.facilitator` (B5: las tareas pasan a las subredes públicas con IP
pública; es un rolling deployment, el NAT sigue vivo), `aws_cloudwatch_metric_alarm.orphan_no_running_tasks`
(B8: la alarma pasa a `HealthyHostCount`, válida con Insights prendido o apagado) y
`aws_lambda_function.balances` (B7). Lo demás NO lo aplica el deploy y el drift gate del
PR lo va a listar como «Unapplied infrastructure» (esperado): NAT + EIP + ruta privada (B5),
`aws_ecs_cluster.main` (B8), endpoints gateway con la tabla pública (B5), dashboards,
alarma y repos ECR (lote A).

1. Antes del merge: confirmar en los paneles de los proveedores de RPC que no haya
   allowlist por IP de salida (el repo no la tiene; ver arriba).
2. Merge = deploy: B3, B5 (tareas), B7, alarma de B8. Verificar `/health`, un settle de
   prueba y el log `tokio worker threads` (`workers=1` → volver a 1024/2048).
3. Apply completo en la tanda (`terraform plan -out=t.tfplan`, revisar, `apply`): borra NAT
   + EIP, la ruta por defecto privada, apaga Container Insights, asocia los endpoints
   gateway a la tabla pública y borra lote A. Antes de borrar el NAT,
   `ActiveConnectionCount` del NAT en 0.
4. B15: `python3 scripts/ecr_rollback_anchors.py --tag`, después `--preview` en 0, y recién
   ahí `enable_facilitator_ecr_lifecycle = true` + apply.

Rollback de cada uno: ver la tabla del PR (todos por tfvars salvo B7, que es revertir el
commit y redeployar la Lambda).
