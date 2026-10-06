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
- **Falta:** mutaciones, refutador adversarial, merge de `origin/main`, preflight sobre la
  unión, UN push y UN PR.
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

1. Antes del merge (el CI de deploy solo aplica `-target` sobre el servicio y la task
   definition; la política inline documentada en `docs/CICD_SETUP.md` no incluye
   `ecs:UpdateCluster*`): aplicar a mano B8
   `-target=aws_cloudwatch_metric_alarm.orphan_no_running_tasks -target=aws_ecs_cluster.main`.
2. Antes del merge, si se quiere B5 en su propia ventana: comprobar en los paneles de los
   proveedores de RPC que no haya allowlist por IP de salida. El merge mueve las tareas a
   las subredes públicas (`aws_ecs_service.facilitator` está en el `-target` del CI); el
   NAT sigue vivo hasta un apply completo.
3. Apply completo: destruye NAT + EIP (B5), los dashboards, la alarma y los 4 repos ECR
   (lote A). La ruta `0.0.0.0/0` vieja de las tablas privadas queda como blackhole;
   borrarla con `aws ec2 delete-route` es opcional.
4. B15: `scripts/ecr_rollback_anchors.py --tag`, después `--preview` en 0, y recién ahí
   `enable_facilitator_ecr_lifecycle = true` + apply.
5. Después del deploy de B3: leer `tokio worker threads` en `/ecs/facilitator-production`;
   `workers=1` → volver a 1024/2048.
