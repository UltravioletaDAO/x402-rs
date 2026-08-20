# DX402 — Pinata como backend de almacenamiento

**Fecha:** 2026-08-19
**Decisiones tomadas (Saul):** privado por defecto, público como opt-in por
anclaje declarado en el recibo; Pinata primario con **fallback automático a S3**.

---

## 0. Lo que ya existe (no hay que construirlo)

| pieza | estado |
|---|---|
| `EvidenceStore` trait (4 métodos) | ya está |
| `StorageBackend::Ipfs` | ya está |
| Flag `DX402_STORE_BACKEND=ipfs` | ya parsea |
| El puntero direcciona el **pago**, no el layout | ya está |
| Credenciales en Secrets Manager | **hecho**: `facilitator-dx402-pinata` (`jwt`, `api_key`, `api_secret`) |

Hoy `service.rs:175` avisa *"only the s3 backend is implemented in v0.1"* y
apaga DX402. Implementar Pinata es un archivo nuevo más un brazo de `match`.

---

## 1. La API real, medida contra la cuenta (no la documentada)

Verificado end-to-end el 2026-08-19. **Tres cosas difieren de la doc** y cambian
el diseño:

```
POST https://uploads.pinata.cloud/v3/files      Authorization: Bearer <jwt>
  multipart: file, network=private|public, name, keyvalues, cid_version
  -> {"data":{"id","cid","network","size","keyvalues",...}}
```

1. **`name` con barra se trunca.** Mandé `evidence/0xabc.dx402` y volvió como
   `evidence`. El layout tipo S3 **no sobrevive**: el índice confiable es
   `keyvalues.paymentId`, no el nombre.
2. **Dedupea por contenido.** Subir los mismos bytes devuelve el registro
   *anterior*, incluida su red: un upload pedido como `public` volvió con
   `"network":"private"`. No nos afecta (cada sobre lleva un nonce aleatorio, así
   que el ciphertext es único) pero es una trampa si alguna vez se ancla algo
   determinista.
3. **Los privados se leen sólo con URL firmada del gateway PROPIO de la cuenta**
   (`amaranth-broad-whippet-395.mypinata.cloud`). Contra el gateway genérico da
   **403**. Endpoint: `POST /v3/files/private/download_link` con
   `{url, date, expires, method}`.

Medido además:

| propiedad | privado | público |
|---|---|---|
| resoluble desde `ipfs.io` sin credenciales | **no** (HTTP 000) | **sí** (HTTP 200) |
| `DELETE /v3/files/{net}/{id}` | 200, borrado real | 200, pero sólo despinnea |
| retención de 90 días | **se cumple** | **NO se cumple** |

**La consecuencia que manda el diseño:** en público, despinnear saca nuestra
copia, no la de la red. `retention: 90d` pasaría a significar *"dejamos de
pagarlo"*, y ese `retentionUntil` va **firmado por nosotros** en el recibo. Por
eso privado es el default.

### Trampa operativa: el JWT vence

`exp = 1797682702` → **2026-12-19**. Un JWT vencido no falla ruidosamente: los
anclajes van a caer al fallback de S3 y nadie se va a enterar salvo por los logs.
Hay que (a) alarmar antes de esa fecha, y (b) que el fallback logue con nivel
`warn` y una categoría propia, no genérica.

> El JWT lleva `api_key` y `api_secret` en claro dentro del payload. Es
> equivalente a las credenciales completas: tratarlo como tal.

---

## 2. Diseño

### 2.1 El puntero declara la red — sin tocar el recibo

`Dx402EvidenceReceipt` tiene orden de campos **normativo**: agregar uno cambia el
type hash e invalida la verificación de los 86 recibos ya emitidos. Pero el
puntero **ya está dentro del struct firmado** (`string pointer`), así que la
distinción viaja ahí y el recibo la declara con **cero cambios de esquema**:

```
privado  ->  ipfs+https://facilitator.ultravioletadao.xyz/dx402/blob/{paymentId}
público  ->  ipfs://{cid}
s3       ->  s3+https://facilitator.ultravioletadao.xyz/dx402/blob/{paymentId}   (hoy)
```

El privado se resuelve por nosotros (firmamos la URL al vuelo); el público lo
resuelve cualquiera sin pasar por el facilitador, que es exactamente lo que se
está pidiendo al elegirlo.

### 2.2 El CID se calcula ANTES de subir

El orden reserva-primero-escribe-después es load-bearing (v1.84.0: un duplicado
rechazado destruía la evidencia que perdía). Pero un puntero `ipfs://{cid}`
necesita el CID, y el CID necesita el contenido.

Se resuelve sin romper el orden: un CIDv1 raw (`bafkrei…`) es
`base32(multihash(sha256(bytes)))` — **determinista y calculable local** para un
bloque único, y nuestros sobres pesan ~4 KB. Requiere el crate `cid` (+ `multihash`).

Eso obliga a un cambio chico en el trait, que además es honesto:

```rust
// antes: el nombre sólo depende del pago
fn pointer_for_payment(&self, payment_id: &str) -> DurablePointer;
// después: algunos backends nombran por contenido
fn pointer_for(&self, payment_id: &str, blob: &[u8]) -> DurablePointer;
```

S3 ignora `blob`. Pinata privado también. Pinata público calcula el CID.

### 2.3 El fallback es un store compuesto

```rust
struct FallbackEvidenceStore { primary: Arc<dyn EvidenceStore>, fallback: Arc<dyn EvidenceStore> }
```

Reglas que no son negociables:

- **`backend` en el registro debe decir dónde aterrizó de verdad**, no lo que
  estaba configurado. Si dice `ipfs` y quedó en S3, la lectura va al lugar
  equivocado y la evidencia se ve perdida sin estarlo.
- El puntero debe ser **el del store que ganó**, y se firma ese.
- Un fallback es un evento operativo: `warn` con categoría propia
  (`dx402_pinata_unavailable`), nunca un error genérico.
- Timeout obligatorio (10 s, igual que el hook del vendedor). **Un fallo de
  Pinata no puede bloquear un pago**; si los dos fallan, se degrada a skip.

### 2.4 Quién elige: tres decisiones, tres actores

**Definido por Saul, 2026-08-19.** La pregunta "¿variable global o elección por
llamada?" mezcla tres decisiones distintas:

| decisión | quién | dónde vive |
|---|---|---|
| qué backends **puede** ofrecer este facilitador | operador | variable global |
| cuál **usa** este anclaje | vendedor | `AnchorRequest`, por llamada |
| **qué cuesta** | vendedor <-> comprador | x402, que ya existe |

La variable global no es la elección de producto: es la **capacidad del
despliegue**. Un operador sin llave de Pinata no puede ofrecer IPFS, y eso no
debe impedirle al vendedor elegir entre lo que sí hay.

Se elige por llamada porque **no son el mismo producto**. Privado/S3 promete "lo
guardamos 90 días y lo podemos borrar"; IPFS público promete "es permanente y
nadie lo baja, nosotros incluidos". Un switch global mete a todos los vendedores
del facilitador en la misma promesa.

#### El límite duro: `public` necesita el consentimiento del COMPRADOR

`public` es irreversible, y **no afecta a quien lo elige**. El blob es la compra
del comprador, cifrada a su clave. Si el vendedor elige unilateralmente
"permanente y público", el ciphertext del comprador queda en IPFS para siempre
sin que el comprador lo haya aceptado — y el spec ya advierte que ECDH sobre
secp256k1 no es post-cuántico, así que "permanente" es una apuesta sobre un
futuro que el comprador no firmó.

> El vendedor elige libremente entre las opciones **reversibles**. `public` --lo
> único irrevocable-- exige el consentimiento del comprador, no sólo el del
> vendedor.

El canal ya está diseñado: el opt-in del comprador vía `accepts`
(`05-DISENO-v0.2.md`), que es la misma negociación donde el comprador pide
evidencia durable y aporta su clave.

**Regla intermedia, porque ese opt-in todavía no está implementado:** `public`
queda apagado por operador hasta que exista. `private` y `s3` se eligen por
llamada desde ya. Entregable ahora, sin comprometer a nadie.

#### Precios: fuera del facilitador

El costo real de 50 KB por 90 días es **$0.0000035** (§3 del backlog). Cobrar
"para cubrir el storage" sería deshonesto. Lo que tiene valor es *poder
reclamar*.

El vendedor le pone precio a la variante durable en su propio `accepts` -- que es
exactamente cómo x402 cobra distinto por recursos distintos. **El facilitador no
necesita conocer ningún precio.** Construir una tarifa acá sería inventar un
segundo protocolo de pago al lado del que estamos extendiendo.

#### Dos consecuencias operativas

- **Nunca degradar en silencio.** Si el vendedor pide un backend que el operador
  no ofrece, se responde con error claro. Guardarlo en otro lado calladamente
  deja a un vendedor creyendo que su evidencia es permanente cuando no lo es --
  peor que decirle que no. (Distinto del **fallback por FALLO** de §2.3, que sí
  es correcto porque no cambia la promesa: S3 es el backend más conservador, y
  el registro declara dónde aterrizó.)
- **Publicar qué se ofrece.** `/supported` (o `/dx402/stats`) anuncia los
  backends disponibles. Nadie elige bien a ciegas, y sin esto la elección por
  llamada no es usable.

### 2.5 Opt-in por anclaje

`AnchorRequest.storage_network: Option<StorageNetwork>` (`private` | `public`),
**default `private`**. Mismo precedente que `retention: permanent`: global con
opt-in por ruta, y elegir permanencia es una decisión explícita del vendedor.

---

## 2.6 Frontend: anunciar lo que realmente hay

**Regla rectora: la landing no puede tener una lista de backends escrita a mano.**
Es la misma disciplina que ya impone `scripts/verify_landing_canonical.py` sobre
el conteo de redes -- una landing que promete algo que el facilitador no ofrece es
peor que una landing sin esa sección.

### Lo que ya existe (verificado en `static/index.html`)

| pieza | estado |
|---|---|
| Sección DX402 con 3 tarjetas (Durable / Private / Coupled) | ya está, ~línea 2515 |
| Contador vivo `data-live-count="dx402-anchors"` desde `/dx402/stats` | ya está, ~línea 3209 |
| Degradación suave (si el fetch falla, queda el placeholder) | ya está |
| i18n EN/ES (128 `data-i18n`, dos diccionarios al final) | ya está |
| Mención de storage **agnóstica** ("not the storage backend") | ya está -- no hay que corregir nada |

O sea que no hay que construir la sección: hay que **alimentarla**.

### Hueco encontrado al revisar: la landing anuncia DX402 aunque esté apagado

La sección es HTML estático, así que se renderiza siempre. Con `ENABLE_DX402=false`
(el default del fork), `/dx402/stats` da 404, el contador queda en `—` y la página
igual promete evidencia durable. Para nosotros es cosmético porque está encendido;
para cualquier operador que corra este fork es una promesa falsa.

**Arreglo:** la sección se muestra sólo si `/supported` incluye `durable-evidence`
en `extensions`. Una condición, sin backend nuevo -- ese campo ya existe.

### Prerrequisito: hoy el anuncio miente cuando la config está a medias

La señal existe y es la correcta: `/supported` arma `extensions` desde
`Dx402Config::from_env().enabled` (`handlers.rs:1711`), o sea **desde
`ENABLE_DX402`**. El frontend puede confiar en eso.

Pero las rutas se registran sólo si el *servicio* se construyó
(`main.rs:588`, `match dx402_service`), y el servicio devuelve `None` si falta el
bucket, el public base, o si el backend pedido no está implementado. Entonces:

```
ENABLE_DX402=true + DX402_STORE_BUCKET sin definir
  -> el servicio es None      -> /dx402/* NO existe
  -> /supported igual anuncia "durable-evidence"
```

El comentario en `handlers.rs:1707` dice que existe justamente para no *"anunciar
una extensión cuyas rutas dan 404"*, y no lo consigue: mira el flag, no el
servicio. Es cosmético hoy (nuestra config está completa) y deja de serlo en el
momento en que el frontend se cuelgue de esa señal, que es lo que este plan hace.

**Arreglo, previo a lo del frontend:** anunciar según el servicio exista, no según
el flag. Con Pinata suma un caso más -- `ENABLE_DX402=true` con la llave de Pinata
vencida y sin fallback -- así que el problema crece con este trabajo.

### El cambio de API que lo habilita

`/dx402/stats` hoy devuelve el backend **activo**, no los **disponibles**:

```json
{"anchored":86, "mode":"90d", "backend":"s3", "receiptSigner":"0x7bC4…"}
```

Se agrega el allowlist, sin romper el campo actual:

```json
{"anchored":86, "mode":"90d",
 "backend":"s3",                                  // el default, se mantiene
 "backends":[                                     // NUEVO: lo que este despliegue ofrece
   {"id":"s3",           "retention":"90d", "revocable":true,  "public":false},
   {"id":"ipfs-private", "retention":"90d", "revocable":true,  "public":false},
   {"id":"ipfs-public",  "retention":"permanent", "revocable":false, "public":true, "enabled":false}
 ],
 "receiptSigner":"0x7bC4…"}
```

`revocable` y `public` no son decoración: son **la diferencia de producto** que el
vendedor necesita ver para elegir. Un backend con `enabled:false` se muestra en
gris con su motivo (hoy `ipfs-public` espera el opt-in del comprador, §2.4).

### Lo que se agrega en la landing

1. Una cuarta tarjeta **"Dónde vive"** en la grilla de DX402, poblada desde
   `backends`. Cero listas escritas a mano.
2. Por backend: nombre, retención, y si es **revocable o irreversible**. Esa
   última es la que le importa a quien decide, y es la que una landing tiende a
   omitir porque no vende.
3. Un aviso explícito en `ipfs-public`: *"permanente e irrevocable; ni nosotros lo
   podemos bajar"*. Si se anuncia permanencia hay que anunciar también que no hay
   vuelta atrás.
4. **i18n en los DOS diccionarios** (EN y ES). Es el olvido clásico de esta página.
5. Degradación suave igual que el contador: si `/dx402/stats` no responde, la
   tarjeta no aparece -- nunca una lista por defecto inventada.

### Verificación

Extender `scripts/verify_landing_canonical.py` (o un hermano
`verify_landing_dx402.py`) para que falle si:

- la landing nombra un backend que `/dx402/stats` no lista;
- `/dx402/stats` lista uno que la landing no sabe renderizar;
- una cadena i18n existe en EN y falta en ES, o al revés;
- la sección DX402 se muestra con la extensión ausente de `/supported`.

---

## 3. Orden de trabajo

| # | qué | por qué en ese orden |
|---|---|---|
| 1 | `pointer_for(payment_id, blob)` en el trait + S3 sin cambios de conducta | cambio mecánico, aislado, con los tests actuales de guardia |
| 2 | `PinataEvidenceStore` (upload, signed-url read, delete) + tests contra un mock | el grueso; sin tocar producción |
| 3 | `FallbackEvidenceStore` + test de "primario caído -> aterriza en el fallback y el registro lo dice" | la parte que puede mentir sobre dónde está la evidencia |
| 4 | `DX402_STORE_BACKENDS` como **allowlist** (no elección única) + anunciarlos en `/supported` | sin esto la elección por llamada no es usable |
| 4b | `storage` en `AnchorRequest` + esquema del puntero, con error claro si el operador no lo ofrece | superficie de API |
| 5 | Terraform: leer el secreto, IAM, variable `dx402_storage_backend` | despliegue |
| 6 | Barredor de retención (borra lo vencido en Pinata privado) | **sin esto, "90d" es mentira en el camino Pinata** |
| 7 | `scripts/dx402-e2e-check.py` contra el backend Pinata | la verificación real |
| 8 | `/dx402/stats` devuelve `backends[]` con `retention`/`revocable`/`public` | es lo que alimenta al frontend Y al vendedor que elige |
| 9 | Landing: tarjeta "Dónde vive" poblada desde la API + i18n EN/ES + ocultar la sección si la extensión no está | anunciar sólo lo que existe |
| 10 | `verify_landing_dx402.py` en CI | sin esto la landing vuelve a mentir en dos releases |

El paso 6 es el que convierte esto de "implementar un trait" en trabajo de
verdad: en S3 la expiración la hace una regla del bucket; en Pinata la tenemos
que hacer nosotros.

---

## 3-bis. Documentación al cerrar (facilitador y SDKs)

No es un paso opcional al final: es parte del entregable. Enumerado sobre lo que
**realmente** existe hoy, verificado con grep, no supuesto.

### Facilitador

| archivo | qué cambia |
|---|---|
| `docs/DX402.md` | la guía: elección de backend, tabla retención/revocabilidad, y que `public` es irreversible |
| `docs/plans/dx402/02-SPEC-v0.1.md` | `storage` en `AnchorRequest`, esquemas de puntero (`ipfs+https://` vs `ipfs://`), nuevos códigos de error |
| `CLAUDE.md` | dice *"only `s3` in v0.1"* -- queda falso el día que Pinata entre |
| `src/openapi.rs` | 19 menciones de dx402; `/dx402/stats` cambia de forma (`backends[]`) y `/dx402/anchor` gana un campo |
| `README.md` | 16 menciones; la sección DX402 describe el almacenamiento |
| `docs/plans/dx402/03-DEPLOY-RUNBOOK.md` | el secreto de Pinata, el barredor de retención, y la alarma del JWT |
| `docs/CHANGELOG.md` | entrada de la versión |
| **`.env.example`** | **hoy no tiene NINGUNA variable DX402** -- quien desarrolla local no sabe que existen |

### SDKs — el hueco es peor de lo que parece

**Los README de los dos SDKs no mencionan DX402 ni una sola vez** (verificado:
0 coincidencias en ambos). Publicamos `anchor_evidence`, `recover_evidence`,
`seal_evidence`, los helpers de firma y el opt-in entero, y **no está documentado
en el lugar donde alguien lo buscaría**. Eso no es "actualizar la doc", es
escribirla por primera vez.

| entregable | contenido mínimo |
|---|---|
| README de Python y de TS | sección DX402: lado vendedor en una llamada, lado comprador, y **qué significa `verified` vs `signed`** (cambió en 1.84.0 y no está escrito en ningún README) |
| docstrings / JSDoc de `anchor_evidence` | el nuevo parámetro `storage`, y que `public` es irreversible |
| Ambos | que `proofOfPayment` es lo único que llega a `verified: true` |
| Ambos | tabla de backends y su retención, con el enlace a `/dx402/stats` como fuente viva |

**Regla:** la doc de los SDKs no puede traer una lista de backends escrita a
mano, por el mismo motivo que la landing. Se documenta *cómo preguntar*
(`GET /dx402/stats`), no *qué hay* -- porque lo que hay depende del despliegue
contra el que se hable, y un integrador puede apuntar a un facilitador que no es
el nuestro.

### Verificación de la doc

- `verify_landing_dx402.py` cubre la landing (§2.6).
- Para los SDKs: un test que importe los símbolos que el README promete. Es
  barato y ataca el modo de falla real -- un README que documenta una función
  que se renombró. Ya nos pasó con la mitad vendedora que no era importable
  desde la raíz del paquete.

---

## 4. Criterio de terminado

1. `DX402_STORE_BACKEND=pinata` levanta y ancla.
2. `scripts/dx402-e2e-check.py` pasa **21/21** contra ese backend, incluidas las
   dos que importan: el pagador descifra, y otra wallet **no**.
3. Test de que con Pinata caído el anclaje **aterriza en S3**, el registro dice
   `backend: s3`, y el pago no se ve afectado.
4. Test de que con los dos caídos se degrada a `skip` y el pago tampoco.
5. Un anclaje `public` es resoluble desde `ipfs.io` **sin** credenciales; uno
   `private` **no**.
6. El barredor borra un objeto vencido y `GET /dx402/blob` devuelve 410, no 500.
7. Pedir un backend que el operador NO ofrece devuelve un error nombrándolo --
   **no** un anclaje silencioso en otro lado.
8. `/supported` anuncia los backends disponibles, y coincide con lo que el
   facilitador realmente acepta.
9. La landing muestra los backends **que devuelve la API**, no una lista fija:
   apagar Pinata y recargar hace desaparecer la opción sin tocar el HTML.
10. Con `ENABLE_DX402=false` la sección DX402 **no se muestra**, en vez de
    prometer una función apagada.
11. `verify_landing_dx402.py` falla si la landing y `/dx402/stats` discrepan, y
    si una cadena i18n existe en un idioma y no en el otro.
12. `/supported` anuncia `durable-evidence` **sólo si el servicio existe**, no
    sólo si el flag está: con `ENABLE_DX402=true` y el bucket sin configurar,
    la extensión NO se anuncia y la landing no muestra la sección.
13. Los README de ambos SDKs tienen sección DX402, y un test importa cada símbolo
    que prometen.
14. `.env.example` documenta todas las variables DX402, incluidas las de Pinata.

---

## 5. Riesgos anotados

- **El JWT vence el 2026-12-19.** Sin alarma, el síntoma es "todo sigue andando"
  con la evidencia yéndose calladita a S3.
- **`public` es irreversible.** Un vendedor que lo pide una vez publicó ese blob
  para siempre; el cifrado aguanta, pero el spec ya advierte que ECDH sobre
  secp256k1 no es post-cuántico.
- **La cuenta de Pinata es compartida** con otro proyecto (tenía
  `chamba-agent-metadata.json` de enero). Cualquier operación de limpieza tiene
  que filtrar por lo nuestro — ver el incidente del 2026-08-19.
- **Costo por GB** frente a S3: hoy son 335 KB en total, así que es irrelevante a
  esta escala, pero conviene medirlo antes de mover el default.
