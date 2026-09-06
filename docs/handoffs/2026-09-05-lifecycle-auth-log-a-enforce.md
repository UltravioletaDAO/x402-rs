# 2026-09-05 — `ESCROW_LIFECYCLE_AUTH`: de `off` a `log` en producción, la medición, y el veredicto sobre `enforce`

**Encargo:** c0der master-4, 2026-09-05, segundo intento (el primero nunca llegó a un worker:
la sesión ya había cerrado y el texto se ejecutó como comando de PowerShell).
**Worker:** Orca `task_256125fcbe05`, rama `0xultravioleta/x4-enforce`, corrido desde WSL.
**Decisión del dueño que gobierna:** *"PR, desplegar y encender YA"* — leída como la decisión de
camino (firma obligatoria), no como la orden de cortar a Execution Market a ciegas.
**Reglas seguidas:** cero `settle` / `release` / `refundInEscrow` contra producción; cero valores de
secretos leídos ni impresos; push solo de ramas propias; `terraform apply` acotado con `-target` al
recurso autorizado; el apply de los access logs del ALB NO se hizo.

**Veredicto en tres líneas.** Producción corre en `log` desde `2026-09-06T00:41:14Z` (verificado en
vivo). **`enforce` NO se enciende todavía:** ningún llamador firma — Execution Market en HEAD
`aec2eb69` no manda `lifecycleAuth` y el SDK de Python tampoco lo implementa —, así que hoy
`enforce` rechazaría el 100 % del tráfico real (2 953 release/refund de 22 pagadores en 17 días de
logs) y EM trata ese 4xx como permanente. Primero firma el SDK, después EM; eso lo despacha c0der.

---

## Fase 0 — arranque en WSL

- `.git` del worktree apuntaba a `Z:/…/.git/worktrees/x4-enforce`; reescrito a
  `/mnt/z/ultravioleta/dao/x402-rs/.git/worktrees/x4-enforce`. El registro del dueño
  (`.git/worktrees/x4-enforce/gitdir` → `C:/Users/lxhxr/orca/...`) no se tocó.
- `git config core.autocrlf true`. Después de eso `git status` solo lista `contracts`, `target` y
  `tests/crossmint-smart-wallet/node_modules` sin trackear (artefactos del worktree).
- Base: `origin/main` = `7a63f29d`, que ya contiene PR #21 (`f33bc50b`, el módulo
  `lifecycle_auth`). Prod respondía `{"version":"2.14.0"}` y `GET /settle` →
  `"escrowLifecycleAuth":"off"` antes de tocar nada.

Nota operativa: `orca orchestration send` SÍ llega desde este shell (a diferencia del worker
anterior): heartbeats y `worker_done` enviados.

---

## Fase 1 — `off` → `log` en producción

### Qué se cambió (commit `chore(escrow)` en esta rama)

| Archivo | Cambio |
|---|---|
| `terraform/environments/production/variables.tf` | `variable "escrow_lifecycle_auth"` con `validation` a `off|log|enforce`, default `off` |
| `terraform/environments/production/production.auto.tfvars` | `escrow_lifecycle_auth = "log"` — el valor operativo, versionado, que CI y el operador leen igual |
| `terraform/environments/production/main.tf` | `ESCROW_LIFECYCLE_AUTH = var.escrow_lifecycle_auth` en el `environment` de la task def |
| `src/payment_operator/lifecycle_auth.rs` | las dos líneas de log del gate llevan ahora `operator = ?payment_info.operator` (2 líneas) |

Por qué `operator` en el log: el ALB no guarda access logs y la app no registra la IP del par,
así que la única forma de distinguir "quién llama" es el `PaymentOperator` que nombra el
`paymentInfo`: los de EM (`0x271f…F0Eb`, `0x0303…cBe5` en Base, ver
`docs/handoffs/2026-09-05-refund-sin-firma.md` en la rama `x4-refund-firma`) contra cualquier
otro. Esa línea nueva solo se ve cuando esta rama llegue a `main` (CI construye la imagen);
el modo `log` en sí NO necesita imagen nueva: la imagen desplegada ya trae el módulo.

Tests del módulo con el cambio: `cargo test --locked -p x402-rs --features
solana,near,stellar,algorand,sui,xrpl --lib lifecycle_auth -- --test-threads=1` →
`18 passed; 0 failed`.

### Dónde se lee el modo en vivo

Ya estaba expuesto (PR #21): `GET /settle` publica `escrowLifecycleAuth` con el valor efectivo
(`src/handlers.rs:92`). No hizo falta tocar `.well-known/x402`.

### El apply, acotado

```bash
cd terraform/environments/production
terraform plan  -out=phase1.tfplan -target=aws_ecs_task_definition.facilitator -target=aws_ecs_service.facilitator
terraform apply phase1.tfplan
```

Lo que decía el plan, leído antes de aplicar: `Plan: 1 to add, 1 to change, 1 to destroy`
(task def reemplazada por revisión nueva + servicio apuntando a ella). El único diff real en el
container: `+ {name = "ESCROW_LIFECYCLE_AUTH", value = "log"}`; el resto era la normalización
habitual de AWS (`mountPoints`, `hostPort`, `systemControls`, `volumesFrom`). **Sin línea `image`
en el diff**: `image-pin.tf` leyó la imagen corriendo (`facilitator:2.14.0-7a63f29`) y la
redesplegó igual.

### Verificación en vivo

| Qué | Comando | Resultado |
|---|---|---|
| Task def nueva con la variable | `aws ecs describe-task-definition --task-definition facilitator-production:395 … \| jq '… select(.name=="ESCROW_LIFECYCLE_AUTH")'` | `ESCROW_LIFECYCLE_AUTH=log`, imagen `2.14.0-7a63f29` |
| Rollout | `aws ecs describe-services --cluster facilitator-production --services facilitator-production --query 'services[0].deployments[0].rolloutState'` | `COMPLETED` a las `2026-09-06T00:41:14Z` (394 → 395) |
| Modo efectivo | `curl -s https://facilitator.ultravioletadao.xyz/settle \| jq .escrowLifecycleAuth` | `"log"` |
| Versión intacta | `curl -s https://facilitator.ultravioletadao.xyz/version` | `{"version":"2.14.0"}` |

### La trampa que queda abierta hasta el merge

CI despliega `main` con el mismo `-target` (task def + servicio; `.github/workflows/ci.yaml:372-378`)
y **el job `deploy` depende de `[test, preflight]`, no de `plan`** (`ci.yaml:286`; corrida
reciente: `7a63f29d` a las 19:50Z de hoy, `success`). Mientras esta rama no esté en `main`,
cualquier merge ajeno a `main` re-registra la task def SIN la variable y producción vuelve a
`off` en silencio. Verificar después de cada deploy con el `curl` de arriba hasta que el PR
esté mergeado.

---

## Fase 2 — Medir

### La ventana, y por qué

Tasa base, de los logs (retención 30 días; el primer release/refund con logs es del
2026-08-20 17:31Z):

```
# Logs Insights, /ecs/facilitator-production, 17 días hasta 2026-09-06T00:30Z
filter @message like /Processing escrow scheme settlement \((release|refundInEscrow)\)/
| parse @message /payer.{0,12}=.{0,6}(?<payer>0x[0-9a-fA-F]{40})/
| parse @message /receiver.{0,12}=.{0,6}(?<receiver>0x[0-9a-fA-F]{40})/
| stats count(*) as n, count_distinct(payer) as payers, count_distinct(receiver) as receivers,
        min(@timestamp) as first, max(@timestamp) as last
```

(Los `.{0,12}=.{0,6}` absorben los escapes ANSI que `tracing_subscriber::fmt` deja en
CloudWatch; un `parse` literal `payer=` no casa.)

| Métrica | Valor |
|---|---|
| release + refundInEscrow, 2026-08-20 17:31Z → 2026-09-04 22:49Z | **2 953** (2 801 release, 152 refundInEscrow) |
| Pagadores distintos | **22** |
| Receptores distintos | **38** |
| Redes | 9: arbitrum 2 013, celo 291, avalanche 140, base 120, optimism 123, ethereum 91, polygon 82, monad 62, skale-base 31 |
| Por día | de **0** (09-05 entero) a **833** (08-31); mediana 74. Es tráfico a ráfagas, no un flujo |
| Última llamada antes de encender `log` | `2026-09-04 22:49:48Z` |

La ventana que fija este worker: **desde `2026-09-06T00:41:14Z` (rollout `COMPLETED`) hasta que
haya al menos 100 órdenes y al menos un `refundInEscrow`, con un mínimo de 24 h.** Con la tasa
base (~174/día de promedio, pero 0 el 09-05) eso son entre 1 y 3 días. Una ventana de horas no
sirve: el día anterior al encendido tuvo cero llamadas.

### Lo medido dentro de la sesión

Ventana `2026-09-06T00:41:14Z` → `2026-09-06T00:45Z`: **0 órdenes** (cero `escrow lifecycle
order`, cero `Processing escrow scheme settlement (release|refundInEscrow)`). Se actualiza al
final de este archivo con la última lectura de la sesión.

### Lo que la ventana NO puede cambiar (medido en el código, no en los logs)

| Llamador | ¿Manda `lifecycleAuth`? | Evidencia |
|---|---|---|
| Execution Market, HEAD `aec2eb69` (2026-09-04) | **No** | `grep -rn "lifecycleAuth\|LifecycleOrder" execution-market --include=*.py --include=*.ts` → vacío; `_post_facilitator_json` sigue siendo `client.post(url, json=payload)` sin headers ni firma (`mcp_server/integrations/x402/payment_dispatcher.py:1284-1312`) |
| SDK Python `uvd-x402-sdk` | **No** | `grep -rn lifecycleAuth uvd-x402-sdk-python/src` → vacío |
| SDK TypeScript | no implementa release/refund | (handoff anterior) |

Y en la ventana con logs el campo ni existía (PR #21 se desplegó el 2026-09-05 13:31Z; la
última orden es del 09-04). Conclusión que no depende de la ventana: **hoy 100 % de las órdenes
llegan sin firma → verdict `missing`; con `enforce`, 100 % rechazadas.**

### La consulta para seguir midiendo (para c0der, cuando la ventana esté completa)

```
# Logs Insights, /ecs/facilitator-production, desde 2026-09-06T00:41:14Z
filter @message like /escrow lifecycle order/
| parse @message /action.{0,12}=.{0,6}(?<action>[a-zA-Z]+)/
| parse @message /verdict.{0,12}=.{0,6}(?<verdict>[a-z_]+)/
| parse @message /operator.{0,12}=.{0,6}(?<operator>0x[0-9a-fA-F]{40})/
| parse @message /payer.{0,12}=.{0,6}(?<payer>0x[0-9a-fA-F]{40})/
| stats count(*) as n, count_distinct(payer) as payers, count_distinct(operator) as operators by action, verdict
```

(`operator` aparece cuando esta rama esté en `main`; antes, agrupar por `payer`.) Lectura:
`missing` = llega sin firma (rompe con `enforce`); `ok` = firmó un rol válido;
`unauthorized_role` / `bad_signature` / `expired` / `replayed` / `owner_unverifiable` = firmó y
NO pasaría. `enforce` se puede encender cuando `missing + no-ok = 0` sobre una ventana completa
**y** los `ok` cubren a todos los operadores de EM.

---

## Fase 3 — Veredicto sobre `enforce`

**NO, todavía.** No es una cuestión de ventana: nadie firma. Encender `enforce` hoy convierte
cada approve, cancel, disputa, expiración, barredor y stream de EM en un 4xx que EM clasifica como
permanente (`payment_dispatcher.py:2606-2611`), un corte del rail de dinero, no una degradación.
El agujero sondeado el 2026-08-30 no produjo daño (2 tx minadas contra un escrow inexistente,
costo: gas); una semana más de eso es más barato que un corte de EM.

Orden para llegar a `enforce` (upstream-first, lo despacha c0der, no este worker):

1. **SDK Python** `uvd-x402-sdk`: `_settle_via_facilitator` firma `LifecycleOrder` EIP-712 con la
   llave/wallet que ya tiene (`advanced_escrow.py:592-650`). Dominio y struct en
   `src/payment_operator/lifecycle_auth.rs` (`LifecycleOrder(string action, uint256 amount,
   uint256 deadline, bytes32 nonce, PaymentInfo paymentInfo)`; deadline ≤ 900 s por
   `ESCROW_LIFECYCLE_MAX_DEADLINE_SECS`). Release a PyPI.
2. **EM** consume el SDK y firma con `WALLET_PRIVATE_KEY` como dueño del operador
   (`FEE_RECIPIENT()`); verificar antes que esa llave deriva a `0xaE07…A6ad` haciendo que EM
   loguee `_get_platform_address()` (dirección pública, no la llave). [HIPÓTESIS del handoff
   anterior, sigue sin confirmar.]
3. Ventana completa en `log` con la consulta de la fase 2: `missing = 0` y `ok` en todos los
   operadores de EM.
4. `enforce` = **una línea**: `escrow_lifecycle_auth = "enforce"` en
   `production.auto.tfvars` + merge (CI lo aplica en el deploy) o el mismo apply acotado de la
   fase 1. Rollback: la misma línea a `"log"`.

---

## Fase 4 — Access logs del ALB: ya estaba propuesto, falta el paso 2

Lo que encontró el disco, y que el encargo no sabía: el terraform de los access logs **ya
existe** (`terraform/environments/production/alb-access-logs.tf` + bloque `dynamic "access_logs"`
en `aws_lb.main`, `main.tf:432-439`, gateado por `var.alb_access_logs_enabled`, default `false`),
y el **paso 1 ya está aplicado**: `terraform state list` lista `aws_s3_bucket.alb_logs` y sus
cinco recursos asociados, `aws s3api head-bucket --bucket facilitator-production-alb-logs` responde,
y el ALB en vivo dice `access_logs.s3.enabled = false`.

Lo que queda es el paso 2 (que el ALB escriba en ese bucket). Está en su propia rama y su propio
PR, **sin aplicar**, porque `aws_lb.main` está dentro del grafo de `-target` de CI:

```bash
# medido: el set de -target de CI con el flag en true
terraform plan -lock=false -var 'alb_access_logs_enabled=true' \
  -target=aws_ecs_task_definition.facilitator -target=aws_ecs_service.facilitator \
  -target=aws_appautoscaling_target.ecs_target -target=aws_appautoscaling_policy.ecs_alb_request_count \
  -target=aws_appautoscaling_policy.ecs_memory
#   # aws_lb.main will be updated in-place
#   Plan: 0 to add, 1 to change, 0 to destroy.
```

O sea: **mergear la rama `0xultravioleta/x4-alb-access-logs` a `main` ES aplicarlo** en el
siguiente deploy. Por eso va separada de la de `log` y no se mergea sin OK del dueño.

Plan guardado (`terraform plan -var 'alb_access_logs_enabled=true' -target=aws_lb.main`):

```
  # aws_lb.main will be updated in-place
      ~ access_logs {
          + bucket  = "facilitator-production-alb-logs"
          ~ enabled = false -> true
          + prefix  = "alb"
        }
Plan: 0 to add, 1 to change, 0 to destroy.
```

Costo: S3 por objeto de log (un archivo cada 5 min por nodo del ALB), expiración a 90 días ya
configurada en el bucket. Sin esto, la próxima sonda tampoco tendrá origen.

---

## Para c0der

- **Estado del modo:** `log` en producción desde `2026-09-06T00:41:14Z`, verificado con
  `curl -s https://facilitator.ultravioletadao.xyz/settle | jq .escrowLifecycleAuth`. Imagen
  sin cambio (`2.14.0-7a63f29`). **Hasta que el PR de esta rama se mergee, un deploy ajeno de
  `main` lo devuelve a `off`**: re-verificar con el mismo `curl` después de cada deploy.
- **El número:** en 17 días de logs, 2 953 release/refund de 22 pagadores y 38 receptores en 9
  redes, **0 con firma** (el campo no existía y ningún llamador lo manda). En la ventana de `log`
  de esta sesión: ver la última lectura al pie. Con `enforce` hoy: **100 % rechazado**.
- **Veredicto:** `enforce` NO. Camino: SDK Python firma → EM firma → ventana completa en `log`
  (≥ 100 órdenes, ≥ 1 refund, ≥ 24 h, `missing = 0`) → `escrow_lifecycle_auth = "enforce"`.
- **Necesita decisión del dueño:** (1) mergear el PR de `log` (cierra la trampa del revert);
  (2) despachar SDK Python y EM; (3) mergear o no el PR de access logs del ALB sabiendo que
  merge = apply (costo S3, 90 días); (4) confirmar que `WALLET_PRIVATE_KEY` de EM deriva al
  `FEE_RECIPIENT()` del operador antes de que EM firme como dueño.

## Declaración

Ningún `settle`, `release` ni `refundInEscrow` se ejecutó contra producción ni contra ninguna
cadena. Ningún valor de secreto se leyó ni se imprimió (solo nombres). Un único `terraform
apply`, acotado a `aws_ecs_task_definition.facilitator` y `aws_ecs_service.facilitator`,
autorizado por `autorizaciones.toml`. El apply de los access logs del ALB no se hizo. Push
solo de ramas `0xultravioleta/x4-enforce` y `0xultravioleta/x4-alb-access-logs`; `main` intacto.
