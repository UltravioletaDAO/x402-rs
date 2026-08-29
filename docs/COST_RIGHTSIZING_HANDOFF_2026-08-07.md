# Handoff: right-sizing de costos del facilitator — 2026-08-07

> **De:** sesión de infraestructura de KarmaKadabra (operador: Saul).
> **Para:** la sesión/equipo del facilitator — los cambios se aplican DESDE ESTE REPO; nada se tocó desde afuera.
> **Contexto:** análisis de costos de todo el stack UVD (reporte en `karmakadabra/docs/reports/EKS_CONSOLIDATION_COST_ANALYSIS_2026-08-07.md`, cifras verificadas contra Cost Explorer). El sistema facilitator ≈ $135/mes con compartidos. Este handoff recorta ~$34-41/mes con cambios que el propio tfvars ya anticipaba. KarmaKadabra ya ejecutó su parte (flota a FARGATE_SPOT); 402milly también; EM tiene su propio handoff en su repo.

## Datos medidos [Cost Explorer + CloudWatch julio 2026]

- Fargate facilitator: **$36.04/mes** (1 vCPU / 2 GB, 1 task, `USE2-Fargate-vCPU-Hours`).
- CPU julio: **avg 1.8% / pico diario máx 91.8%** (ráfagas de ~1 min, presumiblemente settlements).
- Memoria julio: **avg 15.3% / pico 31.5%** de 2048 MB → pico real ≈ **645 MB**.
- Container Insights: ON (`enable_container_insights = true` en tfvars).
- Retención de logs: ya está en 7d ✓ — nada que hacer ahí.

## Cambios propuestos (todos son perillas EXISTENTES de `environments/production/terraform.tfvars`)

### 1. Container Insights OFF (~$10-15/mes)
```hcl
enable_container_insights = false   # era true
```
Las métricas estándar ECS (CPU/Memory del service) se conservan; solo se pierden las per-container de Insights. El cluster de KK ya corre así.

### 2. CPU 1024 → 512 (~$14.8/mes) — el propio tfvars lo anticipa
La línea actual dice literalmente: `task_cpu = 1024  # 1 vCPU (start here, can optimize to 512 after testing)`. El testing ya existe: julio entero con avg 1.8%.
```hcl
task_cpu = 512
```

> **ALTO -- agregado 2026-08-29, drift audit.** Este cambio NO es puramente una decisión de
> costo y podria salir caro. `docs/handoffs/2026-08-20-diagnostico-performance-facilitador.md`
> (seccion "Lo que quedo sin verificar") dejo abierta esta pregunta: `#[tokio::main]` no fija
> `worker_threads`, asi que tokio usa `available_parallelism()`, que en Linux lee
> `sched_getaffinity()` (la mascara de afinidad de CPU) -- **no** la cuota de cgroup. Con 1024
> CPU units (1 vCPU) no se sabe si eso resuelve a 1 worker o a N. Bajar a 512 (media vCPU)
> empuja ese numero hacia 1, y con un solo worker cualquier tramo de codigo sin `.await`
> congela el servidor entero -- no solo el request que lo causo. `task_memory` (item 3 abajo)
> no tiene este problema y es defendible por si solo.
>
> **2026-08-29: se cerro la medicion, no la pregunta.** `src/main.rs` ahora loguea
> `workers=N tokio worker threads` al arrancar (info!, junto al resto de la config
> efectiva). El proximo deploy contesta esto con un `aws logs filter-log-events` -- sin
> ECS Exec, sin investigacion aparte. Leer ese numero antes de agendar este cambio: 1
> worker mantiene 1024, N workers con margen habilita 512 con evidencia en vez de por
> precaucion.

Riesgo honesto: los picos de ~92% (ráfagas de settlement de ~1 min) a 512 se throttlean → un settlement puntual puede tardar algo más (el p95 de mint ya es ~28s por diseño; el throttle agrega poco relativo, pero es del money path — decisión de ustedes). Revert = volver el valor + apply.

### 3. OPCIONAL: memoria 2048 → 1024 (~$7.3/mes) — también anticipado
Línea actual: `task_memory = 2048  # 2 GB (start here, can optimize to 1024 after testing)`. Pico medido 645 MB → quedaría al 63% de 1024. Rust estable, sin picos tipo INC de EM. Más fino que el cambio de CPU; si prefieren un solo cambio a la vez, hagan CPU primero y memoria en una segunda pasada.

### 4. zama-testnet: provisioned concurrency OFF (~$9/mes)
Ese stack NO tiene tfvars → manda el default. En `environments/zama-testnet/variables.tf`:
```hcl
variable "enable_provisioned_concurrency" {
  default = false   # era true — es TESTNET, un cold start no duele
}
```

## ⚠️ DISCREPANCIA DETECTADA — leer ANTES de aplicar

El tfvars declara `use_nat_instance = true` ("$8/month vs $32 NAT Gateway") y `enable_vpc_endpoints = false`, **pero la realidad desplegada muestra lo contrario**: hay un NAT **Gateway** vivo en la VPC del facilitator (verificado 2026-08-07 por CLI read-only), Cost Explorer factura ~$67/mes de `NatGateway-Hours` en us-east-2 (≈ DOS gateways: EM + facilitator), y el inventario vio un VPC endpoint de Secrets Manager activo. Es decir: **el tfvars divergió de lo aplicado** (knobs editados que nunca se aplicaron, o recursos que quedaron de una config anterior).

Consecuencia práctica: **un `terraform plan` va a proponer MÁS que el right-sizing** — posiblemente reemplazar el NAT Gateway por instancia y borrar endpoints. Eso toca el egress del settlement path y NO debe pasar de contrabando dentro de un cambio de costos.

**Procedimiento obligatorio:**
```bash
cd terraform/environments/production
terraform plan   # SIN -var image_tag (image-pin.tf conserva la imagen desplegada en applies pelados)
# LEER EL PLAN COMPLETO. Esperado para ESTE handoff: cluster setting (insights) + task definition
# nueva revisión (cpu/mem) + update del service. Si aparecen NAT/route tables/VPC endpoints:
# PARAR — primero reconciliar la discrepancia (o alinear tfvars a la realidad, o agendar el
# swap de NAT como cambio propio con ventana). El swap de NAT sí ahorra ~$25/mes extra, pero
# es un cambio de red del money path, no un ajuste de tamaño.
terraform apply
# Verificación:
curl -s https://facilitator.ultravioletadao.xyz/health
aws logs filter-log-events --log-group-name /ecs/facilitator-production \
  --filter-pattern "[SETTLEMENT]" --region us-east-2 --max-items 5   # settlements siguen fluyendo
# Vigilar 24-48h: latencia de settlement y memoria de la task (si aplicaron el cambio 3).
```

## Explícitamente FUERA de este handoff

- **Fargate Spot**: el tfvars ya registra la decisión (`use_fargate_spot = false # Facilitator needs stability`) — se respeta; el facilitator es settlement path y su base no se interrumpe.
- **El swap NAT Gateway → NAT instance** (~$25/mes): ver discrepancia arriba — cambio aparte, consciente, con ventana.
- **Poda de métricas custom us-east-2** ($35/mes entre facilitator y EM): requiere curación alarma por alarma; se arma la lista juntos si interesa.

**Ahorro de este handoff: ~$34-41/mes** (insights + cpu + zama, +memoria si aplican el opcional). El análisis completo con la verificación adversarial vive en el repo de karmakadabra.
