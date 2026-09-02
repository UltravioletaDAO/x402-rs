---
date: 2026-08-22
tags:
  - type/handoff
  - domain/infrastructure
  - domain/observability
  - priority/p0
status: active
---

# Handoff para continuar desde WSL — alertas, salud por cadena y el backlog de performance

> **Para:** la sesión que continúe esto desde WSL (donde sí se puede correr
> Terraform con la versión correcta y actuar sobre el facilitador).
> **De:** la sesión de Windows del 2026-08-20/21.
> **Estado:** hay **un commit local sin pushear** y **una decisión de IAM
> pendiente** que bloquea su despliegue. Todo lo demás ya está en producción.

---

## 0. Lee esto primero: la trampa que bloquea todo

**NO corras `terraform apply` desde Windows.** La versión local es **1.14.3**; el
state remoto lo escribió **1.9.8**, que es la que fija el workflow
(`.github/workflows/ci.yaml`, `terraform_version: "1.9.8"`).

Aplicar con 1.14.3 **actualiza el formato del state file y CI deja de poder
leerlo** — rompe el deploy de todo el proyecto para arreglar una cosa chica.

Desde WSL, antes de tocar nada:

```bash
terraform version    # tiene que decir 1.9.8
```

Si no es 1.9.8, instalala (tfenv o el binario directo). Esta es la razón por la
que este trabajo se pasó a WSL.

Verificar la versión del state sin tocarlo:

```bash
aws s3 cp s3://facilitator-terraform-state/production/terraform.tfstate - \
  --region us-east-2 | head -c 120
# {"version": 4, "terraform_version": "1.9.8", "serial": ..., ...}
```

---

## 1. Estado del repo

| | |
|---|---|
| Rama | `main` |
| Último pusheado | `5ac06380` (2026-08-21, de **otra sesión**: ERC-8004 Scroll) |
| **Sin pushear** | **`447b9447`** `feat(alertas): darle voz propia al facilitador y vigilancia por cadena` |
| Versión en producción | `1.92.0` |

> El hash `447b9447` puede volver a cambiar si otra sesión pushea y hay rebase.
> Identificá el commit por su asunto, no por el hash.

**Por qué no se pusheó:** el push dispara CI, y CI fallaría — le faltan permisos
IAM (ver §3). Empujarlo antes de resolver eso deja un run rojo en `main`.

---

## 2. Lo que YA está en producción y verificado

No hay que rehacer nada de esto.

### 2.1 El RPC de Sui estaba muerto — arreglado

`fullnode.mainnet.sui.io` y `fullnode.testnet.sui.io` dejaron de servir JSON-RPC
(`-32601` a los seis métodos probados, incluido el handshake del SDK). **Sui
mainnet no podía liquidar nada**, y no se sabe desde cuándo.

Reemplazos, verificados contra las wallets reales:

```
mainnet  https://sui-rpc.publicnode.com          -> 20,81 SUI  chain 35834a8a
testnet  https://sui-testnet-rpc.publicnode.com  ->  1,00 SUI  chain 4c78adac
```

Estaba en **seis** lugares (`main.tf`, `lambda-balances.tf`, `handler.py`,
`sui.rs`, `.env.example` ×2). El peor era el fallback de `sui.rs`: sin la
variable de entorno, el código caía **por defecto** al endpoint muerto.

Commits: `f5bc8500`, `5ffdc586`, `5f3cee24`.

### 2.2 Celo se había quedado sin gas — recargada

`0,0284 CELO` contra `0,1134` por settle de escrow → **cero settles**. Causó
**440 errores `insufficient funds`** el 20-ago entre 18:00 y 21:45 y **409 de 973
settlements de escrow fallidos en 24h**. Hoy: **342,8 CELO**.

### 2.3 El CI no aplicaba nunca la Lambda de balances — arreglado

Tres capas encadenadas, cada una tapando a la siguiente:

1. El apply targeted del deploy excluía la Lambda (por el hash inestable del zip).
2. **El usuario de CI no tenía NINGÚN permiso de escritura sobre Lambda** — un
   apply completo tampoco habría funcionado.
3. **El state reportaba desplegado un código que no lo estaba**: Terraform compara
   el zip contra su propio state, nunca contra AWS, y ambos coincidían mientras
   AWS corría otra cosa.

Arreglado con un step condicional en CI, el statement IAM `BalancesLambdaDeploy`
(`facilitator-cicd-infra` v2) y un cambio que movió el hash. Documentado en
`docs/CICD_SETUP.md`.

**Verificación actual:**
```bash
curl -s https://facilitator.ultravioletadao.xyz/api/balances | jq '.balances | with_entries(select(.key|test("sui")))'
# {"sui-mainnet": "20.8149", "sui-testnet": "1.0000"}
```

---

## 3. BLOQUEO ACTIVO — la decisión de IAM

El commit `447b9447` crea alarmas, un topic SNS y un schedule. **CI no tiene
permisos para nada de eso.** Hay que ampliar la policy `facilitator-cicd-infra`.

### 3.1 Los cuatro statements de rutina (sin controversia)

Acotados por prefijo `facilitator-*`:

| Sid | Recurso |
|---|---|
| `ObservabilitySns` | `arn:aws:sns:us-east-2:<AWS_ACCOUNT_ID>:facilitator-*` |
| `ObservabilityAlarms` | `arn:aws:cloudwatch:us-east-2:<AWS_ACCOUNT_ID>:alarm:facilitator-*` |
| `ObservabilitySchedule` | `arn:aws:events:us-east-2:<AWS_ACCOUNT_ID>:rule/facilitator-*` |
| `BalancesLambdaInvokePermission` | `arn:aws:lambda:us-east-2:<AWS_ACCOUNT_ID>:function:facilitator-production-balances` |

### 3.2 La parte que necesita decisión humana

La policy tiene una barrera anti-escalada deliberada:

```json
{
  "Sid": "DenyPrivilegeEscalation",
  "Effect": "Deny",
  "Action": ["iam:PutRolePolicy", "iam:AttachRolePolicy", "iam:PutUserPolicy", "..."],
  "NotResource": ["arn:aws:iam::<AWS_ACCOUNT_ID>:role/facilitator-production-ecs-task"]
}
```

CI solo puede escribir policies en **un** rol. Pero la Lambda de balances necesita
`cloudwatch:PutMetricData` para emitir las métricas que alimentan las 28 alarmas
nuevas. **Sin ese permiso las alarmas se crean y nunca reciben un dato.**

**Opción A — ampliar el allowlist (recomendada).** Agregar
`arn:aws:iam::<AWS_ACCOUNT_ID>:role/facilitator-production-balances-lambda` al
`Resource` del Allow y al `NotResource` del Deny. Todo queda en Terraform.
Riesgo acotado: ese rol solo lee secretos de RPC, no puede asumir otros roles, y
el `PutMetricData` va limitado por condición de namespace.

**Opción B — aplicar ese permiso a mano** por CLI, sin tocar la barrera. Funciona,
pero crea drift entre AWS y Terraform — exactamente lo que ya mordió con el state
de la Lambda (§2.3, punto 3).

> **Saul no se pronunció sobre esto.** Se le presentaron ambas opciones y la
> sesión terminó antes de la respuesta. **Preguntar antes de aplicar.**

### 3.3 El script que aplica la Opción A

Genera la versión nueva de la policy y la deja lista para revisar. **No la aplica.**

```bash
cd /tmp
ACC=$(aws sts get-caller-identity --query Account --output text); REG=us-east-2
LAMBDA_ROLE="arn:aws:iam::$ACC:role/facilitator-production-balances-lambda"

V=$(aws iam get-policy --policy-arn arn:aws:iam::$ACC:policy/facilitator-cicd-infra \
      --query 'Policy.DefaultVersionId' --output text)
aws iam get-policy-version --policy-arn arn:aws:iam::$ACC:policy/facilitator-cicd-infra \
  --version-id "$V" --query 'PolicyVersion.Document' --output json > infra-cur.json

python3 - <<'PY'
import json
ACC=$(aws sts get-caller-identity --query Account --output text); REG="us-east-2"
LAMBDA_ROLE=f"arn:aws:iam::{ACC}:role/facilitator-production-balances-lambda"
doc=json.load(open('infra-cur.json'))
nuevos=[
 {"Sid":"ObservabilitySns","Effect":"Allow",
  "Action":["sns:CreateTopic","sns:GetTopicAttributes","sns:SetTopicAttributes",
            "sns:Subscribe","sns:ListSubscriptionsByTopic","sns:TagResource",
            "sns:UntagResource","sns:ListTagsForResource"],
  "Resource":f"arn:aws:sns:{REG}:{ACC}:facilitator-*"},
 {"Sid":"ObservabilityAlarms","Effect":"Allow",
  "Action":["cloudwatch:PutMetricAlarm","cloudwatch:DeleteAlarms",
            "cloudwatch:TagResource","cloudwatch:UntagResource"],
  "Resource":f"arn:aws:cloudwatch:{REG}:{ACC}:alarm:facilitator-*"},
 {"Sid":"ObservabilitySchedule","Effect":"Allow",
  "Action":["events:PutRule","events:DeleteRule","events:PutTargets",
            "events:RemoveTargets","events:TagResource","events:UntagResource"],
  "Resource":f"arn:aws:events:{REG}:{ACC}:rule/facilitator-*"},
 {"Sid":"BalancesLambdaInvokePermission","Effect":"Allow",
  "Action":["lambda:AddPermission","lambda:RemovePermission","lambda:GetPolicy"],
  "Resource":f"arn:aws:lambda:{REG}:{ACC}:function:facilitator-production-balances"},
]
have={s.get('Sid') for s in doc['Statement']}
for n in nuevos:
    if n['Sid'] not in have: doc['Statement'].append(n)
# OPCION A unicamente: ampliar el allowlist del rol de la Lambda
for s in doc['Statement']:
    if s.get('Sid')=='IamRolePolicyForTaskRoleOnly':
        r=s['Resource']; r=[r] if isinstance(r,str) else r
        if LAMBDA_ROLE not in r: r.append(LAMBDA_ROLE)
        s['Resource']=r
    if s.get('Sid')=='DenyPrivilegeEscalation':
        nr=s['NotResource']; nr=[nr] if isinstance(nr,str) else nr
        if LAMBDA_ROLE not in nr: nr.append(LAMBDA_ROLE)
        s['NotResource']=nr
json.dump(doc, open('infra-new.json','w'), indent=2)
print("statements:", len(doc['Statement']),
      "| bytes:", len(json.dumps(doc,separators=(',',':'))), "de 6144")
PY

# REVISAR infra-new.json ANTES de esta linea:
aws iam create-policy-version \
  --policy-arn arn:aws:iam::$ACC:policy/facilitator-cicd-infra \
  --policy-document file://infra-new.json --set-as-default
```

**Límite de versiones:** una policy admite 5. Si ya hay 5, borrar la más vieja
antes (`aws iam delete-policy-version`). Al 2026-08-21 estaba en **v2**.

---

## 4. Qué hace el commit sin pushear

`447b9447` — archivos: `alerts.tf` (nuevo), `variables.tf`, `lambda-balances.tf`,
`cloudwatch-near-metrics.tf`, `cloudwatch-v2-metrics.tf`,
`lambda/balances/handler.py`, `.github/workflows/ci.yaml`.

- **Topic SNS propio** `facilitator-production-alerts`, con `0xultravioleta@gmail.com`
  suscrito (variable `alerts_email`, default en `variables.tf`).
  **La confirmación del correo es manual**: AWS manda un link que hay que clickear
  una vez. Hasta entonces la suscripción queda en `PendingConfirmation`.
- **4 de las 5 alarmas mudas** enchufadas al topic nuevo.
- **La Lambda emite métricas** `ChainRpcHealthy` (1/0) y `ChainNativeBalance` por
  cadena, namespace `Facilitator/Chains`. El `null` que ya devolvía **es** la señal
  de cadena ilegible; solo faltaba emitirla. `publish_chain_metrics` nunca lanza.
- **28 alarmas nuevas**: inalcanzable + balance bajo × 14 mainnet.
- **Schedule de 15 min** (EventBridge). Antes la Lambda solo corría cuando alguien
  abría la landing, así que sin visitas no había datapoints.
- **Step de CI** que aplica todo esto cuando cambian sus archivos.

**Decisión deliberada que NO hay que "arreglar":** la quinta alarma
(`facilitator-x402-v1-traffic-sudden-drop`) **queda apagada**. Su umbral de 5 req/h
nunca se calibró contra el tráfico real (900-1200 req/h en baseline). Una alarma que
grita en falso enseña a ignorar el canal. La razón está escrita en el `.tf`.

**Umbrales:** verificados contra los nombres que emite la Lambda (los 14 coinciden
exacto) y contra los balances del 21-ago (los 14 por encima). **Ninguna nace
gritando.** Si pasó mucho tiempo, re-verificar antes de aplicar:

```bash
curl -s https://facilitator.ultravioletadao.xyz/api/balances | jq -r '.balances | to_entries[] | select(.key|endswith("-mainnet")) | "\(.key)\t\(.value)"'
```

---

## 5. Secuencia para desplegar desde WSL

```bash
# 1. Confirmar la versión (§0)
terraform version                      # 1.9.8

# 2. Resolver el bloqueo de IAM (§3) — PREGUNTAR A SAUL primero

# 3. Empujar el commit. Push a main = deploy a producción.
git log origin/main..HEAD --oneline    # ver que sea el de alertas
git push origin main

# 4. Seguir el run
gh run watch $(gh run list --limit 1 --json databaseId --jq '.[0].databaseId') --exit-status
```

Si CI falla en el step de observabilidad, el error dirá qué permiso falta.

---

## 6. Criterio de éxito

Está listo cuando las cuatro cosas den verde:

```bash
# a) Las métricas por cadena existen (esperar >=15 min tras el deploy)
aws cloudwatch list-metrics --namespace Facilitator/Chains --region us-east-2 \
  --query 'length(Metrics)'          # > 0

aws cloudwatch get-metric-statistics --namespace Facilitator/Chains \
  --metric-name ChainRpcHealthy --dimensions Name=Chain,Value=celo-mainnet \
  --start-time "$(date -u -d '1 hour ago' +%Y-%m-%dT%H:%M:%S)" \
  --end-time "$(date -u +%Y-%m-%dT%H:%M:%S)" --period 900 --statistics Maximum \
  --region us-east-2 --query 'Datapoints'        # no vacio

# b) Las 32 alarmas existen y NINGUNA quedo en INSUFFICIENT_DATA
aws cloudwatch describe-alarms --alarm-name-prefix facilitator --region us-east-2 \
  --query 'MetricAlarms[].[AlarmName,StateValue]' --output text | sort -k2

# c) Ninguna alarma muda (salvo la de v1-traffic, que es a proposito)
aws cloudwatch describe-alarms --alarm-name-prefix facilitator --region us-east-2 \
  --query 'MetricAlarms[?length(AlarmActions)==`0`].AlarmName' --output text
# esperado: solo facilitator-x402-v1-traffic-sudden-drop

# d) La suscripcion de correo esta CONFIRMADA (no PendingConfirmation)
aws sns list-subscriptions-by-topic --region us-east-2 \
  --topic-arn arn:aws:sns:us-east-2:<AWS_ACCOUNT_ID>:facilitator-production-alerts \
  --query 'Subscriptions[].[Endpoint,SubscriptionArn]' --output text
```

**Prueba de fuego real** (opcional pero recomendada): bajar temporalmente el
umbral de una alarma de balance para que dispare, comprobar que llega el correo, y
devolverlo. Una alarma que nunca se probó es una hipótesis, no una alarma — esa es
la lección #1 del handoff de Execution Market.

---

## 7. Backlog: lo que el diagnóstico encontró y NADIE arregló todavía

Contexto completo en
[`2026-08-20-diagnostico-performance-facilitador.md`](2026-08-20-diagnostico-performance-facilitador.md),
que incluye las 12 hipótesis descartadas y por qué (para no reperseguirlas).

**El resumen en una línea:** el facilitador no se cae por exceso de carga; se traba
con casi nada, y el reintento del cliente le impide curarse. 216 settles en 4 horas
(0,015 req/s) bastaron para 4 horas de degradación.

### 7.1 Los cinco fixes de código, en el ORDEN SEGURO

**`#1 → #4 → #3 → #2 → #5`. El #3 NUNCA antes del #1.**

| # | Fix | Archivo | Riesgo |
|---|---|---|---|
| 1 | Devolver el nonce ante `txpool is full` (`is_mempool_full`, match por mensaje **no** por `-32003`: ese código está sobrecargado) | `src/chain/evm.rs:811-825` | Bajo |
| 4 | Timeout explícito en reqwest (10s request / 3s connect) + los 4 sitios de `algorand.rs:366`, `stellar.rs:488`, `stellar.rs:2110`, `xrpl.rs:330` | `src/chain/evm.rs:263-272` | Bajo |
| 3 | `-32003` retryable → 502 + `Retry-After` en vez de 400 | `src/handlers.rs:302-313` | Medio — **solo después del #1** |
| 2 | Que el hueco de nonce sane bajo tráfico (contador de `pending` congelado, **no** "última confirmada") | `src/chain/evm.rs:2378-2390` | **Alto — considerar follow-up** |
| 5 | Failover con `FallbackLayer`, **`active_transport_count=1`** | `src/chain/evm.rs:262-270` | Medio |

**Por qué el #3 nunca va solo:** hoy `txpool is full` devuelve 400 y el cliente no
reintenta. Si se hace retryable primero, Execution Market reintenta más y **cada
reintento quema otro nonce** — convierte un atasco ocasional en uno veloz.

**Trampas al aplicar:**
- `released unbroadcast nonce` (`evm.rs:2429`) es `debug!` y prod corre en `info`
  → **subirlo o el #1 es inverificable**.
- El helper de test `resync_nonce` (`evm.rs:2606`) es una **copia literal** del
  match de producción. Si se toca uno y no el otro, **los tests quedan verdes
  probando el código viejo**.
- `FallbackLayer::default()` consulta **3 transportes en paralelo** y **no** incluye
  `eth_getTransactionCount` en `sequential_methods` → el resync de nonce se vuelve
  no determinista. `count=1` es **requisito de corrección**, no preferencia.

### 7.2 Cosas de infra que siguen abiertas

1. **Las 3 alarmas huérfanas** (`-5xx-errors`, `-latency-p99`, `-no-running-tasks`)
   **no están en Terraform** — creadas a mano, publican al topic de Execution
   Market. Saul pidió **mantener EM y agregar el nuestro**. Requiere declararlas +
   `terraform import`. No se hizo por la versión de Terraform (§0).
2. **`min_capacity = 2`** — hoy corre **una sola task** y nunca autoescaló.
3. **Autoescalado por `ALBRequestCountPerTarget`**, no por CPU: el servicio es
   I/O-bound y la CPU nunca pasó del 25% en tres episodios de degradación.
4. **Alarma temprana de p99 > 2s** — la actual es `>10s` y no vio dos horas a 5-8s.
5. **`access_logs.s3` del ALB** — sin eso el HTTP 460 es invisible, y fueron 201 de
   417 fallos reales.
6. **Subir la retención de logs** (7 días perdió el episodio del 10-ago).
7. **`RUST_LOG=info,x402_rs::chain::evm=debug`** — sin eso el bug del nonce es
   inobservable en producción.

### 7.3 Preguntas abiertas de verdad

- ~~**Qué abre el primer hueco de nonce en monad.**~~ **RESUELTO 2026-08-28.**
  Lo abre `/feedback`, que manda con `call.send().await` crudo
  (`src/handlers.rs:4189`, y 13 sitios más) sobre el **mismo `EvmProvider` del
  cache** — o sea el mismo `PendingNonceManager` — que usa `/settle`. Sin
  `estimate_gas` previo, el `JoinFill` de alloy llena gas y nonce con un
  `try_join!`: si la estimación revierte, `NonceFiller` **ya comprometió el
  nonce** y nadie lo devuelve. `/settle` sí se blinda estimando primero
  (`evm.rs:611-636`); el blindaje nunca se extendió a los otros caminos.
  Monad duele más que polygon porque **no tiene mempool global**: el RPC reenvía
  a los siguientes líderes, hasta 3 veces, y abandona — en polygon un hueco
  demora, en monad mata. Prueba dura: la distribución es **bimodal** (33 de 41
  minadas en ≤1s, 4 a 151-283s, 4 destruidas; nada en el medio) y el hueco quedó
  reconstruido con timestamps de bloque (nonces 378-381, 24-ago).
- **El 88% era un número inflado y NO es re-verificable.** Los logs de esa
  ventana expiraron (retención de 7 días). El método marcaba ausencia si "ambos
  RPC" devolvían `null`, pero el segundo (`monad.drpc.org`) está **podado** y
  devuelve `null` para transacciones sí minadas, así que la regla corría con un
  solo voto. Medido de nuevo el 2026-08-28: **19,5% (8/41)** en monad.
  Se mantienen refutadas la falta de fondos (79,4 MON) y el piso de gas (base fee
  constante en 100 gwei, alloy firma a 202). Lo de `txpool is full` hay que
  leerlo distinto: **monad no implementa `txpool_*` en absoluto** (`-32601`), así
  que "cero eventos ahí" estaba garantizado y nunca fue señal de nada.
- **SIN VERIFICAR:** no se mandó ninguna transacción de prueba al RPC de monad,
  así que "¿acepta y descarta?" no tiene test directo.
- **Worker threads de tokio.** `available_parallelism()` usa `sched_getaffinity()`,
  no la cuota cgroup. En Fargate puede dar 1 worker (head-of-line blocking) o N con
  presupuesto de 1 (thrashing). **Patologías distintas con fixes opuestos.** Se
  resuelve con una línea de log al boot o habilitando ECS Exec.
- ~~**Dos transacciones se minaron mientras el cliente recibió 400.**~~
  **MEDIDO 2026-08-28: son 68.** Sobre las 957 emisiones de 7 días cruzadas
  hash por hash contra la cadena: **68 minadas a las que se les devolvió error**
  (celo 36, polygon 23, skale-base 4, monad 4, base 1) y **72 ausentes
  confirmadas contra 2 RPC**. Fallo visible al cliente: **140/957 = 14,6%**.
  **Celo es hoy la peor cadena: 95 de 160 = 59%.**
  Para calibrar `TX_RECEIPT_TIMEOUT_SECS`: monad espera 151-283s y skale-base
  210-1195s (un timeout de 300s los cubre casi todos), pero **celo y polygon
  esperan de 2 minutos a 3,4 horas** — ahí ningún timeout HTTP alcanza y la
  única salida es settlement asíncrono.
- **`polygon-testnet` devuelve `None`** en el readout de balances — otra cadena
  ilegible, sin investigar (es testnet, no se alarma).

---

## 8. Otras trampas conocidas de este repo

- **`terraform.tfvars` está gitignored** y CI corre sin él, o sea con los
  **defaults de `variables.tf`**. El 14-ago eso revirtió el `alb_idle_timeout` de
  600 a 180, deploy tras deploy. Hoy los defaults coinciden; **si cambiás uno,
  cambiá el otro**. (`desired_count` está a salvo por `ignore_changes`.)
- **Ninguna policy IAM vive en Terraform.** Se aplican a mano y `docs/CICD_SETUP.md`
  es su único registro. Eso es lo que hizo invisible el permiso faltante de Lambda.
- **Push a `main` = deploy a producción.** No es un "guardar".
- **La versión de release vive en `VERSION`**, no en `Cargo.toml` (que tiene un
  `0.0.0` congelado a propósito, para que la capa de dependencias de Docker no se
  invalide en cada release).
- El repo tiene **otras sesiones trabajando en paralelo** (el 21-ago entró
  `5ac06380` de ERC-8004/Scroll). Hacé `git pull --rebase` antes de pushear.

---

## 9. Handoff pendiente de enviar

`docs/handoffs/2026-08-20-respuesta-a-execution-market.md` está escrito y
commiteado pero **no se le pasó a Execution Market**. Contiene tres cosas que les
sirven directo:

1. **Su `FACILITATOR_TIMEOUT_SECONDS` y nuestro timeout de recibo empatan en 30s
   exactos** — de ahí sus 201 HTTP 460. Subirlo por encima de 35s les cambia el
   resultado hoy, sin esperar ningún fix nuestro.
2. **Celo estaba sin gas**: buena parte de sus fallos de Celo no era degradación.
3. **Dos transacciones minadas con 400 devuelto** — si su reconciliación se fía del
   código HTTP, quedaron mal contabilizadas.

Y les pide: los request ids de sus 502 de la ventana 21:49→01:53 (para cerrar el
88% de monad), y qué vieron en Sui.
