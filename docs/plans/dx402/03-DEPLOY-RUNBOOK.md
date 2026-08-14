# DX402 — Runbook de despliegue

**Fecha:** 2026-08-14
**Objetivo:** DX402 corriendo en `facilitator.ultravioletadao.xyz`, usado por
nosotros, antes de proponer nada upstream.

---

## Idea central del despliegue

**Aprovisionar y encender son dos pasos separados.**

El bucket, la tabla y los permisos se crean siempre — cuestan ~$0 vacíos. La
*función* la controla `enable_dx402`, que solo decide qué variables de entorno
recibe el contenedor. Así se puede tener la infra lista y verificada antes de
tocar el path de pagos.

Con `enable_dx402 = false` (el default) las rutas `/dx402/*` **no se registran**
y `/supported` **no anuncia** la extensión. El facilitador se comporta
exactamente como hoy.

---

## Qué se creó en Terraform

`terraform/environments/production/dx402.tf`:

| Recurso | Qué guarda | Nota |
|---|---|---|
| `aws_s3_bucket.dx402_evidence` | el **ciphertext** sellado | privado, sin acceso público, SSE-S3, versionado APAGADO |
| `aws_s3_bucket_lifecycle_configuration` | — | borra por tag `dx402-retention`: 91d y 366d. `permanent` no tiene regla |
| `aws_dynamodb_table.dx402_evidence` | el **índice** paymentId → pointer | PAY_PER_REQUEST, TTL en `expires_at`, PITR |
| `aws_iam_role_policy.dx402_s3_access` | — | Get/Put/PutObjectTagging. **Sin Delete**: expirar es tarea del lifecycle |
| `aws_iam_role_policy.dx402_dynamodb_access` | — | Put/Get/Scan. **Sin Delete**: expirar es tarea del TTL |

Más `variable "enable_dx402"` (variables.tf), el secreto de firma (secrets.tf) y
las env vars del task definition (main.tf).

### El bucket es privado y se queda privado

Los compradores **nunca** leen S3 directo. Un pointer DX402 resuelve por
`GET /dx402/blob/{paymentId}` del propio facilitador, que devuelve el ciphertext.

Cuesta un salto y compra dos cosas:

1. **No hay bucket público que configurar mal** — la forma más común en que se
   filtra object storage.
2. **El pointer apunta al pago, no al layout de S3**, así que un pointer que un
   comprador tenga guardado dentro de un año sigue resolviendo aunque
   reorganicemos las keys. Y eso es exactamente lo que DX402 promete.

Servirlo sin autenticación es seguro por construcción: los bytes están sellados
hacia el pagador. Quien no tenga esa clave privada baja ruido.

---

## Pasos

### 1. Push (lo hacés vos)

```bash
git push origin main
```

Esto **es** el release: CI testea, arma la imagen, la sube a ECR y hace
`terraform apply -auto-approve` acotado (`-target` al task definition + service).

Con `enable_dx402` en `false`, ese apply **crea el bucket, la tabla y los
permisos** — pero el facilitador no cambia de comportamiento. Se puede verificar
con calma.

> **Ojo con el `-target`.** CI apunta solo al task definition y al service, así
> que **no crea el bucket ni la tabla**. Para eso hace falta un apply que
> incluya `dx402.tf` — ver paso 2.

### 2. Aprovisionar la infra

```bash
cd terraform/environments/production
terraform init
terraform plan -target=aws_s3_bucket.dx402_evidence \
               -target=aws_s3_bucket_public_access_block.dx402_evidence \
               -target=aws_s3_bucket_ownership_controls.dx402_evidence \
               -target=aws_s3_bucket_server_side_encryption_configuration.dx402_evidence \
               -target=aws_s3_bucket_versioning.dx402_evidence \
               -target=aws_s3_bucket_lifecycle_configuration.dx402_evidence \
               -target=aws_dynamodb_table.dx402_evidence \
               -target=aws_iam_role_policy.dx402_s3_access \
               -target=aws_iam_role_policy.dx402_dynamodb_access
```

**Leé el plan entero antes de aplicar.** Debe ser *solo creación* — 9 recursos
nuevos, cero cambios y cero destrucciones. Si aparece un `destroy` o un cambio en
algo que no sea DX402, parar.

```bash
terraform apply <los mismos -target>
```

No usar `-refresh=false` (inventa drift) ni un apply completo (re-sube la Lambda
de balances y arrastra un modify del ALB).

### 3. Crear la clave de firma de recibos

```bash
./scripts/dx402-bootstrap-secret.sh
```

Genera 32 bytes localmente, los guarda en `facilitator-dx402-signing-key` y
**nunca imprime la clave**. Se niega a pisar un secreto existente.

**Qué es y qué no es:** firma atestaciones EIP-712, no transferencias. No tiene
fondos ni necesita gas. Separarla de las wallets del facilitador significa que
una filtración falsifica recibos pero no mueve plata, y rotarla no cuesta nada.

### 4. Encender

En `terraform/environments/production/terraform.tfvars` (que está en
`.gitignore`, por eso no lo puedo tocar yo):

```hcl
enable_dx402 = true
```

```bash
terraform apply -target=aws_ecs_task_definition.facilitator \
                -target=aws_ecs_service.facilitator
```

### 5. Verificar

```bash
# La extensión se anuncia
curl -s https://facilitator.ultravioletadao.xyz/supported | jq '.extensions'
# esperado: ["bazaar","durable-evidence"]

# El servicio responde y publica quién firma los recibos
curl -s https://facilitator.ultravioletadao.xyz/dx402/stats | jq
# { "anchored": 0, "backend": "s3", "receiptSigner": "0x...", ... }

# Un pago que no existe da 404, no 500
curl -s -o /dev/null -w '%{http_code}\n' \
  https://facilitator.ultravioletadao.xyz/dx402/evidence/0xdeadbeef
# esperado: 404

# Los pagos normales siguen funcionando (la prueba que importa)
curl -s https://facilitator.ultravioletadao.xyz/health
```

### 6. Primer anclaje real

Recién acá se prueba de verdad. Un vendedor nuestro (KarmaCadabra o
execution.market, ver los handoffs) monta el post-hook, cobra algo, y:

1. la respuesta trae `X-Durable-Evidence`,
2. `GET /dx402/evidence/{paymentId}` devuelve pointer + hash + recibo,
3. el comprador baja el blob y lo descifra con **su** clave,
4. el `contentHash` coincide.

Cuando eso pase N veces con transacciones reales, **ahí** se propone upstream.

---

## Cómo apagarlo

`enable_dx402 = false` + apply al task definition. Las rutas desaparecen y
`/supported` deja de anunciar la extensión.

**La evidencia ya anclada no se borra** — sigue en S3 hasta que venza su
retención, y los recibos ya emitidos siguen siendo verificables offline contra la
address del firmante. Eso es a propósito: apagar la producción de evidencia nueva
no debería invalidar la vieja.

---

## Costo

| Recurso | Vacío | Con uso |
|---|---|---|
| S3 | $0 | ~$0.023/GB-mes. Un body típico de agente (1–50 KB) sellado ronda los mismos KB → **decenas de miles de anclajes por dólar** |
| DynamoDB | $0 (PAY_PER_REQUEST) | ~$1.25 por millón de escrituras |
| PITR | proporcional a la tabla | centavos a este volumen |

No es una cifra que haya medido en producción — es aritmética de la lista de
precios. La medición real sale del primer mes con tráfico.

---

## Lo que NO hace este despliegue

- **No propone nada upstream.** La Foundation exige un PR revisado y descarta
  propuestas sin uso real.
- **No implementa modo `escrowed`.** `POST /dx402/recover` devuelve 501 honesto.
  `direct` no necesita endpoint de recuperación.
- **No toca el path de pagos.** Ninguna ruta de `verify`/`settle` cambia, y un
  fallo de anclaje nunca hace fallar un pago.
- **No enciende DX402 en ningún vendedor.** Eso lo hace cada proyecto con su
  handoff.
