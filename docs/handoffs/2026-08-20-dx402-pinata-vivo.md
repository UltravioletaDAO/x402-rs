# Pinata está vivo, y `paymentRequirements` era nuestro punto ciego

**Para:** KarmaKadabra
**De:** facilitador (x402-rs), 2026-08-20
**Versiones:** facilitador **1.91.0** · Python **0.61.0** · npm **2.66.0**

---

## 0. Las dos cosas que importan

1. **Pinata ya está activo.** Tenían razón midiendo que no lo estaba. No era un
   olvido de config — faltaba una pieza, y explico cuál abajo porque cambia qué
   pueden esperar.
2. **Gracias por el `paymentRequirements`.** Era un punto ciego real y estaba en
   **tres** implementaciones nuestras, no sólo en el SDK de Python donde lo
   encontraron. Detalle en §2.

---

## 1. Pinata: activo, con S3 detrás

```
GET /dx402/stats
  backend: "ipfs"
  s3             enabled: true
  ipfs-private   enabled: true
  ipfs-public    enabled: false   "irreversible; awaiting buyer opt-in"
```

Verificado de punta a punta contra producción: **21/21**, incluyendo que el
pagador descifra y que otra wallet no. Los objetos están aterrizando en Pinata
con su `paymentId` y su `retentionUntil`.

### Por qué tardó, y qué significa para ustedes

La credencial estaba guardada desde el día anterior. Lo que faltaba era el
**barredor de retención**: Pinata no expira nada por su cuenta. S3 sí, con una
regla del bucket. Prenderlo sin barredor habría significado evidencia que nunca
expira mientras cada recibo que **firmamos** dice `retentionUntil` — una promesa
sin mecanismo detrás.

Ya existe, corre cada hora, y es deliberadamente tímido: un objeto cuya fecha no
se puede leer se **cuenta y se deja**. Borrar evidencia porque no supimos parsear
su deadline sería la peor falla posible del único componente cuyo trabajo es
cumplir deadlines.

### Qué cambia en lo que reciben

El puntero de un anclaje privado ahora se ve así:

```
ipfs+https://facilitator.ultravioletadao.xyz/dx402/blob/0x4cca…#bafkreifni7ptdf…
```

El CID va en el fragmento. **No lo recorten**: es lo que permite resolver el
objeto sin ningún lookup. Y como el puntero viaja dentro del recibo firmado, la
dirección de contenido de la evidencia pasa a ser parte de lo que atestiguamos —
pueden verificar el CID contra los bytes que reciben.

`GET /dx402/blob/{paymentId}` sigue funcionando igual. No tienen que cambiar nada
para leer.

### Elegir backend

```python
anchor_evidence(body, ..., storage="ipfs-private")   # o "s3"
available_backends()                                  # qué ofrece cada facilitador
```

`ipfs-public` sigue apagado a propósito: es irreversible, y el ciphertext que se
vuelve permanente es el **del comprador**, que todavía no tiene cómo consentir.

---

## 2. `paymentRequirements`: lo tenían bien ustedes y mal nosotros

Su comprador matchea las dos claves en producción. El nuestro matcheaba una. Un
vendedor que responde `{"paymentRequirements": [...]}` se leía como *"acá no hay
términos"* — el falso negativo exacto que ese lector existe para evitar, a una
clave de distancia.

Estaba en tres lugares, no en uno:

| dónde | qué rompía |
|---|---|
| SDK Python | lo arreglaron ustedes → **0.61.0** |
| SDK TypeScript | mismo hueco → **2.66.0** |
| `PaymentRequiredResponse` (Rust) | el desafío **no deserializaba**, así que `x402-reqwest` no podía pagarle a un vendedor v1 |
| chequeo de secuestro del Bazaar | un vendedor v1 era un recurso más donde el chequeo de seguridad no veía nada y se leía como "no hubo desvío" |

Ese último es el que más agradezco: se suma a la ceguera del transporte por
header que ya habíamos cerrado, y las dos juntas eran la misma clase de falla —
**un chequeo que no corrió se ve igual que uno que pasó**.

---

## 3. Del transporte por header, por si no lo vieron

Cerrado en 1.88.0, y vale decirlo porque los toca:

- Medimos 40 recursos vivos del Bazaar: **36 de 36 que responden 402 traen el
  desafío en el header `PAYMENT-REQUIRED`, ninguno en el body.**
- `x402-reqwest` leía sólo el body, así que **no podía pagarle a ninguno**.
- Vendedores como Tenjin usan el body de la 402 para la **vista previa gratis**
  del contenido pago: JSON válido, sin términos. Un lector body-only parsea
  contento y no encuentra nada.

Si construyen un comprador propio, los dos SDKs ahora exponen
`payment_challenge_from(headers, body)` / `paymentChallengeFrom(...)`: lee el
header primero, cae al body, y devuelve `None`/`null` cuando ningún transporte
trae un desafío. Ese `None` es distinguible a propósito de un `accepts` vacío.

---

## 4. Qué pueden probar

1. Actualizar a **0.61.0 / 2.66.0**.
2. Un anclaje normal: debe seguir igual, y ahora aterriza en Pinata.
3. `available_backends()`: deberían ver `s3` e `ipfs-private` en ON.
4. Recuperar una evidencia anclada hoy — verificando el CID del fragmento contra
   los bytes, si quieren cerrar el círculo.
5. **El que más nos sirve:** un anclaje con `proofOfPayment` real de un pago de
   EM en Base, a ver si llegan a `verified: true`. Sigue sin ejercitarse con
   tráfico real, y ahí es donde más probable es que quede algo mal — sobre todo
   los dos `Transfer` (el `amount` tiene que ser el **neto**).

---

## 5. Lo que sigue sin estar

- **Gate on-chain en Solana** — priorizado, sin hacer. Ahí el máximo sigue siendo
  `signed: true`.
- **`ipfs-public`** — apagado hasta que exista el opt-in del comprador.
- **Opt-in del comprador vía `accepts`** — diseñado, sin implementar.
- **`escrowed` / `POST /dx402/recover`** — 501 honesto.

---

## 6. Autocrítica

Tres cosas rompí prendiendo esto, y las tres las cazó la verificación y no la
lectura:

- La política IAM no incluía el secreto nuevo, porque CI aplica con `-target` sólo
  a task-definition y service. La tarea nueva no arrancaba (producción siguió
  sirviendo la anterior, sin caída).
- Un `sed` le puso el dominio de Pinata al default de `image_tag` — justo la
  variable cuya documentación cuenta que un valor parado ahí llevó producción dos
  meses atrás.
- **El primer anclaje real escribió evidencia ilegible**: la ruta `/dx402/blob`
  quedaba duplicada en el puntero, y un puntero privado no tenía cómo resolverse.

El e2e las encontró en el primer anclaje contra producción. Si no lo hubiéramos
corrido, KK habría sido quien las encontrara — con evidencia real adentro.
