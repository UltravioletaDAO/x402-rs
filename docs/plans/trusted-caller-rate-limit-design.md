---
date: 2026-08-29
tags:
  - type/plan
  - domain/rate-limiting
  - priority/p1
status: proposal — pendiente de decisión de Saul
---

# Diseño — identidad de servicios propios y límites de tasa por familia de endpoint

## Resumen para Saul

El incidente (barrido de 9 redes en paralelo contra `/identity/{network}/owner/{address}`,
p99 de 0,4s a 11,6s) no chocó con un límite mal calibrado — chocó con la ausencia total de
uno. `handlers::routes()` se mergea en `src/main.rs` sin ninguna capa `GovernorLayer`, y ese
router incluye exactamente el endpoint que causó el incidente. Ese hallazgo, el fix de la
causa raíz (`totalSupply()` en vez del probe secuencial) y el governor nuevo para
`/identity/*` **ya están en implementación** — los está escribiendo `lat-nonce` en
`src/handlers.rs`/`src/main.rs`, este documento no los repite.

Lo que queda es diseño puro, sin código, para que decidas con la información completa:

1. **Cómo identificamos a Execution Market/MeshRelay como "servicio propio"** para darles
   trato distinto — recomiendo un token compartido, no IP.
2. **Por qué ese trato es un techo generoso y no "ilimitado" literal** — pediste infinito,
   y hay una razón medida para no dártelo tal cual.
3. **Una deuda técnica que encontré al lado**: los 5 rate limiters que ya existen no siguen
   el patrón de configuración que el propio `main.rs` usa para todo lo demás.
4. **Dónde encaja el WAF del ALB** frente al límite de la aplicación, y cuánto cuesta.

---

## 1. Identidad de servicios propios: token compartido, no IP

### Por qué no IP ni CIDR de VPC

Execution Market y MeshRelay le pegan al ALB público como cualquier cliente x402 — no están
en nuestra VPC. Confirmé en `terraform/environments/production/main.tf` que el NAT gateway
tiene una Elastic IP propia (`aws_eip.nat`, `main.tf:79-90`), pero esa estabilidad es de
**nuestro** tráfico saliente hacia los RPCs, no del tráfico entrante de esos servicios — su
IP de salida es infraestructura de ellos, fuera de nuestra visibilidad y de nuestro control.

Un allowlist de IP nos deja atados a que un tercero nos avise antes de redeployar su propia
infraestructura (cambio de NAT, de región, de proveedor). Si no avisa, se rompe en silencio:
o el allowlist queda permisivo con una IP que ya no es de ellos (fail-open sin darnos
cuenta), o se queda corto y les negamos servicio (fail-closed sin que sea intencional). CIDR
de VPC directamente no aplica: no hay tráfico interno real que filtrar, todo llega desde
Internet al ALB.

Un token es identidad que **nosotros** emitimos y controlamos: revocable al instante,
rotable ante sospecha, en un solo lugar (Secrets Manager) — sin depender de que un tercero
nos mantenga informados de su propia infraestructura.

### El patrón ya existe en este mismo archivo

`src/handlers.rs:801-817` ya tiene `BAZAAR_ADMIN_TOKEN` y `ERC8004_ADMIN_TOKEN`, con
`admin_auth()` (`handlers.rs:818`) comparando en tiempo constante
(`constant_time_eq()`) contra un valor leído de env var, y `admin_reject()`
(`handlers.rs:839`) convirtiendo el resultado en una respuesta. Reusar esa forma —no
necesariamente esas funciones tal cual, ver la diferencia de fail-open/fail-closed abajo—
significa que quien lo implemente no inventa un mecanismo de autenticación nuevo.

**Diferencia importante con esos dos tokens**: `BAZAAR_ADMIN_TOKEN` y `ERC8004_ADMIN_TOKEN`
son **fail-closed** — sin token configurado, la ruta responde 404 y es indistinguible de
"no existe". Eso es correcto para rutas de administración que nunca deberían ser públicas.

El token de servicio propio tiene que ser **fail-open**: el facilitador es público por
diseño, cualquiera debe poder pegarle a `/identity/owner` sin token — solo que sin él cae
bajo el límite estricto, en vez de quedar bloqueado. La ausencia de token nunca es un error;
es simplemente "tratá a este caller como público".

### Formato del secret propuesto

Mismo patrón que `facilitator-rpc-mainnet` (`terraform/environments/production/secrets.tf`):
un secret con una key JSON por servicio, no un token único compartido por todos. Así se
puede rotar el de Execution Market sin tocar el de MeshRelay, y cada uno se identifica por
nombre en los logs (nunca el token, solo el nombre):

```json
// facilitator-trusted-callers (AWS Secrets Manager)
{
  "execution-market": "<token opaco, generado con secrets.token_hex(32) o similar>",
  "meshrelay": "<token opaco, distinto>"
}
```

Header en la request: `Authorization: Bearer <token>` — mismo header que ya usan los admin
tokens, ningún header nuevo que documentar en `/docs`.

### El despacho — por qué un solo `GovernorLayer` no alcanza

Esto es lo que alguien va a intentar y no va a funcionar, así que lo dejo explícito:
**`tower_governor` aplica UNA sola cuota (`period` + `burst_size`) por `GovernorConfig`,
aplicada a todas las claves que devuelva el `KeyExtractor`.** No hay forma de que, dentro de
una misma capa, una clave (`trusted:execution-market`) tenga una cuota distinta de otra
(`ip:1.2.3.4`) — cambiar el extractor de clave no cambia la cuota, solo cambia cómo se
agrupan los buckets bajo la MISMA cuota.

La única forma correcta de dar dos tratos distintos es tener **dos `GovernorConfig`/capas
distintas** y decidir, ANTES de que el governor vea el request, cuál de las dos aplica. En
axum/tower eso se hace con un branch a nivel de `Service` — el patrón estándar es
`tower::util::Either` (o un middleware `from_fn` que decide y despacha a uno de dos routers
internos), nunca un solo layer con lógica condicional adentro:

```rust
// Pseudocódigo ilustrativo — NO es la implementación final, solo el mecanismo.
//
// El middleware de confianza corre AFUERA (después, en orden de .layer()) del
// GovernorLayer estricto, y para un caller válido despacha directo al handler
// por un segundo Router/Service SIN el governor estricto en el medio —
// nunca "salta" el governor desde adentro del mismo Service, porque el layer
// ya corrió antes de que el handler exista.
async fn trusted_caller_dispatch(
    headers: HeaderMap,
    State(state): State<TrustedDispatchState>,
    req: Request,
) -> Response {
    match validate_trusted_token(&headers, &state.trusted_tokens) {
        Some(caller_name) => {
            tracing::info!(caller = %caller_name, "Trusted caller — governor generoso");
            state.trusted_router.oneshot(req).await  // GovernorConfig generoso
        }
        None => state.public_router.oneshot(req).await, // GovernorConfig estricto, keyed por IP
    }
}
```

Dos `Router` con las MISMAS rutas montadas, cada uno con su propio `GovernorLayer`, y un
`Either`/dispatcher eligiendo cuál atiende cada request. Nombro el mecanismo acá para que
quien lo implemente no pierda tiempo redescubriendo que "un extractor de clave más
inteligente" no resuelve esto solo.

### Por qué el techo es generoso y no infinito

Pediste que los servicios propios tengan llamadas ilimitadas. La razón para NO dártelo tal
cual, literal, está medida y no es hipotética: el diagnóstico de performance del 20-ago
(`docs/handoffs/2026-08-20-diagnostico-performance-facilitador.md`, sección "CAUSA RAÍZ #1")
midió a **Execution Market reintentando hasta 39 veces por un solo submission** durante un
hueco de nonce que abrimos nosotros — no un bug de ellos, un bug nuestro (`call.send().await`
crudo sobre el mismo `PendingNonceManager` que `/settle`, `handlers.rs:4189` y otros 13
sitios iguales).

Un token sin ningún techo convierte cualquier bug nuestro en un martillo interno contra
nosotros mismos: el mismo cliente de confianza que hoy reintenta 39 veces en un hueco de
nonce, sin límite, puede convertir ese hueco en una ráfaga de miles de requests sin que nada
lo frene, exactamente cuando el sistema ya está en mal estado.

Un **techo generoso** (propongo un orden de magnitud como 50-100x el límite estricto por
familia — el número final lo fija quien implemente contra tráfico real medido, no a ojo)
cumple lo que pediste: en operación normal, Execution Market y MeshRelay nunca lo van a ver.
Y sigue actuando de fusible el día que algo nuestro entra en loop contra ellos.

---

## 2. Deuda técnica: los 5 governors existentes no siguen el patrón de config del propio archivo

`src/main.rs` ya tiene el patrón correcto para exactamente este tipo de config — default en
código + override por env var + log del valor efectivo al boot — escrito para
`MAX_REQUEST_BODY_BYTES`:

```rust
// main.rs:43-45 y 435-439
const DEFAULT_MAX_REQUEST_BODY_BYTES: usize = 64 * 1024;
// ...
let max_body_bytes = std::env::var("MAX_REQUEST_BODY_BYTES")
    .ok()
    .and_then(|s| s.parse::<usize>().ok())
    .map(|n| n.max(16 * 1024))
    .unwrap_or(DEFAULT_MAX_REQUEST_BODY_BYTES);
tracing::info!(max_body_bytes, "HTTP request body limit configured");
```

Pero los 5 `GovernorConfigBuilder` de ese mismo archivo (`main.rs:456` `/verify`+`/settle`,
`489` `/discovery/register`, `505` lecturas de discovery, `519` `/events`, y ahora `540`
`/identity/*` de `lat-nonce`, que sí sigue el patrón correcto) tienen sus valores
**hardcodeados como literales** (`.per_second(2)`, `.burst_size(30)`, etc.) — cero override
por env var, cero forma de subir o bajar un umbral sin recompilar y redeployar. Contradice
la regla de configuración centralizada: hoy esos umbrales están definidos en un solo lugar,
sí, pero no son ajustables en vivo, que es la otra mitad de la regla.

**Qué habría que cambiar**: replicar para cada uno de los 4 governors originales el mismo
patrón que ya usa `MAX_REQUEST_BODY_BYTES` y el que `lat-nonce` ya usó para
`identity_read_rate_limit()` — una función tipo `verify_settle_rate_limit() -> (u64, u32)`
por familia, con su propio par de env vars (`VERIFY_SETTLE_RATE_PER_MS` /
`VERIFY_SETTLE_RATE_BURST`, etc.) y log del valor efectivo al boot.

**El riesgo de hacerlo**: no es el cambio de código en sí — es que, a diferencia de
`MAX_REQUEST_BODY_BYTES` (que tiene un `.max(16 * 1024)` como piso de seguridad), un
`GovernorConfigBuilder` mal parseado o con un valor absurdo (`per_second(0)`, por ejemplo)
puede fallar el `.expect(...)` al boot y tirar abajo el proceso entero — el patrón nuevo
necesita el mismo tipo de piso/saneo que `max_body_bytes` ya tiene, y el `/verify`+`/settle`
es el más sensible de los 5 porque gobierna el camino del dinero: **un valor mal puesto ahí
en producción, aunque sea "solo" una variable de entorno, puede frenar pagos reales**. No es
un cambio para hacer sin un revisor humano mirando el valor antes de aplicar el
`terraform apply`, ni parte del deploy automático de imagen.

No lo estoy implementando — queda anotado para decidir si se hace ahora, junto con el
governor nuevo, o como deuda técnica separada.

---

## 3. WAF del ALB — complemento, no reemplazo

Hoy no existe ningún `aws_wafv2_web_acl` en Terraform. Encaja bien como **backstop
grueso**, exactamente para el caso que describiste — "un host desconocido que pegue mil
veces" — cortado antes de gastar un solo RPC, más barato que dejar que el request llegue
siquiera al governor de la aplicación.

**Qué cubre**: una `RateBasedStatement` (requests por 5 minutos por IP de origen) frena a
un solo actor golpeando fuerte, sin escribir ni desplegar código Rust — es el cambio más
rápido de aplicar de todo este documento.

**Qué NO cubre, y es lo que más importa**: el WAF opera por IP + patrón de URL, no por
costo real. No sabe que `/identity/owner` cuesta hasta 128.000 llamadas `ownerOf()`
(`OWNER_SCAN_MAX_BATCHES`, `handlers.rs`) y un logo PNG cuesta cero — así que no puede
reemplazar la política por familia de endpoint, solo puede poner un piso parejo para todos.
Tampoco resuelve la identificación de servicios propios: mismo problema de IP que en la
sección 1 — un WAF allowlist de IP tiene exactamente la misma fragilidad frente a
infraestructura de terceros que no controlamos.

**Umbral propuesto**: generoso a propósito — algo del orden de 2000 req/5min por IP — para
no interferir nunca con el governor de la aplicación, que es el que hace el trabajo fino por
familia. El WAF está para parar una inundación obvia, no para calibrar tráfico legítimo.

**Costo** (verificado contra la página de pricing de AWS WAF, no de memoria): USD 5,00/mes
por Web ACL + USD 1,00/mes por regla + USD 0,60 por millón de requests procesados. Con 1 Web
ACL y 1 regla rate-based, contra el tráfico medido en el diagnóstico del 20-ago
(900-3800 req/h, ~2-2,7M req/mes en el peor caso observado), el total ronda **USD 7-9/mes**.

Es un recurso Terraform nuevo — no lo agrego ni lo aplico. Queda como propuesta para decidir
en la misma conversación que el resto de este documento.

---

## Referencias

- `docs/handoffs/2026-08-20-diagnostico-performance-facilitador.md` — diagnóstico de
  performance del 20-ago, incluye los 39 reintentos de Execution Market citados en la
  sección 1 y el resto de causas raíz de latencia del facilitador (no relacionadas con este
  documento salvo esa cifra).
- `src/handlers.rs:801-839` — `BAZAAR_ADMIN_TOKEN`/`ERC8004_ADMIN_TOKEN`, `admin_auth()`,
  `admin_reject()` — el patrón de token existente que este diseño reusa parcialmente.
- `src/main.rs:43-45,435-439` — patrón de config centralizada (`MAX_REQUEST_BODY_BYTES`)
  que la sección 2 propone extender a los governors existentes.
- `terraform/environments/production/main.tf:79-146` — NAT gateway, EIPs, security groups
  del ALB, base de la sección "por qué no IP".
- `terraform/environments/production/secrets.tf` — patrón de secret con una key JSON por
  entrada (`facilitator-rpc-mainnet`), base del formato de secret propuesto en la sección 1.
