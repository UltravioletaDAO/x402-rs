# Backlog — el techo de 30s de API Gateway sobre el Lambda FHE

**Estado:** no implementado. El timeout ya está unificado en 90s
(commit `61bf47de`, 2026-08-31), pero **el llamador sigue cortándose a los 30**
y eso no se puede arreglar con configuración: hay que cambiar el punto de
entrada del Lambda.
**Por qué importa:** hoy nadie lo nota porque el path FHE no tiene tráfico. En
mainnet, con operaciones más pesadas y más cola en el relayer, un `/settle`
lento devuelve 504 al vendedor mientras el Lambda sigue corriendo y facturando.

---

## Qué pasa hoy

`terraform/environments/zama-testnet/main.tf` — el Lambda FHE se expone por un
**API Gateway HTTP API**:

```hcl
resource "aws_apigatewayv2_api" "main" {
  name          = "zama-facilitator-${var.environment}"
  protocol_type = "HTTP"
  ...
}

resource "aws_apigatewayv2_integration" "lambda" {
  ...
  timeout_milliseconds = min(var.fhe_request_timeout_secs * 1000, 30000)
}
```

Ese `min(...)` no es una preferencia nuestra. Es un clamp contra un límite de
AWS, escrito explícitamente para que el techo se vea en el código y no
únicamente cuando alguien recibe un 504.

Los tres saltos, con el valor que cada uno respeta:

| Salto | Configurado | Respeta |
|---|---|---|
| Proxy Rust (`FHE_PROXY_TIMEOUT_SECS`) | 90s | 90s |
| API Gateway HTTP API | 90s pedidos | **30s — techo duro** |
| Lambda (`fhe_request_timeout_secs`) | 90s | 90s |

## La evidencia

La tabla de cuotas de AWS para HTTP APIs
([http-api-quotas](https://docs.aws.amazon.com/apigateway/latest/developerguide/http-api-quotas.html)):

| Resource or operation | Default quota | Can be increased |
|---|---|---|
| Maximum integration timeout | 30 seconds | **No** |

No es un default que se sube pidiendo aumento de cuota. El aumento más allá de
29 segundos existe **solo para REST APIs regionales y privadas** — no aplica a
HTTP APIs. Verificado contra la documentación de AWS el 2026-08-31, no de
memoria.

## El modo de falla, en concreto

Pasados los 30 segundos:

1. API Gateway devuelve **504** al facilitador.
2. El proxy Rust lo propaga como error al vendedor.
3. **El Lambda no se entera y sigue corriendo** hasta su propio timeout de 90s,
   consumiendo tiempo facturado, sin nadie escuchando la respuesta.
4. Si la operación era un `/settle`, **la transacción puede haber salido igual**.

El punto 4 es el que duele: es la misma clase de bug que el forwarding del
writer-lease (`fa4530a1`) — reportar fallo por una operación que después
aterriza. Ahí se arregló subiendo el margen; acá no se puede, porque el límite
no es nuestro.

## Opciones

**A. Lambda Function URL en vez del HTTP API** — recomendada.
Hereda el timeout de la función (hasta 15 minutos), así que los 90s se
respetan de verdad. Arrastra: dominio custom (el ACM + Route53 que ya existe
apunta al API Gateway), configuración de CORS (hoy vive en
`aws_apigatewayv2_api.cors_configuration`), y los access logs
(`aws_cloudwatch_log_group.api_gw` + el `access_log_settings` del stage, que un
Function URL no produce igual). No es un swap de una línea.

**B. ALB delante del Lambda.** Sin el límite de 30s, y encaja con el ALB que ya
opera el facilitador. Pero suma costo fijo por un servicio que hoy no tiene
tráfico, y es más infraestructura que la que el problema justifica.

**C. Partir la operación: aceptar 202 + polling.** El patrón que ya usa
`/register` de ERC-8004 con `Prefer: respond-async` (`jobId` + endpoint de
status). Elimina el problema de raíz en vez de correr el techo, y es lo correcto
si las operaciones FHE resultan ser de decenas de segundos. Pero cambia el
contrato del scheme `fhe-transfer` hacia afuera, y eso hay que decidirlo antes
de que haya integradores, no después.

**D. Bajar todo a 30s y aceptarlo.** Honesto y gratis, pero solo sirve si la
latencia real queda cómodamente por debajo. Hay que medir antes de elegirlo.

## Cuándo deja de ser backlog

**Medir primero, decidir después.** El disparador es la latencia real de un
`/verify` y un `/settle` FHE contra **mainnet** — no contra Sepolia con montos
de prueba, donde nadie se acerca a 30s.

- p99 cómodamente bajo 25s → opción D, y este backlog se cierra.
- p99 acercándose o pasando 30s → **bloqueante**, y se hace A (o C si los
  tiempos son de decenas de segundos).

Esa medición es parte del Milestone 1 de
`docs/plans/zama-developer-program/01-APPLICATION.md`, así que sale sola cuando
se ataque mainnet. Ver el punto 6 de
`docs/plans/zama-developer-program/02-MAINNET-READINESS.md`.

## Qué NO hacer

- **No subir `fhe_request_timeout_secs` creyendo que arregla algo.** El Lambda y
  el proxy ya están en 90; el que corta es el gateway. Subirlo más solo alarga
  el tiempo que el Lambda queda corriendo para nadie.
- **No quitar el `min(...)`.** Terraform aceptaría el valor y la API de AWS lo
  rechazaría, o peor, quedaría el default implícito de 30s sin ninguna señal en
  el código de que existe un techo.
- **No anunciar `fhe-transfer` en mainnet en `/supported`** antes de tener la
  medición de latencia. Anunciar un scheme que devuelve 504 bajo carga real es
  exactamente el fallo que el orden de ejecución de `02-MAINNET-READINESS.md`
  está diseñado para evitar.
