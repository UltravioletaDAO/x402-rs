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

El paso 6 es el que convierte esto de "implementar un trait" en trabajo de
verdad: en S3 la expiración la hace una regla del bucket; en Pinata la tenemos
que hacer nosotros.

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
