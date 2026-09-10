# Versionado del bucket del catalogo de discovery

Fecha: 2026-09-10
Rama: `0xultravioleta/x4-bucket-versioning`
Origen: auditoria Astra 6 del 2026-09-09, seccion 6 (A3), "puerto de escape"

## Para c0der

### Que se creo

Dos recursos de Terraform nuevos en `terraform/environments/production/discovery-bucket.tf`,
sobre el bucket `facilitator-discovery-prod` (us-east-2), que sigue **sin estar
declarado** en Terraform:

| Recurso | Que hace |
|---|---|
| `aws_s3_bucket_versioning.discovery` | versionado `Enabled` |
| `aws_s3_bucket_lifecycle_configuration.discovery` | expira versiones no actuales y aborta multipart colgados |

Reglas del lifecycle:

| Regla | Alcance | Efecto |
|---|---|---|
| `noncurrent-versions-30d` | todo el bucket | versiones no actuales expiran a los 30 dias |
| `health-overlay-noncurrent-1d` | prefijo `bazaar/health.json` | expiran al dia |
| `abort-incomplete-uploads` | todo el bucket | aborta multipart incompletos a los 7 dias |

Los dos recursos toman el bucket por **nombre literal**, no por referencia. No hace falta
importar nada, ningun plan puede proponer destruir el bucket, y un apply dirigido a estos
dos targets no arrastra nada mas. Si mas adelante queres que Terraform sea dueño del bucket,
el comando es este y **no lo corri**:

```
terraform import aws_s3_bucket.discovery facilitator-discovery-prod
```

En el pipeline: un paso nuevo al final del job `deploy` de `.github/workflows/ci.yaml`, con
la misma forma que los pasos del Lambda de balances y de observabilidad. Corre solo cuando
cambia `discovery-bucket.tf` -- lo cual incluye este push -- o cuando el compare de GitHub
falla, que es el caso de un "Run workflow" manual.

Va **ultimo a proposito**. Configura un bucket que no tiene nada que ver con la release: la
imagen ya se aplico, el rollout ya espero y `/health` ya contesto. Si este paso falla, no
puede saltearse el veredicto del rollout.

### Bloqueante antes de mergear: falta un permiso IAM

Simulado contra la identidad viva el 2026-09-10, no supuesto:

```
s3:PutBucketVersioning        implicitDeny
s3:PutLifecycleConfiguration  implicitDeny
s3:GetBucketVersioning        allowed     (ReadOnlyAccess)
s3:GetLifecycleConfiguration  allowed     (ReadOnlyAccess)
```

Las lecturas alcanzan para que el drift gate planifique, asi que el gate funciona hoy. Las
escrituras son las que necesita el apply. **Sin ese permiso el paso nuevo falla con
AccessDenied** (el paso lo detecta y lo dice con ese nombre, en vez de dejar un error
cripto). No rompe la release: la imagen y el rollout ya pasaron.

Es un cambio de IAM, va a mano y con credenciales humanas, **antes** del merge. La politica
que corresponde es la gestionada `facilitator-cicd-infra`, que hoy esta en v6 y coincide
byte a byte con `terraform/environments/production/cicd-iam-policy.tf`.

**No agregue el statement a ese archivo en este PR, y es a proposito.** Ese recurso no esta
en ninguna lista de targets (nunca debe estarlo: si CI pudiera aplicarlo, CI podria darse
permisos). Declararlo antes de aplicarlo deja el drift gate en rojo hasta que alguien corra
el apply a mano, y el criterio de cierre pide el gate verde. El orden seguro en este repo es
**aplicar primero, declarar despues**: es exactamente lo que dice la cabecera de ese archivo
("byte-for-byte the live document, so the plan right after the import is empty").

El statement a agregar:

```json
{
  "Sid": "DiscoveryBucketVersioning",
  "Effect": "Allow",
  "Action": [
    "s3:PutBucketVersioning",
    "s3:GetBucketVersioning",
    "s3:PutLifecycleConfiguration",
    "s3:GetLifecycleConfiguration"
  ],
  "Resource": "arn:aws:s3:::facilitator-discovery-prod"
}
```

Ojo con el limite de versiones: AWS corta una politica gestionada en 5 versiones y esta ya
va por la v6, asi que la rotacion ya viene pasando. Si el apply falla con `LimitExceeded`,
borra la version no-default mas vieja y repeti.

Despues de aplicarlo, agregalo tambien a `cicd-iam-policy.tf` en un cambio aparte: el plan
sale vacio porque el archivo ya coincide con lo vivo, y el gate se queda verde. Ese archivo
existe justamente porque "there is no drift detection for a resource that is not declared
anywhere", y tres permisos faltantes fueron invisibles hasta que un deploy se estrello con
ellos.

### El chequeo que grita si se apaga

Paso nuevo en el job `plan` (el drift gate), `Discovery catalog bucket is still versioned`.
Es de solo lectura, corre en cada PR y en cada push, y no bloquea la release porque `deploy`
no lo tiene en `needs`.

El drift gate solo no alcanzaba. Una vez que `aws_s3_bucket_versioning.discovery` esta en
una lista de targets, un bucket suspendido aparece en el plan como un cambio que el deploy
**si** cubre, y el reporte lo imprime bajo "Pending, but a deploy applies these - not a
failure". Cierto en principio e inutil en la practica: ese paso solo corre cuando cambia el
`.tf`, asi que la fila se quedaria ahi para siempre sin hacer fallar nada.

Por eso le pregunta a S3 directo. Verdades que usa:

- **Suspended es el caso ruidoso, y el unico que puede significar que alguien lo apago.** El
  versionado de S3 es de una sola via: un bucket que estuvo `Enabled` puede pasar a
  `Suspended` pero nunca vuelve a la respuesta vacia de "nunca configurado".
- Por eso, una respuesta vacia despues de que esto este mergeado significa que el recurso
  **nunca se creo**, no que lo desactivaron. Tambien falla.
- Lo que pide la configuracion lo lee del plan JSON, no con un grep del `.tf`: un grep no
  distingue un recurso vivo de uno comentado, y esto tiene que notar tambien que borren el
  recurso.
- Si la lectura a AWS no se completa, avisa y no falla. Una caida nuestra no es evidencia de
  que el bucket este mal.

Probado contra los seis estados (hoy / post-merge / suspendido / declarado-pero-nunca-
aplicado / configuracion que deja de pedir Enabled / lectura fallida).

Si algun dia sale rojo por `Suspended`, el arreglo no es un commit: es un **Run workflow**
manual sobre `main`. Un dispatch no tiene `github.event.before`, el compare falla, el paso
entra por la rama "applying to be safe" y vuelve a aplicar los dos targets.

### Costo estimado

Medido el 2026-09-10 sobre el bucket real. Precio S3 Standard us-east-2, $0.023/GB-mes. La
expiracion por lifecycle no se cobra; el versionado no agrega PUTs, solo guarda los que ya
habia.

Los dos objetos que se reescriben:

| Objeto | Tamaño | Reescrituras | Fuente |
|---|---|---|---|
| `bazaar/resources.json` | 15.206.413 B | pocas por dia | agregacion horaria + un read-modify-write por alta. La ultima escritura tenia ~3h al medir |
| `bazaar/health.json` | 6.734.205 B | hasta 1440/dia | el prober persiste una vez por tick de 60s si el overlay quedo sucio. Dos lecturas independientes con 4 min de diferencia lo encontraron escrito segundos antes |

Los otros dos objetos (`bazaar/backups/resources-pre-ws-a-20260724.json`,
`bazaar/resources.backup-pre-402milly-fix.json`) son estaticos: no generan versiones.

| Escenario | Con el carve-out del overlay | Con 30 dias planos |
|---|---|---|
| 10 escrituras de catalogo/dia | $0.31/mes | $6.33/mes |
| 30 escrituras/dia | $0.50/mes | $6.52/mes |
| 100 escrituras/dia | $1.18/mes | $7.21/mes |

De donde sale la diferencia: a 30 dias planos, `health.json` solo acumularia ~43.200
versiones no actuales, ~271 GiB, ~$6.23/mes. Veinte veces lo que cuesta proteger el catalogo
que es el motivo de todo esto, y **no compra nada**: el overlay de salud es estado
**derivado**. Cada registro se reconstruye probando, asi que una version vieja no es un
punto de recuperacion, es una copia rancia de algo que el prober regenera en menos de una
hora.

Ambas reglas matchean `bazaar/health.json`. Donde dos reglas de expiracion cubren el mismo
objeto, S3 aplica la mas corta. Si eso resultara al reves, el sintoma es la factura de
arriba y el arreglo es una linea.

### Como verificar despues del merge

```bash
# 1. Versionado activo. Antes de esto contestaba VACIO.
aws s3api get-bucket-versioning --bucket facilitator-discovery-prod --region us-east-2
# esperado: {"Status": "Enabled"}

# 2. Las tres reglas de lifecycle. Antes: NoSuchLifecycleConfiguration.
aws s3api get-bucket-lifecycle-configuration --bucket facilitator-discovery-prod \
  --region us-east-2 --query 'Rules[].[ID,Status,NoncurrentVersionExpiration.NoncurrentDays]' --output table

# 3. Las versiones empiezan a crecer. La primera reescritura del catalogo deja
#    una version no actual; antes de eso solo se ve la actual.
aws s3api list-object-versions --bucket facilitator-discovery-prod --region us-east-2 \
  --prefix bazaar/resources.json \
  --query 'Versions[].[VersionId,LastModified,Size,IsLatest]' --output table

# 4. El chequeo del pipeline en verde: job "Terraform plan (drift gate)",
#    paso "Discovery catalog bucket is still versioned" -> "Catalog bucket is versioned."
```

Para recuperar un catalogo pisado, que es el punto de todo esto:

```bash
aws s3api list-object-versions --bucket facilitator-discovery-prod --region us-east-2 \
  --prefix bazaar/resources.json --query 'Versions[].[LastModified,VersionId]' --output text
aws s3api copy-object --bucket facilitator-discovery-prod --region us-east-2 \
  --key bazaar/resources.json \
  --copy-source 'facilitator-discovery-prod/bazaar/resources.json?versionId=<VERSION_ID>'
```

## Contexto: por que versionado si las escrituras ya son condicionales

Cada alta es un read-modify-write de **un** objeto: GET del catalogo entero, edicion en
memoria, PUT del catalogo entero. Son 15.206.413 bytes por escritura, y el catalogo anterior
deja de existir en ese instante.

El PR #31 (mergeado hoy) hizo esas escrituras condicionales por ETag
(`src/discovery_store.rs`, `If-Match` / `If-None-Match`). Ese es el mecanismo real y sigue
siendolo: dos escritores concurrentes ya no se pisan en silencio, uno de los dos es
rechazado.

Lo que no da es una vuelta atras. Una escritura correcta segun todos los chequeos y
igualmente equivocada -- un merge malo, una migracion mala, una pasada de curacion que tira
filas que no debia -- queda como la unica copia. La auditoria pidio el versionado como
**puerto de escape** detras de ese mecanismo, no como reemplazo.

## Lo que no toque

- El bucket en si: sin declarar y sin importar.
- `cicd-iam-policy.tf`: ver arriba, aplicar primero y declarar despues.
- `scripts/`, `src/chain/`, `terraform/environments/production/alerts*.tf` (x4-polygon-cola)
  y `src/discovery*`, `static/bazaar.html` (x4-precios-p0).
- `expired_object_delete_marker` en el lifecycle: util e higienico, pero no lo puedo validar
  contra la API real desde aca y el spec no lo pedia. Candidato a seguimiento.
